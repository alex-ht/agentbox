//! `now [--tz ZONE]`: current time in ISO 8601 with timezone details.

use crate::envelope::{AppError, CmdResult, Output};
use chrono::{DateTime, FixedOffset, Local, Offset, Utc};
use serde_json::json;
use std::str::FromStr;

pub fn run(tz: Option<&str>) -> CmdResult {
    let now = Utc::now();
    Ok(Output::new(describe(now, tz)?))
}

/// Describe instant `now` in the requested zone (local zone when `None`).
pub fn describe(now: DateTime<Utc>, tz: Option<&str>) -> Result<serde_json::Value, AppError> {
    let (dt, name): (DateTime<FixedOffset>, String) = match tz
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => {
            let local = now.with_timezone(&Local);
            let name = iana_time_zone::get_timezone().unwrap_or_else(|_| "local".into());
            (local.fixed_offset(), name)
        }
        Some(z)
            if z.eq_ignore_ascii_case("utc")
                || z.eq_ignore_ascii_case("z")
                || z.eq_ignore_ascii_case("gmt") =>
        {
            (now.fixed_offset(), "UTC".into())
        }
        Some(z) => {
            if let Ok(tz) = chrono_tz::Tz::from_str(z) {
                let t = now.with_timezone(&tz);
                let off = t.offset().fix();
                (now.with_timezone(&off), tz.name().to_string())
            } else if let Some(off) = parse_offset(z) {
                (now.with_timezone(&off), format!("UTC{}", off))
            } else {
                return Err(AppError::new(
                    "bad_timezone",
                    format!("unknown timezone `{z}`"),
                    "Use an IANA name like `Asia/Taipei`, `America/New_York`, `Europe/Berlin`, or an offset like `+08:00`.",
                ));
            }
        }
    };
    Ok(json!({
        "iso": dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        "timezone": name,
        "utc_offset": dt.offset().to_string(),
        "date": dt.format("%Y-%m-%d").to_string(),
        "time": dt.format("%H:%M:%S").to_string(),
        "weekday": dt.format("%A").to_string(),
        "unix": dt.timestamp(),
    }))
}

/// Parse `+08:00`, `-0530`, `+8`, `UTC+8`, `GMT-03:30`.
fn parse_offset(s: &str) -> Option<FixedOffset> {
    let s = s.trim();
    let s = s
        .strip_prefix("UTC")
        .or_else(|| s.strip_prefix("utc"))
        .or_else(|| s.strip_prefix("GMT"))
        .or_else(|| s.strip_prefix("gmt"))
        .unwrap_or(s);
    let (sign, rest) = match s.chars().next()? {
        '+' => (1, &s[1..]),
        '-' => (-1, &s[1..]),
        _ => return None,
    };
    let (h, m) = if let Some((h, m)) = rest.split_once(':') {
        (h.parse::<i32>().ok()?, m.parse::<i32>().ok()?)
    } else if rest.len() == 4 {
        (
            rest[..2].parse::<i32>().ok()?,
            rest[2..].parse::<i32>().ok()?,
        )
    } else {
        (rest.parse::<i32>().ok()?, 0)
    };
    if h > 14 || m >= 60 {
        return None;
    }
    FixedOffset::east_opt(sign * (h * 3600 + m * 60))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fixed() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 8, 2, 30, 0).unwrap()
    }

    #[test]
    fn iana_zone() {
        let v = describe(fixed(), Some("Asia/Taipei")).unwrap();
        assert_eq!(v["iso"], "2026-10-08T10:30:00+08:00");
        assert_eq!(v["timezone"], "Asia/Taipei");
        assert_eq!(v["weekday"], "Thursday");
    }

    #[test]
    fn dst_zone_and_utc() {
        let v = describe(fixed(), Some("America/New_York")).unwrap();
        assert_eq!(v["utc_offset"], "-04:00");
        let v = describe(fixed(), Some("utc")).unwrap();
        assert_eq!(v["iso"], "2026-10-08T02:30:00+00:00");
    }

    #[test]
    fn offsets() {
        assert_eq!(
            describe(fixed(), Some("+05:30")).unwrap()["time"],
            "08:00:00"
        );
        assert_eq!(
            describe(fixed(), Some("UTC-3")).unwrap()["time"],
            "23:30:00"
        );
        assert_eq!(
            describe(fixed(), Some("-0100")).unwrap()["time"],
            "01:30:00"
        );
    }

    #[test]
    fn local_and_bad_zone() {
        assert!(describe(fixed(), None).unwrap()["iso"].is_string());
        let e = describe(fixed(), Some("Mars/Base")).unwrap_err();
        assert_eq!(e.code, "bad_timezone");
        assert!(e.hint.contains("Asia/Taipei"));
    }
}
