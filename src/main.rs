use clap::{Args as ClapArgs, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "acc",
    version,
    about = "plaintext double-entry accounting command line tool",
    disable_version_flag = true
)]
struct Args {
    /// Print version and exit. Lower-case `-v` — upper-case `-V` is taken
    /// by `--unrealized` (reusing the `-V` letter ledger spends on market
    /// valuation, here for acc's opt-in unrealized revaluation).
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version: Option<bool>,

    /// Ledger file or directory to load. Directories are walked
    /// recursively. May be given multiple times; order is preserved
    /// and matters for transactions with identical dates. Pre-parsed
    /// out of argv before clap sees it, so it works anywhere on the
    /// command line — `acc -f CONFIG bal -f JOURNAL` collects both.
    #[arg(short = 'f', long = "file", value_name = "PATH")]
    paths: Vec<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

/// Filter / sort / conversion flags shared by every report-style
/// command (`balance`, `register`, `print`, `accounts`, `codes`,
/// `commodities`, `navigate`). Flattened into each variant via
/// `#[command(flatten)]` so `acc format --help` and `acc update
/// --help` stay uncluttered.
#[derive(ClapArgs, Clone, Debug)]
struct ReportArgs {
    /// Include only transactions on or after this date (YYYY-MM-DD)
    #[arg(long = "begin", short = 'b', conflicts_with = "periods")]
    begin: Option<String>,

    /// Include only transactions before this date (YYYY-MM-DD)
    #[arg(long = "end", short = 'e', conflicts_with = "periods")]
    end: Option<String>,

    /// Include only transactions in this period. Accepts a year
    /// (YYYY), a month (YYYY-MM), or a single day (YYYY-MM-DD).
    /// Shortcut for setting `--begin` and `--end` together. Repeat
    /// `-p` to include multiple discrete periods — each period works
    /// independently and a transaction is kept if it matches any.
    #[arg(long = "period", short = 'p', value_name = "PERIOD")]
    periods: Vec<String>,

    /// Include transactions dated after today. Hidden by default so
    /// forward-dated recurring entries (rent, subscriptions) don't
    /// clutter "what has happened" reports.
    #[arg(long)]
    future: bool,

    /// Show real postings only — drop every virtual posting
    /// (paren-virtual `(account)` and bracket-virtual `[account]`)
    /// from the output. The realizer, lotter and translator inject
    /// *real* postings (slippage gain/loss, capital gain/loss, currency
    /// translation adjustment), so `-R` keeps those; it only removes
    /// the `(…)`/`[…]` virtual postings written in the source journal.
    #[arg(short = 'R', long = "real")]
    real: bool,

    /// Related postings. With a pattern filter, show the *other*
    /// postings of the matched transactions — the counter-parties —
    /// instead of the matched postings themselves. `acc reg ^expenses:cta
    /// -r` shows which accounts balance against expenses:cta in each
    /// transaction.
    #[arg(short = 'r', long = "related")]
    related: bool,

    /// Show every posting of a matched transaction — both the matched
    /// posting and its counter-parties — instead of just the matched
    /// posting (default) or just the counter-parties (`-r`). A pattern
    /// then picks *which transactions* to show in full, not which lines
    /// of them.
    #[arg(long = "related-all")]
    related_all: bool,

    /// Sort by field: date, amount, account, description. Prefix with - for reverse. (default: date)
    #[arg(short = 'S', long = "sort", default_value = "date")]
    sort: Vec<String>,

    /// Convert all amounts into this commodity using exchange rates.
    /// Mirrors ledger's `-X`. Each posting is valued at the exchange
    /// rate on its own transaction date (historical valuation).
    #[arg(short = 'X', long = "exchange", value_name = "COMMODITY")]
    exchange: Option<String>,

    /// Show unrealized revaluation: revalue open foreign-currency balances at the
    /// latest available exchange rate, instead of the historical
    /// per-posting valuation. Revalues every open foreign balance (scope
    /// with a filter); off by default — the default stays historical
    /// (realized only). Only meaningful together with `-X`, and requires
    /// `holding gain` / `holding loss` accounts to be declared.
    #[arg(short = 'V', long = "unrealized")]
    unrealized: bool,

