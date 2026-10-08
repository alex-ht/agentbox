//! Report templates: required document structure, loaded from TOML.

use crate::envelope::{AppError, CmdResult, Output};
use crate::state::Store;
use serde::Deserialize;
use serde_json::json;

/// Built-in templates embedded in the binary: (name, TOML source).
pub const BUILTINS: &[(&str, &str)] = &[
    ("brief", include_str!("../../templates/brief.toml")),
    ("compare", include_str!("../../templates/compare.toml")),
    ("top-n", include_str!("../../templates/top-n.toml")),
    (
        "exec-lookup",
        include_str!("../../templates/exec-lookup.toml"),
    ),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CitationStyle {
    /// `[text](https://...)`
    InlineLink,
    /// `[^1]` with `[^1]: https://...` definitions
    Footnote,
    /// `[1]` with a numbered source list
    Numbered,
}

impl CitationStyle {
    pub fn name(self) -> &'static str {
        match self {
            CitationStyle::InlineLink => "inline-link",
            CitationStyle::Footnote => "footnote",
            CitationStyle::Numbered => "numbered",
        }
    }

    pub fn example(self) -> &'static str {
        match self {
            CitationStyle::InlineLink => "([example.com](https://example.com/page))",
            CitationStyle::Footnote => {
                "[^1] plus a line `[^1]: https://example.com/page` in Sources"
            }
            CitationStyle::Numbered => "[1] plus `1. [Title](https://example.com/page)` in Sources",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Template {
    pub name: String,
    pub description: String,
    /// Level of the single title heading on the first line; 0 = no title.
    pub title_level: u8,
    pub min_words: Option<usize>,
    pub max_words: Option<usize>,
    pub citation_style: CitationStyle,
    pub require_sources_section: bool,
    pub sources_heading: String,
    pub sources_level: u8,
    pub sources_from_notes: bool,
    pub min_sources: usize,
    pub forbid: Vec<String>,
    #[serde(rename = "section")]
    pub sections: Vec<SectionSpec>,
}

impl Default for Template {
    fn default() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            title_level: 1,
            min_words: None,
            max_words: None,
            citation_style: CitationStyle::InlineLink,
            require_sources_section: false,
            sources_heading: "Sources".into(),
            sources_level: 2,
            sources_from_notes: false,
            min_sources: 0,
            forbid: Vec::new(),
            sections: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SectionSpec {
    /// Literal heading text, or a pattern with `{n}` for numbered items,
    /// e.g. `{n}. {title}` (anything in `{...}` after `{n}` is free text).
    pub heading: String,
    /// Exact heading level (1-6).
    pub level: u8,
    pub required: bool,
    /// For numbered sections: allowed item count.
    pub repeat_min: Option<usize>,
    pub repeat_max: Option<usize>,
    pub min_words: Option<usize>,
    pub max_words: Option<usize>,
    pub require_citation: bool,
    pub must_contain: Vec<String>,
    /// Section must contain a Markdown table...
    pub table: bool,
    /// ...whose header includes these columns.
    pub columns: Vec<String>,
    /// Other accepted heading texts.
    pub aliases: Vec<String>,
    /// Guidance shown in TODO placeholders.
    pub hint: String,
    /// Note tags that belong in this section.
    pub keywords: Vec<String>,
}

impl Default for SectionSpec {
    fn default() -> Self {
        Self {
            heading: String::new(),
            level: 2,
            required: true,
            repeat_min: None,
            repeat_max: None,
            min_words: None,
            max_words: None,
            require_citation: false,
            must_contain: Vec::new(),
            table: false,
            columns: Vec::new(),
            aliases: Vec::new(),
            hint: String::new(),
            keywords: Vec::new(),
        }
    }
}

/// Parsed `{n}` heading pattern: `prefix` + number + `sep` + free text.
#[derive(Debug, Clone, PartialEq)]
pub struct NumPattern {
    pub prefix: String,
    pub sep: String,
    pub has_title: bool,
}

impl NumPattern {
    pub fn render(&self, n: usize, title: &str) -> String {
        let mut s = format!("{}{n}{}", self.prefix, self.sep);
        if self.has_title {
            s.push_str(title);
        }
        s.trim_end().to_string()
    }

    /// Strict match; returns (number, title text).
    pub fn matches(&self, text: &str) -> Option<(usize, String)> {
        let rest = strip_prefix_ci(text.trim(), &self.prefix)?;
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let n = digits.parse::<usize>().ok()?;
        let rest = &rest[digits.len()..];
        let title = if self.sep.trim().is_empty() {
            if !self.sep.is_empty() && !rest.starts_with(char::is_whitespace) && !rest.is_empty() {
                return None;
            }
            rest.trim()
        } else {
            rest.strip_prefix(self.sep.trim_end())?.trim()
        };
        if self.has_title && title.is_empty() {
            return None;
        }
        Some((n, title.to_string()))
    }

    /// Lenient match for near-misses like `1) Foo`, `1: Foo`, `1.Foo`.
    pub fn loose(&self, text: &str) -> Option<(usize, String)> {
        let t = text.trim();
        let rest = strip_prefix_ci(t, &self.prefix).unwrap_or(t);
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() || digits.len() > 2 {
            return None;
        }
        let n = digits.parse::<usize>().ok()?;
        let after = &rest[digits.len()..];
        let first = after.chars().next();
        if !matches!(
            first,
            Some('.' | ')' | ':' | '、' | '：' | '-' | '–' | '．')
        ) {
            return None;
        }
        let title = after
            .trim_start_matches(['.', ')', ':', '、', '：', '-', '–', '．'])
            .trim()
            .to_string();
        Some((n, title))
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix.is_empty() {
        return Some(s);
    }
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

impl SectionSpec {
    pub fn numbered(&self) -> Option<NumPattern> {
        let pos = self.heading.find("{n}")?;
        let prefix = self.heading[..pos].to_string();
        let after = &self.heading[pos + 3..];
        let (sep, has_title) = match after.find('{') {
            Some(i) => (after[..i].to_string(), true),
            None => (after.to_string(), false),
        };
        Some(NumPattern {
            prefix,
            sep,
            has_title,
        })
    }

    /// Does `text` name this (literal) section?
    pub fn names(&self, text: &str) -> bool {
        let t = text.trim();
        t.eq_ignore_ascii_case(self.heading.trim())
            || self
                .aliases
                .iter()
                .any(|a| t.eq_ignore_ascii_case(a.trim()))
    }

    pub fn label(&self) -> String {
        format!("{} {}", "#".repeat(self.level as usize), self.heading)
    }
}

/// Template-time overrides from the CLI.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub n: Option<usize>,
    pub columns: Option<String>,
}

impl Template {
    pub fn parse(src: &str, origin: &str) -> Result<Self, AppError> {
        let t: Template = toml::from_str(src).map_err(|e| {
            AppError::new(
                "bad_template",
                format!("template {origin} is invalid: {}", e.message()),
                "Compare with a built-in: `agentbox report template show brief`. Field names are case-sensitive.",
            )
        })?;
        t.validate(origin)?;
        Ok(t)
    }

    fn validate(&self, origin: &str) -> Result<(), AppError> {
        let bad = |msg: String| {
            AppError::new(
                "bad_template",
                format!("template {origin}: {msg}"),
                "Fix the template TOML; see `agentbox report template show top-n` for an example.",
            )
        };
        if !(0..=6).contains(&self.title_level) {
            return Err(bad("title_level must be 0-6".into()));
        }
        if !(1..=6).contains(&self.sources_level) {
            return Err(bad("sources_level must be 1-6".into()));
        }
        if self.require_sources_section && self.sources_heading.trim().is_empty() {
            return Err(bad("sources_heading is empty".into()));
        }
        if self.sections.is_empty() {
            return Err(bad("needs at least one [[section]]".into()));
        }
        for s in &self.sections {
            if s.heading.trim().is_empty() {
                return Err(bad("a section has an empty heading".into()));
            }
            if !(1..=6).contains(&s.level) {
                return Err(bad(format!("section `{}`: level must be 1-6", s.heading)));
            }
            if (s.repeat_min.is_some() || s.repeat_max.is_some()) && s.numbered().is_none() {
                return Err(bad(format!(
                    "section `{}`: repeat_min/max need a `{{n}}` heading",
                    s.heading
                )));
            }
            if let (Some(a), Some(b)) = (s.repeat_min, s.repeat_max) {
                if a > b {
                    return Err(bad(format!(
                        "section `{}`: repeat_min > repeat_max",
                        s.heading
                    )));
                }
            }
            if !s.columns.is_empty() && !s.table {
                return Err(bad(format!(
                    "section `{}`: columns need table = true",
                    s.heading
                )));
            }
        }
        Ok(())
    }

    /// Apply `--n` (numbered sections) and `--columns` (table sections).
    pub fn apply(mut self, o: &Overrides) -> Result<Self, AppError> {
        if let Some(n) = o.n {
            if n == 0 || n > 50 {
                return Err(AppError::new(
                    "bad_args",
                    "--n must be between 1 and 50",
                    "Example: `--n 5`.",
                ));
            }
            let mut any = false;
            for s in self.sections.iter_mut().filter(|s| s.numbered().is_some()) {
                s.repeat_min = Some(n);
                s.repeat_max = Some(n);
                any = true;
            }
            if !any {
                return Err(AppError::new(
                    "bad_args",
                    format!(
                        "template `{}` has no numbered `{{n}}` section, so --n does not apply",
                        self.name
                    ),
                    "Drop --n, or use `--template top-n`.",
                ));
            }
        }
        if let Some(cols) = o.columns.as_deref() {
            let cols: Vec<String> = cols
                .split(',')
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect();
            let mut any = false;
            for s in self.sections.iter_mut().filter(|s| s.table) {
                s.columns = cols.clone();
                any = true;
            }
            if !any {
                return Err(AppError::new(
                    "bad_args",
                    format!(
                        "template `{}` has no table section, so --columns does not apply",
                        self.name
                    ),
                    "Drop --columns, or use `--template compare`.",
                ));
            }
        }
        Ok(self)
    }
}

/// Resolve `--template` as a file path, a user template name
/// (`$AGENTBOX_HOME/templates/<name>.toml`) or a built-in name.
/// Returns (template, raw TOML, origin description).
pub fn load(store: &Store, spec: &str) -> Result<(Template, String, String), AppError> {
    let spec = spec.trim();
    let looks_like_path = spec.contains('/') || spec.contains('\\') || spec.ends_with(".toml");
    if looks_like_path {
        let src = std::fs::read_to_string(spec)
            .map_err(|e| AppError::io(&format!("read template {spec}"), e))?;
        return Ok((Template::parse(&src, spec)?, src, format!("file {spec}")));
    }
    let user = store.root().join("templates").join(format!("{spec}.toml"));
    if user.is_file() {
        let src = std::fs::read_to_string(&user).map_err(|e| AppError::io("read template", e))?;
        let origin = format!("user {}", user.display());
        return Ok((Template::parse(&src, &origin)?, src, origin));
    }
    if let Some((_, src)) = BUILTINS.iter().find(|(n, _)| *n == spec) {
        return Ok((
            Template::parse(src, spec)?,
            src.to_string(),
            "built-in".into(),
        ));
    }
    Err(AppError::new(
        "unknown_template",
        format!("no template named `{spec}`"),
        format!(
            "Built-ins: {}. List all with `agentbox report templates`, or pass a path like `--template ./my.toml`.",
            BUILTINS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
        ),
    ))
}

fn user_templates(store: &Store) -> Vec<(String, std::path::PathBuf)> {
    let dir = store.root().join("templates");
    let mut out: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let p = e.path();
                    let name = p.file_name()?.to_str()?.strip_suffix(".toml")?.to_string();
                    Some((name, p))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

pub fn list(store: &Store) -> CmdResult {
    let mut items = Vec::new();
    for (name, src) in BUILTINS {
        let t = Template::parse(src, name)?;
        items.push(json!({ "name": name, "source": "built-in", "description": t.description }));
    }
    for (name, path) in user_templates(store) {
        let desc = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| Template::parse(&s, &name).ok())
            .map(|t| t.description)
            .unwrap_or_else(|| "(invalid template)".into());
        items.push(
            json!({ "name": name, "source": path.display().to_string(), "description": desc }),
        );
    }
    let dir = store.root().join("templates");
    Ok(Output::new(json!({ "templates": items, "user_dir": dir.display().to_string() })).hint(format!(
        "Copy one with `agentbox report template show top-n`, edit it, and save it as {}{}NAME.toml (or pass --template ./file.toml).",
        dir.display(),
        std::path::MAIN_SEPARATOR
    )))
}

pub fn show(store: &Store, name: &str) -> CmdResult {
    let (t, src, origin) = load(store, name)?;
    Ok(
        Output::new(json!({ "name": t.name, "origin": origin, "toml": src })).hint(
            "Save the `toml` text to a file, edit it, then use `--template ./that-file.toml`.",
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_parse() {
        for (name, src) in BUILTINS {
            let t = Template::parse(src, name).unwrap();
            assert_eq!(&t.name, name);
            assert!(!t.description.is_empty());
            assert!(t.require_sources_section);
        }
    }

    #[test]
    fn numbered_pattern() {
        let s = SectionSpec {
            heading: "{n}. {title}".into(),
            ..SectionSpec::default()
        };
        let p = s.numbered().unwrap();
        assert_eq!(p.render(2, "Foo"), "2. Foo");
        assert_eq!(p.matches("2. Foo bar"), Some((2, "Foo bar".into())));
        assert_eq!(p.matches("2) Foo"), None);
        assert_eq!(p.matches("2."), None, "title required");
        assert_eq!(p.loose("2) Foo"), Some((2, "Foo".into())));
        assert_eq!(p.loose("2024 results"), None);
        let s = SectionSpec {
            heading: "Item {n}: {title}".into(),
            ..SectionSpec::default()
        };
        let p = s.numbered().unwrap();
        assert_eq!(p.matches("item 3: Bar"), Some((3, "Bar".into())));
        assert_eq!(p.render(1, "X"), "Item 1: X");
        let s = SectionSpec {
            heading: "Step {n}".into(),
            ..SectionSpec::default()
        };
        assert_eq!(
            s.numbered().unwrap().matches("Step 4"),
            Some((4, String::new()))
        );
    }

    #[test]
    fn rejects_bad_templates_with_hints() {
        let e = Template::parse(
            "name = \"x\"\n[[section]]\nheading = \"A\"\nlevle = 2\n",
            "t",
        )
        .unwrap_err();
        assert_eq!(e.code, "bad_template");
        assert!(e.message.contains("levle"), "{}", e.message);
        let e = Template::parse(
            "name = \"x\"\n[[section]]\nheading = \"A\"\nrepeat_min = 2\n",
            "t",
        )
        .unwrap_err();
        assert!(e.message.contains("{n}"));
        let e = Template::parse("name = \"x\"\n", "t").unwrap_err();
        assert!(e.message.contains("at least one"));
        let e = Template::parse(
            "name = \"x\"\ncitation_style = \"apa\"\n[[section]]\nheading = \"A\"\n",
            "t",
        )
        .unwrap_err();
        assert_eq!(e.code, "bad_template");
    }

    #[test]
    fn overrides_apply() {
        let (_, src) = BUILTINS.iter().find(|(n, _)| *n == "top-n").unwrap();
        let t = Template::parse(src, "top-n")
            .unwrap()
            .apply(&Overrides {
                n: Some(5),
                columns: None,
            })
            .unwrap();
        assert_eq!(t.sections[0].repeat_min, Some(5));
        let e = Template::parse(src, "top-n")
            .unwrap()
            .apply(&Overrides {
                n: None,
                columns: Some("A,B".into()),
            })
            .unwrap_err();
        assert_eq!(e.code, "bad_args");
        let (_, src) = BUILTINS.iter().find(|(n, _)| *n == "compare").unwrap();
        let t = Template::parse(src, "compare")
            .unwrap()
            .apply(&Overrides {
                n: None,
                columns: Some("Tool, Cost".into()),
            })
            .unwrap();
        assert_eq!(t.sections[1].columns, vec!["Tool", "Cost"]);
    }

    #[test]
    fn load_resolves_user_dir_then_builtin() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        std::fs::create_dir_all(tmp.path().join("templates")).unwrap();
        std::fs::write(
            tmp.path().join("templates/brief.toml"),
            "name = \"mine\"\n[[section]]\nheading = \"X\"\n",
        )
        .unwrap();
        assert_eq!(load(&store, "brief").unwrap().0.name, "mine");
        assert_eq!(load(&store, "top-n").unwrap().0.name, "top-n");
        assert_eq!(load(&store, "nope").unwrap_err().code, "unknown_template");
        let listed = list(&store).unwrap();
        assert_eq!(
            listed.data["templates"].as_array().unwrap().len(),
            BUILTINS.len() + 1
        );
        assert!(show(&store, "compare").unwrap().data["toml"]
            .as_str()
            .unwrap()
            .contains("[[section]]"));
    }
}
