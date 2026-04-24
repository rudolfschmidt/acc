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
    /// (fx gain / fx loss / translation adjustment) are hidden.
    #[arg(short = 'R', long = "real")]
    real: bool,

    /// Related postings. With a pattern filter, show the *other*
    /// postings of the matched transactions — the counter-parties —
    /// instead of the matched postings themselves. `acc reg ^ex:cta
    /// -r` shows which accounts balance against ex:cta in each
    /// transaction.
    #[arg(short = 'r', long = "related")]
    related: bool,

    /// Sort by field: date, amount, account, description. Prefix with - for reverse. (default: date)
    #[arg(short = 'S', long = "sort", default_value = "date")]
    sort: Vec<String>,

    /// Convert all amounts into this commodity using exchange rates
    #[arg(short = 'x', long = "exchange", value_name = "COMMODITY")]
    exchange: Option<String>,

    /// Market-snapshot mode for `-x`: convert every posting at a fixed date
    /// instead of at its own tx.date. Without a value: today. With a value
    /// (YYYY-MM-DD): snapshot as of that date.
    #[arg(
        long = "market",
        value_name = "DATE",
        num_args = 0..=1,
        default_missing_value = "today",
    )]
    market: Option<String>,
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
    /// Only the parser runs: no balance check, so journals with
    /// unbalanced transactions still format fine. Transactions
    /// are stably date-sorted by default (`--no-sort` to disable).
    /// Writes atomically. Pass `-` as path to pipe via
    /// stdin/stdout (`:%!acc format -` in vim).
    #[command(arg_required_else_help = true)]
    Format {
        /// Skip the chronological sort and keep transactions in their
        /// source order.
        #[arg(long = "no-sort")]
        no_sort: bool,
        /// Files or directories to format. Directories are walked
        /// recursively for `.ledger` files. `-` reads from stdin,
        /// writes to stdout.
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
            | Self::Diff { .. } => &[],
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
            | Self::Diff { .. } => None,
        }
    }
}

fn main() {
    if let Err(e) = start() {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}

fn has_ledger_ext(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("ledger")
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
            } else if path.is_file() && has_ledger_ext(&path) {
                out.push(path);
            }
        }
    }
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
    // load pipeline. Parse-only, no resolve / book / rebalance, so a
    // journal with balance errors still formats without complaining.
    if let Command::Format { no_sort, paths } = &command {
        return acc::commands::format::run(paths, *no_sort);
    }

    // Diff is standalone — source-level file/tree comparison, no
    // downstream pipeline.
    if let Command::Diff { snapshot, paths } = &command {
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
    for input in &args.paths {
        let path = std::path::Path::new(input);
        if path.is_dir() {
            collect_ledger_files(path, &mut paths);
        } else if has_ledger_ext(path) {
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

    // Resolve the `-x` target through the journal's aliases so
    // `-x EUR` and `-x €` both collapse to the canonical symbol the
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
    // `-x TARGET` plus both `fx gain` / `fx loss` accounts
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

    // Translator phase (CTA): emit synthetic translation-adjustment
    // transactions for transit accounts whose native sum is zero but
    // whose target drift is non-zero. Both `cta gain` and `cta loss`
    // accounts must be declared so positive and negative drifts can
    // be routed separately. Runs *before* filter so `acc bal in:cta`
    // pattern matches the injected postings.
    if let Some(target) = exchange_target.as_deref() {
        if let (Some(cta_gain), Some(cta_loss)) = (
            journal.cta_gain.as_deref(),
            journal.cta_loss.as_deref(),
        ) {
            let fixed_date: Option<String> = filter_args.and_then(|f| f.market.as_deref()).map(|m| {
                if m == "today" {
                    acc::date::ms_to_date(acc::date::current_ms())
                } else {
                    m.to_string()
                }
            });
            let precision = journal.precisions.get(target).copied().unwrap_or(2);
            acc::translator::translate(
                &mut journal.transactions,
                target,
                &journal.prices,
                fixed_date.as_deref(),
                cta_gain,
                cta_loss,
                precision,
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
    let related = filter_args.map(|f| f.related).unwrap_or(false);
    let mut journal = acc::filter::filter(
        journal,
        command.patterns(),
        begin,
        end,
        related,
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

    // Rebalance phase: convert posting amounts into -x target.
    // `--market [DATE]` picks a fixed snapshot date; without it each
    // posting uses its own tx.date.
    if let Some(target) = exchange_target.as_deref() {
        let fixed_date: Option<String> = filter_args.and_then(|f| f.market.as_deref()).map(|m| {
            if m == "today" {
                acc::date::ms_to_date(acc::date::current_ms())
            } else {
                m.to_string()
            }
        });
        acc::rebalancer::rebalance(
            &mut journal.transactions,
            target,
            &journal.prices,
            fixed_date.as_deref(),
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
