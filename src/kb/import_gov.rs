//! Importer for data.gov.tw dataset 146973 (我的E政府申辦服務).
//!
//! The three downloads share one shape. Online rows use `線上申辦_*`,
//! counter rows use `臨櫃_*`, and email/fax rows use `電郵傳真_*`.

use super::db::{Record, ServiceUrl, GOV_SOURCE};
use crate::envelope::AppError;
use serde_json::{Map, Value};
use std::fs;
use std::path::Path;

pub const LICENSE: &str = "OGDL-1.0";
const DESC_CHARS: usize = 400;

pub struct Parsed {
    pub records: Vec<Record>,
    pub skipped: usize,
}

pub fn read_file(path: &Path) -> Result<Parsed, AppError> {
    let bytes = fs::read(path).map_err(|e| AppError::io(&format!("read {}", path.display()), e))?;
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    let text = String::from_utf8(bytes.to_vec()).map_err(|_| {
        AppError::new(
            "bad_json",
            format!("{} is not UTF-8", path.display()),
            "Save the data.gov.tw JSON as UTF-8 and retry the same `agentbox kb import --services` command.",
        )
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|e| {
        AppError::new(
            "bad_json",
            format!("{}: {e}", path.display()),
            "Pass a JSON array from https://www.gov.tw/govonlineservice.json (or the counter / other files).",
        )
    })?;
    let objects = as_objects(value)
        .map_err(|e| AppError::new(e.code, format!("{}: {}", path.display(), e.message), e.hint))?;
    let channel = channel_of(&path.display().to_string());
    let mut records = Vec::new();
    let mut skipped = 0;
    for obj in objects {
        match record_from(&obj, channel) {
            Some(rec) => records.push(rec),
            None => skipped += 1,
        }
    }
    Ok(Parsed { records, skipped })
}

fn as_objects(value: Value) -> Result<Vec<Map<String, Value>>, AppError> {
    match value {
        Value::Array(items) => items.into_iter().map(expect_object).collect(),
        Value::Object(map) if map.contains_key("服務代碼") || map.contains_key("標題") => {
            Ok(vec![map])
        }
        Value::Object(map) => {
            let array = map.into_values().find_map(|v| match v {
                Value::Array(a) => Some(a),
                _ => None,
            });
            match array {
                Some(items) => items.into_iter().map(expect_object).collect(),
                None => Err(AppError::new(
                    "bad_json",
                    "JSON object has no service array",
                    "Pass the dataset file itself (a JSON array), not the data.gov.tw API wrapper.",
                )),
            }
        }
        _ => Err(AppError::new(
            "bad_json",
            "service file must be a JSON array",
            "Example: `agentbox kb import --services govonlineservice.json`.",
        )),
    }
}

fn expect_object(v: Value) -> Result<Map<String, Value>, AppError> {
    match v {
        Value::Object(m) => Ok(m),
        _ => Err(AppError::new(
            "bad_json",
            "service array contains a non-object",
            "Re-download the JSON from www.gov.tw and retry.",
        )),
    }
}

pub fn channel_of(path: &str) -> &'static str {
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();
    if name.contains("counter") {
        "counter"
    } else if name.contains("other") {
        "other"
    } else if name.contains("online") {
        "online"
    } else {
        "gov"
    }
}

