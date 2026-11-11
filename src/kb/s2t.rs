//! Simplified-to-Traditional conversion for CrUX page titles.
//!
//! The maps are OpenCC's `STCharacters` and `TWVariants` (Apache-2.0,
//! https://github.com/BYVoid/OpenCC). Conversion is character by character,
//! then Taiwan glyph variants (爲 → 為). Whole-word Taiwan phrases such as
//! 博客 → 部落格 are not applied: they rewrite proper names (博客來).

use std::collections::HashMap;
use std::sync::OnceLock;

pub fn to_traditional(s: &str) -> String {
    if !s.chars().any(han) {
        return s.to_string();
    }
    s.chars()
        .map(|c| char_map().get(&c).copied().unwrap_or(c))
        .map(|c| tw_variants().get(&c).copied().unwrap_or(c))
        .collect()
}

fn han(c: char) -> bool {
    ('\u{3400}'..='\u{9fff}').contains(&c)
}

fn char_map() -> &'static HashMap<char, char> {
    static MAP: OnceLock<HashMap<char, char>> = OnceLock::new();
    MAP.get_or_init(|| load_char_map(include_str!("s2t_chars.txt")))
}

fn tw_variants() -> &'static HashMap<char, char> {
    static MAP: OnceLock<HashMap<char, char>> = OnceLock::new();
    MAP.get_or_init(|| load_char_map(include_str!("s2t_tw_variants.txt")))
}

fn load_char_map(text: &str) -> HashMap<char, char> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, values)) = line.split_once('\t') else {
            continue;
        };
        let Some(from) = key.chars().next() else {
            continue;
        };
        if key.chars().count() != 1 {
            continue;
        }
        let Some(to) = values
            .split_whitespace()
            .next()
            .and_then(|s| s.chars().next())
        else {
            continue;
        };
        if to != from {
            map.insert(from, to);
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_simplified_titles_and_leaves_traditional() {
        assert_eq!(to_traditional("围棋网"), "圍棋網");
        assert_eq!(to_traditional("网络小说"), "網絡小說");
        assert_eq!(to_traditional("软件"), "軟件");
        assert_eq!(to_traditional("在线视频"), "在線視頻");
        assert_eq!(to_traditional("为"), "為");
        // OpenCC maps 台 to the formal traditional 臺. Search folding treats them as equal.
        assert_eq!(to_traditional("台灣高鐵"), "臺灣高鐵");
        // A brand must not be rewritten as the common noun 部落格.
        assert_eq!(to_traditional("博客來"), "博客來");
        assert_eq!(to_traditional("YouTube"), "YouTube");
    }
}
