//! `extract`: pull structured items out of stored docs or local files.
//! Deterministic pattern matching only; no model in the loop.

pub mod dates;
pub mod kinds;
pub mod people;

#[cfg(test)]
mod tests;

use crate::envelope::{AppError, CmdResult, Output};
use crate::markdown;
use crate::state::Store;
use crate::table::{self, parse, value::num_value, Table};
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::path::Path;
use std::sync::LazyLock;

pub const KINDS: &[&str] = &[
    "tables", "prices", "dates", "people", "links", "numbers", "emails",
];
const CONTEXT_CHARS: usize = 120;
const PREVIEW_ROWS: usize = 5;

static LINK_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!?\[([^\]]*)\]\([^)]*\)").expect("valid regex"));
static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#{1,6}\s+").expect("valid regex"));
static REF_MARK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[(?:\d{1,3}|[a-z]|note \d+|citation needed|update)\]").expect("valid regex")
});
static ESCAPED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\([\\`*_{}()#+\-.!<>~])").expect("valid regex"));
static PART: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r" \(part \d+/\d+\)$").expect("valid regex"));
static BULLET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[-*+]\s+").expect("valid regex"));

/// Markdown line -> readable text: link text kept, URLs, emphasis, heading
/// marks and outer table pipes dropped, whitespace collapsed.
pub fn plain(s: &str) -> String {
    let t = s.trim();
    let t = HEADING.replace(t, "");
    let t = t.strip_prefix("> ").unwrap_or(&t).to_string();
    let t = BULLET.replace(&t, "").to_string();
    // Escaped brackets (`\[15\]`) must not break link matching.
    let t = t.replace("\\[", "\u{27e6}").replace("\\]", "\u{27e7}");
    let t = LINK_TEXT.replace_all(&t, "$1");
    let t = t.replace('\u{27e6}', "[").replace('\u{27e7}', "]");
    let t = ESCAPED.replace_all(&t, "$1");
    // Wikipedia-style reference marks: "[1]", "[a]", "[citation needed]".
    let t = REF_MARK.replace_all(&t, "");
    let mut t = t.replace("**", "").replace("__", "").replace('`', "");
    if t.starts_with('|') {
        t = t.trim_matches('|').to_string();
    }
    let t = t.replace("\\|", "|");
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub struct ExtractArgs {
    pub sources: Vec<String>,
    pub from: Option<String>,
    pub kind: String,
    pub section: Option<usize>,
    pub grep: Option<String>,
    pub limit: usize,
    pub site: Option<String>,
    pub save_table: bool,
}

struct Source {
    label: String,
    url: Option<String>,
    title: String,
    body: String,
}

impl Source {
    fn host(&self) -> Option<String> {
        self.url
            .as_deref()
            .and_then(|u| reqwest::Url::parse(u).ok())
            .and_then(|u| u.host_str().map(str::to_string))
    }
}

fn is_doc(s: &str) -> bool {
    s.len() > 4 && s[..4].eq_ignore_ascii_case("doc:")
}

fn load_source(store: &Store, s: &str) -> Result<Source, AppError> {
    if is_doc(s) {
        let (meta, body) = store.load_doc(s)?;
        return Ok(Source {
            label: format!("doc:{}", meta.id),
            url: (!meta.url.is_empty()).then_some(meta.url),
            title: meta.title,
            body,
        });
    }
    let text = std::fs::read_to_string(s).map_err(|e| {
        let mut err = AppError::io(&format!("read {s}"), e);
        err.hint = format!(
            "Pass a doc handle like doc:3 (from `agentbox fetch`) or an existing file path. {}",
            err.hint
        );
        err
    })?;
    let ext = Path::new(s)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let (title, body) = if ext == "html" || ext == "htm" {
        (
            markdown::extract_title(&text).unwrap_or_default(),
            markdown::html_to_markdown(&text, None),
        )
    } else {
        (String::new(), text)
    };
    Ok(Source {
        label: s.to_string(),
        url: None,
        title,
        body,
    })
}

/// An extracted item with its dedupe key.
type Keyed = (String, Map<String, Value>);

struct Line {
    no: usize,
    section: usize,
    raw: String,
    plain: String,
}

/// Lines outside code fences, tagged with their section number (the same
/// numbering as `read --section`).
fn lines_of(
    src: &Source,
    only: Option<usize>,
) -> Result<(Vec<Line>, Vec<markdown::Section>), AppError> {
    let sections = markdown::split_sections(&src.body);
    if let Some(n) = only {
        if n == 0 || n > sections.len() {
            return Err(AppError::new(
                "bad_args",
                format!(
                    "{} has {} sections; --section {n} does not exist",
                    src.label,
                    sections.len()
                ),
                format!(
                    "See the outline with `agentbox read {} --max-chars 1` or drop --section.",
                    src.label
                ),
            ));
        }
    }
    let mut out = Vec::new();
    let mut offset = 0;
    let mut in_fence = false;
    for (i, line) in src.body.split_inclusive('\n').enumerate() {
        let start = offset;
        offset += line.len();
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || line.trim().is_empty() {
            continue;
        }
        let section = sections
            .iter()
            .find(|s| start >= s.start && start < s.end)
            .map_or(0, |s| s.index);
        if only.is_some_and(|n| n != section) {
            continue;
        }
        out.push(Line {
            no: i + 1,
            section,
            raw: line.trim_end().to_string(),
            plain: plain(line),
        });
    }
    Ok((out, sections))
}

/// About CONTEXT_CHARS characters of `text` around the byte range.
pub fn context(text: &str, start: usize, end: usize) -> String {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len();
    if n <= CONTEXT_CHARS {
        return text.trim().to_string();
    }
    let si = chars.iter().position(|(b, _)| *b >= start).unwrap_or(n);
    let ei = chars
        .iter()
        .position(|(b, _)| *b >= end)
        .unwrap_or(n)
        .max(si);
    let width = CONTEXT_CHARS.max(ei - si);
    let mut a = si.saturating_sub((width - (ei - si)) / 2);
    let b = (a + width).min(n);
    a = a.min(b.saturating_sub(width));
    let mut s: String = chars[a..b].iter().map(|(_, c)| c).collect();
    s = s.trim().to_string();
    if a > 0 {
        s = format!("…{s}");
    }
    if b < n {
        s.push('…');
    }
    s
}

fn grep_ok(item: &Map<String, Value>, kw: Option<&str>) -> bool {
    let Some(kw) = kw else { return true };
    let kw = kw.to_lowercase();
    item.values().any(|v| match v {
        Value::String(s) => s.to_lowercase().contains(&kw),
        Value::Array(a) => a.iter().any(|x| x.to_string().to_lowercase().contains(&kw)),
        _ => false,
    })
}

fn site_ok(url: &str, site: &str) -> bool {
    let site = site.trim().trim_start_matches("www.").to_lowercase();
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| h.trim_start_matches("www.").to_lowercase())
        })
        .is_some_and(|h| h == site || h.ends_with(&format!(".{site}")))
}

