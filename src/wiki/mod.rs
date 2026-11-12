//! `wiki search` and `wiki open`: Wikipedia via the MediaWiki Action API.
//!
//! Tool order for facts is `kb` (Taiwan official sites), then `wiki`, then
//! web `search` (DuckDuckGo). Article text is stored as a `doc:` handle; the
//! command returns an outline, not the whole page.

#[cfg(test)]
mod tests;

use crate::cmd::fetch;
use crate::cmd::search::normalize_url;
use crate::envelope::{AppError, CmdResult, Output};
use crate::kb::shell_quote;
use crate::net;
use crate::state::{DocMeta, Store};
use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

const REPO: &str = "https://github.com/alex-ht/agentbox";
/// Keep in sync with `reqwest` in Cargo.lock. A unit test checks the lockfile.
const REQWEST_VERSION: &str = "0.12.28";

const LIMIT_MAX: usize = 20;
const QUERY_MAX: usize = 300;
const TITLE_MAX: usize = 300;
const SNIPPET_CHARS: usize = 240;
const MAX_ATTEMPTS: u32 = 3;
const MAX_WAIT_SECS: u64 = 10;
const MAX_BODY: u64 = 8 * 1024 * 1024;
const SEARCH_TTL_HOURS: i64 = 24;
const PAGE_TTL_DAYS: i64 = 7;
const CANDIDATES_MAX: usize = 8;

/// Wikimedia User-Agent. `contact` is an optional email or URL; the repo URL
/// is always present and a personal address is never built in.
pub fn user_agent(contact: Option<&str>) -> String {
    let paren = match clean_contact(contact) {
        Some(c) => format!("{REPO}; {c}"),
        None => REPO.to_string(),
    };
    format!(
        "agentbox/{} ({paren}) reqwest/{REQWEST_VERSION}",
        env!("CARGO_PKG_VERSION")
    )
}

fn clean_contact(contact: Option<&str>) -> Option<String> {
    let raw = contact?.trim();
    if raw.is_empty() {
        return None;
    }
    let s: String = raw
        .chars()
        .filter(|c| (c.is_ascii_graphic() || *c == ' ') && *c != '(' && *c != ')')
        .take(180)
        .collect();
    let s = s.trim().trim_matches(';').trim().to_string();
    if s.is_empty() || s == REPO {
        None
    } else {
        Some(s)
    }
}

/// API base override (`AGENTBOX_WIKI_API`) for tests. Empty means the public
/// `https://<lang>.wikipedia.org/w/api.php` host.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub api: Option<String>,
}

impl Endpoints {
    pub fn from_env() -> Self {
        Self {
            api: std::env::var("AGENTBOX_WIKI_API")
                .ok()
                .map(|s| s.trim().trim_end_matches('/').to_string())
                .filter(|s| !s.is_empty()),
        }
    }
}

pub struct Ctx<'a> {
    pub store: &'a Store,
    pub endpoints: &'a Endpoints,
    pub contact: Option<&'a str>,
    pub timeout: u64,
}

pub struct SearchArgs {
    pub query: String,
    pub lang: String,
    pub limit: usize,
}

pub struct OpenArgs {
    pub title: String,
    pub lang: String,
}

struct Edition {
    /// `zh.wikipedia.org` site code.
    site: String,
    /// Reported language (`zh`, `en`, …).
    lang: String,
    /// `zh-tw` when the caller asked for Chinese.
    variant: Option<String>,
}

