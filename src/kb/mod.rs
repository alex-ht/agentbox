//! Offline knowledge base: `kb search`, `kb get`, `kb open`, `kb import`.
//!
//! Tool order for Taiwan official facts is kb, then fetch/read, then web
//! search. URLs come only from stored records.

mod curated;
mod db;
mod import_gov;
mod score;

#[cfg(test)]
mod tests;

use crate::cmd::fetch::{self, normalize_url};
use crate::envelope::{AppError, CmdResult, Output};
use crate::state::{DocMeta, Store};
use db::{Recall, Record, SearchRow};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

const SNAPSHOT_TTL_DAYS: i64 = 30;
const LIMIT_MAX: usize = 50;

pub struct SearchArgs {
    pub query: String,
    pub category: Option<String>,
    pub kind: String,
    pub limit: usize,
}

pub struct ImportArgs {
    pub curated: bool,
    pub curated_file: Option<String>,
    pub services: Vec<String>,
    pub replace: bool,
}

pub fn run_search(store: &Store, args: &SearchArgs) -> CmdResult {
    let query = args.query.trim();
    if query.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "query is empty",
            "Example: `agentbox kb search 報稅`.",
        ));
    }
    if query.chars().count() > 200 {
        return Err(AppError::new(
            "bad_args",
            "query is too long",
            "Use a short official name, e.g. `agentbox kb search 報稅`.",
        ));
    }
    if args.limit == 0 || args.limit > LIMIT_MAX {
        return Err(AppError::new(
            "bad_args",
            format!("limit must be 1..={LIMIT_MAX}"),
            "Example: `agentbox kb search 報稅 --limit 8`.",
        ));
    }
    let category = args.category.as_deref().unwrap_or("").trim();
    if !category.is_empty() && !curated::known_category(category) {
        return Err(AppError::new(
            "bad_args",
            format!("unknown category `{category}`"),
            format!(
                "Use one of: {}. Example: `agentbox kb search {} --category tax`.",
                curated::LIFE_CATEGORIES.join(", "),
                shell_quote(query)
            ),
        ));
    }
    let kind = if args.kind.is_empty() {
        "all"
    } else {
        args.kind.as_str()
    };
    let norm = score::normalize(query);
    if norm.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "query has no searchable characters",
            "Example: `agentbox kb search 報稅`.",
        ));
    }

    let Some(conn) = db::open_if_exists(store)? else {
        return Err(kb_empty());
    };
    if db::count(&conn)? == 0 {
        return Err(kb_empty());
    }
    let recall = db::recall(&conn, &norm)?;
    let via = match &recall {
        Recall::Trigram(_) => "trigram",
        Recall::Scan => "scan",
    };
    let mut rows = db::load_search_rows(&conn, category, kind)?;
    if let Recall::Trigram(ids) = &recall {
        rows.retain(|r| ids.contains(&r.rowid));
    }
    let mut ranked: Vec<(f64, SearchRow)> = rows
        .into_iter()
        .map(|row| {
            let s = score::score_record(
                &norm,
                score::Fields {
                    name_zh: &row.name_zh,
                    name_en: &row.name_en,
                    aliases: &row.aliases,
                    category: &row.category,
                    description: &row.description,
                    official_url: &row.official_url,
                },
            );
            (s, row)
        })
        .filter(|(s, _)| *s > 0.0)
        .collect();
    ranked.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| {
                a.1.name_zh
                    .chars()
                    .count()
                    .cmp(&b.1.name_zh.chars().count())
            })
            .then_with(|| a.1.id.cmp(&b.1.id))
    });

    let hits: Vec<&(f64, SearchRow)> = ranked
        .iter()
        .filter(|(s, _)| *s >= score::HIT_MIN)
        .take(args.limit)
        .collect();
    if hits.is_empty() {
        let closest: Vec<&(f64, SearchRow)> = ranked
            .iter()
            .filter(|(s, _)| *s >= score::CLOSE_MIN)
            .take(3)
            .collect();
        return Err(not_found(query, &closest));
    }
    let top = &hits[0].1.id;
    let hint = format!(
        "Open the top hit with `agentbox kb open {}`, or read the full record with `agentbox kb get {}`. Use only these URLs; do not invent one.",
        shell_quote(top),
        shell_quote(top)
    );
    let hit_json: Vec<Value> = hits
        .iter()
        .map(|(s, row)| {
            json!({
                "id": row.id,
                "kind": row.kind,
                "name_zh": display_name(row),
                "category": row.category,
                "official_url": row.official_url,
                "verify_status": row.verify_status,
                "score": score::round_score(*s),
                "match": hit_match(&norm, row),
            })
        })
        .collect();
    Ok(Output::new(json!({
        "query": query,
        "via": via,
        "hits": hit_json,
    }))
    .hint(hint))
}