/// Items of one kind from one source (before grep/limit), with dedupe keys.
fn items_of(store: &Store, src: &Source, a: &ExtractArgs) -> Result<Vec<Keyed>, AppError> {
    let (lines, sections) = lines_of(src, a.section)?;
    let host = src.host();
    let base = src.url.as_deref().and_then(|u| reqwest::Url::parse(u).ok());
    let mut out: Vec<Keyed> = Vec::new();
    let tail = |m: &mut Map<String, Value>, l: &Line| {
        m.insert("section".into(), json!(l.section));
        m.insert("line".into(), json!(l.no));
        m.insert("doc".into(), json!(src.label));
        m.insert("url".into(), json!(src.url));
    };
    match a.kind.as_str() {
        "tables" => return tables_of(store, src, a, &lines, &sections),
        "prices" => {
            for l in &lines {
                for h in kinds::find_prices(&l.plain, host.as_deref()) {
                    let mut m = Map::new();
                    m.insert("value".into(), num_value(h.value));
                    m.insert("currency".into(), json!(h.currency));
                    m.insert("period".into(), json!(h.period));
                    m.insert("per".into(), json!(h.per));
                    m.insert("raw".into(), json!(h.raw));
                    m.insert("context".into(), json!(context(&l.plain, h.start, h.end)));
                    tail(&mut m, l);
                    let key = format!("{}|{:?}|{:?}|{:?}", h.value, h.currency, h.period, h.per);
                    out.push((key, m));
                }
            }
        }
        "dates" => {
            for l in &lines {
                for h in dates::find_dates(&l.plain) {
                    let mut m = Map::new();
                    m.insert("date".into(), json!(h.iso));
                    m.insert("precision".into(), json!(h.precision));
                    if h.ambiguous {
                        m.insert("ambiguous".into(), json!(true));
                    }
                    m.insert("raw".into(), json!(&l.plain[h.start..h.end]));
                    m.insert("context".into(), json!(context(&l.plain, h.start, h.end)));
                    tail(&mut m, l);
                    out.push((h.iso.clone(), m));
                }
            }
        }
        "people" => {
            for l in &lines {
                for h in people::find_people(&l.plain) {
                    let mut m = Map::new();
                    m.insert("name".into(), json!(h.name));
                    m.insert("role".into(), json!(h.role));
                    m.insert("org".into(), json!(h.org));
                    m.insert("confidence".into(), json!(h.confidence));
                    m.insert("context".into(), json!(context(&l.plain, h.start, h.end)));
                    tail(&mut m, l);
                    out.push((
                        format!("{}|{}", h.name.to_lowercase(), h.role.to_lowercase()),
                        m,
                    ));
                }
            }
        }
        "links" => {
            for l in &lines {
                for h in kinds::find_links(&l.raw, base.as_ref()) {
                    if a.site.as_deref().is_some_and(|s| !site_ok(&h.url, s)) {
                        continue;
                    }
                    let mut m = Map::new();
                    m.insert("text".into(), json!(h.text));
                    m.insert("url".into(), json!(h.url));
                    m.insert("section".into(), json!(l.section));
                    m.insert("line".into(), json!(l.no));
                    m.insert("doc".into(), json!(src.label));
                    m.insert("source".into(), json!(src.url));
                    out.push((crate::cmd::search::normalize_url(&h.url), m));
                }
            }
        }
        "numbers" => {
            for l in &lines {
                for h in kinds::find_numbers(&l.plain, host.as_deref()) {
                    let mut m = Map::new();
                    m.insert("value".into(), num_value(h.value));
                    m.insert("unit".into(), json!(h.unit));
                    m.insert("raw".into(), json!(h.raw));
                    m.insert("context".into(), json!(context(&l.plain, h.start, h.end)));
                    tail(&mut m, l);
                    out.push((
                        format!("{}|{:?}|{}", h.value, h.unit, h.raw.to_lowercase()),
                        m,
                    ));
                }
            }
        }
        "emails" => {
            for l in &lines {
                for (e, s, en) in kinds::find_emails(&l.plain) {
                    let mut m = Map::new();
                    m.insert("email".into(), json!(e));
                    m.insert("context".into(), json!(context(&l.plain, s, en)));
                    tail(&mut m, l);
                    out.push((e.to_lowercase(), m));
                }
            }
        }
        other => {
            return Err(AppError::new(
                "bad_args",
                format!("unknown kind `{other}`"),
                format!("Kinds: {}.", KINDS.join(", ")),
            ))
        }
    }
    Ok(out)
}

