//! Command-line definitions (clap derive). Keep help text short: one line
//! per flag and 1-2 examples per subcommand.

use clap::{Parser, Subcommand, ValueEnum};

pub const TOP_HELP: &str = "\
Output is one JSON line: {\"ok\":true,\"data\":...,\"hint\":...} or
{\"ok\":false,\"error\":{\"code\",\"message\"},\"hint\":...}. Use --format md for Markdown.
State (docs, notes) lives in $AGENTBOX_HOME (default ~/.agentbox).

Examples:
  agentbox fetch https://example.com
  agentbox read doc:1 --grep price";

#[derive(Debug, Parser)]
#[command(
    name = "agentbox",
    version,
    about = "Busybox-style toolkit for small LLM agents",
    after_help = TOP_HELP,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Output format (csv applies to table output only)
    #[arg(long, global = true, value_enum, default_value_t = Format::Json)]
    pub format: Format,

    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Json,
    Md,
    /// CSV for table output (`table show/query`); other output stays JSON
    Csv,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Download a web page as Markdown; returns a doc handle and outline
    #[command(
        after_help = "Examples:\n  agentbox fetch https://example.com\n  agentbox fetch example.com/pricing --timeout 60"
    )]
    Fetch {
        /// URL to fetch (https:// is added if missing)
        url: String,
        /// Timeout in seconds
        #[arg(long, default_value_t = 30)]
        timeout: u64,
    },

    /// Read a stored doc: a section, keyword snippets, or from an offset
    #[command(
        after_help = "Examples:\n  agentbox read doc:1 --section 2\n  agentbox read doc:1 --grep \"annual revenue\""
    )]
    Read {
        /// Doc handle, e.g. doc:1
        doc: String,
        /// Section number from the outline
        #[arg(long)]
        section: Option<usize>,
        /// Return snippets around this keyword (case-insensitive)
        #[arg(long)]
        grep: Option<String>,
        /// Maximum characters to return
        #[arg(long, default_value_t = 4000)]
        max_chars: usize,
        /// Start at this character offset (from next_offset)
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },

    /// Evaluate arithmetic: + - * / % ^, sqrt, round(x,2), min, max, avg...
    #[command(
        after_help = "Examples:\n  agentbox calc \"(182.5 - 170) / 170 * 100\"\n  agentbox calc \"round(1299 * 0.85, 2)\""
    )]
    Calc {
        /// Expression (quote it, or pass it as several words)
        #[arg(required = true, num_args = 1.., allow_hyphen_values = true, trailing_var_arg = true)]
        expr: Vec<String>,
    },

    /// Current date and time, optionally in another timezone
    #[command(after_help = "Examples:\n  agentbox now\n  agentbox now --tz America/New_York")]
    Now {
        /// IANA zone (Asia/Taipei) or offset (+08:00)
        #[arg(long)]
        tz: Option<String>,
    },

    /// Read, write or edit local text files (changes need --apply)
    #[command(subcommand)]
    File(FileCmd),

    /// Scratch notes with sources, kept across calls
    #[command(subcommand)]
    Note(NoteCmd),

    /// Web search (Tavily with your API key, else keyless DuckDuckGo/Bing)
    #[command(
        after_help = "Examples:\n  agentbox search \"EU AI Act GPAI obligations\" --site europa.eu --save 2\n  agentbox search \"Nvidia earnings\" --news --time week --answer\n\nTavily key: env TAVILY_API_KEY or `agentbox config set tavily.api_key -` (never a flag)."
    )]
    Search {
        /// Search query
        query: String,
        /// Number of results (max 20)
        #[arg(long, default_value_t = 5)]
        max_results: usize,
        /// Only this domain, e.g. europa.eu (repeatable)
        #[arg(long)]
        site: Vec<String>,
        /// Exclude this domain (repeatable)
        #[arg(long)]
        exclude_site: Vec<String>,
        /// Only results from the last N days
        #[arg(long)]
        days: Option<u32>,
        /// Only results from the last day, week, month or year
        #[arg(long, value_parser = ["day", "week", "month", "year"])]
        time: Option<String>,
        /// News sources only (Tavily)
        #[arg(long)]
        news: bool,
        /// Deeper, more relevant search; costs 2 credits (Tavily)
        #[arg(long)]
        deep: bool,
        /// Include a short generated answer (Tavily)
        #[arg(long)]
        answer: bool,
        /// Also store the top N results as doc handles (max 5)
        #[arg(long, default_value_t = 0)]
        save: usize,
        /// Backend; default auto = tavily if a key is set, else ddg (bing fallback)
        #[arg(long, value_parser = ["auto", "tavily", "ddg", "bing"])]
        backend: Option<String>,
    },

    /// Pull tables, prices, dates, people, links, numbers or emails out of docs or files
    #[command(
        after_help = "Examples:\n  agentbox extract doc:3 --kind tables\n  agentbox extract doc:1 doc:2 --kind prices --save-table\n  agentbox extract --from doc:1,doc:2 --kind people --grep CEO\n  agentbox extract doc:4 --kind links --site github.com"
    )]
    Extract {
        /// Doc handles (doc:N) or file paths; several allowed
        sources: Vec<String>,
        /// What to extract
        #[arg(long, value_parser = ["tables", "prices", "dates", "people", "links", "numbers", "emails"])]
        kind: String,
        /// More sources, comma-separated, e.g. doc:1,doc:2
        #[arg(long)]
        from: Option<String>,
        /// Only this section number (from the fetch/read outline)
        #[arg(long)]
        section: Option<usize>,
        /// Keep only items whose text contains this keyword
        #[arg(long)]
        grep: Option<String>,
        /// Maximum items returned
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Links only: keep links to this domain (subdomains included)
        #[arg(long)]
        site: Option<String>,
        /// Also save all items as a table handle (tbl:N) for `table query`
        #[arg(long)]
        save_table: bool,
    },

    /// Show, query (filter/sort/group), import or export tables
    #[command(subcommand)]
    Table(TableCmd),

    /// [planned] Stock quote and price history for a ticker
    #[command(after_help = "Example:\n  agentbox quote NVDA --range 1mo")]
    Quote {
        /// Ticker symbol, e.g. AAPL or 2330.TW
        symbol: String,
        /// History range
        #[arg(long, default_value = "1d", value_parser = ["1d", "5d", "1mo", "6mo", "1y", "5y"])]
        range: String,
    },

    /// [planned] Prediction-market odds (e.g. Polymarket) for a topic
    #[command(after_help = "Example:\n  agentbox market \"fed rate cut\" --limit 5")]
    Market {
        /// Topic or keywords
        query: String,
        /// Number of markets
        #[arg(long, default_value_t = 10)]
        limit: u32,
        /// Include closed markets
        #[arg(long)]
        closed: bool,
    },

    /// Build a report skeleton from a template, or check a report against one
    #[command(subcommand)]
    Report(ReportCmd),

    /// Settings such as the Tavily API key (stored in the state dir)
    #[command(subcommand)]
    Config(ConfigCmd),

    /// Print OpenAI-style function-tool JSON schemas for all subcommands
    #[command(after_help = "Examples:\n  agentbox schema\n  agentbox schema --implemented-only")]
    Schema {
        /// Leave out planned (not yet implemented) tools
        #[arg(long)]
        implemented_only: bool,
    },

    /// Run a tool by its schema name with JSON arguments (for agent frameworks)
    #[command(
        after_help = "Examples:\n  agentbox call fetch '{\"url\":\"https://example.com\"}'\n  agentbox call file_replace '{\"path\":\"a.txt\",\"find\":\"x\",\"replace\":\"y\"}'"
    )]
    Call {
        /// Tool name from `agentbox schema`, e.g. read or note_add
        name: String,
        /// JSON object of arguments; `-` reads from stdin
        #[arg(default_value = "{}")]
        args: String,
    },
}

