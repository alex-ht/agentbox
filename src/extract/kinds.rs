//! Scanners for prices, numbers with units, links and emails.

use crate::table::value::{currency_iso, magnitude, parse_decimal};
use regex::Regex;
use std::sync::LazyLock;

const CODES: &str =
    "USD|EUR|GBP|JPY|TWD|NTD|CNY|RMB|HKD|AUD|CAD|SGD|CHF|INR|KRW|BRL|NZD|SEK|NOK|DKK|MXN";

#[derive(Debug, Clone, PartialEq)]
pub struct PriceHit {
    pub value: f64,
    pub currency: Option<String>,
    pub period: Option<&'static str>,
    pub per: Option<&'static str>,
    pub raw: String,
    pub start: usize,
    pub end: usize,
}

static PRICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?x)
        (?:
          (?P<pre>US\$|NT\$|HK\$|AU\$|A\$|CA\$|C\$|NZ\$|S\$|R\$|\$|€|£|¥|₩|₹|\b(?:{CODES}))
          \s?(?P<num>\d{{1,3}}(?:,\d{{3}})+(?:\.\d+)?|\d+(?:\.\d+|,\d{{2}}\b)?)
          (?:\s?(?P<mag>(?:thousand|million|billion|mn|bn|[kKMB])\b|萬|万|億|亿))?
        |
          (?P<num2>\d{{1,3}}(?:,\d{{3}})+(?:\.\d+)?|\d+(?:\.\d+|,\d{{2}})?)
          \s?(?P<suf>(?:{CODES})\b|€|元|円|(?i:dollars|euros)\b)
        )"
    ))
    .expect("valid price regex")
});

static TAIL_UNIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:/\s*|per\s+|a\s+|an\s+|each\s+|every\s+)(?P<u>user|seat|member|agent|device|host|month|mo|year|yr|annum|week|wk|day|hour|hr|GB|TB|MB)s?\.?(?:\b|$)")
        .expect("valid regex")
});
static TAIL_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*,?\s*(?:billed\s+|paid\s+)?(?P<w>monthly|annually|yearly|per\s+annum|one[- ]time|once|lifetime|weekly|daily)(?:\b|$)")
        .expect("valid regex")
});
static TAIL_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^\s*(?:{CODES})\b")).expect("valid regex"));
static TAIL_RANGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s?(?:[–—-]|to)\s?(?:US\$|\$|€|£)?\d[\d,]*(?:\.\d+)?").expect("valid regex")
});
static TAIL_CJK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:/|每)\s*(?P<c>月|年|人|位|席|小時|天|週|周)").expect("valid regex")
});

fn unit_kind(u: &str) -> (Option<&'static str>, Option<&'static str>) {
    // (period, per)
    match u.to_lowercase().as_str() {
        "month" | "mo" | "monthly" | "月" => (Some("month"), None),
        "year" | "yr" | "annum" | "annually" | "yearly" | "per annum" | "年" => {
            (Some("year"), None)
        }
        "week" | "wk" | "weekly" | "週" | "周" => (Some("week"), None),
        "day" | "daily" | "天" => (Some("day"), None),
        "hour" | "hr" | "小時" => (Some("hour"), None),
        "one-time" | "one time" | "once" | "lifetime" => (Some("one-time"), None),
        "user" | "member" | "agent" | "人" | "位" => (None, Some("user")),
        "seat" | "席" => (None, Some("seat")),
        "device" | "host" => (None, Some("device")),
        "gb" => (None, Some("GB")),
        "tb" => (None, Some("TB")),
        "mb" => (None, Some("MB")),
        _ => (None, None),
    }
}

