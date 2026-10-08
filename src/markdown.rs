//! HTML -> Markdown conversion, title extraction and section splitting.

use serde::Serialize;

/// Tags dropped entirely during conversion (boilerplate, scripts, chrome).
const SKIP_TAGS: &[&str] = &[
    "head", "script", "style", "noscript", "template", "svg", "canvas", "iframe", "nav", "footer",
    "aside", "form", "button", "select",
];

/// Sections longer than this (in chars) are split into parts at paragraph breaks.
pub const MAX_SECTION_CHARS: usize = 6000;

/// Convert HTML into clean Markdown. Relative links are resolved against
/// `base_url` so the agent can fetch them directly.
pub fn html_to_markdown(html: &str, base_url: Option<&str>) -> String {
    let base = base_url.and_then(|u| reqwest::Url::parse(u).ok());
    let table_base = base.clone();
    let converter = htmd::HtmlToMarkdown::builder()
        .skip_tags(SKIP_TAGS.to_vec())
        .add_handler(vec!["a"], move |el: htmd::Element| {
            Some(anchor(&el, base.as_ref()))
        })
        .add_handler(vec!["table"], move |el: htmd::Element| {
            Some(match pipe_table(el.node, table_base.as_ref()) {
                Some(t) => format!("\n\n{t}\n\n"),
                None => el.content.to_string(),
            })
        })
        .build();
    let md = converter.convert(html).unwrap_or_default();
    tidy(&strip_images(&md))
}

/// Render a link as `[text](absolute-url)`, dropping title attributes,
/// in-page fragments and javascript: pseudo-links.
fn anchor(el: &htmd::Element, base: Option<&reqwest::Url>) -> String {
    let content = el.content;
    let text = content.trim();
    let href = el
        .attrs
        .iter()
        .find(|a| &a.name.local == "href")
        .map(|a| a.value.trim().to_string());
    let Some(href) =
        href.filter(|h| !h.is_empty() && !h.starts_with('#') && !h.starts_with("javascript:"))
    else {
        return content.to_string();
    };
    if text.is_empty() {
        return content.to_string();
    }
    let resolved = base
        .and_then(|b| b.join(&href).ok())
        .map(|u| u.to_string())
        .unwrap_or(href);
    let link = resolved
        .replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29");
    let lead = if content.starts_with(char::is_whitespace) {
        " "
    } else {
        ""
    };
    let trail = if content.ends_with(char::is_whitespace) {
        " "
    } else {
        ""
    };
    format!("{lead}[{text}]({link}){trail}")
}

/// Longest cell text accepted before a table is treated as page layout.
const MAX_CELL_CHARS: usize = 300;

/// Render an HTML `<table>` as a Markdown pipe table. Returns None for
/// layout tables (one column, or cells holding whole paragraphs), which are
/// better left as flowing text.
fn pipe_table(node: &markup5ever_rcdom::Handle, base: Option<&reqwest::Url>) -> Option<String> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    collect_rows(node, base, &mut rows, true);
    rows.retain(|r| r.iter().any(|c| !c.is_empty()));
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    if width < 2 || rows.len() < 2 {
        return None;
    }
    if rows
        .iter()
        .flatten()
        .any(|c| c.chars().count() > MAX_CELL_CHARS)
    {
        return None;
    }
    let line = |r: &[String]| {
        let mut cells: Vec<String> = r.iter().map(|c| c.replace('|', "\\|")).collect();
        cells.resize(width, String::new());
        format!("| {} |", cells.join(" | "))
    };
    let mut out = vec![line(&rows[0]), format!("|{}", " --- |".repeat(width))];
    out.extend(rows[1..].iter().map(|r| line(r)));
    Some(out.join("\n"))
}

