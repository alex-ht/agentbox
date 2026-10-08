//! `quote get|history|search`: stock, index, FX and crypto prices.
//!
//! Keyless by default: Yahoo Finance's chart and search endpoints, with Stooq
//! as the fallback for latest quotes. Stooq's history download needs a free
//! API key (env `STOOQ_API_KEY` or `config set stooq.api_key -`); like the
//! Tavily key it is never accepted as a flag and is scrubbed from output.

mod aliases;
#[cfg(test)]
mod tests;

use crate::envelope::{AppError, CmdResult, Output};
use crate::net::{self, encode, jround, snippet, Fail};
use crate::state::Store;
use crate::table::{self, parse::Parsed, Table};
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, TimeZone, Utc};
use serde_json::{json, Map, Value};

pub const NOTE: &str = "Quotes may be delayed (often ~15 min) and are for research, not trading.";
const MAX_SYMBOLS: usize = 10;
const ROWS_SHOWN: usize = 30;
const TIMEOUT_SECS: u64 = 20;

/// Service base URLs; overridable via env for tests and proxies.
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// Yahoo hosts tried in order (the second one absorbs per-host 429s).
    pub yahoo: Vec<String>,
    pub stooq: String,
}

impl Endpoints {
    pub fn from_env() -> Self {
        let yahoo = match std::env::var("AGENTBOX_YAHOO_URL") {
            Ok(v) if !v.trim().is_empty() => vec![v.trim().trim_end_matches('/').to_string()],
            _ => vec![
                "https://query1.finance.yahoo.com".to_string(),
                "https://query2.finance.yahoo.com".to_string(),
            ],
        };
        Self {
            yahoo,
            stooq: net::env_or("AGENTBOX_STOOQ_URL", "https://stooq.com"),
        }
    }
}

/// Everything a quote request needs besides its arguments.
pub struct Ctx<'a> {
    pub ep: &'a Endpoints,
    pub stooq_key: Option<&'a str>,
    /// Current time (unix seconds), injectable for tests.
    pub now: i64,
    client: reqwest::blocking::Client,
}

impl<'a> Ctx<'a> {
    pub fn new(ep: &'a Endpoints, stooq_key: Option<&'a str>) -> Self {
        Self {
            ep,
            stooq_key,
            now: Utc::now().timestamp(),
            client: net::client(TIMEOUT_SECS),
        }
    }

    fn redact(&self, s: &str) -> String {
        crate::cmd::search::redact(s, self.stooq_key)
    }

    /// GET a Yahoo path, moving to the next host on 429/5xx/network errors.
    fn yahoo_get(&self, path_query: &str) -> Result<String, Fail> {
        let mut last = None;
        for base in &self.ep.yahoo {
            match net::get(&self.client, &format!("{base}{path_query}")) {
                Ok(b) => return Ok(b),
                Err(Fail::Status(s, b)) if s == 429 || s >= 500 => last = Some(Fail::Status(s, b)),
                Err(f @ Fail::Net { .. }) => last = Some(f),
                Err(f) => return Err(f),
            }
        }
        Err(last.unwrap_or(Fail::Status(0, String::new())))
    }
}

/// A per-symbol failure, mapped to an envelope error at the end.
#[derive(Debug, Clone, PartialEq)]
enum QErr {
    NotFound(String),
    NoData(String),
    Limited(String),
    Net(String),
    Http(String),
    NeedsKey(String),
    Unsupported(String),
    Ambiguous(String),
}

impl QErr {
    /// Errors worth retrying on the other backend.
    fn transient(&self) -> bool {
        matches!(self, QErr::Limited(_) | QErr::Net(_) | QErr::Http(_))
    }

    fn message(&self) -> &str {
        match self {
            QErr::NotFound(m)
            | QErr::NoData(m)
            | QErr::Limited(m)
            | QErr::Net(m)
            | QErr::Http(m)
            | QErr::NeedsKey(m)
            | QErr::Unsupported(m)
            | QErr::Ambiguous(m) => m,
        }
    }

