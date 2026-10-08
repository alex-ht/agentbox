//! `skill install`: copy the bundled Agent Skill (agentskills.io format) into
//! a skills directory so agents learn how to drive agentbox.

use crate::envelope::{AppError, CmdResult, Output};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

/// Skill folder name; must match `name:` in SKILL.md.
pub const NAME: &str = "agentbox";

/// Every file of `skills/agentbox/`, relative to the skill root.
pub const FILES: &[(&str, &str)] = &[
    ("SKILL.md", include_str!("../skills/agentbox/SKILL.md")),
    (
        "references/research.md",
        include_str!("../skills/agentbox/references/research.md"),
    ),
    (
        "references/data.md",
        include_str!("../skills/agentbox/references/data.md"),
    ),
    (
        "references/reports.md",
        include_str!("../skills/agentbox/references/reports.md"),
    ),
    (
        "references/finance-markets.md",
        include_str!("../skills/agentbox/references/finance-markets.md"),
    ),
    (
        "references/files.md",
        include_str!("../skills/agentbox/references/files.md"),
    ),
    (
        "references/task-playbooks.md",
        include_str!("../skills/agentbox/references/task-playbooks.md"),
    ),
    (
        "assets/vendor-shortlist.toml",
        include_str!("../skills/agentbox/assets/vendor-shortlist.toml"),
    ),
];

fn default_dir() -> PathBuf {
    crate::state::user_home().join(".agents").join("skills")
}

/// Write the skill to `<dir>/agentbox/`. Preview unless `apply`.
pub fn install(dir: Option<&str>, apply: bool) -> CmdResult {
    let root = dir.map(PathBuf::from).unwrap_or_else(default_dir);
    let target = root.join(NAME);
    let mut files = Vec::new();
    let mut changed = 0;
    for (rel, content) in FILES {
        let path = target.join(rel);
        let status = match fs::read_to_string(&path) {
            Ok(old) if old == *content => "unchanged",
            Ok(_) => "update",
            Err(_) => "create",
        };
        if status != "unchanged" {
            changed += 1;
            if apply {
                write(&path, content)?;
            }
        }
        files.push(json!({ "file": rel, "status": status, "bytes": content.len() }));
    }
    let target_s = target.display().to_string();
    let hint = if changed == 0 {
        format!("The skill in {target_s} is up to date. Make sure `agentbox` is on PATH.")
    } else if apply {
        format!(
            "Installed to {target_s}. Start a new agent session so it loads the skill; the agentbox binary must be on PATH."
        )
    } else {
        "Preview only. Rerun with --apply to write these files (use --dir for another skills folder, e.g. an OpenClaw workspace's skills/).".to_string()
    };
    Ok(Output::new(json!({
        "skill": NAME,
        "path": target_s,
        "applied": apply && changed > 0,
        "changed": changed,
        "files": files,
    }))
    .hint(hint))
}

