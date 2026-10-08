//! `market search|get|trending|history`: Polymarket prediction markets,
//! read-only and keyless (Gamma API for events/markets, CLOB API for price
//! history).

#[cfg(test)]
mod tests;

use crate::envelope::{AppError, CmdResult, Output};
use crate::net::{self, encode, jround, round, snippet, Fail};
use crate::state::Store;
use crate::table::{self, parse::Parsed, Table};
use chrono::DateTime;
use serde_json::{json, Map, Value};

pub const SITE: &str = "https://polymarket.com";
const TIMEOUT_SECS: u64 = 25;
const SEARCH_PAGE: usize = 25;
const SEARCH_PAGES: usize = 3;
const SCAN_PAGE: usize = 100;
const SCAN_PAGES: usize = 3;
const MARKETS_IN_LIST: usize = 3;
const HISTORY_ROWS_SHOWN: usize = 40;
const DESCRIPTION_CHARS: usize = 800;
const TAG_EXAMPLES: &str = "politics, elections, crypto, sports, economy, tech, geopolitics";

/// Service base URLs; overridable via env for tests and proxies.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub gamma: String,
    pub clob: String,
}

impl Endpoints {
    pub fn from_env() -> Self {
        Self {
            gamma: net::env_or("AGENTBOX_GAMMA_URL", "https://gamma-api.polymarket.com"),
            clob: net::env_or("AGENTBOX_CLOB_URL", "https://clob.polymarket.com"),
        }
    }
}

pub struct Ctx<'a> {
    pub ep: &'a Endpoints,
    client: reqwest::blocking::Client,
}

impl<'a> Ctx<'a> {
    pub fn new(ep: &'a Endpoints) -> Self {
        Self {
            ep,
            client: net::client(TIMEOUT_SECS),
        }
    }

    fn get_json(&self, base: &str, path_query: &str) -> Result<Value, Fail> {
        let body = net::get(&self.client, &format!("{base}{path_query}"))?;
        serde_json::from_str(&body)
            .map_err(|_| Fail::Status(200, format!("non-JSON answer: {}", snippet(&body, 120))))
    }

    fn gamma(&self, path_query: &str) -> Result<Value, Fail> {
        self.get_json(&self.ep.gamma, path_query)
    }
}

/// Map a transport failure to an envelope error. Connection failures get the
/// DNS-filter hint: some resolvers (RPZ blocklists) sinkhole polymarket.com.
fn fail_error(service: &str, f: Fail) -> AppError {
    match f {
        Fail::Net {
            message,
            timeout,
            dns,
        } => {
            let code = if dns {
                "dns_error"
            } else if timeout {
                "timeout"
            } else {
                "network_error"
            };
            AppError::new(
                code,
                format!("{service} request failed: {message}"),
                "If other sites work, a DNS filter may be blocking Polymarket (some RPZ-based resolvers sinkhole polymarket.com). Check `nslookup gamma-api.polymarket.com` against a public resolver such as 1.1.1.1, or try another network.",
            )
        }
        Fail::Status(429, _) => AppError::new(
            "rate_limited",
            format!("{service} answered HTTP 429"),
            "Polymarket is rate-limiting this network; wait a minute and retry.",
        ),
        Fail::Status(s, body) => AppError::new(
            "http_error",
            format!("{service} answered HTTP {s}: {}", snippet(&body, 160)),
            "Retry later; if it persists the API may have changed.",
        ),
    }
}

// ------------------------------------------------------------- parsing