    fn into_app(self, sym: &str) -> AppError {
        let (code, hint) = match &self {
            QErr::NotFound(_) => (
                "symbol_not_found",
                format!("Look the ticker up with `agentbox quote search \"{}\"` (company name works too).", sym),
            ),
            QErr::NoData(_) => (
                "no_data",
                format!("Yahoo knows `{sym}` but has no price for it. Check the exchange suffix with `agentbox quote search \"{sym}\"`."),
            ),
            QErr::Limited(_) => (
                "rate_limited",
                "Yahoo is rate-limiting or blocking this network. Wait a minute, or try `--backend stooq` (latest quotes only).".to_string(),
            ),
            QErr::Net(m) => (
                "network_error",
                if m.contains("fallback") {
                    "Both Yahoo and Stooq failed from this network; check connectivity (or a proxy/firewall) and retry.".to_string()
                } else if m.starts_with("Stooq") {
                    "Check network access to stooq.com, or use `--backend yahoo`.".to_string()
                } else {
                    "Check network access to finance.yahoo.com, or try `--backend stooq`.".to_string()
                },
            ),
            QErr::Http(_) => (
                "http_error",
                "The data service had a problem; retry later or try the other `--backend`.".to_string(),
            ),
            QErr::NeedsKey(_) => (
                "stooq_needs_key",
                "Stooq now wants a free API key: get one at https://stooq.com/q/d/?s=spy.us&get_apikey, then set env STOOQ_API_KEY or run `agentbox config set stooq.api_key -` (reads stdin). Or use `--backend yahoo`.".to_string(),
            ),
            QErr::Unsupported(_) => (
                "unsupported_symbol",
                "Stooq does not list this market (e.g. Taiwan); use `--backend yahoo`.".to_string(),
            ),
            QErr::Ambiguous(_) => ("ambiguous_symbol", tw_hint(sym)),
        };
        AppError::new(code, self.message().to_string(), hint)
    }
}

fn tw_hint(num: &str) -> String {
    format!(
        "Add the exchange suffix: `{num}.TW` (TWSE) or `{num}.TWO` (TPEx, OTC) for Taiwan, `{num}.HK` for Hong Kong, `{num}.T` for Tokyo. Example: `agentbox quote get {num}.TW`."
    )
}

/// "2330" or "6488": a bare listing number that needs an exchange suffix.
pub fn is_bare_number(sym: &str) -> bool {
    (4..=6).contains(&sym.len()) && sym.bytes().all(|b| b.is_ascii_digit())
}

