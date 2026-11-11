//! Normalized ranking for knowledge-base search.
//!
//! Scores are in `0.0..=1.0` (higher is better). `1.0` is an exact normalized
//! name or alias. A multi-word query is OR-matched: each term is scored on its
//! own, and term coverage (how many content terms hit) is mixed into the
//! score so a full match ranks above a partial one. Generic words such as
//! 官網 or 官方網站 are low-weight stopwords and do not have to occur.
//! Trigram retrieval is separate; this score is what every candidate,
//! including the short-query and typo fallback, is sorted by.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// Minimum score returned as a hit.
pub const HIT_MIN: f64 = 0.47;
/// Minimum score mentioned as a near miss in a not_found hint.
pub const CLOSE_MIN: f64 = 0.32;

/// Fold width, ASCII case and a few Traditional-Chinese variants, then drop
/// punctuation and whitespace so "身分 證" and "身份證" compare fairly.
pub fn normalize(s: &str) -> String {
    s.chars()
        .map(fold_width)
        .flat_map(|c| c.to_lowercase())
        .map(fold_variant)
        .filter(|c| c.is_alphanumeric())
        .collect()
}

fn fold_width(c: char) -> char {
    match c {
        '\u{3000}' => ' ',
        c if ('\u{FF01}'..='\u{FF5E}').contains(&c) => {
            char::from_u32(c as u32 - 0xFEE0).unwrap_or(c)
        }
        c => c,
    }
}

fn fold_variant(c: char) -> char {
    match c {
        '份' => '分',
        '証' => '證',
        '台' => '臺',
        '裡' | '裏' => '裏',
        c => c,
    }
}

/// Best score of `query` (already normalized) against one raw field.
/// `allow_edit` is for short fields (names, aliases). Long descriptions skip
/// the sliding-window edit distance.
pub fn score_text(query_norm: &str, text: &str, allow_edit: bool) -> f64 {
    if query_norm.is_empty() || text.is_empty() {
        return 0.0;
    }
    let text_norm = normalize(text);
    if text_norm.is_empty() {
        return 0.0;
    }
    if text_norm == query_norm {
        return 1.0;
    }
    let q: Vec<char> = query_norm.chars().collect();
    let t: Vec<char> = text_norm.chars().collect();
    if let Some(pos) = text_norm.find(query_norm) {
        let cov = q.len() as f64 / t.len().max(1) as f64;
        let start = if pos == 0 { 0.03 } else { 0.0 };
        return (0.84 + 0.12 * cov + start).min(0.99);
    }

    let mut best = 0.0_f64;
    let budget = edit_budget(q.len());
    if allow_edit && budget > 0 && t.len() <= 80 && q.len() <= 24 {
        let lo = q.len().saturating_sub(budget).max(1);
        let hi = (q.len() + budget).min(t.len());
        if t.len() >= lo {
            for w in lo..=hi {
                let last = t.len() - w;
                for i in 0..=last {
                    let Some(dist) = levenshtein(&q, &t[i..i + w], budget) else {
                        continue;
                    };
                    if dist == 0 {
                        continue;
                    }
                    let s = 0.78 * (1.0 - dist as f64 / q.len() as f64);
                    best = best.max(s);
                }
            }
        }
    }

    if q.len() >= 4 {
        let qb = bigrams(&q);
        let tb: HashSet<String> = bigrams(&t).into_iter().collect();
        if !qb.is_empty() {
            let hit = qb.iter().filter(|b| tb.contains(*b)).count();
            let ratio = hit as f64 / qb.len() as f64;
            if ratio >= 0.67 {
                best = best.max(0.40 + 0.28 * ratio);
            }
        }
    }

    if q.len() >= 2 {
        if let Some(win) = min_window(&t, &q) {
            let ratio = q.len() as f64 / win as f64;
            let s = if win <= q.len() * 3 + 1 {
                0.36 + 0.42 * ratio
            } else if win <= q.len() * 5 + 2 {
                0.30 * ratio
            } else {
                0.0
            };
            // Coverage is a fallback for split words ("報稅" inside "所得稅…申報").
            // It must not outrank a real substring.
            best = best.max(s.min(0.74));
        }
    }
    best
}