/// Gamma encodes some arrays as JSON strings (`"[\"Yes\", \"No\"]"`).
pub fn str_list(v: Option<&Value>) -> Vec<String> {
    let arr = match v {
        Some(Value::Array(a)) => a.clone(),
        Some(Value::String(s)) => serde_json::from_str::<Vec<Value>>(s).unwrap_or_default(),
        _ => Vec::new(),
    };
    arr.iter()
        .map(|x| match x {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
        .collect()
}

/// A number that may arrive as a JSON number or a numeric string.
pub fn fnum(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
}

fn money(v: Option<f64>) -> Value {
    v.map_or(Value::Null, |x| jround(x, 0))
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() > max {
        format!("{}…", text.chars().take(max).collect::<String>().trim_end())
    } else {
        text.to_string()
    }
}

/// Probabilities in percent, one per outcome.
fn outcomes(m: &Value) -> Vec<(String, f64)> {
    let names = str_list(m.get("outcomes"));
    let prices = str_list(m.get("outcomePrices"));
    names
        .into_iter()
        .zip(prices)
        .filter_map(|(n, p)| p.parse::<f64>().ok().map(|p| (n, round(p * 100.0, 1))))
        .collect()
}

fn is_yes_no(o: &[(String, f64)]) -> bool {
    o.len() == 2 && o[0].0.eq_ignore_ascii_case("yes") && o[1].0.eq_ignore_ascii_case("no")
}

/// (outcome, pct) used for sorting and tables: Yes for Yes/No markets,
/// otherwise the favourite.
fn headline(o: &[(String, f64)]) -> Option<(String, f64)> {
    if is_yes_no(o) {
        return Some(o[0].clone());
    }
    o.iter()
        .cloned()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
}

fn odds_text(o: &[(String, f64)]) -> String {
    o.iter()
        .map(|(n, p)| format!("{n} {p}%"))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Placeholder rows Polymarket pre-creates ("Person X") have no prices.
fn is_real(m: &Value) -> bool {
    !outcomes(m).is_empty()
}

fn is_closed(v: &Value) -> bool {
    v.get("closed").and_then(Value::as_bool).unwrap_or(false)
}

fn pts(v: Option<f64>) -> Value {
    v.map_or(Value::Null, |x| jround(x * 100.0, 1))
}

fn market_url(event_slug: Option<&str>, m: &Value) -> Value {
    match (event_slug, s(m, "slug")) {
        (Some(e), Some(ms)) => json!(format!("{SITE}/event/{e}/{ms}")),
        (None, Some(ms)) => json!(format!("{SITE}/market/{ms}")),
        (Some(e), None) => json!(format!("{SITE}/event/{e}")),
        _ => Value::Null,
    }
}

fn market_view(m: &Value, event_slug: Option<&str>, full: bool) -> Map<String, Value> {
    let o = outcomes(m);
    let mut v = Map::new();
    v.insert("market_id".into(), json!(s(m, "id")));
    v.insert("question".into(), json!(s(m, "question")));
    if let Some(label) = s(m, "groupItemTitle") {
        v.insert("label".into(), json!(label));
    }
    v.insert(
        "odds".into(),
        if o.is_empty() {
            Value::Null
        } else {
            json!(odds_text(&o))
        },
    );
    if is_yes_no(&o) {
        v.insert("yes_pct".into(), jround(o[0].1, 1));
    } else if let Some((name, p)) = headline(&o) {
        v.insert("leader".into(), json!(name));
        v.insert("leader_pct".into(), jround(p, 1));
    }
    if full {
        v.insert(
            "outcomes".into(),
            json!(o
                .iter()
                .map(|(n, p)| json!({"outcome": n, "pct": jround(*p, 1)}))
                .collect::<Vec<_>>()),
        );
    }
    v.insert(
        "change_1d_pts".into(),
        pts(fnum(m.get("oneDayPriceChange"))),
    );
    if full {
        v.insert(
            "change_1w_pts".into(),
            pts(fnum(m.get("oneWeekPriceChange"))),
        );
        v.insert("last_trade_pct".into(), pts(fnum(m.get("lastTradePrice"))));
    }
    v.insert(
        "volume".into(),
        money(fnum(m.get("volumeNum")).or(fnum(m.get("volume")))),
    );
    v.insert("volume_24h".into(), money(fnum(m.get("volume24hr"))));
    if full {
        v.insert(
            "liquidity".into(),
            money(fnum(m.get("liquidityNum")).or(fnum(m.get("liquidity")))),
        );
    }
    v.insert("end_date".into(), json!(s(m, "endDate")));
    v.insert("closed".into(), json!(is_closed(m)));
    v.insert("slug".into(), json!(s(m, "slug")));
    if full {
        v.insert("url".into(), market_url(event_slug, m));
    }
    v
}

/// Real markets, open ones first, then by headline probability.
fn sorted_markets(e: &Value) -> Vec<&Value> {
    let mut ms: Vec<&Value> = e
        .get("markets")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter(|m| is_real(m)).collect())
        .unwrap_or_default();
    ms.sort_by(|a, b| {
        let key = |m: &Value| headline(&outcomes(m)).map_or(0.0, |h| h.1);
        is_closed(a).cmp(&is_closed(b)).then(
            key(b)
                .partial_cmp(&key(a))
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    ms
}

fn tag_slugs(e: &Value) -> Value {
    let tags: Vec<String> = e
        .get("tags")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|t| s(t, "slug")).collect())
        .unwrap_or_default();
    if tags.is_empty() {
        Value::Null
    } else {
        json!(tags.join(", "))
    }
}

fn event_url(e: &Value) -> Value {
    s(e, "slug").map_or(Value::Null, |sl| json!(format!("{SITE}/event/{sl}")))
}

fn event_view(e: &Value, max_markets: usize, full: bool) -> Map<String, Value> {
    let ms = sorted_markets(e);
    let slug = s(e, "slug");
    let mut v = Map::new();
    v.insert("event_id".into(), json!(s(e, "id")));
    v.insert("title".into(), json!(s(e, "title")));
    v.insert("slug".into(), json!(slug));
    v.insert("url".into(), event_url(e));
    v.insert("active".into(), json!(!is_closed(e)));
    v.insert("end_date".into(), json!(s(e, "endDate")));
    v.insert("volume".into(), money(fnum(e.get("volume"))));
    v.insert("volume_24h".into(), money(fnum(e.get("volume24hr"))));
    v.insert("liquidity".into(), money(fnum(e.get("liquidity"))));
    v.insert("tags".into(), tag_slugs(e));
    if full {
        if let Some(d) = s(e, "description") {
            v.insert("description".into(), json!(truncate(&d, DESCRIPTION_CHARS)));
        }
        v.insert("resolution_source".into(), json!(s(e, "resolutionSource")));
        v.insert("start_date".into(), json!(s(e, "startDate")));
    }
    let open = ms.iter().filter(|m| !is_closed(m)).count();
    v.insert("markets_total".into(), json!(ms.len()));
    v.insert("markets_open".into(), json!(open));
    // Lists skip settled sub-markets of live events; `get` shows everything.
    let pool: Vec<&Value> = if !full && open > 0 {
        ms.iter().copied().filter(|m| !is_closed(m)).collect()
    } else {
        ms.clone()
    };
    let shown: Vec<Value> = pool
        .iter()
        .take(max_markets)
        .map(|m| Value::Object(market_view(m, slug.as_deref(), full)))
        .collect();
    if ms.len() > shown.len() {
        v.insert("markets_omitted".into(), json!(ms.len() - shown.len()));
    }
    v.insert("markets".into(), json!(shown));
    v
}

/// Market-level rows for `--save-table`.
pub const TABLE_COLUMNS: &[&str] = &[
    "event",
    "market",
    "outcome",
    "pct",
    "odds",
    "change_1d_pts",
    "volume_24h",
    "volume",
    "end_date",
    "url",
    "market_id",
];

fn table_rows(events: &[&Value]) -> Vec<Vec<Value>> {
    let mut rows = Vec::new();
    for e in events {
        let slug = s(e, "slug");
        let active = !is_closed(e);
        for m in sorted_markets(e) {
            if active && is_closed(m) {
                continue;
            }
            let o = outcomes(m);
            let h = headline(&o);
            rows.push(vec![
                json!(s(e, "title")),
                json!(s(m, "groupItemTitle").or_else(|| s(m, "question"))),
                json!(h.as_ref().map(|x| x.0.clone())),
                h.as_ref().map_or(Value::Null, |x| jround(x.1, 1)),
                json!(odds_text(&o)),
                pts(fnum(m.get("oneDayPriceChange"))),
                money(fnum(m.get("volume24hr"))),
                money(fnum(m.get("volumeNum")).or(fnum(m.get("volume")))),
                json!(s(m, "endDate")),
                market_url(slug.as_deref(), m),
                json!(s(m, "id")),
            ]);
        }
    }
    rows
}

fn save_rows(
    store: &Store,
    title: &str,
    source: &str,
    events: &[&Value],
) -> Result<(u64, usize), AppError> {
    let rows = table_rows(events);
    let n = rows.len();
    let t = Table::new(
        title,
        source,
        Parsed {
            columns: TABLE_COLUMNS.iter().map(|c| c.to_string()).collect(),
            rows,
        },
    );
    Ok((table::save(store, t)?, n))
}

// -------------------------------------------------------------- search

#[derive(Debug, Clone)]
pub struct SearchArgs {
    pub query: String,
    pub limit: usize,
    pub active: bool,
    pub closed: bool,
    pub sort: String,
    pub tag: Option<String>,
    pub save_table: bool,
}

/// "active", "closed" or `None` (both) from the two flags; neither = active.
fn status(active: bool, closed: bool) -> Option<&'static str> {
    match (active, closed) {
        (true, true) => None,
        (false, true) => Some("closed"),
        _ => Some("active"),
    }
}

fn haystack(e: &Value) -> String {
    let mut h = String::new();
    for k in ["title", "description", "slug", "ticker"] {
        if let Some(x) = s(e, k) {
            h.push_str(&x);
            h.push(' ');
        }
    }
    for m in e
        .get("markets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for k in ["question", "groupItemTitle"] {
            if let Some(x) = s(m, k) {
                h.push_str(&x);
                h.push(' ');
            }
        }
    }
    for t in e
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for k in ["label", "slug"] {
            if let Some(x) = s(t, k) {
                h.push_str(&x);
                h.push(' ');
            }
        }
    }
    h.to_lowercase()
}

