//! CrUX popular-sites import.
//!
//! Ranks are magnitude buckets, not positions. Origins are folded to a
//! registrable domain with the public suffix list. Adult, infrastructure,
//! and backend hostnames are dropped. See `popular_denylist.txt`.

use super::curated::known_category;
use super::db::{self, Record};
use crate::envelope::AppError;
use flate2::read::GzDecoder;
use regex::Regex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;

pub const CRUX_LICENSE: &str = "CC-BY-4.0";
pub const SEED_TW: &[u8] = include_bytes!("popular_tw.json.gz");
pub const SEED_GLOBAL: &[u8] = include_bytes!("popular_global.json.gz");

const NOISE_LABELS: &[&str] = &[
    "login",
    "signin",
    "signon",
    "sso",
    "auth",
    "oauth",
    "accounts",
    "account",
    "admin",
    "administrator",
    "api",
    "apis",
    "cdn",
    "static",
    "assets",
    "img",
    "images",
    "cart",
    "checkout",
    "basket",
    "webmail",
    "mail",
    "smtp",
    "ads",
    "ad",
    "analytics",
    "pixel",
    "track",
    "tracking",
    "telemetry",
    "metrics",
    "staging",
    "stage",
    "sandbox",
    "moodle",
    "eeclass",
    "elearn",
    "ilearn",
    "lms",
    "amp",
    "ns",
    "dns",
];

/// Public suffixes that host many unrelated sites. One CrUX origin there is
/// not "the" homepage of that suffix.
const SHARED_SUFFIXES: &[&str] = &["github.io", "blogspot.com", "blogspot.tw", "wordpress.com"];

const ADULT_PARTS: &[&str] = &[
    "porn", "xvideo", "xnxx", "xhamster", "hentai", "onlyfans", "pornhub", "redtube", "youporn",
    "missav", "nhentai", "hanime", "18comic", "18av", "18porn", "18jav", "avday", "7mmtv",
];

#[derive(Debug, Clone, Default)]
pub struct PrepareStats {
    pub input_rows: usize,
    pub kept_domains: usize,
    pub dropped_rank: usize,
    pub dropped_adult: usize,
    pub dropped_infra: usize,
    pub dropped_ip: usize,
    pub dropped_noise_only: usize,
    pub dropped_other: usize,
}

#[derive(Debug, Clone)]
struct Origin {
    url: String,
    host: String,
    https: bool,
    bucket: u32,
}

#[derive(Debug, Default, Deserialize)]
struct AliasFile {
    #[serde(default)]
    site: Vec<AliasSite>,
}

