//! CrUX popular-sites import.
//!
//! Ranks are magnitude buckets, not positions. Origins are folded to a
//! registrable domain with the public suffix list. Adult, infrastructure,
//! and backend hostnames are dropped. See `popular_denylist.txt`.

use super::curated::known_category;
use super::db::{self, Record};
use crate::envelope::AppError;
use flate2::read::GzDecoder;
use regex::Regex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;

pub const CRUX_LICENSE: &str = "CC-BY-4.0";
pub const SEED_TW: &[u8] = include_bytes!("popular_tw.json.gz");
pub const SEED_GLOBAL: &[u8] = include_bytes!("popular_global.json.gz");

const NOISE_LABELS: &[&str] = &[
    "login",
    "signin",
    "signon",
    "sso",
    "auth",
    "oauth",
    "accounts",
    "account",
    "admin",
    "administrator",
    "api",
    "apis",
    "cdn",
    "static",
    "assets",
    "img",
    "images",
    "cart",
    "checkout",
    "basket",
    "webmail",
    "mail",
    "smtp",
    "ads",
    "ad",
    "analytics",
    "pixel",
    "track",
    "tracking",
    "telemetry",
    "metrics",
    "staging",
    "stage",
    "sandbox",
    "moodle",
    "eeclass",
    "elearn",
    "ilearn",
    "lms",
    "amp",
    "ns",
    "dns",
];

/// Public suffixes that host many unrelated sites. One CrUX origin there is
/// not "the" homepage of that suffix.
const SHARED_SUFFIXES: &[&str] = &["github.io", "blogspot.com", "blogspot.tw", "wordpress.com"];

const ADULT_PARTS: &[&str] = &[
    "porn", "xvideo", "xnxx", "xhamster", "hentai", "onlyfans", "pornhub", "redtube", "youporn",
    "missav", "nhentai", "hanime", "18comic", "18av", "18porn", "18jav", "avday", "7mmtv",
];

#[derive(Debug, Clone, Default)]
pub struct PrepareStats {
    pub input_rows: usize,
    pub kept_domains: usize,
    pub dropped_rank: usize,
    pub dropped_adult: usize,
    pub dropped_infra: usize,
    pub dropped_ip: usize,
    pub dropped_noise_only: usize,
    pub dropped_other: usize,
}

#[derive(Debug, Clone)]
struct Origin {
    url: String,
    host: String,
    https: bool,
    bucket: u32,
}

#[derive(Debug, Default, Deserialize)]
struct AliasFile {
    #[serde(default)]
    site: Vec<AliasSite>,
}

