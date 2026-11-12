use super::*;
use crate::state::Store;
use crate::testutil::{self, serve};
use std::time::Duration;

fn temp_store() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::new(tmp.path());
    (tmp, store)
}

fn endpoints(base: &str) -> Endpoints {
    Endpoints {
        api: Some(format!("{base}/w/api.php")),
    }
}

fn ctx<'a>(store: &'a Store, ep: &'a Endpoints) -> Ctx<'a> {
    Ctx {
        store,
        endpoints: ep,
        contact: None,
        timeout: 5,
    }
}

fn json_ok(body: &str) -> String {
    testutil::http("200 OK", "application/json", body)
}

fn http_extra(status: &str, extra: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\n{extra}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

const SEARCH_ZH: &str = r#"{
  "batchcomplete": true,
  "query": {
    "searchinfo": {"totalhits": 2},
    "search": [
      {
        "ns": 0,
        "title": "台北101",
        "pageid": 1,
        "snippet": "<span class=\"searchmatch\">台北101</span>是位於&quot;臺北市&quot;的摩天大樓&#160;。"
      },
      {
        "ns": 0,
        "title": "台北",
        "pageid": 2,
        "snippet": "台北是城市"
      }
    ]
  }
}"#;

#[test]
fn user_agent_follows_wikimedia_policy() {
    let plain = user_agent(None);
    assert_eq!(
        plain,
        format!(
            "agentbox/{} (https://github.com/alex-ht/agentbox) reqwest/{REQWEST_VERSION}",
            env!("CARGO_PKG_VERSION")
        )
    );
    assert!(!plain.contains('@'));
    let with = user_agent(Some(" ops@example.com "));
    assert!(with.contains("(https://github.com/alex-ht/agentbox; ops@example.com)"));
    assert_eq!(user_agent(Some(REPO)), plain);
    assert!(!user_agent(Some("bad\nname (x)")).contains('\n'));
    let lock = include_str!("../../Cargo.lock");
    assert!(
        lock.contains(&format!(
            "name = \"reqwest\"\nversion = \"{REQWEST_VERSION}\""
        )),
        "REQWEST_VERSION drifted from Cargo.lock"
    );
}

#[test]
fn search_strips_html_and_requests_zh_tw() {
    let (base, rx) = serve(vec![json_ok(SEARCH_ZH)]);
    let (_tmp, store) = temp_store();
    let ep = endpoints(&base);
    let out = run_search(
        &ctx(&store, &ep),
        &SearchArgs {
            query: "台北101".into(),
            lang: "zh".into(),
            limit: 5,
        },
    )
    .unwrap();
    assert_eq!(out.data["lang"], "zh");
    assert_eq!(out.data["variant"], "zh-tw");
    assert_eq!(out.data["from_cache"], false);
    assert_eq!(out.data["total"], 2);
    let hits = out.data["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0]["title"], "台北101");
    let snippet = hits[0]["snippet"].as_str().unwrap();
    assert!(!snippet.contains('<'), "{snippet}");
    assert!(!snippet.contains("&quot;"), "{snippet}");
    assert!(
        snippet.contains("台北101是位於\"臺北市\"的摩天大樓"),
        "{snippet}"
    );
    assert_eq!(
        hits[0]["url"],
        "https://zh.wikipedia.org/wiki/%E5%8F%B0%E5%8C%97101"
    );
    let req = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(req.line.contains("list=search"), "{}", req.line);
    assert!(req.line.contains("maxlag=5"), "{}", req.line);
    assert!(req.line.contains("variant=zh-tw"), "{}", req.line);
    assert!(req.line.contains("srnamespace=0"), "{}", req.line);
    assert!(req.line.contains("%E5%8F%B0%E5%8C%97101"), "{}", req.line);
    assert_eq!(req.header("accept-language"), Some("zh-TW"));
    let ua = req.header("user-agent").unwrap_or("");
    assert!(ua.starts_with("agentbox/"), "{ua}");
    assert!(ua.contains("reqwest/"), "{ua}");
    assert!(ua.contains("https://github.com/alex-ht/agentbox"), "{ua}");
    assert!(store.doc_ids().is_empty());
}

#[test]
fn english_search_does_not_send_zh_variant() {
    let body = r#"{"query":{"searchinfo":{"totalhits":1},"search":[{"title":"Albert Einstein","snippet":"<span class=\"searchmatch\">Albert</span> Einstein"}]}}"#;
    let (base, rx) = serve(vec![json_ok(body)]);
    let (_tmp, store) = temp_store();
    let ep = endpoints(&base);
    let out = run_search(
        &ctx(&store, &ep),
        &SearchArgs {
            query: "Albert Einstein".into(),
            lang: "EN".into(),
            limit: 3,
        },
    )
    .unwrap();
    assert_eq!(out.data["lang"], "en");
    assert!(out.data["variant"].is_null());
    assert_eq!(out.data["hits"][0]["snippet"], "Albert Einstein");
    assert_eq!(
        out.data["hits"][0]["url"],
        "https://en.wikipedia.org/wiki/Albert_Einstein"
    );
    let req = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(!req.line.contains("variant="), "{}", req.line);
    assert!(!req.line.contains("converttitles"), "{}", req.line);
    assert_eq!(req.header("accept-language"), Some("en"));
    assert!(req.line.contains("Albert%20Einstein"), "{}", req.line);
}

#[test]
fn search_is_cached() {
    let (base, rx) = serve(vec![json_ok(SEARCH_ZH)]);
    let (_tmp, store) = temp_store();
    let ep = endpoints(&base);
    let args = SearchArgs {
        query: "台北101".into(),
        lang: "zh".into(),
        limit: 5,
    };
    let first = run_search(&ctx(&store, &ep), &args).unwrap();
    let second = run_search(&ctx(&store, &ep), &args).unwrap();
    assert_eq!(first.data["from_cache"], false);
    assert_eq!(second.data["from_cache"], true);
    assert_eq!(first.data["hits"], second.data["hits"]);
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_err());
}

