//! A careful line-based Markdown reader for report checking: ATX headings
//! (outside code fences), section spans, tables, citations, URLs and
//! word counts. Line numbers are 1-based.

#[derive(Debug, Clone, PartialEq)]
pub struct Heading {
    pub line: usize,
    pub level: u8,
    pub text: String,
    pub raw: String,
}

#[derive(Debug, Clone)]
pub struct Doc {
    pub lines: Vec<String>,
    /// Whether each line is inside (or delimits) a fenced code block.
    pub in_code: Vec<bool>,
    pub headings: Vec<Heading>,
    /// Lines like `##Foo` (missing space after `#`).
    pub nospace: Vec<usize>,
    /// Setext heading underline lines (`===` / `---` under text).
    pub setext: Vec<usize>,
}

impl Doc {
    pub fn parse(text: &str) -> Self {
        let lines: Vec<String> = text
            .lines()
            .map(|l| l.trim_end_matches('\r').to_string())
            .collect();
        let mut in_code = vec![false; lines.len()];
        let mut headings = Vec::new();
        let mut nospace = Vec::new();
        let mut setext = Vec::new();
        let mut fence: Option<(char, usize)> = None;
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim_start();
            let indent = line.len() - t.len();
            if let Some((ch, len)) = fence {
                in_code[i] = true;
                let run = t.chars().take_while(|&c| c == ch).count();
                if run >= len && t[run * ch.len_utf8()..].trim().is_empty() {
                    fence = None;
                }
                continue;
            }
            if indent <= 3 {
                let ch = t.chars().next().unwrap_or(' ');
                if ch == '`' || ch == '~' {
                    let run = t.chars().take_while(|&c| c == ch).count();
                    if run >= 3 {
                        fence = Some((ch, run));
                        in_code[i] = true;
                        continue;
                    }
                }
            }
            if indent >= 4 {
                continue; // indented code
            }
            if let Some(h) = atx(t) {
                headings.push(Heading {
                    line: i + 1,
                    level: h.0,
                    text: h.1,
                    raw: line.clone(),
                });
            } else if t.starts_with('#') {
                let hashes = t.chars().take_while(|&c| c == '#').count();
                if (1..=6).contains(&hashes)
                    && t[hashes..].starts_with(|c: char| c.is_alphanumeric())
                {
                    nospace.push(i + 1);
                }
            } else if i > 0 && is_setext_underline(t) {
                let prev = lines[i - 1].trim();
                let prev_is_text = !prev.is_empty()
                    && !in_code[i - 1]
                    && !prev.starts_with(['#', '|', '-', '*', '>', '+'])
                    && !prev.starts_with("<!--")
                    && !prev.chars().next().is_some_and(|c| c.is_ascii_digit());
                if prev_is_text {
                    setext.push(i + 1);
                }
            }
        }
        Self {
            lines,
            in_code,
            headings,
            nospace,
            setext,
        }
    }

    /// Body line range (0-based, exclusive end) of the heading at `idx`:
    /// up to the next heading of the same or higher level.
    pub fn section_range(&self, idx: usize) -> (usize, usize) {
        let h = &self.headings[idx];
        let end = self.headings[idx + 1..]
            .iter()
            .find(|n| n.level <= h.level)
            .map(|n| n.line - 1)
            .unwrap_or(self.lines.len());
        (h.line, end)
    }

    pub fn text_of(&self, range: (usize, usize)) -> String {
        self.lines[range.0..range.1].join("\n")
    }
}

/// Parse an ATX heading (`## Title`), returning (level, text).
fn atx(t: &str) -> Option<(u8, String)> {
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &t[hashes..];
    if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return None;
    }
    let text = rest.trim();
    // Strip an optional closing sequence of #'s.
    let text = match text.trim_end_matches('#') {
        stripped if stripped.ends_with(' ') || stripped.is_empty() => stripped.trim_end(),
        _ => text,
    };
    Some((hashes as u8, text.to_string()))
}

fn is_setext_underline(t: &str) -> bool {
    let t = t.trim();
    t.len() >= 3 && (t.chars().all(|c| c == '=') || t.chars().all(|c| c == '-'))
}