#[derive(Clone, Copy)]
pub struct Fields<'a> {
    pub name_zh: &'a str,
    pub name_en: &'a str,
    pub aliases: &'a [String],
    pub category: &'a str,
    pub description: &'a str,
    pub official_url: &'a str,
}

/// Weighted max across the fields a person would actually type.
pub fn score_record(query_norm: &str, f: Fields<'_>) -> f64 {
    let mut best = score_text(query_norm, f.name_zh, true);
    best = best.max(score_text(query_norm, f.name_en, true) * 0.96);
    for alias in f.aliases {
        best = best.max(score_text(query_norm, alias, true));
    }
    best = best.max(score_text(query_norm, f.category, false) * 0.6);
    best = best.max(score_text(query_norm, f.description, false) * 0.7);
    let url_ok = query_norm.contains('.') || query_norm.len() >= 12;
    if url_ok {
        best = best.max(score_text(query_norm, f.official_url, false) * 0.45);
    }
    best.min(1.0)
}

fn edit_budget(q_len: usize) -> usize {
    if q_len < 3 {
        0
    } else if q_len <= 6 {
        1
    } else {
        2
    }
}

fn bigrams(chars: &[char]) -> Vec<String> {
    if chars.len() < 2 {
        return Vec::new();
    }
    chars.windows(2).map(|w| w.iter().collect()).collect()
}

fn min_window(text: &[char], query: &[char]) -> Option<usize> {
    if query.is_empty() || text.is_empty() {
        return None;
    }
    let mut need: HashMap<char, usize> = HashMap::new();
    for c in query {
        *need.entry(*c).or_insert(0) += 1;
    }
    let required = need.len();
    let mut have: HashMap<char, usize> = HashMap::new();
    let mut formed = 0;
    let mut best: Option<usize> = None;
    let mut left = 0;
    for right in 0..text.len() {
        let c = text[right];
        if let Some(n) = need.get(&c).copied() {
            let e = have.entry(c).or_insert(0);
            *e += 1;
            if *e == n {
                formed += 1;
            }
        }
        while formed == required && left <= right {
            best = Some(best.map_or(right - left + 1, |b| b.min(right - left + 1)));
            let lc = text[left];
            if let Some(n) = need.get(&lc).copied() {
                let e = have.get_mut(&lc).expect("char was counted");
                *e -= 1;
                if *e < n {
                    formed -= 1;
                }
            }
            left += 1;
        }
    }
    best
}

fn levenshtein(a: &[char], b: &[char], max_dist: usize) -> Option<usize> {
    if a.len().abs_diff(b.len()) > max_dist {
        return None;
    }
    let m = b.len();
    let mut prev: Vec<usize> = (0..=m).collect();
    let mut curr = vec![0; m + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        let mut row_min = curr[0];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
            row_min = row_min.min(curr[j + 1]);
        }
        if row_min > max_dist {
            return None;
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    (prev[m] <= max_dist).then_some(prev[m])
}

/// Mild CrUX boost used only as a tie-break after the text score.
/// A bucket of 1000 outranks 5000, which outranks everything else.
/// It never changes whether a row clears [`HIT_MIN`].
pub fn popularity_key(bucket: Option<i64>) -> u8 {
    match bucket {
        Some(b) if (1..=1000).contains(&b) => 3,
        Some(b) if b <= 5_000 => 2,
        Some(b) if b <= 10_000 => 1,
        _ => 0,
    }
}

/// Round to 3 decimal places for a stable JSON number.
pub fn round_score(score: f64) -> f64 {
    (score.clamp(0.0, 1.0) * 1000.0).round() / 1000.0
}

/// One query token. `stop` terms (官網, 網站, 怎麼, …) are low-weight: they
/// never have to match, and they do not count against coverage.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryTerm {
    pub raw: String,
    pub norm: String,
    pub stop: bool,
}

#[derive(Debug, Clone)]
pub struct PreparedQuery {
    pub terms: Vec<QueryTerm>,
}

