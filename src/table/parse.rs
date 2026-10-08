//! Parsers for table sources: CSV/TSV, JSON and Markdown pipe tables.

use serde_json::{Map, Value};

/// A table as parsed from text, before it gets a handle.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

/// Give empty headers a name and make duplicates unique.
pub fn clean_headers(raw: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, h) in raw.iter().enumerate() {
        let base = h.trim().trim_start_matches('\u{feff}').to_string();
        let base = if base.is_empty() {
            format!("column{}", i + 1)
        } else {
            base
        };
        let mut name = base.clone();
        let mut k = 2;
        while out.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
            name = format!("{base}_{k}");
            k += 1;
        }
        out.push(name);
    }
    out
}

/// Pad or extend rows so every row has exactly `columns.len()` cells,
/// adding `columnN` headers when a row is longer than the header.
fn square(mut columns: Vec<String>, mut rows: Vec<Vec<Value>>) -> Parsed {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    while columns.len() < width {
        columns.push(format!("column{}", columns.len() + 1));
    }
    let columns = clean_headers(&columns);
    for r in &mut rows {
        r.resize(columns.len(), Value::String(String::new()));
    }
    Parsed { columns, rows }
}

/// Guess the delimiter from the first line: tab, semicolon or comma.
pub fn sniff_delimiter(text: &str) -> char {
    let first = text.lines().next().unwrap_or("");
    let count = |c: char| first.matches(c).count();
    let (t, s, c) = (count('\t'), count(';'), count(','));
    if t > 0 && t >= c && t >= s {
        '\t'
    } else if s > c {
        ';'
    } else {
        ','
    }
}

/// RFC 4180 CSV (quotes, doubled quotes, newlines inside quotes).
pub fn parse_csv(text: &str, delim: Option<char>) -> Result<Parsed, String> {
    let text = text.trim_start_matches('\u{feff}');
    let d = delim.unwrap_or_else(|| sniff_delimiter(text));
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut field = String::new();
    let mut record: Vec<String> = Vec::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(ch);
            }
        } else if ch == '"' && field.trim().is_empty() {
            field.clear();
            in_quotes = true;
        } else if ch == d {
            record.push(std::mem::take(&mut field));
        } else if ch == '\n' || ch == '\r' {
            if ch == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            record.push(std::mem::take(&mut field));
            records.push(std::mem::take(&mut record));
        } else {
            field.push(ch);
        }
    }
    if in_quotes {
        return Err("unterminated quoted field".into());
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records.retain(|r| r.iter().any(|f| !f.trim().is_empty()));
    let mut it = records.into_iter();
    let header = it.next().ok_or("the file has no rows")?;
    let rows: Vec<Vec<Value>> = it
        .map(|r| {
            r.into_iter()
                .map(|f| Value::String(f.trim().to_string()))
                .collect()
        })
        .collect();
    Ok(square(header, rows))
}

/// JSON: an array of objects (or of arrays, first row = header), or an
/// object containing such an array (e.g. an agentbox envelope).
pub fn parse_json(text: &str) -> Result<Parsed, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    let arr = find_array(&v, 0).ok_or("no array of records found in the JSON")?;
    if arr.iter().all(Value::is_object) {
        let mut columns: Vec<String> = Vec::new();
        for o in arr {
            for k in o.as_object().into_iter().flat_map(Map::keys) {
                if !columns.contains(k) {
                    columns.push(k.clone());
                }
            }
        }
        let rows = arr
            .iter()
            .map(|o| columns.iter().map(|c| scalar(o.get(c))).collect())
            .collect();
        Ok(square(columns, rows))
    } else if arr.iter().all(Value::is_array) {
        let mut it = arr.iter().filter_map(Value::as_array);
        let header: Vec<String> = it
            .next()
            .map(|h| h.iter().map(crate::table::value::cell_str).collect())
            .unwrap_or_default();
        let rows = it
            .map(|r| r.iter().map(|c| scalar(Some(c))).collect())
            .collect();
        Ok(square(header, rows))
    } else {
        Ok(square(
            vec!["value".into()],
            arr.iter().map(|x| vec![scalar(Some(x))]).collect(),
        ))
    }
}

fn scalar(v: Option<&Value>) -> Value {
    match v {
        None => Value::Null,
        Some(Value::Array(_) | Value::Object(_)) => {
            Value::String(v.map(Value::to_string).unwrap_or_default())
        }
        Some(x) => x.clone(),
    }
}

fn find_array(v: &Value, depth: usize) -> Option<&Vec<Value>> {
    match v {
        Value::Array(a) if !a.is_empty() => Some(a),
        Value::Object(m) if depth < 3 => {
            // Prefer arrays of objects, then any array.
            m.values()
                .find_map(|x| match x {
                    Value::Array(a) if !a.is_empty() && a.iter().all(Value::is_object) => Some(a),
                    _ => None,
                })
                .or_else(|| m.values().find_map(|x| find_array(x, depth + 1)))
        }
        _ => None,
    }
}

/// A Markdown pipe table found in a document.
#[derive(Debug, Clone, PartialEq)]
pub struct MdTable {
    /// 1-based line number of the header row.
    pub line: usize,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Split a pipe-table row into cells; `\|` is a literal pipe.
pub fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = if t.ends_with('|') && !t.ends_with("\\|") {
        &t[..t.len() - 1]
    } else {
        t
    };
    let mut cells = Vec::new();
    let mut cur = String::new();
    let mut chars = t.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&'|') {
            cur.push('|');
            chars.next();
        } else if c == '|' {
            cells.push(cur.trim().to_string());
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    cells.push(cur.trim().to_string());
    cells
}

fn is_separator(line: &str) -> bool {
    let cells = split_row(line);
    !cells.is_empty()
        && line.contains('-')
        && cells.iter().all(|c| {
            let c = c.trim();
            let inner = c.trim_start_matches(':').trim_end_matches(':');
            !inner.is_empty() && inner.chars().all(|ch| ch == '-')
        })
}

fn has_pipe(line: &str) -> bool {
    line.replace("\\|", "").contains('|')
}

/// All pipe tables in a Markdown text (outside code fences).
pub fn md_tables(text: &str) -> Vec<MdTable> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            i += 1;
            continue;
        }
        if !in_fence && has_pipe(lines[i]) && i + 1 < lines.len() && is_separator(lines[i + 1]) {
            let columns = split_row(lines[i]);
            let mut rows = Vec::new();
            let mut j = i + 2;
            while j < lines.len() && has_pipe(lines[j]) && !lines[j].trim().is_empty() {
                let mut r = split_row(lines[j]);
                r.resize(columns.len().max(r.len()), String::new());
                rows.push(r);
                j += 1;
            }
            out.push(MdTable {
                line: i + 1,
                columns,
                rows,
            });
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

impl MdTable {
    pub fn to_parsed(&self, clean: &dyn Fn(&str) -> String) -> Parsed {
        let cols: Vec<String> = self.columns.iter().map(|c| clean(c)).collect();
        let rows = self
            .rows
            .iter()
            .map(|r| r.iter().map(|c| Value::String(clean(c))).collect())
            .collect();
        square(cols, rows)
    }
}
