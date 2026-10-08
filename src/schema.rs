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
        implemented: false,
        desc: "Search the web and return titles, URLs and snippets.",
        params: &[
            pos("query", "Search query"),
            dflt("limit", Kind::Int, "10", "Number of results"),
            opt("site", Kind::Str, "Restrict to one domain, e.g. europa.eu"),
            opt("days", Kind::Int, "Only results from the last N days"),
        ],
    },
    Spec {
        name: "extract",
        path: &["extract"],
        implemented: false,
        desc: "Extract links, tables, numbers, dates or emails from a stored doc.",
        params: &[
            pos("doc", "Doc handle, e.g. doc:3"),
            choice("what", "links", &["links", "tables", "numbers", "dates", "emails"], "What to extract"),
            opt("grep", Kind::Str, "Keep only items containing this keyword"),
        ],
    },
    Spec {
        name: "table",
        path: &["table"],
        implemented: false,
        desc: "Filter, sort and total a CSV file or a table from a doc.",
        params: &[
            pos("source", "CSV file path or doc handle"),
            opt("filter", Kind::Str, "Row filter like price<100 or vendor=Dell"),
            opt("sort", Kind::Str, "Column to sort by"),
            flag("desc", "Sort descending"),
            opt("sum", Kind::Str, "Column to total"),
            opt("cols", Kind::Str, "Comma-separated columns to keep"),
            dflt("limit", Kind::Int, "50", "Maximum rows"),
        ],
    },
    Spec {
        name: "quote",
        path: &["quote"],
        implemented: false,
        desc: "Get a stock quote and recent price history for a ticker.",
        params: &[
            pos("symbol", "Ticker, e.g. AAPL or 2330.TW"),
            choice("range", "1d", &["1d", "5d", "1mo", "6mo", "1y", "5y"], "History range"),
        ],
    },
    Spec {
        name: "market",
        path: &["market"],
        implemented: false,
        desc: "Get prediction-market odds (e.g. Polymarket) for a topic.",
        params: &[
            pos("query", "Topic or keywords"),
            dflt("limit", Kind::Int, "10", "Number of markets"),
            flag("closed", "Include closed markets"),
        ],
    },
    Spec {
        name: "report",
        path: &["report"],
        implemented: false,
        desc: "Compile saved notes into a Markdown report with sources. Without apply=true it only previews.",
        params: &[
            pos("title", "Report title"),
            opt("tag", Kind::Str, "Only use notes with this tag"),
            opt("out", Kind::Str, "Output file path"),
            flag("apply", "Actually write the file"),
        ],
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
        };
        let mut prop = json!({ "type": ty, "description": p.desc });
        if !p.choices.is_empty() {
            prop["enum"] = json!(p.choices);
        }
        if let Some(d) = p.default {
            prop["default"] = match p.kind {
                Kind::Int => json!(d.parse::<i64>().unwrap_or(0)),
                Kind::Bool => json!(d == "true"),
                Kind::Str => json!(d),
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

    /// Every leaf subcommand except schema/call has a spec whose params match
    /// the clap args exactly (names, positional-ness, defaults, choices).
    #[test]
    fn specs_match_clap_definitions() {
        let root = Cli::command();
        let mut leaves = Vec::new();
        for sc in root.get_subcommands() {
            let name = sc.get_name().to_string();
            if name == "schema" || name == "call" {
                continue;
            }
            if sc.has_subcommands() {
                for sub in sc.get_subcommands() {
                    leaves.push((format!("{name}_{}", sub.get_name()), sub.clone()));
                }
            } else {
                leaves.push((name, sc.clone()));
            }
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