pub fn run_search(ctx: &Ctx, args: &SearchArgs) -> CmdResult {
    let query = args.query.trim();
    if query.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "query is empty",
            "Example: `agentbox wiki search 台北101`.",
        ));
    }
    if query.chars().count() > QUERY_MAX {
        return Err(AppError::new(
            "bad_args",
            "query is too long",
            "Use a short name or phrase, e.g. `agentbox wiki search 全民健康保險`.",
        ));
    }
    if args.limit == 0 || args.limit > LIMIT_MAX {
        return Err(AppError::new(
            "bad_args",
            format!("limit must be 1..={LIMIT_MAX}"),
            "Example: `agentbox wiki search 台北101 --limit 5`.",
        ));
    }
    let edition = edition(&args.lang)?;
    let url = search_url(ctx.endpoints, &edition, query, args.limit);
    let (body, from_cache) = fetch_json(ctx, &url, &edition)?;
    let parsed: Value = serde_json::from_str(&body).map_err(|e| bad_json(e.to_string()))?;
    let total = parsed
        .pointer("/query/searchinfo/totalhits")
        .and_then(Value::as_u64);
    let rows = parsed
        .pointer("/query/search")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut hits = Vec::new();
    for row in rows {
        let Some(title) = row.get("title").and_then(Value::as_str) else {
            continue;
        };
        let title = title.trim();
        if title.is_empty() {
            continue;
        }
        let snippet = short_text(&strip_html(
            row.get("snippet").and_then(Value::as_str).unwrap_or(""),
        ));
        hits.push(json!({
            "title": title,
            "snippet": snippet,
            "url": article_url(&edition.site, title),
        }));
        if hits.len() >= args.limit {
            break;
        }
    }
    if hits.is_empty() {
        return Err(search_not_found(query, &edition));
    }
    let top = hits[0]["title"].as_str().unwrap_or(query);
    let hint = format!(
        "Open the top hit with `agentbox wiki open {} --lang {}`, then read it with `agentbox read doc:N --grep KEYWORD`. Search `kb` first for a Taiwan official site. Use `agentbox search` (DuckDuckGo) only if Wikipedia has nothing. Cite these URLs; do not invent one.",
        shell_quote(top),
        edition.lang
    );
    let mut data = json!({
        "query": query,
        "lang": edition.lang,
        "variant": edition.variant,
        "from_cache": from_cache,
        "hits": hits,
    });
    if let Some(n) = total {
        data["total"] = json!(n);
    }
    Ok(Output::new(data).hint(hint))
}

pub fn run_open(ctx: &Ctx, args: &OpenArgs) -> CmdResult {
    let target = parse_target(&args.title)?;
    let lang = if let Some(from_url) = &target.lang_from_url {
        from_url.clone()
    } else {
        args.lang.clone()
    };
    let url_overrode = target
        .lang_from_url
        .as_ref()
        .is_some_and(|u| !u.eq_ignore_ascii_case(args.lang.trim()));
    let edition = edition(&lang)?;
    let title = normalize_title(&target.title);
    if title.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "title is empty",
            "Example: `agentbox wiki open 台北101` or `agentbox wiki open https://en.wikipedia.org/wiki/Albert_Einstein`.",
        ));
    }
    if title.chars().count() > TITLE_MAX {
        return Err(AppError::new(
            "bad_args",
            "title is too long",
            "Pass a Wikipedia article title, not a paragraph.",
        ));
    }
    let url = open_url(ctx.endpoints, &edition, &title);
    let (body, from_cache) = fetch_json(ctx, &url, &edition)?;
    let parsed: Value = serde_json::from_str(&body).map_err(|e| bad_json(e.to_string()))?;
    let page = first_page(&parsed).ok_or_else(|| {
        AppError::new(
            "bad_response",
            "Wikipedia returned no page",
            "Retry the same `wiki open`. If it persists, `wiki search` the title.",
        )
    })?;
    let final_title = page
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or(&title)
        .trim()
        .to_string();
    let canonical = page
        .get("canonicalurl")
        .and_then(Value::as_str)
        .or_else(|| page.get("fullurl").and_then(Value::as_str))
        .map(str::trim)
        .filter(|s| s.starts_with("http://") || s.starts_with("https://"))
        .map(str::to_string)
        .unwrap_or_else(|| article_url(&edition.site, &final_title));
    let redirected_from = redirect_from(&parsed, &final_title);
    if is_missing(&page) {
        return Err(missing_page(&title, &edition, redirected_from.as_deref()));
    }
    if is_disambiguation(&page) {
        let extract = page.get("extract").and_then(Value::as_str).unwrap_or("");
        return Err(disambiguation(
            &final_title,
            &canonical,
            &edition,
            extract,
            redirected_from.as_deref(),
        ));
    }
    let extract = page.get("extract").and_then(Value::as_str).unwrap_or("");
    if extract.trim().is_empty() {
        return Err(AppError::new(
            "no_content",
            format!("Wikipedia page `{final_title}` has no readable text"),
            format!(
                "Search nearby titles with `agentbox wiki search {} --lang {}`.",
                shell_quote(&final_title),
                edition.lang
            ),
        ));
    }
    let markdown = wiki_to_markdown(&final_title, extract);
    let (id, reused) = store_article(ctx.store, &canonical, &final_title, &markdown)?;
    let mut out = fetch::summarize(id, &final_title, &canonical, &markdown);
    out.data["lang"] = json!(edition.lang);
    out.data["variant"] = json!(edition.variant);
    out.data["from_cache"] = json!(from_cache || reused);
    if let Some(from) = &redirected_from {
        out.data["redirected_from"] = json!(from);
    }
    if let Some(hint) = out.hint.as_mut() {
        hint.push_str(" Cite `data.url` from this result; do not invent another URL.");
        if from_cache || reused {
            hint.push_str(" Reused a cached Wikipedia copy.");
        }
        if url_overrode {
            hint.push_str(&format!(
                " Language `{lang}` comes from the article URL (--lang {} was not used).",
                args.lang.trim()
            ));
        }
        if redirected_from.is_some() {
            hint.push_str(" The title redirected; the doc is the target article.");
        }
    }
    Ok(out)
}

