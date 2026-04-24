# acc

[![crates.io](https://img.shields.io/crates/v/acc.svg)](https://crates.io/crates/acc)
[![license](https://img.shields.io/crates/l/acc.svg)](LICENSE)

> **acc(ounting)** — a plaintext double-entry accounting CLI in the
> ledger tradition, written in Rust.

acc reads the [ledger](https://www.ledger-cli.org/) journal format and
continues John Wiegley's CLI-first lineage: reports, filters, currency
conversion, an interactive navigator — all driven from plaintext files
you own and edit with whatever tools you already use. Independent
codebase, its own semantic choices, no database, no cloud, no account.

---

## Quick start

```sh
cargo install acc
```

Put this in `journal.ledger`:

```
2024-01-01 (opening) initial balances
    assets:checking           $5000.00
    equity:opening           $-5000.00

2024-01-05 (42) Groceries
    expenses:food              $58.20
    assets:checking

2024-01-10 * paycheck
    assets:checking           $2500.00
    income:salary            $-2500.00
```

Run:

```
$ acc -f journal.ledger bal
 $7441.80  assets
 $7441.80    checking
$-5000.00  equity
$-5000.00    opening
   $58.20  expenses
   $58.20    food
$-2500.00  income
$-2500.00    salary
---------
        0
```

```
$ acc -f journal.ledger reg
2024-01-01 initial balances   assets:checking    $5000.00   $5000.00
                              equity:opening    $-5000.00          0
2024-01-05 Groceries          expenses:food        $58.20      $58.20
                              assets:checking     $-58.20          0
2024-01-10 paycheck           assets:checking   $2500.00    $2500.00
                              income:salary    $-2500.00          0
```

The repo ships a [`demo.ledger`](demo.ledger) you can use instead of
typing the example above:

```
git clone https://github.com/rudolfschmidt/acc
cd acc
cargo run -- -f demo.ledger bal
```

---

## Contents

- [Installation](#installation)
- [Goals and scope](#goals-and-scope)
- [Accounting standard focus](#accounting-standard-focus)
- [Reference](#reference)
- [Examples](#examples)
- [Journal format](#journal-format)
- [Filtering](#filtering)
- [Currency conversion](#currency-conversion)
- [Rate updates (`acc update`)](#rate-updates-acc-update)
- [Directives](#directives)
- [Philosophy](#philosophy)
- [Influences and relation to related tools](#influences-and-relation-to-related-tools)
- [FAQ](#faq)
- [Contributing](#contributing)

---

## Installation

From crates.io:

```
cargo install acc
```

From source:

```
git clone https://github.com/rudolfschmidt/acc
cd acc
cargo build --release
./target/release/acc --help
```

Minimum Rust: **1.85** (edition 2024). Runs anywhere Rust builds
(Linux, macOS, Windows, BSDs).

**What gets written where:** acc never writes to your journal. The
only thing that writes to disk is `acc update`, which writes rate
files under `$ACC_PRICES_DIR`. Network I/O happens only in `acc
update` (to MEXC and openexchangerates.org).

---

## Goals and scope

**Goal:** provide a CLI tooling surface for plaintext double-entry
bookkeeping — parse the ledger format, produce the reports users
need, support the currency-conversion workflows real journals
demand.

**Position:** primarily inspired by ledger, developed as a further
step in that lineage rather than a rewrite of it. Ideas from hledger
(stricter parsing, better errors) and beancount (typed accounts,
lot tracking) are picked up where they solve real problems.
Implementation and semantic choices are acc's own.

**Supported today:** `balance`, `register`, `print`, `accounts`,
`commodities`, `codes`, `check`, interactive `navigate`, `update`
(rate fetching); transactions with states, codes, arithmetic
expressions in amounts, `@` / `@@` cost annotations, `{COST}`
lot annotations, virtual postings, balance assertions and
assignments; directives `commodity` (with `alias`, `precision`),
`account` (with `fx gain` / `fx loss` / `cta gain` / `cta loss`),
and `P`; filter DSL across account / description / code /
commodity plus `-r` sibling-posting view; per-posting currency
conversion at `tx.date` with `--market` snapshot mode; multi-hop
price lookups; **automatic IAS 21 / ASC 830 translation adjustment**
(CTA) for transit accounts; `-R` real-only output.

**Not in scope today:** `include` directive, `apply/end`, `define`,
the short-form directives `D` / `Y` / `A` / `N`, `tag`, `payee`,
periodic transactions (`~` blocks), automated transactions (`=`
blocks — the line-leading `=`, not the posting-level balance
assertion / assignment which *does* work), CSV import, query
language, budget reports, web UI, value expressions.

Journals using any of those will fail to load — acc has no
silent-skip policy for directives it doesn't understand.

Some of the list is permanently out of scope (CSV import,
BQL-style queries, web UI — adjacent tools cover those). Some
might land later (periodic and automated transactions, a few of
the short-form directives).

---

## Accounting standard focus

acc aims for **professional accounting correctness**, not hobby-grade
approximations. The reference is IFRS **IAS 21** (*The Effects of
Changes in Foreign Exchange Rates*) and its US-GAAP counterpart
**ASC 830** (*Foreign Currency Matters*) — both codify how to handle
multi-currency reporting without distorting historical records.

### The three rules IFRS / GAAP codify

| Rule | What the standard says | How acc handles it |
|------|-----------------------|---------------------|
| **(1) Income & expense** | Translate at the rate of each transaction (or period average). Must not revalue retroactively — quarterly and annual comparisons would break. | Default: per-posting conversion at `tx.date`. A 2020 expense stays at its 2020 `$`-value forever under `-x $`. |
| **(2) Monetary balance items** | Cash, receivables, payables are shown at the **current rate** at the report date — what's in the account is worth what it's worth today. | Opt-in: `--market [DATE]`. |
| **(3) Cumulative Translation Adjustment (CTA)** | The difference arising from applying different rates under (1) vs (2) is booked to a dedicated equity account under Other Comprehensive Income. | Implemented: declare `cta gain` / `cta loss` accounts. See [`cta gain` / `cta loss`](#cta-gain--cta-loss--currency-translation-adjustment). |

### Why this matters — and how acc differs

**ledger-cli** and **hledger** default to *one rate for everything*
at the report date. Simple, but violates rule (1): a 2020 expense
shows a different value every time exchange rates move. Reports
across periods become incomparable. Neither tool implements CTA.

**beancount** has the `account_previous_conversions` option
(inherited into rustledger), but the automatic booking to the CTA
account is not wired up — it remains a manual post-processing
step in both tools.

**acc's default is historical-per-transaction**, which preserves
income/expense stability (rule 1) and matches the temporal method
of IAS 21. `--market` covers rule (2). `cta gain` / `cta loss`
closes the loop on rule (3). **acc is the first plaintext-accounting
tool that implements full IFRS IAS 21 currency translation
automatically** — the other tools either skip drift by collapsing to
a single rate (losing historical stability) or carry the option in
their schema without wiring up the booking.

### Professional focus, no ceremony

acc is deliberately not a hobby budget tool. Reports are meant to
be auditable, reproducible, and consistent with how real accounting
is done. Where correctness requires a concept from IFRS or GAAP
(CTA, temporal method, cost-basis preservation via `{cost}` lot
annotations), acc adopts it — not as boilerplate, but because the
alternatives produce wrong numbers.

At the same time: no unnecessary ceremony. You don't declare units,
dimensions, operations, or business-entity boundaries. The file
format stays ledger-native and editor-friendly. Professional
correctness comes through semantics, not syntax overhead.

---

## Reference

Man-page style. Every command, every flag, every environment
variable.

### `acc` — global flags

```
acc [GLOBAL OPTIONS] <COMMAND> [COMMAND OPTIONS] [ARGS]
```

| Flag                       | Default | Description |
|----------------------------|---------|-------------|
| `-f`, `--file PATH`        | —       | Journal file or directory. Directories walked recursively, only `.ledger` files loaded. Repeat `-f` for multiple sources (order preserved). Works at any position — before or after the subcommand. `-f -` reads from stdin — only with `print --raw`; other commands silently ignore it. |
| `-b`, `--begin DATE`       | —       | Include transactions on or after `DATE`. Accepts `YYYY`, `YYYY-MM`, or `YYYY-MM-DD` — each picks the *start* of the specified period. Conflicts with `-p`. |
| `-e`, `--end DATE`         | —       | Include transactions strictly before `DATE` (exclusive). Same grammar as `-b`. Conflicts with `-p`. |
| `-p`, `--period PERIOD`    | —       | Shorthand spanning a full period. `YYYY` = year, `YYYY-MM` = month, `YYYY-MM-DD` = single day. Repeat `-p` to include multiple discrete periods — a transaction is kept if it falls within any. Conflicts with `-b` / `-e`. |
| `--future`                 | off     | Include transactions dated after today. Hidden by default (rent, subscriptions, recurring forward-dated entries shouldn't clutter "what has happened" reports). When also using `-e` / `-p`, the earlier cutoff wins. |
| `-S`, `--sort FIELD`       | `date`  | Sort key: `date` (alias `d`), `amount` (`amt`), `account` (`acc`), `description` (`desc`, `payee`). Prefix with `-` for reverse (`--sort -amount`). Repeat `--sort` for secondary / tertiary keys. Unknown fields silently fall back to `date`. |
| `-x`, `--exchange SYMBOL`  | —       | Convert every amount into `SYMBOL` using the price DB. |
| `--market [DATE]`          | —       | Snapshot mode for `-x`. No value = today. `YYYY-MM-DD` = that date. Without `--market`, `-x` converts each posting at its own `tx.date`. |
| `-R`, `--real`             | off     | Drop virtual postings from the output (both `(account)` paren-virtual and `[account]` bracket-virtual). Realizer and translator still compute their adjustments for correctness, but the injected labels (fx gain / fx loss / translation adjustment) are hidden. |
| `-r`, `--related`          | off     | With a pattern filter, show the *other* postings of matched transactions — the counter-parties — instead of the matched postings themselves. `acc reg ^expenses -r` shows which accounts balanced against expenses. Modeled on ledger-cli's `--related`. |
| `-h`, `--help`             | —       | Print help. Works on `acc` and every subcommand. |
| `-V`, `--version`          | —       | Print version and exit. |

Running `acc` with no subcommand prints help.

### `acc balance` (alias `bal`)

```
acc [GLOBAL OPTIONS] balance [OPTIONS] [PATTERN]...
```

Account balances, grouped hierarchically by default.

| Flag               | Default | Description |
|--------------------|---------|-------------|
| `--flat`           | off     | One line per account, no tree indentation. Conflicts with `--tree`. |
| `--tree`           | on      | Hierarchical tree (default unless `--flat`). |
| `-E`, `--empty`    | off     | Include zero-balance accounts (default: hidden). |
| `PATTERN...`       | —       | Positional account-name patterns. See [Filtering](#filtering). |

Example output see the [Examples](#examples) section below.

### `acc register` (alias `reg`)

```
acc [GLOBAL OPTIONS] register [PATTERN]...
```

Transaction-by-transaction register with per-commodity running total.

| Arg            | Description |
|----------------|-------------|
| `PATTERN...`   | Positional pattern filters. |

Example output:

```
$ acc -f demo.ledger reg
2024-01-01 initial balances   assets:checking    $5000.00   $5000.00
                              equity:opening    $-5000.00          0
2024-01-05 Groceries          expenses:food        $58.20      $58.20
                              assets:checking     $-58.20          0
2024-01-10 paycheck           assets:checking   $2500.00    $2500.00
                              income:salary    $-2500.00          0
```

### `acc print`

```
acc [GLOBAL OPTIONS] print [OPTIONS] [PATTERN]...
```

Re-emit the journal.

| Flag         | Default | Description |
|--------------|---------|-------------|
| `--raw`      | off     | Dump the original source bytes verbatim. Missing amounts stay missing, assertions stay visible, nothing computed. Bypasses the full pipeline. |
| `PATTERN...` | —       | Positional pattern filters (ignored with `--raw`). |

Default mode emits balanced, normalised output with every missing
amount filled in by the booker.

### `acc accounts`

```
acc [GLOBAL OPTIONS] accounts [OPTIONS] [PATTERN]...
```

List every account referenced in the journal.

| Flag         | Default | Description |
|--------------|---------|-------------|
| `--flat`     | on      | One account per line (default). |
| `--tree`     | off     | Hierarchical tree. |
| `PATTERN...` | —       | Positional pattern filters. |

### `acc commodities`

```
acc [GLOBAL OPTIONS] commodities [OPTIONS] [PATTERN]...
```

List every commodity used.

| Flag         | Default | Description |
|--------------|---------|-------------|
| `--date`     | off     | Prefix each commodity with its first-seen transaction date; switch sort to chronological. Default sort is alphabetical. |
| `PATTERN...` | —       | Positional pattern filters. |

### `acc codes`

```
acc [GLOBAL OPTIONS] codes [PATTERN]...
```

List every transaction code observed.

| Arg          | Description |
|--------------|-------------|
| `PATTERN...` | Positional pattern filters. |

### `acc check`

```
acc [GLOBAL OPTIONS] check
```

Run all built-in consistency checks and report.

No flags. Current checks: `commodity-casing` (multi-char commodity
symbols must be all-uppercase; single-char symbols like `$` `€` `£`
are exempt).

### `acc navigate` (aliases `nav`, `ui`)

```
acc [GLOBAL OPTIONS] navigate [OPTIONS] [PATTERN]...
```

Interactive TUI. Live-filter the account tree as you type.

| Flag             | Default | Description |
|------------------|---------|-------------|
| `-E`, `--empty`  | off     | Include zero-balance accounts. |
| `PATTERN...`     | —       | Initial pattern filter. |

Key bindings:

| Key                  | Action                 |
|----------------------|------------------------|
| `↑` / `↓`            | Move cursor            |
| `Enter` / `Space`    | Toggle expand/collapse |
| `→`                  | Expand node            |
| `←`                  | Collapse node          |
| `PgUp` / `PgDn`      | Jump one page          |
| `Ctrl-u` / `Ctrl-d`  | Half page up / down    |
| `Home` / `End`       | First / last row       |
| Type letters         | Live filter            |
| `Backspace`          | Drop last filter char  |
| `Esc` / `Ctrl+C`     | Quit                   |

### `acc update`

```
acc update [OPTIONS]
```

Fetch exchange rates into `$ACC_PRICES_DIR`. Standalone — does not
read the journal.

| Flag                  | Default | Description |
|-----------------------|---------|-------------|
| `--pair BASE/QUOTE`   | —       | Trading pair to update. Repeat `--pair` for multiple pairs. If omitted, every existing crypto file under `$ACC_PRICES_DIR/crypto/` is continued from the day after its last cached entry. |
| `--since DATE`        | —       | Overwrite data from `DATE` onwards (`YYYY-MM-DD`). Conflicts with `--date`. |
| `--date DATE`         | —       | Fetch only this one date. Overrides `--since`. |
| `--daily`             | on      | Daily cadence (default). |
| `--monthly`           | off     | Fiat only: 1st of each month. Conflicts with `--daily`, `--yearly`, `--crypto`, `--pair`. |
| `--yearly`            | off     | Fiat only: Jan 1st of each year. Same conflicts as `--monthly`. |
| `--skip`              | off     | Fiat only: skip dates whose file already exists (no API call, no overwrite). Conflicts with `--crypto`, `--pair`. |
| `--crypto`            | off     | Crypto only. |
| `--fiat`              | off     | Fiat only. |

If neither `--crypto` nor `--fiat` is passed, both scopes run.

Incremental by default: without `--since` or `--date`, each existing
crypto pair resumes from the day after its last cached entry (only
the new days get fetched). Fiat behaves the same way — starts from
the day after the last cached file.

Output locations:

| Scope  | Path                                                        |
|--------|-------------------------------------------------------------|
| Crypto | `$ACC_PRICES_DIR/crypto/MEXC_{BASE}_{QUOTE}.ledger`         |
| Fiat   | `$ACC_PRICES_DIR/fiat/{YYYY-MM-DD}.ledger`                  |

### Environment variables

| Variable                    | Used by           | Description |
|-----------------------------|-------------------|-------------|
| `ACC_PRICES_DIR`            | main pipeline, `update` | Directory of rate files. When `-x` is set, `.ledger` files under it are auto-loaded before your own `-f` paths. `acc update` writes here. |
| `OPENEXCHANGERATES_API_KEY` | `update` (fiat)   | API key from [openexchangerates.org](https://openexchangerates.org). Required for fiat fetching. |

### Exit codes

| Code | Meaning                                                  |
|------|----------------------------------------------------------|
| `0`  | Success.                                                 |
| `1`  | Load failure (parse / resolve / book / IO error) or invalid CLI argument. Error message on stderr. |

---

## Examples

Visual walkthroughs of each command with realistic output.

Feature-focused, copy-paste-ready walkthroughs live in
[`examples/`](examples/) — one markdown file per topic (basics,
filters, currency conversion, fx gain/loss, CTA, lots and costs,
assertions), each with the journal inline and every command's
verbatim output.

### `bal` — hierarchical balances

```
$ acc -f demo.ledger bal
 $7441.80  assets
 $7441.80    checking
$-5000.00  equity
$-5000.00    opening
   $58.20  expenses
   $58.20    food
$-2500.00  income
$-2500.00    salary
---------
        0
```

```
$ acc -f demo.ledger bal --flat
 $7441.80  assets:checking
$-5000.00  equity:opening
   $58.20  expenses:food
$-2500.00  income:salary
```

```
$ acc -f demo.ledger bal ^assets
 $7441.80  assets
 $7441.80    checking
```

### `reg` — register with running total

```
$ acc -f demo.ledger reg
2024-01-01 initial balances   assets:checking    $5000.00   $5000.00
                              equity:opening    $-5000.00          0
2024-01-05 Groceries          expenses:food        $58.20      $58.20
                              assets:checking     $-58.20          0
2024-01-10 paycheck           assets:checking   $2500.00    $2500.00
                              income:salary    $-2500.00          0
```

### `print` — normalised vs raw

```
$ acc -f demo.ledger print
2024-01-01 (opening) initial balances
    assets:checking            $5000.00
    equity:opening            $-5000.00

2024-01-05 (42) Groceries
    expenses:food                $58.20
    assets:checking             $-58.20

2024-01-10 * paycheck
    assets:checking            $2500.00
    income:salary             $-2500.00
```

```
$ acc -f demo.ledger print --raw
2024-01-01 (opening) initial balances
    assets:checking           $5000.00
    equity:opening           $-5000.00

2024-01-05 (42) Groceries
    expenses:food              $58.20
    assets:checking

2024-01-10 * paycheck
    assets:checking           $2500.00
    income:salary            $-2500.00
```

### `accounts` — flat and tree

```
$ acc -f demo.ledger accounts
assets:checking
equity:opening
expenses:food
income:salary
```

```
$ acc -f demo.ledger accounts --tree
assets
  checking
equity
  opening
expenses
  food
income
  salary
```

### `commodities`

```
$ acc -f demo.ledger commodities
$
```

```
$ acc -f demo.ledger commodities --date
2024-01-01  $
```

### `codes`

```
$ acc -f demo.ledger codes
42
opening
```

### `check`

```
$ acc -f demo.ledger check
Scanned 3 transactions, 7 postings.

Checks:
  ✓ commodity-casing — multi-char commodity symbols must be all-uppercase

No issues found.
```

When an issue is found, the check is marked `✗` and the offending
locations are listed.

### Error output

Parse, resolve, and booker errors render in ledger-cli style with a
path + line reference, a headline, and the offending source
excerpt:

```
While parsing file "journal.ledger" at line 5:
>> unbalanced transaction

3 | 2024-03-15 * Coffee
4 |     expenses:food      $4.50
5 |     assets:cash        $1.00
```

Path and line are cyan, the headline red-bold, the excerpt in the
default colour. Colour auto-disables when stdout is not a TTY
(piping to a file stays clean).

---

## Journal format

A journal file is a sequence of transactions and directives.
Comments start with `;` or `#`.

### Transactions

```
DATE [STATE] [(CODE)] DESCRIPTION
    ACCOUNT  AMOUNT [COST] [= ASSERTION]
    ACCOUNT  AMOUNT
    ...
```

- `DATE`: `YYYY-MM-DD`. Other formats are rejected.
- `STATE`: `*` (cleared), `!` (pending), or omitted (uncleared).
- `(CODE)`: optional transaction code in parens. Empty `()` is
  tolerated as "no code".
- At least **two postings**. Postings must balance (sum to zero per
  commodity); one posting's amount may be omitted and acc infers
  it. In multi-commodity transactions with an ambiguous missing
  amount, acc expands that posting into one per commodity.

```
2024-03-15 * (42) Coffee
    expenses:food:coffee       $4.50
    assets:cash
```

### Amounts

Symbol placement is flexible — ledger-compatible variants are
accepted:

```
$100.00       $-100.00       -$100.00       100 USD       -100 USD
```

Thousands separators work:

```
assets:checking   $1,250,000.00
```

Parenthesised arithmetic expressions are evaluated at parse time:

```
income:monthly   (1200/12)    # = 100
expenses:bills   ((1+2)*3)    # = 9
```

Operators: `+ - * /` with standard precedence, unary minus,
parenthesised sub-expressions. Non-terminating divisions round.
An expression may reference at most one commodity; mixing
`1 EUR + 1 USD` in one expression is a parse error.

### Costs and lots

Cost annotations give multi-commodity transactions their conversion
basis:

```
assets:btc   BTC 0.5 @ $40000       # per-unit cost
assets:btc   BTC 0.5 @@ $20000      # total cost (same result)
```

Lot annotations record the acquisition basis of a held position so
sell-from-lot math works:

```
; acquire a lot
2023-06-01 buy
    assets:btc    BTC 0.1 {$30000}
    assets:cash  $-3000

; sell part of the lot at a higher price → gain
2024-06-01 sell
    assets:btc    BTC -0.05 {$30000} @ $40000
    assets:cash   $2000
    income:gain  $-500
```

`{COST}` = floating lot cost; `{=COST}` = fixed lot cost (pins it
so display semantics don't drift). The booker prefers lot cost
over `@`-cost for balance math. `{{TOTAL}}` (double-brace total)
and `[DATE]` (acquisition date) parse and are consumed for format
compatibility but are not modelled further.

### Virtual postings

- `(account)` — **paren-virtual**: does not participate in the
  transaction balance. Use for memo-only notations (e.g. tax
  allocation, budget bucket) that shouldn't offset a real account.
- `[account]` — **bracket-virtual**: does participate in the
  balance. Use when a "virtual" distinction exists at the reporting
  level (hidden by default from some reports) but the balance still
  matters.
- Plain account — real, counted in the balance.

### Balance assertions

```
2024-03-15 reconcile
    assets:bank     $0.00 = $4321.50
    equity:adjust
```

The `= $4321.50` asserts the account's running balance equals the
target after this posting. A mismatch halts with an error naming
the account, the expected amount, and the actual amount.

### Balance assignments

Same `=` syntax, but no amount on the posting — acc fills in
whatever brings the account to the target:

```
2024-03-15 reconcile
    assets:bank     = $4321.50
    equity:adjust
```

Useful for reconciling against a bank statement: write the ending
balance, let the tool figure out the delta.

---

## Filtering

Every report command accepts positional pattern arguments. Combined
with the global date flags, this is the query surface.

### Account patterns

Case-insensitive substring by default; `^` / `$` for anchors:

```
acc bal assets              # contains "assets" (case-insensitive)
acc bal ^assets             # starts with
acc bal checking$           # ends with
acc bal ^assets:checking$   # exact match
```

All filter dimensions — account, description, code, and commodity
— match case-insensitively. `com usd` matches a `USD` posting, and
`@Coffee` matches a transaction described as `coffee`.

### Pattern keywords

Reach other dimensions:

| Pattern      | Matches                                              | Short |
|--------------|------------------------------------------------------|-------|
| `desc TEXT`  | description contains `TEXT` (case-insensitive)       | `@TEXT` |
| `code VAL`   | transaction code equals `VAL` (case-insensitive)     | `#VAL`  |
| `com SYMBOL` | posting commodity equals `SYMBOL` (case-insensitive) | —     |

Commodity has no short prefix because `:` and `$` / `€` already
carry other meaning in ledger syntax.

### Combinators

```
acc reg not @coffee              # everything except coffee
acc reg com EUR and ^assets      # EUR postings in assets accounts
acc bal com USD or com EUR       # USD or EUR
```

Default between bare tokens is OR. Precedence is
`or` < `and` < `not`. Values with spaces need quoting:

```
acc reg @"coffee shop"
```

### Per-posting filtering

Postings that don't match are dropped from surviving transactions;
transactions that end up empty are removed. A transfer
`assets:usd +100 USD / assets:eur -85 EUR` filtered with `com EUR`
shows only the EUR leg.

### Date range: `-p`, `-b`, `-e`

All three accept `YYYY`, `YYYY-MM`, or `YYYY-MM-DD`.

```
acc -p 2024 bal                  # all of 2024
acc -p 2024-03 bal               # March 2024
acc -p 2024-03-15 bal            # single day
acc bal -b 2024 -e 2025          # 2024 only (exclusive end)
acc bal -b 2024-06               # from June 2024 onwards
```

`-p` conflicts with `-b`/`-e`.

### `--future` and `--sort`

- `--future`: include transactions dated after today. Hidden by
  default so forward-dated recurring entries (rent, subscriptions)
  don't clutter "what happened" reports.
- `--sort FIELD`: `date` (default), `amount`, `account`,
  `description`. Prefix with `-` for reverse. Repeat `--sort` for
  secondary / tertiary keys.

---

## Currency conversion

`-x TARGET` converts every amount into `TARGET` using the price DB.

### Default: per-posting conversion at `tx.date`

```
acc -f journal.ledger bal -x €
```

Each posting is converted using the latest `P` rate on or before
its transaction's own date. A $5 coffee from 2020 always shows as
its 2020 € equivalent, regardless of when the report runs. Reports
are historically reproducible — same journal + same rate files =
same result, forever.

### `--market [DATE]` for snapshot revaluation

For year-end statements, current portfolio value, etc. — opt in to
rolling valuation:

```
acc bal -x € --market               # rates as of today
acc bal -x € --market 2024-12-31    # rates as of year-end 2024
```

### Multi-hop

If no direct `P BASE QUOTE` rate exists, acc does BFS over the
commodity graph. `TOKEN → STABLECOIN → USD → EUR` resolves
transparently if the intermediate pairs exist. Inverse rates are
computed on demand, so a stored `USD/EUR` also serves `EUR/USD`.

### Missing rates

If no path exists between a posting's commodity and the target,
the posting stays in its original commodity. No error, just a
remainder visible in the report.

### `$ACC_PRICES_DIR`

When `-x` is set, every `.ledger` file under the directory the env
var points to is loaded before your own `-f` paths:

```
export ACC_PRICES_DIR=~/accounting/prices/
```

You can put both acc-fetched (`acc update`) and hand-written `P`
directives here. No-op when `-x` is absent.

### `fx gain` / `fx loss` realisation

Declare the two accounts:

```
account Equity:FxGain
    fx gain

account Equity:FxLoss
    fx loss
```

With `-x TARGET` and both accounts declared, acc converts every
posting of a multi-commodity transaction to the target at the
market rate on `tx.date` and sums them up. If the sum is non-zero,
the transaction's implied rate differed from the market rate — the
difference becomes the realised gain or loss, and acc injects a
paren-virtual posting to close it out: `fx gain` when the user
came out ahead of market, `fx loss` when behind. Differences
below the target's display precision are ignored.

**Example.** Target `€`, market rate `P 2024-06-15 USD EUR 0.90`.

```
2024-06-15 sold USD for EUR
    assets:usd  $-1000
    assets:eur   €920
```

At market rate `$1000` is worth `€900`, but the user got `€920` —
`€20` gain. acc adds:

```
    (Equity:FxGain)  €-20
```

Report on them directly:

```
acc bal Equity:FxGain Equity:FxLoss -x €    # total realised gains / losses
acc reg Equity:FxGain -x €                  # per-transaction breakdown
```

### `cta gain` / `cta loss` — Currency Translation Adjustment

This is the feature that makes acc IFRS-compliant for multi-currency
reporting. If you never report in a currency different from your
journal's native commodity, you don't need it. If you do, it is the
mechanism that prevents rate drift from distorting your balance
sheet.

#### The problem it solves

Per-posting conversion at `tx.date` is historically stable — that's
why it's the default — but it has a structural side effect: when a
transit account receives money in one currency and pays the same
amount out later, the account is empty in its native currency but
shows a non-zero residual in any other currency. The rate moved
between inflow and outflow, so the converted in-flow and the
converted out-flow don't cancel.

Concrete: receive `€10000` on 2024-01-15 (rate `EUR/USD = 1.10`,
so worth `$11000`), pay out `€10000` on 2024-06-15 (rate
`EUR/USD = 1.05`, so worth `$10500`). Account is empty in `€`, but
`-x USD` shows a `+$500` phantom. Nothing economically happened —
the money passed through — but the account looks like it gained
`$500`.

```
$ acc bal -x USD               # without cta accounts declared
  USD500.00 assets
  USD500.00   checking          ← phantom drift
  ...
```

This matters for:
- **Audit trails** — auditors expect transit accounts to reflect
  their real state.
- **Cross-period comparability** — the same flows should net out
  to the same balance regardless of reporting currency.
- **Tax and financial statements** — drift on asset accounts
  misrepresents where value actually sits and can trip compliance
  reviews.

#### What the standards require

| Framework | Reference | Key rule |
|-----------|-----------|----------|
| IFRS | **IAS 21** *The Effects of Changes in Foreign Exchange Rates* §§ 39–48 | Translation differences from applying different exchange rates to different account classes must be recognised in **other comprehensive income** (OCI), not in profit or loss. |
| US-GAAP | **ASC 830-30** *Foreign Currency Matters — Translation of Financial Statements* | Translation adjustments are accumulated in a separate component of equity called **Cumulative Translation Adjustment (CTA)**, never flowed through the income statement. |

Both standards codify the same outcome: the drift is real but it is
not an income event. It belongs on a dedicated equity account so the
income statement stays stable and the balance sheet stays honest.
Without a CTA account the drift sits on whatever transit account the
rate movement happened to touch, which violates both standards.

#### How to enable it

Declare two accounts — one for positive drift, one for negative —
exactly like the existing `fx gain` / `fx loss` pair:

```
account equity:cta:gain
    cta gain

account equity:cta:loss
    cta loss
```

Account names are your choice; the sub-directives are what acc
looks for. Both must be declared for the feature to activate. If
only one is declared, the translator phase is skipped.

#### What acc does

With both `cta gain` and `cta loss` declared and `-x TARGET` set,
acc walks every `(account, commodity)` group chronologically. For
every group whose native amounts sum to exactly zero over the
reporting period — the definition of a transit account — it tracks
running native and running target. At every zero-crossing of the
native balance where the running target is non-zero, a synthetic
transaction is emitted on that date:

```
<date> * translation adjustment
    [<transit-account>]    TARGET -drift
    [<cta-account>]        TARGET drift
```

Both postings are bracket-virtual (`[...]`) so they participate in
balance — the transit account's target sum is driven to zero — while
rendering as bracketed in the register to mark them as automatic
translator adjustments. Positive drift (target value lost while
holding native) routes to `cta loss`; negative drift (target value
gained) routes to `cta gain`.

#### The same example, with CTA

```
account equity:cta:gain
    cta gain
account equity:cta:loss
    cta loss

P 2024-01-15 EUR USD 1.10
P 2024-06-15 EUR USD 1.05

2024-01-15 * salary arrives
    assets:checking     €10000
    income:salary      €-10000

2024-06-15 * invoice paid
    expenses:services   €10000
    assets:checking    €-10000
```

```
$ acc bal -x USD
USD10500.00 expenses
USD10500.00   services
  USD500.00 equity
  USD500.00   cta:loss        ← drift booked explicitly
USD-11000.00 income
USD-11000.00   salary
```

`assets:checking` is gone from the balance (genuinely zero in both
currencies). The `$500` translation loss is named on
`equity:cta:loss` instead of silently sitting on the transit
account. The income statement (`income:salary`, `expenses:services`)
stays at its 2024 historical rates — no retroactive revaluation.

Run the register to see the automatic booking:

```
$ acc reg -x USD
2024-01-15 * salary arrives          assets:checking        USD11000.00
                                     income:salary         USD-11000.00
2024-06-15 * invoice paid            expenses:services      USD10500.00
                                     assets:checking       USD-10500.00
2024-06-15 * translation adjustment  [assets:checking]       USD-500.00
                                     [equity:cta:loss]        USD500.00
```

Auditable, reproducible, name-attributable.

#### Interaction with `--market`

`--market` converts every posting at one fixed date's rate. Under a
single rate, transit accounts net to zero in target automatically —
there is no drift to book. So the translator emits nothing under
`--market`. CTA materialises only in the default per-tx-date mode
where drift is structurally possible.

#### Interaction with `fx gain` / `fx loss`

The two mechanisms are complementary, never overlapping. The
realizer handles **multi-commodity transactions** where a user's
implied conversion rate diverges from the market rate — a realized
trading event. The translator handles **single-commodity transit
flows** where rate movement between inflow and outflow creates a
purely unrealized holding-period difference. acc tags
`(account, commodity)` groups touched by any multi-commodity
transaction as "realizer territory" and excludes them from CTA to
prevent double-booking.

#### Position in the plaintext-accounting ecosystem

As of this writing, acc is the only plaintext-accounting tool that
implements IAS 21 / ASC 830 translation adjustment automatically:

- **ledger-cli** and **hledger** default to single-rate valuation
  under `-V` / `-X`, which sidesteps the drift at the cost of
  historical income-statement stability. Neither tool has a CTA
  concept.
- **beancount** exposes `account_previous_conversions` and
  `account_current_conversions` options but does not populate them
  automatically — they require explicit invocation of
  `summarize.conversions()` at the user's discretion.
- **rustledger** carries the beancount option schema forward but
  the booking logic is not wired into the pipeline.

acc's default per-posting-tx.date conversion preserves IAS 21
rule (1) (historical income/expense). `--market` covers rule (2)
(current rate for monetary items). The `cta gain` / `cta loss`
pair covers rule (3) (translation differences to OCI / equity).
The three together close the loop.

---

## Rate updates (`acc update`)

Fetches daily rates into `$ACC_PRICES_DIR` from two sources:

- **MEXC klines** for crypto (no API key required)
- **openexchangerates.org** for fiat (needs
  `OPENEXCHANGERATES_API_KEY` in the environment; see
  [openexchangerates.org](https://openexchangerates.org) for sign-up
  — free tier covers typical personal use)

Files are stored at:

- Crypto: `$ACC_PRICES_DIR/crypto/MEXC_{BASE}_{QUOTE}.ledger`
  (one file per pair)
- Fiat: `$ACC_PRICES_DIR/fiat/{YYYY-MM-DD}.ledger`
  (one file per day with all currencies)

Rates are stored byte-for-byte as the API returned them — no
rounding, no `Rational` round-trip, no f64 lossy conversion.

### Examples

```
# Crypto: one pair at a time, repeat --pair for more
acc update --pair BTC/USDT
acc update --pair BTC/USDT --pair ETH/USDT
acc update --pair BTC/USDT --since 2024-01-01
acc update --pair BTC/USDT --date 2024-06-15

# Refresh every existing crypto pair in $ACC_PRICES_DIR/crypto/
acc update --crypto

# Fiat
acc update --fiat                       # daily since last file
acc update --fiat --monthly             # 1st of each month
acc update --fiat --yearly              # Jan 1st of each year
acc update --fiat --skip                # skip days already fetched
```

Running `acc update` alone (no scope, no `--pair`) continues every
existing crypto pair from the day after its last cached entry
**and** fetches fiat from the day after the last cached fiat file.
Both scopes run incrementally — no full re-download.

---

## Directives

acc recognises a minimal set of ledger directives.

### `commodity`

```
commodity $
    alias USD
    alias USDT
    precision 2

commodity €
    alias EUR
    precision 2

commodity BTC
    precision 8
```

Pick whichever symbol you write most often in postings as the
canonical one, and declare every alternative spelling via `alias`.
The parser normalises aliases to the canonical symbol at load time
so downstream code (balance, filter, price lookup) sees one form.

- `alias OTHER` — `OTHER` is normalised to the parent symbol. Repeat
  `alias` for multiple alternatives (e.g. `$` canonical, with `USD`
  and `USDT` both aliased to it).
- `precision N` — pins the display precision to exactly `N`
  fractional digits, overriding the observed-maximum default.
  Useful when a stray high-precision amount would otherwise force
  every report column to render with many decimals.

Commodity symbols are **case-sensitive**. `USD` and `usd` are two
different commodities — the balancer, the price DB, and the
aggregator all treat them as distinct. If a journal mixes cases
accidentally, declare the minority spelling as an `alias` so it
folds into the canonical form. Only filter patterns (`com usd`)
match case-insensitively, as a user-friendliness convenience.

### `account`

```
account Equity:FxGain
    fx gain

account Equity:FxLoss
    fx loss

account Equity:CTA:Gain
    cta gain

account Equity:CTA:Loss
    cta loss
```

Four semantically meaningful sub-directives:

- `fx gain` / `fx loss` — target accounts the realiser uses for
  realised FX gain/loss on multi-commodity transactions whose
  implied conversion rate diverges from the market rate. See
  [`fx gain` / `fx loss` realisation](#fx-gain--fx-loss-realisation).
- `cta gain` / `cta loss` — target accounts the translator uses
  for IAS 21 / ASC 830 Cumulative Translation Adjustment: the
  holding-period drift on single-commodity transit accounts when
  rates move between inflow and outflow. See
  [`cta gain` / `cta loss`](#cta-gain--cta-loss--currency-translation-adjustment).

Each sub-directive must be unique across the journal — declaring
two different accounts with `cta gain` is an error. Both halves of
a pair must be declared for their feature to activate.

### `P` — price

```
P 2024-06-15 USD EUR 0.92
P 2024-06-15 BTC USDT 63210.50
```

Date, base commodity, quote commodity, rate. The rate is
units-of-quote per unit-of-base. Populates the price DB that `-x`
queries.

### Comments

```
; line comment
# line comment
    ; indented comments attach to the preceding transaction / posting
```

### Not supported

acc has no silent-skip policy for directives it doesn't understand
— journals using any of the following will fail to load. Listed
here so ledger-cli migrants know what to strip or rewrite:

- `include` — multi-file journals compose via `-f DIR` (recursive)
  or multiple `-f PATH` arguments instead.
- `apply` / `end`, `define` — scope-block and macro directives.
- `D`, `Y`, `A`, `N` — short-form defaults.
- `tag`, `payee` — metadata directives.
- `~` blocks (periodic transactions) — syntax is rejected at the
  parser level.
- `=` blocks (automated transactions at the line-leading position)
  — the line-leading `=` rejects. Note: the posting-level `=` for
  balance assertions and assignments is unrelated and works fine.

---

## Philosophy

**Plain text, user-owned.** Journal files live where you put them,
edited with whatever editor you already use. No database, no
sync service, no lock-in. `git diff` is your audit log.

**Reproducible reports.** Same journal + same rate files produce
the same output today and a year from now. By default every
amount converts at its own transaction date's rate, not at "right
now" — last year's numbers don't silently shift every time the
report runs. `--market` opts into rolling revaluation when that's
what you want.

**`P` directives are the source of truth.** Unlike ledger-cli, acc
does not infer rates from the amounts of 2-commodity transactions.
Inferred rates reflect fees, rounding, and split executions rather
than quotable market prices; letting them into the price DB means
unrelated transactions perturb every report.

**Pure pipeline.** The parser is pure (no I/O, no shared state),
which lets file parsing run in parallel across thousands of files.
Each later phase has a single responsibility and its own unit
tests. Multi-thousand-file journals load in seconds.

**Own codebase, own decisions.** acc is in the ledger family but
not a clone. Where a convention from ledger-cli or hledger serves
the design, acc adopts it. Where the design calls for something
else — per-posting `tx.date` conversion, strict `P`-directive
semantics, phase-scoped typed errors — acc takes the different
path.

---

## Influences and relation to related tools

### ledger-cli (John Wiegley, C++) — direct inspiration

The original. acc takes the journal format from ledger and
continues its CLI-first approach.

What acc borrows: the file format itself, the core reports, `@` /
`@@` cost annotations, lot annotations, virtual postings, balance
assertions and assignments, the `P` directive.

Where acc diverges deliberately: rates come only from explicit `P`
directives (no inference from 2-commodity transactions),
conversion happens per posting at each transaction's own date by
default, errors carry typed per-phase context.

### hledger (Simon Michael, Haskell) — inspiration for discipline

hledger grew out of ledger-cli with a stricter parser, better
errors, a web UI, and a CSV rule-based importer.

What acc borrows in spirit: typed errors per phase, inline unit
tests, refusing to paper over ambiguous inputs with silent
heuristics.

Where acc is its own thing: no CSV importer, no web UI, smaller
surface area, Rust toolchain instead of Haskell.

### beancount (Martin Blais, Python) — adjacent ecosystem

beancount isn't a ledger-format tool — it has its own syntax with
typed accounts, stricter lot handling, a SQL-like query language
(BQL), and a plugin ecosystem. The two tools don't read each
other's journals.

What acc looks at for ideas: lot-tracking semantics, explicit
account declarations.

Where acc stays separate: the ledger format (not beancount), the
ledger-family ergonomics (terse, editor-friendly).

### What this means in practice

- Journal in the ledger format: acc reads it, within the scope
  documented under [Directives](#directives).
- Journal in hledger's extended format: common subset works; some
  hledger-specific extensions may not.
- Journal in beancount: acc doesn't read it.
- Need periodic transactions that fire, value expressions, budget
  reports, a CSV importer, or BQL: out of scope for acc; the tools
  above cover those.

---

## FAQ

### Why plaintext instead of a database?

Plaintext files are portable, editable with any editor, and work
with every version-control tool. You can read them a decade from
now without needing the original program. No vendor lock-in, no
migration pain.

### Why does a $5 expense from 2020 show different € values under ledger-cli and acc?

ledger-cli converts every posting using the rate as of the *report
date*. acc converts each posting using the rate as of its own
*transaction date*. So a 2020 expense re-prices under ledger-cli
whenever exchange rates move; under acc it stays fixed at the 2020
rate forever.

If you want ledger-cli-style rolling revaluation, use `--market`:

```
acc bal -x € --market               # rates as of today
acc bal -x € --market 2024-12-31    # rates as of a specific date
```

### Can acc read my hledger or beancount journal?

Hledger: mostly yes, for the common subset of the ledger format.
Hledger-specific extensions may not parse. The `include`,
`apply/end`, etc. directives are not supported either way.

Beancount: no. Beancount uses a different format.

### `@` vs `@@` — what's the difference?

`@` is per-unit cost, `@@` is total cost. Both describe the same
transaction, just in different numeric form:

```
assets:btc   BTC 2 @  $40000    # 2 BTC × $40,000 each = $80,000
assets:btc   BTC 2 @@ $80000    # 2 BTC for $80,000 total
```

Balance math uses whichever you wrote; both resolve to the same
effective amount on the cost side.

### When do I use virtual postings?

- `(account)` (paren-virtual) — posting is **not** balanced by the
  tool. Use when you want a note attached to the transaction that
  doesn't offset any real account (e.g. budget bucket allocation,
  tax category marker).
- `[account]` (bracket-virtual) — posting **is** balanced. Use
  when you want a separate accounting view that hides from some
  reports but still balances (e.g. unrealised gains, internal
  allocations).
- Plain accounts — the default; everything balances.

### How do I see realised FX gain/loss?

Declare `fx gain` and `fx loss` accounts (see [Currency
conversion](#currency-conversion)) and run with `-x`:

```
acc bal Equity:FxGain Equity:FxLoss -x €
```

The realiser automatically injects the gain/loss postings for
multi-commodity transactions whose implied rate diverges from the
market rate.

### Does acc write to my journal files?

No. Your journal is read-only from acc's perspective. The only
thing that writes is `acc update`, and only to `$ACC_PRICES_DIR`.

### Does acc make network calls?

Only `acc update`, and only to the configured APIs (MEXC for
crypto, openexchangerates.org for fiat). No telemetry, no
analytics, no background traffic.

### How do I compose a multi-file journal?

Either `-f` a directory:

```
acc -f ~/accounting/ bal
```

Or list files explicitly:

```
acc -f 2023.ledger -f 2024.ledger -f prices.ledger bal
```

There's no `include` directive; `-f` accepting directories covers
the same use case.

### How do I report a bug or suggest a feature?

Open an issue at
<https://github.com/rudolfschmidt/acc/issues>. Include the
`acc --version` output and a minimal reproducing journal snippet
if possible.

### Where do I see changes over time?

[CHANGELOG.md](CHANGELOG.md) has the project's development
history. For your own journal, use `git log` — every journal
should be in version control.

---

## Contributing

Bug reports, patches, and feature discussion are welcome at
<https://github.com/rudolfschmidt/acc>.

Local development:

```
git clone https://github.com/rudolfschmidt/acc
cd acc

cargo build --release           # build the binary
cargo test                      # run the full test suite (unit + integration)
cargo run -- -f demo.ledger bal # try a build against the bundled demo
```

The test suite is structured as:

- `src/**/mod.rs` `#[cfg(test)]` — per-phase unit tests on inline
  input strings
- `tests/pipeline.rs` — end-to-end happy-path tests via
  `acc::load()`
- `tests/errors.rs` — failure-mode tests asserting `LoadError`
  variants
- `tests/lot_and_expression.rs` — lot annotations, expressions,
  multi-commodity split
- `tests/conversion.rs` — `-x`, `--market`, inverse + multi-hop
  rebalance

Before sending a patch, please `cargo test` and `cargo clippy` locally.

---

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

GPL-3.0. See [LICENSE](LICENSE).
