//! `table query`: where / group-by + agg / sort / select / limit.
//!
//! Where syntax (kept tiny on purpose): `COLUMN OP VALUE`, where OP is one of
//! `=` `!=` `<` `<=` `>` `>=` `contains` `startswith`. Values are auto-typed:
//! numbers (incl. "$1,299/mo") compare numerically, dates by calendar order,
//! everything else as case-insensitive text.

use super::value::{cell_date, cell_num, cell_str, is_blank, num_value, parse_num};
use super::{shell_arg, Table};
use crate::envelope::AppError;
use serde_json::Value;
use std::cmp::Ordering;

pub const WHERE_HELP: &str = "Use COLUMN OP VALUE with OP one of = != < <= > >= contains startswith, e.g. --where \"Price < 100\" or --where \"Plan contains pro\". Repeat --where to AND conditions.";

#[derive(Debug, Default, Clone)]
pub struct Query {
    pub select: Option<String>,
    pub wheres: Vec<String>,
    pub sorts: Vec<String>,
    pub group_by: Option<String>,
    pub agg: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Contains,
    StartsWith,
}

impl Op {
    fn symbol(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Contains => "contains",
            Op::StartsWith => "startswith",
        }
    }
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Closest column name, for "did you mean" hints.
pub fn nearest<'a>(name: &str, columns: &'a [String]) -> Option<&'a String> {
    let n = norm(name);
    columns
        .iter()
        .map(|c| {
            let cn = norm(c);
            let d = if cn.contains(&n) || n.contains(&cn) {
                0
            } else {
                levenshtein(&n, &cn)
            };
            (d, c)
        })
        .filter(|(d, c)| *d <= (norm(c).len().max(3) / 2).max(2))
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

/// Resolve a column name case-insensitively (also ignoring spaces and
/// punctuation). `example` renders a corrected example for the hint.
pub fn resolve(
    columns: &[String],
    name: &str,
    example: &dyn Fn(&str) -> String,
) -> Result<usize, AppError> {
    let name = name.trim().trim_matches(['"', '\'', '`']);
    if let Some(i) = columns.iter().position(|c| c == name) {
        return Ok(i);
    }
    if let Some(i) = columns.iter().position(|c| c.eq_ignore_ascii_case(name)) {
        return Ok(i);
    }
    let n = norm(name);
    if !n.is_empty() {
        if let Some(i) = columns.iter().position(|c| norm(c) == n) {
            return Ok(i);
        }
    }
    let list = columns.join(", ");
    let hint = match nearest(name, columns) {
        Some(c) => format!("Did you mean `{c}`? Try: {}", example(c)),
        None => format!(
            "Use one of the listed columns, e.g. {}",
            example(columns.first().map_or("Column", String::as_str))
        ),
    };
    Err(AppError::new(
        "unknown_column",
        format!("no column named `{name}`; columns are: {list}"),
        hint,
    ))
}

