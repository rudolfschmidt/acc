use clap::{Args as ClapArgs, Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "acc",
    version,
    about = "plaintext double-entry accounting command line tool"
)]
struct Args {
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
    /// from the output. Realizer and translator still compute their
    /// adjustments for correctness, but their injected postings
    /// (fx gain / fx loss / currency translation adjustment) are hidden.
    #[arg(short = 'R', long = "real")]
    real: bool,

    /// Related postings. With a pattern filter, show the *other*
    /// postings of the matched transactions — the counter-parties —
    /// instead of the matched postings themselves. `acc reg ^ex:cta
    /// -r` shows which accounts balance against ex:cta in each
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

    /// Keep only transactions whose balance-contributing postings use
    /// at least N distinct commodities (paren-virtual fx labels are
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
/// balance-contributing postings (real and bracket-virtual). Paren-
/// virtual postings — the realizer's informational fx gain/loss labels
/// — are skipped, so they never inflate the count. Drives
/// `--commodities N` / `--mixed`.
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

fn start() -> Result<(), acc::Error> {
    let (files, argv) = split_file_args();
    let mut args = Args::parse_from(argv);
    args.paths = files;

    let Some(command) = args.command else {
        use clap::CommandFactory;
        let _ = Args::command().print_help();
        return Ok(());
    };

    // Format is standalone — does its own file IO, bypasses the main
    // report pipeline. It runs the full validation (parse → resolve →
    // book) itself before writing, all-or-nothing.
    if let Command::Format { sort, paths } = &command {
        return acc::commands::format::run(paths, *sort);
    }

    // Diff is standalone — source-level file/tree comparison, no
    // downstream pipeline. Path-count validation is conditional on
    // `--snapshot` and not expressible via clap-derive alone, so the
    // check runs here with a clap-styled error so the user sees the
    // usual `error: …` + Usage block.
    if let Command::Diff { snapshot, paths } = &command {
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
        return acc::commands::diff::run(snapshot.as_deref(), paths);
    }

    // Update is standalone — does not read the journal.
    if let Command::Update {
        pairs,
        since,
        date,
        monthly,
        yearly,
        skip,
        crypto,
        fiat,
        ..
    } = &command
    {
        let flags = if *crypto || *fiat {
            acc::commands::update::UpdateFlags {
                crypto: *crypto,
                fiat: *fiat,
            }
        } else if !pairs.is_empty() {
            acc::commands::update::UpdateFlags {
                crypto: true,
                fiat: false,
            }
        } else {
            acc::commands::update::UpdateFlags {
                crypto: true,
                fiat: true,
            }
        };
        let cadence = if *yearly {
            acc::commands::update::Cadence::Yearly
        } else if *monthly {
            acc::commands::update::Cadence::Monthly
        } else {
            acc::commands::update::Cadence::Daily
        };
        return acc::commands::update::run(
            pairs,
            since.as_deref(),
            date.as_deref(),
            cadence,
            *skip,
            flags,
        );
    }

    // Sweep is standalone — it loads and books the journal, scopes it to
    // the pass-through account itself, and writes offsetting entries to a
    // file. It does not use the report flags (-X, sort, date ranges).
    if let Command::Sweep {
        account,
        segment,
        income,
        expense,
    } = &command
    {
        if args.paths.is_empty() {
            eprintln!("Error: No files specified. Use -f PATH.");
            std::process::exit(1);
        }
        let mut sweep_paths: Vec<std::path::PathBuf> = Vec::new();
        for input in &args.paths {
            let path = std::path::Path::new(input);
            if path.is_dir() {
                collect_ledger_files(path, &mut sweep_paths);
            } else {
                sweep_paths.push(path.to_path_buf());
            }
        }
        let journal = acc::load(&sweep_paths).map_err(|e| acc::Error::from(e.to_string()))?;
        return acc::commands::sweep::run(journal, account, segment, income, expense);
    }

    if args.paths.is_empty() {
        eprintln!("Error: No files specified. Use -f PATH.");
        std::process::exit(1);
    }

    // Filter / convert flags for report-style commands. `None` for
    // `check` (runs a minimal load → validate path). `format` and
    // `update` already returned above.
    let filter_args: Option<&ReportArgs> = command.filter();

    let mut paths: Vec<std::path::PathBuf> = Vec::new();

    // Load prices first when `-x` is set — the rebalancer and
    // realizer both need P-directives already in the journal.
    // `$ACC_PRICES_DIR` contains them.
    if filter_args.map(|f| f.exchange.is_some()).unwrap_or(false) {
        if let Ok(dir) = std::env::var("ACC_PRICES_DIR") {
            let path = std::path::Path::new(&dir);
            if path.is_dir() {
                collect_ledger_files(path, &mut paths);
            }
        }
    }

    // User-provided paths come after so their declarations win on
    // any later single-pass resolution.
    //
    // Explicit `-f FILE` is honoured regardless of extension: when the
    // user names a path, the loader will try to read it and surface a
    // proper error if it's missing, instead of silently skipping it.
    // The extension filter only applies when walking a directory tree,
    // where we need to skip backups / READMEs / unrelated files.
    for input in &args.paths {
        let path = std::path::Path::new(input);
        if path.is_dir() {
            collect_ledger_files(path, &mut paths);
        } else {
            paths.push(path.to_path_buf());
        }
    }

    // print --raw: dump source bytes, skip the full pipeline.
    if let Command::Print { raw: true, .. } = &command {
        for path in &paths {
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

    let mut journal = acc::load(&paths).map_err(|e| acc::Error::from(e.to_string()))?;

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

    // Expander phase: apply `= /pattern/` auto-rules by injecting
    // their postings into every matching transaction (scaled by the
    // triggering posting's amount). Runs before realizer so later
    // phases see the full, expanded journal.
    acc::expander::expand(&mut journal.transactions, &journal.auto_rules);

    // Realizer phase: inject fx gain/loss postings where the
    // transaction's implied rate diverges from the market rate in
    // the price DB. Runs *before* the filter so pattern matches like
    // `acc bal Equity:FxGain` can see the synthetic postings. Needs
    // `-X TARGET` plus both `fx gain` / `fx loss` accounts
    // declared in the journal; otherwise silently skipped.
    if let Some(target) = exchange_target.as_deref() {
        if let (Some(gain), Some(loss)) = (
            journal.fx_gain.as_deref(),
            journal.fx_loss.as_deref(),
        ) {
            acc::realizer::realize(
                &mut journal.transactions,
                target,
                &journal.prices,
                &journal.precisions,
                gain,
                loss,
            );
        }
    }

    // Lotter phase (capital gains): FIFO lot tracking. Runs whenever
    // both `capital gain` / `capital loss` accounts are declared —
    // unlike the realizer/translator it works with *or* without `-X`.
    // With `-X` it values lots at market (price DB) and books the
    // holding-period movement (the trade-day deviation stays with the
    // realizer's fx gain/loss); without `-X` it books the total native
    // gain straight from the books. Returns the (account, commodity)
    // pairs it realized a gain on so CTA can exclude them (else both
    // book the same drift and double-count).
    let capital_tracked = if let (Some(cg), Some(cl)) = (
        journal.capital_gain.as_deref(),
        journal.capital_loss.as_deref(),
    ) {
        acc::lotter::realize_capital(
            &mut journal.transactions,
            cg,
            cl,
            exchange_target.as_deref(),
            &journal.prices,
            &journal.precisions,
        )
    } else {
        std::collections::HashSet::new()
    };

    // Translator phase (CTA): emit synthetic translation-adjustment
    // transactions for transit accounts whose native sum is zero but
    // whose target drift is non-zero. Both `cta gain` and `cta loss`
    // accounts must be declared so positive and negative drifts can
    // be routed separately. Runs *before* filter so `acc bal in:cta`
    // pattern matches the injected postings. Lot-tracked pairs are
    // excluded — the lotter already booked their holding-period drift.
    if let Some(target) = exchange_target.as_deref() {
        if let (Some(cta_gain), Some(cta_loss)) = (
            journal.cta_gain.as_deref(),
            journal.cta_loss.as_deref(),
        ) {
            let precision = journal.precisions.get(target).copied().unwrap_or(2);
            acc::translator::translate(
                &mut journal.transactions,
                target,
                &journal.prices,
                cta_gain,
                cta_loss,
                precision,
                &capital_tracked,
            );
        }
    }

    // Expand `-p` / `-b` / `-e` with the same period grammar: a year
    // (`YYYY`), a month (`YYYY-MM`) or a day (`YYYY-MM-DD`). Both
    // `-b` and `-e` take the *start* of the specified period — they
    // are point-in-time cutoffs, not spans. `-e` stays exclusive as
    // before. `-p` is the only one that spans: its half-open range
    // covers the whole period.
    // Expand every `-p` up front. Single `-p` becomes a normal
    // begin/end pair for the main filter. Multiple `-p` drop the
    // begin/end route and filter transactions against the union of
    // periods further below.
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
        0 => (None, None),
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
    let begin = explicit_begin.as_deref().or(period_begin.as_deref());
    let user_end = explicit_end.as_deref().or(period_end.as_deref());

    // Hide future transactions by default. The filter's `end` is
    // exclusive, so `today+1` keeps everything up to and including
    // today and drops anything strictly after. When the user already
    // passed `-e` / `-p`, we take the earlier of the two cutoffs.
    let show_future = filter_args.map(|f| f.future).unwrap_or(false);
    let future_cap: Option<String> = (!show_future).then(|| {
        let today_str = acc::date::ms_to_date(acc::date::current_ms());
        let today = acc::date::Date::parse(&today_str)
            .expect("current_ms() returns valid YYYY-MM-DD");
        acc::date::Date::from_days(today.days() + 1).to_string()
    });
    let end: Option<&str> = match (user_end, future_cap.as_deref()) {
        (Some(u), Some(cap)) => Some(if u < cap { u } else { cap }),
        (Some(u), None) => Some(u),
        (None, Some(cap)) => Some(cap),
        (None, None) => None,
    };

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
        acc::rebalancer::rebalance(
            &mut journal.transactions,
            target,
            &journal.prices,
        );
    }

    // `-R` / `--real`: drop every virtual posting from the output.
    // Realizer/translator injections (fx gain/loss, CTA release) all
    // use virtual postings, so this hides them while keeping the
    // underlying computation intact.
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
        Command::Print { raw: false, .. } => acc::commands::print::run(&journal),
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
