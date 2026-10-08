//! State directory and the document store (`doc:N` handles).
//!
//! Layout under the state dir:
//!   docs/<N>.md    converted document body
//!   docs/<N>.json  metadata (url, title, fetched_at, ...)
//!   notes.jsonl    scratch notes, one JSON object per line

use crate::envelope::AppError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Resolve the state directory: `$AGENTBOX_HOME`, else `~/.agentbox`
/// (`%USERPROFILE%\.agentbox` on Windows).
pub fn default_home() -> PathBuf {
    if let Some(p) = std::env::var_os("AGENTBOX_HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(p);
    }
    user_home().join(".agentbox")
}

/// The user's home directory (`%USERPROFILE%` first on Windows), else `.`.
pub fn user_home() -> PathBuf {
    let candidates: [&str; 2] = if cfg!(windows) {
        ["USERPROFILE", "HOME"]
    } else {
        ["HOME", "USERPROFILE"]
    };
    candidates
        .iter()
        .find_map(|k| std::env::var_os(k).filter(|s| !s.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DocMeta {
    pub id: u64,
    pub url: String,
    pub title: String,
    pub content_type: String,
    pub fetched_at: String,
}

#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn docs_dir(&self) -> PathBuf {
        self.root.join("docs")
    }

    pub fn notes_path(&self) -> PathBuf {
        self.root.join("notes.jsonl")
    }

    pub fn ensure_dir(&self, dir: &Path) -> Result<(), AppError> {
        fs::create_dir_all(dir).map_err(|e| {
            AppError::new(
                "state_error",
                format!("cannot create state dir {}: {e}", dir.display()),
                "Set AGENTBOX_HOME to a writable directory and retry.",
            )
        })
    }

    /// All stored doc ids, ascending.
    pub fn doc_ids(&self) -> Vec<u64> {
        let mut ids: Vec<u64> = fs::read_dir(self.docs_dir())
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        name.strip_suffix(".md")?.parse::<u64>().ok()
                    })
                    .collect()
            })
            .unwrap_or_default();
        ids.sort_unstable();
        ids
    }

    /// Metadata of all stored docs, ascending by id.
    pub fn doc_metas(&self) -> Vec<DocMeta> {
        let dir = self.docs_dir();
        self.doc_ids()
            .into_iter()
            .filter_map(|id| fs::read_to_string(dir.join(format!("{id}.json"))).ok())
            .filter_map(|s| serde_json::from_str::<DocMeta>(&s).ok())
            .collect()
    }

    /// Store a document and return its new id.
    pub fn save_doc(&self, mut meta: DocMeta, body: &str) -> Result<u64, AppError> {
        let dir = self.docs_dir();
        self.ensure_dir(&dir)?;
        let id = self.doc_ids().last().copied().unwrap_or(0) + 1;
        meta.id = id;
        let meta_json = serde_json::to_string_pretty(&meta).expect("DocMeta serializes");
        fs::write(dir.join(format!("{id}.md")), body).map_err(|e| AppError::io("write doc", e))?;
        fs::write(dir.join(format!("{id}.json")), meta_json)
            .map_err(|e| AppError::io("write doc meta", e))?;
        Ok(id)
    }

    pub fn load_doc(&self, handle: &str) -> Result<(DocMeta, String), AppError> {
        let id = parse_handle(handle)?;
        let dir = self.docs_dir();
        let body = fs::read_to_string(dir.join(format!("{id}.md"))).map_err(|_| {
            let ids = self.doc_ids();
            let hint = if ids.is_empty() {
                "No docs stored yet. Create one with `agentbox fetch <url>`.".to_string()
            } else {
                let recent: Vec<String> = ids
                    .iter()
                    .rev()
                    .take(5)
                    .map(|i| format!("doc:{i}"))
                    .collect();
                format!(
                    "Recent docs: {}. Or fetch a new one with `agentbox fetch <url>`.",
                    recent.join(", ")
                )
            };
            AppError::new("doc_not_found", format!("doc:{id} does not exist"), hint)
        })?;
        let meta = fs::read_to_string(dir.join(format!("{id}.json")))
            .ok()
            .and_then(|s| serde_json::from_str::<DocMeta>(&s).ok())
            .unwrap_or(DocMeta {
                id,
                url: String::new(),
                title: String::new(),
                content_type: String::new(),
                fetched_at: String::new(),
            });
        Ok((meta, body))
    }
}

/// Accept `doc:3`, `DOC:3` or a bare `3`.
pub fn parse_handle(handle: &str) -> Result<u64, AppError> {
    let h = handle.trim();
    let num = if h.len() > 4 && h[..4].eq_ignore_ascii_case("doc:") {
        &h[4..]
    } else {
        h
    };
    num.parse::<u64>().map_err(|_| {
        AppError::new(
            "bad_handle",
            format!("`{handle}` is not a doc handle"),
            "Doc handles look like `doc:3`; they are returned by `agentbox fetch`.",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> DocMeta {
        DocMeta {
            id: 0,
            url: "https://e.com".into(),
            title: "T".into(),
            content_type: "text/html".into(),
            fetched_at: "now".into(),
        }
    }

    #[test]
    fn handles_parse() {
        assert_eq!(parse_handle("doc:3").unwrap(), 3);
        assert_eq!(parse_handle("DOC:12").unwrap(), 12);
        assert_eq!(parse_handle("7").unwrap(), 7);
        assert_eq!(parse_handle("doc:x").unwrap_err().code, "bad_handle");
    }

    #[test]
    fn save_and_load_increment_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        assert_eq!(store.save_doc(meta(), "one").unwrap(), 1);
        assert_eq!(store.save_doc(meta(), "two").unwrap(), 2);
        let (m, body) = store.load_doc("doc:2").unwrap();
        assert_eq!(m.id, 2);
        assert_eq!(body, "two");
        let err = store.load_doc("doc:9").unwrap_err();
        assert_eq!(err.code, "doc_not_found");
        assert!(err.hint.contains("doc:2"));
    }
}
