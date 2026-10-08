//! Cell typing: numbers (incl. currency strings like "$1,299/mo"), dates,
//! and plain text. Shared by `table` and `extract`.

use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

/// Currency symbols and codes with their ISO 4217 code. Order matters:
/// longer prefixes first so `US$` wins over `$`.
pub const CURRENCY_SYMBOLS: &[(&str, &str)] = &[
    ("US$", "USD"),
    ("NT$", "TWD"),
    ("HK$", "HKD"),
    ("AU$", "AUD"),
    ("A$", "AUD"),
    ("CA$", "CAD"),
    ("C$", "CAD"),
    ("NZ$", "NZD"),
    ("S$", "SGD"),
    ("R$", "BRL"),
    ("$", "USD"),
    ("€", "EUR"),
    ("£", "GBP"),
    ("¥", "JPY"),
    ("₩", "KRW"),
    ("₹", "INR"),
    ("元", "CNY"),
    ("円", "JPY"),
];

pub const CURRENCY_CODES: &[&str] = &[
    "USD", "EUR", "GBP", "JPY", "TWD", "NTD", "CNY", "RMB", "HKD", "AUD", "CAD", "SGD", "CHF",
    "INR", "KRW", "BRL", "NZD", "SEK", "NOK", "DKK", "MXN",
];

/// ISO code for a currency symbol or code. Ambiguous symbols (`$`, `¥`, `元`)
/// use the source host's country when known (e.g. `$` on a .ca site is CAD).
pub fn currency_iso(token: &str, host: Option<&str>) -> Option<String> {
    let t = token.trim();
    let upper = t.to_uppercase();
    match upper.as_str() {
        "NTD" => return Some("TWD".into()),
        "RMB" => return Some("CNY".into()),
        "DOLLARS" | "DOLLAR" => return Some(by_host("USD", host)),
        "EUROS" | "EURO" => return Some("EUR".into()),
        _ => {}
    }
    if CURRENCY_CODES.contains(&upper.as_str()) {
        return Some(upper);
    }
    let iso = CURRENCY_SYMBOLS.iter().find(|(s, _)| *s == t)?.1;
    Some(match t {
        "$" => by_host("USD", host),
        "¥" if host_tld(host) == Some("cn") => "CNY".into(),
        "元" => match host_tld(host) {
            Some("tw") => "TWD".into(),
            Some("hk") => "HKD".into(),
            _ => "CNY".into(),
        },
        _ => iso.into(),
    })
}

fn host_tld(host: Option<&str>) -> Option<&str> {
    host.and_then(|h| h.rsplit('.').next())
}

fn by_host(default: &str, host: Option<&str>) -> String {
    match host_tld(host) {
        Some("tw") => "TWD",
        Some("ca") => "CAD",
        Some("au") => "AUD",
        Some("nz") => "NZD",
        Some("sg") => "SGD",
        Some("hk") => "HKD",
        _ => default,
    }
    .into()
}

/// Multiplier for magnitude words and suffixes.
pub fn magnitude(word: &str) -> Option<f64> {
    Some(match word.trim() {
        "k" | "K" | "thousand" | "Thousand" => 1e3,
        "M" | "mn" | "million" | "Million" | "mil" => 1e6,
        "B" | "bn" | "billion" | "Billion" => 1e9,
        "T" | "tn" | "trillion" | "Trillion" => 1e12,
        "萬" | "万" => 1e4,
        "億" | "亿" => 1e8,
        "兆" => 1e12,
        _ => return None,
    })
}

/// Parse a decimal token with `,` thousands separators. A lone comma followed
/// by exactly two digits ("9,99") is treated as a decimal comma.
pub fn parse_decimal(tok: &str) -> Option<f64> {
    let t = tok.trim();
    let commas = t.matches(',').count();
    let s = if commas == 1 && !t.contains('.') && t.split(',').nth(1).is_some_and(|d| d.len() == 2)
    {
        t.replace(',', ".")
    } else {
        t.replace(',', "")
    };
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// A number parsed from a table cell.
#[derive(Debug, Clone, PartialEq)]
pub struct Num {
    pub value: f64,
    pub currency: Option<String>,
    pub percent: bool,
}

static CELL_NUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?P<pre>[^\d+\-−.]*?)\s*(?P<sign>[+\-−])?\s*(?P<num>\d{1,3}(?:,\d{3})+(?:\.\d+)?|\d+(?:[.,]\d+)?)(?P<rest>.*)$",
    )
    .expect("valid regex")
});

static REST_MAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s?(?P<m>thousand|million|billion|trillion|mn|bn|tn|[kKMBT]|萬|万|億|亿|兆)(?:\b|$|[^A-Za-z])")
        .expect("valid regex")
});

