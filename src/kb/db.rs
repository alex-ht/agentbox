//! SQLite store at `$AGENTBOX_HOME/kb/kb.sqlite`.
//!
//! The full-text index uses FTS5's trigram tokenizer so a Chinese substring
//! such as 「所得稅」 matches inside 「綜合所得稅結算申報」 without spaces.

use crate::envelope::AppError;
use crate::state::Store;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const GOV_SOURCE: &str = "data.gov.tw:146973";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceUrl {
    pub rel: String,
    pub label: String,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct Record {
    pub id: String,
    pub kind: String,
    pub name_zh: String,
    pub name_en: String,
    pub aliases: Vec<String>,
    pub category: String,
    pub official_url: String,
    pub service_urls: Vec<ServiceUrl>,
    pub description: String,
    pub source: String,
    pub source_license: String,
    pub last_verified: String,
    pub verify_status: String,
    pub cached_doc: String,
    pub updated_at: String,
    /// CrUX magnitude bucket (1000, 5000, ...). Empty for curated rows.
    pub popularity_bucket: Option<i64>,
    /// `tw` or `global` when the row came from a CrUX list.
    pub popularity_scope: String,
    /// Other origins folded into this registrable domain.
    pub alt_urls: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SearchRow {
    pub rowid: i64,
    pub id: String,
    pub kind: String,
    pub name_zh: String,
    pub name_en: String,
    pub aliases: Vec<String>,
    pub category: String,
    pub official_url: String,
    pub verify_status: String,
    pub description: String,
    pub popularity_bucket: Option<i64>,
}

pub enum Recall {
    /// Rowids from an FTS5 trigram query. Empty is not used; that becomes [`Recall::Scan`].
    Trigram(HashSet<i64>),
    /// Full scan: query shorter than 3 characters, or trigram returned nothing.
    Scan,
}

pub fn db_path(store: &Store) -> PathBuf {
    store.root().join("kb").join("kb.sqlite")
}

pub fn attribution_path(store: &Store) -> PathBuf {
    store.root().join("kb").join("ATTRIBUTION.txt")
}

pub fn open(store: &Store) -> Result<Connection, AppError> {
    let path = db_path(store);
    if let Some(parent) = path.parent() {
        store.ensure_dir(parent)?;
    }
    open_path(&path)
}

pub fn open_if_exists(store: &Store) -> Result<Option<Connection>, AppError> {
    let path = db_path(store);
    if !path.exists() {
        return Ok(None);
    }
    open_path(&path).map(Some)
}

fn open_path(path: &Path) -> Result<Connection, AppError> {
    let conn = Connection::open(path).map_err(|e| sql_err(path, e))?;
    conn.busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|e| sql_err(path, e))?;
    migrate(&conn, path)?;
    Ok(conn)
}

fn migrate(conn: &Connection, path: &Path) -> Result<(), AppError> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS kb_record (
          id              TEXT PRIMARY KEY,
          kind            TEXT NOT NULL CHECK (kind IN ('org','service')),
          name_zh         TEXT,
          name_en         TEXT,
          aliases         TEXT,
          category        TEXT,
          official_url    TEXT,
          service_urls    TEXT,
          description     TEXT,
          source          TEXT,
          source_license  TEXT,
          last_verified   TEXT,
          verify_status   TEXT,
          cached_doc      TEXT,
          updated_at      TEXT,
          popularity_bucket INTEGER,
          popularity_scope  TEXT,
          alt_urls          TEXT
        );
        CREATE INDEX IF NOT EXISTS kb_record_category ON kb_record(category);
        CREATE INDEX IF NOT EXISTS kb_record_kind ON kb_record(kind);
        CREATE INDEX IF NOT EXISTS kb_record_source ON kb_record(source);
        CREATE VIRTUAL TABLE IF NOT EXISTS kb_fts USING fts5(
          name_zh, name_en, aliases, category, description, official_url,
          content='kb_record', content_rowid='rowid',
          tokenize='trigram'
        );
        CREATE TRIGGER IF NOT EXISTS kb_record_ai AFTER INSERT ON kb_record BEGIN
          INSERT INTO kb_fts(rowid, name_zh, name_en, aliases, category, description, official_url)
          VALUES (new.rowid, new.name_zh, new.name_en, new.aliases, new.category, new.description, new.official_url);
        END;
        CREATE TRIGGER IF NOT EXISTS kb_record_ad AFTER DELETE ON kb_record BEGIN
          INSERT INTO kb_fts(kb_fts, rowid, name_zh, name_en, aliases, category, description, official_url)
          VALUES ('delete', old.rowid, old.name_zh, old.name_en, old.aliases, old.category, old.description, old.official_url);
        END;
        CREATE TRIGGER IF NOT EXISTS kb_record_au AFTER UPDATE ON kb_record BEGIN
          INSERT INTO kb_fts(kb_fts, rowid, name_zh, name_en, aliases, category, description, official_url)
          VALUES ('delete', old.rowid, old.name_zh, old.name_en, old.aliases, old.category, old.description, old.official_url);
          INSERT INTO kb_fts(rowid, name_zh, name_en, aliases, category, description, official_url)
          VALUES (new.rowid, new.name_zh, new.name_en, new.aliases, new.category, new.description, new.official_url);
        END;
        ",
    )
    .map_err(|e| sql_err(path, e))?;
    // Older databases created before the popular-sites columns.
    for sql in [
        "ALTER TABLE kb_record ADD COLUMN popularity_bucket INTEGER",
        "ALTER TABLE kb_record ADD COLUMN popularity_scope TEXT",
        "ALTER TABLE kb_record ADD COLUMN alt_urls TEXT",
    ] {
        let _ = conn.execute(sql, []);
    }
    Ok(())
}

