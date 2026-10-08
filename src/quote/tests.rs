use super::*;
use crate::testutil::{http, serve};

const NOW: i64 = 1_791_440_000; // 2026-10-08T05:33:20Z

fn eps(yahoo: &str, stooq: &str) -> Endpoints {
    Endpoints {
        yahoo: vec![yahoo.to_string()],
        stooq: stooq.to_string(),
    }
}

fn ctx<'a>(ep: &'a Endpoints, key: Option<&'a str>) -> Ctx<'a> {
    let mut c = Ctx::new(ep, key);
    c.now = NOW;
    c
}

fn json_ok(body: &str) -> String {
    http("200 OK", "application/json", body)
}

/// A Yahoo chart answer for one symbol.
fn chart(symbol: &str, price: f64, prev: f64, extra: &str) -> String {
    format!(
        r#"{{"chart":{{"result":[{{"meta":{{"currency":"TWD","symbol":"{symbol}","exchangeName":"TAI","fullExchangeName":"Taiwan","instrumentType":"EQUITY","regularMarketTime":1791437414,"gmtoffset":28800,"timezone":"CST","exchangeTimezoneName":"Asia/Taipei","regularMarketPrice":{price},"fiftyTwoWeekHigh":2590.0,"fiftyTwoWeekLow":1375.0,"regularMarketDayHigh":2575.0,"regularMarketDayLow":2550.0,"regularMarketVolume":22484073,"longName":"Taiwan Semiconductor Manufacturing Company Limited","chartPreviousClose":{prev},"priceHint":2,"currentTradingPeriod":{{"pre":{{"start":1791421200,"end":1791422100}},"regular":{{"start":1791422100,"end":1791438300}},"post":{{"start":1791438300,"end":1791438300}}}}{extra}}},"timestamp":[1791437414],"indicators":{{"quote":[{{"close":[{price}]}}]}}}}],"error":null}}}}"#
    )
}

const NOT_FOUND: &str = r#"{"chart":{"result":null,"error":{"code":"Not Found","description":"No data found, symbol may be delisted"}}}"#;

#[test]
fn get_parses_yahoo_meta() {
    let (base, rx) = serve(vec![json_ok(&chart("2330.TW", 2550.0, 2585.0, ""))]);
    let ep = eps(&base, "http://127.0.0.1:9");
    let out = run_get(
        &ctx(&ep, None),
        &GetArgs {
            symbols: vec!["2330.tw".into()],
            backend: "auto".into(),
        },
    )
    .unwrap();
    let req = rx.recv().unwrap();
    assert_eq!(
        req.line,
        "GET /v8/finance/chart/2330.TW?range=1d&interval=1d HTTP/1.1"
    );
    assert!(req.header("user-agent").unwrap().contains("agentbox"));
    let q = &out.data["quotes"][0];
    assert_eq!(q["symbol"], "2330.TW");
    assert_eq!(q["price"], json!(2550));
    assert_eq!(q["change"], json!(-35));
    assert_eq!(q["change_pct"], json!(-1.35));
    assert_eq!(q["previous_close"], json!(2585));
    assert_eq!(q["week52_high"], json!(2590));
    assert_eq!(q["currency"], "TWD");
    assert_eq!(q["exchange"], "Taiwan");
    assert_eq!(q["type"], "equity");
    assert_eq!(q["market_state"], "closed");
    assert_eq!(q["time"], "2026-10-08T13:30:14+08:00");
    assert_eq!(q["timezone"], "Asia/Taipei");
    assert_eq!(q["backend"], "yahoo");
    assert_eq!(out.data["note"], NOTE);
    assert!(out.hint.unwrap().contains("quote history 2330.TW"));
}

#[test]
fn market_state_and_zero_volume() {
    let meta: Value = serde_json::from_str(
        r#"{"regularMarketPrice":49313.44,"chartPreviousClose":49822.6,"regularMarketVolume":0,"gmtoffset":0,
            "currentTradingPeriod":{"pre":{"start":100,"end":200},"regular":{"start":200,"end":300},"post":{"start":300,"end":400}}}"#,
    )
    .unwrap();
    let state = |now| quote_from_meta(&meta, "^TWII", now).unwrap()["market_state"].clone();
    assert_eq!(state(150), "pre");
    assert_eq!(state(250), "regular");
    assert_eq!(state(350), "post");
    assert_eq!(state(500), "closed");
    let q = quote_from_meta(&meta, "^TWII", 250).unwrap();
    assert_eq!(q["volume"], Value::Null);
    assert_eq!(q["change"], json!(-509.16));
    assert!(matches!(
        quote_from_meta(&json!({"fullExchangeName": "Japan OTC"}), "2330", 0),
        Err(QErr::NoData(m)) if m.contains("Japan OTC")
    ));
}