impl PreparedQuery {
    /// Norms sent to FTS. Stopwords are omitted when a content term exists,
    /// so a missing 官方網站 does not hide rows that match 高鐵.
    pub fn recall_norms(&self) -> Vec<String> {
        let content: Vec<String> = self
            .terms
            .iter()
            .filter(|t| !t.stop)
            .map(|t| t.norm.clone())
            .collect();
        if content.is_empty() {
            self.terms.iter().map(|t| t.norm.clone()).collect()
        } else {
            content
        }
    }

    pub fn content_terms(&self) -> Vec<&QueryTerm> {
        let content: Vec<&QueryTerm> = self.terms.iter().filter(|t| !t.stop).collect();
        if content.is_empty() {
            self.terms.iter().collect()
        } else {
            content
        }
    }
}

#[derive(Debug, Clone)]
pub struct QueryScore {
    pub score: f64,
    /// Query terms (original spelling) that cleared [`HIT_MIN`], in query order.
    pub matched_terms: Vec<String>,
    /// `exact`, `partial`, `fuzzy`, or `none` when nothing cleared the bar.
    pub kind: &'static str,
}

/// Split on whitespace and punctuation, then peel known generic words off
/// either end so `高鐵官方網站` is `高鐵` plus a stopword.
pub fn prepare_query(query: &str) -> PreparedQuery {
    let mut terms = Vec::new();
    for piece in split_pieces(query) {
        peel(&piece, &mut terms);
    }
    let mut seen = HashSet::new();
    terms.retain(|t| seen.insert(t.norm.clone()));
    PreparedQuery { terms }
}

pub fn score_query(query: &PreparedQuery, f: Fields<'_>) -> QueryScore {
    if query.terms.is_empty() {
        return QueryScore {
            score: 0.0,
            matched_terms: Vec::new(),
            kind: "none",
        };
    }
    let term_scores: Vec<f64> = query
        .terms
        .iter()
        .map(|t| {
            if t.norm.is_empty() {
                0.0
            } else {
                score_record(&t.norm, f)
            }
        })
        .collect();
    let content: Vec<usize> = query
        .terms
        .iter()
        .enumerate()
        .filter(|(_, t)| !t.stop)
        .map(|(i, _)| i)
        .collect();
    let use_idx: Vec<usize> = if content.is_empty() {
        (0..query.terms.len()).collect()
    } else {
        content.clone()
    };
    let matched: Vec<usize> = use_idx
        .iter()
        .copied()
        .filter(|&i| term_scores[i] >= HIT_MIN)
        .collect();
    let mut score = if use_idx.len() == 1 {
        term_scores[use_idx[0]]
    } else if matched.is_empty() {
        use_idx
            .iter()
            .map(|&i| term_scores[i])
            .fold(0.0_f64, f64::max)
    } else {
        let coverage = matched.len() as f64 / use_idx.len() as f64;
        let best = matched
            .iter()
            .map(|&i| term_scores[i])
            .fold(0.0_f64, f64::max);
        let blended = 0.62 * coverage + 0.38 * best;
        // One strong term must still surface when the other words miss.
        if best >= HIT_MIN {
            blended.max(HIT_MIN)
        } else {
            blended
        }
    };
    if !content.is_empty() && content.len() != query.terms.len() && !matched.is_empty() {
        let stops: Vec<usize> = query
            .terms
            .iter()
            .enumerate()
            .filter(|(_, t)| t.stop)
            .map(|(i, _)| i)
            .collect();
        if !stops.is_empty() {
            let hit = stops.iter().filter(|&&i| term_scores[i] >= HIT_MIN).count();
            score = (score + 0.02 * (hit as f64 / stops.len() as f64)).min(1.0);
        }
    }
    let matched_terms: Vec<String> = term_scores
        .iter()
        .enumerate()
        .filter(|(_, s)| **s >= HIT_MIN)
        .map(|(i, _)| query.terms[i].raw.clone())
        .collect();
    let kind = match_kind(query, f, &term_scores);
    QueryScore {
        score: score.min(1.0),
        matched_terms,
        kind,
    }
}

