//! `read <doc>`: return a section, keyword-centered snippets, or a window of a stored doc.

use crate::envelope::{AppError, CmdResult, Output};
use crate::markdown::split_sections;
use crate::state::Store;
use serde_json::json;

/// Characters of context on each side of a grep match.
const GREP_CONTEXT: usize = 240;
/// Maximum snippets returned for a grep.
const GREP_MAX_SNIPPETS: usize = 8;

#[derive(Debug, Clone)]
pub struct ReadArgs {
    pub doc: String,
    pub section: Option<usize>,
    pub grep: Option<String>,
    pub max_chars: usize,
    pub offset: usize,
}

pub fn run(store: &Store, args: &ReadArgs) -> CmdResult {
    let (meta, body) = store.load_doc(&args.doc)?;
    let handle = format!("doc:{}", meta.id);
    read_body(&handle, &meta.title, &body, args)
}

/// Pure core of `read`, operating on an in-memory body.
pub fn read_body(handle: &str, title: &str, body: &str, args: &ReadArgs) -> CmdResult {
    let max_chars = args.max_chars.max(100);
    let sections = split_sections(body);

    if let Some(keyword) = args.grep.as_deref() {
        return grep(handle, title, body, &sections, keyword, max_chars);
    }

    let (text, section_info) = match args.section {
        Some(n) => {
            let sec = sections.iter().find(|s| s.index == n).ok_or_else(|| {
                AppError::new(
                    "bad_section",
                    format!("{handle} has no section {n} (it has {} sections)", sections.len()),
                    format!("Pick a section between 1 and {}; see the outline from `agentbox fetch`, or use `--grep KEYWORD`.", sections.len().max(1)),
                )
            })?;
            (&body[sec.start..sec.end], Some((n, sec.heading.clone())))
        }
        None => (body, None),
    };

    let total = text.chars().count();
    if args.offset > 0 && args.offset >= total {
        return Err(AppError::new(
            "bad_offset",
            format!("offset {} is past the end ({total} chars)", args.offset),
            "Use a smaller --offset; the previous response's `next_offset` tells you where to continue.",
        ));
    }
    let (chunk, truncated) = window(text, args.offset, max_chars);
    let returned = chunk.chars().count();
    let next_offset = args.offset + returned;
    let mut data = json!({ "doc": handle, "title": title });
    if let Some((n, heading)) = &section_info {
        data["section"] = json!(n);
        data["heading"] = json!(heading);
    }
    data["offset"] = json!(args.offset);
    data["chars"] = json!(returned);
    data["total_chars"] = json!(total);
    data["truncated"] = json!(truncated);
    if truncated {
        data["next_offset"] = json!(next_offset);
    }
    data["content"] = json!(chunk);

    let out = Output::new(data);
    Ok(if truncated {
        let sec_arg = section_info
            .map(|(n, _)| format!(" --section {n}"))
            .unwrap_or_default();
        out.hint(format!(
            "Truncated at {returned} of {total} chars. Continue with `agentbox read {handle}{sec_arg} --offset {next_offset}`."
        ))
    } else if section_info.is_none() && sections.len() > 1 {
        out.hint(format!("Whole doc returned. Next time use `--section N` (1-{}) or `--grep KEYWORD` to save context.", sections.len()))
    } else {
        out
    })
}

/// Take up to `max` chars starting at char `offset`. Returns (slice, truncated).
fn window(text: &str, offset: usize, max: usize) -> (&str, bool) {
    let start = text
        .char_indices()
        .nth(offset)
        .map(|(i, _)| i)
        .unwrap_or(text.len());
    let rest = &text[start..];
    match rest.char_indices().nth(max) {
        Some((end, _)) => (&rest[..end], true),
        None => (rest, false),
    }
}

