use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "acc",
    version,
    about = "plaintext double-entry accounting command line tool"
)]
struct Args {
    /// Ledger file or directory to load. Directories are walked
    /// recursively. May be given multiple times; order is preserved
    /// and matters for transactions with identical dates.
    #[arg(short = 'f', long = "file", value_name = "PATH")]
    paths: Vec<String>,

    /// Include only transactions on or after this date (YYYY-MM-DD)
    #[arg(long = "begin", short = 'b', conflicts_with = "period", global = true)]
    begin: Option<String>,

    /// Include only transactions before this date (YYYY-MM-DD)
    #[arg(long = "end", short = 'e', conflicts_with = "period", global = true)]
    end: Option<String>,

    /// Include only transactions in this period. Accepts a year
    /// (YYYY), a month (YYYY-MM), or a single day (YYYY-MM-DD).
    /// Shortcut for setting `--begin` and `--end` together.
    #[arg(long = "period", short = 'p', value_name = "PERIOD", global = true)]
    period: Option<String>,

    /// Include future transactions (default: only up to today)
    #[arg(long)]
    future: bool,

    /// Sort by field: date, amount, account, description. Prefix with - for reverse. (default: date)
    #[arg(short = 'S', long = "sort", default_value = "date")]
    sort: Vec<String>,

    /// Convert all amounts into this commodity using exchange rates
    #[arg(
        short = 'x',
        long = "exchange",
        value_name = "COMMODITY",
        global = true
    )]
    exchange: Option<String>,

    /// Market-snapshot mode for `-x`: convert every posting at a fixed date
    /// instead of at its own tx.date. Without a value: today. With a value
    /// (YYYY-MM-DD): snapshot as of that date.
    #[arg(
        long = "market",
        value_name = "DATE",
        num_args = 0..=1,
        default_missing_value = "today",
        global = true,
    )]
    market: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show account balances
    #[command(visible_alias = "bal")]
    Balance {
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
        /// Filter by account name pattern
        pattern: Vec<String>,
    },
    /// Print formatted transactions
    Print {
        /// Show raw data without computed amounts
        #[arg(long)]
        raw: bool,
        /// Filter by account name pattern
        pattern: Vec<String>,
    },
    /// List all accounts
    Accounts {
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
        /// Filter by account name pattern
        pattern: Vec<String>,
    },
    /// List all commodities
    Commodities {
        /// Also show the first date the commodity was used
        #[arg(long)]
        date: bool,
        /// Filter by account name pattern
        pattern: Vec<String>,
    },
    /// Interactive account navigater
    #[command(visible_aliases = ["nav", "ui"])]
    Navigate {
        /// Show accounts with zero balance
        #[arg(short = 'E', long)]
        empty: bool,
        /// Filter by account name pattern
        pattern: Vec<String>,
    },
    /// Run consistency checks over the journal
    Check,
    /// Update exchange rate data (MEXC for crypto, openexchangerates for fiat).
    /// Standalone — does not read the journal.
    Update {
        /// Trading pair(s) in BASE/QUOTE format, e.g. BTC/USDT ETH/USDT.
        /// If omitted, all existing crypto files are updated.
        #[arg(long = "pair", num_args = 1..)]
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
            | Self::Codes { pattern }
            | Self::Commodities { pattern, .. }
            | Self::Navigate { pattern, .. } => pattern.as_slice(),
            Self::Update { .. } | Self::Check => &[],
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

fn start() -> Result<(), acc::Error> {
    let args = Args::parse();

    let Some(command) = args.command else {
        use clap::CommandFactory;
        let _ = Args::command().print_help();
        return Ok(());
    };

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

    let mut paths: Vec<std::path::PathBuf> = Vec::new();

    // Load prices first when `-x` is set — the rebalancer and
    // realizer both need P-directives already in the journal.
    // `$ACC_PRICES_DIR` contains them.
    if args.exchange.is_some() {
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

    // Realizer phase: inject fx gain/loss postings where the
    // transaction's implied rate diverges from the market rate in
    // the price DB. Runs *before* the filter so pattern matches like
    // `acc bal Equity:FxGain` can see the synthetic postings. Needs
    // `-x TARGET` plus both `fx gain` / `fx loss` accounts
    // declared in the journal; otherwise silently skipped.
    if let Some(target) = args.exchange.as_deref() {
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

    // Expand `-p` / `-b` / `-e` with the same period grammar: a year
    // (`YYYY`), a month (`YYYY-MM`) or a day (`YYYY-MM-DD`). Both
    // `-b` and `-e` take the *start* of the specified period — they
    // are point-in-time cutoffs, not spans. `-e` stays exclusive as
    // before. `-p` is the only one that spans: its half-open range
    // covers the whole period.
    let (period_begin, period_end) = match args.period.as_deref() {
        Some(p) => match expand_period(p) {
            Ok((b, e)) => (Some(b), Some(e)),
            Err(e) => fail(&e),
        },
        None => (None, None),
    };
    let explicit_begin = args.begin.as_deref().map(|b| match expand_period(b) {
        Ok((start, _)) => start,
        Err(e) => fail(&e),
    });
    let explicit_end = args.end.as_deref().map(|e| match expand_period(e) {
        Ok((start, _)) => start,
        Err(e) => fail(&e),
    });
    let begin = explicit_begin.as_deref().or(period_begin.as_deref());
    let end = explicit_end.as_deref().or(period_end.as_deref());

    // Filter phase: scope the journal to the command's pattern and
    // the global --begin / --end date range. Runs once here so every
    // commander sees an already-scoped journal.
    let mut journal = acc::filter::filter(journal, command.patterns(), begin, end);

    // Rebalance phase: convert posting amounts into -x target.
    // `--market [DATE]` picks a fixed snapshot date; without it each
    // posting uses its own tx.date.
    if let Some(target) = args.exchange.as_deref() {
        let fixed_date: Option<String> = args.market.as_deref().map(|m| {
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

    // Sort phase: user-controlled ordering applied after rebalance.
    // The booker has already validated assertions in natural date
    // order, so this is pure presentation.
    acc::sorter::sort(&mut journal.transactions, &args.sort);

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
