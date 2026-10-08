//! `file read|write|replace`: local text files. Changes preview a unified diff
//! and only touch the disk with `--apply`.

use crate::envelope::{AppError, CmdResult, Output};
use serde_json::json;
use similar::TextDiff;
use std::fs;
use std::path::Path;

/// Diffs longer than this (chars) are truncated in the response.
const MAX_DIFF_CHARS: usize = 8000;

pub fn read(path: &str, lines: Option<&str>, max_chars: usize) -> CmdResult {
    let text = read_text(path)?;
    let all: Vec<&str> = text.split_inclusive('\n').collect();
    let total = all.len();
    let (from, to) = match lines {
        Some(spec) => parse_range(spec, total)?,
        None => (1, total.max(1)),
    };
    let selected: String = all
        .iter()
        .skip(from.saturating_sub(1))
        .take(to + 1 - from)
        .copied()
        .collect();
    let max_chars = max_chars.max(100);
    let (content, truncated) = match selected.char_indices().nth(max_chars) {
        Some((cut, _)) => (selected[..cut].to_string(), true),
        None => (selected, false),
    };
    let shown_to = if truncated {
        from + content.matches('\n').count()
    } else {
        to.min(total)
    };
    let out = Output::new(json!({
        "path": path,
        "total_lines": total,
        "from": from,
        "to": shown_to,
        "truncated": truncated,
        "content": content,
    }));
    Ok(if truncated {
        out.hint(format!(
            "Truncated. Continue with `agentbox file read {path} --lines {}:{}`.",
            shown_to + 1,
            total
        ))
    } else {
        out
    })
}

pub fn write(path: &str, content: &str, apply: bool) -> CmdResult {
    let p = Path::new(path);
    let (old, exists) = if p.exists() {
        (read_text(path)?, true)
    } else {
        (String::new(), false)
    };
    change(path, &old, content, exists, apply, json!({}))
}

pub fn replace(path: &str, find: &str, replacement: &str, all: bool, apply: bool) -> CmdResult {
    if find.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "--find must not be empty",
            "Pass the exact text to replace, copied from `agentbox file read`.",
        ));
    }
    let old = read_text(path)?;
    let positions: Vec<usize> = old.match_indices(find).map(|(i, _)| i).collect();
    match positions.len() {
        0 => {
            let hint = match near_match_line(&old, find) {
                Some(n) => format!(
                    "Not found exactly, but line {n} looks similar (whitespace or case may differ). Copy the exact text with `agentbox file read {path} --lines {n}:{}`.",
                    n + find.lines().count().max(1) - 1
                ),
                None => format!("Copy the exact text (including whitespace) with `agentbox file read {path}`."),
            };
            return Err(AppError::new(
                "not_found",
                format!("--find text does not occur in {path}"),
                hint,
            ));
        }
        n if n > 1 && !all => {
            let lines: Vec<String> = positions
                .iter()
                .take(10)
                .map(|&i| (line_of(&old, i)).to_string())
                .collect();
            return Err(AppError::new(
                "ambiguous",
                format!("--find text occurs {n} times in {path} (lines {})", lines.join(", ")),
                "Include more surrounding text in --find so it matches once, or pass --all to replace every occurrence.",
            ));
        }
        _ => {}
    }
    let new = if all {
        old.replace(find, replacement)
    } else {
        old.replacen(find, replacement, 1)
    };
    change(
        path,
        &old,
        &new,
        true,
        apply,
        json!({ "replacements": positions.len() }),
    )
}

fn change(
    path: &str,
    old: &str,
    new: &str,
    exists: bool,
    apply: bool,
    extra: serde_json::Value,
) -> CmdResult {
    let (diff, added, removed) = unified_diff(path, old, new);
    let unchanged = old == new && exists;
    let mut data = json!({
        "path": path,
        "applied": false,
        "exists": exists,
        "added_lines": added,
        "removed_lines": removed,
    });
    if let serde_json::Value::Object(m) = extra {
        for (k, v) in m {
            data[k] = v;
        }
    }
    let (diff_text, diff_truncated) = match diff.char_indices().nth(MAX_DIFF_CHARS) {
        Some((cut, _)) => (format!("{}\n... (diff truncated)", &diff[..cut]), true),
        None => (diff, false),
    };
    data["diff"] = json!(diff_text);
    if diff_truncated {
        data["diff_truncated"] = json!(true);
    }
    if unchanged {
        return Ok(Output::new(data).hint("No changes: the file already has this content."));
    }
    if !apply {
        return Ok(Output::new(data).hint(
            "Preview only. Review the diff, then rerun the same command with --apply to write it.",
        ));
    }
    if let Some(parent) = Path::new(path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|e| AppError::io("create parent dir", e))?;
    }
    fs::write(path, new).map_err(|e| AppError::io(&format!("write {path}"), e))?;
    data["applied"] = json!(true);
    Ok(Output::new(data).hint(format!("Written. Verify with `agentbox file read {path}`.")))
}

/// Returns (unified diff text, added line count, removed line count).
pub fn unified_diff(path: &str, old: &str, new: &str) -> (String, usize, usize) {
    let diff = TextDiff::from_lines(old, new);
    let mut added = 0;
    let mut removed = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Insert => added += 1,
            similar::ChangeTag::Delete => removed += 1,
            similar::ChangeTag::Equal => {}
        }
    }
    let text = diff
        .unified_diff()
        .context_radius(3)
        .header(&format!("{path} (current)"), &format!("{path} (proposed)"))
        .to_string();
    (text, added, removed)
}