fn sql_err(path: &Path, err: rusqlite::Error) -> AppError {
    AppError::new(
        "kb_error",
        format!("knowledge base {}: {err}", path.display()),
        "If the database is corrupt, delete $AGENTBOX_HOME/kb/kb.sqlite and rerun `agentbox kb import --curated`.",
    )
}

pub fn count(conn: &Connection) -> Result<i64, AppError> {
    conn.query_row("SELECT COUNT(*) FROM kb_record", [], |r| r.get(0))
        .map_err(|e| sql_err(Path::new("kb.sqlite"), e))
}

pub fn count_source(conn: &Connection, source: &str) -> Result<i64, AppError> {
    conn.query_row(
        "SELECT COUNT(*) FROM kb_record WHERE source = ?1",
        params![source],
        |r| r.get(0),
    )
    .map_err(|e| sql_err(Path::new("kb.sqlite"), e))
}

/// FTS row count, used to prove the trigram index is maintained.
#[cfg(test)]
pub fn fts_count(conn: &Connection) -> Result<i64, AppError> {
    conn.query_row("SELECT COUNT(*) FROM kb_fts", [], |r| r.get(0))
        .map_err(|e| sql_err(Path::new("kb.sqlite"), e))
}

pub fn recall(conn: &Connection, norm: &str) -> Result<Recall, AppError> {
    if norm.chars().count() < 3 {
        return Ok(Recall::Scan);
    }
    match trigram_rowids(conn, norm) {
        Ok(ids) if ids.is_empty() => Ok(Recall::Scan),
        Ok(ids) => Ok(Recall::Trigram(ids.into_iter().collect())),
        Err(e) if short_trigram_query(&e.message) => Ok(Recall::Scan),
        Err(e) => Err(e),
    }
}

fn short_trigram_query(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("trigram") || m.contains("too short") || m.contains("no such column")
}

fn trigram_rowids(conn: &Connection, norm: &str) -> Result<Vec<i64>, AppError> {
    // Quote the whole string so it is one phrase. Trigram then matches it as
    // a substring of at least 3 characters.
    let q = format!("\"{}\"", norm.replace('"', ""));
    let mut stmt = conn
        .prepare("SELECT rowid FROM kb_fts WHERE kb_fts MATCH ?1")
        .map_err(|e| sql_err(Path::new("kb.sqlite"), e))?;
    let rows = stmt
        .query_map(params![q], |r| r.get(0))
        .map_err(|e| sql_err(Path::new("kb.sqlite"), e))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| sql_err(Path::new("kb.sqlite"), e))?);
    }
    Ok(out)
}

pub fn load_search_rows(
    conn: &Connection,
    category: &str,
    kind: &str,
) -> Result<Vec<SearchRow>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT rowid, id, kind, name_zh, name_en, aliases, category, official_url,
                    verify_status, description, popularity_bucket
             FROM kb_record
             WHERE (?1 = '' OR category = ?1) AND (?2 = 'all' OR kind = ?2)",
        )
        .map_err(|e| sql_err(Path::new("kb.sqlite"), e))?;
    let rows = stmt
        .query_map(params![category, kind], |r| {
            Ok(SearchRow {
                rowid: r.get(0)?,
                id: r.get(1)?,
                kind: r.get(2)?,
                name_zh: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                name_en: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                aliases: parse_string_list(&r.get::<_, Option<String>>(5)?.unwrap_or_default()),
                category: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                official_url: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                verify_status: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
                description: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
                popularity_bucket: r.get(10)?,
            })
        })
        .map_err(|e| sql_err(Path::new("kb.sqlite"), e))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| sql_err(Path::new("kb.sqlite"), e))?);
    }
    Ok(out)
}