#[derive(Debug, Deserialize)]
struct AliasSite {
    domain: String,
    #[serde(default)]
    name_zh: String,
    #[serde(default)]
    name_en: String,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    category: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedFile {
    pub source: String,
    pub license: String,
    pub month: String,
    pub scope: String,
    pub records: Vec<SeedRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeedRecord {
    pub id: String,
    pub name_zh: String,
    pub name_en: String,
    pub aliases: Vec<String>,
    pub category: String,
    pub official_url: String,
    pub alt_urls: Vec<String>,
    pub description: String,
    pub source: String,
    pub source_license: String,
    pub last_verified: String,
    pub verify_status: String,
    pub popularity_bucket: i64,
    pub popularity_scope: String,
}

pub fn prepare_csv(
    csv: &str,
    scope: &str,
    max_rank: u32,
    month: &str,
) -> (Vec<Record>, PrepareStats) {
    let mut stats = PrepareStats::default();
    let mut groups: HashMap<String, Vec<Origin>> = HashMap::new();
    for line in csv.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.to_ascii_lowercase().starts_with("origin,") {
            continue;
        }
        stats.input_rows += 1;
        let Some((origin, rank)) = line.split_once(',') else {
            stats.dropped_other += 1;
            continue;
        };
        let Ok(rank) = rank.trim().parse::<u32>() else {
            stats.dropped_other += 1;
            continue;
        };
        if rank == 0 || rank > max_rank {
            stats.dropped_rank += 1;
            continue;
        }
        let Some((https, host)) = split_origin(origin) else {
            stats.dropped_other += 1;
            continue;
        };
        if is_ip_host(&host) {
            stats.dropped_ip += 1;
            continue;
        }
        let Some(domain) = registrable(&host) else {
            stats.dropped_other += 1;
            continue;
        };
        if shared_suffix(&host) {
            stats.dropped_infra += 1;
            continue;
        }
        if is_adult_host(&host) || adult_domain(&domain) {
            stats.dropped_adult += 1;
            continue;
        }
        if is_infra_domain(&domain) || is_infra_domain(&host) {
            stats.dropped_infra += 1;
            continue;
        }
        groups.entry(domain).or_default().push(Origin {
            url: origin.trim().trim_end_matches('/').to_string(),
            host,
            https,
            bucket: rank,
        });
    }

    let mut records = Vec::new();
    for (domain, origins) in groups {
        let bucket = origins.iter().map(|o| o.bucket).min().unwrap_or(max_rank);
        let homepage = origins
            .iter()
            .filter(|o| is_apex_or_www(&o.host, &domain))
            .min_by_key(|o| homepage_rank(o, &domain));
        let useful: Vec<&Origin> = origins
            .iter()
            .filter(|o| !is_noise_host(&o.host, &domain))
            .collect();
        let (url, alts) = if let Some(home) = homepage {
            let alts = useful
                .iter()
                .filter(|o| o.host != home.host)
                .map(|o| o.url.clone())
                .take(6)
                .collect::<Vec<_>>();
            (home.url.clone(), alts)
        } else if useful.is_empty() {
            stats.dropped_noise_only += 1;
            continue;
        } else {
            let alts = useful
                .iter()
                .map(|o| o.url.clone())
                .take(6)
                .collect::<Vec<_>>();
            (format!("https://{domain}"), alts)
        };
        let mut rec = Record::blank(format!("pop:{domain}"), "org");
        rec.name_en = domain.clone();
        rec.official_url = if url.starts_with("http") {
            url
        } else {
            format!("https://{url}")
        };
        rec.alt_urls = alts;
        rec.category = guess_category(&domain, "", "");
        rec.source = format!("crux:{scope}:{month}");
        rec.source_license = CRUX_LICENSE.to_string();
        rec.verify_status = "unverified".to_string();
        rec.popularity_bucket = Some(i64::from(bucket));
        rec.popularity_scope = scope.to_string();
        rec.description =
            format!("Chrome UX Report {scope} {month}, popularity bucket {bucket}. CC BY 4.0.");
        records.push(rec);
    }
    apply_overlay(&mut records);
    let dropped = polish_records(&mut records);
    stats.dropped_adult += dropped;
    records.sort_by(|a, b| a.id.cmp(&b.id));
    stats.kept_domains = records.len();
    (records, stats)
}

pub fn read_crux_file(path: &Path) -> Result<String, AppError> {
    let bytes =
        std::fs::read(path).map_err(|e| AppError::io(&format!("read {}", path.display()), e))?;
    let gz =
        path.extension().and_then(|e| e.to_str()) == Some("gz") || bytes.starts_with(&[0x1f, 0x8b]);
    if gz {
        let mut dec = GzDecoder::new(&bytes[..]);
        let mut s = String::new();
        dec.read_to_string(&mut s).map_err(|e| {
            AppError::new(
                "bad_kb",
                format!("gzip {}: {e}", path.display()),
                "Pass a CrUX csv.gz from zakird/crux-top-lists, for example data/country/tw/202610.csv.gz.",
            )
        })?;
        Ok(s)
    } else {
        String::from_utf8(bytes).map_err(|_| {
            AppError::new(
                "bad_kb",
                format!("{} is not UTF-8", path.display()),
                "CrUX files are UTF-8 CSV, optionally gzipped.",
            )
        })
    }
}

pub fn month_from_path(path: &str) -> String {
    let digits: String = path.chars().filter(|c| c.is_ascii_digit()).collect();
    if let Some(idx) = digits.find("202") {
        let rest = &digits[idx..];
        if rest.len() >= 6 {
            return rest[..6].to_string();
        }
    }
    "unknown".to_string()
}

/// HEAD/GET the homepage. Not used by unit tests.
pub fn verify_records(records: &mut [Record]) {
    const WORKERS: usize = 16;
    let mut slices: Vec<&mut [Record]> = Vec::new();
    let chunk = records.len().div_ceil(WORKERS).max(1);
    let mut rest = records;
    while !rest.is_empty() {
        let n = chunk.min(rest.len());
        let (head, tail) = rest.split_at_mut(n);
        slices.push(head);
        rest = tail;
    }
    std::thread::scope(|scope| {
        for slice in slices {
            scope.spawn(|| {
                let client = match reqwest::blocking::Client::builder()
                    .user_agent(concat!(
                        "agentbox/",
                        env!("CARGO_PKG_VERSION"),
                        " crux-seed"
                    ))
                    .timeout(std::time::Duration::from_secs(8))
                    .redirect(reqwest::redirect::Policy::limited(5))
                    .build()
                {
                    Ok(c) => c,
                    Err(_) => return,
                };
                for rec in slice.iter_mut() {
                    let (status, title) = probe(&client, &rec.official_url);
                    rec.verify_status = status.to_string();
                    rec.last_verified = chrono::Utc::now().format("%Y-%m-%d").to_string();
                    apply_title(rec, &title);
                }
            });
        }
    });
}

/// Best-effort Wikidata P856 lookup. Failures leave titles/overlays in place.
pub fn enrich_wikidata(records: &mut [Record]) -> usize {
    let domains: Vec<String> = records
        .iter()
        .filter_map(|r| r.id.strip_prefix("pop:").map(|s| s.to_string()))
        .collect();
    let mut hits: HashMap<String, WikiHit> = HashMap::new();
    for chunk in domains.chunks(20) {
        if let Some(map) = wikidata_batch(chunk) {
            hits.extend(map);
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let mut n = 0;
    for rec in records {
        let Some(domain) = rec.id.strip_prefix("pop:") else {
            continue;
        };
        let Some(hit) = hits.get(domain) else {
            continue;
        };
        if rec.name_zh.is_empty() {
            if let Some(name) = hit.zh.clone() {
                rec.name_zh = name;
            }
        }
        if rec.name_en.is_empty() || rec.name_en == domain {
            if let Some(name) = hit.en.clone() {
                rec.name_en = name;
            }
        }
        for alias in &hit.aliases {
            push_alias(&mut rec.aliases, alias);
        }
        if rec.category == "general" {
            if let Some(cat) = hit.category.clone() {
                rec.category = cat;
            }
        }
        if !rec.description.contains("Wikidata") {
            rec.description
                .push_str(" Names from Wikidata (CC0) when present.");
        }
        n += 1;
    }
    n
}

pub fn records_to_seed(records: &[Record], scope: &str, month: &str) -> SeedFile {
    SeedFile {
        source: format!("crux:{scope}:{month}"),
        license: CRUX_LICENSE.to_string(),
        month: month.to_string(),
        scope: scope.to_string(),
        records: records.iter().map(SeedRecord::from_record).collect(),
    }
}

pub fn seed_from_bytes(bytes: &[u8]) -> Result<SeedFile, AppError> {
    let mut seed = parse_seed_bytes(bytes)?;
    polish_seed(&mut seed);
    Ok(seed)
}

fn parse_seed_bytes(bytes: &[u8]) -> Result<SeedFile, AppError> {
    let mut dec = GzDecoder::new(bytes);
    let mut s = String::new();
    dec.read_to_string(&mut s).map_err(|e| {
        AppError::new(
            "bad_kb",
            format!("popular seed gzip: {e}"),
            "Rebuild src/kb/popular_tw.json.gz.",
        )
    })?;
    serde_json::from_str(&s).map_err(|e| {
        AppError::new(
            "bad_kb",
            format!("popular seed json: {e}"),
            "Rebuild the popular seed.",
        )
    })
}

/// Decode entities, convert Simplified titles, replace Japanese-only or empty
/// names with a brand taken from the domain, and drop adult rows.
fn polish_seed(seed: &mut SeedFile) {
    let mut recs: Vec<Record> = seed
        .records
        .iter()
        .cloned()
        .map(SeedRecord::into_record)
        .collect();
    polish_records(&mut recs);
    seed.records = recs.iter().map(SeedRecord::from_record).collect();
}

pub fn load_bundled() -> Result<Vec<Record>, AppError> {
    let mut out = Vec::new();
    for bytes in [SEED_TW, SEED_GLOBAL] {
        if bytes.is_empty() {
            continue;
        }
        let seed = seed_from_bytes(bytes)?;
        out.extend(seed.records.into_iter().map(|r| r.into_record()));
    }
    Ok(out)
}

/// Insert popular rows. A curated (or earlier) row with the same registrable
/// domain keeps its URL and name; aliases and the CrUX bucket are attached.
pub fn merge_records(conn: &Connection, incoming: &[Record]) -> Result<(usize, usize), AppError> {
    let incoming = dedupe_same_id(incoming);
    let existing = db::list_merge_rows(conn)?;
    let mut by_domain: HashMap<String, usize> = HashMap::new();
    for (i, row) in existing.iter().enumerate() {
        if let Some(domain) = host_of(&row.official_url).and_then(|h| registrable(&h)) {
            by_domain.entry(domain).or_insert(i);
        }
    }
    let mut inserted = 0;
    let mut merged = 0;
    for rec in incoming {
        let Some(domain) = host_of(&rec.official_url).and_then(|h| registrable(&h)) else {
            continue;
        };
        if let Some(&idx) = by_domain.get(&domain) {
            let row = &existing[idx];
            let mut aliases = row.aliases.clone();
            for a in rec
                .aliases
                .iter()
                .chain(std::iter::once(&rec.name_zh))
                .chain(std::iter::once(&rec.name_en))
            {
                push_alias(&mut aliases, a);
            }
            let mut alts = row.alt_urls.clone();
            for u in &rec.alt_urls {
                if alts.len() < 8 && !alts.contains(u) {
                    alts.push(u.clone());
                }
            }
            let scope = if row.popularity_scope.is_empty() {
                rec.popularity_scope.as_str()
            } else {
                row.popularity_scope.as_str()
            };
            db::attach_popularity(conn, &row.id, &aliases, &alts, rec.popularity_bucket, scope)?;
            merged += 1;
        } else {
            db::upsert(conn, &rec)?;
            inserted += 1;
        }
    }
    Ok((inserted, merged))
}

fn dedupe_same_id(incoming: &[Record]) -> Vec<Record> {
    let mut map: HashMap<String, Record> = HashMap::new();
    for rec in incoming {
        if let Some(prev) = map.get_mut(&rec.id) {
            let tw = prev.popularity_scope == "tw" || rec.popularity_scope == "tw";
            let bucket = match (prev.popularity_bucket, rec.popularity_bucket) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            if prev.name_zh.is_empty() {
                prev.name_zh = rec.name_zh.clone();
            }
            if prev.name_en.is_empty() || prev.name_en == rec.id.trim_start_matches("pop:") {
                prev.name_en = rec.name_en.clone();
            }
            for a in &rec.aliases {
                push_alias(&mut prev.aliases, a);
            }
            for u in &rec.alt_urls {
                if prev.alt_urls.len() < 8 && !prev.alt_urls.contains(u) {
                    prev.alt_urls.push(u.clone());
                }
            }
            prev.popularity_bucket = bucket;
            if tw {
                prev.popularity_scope = "tw".to_string();
            }
        } else {
            map.insert(rec.id.clone(), rec.clone());
        }
    }
    let mut out: Vec<Record> = map.into_values().collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn apply_overlay(records: &mut [Record]) {
    let file: AliasFile = toml::from_str(include_str!("popular_aliases.toml")).unwrap_or_default();
    let map: HashMap<String, &AliasSite> =
        file.site.iter().map(|s| (s.domain.clone(), s)).collect();
    for rec in records {
        let Some(domain) = rec.id.strip_prefix("pop:") else {
            continue;
        };
        let Some(site) = map.get(domain) else {
            continue;
        };
        if !site.name_zh.is_empty() {
            rec.name_zh = site.name_zh.clone();
        }
        if !site.name_en.is_empty() {
            rec.name_en = site.name_en.clone();
        }
        for a in &site.aliases {
            push_alias(&mut rec.aliases, a);
        }
        if known_category(&site.category) {
            rec.category = site.category.clone();
        }
    }
}

fn apply_title(rec: &mut Record, title: &str) {
    let lower = title.to_ascii_lowercase();
    if lower.contains("just a moment")
        || lower.contains("attention required")
        || lower.contains("access denied")
        || lower.contains("human verification")
        || lower.trim() == "403"
    {
        return;
    }
    let name = title_name(title);
    if name.chars().count() < 2 {
        return;
    }
    let domain = rec.id.strip_prefix("pop:").unwrap_or("");
    let cjk = name.chars().any(|c| ('\u{3400}'..='\u{9fff}').contains(&c));
    if cjk && rec.name_zh.is_empty() {
        rec.name_zh = name;
    } else if !cjk && (rec.name_en.is_empty() || rec.name_en == domain) {
        rec.name_en = name;
    }
    if rec.category == "general" {
        rec.category = guess_category(domain, &rec.name_zh, &rec.name_en);
    }
}

fn title_name(title: &str) -> String {
    let t = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut = t
        .split(['|', '-', '–', '—', '｜'])
        .next()
        .unwrap_or(&t)
        .trim();
    let cut = if cut.chars().count() >= 2 && cut.chars().count() <= 40 {
        cut
    } else {
        t.trim()
    };
    cut.chars().take(40).collect()
}

fn probe(client: &reqwest::blocking::Client, url: &str) -> (&'static str, String) {
    let resp = match client.get(url).send() {
        Ok(r) => r,
        Err(_) => return ("unverified", String::new()),
    };
    let code = resp.status().as_u16();
    let body = resp.text().unwrap_or_default();
    let title = page_title(&body);
    let challenge = {
        let l = title.to_ascii_lowercase();
        l.contains("just a moment")
            || l.contains("attention required")
            || l.contains("access denied")
            || l == "403"
    };
    let status = if matches!(code, 401 | 403 | 429) || challenge {
        "blocked"
    } else if (200..300).contains(&code) {
        if title.trim().is_empty() {
            "connected"
        } else {
            "verified"
        }
    } else {
        "unverified"
    };
    (status, title)
}

fn page_title(body: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());
    let Some(c) = re.captures(body) else {
        return String::new();
    };
    let raw = c.get(1).map(|m| m.as_str()).unwrap_or("");
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

struct WikiHit {
    zh: Option<String>,
    en: Option<String>,
    aliases: Vec<String>,
    category: Option<String>,
}

fn wikidata_batch(domains: &[String]) -> Option<HashMap<String, WikiHit>> {
    let mut iris = Vec::new();
    for d in domains {
        for url in [
            format!("https://{d}"),
            format!("https://{d}/"),
            format!("https://www.{d}"),
            format!("https://www.{d}/"),
            format!("http://{d}"),
            format!("http://www.{d}"),
        ] {
            iris.push(format!("<{url}>"));
        }
    }
    let values = iris.join(" ");
    let query = format!(
        r#"SELECT ?official ?zhTw ?zhHant ?zh ?en ?alt ?type WHERE {{
  VALUES ?official {{ {values} }}
  ?item wdt:P856 ?official .
  OPTIONAL {{ ?item rdfs:label ?zhTw FILTER(LANG(?zhTw)="zh-tw") }}
  OPTIONAL {{ ?item rdfs:label ?zhHant FILTER(LANG(?zhHant)="zh-hant") }}
  OPTIONAL {{ ?item rdfs:label ?zh FILTER(LANG(?zh)="zh") }}
  OPTIONAL {{ ?item rdfs:label ?en FILTER(LANG(?en)="en") }}
  OPTIONAL {{ ?item skos:altLabel ?alt FILTER(LANG(?alt)="zh-tw" || LANG(?alt)="zh") }}
  OPTIONAL {{ ?item wdt:P31 ?type }}
}}"#
    );
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!(
            "agentbox/",
            env!("CARGO_PKG_VERSION"),
            " (https://github.com/alex-ht/agentbox)"
        ))
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .ok()?;
    let resp = client
        .post("https://query.wikidata.org/sparql")
        .header("Accept", "application/sparql-results+json")
        .form(&[("query", query.as_str())])
        .send()
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let text = resp.text().ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let bindings = value["results"]["bindings"].as_array()?;
    let mut out: HashMap<String, WikiHit> = HashMap::new();
    for row in bindings {
        let official = binding_str(row, "official").unwrap_or_default();
        let Some(host) = host_of(&official) else {
            continue;
        };
        let Some(domain) = registrable(&host) else {
            continue;
        };
        if !domains.iter().any(|d| d == &domain) {
            continue;
        }
        let hit = out.entry(domain).or_insert(WikiHit {
            zh: None,
            en: None,
            aliases: Vec::new(),
            category: None,
        });
        if hit.zh.is_none() {
            hit.zh = binding_str(row, "zhTw")
                .or_else(|| binding_str(row, "zhHant"))
                .or_else(|| binding_str(row, "zh"));
        }
        if hit.en.is_none() {
            hit.en = binding_str(row, "en");
        }
        if let Some(alias) = binding_str(row, "alt") {
            if alias.chars().count() <= 40 {
                push_alias(&mut hit.aliases, &alias);
            }
        }
        if hit.category.is_none() {
            hit.category = binding_str(row, "type")
                .as_deref()
                .and_then(category_from_qid);
        }
    }
    Some(out)
}

fn binding_str(row: &serde_json::Value, key: &str) -> Option<String> {
    row.get(key)?
        .get("value")?
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn category_from_qid(qid_url: &str) -> Option<String> {
    let qid = qid_url.rsplit('/').next().unwrap_or("");
    let cat = match qid {
        "Q11032" | "Q1193236" | "Q1153191" | "Q1110794" => "media",
        "Q3220391" | "Q1668024" | "Q8700794" => "social",
        "Q7397" | "Q35127" => "general",
        _ => return None,
    };
    Some(cat.to_string())
}

fn guess_category(domain: &str, name_zh: &str, name_en: &str) -> String {
    let host = domain.to_ascii_lowercase();
    let blob = format!(
        "{host} {} {}",
        name_zh.to_ascii_lowercase(),
        name_en.to_ascii_lowercase()
    );
    const SEARCH: &[&str] = &[
        "google.",
        "bing.com",
        "duckduckgo.com",
        "baidu.com",
        "yahoo.com",
    ];
    const SOCIAL: &[&str] = &[
        "facebook.com",
        "instagram.com",
        "twitter.com",
        "x.com",
        "tiktok.com",
        "line.me",
        "dcard.tw",
        "ptt.cc",
        "reddit.com",
        "linkedin.com",
        "threads.net",
        "weibo.com",
    ];
    const TECH: &[&str] = &[
        "github.com",
        "gitlab.com",
        "stackoverflow.com",
        "npmjs.com",
        "pypi.org",
    ];
    const MEDIA: &[&str] = &["wikipedia.org", "wikimedia.org"];
    if SEARCH
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    {
        return "search".into();
    }
    if SOCIAL
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    {
        return "social".into();
    }
    if TECH
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    {
        return "tech".into();
    }
    if MEDIA
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
    {
        return "media".into();
    }
    if blob.contains("新聞") || blob.contains("news") || name_zh.contains('報') {
        return "media".into();
    }
    "general".into()
}

fn push_alias(aliases: &mut Vec<String>, alias: &str) {
    let alias = alias.trim();
    if alias.is_empty() || aliases.iter().any(|a| a == alias) || aliases.len() >= 12 {
        return;
    }
    aliases.push(alias.to_string());
}

fn split_origin(origin: &str) -> Option<(bool, String)> {
    let origin = origin.trim();
    let (scheme, rest) = origin.split_once("://")?;
    let https = scheme.eq_ignore_ascii_case("https");
    if !https && !scheme.eq_ignore_ascii_case("http") {
        return None;
    }
    let host = rest.split(['/', '?', '#']).next()?.split('@').next_back()?;
    if host.starts_with('[') {
        return None;
    }
    let host = host.split(':').next()?;
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() || host.contains(' ') {
        return None;
    }
    Some((https, host))
}

fn is_ip_host(host: &str) -> bool {
    if host.starts_with('[') {
        return true;
    }
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| p.parse::<u8>().is_ok() && !p.is_empty())
}

pub fn registrable(host: &str) -> Option<String> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    psl::domain_str(&host).map(|s| s.to_string())
}

