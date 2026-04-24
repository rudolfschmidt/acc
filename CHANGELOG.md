# Changelog

## 0.3.1 — 2026-04-24

License identifier updated from the deprecated SPDX `GPL-3.0` to
`GPL-3.0-or-later`. No behaviour change; metadata-only release so
crates.io and downstream packagers get a clean identifier.

## 0.3.0 — 2026-04-24

Automatic IAS 21 / ASC 830 Currency Translation Adjustment (CTA)
booking, plus `-r`, `-R`, multi-`-p`, working `--future`, and an
argv pre-parse that lets `-f` sit anywhere on the command line.

### Fri 24 Apr 2026 - `examples/` directory

Seven feature-focused walkthroughs under `examples/` — one
markdown file per topic, each with the journal inline, the
commands, and the verbatim output acc produces. Covers basics
(`bal` / `reg` / `print` / `accounts` / `commodities` / `codes`),
the filter DSL including `-r` / `-R` / multi-`-p`, currency
conversion with `-x` and `--market` and multi-hop lookups, fx
gain/loss realisation, CTA translation adjustment, lot and cost
annotations (`@` / `@@` / `{COST}`), and balance assertions /
assignments. Cross-linked from the README's *Examples* section
and indexed by `examples/README.md`. Added to the published
crate's `include` list so `cargo publish` ships the walkthroughs
alongside the main README.

Also: `Cargo.toml`'s `include` was extended from `**/*.rs` to
cover `README.md`, `CHANGELOG.md`, `LICENSE`, `demo.ledger`, and
the new `examples/` tree — those files are now part of the
published crate. Version bumped to `0.3.0` reflecting the CTA
feature plus the new flags.

`.gitignore` was untracked (moved to `.git/info/exclude` locally)
— the file contains deployment-local paths and doesn't belong in
the shared repo.

### Fri 24 Apr 2026 - CTA: Currency Translation Adjustment phase

A long-standing display problem with the default per-posting
historical conversion was tracked down and resolved: transit
accounts (cash, wallets, escrow) that netted to zero in their
native commodity kept showing non-zero drift in a `-x` target
currency, even though nothing economically happened — the money had
flowed through and out. The drift was real (rate moved between
inflow and outflow), but attributing it to the asset account
misrepresented where the value actually sat. Under IFRS IAS 21 and
US-GAAP ASC 830 this translation residual belongs on a **Cumulative
Translation Adjustment** account in equity / other comprehensive
income, not smeared over the balance-sheet items that briefly held
the foreign currency.

A new `translator/` phase was introduced between `realizer` and
`filter`. For every `(account, commodity)` group whose native
amounts summed to zero over the reporting period, the translator
walked postings chronologically, tracked running native and target
sums, and at every zero-crossing of the native balance emitted a
synthetic transaction on that date:

```
<date> * translation adjustment
    [<transit-account>]   TARGET -drift
    [<cta-account>]       TARGET drift
```

Both postings are **bracket-virtual** (`is_virtual: true,
balanced: true`) so they participate in balance — driving the
transit account's target sum to zero — while rendering in square
brackets in the register to mark them as translator-injected.
Double-entry remains intact: the two postings sum to zero in the
target currency.

Two new account sub-directives — `cta gain` and `cta loss` —
were added, parallel to the existing `fx gain` / `fx loss` pair.
Both must be declared for the translator to run; positive drift
(target value retained while holding native) routes to
`cta_loss`, negative drift (target value increased while holding
native) routes to `cta_gain`, following the sign convention of the
existing fx realizer. Multi-commodity transactions are tainted and
skipped — those belong to the realizer (fx gain/loss on trades) and
co-booking would double-count the same rate divergence.

Deliberate interaction with `--market`: when rebalance uses a fixed
snapshot date, every posting converts at one rate, so transit
accounts net to zero in target automatically and the translator
emits nothing. CTA only materialises under the default per-tx-date
mode, which is where drift is structurally possible.

This is, as far as the research could find, the first
plaintext-accounting tool to implement IAS 21 / ASC 830
translation adjustment automatically. hledger and ledger-cli
default to single-rate revaluation (no drift in the first place, but
historical stability lost for income/expense). beancount and
rustledger have the option infrastructure for conversion accounts
but no automatic booking — users would need to invoke
`summarize.conversions()` manually.

### Fri 24 Apr 2026 - Register renders bracket-virtual with `[...]`

The register's `render_account` previously mapped both posting-
virtual forms to `(account)` parentheses. With the translator
emitting `is_virtual: true, balanced: true` postings that do
participate in balance, the rendering was extended to distinguish:
`is_virtual && balanced` → `[account]`, `is_virtual && !balanced`
→ `(account)`, real postings unchanged. This matches ledger's
convention and makes translator-injected postings visually
distinguishable from realizer-injected fx gain/loss labels in
register output.

### Fri 24 Apr 2026 - `-r` / `--related`, modelled on ledger-cli

The flag was added with semantics taken directly from ledger-cli:
when a pattern filter would have dropped every non-matching
posting, `-r` flips the filter to keep the **sibling** postings of
the matched transactions instead — the counter-parties, the other
half of each trade. `acc reg ^ex:cta -r` answers "what accounts
did the CTA drift balance against in each adjustment" without
having to stare at full transactions.

The implementation went into the existing filter phase: if any
posting in the transaction matched, the matched postings were
dropped and the rest retained, else the whole transaction was
dropped. No new phase needed.

### Fri 24 Apr 2026 - `-R` / `--real`

The complement to `-r`: strip every virtual posting from the
output while keeping the computation that produced them intact.
Realizer still injects fx gain/loss; translator still emits
translation-adjustment transactions; rebalance still converts. But
the resulting virtual postings (both paren-virtual and
bracket-virtual) are dropped from the journal before the command
runs, so the user can see the "real" movements without the
auto-computed labels obscuring them. Transactions that become
empty after the filter are removed entirely.

### Fri 24 Apr 2026 - Multi-period `-p` with union semantics

`-p` became repeatable. The first implementation tried range
semantics — earliest period's start, latest period's end — but
that was pointed out to be redundant with `-b` / `-e`. The
semantics were flipped to **union**: each `-p` is an independent
period, and a transaction is kept if it falls within any of them.
`acc reg -p 2023-10-01 -p 2023-11-30` shows postings on exactly
those two days, not everything between them. Single `-p` behaviour
is unchanged.

### Fri 24 Apr 2026 - `--future` actually implemented

The flag had been declared in clap and documented in the README
for months but was never read anywhere — a dead signal. It now
clamps the filter's effective `end` to `today + 1` (exclusive)
unless `--future` is passed, hiding forward-dated transactions
(rent, subscriptions, recurring entries) from "what has happened"
reports. When the user also passes `-e` or `-p`, the earlier of
the two cutoffs wins.

### Fri 24 Apr 2026 - `-f` pre-parsed out of argv

