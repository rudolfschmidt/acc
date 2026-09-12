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
    #[arg(short = 'f', long = "file", value_name = "PATH", value_hint = clap::ValueHint::AnyPath)]
    paths: Vec<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

/// The date-range flags, flattened into the report commands and into
/// `update` alike — a period means the same span wherever it is
/// written, and `crate::period` resolves it for both.
#[derive(ClapArgs, Clone, Debug)]
struct PeriodArgs {
    /// Start at this date, inclusive. Takes a year (YYYY), a month
    /// (YYYY-MM) or a day (YYYY-MM-DD) and uses its first day.
    #[arg(long = "begin", short = 'b', conflicts_with = "periods")]
    begin: Option<String>,

    /// Stop before this date, exclusive. Takes the same forms as
    /// `--begin` and uses the first day of the period named, so
    /// `-e 2025` and `-e 2025-01-01` are the same bound.
    #[arg(long = "end", short = 'e', conflicts_with = "periods")]
    end: Option<String>,

    /// A whole period instead of `--begin`/`--end`. Accepts a year
    /// (YYYY), a month (YYYY-MM), or a single day (YYYY-MM-DD).
    /// Reports take `-p` repeatedly and keep a transaction that
    /// matches any of them; `update` works on one period at a time.
    #[arg(long = "period", short = 'p', value_name = "PERIOD")]
    periods: Vec<String>,
}

/// Filter / sort / conversion flags shared by every report-style
/// command (`balance`, `register`, `print`, `accounts`, `codes`,
/// `commodities`, `navigate`). Flattened into each variant via
/// `#[command(flatten)]` so `acc format --help` and `acc update
/// --help` stay uncluttered.
#[derive(ClapArgs, Clone, Debug)]
struct ReportArgs {
    #[command(flatten)]
    period: PeriodArgs,

    /// Include transactions dated after today. Hidden by default so
    /// forward-dated recurring entries (rent, subscriptions) don't
    /// clutter "what has happened" reports.
    #[arg(long)]
    future: bool,

    /// Show real postings only — drop every virtual posting
    /// (paren-virtual `(account)` and bracket-virtual `[account]`)
    /// from the output. The realizer, lotter and translator inject
    /// *real* postings (slippage gain/loss, capital gain/loss, currency
    /// translation adjustment), so `--real` keeps those; it only removes
    /// the `(…)`/`[…]` virtual postings written in the source journal.
    #[arg(long = "real")]
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
    /// posting (default) or just the counter-parties (`-r`). `-R` pairs
    /// with `-r`: `-r` narrows to the counter-parties, `-R` widens to the
    /// whole transaction. A pattern then picks *which transactions* to
    /// show in full, not which lines of them.
    #[arg(short = 'R', long = "related-all")]
    related_all: bool,

    /// Show only postings whose amount is positive (`>= 0`). A
    /// secondary filter applied after selection: it narrows which
    /// postings are shown, by sign. A zero amount counts as both
    /// positive and negative, so it shows under `--pos` and `--neg`.
    #[arg(long = "pos")]
    pos: bool,

    /// Show only postings whose amount is negative (`<= 0`). The
    /// negative counterpart of `--pos`; zero amounts show under both.
    #[arg(long = "neg")]
    neg: bool,

    /// Show only postings whose account matches PATTERN. A projection
    /// applied after transaction selection, on the *full* posting set
    /// of each matched transaction — the positional pattern picks which
    /// transactions, this picks which of their postings — so
    /// `--related-all` is not needed to widen first. Account-only:
    /// `^acc` (starts-with), `acc$` (ends-with), `^acc$` (exact), or
    /// `acc` (contains); case-insensitive. The running total sums only
    /// the shown postings. Named after ledger's `-d`.
    #[arg(short = 'd', long = "display", value_name = "PATTERN")]
    display: Option<String>,

    /// Show only postings whose amount matches EXPR. A secondary
    /// projection like `--pos` / `--neg`, applied after selection: it
    /// narrows which postings are shown by comparing each amount against a
    /// threshold. EXPR is `[op]number` with op one of `>`, `<`, `>=`,
    /// `<=`, `=` and `<>` (not equal); a bare number is `=`. The comparison
    /// is signed, so `-a 100` keeps `100` but not `-100`, `-a '>50'` keeps
    /// amounts above 50, and `-a '<>0'` keeps every non-zero amount. Under
    /// `-X` the threshold is read in the target commodity.
    // `allow_hyphen_values` so a negative threshold (`-a -100`) is taken
    // as the value, not parsed as another flag.
    #[arg(short = 'a', long = "amount", value_name = "EXPR", allow_hyphen_values = true)]
    amount: Option<String>,