fn collect_rows(
    node: &markup5ever_rcdom::Handle,
    base: Option<&reqwest::Url>,
    rows: &mut Vec<Vec<String>>,
    top: bool,
) {
    use markup5ever_rcdom::NodeData;
    let tag = match &node.data {
        NodeData::Element { name, .. } => name.local.to_string(),
        _ => String::new(),
    };
    if tag == "table" && !top {
        return; // nested tables are flattened into their cell's text
    }
    if tag == "tr" {
        let mut cells = Vec::new();
        for c in node.children.borrow().iter() {
            if matches!(crate::dom::tag(c).as_deref(), Some("td" | "th")) {
                let mut text = String::new();
                cell_text(c, base, &mut text);
                let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
                let span = crate::dom::attr(c, "colspan")
                    .and_then(|s| s.trim().parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, 20);
                cells.push(text);
                cells.extend(std::iter::repeat_n(String::new(), span - 1));
            }
        }
        rows.push(cells);
        return;
    }
    for c in node.children.borrow().iter() {
        collect_rows(c, base, rows, false);
    }
}

/// Inline text of a cell: links kept as `[text](url)`, everything else flat.
fn cell_text(node: &markup5ever_rcdom::Handle, base: Option<&reqwest::Url>, out: &mut String) {
    use markup5ever_rcdom::NodeData;
    match &node.data {
        NodeData::Text { contents } => out.push_str(&decode_entities(&contents.borrow())),
        NodeData::Element { name, .. } => {
            let tag = name.local.to_string();
            if SKIP_TAGS.contains(&tag.as_str()) || tag == "img" {
                return;
            }
            if tag == "br" {
                out.push(' ');
                return;
            }
            let block = matches!(
                tag.as_str(),
                "p" | "div" | "li" | "ul" | "ol" | "table" | "tr"
            );
            if block {
                out.push(' ');
            }
            if tag == "a" {
                let mut inner = String::new();
                for c in node.children.borrow().iter() {
                    cell_text(c, base, &mut inner);
                }
                let inner = inner.split_whitespace().collect::<Vec<_>>().join(" ");
                let href = crate::dom::attr(node, "href").filter(|h| {
                    let h = h.trim();
                    !h.is_empty() && !h.starts_with('#') && !h.starts_with("javascript:")
                });
                match href {
                    Some(h) if !inner.is_empty() => {
                        let url = base
                            .and_then(|b| b.join(h.trim()).ok())
                            .map(|u| u.to_string())
                            .unwrap_or_else(|| h.trim().to_string());
                        let url = url
                            .replace(' ', "%20")
                            .replace('(', "%28")
                            .replace(')', "%29");
                        out.push_str(&format!(" [{inner}]({url}) "));
                    }
                    _ => out.push_str(&inner),
                }
            } else {
                for c in node.children.borrow().iter() {
                    cell_text(c, base, out);
                }
            }
            if block {
                out.push(' ');
            }
        }
        _ => {}
    }
}

/// Remove `![alt](src)` image syntax; images are noise for a text-only agent.
pub fn strip_images(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut rest = md;
    while let Some(pos) = rest.find("![") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos..];
        // Find the matching "](" then the closing ")".
        let skipped = after
            .find("](")
            .filter(|&mid| !after[..mid].contains('\n'))
            .and_then(|mid| after[mid..].find(')').map(|end| mid + end + 1));
        match skipped {
            Some(len) => rest = &after[len..],
            None => {
                out.push_str("![");
                rest = &after[2..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Trim trailing spaces and collapse runs of blank lines.
pub fn tidy(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let mut blank = 0;
    for line in md.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
            out.push('\n');
        } else {
            blank = 0;
            out.push_str(line);
            out.push('\n');
        }
    }
    out.trim().to_string() + "\n"
}

/// Extract `<title>` text from raw HTML.
pub fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase(); // same byte length as `html`
    let open = lower.find("<title")?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title")?;
    let title = decode_entities(&html[start..end]);
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

/// Decode the handful of HTML entities that commonly appear in titles.
pub fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&apos;", "'")
        .replace("&ndash;", "–")
        .replace("&mdash;", "—")
        .replace("&amp;", "&")
}

/// One addressable section of a document. `start`/`end` are byte offsets.
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub index: usize,
    pub heading: String,
    pub level: usize,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct OutlineEntry {
    pub section: usize,
    pub heading: String,
    pub chars: usize,
}

