//! `report build|check|templates|template show`.

pub mod build;
pub mod check;
pub mod mdparse;
pub mod template;

use crate::cmd::note;
use crate::cmd::search::normalize_url;
use crate::envelope::{AppError, CmdResult, Output};
use crate::state::{parse_handle, Store};
use serde_json::json;
use template::Overrides;

/// URLs the report may cite when `sources_from_notes` is on.
pub fn allowed_urls(store: &Store) -> Vec<String> {
    let metas = store.doc_metas();
    let mut out: Vec<String> = metas
        .iter()
        .filter(|d| !d.url.is_empty())
        .map(|d| normalize_url(&d.url))
        .collect();
    for n in note::load(store) {
        let Some(src) = n.source else { continue };
        let src = src.trim().to_string();
        if src.starts_with("http://") || src.starts_with("https://") {
            out.push(normalize_url(&src));
        } else if let Ok(id) = parse_handle(&src) {
            if let Some(d) = metas.iter().find(|d| d.id == id) {
                out.push(normalize_url(&d.url));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

pub struct BuildArgs<'a> {
    pub title: &'a str,
    pub template: &'a str,
    pub overrides: Overrides,
    pub tag: Option<&'a str>,
    pub out: Option<&'a str>,
    pub apply: bool,
}

pub fn run_build(store: &Store, a: &BuildArgs) -> CmdResult {
    let (t, _, origin) = template::load(store, a.template)?;
    let t = t.apply(&a.overrides)?;
    let notes: Vec<note::Note> = note::load(store)
        .into_iter()
        .filter(|n| {
            a.tag.is_none_or(|tag| {
                n.tag
                    .as_deref()
                    .is_some_and(|nt| nt.eq_ignore_ascii_case(tag))
            })
        })
        .collect();
    let built = build::render(&t, a.title, &notes, &store.doc_metas());
    let outline = build::outline(&built.content);
    let summary = json!({
        "template": t.name,
        "template_origin": origin,
        "outline": outline,
        "todos": built.todos,
        "notes_placed": built.placed,
        "notes_unassigned": built.unassigned,
        "sources": built.sources,
    });
    let check_cmd = |path: &str| {
        let mut s = format!("agentbox report check {path} --template {}", a.template);
        if let Some(n) = a.overrides.n {
            s.push_str(&format!(" --n {n}"));
        }
        if let Some(c) = &a.overrides.columns {
            s.push_str(&format!(" --columns \"{c}\""));
        }
        s
    };
    match a.out {
        None => {
            let mut data = summary;
            data["content"] = json!(built.content);
            Ok(Output::new(data).hint(format!(
                "Draft only. Save it with `--out report.md --apply`, replace each <!-- TODO(n) --> via `agentbox file replace report.md --todo N --replace \"text\" --apply`, then run `{}`.",
                check_cmd("report.md")
            )))
        }
        Some(path) => {
            let written = crate::cmd::file::write(path, &built.content, a.apply)?;
            let mut data = summary;
            if let serde_json::Value::Object(m) = written.data {
                for (k, v) in m {
                    data[k] = v;
                }
            }
            let hint = if a.apply {
                format!(
                    "Written. Fill the {} TODOs one by one with `agentbox file replace {path} --todo 1 --replace \"text\" --apply` (TODO 2, 3, ... the same way), then run `{}`.",
                    built.todos,
                    check_cmd(path)
                )
            } else {
                "Preview only. Rerun the same command with --apply to write the file.".to_string()
            };
            Ok(Output::new(data).hint(hint))
        }
    }
}

pub fn run_check(
    store: &Store,
    file: &str,
    template_spec: &str,
    overrides: &Overrides,
) -> CmdResult {
    let (t, _, _) = template::load(store, template_spec)?;
    let t = t.apply(overrides)?;
    let text =
        std::fs::read_to_string(file).map_err(|e| AppError::io(&format!("read {file}"), e))?;
    let allowed = if t.sources_from_notes {
        Some(allowed_urls(store))
    } else {
        None
    };
    let report = check::check_text(&text, &t, file, allowed.as_deref());
    let errors = report.errors();
    let out = Output::new(check::to_json(&report, &t.name, file));
    Ok(if report.pass {
        out.hint("All required checks pass.")
    } else {
        out.hint(format!(
            "{errors} error(s). Apply the `fix` of each issue, top to bottom (line numbers refer to the file as checked; a fix may shift later lines), then re-run this check."
        ))
    })
}

#[cfg(test)]
mod tests;
