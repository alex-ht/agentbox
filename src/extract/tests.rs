use super::kinds::{find_emails, find_links, find_numbers, find_prices};
use super::*;
use crate::state::DocMeta;

fn prices(s: &str) -> Vec<kinds::PriceHit> {
    find_prices(s, None)
}

#[test]
fn prices_tricky_formats() {
    let p = prices("$1,299");
    assert_eq!(
        (p[0].value, p[0].currency.as_deref()),
        (1299.0, Some("USD"))
    );

    let p = prices("Pro is US$ 20/month for teams");
    assert_eq!(p[0].value, 20.0);
    assert_eq!(p[0].currency.as_deref(), Some("USD"));
    assert_eq!(p[0].period, Some("month"));
    assert_eq!(p[0].raw, "US$ 20/month");

    let p = prices("方案 NT$1,990 起");
    assert_eq!(
        (p[0].value, p[0].currency.as_deref()),
        (1990.0, Some("TWD"))
    );

    let p = prices("Business: €9.99 per user/month, billed annually");
    assert_eq!(p[0].value, 9.99);
    assert_eq!(p[0].currency.as_deref(), Some("EUR"));
    assert_eq!(p[0].per, Some("user"));
    assert_eq!(
        p[0].period,
        Some("month"),
        "billed annually must not override /month"
    );

    let p = prices("License: 1,200 USD one-time");
    assert_eq!(
        (p[0].value, p[0].currency.as_deref(), p[0].period),
        (1200.0, Some("USD"), Some("one-time"))
    );

    let p = prices("£8 per seat per month");
    assert_eq!((p[0].per, p[0].period), (Some("seat"), Some("month")));

    let p = prices("$96 billed annually");
    assert_eq!(p[0].period, Some("year"));

    let p = prices("月費 299元/月");
    assert_eq!((p[0].value, p[0].period), (299.0, Some("month")));

    let p = prices("9,99 € pro Monat");
    assert_eq!((p[0].value, p[0].currency.as_deref()), (9.99, Some("EUR")));

    let p = prices("Revenue hit $5.7 billion");
    assert_eq!(p[0].value, 5.7e9);

    let p = prices("$10 – $20 per month");
    assert_eq!(p.len(), 2);
    assert_eq!(p[1].period, Some("month"));

    assert!(prices("Version 2.0 shipped in 2026 with 12 features").is_empty());
}

#[test]
fn dollar_currency_follows_site_country() {
    assert_eq!(
        find_prices("$499", Some("www.example.com.tw"))[0]
            .currency
            .as_deref(),
        Some("TWD")
    );
    assert_eq!(
        find_prices("$499", Some("shop.example.ca"))[0]
            .currency
            .as_deref(),
        Some("CAD")
    );
    assert_eq!(
        find_prices("1,990元", Some("example.tw"))[0]
            .currency
            .as_deref(),
        Some("TWD")
    );
    assert_eq!(
        find_prices("¥500", Some("example.cn"))[0]
            .currency
            .as_deref(),
        Some("CNY")
    );
    assert_eq!(
        find_prices("¥500", None)[0].currency.as_deref(),
        Some("JPY")
    );
}