#[test]
fn open_stores_a_doc_handle_with_sections() {
    let body = r#"{
      "query": {
        "pages": [{
          "pageid": 7253,
          "ns": 0,
          "title": "台北101",
          "extract": "台北101是摩天大樓。\n\n== 歷史 ==\n於2004年落成。\n",
          "canonicalurl": "https://zh.wikipedia.org/wiki/%E5%8F%B0%E5%8C%97101"
        }]
      }
    }"#;
    let (base, rx) = serve(vec![json_ok(body)]);
    let (_tmp, store) = temp_store();
    let ep = endpoints(&base);
    let out = run_open(
        &ctx(&store, &ep),
        &OpenArgs {
            title: "台北101".into(),
            lang: "zh".into(),
        },
    )
    .unwrap();
    assert_eq!(out.data["doc"], "doc:1");
    assert_eq!(out.data["title"], "台北101");
    assert_eq!(out.data["lang"], "zh");
    assert_eq!(out.data["variant"], "zh-tw");
    assert_eq!(out.data["from_cache"], false);
    assert_eq!(
        out.data["url"],
        "https://zh.wikipedia.org/wiki/%E5%8F%B0%E5%8C%97101"
    );
    let (meta, text) = store.load_doc("doc:1").unwrap();
    assert_eq!(meta.url, out.data["url"].as_str().unwrap());
    assert!(meta.content_type.contains("source=wikipedia"));
    assert!(text.starts_with("# 台北101\n"));
    assert!(text.contains("## 歷史\n"), "{text}");
    assert!(!text.contains("=="), "{text}");
    let req = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(req.line.contains("maxlag=5"), "{}", req.line);
    assert!(req.line.contains("variant=zh-tw"), "{}", req.line);
    assert!(req.line.contains("explaintext=1"), "{}", req.line);
    assert!(req.line.contains("redirects=1"), "{}", req.line);
    assert!(req.line.contains("exsectionformat=wiki"), "{}", req.line);
    assert_eq!(req.header("accept-language"), Some("zh-TW"));
    let read = crate::cmd::read::run(
        &store,
        &crate::cmd::read::ReadArgs {
            doc: "doc:1".into(),
            section: None,
            grep: Some("落成".into()),
            max_chars: 4000,
            offset: 0,
        },
    )
    .unwrap();
    assert!(read.data["snippets"][0]["text"]
        .as_str()
        .unwrap()
        .contains("落成"));
}

#[test]
fn open_follows_redirects_and_reuses_the_doc() {
    let body = r#"{
      "query": {
        "redirects": [{"from": "Einstein", "to": "Albert Einstein"}],
        "pages": [{
          "pageid": 736,
          "ns": 0,
          "title": "Albert Einstein",
          "extract": "Albert Einstein was a physicist.\n\n== Early life ==\nBorn in Ulm.\n",
          "canonicalurl": "https://en.wikipedia.org/wiki/Albert_Einstein"
        }]
      }
    }"#;
    let (base, rx) = serve(vec![json_ok(body)]);
    let (_tmp, store) = temp_store();
    let ep = endpoints(&base);
    let args = OpenArgs {
        title: "Einstein".into(),
        lang: "en".into(),
    };
    let out = run_open(&ctx(&store, &ep), &args).unwrap();
    assert_eq!(out.data["title"], "Albert Einstein");
    assert_eq!(out.data["redirected_from"], "Einstein");
    assert_eq!(
        out.data["url"],
        "https://en.wikipedia.org/wiki/Albert_Einstein"
    );
    assert_eq!(out.data["lang"], "en");
    assert!(out.data["variant"].is_null());
    let again = run_open(&ctx(&store, &ep), &args).unwrap();
    assert_eq!(again.data["doc"], "doc:1");
    assert_eq!(again.data["from_cache"], true);
    assert_eq!(store.doc_ids(), vec![1]);
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_err());
}

