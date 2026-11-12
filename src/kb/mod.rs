//! Offline knowledge base: `kb search`, `kb get`, `kb open`, `kb import`.
//!
//! Tool order for Taiwan official facts is kb, then fetch/read, then web
//! search. URLs come only from stored records.

mod curated;
mod db;
mod import_gov;
mod popular;
mod s2t;
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

#[derive(Default)]
pub struct ImportArgs {
    pub curated: bool,
    pub curated_file: Option<String>,
    pub services: Vec<String>,
    pub replace: bool,
    pub crux: Option<String>,
    pub scope: Option<String>,
    pub max_rank: Option<u32>,
    pub verify: bool,
    pub enrich: bool,
    pub write_seed: Option<String>,
    pub popular: bool,
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
    let prepared = score::prepare_query(query);
    if prepared.terms.is_empty() {
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
    let recall_norms = prepared.recall_norms();
    let recall = db::recall(&conn, &recall_norms)?;
    let via = match &recall {
        Recall::Trigram(_) => "trigram",
        Recall::Scan => "scan",
    };
    // Category is applied after scoring so a too-narrow filter can say how
    // many hits exist in other categories.
    let mut rows = db::load_search_rows(&conn, "", kind)?;
    if let Recall::Trigram(ids) = &recall {
        rows.retain(|r| ids.contains(&r.rowid));
    }
    let mut ranked: Vec<Ranked> = rows
        .into_iter()
        .filter_map(|row| {
            let scored = score::score_query(
                &prepared,
                score::Fields {
                    name_zh: &row.name_zh,
                    name_en: &row.name_en,
                    aliases: &row.aliases,
                    category: &row.category,
                    description: &row.description,
                    official_url: &row.official_url,
                },
            );
            if scored.score <= 0.0 {
                return None;
            }
            Some(Ranked {
                score: scored.score,
                matched_terms: scored.matched_terms,
                kind: scored.kind,
                row,
            })
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| {
                score::popularity_key(b.row.popularity_bucket)
                    .cmp(&score::popularity_key(a.row.popularity_bucket))
            })
            .then_with(|| {
                a.row
                    .name_zh
                    .chars()
                    .count()
                    .cmp(&b.row.name_zh.chars().count())
            })
            .then_with(|| a.row.id.cmp(&b.row.id))
    });

