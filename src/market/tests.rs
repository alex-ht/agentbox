use super::*;
use crate::testutil::{http, serve};

fn eps(gamma: &str, clob: &str) -> Endpoints {
    Endpoints {
        gamma: gamma.to_string(),
        clob: clob.to_string(),
    }
}

fn ok(body: &str) -> String {
    http("200 OK", "application/json", body)
}

fn not_found() -> String {
    http(
        "404 Not Found",
        "application/json",
        r#"{"type":"not found error","error":"slug not found"}"#,
    )
}

fn market(id: &str, q: &str, label: &str, yes: f64, closed: bool) -> String {
    format!(
        r#"{{"id":"{id}","question":"{q}","slug":"m-{id}","groupItemTitle":"{label}","outcomes":"[\"Yes\", \"No\"]","outcomePrices":"[\"{yes}\", \"{no}\"]","clobTokenIds":"[\"tok{id}yes\", \"tok{id}no\"]","volumeNum":1234.5,"volume24hr":99.4,"liquidityNum":50,"endDate":"2026-11-04T00:00:00Z","closed":{closed},"oneDayPriceChange":-0.015,"oneWeekPriceChange":0.02,"lastTradePrice":0.61}}"#,
        no = ((1.0 - yes) * 1000.0).round() / 1000.0
    )
}

fn placeholder() -> String {
    r#"{"id":"9","question":"Will Person X win?","slug":"m-9","groupItemTitle":"Person X","outcomes":"[\"Yes\", \"No\"]","clobTokenIds":"[\"a\",\"b\"]","active":false,"closed":false}"#.to_string()
}

fn event(
    id: &str,
    title: &str,
    desc: &str,
    volume: f64,
    closed: bool,
    markets: &[String],
) -> String {
    format!(
        r#"{{"id":"{id}","slug":"ev-{id}","title":"{title}","description":"{desc}","volume":{volume},"volume24hr":10.0,"liquidity":5.0,"endDate":"2026-12-0{id}T00:00:00Z","startDate":"2026-01-0{id}T00:00:00Z","closed":{closed},"tags":[{{"label":"Politics","slug":"politics"}}],"markets":[{}]}}"#,
        markets.join(",")
    )
}

#[test]
fn parsing_helpers() {
    assert_eq!(str_list(Some(&json!("[\"Yes\", \"No\"]"))), ["Yes", "No"]);
    assert_eq!(str_list(Some(&json!(["a", 1]))), ["a", "1"]);
    assert!(str_list(None).is_empty());
    assert_eq!(fnum(Some(&json!("12.5"))), Some(12.5));
    assert_eq!(fnum(Some(&json!(3))), Some(3.0));
    let m: Value = serde_json::from_str(&market("1", "Q?", "", 0.535, false)).unwrap();
    assert_eq!(
        outcomes(&m),
        vec![("Yes".to_string(), 53.5), ("No".to_string(), 46.5)]
    );
    let v = market_view(&m, Some("ev"), true);
    assert_eq!(v["odds"], "Yes 53.5% · No 46.5%");
    assert_eq!(v["yes_pct"], json!(53.5));
    assert_eq!(v["change_1d_pts"], json!(-1.5));
    assert_eq!(v["last_trade_pct"], json!(61));
    assert_eq!(v["volume"], json!(1235));
    assert_eq!(v["url"], "https://polymarket.com/event/ev/m-1");
    assert!(!v.contains_key("label"), "empty groupItemTitle is dropped");
    let sports: Value = serde_json::from_str(
        r#"{"outcomes":["Lakers","Celtics"],"outcomePrices":["0.42","0.58"],"slug":"g"}"#,
    )
    .unwrap();
    let v = market_view(&sports, None, false);
    assert_eq!(
        (v["leader"].clone(), v["leader_pct"].clone()),
        (json!("Celtics"), json!(58))
    );
    assert!(!v.contains_key("yes_pct"));
    let p: Value = serde_json::from_str(&placeholder()).unwrap();
    assert!(!is_real(&p));
}