    /// Keep only transactions whose balance-contributing postings use
    /// at least N distinct commodities (paren-virtual slippage labels are
    /// ignored). Counts native commodities — applied before `-X`
    /// conversion. `--commodities 2` finds every currency-mixing
    /// transaction; `3` those mixing at least three.
    #[arg(long = "commodities", value_name = "N", conflicts_with = "mixed")]
    commodities: Option<usize>,

    /// Alias for `--commodities 2`: keep only transactions that mix at
    /// least two commodities.
    #[arg(long = "mixed")]
    mixed: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Show account balances
    #[command(visible_alias = "bal")]
    Balance {
        #[command(flatten)]
        filter: ReportArgs,
        /// Show as a flat list
        #[arg(long, conflicts_with = "tree")]
        flat: bool,
        /// Show as an indented tree (default)
        #[arg(long)]
        tree: bool,
        /// Show accounts with zero balance
        #[arg(short = 'E', long)]
        empty: bool,
        /// Filter by account name pattern
        pattern: Vec<String>,
    },
    /// Show transaction register
    #[command(visible_alias = "reg")]
    Register {
        #[command(flatten)]
        filter: ReportArgs,
        /// Filter by account name pattern
        pattern: Vec<String>,
    },
    /// Print formatted transactions
    Print {
        #[command(flatten)]
        filter: ReportArgs,
        /// Show raw data without computed amounts
        #[arg(long)]
        raw: bool,
        /// Filter by account name pattern
        pattern: Vec<String>,
    },
    /// List all accounts
    Accounts {
        #[command(flatten)]
        filter: ReportArgs,
        /// Show as flat list (default)
        #[arg(long)]
        flat: bool,
        /// Show as tree
        #[arg(long)]
        tree: bool,
        /// Filter by account name pattern
        pattern: Vec<String>,
    },
    /// List all transaction codes
    Codes {
        #[command(flatten)]
        filter: ReportArgs,
        /// Filter by account name pattern
        pattern: Vec<String>,
    },
    /// List all commodities
    Commodities {
        #[command(flatten)]
        filter: ReportArgs,
        /// Also show the first date the commodity was used
        #[arg(long)]
        date: bool,
        /// Filter by account name pattern
        pattern: Vec<String>,
    },
    /// Interactive account navigater
    #[command(visible_aliases = ["nav", "ui"])]
    Navigate {
        #[command(flatten)]
        filter: ReportArgs,
        /// Show accounts with zero balance
        #[arg(short = 'E', long)]
        empty: bool,
        /// Filter by account name pattern
        pattern: Vec<String>,
    },
    /// Run consistency checks over the journal
    Check,
    /// Reformat a ledger journal: account column left-aligned,
    /// amount column right-aligned.
    ///
    /// Everything after the amount (`@` cost, `{…}` lot,
    /// `= assertion`, `; comment`) is passed through 1:1 from
    /// source — expressions like `(USD 1200/12)` are never
    /// re-evaluated, so no precision drift. Commodity and number
    /// are glued together (`USD -100` → `USD-100`).
    ///
    /// The input is fully validated first (parse, resolve, book — the
    /// same checks `acc reg` runs); a structural error such as an
    /// unbalanced transaction or a single-space account/amount aborts
    /// the run with nothing written (all-or-nothing). Transactions keep
    /// their source order by default (`--sort` to date-sort them).
    /// Writes atomically. Pass `-` as path to pipe via
    /// stdin/stdout (`:%!acc format -` in vim).
    #[command(arg_required_else_help = true)]
    Format {
        /// Stably date-sort transactions. Off by default — source order
        /// is preserved unless this flag is given.
        #[arg(long = "sort")]
        sort: bool,
        /// Files or directories to format. Directories are walked
        /// recursively for journal files (`.ledger` only). Files named
        /// explicitly are formatted regardless of extension. `-` reads
        /// from stdin, writes to stdout.
        paths: Vec<String>,
    },
    /// Compare two ledger files or directory trees. Whitespace
    /// differences (indent, column alignment) are ignored — only
    /// actual character differences are shown, like `diff -w`. Output
    /// follows `git diff` conventions (`--- / +++ / @@`).
    ///
    /// Two modes:
    ///
    /// - Explicit: `acc diff OLD NEW` — both paths given directly.
    /// - Snapshot: `acc diff --snapshot SNAP [PATH...]` — acc finds
    ///   the corresponding path(s) inside `SNAP` via suffix match, so
    ///   you only give the working file and the snapshot root once.
    ///   `PATH` defaults to the current directory.
    ///
    /// Exits 1 when differences are found, 0 when identical.
    #[command(arg_required_else_help = true)]
    Diff {
        /// Snapshot root directory. When set, acc locates the matching
        /// path inside this tree by longest-suffix match against each
        /// positional PATH. The positional args become working-side
        /// paths only; the snapshot-side paths are derived.
        #[arg(long = "snapshot", value_name = "DIR")]
        snapshot: Option<String>,

        /// Paths. Without `--snapshot`: exactly two paths (OLD NEW).
        /// With `--snapshot`: one or more working paths; defaults to
        /// the current directory if none given.
        paths: Vec<String>,
    },
    /// Close the open balance of a pass-through account.
    ///
    /// Sweep pairs equal-and-opposite amounts on ACCOUNT across the whole
    /// account (per commodity, over all dates) — like reading `reg
    /// ACCOUNT` — and writes one offsetting entry per still-open posting,
    /// at that posting's date, to bring the account back to zero. A debit
    /// posting (> 0) books to EXPENSE:SEGMENT, a credit posting (< 0) to
    /// INCOME:SEGMENT. Each entry takes the account's last segment as its
    /// title and is cleared.
    ///
    /// Idempotent and file-agnostic: a posting whose offset already
    /// exists anywhere in the loaded journal cancels and is skipped, so
    /// re-running only closes newly-opened postings — no markers, no
    /// file-name tracking. A round-trip settled later (invoice then
    /// payment) cancels too. All four arguments are required. Output is
    /// appended to `<title>.ledger` and aligned via `acc format`.
    #[command(arg_required_else_help = true)]
    Sweep {
        /// Pass-through account to close (filter pattern, e.g. `^a:b:c$`).
        account: String,
        /// Segment appended after the income / expense account.
        segment: String,
        /// Income account — used when the remainder is a credit (< 0).
        income: String,
        /// Expense account — used when the remainder is a debit (> 0).
        expense: String,
    },
    /// Update exchange rate data (MEXC for crypto, openexchangerates for fiat).
    /// Standalone — does not read the journal.
    Update {
        /// Trading pair in BASE/QUOTE format, e.g. BTC/USDT. Repeat
        /// `--pair` to update multiple pairs. If omitted, all existing
        /// crypto files under $ACC_PRICES_DIR/crypto/ are updated.
        #[arg(long = "pair")]
        pairs: Vec<String>,
        /// Overwrite data from this date onwards (YYYY-MM-DD)
        #[arg(long = "since", conflicts_with = "date")]
        since: Option<String>,
        /// Fetch only this specific date (YYYY-MM-DD). Overrides --since.
        #[arg(long = "date")]
        date: Option<String>,
        /// Step forward by day (default). Compatible with crypto and fiat.
        #[arg(long, conflicts_with_all = ["monthly", "yearly"])]
        daily: bool,
        /// Fiat only: step forward by month (1st of each month) instead of daily
        #[arg(long, conflicts_with_all = ["daily", "yearly", "crypto", "pairs"])]
        monthly: bool,
        /// Fiat only: step forward by year (Jan 1st) instead of daily
        #[arg(long, conflicts_with_all = ["daily", "monthly", "crypto", "pairs"])]
        yearly: bool,
        /// Fiat only: skip dates whose file already exists (no API call, no overwrite)
        #[arg(long, conflicts_with_all = ["crypto", "pairs"])]
        skip: bool,
        /// Update crypto only (default: both crypto and fiat)
        #[arg(long)]
        crypto: bool,
        /// Update fiat only (default: both crypto and fiat)
        #[arg(long)]
        fiat: bool,
    },
}

impl Command {
    /// Patterns the user supplied for this command, or an empty slice
    /// for commands without a pattern argument. The match is
    /// exhaustive — adding a new `Command` variant is a compile error
    /// here until the new variant is classified.
    fn patterns(&self) -> &[String] {
        match self {
            Self::Balance { pattern, .. }
            | Self::Register { pattern, .. }
            | Self::Print { pattern, .. }
            | Self::Accounts { pattern, .. }
            | Self::Codes { pattern, .. }
            | Self::Commodities { pattern, .. }
            | Self::Navigate { pattern, .. } => pattern.as_slice(),
            Self::Update { .. }
            | Self::Check
            | Self::Format { .. }
            | Self::Diff { .. }
            | Self::Sweep { .. } => &[],
        }
    }