fn search_not_found(query: &str, edition: &Edition) -> AppError {
    let other = if edition.lang == "en" { "zh" } else { "en" };
    AppError::new(
        "not_found",
        format!("no {0} Wikipedia hit for {1}", edition.lang, query),
        format!(
            "No {lang} Wikipedia hit for {q}. Try `agentbox wiki search {q} --lang {other}`, or `agentbox search {q}` (DuckDuckGo) as a last resort. Do not invent facts.",
            lang = edition.lang,
            q = shell_quote(query),
            other = other
        ),
    )
}

fn missing_page(title: &str, edition: &Edition, redirected_from: Option<&str>) -> AppError {
    let mut err = AppError::new(
        "not_found",
        format!("no {0} Wikipedia page titled {1}", edition.lang, title),
        format!(
            "No page titled {q} on {lang} Wikipedia. Search with `agentbox wiki search {q} --lang {lang}`.",
            q = shell_quote(title),
            lang = edition.lang
        ),
    );
    if let Some(from) = redirected_from {
        err = err.with_details(json!({ "redirected_from": from, "lang": edition.lang }));
    }
    err
}

fn disambiguation(
    title: &str,
    url: &str,
    edition: &Edition,
    extract: &str,
    redirected_from: Option<&str>,
) -> AppError {
    let candidates = candidate_titles(extract);
    let shown: Vec<Value> = candidates
        .iter()
        .take(CANDIDATES_MAX)
        .map(|t| {
            json!({
                "title": t,
                "url": article_url(&edition.site, t),
            })
        })
        .collect();
    let example = candidates
        .first()
        .map(|t| {
            format!(
                "Open one, for example `agentbox wiki open {} --lang {}`.",
                shell_quote(t),
                edition.lang
            )
        })
        .unwrap_or_else(|| {
            format!(
                "Narrow it with `agentbox wiki search {} --lang {}`.",
                shell_quote(title),
                edition.lang
            )
        });
    let intro = extract
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('='))
        .unwrap_or("");
    let intro = short_text(intro);
    let message = if intro.is_empty() {
        format!(
            "`{title}` is a disambiguation page on {} Wikipedia",
            edition.lang
        )
    } else {
        format!(
            "`{title}` is a disambiguation page on {} Wikipedia. {intro}",
            edition.lang
        )
    };
    let mut details = json!({
        "title": title,
        "url": url,
        "lang": edition.lang,
        "candidates": shown,
    });
    if let Some(from) = redirected_from {
        details["redirected_from"] = json!(from);
    }
    AppError::new(
        "disambiguation",
        message,
        format!("{example} Or narrow it with `agentbox wiki search {} --lang {}`. Do not guess which one.", shell_quote(title), edition.lang),
    )
    .with_details(details)
}

fn candidate_titles(extract: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in extract.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('=') || t.ends_with(':') {
            continue;
        }
        let title = t.split_once(',').map(|(h, _)| h.trim()).unwrap_or(t);
        if title.chars().count() < 2 || title.chars().count() > 120 {
            continue;
        }
        if out.iter().any(|e: &String| e == title) {
            continue;
        }
        out.push(title.to_string());
        if out.len() == CANDIDATES_MAX {
            break;
        }
    }
    out
}

fn store_article(
    store: &Store,
    url: &str,
    title: &str,
    body: &str,
) -> Result<(u64, bool), AppError> {
    if let Some((meta, old)) = fresh_wiki_doc(store, url) {
        if old == body {
            return Ok((meta.id, true));
        }
    }
    let meta = DocMeta {
        id: 0,
        url: url.to_string(),
        title: title.to_string(),
        content_type: "text/markdown; source=wikipedia".into(),
        fetched_at: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
    };
    let id = store.save_doc(meta, body)?;
    Ok((id, false))
}

