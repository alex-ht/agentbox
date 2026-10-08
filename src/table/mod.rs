//! `table`: show / query / import / export tables stored as `tbl:N` handles
//! or read from CSV, TSV, JSON and Markdown files.
//!
//! Stored tables live in `$AGENTBOX_HOME/tables/<N>.json`.

pub mod parse;
pub mod query;
pub mod value;

#[cfg(test)]
mod tests;

use crate::envelope::{AppError, CmdResult, Output};
use crate::state::Store;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// Default number of rows shown by `table show` / `table query`.
pub const DEFAULT_LIMIT: usize = 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Table {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub title: String,
    /// Where the data came from: URL, file path or doc handle(s).
    #[serde(default)]
    pub source: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    #[serde(default)]
    pub created: String,
}

impl Table {
    pub fn new(title: &str, source: &str, parsed: parse::Parsed) -> Self {
        Table {
            id: 0,
            title: title.to_string(),
            source: source.to_string(),
            columns: parsed.columns,
            rows: parsed.rows,
            created: String::new(),
        }
    }

    /// Rows as objects keyed by column name (what agents read most easily).
    pub fn objects(&self, limit: usize) -> Vec<Value> {
        self.rows
            .iter()
            .take(limit)
            .map(|r| {
                let m: Map<String, Value> = self
                    .columns
                    .iter()
                    .cloned()
                    .zip(r.iter().cloned())
                    .collect();
                Value::Object(m)
            })
            .collect()
    }

    pub fn column_types(&self) -> Vec<Value> {
        self.columns
            .iter()
            .enumerate()
            .map(|(i, c)| json!({"name": c, "type": value::infer_type(self.rows.iter().map(|r| &r[i]))}))
            .collect()
    }

    pub fn handle(&self) -> Option<String> {
        (self.id > 0).then(|| format!("tbl:{}", self.id))
    }
}

fn tables_dir(store: &Store) -> PathBuf {
    store.root().join("tables")
}

fn table_ids(store: &Store) -> Vec<u64> {
    let mut ids: Vec<u64> = fs::read_dir(tables_dir(store))
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .strip_suffix(".json")?
                        .parse::<u64>()
                        .ok()
                })
                .collect()
        })
        .unwrap_or_default();
    ids.sort_unstable();
    ids
}

/// Store a table and return its id. An identical table already stored
/// (same source, columns and rows) is reused instead of duplicated.
pub fn save(store: &Store, mut t: Table) -> Result<u64, AppError> {
    let dir = tables_dir(store);
    store.ensure_dir(&dir)?;
    let ids = table_ids(store);
    for &id in ids.iter().rev().take(200) {
        if let Ok(old) = load_id(store, id) {
            if old.source == t.source && old.columns == t.columns && old.rows == t.rows {
                return Ok(id);
            }
        }
    }
    let id = ids.last().copied().unwrap_or(0) + 1;
    t.id = id;
    t.created = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let text = serde_json::to_string(&t).expect("table serializes");
    fs::write(dir.join(format!("{id}.json")), text).map_err(|e| AppError::io("write table", e))?;
    Ok(id)
}

fn load_id(store: &Store, id: u64) -> Result<Table, AppError> {
    let path = tables_dir(store).join(format!("{id}.json"));
    let text = fs::read_to_string(&path).map_err(|_| {
        let ids = table_ids(store);
        let hint = if ids.is_empty() {
            "No tables stored yet. Create one with `agentbox extract doc:N --kind tables` or `agentbox table import FILE`.".to_string()
        } else {
            let recent: Vec<String> = ids.iter().rev().take(5).map(|i| format!("tbl:{i}")).collect();
            format!("Recent tables: {}.", recent.join(", "))
        };
        AppError::new("table_not_found", format!("tbl:{id} does not exist"), hint)
    })?;
    let mut t: Table = serde_json::from_str(&text).map_err(|e| {
        AppError::new(
            "state_error",
            format!("tbl:{id} is corrupt: {e}"),
            "Re-create it with `agentbox table import` or `agentbox extract`.",
        )
    })?;
    t.id = id;
    Ok(t)
}

fn table_handle(s: &str) -> Option<Result<u64, AppError>> {
    let t = s.trim();
    if t.len() > 4 && t[..4].eq_ignore_ascii_case("tbl:") {
        Some(t[4..].parse::<u64>().map_err(|_| {
            AppError::new(
                "bad_handle",
                format!("`{s}` is not a table handle"),
                "Table handles look like `tbl:3`.",
            )
        }))
    } else {
        None
    }
}