#[test]
fn targets() {
    let slug = |e: &str| Target::Slug {
        event: e.into(),
        market: None,
    };
    assert_eq!(parse_target("fed-decision"), Some(slug("fed-decision")));
    assert_eq!(parse_target("12345"), Some(Target::Id("12345".into())));
    assert_eq!(
        parse_target("https://polymarket.com/event/fed-decision?tid=1"),
        Some(slug("fed-decision"))
    );
    assert_eq!(
        parse_target("https://polymarket.com/event/ev/m-1/"),
        Some(Target::Slug {
            event: "ev".into(),
            market: Some("m-1".into())
        })
    );
    assert_eq!(
        parse_target("polymarket.com/market/m-1"),
        Some(Target::Slug {
            event: String::new(),
            market: Some("m-1".into())
        })
    );
    assert_eq!(parse_target("  "), None);
    assert_eq!(parse_target("a/b/c"), None);
}

#[test]
fn search_filters_keywords_dedupes_and_sorts() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path());
    let m = |id: &str, y: f64| market(id, "Will X win?", "X", y, false);
    let page1 = format!(
        r#"{{"events":[{},{},{}],"pagination":{{"hasMore":true,"totalResults":4}}}}"#,
        event(
            "1",
            "Small Election Market",
            "",
            100.0,
            false,
            &[m("11", 0.2)]
        ),
        event(
            "2",
            "Unrelated sports thing",
            "nothing here",
            9e6,
            false,
            &[m("21", 0.5)]
        ),
        event(
            "3",
            "Big race",
            "Who wins the ELECTION?",
            5000.0,
            false,
            &[
                m("31", 0.1),
                m("32", 0.7),
                market("33", "Old", "Old", 1.0, true),
                placeholder()
            ]
        ),
    );
    let page2 = format!(
        r#"{{"events":[{},{}],"pagination":{{"hasMore":false,"totalResults":4}}}}"#,
        event(
            "1",
            "Small Election Market",
            "",
            100.0,
            false,
            &[m("11", 0.2)]
        ),
        event("4", "Resolved election", "", 7000.0, true, &[m("41", 1.0)]),
    );
    let (base, rx) = serve(vec![ok(&page1), ok(&page2)]);
    let ep = eps(&base, "http://127.0.0.1:9");
    let out = run_search(
        &store,
        &Ctx::new(&ep),
        &SearchArgs {
            query: "Election".into(),
            limit: 10,
            active: false,
            closed: false,
            sort: "volume".into(),
            tag: Some("politics".into()),
            save_table: true,
        },
    )
    .unwrap();
    let l1 = rx.recv().unwrap().line;
    assert!(
        l1.contains("/public-search?q=Election&limit_per_type=25&page=1"),
        "{l1}"
    );
    assert!(
        l1.contains("&events_status=active")
            && l1.contains("&sort=volume&ascending=false")
            && l1.contains("&events_tag=politics"),
        "{l1}"
    );
    assert!(rx.recv().unwrap().line.contains("page=2"));
    let d = &out.data;
    assert_eq!(d["via"], "public-search");
    assert_eq!(d["match"], "keyword");
    assert_eq!(d["more_available"], false);
    let titles: Vec<&str> = d["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        ["Big race", "Small Election Market"],
        "keyword filter, dedupe, active only, by volume"
    );
    let big = &d["events"][0];
    assert_eq!(big["url"], "https://polymarket.com/event/ev-3");
    assert_eq!(big["markets_total"], 3, "placeholder dropped");
    assert_eq!(big["markets_open"], 2);
    assert_eq!(big["markets"][0]["label"], "X");
    assert_eq!(big["markets"][0]["yes_pct"], json!(70));
    assert_eq!(
        big["markets"].as_array().unwrap().len(),
        2,
        "settled sub-market hidden in lists"
    );
    assert_eq!(big["tags"], "politics");
    let tbl = d["table"].as_str().unwrap();
    let t = crate::table::load(&store, tbl).unwrap();
    assert_eq!(t.columns, TABLE_COLUMNS);
    assert_eq!(t.rows.len(), 3);
    assert_eq!(t.rows[0][3], json!(70));
    assert_eq!(t.rows[0][9], "https://polymarket.com/event/ev-3/m-32");
    let hint = out.hint.unwrap();
    assert!(
        hint.contains("market get ev-3") && hint.contains(tbl),
        "{hint}"
    );
}

