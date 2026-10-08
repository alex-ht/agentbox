//! Hand-written tool spec table. It drives `agentbox schema` (OpenAI-style
//! function tools) and `agentbox call` (JSON args -> CLI argv). Tests check
//! that it stays in sync with the clap definitions.

use crate::envelope::{AppError, CmdResult, Output};
use serde_json::{json, Map, Value};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Str,
    Int,
    Bool,
    /// Array of strings; becomes a repeated `--flag` on the CLI.
    List,
}

#[derive(Debug, Clone, Copy)]
pub struct Param {
    pub name: &'static str,
    pub kind: Kind,
    pub required: bool,
    pub positional: bool,
    pub desc: &'static str,
    pub default: Option<&'static str>,
    pub choices: &'static [&'static str],
}

#[derive(Debug, Clone, Copy)]
pub struct Spec {
    /// Function name exposed to the agent, e.g. `file_read`.
    pub name: &'static str,
    /// CLI path, e.g. `["file", "read"]`.
    pub path: &'static [&'static str],
    pub implemented: bool,
    pub desc: &'static str,
    pub params: &'static [Param],
}

const fn pos(name: &'static str, desc: &'static str) -> Param {
    Param {
        name,
        kind: Kind::Str,
        required: true,
        positional: true,
        desc,
        default: None,
        choices: &[],
    }
}
const fn pos_opt(name: &'static str, desc: &'static str) -> Param {
    Param {
        name,
        kind: Kind::Str,
        required: false,
        positional: true,
        desc,
        default: None,
        choices: &[],
    }
}
const fn req_pick(
    name: &'static str,
    choices: &'static [&'static str],
    desc: &'static str,
) -> Param {
    Param {
        name,
        kind: Kind::Str,
        required: true,
        positional: false,
        desc,
        default: None,
        choices,
    }
}
const fn opt(name: &'static str, kind: Kind, desc: &'static str) -> Param {
    Param {
        name,
        kind,
        required: false,
        positional: false,
        desc,
        default: None,
        choices: &[],
    }
}
const fn req(name: &'static str, desc: &'static str) -> Param {
    Param {
        name,
        kind: Kind::Str,
        required: true,
        positional: false,
        desc,
        default: None,
        choices: &[],
    }
}
const fn dflt(name: &'static str, kind: Kind, default: &'static str, desc: &'static str) -> Param {
    Param {
        name,
        kind,
        required: false,
        positional: false,
        desc,
        default: Some(default),
        choices: &[],
    }
}
const fn flag(name: &'static str, desc: &'static str) -> Param {
    Param {
        name,
        kind: Kind::Bool,
        required: false,
        positional: false,
        desc,
        default: None,
        choices: &[],
    }
}
const fn pick(name: &'static str, choices: &'static [&'static str], desc: &'static str) -> Param {
    Param {
        name,
        kind: Kind::Str,
        required: false,
        positional: false,
        desc,
        default: None,
        choices,
    }
}
const fn choice(
    name: &'static str,
    default: &'static str,
    choices: &'static [&'static str],
    desc: &'static str,
) -> Param {
    Param {
        name,
        kind: Kind::Str,
        required: false,
        positional: false,
        desc,
        default: Some(default),
        choices,
    }
}