/// Load a `tbl:N` handle or parse a CSV/TSV/JSON/Markdown file.
pub fn load(store: &Store, src: &str) -> Result<Table, AppError> {
    if let Some(id) = table_handle(src) {
        return load_id(store, id?);
    }
    if src.trim().len() > 4 && src.trim()[..4].eq_ignore_ascii_case("doc:") {
        return Err(AppError::new(
            "bad_args",
            format!("`{src}` is a doc, not a table"),
            format!("Pull its tables out first with `agentbox extract {src} --kind tables`, then use the returned tbl:N."),
        ));
    }
    let text = fs::read_to_string(src).map_err(|e| AppError::io(&format!("read {src}"), e))?;
    let (parsed, _) = parse_file(src, &text)?;
    let title = Path::new(src)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    Ok(Table::new(&title, src, parsed))
}

/// Parse file text by extension (sniffing when unknown). Returns the table
/// and how many tables the file had (Markdown can hold several).
pub fn parse_file(path: &str, text: &str) -> Result<(parse::Parsed, usize), AppError> {
    let ext = Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let bad = |msg: String| {
        AppError::new(
            "bad_table",
            format!("{path}: {msg}"),
            "Supported: .csv, .tsv, .json (array of objects), .md (pipe table). Check the file with `agentbox file read`.",
        )
    };
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    let kind = match ext.as_str() {
        "csv" => "csv",
        "tsv" | "tab" => "tsv",
        "json" => "json",
        "md" | "markdown" => "md",
        _ if trimmed.starts_with('[') || trimmed.starts_with('{') => "json",
        _ if !parse::md_tables(text).is_empty() => "md",
        _ => "csv",
    };
    match kind {
        "csv" => parse::parse_csv(text, None).map(|p| (p, 1)).map_err(bad),
        "tsv" => parse::parse_csv(text, Some('\t'))
            .map(|p| (p, 1))
            .map_err(bad),
        "json" => parse::parse_json(text).map(|p| (p, 1)).map_err(bad),
        _ => {
            let tables = parse::md_tables(text);
            let first = tables
                .first()
                .ok_or_else(|| bad("no Markdown pipe table found".into()))?;
            Ok((first.to_parsed(&crate::extract::plain), tables.len()))
        }
    }
}

fn summary(t: &Table, limit: usize) -> Map<String, Value> {
    let mut m = Map::new();
    if let Some(h) = t.handle() {
        m.insert("table".into(), json!(h));
    }
    m.insert("title".into(), json!(t.title));
    m.insert("source".into(), json!(t.source));
    m.insert("row_count".into(), json!(t.rows.len()));
    m.insert("columns".into(), Value::Array(t.column_types()));
    m.insert("rows".into(), Value::Array(t.objects(limit)));
    if t.rows.len() > limit {
        m.insert("truncated".into(), json!(true));
    }
    m
}

fn example_query(src: &str, t: &Table) -> String {
    let types = t.column_types();
    let num = types
        .iter()
        .find(|c| matches!(c["type"].as_str(), Some("number" | "currency")))
        .and_then(|c| c["name"].as_str());
    match num {
        Some(c) => format!(
            "agentbox table query {src} --sort {} --limit 5",
            shell_arg(&format!("-{c}"))
        ),
        None => format!(
            "agentbox table query {src} --where {}",
            shell_arg(&format!(
                "{} contains KEYWORD",
                t.columns.first().map_or("Column", String::as_str)
            ))
        ),
    }
}

/// Quote a shell argument for the hint: bare when safe, single quotes when
/// it holds `$`, backticks or double quotes (so bash/PowerShell keep it
/// literal), double quotes otherwise.
pub fn shell_arg(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || "-_.:/,+=@%".contains(c));
    if safe {
        s.to_string()
    } else if s.contains(['$', '`', '"']) && !s.contains('\'') {
        format!("'{s}'")
    } else {
        format!("\"{}\"", s.replace('"', "\\\""))
    }
}

pub fn run_show(store: &Store, src: &str, limit: usize) -> CmdResult {
    let t = load(store, src)?;
    let data = summary(&t, limit);
    let mut hint = format!("Filter/sort with `{}`.", example_query(src, &t));
    if t.rows.len() > limit {
        hint = format!("Showing {limit} of {} rows. {hint}", t.rows.len());
    }
    Ok(Output::new(Value::Object(data)).hint(hint))
}

pub fn run_import(store: &Store, file: &str) -> CmdResult {
    let text = fs::read_to_string(file).map_err(|e| AppError::io(&format!("read {file}"), e))?;
    let (parsed, count) = parse_file(file, &text)?;
    let title = Path::new(file)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut t = Table::new(&title, file, parsed);
    t.id = save(store, t.clone())?;
    let handle = t.handle().unwrap_or_default();
    let mut data = summary(&t, 5);
    let mut hint = format!(
        "Imported as {handle}. Next: `{}`.",
        example_query(&handle, &t)
    );
    if count > 1 {
        data.insert("tables_in_file".into(), json!(count));
        hint = format!("{hint} The file has {count} tables; only the first was imported. Import all with `agentbox extract {file} --kind tables`.");
    }
    Ok(Output::new(Value::Object(data)).hint(hint))
}

pub struct QueryArgs {
    pub source: String,
    pub query: query::Query,
    pub limit: Option<usize>,
    pub save: bool,
}