/// Every query word occurs (case-insensitively) somewhere in the event.
pub fn matches_all(e: &Value, query: &str) -> bool {
    let h = haystack(e);
    query
        .to_lowercase()
        .split_whitespace()
        .all(|w| h.contains(w))
}

fn status_ok(e: &Value, st: Option<&str>) -> bool {
    match st {
        Some("active") => !is_closed(e),
        Some("closed") => is_closed(e),
        _ => true,
    }
}

fn sort_events(events: &mut [Value], sort: &str) {
    let f = |e: &Value, k: &str| fnum(e.get(k)).unwrap_or(0.0);
    let date = |e: &Value, k: &str| s(e, k).unwrap_or_default();
    match sort {
        "liquidity" => events.sort_by(|a, b| f(b, "liquidity").total_cmp(&f(a, "liquidity"))),
        "end" => events.sort_by(|a, b| {
            let (x, y) = (date(a, "endDate"), date(b, "endDate"));
            // Missing end dates last.
            x.is_empty().cmp(&y.is_empty()).then(x.cmp(&y))
        }),
        "newest" => events.sort_by(|a, b| {
            let k = |e: &Value| {
                s(e, "startDate")
                    .or_else(|| s(e, "createdAt"))
                    .unwrap_or_default()
            };
            k(b).cmp(&k(a))
        }),
        _ => events.sort_by(|a, b| f(b, "volume").total_cmp(&f(a, "volume"))),
    }
}

