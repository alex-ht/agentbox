//! agentbox: a busybox-style toolkit for small language-model agents.

mod cli;
mod cmd;
mod config;
mod dom;
mod envelope;
mod extract;
mod markdown;
mod render;
mod report;
mod schema;
mod state;
mod table;
#[cfg(test)]
mod testutil;

use clap::Parser;
use cli::{Cli, Cmd, ConfigCmd, FileCmd, Format, NoteCmd, ReportCmd, TableCmd, TemplateCmd};
use envelope::{envelope, AppError, CmdResult};
use state::Store;
use std::io::{Read, Write};
use std::process::ExitCode;

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    // Detect --format early so even argument errors honor it.
    let format = if argv.windows(2).any(|w| w[0] == "--format" && w[1] == "md")
        || argv.iter().any(|a| a == "--format=md")
    {
        Format::Md
    } else {
        Format::Json
    };
    let cli = match Cli::try_parse_from(&argv) {
        Ok(cli) => cli,
        Err(e) => return clap_error(e, format),
    };
    let store = Store::new(state::default_home());
    let res = dispatch(cli.cmd, &store);
    emit(&res, cli.format)
}

/// Print help/version as plain text; turn real parse errors into an envelope.
fn clap_error(e: clap::Error, format: Format) -> ExitCode {
    use clap::error::ErrorKind;
    match e.kind() {
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayVersion
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            let _ = e.print();
            ExitCode::SUCCESS
        }
        _ => {
            let rendered = e.render().to_string();
            let message = clap_message(&rendered);
            let usage = rendered
                .lines()
                .find(|l| l.trim_start().starts_with("Usage:"))
                .map(str::trim);
            let hint = match usage {
                Some(u) => format!("{u}. Run with --help for examples."),
                None => "Run `agentbox --help` to list subcommands, or `agentbox <subcommand> --help` for examples.".into(),
            };
            let res: CmdResult = Err(AppError::new("bad_args", message, hint));
            emit(&res, format);
            ExitCode::from(2)
        }
    }
}

/// First line of a clap error, plus the indented detail lines when the
/// first line ends with ':' (e.g. the list of missing arguments).
fn clap_message(rendered: &str) -> String {
    let mut lines = rendered.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines
        .next()
        .unwrap_or("invalid arguments")
        .trim_start_matches("error: ")
        .to_string();
    if first.ends_with(':') {
        let details: Vec<&str> = lines.take_while(|l| !l.starts_with("Usage:")).collect();
        format!("{first} {}", details.join(", "))
    } else {
        first
    }
}