fn heading_of(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|&c| c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.starts_with(' ') {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim();
    Some((
        level,
        if text.is_empty() {
            "(untitled)".into()
        } else {
            text.to_string()
        },
    ))
}

/// Split Markdown into sections at ATX headings (ignoring fenced code),
/// then split oversized sections into parts at blank lines.
pub fn split_sections(md: &str) -> Vec<Section> {
    let mut raw: Vec<(String, usize, usize)> = Vec::new(); // heading, level, start
    let mut in_fence = false;
    let mut offset = 0;
    for line in md.split_inclusive('\n') {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
        } else if !in_fence {
            if let Some((level, text)) = heading_of(line) {
                raw.push((text, level, offset));
            }
        }
        offset += line.len();
    }
    let mut bounds: Vec<(String, usize, usize, usize)> = Vec::new();
    let first_start = raw.first().map(|r| r.2).unwrap_or(md.len());
    if !md[..first_start].trim().is_empty() {
        bounds.push(("(top)".into(), 0, 0, first_start));
    }
    for (i, (h, lvl, start)) in raw.iter().enumerate() {
        let end = raw.get(i + 1).map(|r| r.2).unwrap_or(md.len());
        bounds.push((h.clone(), *lvl, *start, end));
    }
    let mut out = Vec::new();
    for (heading, level, start, end) in bounds {
        let parts = split_long(md, start, end);
        let n = parts.len();
        for (k, (s, e)) in parts.into_iter().enumerate() {
            let heading = if n > 1 {
                format!("{heading} (part {}/{n})", k + 1)
            } else {
                heading.clone()
            };
            out.push(Section {
                index: out.len() + 1,
                heading,
                level,
                start: s,
                end: e,
            });
        }
    }
    out
}

/// Split byte range [start, end) of `md` into chunks of at most
/// MAX_SECTION_CHARS chars, preferring paragraph boundaries.
fn split_long(md: &str, start: usize, end: usize) -> Vec<(usize, usize)> {
    let text = &md[start..end];
    if text.chars().count() <= MAX_SECTION_CHARS {
        return vec![(start, end)];
    }
    let mut parts = Vec::new();
    let mut part_start = 0; // relative byte offset
    let mut part_chars = 0;
    let mut last_break: Option<usize> = None; // relative byte offset after "\n\n"
    let mut prev_nl = false;
    for (i, c) in text.char_indices() {
        part_chars += 1;
        if c == '\n' {
            if prev_nl {
                last_break = Some(i + 1);
            }
            prev_nl = true;
        } else {
            prev_nl = false;
        }
        if part_chars >= MAX_SECTION_CHARS {
            let cut = match last_break {
                Some(b) if b > part_start => b,
                _ => i + c.len_utf8(),
            };
            parts.push((start + part_start, start + cut));
            part_start = cut;
            part_chars = text[cut..i + c.len_utf8()].chars().count();
            last_break = None;
        }
    }
    if part_start < text.len() {
        parts.push((start + part_start, end));
    }
    parts
}