fn fresh_wiki_doc(store: &Store, url: &str) -> Option<(DocMeta, String)> {
    let want = normalize_url(url);
    let now = chrono::Utc::now();
    for meta in store.doc_metas().into_iter().rev() {
        if !meta.content_type.contains("source=wikipedia") {
            continue;
        }
        if normalize_url(&meta.url) != want {
            continue;
        }
        let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&meta.fetched_at) else {
            continue;
        };
        let age = now - ts.with_timezone(&chrono::Utc);
        if age > chrono::Duration::days(PAGE_TTL_DAYS) {
            continue;
        }
        let Ok((_, body)) = store.load_doc(&format!("doc:{}", meta.id)) else {
            continue;
        };
        return Some((meta, body));
    }
    None
}

struct Target {
    title: String,
    lang_from_url: Option<String>,
}

fn parse_target(raw: &str) -> Result<Target, AppError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "title is empty",
            "Example: `agentbox wiki open 台北101` or `agentbox wiki open https://en.wikipedia.org/wiki/Albert_Einstein`.",
        ));
    }
    if raw.contains("://") {
        return parse_wiki_url(raw).ok_or_else(|| {
            AppError::new(
                "bad_args",
                format!("`{raw}` is not a Wikipedia article URL"),
                "Pass an article title, or a URL like `https://zh.wikipedia.org/wiki/台北101`.",
            )
        });
    }
    Ok(Target {
        title: raw.to_string(),
        lang_from_url: None,
    })
}

fn parse_wiki_url(raw: &str) -> Option<Target> {
    let url = reqwest::Url::parse(raw.trim()).ok()?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let rest = host.strip_suffix(".wikipedia.org")?;
    let lang = rest
        .strip_suffix(".m")
        .filter(|s| !s.is_empty())
        .unwrap_or(rest);
    if lang.contains('.') || !valid_lang(lang) {
        return None;
    }
    let path = url.path();
    let title = if let Some(rest) = path.strip_prefix("/wiki/") {
        if rest.is_empty() {
            return None;
        }
        percent_decode(rest)
    } else if path.ends_with("/index.php") {
        url.query_pairs()
            .find(|(k, _)| k == "title")
            .map(|(_, v)| v.into_owned())
            .filter(|t| !t.is_empty())?
    } else {
        let mut segs = path.trim_start_matches('/').splitn(2, '/');
        let first = segs.next().unwrap_or("");
        let rest = segs.next().unwrap_or("");
        if rest.is_empty() || !first.starts_with("zh-") {
            return None;
        }
        percent_decode(rest)
    };
    let title = normalize_title(&title);
    if title.is_empty() {
        return None;
    }
    Some(Target {
        title,
        lang_from_url: Some(lang.to_string()),
    })
}

/// `Url::path` keeps percent-encoding. Decode `%HH` bytes, leaving a bad
/// escape as the original characters.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn normalize_title(title: &str) -> String {
    title
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn edition(lang: &str) -> Result<Edition, AppError> {
    let lang = lang.trim().to_ascii_lowercase();
    if !valid_lang(&lang) {
        return Err(AppError::new(
            "bad_args",
            format!("`{lang}` is not a Wikipedia language code"),
            "Example: `agentbox wiki search 台北101 --lang zh` or `--lang en`.",
        ));
    }
    let (site, variant) = match lang.as_str() {
        "zh" | "zh-tw" | "zh-hant" | "zh-hant-tw" => ("zh", Some("zh-tw".to_string())),
        "zh-cn" | "zh-hans" | "zh-hans-cn" | "zh-sg" | "zh-hans-sg" => {
            ("zh", Some("zh-cn".to_string()))
        }
        "zh-hk" | "zh-hant-hk" | "zh-mo" | "zh-hant-mo" => ("zh", Some("zh-hk".to_string())),
        "zh-my" | "zh-hans-my" => ("zh", Some("zh-my".to_string())),
        other => (other, None),
    };
    let reported = if site == "zh" { "zh" } else { site };
    Ok(Edition {
        site: site.to_string(),
        lang: reported.to_string(),
        variant,
    })
}