pub const SPECS: &[Spec] = &[
    Spec {
        name: "fetch",
        path: &["fetch"],
        implemented: true,
        desc: "Download a web page, convert it to clean Markdown and store it. Returns a doc handle (e.g. doc:3), title and section outline; read the content with `read`.",
        params: &[pos("url", "Page URL; https:// is added if missing"), dflt("timeout", Kind::Int, "30", "Timeout in seconds")],
    },
    Spec {
        name: "read",
        path: &["read"],
        implemented: true,
        desc: "Read a stored doc. Pass section to get one section, grep to get snippets around a keyword, or neither to read from the start. Long output is truncated; continue with offset.",
        params: &[
            pos("doc", "Doc handle from fetch, e.g. doc:3"),
            opt("section", Kind::Int, "Section number from the outline"),
            opt("grep", Kind::Str, "Keyword to find (case-insensitive); returns snippets with their section numbers"),
            dflt("max_chars", Kind::Int, "4000", "Maximum characters to return"),
            dflt("offset", Kind::Int, "0", "Character offset to continue from (next_offset of the previous call)"),
        ],
    },
    Spec {
        name: "calc",
        path: &["calc"],
        implemented: true,
        desc: "Evaluate an arithmetic expression exactly as written. Supports + - * / % ^, parentheses, pi, e, and sqrt abs round(x,digits) floor ceil ln log log2 log10 exp pow min max sum avg.",
        params: &[pos("expr", "Expression, e.g. (182.5 - 170) / 170 * 100")],
    },
    Spec {
        name: "now",
        path: &["now"],
        implemented: true,
        desc: "Get the current date, time, weekday and UTC offset, in the local timezone or a given one.",
        params: &[opt("tz", Kind::Str, "IANA timezone like Asia/Taipei or America/New_York, or an offset like +08:00")],
    },
    Spec {
        name: "file_read",
        path: &["file", "read"],
        implemented: true,
        desc: "Read a local UTF-8 text file, optionally only a line range.",
        params: &[
            pos("path", "File path"),
            opt("lines", Kind::Str, "Line range START:END, 1-based inclusive, e.g. 10:40"),
            dflt("max_chars", Kind::Int, "20000", "Maximum characters to return"),
        ],
    },
    Spec {
        name: "file_write",
        path: &["file", "write"],
        implemented: true,
        desc: "Write a whole text file. Without apply=true it only returns a diff preview; call again with apply=true to write.",
        params: &[pos("path", "File path"), req("content", "Full new file content"), flag("apply", "Actually write the file")],
    },
    Spec {
        name: "file_replace",
        path: &["file", "replace"],
        implemented: true,
        desc: "Replace exact text in a file. The find text must occur exactly once unless all=true. Without apply=true it only returns a diff preview.",
        params: &[
            pos("path", "File path"),
            req("find", "Exact text to find, including whitespace"),
            req("replace", "Replacement text"),
            flag("all", "Replace every occurrence"),
            flag("apply", "Actually write the file"),
        ],
    },
    Spec {
        name: "note_add",
        path: &["note", "add"],
        implemented: true,
        desc: "Save a short fact or finding to the scratchpad, with its source, so it can be cited later.",
        params: &[
            pos("text", "The note text"),
            opt("source", Kind::Str, "Source URL or doc handle"),
            opt("tag", Kind::Str, "Tag for grouping, e.g. pricing"),
        ],
    },
    Spec {
        name: "note_list",
        path: &["note", "list"],
        implemented: true,
        desc: "List saved notes and the distinct sources they cite.",
        params: &[
            opt("tag", Kind::Str, "Only notes with this tag"),
            opt("grep", Kind::Str, "Only notes containing this keyword"),
            dflt("limit", Kind::Int, "50", "Maximum notes to return (latest kept)"),
        ],
    },
    Spec {
        name: "search",
        path: &["search"],
        implemented: true,
        desc: "Search the web. Returns ranked results (title, url, snippet) and which backend was used. Use save to store the top results as doc handles you can `read` directly.",
        params: &[
            pos("query", "Search query"),
            dflt("max_results", Kind::Int, "5", "Number of results, 1-20"),
            opt("site", Kind::List, "Only these domains, e.g. [\"europa.eu\"]"),
            opt("exclude_site", Kind::List, "Exclude these domains"),
            opt("days", Kind::Int, "Only results from the last N days"),
            pick("time", &["day", "week", "month", "year"], "Only results from the last day/week/month/year"),
            flag("news", "News sources only (best with Tavily)"),
            flag("deep", "Deeper, more relevant search (Tavily, slower)"),
            flag("answer", "Include a short generated answer (Tavily)"),
            dflt("save", Kind::Int, "0", "Also store the top N results (max 5) as doc handles"),
            pick("backend", &["auto", "tavily", "ddg", "bing"], "Search backend; auto uses Tavily when a key is configured"),
        ],
    },
    Spec {
        name: "extract",
        path: &["extract"],
        implemented: true,
        desc: "Pull structured items out of stored docs or local files, deterministically (no regex needed). kind=tables saves each table as tbl:N; prices gives value/currency/period/per; dates gives ISO dates; people gives heuristic name+role pairs; links, numbers (with units/percent/magnitude) and emails. Every item carries its doc, url, section and line for citing.",
        params: &[
            pos_opt("sources", "Doc handle(s) or file path(s), comma-separated, e.g. doc:1 or doc:1,doc:2"),
            req_pick("kind", &["tables", "prices", "dates", "people", "links", "numbers", "emails"], "What to extract"),
            opt("from", Kind::Str, "More sources, comma-separated (same as sources)"),
            opt("section", Kind::Int, "Only this section number from the doc outline"),
            opt("grep", Kind::Str, "Keep only items whose text contains this keyword"),
            dflt("limit", Kind::Int, "20", "Maximum items returned"),
            opt("site", Kind::Str, "kind=links only: keep links to this domain"),
            flag("save_table", "Also save all items as a table (tbl:N) to sort/filter with table_query"),
        ],
    },
    Spec {
        name: "table_show",
        path: &["table", "show"],
        implemented: true,
        desc: "Show a table's columns with inferred types (number/currency/date/text), its row count and first rows. Source is a tbl:N handle (from extract or table_import) or a CSV/TSV/JSON/Markdown file.",
        params: &[
            pos("source", "Table handle like tbl:2, or a table file path"),
            dflt("limit", Kind::Int, "20", "Rows to show"),
        ],
    },
    Spec {
        name: "table_query",
        path: &["table", "query"],
        implemented: true,
        desc: "Filter, sort, group and aggregate table rows. where items look like \"Price < 100\" (ops: = != < <= > >= contains startswith; ANDed); numbers such as \"$1,299/mo\" compare numerically. sort items are column names, prefix - for descending. Optionally save the result as a new tbl:N.",
        params: &[
            pos("source", "Table handle like tbl:2, or a table file path"),
            opt("select", Kind::Str, "Comma-separated columns to keep, e.g. Plan,Price"),
            opt("where", Kind::List, "Row filters, e.g. [\"Price < 100\", \"Plan contains pro\"]"),
            opt("sort", Kind::List, "Sort keys, e.g. [\"-Price\"] for most expensive first"),
            opt("limit", Kind::Int, "Maximum rows returned (default 20)"),
            opt("group_by", Kind::Str, "Group rows by this column"),
            opt("agg", Kind::Str, "Aggregates, e.g. sum:Price,avg:Price,count"),
            flag("save", "Save the result as a new tbl:N"),
        ],
    },
    Spec {
        name: "table_import",
        path: &["table", "import"],
        implemented: true,
        desc: "Import a CSV, TSV, JSON (array of objects) or Markdown pipe-table file as a tbl:N handle.",
        params: &[pos("file", "Path to the table file")],
    },
    Spec {
        name: "table_export",
        path: &["table", "export"],
        implemented: true,
        desc: "Write a table to a .csv, .tsv, .md or .json file (format from the extension). Shows a diff preview unless apply is true.",
        params: &[
            pos("source", "Table handle like tbl:2, or a table file path"),
            req("out", "Output file path, e.g. prices.csv"),
            flag("apply", "Actually write the file"),
        ],
    },
    Spec {
        name: "quote_get",
        path: &["quote", "get"],
        implemented: true,
        desc: "Latest price, previous close, change and change %, currency, exchange, market state, time and 52-week high/low for stocks, indices, FX and crypto. Symbols use Yahoo format: NVDA, 2330.TW (Taiwan TWSE), 6488.TWO (Taiwan OTC), 0700.HK, ^GSPC, ^TWII, USDTWD=X, BTC-USD. Data may be delayed ~15 min.",
        params: &[
            pos("symbols", "One or more symbols, comma-separated, e.g. NVDA,2330.TW,^TWII"),
            choice("backend", "auto", &["auto", "yahoo", "stooq"], "Data source; auto falls back to Stooq if Yahoo is blocked"),
        ],
    },
    Spec {
        name: "quote_history",
        path: &["quote", "history"],
        implemented: true,
        desc: "Price history (date, open, high, low, close, volume) with a summary: start/end close, % change, high and low with dates. save=true stores all rows as tbl:N for table_query.",
        params: &[
            pos("symbol", "Symbol, e.g. NVDA or 2330.TW"),
            choice("range", "1mo", &["5d", "1mo", "3mo", "6mo", "ytd", "1y", "5y", "max"], "Time window"),
            pick("interval", &["1d", "1wk", "1mo"], "Bar size (default 1d; 1wk for 5y; 1mo for max)"),
            choice("backend", "auto", &["auto", "yahoo", "stooq"], "Data source"),
            flag("save", "Save all rows as a table (tbl:N)"),
        ],
    },
    Spec {
        name: "quote_search",
        path: &["quote", "search"],
        implemented: true,
        desc: "Find ticker symbols by company name or keyword (e.g. \"Taiwan Semiconductor\"; Chinese names of major Taiwan/HK stocks like 台積電 also work). Returns symbol, name, exchange and type.",
        params: &[pos("query", "Company name, keyword or ticker"), dflt("limit", Kind::Int, "10", "Maximum results")],
    },
    Spec {
        name: "market_search",
        path: &["market", "search"],
        implemented: true,
        desc: "Search Polymarket prediction markets by keyword (every word must appear in the event title, questions or description). Returns events with outcome probabilities in %, volume, 24h volume, liquidity, end date and polymarket.com URL. Default: active events sorted by volume.",
        params: &[
            pos("query", "Keywords, e.g. \"fed rate\" or election"),
            dflt("limit", Kind::Int, "10", "Maximum events returned"),
            flag("active", "Only active events (the default)"),
            flag("closed", "Only closed/resolved events; with active=true, both"),
            choice("sort", "volume", &["volume", "liquidity", "end", "newest"], "Order of results"),
            opt("tag", Kind::Str, "Tag slug filter, e.g. politics, crypto, sports"),
            flag("save_table", "Also save every market of the results as a table (tbl:N)"),
        ],
    },
    Spec {
        name: "market_get",
        path: &["market", "get"],
        implemented: true,
        desc: "One Polymarket event with all its markets and outcome probabilities, rules (description) and URLs; or one market. Accepts a slug, numeric id or polymarket.com URL.",
        params: &[
            pos("id", "Event/market slug, id, or polymarket.com URL"),
            dflt("limit", Kind::Int, "20", "Maximum markets listed for multi-market events"),
        ],
    },
    Spec {
        name: "market_trending",
        path: &["market", "trending"],
        implemented: true,
        desc: "The most active Polymarket events right now, by 24h volume, optionally for one tag (politics, crypto, sports, ...).",
        params: &[
            dflt("limit", Kind::Int, "10", "Maximum events returned"),
            opt("tag", Kind::Str, "Tag slug filter, e.g. politics, crypto, sports"),
            flag("save_table", "Also save every open market of the results as a table (tbl:N)"),
        ],
    },
    Spec {
        name: "market_history",
        path: &["market", "history"],
        implemented: true,
        desc: "Probability over time (in %) for one Polymarket market, with a summary of the change in percentage points, high and low. Pass a market slug or id from market_get.",
        params: &[
            pos("id", "Market slug or id (or a single-market event slug / URL)"),
            choice("interval", "1w", &["1d", "1w", "1m", "max"], "Window: 1d, 1w, 1m or max"),
            flag("save", "Save all points as a table (tbl:N)"),
        ],
    },
    Spec {
        name: "report_build",
        path: &["report", "build"],
        implemented: true,
        desc: "Draft a Markdown report skeleton from a template: exact headings and numbering, <!-- TODO(n) --> placeholders, your notes with citations, and a Sources section. Fill TODOs with file_replace, then run report_check.",
        params: &[
            pos("title", "Report title (the H1)"),
            dflt("template", Kind::Str, "brief", "Template: brief, compare, top-n, exec-lookup, a user template name, or a .toml path"),
            opt("n", Kind::Int, "Number of numbered items for templates like top-n"),
            opt("columns", Kind::Str, "Comma-separated table columns for templates like compare"),
            opt("tag", Kind::Str, "Only use notes with this tag"),
            opt("out", Kind::Str, "Output file path; omit to get the draft inline"),
            flag("apply", "Actually write the out file"),
        ],
    },
    Spec {
        name: "report_check",
        path: &["report", "check"],
        implemented: true,
        desc: "Check a Markdown report against a template (heading levels, numbering, required sections, citations, sources, length, leftover TODOs). Returns pass, score and issues; each issue has a concrete fix, often a ready file_replace command.",
        params: &[
            pos("file", "Markdown file to check"),
            dflt("template", Kind::Str, "brief", "Template name or .toml path (same as used for report_build)"),
            opt("n", Kind::Int, "Required number of numbered items"),
            opt("columns", Kind::Str, "Required table columns, comma-separated"),
        ],
    },
    Spec {
        name: "report_templates",
        path: &["report", "templates"],
        implemented: true,
        desc: "List available report templates (built-in and user) with descriptions.",
        params: &[],
    },
    Spec {
        name: "report_template_show",
        path: &["report", "template", "show"],
        implemented: true,
        desc: "Show a report template's TOML (required sections, levels, word limits, citation style).",
        params: &[pos("name", "Template name or path")],
    },
];