/// Parse `COLUMN OP VALUE`.
pub fn parse_where(expr: &str) -> Result<(String, Op, String), AppError> {
    let e = expr.trim();
    let lower = e.to_lowercase();
    let bad = |msg: String| AppError::new("bad_where", msg, WHERE_HELP);
    let mut found: Option<(usize, usize, Op)> = None;
    for (word, op) in [
        (" not contains ", None),
        (" contains ", Some(Op::Contains)),
        (" startswith ", Some(Op::StartsWith)),
        (" starts with ", Some(Op::StartsWith)),
    ] {
        if let Some(p) = lower.find(word) {
            let Some(op) = op else {
                return Err(bad(format!("`{e}`: `not contains` is not supported")));
            };
            found = Some((p, p + word.len(), op));
            break;
        }
    }
    if found.is_none() {
        let bytes = e.as_bytes();
        if let Some(p) = e.find(['<', '>', '=', '!']) {
            let next = bytes.get(p + 1).copied();
            let (op, len) = match (bytes[p], next) {
                (b'<', Some(b'=')) => (Op::Le, 2),
                (b'>', Some(b'=')) => (Op::Ge, 2),
                (b'!', Some(b'=')) => (Op::Ne, 2),
                (b'<', Some(b'>')) => (Op::Ne, 2),
                (b'=', Some(b'=')) => (Op::Eq, 2),
                (b'<', _) => (Op::Lt, 1),
                (b'>', _) => (Op::Gt, 1),
                (b'=', _) => (Op::Eq, 1),
                _ => return Err(bad(format!("`{e}`: `!` must be followed by `=`"))),
            };
            found = Some((p, p + len, op));
        }
    }
    let Some((s, t, op)) = found else {
        return Err(bad(format!("`{e}` has no operator")));
    };
    let col = e[..s].trim().trim_matches(['"', '\'', '`']).to_string();
    let raw_val = e[t..].trim();
    let quoted = raw_val.len() >= 2
        && ((raw_val.starts_with('"') && raw_val.ends_with('"'))
            || (raw_val.starts_with('\'') && raw_val.ends_with('\'')));
    let val = if quoted {
        raw_val[1..raw_val.len() - 1].to_string()
    } else {
        raw_val.to_string()
    };
    if col.is_empty() {
        return Err(bad(format!(
            "`{e}` is missing the column name before `{}`",
            op.symbol()
        )));
    }
    if val.is_empty() && !quoted {
        return Err(bad(format!(
            "`{e}` is missing a value after `{}`",
            op.symbol()
        )));
    }
    Ok((col, op, val))
}

fn matches(cell: &Value, op: Op, val: &str) -> bool {
    let text = cell_str(cell);
    match op {
        Op::Contains => return text.to_lowercase().contains(&val.to_lowercase()),
        Op::StartsWith => return text.to_lowercase().starts_with(&val.to_lowercase()),
        _ => {}
    }
    // Numeric when the value is a number and the cell has one.
    if let Some(v) = parse_num(val).map(|n| n.value) {
        if let Some(c) = cell_num(cell) {
            let ord = c.partial_cmp(&v).unwrap_or(Ordering::Equal);
            return cmp_ok(op, ord);
        }
        if matches!(op, Op::Lt | Op::Le | Op::Gt | Op::Ge) {
            return false;
        }
    }
    // Dates by calendar order.
    if let Some(v) = crate::extract::dates::parse_whole(val) {
        if let Some(c) = cell_date(cell) {
            return cmp_ok(op, c.as_str().cmp(v.as_str()));
        }
        if matches!(op, Op::Lt | Op::Le | Op::Gt | Op::Ge) {
            return false;
        }
    }
    let ord = text.trim().to_lowercase().cmp(&val.trim().to_lowercase());
    cmp_ok(op, ord)
}

fn cmp_ok(op: Op, ord: Ordering) -> bool {
    match op {
        Op::Eq => ord == Ordering::Equal,
        Op::Ne => ord != Ordering::Equal,
        Op::Lt => ord == Ordering::Less,
        Op::Le => ord != Ordering::Greater,
        Op::Gt => ord == Ordering::Greater,
        Op::Ge => ord != Ordering::Less,
        Op::Contains | Op::StartsWith => false,
    }
}

/// Sort key mode for a column: numeric, date or text.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Num,
    Date,
    Text,
}

fn mode_of(rows: &[Vec<Value>], i: usize) -> Mode {
    let cells: Vec<&Value> = rows
        .iter()
        .map(|r| &r[i])
        .filter(|v| !is_blank(&cell_str(v)))
        .collect();
    let n = cells.len();
    let nums = cells.iter().filter(|v| cell_num(v).is_some()).count();
    let dates = cells.iter().filter(|v| cell_date(v).is_some()).count();
    // Mostly-numeric columns sort numerically; odd cells ("Contact us") go last.
    if n > 0 && nums * 2 >= n && nums >= dates {
        Mode::Num
    } else if n > 0 && dates * 2 >= n {
        Mode::Date
    } else {
        Mode::Text
    }
}