/// Remove HTML comments (possibly multi-line).
pub fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<!--") {
        out.push_str(&rest[..start]);
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xAC00..=0xD7AF | 0x20000..=0x2FA1F)
}

/// Word count: latin/digit runs count once, each CJK character counts once.
/// Comments, link targets, bare URLs and Markdown punctuation are ignored.
pub fn count_words(s: &str) -> usize {
    let s = strip_comments(s);
    let s = strip_link_targets(&s);
    let mut count = 0;
    for token in s.split_whitespace() {
        if token.starts_with("http://") || token.starts_with("https://") {
            continue;
        }
        let mut in_word = false;
        for c in token.chars() {
            if is_cjk(c) {
                count += 1;
                in_word = false;
            } else if c.is_alphanumeric() {
                if !in_word {
                    count += 1;
                    in_word = true;
                }
            } else if !(in_word && matches!(c, '\'' | '’' | '-' | '.' | ',')) {
                in_word = false;
            }
        }
    }
    count
}

/// `[text](url)` -> `[text]`
fn strip_link_targets(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find("](") {
        out.push_str(&rest[..pos + 1]);
        let after = &rest[pos + 2..];
        match after.find(')') {
            Some(end)
                if !after[..end].contains(char::is_whitespace) || after[..end].starts_with('<') =>
            {
                rest = &after[end + 1..]
            }
            _ => rest = &rest[pos + 1..],
        }
    }
    out.push_str(rest);
    out
}

/// All http(s) URLs in `s` (inline links, autolinks, bare), in order.
pub fn urls(s: &str) -> Vec<String> {
    let s = strip_comments(s);
    let mut out = Vec::new();
    let mut i = 0;
    let bytes = s.as_bytes();
    while i < s.len() {
        let rest = &s[i..];
        let start = match (rest.find("http://"), rest.find("https://")) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => break,
        };
        let begin = i + start;
        let mut end = begin;
        while end < s.len() {
            let c = bytes[end];
            if c.is_ascii_whitespace() || matches!(c, b')' | b'>' | b'"' | b'<' | b']' | b'|') {
                break;
            }
            end += 1;
        }
        let url = s[begin..end].trim_end_matches(['.', ',', ';', ':', '!', '?', '\'']);
        if url.len() > "https://".len() {
            out.push(url.to_string());
        }
        i = end.max(begin + 1);
    }
    out
}

/// Citation markers found in `s`, by style.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Citations {
    pub inline: usize,
    pub footnote: usize,
    pub numbered: usize,
}

impl Citations {
    pub fn total(&self) -> usize {
        self.inline + self.footnote + self.numbered
    }
}

pub fn citations(s: &str) -> Citations {
    let s = strip_comments(s);
    let mut c = Citations::default();
    for line in s.lines() {
        let t = line.trim_start();
        // Definitions are sources, not citations.
        if t.starts_with("[^") && t.contains("]:") {
            continue;
        }
        c.inline += line.matches("](http").count()
            + line.matches("](<http").count()
            + line.matches("<http").count();
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '[' {
                let close = chars[i..].iter().position(|&ch| ch == ']').map(|p| i + p);
                if let Some(j) = close {
                    let inner: String = chars[i + 1..j].iter().collect();
                    let next = chars.get(j + 1).copied();
                    let prev = if i > 0 { Some(chars[i - 1]) } else { None };
                    if inner.starts_with('^') && inner.len() > 1 && next != Some(':') {
                        c.footnote += 1;
                    } else if !inner.is_empty()
                        && inner.split(',').all(|p| {
                            !p.trim().is_empty() && p.trim().chars().all(|ch| ch.is_ascii_digit())
                        })
                        && next != Some('(')
                        && next != Some(':')
                        && prev != Some(']')
                        && prev != Some('!')
                    {
                        c.numbered += 1;
                    }
                    i = j + 1;
                    continue;
                }
            }
            i += 1;
        }
    }
    c
}