#[derive(Debug, Deserialize)]
struct AliasSite {
    domain: String,
    #[serde(default)]
    name_zh: String,
    #[serde(default)]
    name_en: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedFile {
    pub source: String,
    pub license: String,
    pub month: String,
    pub scope: String,
    pub records: Vec<SeedRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedRecord {
    pub id: String,
    pub name_zh: String,
    pub name_en: String,
    pub aliases: Vec<String>,
    pub category: String,
    pub official_url: String,
    pub alt_urls: Vec<String>,
    pub description: String,
    pub source: String,
    pub source_license: String,
    pub last_verified: String,
    pub verify_status: String,
    pub popularity_bucket: i64,
    pub popularity_scope: String,
}

pub fn prepare_csv(
    csv: &str,
    scope: &str,
    max_rank: u32,
    month: &str,
) -> (Vec<Record>, PrepareStats) {
    let mut stats = PrepareStats::default();
    let mut groups: HashMap<String, Vec<Origin>> = HashMap::new();
    for line in csv.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.to_ascii_lowercase().starts_with("origin,") {
            continue;
        }
        stats.input_rows += 1;
        let Some((origin, rank)) = line.split_once(',') else {
            stats.dropped_other += 1;
            continue;
        };
        let Ok(rank) = rank.trim().parse::<u32>() else {
            stats.dropped_other += 1;
            continue;
        };
        if rank == 0 || rank > max_rank {
            stats.dropped_rank += 1;
            continue;
        }
        let Some((https, host)) = split_origin(origin) else {
            stats.dropped_other += 1;
            continue;
        };
        if is_ip_host(&host) {
            stats.dropped_ip += 1;
            continue;
        }
        let Some(domain) = registrable(&host) else {
            stats.dropped_other += 1;
            continue;
        };
        if shared_suffix(&host) {
            stats.dropped_infra += 1;
            continue;
        }
        if is_adult_host(&host) || adult_domain(&domain) {
            stats.dropped_adult += 1;
            continue;
        }
        if is_infra_domain(&domain) || is_infra_domain(&host) {
            stats.dropped_infra += 1;
            continue;
        }
        groups.entry(domain).or_default().push(Origin {
            url: origin.trim().trim_end_matches('/').to_string(),
            host,
            https,
            bucket: rank,
        });
    }

    let mut records = Vec::new();
    for (domain, origins) in groups {
        let bucket = origins.iter().map(|o| o.bucket).min().unwrap_or(max_rank);
        let homepage = origins
            .iter()
            .filter(|o| is_apex_or_www(&o.host, &domain))
            .min_by_key(|o| homepage_rank(o, &domain));
        let useful: Vec<&Origin> = origins
            .iter()
            .filter(|o| !is_noise_host(&o.host, &domain))
            .collect();
        let (url, alts) = if let Some(home) = homepage {
            let alts = useful
                .iter()
                .filter(|o| o.host != home.host)
                .map(|o| o.url.clone())
                .take(6)
                .collect::<Vec<_>>();
            (home.url.clone(), alts)
        } else if useful.is_empty() {
            stats.dropped_noise_only += 1;
            continue;
        } else {
            let alts = useful
                .iter()
                .map(|o| o.url.clone())
                .take(6)
                .collect::<Vec<_>>();
            (format!("https://{domain}"), alts)
        };
        let mut rec = Record::blank(format!("pop:{domain}"), "org");
        rec.name_en = domain.clone();
        rec.official_url = if url.starts_with("http") {
            url
        } else {
            format!("https://{url}")
        };
        rec.alt_urls = alts;
        rec.category = guess_category(&domain, "", "");
        rec.source = format!("crux:{scope}:{month}");
        rec.source_license = CRUX_LICENSE.to_string();
        rec.verify_status = "unverified".to_string();
        rec.popularity_bucket = Some(i64::from(bucket));
        rec.popularity_scope = scope.to_string();
        rec.description =
            format!("Chrome UX Report {scope} {month}, popularity bucket {bucket}. CC BY 4.0.");
        records.push(rec);
    }
    apply_overlay(&mut records);
    records.sort_by(|a, b| a.id.cmp(&b.id));
    stats.kept_domains = records.len();
    (records, stats)
}

pub fn read_crux_file(path: &Path) -> Result<String, AppError> {
    let bytes =
        std::fs::read(path).map_err(|e| AppError::io(&format!("read {}", path.display()), e))?;
    let gz =
        path.extension().and_then(|e| e.to_str()) == Some("gz") || bytes.starts_with(&[0x1f, 0x8b]);
    if gz {
        let mut dec = GzDecoder::new(&bytes[..]);
        let mut s = String::new();
        dec.read_to_string(&mut s).map_err(|e| {
            AppError::new(
                "bad_kb",
                format!("gzip {}: {e}", path.display()),
                "Pass a CrUX csv.gz from zakird/crux-top-lists, for example data/country/tw/202610.csv.gz.",
            )
        })?;
        Ok(s)
    } else {
        String::from_utf8(bytes).map_err(|_| {
            AppError::new(
                "bad_kb",
                format!("{} is not UTF-8", path.display()),
                "CrUX files are UTF-8 CSV, optionally gzipped.",
            )
        })
    }
}

pub fn month_from_path(path: &str) -> String {
    let digits: String = path.chars().filter(|c| c.is_ascii_digit()).collect();
    if let Some(idx) = digits.find("202") {
        let rest = &digits[idx..];
        if rest.len() >= 6 {
            return rest[..6].to_string();
        }
    }
    "unknown".to_string()
}

/// HEAD/GET the homepage. Not used by unit tests.
pub fn verify_records(records: &mut [Record]) {
    const WORKERS: usize = 16;
    let mut slices: Vec<&mut [Record]> = Vec::new();
    let chunk = records.len().div_ceil(WORKERS).max(1);
    let mut rest = records;
    while !rest.is_empty() {
        let n = chunk.min(rest.len());
        let (head, tail) = rest.split_at_mut(n);
        slices.push(head);
        rest = tail;
    }
    std::thread::scope(|scope| {
        for slice in slices {
            scope.spawn(|| {
                let client = match reqwest::blocking::Client::builder()
                    .user_agent(concat!(
                        "agentbox/",
                        env!("CARGO_PKG_VERSION"),
                        " crux-seed"
                    ))
                    .timeout(std::time::Duration::from_secs(8))
                    .redirect(reqwest::redirect::Policy::limited(5))
                    .build()
                {
                    Ok(c) => c,
                    Err(_) => return,
                };
                for rec in slice.iter_mut() {
                    let (status, title) = probe(&client, &rec.official_url);
                    rec.verify_status = status.to_string();
                    rec.last_verified = chrono::Utc::now().format("%Y-%m-%d").to_string();
                    apply_title(rec, &title);
                }
            });
        }
    });
}

/// Best-effort Wikidata P856 lookup. Failures leave titles/overlays in place.
pub fn enrich_wikidata(records: &mut [Record]) -> usize {
    let domains: Vec<String> = records
        .iter()
        .filter_map(|r| r.id.strip_prefix("pop:").map(|s| s.to_string()))
        .collect();
    let mut hits: HashMap<String, WikiHit> = HashMap::new();
    for chunk in domains.chunks(20) {
        if let Some(map) = wikidata_batch(chunk) {
            hits.extend(map);
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let mut n = 0;
    for rec in records {
        let Some(domain) = rec.id.strip_prefix("pop:") else {
            continue;
        };
        let Some(hit) = hits.get(domain) else {
            continue;
        };
        if rec.name_zh.is_empty() {
            if let Some(name) = hit.zh.clone() {
                rec.name_zh = name;
            }
        }
        if rec.name_en.is_empty() || rec.name_en == domain {
            if let Some(name) = hit.en.clone() {
                rec.name_en = name;
            }
        }
        for alias in &hit.aliases {
            push_alias(&mut rec.aliases, alias);
        }
        if rec.category == "general" {
            if let Some(cat) = hit.category.clone() {
                rec.category = cat;
            }
        }
        if !rec.description.contains("Wikidata") {
            rec.description
                .push_str(" Names from Wikidata (CC0) when present.");
        }
        n += 1;
    }
    n
}

pub fn records_to_seed(records: &[Record], scope: &str, month: &str) -> SeedFile {
    SeedFile {
        source: format!("crux:{scope}:{month}"),
        license: CRUX_LICENSE.to_string(),
        month: month.to_string(),
        scope: scope.to_string(),
        records: records.iter().map(SeedRecord::from_record).collect(),
    }
}

pub fn seed_from_bytes(bytes: &[u8]) -> Result<SeedFile, AppError> {
    let mut dec = GzDecoder::new(bytes);
    let mut s = String::new();
    dec.read_to_string(&mut s).map_err(|e| {
        AppError::new(
            "bad_kb",
            format!("popular seed gzip: {e}"),
            "Rebuild src/kb/popular_tw.json.gz.",
        )
    })?;
    serde_json::from_str(&s).map_err(|e| {
        AppError::new(
            "bad_kb",
            format!("popular seed json: {e}"),
            "Rebuild the popular seed.",
        )
    })
}

pub fn load_bundled() -> Result<Vec<Record>, AppError> {
    let mut out = Vec::new();
    for bytes in [SEED_TW, SEED_GLOBAL] {
        if bytes.is_empty() {
            continue;
        }
        let seed = seed_from_bytes(bytes)?;
        out.extend(seed.records.into_iter().map(|r| r.into_record()));
    }
    Ok(out)
}

/// Insert popular rows. A curated (or earlier) row with the same registrable
/// domain keeps its URL and name; aliases and the CrUX bucket are attached.
pub fn merge_records(conn: &Connection, incoming: &[Record]) -> Result<(usize, usize), AppError> {
    let incoming = dedupe_same_id(incoming);
    let existing = db::list_merge_rows(conn)?;
    let mut by_domain: HashMap<String, usize> = HashMap::new();
    for (i, row) in existing.iter().enumerate() {
        if let Some(domain) = host_of(&row.official_url).and_then(|h| registrable(&h)) {
            by_domain.entry(domain).or_insert(i);
        }
    }
    let mut inserted = 0;
    let mut merged = 0;
    for rec in incoming {
        let Some(domain) = host_of(&rec.official_url).and_then(|h| registrable(&h)) else {
            continue;
        };
        if let Some(&idx) = by_domain.get(&domain) {
            let row = &existing[idx];
            let mut aliases = row.aliases.clone();
            for a in rec
                .aliases
                .iter()
                .chain(std::iter::once(&rec.name_zh))
                .chain(std::iter::once(&rec.name_en))
            {
                push_alias(&mut aliases, a);
            }
            let mut alts = row.alt_urls.clone();
            for u in &rec.alt_urls {
                if alts.len() < 8 && !alts.contains(u) {
                    alts.push(u.clone());
                }
            }
            let scope = if row.popularity_scope.is_empty() {
                rec.popularity_scope.as_str()
            } else {
                row.popularity_scope.as_str()
            };
            db::attach_popularity(conn, &row.id, &aliases, &alts, rec.popularity_bucket, scope)?;
            merged += 1;
        } else {
            db::upsert(conn, &rec)?;
            inserted += 1;
        }
    }
    Ok((inserted, merged))
}

fn dedupe_same_id(incoming: &[Record]) -> Vec<Record> {
    let mut map: HashMap<String, Record> = HashMap::new();
    for rec in incoming {
        if let Some(prev) = map.get_mut(&rec.id) {
            let tw = prev.popularity_scope == "tw" || rec.popularity_scope == "tw";
            let bucket = match (prev.popularity_bucket, rec.popularity_bucket) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            if prev.name_zh.is_empty() {
                prev.name_zh = rec.name_zh.clone();
            }
            if prev.name_en.is_empty() || prev.name_en == rec.id.trim_start_matches("pop:") {
                prev.name_en = rec.name_en.clone();
            }
            for a in &rec.aliases {
                push_alias(&mut prev.aliases, a);
            }
            for u in &rec.alt_urls {
                if prev.alt_urls.len() < 8 && !prev.alt_urls.contains(u) {
                    prev.alt_urls.push(u.clone());
                }
            }
            prev.popularity_bucket = bucket;
            if tw {
                prev.popularity_scope = "tw".to_string();
            }
        } else {
            map.insert(rec.id.clone(), rec.clone());
        }
    }
    let mut out: Vec<Record> = map.into_values().collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn apply_overlay(records: &mut [Record]) {
    let file: AliasFile = toml::from_str(include_str!("popular_aliases.toml")).unwrap_or_default();
    let map: HashMap<String, &AliasSite> =
        file.site.iter().map(|s| (s.domain.clone(), s)).collect();
    for rec in records {
        let Some(domain) = rec.id.strip_prefix("pop:") else {
            continue;
        };
        let Some(site) = map.get(domain) else {
            continue;
        };
        if !site.name_zh.is_empty() {
            rec.name_zh = site.name_zh.clone();
        }
        if !site.name_en.is_empty() {
            rec.name_en = site.name_en.clone();
        }
        for a in &site.aliases {
            push_alias(&mut rec.aliases, a);
        }
        if known_category(&site.category) {
            rec.category = site.category.clone();
        }
    }
}

fn apply_title(rec: &mut Record, title: &str) {
    let lower = title.to_ascii_lowercase();
    if lower.contains("just a moment")
        || lower.contains("attention required")
        || lower.contains("access denied")
        || lower.contains("human verification")
        || lower.trim() == "403"
    {
        return;
    }
    let name = title_name(title);
    if name.chars().count() < 2 {
        return;
    }
    let domain = rec.id.strip_prefix("pop:").unwrap_or("");
    let cjk = name.chars().any(|c| ('\u{3400}'..='\u{9fff}').contains(&c));
    if cjk && rec.name_zh.is_empty() {
        rec.name_zh = name;
    } else if !cjk && (rec.name_en.is_empty() || rec.name_en == domain) {
        rec.name_en = name;
    }
    if rec.category == "general" {
        rec.category = guess_category(domain, &rec.name_zh, &rec.name_en);
    }
}

fn title_name(title: &str) -> String {
    let t = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut = t
        .split(['|', '-', '–', '—', '｜'])
        .next()
        .unwrap_or(&t)
        .trim();
    let cut = if cut.chars().count() >= 2 && cut.chars().count() <= 40 {
        cut
    } else {
        t.trim()
    };
    cut.chars().take(40).collect()
}

fn probe(client: &reqwest::blocking::Client, url: &str) -> (&'static str, String) {
    let resp = match client.get(url).send() {
        Ok(r) => r,
        Err(_) => return ("unverified", String::new()),
    };
    let code = resp.status().as_u16();
    let body = resp.text().unwrap_or_default();
    let title = page_title(&body);
    let challenge = {
        let l = title.to_ascii_lowercase();
        l.contains("just a moment")
            || l.contains("attention required")
            || l.contains("access denied")
            || l == "403"
    };
    let status = if matches!(code, 401 | 403 | 429) || challenge {
        "blocked"
    } else if (200..300).contains(&code) {
        if title.trim().is_empty() {
            "connected"
        } else {
            "verified"
        }
    } else {
        "unverified"
    };
    (status, title)
}

fn page_title(body: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());
    let Some(c) = re.captures(body) else {
        return String::new();
    };
    let raw = c.get(1).map(|m| m.as_str()).unwrap_or("");
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct WikiHit {
    zh: Option<String>,
    en: Option<String>,
    aliases: Vec<String>,
    category: Option<String>,
}

fn wikidata_batch(domains: &[String]) -> Option<HashMap<String, WikiHit>> {
    let mut iris = Vec::new();
    for d in domains {
        for url in [
            format!("https://{d}"),
            format!("https://{d}/"),
            format!("https://www.{d}"),
            format!("https://www.{d}/"),
            format!("http://{d}"),
            format!("http://www.{d}"),
        ] {
            iris.push(format!("<{url}>"));
        }
    }
    let values = iris.join(" ");
    let query = format!(
        r#"SELECT ?official ?zhTw ?zhHant ?zh ?en ?alt ?type WHERE {{
  VALUES ?official {{ {values} }}
  ?item wdt:P856 ?official .
  OPTIONAL {{ ?item rdfs:label ?zhTw FILTER(LANG(?zhTw)="zh-tw") }}
  OPTIONAL {{ ?item rdfs:label ?zhHant FILTER(LANG(?zhHant)="zh-hant") }}
  OPTIONAL {{ ?item rdfs:label ?zh FILTER(LANG(?zh)="zh") }}
  OPTIONAL {{ ?item rdfs:label ?en FILTER(LANG(?en)="en") }}
  OPTIONAL {{ ?item skos:altLabel ?alt FILTER(LANG(?alt)="zh-tw" || LANG(?alt)="zh") }}
  OPTIONAL {{ ?item wdt:P31 ?type }}
}}"#
    );
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!(
            "agentbox/",
            env!("CARGO_PKG_VERSION"),
            " (https://github.com/alex-ht/agentbox)"
        ))
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .ok()?;
    let resp = client
        .post("https://query.wikidata.org/sparql")
        .header("Accept", "application/sparql-results+json")
        .form(&[("query", query.as_str())])
        .send()
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let text = resp.text().ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let bindings = value["results"]["bindings"].as_array()?;
    let mut out: HashMap<String, WikiHit> = HashMap::new();
    for row in bindings {
        let official = binding_str(row, "official").unwrap_or_default();
        let Some(host) = host_of(&official) else {
            continue;
        };
        let Some(domain) = registrable(&host) else {
            continue;
        };
        if !domains.iter().any(|d| d == &domain) {
            continue;
        }
        let hit = out.entry(domain).or_insert(WikiHit {
            zh: None,
            en: None,
            aliases: Vec::new(),
            category: None,
        });
        if hit.zh.is_none() {
            hit.zh = binding_str(row, "zhTw")
                .or_else(|| binding_str(row, "zhHant"))
                .or_else(|| binding_str(row, "zh"));
        }
        if hit.en.is_none() {
            hit.en = binding_str(row, "en");
        }
        if let Some(alias) = binding_str(row, "alt") {
            if alias.chars().count() <= 40 {
                push_alias(&mut hit.aliases, &alias);
            }
        }
        if hit.category.is_none() {
            hit.category = binding_str(row, "type")
                .as_deref()
                .and_then(category_from_qid);
        }
    }
    Some(out)
}

fn binding_str(row: &serde_json::Value, key: &str) -> Option<String> {
    row.get(key)?
        .get("value")?
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn category_from_qid(qid_url: &str) -> Option<String> {
    let qid = qid_url.rsplit('/').next().unwrap_or("");
    let cat = match qid {
        "Q11032" | "Q1193236" | "Q1153191" | "Q1110794" => "media",
        "Q3220391" | "Q1668024" | "Q8700794" => "social",
        "Q7397" | "Q35127" => "general",
        _ => return None,
    };
    Some(cat.to_string())
}

fn guess_category(domain: &str, name_zh: &str, name_en: &str) -> String {
    let host = domain.to_ascii_lowercase();
    let blob = format!(
        "{host} {} {}",
        name_zh.to_ascii_lowercase(),
        name_en.to_ascii_lowercase()
    );
    const SEARCH: &[&str] = &[
        "google.",
        "bing.com",
        "duckduckgo.com",
        "baidu.com",
        "yahoo.com",
    ];
    const SOCIAL: &[&str] = &[
        "facebook.com",
        "instagram.com",
        "twitter.com",
        "x.com",
        "tiktok.com",
        "line.me",
        "dcard.tw",
        "ptt.cc",
        "reddit.com",
        "linkedin.com",
        "threads.net",
        "weibo.com",
    ];
    const TECH: &[&str] = &[
        "github.com",
        "gitlab.com",
        "stackoverflow.com",
        "npmjs.com",
        "pypi.org",
    ];
    const MEDIA: &[&str] = &["wikipedia.org", "wikimedia.org"];
    if SEARCH
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    {
        return "search".into();
    }
    if SOCIAL
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    {
        return "social".into();
    }
    if TECH
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    {
        return "tech".into();
    }
    if MEDIA
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    {
        return "media".into();
    }
    if blob.contains("新聞") || blob.contains("news") || name_zh.contains('報') {
        return "media".into();
    }
    "general".into()
}

fn push_alias(aliases: &mut Vec<String>, alias: &str) {
    let alias = alias.trim();
    if alias.is_empty() || aliases.iter().any(|a| a == alias) || aliases.len() >= 12 {
        return;
    }
    aliases.push(alias.to_string());
}

fn split_origin(origin: &str) -> Option<(bool, String)> {
    let origin = origin.trim();
    let (scheme, rest) = origin.split_once("://")?;
    let https = scheme.eq_ignore_ascii_case("https");
    if !https && !scheme.eq_ignore_ascii_case("http") {
        return None;
    }
    let host = rest.split(['/', '?', '#']).next()?.split('@').next_back()?;
    if host.starts_with('[') {
        return None;
    }
    let host = host.split(':').next()?;
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host.contains(' ') {
        return None;
    }
    Some((https, host))
}

fn is_ip_host(host: &str) -> bool {
    if host.starts_with('[') {
        return true;
    }
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| p.parse::<u8>().is_ok() && !p.is_empty())
}

pub fn registrable(host: &str) -> Option<String> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    psl::domain_str(&host).map(|s| s.to_string())
}