#[test]
fn dates_english_iso_numeric_and_cjk() {
    let d = |s: &str| {
        dates::find_dates(s)
            .into_iter()
            .map(|h| h.iso)
            .collect::<Vec<_>>()
    };
    assert_eq!(d("Released on October 8, 2026."), vec!["2026-10-08"]);
    assert_eq!(
        d("Released Oct. 8th, 2026 and 8 October 2026"),
        vec!["2026-10-08", "2026-10-08"]
    );
    assert_eq!(d("Updated 2026-10-08T09:30:00Z"), vec!["2026-10-08"]);
    assert_eq!(d("on 2026/1/5"), vec!["2026-01-05"]);
    assert_eq!(d("發布日期：2026年10月8日"), vec!["2026-10-08"]);
    assert_eq!(d("民國115年10月8日公告"), vec!["2026-10-08"]);
    assert_eq!(d("2026 年 3 月"), vec!["2026-03"]);
    assert_eq!(d("Launch in March 2027"), vec!["2027-03"]);
    assert_eq!(d("Due 25/12/2026"), vec!["2026-12-25"]);
    let amb = dates::find_dates("Due 03/04/2026");
    assert_eq!(amb[0].iso, "2026-03-04");
    assert!(amb[0].ambiguous);
    assert!(d("February 30, 2026 is not a date; 2026-13-01 neither").is_empty());
    assert!(d("version 1.2.3 and 10.5").is_empty());
    assert_eq!(
        dates::parse_whole(" 2026年10月8日 ").as_deref(),
        Some("2026-10-08")
    );
    assert_eq!(dates::parse_whole("due 2026-10-08"), None);
}

#[test]
fn people_patterns() {
    let p = |s: &str| people::find_people(s);
    let h = p("Jensen Huang, CEO of Nvidia, said demand is strong.");
    assert_eq!(
        (h[0].name.as_str(), h[0].role.as_str(), h[0].org.as_deref()),
        ("Jensen Huang", "CEO", Some("Nvidia"))
    );

    let h = p("Nvidia CEO Jensen Huang said on Monday");
    assert_eq!(
        (h[0].name.as_str(), h[0].role.as_str(), h[0].org.as_deref()),
        ("Jensen Huang", "CEO", Some("Nvidia"))
    );

    let h = p("Jensen Huang (Chief Executive Officer) opened the event.");
    assert_eq!(h[0].role, "Chief Executive Officer");

    let h = p("Jensen Huang is the founder and CEO of NVIDIA.");
    assert_eq!(h[0].name, "Jensen Huang");
    assert_eq!(h[0].role, "founder and CEO");
    assert_eq!(h[0].org.as_deref(), Some("NVIDIA"));
    assert!(h[0].confidence >= 0.9);

    let h = p("Lisa Su — Chair and CEO");
    assert_eq!(
        (h[0].name.as_str(), h[0].role.as_str()),
        ("Lisa Su", "Chair and CEO")
    );

    let h = p("Meet Jane van der Berg, VP of Engineering at Acme Corp");
    assert_eq!(h[0].name, "Jane van der Berg");
    assert_eq!(h[0].role, "VP of Engineering");
    assert_eq!(h[0].org.as_deref(), Some("Acme Corp"));

    let h = p("Finance Minister Chrystia Freeland announced");
    assert_eq!(h[0].name, "Chrystia Freeland");

    let h = p("輝達執行長黃仁勳今天表示");
    assert_eq!(
        (h[0].name.as_str(), h[0].role.as_str()),
        ("黃仁勳", "執行長")
    );
    assert!(p("執行長表示看好").is_empty());

    assert!(p("The Chief Executive Officer of the company spoke.").is_empty());
    assert!(p("Our products ship in March.").is_empty());
}

#[test]
fn numbers_with_units() {
    let n = |s: &str| {
        find_numbers(s, None)
            .into_iter()
            .map(|h| (h.value, h.unit))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        n("a $5.7 trillion market"),
        vec![(5.7e12, Some("USD".into()))]
    );
    assert_eq!(
        n("grew 12% to 1.2 million users"),
        vec![(12.0, Some("%".into())), (1.2e6, Some("users".into()))]
    );
    assert_eq!(
        n("256 GB of RAM, 3.2x faster"),
        vec![(256.0, Some("GB".into())), (3.2, Some("x".into()))]
    );
    assert_eq!(n("10k stars"), vec![(1e4, Some("stars".into()))]);
    assert_eq!(n("營收 3.5億"), vec![(3.5e8, None)]);
    assert_eq!(n("up 7 percent"), vec![(7.0, Some("%".into()))]);
    assert!(n("In 2026 the H100 and B200 shipped 3 units").is_empty());
}