#[derive(Debug, Subcommand)]
pub enum FileCmd {
    /// Read a text file, optionally a line range
    #[command(
        after_help = "Examples:\n  agentbox file read notes.md\n  agentbox file read src/main.rs --lines 10:40"
    )]
    Read {
        /// File path
        path: String,
        /// Line range START:END (1-based), e.g. 10:40
        #[arg(long)]
        lines: Option<String>,
        /// Maximum characters to return
        #[arg(long, default_value_t = 20000)]
        max_chars: usize,
    },
    /// Write a whole file (shows a diff; --apply to write)
    #[command(
        after_help = "Examples:\n  agentbox file write out.md --content \"# Title\"\n  agentbox file write out.md --content \"# Title\" --apply"
    )]
    Write {
        /// File path
        path: String,
        /// New full file content
        #[arg(long, allow_hyphen_values = true)]
        content: String,
        /// Actually write the file
        #[arg(long)]
        apply: bool,
    },
    /// Replace exact text (shows a diff; --apply to write)
    #[command(
        after_help = "Examples:\n  agentbox file replace config.toml --find \"debug = true\" --replace \"debug = false\"\n  agentbox file replace a.md --find old --replace new --all --apply"
    )]
    Replace {
        /// File path
        path: String,
        /// Exact text to find (must occur once unless --all)
        #[arg(long, allow_hyphen_values = true)]
        find: String,
        /// Replacement text
        #[arg(long, allow_hyphen_values = true)]
        replace: String,
        /// Replace every occurrence
        #[arg(long)]
        all: bool,
        /// Actually write the file
        #[arg(long)]
        apply: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum NoteCmd {
    /// Save a note, ideally with its source URL
    #[command(
        after_help = "Examples:\n  agentbox note add \"NVDA closed at 132.5\" --source https://stooq.com --tag stock"
    )]
    Add {
        /// Note text (quote it, or pass several words)
        #[arg(required = true, num_args = 1..)]
        text: Vec<String>,
        /// Source URL or doc handle
        #[arg(long)]
        source: Option<String>,
        /// Tag for grouping, e.g. pricing
        #[arg(long)]
        tag: Option<String>,
    },
    /// List notes and the sources they cite
    #[command(
        after_help = "Examples:\n  agentbox note list\n  agentbox note list --tag pricing --grep enterprise"
    )]
    List {
        /// Only notes with this tag
        #[arg(long)]
        tag: Option<String>,
        /// Only notes containing this keyword
        #[arg(long)]
        grep: Option<String>,
        /// Maximum notes (latest first kept)
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigCmd {
    /// Store a setting; use `-` (or omit the value) to read it from stdin
    #[command(
        after_help = "Examples:\n  agentbox config set tavily.api_key -      (then paste the key, press Enter)\n  agentbox config set search.backend bing\n\nKeys: tavily.api_key, search.backend. Env TAVILY_API_KEY overrides the file."
    )]
    Set {
        /// Setting name, e.g. tavily.api_key
        key: String,
        /// Value; `-` or omitted reads one line from stdin
        value: Option<String>,
    },
    /// Show settings and where they come from (secrets are masked)
    #[command(
        after_help = "Examples:\n  agentbox config get\n  agentbox config get tavily.api_key"
    )]
    Get {
        /// Setting name; omit to show all
        key: Option<String>,
    },
    /// Remove a setting from the config file
    #[command(after_help = "Example:\n  agentbox config unset tavily.api_key")]
    Unset {
        /// Setting name
        key: String,
    },
    /// Print the config file path
    Path,
}