    /// Return the filter / convert flags for this command, or `None`
    /// for commands that don't participate in the load → filter →
    /// rebalance → sort pipeline (format, update, check, diff).
    fn filter(&self) -> Option<&ReportArgs> {
        match self {
            Self::Balance { filter, .. }
            | Self::Register { filter, .. }
            | Self::Print { filter, .. }
            | Self::Accounts { filter, .. }
            | Self::Codes { filter, .. }
            | Self::Commodities { filter, .. }
            | Self::Navigate { filter, .. } => Some(filter),
            Self::Update { .. }
            | Self::Check
            | Self::Format { .. }
            | Self::Diff { .. }
            | Self::Sweep { .. } => None,
        }
    }
}

fn main() {
    if let Err(e) = start() {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}

/// Print an error to stderr and exit with status 1. Used when a CLI
/// argument fails validation (e.g. a malformed `-p`/`-b`/`-e` value);
/// the `!` return type lets `match` arms call it in place of a value.
fn fail(msg: &str) -> ! {
    eprintln!("Error: {}", msg);
    std::process::exit(1);
}

/// Expand a `-p` period string into `(begin, end)` half-open bounds.
/// Accepts `YYYY`, `YYYY-MM`, or `YYYY-MM-DD`.
///
/// - `2024`       → `2024-01-01` .. `2025-01-01`
/// - `2024-12`    → `2024-12-01` .. `2025-01-01`
/// - `2024-12-06` → `2024-12-06` .. `2024-12-07`
fn expand_period(s: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = s.split('-').collect();
    match parts.as_slice() {
        [y] => {
            let year: i32 = y.parse().map_err(|_| format!("invalid year: `{}`", s))?;
            Ok((format!("{:04}-01-01", year), format!("{:04}-01-01", year + 1)))
        }
        [y, m] => {
            let year: i32 = y.parse().map_err(|_| format!("invalid year: `{}`", s))?;
            let month: u32 = m.parse().map_err(|_| format!("invalid month: `{}`", s))?;
            if !(1..=12).contains(&month) {
                return Err(format!("month out of range: `{}`", s));
            }
            let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
            Ok((
                format!("{:04}-{:02}-01", year, month),
                format!("{:04}-{:02}-01", ny, nm),
            ))
        }
        [_, _, _] => {
            // Single date: use it as begin, begin+1day as end.
            let date = acc::date::Date::parse(s).map_err(|e| e.to_string())?;
            let next = acc::date::Date::from_days(date.days() + 1);
            Ok((date.to_string(), next.to_string()))
        }
        _ => Err(format!("invalid period: `{}` (expected YYYY, YYYY-MM, or YYYY-MM-DD)", s)),
    }
}

fn collect_ledger_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                collect_ledger_files(&path, out);
            } else if path.is_file() && acc::is_journal_file(&path) {
                out.push(path);
            }
        }
    }
}

