use super::*;
use crate::envelope::envelope;
use crate::render::render_md;
use crate::schema;
use crate::state::Store;
use crate::testutil::{http, serve};
use std::fs;
use std::path::PathBuf;

fn store_dir() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path());
    (tmp, store)
}

fn import_curated(store: &Store) {
    run_import(
        store,
        &ImportArgs {
            curated: true,
            ..Default::default()
        },
    )
    .unwrap();
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/kb/fixtures/services.json")
}

fn import_services(store: &Store, path: &str) {
    run_import(
        store,
        &ImportArgs {
            services: vec![path.to_string()],
            ..Default::default()
        },
    )
    .unwrap();
}

const FORBIDDEN_HOSTS: &[&str] = &[
    "my.gov.tw",
    "www.my.gov.tw",
    "medcloud.nhi.gov.tw",
    "www.labor.gov.tw",
    "www.cla.gov.tw",
    "www.eservice.mol.gov.tw",
    "1968.motc.gov.tw",
    "www.shingas.com.tw",
    "eoc.nfa.gov.tw",
    "www.cdprc.ey.gov.tw",
    "www.emic.gov.tw",
];

fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

#[test]
fn curated_urls_are_checked_and_forbidden_hosts_are_absent() {
    let records = curated::load_embedded().unwrap();
    assert!(records.len() >= 70, "curated count {}", records.len());
    let mut cats = std::collections::HashSet::new();
    for rec in &records {
        cats.insert(rec.category.as_str());
        assert!(
            rec.official_url.starts_with("https://"),
            "{} {}",
            rec.id,
            rec.official_url
        );
        assert!(matches!(
            rec.verify_status.as_str(),
            "verified" | "connected" | "blocked" | "unverified"
        ));
        assert_eq!(rec.source, "curated:kb-v1");
        assert_eq!(rec.source_license, "official-url-only");
        assert_eq!(rec.last_verified, "2026-11-11");
        let host = host_of(&rec.official_url);
        assert!(
            !FORBIDDEN_HOSTS.contains(&host.as_str()),
            "{} has forbidden host {host}",
            rec.id
        );
    }
    for cat in curated::LIFE_CATEGORIES {
        assert!(cats.contains(cat), "missing category {cat}");
    }
    let by_id = |id: &str| records.iter().find(|r| r.id == id).unwrap();
    assert_eq!(by_id("org:nhi").verify_status, "blocked");
    assert_eq!(by_id("org:nhi").official_url, "https://www.nhi.gov.tw");
    assert_eq!(
        by_id("org:etax").official_url,
        "https://www.etax.nat.gov.tw/etwmain/"
    );
    assert_eq!(
        by_id("org:freeway-1968").official_url,
        "https://1968.freeway.gov.tw"
    );
    assert_eq!(by_id("org:ris").official_url, "https://www.ris.gov.tw");
    assert_eq!(
        by_id("org:cwa-opendata").official_url,
        "https://opendata.cwa.gov.tw"
    );
    assert_eq!(by_id("org:cwa").official_url, "https://www.cwa.gov.tw");
    assert_eq!(by_id("org:cwa").verify_status, "verified");
    assert_eq!(by_id("org:vscinemas").verify_status, "unverified");
    assert!(records.len() >= 140, "curated count {}", records.len());
}