#[test]
fn links_and_emails() {
    let base = reqwest::Url::parse("https://acme.test/docs/").unwrap();
    let l = find_links(
        "See [guide](intro.html), [repo](https://github.com/acme/x) and https://acme.test/faq.",
        Some(&base),
    );
    let urls: Vec<&str> = l.iter().map(|h| h.url.as_str()).collect();
    assert_eq!(
        urls,
        vec![
            "https://acme.test/docs/intro.html",
            "https://github.com/acme/x",
            "https://acme.test/faq"
        ]
    );
    assert_eq!(l[0].text, "guide");
    let e: Vec<String> = find_emails("Mail sales@acme.test or logo@2x.png")
        .into_iter()
        .map(|x| x.0)
        .collect();
    assert_eq!(e, vec!["sales@acme.test"]);
}

#[test]
fn plain_and_context() {
    assert_eq!(
        plain("## **Pro** plan: [docs](https://x.test) `fast`"),
        "Pro plan: docs fast"
    );
    assert_eq!(plain("| Basic | $9 \\| mo |"), "Basic | $9 | mo");
    let long = format!("{}PRICE $5{}", "a ".repeat(100), " b".repeat(100));
    let start = long.find('$').unwrap();
    let c = context(&long, start, start + 2);
    assert!(
        c.contains("$5") && c.starts_with('…') && c.ends_with('…'),
        "{c}"
    );
    assert!(c.chars().count() <= CONTEXT_CHARS + 2);
}

fn store_with(docs: &[(&str, &str)]) -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path());
    for (url, body) in docs {
        store
            .save_doc(
                DocMeta {
                    id: 0,
                    url: url.to_string(),
                    title: "T".into(),
                    content_type: "text/html".into(),
                    fetched_at: "now".into(),
                },
                body,
            )
            .unwrap();
    }
    (tmp, store)
}

fn args(sources: &[&str], kind: &str) -> ExtractArgs {
    ExtractArgs {
        sources: sources.iter().map(|s| s.to_string()).collect(),
        from: None,
        kind: kind.into(),
        section: None,
        grep: None,
        limit: 20,
        site: None,
        save_table: false,
    }
}

const PRICING_A: &str = "# Acme pricing\n\n## Plans\n\n| Plan | Price |\n|---|---|\n| Basic | $9/mo |\n| Pro | $29/mo |\n\nEnterprise starts at $99 per user/month.\n\n```\nfake $1,000,000 in code\n```\n";
const PRICING_B: &str =
    "# Beta pricing\n\nStarter €12 per month. Team €30 per month. Starter €12 per month again.\n";

#[test]
fn multi_doc_prices_keep_sources_dedupe_and_save_table() {
    let (_t, store) = store_with(&[
        ("https://acme.test/pricing", PRICING_A),
        ("https://beta.test/p", PRICING_B),
    ]);
    let mut a = args(&["doc:1"], "prices");
    a.from = Some("doc:2".into());
    a.save_table = true;
    let out = run(&store, &a).unwrap();
    let d = &out.data;
    assert_eq!(d["total"], 5, "{d}");
    let items = d["items"].as_array().unwrap();
    assert_eq!(items[0]["doc"], "doc:1");
    assert_eq!(items[0]["url"], "https://acme.test/pricing");
    assert_eq!(items[0]["section"], 2);
    assert_eq!(items[0]["line"], 7);
    assert_eq!(items[4]["doc"], "doc:2");
    assert_eq!(items[4]["currency"], "EUR");
    assert!(
        !items.iter().any(|i| i["value"] == 1_000_000),
        "code fences are skipped"
    );
    let tbl = d["table"].as_str().unwrap();
    let t = crate::table::load(&store, tbl).unwrap();
    assert_eq!(t.rows.len(), 5);
    assert!(t.columns.contains(&"value".to_string()) && t.columns.contains(&"doc".to_string()));
    // The saved items can be sorted with table query.
    let q = crate::table::query::Query {
        sorts: vec!["-value".into()],
        ..Default::default()
    };
    let r = crate::table::query::run(&t, &q).unwrap();
    assert_eq!(crate::table::value::cell_str(&r.rows[0][0]), "99");
}