/// A Markdown table found in a line range.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub line: usize,
    pub columns: Vec<String>,
    pub rows: usize,
}

pub fn find_table(doc: &Doc, range: (usize, usize)) -> Option<Table> {
    let is_row = |i: usize| !doc.in_code[i] && doc.lines[i].trim_start().starts_with('|');
    let mut i = range.0;
    while i + 1 < range.1 {
        if is_row(i) && is_row(i + 1) && is_separator(&doc.lines[i + 1]) {
            let columns = split_row(&doc.lines[i]);
            let mut j = i + 2;
            while j < range.1 && is_row(j) {
                j += 1;
            }
            return Some(Table {
                line: i + 1,
                columns,
                rows: j - (i + 2),
            });
        }
        i += 1;
    }
    None
}

fn is_separator(line: &str) -> bool {
    let cells = split_row(line);
    !cells.is_empty()
        && cells.iter().all(|c| {
            let c = c.trim();
            !c.is_empty() && c.trim_matches(':').chars().all(|ch| ch == '-') && c.contains('-')
        })
}

fn split_row(line: &str) -> Vec<String> {
    let t = line.trim().trim_start_matches('|').trim_end_matches('|');
    t.split('|')
        .map(|c| strip_comments(c).trim().to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headings_ignore_code_fences_and_flag_nospace_and_setext() {
        let md = "# Title\n\n```md\n## Not a heading\n```\n~~~~\n# also not\n~~~~\n##Bad\n## 1. Real ##\nSetext\n======\n    # indented code\n";
        let d = Doc::parse(md);
        let hs: Vec<(usize, u8, &str)> = d
            .headings
            .iter()
            .map(|h| (h.line, h.level, h.text.as_str()))
            .collect();
        assert_eq!(hs, vec![(1, 1, "Title"), (10, 2, "1. Real")]);
        assert_eq!(d.nospace, vec![9]);
        assert_eq!(d.setext, vec![12]);
        assert!(d.in_code[3]);
    }

    #[test]
    fn section_ranges_include_subsections() {
        let d = Doc::parse("# T\n## A\na\n### A1\nx\n## B\nb\n");
        assert_eq!(d.section_range(1), (2, 5));
        assert_eq!(d.text_of(d.section_range(1)), "a\n### A1\nx");
        assert_eq!(d.section_range(3), (6, 7));
    }

    #[test]
    fn word_counts() {
        assert_eq!(
            count_words("Hello, world! It's 3.5% of [the site](https://x.test/a)"),
            7
        );
        assert_eq!(
            count_words("<!-- TODO: lots of words here -->\nTwo words"),
            2
        );
        assert_eq!(count_words("台積電 2025 年營收"), 7);
        assert_eq!(count_words("see https://example.com/page now"), 2);
    }

    #[test]
    fn finds_urls_and_citations() {
        let s = "A ([ex](https://a.test/x)). B <https://b.test>. C https://c.test/p, done [1] [^2]\n[^2]: https://d.test\n![img](https://e.test/i.png)";
        assert_eq!(
            urls(s),
            vec![
                "https://a.test/x",
                "https://b.test",
                "https://c.test/p",
                "https://d.test",
                "https://e.test/i.png"
            ]
        );
        let c = citations(s);
        assert_eq!(c.inline, 3, "{c:?}");
        assert_eq!(c.numbered, 1);
        assert_eq!(c.footnote, 1);
        let c = citations("[link](/relative) and [3, 4] and [x][1]");
        assert_eq!(c.numbered, 1);
        assert_eq!(c.inline, 0);
    }

    #[test]
    fn finds_tables() {
        let d =
            Doc::parse("## C\n\n| Option | Price |\n|---|:---:|\n| A | $1 |\n| B | $2 |\n\ntext\n");
        let t = find_table(&d, (1, d.lines.len())).unwrap();
        assert_eq!(t.line, 3);
        assert_eq!(t.columns, vec!["Option", "Price"]);
        assert_eq!(t.rows, 2);
        let d = Doc::parse("| a |\n| b |\n");
        assert!(find_table(&d, (0, 2)).is_none());
    }
}