fn read_text(path: &str) -> Result<String, AppError> {
    let bytes = fs::read(path).map_err(|e| AppError::io(&format!("read {path}"), e))?;
    String::from_utf8(bytes).map_err(|_| {
        AppError::new(
            "not_text",
            format!("{path} is not UTF-8 text"),
            "Only UTF-8 text files are supported. Convert the file to UTF-8 first.",
        )
    })
}

/// Parse `A:B`, `A:`, `:B` or `A` (1-based, inclusive) into a clamped range.
fn parse_range(spec: &str, total: usize) -> Result<(usize, usize), AppError> {
    let bad = || {
        AppError::new(
            "bad_args",
            format!("bad --lines value `{spec}`"),
            "Use START:END with 1-based line numbers, e.g. `--lines 10:40`, `--lines 50:` or `--lines 7`.",
        )
    };
    let parse = |s: &str, default: usize| -> Result<usize, AppError> {
        let s = s.trim();
        if s.is_empty() {
            Ok(default)
        } else {
            s.parse::<usize>().map_err(|_| bad())
        }
    };
    let (from, to) = match spec.split_once(':').or_else(|| spec.split_once('-')) {
        Some((a, b)) => (parse(a, 1)?, parse(b, total)?),
        None => {
            let n = parse(spec, 1)?;
            (n, n)
        }
    };
    if from == 0 || from > to {
        return Err(bad());
    }
    if from > total.max(1) {
        return Err(AppError::new(
            "bad_args",
            format!("line {from} is past the end (file has {total} lines)"),
            format!("Pick lines between 1 and {total}."),
        ));
    }
    Ok((from, to.min(total.max(1))))
}

fn line_of(text: &str, byte_pos: usize) -> usize {
    text[..byte_pos].matches('\n').count() + 1
}

/// Find a line that matches the first non-empty line of `find` after
/// trimming and lowercasing; used to explain near misses.
fn near_match_line(text: &str, find: &str) -> Option<usize> {
    let key = find
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())?
        .to_lowercase();
    let key: String = key.split_whitespace().collect::<Vec<_>>().join(" ");
    text.lines()
        .position(|l| {
            let norm: String = l
                .trim()
                .to_lowercase()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            norm.contains(&key)
        })
        .map(|i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_file(content: &str) -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        fs::write(&p, content).unwrap();
        (dir, p.to_string_lossy().to_string())
    }

    #[test]
    fn read_with_range() {
        let (_d, p) = tmp_file("l1\nl2\nl3\nl4\n");
        let out = read(&p, Some("2:3"), 1000).unwrap();
        assert_eq!(out.data["content"], "l2\nl3\n");
        assert_eq!(out.data["total_lines"], 4);
        assert_eq!(out.data["to"], 3);
        let out = read(&p, Some("3:"), 1000).unwrap();
        assert_eq!(out.data["content"], "l3\nl4\n");
        assert_eq!(read(&p, Some("9"), 1000).unwrap_err().code, "bad_args");
        assert_eq!(read(&p, Some("x:y"), 1000).unwrap_err().code, "bad_args");
    }

    #[test]
    fn read_missing_file_has_hint() {
        let e = read("/definitely/not/here.txt", None, 100).unwrap_err();
        assert_eq!(e.code, "io_error");
        assert!(!e.hint.is_empty());
    }

    #[test]
    fn write_previews_then_applies() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir
            .path()
            .join("sub")
            .join("new.md")
            .to_string_lossy()
            .to_string();
        let out = write(&p, "hello\n", false).unwrap();
        assert_eq!(out.data["applied"], false);
        assert!(out.data["diff"].as_str().unwrap().contains("+hello"));
        assert!(!Path::new(&p).exists());
        let out = write(&p, "hello\n", true).unwrap();
        assert_eq!(out.data["applied"], true);
        assert_eq!(fs::read_to_string(&p).unwrap(), "hello\n");
        let out = write(&p, "hello\n", true).unwrap();
        assert!(out.hint.unwrap().contains("No changes"));
    }

    #[test]
    fn replace_unique_text() {
        let (_d, p) = tmp_file("price = 10\nname = x\n");
        let out = replace(&p, "price = 10", "price = 12", false, false).unwrap();
        let diff = out.data["diff"].as_str().unwrap();
        assert!(
            diff.contains("-price = 10") && diff.contains("+price = 12"),
            "{diff}"
        );
        assert_eq!(fs::read_to_string(&p).unwrap(), "price = 10\nname = x\n");
        replace(&p, "price = 10", "price = 12", false, true).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "price = 12\nname = x\n");
    }

    #[test]
    fn replace_zero_or_many_matches_errors_with_hint() {
        let (_d, p) = tmp_file("a = 1\n  Foo  Bar\na = 1\n");
        let e = replace(&p, "zzz", "y", false, false).unwrap_err();
        assert_eq!(e.code, "not_found");
        let e = replace(&p, "foo bar", "y", false, false).unwrap_err();
        assert!(e.hint.contains("line 2"), "{}", e.hint);
        let e = replace(&p, "a = 1", "a = 2", false, false).unwrap_err();
        assert_eq!(e.code, "ambiguous");
        assert!(e.message.contains("lines 1, 3"), "{}", e.message);
        let out = replace(&p, "a = 1", "a = 2", true, true).unwrap();
        assert_eq!(out.data["replacements"], 2);
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            "a = 2\n  Foo  Bar\na = 2\n"
        );
    }
}