#[test]
fn positional_sources_can_be_comma_separated() {
    let (_t, store) = store_with(&[
        ("https://acme.test/pricing", PRICING_A),
        ("https://beta.test/p", PRICING_B),
    ]);
    let out = run(&store, &args(&["doc:1,doc:2"], "prices")).unwrap();
    assert_eq!(out.data["sources"].as_array().unwrap().len(), 2);
}

#[test]
fn tables_kind_saves_handles_and_previews() {
    let (_t, store) = store_with(&[("https://acme.test/pricing", PRICING_A)]);
    let out = run(&store, &args(&["doc:1"], "tables")).unwrap();
    let it = &out.data["items"][0];
    assert_eq!(it["table"], "tbl:1");
    assert_eq!(it["headers"], json!(["Plan", "Price"]));
    assert_eq!(it["rows_count"], 2);
    assert_eq!(it["preview"][1]["Price"], "$29/mo");
    assert_eq!(it["title"], "Plans");
    // Same extraction again reuses the stored table.
    let again = run(&store, &args(&["doc:1"], "tables")).unwrap();
    assert_eq!(again.data["items"][0]["table"], "tbl:1");
}

#[test]
fn html_table_fragments_inside_markdown_are_found() {
    let body = "# Raw\n\n<table><tr><th>Name</th><th>Score</th></tr><tr><td>A</td><td>1</td></tr></table>\n";
    let (_t, store) = store_with(&[("https://x.test", body)]);
    let out = run(&store, &args(&["doc:1"], "tables")).unwrap();
    assert_eq!(out.data["items"][0]["headers"], json!(["Name", "Score"]));
}

#[test]
fn limit_grep_section_site_and_errors() {
    let body = "# A\n\n[one](https://a.test/1) [two](https://b.test/2) [three](https://sub.a.test/3)\n\n## B\n\nsales@a.test\n";
    let (_t, store) = store_with(&[("https://a.test/", body)]);
    let mut a = args(&["doc:1"], "links");
    a.limit = 1;
    let out = run(&store, &a).unwrap();
    assert_eq!(out.data["total"], 3);
    assert_eq!(out.data["truncated"], true);
    assert!(out.hint.unwrap().contains("Showing 1 of 3"));

    let mut a = args(&["doc:1"], "links");
    a.site = Some("a.test".into());
    assert_eq!(run(&store, &a).unwrap().data["total"], 2);

    let mut a = args(&["doc:1"], "links");
    a.grep = Some("two".into());
    assert_eq!(run(&store, &a).unwrap().data["total"], 1);

    let mut a = args(&["doc:1"], "emails");
    a.section = Some(2);
    assert_eq!(
        run(&store, &a).unwrap().data["items"][0]["email"],
        "sales@a.test"
    );
    a.section = Some(1);
    assert_eq!(run(&store, &a).unwrap().data["total"], 0);
    a.section = Some(9);
    assert_eq!(run(&store, &a).unwrap_err().code, "bad_args");

    assert_eq!(
        run(&store, &args(&[], "links")).unwrap_err().code,
        "bad_args"
    );
    assert_eq!(
        run(&store, &args(&["doc:7"], "links")).unwrap_err().code,
        "doc_not_found"
    );
    let none = run(&store, &args(&["doc:1"], "prices")).unwrap();
    assert!(none.hint.unwrap().contains("No prices"));
}

