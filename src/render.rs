//! `--format md`: render an envelope as compact, human/LLM-friendly Markdown.

use serde_json::Value;

/// Keys whose string values are code-like and should be fenced.
const FENCED_KEYS: &[(&str, &str)] = &[("diff", "diff")];

pub fn render_md(env: &Value) -> String {
    let mut out = String::new();
    let ok = env.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if ok {
        if let Some(data) = env.get("data") {
            if is_check_report(data) {
                render_checklist(data, &mut out);
            } else {
                render_value(data, 0, &mut out);
            }
        }
    } else {
        let code = env
            .pointer("/error/code")
            .and_then(Value::as_str)
            .unwrap_or("error");
        let msg = env
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("");
        out.push_str(&format!("**Error `{code}`**: {msg}\n"));
    }
    if let Some(hint) = env.get("hint").and_then(Value::as_str) {
        if !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(&format!("> hint: {hint}\n"));
    }
    out.trim_end().to_string() + "\n"
}

/// `report check` output: `{pass, score, issues:[...], stats:{...}}`.
fn is_check_report(data: &Value) -> bool {
    data.get("pass").is_some_and(Value::is_boolean)
        && data.get("issues").is_some_and(Value::is_array)
}

/// Render a `report check` result as a human-readable checklist.
fn render_checklist(data: &Value, out: &mut String) {
    let pass = data["pass"].as_bool().unwrap_or(false);
    let file = data.get("file").and_then(Value::as_str).unwrap_or("report");
    let template = data.get("template").and_then(Value::as_str).unwrap_or("?");
    let score = data.get("score").map(scalar).unwrap_or_default();
    let verdict = if pass { "PASS" } else { "FAIL" };
    out.push_str(&format!(
        "## {verdict}: `{file}` (template `{template}`), score {score}/100\n\n"
    ));
    if let Some(Value::Object(stats)) = data.get("stats") {
        let parts: Vec<String> = stats
            .iter()
            .map(|(k, v)| format!("{k} {}", scalar(v)))
            .collect();
        out.push_str(&format!("Stats: {}\n\n", parts.join(" · ")));
    }
    let issues = data["issues"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    if issues.is_empty() {
        out.push_str("- [x] No issues found.\n");
        return;
    }
    let count = |sev: &str| issues.iter().filter(|i| i["severity"] == sev).count();
    out.push_str(&format!(
        "{} error(s), {} warning(s):\n\n",
        count("error"),
        count("warn")
    ));
    for it in issues {
        let sev = it
            .get("severity")
            .and_then(Value::as_str)
            .unwrap_or("error");
        let rule = it.get("rule").and_then(Value::as_str).unwrap_or("?");
        let line = match it.get("line").and_then(Value::as_u64) {
            Some(n) => format!(" (line {n})"),
            None => String::new(),
        };
        let msg = it.get("message").and_then(Value::as_str).unwrap_or("");
        out.push_str(&format!("- [ ] **{sev}** `{rule}`{line}: {msg}\n"));
        if let Some(fix) = it
            .get("fix")
            .and_then(Value::as_str)
            .filter(|f| !f.is_empty())
        {
            if fix.starts_with("agentbox ") {
                out.push_str(&format!("  - fix: `{fix}`\n"));
            } else {
                out.push_str(&format!("  - fix: {fix}\n"));
            }
        }
    }
}

fn scalar(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn is_scalar(v: &Value) -> bool {
    !matches!(v, Value::Array(_) | Value::Object(_))
}

fn is_table(items: &[Value]) -> bool {
    !items.is_empty()
        && items.iter().all(|it| match it {
            Value::Object(m) => m
                .values()
                .all(|v| is_scalar(v) && !v.as_str().is_some_and(|s| s.contains('\n'))),
            _ => false,
        })
}

fn render_table(items: &[Value], out: &mut String) {
    let mut cols: Vec<String> = Vec::new();
    for it in items {
        if let Value::Object(m) = it {
            for k in m.keys() {
                if !cols.contains(k) {
                    cols.push(k.clone());
                }
            }
        }
    }
    out.push_str(&format!("| {} |\n", cols.join(" | ")));
    out.push_str(&format!("|{}\n", "---|".repeat(cols.len())));
    for it in items {
        let cells: Vec<String> = cols
            .iter()
            .map(|c| {
                it.get(c)
                    .map(scalar)
                    .unwrap_or_default()
                    .replace('|', "\\|")
            })
            .collect();
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
    }
}

fn render_value(v: &Value, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                match val {
                    Value::String(s)
                        if s.contains('\n') || FENCED_KEYS.iter().any(|(fk, _)| fk == k) =>
                    {
                        if let Some((_, lang)) = FENCED_KEYS.iter().find(|(fk, _)| fk == k) {
                            out.push_str(&format!(
                                "\n**{k}**:\n\n```{lang}\n{}\n```\n\n",
                                s.trim_end()
                            ));
                        } else {
                            out.push_str(&format!("\n**{k}**:\n\n{}\n\n", s.trim_end()));
                        }
                    }
                    Value::Array(items) if is_table(items) => {
                        out.push_str(&format!("\n**{k}**:\n\n"));
                        render_table(items, out);
                        out.push('\n');
                    }
                    Value::Array(items) if items.is_empty() => {
                        out.push_str(&format!("{indent}- **{k}**: (none)\n"));
                    }
                    Value::Array(_) | Value::Object(_) => {
                        out.push_str(&format!("{indent}- **{k}**:\n"));
                        render_value(val, depth + 1, out);
                    }
                    _ => out.push_str(&format!("{indent}- **{k}**: {}\n", scalar(val))),
                }
            }
        }
        Value::Array(items) => {
            if is_table(items) {
                render_table(items, out);
            } else {
                for it in items {
                    if is_scalar(it) {
                        out.push_str(&format!("{indent}- {}\n", scalar(it)));
                    } else {
                        out.push_str(&format!("{indent}-\n"));
                        render_value(it, depth + 1, out);
                    }
                }
            }
        }
        other => out.push_str(&format!("{indent}{}\n", scalar(other))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn renders_error_with_hint() {
        let env = json!({"ok":false,"error":{"code":"x","message":"bad"},"hint":"do y"});
        let md = render_md(&env);
        assert!(md.contains("**Error `x`**: bad"));
        assert!(md.contains("> hint: do y"));
    }

    #[test]
    fn renders_array_of_objects_as_table() {
        let env = json!({"ok":true,"data":{"outline":[{"section":1,"heading":"A"},{"section":2,"heading":"B|C"}]},"hint":null});
        let md = render_md(&env);
        assert!(md.contains("| section | heading |"));
        assert!(md.contains("| 2 | B\\|C |"));
    }

    #[test]
    fn check_report_renders_as_checklist() {
        let env = json!({"ok":true,"data":{"file":"r.md","template":"top-n","pass":false,"score":88,
            "issues":[{"severity":"error","rule":"heading_level","line":3,"message":"bad level",
                       "fix":"agentbox file replace \"r.md\" --find \"### 1. A\" --replace \"## 1. A\" --apply"}],
            "stats":{"words":10,"sections":2}},"hint":null});
        let md = render_md(&env);
        assert!(
            md.starts_with("## FAIL: `r.md` (template `top-n`), score 88/100"),
            "{md}"
        );
        assert!(md.contains("Stats: words 10 · sections 2"));
        assert!(md.contains("- [ ] **error** `heading_level` (line 3): bad level"));
        assert!(md.contains("  - fix: `agentbox file replace"));
        let env =
            json!({"ok":true,"data":{"pass":true,"score":100,"issues":[],"stats":{}},"hint":null});
        assert!(render_md(&env).contains("- [x] No issues found."));
    }

    #[test]
    fn multiline_strings_are_block_rendered() {
        let env = json!({"ok":true,"data":{"content":"line1\nline2","diff":"-a\n+b"},"hint":null});
        let md = render_md(&env);
        assert!(md.contains("**content**:\n\nline1\nline2"));
        assert!(md.contains("```diff\n-a\n+b\n```"));
    }
}