fn shared_suffix(host: &str) -> bool {
    psl::suffix_str(host).is_some_and(|s| SHARED_SUFFIXES.iter().any(|x| s.eq_ignore_ascii_case(x)))
}

fn adult_text(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    const PARTS: &[&str] = &[
        "a片",
        "色情",
        "無碼",
        "无码",
        "情色",
        "黃片",
        "黄片",
        "成人影片",
        "成人视频",
        "成人視頻",
        "成人卡通",
        "成人短剧",
        "成人短劇",
        "成人漫畫",
        "成人漫画",
        "免费a片",
        "免費a片",
        "吃瓜",
        "porn",
        "hentai",
        "xxx",
        "jav ",
    ];
    PARTS.iter().any(|p| l.contains(p))
}

/// Drop rows whose homepage title is adult. Host filters cannot see this.
pub fn drop_adult_text(records: &mut Vec<Record>) -> usize {
    let before = records.len();
    records.retain(|r| {
        !adult_text(&r.name_zh)
            && !adult_text(&r.name_en)
            && !adult_domain(r.id.trim_start_matches("pop:"))
    });
    before - records.len()
}

fn adult_domain(domain: &str) -> bool {
    is_adult_host(domain) || domain.split(['.', '-']).any(adult_av_label)
}

fn adult_av_label(label: &str) -> bool {
    let l = label.to_ascii_lowercase();
    if matches!(
        l.as_str(),
        "avatar" | "aviation" | "avenue" | "avaya" | "available"
    ) {
        return false;
    }
    if l.contains("jav") || l.contains("chigua") || l.contains("hcomic") {
        return true;
    }
    let digits = l.chars().any(|c| c.is_ascii_digit());
    (l.starts_with("av") && l.len() <= 8) || (l.ends_with("av") && digits)
}