pub fn run_get(store: &Store, id: &str) -> CmdResult {
    let id = id.trim();
    if id.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "id is empty",
            "Example: `agentbox kb get org:ris`. Find ids with `agentbox kb search 戶政`.",
        ));
    }
    let Some(conn) = db::open_if_exists(store)? else {
        return Err(kb_empty());
    };
    if db::count(&conn)? == 0 {
        return Err(kb_empty());
    }
    let Some(rec) = db::get(&conn, id)? else {
        return Err(AppError::new(
            "not_found",
            format!("no knowledge-base record `{id}`"),
            format!("Find an id with `agentbox kb search {}`.", shell_quote(id)),
        ));
    };
    let hint = if rec.service_urls.is_empty() {
        format!(
            "Open the official page with `agentbox kb open {}`.",
            shell_quote(&rec.id)
        )
    } else {
        let rels: Vec<&str> = rec.service_urls.iter().map(|u| u.rel.as_str()).collect();
        format!(
            "Open the official page with `agentbox kb open {}`. Other links: {}.",
            shell_quote(&rec.id),
            rels.join(", ")
        )
    };
    Ok(Output::new(db::record_value(&rec)).hint(hint))
}

pub fn run_open(store: &Store, id: &str, which: &str) -> CmdResult {
    let id = id.trim();
    let which = if which.trim().is_empty() {
        "official"
    } else {
        which.trim()
    };
    if id.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "id is empty",
            "Example: `agentbox kb open org:etax`.",
        ));
    }
    let Some(conn) = db::open_if_exists(store)? else {
        return Err(kb_empty());
    };
    if db::count(&conn)? == 0 {
        return Err(kb_empty());
    }
    let Some(rec) = db::get(&conn, id)? else {
        return Err(AppError::new(
            "not_found",
            format!("no knowledge-base record `{id}`"),
            format!("Find an id with `agentbox kb search {}`.", shell_quote(id)),
        ));
    };
    let url = resolve_url(&rec, which)?;
    drop(conn);

    if let Some((meta, body)) = fresh_snapshot(store, &rec.cached_doc, &url) {
        let mut out = fetch::summarize(meta.id, &meta.title, &meta.url, &body);
        stamp_open(&mut out, &rec.id, which, true);
        return Ok(out);
    }

    let fetched = fetch::fetch_doc(store, &url, 30).map_err(|e| map_fetch_err(&rec, &url, e))?;
    let cached = format!("doc:{}\t{url}", fetched.id);
    let conn = db::open(store)?;
    db::set_cached_doc(&conn, &rec.id, &cached)?;
    let mut out = fetch::summarize(fetched.id, &fetched.title, &fetched.url, &fetched.body);
    stamp_open(&mut out, &rec.id, which, false);
    Ok(out)
}