fn valid_lang(s: &str) -> bool {
    let mut parts = s.split('-');
    let Some(head) = parts.next() else {
        return false;
    };
    if !(2..=12).contains(&head.len())
        || !head.chars().all(|c| c.is_ascii_lowercase())
        || matches!(head, "www" | "upload" | "commons" | "meta")
    {
        return false;
    }
    parts.all(|p| {
        (1..=12).contains(&p.len())
            && p.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

fn api_base(ep: &Endpoints, site: &str) -> String {
    if let Some(over) = &ep.api {
        return over.clone();
    }
    format!("https://{site}.wikipedia.org/w/api.php")
}

fn search_url(ep: &Endpoints, edition: &Edition, query: &str, limit: usize) -> String {
    let limit_s = limit.to_string();
    let mut pairs = vec![
        ("action", "query"),
        ("format", "json"),
        ("formatversion", "2"),
        ("utf8", "1"),
        ("maxlag", "5"),
        ("list", "search"),
        ("srsearch", query),
        ("srlimit", limit_s.as_str()),
        ("srnamespace", "0"),
        ("srprop", "snippet"),
    ];
    if let Some(v) = edition.variant.as_deref() {
        pairs.push(("variant", v));
        pairs.push(("converttitles", "1"));
    }
    action_url(&api_base(ep, &edition.site), &pairs)
}

fn open_url(ep: &Endpoints, edition: &Edition, title: &str) -> String {
    let mut pairs = vec![
        ("action", "query"),
        ("format", "json"),
        ("formatversion", "2"),
        ("utf8", "1"),
        ("maxlag", "5"),
        ("redirects", "1"),
        ("prop", "extracts|pageprops|info"),
        ("inprop", "url"),
        ("explaintext", "1"),
        ("exsectionformat", "wiki"),
        ("exlimit", "1"),
        ("ppprop", "disambiguation"),
        ("titles", title),
    ];
    if let Some(v) = edition.variant.as_deref() {
        pairs.push(("variant", v));
        pairs.push(("converttitles", "1"));
    }
    action_url(&api_base(ep, &edition.site), &pairs)
}

fn action_url(base: &str, pairs: &[(&str, &str)]) -> String {
    let mut url = String::from(base);
    url.push('?');
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            url.push('&');
        }
        url.push_str(k);
        url.push('=');
        url.push_str(&net::encode(v));
    }
    url
}

fn article_url(site: &str, title: &str) -> String {
    let slug = title.trim().replace(' ', "_");
    format!("https://{site}.wikipedia.org/wiki/{}", net::encode(&slug))
}

fn accept_language(edition: &Edition) -> String {
    match edition.variant.as_deref() {
        Some("zh-tw") => "zh-TW".to_string(),
        Some("zh-cn") => "zh-CN".to_string(),
        Some("zh-hk") => "zh-HK".to_string(),
        Some("zh-my") => "zh-MY".to_string(),
        Some(v) => v.to_string(),
        None => edition.lang.clone(),
    }
}

fn fetch_json(ctx: &Ctx, url: &str, edition: &Edition) -> Result<(String, bool), AppError> {
    if let Some(body) = cache_get(ctx.store, url) {
        return Ok((body, true));
    }
    let ua = user_agent(ctx.contact);
    let client = reqwest::blocking::Client::builder()
        .user_agent(&ua)
        .timeout(Duration::from_secs(ctx.timeout.max(1)))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| {
            AppError::new(
                "network_error",
                e.to_string(),
                "Retry; if it persists, report a bug.",
            )
        })?;
    let body = http_get(&client, url, &accept_language(edition))?;
    cache_put(ctx.store, url, &body)?;
    Ok((body, false))
}

enum Step {
    Done(String),
    Retry {
        retry_after: Option<u64>,
        message: String,
    },
}

fn http_get(
    client: &reqwest::blocking::Client,
    url: &str,
    accept_lang: &str,
) -> Result<String, AppError> {
    let mut last = String::from("Wikipedia rate limit");
    for attempt in 1..=MAX_ATTEMPTS {
        match once(client, url, accept_lang)? {
            Step::Done(body) => return Ok(body),
            Step::Retry {
                retry_after,
                message,
            } => {
                last = message;
                if attempt == MAX_ATTEMPTS {
                    break;
                }
                let fallback = 1u64 << (attempt - 1).min(3);
                let secs = retry_after.unwrap_or(fallback).min(MAX_WAIT_SECS);
                if secs > 0 {
                    std::thread::sleep(Duration::from_secs(secs));
                }
            }
        }
    }
    Err(AppError::new(
        "rate_limited",
        format!("Wikipedia asked us to slow down ({last}) and retries are exhausted"),
        "Wait a moment and retry the same wiki command once. Do not invent the article.",
    ))
}

