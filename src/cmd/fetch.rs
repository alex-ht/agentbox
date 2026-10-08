//! `fetch <url>`: HTTP GET, HTML -> Markdown, store as a doc handle.

use crate::envelope::{AppError, CmdResult, Output};
use crate::markdown::{extract_title, html_to_markdown, outline, split_sections};
use crate::state::{DocMeta, Store};
use serde_json::json;
use std::io::Read;
use std::time::Duration;

pub const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; agentbox/",
    env!("CARGO_PKG_VERSION"),
    "; +https://github.com/alex-ht/agentbox)"
);

/// Hard cap on downloaded bytes.
const MAX_BYTES: u64 = 10 * 1024 * 1024;
/// Outline entries returned inline; the rest are summarized.
const MAX_OUTLINE: usize = 60;

pub fn run(store: &Store, url: &str, timeout_secs: u64) -> CmdResult {
    let url = normalize_url(url)?;
    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(timeout_secs.max(1)))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| {
            AppError::new(
                "network_error",
                e.to_string(),
                "Retry; if it persists, report a bug.",
            )
        })?;

    let resp = client
        .get(&url)
        .header(
            reqwest::header::ACCEPT,
            "text/html,application/xhtml+xml,application/json;q=0.9,text/plain;q=0.8,*/*;q=0.5",
        )
        .send()
        .map_err(|e| request_error(&url, &e))?;

    let status = resp.status();
    let final_url = resp.url().to_string();
    if !status.is_success() {
        return Err(status_error(
            &final_url,
            status.as_u16(),
            status.canonical_reason().unwrap_or(""),
        ));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let mut bytes = Vec::new();
    resp.take(MAX_BYTES).read_to_end(&mut bytes).map_err(|e| {
        AppError::new(
            "network_error",
            format!("reading body: {e}"),
            "Retry, or use a longer --timeout.",
        )
    })?;

    let (title, body) = process_body(&content_type, &bytes, &final_url)?;
    let meta = DocMeta {
        id: 0,
        url: final_url.clone(),
        title: title.clone(),
        content_type: content_type.clone(),
        fetched_at: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
    };
    let id = store.save_doc(meta, &body)?;
    Ok(summarize(id, &title, &final_url, &body))
}

/// Build the fetch response for a stored doc.
pub fn summarize(id: u64, title: &str, url: &str, body: &str) -> Output {
    let sections = split_sections(body);
    let mut ol = outline(body, &sections);
    let total_sections = ol.len();
    let omitted = total_sections.saturating_sub(MAX_OUTLINE);
    ol.truncate(MAX_OUTLINE);
    let mut data = json!({
        "doc": format!("doc:{id}"),
        "title": title,
        "url": url,
        "chars": body.chars().count(),
        "sections": total_sections,
        "outline": ol,
    });
    if omitted > 0 {
        data["outline_omitted"] = json!(omitted);
    }
    Output::new(data).hint(format!(
        "Read a section with `agentbox read doc:{id} --section N`, or find facts with `agentbox read doc:{id} --grep KEYWORD`."
    ))
}

/// Add a scheme if missing and reject non-HTTP schemes.
pub fn normalize_url(url: &str) -> Result<String, AppError> {
    let u = url.trim();
    let with_scheme = if u.contains("://") {
        u.to_string()
    } else {
        format!("https://{u}")
    };
    let lower = with_scheme.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://"))
        || with_scheme.len() <= "https://".len()
    {
        return Err(AppError::new(
            "bad_url",
            format!("`{url}` is not an http(s) URL"),
            "Use a full URL such as `https://example.com/page`. For local files use `agentbox file read PATH`.",
        ));
    }
    Ok(with_scheme)
}

fn request_error(url: &str, e: &reqwest::Error) -> AppError {
    if e.is_timeout() {
        AppError::new(
            "timeout",
            format!("timed out fetching {url}"),
            "Retry with a longer timeout, e.g. `--timeout 60`, or try another source.",
        )
    } else if e.is_builder() {
        AppError::new(
            "bad_url",
            format!("invalid URL {url}: {e}"),
            "Check the URL spelling.",
        )
    } else {
        let mut msg = e.to_string();
        let mut src = std::error::Error::source(e);
        while let Some(s) = src {
            msg.push_str(&format!(": {s}"));
            src = s.source();
        }
        AppError::new(
            "network_error",
            msg,
            "Check the domain spelling and network access; if the site is down, try another source.",
        )
    }
}

fn status_error(url: &str, code: u16, reason: &str) -> AppError {
    let hint = match code {
        401 | 403 => "The site refused access (login or bot protection). Try another source covering the same facts.",
        404 | 410 => "Page not found. Fetch the site's homepage or a listing page and follow links from there.",
        429 => "Rate limited. Wait a bit before retrying, or use another source.",
        500..=599 => "Server error. Retry once later, or use another source.",
        _ => "Check the URL, or try another source.",
    };
    AppError::new(
        "http_error",
        format!("HTTP {code} {reason} for {url}"),
        hint,
    )
}