#[test]
fn several_symbols_with_partial_failure() {
    let (base, rx) = serve(vec![
        json_ok(&chart("NVDA", 237.47, 239.24, "")),
        http("404 Not Found", "application/json", NOT_FOUND),
    ]);
    let ep = eps(&base, "http://127.0.0.1:9");
    let out = run_get(
        &ctx(&ep, None),
        &GetArgs {
            symbols: vec!["NVDA, zzzzqq".into(), "2330".into(), "nvda".into()],
            backend: "auto".into(),
        },
    )
    .unwrap();
    assert_eq!(
        rx.recv().unwrap().line.split(' ').nth(1).unwrap(),
        "/v8/finance/chart/NVDA?range=1d&interval=1d"
    );
    assert_eq!(out.data["quotes"].as_array().unwrap().len(), 1);
    let errs = out.data["errors"].as_array().unwrap();
    assert_eq!(errs.len(), 2);
    assert_eq!(errs[0]["symbol"], "ZZZZQQ");
    assert_eq!(errs[0]["code"], "symbol_not_found");
    assert_eq!(errs[1]["code"], "ambiguous_symbol");
    let hint = out.hint.unwrap();
    assert!(hint.contains("quote search \"ZZZZQQ\""), "{hint}");
    assert!(
        hint.contains("2330.TW") && hint.contains("2330.TWO"),
        "{hint}"
    );
}

#[test]
fn single_failure_returns_its_own_error() {
    let (base, _rx) = serve(vec![http("404 Not Found", "application/json", NOT_FOUND)]);
    let ep = eps(&base, "http://127.0.0.1:9");
    let e = run_get(
        &ctx(&ep, None),
        &GetArgs {
            symbols: vec!["ZZZZQQ".into()],
            backend: "yahoo".into(),
        },
    )
    .unwrap_err();
    assert_eq!(e.code, "symbol_not_found");
    assert!(e.message.contains("delisted"));
    let e = run_get(
        &ctx(&ep, None),
        &GetArgs {
            symbols: vec!["6488".into()],
            backend: "auto".into(),
        },
    )
    .unwrap_err();
    assert_eq!(e.code, "ambiguous_symbol");
    assert!(e.hint.contains("6488.TWO"));
    let many: Vec<String> = (0..11).map(|i| format!("S{i}")).collect();
    let e = run_get(
        &ctx(&ep, None),
        &GetArgs {
            symbols: many,
            backend: "auto".into(),
        },
    )
    .unwrap_err();
    assert_eq!(e.code, "bad_args");
}

const STOOQ_CSV: &str = "Symbol,Date,Time,Open,High,Low,Close,Volume,Name\r\nNVDA.US,2026-10-07,22:00:07,238.1,239.08,236.39,237.47,81027431,NVIDIA\r\n";

#[test]
fn auto_falls_back_to_stooq_when_yahoo_is_rate_limited() {
    let (ybase, _y) = serve(vec![http(
        "429 Too Many Requests",
        "text/plain",
        "Too Many Requests",
    )]);
    let (sbase, srx) = serve(vec![http("200 OK", "text/csv", STOOQ_CSV)]);
    let ep = eps(&ybase, &sbase);
    let out = run_get(
        &ctx(&ep, None),
        &GetArgs {
            symbols: vec!["NVDA".into()],
            backend: "auto".into(),
        },
    )
    .unwrap();
    let req = srx.recv().unwrap();
    assert_eq!(
        req.line,
        "GET /q/l/?s=nvda.us&f=sd2t2ohlcvn&h&e=csv HTTP/1.1"
    );
    let q = &out.data["quotes"][0];
    assert_eq!(q["backend"], "stooq");
    assert_eq!(q["price"], json!(237.47));
    assert_eq!(q["open"], json!(238.1));
    assert_eq!(q["change"], Value::Null);
    assert_eq!(q["time"], "2026-10-07 22:00:07");
    assert_eq!(q["stooq_symbol"], "nvda.us");
    assert_eq!(out.data["backend"], "stooq");
    assert!(out.hint.unwrap().contains("no previous close"));
}