fn emit(res: &CmdResult, format: Format) -> ExitCode {
    let env = envelope(res);
    let text = match format {
        Format::Json => serde_json::to_string(&env).expect("envelope serializes"),
        Format::Md => render::render_md(&env).trim_end().to_string(),
        Format::Csv => match res {
            Ok(out) => table::envelope_csv(&out.data)
                .map(|c| c.trim_end().to_string())
                .unwrap_or_else(|| serde_json::to_string(&env).expect("envelope serializes")),
            Err(_) => serde_json::to_string(&env).expect("envelope serializes"),
        },
    };
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{text}");
    if res.is_ok() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn dispatch(cmd: Cmd, store: &Store) -> CmdResult {
    match cmd {
        Cmd::Fetch { url, timeout } => cmd::fetch::run(store, &url, timeout),
        Cmd::Read {
            doc,
            section,
            grep,
            max_chars,
            offset,
        } => cmd::read::run(
            store,
            &cmd::read::ReadArgs {
                doc,
                section,
                grep,
                max_chars,
                offset,
            },
        ),
        Cmd::Calc { expr } => cmd::calc::run(&expr.join(" ")),
        Cmd::Now { tz } => cmd::now::run(tz.as_deref()),
        Cmd::File(FileCmd::Read {
            path,
            lines,
            max_chars,
        }) => cmd::file::read(&path, lines.as_deref(), max_chars),
        Cmd::File(FileCmd::Write {
            path,
            content,
            apply,
        }) => cmd::file::write(&path, &content, apply),
        Cmd::File(FileCmd::Replace {
            path,
            find,
            replace,
            all,
            apply,
        }) => cmd::file::replace(&path, &find, &replace, all, apply),
        Cmd::Note(NoteCmd::Add { text, source, tag }) => {
            cmd::note::add(store, &text.join(" "), source.as_deref(), tag.as_deref())
        }
        Cmd::Note(NoteCmd::List { tag, grep, limit }) => {
            cmd::note::list(store, tag.as_deref(), grep.as_deref(), limit)
        }
        Cmd::Search {
            query,
            max_results,
            site,
            exclude_site,
            days,
            time,
            news,
            deep,
            answer,
            save,
            backend,
        } => {
            let key = config::resolve(store, "tavily.api_key", config::env_for("tavily.api_key"))
                .map(|(k, _)| k);
            let backend = backend
                .or_else(|| config::read_value(&config::path(store), "search.backend"))
                .unwrap_or_else(|| "auto".into());
            let args = cmd::search::SearchArgs {
                query,
                max_results,
                sites: site,
                exclude_sites: exclude_site,
                days,
                time,
                news,
                deep,
                answer,
                save,
                backend,
            };
            cmd::search::run(
                store,
                &args,
                key.as_deref(),
                &cmd::search::Endpoints::from_env(),
            )
        }
        Cmd::Config(ConfigCmd::Set { key, value }) => config::set(store, &key, value.as_deref()),
        Cmd::Config(ConfigCmd::Get { key }) => config::get(store, key.as_deref(), &config::env_for),
        Cmd::Config(ConfigCmd::Unset { key }) => config::unset(store, &key),
        Cmd::Config(ConfigCmd::Path) => config::show_path(store),
        Cmd::Extract {
            sources,
            kind,
            from,
            section,
            grep,
            limit,
            site,
            save_table,
        } => extract::run(
            store,
            &extract::ExtractArgs {
                sources,
                from,
                kind,
                section,
                grep,
                limit,
                site,
                save_table,
            },
        ),
        Cmd::Table(TableCmd::Show { source, limit }) => table::run_show(store, &source, limit),
        Cmd::Table(TableCmd::Query {
            source,
            select,
            wheres,
            sort,
            limit,
            group_by,
            agg,
            save,
        }) => table::run_query(
            store,
            &table::QueryArgs {
                source,
                query: table::query::Query {
                    select,
                    wheres,
                    sorts: sort,
                    group_by,
                    agg,
                },
                limit,
                save,
            },
        ),
        Cmd::Table(TableCmd::Import { file }) => table::run_import(store, &file),
        Cmd::Table(TableCmd::Export { source, out, apply }) => {
            table::run_export(store, &source, &out, apply)
        }
        Cmd::Quote { .. } => cmd::stubs::not_implemented("quote"),
        Cmd::Market { .. } => cmd::stubs::not_implemented("market"),
        Cmd::Report(ReportCmd::Build {
            title,
            template,
            n,
            columns,
            tag,
            out,
            apply,
        }) => report::run_build(
            store,
            &report::BuildArgs {
                title: &title,
                template: &template,
                overrides: report::template::Overrides { n, columns },
                tag: tag.as_deref(),
                out: out.as_deref(),
                apply,
            },
        ),
        Cmd::Report(ReportCmd::Check {
            file,
            template,
            n,
            columns,
        }) => report::run_check(
            store,
            &file,
            &template,
            &report::template::Overrides { n, columns },
        ),
        Cmd::Report(ReportCmd::Templates) => report::template::list(store),
        Cmd::Report(ReportCmd::Template(TemplateCmd::Show { name })) => {
            report::template::show(store, &name)
        }
        Cmd::Schema { implemented_only } => schema::run_schema(implemented_only),
        Cmd::Call { name, args } => call(&name, &args, store),
    }
}

/// `call <name> <json>`: map a function-tool call onto the regular CLI.
fn call(name: &str, raw: &str, store: &Store) -> CmdResult {
    let raw = if raw == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s).map_err(|e| {
            AppError::new(
                "bad_args",
                format!("reading stdin: {e}"),
                "Pipe a JSON object into stdin.",
            )
        })?;
        s
    } else {
        raw.to_string()
    };
    let args: serde_json::Value =
        serde_json::from_str(if raw.trim().is_empty() { "{}" } else { &raw }).map_err(|e| {
            AppError::new(
                "bad_json",
                format!("arguments are not valid JSON: {e}"),
                r#"Pass a JSON object, e.g. {"url":"https://example.com"}."#,
            )
        })?;
    let argv = schema::to_argv(name, &args)?;
    let full = std::iter::once("agentbox".to_string()).chain(argv);
    let cli = Cli::try_parse_from(full).map_err(|e| {
        AppError::new(
            "bad_args",
            clap_message(&e.render().to_string()),
            "Check the argument values against `agentbox schema`.",
        )
    })?;
    if matches!(
        cli.cmd,
        Cmd::Call { .. } | Cmd::Schema { .. } | Cmd::Config(_)
    ) {
        return Err(AppError::new(
            "bad_args",
            "call cannot run call, schema or config",
            "Call a regular tool name.",
        ));
    }
    dispatch(cli.cmd, store)
}
