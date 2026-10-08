//! `config set|get|unset|path`: settings stored in `$AGENTBOX_HOME/config.toml`.
//!
//! Only a tiny TOML subset is needed (`[section]` headers and
//! `key = "string"` lines), so it is parsed by hand to keep dependencies lean.
//! Secrets are never printed: `get` shows a mask plus a short fingerprint.

use crate::envelope::{AppError, CmdResult, Output};
use crate::state::Store;
use serde_json::json;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Known keys: (dotted name, is_secret, environment variable that overrides it).
pub const KEYS: &[(&str, bool, Option<&str>)] = &[
    ("tavily.api_key", true, Some("TAVILY_API_KEY")),
    ("stooq.api_key", true, Some("STOOQ_API_KEY")),
    ("search.backend", false, None),
];

pub fn path(store: &Store) -> PathBuf {
    store.root().join("config.toml")
}

fn known(key: &str) -> Result<(bool, Option<&'static str>), AppError> {
    KEYS.iter()
        .find(|(k, _, _)| *k == key)
        .map(|(_, s, e)| (*s, *e))
        .ok_or_else(|| {
            let names: Vec<&str> = KEYS.iter().map(|(k, _, _)| *k).collect();
            AppError::new(
                "bad_args",
                format!("unknown config key `{key}`"),
                format!("Known keys: {}.", names.join(", ")),
            )
        })
}

/// Read one dotted key (e.g. `tavily.api_key`) from a config file.
pub fn read_value(file: &Path, key: &str) -> Option<String> {
    let text = fs::read_to_string(file).ok()?;
    get_in(&text, key)
}

fn split_key(key: &str) -> (&str, &str) {
    key.split_once('.').unwrap_or(("", key))
}

fn get_in(text: &str, key: &str) -> Option<String> {
    let (section, name) = split_key(key);
    let mut current = String::new();
    for line in text.lines() {
        let t = line.trim();
        if let Some(sec) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            current = sec.trim().to_string();
        } else if current == section {
            if let Some((k, v)) = t.split_once('=') {
                if k.trim() == name {
                    let v = unquote(v.trim());
                    return (!v.is_empty()).then_some(v);
                }
            }
        }
    }
    None
}

