//! `search <query>`: web search via Tavily (bring your own key) or keyless
//! scraping backends (DuckDuckGo HTML, then Bing as a fallback).
//!
//! The Tavily key is read from env `TAVILY_API_KEY` or the state-dir config
//! file, never from a CLI flag, and is scrubbed from every output.

use crate::cmd::fetch::{self, USER_AGENT};
use crate::dom;
use crate::envelope::{AppError, CmdResult, Output};
use crate::state::{DocMeta, Store};
use serde_json::{json, Value};
use std::time::Duration;

pub const MAX_RESULTS_CAP: usize = 20;
pub const MAX_SAVE: usize = 5;
const SNIPPET_CHARS: usize = 300;
const TIMEOUT_SECS: u64 = 30;
const SAVE_FETCH_TIMEOUT_SECS: u64 = 20;
const BACKENDS: &[&str] = &["auto", "tavily", "ddg", "bing"];

/// Service endpoints; overridable via env for tests and proxies.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub tavily: String,
    pub ddg: String,
    pub bing: String,
}

impl Endpoints {
    pub fn from_env() -> Self {
        let get = |k: &str, d: &str| {
            std::env::var(k)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| d.to_string())
        };
        Self {
            tavily: get("AGENTBOX_TAVILY_URL", "https://api.tavily.com"),
            ddg: get("AGENTBOX_DDG_URL", "https://html.duckduckgo.com/html/"),
            bing: get("AGENTBOX_BING_URL", "https://www.bing.com/search"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SearchArgs {
    pub query: String,
    pub max_results: usize,
    pub sites: Vec<String>,
    pub exclude_sites: Vec<String>,
    pub days: Option<u32>,
    pub time: Option<String>,
    pub news: bool,
    pub deep: bool,
    pub answer: bool,
    pub save: usize,
    pub backend: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub published: Option<String>,
    pub score: Option<f64>,
    pub raw: Option<String>,
}

#[derive(Debug, Default)]
struct BackendResult {
    answer: Option<String>,
    hits: Vec<Hit>,
}

/// Why a keyless backend produced nothing usable.
enum Failure {
    Blocked(String),
    Error(AppError),
}

pub fn run(store: &Store, args: &SearchArgs, key: Option<&str>, ep: &Endpoints) -> CmdResult {
    let res = run_inner(store, args, key, ep);
    // Defense in depth: the key must never appear in any output.
    match (res, key) {
        (Ok(mut out), Some(k)) => {
            let s = serde_json::to_string(&out.data).unwrap_or_default();
            if s.contains(k) {
                out.data = serde_json::from_str(&s.replace(k, "[redacted]")).unwrap_or(Value::Null);
            }
            out.hint = out.hint.map(|h| redact(&h, Some(k)));
            Ok(out)
        }
        (Err(mut e), k) => {
            e.message = redact(&e.message, k);
            e.hint = redact(&e.hint, k);
            Err(e)
        }
        (ok, None) => ok,
    }
}

pub fn redact(s: &str, key: Option<&str>) -> String {
    match key {
        Some(k) if k.len() >= 4 => s.replace(k, "[redacted]"),
        _ => s.to_string(),
    }
}

fn run_inner(store: &Store, args: &SearchArgs, key: Option<&str>, ep: &Endpoints) -> CmdResult {
    let query = args.query.trim();
    if query.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "empty query",
            "Example: `agentbox search \"EU AI Act GPAI obligations\"`.",
        ));
    }
    let mut notes: Vec<String> = Vec::new();
    let mut max_results = args.max_results.max(1);
    if max_results > MAX_RESULTS_CAP {
        notes.push(format!("max_results capped at {MAX_RESULTS_CAP}"));
        max_results = MAX_RESULTS_CAP;
    }
    let mut save = args.save;
    if save > MAX_SAVE {
        notes.push(format!("save capped at {MAX_SAVE}"));
        save = MAX_SAVE;
    }
    let time = match args
        .time
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        Some(t @ ("day" | "week" | "month" | "year")) => Some(t.to_string()),
        Some(t) => {
            return Err(AppError::new(
                "bad_args",
                format!("bad --time `{t}`"),
                "Use --time day, week, month or year, or --days N.",
            ));
        }
        None => None,
    };
    if time.is_some() && args.days.is_some() {
        notes.push("both --time and --days given; using --time".into());
    }
    let opts = Opts {
        max_results,
        save,
        time,
        ..Opts::from(args)
    };

    let backend = args.backend.trim();
    let backend = if backend.is_empty() { "auto" } else { backend };
    if !BACKENDS.contains(&backend) {
        return Err(AppError::new(
            "bad_args",
            format!("unknown backend `{backend}`"),
            "Use --backend auto, tavily, ddg or bing.",
        ));
    }
    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(TIMEOUT_SECS))
        .build()
        .map_err(|e| {
            AppError::new(
                "network_error",
                e.to_string(),
                "Retry; if it persists, report a bug.",
            )
        })?;

    let mut fallback_from: Option<&str> = None;
    let (used, result) = match (backend, key) {
        ("tavily", None) => {
            return Err(AppError::new(
                "no_api_key",
                "no Tavily API key configured",
                "Set env TAVILY_API_KEY, or run `agentbox config set tavily.api_key -` and paste the key; or use `--backend ddg`.",
            ));
        }
        ("tavily" | "auto", Some(k)) => ("tavily", tavily(&client, &ep.tavily, k, query, &opts)?),
        ("bing", _) => {
            keyless_notes(&opts, &mut notes);
            (
                "bing",
                bing(&client, &ep.bing, query, &opts).map_err(failure_error)?,
            )
        }
        ("ddg", _) => {
            keyless_notes(&opts, &mut notes);
            (
                "ddg",
                ddg(&client, &ep.ddg, query, &opts).map_err(failure_error)?,
            )
        }
        _ => {
            // auto without a key: DuckDuckGo, then Bing if DDG blocks or fails.
            keyless_notes(&opts, &mut notes);
            match ddg(&client, &ep.ddg, query, &opts) {
                Ok(r) if !r.hits.is_empty() => ("ddg", r),
                first => {
                    let why = match first {
                        Ok(_) => "no results".to_string(),
                        Err(Failure::Blocked(m)) => m,
                        Err(Failure::Error(e)) => e.message,
                    };
                    notes.push(format!("ddg unavailable ({why}); fell back to bing"));
                    fallback_from = Some("ddg");
                    (
                        "bing",
                        bing(&client, &ep.bing, query, &opts).map_err(failure_error)?,
                    )
                }
            }
        }
    };

    if used == "bing" {
        notes.push(
            "bing keyless results can be loosely matched from server IPs; verify relevance".into(),
        );
    }
    let hits = finalize(result.hits, &opts);
    if hits.is_empty() {
        return Err(AppError::new(
            "no_results",
            format!("no results for `{query}` ({used})"),
            "Broaden the query: fewer words, drop --site/--days filters, or try another --backend.",
        ));
    }

    let mut results = Vec::new();
    for (i, h) in hits.iter().enumerate() {
        let mut r = json!({ "rank": i + 1, "title": h.title, "url": h.url, "snippet": h.snippet });
        if let Some(p) = &h.published {
            r["published"] = json!(p);
        }
        if let Some(s) = h.score {
            r["score"] = json!((s * 1000.0).round() / 1000.0);
        }
        if i < opts.save {
            match save_hit(store, h) {
                Ok(id) => r["doc"] = json!(format!("doc:{id}")),
                Err(e) => r["doc_error"] = json!(format!("{}: {}", e.code, e.message)),
            }
        }
        results.push(r);
    }

    let mut data = json!({ "backend": used, "query": query });
    if let Some(f) = fallback_from {
        data["fallback_from"] = json!(f);
    }
    if let Some(a) = result.answer.filter(|a| !a.trim().is_empty()) {
        data["answer"] = json!(a);
    }
    data["results"] = json!(results);
    if !notes.is_empty() {
        data["notes"] = json!(notes);
    }

    let next = if opts.save > 0 {
        "Saved results can be read with `agentbox read doc:N --grep KEYWORD`.".to_string()
    } else {
        "Open a result with `agentbox fetch URL`, or rerun with `--save 2` to store the top results as docs.".to_string()
    };
    let hint = if used == "tavily" {
        next
    } else {
        format!("Keyless {used} results. For better results set a Tavily key: env TAVILY_API_KEY or `agentbox config set tavily.api_key -`. {next}")
    };
    Ok(Output::new(data).hint(hint))
}