#[test]
fn files_and_html_files_work_as_sources() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path().join("home"));
    let md = tmp.path().join("notes.md");
    std::fs::write(&md, "Launch on 2026年10月8日 for NT$1,990.\n").unwrap();
    let html = tmp.path().join("page.html");
    std::fs::write(
        &html,
        "<html><head><title>P</title></head><body><table><tr><th>Plan</th><th>Price</th></tr><tr><td>Pro</td><td>$5</td></tr></table></body></html>",
    )
    .unwrap();
    let out = run(&store, &args(&[md.to_str().unwrap()], "dates")).unwrap();
    assert_eq!(out.data["items"][0]["date"], "2026-10-08");
    assert_eq!(out.data["items"][0]["doc"], md.to_str().unwrap());
    let out = run(&store, &args(&[html.to_str().unwrap()], "tables")).unwrap();
    assert_eq!(out.data["items"][0]["preview"][0]["Price"], "$5");
}

#[test]
fn people_kind_end_to_end() {
    let body = "# Leadership\n\nJensen Huang is the founder and CEO of NVIDIA.\n\nColette Kress, CFO, joined in 2013.\n";
    let (_t, store) = store_with(&[("https://nvidia.test/about", body)]);
    let out = run(&store, &args(&["doc:1"], "people")).unwrap();
    let items = out.data["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[1]["name"], "Colette Kress");
    assert_eq!(items[1]["role"], "CFO");
    assert!(out.hint.unwrap().contains("heuristic"));
}

#[test]
fn prices_real_world_layouts() {
    let p = prices("$ 4 USD per user/month");
    assert_eq!(
        (p[0].value, p[0].per, p[0].period),
        (4.0, Some("user"), Some("month"))
    );
    assert_eq!(p[0].raw, "$ 4 USD per user/month");
    let p = prices("storage fees at $0.07/GB per month and $0.00595/hr");
    assert_eq!(
        (p[0].value, p[0].per, p[0].period),
        (0.07, Some("GB"), Some("month"))
    );
    assert_eq!((p[1].value, p[1].period), (0.00595, Some("hour")));
}

#[test]
fn people_and_plain_edge_cases() {
    let h = people::find_people("Tench Coxe (former managing director of Sutter Hill Ventures)");
    assert_eq!(h[0].name, "Tench Coxe");
    assert_eq!(h[0].role, "managing director of Sutter Hill Ventures");
    assert_eq!(
        plain("Revenue[1] grew[a] fast[citation needed]"),
        "Revenue grew fast"
    );
}

#[test]
fn plain_unescapes_markdown_and_values_are_rounded() {
    assert_eq!(
        plain("as of 2026[\\[update\\]](https://w.test/x) shipped.\\[15\\] a\\_b \\*c\\*"),
        "as of 2026 shipped. a_b *c*"
    );
    assert_eq!(num_value(68.1 * 1e9), json!(68_100_000_000i64));
    assert_eq!(num_value(0.00595), json!(0.00595));
    assert_eq!(num_value(1.0 / 3.0), json!(0.333333333333));
}

#[test]
fn price_ranges_keep_low_end_and_units() {
    let p = prices("Mid tier $12–19/user/mo");
    assert_eq!(
        (p[0].value, p[0].per, p[0].period),
        (12.0, Some("user"), Some("month"))
    );
    assert_eq!(p[0].raw, "$12–19/user/mo");
}

#[test]
fn prices_decimal_comma_and_cjk_magnitude() {
    let p = prices("EU €9,99, list $1,299 and JP ¥1.2萬 today");
    let got: Vec<(f64, &str)> = p.iter().map(|h| (h.value, h.raw.as_str())).collect();
    assert_eq!(
        got,
        [(9.99, "€9,99"), (1299.0, "$1,299"), (12000.0, "¥1.2萬")]
    );
    let n = kinds::find_numbers("disk 500 GiB, bandwidth 2 TB", None);
    assert_eq!(n.len(), 2, "{n:?}");
}