    let in_category = |row: &SearchRow| category.is_empty() || row.category == category;
    let hits: Vec<&Ranked> = ranked
        .iter()
        .filter(|r| r.score >= score::HIT_MIN && in_category(&r.row))
        .take(args.limit)
        .collect();
    if hits.is_empty() {
        let others: Vec<&Ranked> = if category.is_empty() {
            Vec::new()
        } else {
            ranked
                .iter()
                .filter(|r| r.score >= score::HIT_MIN && r.row.category != category)
                .collect()
        };
        let closest: Vec<&Ranked> = ranked
            .iter()
            .filter(|r| in_category(&r.row) && r.score >= score::CLOSE_MIN)
            .take(3)
            .collect();
        return Err(not_found(query, &prepared, &closest, category, &others));
    }
    let top = &hits[0].row.id;
    let hint = format!(
        "Open the top hit with `agentbox kb open {}`, or read the full record with `agentbox kb get {}`. Use only these URLs; do not invent one. `matched_terms` shows which query words hit; `which` lists `kb open --which` values.",
        shell_quote(top),
        shell_quote(top)
    );
    let hit_json: Vec<Value> = hits
        .iter()
        .map(|hit| {
            json!({
                "id": hit.row.id,
                "kind": hit.row.kind,
                "name_zh": display_name(&hit.row),
                "category": hit.row.category,
                "official_url": hit.row.official_url,
                "verify_status": hit.row.verify_status,
                "score": score::round_score(hit.score),
                "match": hit.kind,
                "matched_terms": hit.matched_terms,
                "which": which_values(&hit.row.service_rels),
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

struct Ranked {
    score: f64,
    matched_terms: Vec<String>,
    kind: &'static str,
    row: SearchRow,
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
    let which = which_values(
        &rec.service_urls
            .iter()
            .map(|u| u.rel.clone())
            .collect::<Vec<_>>(),
    );
    let hint = format!(
        "Open a stored page with `agentbox kb open {}`. Available --which values: {}.",
        shell_quote(&rec.id),
        which.join(", ")
    );
    let mut data = db::record_value(&rec);
    data["which"] = json!(which);
    Ok(Output::new(data).hint(hint))
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
    if !args.curated
        && args.curated_file.is_none()
        && args.services.is_empty()
        && args.crux.is_none()
        && !args.popular
    {
        return Err(AppError::new(
            "bad_args",
            "nothing to import",
            "Pass --curated, --popular, --services FILE, or --crux FILE --scope tw --max-rank 5000. Example: `agentbox kb import --curated`.",
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

    let mut popular_records = Vec::new();
    let mut crux_stats = None;
    if let Some(path) = &args.crux {
        let scope = args.scope.clone().unwrap_or_else(|| "tw".into());
        let max_rank = args.max_rank.unwrap_or(5000);
        let month = popular::month_from_path(path);
        let csv = popular::read_crux_file(Path::new(path))?;
        let (mut recs, mut stats) = popular::prepare_csv(&csv, &scope, max_rank, &month);
        if args.enrich {
            let _ = popular::enrich_wikidata(&mut recs);
        }
        if args.verify {
            popular::verify_records(&mut recs);
        }
        let extra_adult = popular::polish_records(&mut recs);
        stats.dropped_adult += extra_adult;
        stats.kept_domains = stats.kept_domains.saturating_sub(extra_adult);
        if let Some(out) = &args.write_seed {
            let seed = popular::records_to_seed(&recs, &scope, &month);
            let text = serde_json::to_string(&seed).map_err(|e| {
                AppError::new(
                    "kb_error",
                    format!("seed json: {e}"),
                    "Retry the same kb import --crux command.",
                )
            })?;
            fs::write(out, text).map_err(|e| AppError::io(&format!("write {out}"), e))?;
        }
        crux_stats = Some(stats);
        popular_records.append(&mut recs);
    }
    if args.popular {
        popular_records.extend(popular::load_bundled()?);
    }

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
    let (popular_inserted, popular_merged) = popular::merge_records(&tx, &popular_records)?;
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
        "popular_inserted": popular_inserted,
        "popular_merged": popular_merged,
        "crux_input_rows": crux_stats.as_ref().map(|s| s.input_rows),
        "crux_kept": crux_stats.as_ref().map(|s| s.kept_domains),
        "crux_dropped_rank": crux_stats.as_ref().map(|s| s.dropped_rank),
        "crux_dropped_adult": crux_stats.as_ref().map(|s| s.dropped_adult),
        "crux_dropped_infra": crux_stats.as_ref().map(|s| s.dropped_infra),
        "crux_dropped_ip": crux_stats.as_ref().map(|s| s.dropped_ip),
        "crux_dropped_noise_only": crux_stats.as_ref().map(|s| s.dropped_noise_only),
        "crux_dropped_other": crux_stats.as_ref().map(|s| s.dropped_other),
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
        let rels = which_values(
            &rec.service_urls
                .iter()
                .map(|u| u.rel.clone())
                .collect::<Vec<_>>(),
        );
        return Err(AppError::new(
            "bad_args",
            format!("`{which}` is not a link on {}", rec.id),
            format!(
                "Available --which values: {}. Example: `agentbox kb open {} --which {}`.",
                rels.join(", "),
                shell_quote(&rec.id),
                shell_quote(rels.first().map(String::as_str).unwrap_or("official"))
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

fn which_values(rels: &[String]) -> Vec<String> {
    let mut out = vec!["official".to_string()];
    for rel in rels {
        if !rel.is_empty() && out.iter().all(|e| !e.eq_ignore_ascii_case(rel)) {
            out.push(rel.clone());
        }
    }
    out
}

fn not_found(
    query: &str,
    prepared: &score::PreparedQuery,
    closest: &[&Ranked],
    category: &str,
    others: &[&Ranked],
) -> AppError {
    let quoted = shell_quote(query);
    let mut parts = Vec::new();
    if category.is_empty() {
        parts.push(format!("No knowledge-base entry matches {quoted}."));
    } else {
        parts.push(format!(
            "No knowledge-base entry matches {quoted} in category {category}."
        ));
    }
    if !others.is_empty() {
        let mut cats: Vec<&str> = others.iter().map(|r| r.row.category.as_str()).collect();
        cats.sort_unstable();
        cats.dedup();
        let shown = cats.iter().take(6).copied().collect::<Vec<_>>().join(", ");
        parts.push(format!(
            "Drop --category; {} hits in other categories: {shown}.",
            others.len()
        ));
    }
    if !closest.is_empty() {
        let list = closest
            .iter()
            .map(|hit| format!("{} {}", hit.row.id, display_name(&hit.row)))
            .collect::<Vec<_>>()
            .join("; ");
        parts.push(format!("Closest: {list}."));
    }
    let content = prepared.content_terms();
    let dropped_stop = prepared.terms.iter().any(|t| t.stop) && !content.is_empty();
    let mut suggestions = Vec::new();
    if dropped_stop {
        let q = content
            .iter()
            .map(|t| t.raw.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        suggestions.push(format!(
            "Drop generic words and retry `agentbox kb search {}`.",
            shell_quote(&q)
        ));
    }
    if content.len() >= 2 {
        suggestions.push(format!(
            "Or search one term at a time, for example `agentbox kb search {}`.",
            shell_quote(&content[0].raw)
        ));
    }
    if let Some(hit) = closest.first() {
        let name = display_name(&hit.row);
        let sq = shell_quote(&name);
        if sq != quoted {
            suggestions.push(format!("Try the formal name `agentbox kb search {sq}`."));
        }
    }
    if !others.is_empty() {
        suggestions.push(format!(
            "Retry without the filter: `agentbox kb search {quoted}`."
        ));
    }
    if suggestions.is_empty() {
        suggestions.push(
            "Split the query into shorter terms, or search the formal official name with `agentbox kb search`."
                .to_string(),
        );
    }
    parts.push(suggestions.join(" "));
    parts.push("Do not invent URLs or steps.".to_string());
    parts.push(
        "If this is general knowledge rather than a Taiwan official site, try `agentbox wiki search` next."
            .to_string(),
    );
    parts.push(format!(
        "Only if that still has nothing, use `agentbox search {quoted} --site gov.tw` as a last resort."
    ));
    let message = if category.is_empty() {
        format!("no knowledge-base entry matches {quoted}")
    } else {
        format!("no knowledge-base entry matches {quoted} in category {category}")
    };
    AppError::new("not_found", message, parts.join(" "))
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
讀不到內文的官方網址可以留下，但 verify_status 會是 unverified，不能當成已核對的內容。

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

三、常用網站（Chrome UX Report，source 以 crux: 開頭）
資料：Google Chrome User Experience Report（CrUX）台灣與全球 origin 名單，
快取檔來自 https://github.com/zakird/crux-top-lists （country/tw 與 global 的 YYYYMM.csv.gz）。
授權：Creative Commons Attribution 4.0 International（CC BY 4.0）
https://creativecommons.org/licenses/by/4.0/
https://developer.chrome.com/docs/crux/methodology/
標示：Chrome User Experience Report, Google.
名次是量級桶（1000、5000…），不是精確排名。桶只在文字相關性打平時當排序加權。
若名稱或別名來自 Wikidata P856，該部分為 CC0。
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