    /// Sort by field: date, amount, account, description. Prefix with - for reverse. (default: date)
    #[arg(short = 'S', long = "sort", default_value = "date")]
    sort: Vec<String>,

    /// Convert all amounts into this commodity using exchange rates.
    /// Mirrors ledger's `-X`. Each posting is valued at the exchange
    /// rate on its own transaction date (historical valuation).
    #[arg(short = 'X', long = "exchange", value_name = "COMMODITY")]
    exchange: Option<String>,

    /// Show native commodities: skip the default valuation that report commands
    /// otherwise apply when `[exchange] default` is configured. No effect when
    /// `-X` is given explicitly (that already picks a target).
    #[arg(long = "native")]
    native: bool,

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

/// The lint checks, selectable as positional arguments to `lint`. With
/// none given, every check runs. The kebab-case value names match each
/// check's reported id, so `lint dir-category` runs just that one.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
enum LintRule {
    CommodityCasing,
    LeafAccounts,
    RoleReferences,
    DirCategory,
}

impl LintRule {
    /// The check's reported id — must match the `name` field of the
    /// corresponding `Lint` so the rule filter lines up.
    fn as_str(self) -> &'static str {
        match self {
            LintRule::CommodityCasing => "commodity-casing",
            LintRule::LeafAccounts => "leaf-accounts",
            LintRule::RoleReferences => "role-references",
            LintRule::DirCategory => "dir-category",
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Show account balances
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
    /// Show each position's trade history and open lots: what was bought
    /// when at which real price, what was sold when for how much, and
    /// the profit — realized per sale, unrealized per open lot (with
    /// `-V`, priced at the latest rate). Entry/Exit are booked trade
    /// rates (the actual payments, converted into the `-X` target at
    /// the trade date), so a line's profit spans market move plus
    /// execution spread — it reconciles with `capital` + `slippage`
    /// together. The first open lot of a position is the FIFO front the
    /// next sale closes. The account pattern picks which positions are
    /// shown; tracking always spans the whole journal, since lots
    /// migrate between accounts through transfers
    Lots {
        /// `-b` scopes which SALES are shown (the open lots always
        /// show), `-e` is the tracking cutoff — only transactions
        /// strictly before it feed the FIFO queues, and lot ages are
        /// measured against the day before. `-p 2025` sets both, so a
        /// period means here what it means everywhere else.
        #[command(flatten)]
        period: PeriodArgs,
        /// Feed forward-dated transactions into the tracking too
        /// (hidden by default, like every report)
        #[arg(long)]
        future: bool,
        /// Value the cost basis in this commodity — each lot at the
        /// market rate of its acquisition day. Defaults to the config's
        /// `[exchange] default` like every report
        #[arg(short = 'X', long = "exchange", value_name = "COMMODITY")]
        exchange: Option<String>,
        /// Skip the default valuation: show each lot's booked trade-rate
        /// basis in its native counter-commodity. No effect when `-X` is
        /// given explicitly
        #[arg(long = "native")]
        native: bool,
        /// Append two columns per lot: its value at the latest available
        /// rate and the unrealized gain/loss against the basis. Only
        /// meaningful together with `-X`
        #[arg(short = 'V', long = "unrealized")]
        unrealized: bool,
        /// Filter by account name pattern
        pattern: Vec<String>,
    },
    /// Swap-cycle signal over the journal's booked rates, made for a
    /// shell prompt: one dim line with the current price, the buy-back
    /// and sell marks with the distance to each, and what acting now
    /// would bring — a loud SWAP line only when a mark is crossed. The
    /// sell mark is the entry side's average booked cost x (1+T/100),
    /// the buy-back mark the exit side's sale rate / (1+T/100), so a
    /// full cycle lands on the original entry price and multiplies the
    /// stack by (1+T/100). The journal-derived rates are cached (keyed
    /// on the flags, invalidated by the journal files' mtimes), so only
    /// the first run after an import pays for the tracking
    Signal {
        /// The held asset position: its open lots' average booked cost
        /// is the entry rate (lots pattern, e.g. '^wallet$ and com xmr')
        #[arg(long = "entry", value_name = "PATTERN")]
        entry: String,
        /// The position holding a past sale's proceeds: its open lots
        /// give the sale rate and the quantity given away (lots
        /// pattern, e.g. '^wallet$ and com usdc')
        #[arg(long = "exit", value_name = "PATTERN")]
        exit: String,
        /// Price pair for the current rate, as in `update --pair`
        #[arg(long = "pair", value_name = "BASE/QUOTE")]
        pair: String,
        /// Signal threshold in percent, symmetric around the round trip
        #[arg(long = "threshold", value_name = "PERCENT", default_value = "40")]
        threshold: String,
        /// Value the entry side in this commodity — pick the unit the
        /// pair quotes in. Defaults to the config's `[exchange] default`
        #[arg(short = 'X', long = "exchange", value_name = "COMMODITY")]
        exchange: Option<String>,
        /// Render for a status bar: the same segments uncoloured and
        /// without the SWAP lines, each leading with the rate its mark
        /// derives from, the crossed side's label in capitals. Split on
        /// `|` for one block per segment
        #[arg(long = "bar")]
        bar: bool,
        /// Scenario price: evaluate at this rate instead of the price DB
        price: Option<String>,
    },
    /// Interactive account navigater
    Navigate {
        #[command(flatten)]
        filter: ReportArgs,
        /// Show accounts with zero balance
        #[arg(short = 'E', long)]
        empty: bool,
        /// Filter by account name pattern
        pattern: Vec<String>,
    },
    /// Lint the journal — flag convention and consistency issues as warnings
    Lint {
        /// Which check to run (`commodity-casing`, `leaf-accounts`,
        /// `role-references`, `dir-category`). Omit to run all.
        rule: Option<LintRule>,
        /// Account prefixes that count as categories (income / expense),
        /// e.g. `--categories '^in:' '^ex:'`. The dir-category check then
        /// only evaluates postings whose account starts with one of these
        /// (a leading `^` is optional); a transaction with none — a pure
        /// transfer — is skipped. Without it, the check falls back to the
        /// last posting.
        #[arg(long = "categories", value_name = "PREFIX", num_args = 1..)]
        categories: Vec<String>,
        /// Preview the fixes for auto-fixable checks (currently only
        /// `dir-category`): each `old → new` account rewrite, writing
        /// nothing. Add `-e` to apply. Checks without a fixer still report.
        #[arg(long = "fix")]
        fix: bool,
        /// Apply the `--fix` rewrites in place (atomic per file).
        #[arg(short = 'e', long = "execute", requires = "fix")]
        execute: bool,
    },
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
        /// On a two-posting transaction whose postings share a commodity,
        /// leave the second posting's amount to be inferred — drop it so it
        /// auto-balances instead of writing it by hand. Off by default.
        #[arg(long = "infer")]
        infer: bool,
        /// The inverse of `--infer`: on a transaction with more than two
        /// postings that share a commodity and have exactly one amount
        /// omitted, fill that amount in (the negated sum of the rest), so the
        /// balancing leg is written out explicitly. Off by default.
        #[arg(long = "fill")]
        fill: bool,
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
    /// ACCOUNT` — and emits one offsetting entry per still-open posting,
    /// at that posting's date, to bring the account back to zero. A debit
    /// posting (> 0) books to EXPENSE:SEGMENT, a credit posting (< 0) to
    /// INCOME:SEGMENT. Each entry takes the account's last segment as its
    /// title and is cleared.
    ///
    /// Idempotent and file-agnostic: a posting whose offset already
    /// exists anywhere in the loaded journal cancels and is skipped, so
    /// re-running only closes newly-opened postings — no markers, no
    /// file-name tracking. A round-trip settled later (invoice then
    /// payment) cancels too. All four arguments are required.
    ///
    /// The entries are printed to stdout, already aligned and date-sorted;
    /// where they go is the caller's job, e.g. `acc sweep … >> FILE`.
    /// The status line is written to stderr.
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
    /// Rename an account across the `-f` files / directories by prefix —
    /// `rename foo:5 foo:4` renames `foo:5`, `foo:50`, `foo:5:…` (OLD need
    /// not be a whole segment); anchored at the start, so `bar:foo:5` is
    /// untouched. Only posting accounts are rewritten (not `account`
    /// directives). Preview by default (writes nothing); `-e` applies it in
    /// place.
    #[command(arg_required_else_help = true)]
    Rename {
        /// The account (prefix) to rename.
        old: String,
        /// The account (prefix) to rename it to.
        new: String,
        /// Apply the rename in place. Without it, only a preview prints.
        #[arg(short = 'e', long = "execute")]
        execute: bool,
    },
    /// Update exchange rate data (MEXC for crypto, openexchangerates for fiat).
    /// Standalone — does not read the journal.
    Update {
        /// Trading pair in BASE/QUOTE format, e.g. BTC/USDT. Repeat
        /// `--pair` to update multiple pairs. If omitted, all existing
        /// crypto files under $PRICES/crypto/ are updated.
        #[arg(long = "pair")]
        pairs: Vec<String>,
        /// The days to fetch, overwriting what is stored for them. Without
        /// them the run continues at the last stored day, which is
        /// refetched because it may still have been open when it was
        /// written.
        #[command(flatten)]
        period: PeriodArgs,
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
    /// Import a bank CSV export into a `@cash` ledger via a per-bank
    /// profile. Default is a dry-run (prints the additions as a diff);
    /// `--execute` appends them. Standalone — does not read the journal.
    Import {
        /// The CSV export(s) to import — list several after the flag
        /// (`--csv a.csv b.csv`), repeat it (`--csv a.csv --csv b.csv`), or
        /// pass a directory (its `*.csv` are read). A named flag (not
        /// positional) so the shell offers file completion, like `--conf`.
        /// Note: zsh completes only the first file after each `--csv`, so
        /// repeat the flag or pass a directory when you want completion for
        /// each. Omit for RPC-source profiles (e.g. `wallet.coin monero`).
        #[arg(long = "csv", value_name = "FILE", value_hint = clap::ValueHint::FilePath, num_args = 1..)]
        csv: Vec<String>,
        /// The import profile (e.g. `bank.conf`). Must be given as `--conf`
        /// (no short flag) so it is never confused with a positional path.
        #[arg(long = "conf", value_name = "FILE", value_hint = clap::ValueHint::FilePath)]
        conf: String,
        /// Execute the import — append the new transactions to the ledger.
        /// Without it, the command only prints what it would add (dry-run).
        #[arg(short = 'e', long = "execute")]
        write: bool,
    },

    /// Print a shell completion script to stdout. Source it once per
    /// shell — e.g. `source <(acc completions zsh)` — so the shell
    /// completes acc's subcommands, flags and file paths. Paths are
    /// completed by the shell's own file completion, so `~` expands
    /// natively; regenerate only when acc's CLI changes.
    Completions {
        /// Target shell (bash, zsh, fish, elvish, powershell).
        #[arg(value_name = "SHELL")]
        shell: clap_complete::Shell,
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
            // `lots` filters its account pattern itself — the queues must
            // be tracked over the whole journal first, so the generic
            // filter phase must not see the pattern.
            Self::Lots { .. } => &[],
            Self::Signal { .. } => &[],
            Self::Update { .. }
            | Self::Lint { .. }
            | Self::Format { .. }
            | Self::Diff { .. }
            | Self::Import { .. }
            | Self::Rename { .. }
            | Self::Completions { .. }
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
            | Self::Lint { .. }
            | Self::Format { .. }
            | Self::Diff { .. }
            | Self::Import { .. }
            | Self::Rename { .. }
            | Self::Completions { .. }
            | Self::Lots { .. }
            | Self::Signal { .. }
            | Self::Sweep { .. } => None,
        }
    }
}

pub fn run() {
    run_from(std::env::args().collect());
}

/// Run the CLI over an explicit argv (`argv[0]` is the program name). A wrapper
/// binary can rewrite the arguments first — e.g. expand short command aliases —
/// then delegate here, since the args are taken as a parameter rather than read
/// from the environment.
pub fn run_from(argv: Vec<String>) {
    run_from_with(argv, &mut |_, _| Ok(()));
}

/// [`run_from`] with an [`Inject`](crate::Inject) hook: the report path calls
/// `inject` after resolving + filling and before booking, so an embedder (e.g.
/// the wrapper) can add its own programmatically-built transactions — the
/// replacement for the removed `=` auto-rules.
pub fn run_from_with(argv: Vec<String>, inject: crate::Inject) {
    if let Err(e) = start(argv, inject) {
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

/// Hand the `-b` / `-e` / `-p` flags to the shared resolver.
fn resolve_period(args: Option<&PeriodArgs>) -> Result<crate::period::Range, String> {
    crate::period::resolve(
        args.and_then(|a| a.begin.as_deref()),
        args.and_then(|a| a.end.as_deref()),
        args.map(|a| a.periods.as_slice()).unwrap_or(&[]),
    )
}

/// Recursively collect journal files under `dir` (sorted, skipping
/// non-journal files) into `out`. Public so a wrapper binary can load the
/// same file set acc's report commands do when handed a directory.
pub fn collect_ledger_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                collect_ledger_files(&path, out);
            } else if path.is_file() && crate::is_journal_file(&path) {
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
fn distinct_commodities(tx: &crate::parser::transaction::Transaction) -> usize {
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
fn split_file_args(argv: Vec<String>) -> (Vec<String>, Vec<String>) {
    let mut iter = argv.into_iter();
    let mut rest: Vec<String> = iter.next().into_iter().collect();
    let mut files = Vec::new();
    while let Some(a) = iter.next() {
        match a.as_str() {
            "-f" | "--file" => {
                if let Some(v) = iter.next() {
                    files.push(expand_tilde(&v));
                }
            }
            _ => rest.push(a),
        }
    }
    (files, rest)
}

/// Expand a leading `~` / `~/` to `$HOME` (the inverse of `shorten_home`).
/// The shell normally does this, but the dynamic shell completion hands
/// back path candidates with the tilde still literal — and zsh escapes it
/// to `\~`, which the shell then won't expand — so a tab-completed
/// `-f ~/…` can reach acc unexpanded. Only a leading bare `~` or `~/` is
/// handled; `~user` and a `~` mid-path are left untouched.
fn expand_tilde(input: &str) -> String {
    let Ok(home) = std::env::var("HOME") else {
        return input.to_string();
    };
    if input == "~" {
        return home;
    }
    match input.strip_prefix("~/") {
        Some(rest) => format!("{home}/{rest}"),
        None => input.to_string(),
    }
}

/// Run the standalone commands that bypass the report pipeline — each
/// does its own file IO and returns directly. Returns `Some(result)` when
/// `command` was one of them (and has now run), `None` for a report-style
/// command (`balance`, `register`, …) the caller must drive through
/// load → enrich → filter → rebalance → sort itself.
fn try_standalone(
    command: &Command,
    paths: &[String],
) -> Option<Result<(), crate::Error>> {
    match command {
        // Format does its own validation (parse → resolve → book) before
        // writing, all-or-nothing.
        Command::Format { sort, infer, fill, paths } => {
            Some(crate::commands::format::run(paths, *sort, *infer, *fill))
        }

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
            Some(crate::commands::diff::run(snapshot.as_deref(), paths))
        }

        // Update fetches exchange rates; it does not read the journal.
        Command::Update {
            pairs,
            period,
            monthly,
            yearly,
            skip,
            crypto,
            fiat,
            ..
        } => {
            let flags = if *crypto || *fiat {
                crate::commands::update::UpdateFlags { crypto: *crypto, fiat: *fiat }
            } else if !pairs.is_empty() {
                crate::commands::update::UpdateFlags { crypto: true, fiat: false }
            } else {
                crate::commands::update::UpdateFlags { crypto: true, fiat: true }
            };
            let cadence = if *yearly {
                crate::commands::update::Cadence::Yearly
            } else if *monthly {
                crate::commands::update::Cadence::Monthly
            } else {
                crate::commands::update::Cadence::Daily
            };
            let range = match resolve_period(Some(period)) {
                Ok(r) => r,
                Err(e) => fail(&e),
            };
            Some(crate::commands::update::run(pairs, &range, cadence, *skip, flags))
        }

        // Import converts a bank CSV into ledger transactions. It reads the
        // target @cash file (for dedup) but never the journal as a whole.
        Command::Import { csv, conf, write } => {
            let csvs: Vec<String> = csv.iter().map(|c| expand_tilde(c)).collect();
            let conf = expand_tilde(conf);
            Some(crate::commands::import::run(&csvs, &conf, *write))
        }

        // Completions just prints a static script for the target shell —
        // no journal, no file IO. Path arguments (`-f`, `-c`, the import
        // CSV) carry a `value_hint`, so the generated script defers them
        // to the shell's own file completion.
        Command::Completions { shell } => {
            use clap::CommandFactory;
            let mut cmd = Args::command();
            clap_complete::generate(*shell, &mut cmd, "acc", &mut std::io::stdout());
            Some(Ok(()))
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
                crate::load(&sweep_paths)
                    .map_err(|e| crate::Error::from(e.to_string()))
                    .and_then(|j| crate::commands::sweep::run(j, account, segment, income, expense)),
            )
        }

        // Rename rewrites account names across the `-f` files. It does its
        // own per-file parse + surgical edit (never the report pipeline).
        // Config files carry no postings, so passing them along with `-f`
        // is harmless — only posting accounts are ever touched.
        Command::Rename { old, new, execute } => {
            if paths.is_empty() {
                eprintln!("Error: No files specified. Use -f PATH.");
                std::process::exit(1);
            }
            let mut rename_paths: Vec<std::path::PathBuf> = Vec::new();
            for input in paths {
                let path = std::path::Path::new(input);
                if path.is_dir() {
                    collect_ledger_files(path, &mut rename_paths);
                } else {
                    rename_paths.push(path.to_path_buf());
                }
            }
            Some(crate::commands::rename::run(&rename_paths, old, new, *execute))
        }

        // Lint validates the whole journal, so it must not go through the
        // report pipeline: no date filter (a forward-dated typo is still a
        // typo), no `-X` / sort, and no enrichment (the injected gain/loss /
        // CTA postings are synthetic — a linter should only see what the user
        // wrote). It parses + books the `-f` files directly, like sweep.
        Command::Lint { rule, categories, fix, execute } => {
            if paths.is_empty() {
                eprintln!("Error: No files specified. Use -f PATH.");
                std::process::exit(1);
            }
            // Ledger root for the dir-category check comes from `$BASE` (there
            // is no `--base` flag).
            let base = std::env::var("BASE").ok();
            let mut lint_paths: Vec<std::path::PathBuf> = Vec::new();
            for input in paths {
                let path = std::path::Path::new(input);
                if path.is_dir() {
                    collect_ledger_files(path, &mut lint_paths);
                } else {
                    lint_paths.push(path.to_path_buf());
                }
            }
            let rule_names: Vec<String> = rule.iter().map(|r| r.as_str().to_string()).collect();
            Some(
                crate::load(&lint_paths)
                    .map_err(|e| crate::Error::from(e.to_string()))
                    .and_then(|j| {
                        crate::commands::lint::run(
                            &j,
                            base.as_deref(),
                            categories,
                            &rule_names,
                            *fix,
                            *execute,
                        )
                    }),
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
    let range = match resolve_period(filter_args.map(|f| &f.period)) {
        Ok(r) => r,
        Err(e) => fail(&e),
    };
    let (period_ranges, begin, user_end) = (range.periods, range.begin, range.end);

    // The filter's `end` is exclusive, so `today+1` keeps everything up to
    // and including today. With an explicit `-e`/`-p`, take the earlier.
    // Only report commands reach here (lint and the other standalone commands
    // returned earlier), so the cutoff never touches a whole-journal check.
    let show_future = filter_args.map(|f| f.future).unwrap_or(false);
    let future_cap: Option<String> = (!show_future).then(|| {
        let today_str = crate::date::ms_to_date(crate::date::current_ms());
        let today = crate::date::Date::parse(&today_str)
            .expect("current_ms() returns valid YYYY-MM-DD");
        crate::date::Date::from_days(today.days() + 1).to_string()
    });
    let end = match (user_end, future_cap) {
        (Some(u), Some(cap)) => Some(if u < cap { u } else { cap }),
        (Some(u), None) => Some(u),
        (None, Some(cap)) => Some(cap),
        (None, None) => None,
    };

    (period_ranges, begin, end)
}

fn start(raw_args: Vec<String>, inject: crate::Inject) -> Result<(), crate::Error> {
    let (files, argv) = split_file_args(raw_args);
    let mut args = Args::parse_from(argv);
    args.paths = files;

    let Some(command) = args.command else {
        use clap::CommandFactory;
        let _ = Args::command().print_help();
        return Ok(());
    };

    // Standalone commands (format, diff, update, sweep, rename, import,
    // completions, lint) bypass the report pipeline entirely and return here.
    if let Some(result) = try_standalone(&command, &args.paths) {
        return result;
    }

    if args.paths.is_empty() {
        eprintln!("Error: No files specified. Use -f PATH.");
        std::process::exit(1);
    }

    // Filter / convert flags for report-style commands. Only report commands
    // reach here — the standalone commands (format, lint, update, …) already
    // returned above via `try_standalone`.
    let filter_args: Option<&ReportArgs> = command.filter();

    // Effective valuation target: an explicit `-X` wins; otherwise, unless the
    // run is `--native`, fall back to the config's `[exchange] default` (unset →
    // no default, i.e. native amounts). This is the single source of truth for
    // whether — and into what — the report values. `lots` carries its own
    // (reduced) flag set instead of `ReportArgs`, so it resolves here too.
    let resolve_exchange = |explicit: Option<&String>, native: bool| {
        if let Some(x) = explicit {
            Some(x.clone())
        } else if native {
            None
        } else {
            crate::loader::config::default_exchange()
        }
    };
    let exchange: Option<String> = match &command {
        Command::Lots { exchange, native, .. } => resolve_exchange(exchange.as_ref(), *native),
        Command::Signal { exchange, .. } => resolve_exchange(exchange.as_ref(), false),
        _ => filter_args.and_then(|f| resolve_exchange(f.exchange.as_ref(), f.native)),
    };

    // Price-DB files (`$PRICES`, only under `-X`) are kept separate
    // from the user's journal files so they can be loaded *selectively*: the
    // journal is parsed first to learn which commodities the report touches,
    // and only the price pairs connecting them are then parsed. The DB is
    // ~800k directives; a report needs a handful of pairs.
    let mut price_paths: Vec<std::path::PathBuf> = Vec::new();
    if exchange.is_some()
        && let Ok(dir) = std::env::var("PRICES")
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

    // `signal` decides warm/cold itself: with a fresh cache it never
    // loads the journal or the price DB — that is what makes it cheap
    // enough for a shell prompt. The loader is handed in lazily for the
    // cold path (a journal file newer than the cache).
    if let Command::Signal { entry, exit, pair, threshold, bar, price, .. } = &command {
        let mut load = || {
            if price_paths.is_empty() {
                crate::load_report_with(&journal_paths, &mut *inject)
            } else {
                crate::load_selective_report_with(
                    &journal_paths,
                    &price_paths,
                    exchange.as_deref(),
                    &mut *inject,
                )
            }
            .map_err(|e| crate::Error::from(e.to_string()))
        };
        return crate::commands::signal::run(
            &crate::commands::signal::Cycle { entry, exit, pair, threshold },
            exchange.as_deref(),
            price.as_deref(),
            *bar,
            &journal_paths,
            &mut load,
        );
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
        crate::load_report_with(&journal_paths, inject)
    } else {
        crate::load_selective_report_with(&journal_paths, &price_paths, exchange.as_deref(), inject)
    }
    .map_err(|e| crate::Error::from(e.to_string()))?;

    // Resolve the `-X` target through the journal's aliases so
    // `-X GBP` and `-X £` both collapse to the canonical symbol the
    // price DB and postings are stored under. Without this, the
    // lookup silently misses when the CLI uses one spelling and the
    // journal the other.
    let exchange_target: Option<String> = exchange.as_deref().map(|t| {
        journal.aliases.get(t).cloned().unwrap_or_else(|| t.to_string())
    });

    // `lots` bypasses enrichment and filtering: it runs the lotter's
    // tracking itself over the *unenriched* journal (an enriched one
    // already carries `{}`-annotated split legs, which the tracker would
    // misread as manual lots) and applies only the date cutoff — the
    // account pattern merely scopes the display inside the command.
    if let Command::Lots { period, future, unrealized, pattern, .. } = &command {
        let range = resolve_period(Some(period)).unwrap_or_else(|err| fail(&err));
        let day = |s: &str| crate::date::Date::parse(s).unwrap_or_else(|err| fail(&err));
        let user_end: Option<crate::date::Date> = range.end.as_deref().map(day);
        // Same default future cutoff as every report: today+1, exclusive.
        let future_cap: Option<crate::date::Date> = (!future)
            .then(|| crate::date::Date::from_days(crate::date::Date::today().days() + 1));
        let cutoff = match (user_end, future_cap) {
            (Some(u), Some(cap)) => Some(if u < cap { u } else { cap }),
            (a, b) => a.or(b),
        };
        if let Some(c) = cutoff {
            journal.transactions.retain(|lt| lt.value.date < c);
        }
        // Ages are measured against the last day *inside* the cutoff.
        let as_of = cutoff
            .map(|c| crate::date::Date::from_days(c.days().saturating_sub(1)))
            .unwrap_or_else(crate::date::Date::today);
        // `-b` scopes which *sales* are shown, not what is tracked.
        let begin: Option<crate::date::Date> = range.begin.as_deref().map(day);
        crate::commands::lots::run(
            &journal,
            exchange_target.as_deref(),
            *unrealized,
            as_of,
            begin,
            pattern,
        );
        return Ok(());
    }

    // Enrichment phases (realizer → lotter → translator, plus
    // the `--unrealized` revaluator). These must see the whole journal —
    // the lotter tracks lots FIFO across all transactions; the translator
    // and revaluator identify pass-through / open positions by their
    // journal-wide native sum — so they run before any filtering, together
    // in `pipeline::enrich`.
    let unrealized = filter_args.map(|f| f.unrealized).unwrap_or(false);
    crate::pipeline::enrich(&mut journal, exchange_target.as_deref(), unrealized);

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
    // and `--related-all` requests the same for any report command,
    // unlike `reg` / `bal` which otherwise reduce to the matched
    // postings only — a pattern then picks *which entries* to show, not
    // which lines of them.
    let related = filter_args.map(|f| f.related).unwrap_or(false);
    let related_all = filter_args.map(|f| f.related_all).unwrap_or(false);
    let whole_transactions = related_all || matches!(command, Command::Print { .. });
    let pos = filter_args.map(|f| f.pos).unwrap_or(false);
    let neg = filter_args.map(|f| f.neg).unwrap_or(false);
    let sign = crate::filter::SignFilter::from_flags(pos, neg);
    let display = filter_args.and_then(|f| f.display.as_deref());
    // Parse `--amount` up front so a malformed EXPR fails cleanly here,
    // consistent with how the date flags are validated before the report runs.
    let amount = filter_args
        .and_then(|f| f.amount.as_deref())
        .map(|e| crate::filter::AmountFilter::parse(e).unwrap_or_else(|err| fail(&err)));
    let mut journal = crate::filter::filter(
        journal,
        &crate::filter::Selection {
            patterns: command.patterns(),
            begin,
            end,
            related,
            whole_transactions,
            sign,
            display,
            amount: amount.as_ref(),
        },
    );

    // Multiple `-p`: keep transactions whose date falls within any
    // of the supplied periods. Half-open `[begin, end)` per period.
    if period_ranges.len() > 1 {
        let parsed: Vec<(crate::date::Date, crate::date::Date)> = period_ranges
            .iter()
            .filter_map(|(b, e)| {
                let b = crate::date::Date::parse(b).ok()?;
                let e = crate::date::Date::parse(e).ok()?;
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
        crate::rebalancer::rebalance(&mut journal.transactions, target, &journal.prices);
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
    crate::sorter::sort(&mut journal.transactions, sort_keys);

    match command {
        Command::Balance { flat, empty, .. } => {
            crate::commands::balance::run(&journal, !flat, empty);
        }
        Command::Register { .. } => crate::commands::register::run(&journal),
        Command::Print { raw: false, .. } => {
            // `print -X` rounds to display precision and re-balances each
            // transaction so the output is a valid, reloadable journal.
            // This is print-specific (bal/reg keep full precision), so it
            // lives here in the print arm, not in the generic pipeline.
            // Order vs. sort is irrelevant — rounding is per-transaction.
            if let Some(target) = exchange_target.as_deref() {
                crate::rebalancer::round_for_print(
                    &mut journal.transactions,
                    target,
                    &journal.precisions,
                );
            }
            crate::commands::print::run(&journal);
        }
        Command::Accounts { tree, .. } => crate::commands::accounts::run(&journal, tree),
        Command::Codes { .. } => crate::commands::codes::run(&journal),
        Command::Commodities { date, .. } => crate::commands::commodities::run(&journal, date),
        Command::Navigate { empty, .. } => {
            if let Err(e) = crate::commands::navigate::run(&journal, empty) {
                eprintln!("navigate: {}", e);
            }
        }
        _ => eprintln!("internal error: unexpected command reached match arm"),
    }
    Ok(())
}