pub fn run_import(store: &Store, args: &ImportArgs) -> CmdResult {
    if !args.curated && args.curated_file.is_none() && args.services.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "nothing to import",
            "Pass --curated, --curated-file FILE, --services FILE, or a combination. Example: `agentbox kb import --curated`.",
        ));
    }
    let curated_records = if args.curated {
        curated::load_embedded()?
    } else if let Some(path) = &args.curated_file {
        let text =
            fs::read_to_string(path).map_err(|e| AppError::io(&format!("read {path}"), e))?;
        curated::parse(&text)?
    } else {
        Vec::new()
    };

    let mut service_records = Vec::new();
    let mut skipped = 0;
    for path in &args.services {
        let parsed = import_gov::read_file(Path::new(path))?;
        skipped += parsed.skipped;
        service_records.extend(parsed.records);
    }

    let mut conn = db::open(store)?;
    let tx = conn.transaction().map_err(|e| {
        AppError::new(
            "kb_error",
            e.to_string(),
            "Retry `agentbox kb import --curated`.",
        )
    })?;
    let mut replaced = 0;
    if args.replace && !curated_records.is_empty() {
        replaced += db::delete_source(&tx, curated::SOURCE)?;
    }
    if args.replace && !service_records.is_empty() {
        replaced += db::delete_source(&tx, db::GOV_SOURCE)?;
    }
    for rec in &curated_records {
        db::upsert(&tx, rec)?;
    }
    for rec in &service_records {
        db::upsert(&tx, rec)?;
    }
    db::rebuild_fts(&tx)?;
    tx.commit().map_err(|e| {
        AppError::new(
            "kb_error",
            e.to_string(),
            "Retry `agentbox kb import --curated`.",
        )
    })?;

    let gov_n = db::count_source(&conn, db::GOV_SOURCE)?;
    let total = db::count(&conn)?;
    write_attribution(store, gov_n)?;
    let db_path = db::db_path(store).display().to_string();
    Ok(Output::new(json!({
        "db": db_path,
        "curated": curated_records.len(),
        "services": service_records.len(),
        "skipped": skipped,
        "replaced": replaced,
        "records": total,
        "attribution": db::attribution_path(store).display().to_string(),
    }))
    .hint("Search it with `agentbox kb search 健保`. URLs in the database are the only official addresses to cite."))
}

fn stamp_open(out: &mut Output, id: &str, which: &str, from_cache: bool) {
    out.data["kb_id"] = json!(id);
    out.data["which"] = json!(which);
    out.data["from_cache"] = json!(from_cache);
    let note = if from_cache {
        " Reused the knowledge-base snapshot."
    } else {
        " Fetched the URL stored on this knowledge-base record."
    };
    if let Some(hint) = out.hint.as_mut() {
        hint.push_str(note);
    }
}

fn fresh_snapshot(store: &Store, cached_doc: &str, url: &str) -> Option<(DocMeta, String)> {
    let (handle, cached_url) = cached_doc.split_once('\t')?;
    if cached_url != url {
        return None;
    }
    let (meta, body) = store.load_doc(handle).ok()?;
    let ts = chrono::DateTime::parse_from_rfc3339(&meta.fetched_at).ok()?;
    let age = chrono::Utc::now() - ts.with_timezone(&chrono::Utc);
    if age > chrono::Duration::days(SNAPSHOT_TTL_DAYS) {
        return None;
    }
    Some((meta, body))
}

fn resolve_url(rec: &Record, which: &str) -> Result<String, AppError> {
    let raw = if which.eq_ignore_ascii_case("official") {
        rec.official_url.clone()
    } else {
        rec.service_urls
            .iter()
            .find(|u| u.rel.eq_ignore_ascii_case(which))
            .map(|u| u.url.clone())
            .unwrap_or_default()
    };
    if raw.is_empty() {
        let rels: Vec<&str> = rec.service_urls.iter().map(|u| u.rel.as_str()).collect();
        let extra = if rels.is_empty() {
            "This record has no other links.".to_string()
        } else {
            format!("Available --which values: {}.", rels.join(", "))
        };
        return Err(AppError::new(
            "bad_args",
            format!("`{which}` is not a link on {}", rec.id),
            format!(
                "Use `agentbox kb open {}` for the official page. {extra}",
                shell_quote(&rec.id)
            ),
        ));
    }
    normalize_url(&raw)
}

fn map_fetch_err(rec: &Record, url: &str, err: AppError) -> AppError {
    let hint = format!(
        "Do not invent a different URL. Retry later with `agentbox kb open {}`. If you only need the address, run `agentbox kb get {}` and report official_url; say the page text was not fetched.",
        shell_quote(&rec.id),
        shell_quote(&rec.id)
    );
    let status = http_status(&err.message);
    let code = if err.code == "http_error" && matches!(status, Some(401 | 403 | 429)) {
        "fetch_blocked"
    } else {
        err.code
    };
    let message = if code == "fetch_blocked" {
        format!(
            "Official URL is known but the page could not be fetched: {}",
            err.message
        )
    } else {
        err.message
    };
    AppError::new(code, message, hint).with_details(json!({
        "id": rec.id,
        "official_url": rec.official_url,
        "url": url,
    }))
}

fn http_status(message: &str) -> Option<u16> {
    let rest = message.split("HTTP ").nth(1)?;
    rest.split_whitespace().next()?.parse().ok()
}

