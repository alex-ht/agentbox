//! Heuristic person + role finder ("Jensen Huang, CEO of Nvidia",
//! "Nvidia CEO Jensen Huang", "Jensen Huang (Chief Executive Officer)",
//! "Jensen Huang is the founder and CEO of Nvidia", "執行長黃仁勳").
//!
//! Pattern-based only: it finds name/role pairs written in common shapes and
//! reports a confidence per pattern. It does not know who anyone is.

use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq)]
pub struct PersonHit {
    pub name: String,
    pub role: String,
    pub org: Option<String>,
    pub confidence: f64,
    pub start: usize,
    pub end: usize,
}

const WORD: &str = r"(?:Mc|Mac|O')?\p{Lu}\p{Ll}+(?:-\p{Lu}\p{Ll}+)?";
const CAPS: &str = r"\p{Lu}[\p{L}\d&.'’-]*(?:\s+(?:&\s+)?\p{Lu}[\p{L}\d&.'’-]*){0,3}";
const ORG_ROLE: &str = r"(?i:co-?founder|founder|owner|president|chair(?:man|woman|person)?|vice[- ]chair(?:man|woman|person)?|ceo|cfo|cto|coo|cio|cmo|cpo|cro|cso|ciso|general\s+manager|managing\s+partner|partner|chief\s+[a-z]+(?:\s+[a-z]+)?\s+officer|chief\s+(?:economist|scientist|architect)|editor-in-chief|principal)";
const DEPT_ROLE: &str = r"(?i:(?:senior\s+|executive\s+)?vice\s+president|s?e?vp|head|(?:executive\s+|non-executive\s+|managing\s+)?director|(?:prime\s+)?minister|secretary(?:-general)?|governor|mayor|senator|commissioner|chancellor|dean|professor)";
const MODS: &str = r"(?:(?i:current|former|new|interim|acting|incoming|outgoing)\s+)?";

fn name_re() -> String {
    format!(
        r"(?P<name>{WORD}(?:\s+\p{{Lu}}\.)?(?:\s+(?:(?:van|von|de|der|den|da|di|du|del|la|le|bin|al)\s+){{0,2}}{WORD}){{1,2}})"
    )
}

fn roles_re() -> String {
    let dept = format!(r"{DEPT_ROLE}(?:\s+(?i:of|for)\s+(?:the\s+)?{CAPS})?");
    let atom = format!(r"(?:{dept}|{ORG_ROLE})");
    format!(r"(?P<role>{atom}(?:\s*(?:,|and|&)\s*{atom}){{0,3}})")
}

fn org_tail() -> String {
    format!(r"(?:,?\s+(?:of|at)\s+(?:the\s+)?(?P<org>{CAPS}))?")
}

static PATTERNS: LazyLock<Vec<(f64, Regex)>> = LazyLock::new(|| {
    let (n, r, o) = (name_re(), roles_re(), org_tail());
    let p = |s: String| Regex::new(&s).expect("valid people regex");
    vec![
        (
            0.9,
            p(format!(
                r"{n},?\s+(?:is|was|serves\s+as|served\s+as|became|has\s+been|remains)\s+(?:also\s+)?(?:the\s+|a\s+|an\s+|its\s+)?{MODS}{r}{o}"
            )),
        ),
        (0.8, p(format!(r"{n},\s+(?:the\s+|its\s+)?{MODS}{r}{o}"))),
        (
            0.8,
            p(format!(
                r"{n}\s*\({MODS}{r}(?:(?:,\s*|\s+(?:of|at)\s+)(?:the\s+)?(?P<org>{CAPS}))?\)"
            )),
        ),
        (
            0.7,
            p(format!(r"(?:(?P<org>{CAPS})(?:'s|’s)?\s+)?{MODS}{r}\s+{n}")),
        ),
        (0.6, p(format!(r"{n}\s*(?:—|–|\s-\s|\||:)\s*{r}{o}"))),
        (0.6, p(format!(r"{r}\s*[:：]\s*{n}"))),
    ]
});

static CJK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?P<role>共同創辦人|創辦人|副董事長|董事長|執行長|總經理|財務長|技術長|營運長|副總裁|總裁|總監|部長|院長|市長|主席|首席執行官|CEO)\s?(?P<name>\p{Han}{2,3})")
        .expect("valid regex")
});