fn dedupe(events: Vec<Value>) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    events
        .into_iter()
        .filter(|e| seen.insert(s(e, "id").or_else(|| s(e, "slug")).unwrap_or_default()))
        .collect()
}

/// Pool from Gamma's public search, several pages deep.
fn search_pool(ctx: &Ctx, a: &SearchArgs, st: Option<&str>) -> Result<(Vec<Value>, bool), Fail> {
    let mut pool = Vec::new();
    let mut more = false;
    for page in 1..=SEARCH_PAGES {
        let mut pq = format!(
            "/public-search?q={}&limit_per_type={SEARCH_PAGE}&page={page}&search_profiles=false&search_tags=false",
            encode(a.query.trim())
        );
        if let Some(st) = st {
            pq.push_str(&format!("&events_status={st}"));
        }
        if matches!(a.sort.as_str(), "volume" | "liquidity") {
            pq.push_str(&format!("&sort={}&ascending=false", a.sort));
        }
        if let Some(t) = &a.tag {
            pq.push_str(&format!("&events_tag={}", encode(t)));
        }
        let v = ctx.gamma(&pq)?;
        let Some(events) = v.get("events").and_then(Value::as_array) else {
            if v.get("events").is_some_and(Value::is_null) {
                break;
            }
            return Err(Fail::Status(200, "unexpected search answer shape".into()));
        };
        pool.extend(events.iter().cloned());
        more = v
            .pointer("/pagination/hasMore")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !more || events.is_empty() {
            more = false;
            break;
        }
    }
    Ok((pool, more))
}

/// Fallback pool: scan the most active events and filter locally.
fn scan_pool(ctx: &Ctx, tag: Option<&str>, st: Option<&str>) -> Result<Vec<Value>, Fail> {
    let mut pool = Vec::new();
    for page in 0..SCAN_PAGES {
        let mut pq = format!(
            "/events?limit={SCAN_PAGE}&offset={}&order=volume24hr&ascending=false",
            page * SCAN_PAGE
        );
        match st {
            Some("active") => pq.push_str("&active=true&closed=false"),
            Some("closed") => pq.push_str("&closed=true"),
            _ => {}
        }
        if let Some(t) = tag {
            pq.push_str(&format!("&tag_slug={}", encode(t)));
        }
        let v = ctx.gamma(&pq)?;
        let events = v.as_array().cloned().unwrap_or_default();
        let n = events.len();
        pool.extend(events);
        if n < SCAN_PAGE {
            break;
        }
    }
    Ok(pool)
}