fn write(path: &Path, content: &str) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| AppError::io("create skill folder", e))?;
    }
    fs::write(path, content).map_err(|e| AppError::io("write skill file", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::{CommandFactory, Parser};

    fn skill_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("skills")
            .join(NAME)
    }

    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, base, out);
            } else {
                let rel = p
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push(rel);
            }
        }
    }

    fn md_files() -> Vec<(String, String)> {
        FILES
            .iter()
            .filter(|(p, _)| p.ends_with(".md"))
            .map(|(p, c)| (p.to_string(), c.replace("\r\n", "\n")))
            .collect()
    }

    #[test]
    fn every_skill_file_is_embedded() {
        let mut on_disk = Vec::new();
        walk(&skill_root(), &skill_root(), &mut on_disk);
        on_disk.sort();
        let mut embedded: Vec<String> = FILES.iter().map(|(p, _)| p.to_string()).collect();
        embedded.sort();
        assert_eq!(on_disk, embedded, "skills/agentbox and skill::FILES differ");
    }

    /// agentskills.io frontmatter rules plus our size budget.
    #[test]
    fn frontmatter_follows_spec() {
        let md = md_files()
            .into_iter()
            .find(|(p, _)| p == "SKILL.md")
            .unwrap()
            .1;
        let fm = md
            .strip_prefix("---\n")
            .and_then(|r| r.split_once("\n---\n"))
            .expect("YAML frontmatter")
            .0;
        let field = |k: &str| {
            fm.lines()
                .find_map(|l| l.strip_prefix(&format!("{k}:")))
                .map(str::trim)
                .unwrap_or_else(|| panic!("missing {k}"))
                .to_string()
        };
        let name = field("name");
        assert_eq!(name, NAME);
        assert!(name.len() <= 64 && !name.starts_with('-') && !name.ends_with('-'));
        assert!(!name.contains("--"));
        assert!(name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        // Folded block scalar: the indented lines after `description: >`.
        assert_eq!(field("description"), ">");
        let desc: Vec<&str> = fm
            .lines()
            .skip_while(|l| !l.starts_with("description:"))
            .skip(1)
            .take_while(|l| l.starts_with("  "))
            .map(str::trim)
            .collect();
        let desc = desc.join(" ");
        assert!(
            !desc.is_empty() && desc.len() <= 1024,
            "description is {} chars",
            desc.len()
        );
        assert!(
            desc.contains("Do not use"),
            "description should say when not to use it"
        );
        let compat = field("compatibility");
        assert!(!compat.is_empty() && compat.len() <= 500);
        assert!(
            fm.contains("        - agentbox"),
            "openclaw requires.bins agentbox"
        );
        let lines = md.lines().count();
        assert!(
            lines < 250,
            "SKILL.md has {lines} lines; keep it short for small models"
        );
        // Rough token estimate: ~4 chars per token.
        assert!(md.len() / 4 < 5000, "SKILL.md is ~{} tokens", md.len() / 4);
    }

    #[test]
    fn referenced_files_exist() {
        let md = md_files()
            .into_iter()
            .find(|(p, _)| p == "SKILL.md")
            .unwrap()
            .1;
        for (rel, _) in FILES.iter().filter(|(p, _)| p.starts_with("references/")) {
            assert!(md.contains(rel), "SKILL.md never points to {rel}");
        }
        for word in md.split(['`', '(', ')', ' ']) {
            if word.starts_with("references/") || word.starts_with("assets/") {
                assert!(
                    FILES.iter().any(|(p, _)| *p == word),
                    "SKILL.md mentions missing file {word}"
                );
            }
        }
    }

    #[test]
    fn asset_templates_parse() {
        for (rel, src) in FILES.iter().filter(|(p, _)| p.ends_with(".toml")) {
            crate::report::template::Template::parse(src, rel)
                .unwrap_or_else(|e| panic!("{rel}: {}", e.message));
        }
    }

    // ---- command examples -------------------------------------------------

    #[derive(Debug, PartialEq)]
    enum Tok {
        Word(String),
        Op(String),
    }

    /// Minimal POSIX-ish word splitter: quotes, backslash escapes, comments,
    /// and the operators `| || & && ; < << > >>`.
    fn tokenize(line: &str) -> Result<Vec<Tok>, String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut in_word = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\'' => {
                    in_word = true;
                    loop {
                        match chars.next() {
                            Some('\'') => break,
                            Some(x) => cur.push(x),
                            None => return Err("unclosed single quote".into()),
                        }
                    }
                }
                '"' => {
                    in_word = true;
                    loop {
                        match chars.next() {
                            Some('"') => break,
                            Some('\\') => match chars.next() {
                                Some(x @ ('"' | '\\' | '$' | '`')) => cur.push(x),
                                Some(x) => {
                                    cur.push('\\');
                                    cur.push(x);
                                }
                                None => return Err("dangling backslash".into()),
                            },
                            Some('$') => {
                                return Err(format!(
                                "unescaped $ inside double quotes in `{line}`; use single quotes"
                            ))
                            }
                            Some(x) => cur.push(x),
                            None => return Err("unclosed double quote".into()),
                        }
                    }
                }
                '\\' => {
                    in_word = true;
                    if let Some(x) = chars.next() {
                        cur.push(x);
                    }
                }
                '#' if !in_word => break,
                ' ' | '\t' => {
                    if in_word {
                        out.push(Tok::Word(std::mem::take(&mut cur)));
                        in_word = false;
                    }
                }
                '|' | '&' | ';' | '<' | '>' => {
                    if in_word {
                        out.push(Tok::Word(std::mem::take(&mut cur)));
                        in_word = false;
                    }
                    let mut op = c.to_string();
                    if chars.peek() == Some(&c) && c != ';' {
                        op.push(chars.next().unwrap());
                    }
                    out.push(Tok::Op(op));
                }
                _ => {
                    in_word = true;
                    cur.push(c);
                }
            }
        }
        if in_word {
            out.push(Tok::Word(cur));
        }
        Ok(out)
    }

    /// `agentbox note add` style mentions: only subcommand names, no arguments.
    fn is_name_only(span: &str) -> bool {
        let mut cmd = Cli::command();
        for w in span.split_whitespace().skip(1) {
            match cmd.find_subcommand(w) {
                Some(sub) => cmd = sub.clone(),
                None => return false,
            }
        }
        true
    }

    /// Commands found in a Markdown file: (line number, argv).
    fn commands(md: &str) -> Vec<(usize, Result<Vec<String>, String>)> {
        let mut found = Vec::new();
        let mut in_fence = false;
        let mut heredoc: Option<String> = None;
        let lines: Vec<&str> = md.lines().collect();
        let mut i = 0;
        while i < lines.len() {
            let n = i + 1;
            let raw = lines[i];
            i += 1;
            let t = raw.trim();
            if let Some(end) = &heredoc {
                if t == end {
                    heredoc = None;
                }
                continue;
            }
            if t.starts_with("```") {
                in_fence = !in_fence;
                continue;
            }
            let mut logical = Vec::new();
            if in_fence {
                let mut l = t.to_string();
                while l.ends_with('\\') && i < lines.len() {
                    l.pop();
                    l.push(' ');
                    l.push_str(lines[i].trim());
                    i += 1;
                }
                logical.push(l.trim_start_matches("$ ").to_string());
            } else {
                // Inline code spans that start with `agentbox `, except bare
                // command names used in prose (`agentbox calc`).
                for (k, span) in t.split('`').enumerate() {
                    if k % 2 == 1 && span.starts_with("agentbox ") && !is_name_only(span) {
                        logical.push(span.to_string());
                    }
                }
            }
            for l in logical {
                let toks = match tokenize(&l) {
                    Ok(t) => t,
                    Err(e) => {
                        if l.contains("agentbox") {
                            found.push((n, Err(e)));
                        }
                        continue;
                    }
                };
                // Split into pipeline/list segments.
                let mut seg: Vec<String> = Vec::new();
                let mut segs = Vec::new();
                let mut skip_next = false;
                for tok in toks {
                    match tok {
                        Tok::Op(op) if op == "<<" => {
                            skip_next = true;
                            seg.push("<<".into());
                        }
                        Tok::Op(op) if op == ">" || op == ">>" => {
                            skip_next = true;
                            seg.push(">".into());
                        }
                        Tok::Op(op) if op == "<" => seg.push("<".into()),
                        Tok::Op(_) => segs.push(std::mem::take(&mut seg)),
                        Tok::Word(w) if skip_next => {
                            skip_next = false;
                            if seg.last().is_some_and(|s| s == "<<") {
                                heredoc = Some(w.clone());
                            }
                            seg.pop();
                        }
                        Tok::Word(w) => seg.push(w),
                    }
                }
                segs.push(seg);
                for s in segs {
                    if s.first().is_some_and(|w| w == "agentbox") {
                        let bad = s.iter().find(|w| {
                            *w == "<"
                                || w.contains("...")
                                || w.contains('…')
                                || ["doc:N", "tbl:N", "PATH", "URL", "SYMBOL", "SLUG"]
                                    .contains(&w.as_str())
                        });
                        found.push((
                            n,
                            match bad {
                                Some(b) => {
                                    Err(format!("placeholder `{b}` in `{l}`; use a real value"))
                                }
                                None => Ok(s),
                            },
                        ));
                    }
                }
            }
        }
        found
    }

    #[test]
    fn tokenizer_and_extractor() {
        let v = commands("```bash\nagentbox note add 'Pro is $20' --tag a # c\necho x | agentbox call read - <<'EOF'\nagentbox not a command\nEOF\n```\nRun `agentbox now --tz UTC` first.\n");
        let argv: Vec<Vec<String>> = v.into_iter().map(|(_, r)| r.unwrap()).collect();
        assert_eq!(
            argv[0],
            ["agentbox", "note", "add", "Pro is $20", "--tag", "a"]
        );
        assert_eq!(argv[1], ["agentbox", "call", "read", "-"]);
        assert_eq!(argv[2], ["agentbox", "now", "--tz", "UTC"]);
        assert_eq!(argv.len(), 3);
        assert!(commands("```\nagentbox read doc:N\n```")
            .iter()
            .all(|(_, r)| r.is_err()));
        assert!(commands("```\nagentbox note add \"costs $20\"\n```")
            .iter()
            .all(|(_, r)| r.is_err()));
    }

    /// Every `agentbox ...` example in the skill parses with the real CLI,
    /// so the skill cannot drift from the binary.
    #[test]
    fn every_skill_command_parses() {
        let mut total = 0;
        let mut errors = Vec::new();
        for (file, md) in md_files() {
            for (line, argv) in commands(&md) {
                total += 1;
                match argv {
                    Err(e) => errors.push(format!("{file}:{line}: {e}")),
                    Ok(argv) => {
                        let res = Cli::try_parse_from(&argv);
                        if let Err(e) = res.as_ref().map(|_| ()).or_else(|e| {
                            use clap::error::ErrorKind::{DisplayHelp, DisplayVersion};
                            if matches!(e.kind(), DisplayHelp | DisplayVersion) {
                                Ok(())
                            } else {
                                Err(e)
                            }
                        }) {
                            let msg = e.render().to_string();
                            errors.push(format!(
                                "{file}:{line}: `{}`\n{}",
                                argv.join(" "),
                                msg.lines().next().unwrap_or("")
                            ));
                        }
                    }
                }
            }
        }
        assert!(
            errors.is_empty(),
            "{} bad command(s):\n{}",
            errors.len(),
            errors.join("\n")
        );
        eprintln!("checked {total} agentbox commands in the skill");
        assert!(
            total > 100,
            "only {total} commands found; extractor broken?"
        );
    }

    /// The cheat-sheet in SKILL.md mentions every agent-facing subcommand.
    #[test]
    fn skill_covers_every_tool() {
        let md = md_files()
            .into_iter()
            .find(|(p, _)| p == "SKILL.md")
            .unwrap()
            .1;
        let mut paths = Vec::new();
        fn leaves(prefix: &str, cmd: &clap::Command, out: &mut Vec<String>) {
            if cmd.has_subcommands() {
                for s in cmd.get_subcommands() {
                    leaves(&format!("{prefix} {}", s.get_name()), s, out);
                }
            } else {
                out.push(prefix.trim().to_string());
            }
        }
        for sc in Cli::command().get_subcommands() {
            if matches!(sc.get_name(), "schema" | "call" | "config" | "skill") {
                continue;
            }
            leaves(sc.get_name(), sc, &mut paths);
        }
        for p in paths {
            let needle = format!("agentbox {p}");
            let ok = md
                .match_indices(&needle)
                .any(|(i, _)| md[i + needle.len()..].starts_with([' ', '\n', '`']));
            assert!(ok, "SKILL.md lacks `agentbox {p}`");
        }
    }

    #[test]
    fn install_previews_then_writes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_string_lossy().to_string();
        let p = install(Some(&dir), false).unwrap();
        assert_eq!(p.data["applied"], false);
        assert_eq!(p.data["changed"], FILES.len());
        assert!(!tmp.path().join("agentbox").exists());
        let a = install(Some(&dir), true).unwrap();
        assert_eq!(a.data["applied"], true);
        let skill = fs::read_to_string(tmp.path().join("agentbox/SKILL.md")).unwrap();
        assert!(skill.starts_with("---"));
        assert!(tmp.path().join("agentbox/references/reports.md").exists());
        let again = install(Some(&dir), true).unwrap();
        assert_eq!(
            (again.data["changed"].clone(), again.data["applied"].clone()),
            (json!(0), json!(false))
        );
        assert!(again.hint.unwrap().contains("up to date"));
    }
}