#[test]
fn open_wikipedia_url_uses_the_host_language() {
    let body = r#"{
      "query": {
        "pages": [{
          "title": "Albert Einstein",
          "extract": "A physicist.\n\n== Work ==\nRelativity.\n",
          "fullurl": "https://en.wikipedia.org/wiki/Albert_Einstein"
        }]
      }
    }"#;
    let (base, rx) = serve(vec![json_ok(body)]);
    let (_tmp, store) = temp_store();
    let ep = endpoints(&base);
    let out = run_open(
        &ctx(&store, &ep),
        &OpenArgs {
            title: "https://en.wikipedia.org/wiki/Albert_Einstein#Early_life".into(),
            lang: "zh".into(),
        },
    )
    .unwrap();
    assert_eq!(out.data["lang"], "en");
    assert!(out.data["variant"].is_null());
    assert!(out.hint.unwrap().contains("article URL"));
    let req = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(req.line.contains("Albert%20Einstein"), "{}", req.line);
    assert!(!req.line.contains("variant="), "{}", req.line);
    assert_eq!(req.header("accept-language"), Some("en"));
}

#[test]
fn open_percent_encoded_zh_url_requests_zh_tw() {
    let body = r#"{"query":{"pages":[{"title":"台北101","extract":"大樓。","canonicalurl":"https://zh.wikipedia.org/wiki/X"}]}}"#;
    let (base, rx) = serve(vec![json_ok(body)]);
    let (_tmp, store) = temp_store();
    let ep = endpoints(&base);
    run_open(
        &ctx(&store, &ep),
        &OpenArgs {
            title: "https://zh.m.wikipedia.org/wiki/%E5%8F%B0%E5%8C%97101".into(),
            lang: "en".into(),
        },
    )
    .unwrap();
    let req = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(req.line.contains("variant=zh-tw"), "{}", req.line);
    assert!(req.line.contains("%E5%8F%B0%E5%8C%97101"), "{}", req.line);
    assert_eq!(req.header("accept-language"), Some("zh-TW"));
}

#[test]
fn missing_page_is_not_found() {
    let body = r#"{"query":{"pages":[{"ns":0,"title":"NoSuchPageXYZ","missing":true,"fullurl":"https://en.wikipedia.org/wiki/NoSuchPageXYZ"}]}}"#;
    let (base, _rx) = serve(vec![json_ok(body)]);
    let (_tmp, store) = temp_store();
    let ep = endpoints(&base);
    let err = run_open(
        &ctx(&store, &ep),
        &OpenArgs {
            title: "NoSuchPageXYZ".into(),
            lang: "en".into(),
        },
    )
    .unwrap_err();
    assert_eq!(err.code, "not_found");
    assert!(err.hint.contains("wiki search"), "{}", err.hint);
    assert!(store.doc_ids().is_empty());
}

#[test]
fn disambiguation_lists_candidates_and_does_not_store() {
    let body = r#"{
      "query": {
        "pages": [{
          "title": "Mercury",
          "canonicalurl": "https://en.wikipedia.org/wiki/Mercury",
          "pageprops": {"disambiguation": ""},
          "extract": "Mercury most commonly refers to:\n\nMercury (planet), the closest planet to the Sun\nMercury (element), a chemical element\n\n== Companies ==\nMercury Marine, a manufacturer\n"
        }]
      }
    }"#;
    let (base, _rx) = serve(vec![json_ok(body)]);
    let (_tmp, store) = temp_store();
    let ep = endpoints(&base);
    let err = run_open(
        &ctx(&store, &ep),
        &OpenArgs {
            title: "Mercury".into(),
            lang: "en".into(),
        },
    )
    .unwrap_err();
    assert_eq!(err.code, "disambiguation");
    let details = err.details.unwrap();
    let titles: Vec<&str> = details["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        vec!["Mercury (planet)", "Mercury (element)", "Mercury Marine"]
    );
    assert!(err.hint.contains("wiki open"), "{}", err.hint);
    assert!(store.doc_ids().is_empty());
}

