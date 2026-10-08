use super::build::render;
use super::check::{check_text, CheckReport};
use super::template::{Overrides, Template, BUILTINS};
use super::*;
use crate::cmd::note::Note;
use crate::state::DocMeta;

fn tpl(name: &str) -> Template {
    let (_, src) = BUILTINS.iter().find(|(n, _)| *n == name).unwrap();
    Template::parse(src, name).unwrap()
}

fn filler(topic: &str) -> String {
    format!(
        "{topic} is widely used in production by many teams and offers a mature ecosystem, clear documentation, \
         strong performance under load, and an active community that ships regular releases with careful attention to stability and security. \
         Teams report that onboarding takes days rather than weeks, and the upgrade path between major versions is well documented."
    )
}

fn top3() -> String {
    format!(
        "# Top 3 Rust Web Frameworks\n\n\
         ## 1. Axum\n\n{} ([tokio.rs](https://tokio.rs/axum))\n\n\
         ## 2. Actix Web\n\n{} ([actix.rs](https://actix.rs))\n\n\
         ## 3. Rocket\n\n{} ([rocket.rs](https://rocket.rs))\n\n\
         ## Sources\n\n- [Axum](https://tokio.rs/axum)\n- [Actix Web](https://actix.rs)\n- [Rocket](https://rocket.rs)\n",
        filler("Axum"),
        filler("Actix Web"),
        filler("Rocket")
    )
}

fn rules(r: &CheckReport) -> Vec<&'static str> {
    r.issues
        .iter()
        .filter(|i| i.severity == "error")
        .map(|i| i.rule)
        .collect()
}

fn issue<'a>(r: &'a CheckReport, rule: &str) -> &'a check::Issue {
    r.issues
        .iter()
        .find(|i| i.rule == rule)
        .unwrap_or_else(|| panic!("no `{rule}` issue in {:#?}", r.issues))
}

#[test]
fn good_top_n_report_passes() {
    let r = check_text(&top3(), &tpl("top-n"), "r.md", None);
    assert!(r.pass, "{:#?}", r.issues);
    assert_eq!(r.score, 100);
    assert_eq!(r.stats.sources, 3);
    assert_eq!(r.stats.sections, 4);
    assert!(r.stats.citations >= 3);
}

#[test]
fn h3_instead_of_h2_numbered_item_is_an_error_with_replace_fix() {
    let md = top3().replace("## 1. Axum", "### 1. Axum");
    let r = check_text(&md, &tpl("top-n"), "r.md", None);
    assert!(!r.pass);
    let i = issue(&r, "heading_level");
    assert_eq!(i.line, Some(3));
    assert_eq!(
        i.fix,
        r####"agentbox file replace "r.md" --find "### 1. Axum" --replace "## 1. Axum" --apply"####
    );
    assert_eq!(rules(&r), vec!["heading_level"]);
}