fn once(
    client: &reqwest::blocking::Client,
    url: &str,
    accept_lang: &str,
) -> Result<Step, AppError> {
    let resp = client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::ACCEPT_LANGUAGE, accept_lang)
        .send()
        .map_err(|e| request_error(url, &e))?;
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok());
    let mut body = String::new();
    resp.take(MAX_BODY).read_to_string(&mut body).map_err(|e| {
        AppError::new(
            "network_error",
            format!("reading Wikipedia response: {e}"),
            "Retry the same wiki command.",
        )
    })?;
    if status == 429 || status == 503 {
        return Ok(Step::Retry {
            retry_after,
            message: format!("HTTP {status}"),
        });
    }
    if !(200..300).contains(&status) {
        return Err(AppError::new(
            "http_error",
            format!("HTTP {status} from Wikipedia"),
            "Check the title, or retry once. If Wikipedia is down, use `agentbox search` as a last resort.",
        ));
    }
    let value: Value = serde_json::from_str(&body).map_err(|e| bad_json(e.to_string()))?;
    if let Some(err) = value.get("error") {
        let code = err.get("code").and_then(Value::as_str).unwrap_or("");
        let info = err
            .get("info")
            .and_then(Value::as_str)
            .unwrap_or("Wikipedia API error");
        if code == "maxlag" || code == "ratelimited" {
            return Ok(Step::Retry {
                retry_after,
                message: format!("{code}: {info}"),
            });
        }
        let hint = "Retry with a plain article title, or `wiki search` it.";
        return Err(match code {
            "badtitle" | "invalidtitle" => AppError::new("bad_args", info, hint),
            _ => AppError::new("api_error", format!("Wikipedia API {code}: {info}"), hint),
        });
    }
    Ok(Step::Done(body))
}

fn request_error(url: &str, e: &reqwest::Error) -> AppError {
    if e.is_timeout() {
        return AppError::new(
            "timeout",
            format!("timed out contacting Wikipedia for {url}"),
            "Retry the same wiki command once.",
        );
    }
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        msg.push_str(&format!(": {s}"));
        src = s.source();
    }
    let lower = msg.to_lowercase();
    if [
        "dns",
        "resolve",
        "lookup",
        "name or service not known",
        "no such host",
    ]
    .iter()
    .any(|k| lower.contains(k))
    {
        return AppError::new(
            "dns_error",
            msg,
            "Check that this machine can resolve wikipedia.org, then retry.",
        );
    }
    AppError::new(
        "network_error",
        msg,
        "Check network access to wikipedia.org and retry the same wiki command.",
    )
}

fn bad_json(detail: String) -> AppError {
    AppError::new(
        "bad_response",
        format!("Wikipedia returned data that is not JSON: {detail}"),
        "Retry the same wiki command.",
    )
}

fn first_page(v: &Value) -> Option<Value> {
    let pages = match v.pointer("/query/pages") {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::Object(o)) => o.values().cloned().collect(),
        _ => return None,
    };
    pages
        .iter()
        .find(|p| !is_missing(p))
        .cloned()
        .or_else(|| pages.into_iter().next())
}

fn is_missing(page: &Value) -> bool {
    match page.get("missing") {
        Some(Value::Bool(b)) => *b,
        Some(v) if !v.is_null() => true,
        _ => false,
    }
}

fn is_disambiguation(page: &Value) -> bool {
    page.get("pageprops")
        .and_then(|p| p.get("disambiguation"))
        .is_some()
}

fn redirect_from(v: &Value, final_title: &str) -> Option<String> {
    let arr = v.pointer("/query/redirects")?.as_array()?;
    let from = arr.first()?.get("from")?.as_str()?.trim();
    if from.is_empty() || from == final_title {
        None
    } else {
        Some(from.to_string())
    }
}

fn strip_html(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        if c == '<' {
            in_tag = true;
            continue;
        }
        if in_tag {
            if c == '>' {
                in_tag = false;
            }
            continue;
        }
        out.push(c);
    }
    decode_entities(&out)
}