pub fn run_search(store: &Store, ctx: &Ctx, a: &SearchArgs) -> CmdResult {
    let q = a.query.trim();
    if q.is_empty() {
        return Err(AppError::new(
            "bad_args",
            "empty search query",
            "Example: `agentbox market search \"fed rate\"`, or `agentbox market trending` for the busiest markets.",
        ));
    }
    let limit = a.limit.clamp(1, 50);
    let st = status(a.active, a.closed);
    let mut capped = false;
    let (pool, via) = match search_pool(ctx, a, st) {
        Ok((p, more)) => {
            capped = more;
            (p, "public-search")
        }
        Err(f @ Fail::Net { .. }) => return Err(fail_error("Polymarket", f)),
        Err(_) => (
            scan_pool(ctx, a.tag.as_deref(), st).map_err(|f| fail_error("Polymarket", f))?,
            "events-scan",
        ),
    };
    let pool: Vec<Value> = dedupe(pool)
        .into_iter()
        .filter(|e| status_ok(e, st))
        .collect();
    let pool_size = pool.len();
    let (mut events, mode): (Vec<Value>, &str) = {
        let strict: Vec<Value> = pool.iter().filter(|e| matches_all(e, q)).cloned().collect();
        if strict.is_empty() && via == "public-search" {
            (pool, "fuzzy")
        } else {
            (strict, "keyword")
        }
    };
    if events.is_empty() {
        let status_tip = match st {
            Some("active") => " Add --closed to include resolved markets.",
            _ => "",
        };
        return Err(AppError::new(
            "no_results",
            format!("no Polymarket events match \"{q}\" (searched {pool_size} events)"),
            format!(
                "Try fewer or broader words (e.g. one keyword), or `agentbox market trending`.{status_tip}"
            ),
        ));
    }
    sort_events(&mut events, &a.sort);
    let total = events.len();
    events.truncate(limit);
    let views: Vec<Value> = events
        .iter()
        .map(|e| Value::Object(event_view(e, MARKETS_IN_LIST, false)))
        .collect();
    let mut data = json!({
        "query": q,
        "status": st.unwrap_or("all"),
        "sort": a.sort,
        "via": via,
        "match": mode,
        "total_matches": total,
        "more_available": capped,
        "returned": views.len(),
        "events": views,
    });
    let first = s(&events[0], "slug").unwrap_or_default();
    let mut hint = String::new();
    if mode == "fuzzy" {
        hint.push_str("No event mentions every keyword; these are Polymarket's closest matches. Try fewer or different words. ");
    }
    if total > limit {
        let plus = if capped { "+" } else { "" };
        hint.push_str(&format!(
            "Showing {limit} of {total}{plus} matches; raise --limit, add words or --tag to narrow. "
        ));
    }
    if a.save_table {
        let refs: Vec<&Value> = events.iter().collect();
        let (id, n) = save_rows(
            store,
            &format!("Polymarket: {q}"),
            &format!("market:search:{q}"),
            &refs,
        )?;
        data["table"] = json!(format!("tbl:{id}"));
        hint.push_str(&format!(
            "{n} markets saved as tbl:{id}; e.g. `agentbox table query tbl:{id} --sort -volume_24h --limit 10`. "
        ));
    }
    hint.push_str(&format!(
        "All outcomes and rules: `agentbox market get {first}`; odds over time: `agentbox market history {first}`."
    ));
    Ok(Output::new(data).hint(hint))
}

// ------------------------------------------------------------ trending

#[derive(Debug, Clone)]
pub struct TrendingArgs {
    pub limit: usize,
    pub tag: Option<String>,
    pub save_table: bool,
}

