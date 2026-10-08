//! `report build`: render a Markdown draft skeleton from a template and notes.

use super::mdparse::Doc;
use super::template::{CitationStyle, SectionSpec, Template};
use crate::cmd::note::Note;
use crate::cmd::search::normalize_url;
use crate::state::{parse_handle, DocMeta};

#[derive(Debug, Clone)]
pub struct Built {
    pub content: String,
    pub todos: usize,
    pub placed: usize,
    pub unassigned: usize,
    pub sources: usize,
}

/// A citable source: URL (when known) plus display title.
#[derive(Debug, Clone, PartialEq)]
struct Source {
    url: Option<String>,
    title: String,
}

struct Renderer<'a> {
    t: &'a Template,
    docs: &'a [DocMeta],
    lines: Vec<String>,
    todo: usize,
    sources: Vec<Source>,
}

fn domain(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| h.trim_start_matches("www.").to_string())
        })
        .unwrap_or_else(|| url.to_string())
}

impl Renderer<'_> {
    fn todo(&mut self, text: &str) -> String {
        self.todo += 1;
        format!(
            "<!-- TODO({}): {} -->",
            self.todo,
            text.replace("-->", "->")
        )
    }

    fn push(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    fn resolve_source(&self, raw: &str) -> Source {
        let raw = raw.trim();
        let url = if raw.starts_with("http://") || raw.starts_with("https://") {
            Some(raw.to_string())
        } else if let Ok(id) = parse_handle(raw) {
            self.docs
                .iter()
                .find(|d| d.id == id)
                .map(|d| d.url.clone())
                .filter(|u| !u.is_empty())
        } else {
            None
        };
        let title = url
            .as_deref()
            .and_then(|u| {
                let n = normalize_url(u);
                self.docs
                    .iter()
                    .find(|d| normalize_url(&d.url) == n)
                    .map(|d| d.title.clone())
            })
            .filter(|t| !t.trim().is_empty())
            .or_else(|| url.as_deref().map(domain))
            .unwrap_or_else(|| raw.to_string());
        Source { url, title }
    }

    /// Register a source and return its citation marker.
    fn cite(&mut self, raw: &str) -> String {
        let src = self.resolve_source(raw);
        let key = |s: &Source| {
            s.url
                .as_deref()
                .map(normalize_url)
                .unwrap_or_else(|| s.title.to_lowercase())
        };
        let idx = match self.sources.iter().position(|s| key(s) == key(&src)) {
            Some(i) => i,
            None => {
                self.sources.push(src.clone());
                self.sources.len() - 1
            }
        };
        match (self.t.citation_style, &src.url) {
            (CitationStyle::InlineLink, Some(u)) => format!(" ([{}]({u}))", domain(u)),
            (CitationStyle::InlineLink, None) => format!(" (source: {})", src.title),
            (CitationStyle::Numbered, _) => format!(" [{}]", idx + 1),
            (CitationStyle::Footnote, _) => format!("[^{}]", idx + 1),
        }
    }

    fn hint(&self, spec: &SectionSpec) -> String {
        let mut h = if spec.hint.trim().is_empty() {
            "Write this section.".to_string()
        } else {
            spec.hint.trim().to_string()
        };
        match (spec.min_words, spec.max_words) {
            (Some(a), Some(b)) => h.push_str(&format!(" {a}-{b} words.")),
            (Some(a), None) => h.push_str(&format!(" At least {a} words.")),
            (None, Some(b)) => h.push_str(&format!(" At most {b} words.")),
            _ => {}
        }
        if spec.require_citation {
            h.push_str(&format!(
                " Cite sources like {}.",
                self.t.citation_style.example()
            ));
        }
        if !spec.must_contain.is_empty() {
            h.push_str(&format!(" Must mention: {}.", spec.must_contain.join(", ")));
        }
        h
    }

    fn section_body(&mut self, spec: &SectionSpec, notes: &[&Note]) {
        self.push("");
        if spec.table {
            let cols = if spec.columns.is_empty() {
                vec!["Item".to_string(), "Details".to_string()]
            } else {
                spec.columns.clone()
            };
            self.push(format!("| {} |", cols.join(" | ")));
            self.push(format!("|{}", "---|".repeat(cols.len())));
            let first = self.todo("one row per option");
            let rest = " |".repeat(cols.len() - 1);
            self.push(format!("| {first} |{rest}"));
            self.push("");
        }
        for n in notes {
            let cite = n
                .source
                .as_deref()
                .map(|s| self.cite(s))
                .unwrap_or_default();
            self.push(format!("- {}{cite}", n.text.trim()));
        }
        if !notes.is_empty() {
            self.push("");
        }
        let hint = self.hint(spec);
        let todo = self.todo(&hint);
        self.push(todo);
        self.push("");
    }
}

fn keywords(spec: &SectionSpec) -> Vec<String> {
    let mut kws: Vec<String> = spec.keywords.iter().map(|k| k.to_lowercase()).collect();
    kws.extend(
        spec.heading
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() >= 4)
            .map(str::to_lowercase),
    );
    kws
}

fn tag_matches(tag: &str, kws: &[String]) -> bool {
    let tag = tag.to_lowercase();
    kws.iter().any(|k| {
        *k == tag
            || (k.len() >= 4
                && tag.len() >= 4
                && (tag.starts_with(k.as_str()) || k.starts_with(tag.as_str())))
    })
}