/// Parse a cell like "$1,299/mo", "1,200 USD", "12%", "3.2x", "-5", "€9,99".
/// Returns None for text, ranges ("$10-$20") and dates.
static SIZE_UNIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<u>[KMGTP]i?B|kB|[KMGTP]i?b?ytes?)(?:\b|$)").expect("valid regex")
});

/// Data sizes compare in G units so "512 MiB" < "4 GiB" < "1 TB" and a bare
/// `Memory >= 2` still means 2 GB/GiB. GB and GiB are treated alike.
fn size_scale(unit: &str) -> Option<f64> {
    let binary = unit.contains('i');
    let base: f64 = if binary { 1024.0 } else { 1000.0 };
    let exp = match unit.chars().next()? {
        'K' | 'k' => -2,
        'M' => -1,
        'G' => 0,
        'T' => 1,
        'P' => 2,
        _ => return None,
    };
    Some(base.powi(exp))
}

pub fn parse_num(s: &str) -> Option<Num> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let c = CELL_NUM.captures(t)?;
    let pre = c.name("pre").map_or("", |m| m.as_str()).trim();
    let rest = c.name("rest").map_or("", |m| m.as_str());
    if rest.chars().any(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let mut currency = None;
    let pre = pre.trim_start_matches(['~', '≈', '约', '約']).trim();
    if !pre.is_empty() {
        currency = Some(currency_iso(pre, None)?);
    }
    let mut value = parse_decimal(c.name("num")?.as_str())?;
    if matches!(c.name("sign").map(|m| m.as_str()), Some("-" | "−")) {
        value = -value;
    }
    let mut rest = rest;
    if let Some(m) = REST_MAG.captures(rest) {
        let word = m.name("m").map_or("", |x| x.as_str());
        if let Some(mult) = magnitude(word) {
            value *= mult;
            rest = &rest[m.name("m").map_or(0, |x| x.end())..];
        }
    }
    let rest_t = rest.trim();
    if let Some(scale) = SIZE_UNIT.captures(rest_t).and_then(|m| size_scale(&m["u"])) {
        value *= scale;
    }
    let percent = rest_t.starts_with('%') || rest_t.to_lowercase().starts_with("percent");
    if currency.is_none() {
        let first: String = rest_t
            .chars()
            .take_while(|ch| !ch.is_whitespace() && *ch != '/' && *ch != ',')
            .collect();
        if !first.is_empty() {
            currency = currency_iso(&first, None).filter(|_| {
                CURRENCY_CODES.contains(&first.to_uppercase().as_str())
                    || ["€", "元", "円"].contains(&first.as_str())
            });
        }
    }
    Some(Num {
        value,
        currency,
        percent,
    })
}

/// Text of a JSON cell as shown to users.
pub fn cell_str(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        other => other.to_string(),
    }
}

/// Placeholder cells that mean "no value".
pub fn is_blank(s: &str) -> bool {
    matches!(
        s.trim().to_lowercase().as_str(),
        "" | "-" | "—" | "–" | "n/a" | "na" | "null" | "none" | "?"
    )
}

/// Numeric value of a cell, if it has one.
pub fn cell_num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => parse_num(s).map(|n| n.value),
        Value::Bool(_) | Value::Null => None,
        _ => None,
    }
}

/// ISO date (YYYY-MM-DD or YYYY-MM) if the whole cell is a date.
pub fn cell_date(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => crate::extract::dates::parse_whole(s),
        _ => None,
    }
}

/// Inferred column type: number, currency, date or text.
pub fn infer_type<'a>(cells: impl Iterator<Item = &'a Value>) -> &'static str {
    let mut seen = 0usize;
    let mut nums = 0usize;
    let mut cur = 0usize;
    let mut dates = 0usize;
    for v in cells {
        if is_blank(&cell_str(v)) {
            continue;
        }
        seen += 1;
        match v {
            Value::Number(_) => nums += 1,
            Value::String(s) => {
                if let Some(n) = parse_num(s) {
                    nums += 1;
                    if n.currency.is_some() {
                        cur += 1;
                    }
                } else if crate::extract::dates::parse_whole(s).is_some() {
                    dates += 1;
                }
            }
            _ => {}
        }
    }
    // Allow a few odd cells ("Contact us") in an otherwise numeric column.
    if seen == 0 {
        "text"
    } else if nums * 5 >= seen * 4 {
        if cur > 0 {
            "currency"
        } else {
            "number"
        }
    } else if dates * 5 >= seen * 4 {
        "date"
    } else {
        "text"
    }
}

/// A JSON number, integral when possible, rounded to 12 significant digits
/// (so 68.1 * 1e9 prints as 68100000000, not 68099999999.99999).
pub fn num_value(x: f64) -> Value {
    if !x.is_finite() {
        return Value::Null;
    }
    let r: f64 = format!("{x:.11e}").parse().unwrap_or(x);
    if r.fract() == 0.0 && r.abs() < 9e15 {
        Value::from(r as i64)
    } else {
        serde_json::Number::from_f64(r)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}