pub fn run_trending(store: &Store, ctx: &Ctx, a: &TrendingArgs) -> CmdResult {
    let limit = a.limit.clamp(1, 50);
    let mut pq =
        format!("/events?active=true&closed=false&order=volume24hr&ascending=false&limit={limit}");
    if let Some(t) = &a.tag {
        pq.push_str(&format!("&tag_slug={}", encode(t.trim())));
    }
    let v = ctx.gamma(&pq).map_err(|f| fail_error("Polymarket", f))?;
    let events = v.as_array().cloned().unwrap_or_default();
    if events.is_empty() {
        return Err(AppError::new(
            "no_results",
            match &a.tag {
                Some(t) => format!("no active events with tag `{t}`"),
                None => "Polymarket returned no active events".to_string(),
            },
            format!("Tags are slugs such as {TAG_EXAMPLES}; every event lists its tags in `tags`."),
        ));
    }
    let views: Vec<Value> = events
        .iter()
        .map(|e| Value::Object(event_view(e, MARKETS_IN_LIST, false)))
        .collect();
    let mut data = json!({
        "tag": a.tag,
        "sort": "volume_24h",
        "returned": views.len(),
        "events": views,
    });
    let first = s(&events[0], "slug").unwrap_or_default();
    let mut hint = String::new();
    if a.save_table {
        let refs: Vec<&Value> = events.iter().collect();
        let label = a.tag.as_deref().unwrap_or("all");
        let (id, n) = save_rows(
            store,
            &format!("Polymarket trending ({label})"),
            &format!("market:trending:{label}"),
            &refs,
        )?;
        data["table"] = json!(format!("tbl:{id}"));
        hint.push_str(&format!(
            "{n} markets saved as tbl:{id}; e.g. `agentbox table query tbl:{id} --where \"pct >= 50\" --sort -volume_24h`. "
        ));
    }
    hint.push_str(&format!(
        "Details: `agentbox market get {first}`. Narrow by topic with --tag ({TAG_EXAMPLES}) or `agentbox market search KEYWORD`."
    ));
    Ok(Output::new(data).hint(hint))
}

// ----------------------------------------------------------------- get

/// What the user pointed at: an id or slugs, possibly inside a URL.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    Id(String),
    Slug {
        event: String,
        market: Option<String>,
    },
}

pub fn parse_target(raw: &str) -> Option<Target> {
    let t = raw.trim().trim_end_matches('/');
    if t.is_empty() {
        return None;
    }
    let path = match t.find("polymarket.com/") {
        Some(i) => &t[i + "polymarket.com/".len()..],
        None => t,
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        ["event", e] => Some(Target::Slug {
            event: e.to_string(),
            market: None,
        }),
        ["event", e, m, ..] => Some(Target::Slug {
            event: e.to_string(),
            market: Some(m.to_string()),
        }),
        ["market", m, ..] => Some(Target::Slug {
            event: String::new(),
            market: Some(m.to_string()),
        }),
        [one] if one.bytes().all(|b| b.is_ascii_digit()) => Some(Target::Id(one.to_string())),
        [one] => Some(Target::Slug {
            event: one.to_string(),
            market: None,
        }),
        _ => None,
    }
}

enum Found {
    Event(Value),
    Market(Value),
}

/// 404 and 422 (bad id) mean "try the other kind".
fn missing(f: &Fail) -> bool {
    matches!(f, Fail::Status(404 | 422, _))
}

fn lookup(ctx: &Ctx, t: &Target, prefer_market: bool) -> Result<Found, AppError> {
    let err = |f| fail_error("Polymarket", f);
    let try_event = |path: String| -> Result<Option<Value>, AppError> {
        match ctx.gamma(&path) {
            Ok(v) if v.is_object() => Ok(Some(v)),
            Ok(Value::Array(a)) => Ok(a.into_iter().next()),
            Ok(_) => Ok(None),
            Err(f) if missing(&f) => Ok(None),
            Err(f) => Err(err(f)),
        }
    };
    let (event_path, market_path) = match t {
        Target::Id(id) => (
            Some(format!("/events/{}", encode(id))),
            Some(format!("/markets/{}", encode(id))),
        ),
        Target::Slug { event, market } => (
            (!event.is_empty() && market.is_none())
                .then(|| format!("/events/slug/{}", encode(event))),
            Some(format!(
                "/markets/slug/{}",
                encode(market.as_deref().unwrap_or(event))
            )),
        ),
    };
    let order: Vec<(bool, String)> = if prefer_market {
        market_path
            .into_iter()
            .map(|p| (true, p))
            .chain(event_path.map(|p| (false, p)))
            .collect()
    } else {
        event_path
            .into_iter()
            .map(|p| (false, p))
            .chain(market_path.map(|p| (true, p)))
            .collect()
    };
    for (is_market, path) in order {
        if let Some(v) = try_event(path)? {
            return Ok(if is_market {
                Found::Market(v)
            } else {
                Found::Event(v)
            });
        }
    }
    let shown = match t {
        Target::Id(id) => id.clone(),
        Target::Slug { event, market } => market.clone().unwrap_or_else(|| event.clone()),
    };
    Err(AppError::new(
        "not_found",
        format!("no Polymarket event or market `{shown}`"),
        "Use a slug or id from `agentbox market search KEYWORD` or `agentbox market trending` (event URLs work too).",
    ))
}

