//! Small blocking-HTTP helpers shared by the keyless data commands
//! (`quote`, `market`).

use crate::cmd::fetch::USER_AGENT;
use std::io::Read;
use std::time::Duration;

/// Cap on response bodies; API answers are far smaller.
const MAX_BODY: u64 = 20 * 1024 * 1024;

/// Why a request produced no usable body.
#[derive(Debug, Clone, PartialEq)]
pub enum Fail {
    /// Connection-level failure (DNS, TLS, refused, timeout). `dns` is set
    /// when the error chain looks like a name-resolution failure.
    Net {
        message: String,
        timeout: bool,
        dns: bool,
    },
    /// The server answered with a non-success status.
    Status(u16, String),
}

pub fn client(timeout_secs: u64) -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .expect("http client builds")
}

/// GET a URL and return the body for 2xx answers.
pub fn get(client: &reqwest::blocking::Client, url: &str) -> Result<String, Fail> {
    let resp = client
        .get(url)
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/csv;q=0.9, */*;q=0.5",
        )
        .send()
        .map_err(|e| net_fail(&e))?;
    let status = resp.status().as_u16();
    let mut body = String::new();
    resp.take(MAX_BODY)
        .read_to_string(&mut body)
        .map_err(|e| Fail::Net {
            message: format!("reading response: {e}"),
            timeout: false,
            dns: false,
        })?;
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        Err(Fail::Status(status, body))
    }
}

fn net_fail(e: &reqwest::Error) -> Fail {
    let mut message = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        message.push_str(&format!(": {s}"));
        src = s.source();
    }
    let lower = message.to_lowercase();
    let dns = [
        "dns",
        "resolve",
        "lookup",
        "name or service not known",
        "no such host",
        "nodename nor servname",
        "temporary failure in name resolution",
    ]
    .iter()
    .any(|k| lower.contains(k));
    Fail::Net {
        message,
        timeout: e.is_timeout(),
        dns,
    }
}

/// Percent-encode one URL path segment or query value (RFC 3986 unreserved
/// characters stay as they are).
pub fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// First `max` characters of an error body, on one line.
pub fn snippet(body: &str, max: usize) -> String {
    let flat: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max).collect::<String>())
    } else {
        flat
    }
}

/// Round to `digits` decimals, dropping float noise.
pub fn round(x: f64, digits: u32) -> f64 {
    let p = 10f64.powi(digits as i32);
    let r = (x * p).round() / p;
    if r == 0.0 {
        0.0
    } else {
        r
    }
}

/// `round` as a JSON value; whole numbers become integers (2550, not 2550.0).
pub fn jround(x: f64, digits: u32) -> serde_json::Value {
    crate::table::value::num_value(round(x, digits))
}

/// Read an env override, ignoring blank values.
pub fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(encode("^GSPC"), "%5EGSPC");
        assert_eq!(encode("USDTWD=X"), "USDTWD%3DX");
        assert_eq!(encode("2330.TW"), "2330.TW");
        assert_eq!(encode("台積電"), "%E5%8F%B0%E7%A9%8D%E9%9B%BB");
        assert_eq!(round(1.23456, 2), 1.23);
        assert_eq!(round(-0.0001, 2), 0.0);
        assert_eq!(snippet("a\n  b   c", 3), "a b…");
    }

    #[test]
    fn connection_refused_is_net_failure() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        let err = get(&client(5), &format!("http://{addr}/x")).unwrap_err();
        assert!(matches!(err, Fail::Net { dns: false, .. }), "{err:?}");
    }
}