#[derive(Debug, Clone, Default)]
struct Opts {
    max_results: usize,
    sites: Vec<String>,
    exclude_sites: Vec<String>,
    days: Option<u32>,
    time: Option<String>,
    news: bool,
    deep: bool,
    answer: bool,
    save: usize,
}

impl From<&SearchArgs> for Opts {
    fn from(a: &SearchArgs) -> Self {
        let clean = |v: &[String]| -> Vec<String> {
            v.iter()
                .flat_map(|s| s.split(','))
                .map(|s| {
                    s.trim()
                        .trim_start_matches("https://")
                        .trim_start_matches("http://")
                        .trim_end_matches('/')
                        .to_string()
                })
                .filter(|s| !s.is_empty())
                .collect()
        };
        Self {
            max_results: a.max_results,
            sites: clean(&a.sites),
            exclude_sites: clean(&a.exclude_sites),
            days: a.days.filter(|d| *d > 0),
            time: a.time.clone(),
            news: a.news,
            deep: a.deep,
            answer: a.answer,
            save: a.save,
        }
    }
}

impl Opts {
    /// Effective recency window as day/week/month/year, if any.
    fn window(&self) -> Option<&'static str> {
        match self.time.as_deref() {
            Some("day") => Some("day"),
            Some("week") => Some("week"),
            Some("month") => Some("month"),
            Some("year") => Some("year"),
            _ => self.days.map(|d| match d {
                0..=1 => "day",
                2..=7 => "week",
                8..=31 => "month",
                _ => "year",
            }),
        }
    }

    /// Query text with site operators, for scraping backends.
    fn decorated_query(&self, q: &str) -> String {
        let mut s = q.to_string();
        if !self.sites.is_empty() {
            let sites: Vec<String> = self.sites.iter().map(|d| format!("site:{d}")).collect();
            s.push(' ');
            s.push_str(&sites.join(" OR "));
        }
        for d in &self.exclude_sites {
            s.push_str(&format!(" -site:{d}"));
        }
        s
    }
}