fn bad_target(raw: &str) -> AppError {
    AppError::new(
        "bad_args",
        format!("`{raw}` is not a Polymarket slug, id or URL"),
        "Pass the `slug` from `agentbox market search`, e.g. `agentbox market get balance-of-power-2026-midterms`.",
    )
}

pub fn run_get(ctx: &Ctx, raw: &str, limit: usize) -> CmdResult {
    let t = parse_target(raw).ok_or_else(|| bad_target(raw))?;
    let prefer_market = matches!(
        &t,
        Target::Slug {
            market: Some(_),
            ..
        }
    );
    let found = lookup(ctx, &t, prefer_market)?;
    let (data, slug_for_hint) = match found {
        Found::Event(e) => {
            let v = event_view(&e, limit.max(1), true);
            let ms = sorted_markets(&e);
            let target = match ms.as_slice() {
                [_] => s(&e, "slug"),
                [first, ..] => s(first, "slug"),
                [] => None,
            }
            .unwrap_or_default();
            (json!({"kind": "event", "event": v}), target)
        }
        Found::Market(m) => {
            let ev = m
                .get("events")
                .and_then(Value::as_array)
                .and_then(|a| a.first())
                .cloned();
            let ev_slug = ev.as_ref().and_then(|e| s(e, "slug"));
            let mut v = market_view(&m, ev_slug.as_deref(), true);
            if let Some(d) = s(&m, "description") {
                v.insert("description".into(), json!(truncate(&d, DESCRIPTION_CHARS)));
            }
            if let Some(e) = &ev {
                v.insert("event_title".into(), json!(s(e, "title")));
                v.insert("event_slug".into(), json!(ev_slug));
            }
            (
                json!({"kind": "market", "market": v}),
                s(&m, "slug").unwrap_or_default(),
            )
        }
    };
    let mut hint = String::new();
    if let Some(n) = data
        .pointer("/event/markets_omitted")
        .and_then(Value::as_u64)
    {
        hint.push_str(&format!(
            "{n} lower-probability markets omitted; raise --limit to see them. "
        ));
    }
    hint.push_str(&format!(
        "Probabilities are market prices, not forecasts. Odds over time: `agentbox market history {slug_for_hint}`."
    ));
    Ok(Output::new(data).hint(hint))
}

// ------------------------------------------------------------- history

#[derive(Debug, Clone)]
pub struct HistoryArgs {
    pub target: String,
    pub interval: String,
    pub save: bool,
}

fn fidelity(interval: &str) -> (&'static str, u32) {
    match interval {
        "1d" => ("1d", 60),
        "1w" => ("1w", 360),
        "1m" => ("1m", 1440),
        _ => ("max", 1440),
    }
}