#[test]
fn rate_limit_without_fallback_hints_stooq() {
    let (ybase, _y) = serve(vec![http("429 Too Many Requests", "text/plain", "x")]);
    let ep = eps(&ybase, "http://127.0.0.1:9");
    let e = run_get(
        &ctx(&ep, None),
        &GetArgs {
            symbols: vec!["NVDA".into()],
            backend: "yahoo".into(),
        },
    )
    .unwrap_err();
    assert_eq!(e.code, "rate_limited");
    assert!(e.hint.contains("--backend stooq"));
}

#[test]
fn stooq_key_page_and_key_redaction() {
    let page = "Get your apikey:\n1. Open https://stooq.com/q/d/?s=nvda.us&get_apikey\n2. Enter the captcha code.";
    let (sbase, _s) = serve(vec![http("200 OK", "text/plain", page)]);
    let ep = eps("http://127.0.0.1:9", &sbase);
    let e = run_get(
        &ctx(&ep, None),
        &GetArgs {
            symbols: vec!["NVDA".into()],
            backend: "stooq".into(),
        },
    )
    .unwrap_err();
    assert_eq!(e.code, "stooq_needs_key");
    assert!(e.hint.contains("STOOQ_API_KEY") && e.hint.contains("config set stooq.api_key"));

    let key = "sk-test-0123456789abcdef";
    let (sbase, srx) = serve(vec![http(
        "500 Internal Server Error",
        "text/plain",
        &format!("bad key {key}"),
    )]);
    let ep = eps("http://127.0.0.1:9", &sbase);
    let e = run_get(
        &ctx(&ep, Some(key)),
        &GetArgs {
            symbols: vec!["NVDA".into()],
            backend: "stooq".into(),
        },
    )
    .unwrap_err();
    assert!(srx.recv().unwrap().line.contains(&format!("&apikey={key}")));
    assert_eq!(e.code, "http_error");
    assert!(
        !e.message.contains(key) && !e.hint.contains(key),
        "{}",
        e.message
    );
    assert!(e.message.contains("[redacted]"));
}

#[test]
fn stooq_symbol_mapping() {
    let m = |s: &str| to_stooq(s);
    assert_eq!(m("NVDA").as_deref(), Some("nvda.us"));
    assert_eq!(m("BRK-B").as_deref(), Some("brk-b.us"));
    assert_eq!(m("BRK.B").as_deref(), Some("brk.b.us"));
    assert_eq!(m("^GSPC").as_deref(), Some("^spx"));
    assert_eq!(m("^DJI").as_deref(), Some("^dji"));
    assert_eq!(m("USDTWD=X").as_deref(), Some("usdtwd"));
    assert_eq!(m("BTC-USD").as_deref(), Some("btcusd"));
    assert_eq!(m("0700.HK").as_deref(), Some("700.hk"));
    assert_eq!(m("7203.T").as_deref(), Some("7203.jp"));
    assert_eq!(m("VOD.L").as_deref(), Some("vod.uk"));
    assert_eq!(m("2330.TW"), None);
    assert_eq!(m("6488.TWO"), None);
}

#[test]
fn symbol_helpers() {
    assert_eq!(
        split_symbols(&["nvda, 2330.tw".into(), "^twii;NVDA".into()]),
        ["NVDA", "2330.TW", "^TWII"]
    );
    assert!(is_bare_number("2330") && is_bare_number("006208"));
    assert!(!is_bare_number("2330.TW") && !is_bare_number("123") && !is_bare_number("NVDA"));
    assert_eq!(default_interval("5y"), "1wk");
    assert_eq!(default_interval("max"), "1mo");
    assert_eq!(default_interval("6mo"), "1d");
    let d = NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
    assert_eq!(range_start("ytd", d).to_string(), "2026-01-01");
    assert_eq!(range_start("3mo", d).to_string(), "2026-07-08");
}

