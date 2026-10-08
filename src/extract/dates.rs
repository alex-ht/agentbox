//! Date finding and ISO normalization: ISO, English month names, numeric
//! M/D/Y, and CJK (2026年10月8日, 民國115年10月8日).

use chrono::NaiveDate;
use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq)]
pub struct DateHit {
    /// `YYYY-MM-DD` (precision day) or `YYYY-MM` (precision month).
    pub iso: String,
    pub precision: &'static str,
    /// True for numeric dates like 03/04/2026 where day and month could swap.
    pub ambiguous: bool,
    pub start: usize,
    pub end: usize,
}

const MONTHS: &str = r"(?P<mon>Jan(?:uary)?|Feb(?:ruary)?|Mar(?:ch)?|Apr(?:il)?|May|June?|July?|Aug(?:ust)?|Sep(?:t(?:ember)?)?|Oct(?:ober)?|Nov(?:ember)?|Dec(?:ember)?)";

fn month_num(m: &str) -> Option<u32> {
    let m = m.to_lowercase();
    let n = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|p| m.starts_with(p))?;
    Some(n as u32 + 1)
}

static PATTERNS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    let p = |s: &str| Regex::new(s).expect("valid date regex");
    vec![
        (
            "roc",
            p(r"民國\s*(?P<y>\d{2,3})\s*年\s*(?P<m>\d{1,2})\s*月(?:\s*(?P<d>\d{1,2})\s*[日號号])?"),
        ),
        (
            "cjk",
            p(r"(?P<y>\d{4})\s*年\s*(?P<m>\d{1,2})\s*月(?:\s*(?P<d>\d{1,2})\s*[日號号])?"),
        ),
        (
            "iso",
            p(
                r"\b(?P<y>(?:19|20)\d{2})(?P<s>[-/.])(?P<m>\d{1,2})(?P<s2>[-/.])(?P<d>\d{1,2})(?:T\d{2}:\d{2}(?::\d{2})?(?:\.\d+)?(?:Z|[+-]\d{2}:?\d{2})?)?\b",
            ),
        ),
        (
            "mdy",
            p(&format!(
                r"\b{MONTHS}\.?\s+(?P<d>\d{{1,2}})(?:st|nd|rd|th)?,?\s+(?P<y>\d{{4}})\b"
            )),
        ),
        (
            "dmy",
            p(&format!(
                r"\b(?P<d>\d{{1,2}})(?:st|nd|rd|th)?\s+(?:of\s+)?{MONTHS}\.?,?\s+(?P<y>\d{{4}})\b"
            )),
        ),
        (
            "num",
            p(r"\b(?P<a>\d{1,2})/(?P<b>\d{1,2})/(?P<y>(?:19|20)\d{2})\b"),
        ),
        (
            "my",
            p(&format!(r"\b{MONTHS}\.?,?\s+(?P<y>(?:19|20)\d{{2}})\b")),
        ),
    ]
});

fn num(c: &regex::Captures, k: &str) -> Option<u32> {
    c.name(k)?.as_str().parse().ok()
}

fn make(y: i32, m: u32, d: Option<u32>) -> Option<(String, &'static str)> {
    match d {
        Some(d) => {
            NaiveDate::from_ymd_opt(y, m, d).map(|dt| (dt.format("%Y-%m-%d").to_string(), "day"))
        }
        None => ((1..=12).contains(&m) && (1000..=9999).contains(&y))
            .then(|| (format!("{y:04}-{m:02}"), "month")),
    }
}

/// All dates in `text`, non-overlapping, in order of appearance.
pub fn find_dates(text: &str) -> Vec<DateHit> {
    let mut hits: Vec<DateHit> = Vec::new();
    for (kind, re) in PATTERNS.iter() {
        for c in re.captures_iter(text) {
            let whole = c.get(0).expect("match");
            if hits
                .iter()
                .any(|h| whole.start() < h.end && h.start < whole.end())
            {
                continue;
            }
            let mut ambiguous = false;
            let made = match *kind {
                "roc" => {
                    num(&c, "y").and_then(|y| make(y as i32 + 1911, num(&c, "m")?, num(&c, "d")))
                }
                "cjk" => num(&c, "y").and_then(|y| make(y as i32, num(&c, "m")?, num(&c, "d"))),
                "iso" => {
                    if c.name("s").map(|m| m.as_str()) != c.name("s2").map(|m| m.as_str()) {
                        None
                    } else {
                        num(&c, "y").and_then(|y| make(y as i32, num(&c, "m")?, num(&c, "d")))
                    }
                }
                "mdy" | "dmy" => {
                    let m = c.name("mon").and_then(|m| month_num(m.as_str()));
                    match (num(&c, "y"), m) {
                        (Some(y), Some(m)) => make(y as i32, m, num(&c, "d")),
                        _ => None,
                    }
                }
                "num" => {
                    let (a, b, y) = (num(&c, "a"), num(&c, "b"), num(&c, "y"));
                    match (a, b, y) {
                        (Some(a), Some(b), Some(y)) => {
                            // US order unless the first part cannot be a month.
                            let (m, d) = if a > 12 { (b, a) } else { (a, b) };
                            ambiguous = a <= 12 && b <= 12 && a != b;
                            make(y as i32, m, Some(d))
                        }
                        _ => None,
                    }
                }
                "my" => {
                    let m = c.name("mon").and_then(|m| month_num(m.as_str()));
                    match (num(&c, "y"), m) {
                        // "May 2026" is fine, but skip the bare word "May" only
                        // when it is the modal verb ("may 2026" is lower-case).
                        (Some(y), Some(m)) => make(y as i32, m, None),
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some((iso, precision)) = made {
                hits.push(DateHit {
                    iso,
                    precision,
                    ambiguous,
                    start: whole.start(),
                    end: whole.end(),
                });
            }
        }
    }
    hits.sort_by_key(|h| h.start);
    hits
}

/// ISO date when the whole string (trimmed) is a single date.
pub fn parse_whole(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() || t.len() > 40 {
        return None;
    }
    let hits = find_dates(t);
    match hits.as_slice() {
        [h] if h.start == 0 && h.end == t.len() => Some(h.iso.clone()),
        _ => None,
    }
}