/// Decode and convert a response body. Returns (title, markdown/text body).
pub fn process_body(
    content_type: &str,
    bytes: &[u8],
    url: &str,
) -> Result<(String, String), AppError> {
    let ct = content_type.to_ascii_lowercase();
    let mime = ct.split(';').next().unwrap_or("").trim();
    let sniff_html = mime.is_empty() && {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
        head.contains("<html") || head.contains("<!doctype html")
    };
    let is_html = mime.contains("html") || sniff_html;
    let is_text = mime.starts_with("text/")
        || mime.contains("json")
        || mime.contains("xml")
        || mime.is_empty();
    if !is_html && !is_text {
        return Err(AppError::new(
            "unsupported_content",
            format!("content type `{mime}` is not text or HTML"),
            "Only HTML, text, JSON and XML are supported in v0.1. Look for an HTML version of the page.",
        ));
    }
    let text = decode(bytes, &ct, is_html);
    if is_html {
        let md = html_to_markdown(&text, Some(url));
        let title = extract_title(&text)
            .or_else(|| first_heading(&md))
            .unwrap_or_else(|| url.to_string());
        Ok((title, md))
    } else if mime.contains("json") {
        let body = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or(text);
        Ok((url.to_string(), body))
    } else {
        Ok((url.to_string(), text))
    }
}

fn first_heading(md: &str) -> Option<String> {
    md.lines()
        .find(|l| l.starts_with('#'))
        .map(|l| l.trim_start_matches('#').trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Decode bytes using the header charset, a `<meta charset>` sniff, or UTF-8.
fn decode(bytes: &[u8], content_type_lower: &str, is_html: bool) -> String {
    let label = charset_param(content_type_lower).or_else(|| {
        if is_html {
            let head =
                String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_ascii_lowercase();
            charset_param(&head)
        } else {
            None
        }
    });
    let enc = label
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (text, _, _) = enc.decode(bytes);
    text.into_owned()
}

fn charset_param(s: &str) -> Option<String> {
    let pos = s.find("charset=")?;
    let rest = s[pos + 8..].trim_start_matches(['"', '\'']);
    let label: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (!label.is_empty()).then_some(label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn normalizes_urls() {
        assert_eq!(normalize_url("example.com").unwrap(), "https://example.com");
        assert_eq!(normalize_url("http://a.b/c").unwrap(), "http://a.b/c");
        assert_eq!(normalize_url("ftp://x").unwrap_err().code, "bad_url");
        assert_eq!(normalize_url("https://").unwrap_err().code, "bad_url");
    }

    #[test]
    fn decodes_declared_charset() {
        let (enc_bytes, _, _) = encoding_rs::BIG5.encode("<html><head><meta charset=\"big5\"><title>台灣</title></head><body><p>你好</p></body></html>");
        let (title, md) = process_body("text/html", &enc_bytes, "u").unwrap();
        assert_eq!(title, "台灣");
        assert!(md.contains("你好"));
    }

    #[test]
    fn json_is_pretty_printed_and_pdf_rejected() {
        let (_, body) = process_body("application/json", br#"{"a":1}"#, "u").unwrap();
        assert_eq!(body, "{\n  \"a\": 1\n}");
        assert_eq!(
            process_body("application/pdf", b"%PDF", "u")
                .unwrap_err()
                .code,
            "unsupported_content"
        );
    }

    /// Serve `responses` one connection each on a local port; returns the base URL.
    fn serve(responses: Vec<String>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for resp in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if line == "\r\n" {
                        break;
                    }
                    line.clear();
                }
                stream.write_all(resp.as_bytes()).unwrap();
            }
        });
        format!("http://{addr}")
    }

    fn http(status: &str, ctype: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn fetch_end_to_end_against_local_server() {
        let html = "<html><head><title>Local</title></head><body><h1>Hello</h1><p>World</p><h2>More</h2><p>x</p></body></html>";
        let base = serve(vec![
            http("200 OK", "text/html; charset=utf-8", html),
            http("404 Not Found", "text/plain", "nope"),
        ]);
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());

        let out = run(&store, &format!("{base}/page"), 5).unwrap();
        assert_eq!(out.data["doc"], "doc:1");
        assert_eq!(out.data["title"], "Local");
        assert_eq!(out.data["outline"][0]["heading"], "Hello");
        assert_eq!(out.data["outline"][1]["heading"], "More");
        let (_, body) = store.load_doc("doc:1").unwrap();
        assert!(body.contains("World"));

        let err = run(&store, &format!("{base}/missing"), 5).unwrap_err();
        assert_eq!(err.code, "http_error");
        assert!(err.message.contains("404"));
        assert!(!err.hint.is_empty());
    }
}
