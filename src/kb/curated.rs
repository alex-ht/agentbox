//! Built-in curated org list (`curated.toml`), verified 2026-11-11.
//!
//! Every URL was requested with curl. `unverified` means the check did not
//! return a readable page; the URL is still the official one we tried.

use super::db::Record;
use crate::cmd::fetch::normalize_url;
use crate::envelope::AppError;
use serde::Deserialize;
use std::collections::HashSet;

pub const SOURCE: &str = "curated:kb-v1";
pub const LICENSE: &str = "official-url-only";
pub const VERIFIED_ON: &str = "2026-11-11";

pub const LIFE_CATEGORIES: &[&str] = &[
    "portal",
    "household",
    "nhi",
    "tax",
    "motor",
    "labor",
    "welfare",
    "transport",
    "medical",
    "utilities",
    "post",
    "weather",
    "consumer",
    "finance",
    "food",
    "shopping",
    "housing",
    "education",
    "entertainment",
    "payment",
];

const ALLOWED_STATUS: &[&str] = &["verified", "connected", "blocked", "unverified"];

pub fn load_embedded() -> Result<Vec<Record>, AppError> {
    parse(include_str!("curated.toml"))
}

pub fn parse(text: &str) -> Result<Vec<Record>, AppError> {
    let file: CuratedFile = toml::from_str(text).map_err(|e| {
        AppError::new(
            "bad_kb",
            format!("curated TOML: {e}"),
            "Fix the TOML (see src/kb/curated.toml) and rerun `agentbox kb import --curated`.",
        )
    })?;
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(file.org.len());
    for org in file.org {
        out.push(to_record(org, &mut seen)?);
    }
    if out.is_empty() {
        return Err(AppError::new(
            "bad_kb",
            "curated TOML has no [[org]] entries",
            "Add at least one org, or run `agentbox kb import --curated` for the built-in list.",
        ));
    }
    Ok(out)
}

/// Categories used by the CrUX popular-sites layer, not by the hand-curated TOML.
pub const POPULAR_CATEGORIES: &[&str] = &["general", "media", "social", "search", "tech"];

pub fn known_category(category: &str) -> bool {
    LIFE_CATEGORIES.contains(&category)
        || POPULAR_CATEGORIES.contains(&category)
        || matches!(category, "online" | "counter" | "other" | "gov")
}

fn to_record(org: CuratedOrg, seen: &mut HashSet<String>) -> Result<Record, AppError> {
    if !org.id.starts_with("org:") || org.id.contains(char::is_whitespace) {
        return Err(bad(&org.id, "id must look like org:ris with no spaces"));
    }
    if !seen.insert(org.id.clone()) {
        return Err(bad(&org.id, "duplicate id"));
    }
    if org.name_zh.trim().is_empty() {
        return Err(bad(&org.id, "name_zh is empty"));
    }
    if !LIFE_CATEGORIES.contains(&org.category.as_str()) {
        return Err(bad(
            &org.id,
            &format!("unknown category `{}`", org.category),
        ));
    }
    if !ALLOWED_STATUS.contains(&org.verify_status.as_str()) {
        return Err(bad(
            &org.id,
            "verify_status must be verified, connected, blocked or unverified",
        ));
    }
    let official_url = normalize_url(org.official_url.trim())
        .map_err(|_| bad(&org.id, "official_url is not an http(s) URL"))?;
    if !official_url.starts_with("https://") {
        return Err(bad(&org.id, "curated URLs must be https"));
    }
    let mut rec = Record::blank(org.id, "org");
    rec.name_zh = org.name_zh.trim().to_string();
    rec.name_en = org.name_en.unwrap_or_default().trim().to_string();
    rec.aliases = org
        .aliases
        .into_iter()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect();
    rec.category = org.category;
    rec.official_url = official_url;
    rec.description = org.description.unwrap_or_default().trim().to_string();
    rec.source = SOURCE.to_string();
    rec.source_license = LICENSE.to_string();
    rec.last_verified = VERIFIED_ON.to_string();
    rec.verify_status = org.verify_status;
    Ok(rec)
}

fn bad(id: &str, message: &str) -> AppError {
    AppError::new(
        "bad_kb",
        format!("{id}: {message}"),
        "Fix the curated entry and rerun `agentbox kb import --curated`.",
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CuratedFile {
    org: Vec<CuratedOrg>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CuratedOrg {
    id: String,
    name_zh: String,
    #[serde(default)]
    name_en: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    category: String,
    official_url: String,
    #[serde(default)]
    description: Option<String>,
    verify_status: String,
}