fn keyless_notes(opts: &Opts, notes: &mut Vec<String>) {
    if opts.deep {
        notes.push("--deep needs Tavily; ignored".into());
    }
    if opts.answer {
        notes.push("--answer needs Tavily; no answer generated".into());
    }
    if opts.news {
        notes.push("--news needs Tavily; keyless backends search the general web".into());
    }
    if opts.days.is_some() && opts.time.is_none() {
        notes.push("keyless backends round --days to day/week/month/year".into());
    }
}

fn failure_error(f: Failure) -> AppError {
    match f {
        Failure::Error(e) => e,
        Failure::Blocked(m) => {
            let other = if m.starts_with("Bing") { "ddg" } else { "bing" };
            AppError::new(
                "blocked",
                m,
                format!("The search engine is showing a bot check. Try `--backend {other}`, wait a few minutes, or set a Tavily key (env TAVILY_API_KEY)."),
            )
        }
    }
}

// ---------------------------------------------------------------- Tavily

fn tavily(
    client: &reqwest::blocking::Client,
    base: &str,
    key: &str,
    query: &str,
    o: &Opts,
) -> Result<BackendResult, AppError> {
    let url = format!("{}/search", base.trim_end_matches('/'));
    let mut body = json!({
        "query": query,
        "search_depth": if o.deep { "advanced" } else { "basic" },
        "topic": if o.news { "news" } else { "general" },
        "max_results": o.max_results,
        "include_answer": o.answer,
        "include_raw_content": if o.save > 0 { json!("markdown") } else { json!(false) },
    });
    if !o.sites.is_empty() {
        body["include_domains"] = json!(o.sites);
    }
    if !o.exclude_sites.is_empty() {
        body["exclude_domains"] = json!(o.exclude_sites);
    }
    if let Some(t) = &o.time {
        body["time_range"] = json!(t);
        body["include_published_date"] = json!(true);
    } else if let Some(d) = o.days {
        let start = chrono::Utc::now().date_naive() - chrono::Duration::days(i64::from(d));
        body["start_date"] = json!(start.format("%Y-%m-%d").to_string());
        body["include_published_date"] = json!(true);
    }
    let resp = client
        .post(&url)
        .bearer_auth(key)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_string())
        .send()
        .map_err(|e| net_error("Tavily", &e))?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    if status != 200 {
        return Err(tavily_status_error(status, &text));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| {
        AppError::new(
            "bad_response",
            format!("Tavily returned invalid JSON: {e}"),
            "Retry, or use `--backend ddg`.",
        )
    })?;
    let hits = v["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|r| Hit {
                    title: r["title"].as_str().unwrap_or("").trim().to_string(),
                    url: r["url"].as_str().unwrap_or("").trim().to_string(),
                    snippet: r["content"].as_str().unwrap_or("").to_string(),
                    published: r["published_date"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map(String::from),
                    score: r["score"].as_f64(),
                    raw: r["raw_content"]
                        .as_str()
                        .filter(|s| !s.trim().is_empty())
                        .map(String::from),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(BackendResult {
        answer: v["answer"].as_str().map(String::from),
        hits,
    })
}

fn tavily_status_error(status: u16, body: &str) -> AppError {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/detail/error")
                .and_then(Value::as_str)
                .map(String::from)
                .or_else(|| {
                    v.get("detail").map(|d| {
                        if let Some(s) = d.as_str() {
                            s.to_string()
                        } else {
                            d.to_string()
                        }
                    })
                })
        })
        .unwrap_or_else(|| body.chars().take(200).collect());
    let msg = format!("Tavily HTTP {status}: {detail}");
    match status {
        401 | 403 => AppError::new(
            "invalid_api_key",
            msg,
            "The Tavily key was rejected. Check which key is active with `agentbox config get tavily.api_key` (env TAVILY_API_KEY wins over the config file), replace it with `agentbox config set tavily.api_key -`, or use `--backend ddg` meanwhile.",
        ),
        429 => AppError::new("rate_limited", msg, "Tavily rate limit hit. Wait a minute and retry, or use `--backend ddg` meanwhile."),
        432 | 433 => AppError::new("quota_exceeded", msg, "Tavily plan or pay-as-you-go limit reached. Raise the limit in the Tavily dashboard, or use `--backend ddg`."),
        400 | 422 => AppError::new("bad_request", msg, "Tavily rejected the parameters. Simplify the query or drop --site/--days options."),
        _ => AppError::new("upstream_error", msg, "Tavily had a server problem. Retry later, or use `--backend ddg`."),
    }
}