/// Split "NVDA, 2330.TW ^TWII" into normalized, deduped symbols.
pub fn split_symbols(raw: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in raw.iter().flat_map(|s| s.split([',', ' ', ';'])) {
        let s = part.trim().to_uppercase();
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

fn iso(ts: i64, offset: i64) -> Option<String> {
    let off = FixedOffset::east_opt(offset as i32)?;
    let t = DateTime::from_timestamp(ts, 0)?.with_timezone(&off);
    Some(t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn local_date(ts: i64, offset: i64) -> Option<String> {
    let off = FixedOffset::east_opt(offset as i32)?;
    Some(
        DateTime::from_timestamp(ts, 0)?
            .with_timezone(&off)
            .format("%Y-%m-%d")
            .to_string(),
    )
}

/// pre / regular / post / closed from Yahoo's `currentTradingPeriod`.
fn market_state(meta: &Value, now: i64) -> Option<&'static str> {
    let p = meta.get("currentTradingPeriod")?;
    let within = |k: &str| {
        let s = p.pointer(&format!("/{k}/start")).and_then(Value::as_i64);
        let e = p.pointer(&format!("/{k}/end")).and_then(Value::as_i64);
        matches!((s, e), (Some(s), Some(e)) if s <= now && now < e)
    };
    p.get("regular")?;
    Some(if within("regular") {
        "regular"
    } else if within("pre") {
        "pre"
    } else if within("post") {
        "post"
    } else {
        "closed"
    })
}

fn digits(meta: &Value) -> u32 {
    meta.get("priceHint")
        .and_then(Value::as_u64)
        .unwrap_or(2)
        .clamp(2, 6) as u32
}

fn num(v: Option<&Value>, d: u32) -> Value {
    v.and_then(Value::as_f64)
        .map_or(Value::Null, |x| jround(x, d))
}

// ------------------------------------------------------------------ Yahoo

fn yahoo_chart(ctx: &Ctx, sym: &str, range: &str, interval: &str) -> Result<Value, QErr> {
    let pq = format!(
        "/v8/finance/chart/{}?range={range}&interval={interval}",
        encode(sym)
    );
    let chart_error = |body: &str| -> Option<String> {
        let v: Value = serde_json::from_str(body).ok()?;
        let e = v.pointer("/chart/error").filter(|e| !e.is_null())?;
        Some(
            e.get("description")
                .and_then(Value::as_str)
                .unwrap_or("symbol not found")
                .to_string(),
        )
    };
    match ctx.yahoo_get(&pq) {
        Ok(body) => {
            if let Some(msg) = chart_error(&body) {
                return Err(QErr::NotFound(format!("Yahoo: `{sym}`: {msg}")));
            }
            let v: Value = serde_json::from_str(&body).map_err(|_| {
                QErr::Http(format!("Yahoo returned non-JSON: {}", snippet(&body, 120)))
            })?;
            v.pointer("/chart/result/0")
                .filter(|r| r.is_object())
                .cloned()
                .ok_or_else(|| QErr::NotFound(format!("Yahoo has no chart data for `{sym}`")))
        }
        Err(Fail::Status(404, body)) => Err(QErr::NotFound(format!(
            "Yahoo: `{sym}`: {}",
            chart_error(&body).unwrap_or_else(|| "symbol not found".into())
        ))),
        Err(Fail::Status(s @ (401 | 403 | 429), _)) => Err(QErr::Limited(format!(
            "Yahoo answered HTTP {s} (rate limit or block)"
        ))),
        Err(Fail::Status(s, body)) => Err(QErr::Http(format!(
            "Yahoo answered HTTP {s}: {}",
            snippet(&body, 120)
        ))),
        Err(Fail::Net { message, .. }) => {
            Err(QErr::Net(format!("Yahoo request failed: {message}")))
        }
    }
}

fn yahoo_quote(ctx: &Ctx, sym: &str) -> Result<Map<String, Value>, QErr> {
    let r = yahoo_chart(ctx, sym, "1d", "1d")?;
    quote_from_meta(&r["meta"], sym, ctx.now)
}

fn quote_from_meta(meta: &Value, sym: &str, now: i64) -> Result<Map<String, Value>, QErr> {
    let Some(price) = meta.get("regularMarketPrice").and_then(Value::as_f64) else {
        return Err(QErr::NoData(format!(
            "Yahoo has no current price for `{sym}`{}",
            meta.get("fullExchangeName")
                .and_then(Value::as_str)
                .map(|x| format!(" (listed on {x})"))
                .unwrap_or_default()
        )));
    };
    let d = digits(meta);
    let prev = meta
        .get("previousClose")
        .or_else(|| meta.get("chartPreviousClose"))
        .and_then(Value::as_f64);
    let offset = meta.get("gmtoffset").and_then(Value::as_i64).unwrap_or(0);
    let s = |k: &str| meta.get(k).and_then(Value::as_str).map(str::to_string);
    let mut m = Map::new();
    m.insert(
        "symbol".into(),
        json!(s("symbol").unwrap_or_else(|| sym.to_string())),
    );
    m.insert(
        "name".into(),
        json!(s("longName").or_else(|| s("shortName"))),
    );
    m.insert("price".into(), jround(price, d));
    m.insert("currency".into(), json!(s("currency")));
    match prev.filter(|p| *p != 0.0) {
        Some(p) => {
            m.insert("change".into(), jround(price - p, d));
            m.insert("change_pct".into(), jround((price / p - 1.0) * 100.0, 2));
            m.insert("previous_close".into(), jround(p, d));
        }
        None => {
            m.insert("change".into(), Value::Null);
            m.insert("change_pct".into(), Value::Null);
            m.insert("previous_close".into(), Value::Null);
        }
    }
    m.insert("day_high".into(), num(meta.get("regularMarketDayHigh"), d));
    m.insert("day_low".into(), num(meta.get("regularMarketDayLow"), d));
    // Indices and FX report a meaningless volume of 0.
    m.insert(
        "volume".into(),
        json!(meta
            .get("regularMarketVolume")
            .and_then(Value::as_u64)
            .filter(|v| *v > 0)),
    );
    m.insert("week52_high".into(), num(meta.get("fiftyTwoWeekHigh"), d));
    m.insert("week52_low".into(), num(meta.get("fiftyTwoWeekLow"), d));
    m.insert(
        "exchange".into(),
        json!(s("fullExchangeName").or_else(|| s("exchangeName"))),
    );
    m.insert(
        "type".into(),
        json!(s("instrumentType").map(|t| t.to_lowercase())),
    );
    m.insert("market_state".into(), json!(market_state(meta, now)));
    m.insert(
        "time".into(),
        json!(meta
            .get("regularMarketTime")
            .and_then(Value::as_i64)
            .and_then(|t| iso(t, offset))),
    );
    m.insert("timezone".into(), json!(s("exchangeTimezoneName")));
    m.insert("backend".into(), json!("yahoo"));
    Ok(m)
}

// ------------------------------------------------------------------ Stooq

/// Map a Yahoo-style symbol to Stooq's naming. `None` when Stooq has no
/// equivalent market (Taiwan).
pub fn to_stooq(sym: &str) -> Option<String> {
    let s = sym.to_lowercase();
    if s.ends_with(".tw") || s.ends_with(".two") {
        return None;
    }
    if let Some(idx) = s.strip_prefix('^') {
        let mapped = match idx {
            "gspc" => "spx",
            "ixic" => "ndq",
            "n225" => "nkx",
            "ftse" => "ukx",
            "gdaxi" => "dax",
            other => other,
        };
        return Some(format!("^{mapped}"));
    }
    if let Some(pair) = s.strip_suffix("=x") {
        return Some(pair.to_string());
    }
    if let Some((base, quote)) = s.split_once('-') {
        if ["usd", "eur", "usdt"].contains(&quote) && base.len() <= 5 && !base.contains('.') {
            return Some(format!("{base}{quote}"));
        }
    }
    if let Some((code, suffix)) = s.rsplit_once('.') {
        let market = match suffix {
            "hk" => return Some(format!("{}.hk", code.trim_start_matches('0'))),
            "t" => "jp",
            "l" => "uk",
            "de" | "f" => "de",
            "us" => "us",
            // Share classes such as BRK.B stay US listings.
            x if x.len() == 1 => return Some(format!("{s}.us")),
            other => other,
        };
        return Some(format!("{code}.{market}"));
    }
    Some(format!("{s}.us"))
}

fn stooq_get(ctx: &Ctx, path_query: &str) -> Result<String, QErr> {
    let key = ctx
        .stooq_key
        .map(|k| format!("&apikey={}", encode(k)))
        .unwrap_or_default();
    let url = format!("{}{path_query}{key}", ctx.ep.stooq);
    let body = match net::get(&ctx.client, &url) {
        Ok(b) => b,
        Err(Fail::Status(s @ (401 | 403 | 429), _)) => {
            return Err(QErr::Limited(format!("Stooq answered HTTP {s}")))
        }
        Err(Fail::Status(s, b)) => {
            return Err(QErr::Http(
                ctx.redact(&format!("Stooq answered HTTP {s}: {}", snippet(&b, 120))),
            ))
        }
        Err(Fail::Net { message, .. }) => {
            return Err(QErr::Net(
                ctx.redact(&format!("Stooq request failed: {message}")),
            ))
        }
    };
    let lower = body.to_lowercase();
    if lower.contains("apikey") || lower.contains("captcha") {
        return Err(QErr::NeedsKey(if ctx.stooq_key.is_some() {
            "Stooq rejected the configured API key (or its daily quota is used up)".into()
        } else {
            "Stooq asked for an API key instead of returning data".into()
        }));
    }
    if lower.contains("exceeded the daily hits limit") {
        return Err(QErr::Limited("Stooq daily request limit reached".into()));
    }
    Ok(body)
}

fn stooq_quote(ctx: &Ctx, sym: &str) -> Result<Map<String, Value>, QErr> {
    let s = to_stooq(sym)
        .ok_or_else(|| QErr::Unsupported(format!("Stooq has no listing for `{sym}`")))?;
    let body = stooq_get(
        ctx,
        &format!("/q/l/?s={}&f=sd2t2ohlcvn&h&e=csv", encode(&s)),
    )?;
    let p = table::parse::parse_csv(&body, Some(','))
        .map_err(|e| QErr::Http(format!("Stooq returned unreadable CSV: {e}")))?;
    let col = |name: &str| p.columns.iter().position(|c| c.eq_ignore_ascii_case(name));
    let row = p
        .rows
        .first()
        .ok_or_else(|| QErr::NotFound(format!("Stooq: no row for `{s}`")))?;
    let cell = |name: &str| -> Option<String> {
        col(name)
            .and_then(|i| row.get(i))
            .and_then(Value::as_str)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty() && v != "N/D")
    };
    let f = |name: &str| cell(name).and_then(|v| v.parse::<f64>().ok());
    let price =
        f("close").ok_or_else(|| QErr::NotFound(format!("Stooq: `{s}` (from {sym}) not found")))?;
    let time = match (cell("date"), cell("time")) {
        (Some(d), Some(t)) => Some(format!("{d} {t}")),
        (Some(d), None) => Some(d),
        _ => None,
    };
    let mut m = Map::new();
    m.insert("symbol".into(), json!(sym));
    m.insert("name".into(), json!(cell("name")));
    m.insert("price".into(), jround(price, 6));
    m.insert("currency".into(), Value::Null);
    m.insert("change".into(), Value::Null);
    m.insert("change_pct".into(), Value::Null);
    m.insert("previous_close".into(), Value::Null);
    m.insert(
        "open".into(),
        f("open").map_or(Value::Null, |x| jround(x, 6)),
    );
    m.insert(
        "day_high".into(),
        f("high").map_or(Value::Null, |x| jround(x, 6)),
    );
    m.insert(
        "day_low".into(),
        f("low").map_or(Value::Null, |x| jround(x, 6)),
    );
    m.insert("volume".into(), json!(f("volume").map(|x| x as u64)));
    m.insert("time".into(), json!(time));
    m.insert("stooq_symbol".into(), json!(s));
    m.insert("backend".into(), json!("stooq"));
    Ok(m)
}

// -------------------------------------------------------------- quote get

#[derive(Debug, Clone)]
pub struct GetArgs {
    pub symbols: Vec<String>,
    pub backend: String,
}

pub fn run_get(ctx: &Ctx, a: &GetArgs) -> CmdResult {
    let syms = split_symbols(&a.symbols);
    if syms.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "no symbol given",
            "Example: `agentbox quote get NVDA 2330.TW ^TWII`.",
        ));
    }
    if syms.len() > MAX_SYMBOLS {
        return Err(AppError::new(
            "bad_args",
            format!(
                "{} symbols given; at most {MAX_SYMBOLS} per call",
                syms.len()
            ),
            "Split the symbols over several calls.",
        ));
    }
    let mut quotes = Vec::new();
    let mut errors: Vec<(String, AppError)> = Vec::new();
    // After one transient Yahoo failure, go straight to Stooq.
    let mut yahoo_down: Option<QErr> = None;
    for sym in &syms {
        let res = if is_bare_number(sym) {
            Err(QErr::Ambiguous(format!(
                "`{sym}` needs an exchange suffix (bare numbers match the wrong market)"
            )))
        } else {
            match a.backend.as_str() {
                "yahoo" => yahoo_quote(ctx, sym),
                "stooq" => stooq_quote(ctx, sym),
                _ => match yahoo_down
                    .clone()
                    .map_or_else(|| yahoo_quote(ctx, sym), Err)
                {
                    Err(e) if e.transient() => {
                        yahoo_down = Some(e.clone());
                        stooq_quote(ctx, sym).map_err(|s| {
                            let code_from = if matches!(s, QErr::NotFound(_) | QErr::Unsupported(_))
                            {
                                e.clone()
                            } else {
                                s.clone()
                            };
                            let msg = format!("{}; fallback: {}", e.message(), s.message());
                            match code_from {
                                QErr::Limited(_) => QErr::Limited(msg),
                                QErr::NeedsKey(_) => QErr::NeedsKey(msg),
                                QErr::Http(_) => QErr::Http(msg),
                                _ => QErr::Net(msg),
                            }
                        })
                    }
                    other => other,
                },
            }
        };
        match res {
            Ok(q) => quotes.push(Value::Object(q)),
            Err(e) => errors.push((sym.clone(), e.into_app(sym))),
        }
    }
    if quotes.is_empty() {
        if errors.len() == 1 {
            return Err(errors.remove(0).1);
        }
        let msg: Vec<String> = errors
            .iter()
            .map(|(s, e)| format!("{s}: {}", e.message))
            .collect();
        let hint = errors[0].1.hint.clone();
        return Err(AppError::new("quote_failed", msg.join("; "), hint));
    }
    let backends: Vec<&str> = {
        let mut b: Vec<&str> = quotes
            .iter()
            .filter_map(|q| q["backend"].as_str())
            .collect();
        b.dedup();
        b
    };
    let mut data = json!({
        "quotes": quotes,
        "backend": backends.join(","),
        "note": NOTE,
    });
    let hint = if errors.is_empty() {
        let first = quotes[0]["symbol"].as_str().unwrap_or("NVDA").to_string();
        let mut h = format!("Price history: `agentbox quote history {first} --range 1mo`.");
        if backends.contains(&"stooq") {
            h.push_str(" Stooq quotes have no previous close, so change is null; `time` is Stooq's local time.");
        }
        h
    } else {
        data["errors"] = json!(errors
            .iter()
            .map(|(s, e)| json!({"symbol": s, "code": e.code, "message": e.message}))
            .collect::<Vec<_>>());
        let parts: Vec<String> = errors
            .iter()
            .map(|(s, e)| format!("{s}: {}", e.hint))
            .collect();
        parts.join(" ")
    };
    Ok(Output::new(data).hint(hint))
}