pub fn get(conn: &Connection, id: &str) -> Result<Option<Record>, AppError> {
    conn.query_row(
        "SELECT id, kind, name_zh, name_en, aliases, category, official_url, service_urls,
                description, source, source_license, last_verified, verify_status, cached_doc,
                updated_at, popularity_bucket, popularity_scope, alt_urls
         FROM kb_record WHERE id = ?1",
        params![id],
        |r| {
            Ok(Record {
                id: r.get(0)?,
                kind: r.get(1)?,
                name_zh: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                name_en: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                aliases: parse_string_list(&r.get::<_, Option<String>>(4)?.unwrap_or_default()),
                category: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                official_url: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                service_urls: parse_service_urls(
                    &r.get::<_, Option<String>>(7)?.unwrap_or_default(),
                ),
                description: r.get::<_, Option<String>>(8)?.unwrap_or_default(),
                source: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
                source_license: r.get::<_, Option<String>>(10)?.unwrap_or_default(),
                last_verified: r.get::<_, Option<String>>(11)?.unwrap_or_default(),
                verify_status: r.get::<_, Option<String>>(12)?.unwrap_or_default(),
                cached_doc: r.get::<_, Option<String>>(13)?.unwrap_or_default(),
                updated_at: r.get::<_, Option<String>>(14)?.unwrap_or_default(),
                popularity_bucket: r.get(15)?,
                popularity_scope: r.get::<_, Option<String>>(16)?.unwrap_or_default(),
                alt_urls: parse_string_list(&r.get::<_, Option<String>>(17)?.unwrap_or_default()),
            })
        },
    )
    .optional()
    .map_err(|e| sql_err(Path::new("kb.sqlite"), e))
}

pub fn upsert(conn: &Connection, rec: &Record) -> Result<(), AppError> {
    let aliases = serde_json::to_string(&rec.aliases).unwrap_or_else(|_| "[]".into());
    let service_urls = serde_json::to_string(&rec.service_urls).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO kb_record (
            id, kind, name_zh, name_en, aliases, category, official_url, service_urls,
            description, source, source_license, last_verified, verify_status, cached_doc, updated_at,
            popularity_bucket, popularity_scope, alt_urls
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)
         ON CONFLICT(id) DO UPDATE SET
            kind = excluded.kind,
            name_zh = excluded.name_zh,
            name_en = excluded.name_en,
            aliases = excluded.aliases,
            category = excluded.category,
            official_url = excluded.official_url,
            service_urls = excluded.service_urls,
            description = excluded.description,
            source = excluded.source,
            source_license = excluded.source_license,
            last_verified = excluded.last_verified,
            verify_status = excluded.verify_status,
            cached_doc = CASE
                WHEN excluded.official_url = kb_record.official_url THEN kb_record.cached_doc
                ELSE ''
            END,
            updated_at = excluded.updated_at,
            popularity_bucket = excluded.popularity_bucket,
            popularity_scope = excluded.popularity_scope,
            alt_urls = excluded.alt_urls",
        params![
            rec.id,
            rec.kind,
            rec.name_zh,
            rec.name_en,
            aliases,
            rec.category,
            rec.official_url,
            service_urls,
            rec.description,
            rec.source,
            rec.source_license,
            rec.last_verified,
            rec.verify_status,
            rec.cached_doc,
            rec.updated_at,
            rec.popularity_bucket,
            rec.popularity_scope,
            serde_json::to_string(&rec.alt_urls).unwrap_or_else(|_| "[]".into()),
        ],
    )
    .map(|_| ())
    .map_err(|e| sql_err(Path::new("kb.sqlite"), e))
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MergeRow {
    pub id: String,
    pub source: String,
    pub official_url: String,
    pub aliases: Vec<String>,
    pub alt_urls: Vec<String>,
    pub popularity_bucket: Option<i64>,
    pub popularity_scope: String,
}