fn history_chart(n: usize) -> String {
    let start = 1_788_000_000i64;
    let ts: Vec<String> = (0..n)
        .map(|i| (start + i as i64 * 86_400).to_string())
        .collect();
    let close: Vec<String> = (0..n)
        .map(|i| {
            if i == 2 {
                "null".into()
            } else {
                format!("{}.0001", 100 + i)
            }
        })
        .collect();
    let high: Vec<String> = (0..n).map(|i| format!("{}", 101 + i)).collect();
    let low: Vec<String> = (0..n)
        .map(|i| {
            if i == 1 {
                "90".into()
            } else {
                format!("{}", 99 + i)
            }
        })
        .collect();
    let vol: Vec<String> = (0..n).map(|i| format!("{}", 1000 + i)).collect();
    format!(
        r#"{{"chart":{{"result":[{{"meta":{{"currency":"USD","symbol":"NVDA","gmtoffset":-14400,"priceHint":2,"longName":"NVIDIA Corporation"}},"timestamp":[{}],"indicators":{{"quote":[{{"open":[{}],"high":[{}],"low":[{}],"close":[{}],"volume":[{}]}}]}}}}],"error":null}}}}"#,
        ts.join(","),
        close.join(","),
        high.join(","),
        low.join(","),
        close.join(","),
        vol.join(",")
    )
}

#[test]
fn history_rows_summary_and_save() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path());
    let (base, rx) = serve(vec![json_ok(&history_chart(40))]);
    let ep = eps(&base, "http://127.0.0.1:9");
    let out = run_history(
        &store,
        &ctx(&ep, None),
        &HistoryArgs {
            symbol: "nvda".into(),
            range: "3mo".into(),
            interval: None,
            backend: "auto".into(),
            save: true,
        },
    )
    .unwrap();
    assert_eq!(
        rx.recv().unwrap().line,
        "GET /v8/finance/chart/NVDA?range=3mo&interval=1d HTTP/1.1"
    );
    let d = &out.data;
    assert_eq!(d["interval"], "1d");
    assert_eq!(d["rows_total"], 39, "null close skipped");
    assert_eq!(d["rows"].as_array().unwrap().len(), 30);
    assert_eq!(d["truncated"], true);
    let s = &d["summary"];
    assert_eq!(s["start_close"], json!(100));
    assert_eq!(s["end_close"], json!(139));
    assert_eq!(s["change"], json!(39));
    assert_eq!(s["change_pct"], json!(39));
    assert_eq!(s["low"], json!(90));
    assert_eq!(s["high"], json!(140));
    assert_eq!(s["points"], 39);
    assert_eq!(s["start_date"], "2026-08-29");
    let last = d["rows"].as_array().unwrap().last().unwrap();
    assert_eq!(last["volume"], json!(1039));
    let tbl = d["table"].as_str().unwrap();
    let t = crate::table::load(&store, tbl).unwrap();
    assert_eq!(t.columns, HISTORY_COLUMNS);
    assert_eq!(t.rows.len(), 39);
    let q = crate::table::run_query(
        &store,
        &crate::table::QueryArgs {
            source: tbl.to_string(),
            query: crate::table::query::Query {
                sorts: vec!["-close".into()],
                ..Default::default()
            },
            limit: Some(1),
            save: false,
        },
    )
    .unwrap();
    assert_eq!(q.data["rows"][0]["close"], json!(139));
    assert!(out.hint.unwrap().contains(tbl));
}

#[test]
fn history_stooq_needs_key_then_parses_csv() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path());
    let ep = eps("http://127.0.0.1:9", "http://127.0.0.1:9");
    let args = HistoryArgs {
        symbol: "NVDA".into(),
        range: "1y".into(),
        interval: Some("1wk".into()),
        backend: "stooq".into(),
        save: false,
    };
    let e = run_history(&store, &ctx(&ep, None), &args).unwrap_err();
    assert_eq!(e.code, "stooq_needs_key");
    let csv = "Date,Open,High,Low,Close,Volume\n2026-09-28,180,190,175,185,100\n2026-10-05,185,200,184,199.5,120\n";
    let (sbase, srx) = serve(vec![http("200 OK", "text/csv", csv)]);
    let ep = eps("http://127.0.0.1:9", &sbase);
    let out = run_history(&store, &ctx(&ep, Some("k123")), &args).unwrap();
    let line = srx.recv().unwrap().line;
    assert!(
        line.starts_with("GET /q/d/l/?s=nvda.us&d1=20251008&d2=20261008&i=w&apikey=k123"),
        "{line}"
    );
    assert_eq!(out.data["backend"], "stooq");
    assert_eq!(out.data["summary"]["change_pct"], json!(7.84));
    assert_eq!(out.data["summary"]["low_date"], "2026-09-28");
}

#[test]
fn history_rejects_bare_numbers() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path());
    let ep = eps("http://127.0.0.1:9", "http://127.0.0.1:9");
    let e = run_history(
        &store,
        &ctx(&ep, None),
        &HistoryArgs {
            symbol: "2330".into(),
            range: "1mo".into(),
            interval: None,
            backend: "auto".into(),
            save: false,
        },
    )
    .unwrap_err();
    assert_eq!(e.code, "ambiguous_symbol");
}

