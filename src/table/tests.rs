use super::parse::{md_tables, parse_csv, parse_json, split_row};
use super::query::{parse_where, run, Op, Query};
use super::value::{cell_str, infer_type, parse_num};
use super::*;

fn t(cols: &[&str], rows: &[&[&str]]) -> Table {
    Table {
        id: 0,
        title: "t".into(),
        source: "test".into(),
        columns: cols.iter().map(|s| s.to_string()).collect(),
        rows: rows
            .iter()
            .map(|r| r.iter().map(|c| json!(c)).collect())
            .collect(),
        created: String::new(),
    }
}

fn plans() -> Table {
    t(
        &["Plan", "Price", "Seats", "Launched", "Vendor"],
        &[
            &["Basic", "$9/mo", "1", "2024-01-15", "Acme"],
            &["Pro", "$1,299/yr", "10", "March 3, 2025", "Acme"],
            &[
                "Team",
                "US$ 49 per user/month",
                "25",
                "2023年5月1日",
                "Beta",
            ],
            &["Free", "$0", "1", "2022-06-01", "Beta"],
            &["Enterprise", "Contact us", "", "", "Gamma"],
        ],
    )
}

fn col(r: &Table, name: &str) -> Vec<String> {
    let i = r.columns.iter().position(|c| c == name).unwrap();
    r.rows.iter().map(|row| cell_str(&row[i])).collect()
}

#[test]
fn currency_and_number_parsing() {
    let n = |s: &str| parse_num(s).map(|n| (n.value, n.currency));
    assert_eq!(n("$1,299/mo"), Some((1299.0, Some("USD".into()))));
    assert_eq!(n("1,200 USD"), Some((1200.0, Some("USD".into()))));
    assert_eq!(n("NT$1,990"), Some((1990.0, Some("TWD".into()))));
    assert_eq!(n("€9,99"), Some((9.99, Some("EUR".into()))));
    assert_eq!(n("US$ 20 per user/month"), Some((20.0, Some("USD".into()))));
    assert_eq!(n("-5"), Some((-5.0, None)));
    assert_eq!(n("1.5M"), Some((1.5e6, None)));
    assert_eq!(n("12%").map(|x| x.0), Some(12.0));
    assert!(parse_num("12%").unwrap().percent);
    assert_eq!(n("256 GB").map(|x| x.0), Some(256.0));
    for not in ["Contact us", "$10 - $20", "2026-10-08", "v2", "Plan 3", ""] {
        assert_eq!(n(not), None, "{not}");
    }
}