// ---------------------------------------------------------- quote history

#[derive(Debug, Clone)]
pub struct HistoryArgs {
    pub symbol: String,
    pub range: String,
    pub interval: Option<String>,
    pub backend: String,
    pub save: bool,
}

pub fn default_interval(range: &str) -> &'static str {
    match range {
        "max" => "1mo",
        "5y" => "1wk",
        _ => "1d",
    }
}

pub const HISTORY_COLUMNS: &[&str] = &["date", "open", "high", "low", "close", "volume"];

struct Series {
    name: Option<String>,
    currency: Option<String>,
    rows: Vec<Vec<Value>>,
    backend: &'static str,
}

fn yahoo_history(ctx: &Ctx, sym: &str, range: &str, interval: &str) -> Result<Series, QErr> {
    let r = yahoo_chart(ctx, sym, range, interval)?;
    let meta = &r["meta"];
    let d = digits(meta);
    let offset = meta.get("gmtoffset").and_then(Value::as_i64).unwrap_or(0);
    let ts = r["timestamp"].as_array().cloned().unwrap_or_default();
    let q = &r["indicators"]["quote"][0];
    let at = |k: &str, i: usize| q.get(k).and_then(|a| a.get(i)).and_then(Value::as_f64);
    let mut rows = Vec::new();
    for (i, t) in ts.iter().enumerate() {
        let (Some(t), Some(close)) = (t.as_i64(), at("close", i)) else {
            continue;
        };
        let Some(date) = local_date(t, offset) else {
            continue;
        };
        let r = |x: Option<f64>| x.map_or(Value::Null, |x| jround(x, d));
        rows.push(vec![
            json!(date),
            r(at("open", i)),
            r(at("high", i)),
            r(at("low", i)),
            jround(close, d),
            at("volume", i).map_or(Value::Null, |v| json!(v as u64)),
        ]);
    }
    // Yahoo can repeat the live bar; keep the last row per date.
    rows.dedup_by(|b, a| {
        if a[0] == b[0] {
            *a = b.clone();
            true
        } else {
            false
        }
    });
    let s = |k: &str| meta.get(k).and_then(Value::as_str).map(str::to_string);
    Ok(Series {
        name: s("longName").or_else(|| s("shortName")),
        currency: s("currency"),
        rows,
        backend: "yahoo",
    })
}