/// Cells without a usable value for the mode sort after all others.
fn unusable(v: &Value, mode: Mode) -> bool {
    is_blank(&cell_str(v))
        || match mode {
            Mode::Num => cell_num(v).is_none(),
            Mode::Date => cell_date(v).is_none(),
            Mode::Text => false,
        }
}

fn cmp_cells(a: &Value, b: &Value, mode: Mode) -> Ordering {
    match mode {
        Mode::Num => cell_num(a)
            .unwrap_or(0.0)
            .partial_cmp(&cell_num(b).unwrap_or(0.0))
            .unwrap_or(Ordering::Equal),
        Mode::Date => cell_date(a).cmp(&cell_date(b)),
        Mode::Text => cell_str(a).to_lowercase().cmp(&cell_str(b).to_lowercase()),
    }
}

/// Parse one sort key: `Price`, `-Price`, `+Price`, `Price desc`, `Price asc`.
fn parse_sort(s: &str) -> (String, bool) {
    let t = s.trim();
    let lower = t.to_lowercase();
    if let Some(c) = t.strip_prefix('-') {
        (c.trim().to_string(), true)
    } else if let Some(c) = t.strip_prefix('+') {
        (c.trim().to_string(), false)
    } else if lower.ends_with(" desc") {
        (t[..t.len() - 5].trim().to_string(), true)
    } else if lower.ends_with(" asc") {
        (t[..t.len() - 4].trim().to_string(), false)
    } else {
        (t.to_string(), false)
    }
}

const AGGS: &[&str] = &["count", "sum", "avg", "min", "max"];

fn aggregate(func: &str, cells: &[&Value]) -> Value {
    let nums: Vec<f64> = cells.iter().filter_map(|v| cell_num(v)).collect();
    match func {
        "count" => Value::from(cells.iter().filter(|v| !is_blank(&cell_str(v))).count()),
        "sum" => num_value(nums.iter().sum()),
        "avg" if !nums.is_empty() => num_value(nums.iter().sum::<f64>() / nums.len() as f64),
        "min" => nums
            .iter()
            .copied()
            .reduce(f64::min)
            .map_or(Value::Null, num_value),
        "max" => nums
            .iter()
            .copied()
            .reduce(f64::max)
            .map_or(Value::Null, num_value),
        _ => Value::Null,
    }
}