/// Read "/user/month", "per seat per month", "billed annually", "/月" after a price.
fn read_tail(tail: &str) -> (Option<&'static str>, Option<&'static str>, usize) {
    let (mut period, mut per) = (None, None);
    // "$ 4 USD per user/month": skip a repeated currency code.
    // "$12–19/user/mo": the low end is the value; skip "–19" to read units.
    let mut pos = TAIL_RANGE.find(tail).map_or(0, |m| m.end());
    pos += TAIL_CODE.find(&tail[pos..]).map_or(0, |m| m.end());
    let code_len = pos;
    for _ in 0..4 {
        let rest = &tail[pos..];
        let m = TAIL_UNIT
            .captures(rest)
            .map(|c| (c.get(0).map_or(0, |m| m.end()), c["u"].to_string()))
            .or_else(|| {
                TAIL_CJK
                    .captures(rest)
                    .map(|c| (c.get(0).map_or(0, |m| m.end()), c["c"].to_string()))
            })
            .or_else(|| {
                TAIL_WORD.captures(rest).map(|c| {
                    let w = c["w"].to_lowercase().replace('-', " ");
                    (c.get(0).map_or(0, |m| m.end()), w)
                })
            });
        let Some((len, word)) = m else { break };
        if len == 0 {
            break;
        }
        let (p, u) = unit_kind(&word);
        period = period.or(p);
        per = per.or(u);
        pos += len;
    }
    if pos == code_len && period.is_none() && per.is_none() {
        // Keep the code in `raw` even when no unit follows.
        return (None, None, code_len);
    }
    (period, per, pos)
}

/// Prices in a line of plain text. `host` helps resolve `$`/`¥`/`元`.
pub fn find_prices(text: &str, host: Option<&str>) -> Vec<PriceHit> {
    let mut out = Vec::new();
    for c in PRICE.captures_iter(text) {
        let whole = c.get(0).expect("match");
        // Skip numbers glued to letters/digits on the left (e.g. "A100$").
        if text[..whole.start()]
            .chars()
            .next_back()
            .is_some_and(|ch| ch.is_ascii_alphanumeric())
            && c.name("pre").is_some_and(|p| p.as_str().starts_with('$'))
        {
            continue;
        }
        let (num, cur) = match (c.name("num"), c.name("num2")) {
            (Some(n), _) => (n.as_str(), c.name("pre").map_or("", |m| m.as_str())),
            (None, Some(n)) => (n.as_str(), c.name("suf").map_or("", |m| m.as_str())),
            _ => continue,
        };
        let Some(mut value) = parse_decimal(num) else {
            continue;
        };
        if let Some(m) = c.name("mag").and_then(|m| magnitude(m.as_str())) {
            value *= m;
        }
        let (period, per, used) = read_tail(&text[whole.end()..]);
        let end = whole.end() + used;
        out.push(PriceHit {
            value,
            currency: currency_iso(cur, host),
            period,
            per,
            raw: text[whole.start()..end].trim().to_string(),
            start: whole.start(),
            end,
        });
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct NumberHit {
    pub value: f64,
    pub unit: Option<String>,
    pub raw: String,
    pub start: usize,
    pub end: usize,
}

static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
        (?P<cur>US\$|\$|€|£|¥)?
        (?P<num>\d{1,3}(?:,\d{3})+(?:\.\d+)?|\d+(?:\.\d+)?)
        (?:
           (?P<msuf>[kKMBT])\b
         | \s?(?P<mag>trillion|billion|million|thousand|tn|bn|mn|萬|万|億|亿|兆)
        )?
        (?:\s?(?P<unit>
            %|(?i:percent|per\s?cent)\b|x\b|×
          | [KMGTP]i?B(?:ps)?\b|[KMGT]bps\b|[kMG]?Hz\b
          | ms\b|(?i:seconds?|secs?|minutes?|mins?|hours?|hrs?|days?|weeks?|months?|years?|yrs?)\b
          | km\b|cm\b|mm\b|kg\b|lbs?\b|(?i:miles?)\b
          | [kMGT]?Wh\b|[kMGT]W\b|°[CF]
          | (?i:users|customers|employees|people|downloads|stars|subscribers|members|companies|countries|cores|tokens|pages|parameters|GPUs)\b
        ))?",
    )
    .expect("valid number regex")
});