#[test]
fn everyday_queries_return_relevant_hits() {
    let (_tmp, store) = store_dir();
    import_curated(&store);
    let expect = [
        (
            "電影",
            &[
                "org:vscinemas",
                "org:ambassador",
                "org:showtimes",
                "org:miramar",
            ][..],
        ),
        (
            "公車",
            &[
                "org:bus-taipei",
                "org:bus-newtaipei",
                "org:bus-taoyuan",
                "org:bus-taichung",
                "org:bus-tainan",
                "org:bus-kaohsiung",
                "org:bus-keelung",
                "org:bus-hsinchu",
                "org:bus-hsinchu-county",
                "org:bus-changhua",
                "org:bus-pingtung",
            ][..],
        ),
        ("天氣", &["org:cwa", "org:cwa-opendata"][..]),
        ("臺鐵", &["org:railway", "org:tra-book", "org:tra-time"][..]),
        ("台鐵", &["org:railway", "org:tra-book", "org:tra-time"][..]),
        (
            "高鐵",
            &["org:thsrc", "org:thsrc-time", "org:thsrc-book"][..],
        ),
        ("租屋", &["org:rent591"][..]),
        ("外送", &["org:ubereats", "org:foodpanda"][..]),
    ];
    for (query, allowed) in expect {
        let out = run_search(
            &store,
            &SearchArgs {
                query: query.into(),
                category: None,
                kind: "org".into(),
                limit: 3,
            },
        )
        .unwrap_or_else(|e| panic!("{query}: {} {}", e.message, e.hint));
        let ids: Vec<&str> = out.data["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["id"].as_str().unwrap())
            .collect();
        assert!(!ids.is_empty(), "{query} returned no hits");
        assert!(
            allowed.contains(&ids[0]),
            "{query} top hit {} not in {allowed:?}",
            ids[0]
        );
        for id in &ids {
            assert!(
                allowed.contains(id),
                "{query} hit {id} is outside {allowed:?}; hits {ids:?}"
            );
        }
    }
}

#[test]
fn trigram_partial_chinese_and_short_query_and_typo_and_alias() {
    let (_tmp, store) = store_dir();
    import_curated(&store);
    import_services(&store, &fixture_path().display().to_string());

    let conn = db::open(&store).unwrap();
    assert_eq!(db::count(&conn).unwrap(), db::fts_count(&conn).unwrap());
    drop(conn);

    let partial = run_search(
        &store,
        &SearchArgs {
            query: "所得稅".into(),
            category: None,
            kind: "all".into(),
            limit: 8,
        },
    )
    .unwrap();
    assert_eq!(partial.data["via"], "trigram");
    let ids: Vec<&str> = partial.data["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["id"].as_str().unwrap())
        .collect();
    assert!(
        ids.contains(&"svc:TAX-TEST-1"),
        "所得稅 hits: {}",
        partial.data
    );
    let tax = partial.data["hits"]
        .as_array()
        .unwrap()
        .iter()
        .find(|h| h["id"] == "svc:TAX-TEST-1")
        .unwrap();
    assert_eq!(tax["match"], "partial");
    assert!(tax["score"].as_f64().unwrap() >= score::HIT_MIN);
    assert!(tax["score"].as_f64().unwrap() <= 1.0);

    let short = run_search(
        &store,
        &SearchArgs {
            query: "報稅".into(),
            category: None,
            kind: "all".into(),
            limit: 8,
        },
    )
    .unwrap();
    assert_eq!(short.data["via"], "scan");
    let short_hits = short.data["hits"].as_array().unwrap();
    assert_eq!(short_hits[0]["id"], "org:etax");
    assert_eq!(short_hits[0]["match"], "exact");
    assert!(
        short_hits.iter().any(|h| h["id"] == "svc:TAX-TEST-1"),
        "報稅 should find 綜合所得稅結算申報: {}",
        short.data
    );
    let svc = short_hits
        .iter()
        .find(|h| h["id"] == "svc:TAX-TEST-1")
        .unwrap();
    assert_eq!(svc["match"], "fuzzy");
    assert_eq!(
        svc["official_url"],
        "https://www.gov.tw/News_Content_2_tax.html"
    );

    let typo = run_search(
        &store,
        &SearchArgs {
            query: "身分正".into(),
            category: None,
            kind: "service".into(),
            limit: 5,
        },
    )
    .unwrap();
    assert_eq!(typo.data["via"], "scan");
    assert_eq!(typo.data["hits"][0]["id"], "svc:HH-TEST-1");
    assert_eq!(typo.data["hits"][0]["match"], "fuzzy");

    let variant = run_search(
        &store,
        &SearchArgs {
            query: "身份證".into(),
            category: None,
            kind: "service".into(),
            limit: 5,
        },
    )
    .unwrap();
    assert_eq!(variant.data["hits"][0]["id"], "svc:HH-TEST-1");

    let alias = run_search(
        &store,
        &SearchArgs {
            query: "勞保局".into(),
            category: None,
            kind: "org".into(),
            limit: 3,
        },
    )
    .unwrap();
    assert_eq!(alias.data["hits"][0]["id"], "org:bli");
    assert_eq!(
        alias.data["hits"][0]["official_url"],
        "https://www.bli.gov.tw"
    );
    assert_eq!(alias.data["hits"][0]["match"], "exact");

    let north = run_search(
        &store,
        &SearchArgs {
            query: "北區國稅局".into(),
            category: Some("tax".into()),
            kind: "org".into(),
            limit: 3,
        },
    )
    .unwrap();
    assert_eq!(north.data["hits"][0]["id"], "org:ntbna");
    let taipei = run_search(
        &store,
        &SearchArgs {
            query: "台北國稅局".into(),
            category: None,
            kind: "org".into(),
            limit: 3,
        },
    )
    .unwrap();
    assert_eq!(taipei.data["hits"][0]["id"], "org:ntbt");

    let road = run_search(
        &store,
        &SearchArgs {
            query: "1968".into(),
            category: None,
            kind: "all".into(),
            limit: 5,
        },
    )
    .unwrap();
    assert_eq!(
        road.data["hits"][0]["official_url"],
        "https://1968.freeway.gov.tw"
    );
    let blob = road.data.to_string();
    assert!(!blob.contains("1968.motc.gov.tw"));
    assert!(!blob.contains("my.gov.tw"));
}

#[test]
fn not_found_hint_offers_rephrase_then_search_last() {
    let (_tmp, store) = store_dir();
    import_curated(&store);
    let err = run_search(
        &store,
        &SearchArgs {
            query: "zzzzqqqq".into(),
            category: None,
            kind: "all".into(),
            limit: 8,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, "not_found");
    assert!(err.hint.contains("agentbox kb search"));
    assert!(err.hint.contains("agentbox search"));
    assert!(err.hint.contains("--site gov.tw"));
    assert!(err.hint.contains("Do not invent"));
    assert!(
        !err.hint.contains("戶政"),
        "hint must not suggest an unrelated query: {}",
        err.hint
    );
    assert!(
        err.hint.contains("formal") || err.hint.contains("Split"),
        "{}",
        err.hint
    );
    let env = envelope(&Err(err));
    assert_eq!(env["ok"], false);
    assert!(env["hint"].as_str().unwrap().contains("last resort"));

    let extra = run_search(
        &store,
        &SearchArgs {
            query: "zzzz 官方網站".into(),
            category: None,
            kind: "all".into(),
            limit: 5,
        },
    )
    .unwrap_err();
    assert_eq!(extra.code, "not_found");
    assert!(
        extra.hint.contains("agentbox kb search zzzz"),
        "drop the generic word: {}",
        extra.hint
    );
    assert!(!extra.hint.contains("戶政"), "{}", extra.hint);
}

#[test]
fn multiword_query_matches_any_term_and_reports_coverage() {
    let (_tmp, store) = store_dir();
    import_curated(&store);

    let rail = run_search(
        &store,
        &SearchArgs {
            query: "高鐵 官方網站".into(),
            category: None,
            kind: "org".into(),
            limit: 5,
        },
    )
    .unwrap_or_else(|e| panic!("{} {}", e.message, e.hint));
    assert!(rail.data["via"].as_str().is_some());
    assert_eq!(rail.data["hits"][0]["id"], "org:thsrc");
    let terms = rail.data["hits"][0]["matched_terms"].as_array().unwrap();
    assert!(terms.iter().any(|t| t == "高鐵"), "{terms:?}");
    assert!(!terms.iter().any(|t| t == "官方網站"), "{terms:?}");
    let which = rail.data["hits"][0]["which"].as_array().unwrap();
    assert!(which.iter().any(|w| w == "official"), "{which:?}");

    let coffee = run_search(
        &store,
        &SearchArgs {
            query: "星巴克 杜拜 巧克力 可頌".into(),
            category: None,
            kind: "all".into(),
            limit: 5,
        },
    )
    .unwrap_or_else(|e| panic!("{} {}", e.message, e.hint));
    let hits = coffee.data["hits"].as_array().unwrap();
    let sb = hits
        .iter()
        .find(|h| h["id"] == "org:starbucks")
        .unwrap_or_else(|| {
            panic!(
                "星巴克 should still hit when extra words miss: {}",
                coffee.data
            )
        });
    let terms = sb["matched_terms"].as_array().unwrap();
    assert!(terms.iter().any(|t| t == "星巴克"), "{terms:?}");
    assert!(!terms.iter().any(|t| t == "杜拜"), "{terms:?}");
    assert!(sb["score"].as_f64().unwrap() >= score::HIT_MIN);
}

#[test]
fn category_mismatch_points_at_other_categories() {
    let (_tmp, store) = store_dir();
    import_curated(&store);
    let err = run_search(
        &store,
        &SearchArgs {
            query: "星巴克".into(),
            category: Some("tax".into()),
            kind: "all".into(),
            limit: 5,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, "not_found");
    let hint = err.hint.to_ascii_lowercase();
    assert!(hint.contains("drop --category"), "{}", err.hint);
    assert!(hint.contains("hits in other categories"), "{}", err.hint);
    assert!(err.hint.contains("food"), "{}", err.hint);
    assert!(!err.hint.contains("戶政"), "{}", err.hint);
}

#[test]
fn empty_database_tells_you_to_import() {
    let (_tmp, store) = store_dir();
    let err = run_search(
        &store,
        &SearchArgs {
            query: "健保".into(),
            category: None,
            kind: "all".into(),
            limit: 8,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, "kb_empty");
    assert!(err.hint.contains("agentbox kb import --curated"));
    assert!(!db::db_path(&store).exists());
}

#[test]
fn get_returns_full_record_and_service_links() {
    let (_tmp, store) = store_dir();
    import_curated(&store);
    import_services(&store, &fixture_path().display().to_string());
    let nhi = run_get(&store, "org:nhi").unwrap();
    assert_eq!(nhi.data["verify_status"], "blocked");
    assert_eq!(nhi.data["official_url"], "https://www.nhi.gov.tw");
    assert_eq!(nhi.data["source"], "curated:kb-v1");
    assert!(nhi.data["aliases"]
        .as_array()
        .unwrap()
        .iter()
        .any(|a| a == "健保"));

    let svc = run_get(&store, "svc:TAX-TEST-1").unwrap();
    assert_eq!(svc.data["source"], "data.gov.tw:146973");
    assert_eq!(svc.data["source_license"], "OGDL-1.0");
    assert_eq!(svc.data["verify_status"], "unverified");
    assert_eq!(svc.data["category"], "tax");
    let rels: Vec<&str> = svc.data["service_urls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["rel"].as_str().unwrap())
        .collect();
    assert!(rels.contains(&"page"));
    assert!(rels.contains(&"apply"));
    assert_eq!(
        svc.data["service_urls"]
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["rel"] == "apply")
            .unwrap()["url"],
        "https://www.etax.nat.gov.tw/etwmain/"
    );

    let dollar = run_get(&store, "svc:A01040000A-1$font$").unwrap();
    assert_eq!(dollar.data["kind"], "service");
    assert!(dollar.hint.unwrap().contains("agentbox kb open"));

    let missing = run_get(&store, "org:nope").unwrap_err();
    assert_eq!(missing.code, "not_found");
    assert!(missing.hint.contains("agentbox kb search"));
}

#[test]
fn import_writes_ogdl_attribution_and_skips_rows_without_urls() {
    let (tmp, store) = store_dir();
    let out = run_import(
        &store,
        &ImportArgs {
            curated: true,
            services: vec![fixture_path().display().to_string()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(out.data["services"], 3);
    assert_eq!(out.data["skipped"], 1);
    assert!(out.data["curated"].as_u64().unwrap() >= 70);
    let text = fs::read_to_string(tmp.path().join("kb/ATTRIBUTION.txt")).unwrap();
    assert!(text.contains("數位發展部"));
    assert!(text.contains("OGDL-1.0"));
    assert!(text.contains("146973"));
    assert!(text.contains("https://data.gov.tw/license/"));
    assert!(text.contains("政府資料開放授權條款"));
    let again = run_import(
        &store,
        &ImportArgs {
            curated: true,
            services: vec![fixture_path().display().to_string()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(again.data["records"], out.data["records"]);
}

#[test]
fn replace_drops_previous_rows_from_the_same_source() {
    let (tmp, store) = store_dir();
    import_curated(&store);
    import_services(&store, &fixture_path().display().to_string());
    let slim = tmp.path().join("one.json");
    fs::write(
        &slim,
        r#"[{"服務代碼":"ONLY","標題":"只有一筆","頁面連結":"https://www.gov.tw/only.html"}]"#,
    )
    .unwrap();
    run_import(
        &store,
        &ImportArgs {
            services: vec![slim.display().to_string()],
            replace: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(run_get(&store, "org:ris").is_ok());
    assert_eq!(
        run_get(&store, "svc:TAX-TEST-1").unwrap_err().code,
        "not_found"
    );
    assert!(run_get(&store, "svc:ONLY").is_ok());
}

#[test]
fn open_uses_cache_then_fetch_and_reports_blocked() {
    let html = "<html><head><title>戶政說明</title></head><body><h1>補發</h1><p>請帶身分證。</p></body></html>";
    let (base, rx) = serve(vec![
        http("200 OK", "text/html; charset=utf-8", html),
        http(
            "403 Forbidden",
            "text/html",
            "<title>Just a moment...</title>",
        ),
    ]);
    let (dir, store) = store_dir();
    let path = dir.path().join("open.json");
    fs::write(
        &path,
        format!(
            r#"[{{"服務代碼":"OPEN1","標題":"測試開啟頁","線上申辦_服務內容":"測試","頁面連結":"{base}/official","線上申辦連結":"{base}/apply"}},{{"服務代碼":"BLOCK1","標題":"受阻頁","線上申辦_服務內容":"受阻","頁面連結":"{base}/blocked"}}]"#
        ),
    )
    .unwrap();
    import_services(&store, &path.display().to_string());

    let first = run_open(&store, "svc:OPEN1", "apply").unwrap();
    assert_eq!(first.data["from_cache"], false);
    assert_eq!(first.data["doc"], "doc:1");
    assert_eq!(first.data["kb_id"], "svc:OPEN1");
    assert_eq!(first.data["which"], "apply");
    assert_eq!(first.data["title"], "戶政說明");
    let req = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
    assert!(req.line.contains("/apply"), "{}", req.line);

    let second = run_open(&store, "svc:OPEN1", "apply").unwrap();
    assert_eq!(second.data["from_cache"], true);
    assert_eq!(second.data["doc"], "doc:1");

    let blocked = run_open(&store, "svc:BLOCK1", "official").unwrap_err();
    assert_eq!(blocked.code, "fetch_blocked");
    let env = envelope(&Err(blocked));
    assert_eq!(env["error"]["official_url"], format!("{base}/blocked"));
    assert_eq!(env["error"]["id"], "svc:BLOCK1");
    assert!(env["hint"].as_str().unwrap().contains("agentbox kb get"));
    assert!(env["hint"].as_str().unwrap().contains("Do not invent"));

    let bad = run_open(&store, "svc:OPEN1", "nope").unwrap_err();
    assert_eq!(bad.code, "bad_args");
    assert!(bad.hint.contains("official"), "{}", bad.hint);
    assert!(bad.hint.contains("apply"), "{}", bad.hint);
    assert!(bad.hint.contains("page"), "{}", bad.hint);

    let got = run_get(&store, "svc:OPEN1").unwrap();
    let which = got.data["which"].as_array().unwrap();
    assert!(which.iter().any(|w| w == "official"));
    assert!(which.iter().any(|w| w == "apply"));
    assert!(which.iter().any(|w| w == "page"));
}

#[test]
fn markdown_format_renders_hits() {
    let (_tmp, store) = store_dir();
    import_services(&store, &fixture_path().display().to_string());
    let out = run_search(
        &store,
        &SearchArgs {
            query: "所得稅".into(),
            category: None,
            kind: "service".into(),
            limit: 3,
        },
    )
    .unwrap();
    let md = render_md(&envelope(&Ok(out)));
    assert!(md.contains("| id |"));
    assert!(md.contains("svc:TAX-TEST-1"));
    assert!(md.contains("> hint:"));
}

#[test]
fn schema_lists_kb_tools_but_not_import() {
    let out = schema::run_schema(false).unwrap();
    let names: Vec<&str> = out.data["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"kb_search"));
    assert!(names.contains(&"kb_get"));
    assert!(names.contains(&"kb_open"));
    assert!(!names.contains(&"kb_import"));
}

#[test]
fn popular_seed_finds_everyday_sites_and_skips_adult() {
    let seed = super::popular::seed_from_bytes(super::popular::SEED_TW).expect("tw seed");
    assert!(seed.records.len() > 1000, "tw seed {}", seed.records.len());
    for host in [
        "youtube.com",
        "shopee.tw",
        "ptt.cc",
        "dcard.tw",
        "books.com.tw",
        "line.me",
        "wikipedia.org",
        "github.com",
    ] {
        assert!(
            seed.records.iter().any(|r| r.id == format!("pop:{host}")),
            "missing {host}"
        );
    }
    for bad in [
        "xvideos.com",
        "18porn.cc",
        "123av.com",
        "pornhub.com",
        "gstatic.com",
        "goodav17.com",
        "rou.video",
    ] {
        assert!(
            seed.records
                .iter()
                .all(|r| r.id != format!("pop:{bad}") && !r.official_url.contains(bad)),
            "adult or infra leaked: {bad}"
        );
    }
    for rec in &seed.records {
        let blob = format!("{} {} {}", rec.name_zh, rec.name_en, rec.aliases.join(" "));
        assert!(
            !blob.contains("&#"),
            "{} still has an HTML entity: {blob}",
            rec.id
        );
        assert!(
            !blob.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c)),
            "{} still has a Japanese-only name: {blob}",
            rec.id
        );
    }
    let (_tmp, store) = store_dir();
    import_curated(&store);
    run_import(
        &store,
        &ImportArgs {
            popular: true,
            ..Default::default()
        },
    )
    .unwrap();
    let cases = [
        ("YouTube", "youtube.com"),
        ("蝦皮", "shopee.tw"),
        ("PTT", "ptt.cc"),
        ("Dcard", "dcard.tw"),
        ("博客來", "books.com.tw"),
        ("LINE", "line.me"),
        ("Wikipedia", "wikipedia.org"),
        ("GitHub", "github.com"),
    ];
    for (query, host) in cases {
        let out = run_search(
            &store,
            &SearchArgs {
                query: query.into(),
                category: None,
                kind: "all".into(),
                limit: 3,
            },
        )
        .unwrap_or_else(|e| panic!("{query}: {} {}", e.message, e.hint));
        let url = out.data["hits"][0]["official_url"].as_str().unwrap();
        assert!(
            url.contains(host),
            "{query} top url {url} does not contain {host}"
        );
    }
}

#[test]
fn popularity_bucket_breaks_ties_only() {
    let (_tmp, store) = store_dir();
    let conn = super::db::open(&store).unwrap();
    let mut exact = super::db::Record::blank("pop:plain.example", "org");
    exact.name_zh = "zzzzbrand".into();
    exact.aliases = vec!["zzzzbrand".into()];
    exact.official_url = "https://plain.example".into();
    exact.category = "general".into();
    let mut popular = super::db::Record::blank("pop:pop.example", "org");
    popular.name_zh = "zzzzbrand".into();
    popular.aliases = vec!["zzzzbrand".into()];
    popular.official_url = "https://pop.example".into();
    popular.popularity_bucket = Some(1000);
    popular.popularity_scope = "tw".into();
    popular.category = "general".into();
    let mut partial = super::db::Record::blank("pop:weak.example", "org");
    partial.name_zh = "zzzzbrand shop".into();
    partial.official_url = "https://weak.example".into();
    partial.popularity_bucket = Some(1000);
    partial.category = "general".into();
    super::db::upsert(&conn, &exact).unwrap();
    super::db::upsert(&conn, &popular).unwrap();
    super::db::upsert(&conn, &partial).unwrap();
    super::db::rebuild_fts(&conn).unwrap();
    drop(conn);
    let out = run_search(
        &store,
        &SearchArgs {
            query: "zzzzbrand".into(),
            category: None,
            kind: "all".into(),
            limit: 3,
        },
    )
    .unwrap();
    let ids: Vec<&str> = out.data["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids[0], "pop:pop.example",
        "bucket should win the exact-name tie: {ids:?}"
    );
    assert_eq!(ids[1], "pop:plain.example", "{ids:?}");
    assert_ne!(ids[0], "pop:weak.example");
}

#[test]
fn unknown_category_is_bad_args() {
    let (_tmp, store) = store_dir();
    import_curated(&store);
    let err = run_search(
        &store,
        &SearchArgs {
            query: "稅".into(),
            category: Some("taxes".into()),
            kind: "all".into(),
            limit: 8,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, "bad_args");
    assert!(err.hint.contains("--category tax"));
}