fn record_from(obj: &Map<String, Value>, channel: &str) -> Option<Record> {
    let code = text(obj, &["服務代碼"]);
    let title = text(obj, &["標題"]);
    if code.is_empty() || title.is_empty() || code.contains(['\n', '\r']) {
        return None;
    }
    let description = truncate(
        &clean(&text(
            obj,
            &[
                "線上申辦_服務內容",
                "臨櫃_服務內容",
                "電郵傳真_服務內容",
                "服務內容",
                "申辦內容",
            ],
        )),
        DESC_CHARS,
    );
    let mut links = Vec::new();
    push_link(&mut links, "page", "服務說明", &text(obj, &["頁面連結"]));
    push_link(
        &mut links,
        "apply",
        "線上申辦",
        &text(obj, &["線上申辦連結"]),
    );
    push_link(
        &mut links,
        "form",
        "表單下載",
        &text(obj, &["臨櫃_表單下載連結"]),
    );
    push_link(
        &mut links,
        "email",
        "電郵傳真",
        &text(obj, &["電郵傳真_連結"]),
    );
    let official = links
        .iter()
        .find(|l| l.rel == "page")
        .or_else(|| links.first())
        .map(|l| l.url.clone())
        .unwrap_or_default();
    if official.is_empty() {
        return None;
    }
    let mut rec = Record::blank(format!("svc:{code}"), "service");
    rec.category = classify(&title, &description, channel);
    if title.is_ascii() {
        rec.name_en = title;
    } else {
        rec.name_zh = title;
    }
    rec.official_url = official;
    rec.service_urls = links;
    rec.description = description;
    rec.source = GOV_SOURCE.to_string();
    rec.source_license = LICENSE.to_string();
    rec.verify_status = "unverified".to_string();
    Some(rec)
}

fn push_link(out: &mut Vec<ServiceUrl>, rel: &str, label: &str, raw: &str) {
    let Some(url) = http_url(raw) else {
        return;
    };
    if out.iter().any(|l| l.url == url && l.rel == rel) {
        return;
    }
    out.push(ServiceUrl {
        rel: rel.to_string(),
        label: label.to_string(),
        url,
    });
}

fn http_url(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.len() > 2000 || t.contains(char::is_whitespace) {
        return None;
    }
    if t.starts_with("https://") || t.starts_with("http://") {
        Some(t.to_string())
    } else {
        None
    }
}

fn text(obj: &Map<String, Value>, keys: &[&str]) -> String {
    for key in keys {
        if let Some(Value::String(s)) = obj.get(*key) {
            let t = s.trim();
            if !t.is_empty() && t != "無資料" {
                return t.to_string();
            }
        }
    }
    String::new()
}

fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars).collect();
    out.push('…');
    out
}

fn classify(title: &str, description: &str, channel: &str) -> String {
    let hay = format!("{title} {description}");
    const RULES: &[(&str, &[&str])] = &[
        ("nhi", &["健保", "健康保險"]),
        (
            "tax",
            &[
                "所得稅",
                "報稅",
                "營業稅",
                "稅務",
                "國稅",
                "房屋稅",
                "牌照稅",
                "綜所稅",
                "貨物稅",
                "遺產稅",
                "贈與稅",
            ],
        ),
        (
            "household",
            &[
                "戶政",
                "身分證",
                "戶籍",
                "印鑑",
                "護照",
                "簽證",
                "地政",
                "移民",
                "居留",
                "出生登記",
            ],
        ),
        (
            "motor",
            &[
                "監理",
                "駕照",
                "駕駛執照",
                "行車執照",
                "牌照",
                "汽車",
                "機車",
            ],
        ),
        (
            "labor",
            &["勞保", "勞工", "就業", "勞退", "職災", "職業災害"],
        ),
        (
            "welfare",
            &["補助", "津貼", "福利", "身障", "低收入", "育兒", "長照"],
        ),
        (
            "transport",
            &["臺鐵", "台鐵", "高鐵", "捷運", "鐵路", "高速公路", "公車"],
        ),
        ("medical", &["醫院", "掛號", "疫苗", "傳染病", "健檢"]),
        (
            "utilities",
            &["電費", "水費", "瓦斯", "電信", "台電", "自來水"],
        ),
        ("post", &["郵政", "郵局"]),
        ("weather", &["氣象", "地震", "颱風", "災害", "消防"]),
        ("consumer", &["發票", "消費爭議", "公平交易"]),
        ("finance", &["銀行", "金融", "保險"]),
    ];
    for (cat, keys) in RULES {
        if keys.iter().any(|k| hay.contains(k)) {
            return (*cat).to_string();
        }
    }
    channel.to_string()
}