fn display_name(row: &SearchRow) -> String {
    if row.name_zh.is_empty() {
        row.name_en.clone()
    } else {
        row.name_zh.clone()
    }
}

fn hit_match(query_norm: &str, row: &SearchRow) -> &'static str {
    let exact = |s: &str| score::normalize(s) == query_norm;
    let partial = |s: &str| {
        let n = score::normalize(s);
        !n.is_empty() && n.contains(query_norm)
    };
    if exact(&row.name_zh) || exact(&row.name_en) || row.aliases.iter().any(|a| exact(a)) {
        "exact"
    } else if partial(&row.name_zh)
        || partial(&row.name_en)
        || partial(&row.description)
        || row.aliases.iter().any(|a| partial(a))
    {
        "partial"
    } else {
        "fuzzy"
    }
}

fn not_found(query: &str, closest: &[&(f64, SearchRow)]) -> AppError {
    let quoted = shell_quote(query);
    let closest_text = if closest.is_empty() {
        String::new()
    } else {
        let list = closest
            .iter()
            .map(|(_, row)| format!("{} {}", row.id, display_name(row)))
            .collect::<Vec<_>>()
            .join("; ");
        format!(" Closest: {list}.")
    };
    let retry = closest
        .first()
        .map(|(_, row)| shell_quote(&display_name(row)))
        .filter(|name| name != &quoted)
        .unwrap_or_else(|| "戶政".to_string());
    AppError::new(
        "not_found",
        format!("no knowledge-base entry matches {quoted}"),
        format!(
            "No knowledge-base entry matches {quoted}.{closest_text} Try a shorter official name, for example `agentbox kb search {retry}`. Do not invent URLs or steps. Only if the knowledge base still has nothing, use `agentbox search {quoted} --site gov.tw` as a last resort."
        ),
    )
}

fn kb_empty() -> AppError {
    AppError::new(
        "kb_empty",
        "the knowledge base has no records",
        "Import the curated official sites with `agentbox kb import --curated`. To add government services, download data.gov.tw dataset 146973 and run `agentbox kb import --services govonlineservice.json`.",
    )
}

fn write_attribution(store: &Store, gov_rows: i64) -> Result<(), AppError> {
    let path = db::attribution_path(store);
    if let Some(parent) = path.parent() {
        store.ensure_dir(parent)?;
    }
    let text = format!(
        "\
agentbox 離線知識庫顯名聲明
寫入時間：{now}

一、精選機關與民間官方入口（source = {curated}）
這些列只收錄名稱、分類與官方網址。verify_status 為 verified、connected、blocked 或 unverified（有查過但這台機器讀不到內文）。
不是政府開放資料的重製，也沒有把網頁全文放進知識庫。民間網站只供查詢官方入口，
授權標示為 {curated_license}。驗證日期：{verified}。
沒有通過連線核對的網址刻意不收錄。

二、我的E政府申辦服務（source = {gov}）
本庫目前有 {gov_rows} 筆來自此資料集的服務項。
資料提供機關：數位發展部
資料集名稱：我的E政府入口網-申辦服務資訊
資料集網址：https://data.gov.tw/dataset/146973
授權：政府資料開放授權條款－第1版（OGDL-1.0，與創用CC 姓名標示 4.0 相容）
授權條款：https://data.gov.tw/license/
資源：
- https://www.gov.tw/govonlineservice.json （線上服務）
- https://www.gov.tw/govcounterservice.json （臨櫃服務）
- https://www.gov.tw/govotherservice.json （電郵傳真）
使用這些服務項時必須保留本顯名聲明（提供機關、資料集名稱、授權）。
未盡顯名義務者，視同自始未取得授權。
",
        now = db::now_stamp(),
        curated = curated::SOURCE,
        curated_license = curated::LICENSE,
        verified = curated::VERIFIED_ON,
        gov = db::GOV_SOURCE,
        gov_rows = gov_rows,
    );
    fs::write(&path, text).map_err(|e| AppError::io("write ATTRIBUTION.txt", e))
}

/// Quote a shell word. Characters that are not plain tokens use single quotes,
/// so `$` in a service id is not expanded.
pub fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let safe = s.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || matches!(c, '-' | '_' | '.' | ':' | '/' | '+')
            || (!c.is_ascii() && !c.is_whitespace())
    });
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}