#[test]
fn search_fuzzy_closed_and_sorting() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path());
    let body = format!(
        r#"{{"events":[{},{}],"pagination":{{"hasMore":false}}}}"#,
        event(
            "1",
            "Alpha",
            "",
            1.0,
            true,
            &[market("11", "Q", "", 0.0, true)]
        ),
        event(
            "2",
            "Beta",
            "",
            2.0,
            true,
            &[market("21", "Q", "", 1.0, true)]
        ),
    );
    let (base, rx) = serve(vec![ok(&body)]);
    let ep = eps(&base, "http://127.0.0.1:9");
    let out = run_search(
        &store,
        &Ctx::new(&ep),
        &SearchArgs {
            query: "gamma ray".into(),
            limit: 10,
            active: false,
            closed: true,
            sort: "end".into(),
            tag: None,
            save_table: false,
        },
    )
    .unwrap();
    let line = rx.recv().unwrap().line;
    assert!(
        line.contains("events_status=closed") && !line.contains("sort="),
        "{line}"
    );
    assert_eq!(out.data["match"], "fuzzy");
    assert_eq!(out.data["status"], "closed");
    assert_eq!(out.data["events"][0]["title"], "Alpha", "soonest end first");
    assert!(out.hint.unwrap().contains("closest matches"));
}

#[test]
fn search_falls_back_to_scanning_events() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path());
    let events = format!(
        "[{},{}]",
        event(
            "1",
            "Fed rate cut in December?",
            "",
            10.0,
            false,
            &[market("11", "Cut?", "", 0.3, false)]
        ),
        event(
            "2",
            "Other",
            "",
            20.0,
            false,
            &[market("21", "Q", "", 0.3, false)]
        ),
    );
    let (base, rx) = serve(vec![
        http("500 Internal Server Error", "text/plain", "oops"),
        ok(&events),
    ]);
    let ep = eps(&base, "http://127.0.0.1:9");
    let out = run_search(
        &store,
        &Ctx::new(&ep),
        &SearchArgs {
            query: "fed rate".into(),
            limit: 10,
            active: true,
            closed: false,
            sort: "volume".into(),
            tag: None,
            save_table: false,
        },
    )
    .unwrap();
    rx.recv().unwrap();
    let scan = rx.recv().unwrap().line;
    assert!(
        scan.contains(
            "/events?limit=100&offset=0&order=volume24hr&ascending=false&active=true&closed=false"
        ),
        "{scan}"
    );
    assert_eq!(out.data["via"], "events-scan");
    assert_eq!(out.data["returned"], 1);
    assert_eq!(out.data["events"][0]["title"], "Fed rate cut in December?");
}

#[test]
fn search_no_results_and_bad_args() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path());
    let (base, _rx) = serve(vec![ok(
        r#"{"events":null,"pagination":{"hasMore":false}}"#,
    )]);
    let ep = eps(&base, "http://127.0.0.1:9");
    let args = SearchArgs {
        query: "zzzz".into(),
        limit: 10,
        active: false,
        closed: false,
        sort: "volume".into(),
        tag: None,
        save_table: false,
    };
    let e = run_search(&store, &Ctx::new(&ep), &args).unwrap_err();
    assert_eq!(e.code, "no_results");
    assert!(e.hint.contains("--closed"));
    let e = run_search(
        &store,
        &Ctx::new(&ep),
        &SearchArgs {
            query: " ".into(),
            ..args
        },
    )
    .unwrap_err();
    assert_eq!(e.code, "bad_args");
}