/// First day covered by a range, counted back from `today`.
fn range_start(range: &str, today: NaiveDate) -> NaiveDate {
    let months_back = |m: u32| {
        today
            .checked_sub_months(chrono::Months::new(m))
            .unwrap_or(today)
    };
    match range {
        "5d" => today - chrono::Duration::days(7),
        "1mo" => months_back(1),
        "3mo" => months_back(3),
        "6mo" => months_back(6),
        "ytd" => NaiveDate::from_ymd_opt(today.year(), 1, 1).unwrap_or(today),
        "1y" => months_back(12),
        "5y" => months_back(60),
        _ => NaiveDate::from_ymd_opt(1970, 1, 1).unwrap_or(today),
    }
}

fn stooq_history(ctx: &Ctx, sym: &str, range: &str, interval: &str) -> Result<Series, QErr> {
    if ctx.stooq_key.is_none() {
        return Err(QErr::NeedsKey(
            "Stooq price history needs an API key (none configured)".into(),
        ));
    }
    let s = to_stooq(sym)
        .ok_or_else(|| QErr::Unsupported(format!("Stooq has no listing for `{sym}`")))?;
    let today = Utc
        .timestamp_opt(ctx.now, 0)
        .single()
        .unwrap_or_else(Utc::now)
        .date_naive();
    let i = match interval {
        "1wk" => "w",
        "1mo" => "m",
        _ => "d",
    };
    let body = stooq_get(
        ctx,
        &format!(
            "/q/d/l/?s={}&d1={}&d2={}&i={i}",
            encode(&s),
            range_start(range, today).format("%Y%m%d"),
            today.format("%Y%m%d")
        ),
    )?;
    let p = table::parse::parse_csv(&body, Some(','))
        .map_err(|e| QErr::Http(format!("Stooq returned unreadable CSV: {e}")))?;
    let idx = |n: &str| p.columns.iter().position(|c| c.eq_ignore_ascii_case(n));
    let (Some(di), Some(ci)) = (idx("date"), idx("close")) else {
        return Err(QErr::NotFound(format!(
            "Stooq: no history for `{s}` (from {sym})"
        )));
    };
    let f = |row: &Vec<Value>, n: &str| {
        idx(n)
            .and_then(|i| row.get(i))
            .and_then(Value::as_str)
            .and_then(|v| v.trim().parse::<f64>().ok())
    };
    let rows = p
        .rows
        .iter()
        .filter_map(|row| {
            let close = row
                .get(ci)
                .and_then(Value::as_str)?
                .trim()
                .parse::<f64>()
                .ok()?;
            let r = |x: Option<f64>| x.map_or(Value::Null, |x| jround(x, 6));
            Some(vec![
                row.get(di).cloned().unwrap_or(Value::Null),
                r(f(row, "open")),
                r(f(row, "high")),
                r(f(row, "low")),
                jround(close, 6),
                f(row, "volume").map_or(Value::Null, |v| json!(v as u64)),
            ])
        })
        .collect();
    Ok(Series {
        name: None,
        currency: None,
        rows,
        backend: "stooq",
    })
}