pub fn run_query(store: &Store, a: &QueryArgs) -> CmdResult {
    let t = load(store, &a.source)?;
    let mut res = query::run(&t, &a.query)?;
    let total = res.rows.len();
    let shown = a.limit.unwrap_or(DEFAULT_LIMIT);
    let mut m = Map::new();
    m.insert("source".into(), json!(a.source));
    if a.save {
        let mut saved = res.clone();
        if let Some(l) = a.limit {
            saved.rows.truncate(l);
        }
        saved.title = format!("query of {}", a.source);
        saved.source = t.handle().unwrap_or_else(|| t.source.clone());
        res.id = save(store, saved)?;
        m.insert("table".into(), json!(res.handle()));
    }
    m.insert("columns".into(), json!(res.columns));
    m.insert("row_count".into(), json!(total));
    m.insert("returned".into(), json!(total.min(shown)));
    m.insert("rows".into(), Value::Array(res.objects(shown)));
    if total > shown {
        m.insert("truncated".into(), json!(true));
    }
    let hint = if total == 0 {
        format!(
            "No rows matched. Check values with `agentbox table show {}` (where compares text case-insensitively; numbers and dates by value).",
            a.source
        )
    } else if total > shown {
        format!("Showing {shown} of {total} rows; raise --limit or add --where to narrow.")
    } else if let Some(h) = res.handle() {
        format!("Saved as {h}. Export with `agentbox table export {h} --out result.csv`.")
    } else {
        "Add --save to keep this result as a new tbl:N, or --format csv/md for other output.".into()
    };
    Ok(Output::new(Value::Object(m)).hint(hint))
}

fn csv_field(s: &str, delim: char) -> String {
    if s.contains(delim) || s.contains('"') || s.contains('\n') || s.contains('\r') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// CSV (or TSV) text of a table.
pub fn to_delimited(columns: &[String], rows: &[Vec<Value>], delim: char) -> String {
    let mut out = String::new();
    let line = |cells: Vec<String>| {
        cells
            .iter()
            .map(|c| csv_field(c, delim))
            .collect::<Vec<_>>()
            .join(&delim.to_string())
    };
    out.push_str(&line(columns.to_vec()));
    out.push('\n');
    for r in rows {
        out.push_str(&line(r.iter().map(value::cell_str).collect()));
        out.push('\n');
    }
    out
}

/// Markdown pipe table text.
pub fn to_markdown(columns: &[String], rows: &[Vec<Value>]) -> String {
    let esc = |s: &str| s.replace('|', "\\|").replace('\n', " ");
    let mut out = format!(
        "| {} |\n|{}\n",
        columns
            .iter()
            .map(|c| esc(c))
            .collect::<Vec<_>>()
            .join(" | "),
        " --- |".repeat(columns.len())
    );
    for r in rows {
        out.push_str(&format!(
            "| {} |\n",
            r.iter()
                .map(|c| esc(&value::cell_str(c)))
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    out
}

pub fn run_export(store: &Store, src: &str, out: &str, apply: bool) -> CmdResult {
    let t = load(store, src)?;
    let ext = Path::new(out)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let content = match ext.as_str() {
        "csv" => to_delimited(&t.columns, &t.rows, ','),
        "tsv" => to_delimited(&t.columns, &t.rows, '\t'),
        "md" | "markdown" => to_markdown(&t.columns, &t.rows),
        "json" => serde_json::to_string_pretty(&Value::Array(t.objects(usize::MAX)))
            .expect("rows serialize")
            + "\n",
        _ => {
            return Err(AppError::new(
                "bad_args",
                format!("cannot tell the export format from `{out}`"),
                format!("Use an --out path ending in .csv, .tsv, .md or .json, e.g. `agentbox table export {src} --out table.csv`."),
            ))
        }
    };
    let res = crate::cmd::file::write(out, &content, apply)?;
    let mut data =
        json!({"source": src, "format": ext, "rows": t.rows.len(), "columns": t.columns});
    if let (Value::Object(d), Value::Object(w)) = (&mut data, res.data) {
        for (k, v) in w {
            d.insert(k, v);
        }
    }
    Ok(Output {
        data,
        hint: res.hint,
    })
}

/// `--format csv`: plain CSV for table-shaped output (`columns` + `rows`).
pub fn envelope_csv(data: &Value) -> Option<String> {
    let cols: Vec<String> = data
        .get("columns")?
        .as_array()?
        .iter()
        .map(|c| match c {
            Value::String(s) => s.clone(),
            other => other
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        })
        .collect();
    let rows: Vec<Vec<Value>> = data
        .get("rows")?
        .as_array()?
        .iter()
        .map(|r| {
            cols.iter()
                .map(|c| r.get(c).cloned().unwrap_or(Value::Null))
                .collect()
        })
        .collect();
    Some(to_delimited(&cols, &rows, ','))
}
