//! `note add|list|clear`: scratch notes and a running source list in the state dir.

use crate::envelope::{AppError, CmdResult, Output};
use crate::state::Store;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::io::Write;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Note {
    pub id: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    pub created: String,
}

pub fn load(store: &Store) -> Vec<Note> {
    fs::read_to_string(store.notes_path())
        .map(|s| {
            s.lines()
                .filter_map(|l| serde_json::from_str::<Note>(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

pub fn add(store: &Store, text: &str, source: Option<&str>, tag: Option<&str>) -> CmdResult {
    let text = text.trim();
    if text.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "note text is empty",
            "Example: `agentbox note add \"Revenue 2025: $3.2B\" --source https://...`",
        ));
    }
    let id = load(store).last().map(|n| n.id).unwrap_or(0) + 1;
    let note = Note {
        id,
        text: text.to_string(),
        source: source
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from),
        tag: tag
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from),
        created: chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
    };
    store.ensure_dir(store.root())?;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(store.notes_path())
        .map_err(|e| AppError::io("open notes", e))?;
    writeln!(
        f,
        "{}",
        serde_json::to_string(&note).expect("Note serializes")
    )
    .map_err(|e| AppError::io("write note", e))?;
    let hint = if note.source.is_none() {
        "Saved. Tip: add `--source URL` so the final answer can cite it."
    } else {
        "Saved. List notes with `agentbox note list`."
    };
    Ok(Output::new(json!({ "note": note })).hint(hint))
}

pub fn list(store: &Store, tag: Option<&str>, grep: Option<&str>, limit: usize) -> CmdResult {
    let all = load(store);
    let grep_l = grep.map(str::to_lowercase);
    let matched: Vec<&Note> = all
        .iter()
        .filter(|n| {
            tag.is_none_or(|t| {
                n.tag
                    .as_deref()
                    .is_some_and(|nt| nt.eq_ignore_ascii_case(t))
            })
        })
        .filter(|n| {
            grep_l.as_ref().is_none_or(|g| {
                n.text.to_lowercase().contains(g)
                    || n.source
                        .as_deref()
                        .is_some_and(|s| s.to_lowercase().contains(g))
            })
        })
        .collect();
    let total = matched.len();
    // Keep the most recent `limit` notes, oldest first.
    let shown: Vec<&Note> = matched
        .iter()
        .skip(total.saturating_sub(limit.max(1)))
        .copied()
        .collect();
    let mut sources: Vec<serde_json::Value> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for n in &matched {
        if let Some(s) = n.source.as_deref() {
            if let Some(pos) = seen.iter().position(|x| *x == s) {
                let c = sources[pos]["notes"].as_u64().unwrap_or(0) + 1;
                sources[pos]["notes"] = json!(c);
            } else {
                seen.push(s);
                sources.push(json!({ "source": s, "notes": 1 }));
            }
        }
    }
    let out = Output::new(json!({
        "total": total,
        "shown": shown.len(),
        "notes": shown,
        "sources": sources,
    }));
    Ok(if all.is_empty() {
        out.hint("No notes yet. Add one with `agentbox note add \"fact\" --source URL`.")
    } else if total > shown.len() {
        out.hint(format!(
            "Showing the latest {} of {total}. Raise --limit or filter with --tag / --grep.",
            shown.len()
        ))
    } else {
        out
    })
}

/// Remove all notes, or only those with `tag`. Preview unless `apply`.
pub fn clear(store: &Store, tag: Option<&str>, apply: bool) -> CmdResult {
    let all = load(store);
    let hit = |n: &Note| {
        tag.is_none_or(|t| {
            n.tag
                .as_deref()
                .is_some_and(|nt| nt.eq_ignore_ascii_case(t))
        })
    };
    let (removed, kept): (Vec<Note>, Vec<Note>) = all.into_iter().partition(hit);
    if apply && !removed.is_empty() {
        let path = store.notes_path();
        if kept.is_empty() {
            fs::remove_file(&path).map_err(|e| AppError::io("remove notes", e))?;
        } else {
            let body: String = kept
                .iter()
                .map(|n| serde_json::to_string(n).expect("Note serializes") + "\n")
                .collect();
            fs::write(&path, body).map_err(|e| AppError::io("write notes", e))?;
        }
    }
    let n = removed.len();
    let hint = if n == 0 {
        "Nothing to remove.".to_string()
    } else if apply {
        format!("Removed {n} note(s). Add new ones with `agentbox note add \"fact\" --source URL --tag item1`.")
    } else {
        format!("Preview only: {n} note(s) would be removed. Rerun the same command with --apply.")
    };
    Ok(Output::new(json!({
        "applied": apply && n > 0,
        "tag": tag,
        "removed": n,
        "kept": kept.len(),
    }))
    .hint(hint))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_previews_then_removes() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("state"));
        add(&store, "a", None, Some("old")).unwrap();
        add(&store, "b", None, Some("item1")).unwrap();
        let p = clear(&store, Some("OLD"), false).unwrap();
        assert_eq!(
            (p.data["removed"].clone(), p.data["applied"].clone()),
            (json!(1), json!(false))
        );
        assert_eq!(load(&store).len(), 2);
        clear(&store, Some("old"), true).unwrap();
        assert_eq!(
            load(&store)
                .iter()
                .map(|n| n.text.as_str())
                .collect::<Vec<_>>(),
            ["b"]
        );
        let r = clear(&store, None, true).unwrap();
        assert_eq!(r.data["removed"], 1);
        assert!(load(&store).is_empty());
        assert_eq!(clear(&store, None, true).unwrap().data["applied"], false);
    }

    #[test]
    fn add_list_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("state"));
        assert!(list(&store, None, None, 10)
            .unwrap()
            .hint
            .unwrap()
            .contains("No notes"));
        add(
            &store,
            "AAPL closed at 230",
            Some("https://a.test"),
            Some("stock"),
        )
        .unwrap();
        add(
            &store,
            "Pro plan $20",
            Some("https://b.test"),
            Some("pricing"),
        )
        .unwrap();
        add(
            &store,
            "Enterprise plan custom",
            Some("https://b.test"),
            Some("pricing"),
        )
        .unwrap();
        let out = list(&store, None, None, 50).unwrap();
        assert_eq!(out.data["total"], 3);
        assert_eq!(out.data["notes"][2]["id"], 3);
        assert_eq!(out.data["sources"][1]["notes"], 2);
        let out = list(&store, Some("PRICING"), Some("pro"), 50).unwrap();
        assert_eq!(out.data["total"], 1);
        let out = list(&store, None, None, 2).unwrap();
        assert_eq!(out.data["shown"], 2);
        assert_eq!(out.data["notes"][0]["id"], 2);
        assert_eq!(add(&store, "  ", None, None).unwrap_err().code, "bad_args");
    }
}