fn decode_entities(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else {
            out.push_str(rest);
            return out;
        };
        if end == 0 || end > 12 {
            out.push('&');
            rest = &rest[1..];
            continue;
        }
        let ent = &rest[1..end];
        let decoded = match ent {
            "amp" => Some("&".to_string()),
            "lt" => Some("<".to_string()),
            "gt" => Some(">".to_string()),
            "quot" => Some("\"".to_string()),
            "apos" | "#39" | "#039" => Some("'".to_string()),
            "nbsp" | "#160" => Some(" ".to_string()),
            _ => decode_numeric(ent),
        };
        if let Some(text) = decoded {
            out.push_str(&text);
            rest = &rest[end + 1..];
        } else {
            out.push('&');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

fn decode_numeric(ent: &str) -> Option<String> {
    let hex = ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X"));
    let code = if let Some(h) = hex {
        u32::from_str_radix(h, 16).ok()?
    } else {
        let n = ent.strip_prefix('#')?;
        if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        n.parse().ok()?
    };
    char::from_u32(code).map(|c| {
        if c == '\u{00a0}' {
            " ".to_string()
        } else {
            c.to_string()
        }
    })
}

fn short_text(s: &str) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= SNIPPET_CHARS {
        flat
    } else {
        let head: String = flat.chars().take(SNIPPET_CHARS).collect();
        format!("{head}…")
    }
}

fn wiki_heading(line: &str) -> Option<(usize, String)> {
    let t = line.trim();
    if t.len() < 3 || !t.starts_with('=') || !t.ends_with('=') {
        return None;
    }
    let level = t.bytes().take_while(|b| *b == b'=').count();
    let end = t.bytes().rev().take_while(|b| *b == b'=').count();
    if level != end || !(1..=6).contains(&level) {
        return None;
    }
    let inner = t[level..t.len() - level].trim();
    if inner.is_empty() || inner.contains('=') {
        return None;
    }
    Some((level, inner.to_string()))
}

fn wiki_to_markdown(title: &str, extract: &str) -> String {
    let mut out = format!("# {title}\n\n");
    for line in extract.replace("\r\n", "\n").lines() {
        if let Some((level, heading)) = wiki_heading(line) {
            out.push_str(&"#".repeat(level));
            out.push(' ');
            out.push_str(&heading);
            out.push('\n');
        } else {
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    let mut collapsed = String::new();
    let mut blanks = 0;
    for line in out.lines() {
        if line.trim().is_empty() {
            blanks += 1;
            if blanks <= 1 {
                collapsed.push('\n');
            }
        } else {
            blanks = 0;
            collapsed.push_str(line);
            collapsed.push('\n');
        }
    }
    collapsed.trim().to_string() + "\n"
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CacheFile {
    url: String,
    fetched_at: String,
    body: String,
}

fn cache_path(store: &Store, url: &str) -> PathBuf {
    store
        .root()
        .join("wiki")
        .join("cache")
        .join(format!("{:016x}.json", fnv64(url)))
}

fn fnv64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn cache_ttl(url: &str) -> chrono::Duration {
    if url.contains("list=search") {
        chrono::Duration::hours(SEARCH_TTL_HOURS)
    } else {
        chrono::Duration::days(PAGE_TTL_DAYS)
    }
}

fn cache_get(store: &Store, url: &str) -> Option<String> {
    let text = fs::read_to_string(cache_path(store, url)).ok()?;
    let file: CacheFile = serde_json::from_str(&text).ok()?;
    if file.url != url {
        return None;
    }
    let ts = chrono::DateTime::parse_from_rfc3339(&file.fetched_at).ok()?;
    let age = chrono::Utc::now() - ts.with_timezone(&chrono::Utc);
    if age > cache_ttl(url) {
        return None;
    }
    Some(file.body)
}

fn cache_put(store: &Store, url: &str, body: &str) -> Result<(), AppError> {
    let path = cache_path(store, url);
    if let Some(dir) = path.parent() {
        store.ensure_dir(dir)?;
    }
    let file = CacheFile {
        url: url.to_string(),
        fetched_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        body: body.to_string(),
    };
    let text = serde_json::to_string(&file).map_err(|e| {
        AppError::new(
            "state_error",
            format!("cache json: {e}"),
            "Retry the same wiki command.",
        )
    })?;
    fs::write(&path, text).map_err(|e| AppError::io("write wiki cache", e))?;
    Ok(())
}