#[test]
fn numbered_item_count_order_and_format() {
    let t = tpl("top-n");
    // Missing item 3.
    let md = top3().replace("## 3. Rocket", "Rocket");
    let r = check_text(&md, &t, "r.md", None);
    assert!(issue(&r, "item_count").message.contains("found 2 of 3"));
    assert!(issue(&r, "item_count").fix.contains("## 3. Title"));
    // Extra item 4 (too many).
    let md = top3().replace(
        "## Sources",
        &format!(
            "## 4. Warp\n\n{} ([warp](https://warp.rs))\n\n## Sources",
            filler("Warp")
        ),
    );
    let r = check_text(&md, &t, "r.md", None);
    assert!(issue(&r, "item_count").message.contains("at most 3"));
    // Wrong numbers.
    let md = top3().replace("## 2. Actix Web", "## 5. Actix Web");
    let r = check_text(&md, &t, "r.md", None);
    assert!(issue(&r, "numbering")
        .fix
        .contains(r####"--replace "## 2. Actix Web""####));
    // Near-miss format `2)`.
    let md = top3().replace("## 2. Actix Web", "## 2) Actix Web");
    let r = check_text(&md, &t, "r.md", None);
    let i = issue(&r, "numbering");
    assert!(
        i.fix
            .contains(r####"--find "## 2) Actix Web" --replace "## 2. Actix Web""####),
        "{}",
        i.fix
    );
    // --n changes the required count.
    let t5 = tpl("top-n")
        .apply(&Overrides {
            n: Some(5),
            columns: None,
        })
        .unwrap();
    assert!(issue(&check_text(&top3(), &t5, "r.md", None), "item_count")
        .message
        .contains("3 of 5"));
}

#[test]
fn headings_in_code_fences_are_ignored() {
    let md = top3().replace(
        "## Sources",
        "```markdown\n## 4. Not real\n# Not a title\n```\n\n## Sources",
    );
    let r = check_text(&md, &tpl("top-n"), "r.md", None);
    assert!(r.pass, "{:#?}", r.issues);
}

#[test]
fn citations_forbidden_h1_todo_and_heading_format() {
    let t = tpl("top-n");
    let md = top3().replace(" ([actix.rs](https://actix.rs))", "");
    assert_eq!(
        issue(&check_text(&md, &t, "r.md", None), "citation").line,
        Some(7)
    );

    let md = top3().replace("Rocket is widely", "As an AI, I think Rocket is widely");
    assert!(rules(&check_text(&md, &t, "r.md", None)).contains(&"forbidden"));

    let md = top3().replace("## Sources", "# Appendix\n\n## Sources");
    let i = issue(&check_text(&md, &t, "r.md", None), "multiple_h1").clone();
    assert!(i.fix.contains(r####"--replace "## Appendix""####));

    let md = top3().replace(
        "## 3. Rocket\n\n",
        "## 3. Rocket\n\n<!-- TODO(4): write more -->\n",
    );
    let r = check_text(&md, &t, "r.md", None);
    assert_eq!(r.stats.todos_left, 1);
    assert!(issue(&r, "todo")
        .fix
        .contains(r####"--find "<!-- TODO(4): write more -->""####));

    let md = top3().replace("## Sources", "##Sources");
    let r = check_text(&md, &t, "r.md", None);
    assert!(issue(&r, "heading_format")
        .fix
        .contains(r####"--replace "## Sources""####));
    assert!(rules(&r).contains(&"sources_section"));
}

#[test]
fn title_rules() {
    let t = tpl("top-n");
    let md = top3().replacen("# Top 3", "## Top 3", 1);
    assert!(issue(&check_text(&md, &t, "r.md", None), "title")
        .fix
        .contains(r####"--replace "# Top 3 Rust Web Frameworks""####));
    let md = format!("Intro text first.\n\n{}", top3());
    assert!(rules(&check_text(&md, &t, "r.md", None)).contains(&"title"));
}

fn brief_ok() -> String {
    format!(
        "# Acme Brief\n\n## Summary\n\n{}\n\n## Key Findings\n\n- {} ([a.test](https://a.test/1))\n- {} ([b.test](https://b.test/2))\n\n## Sources\n\n- [A](https://a.test/1)\n- [B](https://b.test/2)\n",
        filler("Acme"),
        filler("Revenue"),
        filler("Headcount")
    )
}

#[test]
fn brief_sections_order_sources_and_lengths() {
    let t = tpl("brief");
    let r = check_text(&brief_ok(), &t, "b.md", None);
    assert!(r.pass, "{:#?}", r.issues);

    // Similar heading -> rename fix.
    let md = brief_ok().replace("## Summary", "## Executive Summary");
    let i = issue(&check_text(&md, &t, "b.md", None), "missing_section").clone();
    assert!(
        i.fix
            .contains(r####"--find "## Executive Summary" --replace "## Summary""####),
        "{}",
        i.fix
    );

    // Entirely missing.
    let md = brief_ok().replace("## Summary\n", "");
    let f = issue(&check_text(&md, &t, "b.md", None), "missing_section")
        .fix
        .clone();
    assert!(
        f.contains("after the section that starts at line 1 (`# Acme Brief`)"),
        "{f}"
    );

    // Wrong order.
    let ok = brief_ok();
    let parts: Vec<&str> = ok.split("## ").collect();
    let swapped = format!("{}## {}## {}## {}", parts[0], parts[2], parts[1], parts[3]);
    assert!(rules(&check_text(&swapped, &t, "b.md", None)).contains(&"order"));

    // Alias heading for sources and too few sources.
    let md = brief_ok()
        .replace("## Sources", "## References")
        .replace("- [B](https://b.test/2)\n", "");
    let r = check_text(&md, &t, "b.md", None);
    assert!(issue(&r, "sources_section")
        .fix
        .contains(r####"--replace "## Sources""####));
    assert!(issue(&r, "min_sources").message.contains("1 distinct"));

    // Too short overall and per section.
    let md = "# T\n\n## Summary\n\nShort.\n\n## Key Findings\n\n- one ([a](https://a.test))\n\n## Sources\n\n- https://a.test\n- https://b.test\n";
    let r = check_text(md, &t, "b.md", None);
    assert!(rules(&r).contains(&"doc_words"));
    assert!(issue(&r, "section_words")
        .message
        .contains("needs at least 30"));
}

#[test]
fn compare_table_rules() {
    let t = tpl("compare");
    let rec = format!("{} {}", filler("Option A"), filler("Option B"));
    let base = format!(
        "# Compare\n\n## Summary\n\n{}\n\n## Comparison\n\nTABLE\n\nData from vendors ([x](https://x.test)).\n\n## Recommendation\n\n{}\n\n## Sources\n\n- https://x.test\n- https://y.test\n- https://z.test\n",
        filler("Both"),
        rec
    );
    let good = base.replace(
        "TABLE",
        "| Option | Price | Strengths | Weaknesses |\n|---|---|---|---|\n| A | $1 | fast | new |",
    );
    assert!(
        check_text(&good, &t, "c.md", None).pass,
        "{:#?}",
        check_text(&good, &t, "c.md", None).issues
    );
    let r = check_text(&base.replace("TABLE", "no table here"), &t, "c.md", None);
    assert!(issue(&r, "table")
        .fix
        .contains("| Option | Price | Strengths | Weaknesses |"));
    let r = check_text(
        &base.replace("TABLE", "| Option | Cost |\n|---|---|\n| A | $1 |"),
        &t,
        "c.md",
        None,
    );
    assert!(issue(&r, "table_columns")
        .message
        .contains("Price, Strengths, Weaknesses"));
}

#[test]
fn sources_must_come_from_notes_when_required() {
    let mut t = tpl("brief");
    t.sources_from_notes = true;
    let allowed = vec![crate::cmd::search::normalize_url("https://a.test/1")];
    let r = check_text(&brief_ok(), &t, "b.md", Some(&allowed));
    let i = issue(&r, "source_not_in_notes");
    assert!(i.message.contains("https://b.test/2"));
    assert!(i.fix.contains("agentbox fetch https://b.test/2"));
    assert_eq!(
        r.issues
            .iter()
            .filter(|i| i.rule == "source_not_in_notes")
            .count(),
        1,
        "reported once per URL"
    );
    let r = check_text(&brief_ok(), &t, "b.md", Some(&[]));
    assert!(issue(&r, "source_not_in_notes")
        .message
        .contains("none are recorded"));
}

fn note(id: u64, text: &str, source: Option<&str>, tag: Option<&str>) -> Note {
    Note {
        id,
        text: text.into(),
        source: source.map(String::from),
        tag: tag.map(String::from),
        created: String::new(),
    }
}

const STRUCTURE_RULES: &[&str] = &[
    "title",
    "multiple_h1",
    "heading_level",
    "numbering",
    "item_count",
    "missing_section",
    "order",
    "sources_section",
    "heading_format",
    "table_columns",
    "forbidden",
];

#[test]
fn built_skeletons_pass_structure_checks_for_every_builtin() {
    let notes = vec![
        note(
            1,
            "Revenue grew 20%",
            Some("https://a.test/r"),
            Some("findings"),
        ),
        note(2, "Item two fact", Some("doc:1"), Some("item2")),
        note(3, "Loose fact", None, None),
    ];
    let docs = vec![DocMeta {
        id: 1,
        url: "https://docs.test/page".into(),
        title: "Docs Page".into(),
        content_type: String::new(),
        fetched_at: String::new(),
    }];
    for (name, _) in BUILTINS {
        let t = tpl(name);
        let built = render(&t, "My Report", &notes, &docs);
        let r = check_text(&built.content, &t, "r.md", None);
        let bad: Vec<_> = r
            .issues
            .iter()
            .filter(|i| i.severity == "error" && STRUCTURE_RULES.contains(&i.rule))
            .collect();
        assert!(bad.is_empty(), "{name}: {bad:#?}\n{}", built.content);
        assert_eq!(r.stats.todos_left, built.todos, "{name}");
        assert!(built.todos > 0);
    }
}

#[test]
fn build_places_notes_and_renders_citation_styles() {
    let notes = vec![
        note(
            1,
            "Revenue grew 20%",
            Some("https://a.test/r"),
            Some("findings"),
        ),
        note(2, "Second item fact", Some("doc:1"), Some("item-2")),
        note(3, "Unplaced fact", Some("https://c.test"), Some("misc")),
    ];
    let docs = vec![DocMeta {
        id: 1,
        url: "https://docs.test/page".into(),
        title: "Docs Page".into(),
        content_type: String::new(),
        fetched_at: String::new(),
    }];

    let b = render(&tpl("brief"), "Acme", &notes, &docs);
    let kf = b.content.split("## Key Findings").nth(1).unwrap();
    assert!(
        kf.contains("- Revenue grew 20% ([a.test](https://a.test/r))"),
        "{}",
        b.content
    );
    assert!(b.content.contains("<!-- NOTES"));
    assert!(b.content.contains("- [a.test](https://a.test/r)"));
    assert_eq!(b.placed, 1);
    assert_eq!(b.unassigned, 2);

    let b = render(&tpl("top-n"), "Top", &notes, &docs);
    let item2 = b
        .content
        .split("## 2. ")
        .nth(1)
        .unwrap()
        .split("## 3. ")
        .next()
        .unwrap();
    assert!(
        item2.contains("Second item fact ([docs.test](https://docs.test/page))"),
        "{}",
        b.content
    );
    assert!(b.content.contains("- [Docs Page](https://docs.test/page)"));

    let mut t = tpl("brief");
    t.citation_style = super::template::CitationStyle::Numbered;
    let b = render(&t, "Acme", &notes, &docs);
    assert!(
        b.content.contains("- Revenue grew 20% [1]"),
        "{}",
        b.content
    );
    assert!(b.content.contains("1. [a.test](https://a.test/r)"));

    t.citation_style = super::template::CitationStyle::Footnote;
    let b = render(&t, "Acme", &notes, &docs);
    assert!(b.content.contains("- Revenue grew 20%[^1]"));
    assert!(b.content.contains("[^1]: [a.test](https://a.test/r)"));

    let b = render(&tpl("compare"), "C", &[], &docs);
    assert!(b
        .content
        .contains("| Option | Price | Strengths | Weaknesses |\n|---|---|---|---|\n| <!-- TODO("));
    assert!(b
        .content
        .contains("fetched docs you could cite: [Docs Page](https://docs.test/page)"));
}

#[test]
fn run_build_preview_apply_and_check_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("state"));
    note::add(
        &store,
        "Axum is built on Tokio",
        Some("https://tokio.rs/axum"),
        Some("item1"),
    )
    .unwrap();
    let out_path = tmp.path().join("r.md").to_string_lossy().to_string();
    let args = BuildArgs {
        title: "Top Frameworks",
        template: "top-n",
        overrides: Overrides {
            n: Some(2),
            columns: None,
        },
        tag: None,
        out: Some(&out_path),
        apply: false,
    };
    let out = run_build(&store, &args).unwrap();
    assert_eq!(out.data["applied"], false);
    assert!(!std::path::Path::new(&out_path).exists());
    assert_eq!(
        out.data["outline"][1]["heading"]
            .as_str()
            .unwrap()
            .split(' ')
            .next(),
        Some("1.")
    );
    let out = run_build(
        &store,
        &BuildArgs {
            apply: true,
            ..args
        },
    )
    .unwrap();
    assert_eq!(out.data["applied"], true);
    assert_eq!(out.data["notes_placed"], 1);

    let chk = run_check(
        &store,
        &out_path,
        "top-n",
        &Overrides {
            n: Some(2),
            columns: None,
        },
    )
    .unwrap();
    assert_eq!(chk.data["pass"], false);
    assert!(chk.data["stats"]["todos_left"].as_u64().unwrap() > 0);
    assert!(chk.data["issues"]
        .as_array()
        .unwrap()
        .iter()
        .all(|i| !STRUCTURE_RULES.contains(&i["rule"].as_str().unwrap())));

    // Without --out the content is returned inline.
    let out = run_build(
        &store,
        &BuildArgs {
            title: "T",
            template: "brief",
            overrides: Overrides::default(),
            tag: Some("nomatch"),
            out: None,
            apply: false,
        },
    )
    .unwrap();
    assert!(out.data["content"].as_str().unwrap().starts_with("# T\n"));
    assert_eq!(out.data["notes_placed"], 0);
}

#[test]
fn allowed_urls_include_notes_and_docs() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path());
    let id = store
        .save_doc(
            DocMeta {
                id: 0,
                url: "https://doc.test/a".into(),
                title: "A".into(),
                content_type: String::new(),
                fetched_at: String::new(),
            },
            "x",
        )
        .unwrap();
    note::add(&store, "fact", Some("https://www.note.test/x/"), None).unwrap();
    note::add(&store, "fact2", Some(&format!("doc:{id}")), None).unwrap();
    let allowed = allowed_urls(&store);
    assert!(allowed.contains(&crate::cmd::search::normalize_url("https://note.test/x")));
    assert!(allowed.contains(&crate::cmd::search::normalize_url("https://doc.test/a")));
}

#[test]
fn unnumbered_item_is_adopted_with_rename_fix() {
    let t = tpl("top-n");
    let md = top3().replace("## 3. Rocket", "## Rocket");
    let r = check_text(&md, &t, "r.md", None);
    let i = r
        .issues
        .iter()
        .find(|i| i.rule == "numbering")
        .expect("numbering issue");
    assert_eq!(
        i.fix,
        r####"agentbox file replace "r.md" --find "## Rocket" --replace "## 3. Rocket" --apply"####
    );
    assert!(!rules(&r).contains(&"item_count"), "{:#?}", r.issues);
}

#[test]
fn unknown_sections_are_warned() {
    let t = tpl("brief");
    let md = brief_ok().replace("## Sources", "## Background\n\nSome context.\n\n## Sources");
    let r = check_text(&md, &t, "b.md", None);
    let i = issue(&r, "extra_section");
    assert_eq!(i.severity, "warn");
    assert!(
        i.fix.contains("`## Summary`, `## Key Findings`"),
        "{}",
        i.fix
    );
    // Sub-sections one level deeper are fine.
    let md = brief_ok().replace("## Sources", "### Detail\n\nMore.\n\n## Sources");
    assert!(!rules(&check_text(&md, &t, "b.md", None)).contains(&"extra_section"));
}

#[test]
fn style_must_contain_notes_setext_and_empty_table() {
    // Citations present but in the wrong style -> warning only.
    let mut t = tpl("top-n");
    t.citation_style = super::template::CitationStyle::Footnote;
    let r = check_text(&top3(), &t, "r.md", None);
    let i = issue(&r, "citation_style");
    assert_eq!(i.severity, "warn");
    assert!(i.fix.contains("[^1]"), "{}", i.fix);
    assert!(r.pass);

    // must_contain keyword.
    let mut t = tpl("brief");
    t.sections[0].must_contain = vec!["revenue".into()];
    let r = check_text(&brief_ok(), &t, "b.md", None);
    assert!(issue(&r, "must_contain").message.contains("\"revenue\""));

    // Leftover NOTES block from `report build`.
    let t = tpl("brief");
    let md = brief_ok().replace(
        "## Sources",
        "<!-- NOTES (not placed):\n- x\n-->\n\n## Sources",
    );
    let r = check_text(&md, &t, "b.md", None);
    let i = issue(&r, "notes_block");
    assert_eq!(i.severity, "warn");
    assert!(i.fix.contains("delete lines"));

    // Setext heading -> warn with an ATX rewrite.
    let md = brief_ok().replace("## Summary\n", "Summary\n-------\n");
    let r = check_text(&md, &t, "b.md", None);
    let i = r
        .issues
        .iter()
        .find(|i| i.rule == "heading_format")
        .expect("setext warning");
    assert_eq!(i.severity, "warn");
    assert!(i.fix.contains("## Summary"), "{}", i.fix);

    // Table with a header but no data rows -> warning.
    let t = tpl("compare");
    let rec = format!("{} {}", filler("Option A"), filler("Option B"));
    let md = format!(
        "# C\n\n## Summary\n\n{}\n\n## Comparison\n\n| Option | Price | Strengths | Weaknesses |\n|---|---|---|---|\n\nVendors ([x](https://x.test)).\n\n## Recommendation\n\n{}\n\n## Sources\n\n- https://x.test\n- https://y.test\n- https://z.test\n",
        filler("Both"),
        rec
    );
    let r = check_text(&md, &t, "c.md", None);
    let i = issue(&r, "table");
    assert_eq!(i.severity, "warn");
}