fn summary(rows: &[Vec<Value>]) -> Value {
    let f = |r: &Vec<Value>, i: usize| r.get(i).and_then(Value::as_f64);
    let first = &rows[0];
    let last = &rows[rows.len() - 1];
    let (start, end) = (f(first, 4).unwrap_or(0.0), f(last, 4).unwrap_or(0.0));
    let mut hi: Option<(f64, &Value)> = None;
    let mut lo: Option<(f64, &Value)> = None;
    for r in rows {
        let h = f(r, 2).or(f(r, 4));
        let l = f(r, 3).or(f(r, 4));
        if let Some(h) = h {
            if hi.is_none_or(|(x, _)| h > x) {
                hi = Some((h, &r[0]));
            }
        }
        if let Some(l) = l {
            if lo.is_none_or(|(x, _)| l < x) {
                lo = Some((l, &r[0]));
            }
        }
    }
    let d = 6;
    json!({
        "start_date": first[0],
        "start_close": jround(start, 6),
        "end_date": last[0],
        "end_close": jround(end, 6),
        "change": jround(end - start, d),
        "change_pct": if start != 0.0 { jround((end / start - 1.0) * 100.0, 2) } else { Value::Null },
        "high": hi.map_or(Value::Null, |(x, _)| jround(x, 6)),
        "high_date": hi.map(|(_, d)| d.clone()),
        "low": lo.map_or(Value::Null, |(x, _)| jround(x, 6)),
        "low_date": lo.map(|(_, d)| d.clone()),
        "points": rows.len(),
    })
}