fn shared_suffix(host: &str) -> bool {
    psl::suffix_str(host).is_some_and(|s| SHARED_SUFFIXES.iter().any(|x| s.eq_ignore_ascii_case(x)))
}

fn adult_text(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    const PARTS: &[&str] = &[
        "a片",
        "色情",
        "無碼",
        "无码",
        "情色",
        "黃片",
        "黄片",
        "成人影片",
        "成人视频",
        "成人視頻",
        "成人卡通",
        "成人短剧",
        "成人短劇",
        "成人漫畫",
        "成人漫画",
        "免费a片",
        "免費a片",
        "吃瓜",
        "人体摄影",
        "人體攝影",
        "麻豆",
        "porn",
        "hentai",
        "xxx",
        "xvideo",
        "18+",
        "18＋",
        "アダルト",
    ];
    if PARTS.iter().any(|p| l.contains(p)) || l.contains('🔞') {
        return true;
    }
    has_adult_av_token(&l) || has_jav_token(&l)
}

fn has_jav_token(l: &str) -> bool {
    // "java" / "javascript" are not adult. "javbus" and "javhd" are.
    let bytes = l.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'j' && bytes[i + 1] == b'a' && bytes[i + 2] == b'v' {
            let prev_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
            let next = i + 3;
            let next_is_a = next < bytes.len() && bytes[next] == b'a';
            if prev_ok && !next_is_a {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// `av` as its own token, or `av` followed by digits anywhere (`av123`, `goodav17`).
fn has_adult_av_token(l: &str) -> bool {
    let bytes = l.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'a' && bytes[i + 1] == b'v' {
            let next = i + 2;
            let next_byte = bytes.get(next).copied();
            let digits_after = matches!(next_byte, Some(b) if b.is_ascii_digit());
            let boundary_before = i == 0 || !bytes[i - 1].is_ascii_alphabetic();
            let boundary_after = match next_byte {
                None => true,
                Some(b) if !b.is_ascii_alphabetic() => true,
                _ => false,
            };
            if (digits_after || (boundary_before && boundary_after)) && !safe_av_word(l, i) {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn safe_av_word(l: &str, av_at: usize) -> bool {
    let bytes = l.as_bytes();
    let mut start = av_at;
    while start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
        start -= 1;
    }
    let mut end = av_at;
    while end < bytes.len() && bytes[end].is_ascii_alphanumeric() {
        end += 1;
    }
    let word = &l[start..end];
    matches!(
        word,
        "avatar"
            | "aviation"
            | "avenue"
            | "avaya"
            | "available"
            | "java"
            | "javascript"
            | "javadoc"
    )
}

/// Drop rows whose homepage title is adult. Host filters cannot see this.
pub fn drop_adult_text(records: &mut Vec<Record>) -> usize {
    let before = records.len();
    records.retain(|r| !record_is_adult(r));
    before - records.len()
}

/// Clean popular-site names and drop adult rows. Returns how many were dropped.
/// Safe to run more than once.
pub fn polish_records(records: &mut Vec<Record>) -> usize {
    for rec in records.iter_mut() {
        decode_record_text(rec);
    }
    let mut dropped = drop_adult_text(records);
    for rec in records.iter_mut() {
        polish_names(rec);
    }
    dropped += drop_adult_text(records);
    dropped
}

fn record_is_adult(rec: &Record) -> bool {
    let domain = rec.id.trim_start_matches("pop:");
    adult_text(&rec.name_zh)
        || adult_text(&rec.name_en)
        || rec.aliases.iter().any(|a| adult_text(a))
        || adult_domain(domain)
}

fn decode_record_text(rec: &mut Record) {
    rec.name_zh = decode_entities(&rec.name_zh);
    rec.name_en = decode_entities(&rec.name_en);
    for alias in &mut rec.aliases {
        *alias = decode_entities(alias);
    }
}

fn polish_names(rec: &mut Record) {
    let domain = rec.id.trim_start_matches("pop:").to_string();
    if is_junk_title(&rec.name_zh) {
        rec.name_zh.clear();
    } else if is_japanese_name(&rec.name_zh) {
        rec.name_zh = salvage_latin(&rec.name_zh).unwrap_or_default();
    } else {
        rec.name_zh = super::s2t::to_traditional(&rec.name_zh);
        rec.name_zh = title_name(&rec.name_zh);
        if is_junk_title(&rec.name_zh) {
            rec.name_zh.clear();
        }
    }
    if is_junk_title(&rec.name_en) {
        rec.name_en.clear();
    } else if is_japanese_name(&rec.name_en) {
        rec.name_en = salvage_latin(&rec.name_en).unwrap_or_default();
    } else if !rec.name_en.is_empty() {
        rec.name_en = super::s2t::to_traditional(&rec.name_en);
        rec.name_en = title_name(&rec.name_en);
    }
    rec.aliases = rec.aliases.iter().filter_map(|a| clean_alias(a)).collect();
    let zh_empty = rec.name_zh.trim().is_empty();
    let en_empty = rec.name_en.trim().is_empty() || is_domainish(&rec.name_en, &domain);
    if zh_empty && en_empty {
        rec.name_en = brand_from_domain(&domain);
        if !rec.description.contains("display name fell back") {
            rec.description
                .push_str(" Display name fell back to the domain brand.");
        }
    }
    if rec.name_zh.trim().is_empty() {
        rec.name_zh.clear();
    }
}

fn clean_alias(alias: &str) -> Option<String> {
    if is_junk_title(alias) || adult_text(alias) {
        return None;
    }
    if is_japanese_name(alias) {
        return salvage_latin(alias).filter(|s| s.chars().count() >= 2);
    }
    let s = title_name(&super::s2t::to_traditional(alias));
    if s.chars().count() < 2 || is_junk_title(&s) || adult_text(&s) {
        None
    } else {
        Some(s)
    }
}

fn is_japanese_name(s: &str) -> bool {
    s.chars().any(is_kana)
}

fn is_kana(c: char) -> bool {
    ('\u{3040}'..='\u{30ff}').contains(&c) || ('\u{ff66}'..='\u{ff9d}').contains(&c)
}

/// Keep a Latin brand embedded in a Japanese title (`AppMedia（アップメディア）`).
fn salvage_latin(s: &str) -> Option<String> {
    let latin: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '&' {
                c
            } else {
                ' '
            }
        })
        .collect();
    let latin = latin.split_whitespace().collect::<Vec<_>>().join(" ");
    let letters = latin.chars().filter(|c| c.is_ascii_alphabetic()).count();
    (letters >= 3).then_some(latin)
}

fn is_junk_title(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    let t = l.trim();
    t.is_empty()
        || t == "403"
        || t == "404"
        || l.contains("just a moment")
        || l.contains("attention required")
        || l.contains("access denied")
        || l.contains("making sure you")
        || l.contains("not a bot")
        || l.contains("age verification")
        || l.contains("captcha")
        || l.contains("human verification")
}

fn is_domainish(name: &str, domain: &str) -> bool {
    let n = name.trim().trim_end_matches('/').to_ascii_lowercase();
    n.is_empty() || n == domain || n == format!("www.{domain}")
}

fn brand_from_domain(domain: &str) -> String {
    let label = domain.split('.').next().unwrap_or(domain);
    let mut out = String::new();
    let mut cap = true;
    for c in label.chars() {
        if c == '-' || c == '_' {
            if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
            cap = true;
            continue;
        }
        if cap {
            out.extend(c.to_uppercase());
            cap = false;
        } else {
            out.extend(c.to_lowercase());
        }
    }
    let out = out.trim().to_string();
    if out.is_empty() {
        domain.to_string()
    } else {
        out
    }
}

fn decode_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('&') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find(';') {
            if let Some(ch) = decode_one(&after[..end]) {
                out.push(ch);
                rest = &after[end + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &rest[start + 1..];
    }
    out.push_str(rest);
    let out = strip_broken_entities(&out);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Titles are truncated to 40 characters, which can cut an entity in half
/// (`&#260` with no `;`). Drop those fragments after real entities are decoded.
fn strip_broken_entities(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '&' {
            let mut j = i + 1;
            if chars.get(j) == Some(&'#') {
                j += 1;
                if chars.get(j) == Some(&'x') || chars.get(j) == Some(&'X') {
                    j += 1;
                }
                let digits_from = j;
                while j < chars.len() && chars[j].is_ascii_hexdigit() {
                    j += 1;
                }
                let closed = chars.get(j) == Some(&';');
                if !closed && j > digits_from {
                    i = j;
                    continue;
                }
            } else if chars.get(j).is_some_and(|c| c.is_ascii_alphabetic()) {
                while j < chars.len() && chars[j].is_ascii_alphabetic() {
                    j += 1;
                }
                if chars.get(j) != Some(&';') && j > i + 1 {
                    i = j;
                    continue;
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn decode_one(entity: &str) -> Option<char> {
    if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        return char::from_u32(u32::from_str_radix(hex, 16).ok()?);
    }
    if let Some(dec) = entity.strip_prefix('#') {
        return char::from_u32(dec.parse().ok()?);
    }
    Some(match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{00a0}',
        "mdash" => '—',
        "ndash" => '–',
        _ => return None,
    })
}

fn adult_domain(domain: &str) -> bool {
    is_adult_host(domain) || adult_tld(domain) || domain.split(['.', '-']).any(adult_av_label)
}

fn adult_tld(domain: &str) -> bool {
    const TLDS: &[&str] = &["xxx", "adult", "porn", "sex"];
    domain.rsplit('.').next().is_some_and(|t| TLDS.contains(&t))
}

fn adult_av_label(label: &str) -> bool {
    let l = label.to_ascii_lowercase();
    if matches!(
        l.as_str(),
        "avatar"
            | "aviation"
            | "avenue"
            | "avaya"
            | "available"
            | "java"
            | "javascript"
            | "javadoc"
    ) {
        return false;
    }
    if l.contains("chigua") || l.contains("hcomic") || l.contains("madou") {
        return true;
    }
    if has_jav_token(&l) || has_adult_av_token(&l) {
        return true;
    }
    let digits = l.chars().any(|c| c.is_ascii_digit());
    (l.starts_with("av") && l.len() <= 8) || (l.ends_with("av") && digits)
}

fn is_adult_host(host: &str) -> bool {
    host.split(['.', '-'])
        .filter(|l| !l.is_empty())
        .any(|label| {
            let l = label.to_ascii_lowercase();
            l == "sex" || l == "xxx" || l == "porn" || ADULT_PARTS.iter().any(|p| l.contains(p))
        })
}

fn denylist() -> &'static HashSet<String> {
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| {
        include_str!("popular_denylist.txt")
            .lines()
            .map(|l| {
                l.split('#')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
            })
            .filter(|l| !l.is_empty())
            .collect()
    })
}

fn is_infra_domain(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    denylist()
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

fn is_apex_or_www(host: &str, domain: &str) -> bool {
    host == domain || host == format!("www.{domain}")
}

fn is_noise_host(host: &str, domain: &str) -> bool {
    if is_apex_or_www(host, domain) {
        return false;
    }
    let prefix = host.strip_suffix(&format!(".{domain}")).unwrap_or(host);
    prefix.split('.').any(|label| NOISE_LABELS.contains(&label))
}

fn homepage_rank(origin: &Origin, domain: &str) -> u8 {
    let www = u8::from(origin.host != format!("www.{domain}"));
    let https = u8::from(!origin.https);
    www * 2 + https
}

fn host_of(url: &str) -> Option<String> {
    split_origin(url).map(|(_, h)| h).or_else(|| {
        let url = url.trim();
        if url.contains("://") {
            None
        } else {
            Some(url.trim_end_matches('/').to_ascii_lowercase())
        }
    })
}

impl SeedRecord {
    fn from_record(rec: &Record) -> Self {
        Self {
            id: rec.id.clone(),
            name_zh: rec.name_zh.clone(),
            name_en: rec.name_en.clone(),
            aliases: rec.aliases.clone(),
            category: rec.category.clone(),
            official_url: rec.official_url.clone(),
            alt_urls: rec.alt_urls.clone(),
            description: rec.description.clone(),
            source: rec.source.clone(),
            source_license: rec.source_license.clone(),
            last_verified: rec.last_verified.clone(),
            verify_status: rec.verify_status.clone(),
            popularity_bucket: rec.popularity_bucket.unwrap_or(0),
            popularity_scope: rec.popularity_scope.clone(),
        }
    }

    fn into_record(self) -> Record {
        let mut rec = Record::blank(self.id, "org");
        rec.name_zh = self.name_zh;
        rec.name_en = self.name_en;
        rec.aliases = self.aliases;
        rec.category = self.category;
        rec.official_url = self.official_url;
        rec.alt_urls = self.alt_urls;
        rec.description = self.description;
        rec.source = self.source;
        rec.source_license = self.source_license;
        rec.last_verified = self.last_verified;
        rec.verify_status = self.verify_status;
        rec.popularity_bucket = Some(self.popularity_bucket);
        rec.popularity_scope = self.popularity_scope;
        rec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = "\
origin,rank
https://www.youtube.com,1000
https://m.youtube.com,1000
https://accounts.google.com,1000
https://www.google.com,1000
https://18porn.cc,1000
https://123av.com,1000
https://52av.one,1000
https://xvideos.com,5000
https://www.gstatic.com,1000
https://www.example.com,5000
https://cdn.example.com,1000
https://irs.thsrc.com.tw,1000
https://www.thsrc.com.tw,1000
https://203.0.113.8,1000
https://login.onlybank.test,1000
https://moodle.school.edu.tw,5000
https://www.school.edu.tw,5000
https://foo.github.io,1000
https://not-a-url,1000
https://www.late.example,10000
https://www.goodav17.com,1000
https://javbus.com,1000
https://cdn.example.xxx,1000
https://www.avatar.com,5000
";

    #[test]
    fn folds_and_drops_adult_infra_and_backends() {
        let (recs, stats) = prepare_csv(FIXTURE, "tw", 5000, "202610");
        let ids: Vec<&str> = recs.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"pop:youtube.com"), "{ids:?}");
        assert!(ids.contains(&"pop:google.com"), "{ids:?}");
        assert!(ids.contains(&"pop:example.com"), "{ids:?}");
        assert!(ids.contains(&"pop:thsrc.com.tw"), "{ids:?}");
        assert!(ids.contains(&"pop:school.edu.tw"), "{ids:?}");
        assert!(!ids.iter().any(|id| {
            id.contains("porn")
                || id.contains("xvideos")
                || id.contains("gstatic")
                || id.contains("123av")
                || id.contains("52av")
                || id.contains("goodav17")
                || id.contains("javbus")
                || id.contains(".xxx")
        }));
        assert!(
            ids.contains(&"pop:avatar.com"),
            "avatar is not adult: {ids:?}"
        );
        assert!(!ids
            .iter()
            .any(|id| id.contains("onlybank") || id.contains("github.io")));
        assert!(stats.dropped_adult >= 2, "{stats:?}");
        assert!(stats.dropped_ip >= 1, "{stats:?}");
        assert!(stats.dropped_rank >= 1, "{stats:?}");
        let yt = recs.iter().find(|r| r.id == "pop:youtube.com").unwrap();
        assert_eq!(yt.name_en, "YouTube");
        assert!(yt.alt_urls.iter().any(|u| u.contains("m.youtube.com")));
        assert_eq!(yt.popularity_bucket, Some(1000));
        let thsrc = recs.iter().find(|r| r.id == "pop:thsrc.com.tw").unwrap();
        assert!(
            thsrc.official_url.contains("www.thsrc.com.tw"),
            "{}",
            thsrc.official_url
        );
        assert!(thsrc
            .alt_urls
            .iter()
            .any(|u| u.contains("irs.thsrc.com.tw")));
        let school = recs.iter().find(|r| r.id == "pop:school.edu.tw").unwrap();
        assert!(school.official_url.contains("www.school.edu.tw"));
        assert!(school.alt_urls.iter().all(|u| !u.contains("moodle")));
    }

    #[test]
    fn polish_decodes_entities_converts_simplified_and_replaces_japanese() {
        let mut poke = Record::blank("pop:52poke.com", "org");
        poke.name_zh = "神奇宝贝部落格 &#8211; 有多少宝可梦就有多少梦想".into();
        poke.name_en = "52poke.com".into();
        let mut jp = Record::blank("pop:animanch.com", "org");
        jp.name_en = "あにまんch".into();
        let mut mixed = Record::blank("pop:appmedia.jp", "org");
        mixed.name_en = "AppMedia（アップメディア）".into();
        let mut junk = Record::blank("pop:amnesty.tw", "org");
        junk.name_en = "Making sure you&#39;re not a bot!".into();
        let mut adult = Record::blank("pop:rou.video", "org");
        adult.name_en = "18+ 🔞".into();
        let mut entity = Record::blank("pop:anns.tw", "org");
        entity.name_zh = "Ann&#x27;S 寬腳友善第一品牌".into();
        let mut recs = vec![poke, jp, mixed, junk, adult, entity];
        let dropped = polish_records(&mut recs);
        assert_eq!(dropped, 1, "18+ row should be dropped");
        assert!(recs.iter().all(|r| r.id != "pop:rou.video"));
        let poke = recs.iter().find(|r| r.id == "pop:52poke.com").unwrap();
        assert!(!poke.name_zh.contains('&'), "{}", poke.name_zh);
        assert!(poke.name_zh.contains('寶'), "{}", poke.name_zh);
        assert!(poke.name_zh.contains("神奇"), "{}", poke.name_zh);
        assert!(!poke.name_zh.contains('贝'), "{}", poke.name_zh);
        let jp = recs.iter().find(|r| r.id == "pop:animanch.com").unwrap();
        assert_eq!(jp.name_en, "Animanch");
        assert!(jp.description.contains("fell back"));
        let mixed = recs.iter().find(|r| r.id == "pop:appmedia.jp").unwrap();
        assert_eq!(mixed.name_en, "AppMedia");
        assert!(!mixed
            .name_en
            .chars()
            .any(|c| ('\u{3040}'..='\u{30ff}').contains(&c)));
        let junk = recs.iter().find(|r| r.id == "pop:amnesty.tw").unwrap();
        assert_eq!(junk.name_en, "Amnesty");
        let anns = recs.iter().find(|r| r.id == "pop:anns.tw").unwrap();
        assert!(anns.name_zh.contains("Ann'S"), "{}", anns.name_zh);
        assert!(!anns.name_zh.contains("&#"), "{}", anns.name_zh);
    }

    #[test]
    fn registrable_domains_keep_tw_suffix() {
        assert_eq!(
            registrable("www.books.com.tw").as_deref(),
            Some("books.com.tw")
        );
        assert_eq!(
            registrable("irs.thsrc.com.tw").as_deref(),
            Some("thsrc.com.tw")
        );
        assert_eq!(
            registrable("www.youtube.com").as_deref(),
            Some("youtube.com")
        );
    }
}