/// HTML `<table>` fragments left inside a Markdown body (e.g. raw page text
/// from a search API), converted to pipe tables, with their line numbers.
fn html_fragments(body: &str, base: Option<&str>) -> Vec<(usize, parse::MdTable)> {
    let lower = body.to_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(p) = lower[from..].find("<table") {
        let s = from + p;
        let Some(e) = lower[s..].find("</table>").map(|e| s + e + 8) else {
            break;
        };
        let line = body[..s].matches('\n').count() + 1;
        let md = markdown::html_to_markdown(&body[s..e], base);
        for t in parse::md_tables(&md) {
            out.push((line, t));
        }
        from = e;
    }
    out
}

fn tables_of(
    store: &Store,
    src: &Source,
    a: &ExtractArgs,
    lines: &[Line],
    sections: &[markdown::Section],
) -> Result<Vec<Keyed>, AppError> {
    let allowed: HashSet<usize> = lines.iter().map(|l| l.no).collect();
    let mut found: Vec<(usize, parse::MdTable)> = parse::md_tables(&src.body)
        .into_iter()
        .map(|t| (t.line, t))
        .collect();
    found.extend(html_fragments(&src.body, src.url.as_deref()));
    found.sort_by_key(|(l, _)| *l);
    let mut out = Vec::new();
    let mut offsets = vec![0usize];
    for line in src.body.split_inclusive('\n') {
        offsets.push(offsets.last().copied().unwrap_or(0) + line.len());
    }
    for (line, t) in found {
        if !allowed.contains(&line) {
            continue;
        }
        let parsed = t.to_parsed(&plain);
        if parsed.rows.is_empty() || parsed.columns.is_empty() {
            continue;
        }
        let off = offsets.get(line - 1).copied().unwrap_or(0);
        let sec = sections.iter().find(|s| off >= s.start && off < s.end);
        let title = sec
            .map(|s| PART.replace(&s.heading, "").to_string())
            .filter(|h| h != "(top)")
            .unwrap_or_else(|| src.title.clone());
        let table = Table::new(&title, src.url.as_deref().unwrap_or(&src.label), parsed);
        // Grep and limit before saving, so only returned tables get handles.
        if let Some(kw) = a.grep.as_deref() {
            let kw = kw.to_lowercase();
            let hit = table.columns.iter().any(|c| c.to_lowercase().contains(&kw))
                || table
                    .rows
                    .iter()
                    .flatten()
                    .any(|c| table::value::cell_str(c).to_lowercase().contains(&kw));
            if !hit {
                continue;
            }
        }
        let mut m = Map::new();
        m.insert("title".into(), json!(table.title));
        m.insert("headers".into(), json!(table.columns));
        m.insert("rows_count".into(), json!(table.rows.len()));
        m.insert("preview".into(), Value::Array(table.objects(PREVIEW_ROWS)));
        m.insert("section".into(), json!(sec.map_or(0, |s| s.index)));
        m.insert("line".into(), json!(line));
        m.insert("doc".into(), json!(src.label));
        m.insert("url".into(), json!(src.url));
        let key = format!("{}|{:?}|{:?}", src.label, table.columns, table.rows);
        out.push((key, m, table));
    }
    // Save (deduplicated in the store) only what will be returned.
    let mut res = Vec::new();
    for (key, mut m, table) in out {
        if res.len() >= a.limit {
            res.push((key, m));
            continue;
        }
        let id = table::save(store, table)?;
        let mut first = Map::new();
        first.insert("table".into(), json!(format!("tbl:{id}")));
        first.append(&mut m);
        res.push((key, first));
    }
    Ok(res)
}