pub fn run_history(store: &Store, ctx: &Ctx, a: &HistoryArgs) -> CmdResult {
    let sym = a.symbol.trim().to_uppercase();
    if sym.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "no symbol given",
            "Example: `agentbox quote history NVDA --range 6mo`.",
        ));
    }
    if is_bare_number(&sym) {
        return Err(QErr::Ambiguous(format!(
            "`{sym}` needs an exchange suffix (bare numbers match the wrong market)"
        ))
        .into_app(&sym));
    }
    let interval = a
        .interval
        .clone()
        .unwrap_or_else(|| default_interval(&a.range).to_string());
    let res = match a.backend.as_str() {
        "yahoo" => yahoo_history(ctx, &sym, &a.range, &interval),
        "stooq" => stooq_history(ctx, &sym, &a.range, &interval),
        _ => match yahoo_history(ctx, &sym, &a.range, &interval) {
            Err(e) if e.transient() && ctx.stooq_key.is_some() => {
                stooq_history(ctx, &sym, &a.range, &interval)
                    .map_err(|s| QErr::Net(format!("{}; fallback: {}", e.message(), s.message())))
            }
            other => other,
        },
    };
    let series = res.map_err(|e| e.into_app(&sym))?;
    if series.rows.is_empty() {
        return Err(AppError::new(
            "no_data",
            format!("no price rows for `{sym}` over {}", a.range),
            "Try a longer --range, or check the symbol with `agentbox quote search`.",
        ));
    }
    let columns: Vec<String> = HISTORY_COLUMNS.iter().map(|s| s.to_string()).collect();
    let total = series.rows.len();
    let shown_from = total.saturating_sub(ROWS_SHOWN);
    let to_obj = |r: &Vec<Value>| {
        let mut m = Map::new();
        for (c, v) in columns.iter().zip(r) {
            m.insert(c.clone(), v.clone());
        }
        Value::Object(m)
    };
    let mut data = json!({
        "symbol": sym,
        "name": series.name,
        "currency": series.currency,
        "range": a.range,
        "interval": interval,
        "backend": series.backend,
        "summary": summary(&series.rows),
        "columns": columns,
        "rows": series.rows[shown_from..].iter().map(to_obj).collect::<Vec<_>>(),
        "rows_total": total,
    });
    let mut hint = String::new();
    if shown_from > 0 {
        data["truncated"] = json!(true);
        hint.push_str(&format!(
            "Showing the last {ROWS_SHOWN} of {total} rows; the summary covers all of them. "
        ));
    }
    if a.save {
        let title = format!("{sym} {} {}", a.range, interval);
        let t = Table::new(
            &title,
            &format!("quote:{sym}"),
            Parsed {
                columns: columns.clone(),
                rows: series.rows.clone(),
            },
        );
        let id = table::save(store, t)?;
        data["table"] = json!(format!("tbl:{id}"));
        hint.push_str(&format!(
            "All {total} rows saved as tbl:{id}; e.g. `agentbox table query tbl:{id} --sort -volume --limit 5`."
        ));
    } else {
        hint.push_str("Add --save to keep all rows as a tbl:N for `table query`.");
    }
    data["note"] = json!(NOTE);
    Ok(Output::new(data).hint(hint))
}