fn grep(
    handle: &str,
    title: &str,
    body: &str,
    sections: &[crate::markdown::Section],
    keyword: &str,
    max_chars: usize,
) -> CmdResult {
    let needle: Vec<char> = keyword.trim().chars().map(lower).collect();
    if needle.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "--grep needs a non-empty keyword",
            "Example: `--grep revenue`.",
        ));
    }
    let mut snippets = Vec::new();
    let mut total_matches = 0;
    let mut budget = max_chars;
    let mut truncated = false;
    for sec in sections {
        let chars: Vec<char> = body[sec.start..sec.end].chars().collect();
        let lowered: Vec<char> = chars.iter().map(|&c| lower(c)).collect();
        let hits = find_all(&lowered, &needle);
        total_matches += hits.len();
        // Merge overlapping windows into one snippet.
        let mut windows: Vec<(usize, usize, usize)> = Vec::new(); // start, end, match count
        for h in hits {
            let s = h.saturating_sub(GREP_CONTEXT);
            let e = (h + needle.len() + GREP_CONTEXT).min(chars.len());
            match windows.last_mut() {
                Some(last) if s <= last.1 => {
                    last.1 = e;
                    last.2 += 1;
                }
                _ => windows.push((s, e, 1)),
            }
        }
        for (s, e, count) in windows {
            if snippets.len() >= GREP_MAX_SNIPPETS || budget == 0 {
                truncated = true;
                continue;
            }
            let e = e.min(s + budget);
            let mut text: String = chars[s..e].iter().collect();
            text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            if s > 0 {
                text.insert(0, '…');
            }
            if e < chars.len() {
                text.push('…');
            }
            budget = budget.saturating_sub(e - s);
            snippets.push(json!({
                "section": sec.index,
                "heading": sec.heading,
                "matches": count,
                "text": text,
            }));
        }
    }
    if total_matches == 0 {
        return Err(AppError::new(
            "no_match",
            format!("`{keyword}` does not occur in {handle}"),
            format!("Try a shorter keyword or a synonym, or skim the outline with `agentbox read {handle} --section 1`."),
        ));
    }
    let shown = snippets.len();
    let data = json!({
        "doc": handle,
        "title": title,
        "keyword": keyword,
        "total_matches": total_matches,
        "truncated": truncated,
        "snippets": snippets,
    });
    let out = Output::new(data);
    Ok(if truncated {
        out.hint(format!(
            "Showing {shown} snippets. Use a more specific keyword, or read a whole section with `agentbox read {handle} --section N`."
        ))
    } else {
        out.hint(format!(
            "Read the full context with `agentbox read {handle} --section N`."
        ))
    })
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn find_all(hay: &[char], needle: &[char]) -> Vec<usize> {
    let mut out = Vec::new();
    if needle.len() > hay.len() {
        return out;
    }
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == *needle {
            out.push(i);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# Intro\nAcme makes widgets.\n\n# Pricing\nThe Pro plan costs $20 per month.\n\n# Team\nCEO is Jane Doe.\n";

    fn args() -> ReadArgs {
        ReadArgs {
            doc: "doc:1".into(),
            section: None,
            grep: None,
            max_chars: 4000,
            offset: 0,
        }
    }

    #[test]
    fn reads_section() {
        let a = ReadArgs {
            section: Some(2),
            ..args()
        };
        let out = read_body("doc:1", "T", DOC, &a).unwrap();
        assert_eq!(out.data["heading"], "Pricing");
        assert!(out.data["content"].as_str().unwrap().contains("$20"));
        assert_eq!(out.data["truncated"], false);
    }

    #[test]
    fn bad_section_has_hint() {
        let a = ReadArgs {
            section: Some(9),
            ..args()
        };
        let err = read_body("doc:1", "T", DOC, &a).unwrap_err();
        assert_eq!(err.code, "bad_section");
        assert!(err.hint.contains("between 1 and 3"));
    }

    #[test]
    fn truncates_and_continues_with_offset() {
        let body = "x".repeat(250);
        let a = ReadArgs {
            max_chars: 100,
            ..args()
        };
        let out = read_body("doc:1", "T", &body, &a).unwrap();
        assert_eq!(out.data["truncated"], true);
        assert_eq!(out.data["next_offset"], 100);
        assert!(out.hint.unwrap().contains("--offset 100"));
        let a2 = ReadArgs {
            max_chars: 100,
            offset: 200,
            ..args()
        };
        let out2 = read_body("doc:1", "T", &body, &a2).unwrap();
        assert_eq!(out2.data["chars"], 50);
        assert_eq!(out2.data["truncated"], false);
    }

    #[test]
    fn grep_is_case_insensitive_and_reports_section() {
        let a = ReadArgs {
            grep: Some("ceo".into()),
            ..args()
        };
        let out = read_body("doc:1", "T", DOC, &a).unwrap();
        assert_eq!(out.data["total_matches"], 1);
        assert_eq!(out.data["snippets"][0]["section"], 3);
        assert!(out.data["snippets"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Jane Doe"));
    }

    #[test]
    fn grep_handles_multibyte_and_no_match() {
        let body = "# 價格\n專業版每月 20 美元。\n";
        let a = ReadArgs {
            grep: Some("每月".into()),
            ..args()
        };
        let out = read_body("doc:1", "T", body, &a).unwrap();
        assert_eq!(out.data["total_matches"], 1);
        let a = ReadArgs {
            grep: Some("zzz".into()),
            ..args()
        };
        assert_eq!(
            read_body("doc:1", "T", body, &a).unwrap_err().code,
            "no_match"
        );
    }

    #[test]
    fn grep_merges_nearby_matches() {
        let body = "# A\nfoo bar foo baz foo\n";
        let a = ReadArgs {
            grep: Some("foo".into()),
            ..args()
        };
        let out = read_body("doc:1", "T", body, &a).unwrap();
        assert_eq!(out.data["total_matches"], 3);
        assert_eq!(out.data["snippets"].as_array().unwrap().len(), 1);
        assert_eq!(out.data["snippets"][0]["matches"], 3);
    }
}