/// Words that cannot be part of a person's name.
const NOT_NAME: &[&str] = &[
    "The",
    "A",
    "An",
    "Our",
    "Their",
    "His",
    "Her",
    "Its",
    "This",
    "That",
    "These",
    "Those",
    "Today",
    "Yesterday",
    "Meanwhile",
    "However",
    "According",
    "When",
    "After",
    "Before",
    "In",
    "On",
    "At",
    "As",
    "And",
    "But",
    "Also",
    "Then",
    "Meet",
    "Former",
    "Current",
    "Interim",
    "Acting",
    "New",
    "Chief",
    "Executive",
    "Officer",
    "President",
    "Vice",
    "Senior",
    "Director",
    "Founder",
    "Chair",
    "Chairman",
    "Head",
    "Board",
    "Team",
    "Company",
    "Group",
    "Inc",
    "Corp",
    "Corporation",
    "University",
    "Department",
    "Ministry",
    "Minister",
    "Secretary",
    "Governor",
    "Mayor",
    "Senator",
    "Says",
    "Said",
    "Mr",
    "Ms",
    "Mrs",
    "Dr",
    "Sir",
    "Read",
    "More",
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
    "Partner",
    "Owner",
    "Global",
    "Technology",
    "Technologies",
    "Systems",
    "Holdings",
    "Bank",
    "Capital",
    "Labs",
    "News",
];

const NOT_CJK_NAME: &[char] = &[
    '的', '是', '在', '了', '表', '示', '說', '说', '也', '將', '将', '與', '与', '和', '及', '為',
    '为', '等', '就', '都', '而', '指', '出', '認', '今', '昨', '曾', '已',
];

/// Drop leading non-name words; need at least two name words left.
fn clean_name(raw: &str) -> Option<String> {
    let words: Vec<&str> = raw.split_whitespace().collect();
    let start = words
        .iter()
        .position(|w| !NOT_NAME.contains(&w.trim_end_matches('.')))?;
    let rest = &words[start..];
    if rest.len() < 2
        || rest
            .iter()
            .any(|w| NOT_NAME.contains(&w.trim_end_matches('.')))
    {
        return None;
    }
    Some(rest.join(" "))
}

fn clean_org(raw: &str) -> Option<String> {
    let words: Vec<&str> = raw
        .trim()
        .trim_end_matches(['.', ',', ';', ':'])
        .trim_end_matches("'s")
        .trim_end_matches("’s")
        .split_whitespace()
        .collect();
    let start = words.iter().position(|w| {
        !matches!(
            *w,
            "The"
                | "A"
                | "An"
                | "Our"
                | "Meanwhile"
                | "However"
                | "Today"
                | "In"
                | "On"
                | "At"
                | "As"
                | "And"
                | "But"
                | "Former"
                | "Current"
                | "New"
                | "Interim"
                | "Acting"
        )
    })?;
    let org = words[start..].join(" ");
    (!org.is_empty()).then_some(org)
}

fn clean_role(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// All name/role pairs in a line of plain text.
pub fn find_people(text: &str) -> Vec<PersonHit> {
    let mut out: Vec<PersonHit> = Vec::new();
    let push = |hit: PersonHit, out: &mut Vec<PersonHit>| {
        let dup = out.iter().any(|h| {
            h.name.eq_ignore_ascii_case(&hit.name)
                && (h.role.eq_ignore_ascii_case(&hit.role)
                    || (hit.start < h.end && h.start < hit.end))
        });
        if !dup {
            out.push(hit);
        }
    };
    for (conf, re) in PATTERNS.iter() {
        for c in re.captures_iter(text) {
            let (Some(n), Some(r)) = (c.name("name"), c.name("role")) else {
                continue;
            };
            let Some(name) = clean_name(n.as_str()) else {
                continue;
            };
            let whole = c.get(0).expect("match");
            push(
                PersonHit {
                    name,
                    role: clean_role(r.as_str()),
                    org: c.name("org").and_then(|o| clean_org(o.as_str())),
                    confidence: *conf,
                    start: whole.start(),
                    end: whole.end(),
                },
                &mut out,
            );
        }
    }
    for c in CJK.captures_iter(text) {
        let (Some(n), Some(r)) = (c.name("name"), c.name("role")) else {
            continue;
        };
        let name = n.as_str();
        if name.chars().any(|ch| NOT_CJK_NAME.contains(&ch)) {
            continue;
        }
        let whole = c.get(0).expect("match");
        push(
            PersonHit {
                name: name.to_string(),
                role: r.as_str().to_string(),
                org: None,
                confidence: 0.5,
                start: whole.start(),
                end: whole.end(),
            },
            &mut out,
        );
    }
    out.sort_by_key(|h| h.start);
    out
}