// ----------------------------------------------------------- quote search

#[derive(Debug, Clone)]
pub struct SearchArgs {
    pub query: String,
    pub limit: usize,
}

pub fn run_search(ctx: &Ctx, a: &SearchArgs) -> CmdResult {
    let q = a.query.trim();
    if q.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "empty search query",
            "Example: `agentbox quote search \"Taiwan Semiconductor\"`.",
        ));
    }
    let limit = a.limit.clamp(1, 25);
    let mut results: Vec<Value> = aliases::lookup(q)
        .into_iter()
        .map(|al| {
            json!({"symbol": al.symbol, "name": al.name, "exchange": al.exchange, "type": al.kind, "source": "alias"})
        })
        .collect();
    let pq = format!(
        "/v1/finance/search?q={}&quotesCount={}&newsCount=0&listsCount=0",
        encode(q),
        limit.max(6)
    );
    let mut yahoo_err: Option<String> = None;
    match ctx.yahoo_get(&pq) {
        Ok(body) => {
            let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            for it in v["quotes"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                let Some(sym) = it.get("symbol").and_then(Value::as_str) else {
                    continue;
                };
                if results.iter().any(|r| r["symbol"] == sym) {
                    continue;
                }
                let s = |k: &str| it.get(k).and_then(Value::as_str);
                results.push(json!({
                    "symbol": sym,
                    "name": s("longname").or(s("shortname")),
                    "exchange": s("exchDisp").or(s("exchange")),
                    "type": s("typeDisp").or(s("quoteType")),
                    "source": "yahoo",
                }));
            }
        }
        Err(Fail::Status(400, body)) => {
            let why = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| {
                    v.pointer("/finance/error/description")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| snippet(&body, 80));
            yahoo_err = Some(format!("Yahoo search rejected the query ({why})"));
        }
        Err(Fail::Status(s, _)) => yahoo_err = Some(format!("Yahoo search answered HTTP {s}")),
        Err(Fail::Net { message, .. }) => {
            yahoo_err = Some(format!("Yahoo search failed: {message}"))
        }
    }
    results.truncate(limit);
    if results.is_empty() {
        let hint = if is_bare_number(q) {
            tw_hint(q)
        } else if !q.is_ascii() {
            "Yahoo search only understands Latin names and tickers; try the English company name (e.g. \"Taiwan Semiconductor\") or the ticker.".to_string()
        } else {
            "Try the full company name or a shorter keyword.".to_string()
        };
        return Err(match yahoo_err {
            Some(m) if m.contains("HTTP 429") || m.contains("failed") => AppError::new(
                "rate_limited",
                m,
                "Yahoo search is unavailable from this network right now; wait a minute and retry.",
            ),
            Some(m) => AppError::new("no_results", m, hint),
            None => AppError::new("no_results", format!("no symbols match \"{q}\""), hint),
        });
    }
    let first = results[0]["symbol"].as_str().unwrap_or("").to_string();
    let mut hint = format!(
        "Next: `agentbox quote get {}`.",
        crate::table::shell_arg(&first)
    );
    if is_bare_number(q) {
        hint = tw_hint(q);
    }
    if let Some(e) = &yahoo_err {
        hint.push_str(&format!(" ({e}; showing built-in aliases only.)"));
    }
    Ok(Output::new(json!({ "query": q, "results": results })).hint(hint))
}