pub fn find(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.name == name)
}

/// OpenAI-style function tool definition for one spec.
pub fn tool_json(spec: &Spec) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for p in spec.params {
        let ty = match p.kind {
            Kind::Str => "string",
            Kind::Int => "integer",
            Kind::Bool => "boolean",
            Kind::List => "array",
        };
        let mut prop = json!({ "type": ty, "description": p.desc });
        if p.kind == Kind::List {
            prop["items"] = json!({ "type": "string" });
        }
        if !p.choices.is_empty() {
            prop["enum"] = json!(p.choices);
        }
        if let Some(d) = p.default {
            prop["default"] = match p.kind {
                Kind::Int => json!(d.parse::<i64>().unwrap_or(0)),
                Kind::Bool => json!(d == "true"),
                Kind::Str | Kind::List => json!(d),
            };
        }
        props.insert(p.name.to_string(), prop);
        if p.required {
            required.push(p.name);
        }
    }
    let desc = if spec.implemented {
        spec.desc.to_string()
    } else {
        format!("[planned, not implemented yet] {}", spec.desc)
    };
    json!({
        "type": "function",
        "function": {
            "name": spec.name,
            "description": desc,
            "parameters": {
                "type": "object",
                "properties": props,
                "required": required,
                "additionalProperties": false,
            }
        }
    })
}