fn net_error(service: &str, e: &reqwest::Error) -> AppError {
    let code = if e.is_timeout() {
        "timeout"
    } else {
        "network_error"
    };
    let mut msg = format!("{service} request failed: {e}");
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        msg.push_str(&format!(": {s}"));
        src = s.source();
    }
    let hint = if service == "Tavily" {
        "Check network access to api.tavily.com, or try `--backend ddg`."
    } else {
        "Check network access, or try another `--backend` (tavily, ddg, bing)."
    };
    AppError::new(code, msg, hint)
}

// ---------------------------------------------------------------- DuckDuckGo

fn ddg(
    client: &reqwest::blocking::Client,
    base: &str,
    query: &str,
    o: &Opts,
) -> Result<BackendResult, Failure> {
    let q = o.decorated_query(query);
    let mut params: Vec<(&str, String)> = vec![("q", q), ("kl", "wt-wt".into())];
    if let Some(w) = o.window() {
        params.push(("df", w[..1].to_string()));
    }
    let resp = client
        .get(base)
        .query(&params)
        .send()
        .map_err(|e| Failure::Error(net_error("DuckDuckGo", &e)))?;
    let status = resp.status().as_u16();
    let html = resp.text().unwrap_or_default();
    if status == 202 || html.contains("anomaly-modal") || html.contains("challenge-form") {
        return Err(Failure::Blocked(
            "DuckDuckGo returned a bot-check page".into(),
        ));
    }
    if status != 200 {
        return Err(Failure::Error(AppError::new(
            "http_error",
            format!("DuckDuckGo HTTP {status}"),
            "Try `--backend bing`, or set a Tavily key.",
        )));
    }
    Ok(BackendResult {
        answer: None,
        hits: parse_ddg(&html),
    })
}

pub fn parse_ddg(html: &str) -> Vec<Hit> {
    let doc = dom::parse(html);
    let blocks = dom::find_all(&doc, &|n| {
        dom::has_class(n, "result") && !dom::has_class(n, "result--ad")
    });
    let mut hits = Vec::new();
    for b in blocks {
        let Some(a) = dom::find_first(&b, &|n| dom::has_class(n, "result__a")) else {
            continue;
        };
        let href = dom::attr(&a, "href").unwrap_or_default();
        if href.contains("duckduckgo.com/y.js") {
            continue; // ad redirect
        }
        let Some(url) = ddg_target(&href) else {
            continue;
        };
        let snippet = dom::find_first(&b, &|n| dom::has_class(n, "result__snippet"))
            .map(|s| dom::text(&s))
            .unwrap_or_default();
        hits.push(Hit {
            title: dom::text(&a),
            url,
            snippet,
            ..Hit::default()
        });
    }
    hits
}

/// Resolve DDG's `//duckduckgo.com/l/?uddg=<encoded>` redirect links.
pub fn ddg_target(href: &str) -> Option<String> {
    let abs = if href.starts_with("//") {
        format!("https:{href}")
    } else {
        href.to_string()
    };
    let u = reqwest::Url::parse(&abs).ok()?;
    if u.host_str().is_some_and(|h| h.ends_with("duckduckgo.com")) {
        return u
            .query_pairs()
            .find(|(k, _)| k == "uddg")
            .map(|(_, v)| v.to_string());
    }
    matches!(u.scheme(), "http" | "https").then(|| u.to_string())
}

// ---------------------------------------------------------------- Bing

fn bing(
    client: &reqwest::blocking::Client,
    base: &str,
    query: &str,
    o: &Opts,
) -> Result<BackendResult, Failure> {
    let q = o.decorated_query(query);
    let count = (o.max_results * 2).clamp(10, 30).to_string();
    let mut params: Vec<(&str, String)> = vec![("q", q), ("count", count)];
    let filter = match (o.time.as_deref(), o.days) {
        (Some("day"), _) => Some("ex1:\"ez1\"".to_string()),
        (Some("week"), _) => Some("ex1:\"ez2\"".to_string()),
        (Some("month"), _) => Some("ex1:\"ez3\"".to_string()),
        (Some(_), _) => Some(bing_range(365)),
        (None, Some(d)) => Some(bing_range(d)),
        _ => None,
    };
    if let Some(f) = filter {
        params.push(("filters", f));
    }
    let resp = client
        .get(base)
        .query(&params)
        .send()
        .map_err(|e| Failure::Error(net_error("Bing", &e)))?;
    let status = resp.status().as_u16();
    let html = resp.text().unwrap_or_default();
    if status != 200 {
        return Err(Failure::Error(AppError::new(
            "http_error",
            format!("Bing HTTP {status}"),
            "Try `--backend ddg`, or set a Tavily key.",
        )));
    }
    let hits = parse_bing(&html);
    if hits.is_empty() && (html.contains("b_captcha") || html.contains("/challenge/")) {
        return Err(Failure::Blocked("Bing returned a bot-check page".into()));
    }
    Ok(BackendResult { answer: None, hits })
}

