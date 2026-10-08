//! Command-line definitions (clap derive). Keep help text short: one line
//! per flag and 1-2 examples per subcommand.

use clap::{Parser, Subcommand, ValueEnum};

pub const TOP_HELP: &str = "\
Output is one JSON line: {\"ok\":true,\"data\":...,\"hint\":...} or
{\"ok\":false,\"error\":{\"code\",\"message\"},\"hint\":...}. Use --format md for Markdown.
State (docs, notes, tables) lives in $AGENTBOX_HOME (default ~/.agentbox).

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

    /// Stock, index, FX and crypto quotes, price history, ticker lookup
    #[command(subcommand)]
    Quote(QuoteCmd),

    /// Polymarket prediction markets: search, trending, details, odds history
    #[command(subcommand)]
    Market(MarketCmd),

    /// Build a report skeleton from a template, or check a report against one
    #[command(subcommand)]
    Report(ReportCmd),

    /// Settings such as the Tavily and Stooq API keys (stored in the state dir)
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

#[derive(Debug, Subcommand)]
pub enum QuoteCmd {
    /// Latest price, change, 52-week range for one or more symbols
    #[command(
        after_help = "Symbols: NVDA, 2330.TW (TWSE), 6488.TWO (TPEx), 0700.HK, ^GSPC, ^TWII, USDTWD=X, BTC-USD.\nData may be delayed ~15 min; for research, not trading.\n\nExamples:\n  agentbox quote get NVDA 2330.TW ^TWII\n  agentbox quote get USDTWD=X,BTC-USD --format md"
    )]
    Get {
        /// One or more symbols (space- or comma-separated)
        #[arg(required = true)]
        symbols: Vec<String>,
        /// Data source; auto = Yahoo, then Stooq if Yahoo is blocked
        #[arg(long, default_value = "auto", value_parser = ["auto", "yahoo", "stooq"])]
        backend: String,
    },
    /// Daily/weekly/monthly OHLCV rows plus a summary (start, end, % change, high, low)
    #[command(
        after_help = "Examples:\n  agentbox quote history NVDA --range 6mo\n  agentbox quote history 2330.TW --range 1y --interval 1wk --save"
    )]
    History {
        /// Symbol, e.g. NVDA or 2330.TW
        symbol: String,
        /// Time window
        #[arg(long, default_value = "1mo", value_parser = ["5d", "1mo", "3mo", "6mo", "ytd", "1y", "5y", "max"])]
        range: String,
        /// Bar size (default: 1d; 1wk for 5y; 1mo for max)
        #[arg(long, value_parser = ["1d", "1wk", "1mo"])]
        interval: Option<String>,
        /// Data source; Stooq history needs STOOQ_API_KEY
        #[arg(long, default_value = "auto", value_parser = ["auto", "yahoo", "stooq"])]
        backend: String,
        /// Save all rows as a tbl:N for `table query`
        #[arg(long)]
        save: bool,
    },
    /// Find ticker symbols by company name (Chinese names of major TW/HK stocks work too)
    #[command(
        after_help = "Examples:\n  agentbox quote search \"Taiwan Semiconductor\"\n  agentbox quote search 台積電"
    )]
    Search {
        /// Company name, keyword or ticker
        query: String,
        /// Maximum results
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
}

#[derive(Debug, Subcommand)]
pub enum MarketCmd {
    /// Find events by keyword (title, question, description); odds as percentages
    #[command(
        after_help = "Default: active events, sorted by total volume. --closed = resolved only; --active --closed = both.\n\nExamples:\n  agentbox market search \"fed rate\"\n  agentbox market search election --tag politics --limit 15 --save-table\n  agentbox market search bitcoin --sort end"
    )]
    Search {
        /// Keywords; every word must appear in the event
        query: String,
        /// Maximum events returned
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Only active events (the default)
        #[arg(long)]
        active: bool,
        /// Only closed (resolved) events; with --active, both
        #[arg(long)]
        closed: bool,
        /// Order of results
        #[arg(long, default_value = "volume", value_parser = ["volume", "liquidity", "end", "newest"])]
        sort: String,
        /// Only events with this tag slug, e.g. politics, crypto, sports
        #[arg(long)]
        tag: Option<String>,
        /// Also save every market of the results as a tbl:N
        #[arg(long)]
        save_table: bool,
    },
    /// One event (all its markets and outcomes) or one market, with rules
    #[command(
        after_help = "Accepts an event or market slug, a numeric id, or a polymarket.com URL.\n\nExamples:\n  agentbox market get balance-of-power-2026-midterms\n  agentbox market get https://polymarket.com/event/presidential-election-winner-2028"
    )]
    Get {
        /// Event/market slug, id, or polymarket.com URL
        id: String,
        /// Maximum markets listed for multi-market events
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Most active events right now (by 24h volume)
    #[command(
        after_help = "Examples:\n  agentbox market trending\n  agentbox market trending --tag crypto --limit 5 --save-table"
    )]
    Trending {
        /// Maximum events returned
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Only events with this tag slug, e.g. politics, crypto, sports
        #[arg(long)]
        tag: Option<String>,
        /// Also save every open market of the results as a tbl:N
        #[arg(long)]
        save_table: bool,
    },
    /// Probability over time for one market (first outcome, usually Yes)
    #[command(
        after_help = "Pass a market slug or id (multi-market events list their markets in `market get`).\n\nExamples:\n  agentbox market history 2026-balance-of-power-d-senate-d-house-949 --interval 1m\n  agentbox market history 559652 --interval max --save"
    )]
    History {
        /// Market slug or id (or a single-market event slug / URL)
        id: String,
        /// Window: 1d hourly points, 1w 6-hourly, 1m daily, max daily
        #[arg(long, default_value = "1w", value_parser = ["1d", "1w", "1m", "max"])]
        interval: String,
        /// Save all points as a tbl:N for `table query`
        #[arg(long)]
        save: bool,
    },
}