#[test]
fn trending_requests_by_24h_volume() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path());
    let events = format!(
        "[{}]",
        event(
            "1",
            "Hot",
            "",
            10.0,
            false,
            &[
                market("11", "Q", "", 0.3, false),
                market("12", "Q2", "", 0.6, true)
            ]
        )
    );
    let (base, rx) = serve(vec![ok(&events), ok("[]")]);
    let ep = eps(&base, "http://127.0.0.1:9");
    let out = run_trending(
        &store,
        &Ctx::new(&ep),
        &TrendingArgs {
            limit: 5,
            tag: Some("crypto".into()),
            save_table: true,
        },
    )
    .unwrap();
    assert_eq!(
        rx.recv().unwrap().line,
        "GET /events?active=true&closed=false&order=volume24hr&ascending=false&limit=5&tag_slug=crypto HTTP/1.1"
    );
    assert_eq!(out.data["events"][0]["slug"], "ev-1");
    let t = crate::table::load(&store, out.data["table"].as_str().unwrap()).unwrap();
    assert_eq!(
        t.rows.len(),
        1,
        "closed sub-markets of live events are skipped"
    );
    let e = run_trending(
        &store,
        &Ctx::new(&ep),
        &TrendingArgs {
            limit: 5,
            tag: Some("nope".into()),
            save_table: false,
        },
    )
    .unwrap_err();
    assert_eq!(e.code, "no_results");
    assert!(e.hint.contains("politics"));
}

#[test]
fn get_event_market_and_missing() {
    let ev = event(
        "3",
        "Big race",
        "Rules text",
        5000.0,
        false,
        &[
            market("31", "A?", "A", 0.1, false),
            market("32", "B?", "B", 0.7, false),
            placeholder(),
        ],
    );
    let (base, rx) = serve(vec![
        ok(&ev),
        not_found(),
        ok(&market("31", "A?", "A", 0.1, false)),
        not_found(),
        not_found(),
    ]);
    let ep = eps(&base, "http://127.0.0.1:9");
    let c = Ctx::new(&ep);
    let out = run_get(&c, "https://polymarket.com/event/ev-3", 1).unwrap();
    assert_eq!(rx.recv().unwrap().line, "GET /events/slug/ev-3 HTTP/1.1");
    let e = &out.data["event"];
    assert_eq!(out.data["kind"], "event");
    assert_eq!(e["description"], "Rules text");
    assert_eq!(e["markets"][0]["label"], "B");
    assert_eq!(
        e["markets"][0]["outcomes"][0],
        json!({"outcome": "Yes", "pct": 70})
    );
    assert_eq!(e["markets_omitted"], 1);
    let hint = out.hint.unwrap();
    assert!(
        hint.contains("raise --limit") && hint.contains("market history m-32"),
        "{hint}"
    );

    let out = run_get(&c, "m-31", 20).unwrap();
    assert_eq!(rx.recv().unwrap().line, "GET /events/slug/m-31 HTTP/1.1");
    assert_eq!(rx.recv().unwrap().line, "GET /markets/slug/m-31 HTTP/1.1");
    assert_eq!(out.data["kind"], "market");
    assert_eq!(
        out.data["market"]["url"],
        "https://polymarket.com/market/m-31"
    );

    let e = run_get(&c, "nope", 20).unwrap_err();
    assert_eq!(e.code, "not_found");
    assert!(e.hint.contains("market search"));
}