/// Number of distinct commodities used by a transaction's
/// balance-contributing postings (real and bracket-virtual); paren-
/// virtual `(account)` postings are skipped. Drives `--commodities N`
/// / `--mixed`. Injected fx/capital/CTA postings are real and counted,
/// but only ever land on transactions that already mix commodities, so
/// the mix-detection threshold is unaffected.
fn distinct_commodities(tx: &acc::parser::transaction::Transaction) -> usize {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for lp in &tx.postings {
        if lp.value.is_virtual && !lp.value.balanced {
            continue;
        }
        if let Some(a) = &lp.value.amount {
            seen.insert(a.commodity.as_str());
        }
    }
    seen.len()
}

/// Pull every `-f` / `--file PATH` pair out of the raw argv before
/// clap sees it. Works around a clap-derive limitation: global +
/// `Vec<String>` args are bound to a single subcommand level, so
/// `-f` instances split across `acc -f A bal -f B` get dropped on
/// one side. Pre-parsing here collects them all into one place and
/// hands clap an argv that no longer contains the flag.
fn split_file_args() -> (Vec<String>, Vec<String>) {
    let mut iter = std::env::args();
    let mut rest = vec![iter.next().unwrap()];
    let mut files = Vec::new();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "-f" | "--file" => {
                if let Some(v) = iter.next() {
                    files.push(v);
                }
            }
            _ => rest.push(a),
        }
    }
    (files, rest)
}