/// Run a query and return the full result (no limit applied).
pub fn run(t: &Table, q: &Query) -> Result<Table, AppError> {
    let mut columns = t.columns.clone();
    let mut rows: Vec<Vec<Value>> = t.rows.clone();

    // where (AND)
    for w in &q.wheres {
        let (col, op, val) = parse_where(w)?;
        let i = resolve(&columns, &col, &|c| {
            format!(
                "--where {}",
                shell_arg(&format!("{c} {} {val}", op.symbol()))
            )
        })?;
        rows.retain(|r| matches(&r[i], op, &val));
    }

    // group-by / agg
    if q.group_by.is_some() || q.agg.is_some() {
        let key = match &q.group_by {
            Some(g) => Some(resolve(&columns, g, &|c| {
                format!("--group-by {}", shell_arg(c))
            })?),
            None => None,
        };
        let spec = q.agg.clone().unwrap_or_else(|| "count".into());
        let mut aggs: Vec<(String, Option<usize>, String)> = Vec::new(); // func, col, out name
        for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (func, col) = match part.split_once(':') {
                Some((f, c)) => (f.trim().to_lowercase(), Some(c.trim())),
                None => (part.to_lowercase(), None),
            };
            if !AGGS.contains(&func.as_str()) {
                return Err(AppError::new(
                    "bad_agg",
                    format!("unknown aggregate `{func}` in `{part}`"),
                    format!(
                        "Aggregates: {}. Example: --agg \"sum:Price,avg:Price,count\"",
                        AGGS.join(", ")
                    ),
                ));
            }
            match col {
                None if func == "count" => aggs.push((func, None, "count".into())),
                None => {
                    return Err(AppError::new(
                        "bad_agg",
                        format!("`{func}` needs a column, e.g. `{func}:Price`"),
                        format!(
                            "Example: --agg \"{func}:{}\"",
                            columns.last().map_or("Price", String::as_str)
                        ),
                    ))
                }
                Some(c) => {
                    let i = resolve(&columns, c, &|n| {
                        format!("--agg {}", shell_arg(&format!("{func}:{n}")))
                    })?;
                    let name = format!("{func}_{}", columns[i]);
                    aggs.push((func, Some(i), name));
                }
            }
        }
        let mut groups: Vec<(Value, Vec<usize>)> = Vec::new();
        for (ri, r) in rows.iter().enumerate() {
            let k = key.map_or(Value::Null, |i| {
                Value::String(cell_str(&r[i]).trim().to_string())
            });
            match groups.iter_mut().find(|(gk, _)| *gk == k) {
                Some((_, v)) => v.push(ri),
                None => groups.push((k, vec![ri])),
            }
        }
        if key.is_none() && groups.is_empty() {
            groups.push((Value::Null, Vec::new()));
        }
        let mut new_cols: Vec<String> = Vec::new();
        if let Some(i) = key {
            new_cols.push(columns[i].clone());
        }
        new_cols.extend(aggs.iter().map(|a| a.2.clone()));
        let new_rows = groups
            .iter()
            .map(|(k, idx)| {
                let mut out = Vec::new();
                if key.is_some() {
                    out.push(k.clone());
                }
                for (func, col, _) in &aggs {
                    let v = match col {
                        None => Value::from(idx.len()),
                        Some(c) => {
                            let cells: Vec<&Value> = idx.iter().map(|&ri| &rows[ri][*c]).collect();
                            aggregate(func, &cells)
                        }
                    };
                    out.push(v);
                }
                out
            })
            .collect();
        columns = super::parse::clean_headers(&new_cols);
        rows = new_rows;
    }

    // sort (stable; later keys are tie-breakers)
    let mut keys = Vec::new();
    for s in &q.sorts {
        let (name, desc) = parse_sort(s);
        let i = resolve(&columns, &name, &|c| {
            format!(
                "--sort {}",
                shell_arg(&format!("{}{c}", if desc { "-" } else { "" }))
            )
        })?;
        keys.push((i, desc, mode_of(&rows, i)));
    }
    if !keys.is_empty() {
        rows.sort_by(|a, b| {
            for &(i, desc, mode) in &keys {
                let (ab, bb) = (unusable(&a[i], mode), unusable(&b[i], mode));
                let ord = match (ab, bb) {
                    (true, true) => Ordering::Equal,
                    (true, false) => Ordering::Greater, // blanks last
                    (false, true) => Ordering::Less,
                    _ => {
                        let o = cmp_cells(&a[i], &b[i], mode);
                        if desc {
                            o.reverse()
                        } else {
                            o
                        }
                    }
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            Ordering::Equal
        });
    }

    // select
    if let Some(sel) = &q.select {
        let mut idx = Vec::new();
        for name in sel.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            idx.push(resolve(&columns, name, &|c| {
                let mut parts: Vec<String> = sel.split(',').map(|s| s.trim().to_string()).collect();
                if let Some(p) = parts.iter_mut().find(|p| p.as_str() == name) {
                    *p = c.to_string();
                }
                format!("--select {}", shell_arg(&parts.join(",")))
            })?);
        }
        columns = idx.iter().map(|&i| columns[i].clone()).collect();
        rows = rows
            .into_iter()
            .map(|r| idx.iter().map(|&i| r[i].clone()).collect())
            .collect();
    }

    Ok(Table {
        id: 0,
        title: t.title.clone(),
        source: t.source.clone(),
        columns,
        rows,
        created: String::new(),
    })
}