pub fn list_merge_rows(conn: &Connection) -> Result<Vec<MergeRow>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, source, official_url, aliases, alt_urls, popularity_bucket, popularity_scope
             FROM kb_record",
        )
        .map_err(|e| sql_err(Path::new("kb.sqlite"), e))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(MergeRow {
                id: r.get(0)?,
                source: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                official_url: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                aliases: parse_string_list(&r.get::<_, Option<String>>(3)?.unwrap_or_default()),
                alt_urls: parse_string_list(&r.get::<_, Option<String>>(4)?.unwrap_or_default()),
                popularity_bucket: r.get(5)?,
                popularity_scope: r.get::<_, Option<String>>(6)?.unwrap_or_default(),
            })
        })
        .map_err(|e| sql_err(Path::new("kb.sqlite"), e))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| sql_err(Path::new("kb.sqlite"), e))?);
    }
    Ok(out)
}

/// Attach popular-list aliases and a CrUX bucket onto a row that already won
/// (usually a curated org). Does not change its name or official URL.
pub fn attach_popularity(
    conn: &Connection,
    id: &str,
    aliases: &[String],
    alt_urls: &[String],
    bucket: Option<i64>,
    scope: &str,
) -> Result<(), AppError> {
    conn.execute(
        "UPDATE kb_record SET aliases = ?1, alt_urls = ?2,
            popularity_bucket = CASE
                WHEN popularity_bucket IS NULL THEN ?3
                WHEN ?3 IS NULL THEN popularity_bucket
                WHEN ?3 < popularity_bucket THEN ?3
                ELSE popularity_bucket
            END,
            popularity_scope = CASE
                WHEN popularity_scope IS NULL OR popularity_scope = '' THEN ?4
                ELSE popularity_scope
            END
         WHERE id = ?5",
        params![
            serde_json::to_string(aliases).unwrap_or_else(|_| "[]".into()),
            serde_json::to_string(alt_urls).unwrap_or_else(|_| "[]".into()),
            bucket,
            scope,
            id,
        ],
    )
    .map(|_| ())
    .map_err(|e| sql_err(Path::new("kb.sqlite"), e))
}

pub fn delete_source(conn: &Connection, source: &str) -> Result<usize, AppError> {
    conn.execute("DELETE FROM kb_record WHERE source = ?1", params![source])
        .map_err(|e| sql_err(Path::new("kb.sqlite"), e))
}

pub fn set_cached_doc(conn: &Connection, id: &str, cached_doc: &str) -> Result<(), AppError> {
    let now = now_stamp();
    conn.execute(
        "UPDATE kb_record SET cached_doc = ?1, updated_at = ?2 WHERE id = ?3",
        params![cached_doc, now, id],
    )
    .map(|_| ())
    .map_err(|e| sql_err(Path::new("kb.sqlite"), e))
}

pub fn rebuild_fts(conn: &Connection) -> Result<(), AppError> {
    conn.execute("INSERT INTO kb_fts(kb_fts) VALUES('rebuild')", [])
        .map(|_| ())
        .map_err(|e| sql_err(Path::new("kb.sqlite"), e))
}

pub fn now_stamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn parse_string_list(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

fn parse_service_urls(raw: &str) -> Vec<ServiceUrl> {
    serde_json::from_str(raw).unwrap_or_default()
}

impl Record {
    pub fn blank(id: impl Into<String>, kind: &str) -> Self {
        Self {
            id: id.into(),
            kind: kind.to_string(),
            name_zh: String::new(),
            name_en: String::new(),
            aliases: Vec::new(),
            category: String::new(),
            official_url: String::new(),
            service_urls: Vec::new(),
            description: String::new(),
            source: String::new(),
            source_license: String::new(),
            last_verified: String::new(),
            verify_status: String::new(),
            cached_doc: String::new(),
            updated_at: now_stamp(),
            popularity_bucket: None,
            popularity_scope: String::new(),
            alt_urls: Vec::new(),
        }
    }
}

pub fn record_value(rec: &Record) -> Value {
    serde_json::json!({
        "id": rec.id,
        "kind": rec.kind,
        "name_zh": rec.name_zh,
        "name_en": rec.name_en,
        "aliases": rec.aliases,
        "category": rec.category,
        "official_url": rec.official_url,
        "service_urls": rec.service_urls,
        "description": rec.description,
        "source": rec.source,
        "source_license": rec.source_license,
        "last_verified": rec.last_verified,
        "verify_status": rec.verify_status,
        "cached_doc": rec.cached_doc,
        "updated_at": rec.updated_at,
        "popularity_bucket": rec.popularity_bucket,
        "popularity_scope": rec.popularity_scope,
        "alt_urls": rec.alt_urls,
    })
}