/// Run the standalone commands that bypass the report pipeline — each
/// does its own file IO and returns directly. Returns `Some(result)` when
/// `command` was one of them (and has now run), `None` for a report-style
/// command (`balance`, `register`, …) the caller must drive through
/// load → enrich → filter → rebalance → sort itself.
fn try_standalone(
    command: &Command,
    paths: &[String],
) -> Option<Result<(), acc::Error>> {
    match command {
        // Format does its own validation (parse → resolve → book) before
        // writing, all-or-nothing.
        Command::Format { sort, paths } => Some(acc::commands::format::run(paths, *sort)),

        // Diff is a source-level file/tree comparison. Its path-count rule
        // is conditional on `--snapshot` and not expressible via clap
        // alone, so it is checked here with a clap-styled error (the usual
        // `error: …` + Usage block) before dispatching.
        Command::Diff { snapshot, paths } => {
            if snapshot.is_none() && paths.len() != 2 {
                use clap::CommandFactory;
                let mut cmd = Args::command();
                cmd.find_subcommand_mut("diff")
                    .unwrap()
                    .error(
                        clap::error::ErrorKind::WrongNumberOfValues,
                        format!(
                            "expected 2 paths (OLD NEW) without --snapshot, got {}",
                            paths.len()
                        ),
                    )
                    .exit();
            }
            Some(acc::commands::diff::run(snapshot.as_deref(), paths))
        }

        // Update fetches exchange rates; it does not read the journal.
        Command::Update {
            pairs,
            since,
            date,
            monthly,
            yearly,
            skip,
            crypto,
            fiat,
            ..
        } => {
            let flags = if *crypto || *fiat {
                acc::commands::update::UpdateFlags { crypto: *crypto, fiat: *fiat }
            } else if !pairs.is_empty() {
                acc::commands::update::UpdateFlags { crypto: true, fiat: false }
            } else {
                acc::commands::update::UpdateFlags { crypto: true, fiat: true }
            };
            let cadence = if *yearly {
                acc::commands::update::Cadence::Yearly
            } else if *monthly {
                acc::commands::update::Cadence::Monthly
            } else {
                acc::commands::update::Cadence::Daily
            };
            Some(acc::commands::update::run(
                pairs,
                since.as_deref(),
                date.as_deref(),
                cadence,
                *skip,
                flags,
            ))
        }

        // Sweep loads and books the journal, scopes it to the pass-through
        // account, and writes offsetting entries to a file. It ignores the
        // report flags (-X, sort, date ranges).
        Command::Sweep { account, segment, income, expense } => {
            if paths.is_empty() {
                eprintln!("Error: No files specified. Use -f PATH.");
                std::process::exit(1);
            }
            let mut sweep_paths: Vec<std::path::PathBuf> = Vec::new();
            for input in paths {
                let path = std::path::Path::new(input);
                if path.is_dir() {
                    collect_ledger_files(path, &mut sweep_paths);
                } else {
                    sweep_paths.push(path.to_path_buf());
                }
            }
            Some(
                acc::load(&sweep_paths)
                    .map_err(|e| acc::Error::from(e.to_string()))
                    .and_then(|j| acc::commands::sweep::run(j, account, segment, income, expense)),
            )
        }

        _ => None,
    }
}