pub fn run_schema(implemented_only: bool) -> CmdResult {
    let tools: Vec<Value> = SPECS
        .iter()
        .filter(|s| s.implemented || !implemented_only)
        .map(tool_json)
        .collect();
    Ok(Output::new(json!({ "tools": tools })).hint(
        "Expose each entry as a function tool; run a call with `agentbox call <name> '<json args>'`.",
    ))
}

/// Translate a tool call (name + JSON object) into CLI argv (without program name).
pub fn to_argv(name: &str, args: &Value) -> Result<Vec<String>, AppError> {
    let spec = find(name).ok_or_else(|| {
        let names: Vec<&str> = SPECS.iter().map(|s| s.name).collect();
        AppError::new(
            "unknown_tool",
            format!("no tool named `{name}`"),
            format!("Valid names: {}.", names.join(", ")),
        )
    })?;
    let obj = match args {
        Value::Object(m) => m,
        Value::Null => &Map::new(),
        _ => {
            return Err(AppError::new(
                "bad_args",
                "arguments must be a JSON object",
                r#"Example: {"url":"https://example.com"}"#,
            ));
        }
    };
    let valid: Vec<&str> = spec.params.iter().map(|p| p.name).collect();
    if let Some(k) = obj.keys().find(|k| !valid.contains(&k.as_str())) {
        return Err(AppError::new(
            "bad_args",
            format!("`{name}` has no parameter `{k}`"),
            format!("Valid parameters: {}.", valid.join(", ")),
        ));
    }
    let mut argv: Vec<String> = spec.path.iter().map(|s| s.to_string()).collect();
    let mut positionals = Vec::new();
    for p in spec.params {
        let v = match obj.get(p.name) {
            None | Some(Value::Null) => {
                if p.required {
                    return Err(AppError::new(
                        "bad_args",
                        format!("missing required parameter `{}`", p.name),
                        format!(
                            "`{name}` needs: {}.",
                            spec.params
                                .iter()
                                .filter(|p| p.required)
                                .map(|p| p.name)
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                }
                continue;
            }
            Some(v) => v,
        };
        let text = match (p.kind, v) {
            (Kind::List, Value::Array(items)) => {
                for it in items {
                    let Some(s) = it.as_str() else {
                        return Err(AppError::new(
                            "bad_args",
                            format!("parameter `{}` must be an array of strings", p.name),
                            format!(r#"Example: "{}": ["example.com"]"#, p.name),
                        ));
                    };
                    argv.push(format!("--{}={s}", p.name.replace('_', "-")));
                }
                continue;
            }
            (Kind::List, Value::String(s)) => {
                argv.push(format!("--{}={s}", p.name.replace('_', "-")));
                continue;
            }
            (Kind::Bool, Value::Bool(b)) => {
                if *b {
                    argv.push(format!("--{}", p.name.replace('_', "-")));
                }
                continue;
            }
            (Kind::Int, Value::Number(n)) if n.is_u64() || n.is_i64() => n.to_string(),
            (Kind::Int, Value::String(s)) if s.trim().parse::<i64>().is_ok() => {
                s.trim().to_string()
            }
            (Kind::Str, Value::String(s)) => s.clone(),
            (Kind::Str, Value::Number(n)) => n.to_string(),
            _ => {
                return Err(AppError::new(
                    "bad_args",
                    format!("parameter `{}` has the wrong type", p.name),
                    format!("`{}` must be a {:?} value.", p.name, p.kind).to_lowercase(),
                ));
            }
        };
        if p.positional {
            positionals.push(text);
        } else {
            // `--name=value` keeps values that start with '-' intact.
            argv.push(format!("--{}={text}", p.name.replace('_', "-")));
        }
    }
    if !positionals.is_empty() {
        argv.push("--".into());
        argv.extend(positionals);
    }
    Ok(argv)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::CommandFactory;
    use clap::Parser;

    fn sample(p: &Param) -> Value {
        match p.kind {
            Kind::Bool => json!(true),
            Kind::Int => json!(3),
            Kind::List => json!(["a.test", "-b.test"]),
            Kind::Str if !p.choices.is_empty() => json!(p.choices[0]),
            Kind::Str => json!("-sample value"),
        }
    }

    #[test]
    fn every_spec_parses_through_clap() {
        for spec in SPECS {
            let args: Map<String, Value> = spec
                .params
                .iter()
                .map(|p| (p.name.to_string(), sample(p)))
                .collect();
            let argv = to_argv(spec.name, &Value::Object(args)).unwrap();
            let full: Vec<String> = std::iter::once("agentbox".to_string())
                .chain(argv.clone())
                .collect();
            if let Err(e) = Cli::try_parse_from(&full) {
                panic!("{} -> {:?} failed: {e}", spec.name, argv);
            }
        }
    }

    fn collect_leaves(name: &str, cmd: &clap::Command, out: &mut Vec<(String, clap::Command)>) {
        if cmd.has_subcommands() {
            for sub in cmd.get_subcommands() {
                collect_leaves(&format!("{name}_{}", sub.get_name()), sub, out);
            }
        } else {
            out.push((name.to_string(), cmd.clone()));
        }
    }

    /// Every leaf subcommand except schema/call has a spec whose params match
    /// the clap args exactly (names, positional-ness, defaults, choices).
    #[test]
    fn specs_match_clap_definitions() {
        let root = Cli::command();
        let mut leaves = Vec::new();
        for sc in root.get_subcommands() {
            let name = sc.get_name().to_string();
            // Meta commands are not agent tools; config is kept away from
            // agents on purpose so API keys never pass through transcripts.
            if name == "schema" || name == "call" || name == "config" {
                continue;
            }
            collect_leaves(&name, sc, &mut leaves);
        }
        assert_eq!(
            leaves.len(),
            SPECS.len(),
            "spec table and clap leaves differ in count"
        );
        for (name, cmd) in leaves {
            let spec = find(&name).unwrap_or_else(|| panic!("no spec for `{name}`"));
            let args: Vec<_> = cmd
                .get_arguments()
                .filter(|a| !matches!(a.get_id().as_str(), "help" | "version" | "format"))
                .collect();
            assert_eq!(
                args.len(),
                spec.params.len(),
                "param count mismatch for {name}"
            );
            for a in args {
                let id = a.get_id().as_str();
                let p = spec
                    .params
                    .iter()
                    .find(|p| p.name == id)
                    .unwrap_or_else(|| panic!("{name}: spec lacks `{id}`"));
                assert_eq!(a.is_positional(), p.positional, "{name}.{id} positional");
                assert_eq!(a.is_required_set(), p.required, "{name}.{id} required");
                let clap_default: Vec<String> = a
                    .get_default_values()
                    .iter()
                    .map(|v| v.to_string_lossy().to_string())
                    .collect();
                if p.kind != Kind::Bool {
                    assert_eq!(
                        clap_default.first().map(String::as_str),
                        p.default,
                        "{name}.{id} default"
                    );
                }
                let clap_choices: Vec<String> = a
                    .get_possible_values()
                    .iter()
                    .map(|v| v.get_name().to_string())
                    .collect();
                if p.kind == Kind::Str {
                    assert_eq!(
                        clap_choices,
                        p.choices.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                        "{name}.{id} choices"
                    );
                }
            }
            // Planned commands are marked in help text.
            let about = cmd.get_about().map(|s| s.to_string()).unwrap_or_default();
            assert_eq!(
                about.starts_with("[planned]"),
                !spec.implemented,
                "{name} about/implemented mismatch"
            );
        }
    }

    #[test]
    fn schema_json_shape() {
        let out = run_schema(false).unwrap();
        let tools = out.data["tools"].as_array().unwrap();
        assert_eq!(tools.len(), SPECS.len());
        let read = tools
            .iter()
            .find(|t| t["function"]["name"] == "read")
            .unwrap();
        assert_eq!(read["type"], "function");
        assert_eq!(read["function"]["parameters"]["required"], json!(["doc"]));
        assert_eq!(
            read["function"]["parameters"]["properties"]["max_chars"]["default"],
            4000
        );
        let only = run_schema(true).unwrap();
        assert_eq!(
            only.data["tools"].as_array().unwrap().len(),
            SPECS.iter().filter(|s| s.implemented).count()
        );
    }

    #[test]
    fn to_argv_errors() {
        assert_eq!(
            to_argv("nope", &json!({})).unwrap_err().code,
            "unknown_tool"
        );
        assert_eq!(to_argv("fetch", &json!({})).unwrap_err().code, "bad_args");
        assert_eq!(
            to_argv("fetch", &json!({"url":"x","bogus":1}))
                .unwrap_err()
                .code,
            "bad_args"
        );
        assert_eq!(
            to_argv("read", &json!({"doc":"doc:1","section":"two"}))
                .unwrap_err()
                .code,
            "bad_args"
        );
        let argv = to_argv(
            "file_write",
            &json!({"path":"a.md","content":"x","apply":false}),
        )
        .unwrap();
        assert_eq!(argv, vec!["file", "write", "--content=x", "--", "a.md"]);
    }
}