#[test]
fn http_429_retries_then_succeeds() {
    let (base, rx) = serve(vec![
        http_extra("429 Too Many Requests", "Retry-After: 0\r\n", "{}"),
        json_ok(SEARCH_ZH),
    ]);
    let (_tmp, store) = temp_store();
    let ep = endpoints(&base);
    let out = run_search(
        &ctx(&store, &ep),
        &SearchArgs {
            query: "台北101".into(),
            lang: "zh".into(),
            limit: 2,
        },
    )
    .unwrap();
    assert_eq!(out.data["hits"][0]["title"], "台北101");
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_err());
}

#[test]
fn maxlag_retries_then_succeeds() {
    let lag = r#"{"error":{"code":"maxlag","info":"Waiting for 6 seconds lagged","lag":6}}"#;
    let (base, rx) = serve(vec![
        http_extra("200 OK", "Retry-After: 0\r\n", lag),
        json_ok(SEARCH_ZH),
    ]);
    let (_tmp, store) = temp_store();
    let ep = endpoints(&base);
    let out = run_search(
        &ctx(&store, &ep),
        &SearchArgs {
            query: "台北101".into(),
            lang: "zh".into(),
            limit: 1,
        },
    )
    .unwrap();
    assert_eq!(out.data["hits"].as_array().unwrap().len(), 1);
    assert!(rx.try_recv().is_ok());
    let second = rx.try_recv().unwrap();
    assert!(second.line.contains("maxlag=5"), "{}", second.line);
}

#[test]
fn retries_stop_when_rate_limit_persists() {
    let limited = http_extra("429 Too Many Requests", "Retry-After: 0\r\n", "{}");
    let (base, rx) = serve(vec![limited.clone(), limited.clone(), limited]);
    let (_tmp, store) = temp_store();
    let ep = endpoints(&base);
    let err = run_search(
        &ctx(&store, &ep),
        &SearchArgs {
            query: "台北101".into(),
            lang: "zh".into(),
            limit: 1,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, "rate_limited");
    assert!(err.hint.contains("retry"), "{}", err.hint);
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_err());
}

#[test]
fn bad_args_do_not_call_the_network() {
    let (_tmp, store) = temp_store();
    let ep = Endpoints {
        api: Some("http://127.0.0.1:1/w/api.php".into()),
    };
    let c = ctx(&store, &ep);
    assert_eq!(
        run_search(
            &c,
            &SearchArgs {
                query: "  ".into(),
                lang: "zh".into(),
                limit: 5,
            },
        )
        .unwrap_err()
        .code,
        "bad_args"
    );
    assert_eq!(
        run_search(
            &c,
            &SearchArgs {
                query: "台北101".into(),
                lang: "zh".into(),
                limit: 0,
            },
        )
        .unwrap_err()
        .code,
        "bad_args"
    );
    assert_eq!(
        run_search(
            &c,
            &SearchArgs {
                query: "台北101".into(),
                lang: "../en".into(),
                limit: 5,
            },
        )
        .unwrap_err()
        .code,
        "bad_args"
    );
    let bad_url = run_open(
        &c,
        &OpenArgs {
            title: "https://example.com/wiki/Foo".into(),
            lang: "zh".into(),
        },
    )
    .unwrap_err();
    assert_eq!(bad_url.code, "bad_args");
    assert!(bad_url.hint.contains("wikipedia.org"), "{}", bad_url.hint);
}

#[test]
fn empty_search_is_not_found() {
    let body = r#"{"query":{"searchinfo":{"totalhits":0},"search":[]}}"#;
    let (base, _rx) = serve(vec![json_ok(body)]);
    let (_tmp, store) = temp_store();
    let ep = endpoints(&base);
    let err = run_search(
        &ctx(&store, &ep),
        &SearchArgs {
            query: "zzzzqqqq".into(),
            lang: "zh".into(),
            limit: 5,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, "not_found");
    assert!(err.hint.contains("--lang en"), "{}", err.hint);
    assert!(err.hint.contains("agentbox search"), "{}", err.hint);
}

#[test]
fn schema_mentions_kb_then_wiki_then_duckduckgo() {
    let out = crate::schema::run_schema(true).unwrap();
    let tools = out.data["tools"].as_array().unwrap();
    let desc = |name: &str| {
        tools
            .iter()
            .find(|t| t["function"]["name"] == name)
            .unwrap()["function"]["description"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let wiki = desc("wiki_search");
    assert!(wiki.contains("kb_search"), "{wiki}");
    assert!(wiki.to_lowercase().contains("duckduckgo"), "{wiki}");
    let open = desc("wiki_open");
    assert!(open.contains("doc"), "{open}");
    let search = desc("search");
    assert!(search.contains("wiki_search"), "{search}");
    assert!(search.contains("kb_search"), "{search}");
    let kb = desc("kb_search");
    assert!(kb.contains("wiki_search"), "{kb}");
}
