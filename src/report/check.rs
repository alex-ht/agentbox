//! `report check`: grade a Markdown report against a template, read-only.
//! Every issue carries a concrete `fix` a small model can act on.

use super::mdparse::{citations, count_words, find_table, strip_comments, urls, Doc};
use super::template::{CitationStyle, Template};
use crate::cmd::search::normalize_url;
use serde::Serialize;
use serde_json::json;

const ERROR_WEIGHT: i64 = 12;
const WARN_WEIGHT: i64 = 4;
const SOURCE_ALIASES: &[&str] = &[
    "sources",
    "source",
    "references",
    "reference",
    "bibliography",
    "citations",
    "links",
    "參考資料",
    "資料來源",
    "來源",
    "參考來源",
    "参考资料",
];

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Issue {
    pub severity: &'static str,
    pub rule: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
    pub message: String,
    pub fix: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
pub struct Stats {
    pub words: usize,
    pub sections: usize,
    pub citations: usize,
    pub sources: usize,
    pub todos_left: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    pub pass: bool,
    pub score: i64,
    pub issues: Vec<Issue>,
    pub stats: Stats,
}

impl CheckReport {
    pub fn errors(&self) -> usize {
        self.issues.iter().filter(|i| i.severity == "error").count()
    }
}

/// Shell-quote for the suggested commands (double quotes, escaped).
/// Heading text that belongs to a literal template section or the sources section.
fn is_reserved(t: &Template, text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    t.sections
        .iter()
        .any(|s| s.numbered().is_none() && s.names(text))
        || text.trim().eq_ignore_ascii_case(t.sources_heading.trim())
        || SOURCE_ALIASES.contains(&lower.as_str())
}

fn q(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$")
    )
}

fn replace_cmd(file: &str, old: &str, new: &str) -> String {
    format!(
        "agentbox file replace {} --find {} --replace {} --apply",
        q(file),
        q(old),
        q(new)
    )
}

fn hashes(level: u8) -> String {
    "#".repeat(level as usize)
}

struct Checker<'a> {
    doc: Doc,
    t: &'a Template,
    issues: Vec<Issue>,
    used: Vec<bool>,
}

impl Checker<'_> {
    fn err(&mut self, rule: &'static str, line: Option<usize>, message: String, fix: String) {
        self.issues.push(Issue {
            severity: "error",
            rule,
            line,
            message,
            fix,
        });
    }

    fn warn(&mut self, rule: &'static str, line: Option<usize>, message: String, fix: String) {
        self.issues.push(Issue {
            severity: "warn",
            rule,
            line,
            message,
            fix,
        });
    }

    fn heading_label(&self, idx: usize) -> String {
        self.doc.headings[idx].raw.trim().to_string()
    }

    /// Content rules shared by literal and numbered sections.
    fn check_content(&mut self, idx: usize, spec: &super::template::SectionSpec) {
        let range = self.doc.section_range(idx);
        let body = self.doc.text_of(range);
        let label = self.heading_label(idx);
        let ln = self.doc.headings[idx].line;
        let line = Some(ln);
        let words = count_words(&body);
        if let Some(min) = spec.min_words {
            if words < min {
                self.err(
                    "section_words",
                    line,
                    format!("`{label}` has {words} words; needs at least {min}"),
                    format!(
                        "Add about {} more words of concrete facts under `{label}` (line {}).",
                        min - words,
                        ln
                    ),
                );
            }
        }
        if let Some(max) = spec.max_words {
            if words > max {
                self.err(
                    "section_words",
                    line,
                    format!("`{label}` has {words} words; allowed at most {max}"),
                    format!(
                        "Shorten `{label}` by about {} words; keep the key facts.",
                        words - max
                    ),
                );
            }
        }
        let cites = citations(&body);
        if spec.require_citation && cites.total() == 0 {
            self.err(
                "citation",
                line,
                format!("`{label}` has no citation"),
                format!(
                    "Add a citation in {} style to `{label}`, e.g. {}. Use a URL from `agentbox note list`.",
                    self.t.citation_style.name(),
                    self.t.citation_style.example()
                ),
            );
        } else if cites.total() > 0 {
            let in_style = match self.t.citation_style {
                CitationStyle::InlineLink => cites.inline,
                CitationStyle::Footnote => cites.footnote,
                CitationStyle::Numbered => cites.numbered,
            };
            if in_style == 0 {
                self.warn(
                    "citation_style",
                    line,
                    format!(
                        "`{label}` cites sources, but not in the template's {} style",
                        self.t.citation_style.name()
                    ),
                    format!(
                        "Rewrite the citations in `{label}` like {}.",
                        self.t.citation_style.example()
                    ),
                );
            }
        }
        for kw in &spec.must_contain {
            if !body.to_lowercase().contains(&kw.to_lowercase()) {
                self.err(
                    "must_contain",
                    line,
                    format!("`{label}` does not mention \"{kw}\""),
                    format!("Add a sentence mentioning \"{kw}\" under `{label}`."),
                );
            }
        }
        if spec.table {
            match find_table(&self.doc, range) {
                None => {
                    let cols = if spec.columns.is_empty() {
                        vec!["Item".to_string(), "Details".to_string()]
                    } else {
                        spec.columns.clone()
                    };
                    self.err(
                        "table",
                        line,
                        format!("`{label}` needs a Markdown table"),
                        format!(
                            "Add a table under `{label}` starting with the header `| {} |` and a separator row `|{}`.",
                            cols.join(" | "),
                            "---|".repeat(cols.len())
                        ),
                    );
                }
                Some(tbl) => {
                    let missing: Vec<&String> = spec
                        .columns
                        .iter()
                        .filter(|c| !tbl.columns.iter().any(|h| h.eq_ignore_ascii_case(c.trim())))
                        .collect();
                    if !missing.is_empty() {
                        let names: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
                        let header = format!("| {} |", spec.columns.join(" | "));
                        self.err(
                            "table_columns",
                            Some(tbl.line),
                            format!("table under `{label}` lacks column(s): {}", names.join(", ")),
                            format!(
                                "Make the header on line {} exactly `{header}` and fill the new cells in every row.",
                                tbl.line
                            ),
                        );
                    }
                    if tbl.rows == 0 {
                        self.warn(
                            "table",
                            Some(tbl.line),
                            format!("table under `{label}` has no data rows"),
                            "Add one row per item below the separator row.".into(),
                        );
                    }
                }
            }
        }
    }
}