fn unquote(v: &str) -> String {
    let v = match v.find(" #") {
        Some(i) if !v.starts_with('"') => &v[..i],
        _ => v,
    };
    if v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')))
    {
        v[1..v.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        v.trim().to_string()
    }
}

fn quote(v: &str) -> String {
    format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Return `text` with `key` set to `value` (or removed when `None`),
/// keeping every other line untouched.
fn set_in(text: &str, key: &str, value: Option<&str>) -> String {
    let (section, name) = split_key(key);
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut section_seen = false;
    let mut done = false;
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if let Some(sec) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            current = sec.trim().to_string();
        }
        if current == section {
            section_seen = true;
            if let Some((k, _)) = t.split_once('=') {
                if k.trim() == name && !t.starts_with('#') {
                    if let (Some(v), false) = (value, done) {
                        out.push(format!("{name} = {}", quote(v)));
                    }
                    done = true;
                    continue;
                }
            }
        }
        out.push(line.to_string());
        // Append at the end of the matching section if the key was absent.
        let next_is_header = lines.get(i + 1).is_none_or(|n| n.trim().starts_with('['));
        if current == section && next_is_header && !done {
            if let Some(v) = value {
                out.push(format!("{name} = {}", quote(v)));
            }
            done = true;
        }
    }
    if !done && !section_seen {
        if let Some(v) = value {
            if !out.is_empty() && !out.last().is_some_and(|l| l.trim().is_empty()) {
                out.push(String::new());
            }
            if !section.is_empty() {
                out.push(format!("[{section}]"));
            }
            out.push(format!("{name} = {}", quote(v)));
        }
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// Write the config file; on Unix it is created/kept with mode 0600.
fn write_private(file: &Path, contents: &str) -> Result<(), AppError> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(file)
        .map_err(|e| AppError::io("write config", e))?;
    f.write_all(contents.as_bytes())
        .map_err(|e| AppError::io("write config", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(file, fs::Permissions::from_mode(0o600))
            .map_err(|e| AppError::io("chmod config", e))?;
    }
    Ok(())
}

/// Mask a secret: keep only a well-known public prefix (e.g. `tvly-`).
pub fn mask(secret: &str) -> String {
    let prefix = ["tvly-dev-", "tvly-prod-", "tvly-"]
        .into_iter()
        .find(|p| secret.starts_with(p))
        .unwrap_or("");
    format!("{prefix}****")
}

/// Short, non-reversible fingerprint so users can tell two keys apart.
pub fn fingerprint(secret: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in secret.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:08x}", (h >> 32) as u32)
}

/// Describe a value for output, masking secrets.
fn describe(key: &str, value: &str, secret: bool, source: &str) -> serde_json::Value {
    if secret {
        json!({ "key": key, "set": true, "source": source, "value": mask(value), "chars": value.chars().count(), "fingerprint": fingerprint(value) })
    } else {
        json!({ "key": key, "set": true, "source": source, "value": value })
    }
}

pub fn set(store: &Store, key: &str, value: Option<&str>) -> CmdResult {
    let (secret, _) = known(key)?;
    let value = match value {
        Some(v) if v != "-" => v.trim().to_string(),
        _ => {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).map_err(|e| {
                AppError::new(
                    "bad_args",
                    format!("reading stdin: {e}"),
                    "Pipe the value into stdin.",
                )
            })?;
            line.trim().to_string()
        }
    };
    if value.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "empty value",
            format!("Pass the value as an argument or on stdin, e.g. `agentbox config set {key} -` and paste it. Use `config unset {key}` to remove it."),
        ));
    }
    if key == "search.backend" && !["auto", "tavily", "ddg", "bing"].contains(&value.as_str()) {
        return Err(AppError::new(
            "bad_args",
            format!("bad backend `{value}`"),
            "Use one of: auto, tavily, ddg, bing.",
        ));
    }
    let file = path(store);
    store.ensure_dir(store.root())?;
    let old = fs::read_to_string(&file).unwrap_or_default();
    write_private(&file, &set_in(&old, key, Some(&value)))?;
    let mut data = describe(key, &value, secret, "config");
    data["path"] = json!(file.to_string_lossy());
    Ok(Output::new(data).hint(
        "Saved. Never commit this file; it lives outside your project in the agentbox state dir.",
    ))
}

pub fn unset(store: &Store, key: &str) -> CmdResult {
    known(key)?;
    let file = path(store);
    let old = fs::read_to_string(&file).unwrap_or_default();
    let existed = get_in(&old, key).is_some();
    if existed {
        write_private(&file, &set_in(&old, key, None))?;
    }
    Ok(Output::new(
        json!({ "key": key, "removed": existed, "path": file.to_string_lossy() }),
    ))
}

/// Resolve a key: environment variable first, then the config file.
/// Returns (value, source).
pub fn resolve(
    store: &Store,
    key: &str,
    env_value: Option<String>,
) -> Option<(String, &'static str)> {
    if let Some(v) = env_value
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        return Some((v, "env"));
    }
    read_value(&path(store), key).map(|v| (v, "config"))
}

pub fn env_for(key: &str) -> Option<String> {
    KEYS.iter()
        .find(|(k, _, _)| *k == key)
        .and_then(|(_, _, e)| *e)
        .and_then(|e| std::env::var(e).ok())
}