#[derive(Debug, Subcommand)]
pub enum TableCmd {
    /// Columns with inferred types (number/currency/date/text), row count, first rows
    #[command(
        after_help = "Examples:\n  agentbox table show tbl:2\n  agentbox table show prices.csv --limit 5"
    )]
    Show {
        /// Table handle (tbl:N) or a .csv/.tsv/.json/.md file
        source: String,
        /// Rows to show
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Filter, sort, group and aggregate rows (no regex or code needed)
    #[command(
        after_help = "Where: COLUMN OP VALUE, OP = != < <= > >= contains startswith. Repeat --where to AND.\nNumbers like \"$1,299/mo\" compare as numbers; dates by calendar order.\n\nExamples:\n  agentbox table query tbl:2 --where \"Price < 100\" --sort -Price --limit 5\n  agentbox table query tbl:3 --where \"currency = USD\" --sort value --select raw,value,doc\n  agentbox table query sales.csv --group-by Region --agg \"sum:Revenue,count\" --sort -sum_Revenue\n  agentbox table query tbl:2 --format csv"
    )]
    Query {
        /// Table handle (tbl:N) or a .csv/.tsv/.json/.md file
        source: String,
        /// Comma-separated columns to keep, in this order
        #[arg(long)]
        select: Option<String>,
        /// Row filter like "Price < 100" or "Plan contains pro" (repeatable, AND)
        #[arg(long = "where", id = "where")]
        wheres: Vec<String>,
        /// Sort column; prefix with - for descending, e.g. -Price (repeatable)
        #[arg(long, allow_hyphen_values = true)]
        sort: Vec<String>,
        /// Maximum rows returned (default 20)
        #[arg(long)]
        limit: Option<usize>,
        /// Group rows by this column
        #[arg(long)]
        group_by: Option<String>,
        /// Aggregates like "sum:Price,avg:Price,count" (sum avg min max count)
        #[arg(long)]
        agg: Option<String>,
        /// Save the result as a new tbl:N
        #[arg(long)]
        save: bool,
    },
    /// Import a CSV/TSV/JSON/Markdown table file as a tbl:N handle
    #[command(
        after_help = "Examples:\n  agentbox table import prices.csv\n  agentbox table import results.json"
    )]
    Import {
        /// File path (.csv, .tsv, .json array of objects, .md pipe table)
        file: String,
    },
    /// Write a table to .csv, .tsv, .md or .json (preview unless --apply)
    #[command(
        after_help = "Examples:\n  agentbox table export tbl:2 --out prices.csv\n  agentbox table export tbl:2 --out prices.md --apply"
    )]
    Export {
        /// Table handle (tbl:N) or a table file
        source: String,
        /// Output path; the extension picks the format
        #[arg(long)]
        out: String,
        /// Actually write the file
        #[arg(long)]
        apply: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ReportCmd {
    /// Draft a Markdown skeleton with exact headings, TODOs, notes and sources
    #[command(
        after_help = "Examples:\n  agentbox report build \"Top 3 Rust web frameworks\" --template top-n --n 3 --out report.md --apply\n  agentbox report build \"CRM pricing\" --template compare --columns \"Tool,Price,Free tier\" --tag crm"
    )]
    Build {
        /// Report title (becomes the H1)
        title: String,
        /// Built-in name, user template name, or path to a .toml file
        #[arg(long, default_value = "brief")]
        template: String,
        /// Number of numbered items (for templates with a repeated, numbered section)
        #[arg(long)]
        n: Option<usize>,
        /// Comma-separated table columns (templates with a table)
        #[arg(long)]
        columns: Option<String>,
        /// Only use notes with this tag
        #[arg(long)]
        tag: Option<String>,
        /// Output file path; omit to return the draft inline
        #[arg(long)]
        out: Option<String>,
        /// Actually write --out
        #[arg(long)]
        apply: bool,
    },
    /// Grade a report against a template; every issue has a concrete fix
    #[command(
        after_help = "Examples:\n  agentbox report check report.md --template top-n --n 3\n  agentbox --format md report check report.md --template brief"
    )]
    Check {
        /// Markdown file to check
        file: String,
        /// Built-in name, user template name, or path to a .toml file
        #[arg(long, default_value = "brief")]
        template: String,
        /// Required number of numbered items
        #[arg(long)]
        n: Option<usize>,
        /// Required table columns, comma-separated
        #[arg(long)]
        columns: Option<String>,
    },
    /// List built-in and user report templates
    #[command(after_help = "Example:\n  agentbox report templates")]
    Templates,
    /// Show a template's TOML
    #[command(subcommand)]
    Template(TemplateCmd),
}

#[derive(Debug, Subcommand)]
pub enum TemplateCmd {
    /// Print a template's TOML so you can copy and edit it
    #[command(after_help = "Example:\n  agentbox report template show top-n")]
    Show {
        /// Template name or path
        name: String,
    },
}