pub fn outline(md: &str, sections: &[Section]) -> Vec<OutlineEntry> {
    sections
        .iter()
        .map(|s| OutlineEntry {
            section: s.index,
            heading: s.heading.clone(),
            chars: md[s.start..s.end].chars().count(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"<html><head><title> Acme &amp; Co | Pricing </title>
        <style>body{color:red}</style><script>var x=1;</script></head>
        <body><nav><a href="/">Home</a><a href="/about">About</a></nav>
        <h1>Pricing</h1><p>Plans start at <b>$10</b>.</p>
        <img src="logo.png" alt="logo">
        <h2>Enterprise</h2><p>Contact <a href="https://acme.test/sales">sales</a>.</p>
        <footer>Copyright 2026</footer></body></html>"#;

    #[test]
    fn converts_and_strips_boilerplate() {
        let md = html_to_markdown(PAGE, None);
        assert!(md.contains("# Pricing"), "{md}");
        assert!(md.contains("**$10**"), "{md}");
        assert!(md.contains("[sales](https://acme.test/sales)"), "{md}");
        for junk in ["color:red", "var x", "Home", "Copyright", "logo.png"] {
            assert!(!md.contains(junk), "found {junk:?} in {md}");
        }
    }

    #[test]
    fn resolves_relative_links_and_drops_titles() {
        let html = r##"<p>See <a href="/docs/a b(1)" title="Docs">the docs</a>, <a href="#top">top</a>
            and <a href="javascript:void(0)">menu</a>.</p>"##;
        let md = html_to_markdown(html, Some("https://acme.test/en/page"));
        assert!(
            md.contains("See [the docs](https://acme.test/docs/a%20b%281%29),"),
            "{md}"
        );
        assert!(md.contains("top and menu."), "{md}");
        assert!(!md.contains("Docs\""), "{md}");
    }

    #[test]
    fn html_tables_become_pipe_tables() {
        let html = r#"<h2>Plans</h2><table><thead><tr><th>Plan</th><th>Price</th></tr></thead>
            <tbody><tr><td><a href="/pro">Pro</a></td><td><b>$19</b> per<br>user</td></tr>
            <tr><td colspan="2">Billed | yearly</td></tr></tbody></table><p>After.</p>"#;
        let md = html_to_markdown(html, Some("https://acme.test/pricing"));
        assert!(md.contains("| Plan | Price |\n| --- | --- |\n"), "{md}");
        assert!(
            md.contains("| [Pro](https://acme.test/pro) | $19 per user |"),
            "{md}"
        );
        assert!(md.contains("| Billed \\| yearly |  |"), "{md}");
        assert!(md.contains("After."), "{md}");
        // Layout tables (one column / paragraph-sized cells) stay as text.
        let layout = format!(
            "<table><tr><td>{}</td><td>x</td></tr><tr><td>a</td><td>b</td></tr></table>",
            "word ".repeat(80)
        );
        let md = html_to_markdown(&layout, None);
        assert!(!md.contains("| --- |"), "{md}");
        let one = html_to_markdown(
            "<table><tr><td>only</td></tr><tr><td>col</td></tr></table>",
            None,
        );
        assert!(!one.contains('|'), "{one}");
    }

    #[test]
    fn extracts_title() {
        assert_eq!(extract_title(PAGE).as_deref(), Some("Acme & Co | Pricing"));
        assert_eq!(extract_title("<p>no title</p>"), None);
    }

    #[test]
    fn strips_images_but_keeps_links() {
        let s = strip_images("a ![x](y.png) b [l](u) c ![unclosed");
        assert_eq!(s, "a  b [l](u) c ![unclosed");
    }

    #[test]
    fn splits_sections_with_preamble_and_ignores_code_fences() {
        let md = "intro text\n\n# One\nbody1\n```\n# not a heading\n```\n## Two\nbody2\n";
        let secs = split_sections(md);
        let heads: Vec<&str> = secs.iter().map(|s| s.heading.as_str()).collect();
        assert_eq!(heads, vec!["(top)", "One", "Two"]);
        assert!(md[secs[1].start..secs[1].end].contains("# not a heading"));
        assert_eq!(secs[2].level, 2);
        let ol = outline(md, &secs);
        assert_eq!(ol[2].section, 3);
    }

    #[test]
    fn long_sections_are_split_into_parts() {
        let para = "word ".repeat(200) + "\n\n"; // ~1002 chars
        let md = format!("# Big\n{}", para.repeat(15));
        let secs = split_sections(&md);
        assert!(secs.len() >= 3, "got {} sections", secs.len());
        assert!(secs[0].heading.starts_with("Big (part 1/"));
        let total: usize = secs.iter().map(|s| s.end - s.start).sum();
        assert_eq!(total, md.len());
        for s in &secs {
            assert!(md[s.start..s.end].chars().count() <= MAX_SECTION_CHARS);
        }
    }

    #[test]
    fn long_sections_split_on_multibyte_text() {
        let md = "# 長文\n".to_string() + &"中文字".repeat(5000);
        let secs = split_sections(&md);
        assert!(secs.len() >= 3);
        let joined: String = secs.iter().map(|s| &md[s.start..s.end]).collect();
        assert_eq!(joined, md);
    }
}