/// Item number from tags like `1`, `item1`, `item-2`, `#3`, `n4`.
fn item_number(tag: &str) -> Option<usize> {
    let t = tag.trim().to_lowercase();
    let t = t
        .trim_start_matches("item")
        .trim_start_matches('#')
        .trim_start_matches('n')
        .trim_start_matches(['-', '_', ' ']);
    t.parse().ok()
}

fn take<'n>(idx: &[usize], notes: &'n [Note], assigned: &mut [bool]) -> Vec<&'n Note> {
    idx.iter()
        .map(|&k| {
            assigned[k] = true;
            &notes[k]
        })
        .collect()
}

pub fn render(t: &Template, title: &str, notes: &[Note], docs: &[DocMeta]) -> Built {
    let mut r = Renderer {
        t,
        docs,
        lines: Vec::new(),
        todo: 0,
        sources: Vec::new(),
    };
    let title = title.trim();
    if t.title_level > 0 {
        r.push(format!(
            "{} {}",
            "#".repeat(t.title_level as usize),
            if title.is_empty() { "Report" } else { title }
        ));
        r.push("");
    }

    // Assign notes to sections (and numbered items) by tag.
    let mut assigned: Vec<bool> = vec![false; notes.len()];
    let mut placed = 0;
    for spec in &t.sections {
        let level = "#".repeat(spec.level as usize);
        if let Some(p) = spec.numbered() {
            let count = spec.repeat_min.or(spec.repeat_max).unwrap_or(3);
            for i in 1..=count {
                let idx: Vec<usize> = (0..notes.len())
                    .filter(|&k| {
                        !assigned[k] && notes[k].tag.as_deref().and_then(item_number) == Some(i)
                    })
                    .collect();
                let mine = take(&idx, notes, &mut assigned);
                placed += mine.len();
                let heading_title = if p.has_title {
                    r.todo("item title")
                } else {
                    String::new()
                };
                r.push(format!("{level} {}", p.render(i, &heading_title)));
                r.section_body(spec, &mine);
            }
        } else {
            let kws = keywords(spec);
            let idx: Vec<usize> = (0..notes.len())
                .filter(|&k| {
                    !assigned[k]
                        && notes[k]
                            .tag
                            .as_deref()
                            .is_some_and(|tg| tag_matches(tg, &kws))
                })
                .collect();
            let mine = take(&idx, notes, &mut assigned);
            placed += mine.len();
            r.push(format!("{level} {}", spec.heading));
            r.section_body(spec, &mine);
        }
    }

    // Unplaced notes: keep them visible to the writer, out of the rendered text.
    let rest: Vec<&Note> = notes
        .iter()
        .enumerate()
        .filter(|(k, _)| !assigned[*k])
        .map(|(_, n)| n)
        .collect();
    if !rest.is_empty() {
        r.push("<!-- NOTES (not placed in a section; move useful facts above with their citations, then delete this block):");
        for n in &rest {
            let cite = n.source.as_deref().map(|s| r.cite(s)).unwrap_or_default();
            r.push(format!("- {}{cite}", n.text.trim().replace("-->", "->")));
        }
        r.push("-->");
        r.push("");
    }

    // Sources.
    if t.require_sources_section || !r.sources.is_empty() {
        r.push(format!(
            "{} {}",
            "#".repeat(t.sources_level as usize),
            t.sources_heading
        ));
        r.push("");
        let srcs = r.sources.clone();
        for (i, s) in srcs.iter().enumerate() {
            let entry = match &s.url {
                Some(u) => format!("[{}]({u})", s.title.replace(['[', ']'], "")),
                None => s.title.clone(),
            };
            r.push(match t.citation_style {
                CitationStyle::InlineLink => format!("- {entry}"),
                CitationStyle::Numbered => format!("{}. {entry}", i + 1),
                CitationStyle::Footnote => format!("[^{}]: {entry}", i + 1),
            });
        }
        let have = srcs.iter().filter(|s| s.url.is_some()).count();
        if have < t.min_sources {
            let known: Vec<String> = srcs
                .iter()
                .filter_map(|s| s.url.as_deref().map(normalize_url))
                .collect();
            let candidates: Vec<String> = docs
                .iter()
                .filter(|d| !d.url.is_empty() && !known.contains(&normalize_url(&d.url)))
                .take(5)
                .map(|d| format!("[{}]({})", d.title.replace(['[', ']'], ""), d.url))
                .collect();
            let mut text = format!(
                "add at least {} more source(s), one per line",
                t.min_sources - have
            );
            if !candidates.is_empty() {
                text.push_str(&format!(
                    "; fetched docs you could cite: {}",
                    candidates.join("; ")
                ));
            }
            if !srcs.is_empty() {
                r.push("");
            }
            let todo = r.todo(&text);
            r.push(todo);
        }
        r.push("");
    }

    let mut content = r.lines.join("\n");
    while content.ends_with("\n\n") {
        content.pop();
    }
    if !content.ends_with('\n') {
        content.push('\n');
    }
    let unassigned = rest.len();
    Built {
        content,
        todos: r.todo,
        placed,
        unassigned,
        sources: r.sources.len(),
    }
}

pub fn outline(content: &str) -> serde_json::Value {
    let doc = Doc::parse(content);
    serde_json::Value::Array(
        doc.headings
            .iter()
            .map(|h| serde_json::json!({ "line": h.line, "level": h.level, "heading": h.text }))
            .collect(),
    )
}