/// Resolve the effective date filter from `-b` / `-e` / `-p` plus the
/// default "hide future" cutoff (today+1, exclusive). Returns the
/// expanded period ranges (consumed by the multi-period filter) and owned
/// begin/end bounds the caller borrows for the main filter. `-b`/`-e` take
/// the *start* of their period; a single `-p` becomes a begin/end pair,
/// multiple `-p` leave begin/end empty and filter against their union.
/// Exits via `fail` on a malformed date string.
fn resolve_date_range(
    filter_args: Option<&ReportArgs>,
) -> (Vec<(String, String)>, Option<String>, Option<String>) {
    let period_ranges: Vec<(String, String)> = filter_args
        .map(|f| f.periods.as_slice())
        .unwrap_or(&[])
        .iter()
        .map(|p| match expand_period(p) {
            Ok(pair) => pair,
            Err(e) => fail(&e),
        })
        .collect();
    let (period_begin, period_end) = match period_ranges.len() {
        1 => (
            Some(period_ranges[0].0.clone()),
            Some(period_ranges[0].1.clone()),
        ),
        _ => (None, None),
    };
    let explicit_begin = filter_args
        .and_then(|f| f.begin.as_deref())
        .map(|b| match expand_period(b) {
            Ok((start, _)) => start,
            Err(e) => fail(&e),
        });
    let explicit_end = filter_args
        .and_then(|f| f.end.as_deref())
        .map(|e| match expand_period(e) {
            Ok((start, _)) => start,
            Err(e) => fail(&e),
        });
    let begin = explicit_begin.or(period_begin);
    let user_end = explicit_end.or(period_end);

    // The filter's `end` is exclusive, so `today+1` keeps everything up to
    // and including today. With an explicit `-e`/`-p`, take the earlier.
    let show_future = filter_args.map(|f| f.future).unwrap_or(false);
    let future_cap: Option<String> = (!show_future).then(|| {
        let today_str = acc::date::ms_to_date(acc::date::current_ms());
        let today = acc::date::Date::parse(&today_str)
            .expect("current_ms() returns valid YYYY-MM-DD");
        acc::date::Date::from_days(today.days() + 1).to_string()
    });
    let end = match (user_end, future_cap) {
        (Some(u), Some(cap)) => Some(if u < cap { u } else { cap }),
        (Some(u), None) => Some(u),
        (None, Some(cap)) => Some(cap),
        (None, None) => None,
    };

    (period_ranges, begin, end)
}