const SEARCH_TSMC: &str = r#"{"quotes":[
  {"exchange":"NYQ","shortname":"Taiwan Semiconductor","quoteType":"EQUITY","symbol":"TSM","typeDisp":"Equity","longname":"Taiwan Semiconductor Manufacturing Company Limited","exchDisp":"NYSE"},
  {"exchange":"TAI","shortname":"TSMC","quoteType":"EQUITY","symbol":"2330.TW","typeDisp":"Equity","exchDisp":"Taiwan"},
  {"exchange":"SAO","shortname":"TAIWANSMFAC DRN","quoteType":"EQUITY","symbol":"TSMC34.SA","typeDisp":"Equity","exchDisp":"São Paulo"},
  {"exchange":"X","quoteType":"EQUITY"}
]}"#;

#[test]
fn search_merges_aliases_and_yahoo() {
    let (base, rx) = serve(vec![json_ok(SEARCH_TSMC)]);
    let ep = eps(&base, "http://127.0.0.1:9");
    let out = run_search(
        &ctx(&ep, None),
        &SearchArgs {
            query: "TSMC".into(),
            limit: 10,
        },
    )
    .unwrap();
    let line = rx.recv().unwrap().line;
    assert!(
        line.starts_with("GET /v1/finance/search?q=TSMC&quotesCount=10&newsCount=0"),
        "{line}"
    );
    let syms: Vec<&str> = out.data["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["symbol"].as_str().unwrap())
        .collect();
    assert_eq!(syms, ["2330.TW", "TSM", "TSMC34.SA"]);
    assert_eq!(out.data["results"][0]["source"], "alias");
    assert_eq!(out.data["results"][2]["exchange"], "São Paulo");
    assert!(out.hint.unwrap().contains("quote get 2330.TW"));
}

#[test]
fn search_chinese_names_use_aliases_when_yahoo_rejects() {
    let bad = r#"{"finance":{"result":null,"error":{"code":"Bad Request","description":"Invalid Search Query"}}}"#;
    let (base, _rx) = serve(vec![
        http("400 Bad Request", "application/json", bad),
        http("400 Bad Request", "application/json", bad),
    ]);
    let ep = eps(&base, "http://127.0.0.1:9");
    let out = run_search(
        &ctx(&ep, None),
        &SearchArgs {
            query: "台積電股價".into(),
            limit: 5,
        },
    )
    .unwrap();
    assert_eq!(out.data["results"][0]["symbol"], "2330.TW");
    assert!(out.hint.unwrap().contains("Invalid Search Query"));
    let e = run_search(
        &ctx(&ep, None),
        &SearchArgs {
            query: "不存在的公司".into(),
            limit: 5,
        },
    )
    .unwrap_err();
    assert_eq!(e.code, "no_results");
    assert!(e.hint.contains("English"));
}

#[test]
fn search_bare_number_suggests_taiwan_suffixes() {
    let (base, _rx) = serve(vec![json_ok(
        r#"{"quotes":[{"symbol":"2330.T","shortname":"Forside","exchDisp":"Tokyo","typeDisp":"Equity"}]}"#,
    )]);
    let ep = eps(&base, "http://127.0.0.1:9");
    let out = run_search(
        &ctx(&ep, None),
        &SearchArgs {
            query: "2330".into(),
            limit: 5,
        },
    )
    .unwrap();
    assert!(out.hint.unwrap().contains("2330.TWO"));
    let empty = run_search(
        &ctx(&ep, None),
        &SearchArgs {
            query: "   ".into(),
            limit: 5,
        },
    )
    .unwrap_err();
    assert_eq!(empty.code, "bad_args");
}

#[test]
fn alias_lookup() {
    let syms = |q: &str| {
        aliases::lookup(q)
            .iter()
            .map(|a| a.symbol)
            .collect::<Vec<_>>()
    };
    assert_eq!(syms("台積電"), ["2330.TW"]);
    assert_eq!(syms("tsmc"), ["2330.TW", "TSM"]);
    assert_eq!(syms("美元台幣匯率"), ["USDTWD=X"]);
    assert_eq!(syms("S&P 500"), ["^GSPC"]);
    assert!(
        syms("nvidia corp").is_empty(),
        "English keys need an exact match"
    );
}