#[test]
fn column_types_are_inferred() {
    let p = plans();
    let types: Vec<String> = p
        .column_types()
        .iter()
        .map(|c| c["type"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(types, vec!["text", "currency", "number", "date", "text"]);
    let cells = [json!("$5"), json!("1,200 USD"), json!("-")];
    assert_eq!(infer_type(cells.iter()), "currency");
}

#[test]
fn csv_tsv_json_and_markdown_parsing() {
    let csv = "\u{feff}Name,Note,Price\nA,\"has, comma\",1\nB,\"multi\nline \"\"q\"\"\",2\n\n";
    let p = parse_csv(csv, None).unwrap();
    assert_eq!(p.columns, vec!["Name", "Note", "Price"]);
    assert_eq!(p.rows[0][1], json!("has, comma"));
    assert_eq!(p.rows[1][1], json!("multi\nline \"q\""));
    assert_eq!(p.rows.len(), 2);

    let p = parse_csv("a\tb\n1\t2\n", None).unwrap();
    assert_eq!(p.columns, vec!["a", "b"]);
    let p = parse_csv("a;b\n1;2\n", None).unwrap();
    assert_eq!(p.rows[0][1], json!("2"));
    let p = parse_csv("a,a,\n1,2,3,4\n", None).unwrap();
    assert_eq!(p.columns, vec!["a", "a_2", "column3", "column4"]);
    assert!(parse_csv("a\n\"open", None).is_err());

    let p =
        parse_json(r#"[{"name":"A","price":5},{"name":"B","tags":["x"],"price":null}]"#).unwrap();
    assert_eq!(p.columns, vec!["name", "price", "tags"]);
    assert_eq!(p.rows[0][1], json!(5));
    assert_eq!(p.rows[1][2], json!("[\"x\"]"));
    let p = parse_json(r#"{"ok":true,"data":{"results":[{"title":"T","url":"u"}]}}"#).unwrap();
    assert_eq!(p.columns, vec!["title", "url"]);
    let p = parse_json(r#"[["h1","h2"],[1,2]]"#).unwrap();
    assert_eq!(p.columns, vec!["h1", "h2"]);
    assert!(parse_json("{}").is_err());

    let md = "text\n\n| A | B \\| C |\n|:--|--:|\n| 1 | 2 |\n| 3 |\n\n```\n| x | y |\n|---|---|\n```\nno | table here\n";
    let ts = md_tables(md);
    assert_eq!(ts.len(), 1);
    assert_eq!(ts[0].line, 3);
    assert_eq!(ts[0].columns, vec!["A", "B | C"]);
    assert_eq!(ts[0].rows, vec![vec!["1", "2"], vec!["3", ""]]);
    assert_eq!(split_row("a|b"), vec!["a", "b"]);
}

#[test]
fn where_parsing_and_errors() {
    assert_eq!(
        parse_where("Price < 100").unwrap(),
        ("Price".into(), Op::Lt, "100".into())
    );
    assert_eq!(parse_where("Price>=1,000").unwrap().1, Op::Ge);
    assert_eq!(
        parse_where("Plan != 'Pro'").unwrap(),
        ("Plan".into(), Op::Ne, "Pro".into())
    );
    assert_eq!(
        parse_where("Monthly Price <= $20").unwrap().0,
        "Monthly Price"
    );
    assert_eq!(parse_where("Plan contains pro").unwrap().1, Op::Contains);
    assert_eq!(parse_where("Plan starts with P").unwrap().1, Op::StartsWith);
    assert_eq!(parse_where("Note = \"\"").unwrap().2, "");
    for bad in [
        "Price 100",
        "< 5",
        "Price <",
        "Plan not contains x",
        "Price ! 5",
    ] {
        let e = parse_where(bad).unwrap_err();
        assert_eq!(e.code, "bad_where", "{bad}");
        assert!(e.hint.contains("--where \"Price < 100\""));
    }
}

#[test]
fn where_filters_auto_type_values() {
    let q = |w: &[&str]| {
        run(
            &plans(),
            &Query {
                wheres: w.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
        )
        .unwrap()
    };
    assert_eq!(
        col(&q(&["Price < 50"]), "Plan"),
        vec!["Basic", "Team", "Free"]
    );
    assert_eq!(col(&q(&["price >= $1,000"]), "Plan"), vec!["Pro"]);
    assert_eq!(col(&q(&["Seats = 1"]), "Plan"), vec!["Basic", "Free"]);
    assert_eq!(
        col(&q(&["Launched > 2024-01-01"]), "Plan"),
        vec!["Basic", "Pro"]
    );
    assert_eq!(
        col(&q(&["vendor = acme", "Price < 100"]), "Plan"),
        vec!["Basic"]
    );
    assert_eq!(col(&q(&["Plan contains EAM"]), "Plan"), vec!["Team"]);
    assert_eq!(col(&q(&["Plan startswith f"]), "Plan"), vec!["Free"]);
    assert_eq!(
        col(&q(&["Vendor != Beta"]), "Plan"),
        vec!["Basic", "Pro", "Enterprise"]
    );
}

#[test]
fn sort_select_group_and_agg() {
    let p = plans();
    let q = |q: Query| run(&p, &q).unwrap();
    // Numeric sort; text cells like "Contact us" go last.
    let r = q(Query {
        sorts: vec!["-Price".into()],
        ..Default::default()
    });
    assert_eq!(
        col(&r, "Plan"),
        vec!["Pro", "Team", "Basic", "Free", "Enterprise"]
    );
    let r = q(Query {
        sorts: vec!["Price".into()],
        ..Default::default()
    });
    assert_eq!(
        col(&r, "Plan"),
        vec!["Free", "Basic", "Team", "Pro", "Enterprise"]
    );
    // Dates sort by calendar, mixed formats included; blanks last.
    let r = q(Query {
        sorts: vec!["Launched desc".into()],
        ..Default::default()
    });
    assert_eq!(
        col(&r, "Plan"),
        vec!["Pro", "Basic", "Team", "Free", "Enterprise"]
    );
    // Multiple keys: Vendor asc then Seats desc.
    let r = q(Query {
        sorts: vec!["vendor".into(), "-seats".into()],
        ..Default::default()
    });
    assert_eq!(
        col(&r, "Plan"),
        vec!["Pro", "Basic", "Team", "Free", "Enterprise"]
    );
    // Select keeps the requested order.
    let r = q(Query {
        select: Some("price, plan".into()),
        ..Default::default()
    });
    assert_eq!(r.columns, vec!["Price", "Plan"]);
    // Group + aggregates, then sort by an aggregate column.
    let r = q(Query {
        group_by: Some("Vendor".into()),
        agg: Some("sum:Seats,avg:Price,count,max:Price".into()),
        sorts: vec!["-sum_Seats".into()],
        ..Default::default()
    });
    assert_eq!(
        r.columns,
        vec!["Vendor", "sum_Seats", "avg_Price", "count", "max_Price"]
    );
    assert_eq!(
        r.rows[0],
        vec![json!("Beta"), json!(26), json!(24.5), json!(2), json!(49)]
    );
    assert_eq!(
        r.rows[1],
        vec![json!("Acme"), json!(11), json!(654), json!(2), json!(1299)]
    );
    assert_eq!(r.rows[2][2], Value::Null, "avg of no numbers is null");
    // Agg without group-by gives one row; group-by alone counts.
    let r = q(Query {
        agg: Some("count,min:Price".into()),
        ..Default::default()
    });
    assert_eq!(r.rows, vec![vec![json!(5), json!(0)]]);
    let r = q(Query {
        group_by: Some("Vendor".into()),
        ..Default::default()
    });
    assert_eq!(r.columns, vec!["Vendor", "count"]);
}

#[test]
fn column_name_errors_suggest_a_fix() {
    let p = plans();
    let e = run(
        &p,
        &Query {
            wheres: vec!["Prise < 10".into()],
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(e.code, "unknown_column");
    assert!(
        e.message.contains("Plan, Price, Seats, Launched, Vendor"),
        "{}",
        e.message
    );
    assert!(
        e.hint
            .contains("Did you mean `Price`? Try: --where \"Price < 10\""),
        "{}",
        e.hint
    );
    let e = run(
        &p,
        &Query {
            sorts: vec!["-Seat".into()],
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(e.hint.contains("--sort -Seats"), "{}", e.hint);
    let e = run(
        &p,
        &Query {
            select: Some("Plan,Vendr".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(e.hint.contains("--select Plan,Vendor"), "{}", e.hint);
    let e = run(
        &p,
        &Query {
            agg: Some("median:Price".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(e.code, "bad_agg");
    let e = run(
        &p,
        &Query {
            agg: Some("sum".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(e.message.contains("sum:Price"));
    // Case and punctuation are ignored when matching columns.
    let t2 = t(&["Monthly Price"], &[&["$5"]]);
    assert!(run(
        &t2,
        &Query {
            sorts: vec!["monthly_price".into()],
            ..Default::default()
        }
    )
    .is_ok());
}

fn store() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let s = Store::new(tmp.path().join("home"));
    (tmp, s)
}

#[test]
fn import_show_query_save_and_handles() {
    let (tmp, st) = store();
    let csv = tmp.path().join("p.csv");
    std::fs::write(&csv, "Plan,Price\nBasic,$9/mo\nPro,$29/mo\nMax,$99/mo\n").unwrap();
    let f = csv.to_str().unwrap();
    let out = run_import(&st, f).unwrap();
    assert_eq!(out.data["table"], "tbl:1");
    assert_eq!(
        out.data["columns"][1],
        json!({"name":"Price","type":"currency"})
    );
    assert!(out.hint.unwrap().contains("--sort -Price"));
    // Importing the same file again reuses the handle.
    assert_eq!(run_import(&st, f).unwrap().data["table"], "tbl:1");

    let show = run_show(&st, "TBL:1", 2).unwrap();
    assert_eq!(show.data["row_count"], 3);
    assert_eq!(show.data["truncated"], true);
    assert_eq!(show.data["rows"].as_array().unwrap().len(), 2);
    // Files work directly too.
    assert_eq!(run_show(&st, f, 20).unwrap().data["row_count"], 3);

    let qa = QueryArgs {
        source: "tbl:1".into(),
        query: Query {
            wheres: vec!["Price > 10".into()],
            sorts: vec!["-Price".into()],
            ..Default::default()
        },
        limit: Some(1),
        save: true,
    };
    let out = run_query(&st, &qa).unwrap();
    assert_eq!(out.data["row_count"], 2);
    assert_eq!(out.data["returned"], 1);
    assert_eq!(out.data["truncated"], true);
    assert_eq!(out.data["table"], "tbl:2");
    let saved = load(&st, "tbl:2").unwrap();
    assert_eq!(
        saved.rows.len(),
        1,
        "explicit --limit applies to the saved table"
    );
    assert_eq!(saved.source, "tbl:1");

    assert_eq!(load(&st, "tbl:9").unwrap_err().code, "table_not_found");
    assert_eq!(load(&st, "tbl:x").unwrap_err().code, "bad_handle");
    let e = load(&st, "doc:1").unwrap_err();
    assert!(e.hint.contains("extract doc:1 --kind tables"));
    let none = run_query(
        &st,
        &QueryArgs {
            source: "tbl:1".into(),
            query: Query {
                wheres: vec!["Plan = nope".into()],
                ..Default::default()
            },
            limit: None,
            save: false,
        },
    )
    .unwrap();
    assert!(none.hint.unwrap().contains("No rows matched"));
}

#[test]
fn import_markdown_with_several_tables_and_json() {
    let (tmp, st) = store();
    let md = tmp.path().join("r.md");
    std::fs::write(&md, "# R\n\n| **A** | B |\n|---|---|\n| [x](https://x.test) | 1 |\n\n| C | D |\n|---|---|\n| 2 | 3 |\n").unwrap();
    let out = run_import(&st, md.to_str().unwrap()).unwrap();
    assert_eq!(out.data["tables_in_file"], 2);
    assert_eq!(out.data["rows"][0], json!({"A":"x","B":"1"}));
    assert!(out.hint.unwrap().contains("--kind tables"));
    let js = tmp.path().join("d.json");
    std::fs::write(&js, r#"[{"n":"a","v":2},{"n":"b","v":1}]"#).unwrap();
    assert_eq!(
        run_import(&st, js.to_str().unwrap()).unwrap().data["columns"][1]["type"],
        "number"
    );
    let bad = tmp.path().join("x.md");
    std::fs::write(&bad, "no tables").unwrap();
    assert_eq!(
        run_import(&st, bad.to_str().unwrap()).unwrap_err().code,
        "bad_table"
    );
}

#[test]
fn export_preview_apply_and_formats() {
    let (tmp, st) = store();
    let mut tb = t(&["Name", "Note"], &[&["A", "x, \"y\""], &["B|C", "z"]]);
    tb.source = "s".into();
    let id = save(&st, tb).unwrap();
    let h = format!("tbl:{id}");
    let out_csv = tmp.path().join("o.csv");
    let p = out_csv.to_str().unwrap();
    let prev = run_export(&st, &h, p, false).unwrap();
    assert_eq!(prev.data["applied"], false);
    assert!(!out_csv.exists());
    assert!(prev.data["diff"]
        .as_str()
        .unwrap()
        .contains("+A,\"x, \"\"y\"\"\""));
    run_export(&st, &h, p, true).unwrap();
    assert_eq!(
        std::fs::read_to_string(&out_csv).unwrap(),
        "Name,Note\nA,\"x, \"\"y\"\"\"\nB|C,z\n"
    );
    // Round-trip: the exported CSV parses back to the same rows.
    let back = load(&st, p).unwrap();
    assert_eq!(back.rows, load(&st, &h).unwrap().rows);

    let md = tmp.path().join("o.md");
    run_export(&st, &h, md.to_str().unwrap(), true).unwrap();
    assert!(std::fs::read_to_string(&md)
        .unwrap()
        .contains("| B\\|C | z |"));
    let js = tmp.path().join("o.json");
    run_export(&st, &h, js.to_str().unwrap(), true).unwrap();
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&js).unwrap()).unwrap();
    assert_eq!(v[1]["Name"], "B|C");
    let tsv = tmp.path().join("o.tsv");
    run_export(&st, &h, tsv.to_str().unwrap(), true).unwrap();
    assert!(std::fs::read_to_string(&tsv)
        .unwrap()
        .starts_with("Name\tNote\n"));
    assert_eq!(
        run_export(&st, &h, "o.xlsx", false).unwrap_err().code,
        "bad_args"
    );
}

#[test]
fn csv_envelope_output() {
    let data = json!({"columns":["a","b"],"rows":[{"a":"1,5","b":2}]});
    assert_eq!(envelope_csv(&data).unwrap(), "a,b\n\"1,5\",2\n");
    let show = json!({"columns":[{"name":"a","type":"text"}],"rows":[{"a":"x"}]});
    assert_eq!(envelope_csv(&show).unwrap(), "a\nx\n");
    assert!(envelope_csv(&json!({"rows": 3})).is_none());
}

#[test]
fn hints_quote_arguments_safely() {
    assert_eq!(shell_arg("-Price"), "-Price");
    assert_eq!(shell_arg("-Revenue ($B) USD"), "'-Revenue ($B) USD'");
    assert_eq!(shell_arg("Price < 10"), "\"Price < 10\"");
    let tb = t(&["Revenue ($B)"], &[&["$1"]]);
    let e = run(
        &tb,
        &Query {
            sorts: vec!["-Revenue".into(), "-Revnue ($B)".into()],
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(e.hint.contains("--sort '-Revenue ($B)'"), "{}", e.hint);
}

#[test]
fn data_sizes_sort_and_filter_in_g_units() {
    assert_eq!(parse_num("512 MiB").unwrap().value, 0.5);
    assert_eq!(parse_num("4 GiB").unwrap().value, 4.0);
    assert_eq!(parse_num("1 TB").unwrap().value, 1000.0);
    assert_eq!(parse_num("1,000 GiB").unwrap().value, 1000.0);
    assert_eq!(parse_num("12 users").unwrap().value, 12.0);
    let p = t(
        &["Memory"],
        &[&["4 GiB"], &["512 MiB"], &["2 GiB"], &["1 TiB"]],
    );
    let q = Query {
        sorts: vec!["-Memory".into()],
        wheres: vec!["Memory >= 2".into()],
        ..Default::default()
    };
    let out = run(&p, &q).unwrap();
    let mem: Vec<String> = out.rows.iter().map(|r| cell_str(&r[0])).collect();
    assert_eq!(mem, ["1 TiB", "4 GiB", "2 GiB"]);
}