fn iso_utc(t: i64) -> Value {
    DateTime::from_timestamp(t, 0).map_or(Value::Null, |d| {
        json!(d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    })
}

/// Pick the one market to chart, or explain which ones exist.
fn single_market(ctx: &Ctx, raw: &str) -> Result<(Value, Option<Value>), AppError> {
    let t = parse_target(raw).ok_or_else(|| bad_target(raw))?;
    let prefer_market = matches!(
        &t,
        Target::Id(_)
            | Target::Slug {
                market: Some(_),
                ..
            }
    );
    match lookup(ctx, &t, prefer_market)? {
        Found::Market(m) => Ok((m, None)),
        Found::Event(e) => {
            let ms = sorted_markets(&e);
            let open: Vec<&Value> = ms.iter().copied().filter(|m| !is_closed(m)).collect();
            let pick = if ms.len() == 1 {
                Some(ms[0])
            } else if open.len() == 1 {
                Some(open[0])
            } else {
                None
            };
            if let Some(m) = pick {
                return Ok((m.clone(), Some(e.clone())));
            }
            let list: Vec<String> = (if open.is_empty() { &ms } else { &open })
                .iter()
                .take(6)
                .map(|m| {
                    let label = s(m, "groupItemTitle")
                        .or_else(|| s(m, "question"))
                        .unwrap_or_default();
                    let pct = headline(&outcomes(m))
                        .map(|h| format!(" {}%", h.1))
                        .unwrap_or_default();
                    format!("{label}{pct} → {}", s(m, "slug").unwrap_or_default())
                })
                .collect();
            let first = open
                .first()
                .or(ms.first())
                .and_then(|m| s(m, "slug"))
                .unwrap_or_default();
            Err(AppError::new(
                "ambiguous_market",
                format!(
                    "event `{}` has {} markets; history needs one. Top: {}",
                    s(&e, "slug").unwrap_or_default(),
                    ms.len(),
                    list.join("; ")
                ),
                format!("Pick a market slug, e.g. `agentbox market history {first}`."),
            ))
        }
    }
}

pub fn run_history(store: &Store, ctx: &Ctx, a: &HistoryArgs) -> CmdResult {
    let (m, event) = single_market(ctx, &a.target)?;
    let o = outcomes(&m);
    let tokens = str_list(m.get("clobTokenIds"));
    let Some(token) = tokens.first() else {
        return Err(AppError::new(
            "no_data",
            "this market has no order-book token, so it has no price history",
            "Older AMM-era markets lack history; try another market.",
        ));
    };
    let (iv, fid) = fidelity(&a.interval);
    let v = ctx
        .get_json(
            &ctx.ep.clob,
            &format!(
                "/prices-history?market={}&interval={iv}&fidelity={fid}",
                encode(token)
            ),
        )
        .map_err(|f| fail_error("Polymarket CLOB", f))?;
    let points: Vec<(i64, f64)> = v
        .get("history")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| Some((p.get("t")?.as_i64()?, fnum(p.get("p"))?)))
                .collect()
        })
        .unwrap_or_default();
    if points.is_empty() {
        return Err(AppError::new(
            "no_data",
            format!("no price history for this market over `{}`", a.interval),
            "New or thinly traded markets may have no trades yet; try `--interval max`.",
        ));
    }
    let outcome = o
        .first()
        .map(|x| x.0.clone())
        .unwrap_or_else(|| "Yes".into());
    let pct = |p: f64| jround(p * 100.0, 1);
    let (t0, p0) = points[0];
    let (t1, p1) = points[points.len() - 1];
    let hi = points
        .iter()
        .cloned()
        .fold(points[0], |a, b| if b.1 > a.1 { b } else { a });
    let lo = points
        .iter()
        .cloned()
        .fold(points[0], |a, b| if b.1 < a.1 { b } else { a });
    let rows: Vec<Vec<Value>> = points
        .iter()
        .map(|(t, p)| vec![iso_utc(*t), pct(*p)])
        .collect();
    let columns = vec![
        "time".to_string(),
        format!("{}_pct", outcome.to_lowercase().replace(' ', "_")),
    ];
    let total = rows.len();
    let from = total.saturating_sub(HISTORY_ROWS_SHOWN);
    let objs: Vec<Value> = rows[from..]
        .iter()
        .map(|r| json!({ columns[0].clone(): r[0], columns[1].clone(): r[1] }))
        .collect();
    let event_slug = event.as_ref().and_then(|e| s(e, "slug"));
    let mut data = json!({
        "market_id": s(&m, "id"),
        "question": s(&m, "question"),
        "outcome": outcome,
        "url": market_url(event_slug.as_deref(), &m),
        "interval": a.interval,
        "summary": {
            "start_time": iso_utc(t0),
            "start_pct": pct(p0),
            "end_time": iso_utc(t1),
            "end_pct": pct(p1),
            "change_pts": jround((p1 - p0) * 100.0, 1),
            "high_pct": pct(hi.1),
            "high_time": iso_utc(hi.0),
            "low_pct": pct(lo.1),
            "low_time": iso_utc(lo.0),
            "points": total,
        },
        "columns": columns,
        "rows": objs,
        "rows_total": total,
    });
    let mut hint = format!(
        "Probability of `{outcome}` (times in UTC); change_pts is percentage points over the window. "
    );
    if from > 0 {
        data["truncated"] = json!(true);
        hint.push_str(&format!(
            "Showing the last {HISTORY_ROWS_SHOWN} of {total} points. "
        ));
    }
    if a.save {
        let t = Table::new(
            &format!("{} ({})", s(&m, "question").unwrap_or_default(), a.interval),
            &format!("market:history:{}", s(&m, "id").unwrap_or_default()),
            Parsed {
                columns: data["columns"]
                    .as_array()
                    .map(|c| {
                        c.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default(),
                rows,
            },
        );
        let id = table::save(store, t)?;
        data["table"] = json!(format!("tbl:{id}"));
        hint.push_str(&format!("All {total} points saved as tbl:{id}."));
    } else {
        hint.push_str("Add --save to keep all points as a tbl:N.");
    }
    Ok(Output::new(data).hint(hint))
}