fn is_adult_host(host: &str) -> bool {
    host.split(['.', '-'])
        .filter(|l| !l.is_empty())
        .any(|label| {
            let l = label.to_ascii_lowercase();
            l == "sex" || l == "xxx" || l == "porn" || ADULT_PARTS.iter().any(|p| l.contains(p))
        })
}

fn denylist() -> &'static HashSet<String> {
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| {
        include_str!("popular_denylist.txt")
            .lines()
            .map(|l| {
                l.split('#')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
            })
            .filter(|l| !l.is_empty())
            .collect()
    })
}

fn is_infra_domain(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    denylist()
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

fn is_apex_or_www(host: &str, domain: &str) -> bool {
    host == domain || host == format!("www.{domain}")
}

fn is_noise_host(host: &str, domain: &str) -> bool {
    if is_apex_or_www(host, domain) {
        return false;
    }
    let prefix = host.strip_suffix(&format!(".{domain}")).unwrap_or(host);
    prefix.split('.').any(|label| NOISE_LABELS.contains(&label))
}

fn homepage_rank(origin: &Origin, domain: &str) -> u8 {
    let www = u8::from(origin.host != format!("www.{domain}"));
    let https = u8::from(!origin.https);
    www * 2 + https
}

fn host_of(url: &str) -> Option<String> {
    split_origin(url).map(|(_, h)| h).or_else(|| {
        let url = url.trim();
        if url.contains("://") {
            None
        } else {
            Some(url.trim_end_matches('/').to_ascii_lowercase())
        }
    })
}

impl SeedRecord {
    fn from_record(rec: &Record) -> Self {
        Self {
            id: rec.id.clone(),
            name_zh: rec.name_zh.clone(),
            name_en: rec.name_en.clone(),
            aliases: rec.aliases.clone(),
            category: rec.category.clone(),
            official_url: rec.official_url.clone(),
            alt_urls: rec.alt_urls.clone(),
            description: rec.description.clone(),
            source: rec.source.clone(),
            source_license: rec.source_license.clone(),
            last_verified: rec.last_verified.clone(),
            verify_status: rec.verify_status.clone(),
            popularity_bucket: rec.popularity_bucket.unwrap_or(0),
            popularity_scope: rec.popularity_scope.clone(),
        }
    }

    fn into_record(self) -> Record {
        let mut rec = Record::blank(self.id, "org");
        rec.name_zh = self.name_zh;
        rec.name_en = self.name_en;
        rec.aliases = self.aliases;
        rec.category = self.category;
        rec.official_url = self.official_url;
        rec.alt_urls = self.alt_urls;
        rec.description = self.description;
        rec.source = self.source;
        rec.source_license = self.source_license;
        rec.last_verified = self.last_verified;
        rec.verify_status = self.verify_status;
        rec.popularity_bucket = Some(self.popularity_bucket);
        rec.popularity_scope = self.popularity_scope;
        rec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\
origin,rank
https://www.youtube.com,1000
https://m.youtube.com,1000
https://accounts.google.com,1000
https://www.google.com,1000
https://18porn.cc,1000
https://123av.com,1000
https://52av.one,1000
https://xvideos.com,5000
https://www.gstatic.com,1000
https://www.example.com,5000
https://cdn.example.com,1000
https://irs.thsrc.com.tw,1000
https://www.thsrc.com.tw,1000
https://203.0.113.8,1000
https://login.onlybank.test,1000
https://moodle.school.edu.tw,5000
https://www.school.edu.tw,5000
https://foo.github.io,1000
https://not-a-url,1000
https://www.late.example,10000
";

    #[test]
    fn folds_and_drops_adult_infra_and_backends() {
        let (recs, stats) = prepare_csv(FIXTURE, "tw", 5000, "202610");
        let ids: Vec<&str> = recs.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"pop:youtube.com"), "{ids:?}");
        assert!(ids.contains(&"pop:google.com"), "{ids:?}");
        assert!(ids.contains(&"pop:example.com"), "{ids:?}");
        assert!(ids.contains(&"pop:thsrc.com.tw"), "{ids:?}");
        assert!(ids.contains(&"pop:school.edu.tw"), "{ids:?}");
        assert!(!ids.iter().any(|id| {
            id.contains("porn")
                || id.contains("xvideos")
                || id.contains("gstatic")
                || id.contains("123av")
                || id.contains("52av")
        }));
        assert!(!ids
            .iter()
            .any(|id| id.contains("onlybank") || id.contains("github.io")));
        assert!(stats.dropped_adult >= 2, "{stats:?}");
        assert!(stats.dropped_ip >= 1, "{stats:?}");
        assert!(stats.dropped_rank >= 1, "{stats:?}");
        let yt = recs.iter().find(|r| r.id == "pop:youtube.com").unwrap();
        assert_eq!(yt.name_en, "YouTube");
        assert!(yt.alt_urls.iter().any(|u| u.contains("m.youtube.com")));
        assert_eq!(yt.popularity_bucket, Some(1000));
        let thsrc = recs.iter().find(|r| r.id == "pop:thsrc.com.tw").unwrap();
        assert!(
            thsrc.official_url.contains("www.thsrc.com.tw"),
            "{}",
            thsrc.official_url
        );
        assert!(thsrc
            .alt_urls
            .iter()
            .any(|u| u.contains("irs.thsrc.com.tw")));
        let school = recs.iter().find(|r| r.id == "pop:school.edu.tw").unwrap();
        assert!(school.official_url.contains("www.school.edu.tw"));
        assert!(school.alt_urls.iter().all(|u| !u.contains("moodle")));
    }

    #[test]
    fn registrable_domains_keep_tw_suffix() {
        assert_eq!(
            registrable("www.books.com.tw").as_deref(),
            Some("books.com.tw")
        );
        assert_eq!(
            registrable("irs.thsrc.com.tw").as_deref(),
            Some("thsrc.com.tw")
        );
        assert_eq!(
            registrable("www.youtube.com").as_deref(),
            Some("youtube.com")
        );
    }
}
