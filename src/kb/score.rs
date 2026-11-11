//! Normalized ranking for knowledge-base search.
//!
//! Scores are in `0.0..=1.0` (higher is better). `1.0` is an exact normalized
//! name or alias. Trigram retrieval is separate; this score is what every
//! candidate, including the short-query and typo fallback, is sorted by.

use std::collections::{HashMap, HashSet};

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
}