pub fn get(store: &Store, key: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> CmdResult {
    let keys: Vec<&str> = match key {
        Some(k) => {
            known(k)?;
            vec![k]
        }
        None => KEYS.iter().map(|(k, _, _)| *k).collect(),
    };
    let mut items = Vec::new();
    for k in keys {
        let (secret, env_name) = known(k)?;
        items.push(match resolve(store, k, env(k)) {
            Some((v, src)) => {
                let source = if src == "env" {
                    format!("env {}", env_name.unwrap_or(""))
                } else {
                    "config".into()
                };
                describe(k, &v, secret, &source)
            }
            None => json!({ "key": k, "set": false }),
        });
    }
    let unset_tavily = items
        .iter()
        .any(|i| i["key"] == "tavily.api_key" && i["set"] == false);
    let out = Output::new(json!({ "path": path(store).to_string_lossy(), "settings": items }));
    Ok(if unset_tavily {
        out.hint("No Tavily key: search uses keyless backends. Set one with env TAVILY_API_KEY or `agentbox config set tavily.api_key -` (reads stdin).")
    } else {
        out
    })
}

pub fn show_path(store: &Store) -> CmdResult {
    let file = path(store);
    Ok(Output::new(
        json!({ "path": file.to_string_lossy(), "exists": file.exists() }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "tvly-dev-SECRETsecret1234567890";

    #[test]
    fn set_get_unset_roundtrip_masks_secret() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        let out = set(&store, "tavily.api_key", Some(KEY)).unwrap();
        let s = serde_json::to_string(&out.data).unwrap();
        assert!(!s.contains("SECRET"), "{s}");
        assert_eq!(out.data["value"], "tvly-dev-****");
        assert_eq!(
            read_value(&path(&store), "tavily.api_key").as_deref(),
            Some(KEY)
        );

        let no_env = |_: &str| None;
        let out = get(&store, None, &no_env).unwrap();
        let s = serde_json::to_string(&out.data).unwrap();
        assert!(!s.contains("SECRET"), "{s}");
        assert_eq!(out.data["settings"][0]["source"], "config");
        assert_eq!(out.data["settings"][0]["fingerprint"], fingerprint(KEY));

        let env = |_: &str| Some("tvly-fromenv-XYZ".to_string());
        let out = get(&store, Some("tavily.api_key"), &env).unwrap();
        assert_eq!(out.data["settings"][0]["source"], "env TAVILY_API_KEY");
        assert!(!serde_json::to_string(&out.data).unwrap().contains("XYZ"));

        assert_eq!(
            unset(&store, "tavily.api_key").unwrap().data["removed"],
            true
        );
        assert!(read_value(&path(&store), "tavily.api_key").is_none());
        assert!(get(&store, None, &no_env)
            .unwrap()
            .hint
            .unwrap()
            .contains("No Tavily key"));
    }

    #[cfg(unix)]
    #[test]
    fn config_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        fs::write(path(&store), "[search]\nbackend = \"ddg\"\n").unwrap();
        fs::set_permissions(path(&store), fs::Permissions::from_mode(0o644)).unwrap();
        set(&store, "tavily.api_key", Some(KEY)).unwrap();
        let mode = fs::metadata(path(&store)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn set_in_preserves_other_content() {
        let text = "# my settings\n[search]\nbackend = \"ddg\"\n\n[other]\nx = 1\n";
        let t = set_in(text, "tavily.api_key", Some("k1"));
        assert!(
            t.contains("# my settings") && t.contains("backend = \"ddg\"") && t.contains("x = 1")
        );
        assert_eq!(get_in(&t, "tavily.api_key").as_deref(), Some("k1"));
        let t2 = set_in(&t, "tavily.api_key", Some("k2"));
        assert_eq!(t2.matches("api_key").count(), 1);
        assert_eq!(get_in(&t2, "tavily.api_key").as_deref(), Some("k2"));
        let t3 = set_in(&t2, "search.backend", Some("bing"));
        assert_eq!(get_in(&t3, "search.backend").as_deref(), Some("bing"));
        assert_eq!(get_in(&t3, "other.x").as_deref(), Some("1"));
        let t4 = set_in(&t3, "search.backend", None);
        assert_eq!(get_in(&t4, "search.backend"), None);
    }

    #[test]
    fn resolve_prefers_env() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        assert!(resolve(&store, "tavily.api_key", None).is_none());
        fs::write(path(&store), "[tavily]\napi_key = 'from-file'\n").unwrap();
        assert_eq!(
            resolve(&store, "tavily.api_key", None).unwrap(),
            ("from-file".to_string(), "config")
        );
        assert_eq!(
            resolve(&store, "tavily.api_key", Some("from-env".into()))
                .unwrap()
                .1,
            "env"
        );
        assert_eq!(
            resolve(&store, "tavily.api_key", Some("  ".into()))
                .unwrap()
                .1,
            "config"
        );
    }

    #[test]
    fn rejects_unknown_keys_and_bad_backend() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        assert_eq!(
            set(&store, "nope.key", Some("x")).unwrap_err().code,
            "bad_args"
        );
        assert_eq!(
            set(&store, "search.backend", Some("google"))
                .unwrap_err()
                .code,
            "bad_args"
        );
        assert!(mask("abcdef").starts_with("****"));
    }
}