#[test]
fn history_series_summary_and_save() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path());
    let (gbase, grx) = serve(vec![ok(&market("31", "Will A win?", "A", 0.62, false))]);
    let hist = r#"{"history":[{"t":1791000000,"p":0.5},{"t":1791086400,"p":0.71},{"t":1791172800,"p":0.45},{"t":1791259200,"p":0.62}]}"#;
    let (cbase, crx) = serve(vec![ok(hist)]);
    let ep = eps(&gbase, &cbase);
    let out = run_history(
        &store,
        &Ctx::new(&ep),
        &HistoryArgs {
            target: "31".into(),
            interval: "1w".into(),
            save: true,
        },
    )
    .unwrap();
    assert_eq!(grx.recv().unwrap().line, "GET /markets/31 HTTP/1.1");
    assert_eq!(
        crx.recv().unwrap().line,
        "GET /prices-history?market=tok31yes&interval=1w&fidelity=360 HTTP/1.1"
    );
    let d = &out.data;
    assert_eq!(d["outcome"], "Yes");
    assert_eq!(d["columns"], json!(["time", "yes_pct"]));
    let s = &d["summary"];
    assert_eq!(s["start_pct"], json!(50));
    assert_eq!(s["end_pct"], json!(62));
    assert_eq!(s["change_pts"], json!(12));
    assert_eq!(s["high_pct"], json!(71));
    assert_eq!(s["low_pct"], json!(45));
    assert_eq!(s["start_time"], "2026-10-03T04:00:00Z");
    assert_eq!(
        d["rows"][1],
        json!({"time": "2026-10-04T04:00:00Z", "yes_pct": 71})
    );
    let t = crate::table::load(&store, d["table"].as_str().unwrap()).unwrap();
    assert_eq!(t.rows.len(), 4);
}

#[test]
fn history_needs_one_market_and_data() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path());
    let ev = event(
        "3",
        "Big race",
        "",
        1.0,
        false,
        &[
            market("31", "A?", "A", 0.1, false),
            market("32", "B?", "B", 0.7, false),
        ],
    );
    let single = event(
        "4",
        "Solo",
        "",
        1.0,
        false,
        &[market("41", "Solo?", "", 0.4, false)],
    );
    let (gbase, _g) = serve(vec![ok(&ev), ok(&single)]);
    let (cbase, _c) = serve(vec![ok(r#"{"history":[]}"#)]);
    let ep = eps(&gbase, &cbase);
    let c = Ctx::new(&ep);
    let args = |t: &str| HistoryArgs {
        target: t.into(),
        interval: "max".into(),
        save: false,
    };
    let e = run_history(&store, &c, &args("ev-3")).unwrap_err();
    assert_eq!(e.code, "ambiguous_market");
    assert!(e.message.contains("B 70% → m-32"), "{}", e.message);
    assert!(e.hint.contains("market history m-32"));
    let e = run_history(&store, &c, &args("ev-4")).unwrap_err();
    assert_eq!(e.code, "no_data");
    assert!(e.hint.contains("--interval max"));
}

#[test]
fn network_failures_mention_dns_filters() {
    let e = fail_error(
        "Polymarket",
        Fail::Net {
            message: "dns error: failed to lookup address".into(),
            timeout: false,
            dns: true,
        },
    );
    assert_eq!(e.code, "dns_error");
    assert!(e.hint.contains("RPZ") && e.hint.contains("nslookup gamma-api.polymarket.com"));
    let e = fail_error(
        "Polymarket",
        Fail::Net {
            message: "tls handshake eof".into(),
            timeout: false,
            dns: false,
        },
    );
    assert_eq!(e.code, "network_error");
    assert!(e.hint.contains("DNS filter"));
    assert_eq!(
        fail_error("Polymarket", Fail::Status(429, String::new())).code,
        "rate_limited"
    );
    // A dead endpoint is reported the same way end to end.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    drop(l);
    let base = format!("http://{addr}");
    let ep = eps(&base, &base);
    let tmp = tempfile::tempdir().unwrap();
    let e = run_trending(
        &Store::new(tmp.path()),
        &Ctx::new(&ep),
        &TrendingArgs {
            limit: 3,
            tag: None,
            save_table: false,
        },
    )
    .unwrap_err();
    assert_eq!(e.code, "network_error");
    assert!(e.hint.contains("polymarket.com"));
}