A reported "conversion silently does nothing" bug turned out to
be a clap-derive limitation: `global = true` on `Vec<String>`
binds the field to a single subcommand level's matches, so `-f`
given both before **and** after the subcommand (the shape the
user's wrapper script produced: `acc -f CONFIG -f PRICES bal -f
FILE -x €`) silently dropped one side. The fix pre-parses argv
before handing it to clap: every `-f PATH` / `--file PATH`
occurrence is pulled out into a single list, the rest goes to
clap. The `-f` declaration stayed on the Args struct for `--help`
and documentation; its value is populated manually from the
pre-parse.

### Thu 23 Apr 2026 - Design decisions locked down during the phase work

Several architectural decisions were made explicit during the
pipeline work. Each was written down after a bug or a round of
refactor churn made the implicit rule necessary, so the lessons
stopped having to be re-learned.

The parser was made pure: `&str → Vec<Located<Entry>>`, with no
I/O, no shared state, no alias resolution, and no price-DB
population. This was what enabled `rayon::par_iter()` across files
to be a safe drop-in.

Alias application was moved entirely into the resolver, never during
parse. The reason was declaration order: a file can reference a
commodity before declaring its alias later in the same file, so
only the resolver — running after parse is complete — sees every
alias before applying any. The price DB was similarly restricted
to be built only in the indexer, never populated incrementally
from the parser.

The booker was kept transaction-local. Cross-transaction state
(running balance, balance-assertion, balance-assignment) was pushed
into a separate date-sorted pass rather than mixed into
per-transaction code. Earlier drafts that had running state
threaded through the per-transaction path had become untestable
without orchestrator setup.

Each phase was placed in its own folder under `src/`, with utility
primitives (`decimal.rs`, `date.rs`, `error.rs`, `i256/`) at the
root. Each phase got its own `error.rs` with a typed error
variant; the top-level `acc::Error` unified them only at the binary
boundary. `Box<dyn Error>` inside the pipeline was considered and
rejected — it would have thrown away the per-phase precision.

Every phase got `#[cfg(test)]` unit tests against inline input.
Integration tests under `tests/` were reserved for cross-phase
contracts so they didn't duplicate what unit tests already covered.

Phase boundaries were made one-way: resolver reads
`Vec<Located<Entry>>` and produces `Resolved`; booker reads
`Resolved.transactions`; indexer reads `Resolved.prices`. No phase
calls back into an earlier one.

### Thu 23 Apr 2026 - Pipeline rebuild: parser / resolver / booker / indexer / loader

The old `tokenizer` had grown into a do-it-all module — lexing,
parsing, alias lookup, price-DB population, and balance math all
happened in one pass. It worked, but it had become untestable in
isolation, impossible to reason about phase-by-phase, and actively
hostile to any parallel-parse plan because it mutated shared state on
every token. Keeping it would have meant keeping every future feature
entangled with every old one. It was split into single-responsibility
phases, each placed in its own folder under `src/` and each given its
own unit tests.

The entry point moved to a new `parser/` — rewritten as a pure
`&str → Vec<Located<Entry>>` transformation with no I/O, no alias
lookup, and no shared state. That purity is what later made
`rayon::par_iter()` over the file set a drop-in change. Downstream of
the parser, `resolver/` was added to apply commodity aliases once
(after every declaration had been seen), to extract fx-gain/fx-loss
account labels, and to date-sort the transactions. `booker/` took on
balance math — transaction-local missing-amount inference plus
cross-tx running-balance state for balance-assignment and
balance-assertion. `indexer/` was split out to build the price DB
from resolved `P`-directives and expose BFS multi-hop lookups.
`loader/` became the orchestrator that ran parser → resolver →
(indexer, booker) end-to-end and returned a `Journal`.

The downstream report phases — `filter/`, `sorter/`, `rebalancer/`,
`realizer/` — were kept separate and wired per command in `main.rs`.
The main wiring collapsed to a linear `load → realizer → filter →
rebalance → sort → command`. Every phase came with inline-string unit
tests; the integration tests then exercised the chain end-to-end.

### Thu 23 Apr 2026 - Core data types

Four primitives that had been missing from the earlier pipeline were
introduced.

`Transaction.date` had been a plain `String`. That lex-sorted
correctly but made every date query awkward — "all transactions in
Q3 2024" meant parsing each string every time. A new `Date` type in
`src/date.rs` replaced it, storing days-since-1970 as a `u32`,
parsing and formatting `YYYY-MM-DD`, and providing arithmetic like
`day + N` for free.

The `Journal` struct was brought back after having been removed in
the 10 Apr pipeline refactor. That earlier refactor had turned the
pipeline into a pure `Vec<Transaction>` flow, which simplified the
call graph but left every report command reconstructing its own view
of "date-sorted transactions plus prices plus precisions". The new
struct pulled those together once: `acc::load(&[paths])` returned a
`Journal` holding the date-sorted transactions, the `Index` price
DB, the fx-gain/fx-loss account labels, and per-commodity display
precisions. Every report took `&Journal` and read what it needed.

`acc::Error` was introduced to replace the ad-hoc `Box<dyn Error>` /
string-error mix that had accumulated at the binary boundary. Each
phase kept its own typed error (`ParseError`, `ResolveError`,
`BookError`, `LoadError`) so phase tests could still assert on
variants; the top-level `acc::Error` unified them for the CLI via
`From` impls for `String`, `io::Error`, `serde_json::Error`,
`ureq::Error`.

`Located<T>` was added as a wrapper around every entry, posting, and
comment, carrying `file: Arc<str>` plus `line: usize`. With
provenance attached to every element, any error from any phase could
be rendered with its source location without threading file context
through call chains.

### Thu 23 Apr 2026 - Performance: Arc<str> commodity interning + parallel parse

Two perf improvements were landed together because one enabled the
other.

The first was `Arc<str>` commodity interning. Before the change,
every `Price.base`, `Price.quote`, and `Index`-map key had been an
owned `String`, which meant that on a workload with a small
unique-symbol vocabulary and many times more price directives, the
allocator was the top frame in the flamegraph. Commodity strings were
reworked to be interned as `Arc<str>` — shared references handed out
from a `HashSet<Arc<str>>` in the resolver — and allocations dropped
to O(unique symbols) instead of O(total occurrences). Same strings in
memory, but only one copy per distinct commodity.

The second was parallel parsing via `rayon::par_iter()` over the
file set in `loader::read_and_parse`. That was only safe because the
parser had already been made pure — no shared state, no alias lookup
mid-parse, no price-DB mutation. File and source order were preserved
via an ordered `collect()` so downstream phases still saw the stream
as if it had been processed sequentially.

The net effect on a realistic multi-file workload: the parse phase
ran roughly 7× faster, the index phase roughly 3× faster. The
flamegraph-by-flamegraph sequence that produced these numbers is
documented under "Profiling-driven performance tuning" below.

### Thu 23 Apr 2026 - Price DB with BFS multi-hop

The indexer's output — the `Index` — was reshaped into a nested
structure: `HashMap<Arc<str>, HashMap<Arc<str>, BTreeMap<u32,
Decimal>>>`. The outer two levels keyed on base and quote commodity;
the inner `BTreeMap` keyed on day-of-year (`u32` from the new `Date`
type) and stored the rate. That shape was chosen because the single
most common report query — "latest rate on or before day D" — reduces
on a `BTreeMap` to `range(..=D).next_back()`, `O(log n)`. A flat
`HashMap<(base, quote, date), rate>` would have required either an
exhaustive scan or a parallel sorted-index structure; the
`BTreeMap` gave temporal queries for free.

Only one direction was stored per pair. Reciprocal rates (`EUR/USD`
from a stored `USD/EUR`) were computed on demand via
`Decimal::div_rounded`, which kept the DB compact and avoided having
to decide on write which direction was canonical.

Multi-hop lookups were added on top via breadth-first search across
the commodity graph of loaded `P` pairs. No hard-coded bridge
currency — the graph decides which paths exist. A four-hop `TOKEN →
STABLECOIN → USD → EUR` resolves fine if the pairs exist; a request
with no path returns `None` and the rebalancer leaves the posting in
its original commodity.

### Thu 23 Apr 2026 - Lot annotations and valuation expressions

Two parser features landed together: curly-brace lot annotations
(`{COST}`, `{=COST}`) and parenthesised amount expressions
(`(1200/12)`).

Lot annotations were needed for sell-from-lot accounting. When a
position acquired at cost X is sold at current market price Y,
ledger-cli balances against the lot cost X, with the X/Y difference
becoming the realised gain/loss. acc's booker was changed to do the
same: `{COST}` was parsed into `LotCost::Floating` and `{=COST}`
into `LotCost::Fixed`, and the booker was updated to prefer lot cost
for balance math ahead of any `@` market cost on the same posting.
The `@` market cost was kept on the posting so the rebalancer could
still use it for the conversion display. `{{TOTAL}}` (double-brace
total cost) and `[DATE]` (lot date) were parsed and consumed so
existing ledger-cli journals loaded without errors, but their
information was not modelled further.

Amount expressions were added as parse-time evaluation in
`parser/expression.rs` via recursive descent. The supported operators
ended up being `+ - * /`, unary minus, and parenthesised
subexpressions. `(€1200/12)` resolved to `€100`, `((1+2)*3)` to `9`,
and non-terminating division rounded via `Decimal::div_rounded`. Two
decisions came out of this work. First, expression-derived amounts
were given `decimals: 0` so that a `(€1200/12.33333)` division
couldn't accidentally inflate the display precision of the target
commodity — only directly written amounts contribute to observed
precision. Second, two distinct commodities inside one expression
were rejected as a parse error, which sidestepped any implicit
conversion inside arithmetic.

### Thu 23 Apr 2026 - Multi-commodity posting semantics

Two related booker changes were driven by real-world transaction
shapes that the earlier strict semantics had been rejecting.

The first was missing-amount handling in multi-commodity
transactions. A transaction with multiple commodities and a trailing
posting with no amount had been rejected as
`BookErrorKind::MultiCommodityInference`. Ledger-cli handles this
case differently — its `finalize` phase 7 expands the missing-amount
posting into one posting per commodity, each balancing exactly its
own commodity. acc's booker was updated to do the same, so
`assets:foo FOO -100 / assets:usd $-50 / expenses:wo` now expands
into three effective postings, with `expenses:wo FOO 100` and
`expenses:wo $50` replacing the single ambiguous trailing posting.
The `MultiCommodityInference` variant was deleted.

The second was the balance-check tolerance, which had been getting
too strict on transactions with high-precision `@`-rates. The
rebalancer's `effective_amount()` function was changed to set
`decimals: 0` on cost-derived amounts, so that the `is_display_zero`
threshold for balance-checking is driven by directly written posting
amounts in each commodity — not by the trailing digits of an
`@`-rate like `€0.00471698…`. Without that fix, a transaction with a
lot-cost conversion at a realistic 8-decimal rate would fail
balance-checking by sub-cent residuals that rounded to zero at any
display precision humans use.

### Thu 23 Apr 2026 - `commodity` sub-directives

The `commodity SYMBOL` block was extended to accept indented
children. Two forms landed here: `alias OTHER_SYMBOL` (previously
handled as a separate `alias` concept, now unified as a sub-directive
of `commodity`) and `precision N`, which pinned the display precision
of that commodity to exactly `N` fractional digits.

The `precision` sub-directive was introduced because the
observed-precision heuristic — take the max number of fractional
digits seen on any posting in that commodity — is wrong in practice.
Real journals occasionally carry a high-precision amount (e.g.
`0.12345678 BTC`) that shouldn't force every EUR balance report to
display 8 digits. Declaring `commodity EUR` with `precision 2` under
it pinned EUR to 2 digits regardless of whatever precision a stray
amount elsewhere happened to use.

The resolver was updated to collect explicit `precision N` overrides
during its directive pass; the loader merged them over the observed
maximum from directly written amounts. Critically,
`loader::precisions_per_commodity` was also narrowed to consider only
`Posting.amount.decimals` when computing the observed maximum. Cost
annotations (`@`, `@@`, `{…}`) and balance assertions (`= X`) were
excluded from the observed precision — previously an `@`-rate with
8 decimals would inflate the display precision of the target
commodity across every report.

### Thu 23 Apr 2026 - Booker: balance assignment + assertion

Two ledger-cli features that the earlier pipeline had not supported
were added here. Both rely on cross-transaction running balance, so
they were placed together in the booker rather than in the
transaction-local balance module.

Balance assignment added shorthand for "fill in whatever amount
brings this account to `TARGET` after this posting". Writing
`assets:bank = TARGET` without an amount now triggered the booker to
compute the amount from the running balance accumulated across all
prior transactions for that account+commodity. This was the pattern
users reached for when reconciling against a bank statement — write
the ending balance and let the tool figure out the delta.

Balance assertion was the sanity-check counterpart. Writing
`assets:bank X USD = TARGET` with an amount present made the booker
apply the posting to the running balance and verify the result
equaled `TARGET`. A mismatch raised `BookErrorKind::AssertionFailed`
with account, expected, got, and commodity. It was used to catch
import errors or data drift from manual edits.

The implementation was a single date-sorted pass maintaining a
running-balance map keyed by `(account, commodity)`. The
transaction-local balance math stayed in `booker/balance.rs`; only
the cross-tx running state moved into `booker/mod.rs`. The split kept
the per-transaction logic independently testable even though the
feature itself needed cross-transaction state.

### Thu 23 Apr 2026 - Error formatting

Parser, resolver, and booker errors were reformatted to render in
ledger-cli style — path plus line reference, a headline summary, and
the offending source excerpt:

```
While parsing file "path/to/file.ledger" at line N:
>> headline

N | source line
N | source line
```

The path+line portion was rendered cyan, the headline red+bold, the
source excerpt in the default terminal colour. The `colored` crate
was configured to auto-disable when stdout wasn't a TTY, so piping
errors to a file or another program stayed clean.

Two helpers were written to build the excerpt. `render_at_line` took
a single line number and scanned backward for the enclosing
transaction header, so the context showed a balanced transaction
rather than a stray mid-transaction line. `render_range` took
explicit line bounds for transaction-scoped errors (balance
mismatches, assignment failures) where the error spanned the whole
transaction rather than a single line.

### Thu 23 Apr 2026 - Date filters: `-p` / `-b` / `-e` with period expansion

Three CLI date-range flags were added, all sharing one period
grammar.

`-p` / `--period` was the new convenience flag. It accepted a year
(`YYYY`), a month (`YYYY-MM`), or a single day (`YYYY-MM-DD`), and
expanded to the corresponding half-open begin/end range. A year
covered 12 months, a month covered 1 month, a day covered 24 hours.
`acc bal -p 2024-12` was now all of December 2024 without having to
write out the bounds manually.

`-b` / `--begin` and `-e` / `--end` were extended to accept the same
three formats and interpret them the same way: each picked the
*start* of the specified period as its cutoff. `-b 2024` became "on
or after 2024-01-01". `-e 2026` became "before 2026-01-01", which
meant the last included transaction was 2025-12-31 — `-e` kept its
exclusive semantics.

`-p` was marked as conflicting with `-b`/`-e` at the clap level —
combining them would have been nonsensical and clap errored out
rather than silently picking one. All three flags were declared
`global = true` (clap-speak for "appears before or after the
subcommand"), so both `acc -p 2024 bal` and `acc bal -p 2024` were
accepted.

### Thu 23 Apr 2026 - `-f` filters on `.ledger` extension

`-f PATH` was narrowed to only load files ending in `.ledger` when
given a directory or explicit file path. Editor backups (`.bak`,
`.swp`), OS metadata (`.DS_Store`), and any other non-journal files
in user-specified directories were silently skipped.

The reason was practical: pointing `-f` at a working journal
directory had been occasionally pulling in stale `.bak` files,
triggering parse errors that took time to trace back to editor leftovers
rather than real journal content. The extension filter turned that
class of confusion into a non-issue. Explicit paths with non-`.ledger`
extensions got the same treatment — `acc -f notes.txt` was quietly
ignored rather than failing with a parse error.

### Thu 23 Apr 2026 - Decimal: MAX_SCALE 28 → 20

The custom `Decimal` type (i128 mantissa plus fixed scale, introduced
during the 10 Apr v0.2.0 work) had been configured with `MAX_SCALE =
28` to match `rust_decimal`. Profiling a real accounting
multiplication — an integer product around `5 × 10^10` — surfaced a
panic inside `Decimal::mul_rounded`'s intermediate i128 quotient at
that scale.

The root cause: at scale 28, the i128 mantissa had only ~1.7 × 10^10
headroom for the integer portion. Any multiplication whose integer
result approached or exceeded that bound overflowed during the
rounding step. `MAX_SCALE` was lowered to 20, which kept ~1.7 × 10^18
integer headroom and still gave more fractional precision than any
real financial workload needed. The panic went away; the mantissa
became big enough to hold every arithmetic result the pipeline
produced.

`rust_decimal` stays at scale 28 but pays for it with a larger
representation (128-bit plus extras). acc's `Decimal` was i128-only
and optimised for stack allocation, so giving up 8 fractional digits
for 8 orders of magnitude of integer headroom was the better trade
for this shape.

### Thu 23 Apr 2026 - Realizer phase (fx gain/loss injection)

A new optional pipeline phase was added in `src/realizer/` that
materialised FX gain/loss as explicit postings. It was made active
only when `-x TARGET` was set and both `fx gain` and `fx loss`
accounts were declared in the journal (via `account Equity:FxGain \n
fx gain` and the loss analogue). Otherwise the phase stayed a no-op
pass-through.

The logic it added: for each multi-commodity transaction, convert
every balance-contributing posting to the target commodity at the
transaction's `tx.date` rate and sum the converted values. If the
sum was non-zero, that was a realised FX gain or loss, and a
paren-virtual posting was injected against the declared `fx gain`
account (as income, i.e. negative posting) when the delta was
positive, or against `fx loss` (as expense, positive posting) when
the delta was negative. The injected posting made the transaction
balance in the target commodity explicitly.

Two positioning choices shaped how the phase behaved. It was ordered
to run *before* the filter phase so that `acc bal Equity:FxGain -x
€` could match the injected postings — running after filter would
have skipped the injections for filtered transactions and
under-reported. Small residuals below the target commodity's display
precision were ignored, so that rate-conversion rounding didn't
produce spurious 0.00-value fx postings.

### Thu 23 Apr 2026 - Filter: commodity keyword

The pattern DSL gained a `com SYMBOL` keyword that matched postings
by their commodity. The match was case-sensitive and compared against
the alias-resolved symbol from the resolver pass, so `com USD`
matched postings that had been written with `$`, `USD`, or any other
declared alias — all normalised to the same canonical symbol before
the filter ran.

Per-posting filtering mattered more here than elsewhere. A transfer
like `assets:usd +100 USD / assets:eur -85 EUR` matches `com EUR` on
only one posting, and neither report should have included both
postings. The filter was set up to drop non-matching postings inside
surviving transactions and remove transactions that ended up empty —
the same rule used by the rest of the filter DSL.

### Thu 23 Apr 2026 - Integration test suite

Unit tests existed per phase, but nothing exercised the chain
end-to-end through `acc::load()`. An integration test suite was added
under `tests/` to fill that gap, split across four focused test
binaries, each covering a different cross-phase contract.

`pipeline.rs` (10 tests) covered the happy path: load an inline
journal, assert on `Journal` contents — transactions are date-sorted,
missing amounts are inferred, commodity aliases are resolved, balance
assertions pass, price directives populate the index, observed vs
explicit precisions merge correctly.

`errors.rs` (9 tests) covered failure modes: unbalanced transactions,
conflicting commodity aliases, duplicate fx-gain accounts,
missing-amount-with-nothing-to-infer, single-posting transactions,
invalid price rates, division by zero in expressions,
two-commodities-in-one-expression. Each test asserted that the
correct `LoadError` variant was returned.

`lot_and_expression.rs` (8 tests) covered the harder parser features
that interact with booker balance math: `{COST}` and `{=COST}` lot
annotations, `[DATE]` lot-date consumption, sub-display-precision
residuals being accepted, parenthesised expressions with various
operator precedence, and `@@` cost-annotation sign handling.

`conversion.rs` (6 tests) covered the rebalancer: `-x TARGET` using
tx.date by default, `--market DATE` using a fixed snapshot date,
inverse rates being computed on demand, multi-hop BFS working through
the commodity graph, missing rates leaving amounts unchanged,
same-commodity being a no-op.

A shared helper `tests/common/mod.rs` wrapped the load-from-inline-
journal pattern: `TempJournal::new(src)` wrote the string to a
per-test temp dir, handed back the path, and cleaned up on `Drop`.
The helper opened with `#![allow(dead_code)]` at module level because
each test binary under `tests/` compiled to its own binary and didn't
necessarily use every helper — without the blanket allow, each binary
would have warned about helpers it happened not to call.

Fixtures used synthetic commodities (`XYZ`, `ABC`, `FOO`) and round
numbers throughout — scenarios stayed readable and no real-world
currency relationships got baked into the tests.

An obsolete `tests/integration.rs` (208 lines targeting the retired
`tokenizer::parse` API) was removed rather than ported; the four new
binaries covered the same ground via `acc::load()`. Total test count
after this work: 187 unit tests plus 33 integration tests, all green.

### Thu 23 Apr 2026 - `print` strips applied annotations

The `print` command was changed to stop rendering `@` / `@@` cost
annotations and `=` balance assertions in its output. These are
parse-time instructions for the booker — cost annotations are applied
to balance math, balance assertions are verified at load — and once
load succeeds there's nothing for a reader of post-load output to do
with them. Keeping them in the print output would have just made the
re-printed journal noisier than the original without adding
information. `print --raw` still renders them because `--raw`
bypasses the booker entirely.

### Thu 23 Apr 2026 - `Costs::Total` sign handling matches ledger-cli

Balance math with `@@` total-cost annotations had been treating the
cost amount's written sign as authoritative. Ledger-cli takes the
posting amount's sign instead — writing `FOO -100 @@ $50` means
"`-100` worth of `$50` in total", so the effective balance
contribution is `-$50`, not `+$50`. acc's booker had been inverting
this in some cases, producing unbalanced-transaction errors on valid
journals. The sign source was changed to the posting amount rather
than the cost amount; the mismatch went away.

### Thu 23 Apr 2026 - `ACC_PRICES_DIR` env var

Support for an `ACC_PRICES_DIR` environment variable was added. When
`-x TARGET` was set, every `.ledger` file under the directory the
env var pointed to was loaded before the command-line `-f` paths.
This made it practical to keep rate files outside the journal
directory — one env export and every `-x` invocation picked them up
without a long `-f` list. Left unset, or called without `-x`, the
env var did nothing, so it could stay exported permanently without
affecting journal-only workflows.

### Thu 23 Apr 2026 - `demo.ledger` quickstart reference

A minimal `demo.ledger` was added to the repo as a quickstart
reference. Two or three balanced transactions covering the common
cases (simple two-commodity, `@`-cost, commodity alias). No tests
depend on it — tests use `TempJournal` — so its role is purely
reader-facing: opening the file, running `acc -f demo.ledger bal`,
and seeing something meaningful without having to write a journal
first.

### Thu 23 Apr 2026 - Ledger-cli parity investigation

acc targets the same journal format as ledger-cli, so side-by-side
runs on the same input were the main correctness check as the
pipeline matured. At some point that check surfaced a material
divergence on a realistic multi-thousand-file workload: `acc bal -x
€` and `ledger -X € bal` disagreed by thousands of units in the
target commodity on the same inputs. Tracking down what caused the
gap — and deciding which behaviour acc should adopt — ended up
driving several of the semantic choices in this block.

The first hypotheses were ruled out one by one: missing price files,
alias mismatches between the two tools, display-precision cutoffs,
sign handling on `@` / `@@` cost annotations. None of them accounted
for the divergence. The root cause turned out to be two behaviours
ledger-cli had that acc didn't. First, ledger-cli infers an implicit
rate from every 2-commodity transaction (`xact.cc` Phases 3 and 6)
and adds it to the in-memory price DB. Second, it rolls every
outstanding balance forward to the report date using the
latest-known rate rather than each posting's own tx.date rate. Both
silently biased the totals relative to the explicit `P DATE BASE
QUOTE RATE` directives declared in the journal.

acc was deliberately taken in a different direction on both points.
Only `P`-directives were kept as contributors to the price DB —
transaction-implied rates reflect fees, rounding, and split
executions, not quotable market rates, and a report that depends on
them becomes non-reproducible. Per-posting conversion was set up to
use each posting's own `tx.date` rate by default, so a 2020 `$5`
expense renders into € at the 2020 rate on every run regardless of
when the report runs. Historical stability was judged to beat the
ledger-cli default for every non-live-valuation query. `--market
[DATE]` was added as the opt-in that reaches ledger-cli-style rolling
revaluation for the cases that actually want it — year-end
statements, current portfolio value, and similar.

The observable consequence: on inputs with many multi-commodity
transactions, acc's cross rates (computed via BFS through the graph
of explicit `P` directives) can differ materially from ledger-cli's.
When they do, acc's number is by construction the shortest path
through declared rates; ledger-cli's carries implied-rate noise on
top.

### Thu 23 Apr 2026 - Considered-and-rejected alternatives

Two directions were built out along the way and then abandoned after
measurement showed they didn't pay off.

Symbol-based commodity interning (`Symbol(u32)` backed by a
`Mutex<Interner>`) was prototyped as a replacement for
per-occurrence `Arc<str>` — smaller keys, cheaper comparisons, lower
memory. What happened: the parser ran under `rayon::par_iter()`, and
every commodity token seen across all files contended on the single
intern mutex. Wall-clock regressed roughly 2× versus `Arc<str>`; on
a larger input the ratio was worse (roughly 3× slower). `DashMap`
was tried next to remove the contention via per-bucket locking, but
hashing overhead on such a small symbol vocabulary cost more than it
saved. The whole experiment was reverted.

Implicit rate inference from 2-commodity transactions — the
ledger-cli behaviour diagnosed in "Ledger-cli parity investigation"
— was prototyped as a second indexer entry point
`indexer::index_with_implicit`. It did match ledger-cli's cross
rates, but at the cost of making the price DB disagree with the
explicit `P`-directive declarations in the journal. The prototype
was deleted rather than left behind a feature flag; the default
`indexer::index` stayed on strict P-directive semantics.

### Thu 23 Apr 2026 - Profiling-driven performance tuning

The wall-clock numbers came out of iterative flamegraph profiling
against a realistic multi-thousand-file load. Each round surfaced
one dominating hot path; the fix then exposed the next one.

Round 1 showed ~60% of wall-clock going to `String` allocation in
commodity tokenisation — fixed by the `Arc<str>` interning
described under "Performance". Round 2 showed serial file I/O
dominating once allocation dropped — fixed by the
`rayon::par_iter()` switch, also under "Performance". Round 3
showed the indexer's `HashMap::insert` at the top — fixed by the
nested `HashMap<base, HashMap<quote, BTreeMap<date, rate>>>`
described under "Price DB". A Decimal-overflow panic surfaced
along the way during a real-value multiplication — fixed by
lowering `MAX_SCALE` to 20, described under its own entry.

The sequence mattered: each fix only became obviously the right
move after the previous round had made it the new bottleneck.
Tackling them out of order would have looked like premature
optimisation.

### Thu 23 Apr 2026 - Test-fixture whitespace convention

A fixture like `2024-06-15 * X\n\tb -3 USD\n` — single space between
account and a negative amount — makes the account parser consume
`-3 USD` as part of the account name. That's a real ambiguity in
ledger syntax rather than an acc bug, but it bit the integration
tests enough times that a convention was pinned down: every fixture
was rewritten to use two-space separation between account and amount
to sidestep it.

### Tue 21 Apr 2026 - New-pipeline phases built in parallel

By the time the 10–11 Apr refactor had settled, it was clear the old
monolithic `tokenizer` was the wrong shape for where the project was
heading — pure parser, typed per-phase errors, composable
transformations, per-phase unit tests. Rewriting it in place would
have broken the CLI for days. Instead the new phases (`parser/`,
`resolver/`, `indexer/`, `balancer/`) were built up in their own
folders alongside the old tokenizer over the course of this window.
The old pipeline kept the app runnable and the test suite green; the
new phases matured behind it. See the individual entries on 23 Apr
for what each phase ended up doing (the `balancer/` module was
renamed to `booker/` a day later when cross-tx state landed).

Draft phase names bounced around before settling. The post-parse
command layer went through `reporter/` and `commander/` before
landing on `booker` / `realizer` / `rebalancer` / `sorter` /
`filter`. A parallel mid-stream `bal → balancer` rename was started
and rolled back. The takeaway recorded: structural renames should
land as one coordinated change, not threaded through an ongoing
refactor.

State at the 22 Apr architecture audit: ~95 unit tests green across
the four new phases, old tokenizer pipeline still serving the CLI,
and outstanding work for the following day: orchestrator (`load()`),
unified top-level `acc::Error`, the `Date` type (dates were still
`String`), report-phase rewiring, integration tests, rayon-parallel
file parse.

### Wed 22 Apr 2026 - Feature wave preceding the pipeline rebuild

The 22 Apr entries below were the last substantial additions on top
of the old `tokenizer`-based pipeline. They defined the surface
later work had to preserve: the `-x TARGET` flag plus price DB, the
filter DSL, commodity aliases, the `update` subcommand's API-rate
storage format, and the per-posting conversion semantics with
`--market`. All of this behaviour was preserved when the
implementation moved under the new phase layout a day later
(`parser` / `resolver` / `booker` / `indexer` / `loader` /
`realizer` / `rebalancer`).

### Wed 22 Apr 2026 - Update pipeline: raw-string rate preservation

The previous fetch path had round-tripped every API response through
`Rational::parse → format_decimal(8)`. A rate of `0.000022616404`
came back as `0.00002262` after rounding — seven significant digits
lost before the price ever reached the ledger. Worse, `serde_json`'s
default number handling silently lossy-converts decimals via `f64`,
so even before the rounding stage the precision was already gone.
Storing the API's own string byte-for-byte was the only way to
guarantee "what appears in the file is what the API returned".

API rate values were changed to be stored byte-for-byte as the API
returned them — no rounding, no `Rational` round-trip, no
reformatting. `serde_json` was rebuilt with the `arbitrary_precision`
feature so that JSON numbers preserved their full source precision
through deserialisation, which prevented the silent f64 lossy
conversion for decimals like `0.000022616404`.

The two fetch paths were reworked accordingly. MEXC crypto data had
been stored as `P DATE QUOTE BASE (1/close)` with `format_decimal(8)`
rounding — i.e. the inverse of the market rate, rounded to 8
decimals — and was changed to store `P DATE BASE QUOTE close`
verbatim, natural direction, no division, no rounding.
Openexchangerates fiat data had been stored as `P DATE USD SYM rate`
with `format_decimal(8)` and was changed to store the raw OXR number
string, which preserved up to 12 decimals on precise currencies like
BTC, XAU, and XAG.

Two follow-ups cleaned up related code. A pointless zero-filter
(`rate == "0" || rate == "0.0"`) was removed from both fetch paths —
`PriceDB::add()` already dropped zero rates at load time — and the
`Rational` import fell out of the update pipeline, so `fetch.rs`,
`fiat.rs`, and `file.rs` ended up operating purely on `String` for
rate values. Load-time lookup performance stayed identical (PriceDB
auto-inserts the inverse via `add()` regardless of which direction
the file uses), but shorter raw strings parsed a bit faster.

### Wed 22 Apr 2026 - `update --daily` flag and cadence default

`--monthly` and `--yearly` had existed already, but there was no
explicit way to say "daily" other than omitting both. Scripts that
wanted to be self-documenting had no token to pass. Making the
default explicit also let the conflict matrix reject nonsense like
`--daily --monthly` at the clap level instead of silently picking
one.

A new `--daily` flag was introduced as the explicit form of the
default cadence, compatible with both `--crypto` and `--fiat`
scopes. The three cadence flags were made mutually exclusive at the
clap level. `--monthly` and `--yearly` stayed fiat-only (they block
`--crypto` and `--pair` because crypto APIs don't paginate that
way). Cadence resolution in `main` was set to precedence yearly →
monthly → daily, defaulting to daily when no flag was given.

### Wed 22 Apr 2026 - Code dedup across update pipeline

A small cleanup pass landed on the `update` subcommand's files after
the larger raw-rate changes earlier the same day. `current_ms()` —
a Unix-timestamp-in-milliseconds helper — had been copy-pasted
identically into `main.rs`, `fetch.rs`, and `fiat.rs`. It was
consolidated into a single implementation in `src/date.rs` and the
three local copies were deleted. A dead `impl From<FetchResult> for
Option<Error>` that was never referenced came out of `fetch.rs`,
and an unused `Error` import was dropped.

### Wed 22 Apr 2026 - `print` formatting and colors

Two old hacks had stopped scaling. Positive amounts had been
prefixed with a leading space so columns *appeared* to line up with
negative amounts that had their minus sign; this broke the moment a
transaction had no negative posting at all. And the state marker
was a variable-width string (` ! `, ` * `, or a single space), so
the description column shifted between rows depending on which
state a transaction had. Both were fixed here, alongside a colour
overhaul.

The Uncleared state marker was widened to `   ` (three spaces) so
every row had a 3-char state marker and the description column
started at the same offset regardless of state. Columns were
switched to width-computed-from-actual-content rather than
fixed-spacing guesses. Critically, the padding was moved from Rust's
`{:<w$}` format specifier — which counts bytes, including ANSI
escape sequences — to explicit `chars().count()`, fixing a
months-old alignment bug that had been papered over on coloured
rows.

Colour conventions were defined: account names blue, negative
amounts red, description bold, transaction code yellow, comments
dimmed, state marker ` * ` green for Cleared and ` ! ` yellow for
Pending. The amount column was right-aligned to the longest
formatted amount across all postings, which replaced the old
leading-space-positive-amount hack. Per-posting layout became:
account left-aligned within `account_max`, then a fixed `GAP = 4`
spaces, then amount right-aligned within `amount_max`. The leading
`\t` indent was removed — lines started with `GAP` spaces instead,
so tab-stop rendering no longer shifted the amount column. `GAP`
was kept as a `usize` constant with a shared `print_spaces(n)`
helper in `src/commands/util.rs`, deduplicated between `print` and
`register`.

### Wed 22 Apr 2026 - Pattern filter keywords and negation

The old filter had been a flat "account name substring" match.
Users who wanted to query things like "all December coffee
transactions" ended up piping through `grep`. Four distinct pattern
dimensions were needed — account, description, transaction code,
commodity — and the existing surface could only express one.

Single-character shortcuts `^` and `$` were already taken by account
anchoring from the 10 Apr work, so the new short prefixes were set
up as `@` for description and `#` for code, plus spelt-out keywords
(`desc`, `code`, `com`) for readable-script use. Commodity got no
shorthand prefix because every natural ASCII shorthand clashes with
ledger syntax (`:` for accounts, `$` / `€` for currencies
themselves). `not <pattern>` was added for negating the following
single pattern across any dimension; `and` / `or` combinators were
kept, and the default between bare tokens stayed OR.

The filter was also switched from per-transaction to per-posting. A
transfer like `assets:usd +100 USD / assets:eur -85 EUR` matches
`com EUR` on only one posting; keeping both postings in the
surviving transaction would have made `reg com EUR` show USD rows
as unsought "context". Non-matching postings are now dropped from
surviving transactions, and transactions that end up empty after
that are removed — the same rule ledger-cli uses.
`Account::from_transactions()` and `register::print()` were cleaned
up to stop re-applying the matcher, since the filter phase had
already handled posting selection.

Pipeline order was swapped to `parse → balance → filter → rebalance
→ sort` (filter had been after rebalance). The reason was `com
SYMBOL`: it needed to match `USD`, not whatever `-x` had converted
USD into. If filtering had stayed after rebalance, `com USD` would
have been worthless together with `-x`.

The concrete syntax: `@foo` matches description containing `foo`
(case-insensitive; values with spaces must be shell-quoted as
`@"foo bar"`); `#XYZ` matches transaction code equal to `XYZ`
(case-insensitive, exact); `desc`, `code`, `com` are keyword forms
that consume the next token as their value (`desc` and `code`
equivalent to `@` and `#`; `com` matches the alias-resolved posting
commodity case-sensitively).

### Wed 22 Apr 2026 - Historical conversion and `--market` flag

This was the core semantic change of the whole currency-conversion
story. Under the old behaviour, `acc bal -x €` converted every
posting using the latest known rate for its commodity — the rate as
of the report date. A `$5` coffee from 2020 therefore had a
different € value on every run, not because anything in the books
had changed but because the USD/EUR rate moves daily. Book-keeping
that "remembers what was paid in €" needs to convert at `tx.date`,
not at `today`. The default was changed to per-posting conversion at
each posting's own `tx.date`, which made the same journal plus the
same rate files produce the same report forever.

For cases like year-end statements or "what's this portfolio worth
right now", the rolling valuation is what's wanted. A new `--market`
flag was added as an opt-in for that — with no value it used
today's date, with a `DATE` argument (`--market 2024-12-31`) it
snapshotted at that date. Making it opt-in kept the default pure
and reproducible.

The whole conversion was moved to one central pre-command phase
(`rebalance`). Previously each command had carried its own
`exchange`/`price_db` parameters and called `apply_exchange`
locally; five conversion sites had already diverged in small ways.
Pulling `--market` through all of them would have been a wide,
bug-prone patch. Centralising the conversion meant the commands
downstream (`bal`, `reg`, `print`, `accounts`, `navigate`) no
longer knew about `-x` at all; the pipeline rebalanced once before
filter and sort, and every report read the already-converted
amounts. The conversion code was placed in `src/prices/rebalance.rs`
(a single central pass); the legacy `src/prices/convert.rs` with
`convert_balance` / `ConvertedBalance` was deleted.

### Wed 22 Apr 2026 - Codebase cleanup

Bogus `Result<(), String>` return types on infallible functions
had forced every caller to match `Ok`/`Err` with a
`.unwrap()`-shaped branch that was never taken. Removing them cut
noise in `main.rs` and made it clear which commands could actually
fail (only `navigate::run` could, via `crossterm` I/O).

Eleven command functions that had never returned `Err` had the
`Result` wrapping removed: `codes`, `commodities`, `validate`,
`print_explicit`, `print_raw`, `accounts::{print_flat, print_tree}`,
`balance::{print_flat, print_tree}`, `register::print`, and
`group_postings_by_account`. `navigate::run` was left returning
`Result` because its `crossterm` terminal calls can genuinely
fail. Two clippy warnings went the same way: `for_kv_map` in the
`PriceDB::find` BFS loop and `doc_lazy_continuation` in a `fiat.rs`
doc comment. End-of-day state: zero clippy warnings, 85 library
tests plus 11 integration tests.

### Wed 22 Apr 2026 - Exchange rates and currency conversion

Real-world journals routinely carry a dozen or more commodities —
several fiat currencies, multiple crypto tokens, precious metals.
Reading any report that mixes them needed conversion. Without
built-in rates the only option had been manual `@`-cost annotations
on every multi-currency transaction, which didn't help for
single-currency asset balances (asking "what's `assets:crypto:btc`
worth in €?" without any `@` ever having been written). A built-in
price DB became the only viable path.

The DB was structured internally as a `BTreeMap<date, rate>` per
pair rather than a flat `HashMap<(base, quote, date), rate>`.
Report queries are almost always "latest rate on or before day D",
which is a temporal range query;
`BTreeMap::range(..=D).next_back()` handles that in `O(log n)`.
A flat HashMap would have needed a full scan to find the
nearest-earlier key. The per-pair map also got replaced cheaply
when `update` rewrote the file for that pair.

Multi-hop lookup was implemented via BFS across the graph of loaded
`P` pairs rather than a hardcoded bridge currency. USD-as-bridge
works in finance textbooks but not once the graph includes crypto
tokens and stablecoins — a four-hop `TOKEN → STABLECOIN → USD →
EUR` is a perfectly real path, and the stablecoin hop is neither
fiat nor the token's native quote. BFS finds whatever path exists
without committing to a bridge concept. Inverse rates were
computed on demand, so a `USD/EUR` entry covered both directions
without needing to store both.

Rate fetching was moved into a standalone `acc update` subcommand
rather than being folded into the main pipeline. Fetching is a
write operation against the price-files directory and has nothing
to do with reading a journal; splitting it out kept the main
pipeline read-only and let `update` run standalone (no `-f`
required).

The user-facing surface: `-x` / `--exchange CURRENCY` was added as
a global flag that converted balances and registers into the
target commodity; `global = true` in clap let it appear before or
after the subcommand. `acc update` fetched daily rates from
external APIs into the directory pointed to by `$ACC_PRICES_DIR`.
Two upstreams were supported: MEXC klines for crypto (stored as
`$ACC_PRICES_DIR/crypto/MEXC_{BASE}_{QUOTE}.ledger`, one file per
pair, no API key required), and openexchangerates.org for fiat
(one file per day holding all returned currencies, API key read
from the `OPENEXCHANGERATES_API_KEY` env var). `update` flags
included `--pair BASE/QUOTE`, `--since DATE`, `--date DATE`,
`--monthly`, `--yearly`, `--skip`, `--crypto`, `--fiat`, with a
clap-level conflict matrix preventing nonsensical combinations
(e.g. `--monthly` with `--crypto` when only fiat has monthly
data). `--pair` implied crypto scope;
`--monthly`/`--yearly`/`--skip` were fiat-only. Fiat update was
set up to follow a progressive-backfill pattern yearly → monthly
→ daily, with `--skip` avoiding re-fetching existing files.

Under the hood, the `src/prices/` module carried `PriceDB` (the
per-pair `BTreeMap<date, rate>` described above, `O(log n)` on
both insert and latest-rate-≤-date lookup) plus `convert_balance()`
for commodity → target conversion with remainder tracking when a
rate was missing. `P DATE BASE TARGET RATE` directives had been
tokenised since 10 Apr (listed among the recognised directives in
the ledger-compat work) but until now just consumed into the token
stream and discarded; they started populating a global registry
that `-x` queried for conversions. A set of date helpers was added
along the way: `date_to_ms`, `ms_to_date`, `day_after`,
`next_month_start`, `next_year_start`.

### Wed 22 Apr 2026 - Commodity alias directives

Real-world journals mix `$` (USD), `€` (EUR), `USDT`, and other
symbols across different files and years of source. Without
aliases, every consumer (balance, filter, price lookup) would have
to know all the spellings of the same currency. That would have
sprinkled normalisation code through the whole app and still
missed cases — `acc bal assets:usd -x €` wouldn't have found `$`
positions because the filter would have looked for the literal
string `usd`, not the commodity behind `$`. A single canonical
symbol per commodity, resolved once at load time, was the cleaner
path.

Aliases were declared in source rather than hardcoded, because
different ledgers (and different regions) have different
conventions about which symbol is canonical. The declaration form
was a `commodity SYMBOL` block with indented `alias OTHER_SYMBOL`
children — a journal anchored on USD writes `commodity USD \n alias
$` and the parser normalises both `$` and `USD` to the canonical
`USD`. Multiple aliases per commodity were supported (e.g. `$`
aliased for both `USD` and `USDT` in journals that use `$`
loosely).

The implementation at this point was a Mutex-based runtime registry
with `commodity::register_alias()` and `commodity::resolve()`,
analogous to the precision registry. It was the simplest choice for
the tokenizer, which was a single-pass module with no structured
state handoff. Alias application happened during amount
tokenisation in `mixed_amount.rs` so downstream code (balance,
filter, prices) saw canonical symbols directly. (A day later the
Mutex registry was replaced by an explicit `Resolved.aliases` value
passed between phases — the Mutex-registry is a tokenizer-era
artefact that didn't survive the later phase split.)

### Wed 22 Apr 2026 - Register layout rewrite

Three problems drove a full rewrite of the register renderer. The
old register had used Rust's `{:<w$}` format specifier for column
padding, which counts bytes — `{:<10}` applied to a 4-char account
name that happened to carry ANSI colour codes (maybe 14 bytes total
including escape sequences) produced zero padding, and columns
collapsed on any coloured row. The rewrite switched to explicit
`chars().count()` plus a `print_spaces(n)` helper, which fixed
padding per field regardless of embedded escapes.

Long descriptions had also been wrapping past the terminal width
and pushing the amount column off-screen entirely. The new renderer
was set up to truncate only the title column (with `…`) to whatever
`crossterm::terminal::size()` reported, and never to truncate
amounts or totals — the numbers are what matters and they had to
stay readable.

Per-posting filtering was added for the same reason as the filter
entry above: `acc reg com EUR` had to show the EUR posting with
its running EUR total, not mix in the USD posting the query didn't
ask for. Running totals were set up to follow the filtered set so
the numbers made sense for what was on screen.

The final layout: single-line-per-posting with multi-commodity
running-total continuation rows when a transaction spanned multiple
commodities. `Rational::round(precision)` was also added as a
public half-up rounding helper; `format_decimal()` had been
truncating (which is wrong for financial display) and was rewired
to delegate to the new rounding helper.

### Wed 22 Apr 2026 - `commodities` command

The price-DB and alias work from earlier the same day had
introduced enough commodity machinery that "which commodities does
this journal actually use" became a question without an answer
command. `acc codes` already listed transaction codes, `acc
accounts` already listed accounts — `acc commodities` was added to
fill the matching slot. It listed all commodities from the journal,
sorted alphabetically by default. A `--date` flag added the
first-seen transaction date next to each commodity and switched the
sort to chronological, so the introduction order became visible at
a glance.

### Wed 22 Apr 2026 - CLI polish

Several small CLI usability fixes landed together. The default
clap behaviour for "no subcommand" had been to error out and list
every subcommand plus every alias, which buried the useful `--help`
output. `Command` was changed to `Option<Command>` and the `None`
case was handled with an explicit `print_help()` branch, which kept
the output readable when the user just typed `acc` alone.

`visible_alias` was added to every subcommand so aliases (`bal`,
`reg`, `nav`, `ui`, `val`) started showing up in `--help` as
documented surface instead of being invisible "did you mean" hints.
`--pair` was extended to accept multiple values after one flag
(`--pair BTC/USDT ETH/USDT`) rather than requiring `--pair X --pair
Y`. And the tokenizer got a small Windows-line-endings fix:
trailing `CR` (`\r`) was stripped at end-of-line so CRLF-encoded
journals parsed without errors.

### Sat 11 Apr 2026 - `--empty` / `-E` flag hides zero-balance accounts

`bal` and `nav` gained an `--empty` / `-E` flag, and the default
was flipped: zero-balance accounts became hidden unless `-E` was
passed. Most reports had been producing walls of `0.00` rows for
accounts that hadn't seen activity in the queried range — noise
the user had to visually filter every run. Hiding them by default
was the useful behaviour; `-E` was kept as the escape hatch for
the occasional "show me every account regardless" case.

### Sat 11 Apr 2026 - Commodity display precision learned from first usage

Before this change, every commodity had rendered with a hard-coded
2 decimals. That was fine for `$` and `€` but wrong for every
other commodity — stock tickers (`AAPL`) should show 0 decimals,
crypto tokens often need 4 or 8. The parser was changed to observe
each commodity's first-seen fractional digit count and use that as
the display precision across all reports. `$` kept its 2 decimals
(because the first `$` amount in a typical journal is written with
2 decimals), `AAPL` started showing 0, `BTC` started showing 8.

### Sat 11 Apr 2026 - Multi-pattern filter without quoting

Positional filter arguments had previously been treated as a
single substring — `acc bal ^rud ait` was one two-word pattern,
not two independent filters. It was changed so each positional
argument becomes a separate filter pattern, combined with AND.
`acc bal ^assets ^2024` now finds assets accounts accessed in
2024 without needing shell-quoting tricks.
`from_transactions()` was also updated so the account tree used by
`bal --tree` only contained matched accounts, not their
counter-accounts from the same transactions.

### Sat 11 Apr 2026 - Register: hide equity postings, show multi-commodity per line

Two small display fixes landed together. The automatically-generated
equity postings from the balancer had been showing up as rows in
`register` output, which was clutter — the user wrote them
implicitly, not as explicit lines. They were hidden from the
register view; they still exist in the model for balance
verification. And multi-commodity balances in `balance` and
`navigate` were changed to render each commodity on its own line
instead of the old `$10.00, €5.00, BTC 0.01` comma-joined line.

### Sat 11 Apr 2026 - `-0.00` display fixed to `0.00`

Negative zero — which shows up in the model when a commodity's
running balance sums to exactly zero but via a negative-signed
intermediate — had been rendering as `-0.00` in every report. The
display was changed to normalise to `0.00` when the rendered digits
would all be zeros, regardless of the underlying sign bit.

### Sat 11 Apr 2026 - Internal refactors

Three pure-internal code-quality passes landed the same day, none
of them user-visible on their own.

`Posting::account()` and `Posting::is_real()` helpers were added to
replace duplicated pattern-matching logic that had grown across
four files. `Rational::parse()` was introduced to consolidate the
two prior entry points (`create_rational` and
`parse_decimal_to_rational`) into one method with clearer semantics.
All `super::super::super::` module paths were replaced with
`crate::` prefixes — the former had accumulated during the 10 Apr
bin/lib split and had made grep for cross-module references harder
than necessary. A new `crate::commodity` module was added to
centralise amount-formatting logic that had been duplicated across
three reporters.

### Fri 10 Apr 2026 - Interactive account navigator

An interactive TUI for browsing accounts was added as a new
command. It went through two iterations the same day: the first
landed as `browse` — a basic `ratatui` tree browser with
expand/collapse and vim keybindings (see the separate entry below)
— and was renamed and expanded into `navigate` once the feature
set had settled. `browse` no longer exists as a distinct command.

The `navigate` command (aliases `nav` and `tui`) opened an
interactive account tree with instant search. Typing filtered
accounts live, Backspace cleared the search. Each commodity
rendered on its own line with red for negative and green for
positive balances. The currently selected row had a subtle
background-colour highlight. Navigation keys covered arrow keys
plus vim bindings; Esc or Ctrl+C quit; PageUp/Down, Ctrl-u/d, and
Home/End gave fast scrolling.

### Fri 10 Apr 2026 - Validate command

A new command, `val(idate)`, was added to run consistency checks
over the journal without producing a report. The initial version
shipped with a single check: commodity symbols had to be all-
uppercase (which caught typos like `$aud` where `$AUD` was meant).
The framework was designed to grow — each check is a separate
function that takes the parsed journal and returns a list of
issues — so additional checks could land without disturbing the
command shape.

### Fri 10 Apr 2026 - `-f -` reads from stdin

`-f -` was made to read journal data from stdin instead of
requiring a file path. This let acc be used in pipes: `cat
journal.ledger | acc -f - bal` or `some-generator | acc -f -
print`. It combined with other `-f` arguments — multiple sources
(stdin plus files) were all loaded and concatenated in the order
they appeared on the command line.

### Fri 10 Apr 2026 - Empty transaction codes tolerated

Transactions written with an empty code `()` — which ledger-cli
and hledger both accept as a no-op code placeholder — had been
failing acc's parser. They were made valid, equivalent to writing
no code at all.

### Fri 10 Apr 2026 - Multi-commodity balances per line

`balance` output had been joining multi-commodity totals onto one
row (`$10.00, €5.00, BTC 0.01`). With more than two commodities
that row got unreadable. It was changed so each commodity renders
on its own line under the account, indented to align with the
amount column above. The same change was applied in `navigate` for
the same reason.

### Fri 10 Apr 2026 - Pattern filtering anchors (`^prefix`, `suffix$`, `^exact$`)

Account-name filters gained regex-style anchors: `^prefix` matched
names starting with `prefix`, `suffix$` matched names ending in
`suffix`, `^exact$` matched exactly. Without any anchor, substring
matching was kept as before, so existing invocations stayed
backward-compatible.

The reason was ambiguity in real journals. A journal with both
`assets:bank` and `assets:bank:savings` couldn't be queried for
just `assets:bank` in isolation with the substring match, because
`assets:bank:savings` contained `assets:bank` as a substring too.
`^assets:bank$` disambiguated cleanly. The anchors were wired
across every command that took a pattern — balance, register,
accounts, print, navigate.

### Fri 10 Apr 2026 - Include rewrite

The `include` directive was rewritten. The 2020-era features
stayed: `**.<ext>` globs (originally 07 Aug 2020) and cycle errors
(originally 06 Aug 2020). New in this iteration: glob syntax was
extended to accept `*.ledger`, `**/*.ledger`, and
`sub/**/*.dat` alongside the older form, and cycle detection was
moved to a shared `HashSet` across the full include tree rather
than the per-file checks it had been using. Self-includes were
silently skipped.

### Fri 10 Apr 2026 - Interactive account browser (`browse`) - superseded same day

The first iteration of the interactive TUI landed as the `browse`
command (alias `tui`), built on `ratatui` + `crossterm`. It offered
an account tree browser with vim keybindings, expand/collapse
subtrees, balance display, scrolling (PageUp/Down, Ctrl-u/d), and
jump to top/bottom (gg/G). The same day it was replaced by
`navigate` (see above) once live-search and better UX polish were
added; the `browse` command was removed.

### Fri 10 Apr 2026 - Directory loading (`-d`)

`-d DIR` was added as a new flag that loaded every journal file
under `DIR` recursively. Users with journals split across many
per-year or per-category files had been asking for a way to point
at the containing directory once instead of listing each file via
`-f`. `-d` and `-f` combined — the files were concatenated in a
stable (sorted) order. (Later, on 23 Apr, `-d` was absorbed into
`-f` when `-f` learned to accept directory paths directly.)

### Fri 10 Apr 2026 - Date filtering and sorting

Two CLI additions landed together. `--future` was added as a
boolean flag that included transactions dated after today; by
default only transactions up to today were considered. The
default-exclude was chosen because journals routinely contain
forward-dated recurring entries (rent, subscriptions) that
shouldn't show up in "what has happened" reports unless asked for.

`--sort FIELD` was added, accepting `date` (default), `amount`,
`account`, or `description`. Prefix `rev:` reversed the order
(`--sort rev:amount` for largest first). Multiple `--sort` flags
composed as secondary/tertiary criteria: `--sort date --sort
amount` meant "date primary, amount within same-date group". The
same sort mechanism was later extracted into a standalone `sorter`
pipeline phase.

### Fri 10 Apr 2026 - Raw print mode (`print --raw`)

`print --raw` was added to show the original source data without
any of the balancer's derived information — postings with missing
amounts stayed missing instead of being filled in with the
inferred balance. Default `print` (no flag) kept showing the
balanced, explicit form: missing amounts filled in, virtual
postings shown where the balancer added them. `--raw` made it
possible to round-trip the source file through acc unchanged,
versus seeing what acc computed from it. A side benefit: `--raw`
ran a bit faster because it skipped the booker phase entirely.

### Fri 10 Apr 2026 - Pipeline refactor

The monolithic load-then-report code path was split into distinct
pipeline phases: `parse → balance → filter → sort → aggregate →
report`. Each phase became a separate module under `src/`, taking
`Vec<Transaction>` as input and producing `Vec<Transaction>` (or
in the final phases, an aggregated report) as output. The
`Journal` struct that had wrapped the whole load result was
removed — transactions flowed as a raw vector through each phase,
which was simpler to reason about and easier to test
phase-by-phase.

(The pipeline shape was revisited on 23 Apr. The `Journal` struct
came back because several downstream phases wanted shared access
to per-commodity precisions and the price index without threading
them through every call; the core "`Vec<Transaction>` plus metadata
flows through phases" idea stayed.)

### Fri 10 Apr 2026 - Ledger-format parser expansion

The 2020-era parser had handled a minimal subset of the ledger-cli
journal format. Before acc could be a viable alternative to
ledger-cli or hledger, the parser needed to accept the same syntax
those tools accepted — real journals use most of the format, not
just the basics. Parser coverage was expanded in one large pass
here.

Comment syntax was broadened to all four prefixes (`#`, `%`, `|`,
`*`) that ledger-cli recognises. A full set of directives was
added: `commodity`, `account`, `P` (price), `D` (default
commodity), `Y` (year base for short dates), `A` (default
account), `N` (non-budget marker), `tag`, `payee`, `alias`,
`apply/end` blocks, and `define` macros. Automated (`=`) and
periodic (`~`) transaction blocks were made to parse — they
weren't applied yet but the parser tolerated them in real journals
without erroring.

Posting-level syntax was rounded out: inline comments on postings,
thousands separators (`$1,000`), quoted commodities for symbols
with spaces (`"long name"`), lot date/note annotations like
`{lot} [2024-01-01]`, negative signs placed before the commodity
(`-$30` in addition to `$-30`), transactions with no description
(just date plus state), and explicit balance assertions `=
amount`.

Amounts were made to accept expressions — `(9G * 6)` resolved to
`54G`, `((1.0/3.0)*0.11/10.74 VSGBX)` resolved to the expected
VSGBX amount. The evaluator was initially scoped to what real
journals used; the cleaner recursive-descent implementation landed
on 23 Apr. Virtual postings `(account)` and `[account]` were made
to parse, with the balancer aware of the semantic difference
(paren-virtual doesn't participate in balance, bracket-virtual
does). Multi-commodity balance verification used `@` per-unit and
`@@` total cost; `{lot} @ cost` was handled correctly for
gain/loss on disposal.

Benchmark against the ledger-cli test corpus: 36 of 47 ledger
format test files passed cleanly. The remaining 11 were CSV files,
shell scripts, or files intentionally broken to exercise
ledger-cli's error messages — not things a journal format parser
is expected to handle.

### Fri 10 Apr 2026 - Structural refactor, pipeline architecture

Beyond the parser expansion, the internal data model was reshaped
to match the vocabulary ledger-cli and hledger use. `MixedAmount`
(the 2020-era multi-commodity amount type) was renamed `Amount` —
ledger-cli uses "amount" for the same concept. The blanket `Item`
enum that had wrapped every journal entry type was replaced with
a dedicated `Transaction` struct; other entry types got their own
structs rather than enum variants.

The account tree was moved into its own module, `account.rs`. It
held a hierarchical structure where nodes were accounts and
children were sub-accounts (`assets` → `assets:bank` →
`assets:bank:savings`), with `find_or_create()` for building the
tree from a flat posting list, per-commodity running balances at
each node, `total()` aggregating across children, and a
`from_transactions()` builder that constructed the tree from a
filtered transaction stream in one pass.

The processing pipeline took its first clean shape: `parse →
balance → filter → aggregate → report`. Each phase was carved into
a separate module. The `filter/mod.rs` phase absorbed the
date-range (`--begin`/`--end`) and account-pattern filtering that
had been scattered across individual reports. The old `Journal`
struct was removed — transactions flowed as a raw
`Vec<Transaction>` through each phase, and the account tree was
built once after filtering and reused by the reports.

The balance and accounts-tree reports were switched to read the
account tree directly. Previously each one had reconstructed its
own view of the hierarchy from the flat transaction list, which
led to subtle inconsistencies where one report's handling of
virtual postings differed from another's. Reading from the single
shared tree fixed that class of bug.

End-of-day state: 50 tests, zero clippy warnings.

### Fri 10 Apr 2026 - Custom arithmetic engine (`rational.rs`, `i256.rs`)

The `num` crate had been the arithmetic backbone for rational
amounts, but none of its three options were a good fit.
`num::Rational64` (i64-based) overflows on values with more than
18 digits, which is insufficient for high-precision financial
data. `num::BigRational` (heap-allocated `BigInt`) solves
precision but sacrifices Rust's `Copy` semantics — every
assignment and arithmetic operation needs `.clone()`, which would
have added ~50 call sites of syntactic noise across the codebase.
The `num` crate itself drags in 8 sub-crates (num-bigint,
num-rational, num-integer, num-traits, num-complex, num-iter,
autocfg, lazy_static) for what amounts to a fraction type with
four operations.

A custom `Rational` type was written whose numerator and
denominator were `i256` values (two `u128` limbs on the stack).
77 decimal digits of exact precision, `Copy`, stack-allocated,
zero dependencies. Arithmetic used cross-reduction — GCD before
multiplication — to keep intermediate values small. The `i256`
type itself implemented schoolbook multiplication and binary long
division in about 170 lines.

### Fri 10 Apr 2026 - Codebase update (v0.2.0) - project revived

The codebase had sat untouched since 08 Sep 2020 — five and a
half years — and had bit-rotted in the meantime: Rust 2018
edition, `num` dependency tree with 8 transitive crates, manual
argument parsing, `unwrap()` panics throughout, `@@` total-cost
syntax silently dropped, 30+ clippy warnings. Before any new
feature work could go in, a housekeeping pass brought the project
back to a workable baseline.

The Rust edition was moved from 2018 to 2021, picking up the
language features that had stabilised in the meantime. Manual
argument parsing was replaced with `clap` v4, which brought
`--help`, `--version`, proper subcommand dispatch, and typo
detection for free. The `colored` dependency was bumped from 1.x
to 2.x. Every `unwrap()` panic in production code paths was
replaced with explicit error handling, and a custom `acc::Error`
type implementing `std::error::Error` was introduced as the common
error type at the binary boundary.

A few correctness bugs came out along the way. `@@` total-cost
syntax had been silently ignored by the parser and was wired up to
be parsed and handled properly. Enum variant names were tightened
(`UnbalancedPosting` → `Unbalanced`, `BalancedPosting` →
`Balanced`, `EquityPosting` → `Equity`). Typos like `resursive`
and `reselected` were fixed in public surface.

Housekeeping that was less functionally visible: the binary was
fixed to import the library crate correctly (`use acc::` instead
of `mod lib`), all debug `println!` statements and commented-out
dead code were removed from production, and the 30+ outstanding
clippy warnings were resolved. A first round of unit and
integration tests (29 tests) was added alongside the cleanup so
subsequent feature work had a safety net.

### Tue 08 Sep 2020 - Dependency bumps + function relocation

Two housekeeping commits landed this day. Cargo dependency
versions were bumped to their current point, and some internal
functions were relocated into more appropriate modules. No
features, no breaking changes. This was the last commit before the
repo went quiet for 5.5 years — work resumed on 10 Apr 2026 with
the codebase-revival pass.

### Mon 10 Aug 2020 - Cost directives

A follow-up to the previous day's cost-parsing work. The `cost`
directive form was added as a first-class concept — separate from
the `@` / `@@` annotations on individual postings, it let a
commodity's cost basis be declared in one place rather than inline
on every transaction.

### Sun 09 Aug 2020 - Cost parsing + model change

The first real handling of `@` (per-unit) and `@@` (total) cost
annotations on postings was added here. Before this commit, the
tokenizer had recognised the syntax but the model silently
dropped the cost data. The posting struct was reshaped to carry
the cost alongside the amount so it survived through to the
reports, which meant multi-commodity transactions could now be
balanced against their cost side. This was also the first
substantial model-shape change since the 04 Aug modeler removal.

### Fri 07 Aug 2020 - Glob syntax for `include`

`include` was extended to accept `**.<ext>` glob patterns for
recursive multi-file inclusion. Single-file includes still
worked; the new form let a parent journal pull in a whole
directory tree (e.g. all yearly sub-journals) in one line. The
implementation lived in `tokenizer/directives.rs` and expanded the
match-paths set before handing each file to the recursive include
pipeline added on 31 Jul.

### Thu 06 Aug 2020 - `include` cycle errors, error handling pass

Circular `include` chains (A→B→A) had been capable of putting the
parser into an infinite loop. Explicit cycle detection was added
here: the recursive include pipeline started tracking visited
files and raising a proper error message identifying the cycle
instead of spinning forever. The broader error-handling paths were
cleaned up in the same pass — the earlier codebase had a mix of
`panic!()` and returned-error styles that got consolidated toward
the returned-error side.

### Wed 05 Aug 2020 - Data model cleanup

A model refactor went in to eliminate unnecessary `.unwrap()`
calls. Fields that had been `Option<T>` were tightened to
non-nullable types where the data was always present by the time
reports saw it; constructor methods ensured the invariants. A
second pass the same day restructured `model.rs` for better
long-term maintainability. The ripple touched the commands folder
because the model type changes propagated through every report.

### Tue 04 Aug 2020 - Modeler removed, recursive include optimised

The biggest structural change in August. The separate `modeler`
layer — a thin pass-through that the parser had been handing data
to on its way to the reports — was dissolved entirely. 289 lines
of `parsers/modeler.rs` were deleted and the code that had been
talking to it was rewritten to talk directly to the balancer
output.

In the same commit, `include.rs` was split out of
`tokenizer/directives.rs` into its own file — the include logic
had grown large enough that keeping it in the directives module
was making both harder to read. The new `include.rs` came in at
135 lines. Net effect across the commit: ~900 lines deleted,
~600 lines added, mostly reorganisation.

### Mon 03 Aug 2020 - `include` bug fixes, code optimisation

Recursive includes (from 31 Jul) and the bin/lib split (from 01
Aug) had both exposed code paths the original `include`
implementation hadn't anticipated. Several bugs were fixed in
`tokenizer/directives.rs` around path resolution and relative
includes. A general code-optimisation pass tightened up hot code
in the parser module the same day.

### Sun 02 Aug 2020 - Removed debug command

The `debug` CLI command had existed since 16 Jul as a development
aid that dumped the tokenizer/parser state. With the library
split from 01 Aug, the command moved briefly to
`commands/debug.rs` but then made no sense as a user-facing
feature, so it was pulled from the CLI. The underlying debugging
machinery was moved into `parsers/debug.rs` where it stayed
reachable for library consumers. A batch of internal method
renames came along. The posting grammar was also separated into
its own tokenizer submodule `posting.rs` (~102 lines), pulled out
of `tokenizer/transaction.rs`.

### Sat 01 Aug 2020 - Published as library

Up to this point `acc` had been a pure CLI binary. Exposing the
same parse + report code as a library (`cargo install acc` for
the binary, `use acc::…` from dependent crates) let scripts and
other tools reuse the parser directly — the journal-format work
shouldn't have been bottlenecked through the CLI.

The bin/lib split that July had been building towards landed here.
`src/lib/` became the library surface, containing `parsers/` (the
`tokenizer/` modules plus `balancer` and `modeler`), `commands/`
(every report), `model.rs`, and `ledger.rs`. `src/main.rs`
collapsed to ~126 lines of thin CLI shim — argument parsing, error
rendering, and dispatch into library functions. The crate was
published as `cargo install acc`. An include-directive bug
surfaced during the restructuring (the new module layout broke a
path-resolution assumption) and was fixed in the same commit.

### Fri 31 Jul 2020 - Recursive include + library groundwork

`include` was extended to recurse — an included file could itself
include others. The first implementation from 27 Jul had only
handled one level of indirection. The update lived in
`tokenizer/directives.rs` and added ~87 lines of cycle-agnostic
traversal (cycle detection wouldn't come until 06 Aug).

Preparation for the bin/lib split also started showing up in
commits on this day. `Cargo.toml` grew a `[lib]` section and
module paths were adjusted in anticipation of the `src/lib/` move
that finalised on 01 Aug.

### Thu 30 Jul 2020 - Date formats + reg-report bug fix

A new `tokenizer/chars.rs` character-class helper (108 lines) was
added so that date, amount, and commodity parsing could share
classification logic instead of duplicating it. Date parsing was
broadened: additional formats beyond the original `YYYY-MM-DD`
were accepted. The `reg` report had a printing bug fixed (columns
mis-aligning on certain transaction shapes) in
`cmd_printer_register.rs`. A fair amount of internal restructure
came with the new `chars.rs` — `transaction.rs`, `directives.rs`,
and `mod.rs` were all touched to route through it.

### Wed 29 Jul 2020 - Commodity parsing, code structure

Two substantial commits landed on this day. First, the monolithic
`parser_lexer.rs` (~491 lines) was deleted entirely and replaced
by a structured `tokenizer/` module: `mixed_amount.rs` (119
lines, handling commodity + amount parsing including negative
sign placement and quoted commodities), `transaction.rs` (173
lines), and an expanded `mod.rs` (225 lines) coordinating them.
The second commit the same day fixed an `include` regression
caused by the tokenizer restructure — comment and directive
handling paths had shifted. This was the commit that established
the tokenizer folder structure that carried through the rest of
2020.

### Mon 27 Jul 2020 - `include` directive, first implementation

A single-file journal doesn't scale — users split their books by
account type, by year, by source. The `include` directive was the
ledger-compatible mechanism for splitting and recombining, and
this was the first implementation.

A new `ledger.rs` module was added (152 lines) that orchestrated
the reading and tokenising of included files. `main.rs` was
trimmed by ~135 lines because a lot of single-file I/O logic
moved into `ledger.rs`. `model.rs` shed 34 lines as some
directive-handling moved into the tokenizer. The handling at this
point was single-level only; recursive includes came on 31 Jul.

(This was the first of several `include` iterations: globs were
added on 07 Aug 2020, cycle detection on 06 Aug, a clean rewrite
came on 10 Apr 2026, and the directive was removed entirely on 23
Apr 2026 in favour of `-f DIR`.)

### Sun 26 Jul 2020 - `print` report rewrite

The `print` report was rewritten from scratch (~138 lines changed
in `cmd_printer_print.rs`). The old implementation had accumulated
one-off bug fixes that the rewrite replaced with a cleaner loop
over transactions. In the same commit, `parser_lexer.rs` saw a
large restructure (~460 lines touched) as part of making `print`
output byte-identical to ledger-cli for common transaction shapes.

### Fri 24 Jul 2020 - `accounts --tree`, unbalanced-transaction check

The first significant restructure day. Ten commits covered three
threads. First, parser and lexer were substantially rewritten —
`lexer.rs` (~258 lines touched) and `parser.rs` (~243 lines
touched) got cleaner separation of concerns. Second,
`accounts --tree` was added as a hierarchical-rendering variant,
indenting child accounts under parents (63 new lines in
`cmd_accounts.rs`). Third, the balancer grew an explicit
unbalanced-transaction check that raised errors at parse time
rather than silently misreporting in `bal` or `reg`.

`bal` flat gained a grand-total row. All report files were renamed
with a `cmd_printer_` prefix. Model and parser file renames set
the stage for later reorganisations (`parser_logic.rs` →
`parser_model.rs`).

### Fri 17 Jul 2020 - `accounts` and `codes` commands

Two new reports landed. `acc accounts` listed every account that
appeared anywhere in the journal (flat alphabetical, 32 lines in
`cmd_accounts.rs`). `acc codes` listed every transaction code
seen (15 lines in `cmd_codes.rs`). Reports got the first `cmd_`
naming hint (`printer.rs` → `cmd_printer.rs` and friends). A
minimal `demo.ledger` was checked in at the repo root for manual
testing. Some internal comment-model tweaks came along in the
same window.

### Thu 16 Jul 2020 - First register output, `debug` command, code lexing

The first day with working reports. `reg` produced a
register-style dump of transactions with amounts rendered (263 new
lines in `printer_register.rs`). `print` produced ledger-style
formatted output, including inline comments on transactions. A
`debug` command was added for dumping the parsed-model state
during development (removed again on 02 Aug once the development
pattern had settled). The parser was split into a `lexer.rs` +
`parser.rs` pair — the lexer classified input lines into token
types (transaction header, posting, comment, directive), and the
parser built the in-memory model from them. The new `src/lexer.rs`
got ~43 lines of tokenising code. Closing touches: `.gitignore`,
an iterator-pattern change in the parser, and a clippy pass.

### Wed 15 Jul 2020 - Project inception

The initial commit established the repo: `LICENSE` (GPL-3.0) and
a two-line `README.md` stating the project's intent — a
command-line plaintext-accounting tool for the ledger-cli journal
format. `main.rs` picked up argument handling, and the rest of
the day went into lifetime-annotation fixes as the first module
boundaries took shape. By end of day `main.rs` compiled and
accepted command-line arguments but produced no output yet —
reports landed the next day.