fn start() -> Result<(), acc::Error> {
    let (files, argv) = split_file_args();
    let mut args = Args::parse_from(argv);
    args.paths = files;

    let Some(command) = args.command else {
        use clap::CommandFactory;
        let _ = Args::command().print_help();
        return Ok(());
    };

    // Standalone commands (format, diff, update, sweep) bypass the report
    // pipeline entirely and return here.
    if let Some(result) = try_standalone(&command, &args.paths) {
        return result;
    }

    if args.paths.is_empty() {
        eprintln!("Error: No files specified. Use -f PATH.");
        std::process::exit(1);
    }

    // Filter / convert flags for report-style commands. `None` for
    // `check` (runs a minimal load → validate path). `format` and
    // `update` already returned above.
    let filter_args: Option<&ReportArgs> = command.filter();

    // Price-DB files (`$ACC_PRICES_DIR`, only under `-X`) are kept separate
    // from the user's journal files so they can be loaded *selectively*: the
    // journal is parsed first to learn which commodities the report touches,
    // and only the price pairs connecting them are then parsed. The DB is
    // ~800k directives; a report needs a handful of pairs.
    let mut price_paths: Vec<std::path::PathBuf> = Vec::new();
    if filter_args.map(|f| f.exchange.is_some()).unwrap_or(false)
        && let Ok(dir) = std::env::var("ACC_PRICES_DIR")
    {
        let path = std::path::Path::new(&dir);
        if path.is_dir() {
            collect_ledger_files(path, &mut price_paths);
        }
    }

    // Explicit `-f FILE` is honoured regardless of extension: when the
    // user names a path, the loader will try to read it and surface a
    // proper error if it's missing, instead of silently skipping it.
    // The extension filter only applies when walking a directory tree,
    // where we need to skip backups / READMEs / unrelated files.
    let mut journal_paths: Vec<std::path::PathBuf> = Vec::new();
    for input in &args.paths {
        let path = std::path::Path::new(input);
        if path.is_dir() {
            collect_ledger_files(path, &mut journal_paths);
        } else {
            journal_paths.push(path.to_path_buf());
        }
    }

    // print --raw: dump source bytes (prices first, then user files), skip
    // the full pipeline.
    if let Command::Print { raw: true, .. } = &command {
        for path in price_paths.iter().chain(journal_paths.iter()) {
            let source = if path.to_str() == Some("-") {
                use std::io::Read as _;
                let mut s = String::new();
                std::io::stdin().read_to_string(&mut s)?;
                s
            } else {
                std::fs::read_to_string(path)?
            };
            print!("{}", source);
        }
        return Ok(());
    }

    let mut journal = if price_paths.is_empty() {
        acc::load(&journal_paths)
    } else {
        acc::load_selective(
            &journal_paths,
            &price_paths,
            filter_args.and_then(|f| f.exchange.as_deref()),
        )
    }
    .map_err(|e| acc::Error::from(e.to_string()))?;

    // Resolve the `-X` target through the journal's aliases so
    // `-X EUR` and `-X €` both collapse to the canonical symbol the
    // price DB and postings are stored under. Without this, the
    // lookup silently misses when the CLI uses one spelling and the
    // journal the other.
    let exchange_target: Option<String> = filter_args
        .and_then(|f| f.exchange.as_deref())
        .map(|t| {
            journal
                .aliases
                .get(t)
                .cloned()
                .unwrap_or_else(|| t.to_string())
        });

    // Enrichment phases (expander → realizer → lotter → translator, plus
    // the `--unrealized` revaluator). These must see the whole journal —
    // the lotter tracks lots FIFO across all transactions; the translator
    // and revaluator identify pass-through / open positions by their
    // journal-wide native sum — so they run before any filtering, together
    // in `pipeline::enrich`.
    let unrealized = filter_args.map(|f| f.unrealized).unwrap_or(false);
    acc::pipeline::enrich(&mut journal, exchange_target.as_deref(), unrealized);

    // Resolve the -b / -e / -p date filter plus the default future
    // cutoff. The owned bounds are borrowed for the filter below.
    let (period_ranges, begin_owned, end_owned) = resolve_date_range(filter_args);
    let begin = begin_owned.as_deref();
    let end = end_owned.as_deref();

    // Filter phase: scope the journal to the command's pattern and
    // the global --begin / --end date range. Runs once here so every
    // commander sees an already-scoped journal.
    //
    // `print` always keeps whole matched transactions (every posting),
    // and `--related-all` (-A) requests the same for any report command,
    // unlike `reg` / `bal` which otherwise reduce to the matched
    // postings only — a pattern then picks *which entries* to show, not
    // which lines of them.
    let related = filter_args.map(|f| f.related).unwrap_or(false);
    let related_all = filter_args.map(|f| f.related_all).unwrap_or(false);
    let whole_transactions = related_all || matches!(command, Command::Print { .. });
    let mut journal = acc::filter::filter(
        journal,
        command.patterns(),
        begin,
        end,
        related,
        whole_transactions,
    );

    // Multiple `-p`: keep transactions whose date falls within any
    // of the supplied periods. Half-open `[begin, end)` per period.
    if period_ranges.len() > 1 {
        let parsed: Vec<(acc::date::Date, acc::date::Date)> = period_ranges
            .iter()
            .filter_map(|(b, e)| {
                let b = acc::date::Date::parse(b).ok()?;
                let e = acc::date::Date::parse(e).ok()?;
                Some((b, e))
            })
            .collect();
        journal.transactions.retain(|lt| {
            parsed.iter().any(|(b, e)| lt.value.date >= *b && lt.value.date < *e)
        });
    }

    // `--commodities N` / `--mixed`: keep only transactions whose
    // balance-contributing postings span at least N distinct (native)
    // commodities. Runs before rebalance so it sees the original
    // commodities, not the single `-X` target.
    let min_commodities: Option<usize> = filter_args.and_then(|f| {
        if f.mixed { Some(2) } else { f.commodities }
    });
    if let Some(min) = min_commodities {
        journal
            .transactions
            .retain(|lt| distinct_commodities(&lt.value) >= min);
    }

    // Rebalance phase: convert posting amounts into the -X target at
    // each posting's own transaction-date rate (historical valuation).
    if let Some(target) = exchange_target.as_deref() {
        acc::rebalancer::rebalance(&mut journal.transactions, target, &journal.prices);
    }

    // `-R` / `--real`: drop every virtual posting from the output.
    // Realizer/lotter/translator injections (slippage gain/loss, capital
    // gain/loss, CTA) are real postings, so they survive `-R`; this
    // only removes the `(…)`/`[…]` virtual postings from the source.
    let real = filter_args.map(|f| f.real).unwrap_or(false);
    if real {
        for lt in &mut journal.transactions {
            lt.value.postings.retain(|lp| !lp.value.is_virtual);
        }
        journal.transactions.retain(|lt| !lt.value.postings.is_empty());
    }

    // Sort phase: user-controlled ordering applied after rebalance.
    // The booker has already validated assertions in natural date
    // order, so this is pure presentation.
    let default_sort = [String::from("date")];
    let sort_keys: &[String] = filter_args
        .map(|f| f.sort.as_slice())
        .unwrap_or(&default_sort);
    acc::sorter::sort(&mut journal.transactions, sort_keys);

    match command {
        Command::Balance { flat, empty, .. } => {
            acc::commands::balance::run(&journal, !flat, empty);
        }
        Command::Register { .. } => acc::commands::register::run(&journal),
        Command::Print { raw: false, .. } => {
            // `print -X` rounds to display precision and re-balances each
            // transaction so the output is a valid, reloadable journal.
            // This is print-specific (bal/reg keep full precision), so it
            // lives here in the print arm, not in the generic pipeline.
            // Order vs. sort is irrelevant — rounding is per-transaction.
            if let Some(target) = exchange_target.as_deref() {
                acc::rebalancer::round_for_print(
                    &mut journal.transactions,
                    target,
                    &journal.precisions,
                );
            }
            acc::commands::print::run(&journal);
        }
        Command::Accounts { tree, .. } => acc::commands::accounts::run(&journal, tree),
        Command::Codes { .. } => acc::commands::codes::run(&journal),
        Command::Commodities { date, .. } => acc::commands::commodities::run(&journal, date),
        Command::Navigate { empty, .. } => {
            if let Err(e) = acc::commands::navigate::run(&journal, empty) {
                eprintln!("navigate: {}", e);
            }
        }
        Command::Check => acc::commands::checker::run(&journal),
        _ => eprintln!("internal error: unexpected command reached match arm"),
    }
    Ok(())
}