/// Bing custom date range filter: last `days` days (day numbers since epoch).
fn bing_range(days: u32) -> String {
    let today = chrono::Utc::now().timestamp() / 86_400;
    format!("ex1:\"ez5_{}_{}\"", today - i64::from(days), today)
}

pub fn parse_bing(html: &str) -> Vec<Hit> {
    let doc = dom::parse(html);
    let items = dom::find_all(&doc, &|n| {
        dom::tag(n).as_deref() == Some("li") && dom::has_class(n, "b_algo")
    });
    let mut hits = Vec::new();
    for li in items {
        let Some(h2) = dom::find_first(&li, &|n| dom::tag(n).as_deref() == Some("h2")) else {
            continue;
        };
        let Some(a) = dom::find_first(&h2, &|n| dom::tag(n).as_deref() == Some("a")) else {
            continue;
        };
        let Some(url) = dom::attr(&a, "href").and_then(|h| bing_target(&h)) else {
            continue;
        };
        let snippet = dom::find_first(&li, &|n| dom::class_starts_with(n, "b_lineclamp"))
            .or_else(|| dom::find_first(&li, &|n| dom::has_class(n, "b_caption")))
            .map(|s| dom::text(&s))
            .unwrap_or_default();
        hits.push(Hit {
            title: dom::text(&a),
            url,
            snippet,
            ..Hit::default()
        });
    }
    hits
}

/// Resolve Bing's `/ck/a?...&u=a1<base64url>` click-tracking links.
pub fn bing_target(href: &str) -> Option<String> {
    let u = reqwest::Url::parse(href).ok()?;
    if u.host_str().is_some_and(|h| h.ends_with("bing.com")) && u.path().starts_with("/ck/") {
        let enc = u
            .query_pairs()
            .find(|(k, _)| k == "u")
            .map(|(_, v)| v.to_string())?;
        let raw = base64url_decode(enc.strip_prefix("a1").unwrap_or(&enc))?;
        let target = String::from_utf8(raw).ok()?;
        return target.starts_with("http").then_some(target);
    }
    matches!(u.scheme(), "http" | "https").then(|| u.to_string())
}

/// Decode unpadded base64url (RFC 4648 §5).
pub fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

// ---------------------------------------------------------------- shared

/// Dedupe by normalized URL, drop empties, trim snippets, cap the count.
/// Also enforces --site / --exclude-site, since scraped engines may ignore them.
fn finalize(hits: Vec<Hit>, o: &Opts) -> Vec<Hit> {
    let max = o.max_results;
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for mut h in hits {
        if h.url.is_empty() {
            continue;
        }
        let host = reqwest::Url::parse(&h.url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_lowercase))
            .unwrap_or_default();
        let on = |d: &String| {
            let d = d.to_lowercase();
            host == d || host.ends_with(&format!(".{d}"))
        };
        if (!o.sites.is_empty() && !o.sites.iter().any(on)) || o.exclude_sites.iter().any(on) {
            continue;
        }
        let key = normalize_url(&h.url);
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        if h.title.is_empty() {
            h.title = h.url.clone();
        }
        h.snippet = trim_snippet(&h.snippet, SNIPPET_CHARS);
        out.push(h);
        if out.len() >= max {
            break;
        }
    }
    out
}

/// Normalize for dedupe: lowercase scheme/host, drop `www.`, fragment,
/// trailing slash and utm_* tracking parameters.
pub fn normalize_url(url: &str) -> String {
    let Ok(mut u) = reqwest::Url::parse(url) else {
        return url.trim().to_lowercase();
    };
    u.set_fragment(None);
    let pairs: Vec<(String, String)> = u
        .query_pairs()
        .filter(|(k, _)| !k.starts_with("utm_"))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    if pairs.is_empty() {
        u.set_query(None);
    } else {
        u.query_pairs_mut().clear().extend_pairs(pairs);
    }
    let host = u
        .host_str()
        .unwrap_or("")
        .trim_start_matches("www.")
        .to_string();
    let path = u.path().trim_end_matches('/').to_string();
    let query = u.query().map(|q| format!("?{q}")).unwrap_or_default();
    format!("{host}{path}{query}")
}

pub fn trim_snippet(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match s.char_indices().nth(max) {
        None => s,
        Some((cut, _)) => {
            let head = &s[..cut];
            let head = head
                .rfind(' ')
                .filter(|&i| i > cut * 2 / 3)
                .map(|i| &head[..i])
                .unwrap_or(head);
            format!("{}…", head.trim_end_matches([',', '.', ';', ':', ' ']))
        }
    }
}