fn norm_unit(u: &str) -> String {
    let compact: String = u.to_lowercase().split_whitespace().collect();
    let out = match compact.as_str() {
        "percent" | "percent." => "%",
        "×" => "x",
        "sec" | "secs" | "second" | "seconds" => "seconds",
        "min" | "mins" | "minute" | "minutes" => "minutes",
        "hr" | "hrs" | "hour" | "hours" => "hours",
        "day" | "days" => "days",
        "week" | "weeks" => "weeks",
        "month" | "months" => "months",
        "yr" | "yrs" | "year" | "years" => "years",
        "lb" | "lbs" => "lb",
        "mile" | "miles" => "miles",
        "users" | "customers" | "employees" | "people" | "downloads" | "stars" | "subscribers"
        | "members" | "companies" | "countries" | "cores" | "tokens" | "pages" | "parameters"
        | "gpus" => return compact,
        _ => return u.to_string(),
    };
    out.to_string()
}

/// Numbers that carry a unit, percent or magnitude ("12%", "5.7 trillion",
/// "256 GB", "3.2x", "1.2 million users"). Bare numbers are skipped.
pub fn find_numbers(text: &str, host: Option<&str>) -> Vec<NumberHit> {
    let mut out = Vec::new();
    for c in NUMBER.captures_iter(text) {
        let whole = c.get(0).expect("match");
        let (msuf, mag, unit, cur) = (c.name("msuf"), c.name("mag"), c.name("unit"), c.name("cur"));
        if msuf.is_none() && mag.is_none() && unit.is_none() {
            continue;
        }
        let num_start = c.name("num").map_or(whole.start(), |m| m.start());
        // Skip model names and identifiers like "H100" or "v2.5x".
        if cur.is_none()
            && text[..num_start]
                .chars()
                .next_back()
                .is_some_and(|ch| ch.is_alphanumeric() || ch == '.' || ch == '_')
        {
            continue;
        }
        let Some(mut value) = c.name("num").and_then(|m| parse_decimal(m.as_str())) else {
            continue;
        };
        if let Some(m) = msuf.or(mag).and_then(|m| magnitude(m.as_str())) {
            value *= m;
        }
        let unit = unit
            .map(|u| norm_unit(u.as_str()))
            .or_else(|| cur.and_then(|c| currency_iso(c.as_str(), host)));
        out.push(NumberHit {
            value,
            unit,
            raw: whole.as_str().trim().to_string(),
            start: whole.start(),
            end: whole.end(),
        });
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinkHit {
    pub text: String,
    pub url: String,
}

static MD_LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\[(?P<t>[^\]]*)\]\((?P<u>[^)\s]+)(?:\s+"[^"]*")?\)"#).expect("valid regex")
});
static BARE_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"https?://[^\s<>()\[\]"'`]+"#).expect("valid regex"));

/// Links in a raw Markdown line, made absolute against `base` when relative.
pub fn find_links(line: &str, base: Option<&reqwest::Url>) -> Vec<LinkHit> {
    let mut out = Vec::new();
    let mut spans = Vec::new();
    for c in MD_LINK.captures_iter(line) {
        let whole = c.get(0).expect("match");
        spans.push((whole.start(), whole.end()));
        let raw = c["u"].to_string();
        let url = if raw.starts_with("http://") || raw.starts_with("https://") {
            Some(raw)
        } else if raw.starts_with("mailto:")
            || raw.starts_with('#')
            || raw.starts_with("javascript:")
        {
            None
        } else {
            base.and_then(|b| b.join(&raw).ok()).map(|u| u.to_string())
        };
        if let Some(url) = url {
            out.push(LinkHit {
                text: crate::extract::plain(&c["t"]),
                url,
            });
        }
    }
    for m in BARE_URL.find_iter(line) {
        if spans.iter().any(|&(s, e)| m.start() >= s && m.start() < e) {
            continue;
        }
        let url = m.as_str().trim_end_matches(['.', ',', ';', ':', '!', '?']);
        out.push(LinkHit {
            text: String::new(),
            url: url.to_string(),
        });
    }
    out
}

static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}")
        .expect("valid regex")
});

/// Email addresses in a line (image names like logo@2x.png are skipped).
pub fn find_emails(line: &str) -> Vec<(String, usize, usize)> {
    EMAIL
        .find_iter(line)
        .filter(|m| {
            let l = m.as_str().to_lowercase();
            ![".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp"]
                .iter()
                .any(|ext| l.ends_with(ext))
        })
        .map(|m| (m.as_str().to_string(), m.start(), m.end()))
        .collect()
}