fn split_sources(a: &ExtractArgs) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in a.sources.iter().chain(a.from.iter()) {
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(part)) {
                out.push(part.to_string());
            }
        }
    }
    out
}

fn none_hint(kind: &str, first: &str) -> String {
    match kind {
        "tables" => format!("No tables found. The page may build its tables with JavaScript or use layout blocks; look for figures with `agentbox extract {first} --kind prices` or `agentbox read {first} --grep KEYWORD`."),
        "prices" => format!("No prices with a currency found. Check `agentbox read {first} --grep price` (prices split over several lines or images are missed)."),
        "people" => format!("No name/role pairs found. Try `agentbox read {first} --grep CEO` or another role keyword; names written without a role nearby are not detected."),
        _ => format!("Nothing found. Try another --kind, drop --section/--grep, or read the doc with `agentbox read {first} --grep KEYWORD`."),
    }
}

pub fn run(store: &Store, a: &ExtractArgs) -> CmdResult {
    if !KINDS.contains(&a.kind.as_str()) {
        return Err(AppError::new(
            "bad_args",
            format!("unknown kind `{}`", a.kind),
            format!("Kinds: {}.", KINDS.join(", ")),
        ));
    }
    let names = split_sources(a);
    if names.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "no source given",
            "Pass doc handles or files, e.g. `agentbox extract doc:1 doc:2 --kind prices` or `--from doc:1,doc:2`.",
        ));
    }
    let mut sources_info = Vec::new();
    let mut all: Vec<Map<String, Value>> = Vec::new();
    for name in &names {
        let src = load_source(store, name)?;
        sources_info.push(json!({"doc": src.label, "url": src.url, "title": src.title}));
        let mut seen = HashSet::new();
        for (key, item) in items_of(store, &src, a)? {
            if seen.insert(key) && (a.kind == "tables" || grep_ok(&item, a.grep.as_deref())) {
                all.push(item);
            }
        }
    }
    let total = all.len();
    let mut data = Map::new();
    data.insert("kind".into(), json!(a.kind));
    data.insert("sources".into(), Value::Array(sources_info));
    data.insert("total".into(), json!(total));
    let mut saved: Option<String> = None;
    if a.save_table && a.kind != "tables" && total > 0 {
        let mut columns: Vec<String> = Vec::new();
        for it in &all {
            for k in it.keys() {
                if !columns.contains(k) {
                    columns.push(k.clone());
                }
            }
        }
        let rows = all
            .iter()
            .map(|it| {
                columns
                    .iter()
                    .map(|c| it.get(c).cloned().unwrap_or(Value::Null))
                    .collect()
            })
            .collect();
        let t = Table::new(
            &format!("{} from {}", a.kind, names.join(", ")),
            &names.join(","),
            parse::Parsed { columns, rows },
        );
        let id = table::save(store, t)?;
        saved = Some(format!("tbl:{id}"));
        data.insert("table".into(), json!(saved));
    }
    let returned = total.min(a.limit);
    all.truncate(a.limit);
    data.insert("returned".into(), json!(returned));
    if total > returned {
        data.insert("truncated".into(), json!(true));
    }
    let first_table = all
        .first()
        .and_then(|i| i.get("table"))
        .and_then(Value::as_str)
        .map(str::to_string);
    data.insert(
        "items".into(),
        Value::Array(all.into_iter().map(Value::Object).collect()),
    );

    let mut hints: Vec<String> = Vec::new();
    if total == 0 {
        hints.push(none_hint(&a.kind, &names[0]));
    }
    if total > returned {
        hints.push(format!(
            "Showing {returned} of {total} items. Narrow with --grep KEYWORD or --section N, or raise --limit{}.",
            if a.kind != "tables" && saved.is_none() { "; add --save-table to keep all of them as a table" } else { "" }
        ));
    }
    if let Some(t) = &saved {
        let col = match a.kind.as_str() {
            "prices" | "numbers" => "value",
            "dates" => "date",
            "people" => "name",
            "links" => "url",
            _ => "email",
        };
        hints.push(format!(
            "All {total} items saved as {t}; e.g. `agentbox table query {t} --sort {col}`."
        ));
    }
    if let Some(t) = first_table {
        hints.push(format!("Sort or filter a table with `agentbox table query {t} --where \"COLUMN contains KEYWORD\"`, or see all rows with `agentbox table show {t}`."));
    }
    if a.kind == "people" && total > 0 {
        hints.push("People matching is heuristic: confirm each name and role in its context before citing.".into());
    }
    if a.kind == "prices" && total > 0 {
        hints.push("`$` is read as USD unless the site's country says otherwise; check `raw` and `context` for the plan each price belongs to.".into());
    }
    let mut out = Output::new(Value::Object(data));
    if !hints.is_empty() {
        out = out.hint(hints.join(" "));
    }
    Ok(out)
}