/// Store a hit as a doc: Tavily raw content when present, else fetch the page.
fn save_hit(store: &Store, h: &Hit) -> Result<u64, AppError> {
    if let Some(raw) = &h.raw {
        let meta = DocMeta {
            id: 0,
            url: h.url.clone(),
            title: h.title.clone(),
            content_type: "text/markdown; source=tavily".into(),
            fetched_at: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        };
        let mut body = crate::markdown::tidy(&crate::markdown::strip_images(raw));
        if !body.trim_start().starts_with('#') {
            body = format!("# {}\n\n{body}", h.title);
        }
        return store.save_doc(meta, &body);
    }
    fetch::fetch_doc(store, &h.url, SAVE_FETCH_TIMEOUT_SECS).map(|d| d.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{http, serve};

    const KEY: &str = "tvly-dev-TESTKEY0123456789abcdef";

    fn args(q: &str) -> SearchArgs {
        SearchArgs {
            query: q.into(),
            max_results: 5,
            backend: "auto".into(),
            ..SearchArgs::default()
        }
    }

    fn eps(tavily: &str, ddg: &str, bing: &str) -> Endpoints {
        Endpoints {
            tavily: tavily.into(),
            ddg: ddg.into(),
            bing: bing.into(),
        }
    }

    const DEAD: &str = "http://127.0.0.1:9/";

    const DDG_HTML: &str = r#"<html><body>
      <div class="result results_links result--ad"><h2 class="result__title"><a class="result__a" href="https://duckduckgo.com/y.js?ad=1">Buy now</a></h2></div>
      <div class="result results_links web-result"><div class="result__body">
        <h2 class="result__title"><a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&amp;rut=abc">Rust Programming <b>Language</b></a></h2>
        <a class="result__snippet" href="x">A language empowering everyone to build reliable and efficient software.</a>
      </div></div>
      <div class="result results_links web-result"><div class="result__body">
        <h2 class="result__title"><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Frust-lang.org%23intro&amp;rut=def">Rust dup</a></h2>
        <a class="result__snippet" href="x">duplicate</a>
      </div></div>
      <div class="result results_links web-result"><div class="result__body">
        <h2 class="result__title"><a class="result__a" href="https://en.wikipedia.org/wiki/Rust_(programming_language)">Rust - Wikipedia</a></h2>
        <div class="result__snippet">Rust is a general-purpose programming language.</div>
      </div></div>
    </body></html>"#;

    const BING_HTML: &str = r#"<html><body><ol id="b_results">
      <li class="b_algo"><h2><a href="https://www.bing.com/ck/a?!&amp;&amp;p=x&amp;u=a1aHR0cHM6Ly9ydXN0LWxhbmcub3JnLw&amp;ntb=1"><strong>Rust</strong> Programming Language</a></h2>
        <div class="b_caption"><p class="b_lineclamp2">Rust is blazingly fast and memory-efficient.</p></div></li>
      <li class="b_algo"><h2><a href="https://doc.rust-lang.org/book/">The Book</a></h2><div class="b_caption"><p>Learn Rust.</p></div></li>
    </ol></body></html>"#;

    #[test]
    fn tavily_success_with_dedupe_answer_and_save() {
        let long = "word ".repeat(200);
        let body = json!({
            "query": "q", "answer": "Rust 1.0 shipped in May 2015.",
            "results": [
                {"title": "Rust", "url": "https://www.rust-lang.org/", "content": long, "score": 0.91234, "raw_content": "Rust is a language.\n\n## Install\nUse rustup."},
                {"title": "Rust again", "url": "https://rust-lang.org#top", "content": "dup", "score": 0.5},
                {"title": "News", "url": "https://blog.rust-lang.org/x?utm_source=a", "content": "Release notes", "published_date": "Tue, 07 Oct 2026 10:00:00 GMT"}
            ]
        })
        .to_string();
        let (base, rx) = serve(vec![http("200 OK", "application/json", &body)]);
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        let a = SearchArgs {
            answer: true,
            deep: true,
            news: true,
            save: 1,
            sites: vec!["rust-lang.org, https://blog.rust-lang.org/".into()],
            time: Some("week".into()),
            ..args("rust release")
        };
        let out = run(&store, &a, Some(KEY), &eps(&base, DEAD, DEAD)).unwrap();

        let req = rx.recv().unwrap();
        assert!(req.line.starts_with("POST /search "), "{}", req.line);
        assert_eq!(
            req.header("authorization"),
            Some(format!("Bearer {KEY}").as_str())
        );
        let sent: Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(sent["query"], "rust release");
        assert_eq!(sent["search_depth"], "advanced");
        assert_eq!(sent["topic"], "news");
        assert_eq!(sent["include_answer"], true);
        assert_eq!(sent["include_raw_content"], "markdown");
        assert_eq!(sent["time_range"], "week");
        assert_eq!(
            sent["include_domains"],
            json!(["rust-lang.org", "blog.rust-lang.org"])
        );

        let d = &out.data;
        assert_eq!(d["backend"], "tavily");
        assert_eq!(d["answer"], "Rust 1.0 shipped in May 2015.");
        let results = d["results"].as_array().unwrap();
        assert_eq!(results.len(), 2, "duplicate URL should be dropped");
        assert_eq!(results[0]["score"], 0.912);
        assert!(results[0]["snippet"].as_str().unwrap().chars().count() <= SNIPPET_CHARS + 1);
        assert_eq!(results[0]["doc"], "doc:1");
        assert!(results[1].get("doc").is_none());
        assert_eq!(results[1]["published"], "Tue, 07 Oct 2026 10:00:00 GMT");
        let (_, saved) = store.load_doc("doc:1").unwrap();
        assert!(
            saved.starts_with("# Rust\n") && saved.contains("## Install"),
            "{saved}"
        );
        let all = serde_json::to_string(&d).unwrap() + &out.hint.unwrap();
        assert!(!all.contains(KEY));
    }

    #[test]
    fn tavily_days_become_start_date() {
        let (base, rx) = serve(vec![http(
            "200 OK",
            "application/json",
            r#"{"results":[{"title":"t","url":"https://a.test","content":"c"}]}"#,
        )]);
        let tmp = tempfile::tempdir().unwrap();
        let a = SearchArgs {
            days: Some(3),
            backend: "tavily".into(),
            ..args("x")
        };
        run(
            &Store::new(tmp.path()),
            &a,
            Some(KEY),
            &eps(&base, DEAD, DEAD),
        )
        .unwrap();
        let sent: Value = serde_json::from_str(&rx.recv().unwrap().body).unwrap();
        let expect = (chrono::Utc::now().date_naive() - chrono::Duration::days(3))
            .format("%Y-%m-%d")
            .to_string();
        assert_eq!(sent["start_date"], expect);
        assert_eq!(sent["include_raw_content"], false);
        assert!(sent.get("time_range").is_none());
    }

    #[test]
    fn tavily_401_maps_to_invalid_key_and_never_leaks_it() {
        // A hostile/buggy upstream echoing the key must still not leak it.
        let body = format!(r#"{{"detail":{{"error":"Unauthorized: invalid API key {KEY}"}}}}"#);
        let (base, _rx) = serve(vec![http("401 Unauthorized", "application/json", &body)]);
        let tmp = tempfile::tempdir().unwrap();
        let e = run(
            &Store::new(tmp.path()),
            &args("x"),
            Some(KEY),
            &eps(&base, DEAD, DEAD),
        )
        .unwrap_err();
        assert_eq!(e.code, "invalid_api_key");
        assert!(e.hint.contains("agentbox config get"));
        assert!(
            !e.message.contains(KEY) && !e.hint.contains(KEY),
            "{}",
            e.message
        );
        assert!(e.message.contains("[redacted]"));
    }

    #[test]
    fn tavily_429_and_quota() {
        let (base, _rx) = serve(vec![
            http(
                "429 Too Many Requests",
                "application/json",
                r#"{"detail":{"error":"slow down"}}"#,
            ),
            http(
                "432 Plan Limit",
                "application/json",
                r#"{"detail":{"error":"limit"}}"#,
            ),
        ]);
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        let ep = eps(&base, DEAD, DEAD);
        assert_eq!(
            run(&store, &args("x"), Some(KEY), &ep).unwrap_err().code,
            "rate_limited"
        );
        assert_eq!(
            run(&store, &args("x"), Some(KEY), &ep).unwrap_err().code,
            "quota_exceeded"
        );
    }

    #[test]
    fn explicit_tavily_without_key_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let a = SearchArgs {
            backend: "tavily".into(),
            ..args("x")
        };
        let e = run(&Store::new(tmp.path()), &a, None, &eps(DEAD, DEAD, DEAD)).unwrap_err();
        assert_eq!(e.code, "no_api_key");
        assert!(e.hint.contains("TAVILY_API_KEY"));
    }

    #[test]
    fn auto_without_key_uses_ddg_with_dedupe_and_hint() {
        let (base, rx) = serve(vec![http("200 OK", "text/html", DDG_HTML)]);
        let tmp = tempfile::tempdir().unwrap();
        let a = SearchArgs {
            sites: vec!["rust-lang.org".into()],
            time: Some("month".into()),
            answer: true,
            ..args("rust")
        };
        let out = run(
            &Store::new(tmp.path()),
            &a,
            None,
            &eps(DEAD, &format!("{base}/html/"), DEAD),
        )
        .unwrap();
        let req = rx.recv().unwrap();
        assert!(
            req.line.contains("q=rust+site%3Arust-lang.org"),
            "{}",
            req.line
        );
        assert!(req.line.contains("df=m"), "{}", req.line);
        let d = &out.data;
        assert_eq!(d["backend"], "ddg");
        let results = d["results"].as_array().unwrap();
        // The duplicate is dropped; Wikipedia is dropped by the --site filter.
        assert_eq!(results.len(), 1, "{results:?}");
        assert_eq!(results[0]["url"], "https://www.rust-lang.org/");
        assert_eq!(results[0]["title"], "Rust Programming Language");
        assert_eq!(parse_ddg(DDG_HTML).len(), 3, "ad must be skipped");
        assert!(d["notes"].to_string().contains("--answer needs Tavily"));
        assert!(out.hint.unwrap().contains("Tavily key"));
    }

    #[test]
    fn auto_falls_back_to_bing_when_ddg_blocks() {
        let blocked = r#"<html><form id="challenge-form"><div class="anomaly-modal__title">bots</div></form></html>"#;
        let (ddg_base, _r1) = serve(vec![http("202 Accepted", "text/html", blocked)]);
        let (bing_base, r2) = serve(vec![http("200 OK", "text/html", BING_HTML)]);
        let tmp = tempfile::tempdir().unwrap();
        let a = SearchArgs {
            days: Some(10),
            ..args("rust")
        };
        let out = run(
            &Store::new(tmp.path()),
            &a,
            None,
            &eps(DEAD, &ddg_base, &format!("{bing_base}/search")),
        )
        .unwrap();
        let req = r2.recv().unwrap();
        assert!(req.line.contains("filters=ex1"), "{}", req.line);
        let d = &out.data;
        assert_eq!(d["backend"], "bing");
        assert_eq!(d["fallback_from"], "ddg");
        assert_eq!(d["results"][0]["url"], "https://rust-lang.org/");
        assert_eq!(
            d["results"][0]["snippet"],
            "Rust is blazingly fast and memory-efficient."
        );
        assert_eq!(d["results"][1]["url"], "https://doc.rust-lang.org/book/");
    }

    #[test]
    fn explicit_ddg_blocked_is_an_error_with_hint() {
        let (ddg_base, _r) = serve(vec![http(
            "202 Accepted",
            "text/html",
            "<div class=\"anomaly-modal\"></div>",
        )]);
        let tmp = tempfile::tempdir().unwrap();
        let a = SearchArgs {
            backend: "ddg".into(),
            ..args("x")
        };
        let e = run(
            &Store::new(tmp.path()),
            &a,
            None,
            &eps(DEAD, &ddg_base, DEAD),
        )
        .unwrap_err();
        assert_eq!(e.code, "blocked");
        assert!(e.hint.contains("--backend bing") && !e.hint.contains("--backend ddg"));
    }

    #[test]
    fn site_filters_are_enforced_client_side() {
        let hits = vec![
            Hit {
                title: "a".into(),
                url: "https://digital-strategy.ec.europa.eu/x".into(),
                ..Hit::default()
            },
            Hit {
                title: "b".into(),
                url: "https://en.wikipedia.org/wiki/EU".into(),
                ..Hit::default()
            },
            Hit {
                title: "c".into(),
                url: "https://noteuropa.eu/".into(),
                ..Hit::default()
            },
            Hit {
                title: "d".into(),
                url: "https://europa.eu/spam".into(),
                ..Hit::default()
            },
        ];
        let o = Opts {
            max_results: 10,
            sites: vec!["europa.eu".into()],
            exclude_sites: vec!["europa.eu/spam".into()],
            ..Opts::default()
        };
        let kept: Vec<String> = finalize(hits.clone(), &o)
            .into_iter()
            .map(|h| h.title)
            .collect();
        assert_eq!(kept, vec!["a", "d"]);
        let o = Opts {
            max_results: 10,
            exclude_sites: vec!["wikipedia.org".into()],
            ..Opts::default()
        };
        assert_eq!(finalize(hits, &o).len(), 3);
    }

    #[test]
    fn helpers() {
        assert_eq!(
            base64url_decode("aHR0cHM6Ly9ydXN0LWxhbmcub3JnLw").unwrap(),
            b"https://rust-lang.org/"
        );
        assert_eq!(
            bing_target("https://example.com/a").as_deref(),
            Some("https://example.com/a")
        );
        assert_eq!(
            ddg_target("//duckduckgo.com/l/?uddg=https%3A%2F%2Fa.test%2Fx").as_deref(),
            Some("https://a.test/x")
        );
        assert_eq!(
            normalize_url("https://WWW.A.test/x/?utm_source=z#f"),
            normalize_url("http://a.test/x")
        );
        assert_ne!(
            normalize_url("https://a.test/x?id=1"),
            normalize_url("https://a.test/x?id=2")
        );
        let t = trim_snippet(&"abc ".repeat(100), 20);
        assert!(t.ends_with('…') && t.chars().count() <= 21, "{t}");
        assert_eq!(
            redact("key=tvly-abc123", Some("tvly-abc123")),
            "key=[redacted]"
        );
    }

    #[test]
    fn caps_and_validation() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        let ep = eps(DEAD, DEAD, DEAD);
        assert_eq!(
            run(&store, &args("  "), None, &ep).unwrap_err().code,
            "bad_args"
        );
        let a = SearchArgs {
            time: Some("decade".into()),
            ..args("x")
        };
        assert_eq!(run(&store, &a, None, &ep).unwrap_err().code, "bad_args");
        let a = SearchArgs {
            backend: "google".into(),
            ..args("x")
        };
        assert_eq!(run(&store, &a, None, &ep).unwrap_err().code, "bad_args");
    }
}