fn match_kind(query: &PreparedQuery, f: Fields<'_>, term_scores: &[f64]) -> &'static str {
    let content = query.content_terms();
    let mut saw_exact = false;
    let mut saw_partial = false;
    let mut saw_fuzzy = false;
    for t in content {
        let idx = query
            .terms
            .iter()
            .position(|q| q.norm == t.norm)
            .unwrap_or(0);
        if term_scores.get(idx).copied().unwrap_or(0.0) < HIT_MIN {
            continue;
        }
        if field_exact(&t.norm, f) {
            saw_exact = true;
        } else if field_partial(&t.norm, f) {
            saw_partial = true;
        } else {
            saw_fuzzy = true;
        }
    }
    if !saw_exact && !saw_partial && !saw_fuzzy {
        "none"
    } else if saw_exact && !saw_partial && !saw_fuzzy {
        "exact"
    } else if saw_exact || saw_partial {
        "partial"
    } else {
        "fuzzy"
    }
}

fn field_exact(norm: &str, f: Fields<'_>) -> bool {
    let exact = |s: &str| normalize(s) == norm;
    exact(f.name_zh) || exact(f.name_en) || f.aliases.iter().any(|a| exact(a))
}

fn field_partial(norm: &str, f: Fields<'_>) -> bool {
    let partial = |s: &str| {
        let n = normalize(s);
        !n.is_empty() && n.contains(norm)
    };
    partial(f.name_zh)
        || partial(f.name_en)
        || partial(f.description)
        || f.aliases.iter().any(|a| partial(a))
}

fn split_pieces(query: &str) -> Vec<String> {
    let mut cur = String::new();
    let mut parts = Vec::new();
    for c in query.chars() {
        if c.is_whitespace() || is_query_punct(c) {
            if !cur.is_empty() {
                parts.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

fn is_query_punct(c: char) -> bool {
    matches!(
        c,
        ',' | '.'
            | '，'
            | '。'
            | '、'
            | '；'
            | ';'
            | ':'
            | '：'
            | '/'
            | '／'
            | '|'
            | '｜'
            | '('
            | ')'
            | '（'
            | '）'
            | '['
            | ']'
            | '「'
            | '」'
            | '"'
            | '\''
            | '？'
            | '?'
            | '！'
            | '!'
            | '+'
            | '&'
            | '＆'
    )
}

/// Generic words people append to an otherwise specific name. Matched on the
/// normalized form, longest first, so 官方網站 is one stopword rather than 官方 + 網站.
const STOP_RAW: &[&str] = &[
    "官方網站",
    "官方網址",
    "官方首頁",
    "官方網",
    "官方",
    "官網",
    "網站",
    "網址",
    "首頁",
    "主頁",
    "怎麼辦",
    "怎麼",
    "如何",
    "哪裡",
    "哪裏",
    "何處",
    "在哪裡",
    "在哪裏",
    "在哪",
    "申請",
    "辦理",
    "查詢",
    "是什麼",
    "是什么",
    "什麼",
    "什么",
    "website",
    "homepage",
    "official",
    "url",
];

fn stopword_list() -> &'static [String] {
    static LIST: OnceLock<Vec<String>> = OnceLock::new();
    LIST.get_or_init(|| {
        let mut v: Vec<String> = STOP_RAW
            .iter()
            .map(|s| normalize(s))
            .filter(|s| !s.is_empty())
            .collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
        v.dedup();
        v
    })
}

fn is_stop(norm: &str) -> bool {
    stopword_list().iter().any(|s| s == norm)
}

fn peel(raw: &str, out: &mut Vec<QueryTerm>) {
    let norm = normalize(raw);
    if norm.is_empty() {
        return;
    }
    for (n, stop) in peel_norm(&norm) {
        let display = if n == norm {
            raw.to_string()
        } else {
            n.clone()
        };
        out.push(QueryTerm {
            raw: display,
            norm: n,
            stop,
        });
    }
}

fn peel_norm(norm: &str) -> Vec<(String, bool)> {
    if is_stop(norm) {
        return vec![(norm.to_string(), true)];
    }
    for sw in stopword_list() {
        if let Some(rest) = norm.strip_suffix(sw.as_str()) {
            if !rest.is_empty() {
                let mut v = peel_norm(rest);
                v.push((sw.clone(), true));
                return v;
            }
        }
        if let Some(rest) = norm.strip_prefix(sw.as_str()) {
            if !rest.is_empty() {
                let mut v = vec![(sw.clone(), true)];
                v.extend(peel_norm(rest));
                return v;
            }
        }
    }
    vec![(norm.to_string(), false)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields<'a>(name_zh: &'a str, aliases: &'a [String], description: &'a str) -> Fields<'a> {
        Fields {
            name_zh,
            name_en: "",
            aliases,
            category: "tax",
            description,
            official_url: "https://example.test/tax",
        }
    }

    #[test]
    fn folds_variants_and_width() {
        assert_eq!(normalize("身份證"), "身分證");
        assert_eq!(normalize("台鐵"), normalize("臺鐵"));
        assert_eq!(normalize("ＡＢＣ"), "abc");
        assert_eq!(normalize("身分 證"), "身分證");
    }

    #[test]
    fn partial_coverage_finds_split_tax_words() {
        let s = score_text("報稅", "綜合所得稅結算申報", false);
        assert!(s >= HIT_MIN, "報稅 score {s} against 綜合所得稅結算申報");
        let sub = score_text("所得稅", "綜合所得稅結算申報", false);
        assert!(sub > s, "contiguous 所得稅 should outrank split 報稅");
        assert!(sub >= 0.84);
    }

    #[test]
    fn typo_within_one_edit_scores() {
        let s = score_text("身分正", "國民身分證補發", true);
        assert!(s >= HIT_MIN, "typo score {s}");
        let miss = score_record("量子電腦食譜", fields("綜合所得稅結算申報", &[], ""));
        assert!(miss < HIT_MIN, "unrelated score {miss}");
    }

    #[test]
    fn exact_alias_is_one() {
        let aliases = vec!["勞保局".to_string()];
        let s = score_record("勞保局", fields("勞動部勞工保險局", &aliases, ""));
        assert_eq!(s, 1.0);
    }

    #[test]
    fn single_term_score_matches_score_record() {
        let f = fields("綜合所得稅結算申報", &[], "辦理綜合所得稅結算申報。");
        let q = prepare_query("所得稅");
        let s = score_query(&q, f);
        assert_eq!(s.score, score_record("所得稅", f));
        assert_eq!(s.kind, "partial");
        assert_eq!(s.matched_terms, vec!["所得稅".to_string()]);
    }

    #[test]
    fn stopwords_do_not_block_and_coverage_ranks_full_matches_first() {
        let aliases = vec!["高鐵".to_string()];
        let rail = fields("台灣高鐵", &aliases, "台灣高速鐵路。");
        let q = prepare_query("高鐵 官方網站");
        assert!(q.terms.iter().any(|t| t.stop && t.norm == "官方網站"));
        let hit = score_query(&q, rail);
        assert!(hit.score >= HIT_MIN, "score {}", hit.score);
        assert_eq!(hit.matched_terms, vec!["高鐵".to_string()]);
        assert_eq!(hit.kind, "exact");

        let glued = prepare_query("高鐵官方網站");
        let glued_hit = score_query(&glued, rail);
        assert!(glued_hit.score >= HIT_MIN, "glued {}", glued_hit.score);
        assert!(glued_hit.matched_terms.iter().any(|t| t == "高鐵"));

        let starbucks_aliases = vec!["星巴克".to_string()];
        let only_brand = fields("星巴克台灣", &starbucks_aliases, "星巴克台灣。");
        let both = fields("星巴克巧克力", &starbucks_aliases, "星巴克巧克力可頌。");
        let q = prepare_query("星巴克 杜拜 巧克力 可頌");
        let partial = score_query(&q, only_brand);
        let full = score_query(&q, both);
        assert!(partial.score >= HIT_MIN, "partial {}", partial.score);
        assert_eq!(partial.matched_terms, vec!["星巴克".to_string()]);
        assert!(
            full.score > partial.score,
            "full {} should outrank partial {}",
            full.score,
            partial.score
        );
        assert!(full.matched_terms.len() > partial.matched_terms.len());
    }
}