/// Check `text` against template `t`. `allowed` is the normalized set of
/// URLs from notes and fetched docs (used when `sources_from_notes`).
pub fn check_text(text: &str, t: &Template, file: &str, allowed: Option<&[String]>) -> CheckReport {
    let doc = Doc::parse(text);
    let n_headings = doc.headings.len();
    let mut c = Checker {
        doc,
        t,
        issues: Vec::new(),
        used: vec![false; n_headings],
    };

    // --- leftovers: TODO markers and notes blocks
    let mut todos = 0;
    for i in 0..c.doc.lines.len() {
        if c.doc.in_code[i] {
            continue;
        }
        let line = c.doc.lines[i].clone();
        if let Some(start) = line.find("<!-- TODO") {
            todos += 1;
            let marker = match line[start..].find("-->") {
                Some(end) => line[start..start + end + 3].to_string(),
                None => line[start..].trim().to_string(),
            };
            c.err(
                "todo",
                Some(i + 1),
                format!("unfinished placeholder: {marker}"),
                format!(
                    "Write the real content, then run: {}",
                    replace_cmd(file, &marker, "YOUR TEXT")
                ),
            );
        }
        if line.contains("<!-- NOTES") {
            let end = (i..c.doc.lines.len())
                .find(|&j| c.doc.lines[j].contains("-->"))
                .unwrap_or(i);
            c.warn(
                "notes_block",
                Some(i + 1),
                format!("leftover notes block (lines {}-{})", i + 1, end + 1),
                format!(
                    "Move any useful facts into the sections above, then delete lines {}-{}.",
                    i + 1,
                    end + 1
                ),
            );
        }
    }

    // --- forbidden phrases
    for phrase in &t.forbid {
        let p = phrase.to_lowercase();
        for i in 0..c.doc.lines.len() {
            if !c.doc.in_code[i] && c.doc.lines[i].to_lowercase().contains(&p) {
                c.err(
                    "forbidden",
                    Some(i + 1),
                    format!("forbidden phrase \"{phrase}\""),
                    format!(
                        "Rewrite line {} without \"{phrase}\"; state the facts directly.",
                        i + 1
                    ),
                );
            }
        }
    }

    // --- heading syntax problems
    for line_no in c.doc.nospace.clone() {
        let raw = c.doc.lines[line_no - 1].clone();
        let t2 = raw.trim_start();
        let n = t2.chars().take_while(|&ch| ch == '#').count();
        let fixed = format!("{} {}", &t2[..n], &t2[n..]);
        c.err(
            "heading_format",
            Some(line_no),
            format!("`{}` is not a heading (no space after #)", raw.trim()),
            replace_cmd(file, raw.trim(), &fixed),
        );
    }
    for line_no in c.doc.setext.clone() {
        let text_line = c.doc.lines[line_no - 2].trim().to_string();
        let lvl = if c.doc.lines[line_no - 1].trim().starts_with('=') {
            1
        } else {
            2
        };
        c.warn(
            "heading_format",
            Some(line_no - 1),
            format!("`{text_line}` is an underlined (setext) heading; graders usually expect `#` headings"),
            format!("Replace lines {}-{} with the single line `{} {text_line}`.", line_no - 1, line_no, hashes(lvl)),
        );
    }

    // --- title
    let mut title_idx: Option<usize> = None;
    if t.title_level > 0 {
        let first = (0..c.doc.lines.len()).find(|&i| {
            !c.doc.lines[i].trim().is_empty() && !c.doc.lines[i].trim_start().starts_with("<!--")
        });
        let first_heading = c.doc.headings.first().cloned();
        match (first, first_heading) {
            (Some(f), Some(h)) if h.line == f + 1 => {
                title_idx = Some(0);
                if h.level != t.title_level {
                    let fixed = format!("{} {}", hashes(t.title_level), h.text);
                    c.err(
                        "title",
                        Some(h.line),
                        format!(
                            "title must be a level-{} heading, found level {}",
                            t.title_level, h.level
                        ),
                        replace_cmd(file, h.raw.trim(), &fixed),
                    );
                }
            }
            _ => {
                c.err(
                    "title",
                    Some(1),
                    "the first line must be the report title".into(),
                    format!(
                        "Insert `{} Your Report Title` as line 1.",
                        hashes(t.title_level)
                    ),
                );
            }
        }
        if let Some(ti) = title_idx {
            c.used[ti] = true;
        }
        if t.title_level == 1 {
            let extra: Vec<usize> = (0..c.doc.headings.len())
                .filter(|&i| Some(i) != title_idx && c.doc.headings[i].level == 1)
                .collect();
            for i in extra {
                let h = c.doc.headings[i].clone();
                c.err(
                    "multiple_h1",
                    Some(h.line),
                    format!(
                        "extra level-1 heading `{}`; only the title may use `#`",
                        h.raw.trim()
                    ),
                    replace_cmd(file, h.raw.trim(), &format!("## {}", h.text)),
                );
            }
        }
    }

    // --- sections
    let mut order: Vec<(String, usize)> = Vec::new(); // (label, heading idx)
    let mut last_found: Option<usize> = title_idx;
    for spec in &t.sections {
        if let Some(p) = spec.numbered() {
            let cands: Vec<(usize, usize, String, bool)> = (0..c.doc.headings.len())
                .filter(|&i| !c.used[i])
                .filter_map(|i| {
                    let text = &c.doc.headings[i].text;
                    if let Some((n, title)) = p.matches(text) {
                        Some((i, n, title, true))
                    } else {
                        p.loose(text).map(|(n, title)| (i, n, title, false))
                    }
                })
                .collect();
            let min = spec.repeat_min.unwrap_or(1);
            let max = spec.repeat_max.unwrap_or(usize::MAX);
            let pattern_label = format!("{} {}", hashes(spec.level), spec.heading);
            for (j, (i, n, title, strict)) in cands.iter().enumerate() {
                c.used[*i] = true;
                let h = c.doc.headings[*i].clone();
                if j >= max {
                    c.err(
                        "item_count",
                        Some(h.line),
                        format!(
                            "extra numbered item `{}`; the template allows at most {max}",
                            h.raw.trim()
                        ),
                        format!(
                            "Delete or merge the `{}` section (from line {}) into another item.",
                            h.raw.trim(),
                            h.line
                        ),
                    );
                    continue;
                }
                let want_n = j + 1;
                let title_text = if title.is_empty() {
                    "Title".to_string()
                } else {
                    title.clone()
                };
                let canonical = format!("{} {}", hashes(spec.level), p.render(want_n, &title_text));
                if h.level != spec.level {
                    c.err(
                        "heading_level",
                        Some(h.line),
                        format!(
                            "`{}` must be a level-{} heading (`{} `), found level {}",
                            h.raw.trim(),
                            spec.level,
                            hashes(spec.level),
                            h.level
                        ),
                        replace_cmd(file, h.raw.trim(), &canonical),
                    );
                } else if *n != want_n {
                    c.err(
                        "numbering",
                        Some(h.line),
                        format!("item `{}` should be number {want_n}", h.raw.trim()),
                        replace_cmd(file, h.raw.trim(), &canonical),
                    );
                } else if !strict {
                    c.err(
                        "numbering",
                        Some(h.line),
                        format!(
                            "`{}` does not match the required format `{pattern_label}`",
                            h.raw.trim()
                        ),
                        replace_cmd(file, h.raw.trim(), &canonical),
                    );
                }
                c.check_content(*i, spec);
            }
            // Unnumbered headings right after the numbered run (e.g. `## Rocket`)
            // are most likely items that lost their number: adopt them.
            let mut adopted: Vec<usize> = Vec::new();
            if cands.len() < min {
                let start_line = cands
                    .last()
                    .map(|(i, ..)| c.doc.headings[*i].line)
                    .or(last_found.map(|i| c.doc.headings[i].line))
                    .unwrap_or(0);
                let stop_line = c
                    .doc
                    .headings
                    .iter()
                    .filter(|h| h.line > start_line && is_reserved(t, &h.text))
                    .map(|h| h.line)
                    .min()
                    .unwrap_or(usize::MAX);
                adopted = (0..c.doc.headings.len())
                    .filter(|&i| {
                        let h = &c.doc.headings[i];
                        !c.used[i]
                            && h.line > start_line
                            && h.line < stop_line
                            && h.level >= 2
                            && h.level <= spec.level + 1
                    })
                    .take(min - cands.len())
                    .collect();
                for (j, &i) in adopted.iter().enumerate() {
                    c.used[i] = true;
                    let h = c.doc.headings[i].clone();
                    let n = cands.len() + j + 1;
                    let canonical =
                        format!("{} {}", hashes(spec.level), p.render(n, h.text.trim()));
                    c.err(
                        "numbering",
                        Some(h.line),
                        format!(
                            "`{}` looks like item {n} but does not match `{pattern_label}`",
                            h.raw.trim()
                        ),
                        replace_cmd(file, h.raw.trim(), &canonical),
                    );
                    c.check_content(i, spec);
                }
            }
            let k = cands.len() + adopted.len();
            if k < min {
                let after = adopted
                    .last()
                    .copied()
                    .or(cands.last().map(|(i, ..)| *i))
                    .map(|i| c.doc.headings[i].line)
                    .or(last_found.map(|i| c.doc.headings[i].line));
                let missing: Vec<String> = (k + 1..=min)
                    .map(|n| format!("`{} {}`", hashes(spec.level), p.render(n, "Title")))
                    .collect();
                c.err(
                    "item_count",
                    after,
                    format!("found {k} of {min} required `{pattern_label}` sections"),
                    format!(
                        "Add {} {}, each with its own content{}.",
                        missing.join(", "),
                        match after {
                            Some(l) => format!("after the section that starts at line {l}"),
                            None => "after the title".into(),
                        },
                        if spec.require_citation {
                            " and a citation"
                        } else {
                            ""
                        }
                    ),
                );
            }
            if let Some(&first) = cands.first().map(|(i, ..)| i).or(adopted.first()) {
                order.push((pattern_label, first));
                last_found = adopted.last().copied().or(cands.last().map(|(i, ..)| *i));
            }
        } else {
            let exact = (0..c.doc.headings.len()).find(|&i| {
                !c.used[i]
                    && c.doc.headings[i].level == spec.level
                    && spec.names(&c.doc.headings[i].text)
            });
            let any_level = (0..c.doc.headings.len())
                .find(|&i| !c.used[i] && spec.names(&c.doc.headings[i].text));
            let found = exact.or(any_level);
            match found {
                Some(i) => {
                    c.used[i] = true;
                    let h = c.doc.headings[i].clone();
                    if h.level != spec.level {
                        c.err(
                            "heading_level",
                            Some(h.line),
                            format!(
                                "`{}` must be a level-{} heading, found level {}",
                                h.raw.trim(),
                                spec.level,
                                h.level
                            ),
                            replace_cmd(file, h.raw.trim(), &spec.label()),
                        );
                    }
                    c.check_content(i, spec);
                    order.push((spec.label(), i));
                    last_found = Some(i);
                }
                None if spec.required => {
                    let similar = (0..c.doc.headings.len()).find(|&i| {
                        let ht = c.doc.headings[i].text.to_lowercase();
                        let st = spec.heading.to_lowercase();
                        !c.used[i] && (ht.contains(&st) || (st.contains(&ht) && ht.len() >= 4))
                    });
                    let fix = match similar {
                        Some(i) => {
                            let raw = c.doc.headings[i].raw.trim().to_string();
                            c.used[i] = true;
                            format!("Rename the heading on line {}: {}", c.doc.headings[i].line, replace_cmd(file, &raw, &spec.label()))
                        }
                        None => match last_found {
                            Some(li) => format!(
                                "Add a `{}` section with content after the section that starts at line {} (`{}`).",
                                spec.label(),
                                c.doc.headings[li].line,
                                c.doc.headings[li].raw.trim()
                            ),
                            None => format!("Add a `{}` section with content right after the title.", spec.label()),
                        },
                    };
                    c.err(
                        "missing_section",
                        similar.map(|i| c.doc.headings[i].line),
                        format!("required section `{}` is missing", spec.label()),
                        fix,
                    );
                }
                None => {}
            }
        }
    }

    // --- sources section
    let src_label = format!("{} {}", hashes(t.sources_level), t.sources_heading);
    let mut sources_range: Option<(usize, usize)> = None;
    let src_exact = (0..c.doc.headings.len()).find(|&i| {
        !c.used[i]
            && c.doc.headings[i]
                .text
                .trim()
                .eq_ignore_ascii_case(t.sources_heading.trim())
    });
    let src_alias = (0..c.doc.headings.len()).find(|&i| {
        !c.used[i]
            && SOURCE_ALIASES.contains(&c.doc.headings[i].text.trim().to_lowercase().as_str())
    });
    if let Some(i) = src_exact.or(src_alias) {
        c.used[i] = true;
        let h = c.doc.headings[i].clone();
        sources_range = Some(c.doc.section_range(i));
        if (src_exact.is_none() || h.level != t.sources_level) && t.require_sources_section {
            c.err(
                "sources_section",
                Some(h.line),
                format!(
                    "sources heading must be exactly `{src_label}`, found `{}`",
                    h.raw.trim()
                ),
                replace_cmd(file, h.raw.trim(), &src_label),
            );
        }
        order.push((src_label.clone(), i));
    } else if t.require_sources_section {
        c.err(
            "sources_section",
            None,
            format!("missing `{src_label}` section"),
            format!("Add `{src_label}` at the end, with one source per line like `- [Page title](https://...)`."),
        );
    }

    // --- headings the template does not know about
    let top_level = t
        .sections
        .iter()
        .map(|s| s.level)
        .chain(std::iter::once(t.sources_level))
        .max()
        .unwrap_or(2);
    let names: Vec<String> = t
        .sections
        .iter()
        .map(|s| format!("`{}`", s.label()))
        .collect();
    for i in 0..c.doc.headings.len() {
        let h = c.doc.headings[i].clone();
        if c.used[i] || h.level < 2 || h.level > top_level {
            continue;
        }
        c.warn(
            "extra_section",
            Some(h.line),
            format!("`{}` is not a section of the `{}` template", h.raw.trim(), t.name),
            format!(
                "Merge its content into one of {} and delete the heading on line {} (or use a deeper heading level for a sub-section).",
                names.join(", "),
                h.line
            ),
        );
    }

    // --- order
    let mut max_seen: Option<(String, usize)> = None;
    for (label, idx) in &order {
        if let Some((prev_label, prev_idx)) = &max_seen {
            if idx < prev_idx {
                let line = c.doc.headings[*idx].line;
                let prev_line = c.doc.headings[*prev_idx].line;
                c.err(
                    "order",
                    Some(line),
                    format!("`{label}` (line {line}) must come after `{prev_label}` (line {prev_line})"),
                    format!("Move the whole `{label}` section (starting line {line}) below the `{prev_label}` section."),
                );
                continue;
            }
        }
        max_seen = Some((label.clone(), *idx));
    }

    // --- sources count and provenance
    let body_text = match sources_range {
        Some((s, e)) => {
            let mut v = c.doc.lines.clone();
            for line in v.iter_mut().take(e).skip(s.saturating_sub(1)) {
                line.clear();
            }
            v.join("\n")
        }
        None => text.to_string(),
    };
    let source_urls: Vec<String> = match sources_range {
        Some(r) => urls(&c.doc.text_of(r)),
        None => urls(text),
    };
    let mut distinct: Vec<String> = Vec::new();
    for u in &source_urls {
        let n = normalize_url(u);
        if !distinct.contains(&n) {
            distinct.push(n);
        }
    }
    if distinct.len() < t.min_sources {
        c.err(
            "min_sources",
            sources_range.map(|r| r.0),
            format!("{} distinct source URL(s) in `{src_label}`; need at least {}", distinct.len(), t.min_sources),
            format!(
                "Add {} more source line(s) like `- [Title](https://...)` under `{src_label}`, using URLs from `agentbox note list` (find more with `agentbox search`).",
                t.min_sources - distinct.len()
            ),
        );
    }
    if t.sources_from_notes {
        match allowed {
            Some(list) if !list.is_empty() => {
                let mut reported: Vec<String> = Vec::new();
                for (i, line) in c.doc.lines.clone().iter().enumerate() {
                    if c.doc.in_code[i] {
                        continue;
                    }
                    for u in urls(line) {
                        let n = normalize_url(&u);
                        if !list.contains(&n) && !reported.contains(&n) {
                            reported.push(n);
                            c.err(
                                "source_not_in_notes",
                                Some(i + 1),
                                format!("{u} was not fetched or noted in this session"),
                                format!("Cite a URL from `agentbox note list` instead, or verify it first: `agentbox fetch {u}` then `agentbox note add \"<fact>\" --source {u}`."),
                            );
                        }
                    }
                }
            }
            _ => c.err(
                "source_not_in_notes",
                None,
                "template requires sources from notes/fetched docs, but none are recorded".into(),
                "Fetch your sources with `agentbox fetch URL` (or `agentbox search ... --save N`) and record facts with `agentbox note add \"...\" --source URL`.".into(),
            ),
        }
    }

    // --- document length
    let title_line = title_idx.map(|i| c.doc.headings[i].line);
    let words_text: String = body_text
        .lines()
        .enumerate()
        .filter(|(i, _)| {
            Some(i + 1) != title_line && !c.doc.in_code.get(*i).copied().unwrap_or(false)
        })
        .map(|(_, l)| l)
        .collect::<Vec<_>>()
        .join("\n");
    let words = count_words(&words_text);
    if let Some(min) = t.min_words {
        if words < min {
            c.err(
                "doc_words",
                None,
                format!(
                    "report has {words} words (excluding title and sources); needs at least {min}"
                ),
                format!(
                    "Add about {} words of concrete, cited facts to the shortest sections.",
                    min - words
                ),
            );
        }
    }
    if let Some(max) = t.max_words {
        if words > max {
            c.err(
                "doc_words",
                None,
                format!("report has {words} words; allowed at most {max}"),
                format!(
                    "Cut about {} words: remove repetition and filler first.",
                    words - max
                ),
            );
        }
    }

    let stats = Stats {
        words,
        sections: c.doc.headings.len() - usize::from(title_idx.is_some()),
        citations: citations(&strip_comments(&body_text)).total(),
        sources: distinct.len(),
        todos_left: todos,
    };
    let mut issues = c.issues;
    issues.sort_by_key(|i| (i.line.unwrap_or(usize::MAX), i.severity != "error"));
    let errors = issues.iter().filter(|i| i.severity == "error").count() as i64;
    let warns = issues.len() as i64 - errors;
    let score = (100 - ERROR_WEIGHT * errors - WARN_WEIGHT * warns).max(0);
    CheckReport {
        pass: errors == 0,
        score,
        issues,
        stats,
    }
}

pub fn to_json(r: &CheckReport, template: &str, file: &str) -> serde_json::Value {
    json!({
        "file": file,
        "template": template,
        "pass": r.pass,
        "score": r.score,
        "issues": r.issues,
        "stats": r.stats,
    })
}
