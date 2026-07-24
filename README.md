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
 $7441.80 assets
 $7441.80   checking
$-5000.00 equity
$-5000.00   opening
   $58.20 expenses
   $58.20   food
$-2500.00 income
$-2500.00   salary
---------
        0
```

```
$ acc -f journal.ledger reg
2024-01-01 initial balances  assets:checking   $5000.00  $5000.00
                             equity:opening   $-5000.00         0
2024-01-05 Groceries         expenses:food       $58.20    $58.20
                             assets:checking    $-58.20         0
2024-01-10 * paycheck        assets:checking   $2500.00  $2500.00
                             income:salary    $-2500.00         0
```

The repo ships the journal above at
[`examples/journal.ledger`](examples/journal.ledger) so you can
clone and run without copy-pasting:

```
git clone https://github.com/rudolfschmidt/acc
cd acc
cargo run -- -f examples/journal.ledger bal
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
(Linux, macOS, Windows, BSDs). On **Linux/BSD** the build needs a system
**OpenSSL** (HTTPS for `acc update` goes through native-tls); macOS and
Windows use the OS-native TLS stack, so no extra dependency there.

**What gets written where:** report commands never touch disk. Writing
is always explicit and opt-in — `acc format` (in-place alignment),
`acc rename -e` (account renames), `acc import -e` (append to a `@cash`
file), and `acc update` (rate files under `$PRICES`). Network I/O
happens only in `acc update` (to MEXC and openexchangerates.org).

**Shell completion.** `acc completions <shell>` prints a completion
script for subcommands, flags, and file paths. Enable it for the current
shell by sourcing what it prints — for zsh, add to `~/.zshrc`:

```
source <(acc completions zsh)
```

`bash`, `fish`, `elvish` and `powershell` work the same way. Paths are
completed by the shell's own file completion, so `~` expands natively;
regenerate only when acc's CLI changes.

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
`commodities`, `codes`, `lint`, interactive `navigate`, `update`
(rate fetching), `format` (in-place journal formatter with
source-preserving amount pass-through), `diff` (source-level
ledger-aware file / tree comparison); transactions with states,
codes, arithmetic expressions in amounts, `@` / `@@` cost
annotations, `{COST}` lot annotations, virtual postings, balance
assertions and assignments; directives `commodity` (with `alias`,
`precision`), `account` (with `slippage gain` / `slippage
loss` / `holding gain` / `holding loss` / `cta gain`
/ `cta loss` / `capital gain` / `capital loss` / `label`), `P`, and
ledger-style **automated transactions**
(line-leading `= /pattern/` rules that inject scaled postings
into matching transactions, with `$account` / `$segment`
placeholders, plus named `= NAME :: /pattern/` templates
instantiated per pair with positional `$1` / `$2` args, a `= NAME[key] :: value`
lookup table, and an `amount <op> N` rule clause); filter DSL across account /
description / code / commodity plus `-r` sibling-posting view;
per-posting currency conversion at `tx.date`; multi-hop price
lookups; **FIFO realised capital gains** (`capital gain` /
`capital loss`) — the disposed lot's holding-period market move,
composed under `-X` with the per-trade execution spread on `slippage
gain` / `slippage loss`; **opt-in mark-to-market**
(`-V` / `--unrealized`) revaluing open foreign balances to the
latest rate on `holding gain` / `holding loss`;
**automatic IAS 21 / ASC 830 translation adjustment** (CTA) for
same-commodity transit accounts; `-R` real-only output.

**Not in scope today:** `include` directive, `apply/end`, the
short-form directives `D` / `Y` / `A` / `N`, `tag`, `payee`, a general
value-expression language — including the `= ... and expr "..."` conditional
form of automated transactions (a *restricted* `= NAME[key] :: value` lookup
table and an `amount <op> N` rule clause are supported instead), CSV import,
query language, budget reports, web UI.

Journals using any of those will fail to load — acc has no
silent-skip policy for directives it doesn't understand.

Some of the list is permanently out of scope (CSV import,
BQL-style queries, web UI — adjacent tools cover those). Some
might land later (a few of the short-form directives).

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
| **(1) Income & expense** | Translate at the rate of each transaction (or period average). Must not revalue retroactively — quarterly and annual comparisons would break. | Default: per-posting conversion at `tx.date`. A 2020 expense stays at its 2020 `$`-value forever under `-X $`. |
| **(2) Monetary balance items** | Cash, receivables, payables are shown at the **current rate** at the report date — what's in the account is worth what it's worth today. | **Opt-in** via `-V` / `--unrealized`: open foreign balances are marked to the latest available rate, the unrealized revaluation booked to `holding`. Off by default — the default values historically (rule 1) and resolves realized transit drift via CTA (rule 3), so period comparisons stay stable. |
| **(3) Cumulative Translation Adjustment (CTA)** | The difference arising from applying different rates under (1) vs (2) is booked to a dedicated equity account under Other Comprehensive Income. | Implemented: declare `cta gain` / `cta loss` accounts. See [`cta gain` / `cta loss`](#cta-gain--cta-loss--commodity-translation-adjustment). |

### Why this matters — and how acc differs

**ledger-cli** and **hledger** default to *one rate for everything*
at the report date. Simple, but violates rule (1): a 2020 expense
shows a different value every time exchange rates move. Reports
across periods become incomparable. Neither tool implements CTA.

**beancount** has the `account_previous_conversions` option
(inherited into rustledger), but the automatic booking to the CTA
account is not wired up — it remains a manual post-processing
step in both tools.

**acc values historical-per-transaction**, which preserves
income/expense stability (rule 1) and matches the temporal method
of IAS 21. It deliberately does not offer a report-date snapshot
mode for rule (2); instead `cta gain` / `cta loss` resolves the
valuation difference on transit accounts to equity under rule (3).
**acc is the first plaintext-accounting tool that books IFRS IAS 21
commodity translation adjustments automatically** — the other tools
either skip drift by collapsing to a single rate (losing historical
stability) or carry the option in their schema without wiring up the
booking.

### Professional focus, no ceremony

acc is deliberately not a hobby budget tool. Reports are meant to
be auditable, reproducible, and consistent with how real accounting
is done. Where correctness requires a concept from IFRS or GAAP
(CTA, temporal method, FIFO cost-basis preservation and realised
capital gains via `{cost}` lot annotations), acc adopts it — not as
boilerplate, but because the
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
| `-f`, `--file PATH`        | —       | Journal file or directory. A file named explicitly is read whatever its extension. Directories are walked recursively for journal files (only `.ledger`); use a `_` suffix (`foo.ledger_`) to keep a file in the tree but skip it. Repeat `-f` for multiple sources (order preserved). Works at any position — before or after the subcommand. `-f -` reads from stdin — only with `print --raw`; other commands silently ignore it. |
| `-b`, `--begin DATE`       | —       | Include transactions on or after `DATE`. Accepts `YYYY`, `YYYY-MM`, or `YYYY-MM-DD` — each picks the *start* of the specified period. Conflicts with `-p`. |
| `-e`, `--end DATE`         | —       | Include transactions strictly before `DATE` (exclusive). Same grammar as `-b`. Conflicts with `-p`. |
| `-p`, `--period PERIOD`    | —       | Shorthand spanning a full period. `YYYY` = year, `YYYY-MM` = month, `YYYY-MM-DD` = single day. Repeat `-p` to include multiple discrete periods — a transaction is kept if it falls within any. Conflicts with `-b` / `-e`. |
| `--future`                 | off     | Include transactions dated after today. Hidden by default (rent, subscriptions, recurring forward-dated entries shouldn't clutter "what has happened" reports). When also using `-e` / `-p`, the earlier cutoff wins. |
| `-S`, `--sort FIELD`       | `date`  | Sort key: `date` (alias `d`), `amount` (`amt`), `account` (`acc`), `description` (`desc`, `payee`). Prefix with `-` for reverse (`--sort -amount`). Repeat `--sort` for secondary / tertiary keys. Unknown fields silently fall back to `date`. |
| `-X`, `--exchange SYMBOL`  | —       | Convert every amount into `SYMBOL` using the price DB. Each posting is converted at its own `tx.date` rate. |
| `-V`, `--unrealized`       | off     | Mark-to-market: revalue open foreign-currency balances at the latest available rate instead of the historical per-posting valuation, booking the unrealized revaluation to `holding gain` / `holding loss`. Only meaningful with `-X`, and only when those accounts are declared. The default stays historical (realized only). `-V` reuses the letter ledger spends on market valuation, here for acc's opt-in unrealized revaluation. |
| `-R`, `--real`             | off     | Drop virtual postings from the output (both `(account)` paren-virtual and `[account]` bracket-virtual). The realizer, lotter and translator inject *real* postings (slippage/unrealized, capital gain/loss, CTA), so `-R` keeps them; only the `(…)` / `[…]` postings written in the source journal are removed. |
| `-r`, `--related`          | off     | Show the *other* postings of matched transactions — the counter-parties — instead of the match itself. `acc reg ^expenses -r` shows which accounts balanced against expenses. Relates to the whole query: a posting is "matched" when it satisfies the positional pattern **and** the sign / `--amount` filters together, so `acc reg -A '>100' -r` shows the counter-parties of the large postings, not the large postings themselves. Modeled on ledger-cli's `--related`. |
| `--related-all`            | off     | Show *every* posting of a matched transaction — the matched posting **and** its counter-parties — not just the counter-parties (`-r`) or just the match (default). Modeled on ledger-cli's `--related-all`. |
| `--pos`                    | off     | Show only postings whose amount is `>= 0`. A secondary filter applied *after* selection — it narrows which postings show, by sign, and composes with `--related-all`. A zero amount counts as both signs, so it shows under `--pos` and `--neg`. |
| `--neg`                    | off     | Show only postings whose amount is `<= 0`. The negative counterpart of `--pos`; zero amounts show under both. |
| `-A`, `--amount EXPR`      | —       | Show only postings whose *signed* amount matches `EXPR`: an optional operator (`>`, `<`, `>=`, `<=`, `=`, `<>`) followed by a number, e.g. `-A '>100'` (above 100), `-A '<=-50'` (at most −50), `-A '=0'` (exactly zero), `-A '<>0'` (every non-zero amount). A bare number means `=`. Like `--pos` / `--neg` it narrows the postings after selection, and it feeds `-r` (see `--related`). |
| `-d`, `--display PATTERN`  | —       | Show only postings whose account matches `PATTERN`, *after* transaction selection — the positional pattern picks which transactions, `-d` picks which of their postings. Runs on the full posting set, so `--related-all` isn't needed: `acc reg ^assets:vendor -d ^ex` shows the expense postings of the vendor transactions. Account-only: `^acc` (starts-with), `acc$` (ends-with), `^acc$` (exact), `acc` (substring); case-insensitive. The `reg` running total sums only the shown postings — unlike ledger's `-d`, which keeps hidden postings in the total. |
| `--commodities N`          | —       | Keep only transactions whose balance-contributing postings use at least `N` distinct commodities; paren-virtual `(account)` postings are skipped. `--commodities 2` finds every currency-mixing transaction. |
| `--mixed`                  | off     | Alias for `--commodities 2`: keep only transactions that mix at least two commodities. |
| `-h`, `--help`             | —       | Print help. Works on `acc` and every subcommand. |
| `-v`, `--version`          | —       | Print version and exit. (Lower-case — `-V` is `--unrealized`.) |

Running `acc` with no subcommand prints help.

### `acc balance`

```
acc [GLOBAL OPTIONS] balance [OPTIONS] [PATTERN]...
```

Account balances, grouped hierarchically by default. Accounts declared
with a [`label`](#account) show it dimmed after the name (`1000 (foo)`).

| Flag               | Default | Description |
|--------------------|---------|-------------|
| `--flat`           | off     | One line per account, no tree indentation. Conflicts with `--tree`. |
| `--tree`           | on      | Hierarchical tree (default unless `--flat`). |
| `-E`, `--empty`    | off     | Include zero-balance accounts (default: hidden). |
| `PATTERN...`       | —       | Positional account-name patterns. See [Filtering](#filtering). |

Example output see the [Examples](#examples) section below.

### `acc register`

```
acc [GLOBAL OPTIONS] register [PATTERN]...
```

Transaction-by-transaction register with per-commodity running total.
Accounts declared with a [`label`](#account) (or `label-register`) show
it dimmed inline after the labelled segment (`assets:1000 (foo):sub`).

| Arg            | Description |
|----------------|-------------|
| `PATTERN...`   | Positional pattern filters. |

Example output:

```
$ acc -f journal.ledger reg
2024-01-01 initial balances  assets:checking   $5000.00  $5000.00
                             equity:opening   $-5000.00         0
2024-01-05 Groceries         expenses:food       $58.20    $58.20
                             assets:checking    $-58.20         0
2024-01-10 * paycheck        assets:checking   $2500.00  $2500.00
                             income:salary    $-2500.00         0
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

### `acc lint`

```
acc [GLOBAL OPTIONS] lint [RULE] [--base DIR] [--categories PREFIX...] [--fix [-e]]
```

Lint the journal: run all built-in consistency checks and report any
issues as warnings (never a hard failure). Each check reports `✓` (clean),
`✗` (issues found), or `!` (skipped — not runnable without more config).

| Flag                     | Default | Description |
|--------------------------|---------|-------------|
| `RULE`                   | all     | Run just one check by its reported id: `commodity-casing`, `leaf-accounts`, `role-references`, or `dir-category`. Omit to run all. |
| `--base DIR`             | `$BASE` | Run the `dir-category` check: every transaction whose file lives in a direct sub-directory of `DIR` must categorise into that directory. `@…` directories and files directly in `DIR` are exempt. The sub-directory is found relative to `DIR`, so it works however the files were loaded (`-f .` from inside the folder, `-f subdir`, or the whole tree). Falls back to the `$BASE` environment variable when the flag is omitted. |
| `--categories PREFIX...` | off     | Account prefixes that count as *categories* (income / expense), e.g. `--categories '^in:' '^ex:'` (a leading `^` is optional). `dir-category` then checks *every* posting whose account starts with one of these — each such category account must *end with* the folder's name as segments (`food-groceries` → `…:food:groceries`). A transaction with no category posting (a pure transfer) is skipped. Without `--categories`, `dir-category` can't tell a category account from a transfer, so it is skipped with a `!` warning. |
| `--fix`                  | off     | Preview the fixes for auto-fixable checks (currently only `dir-category`): each `old → new` account rewrite, writing nothing. Checks without a fixer still report. |
| `-e`, `--execute`        | off     | Apply the `--fix` rewrites in place (atomic per file). Requires `--fix`. |

Checks: `commodity-casing` (multi-char commodity symbols must be
all-uppercase; single-char symbols like `$` `€` `£` are exempt),
`leaf-accounts` (postings must target leaf accounts, never a parent that
has sub-accounts), `role-references` (every `$role:slot` reference must
resolve to a declared account), and — with `--base` **and**
`--categories` — `dir-category` (a category account's tail must match its
folder). `dir-category` is auto-fixable: `lint dir-category --fix` previews
the account rewrites, `-e` applies them.

`lint` validates the **whole** journal on the source postings — it never
hides forward-dated entries the way reports do (no `--future` needed), and
it runs before enrichment, so it only ever flags what you actually wrote,
not the synthetic postings the pipeline injects under `-X`.

### `acc format`

```
acc format [OPTIONS] [PATHS]...
```

Reformat one or more ledger journal files: account column
left-aligned, amount column right-aligned. Everything after the
amount (`@` cost, `{…}` lot, `= assertion`, `; comment`) passes
through 1:1 from the source line — expressions like
`(USD 1200/12)` are never re-evaluated, so no precision drift.
Commodity symbol and number are glued together (`USD -100` →
`USD-100`). Only the parser runs, so journals with unbalanced
transactions still format.

| Flag          | Default | Description |
|---------------|---------|-------------|
| `--sort`      | off     | Stable date-sort the transactions. Off by default: source order is preserved exactly, so formatting only ever touches whitespace and never reorders your entries. With `--sort`, same-day events keep their original relative position. |
| `--infer`     | off     | On a two-posting transaction whose postings share a commodity, drop the second posting's amount and let it auto-balance — it's just the negation of the first, so writing it is busywork. (Ledger calls the amount-less leg the *null posting* and "infers" its amount; hence the name.) |
| `--fill`      | off     | The inverse of `--infer`: on a transaction with *more* than two postings sharing a commodity and exactly one amount omitted, compute that amount (the negated sum of the rest) and write it out — the balancing leg is no longer obvious there. Together the two canonicalise a journal: trivial balances elided, non-trivial ones spelled out. Both leave multi-currency legs, costs, lots, assertions and virtual postings untouched. |
| `PATHS...`    | —       | Files or directories. Files named explicitly are formatted whatever their extension; directories are walked recursively for journal files (only `.ledger`). Pass `-` to read from stdin and write to stdout (for editor pipes); no other path flag is valid in that mode. |

A comment block (e.g. a commented-out transaction) is surrounded by a
blank line so it stays visually separate from neighbouring entries —
except at the very start or end of the file, where no extra blank line
is added.

Writes atomically via a `.tmp` + rename, so a crash mid-write
never leaves a half-written file.

### `acc diff`

```
acc diff [--snapshot DIR] <PATHS>...
```

Compare two ledger files or directory trees at the source level,
ignoring all whitespace differences. Output follows `git diff`
conventions: `--- OLD` / `+++ NEW` headers, `@@ -line,count
+line,count @@` hunk markers, `-` / `+` prefixed lines, 3 lines
of surrounding context per change block.

| Flag                 | Description |
|----------------------|-------------|
| `--snapshot DIR`     | Snapshot-root mode. acc resolves each positional path, then walks its components right-to-left and pairs it against the longest suffix that exists under `DIR`. Use this when your backups preserve the working-tree layout under a timestamped root — you no longer have to type the full nested path into the snapshot. With no positional argument, the current directory is used. |
| `PATHS`              | Without `--snapshot`: exactly two paths (`OLD NEW`). With `--snapshot`: one or more working-side paths (each resolved against the snapshot root). |

Exit `0` on clean match, `1` on any difference or missing
counterpart file.

Four invocations to illustrate the modes:

```
# Explicit, two files: compare one journal against another.
acc diff journal.ledger journal.ledger.bak

# Explicit, two directories: walk both trees recursively, pair
# journal files (only `.ledger`) by relative path, diff each pair.
# Files present on only one side are reported as
# `- only in OLD` / `+ only in NEW`.
acc diff ~/journals /path/to/backup

# Snapshot, single file: acc finds the matching path inside the
# backup tree by longest-suffix match — no need to type the
# full nested path.
acc diff --snapshot /path/to/backup journal.ledger

# Snapshot, whole working tree: `.` (or omitted) resolves to the
# current directory and the entire subtree is matched against the
# snapshot. Common usage from the working-tree root.
cd ~/journals
acc diff --snapshot /path/to/backup .
```

Both files and directories work in either mode. The snapshot form
saves you from typing the tree path twice and works regardless of
where in the working tree you stand.

### `acc sweep`

```
acc [GLOBAL OPTIONS] sweep <ACCOUNT> <SEGMENT> <INCOME> <EXPENSE>
```

Close the open balance of a pass-through (clearing) account by
generating offsetting entries. Conceptually `reg ACCOUNT`: sweep pairs
equal-and-opposite amounts on the account across the whole account (per
commodity, over all dates), and for every posting that stays open it
emits one offsetting entry — at that posting's date — that brings the
account back to zero. A debit remainder (`> 0`) books to
`EXPENSE:SEGMENT`, a credit remainder (`< 0`) to `INCOME:SEGMENT`. Each
entry is marked cleared (`*`) and titled with the account's last segment.

| Argument   | Description |
|------------|-------------|
| `ACCOUNT`  | The pass-through account to close — a filter pattern (e.g. `^assets:clearing$`). |
| `SEGMENT`  | Appended after the income / expense account (`INCOME:SEGMENT`). |
| `INCOME`   | Account used when the remainder is a credit (`< 0`). |
| `EXPENSE`  | Account used when the remainder is a debit (`> 0`). |

All four arguments are required. The offsetting entries are printed to
**stdout**, already aligned and date-sorted (formatted in memory); the
status line goes to stderr. Where they land is up to the caller —
redirect or append, e.g. `acc sweep … >> cash.ledger`.

**Idempotent and file-agnostic.** Because the generated offsets are
part of the loaded journal, each posting cancels against its offset on
the account, so re-running only closes newly-opened postings — never
duplicates. It does not matter which file an offset lives in, so it is
safe to append the output wherever you like (and move it later). A
genuine round-trip —
an invoice one day, its payment weeks later — cancels the same way and
is left alone; only real open balances are swept. Pairing happens
within the same date first, then across dates, so an offset removed for
one date is re-pulled at that very date even when same-amount postings
elsewhere are still settled.

```
# Close everything sitting on assets:clearing into income / expenses,
# under the `misc` segment, appending the result to a file.
acc -f journal.ledger sweep '^assets:clearing$' misc income expenses >> clearing.ledger
```

### `acc rename`

```
acc [GLOBAL OPTIONS] rename <OLD> <NEW> [-e]
```

Rename an account across the loaded `-f` files. `OLD` is matched with the
same anchors as the report filter: a bare pattern matches **anywhere**
(`contains`), a leading `^` anchors it to the **start** of the account, a
trailing `$` to the **end**, and `^…$` an exact account. The matched span
is rewritten to `NEW` and the rest of the account name is preserved.

So `rename foo:5 foo:4` (contains) renumbers every account containing
`foo:5` — `foo:5`, `foo:50`, `bar:foo:5:cash`, … — in one go, while
`rename ^foo:5 foo:4` only touches accounts that *start* with `foo:5`.

| Argument           | Description |
|--------------------|-------------|
| `OLD`              | Account pattern to rename — `^` anchors the start, `$` the end, otherwise it matches anywhere. |
| `NEW`              | Replacement for the matched span. |
| `-e`, `--execute`  | Apply the rename in place. Without it, only a preview is printed. |

**Safe and surgical.** Each file is parsed, so only real *posting*
accounts are touched — `account` directives, auto-rule patterns, comments
and descriptions are never rewritten. Only the account token on a matched
line changes; the rest of every file stays byte-for-byte identical. A
file that fails to parse is reported and skipped, never edited; writes
are atomic (temp file + rename).

**Preview by default.** `acc rename OLD NEW` prints every `file:line`
that would change (`old → new`) and writes nothing; add `-e` to apply.

```
# Preview renumbering the 5-block to the 4-block across the journal.
acc -f journal.ledger rename foo:5 foo:4
# Apply it.
acc -f journal.ledger rename foo:5 foo:4 -e
# Only rewrite accounts that *start* with foo:5 (anchored).
acc -f journal.ledger rename '^foo:5' foo:4
```

### `acc navigate`

```
acc [GLOBAL OPTIONS] navigate [OPTIONS] [PATTERN]...
```

Interactive TUI. Live-filter the account tree as you type. Each row shows
its balance in a right-aligned amount column, and — like `reg` — any
account label inline as ` (label)`.

| Flag             | Default | Description |
|------------------|---------|-------------|
| `-E`, `--empty`  | off     | Include zero-balance accounts. |
| `PATTERN...`     | —       | Initial pattern filter. |

Key bindings:

| Key                  | Action                     |
|----------------------|----------------------------|
| `↑` / `↓`            | Move cursor                |
| `Enter` / `Space`    | Toggle expand/collapse     |
| `→`                  | Expand node                |
| `←`                  | Collapse node              |
| `Tab`                | Fold / unfold the whole tree |
| `PgUp` / `PgDn`      | Jump one page              |
| `Ctrl-u` / `Ctrl-d`  | Half page up / down        |
| `Home` / `End`       | First / last row           |
| Type letters         | Live filter                |
| `Backspace`          | Drop last filter char      |
| `Esc` / `Ctrl+C`     | Quit                       |

### `acc update`

```
acc update [OPTIONS]
```

Fetch exchange rates into `$PRICES`. Standalone — does not
read the journal.

| Flag                  | Default | Description |
|-----------------------|---------|-------------|
| `--pair BASE/QUOTE`   | —       | Trading pair to update. Repeat `--pair` for multiple pairs. If omitted, every existing crypto file under `$PRICES/crypto/` is continued from the day after its last cached entry. |
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
| Crypto | `$PRICES/crypto/MEXC_{BASE}_{QUOTE}.ledger`         |
| Fiat   | `$PRICES/fiat/{YYYY-MM-DD}.ledger`                  |

### `acc import`

```
acc import [<CSV>] -c <PROFILE> [--execute]
```

Convert a bank's CSV export (or a coin wallet's RPC feed) into ledger
transactions via a profile and append them to a cash-account file.
Standalone — it does not read the journal (only the target file, for
de-duplication).

| Flag                  | Default | Description |
|-----------------------|---------|-------------|
| `[<CSV>]`             | —       | The CSV export to import. Omit for RPC-source profiles (below), which pull from a wallet daemon instead of a file. |
| `-c`, `--conf FILE`   | —       | The per-bank import profile (required). |
| `-e`, `--execute`     | off     | Execute the import — append the new transactions to the target file. Without it a dry-run prints the additions as a diff and writes nothing. |

The profile (`<bank>.conf`) maps the CSV columns and shapes the output;
only the bank-specific bits are configured — standard CSV defaults
(delimiter, UTF-8, ISO dates, dot decimals) are assumed:

```
field.date 0           # CSV column indices
field.payee 2
field.amount 7
output.file path/to/checking.ledger
output.account assets:bank
output.commodity €
commodities path/to/commodities.ledger   # reuse symbols + precision
identity date amount payee                # what makes a row unique
default => expenses:{payee}               # fallback counter account
payee SUPERMARKET => expenses:groceries   # override rule
```

The counter account defaults to the payee slugified (lowercased, spaces
→ dashes); rules override only where that's wrong. A rule is `<field>
<value> => <account>`, matching a column case-insensitively. Like the
report filter, `<value>` matches as a **substring** by default, a leading
`^` anchors the **start**, a trailing `$` the **end**, and `^…$` the whole
field — so `payee ^SUPERMARKET` matches only payees beginning with
`SUPERMARKET`. Combine conditions on one line with `;` (AND), separate
lines for OR.

**Internal transfers.** A movement between two of *your own* accounts can
be booked to a directional in-transit account instead of a payee, so the
two legs — one from each account's export — net to zero once both are
imported; a non-zero balance then means money is still in flight. Declare
this account's own in-transit identity and map each partner IBAN to the
other account's name:

```
transit.field iban                     # which field holds the partner IBAN
transit.self  assets:transit:checking  # this account: prefix + own name
transit XX00…  savings                 # partner IBAN => other account's name
```

The counter account is built as `<prefix>:<sender>:<receiver>`, ordered
by the amount's sign — so both profiles that touch the pair produce the
*same* string and net (the direction comes from the money flow, never
typed). A profile that declares `transit` rows must also set
`transit.field` and `transit.self`, or the import aborts.

**RPC sources (no CSV).** Instead of a file, a profile can pull straight
from a coin daemon's JSON-RPC: set `wallet.coin monero`; acc finds the running
`monero-wallet-rpc` by matching `wallet.address` (no fixed port — it scans `wallet.ports` on `wallet.host`), then calls `get_transfers` and books each
transfer — a receive (amount only, the fee is the sender's), a send (amount
+ your fee), or a self-transfer (fee only, the amount returns to the same
account) — and embeds the full RPC object as a `; rpc:` comment for the
record. Dedup is on the on-chain `txid`; the same categorization grammar
applies, matching the transfer's fields (`type`, `address`, `subaddr`,
`payment_id`, `note`). A wallet with several accounts (major indices) books
each to its own sub-account (`…:<label>`, or `…:<index>` when unlabelled).

**Haveno trades (Monero).** Haveno runs its own Monero wallet, so adding a
`haveno.*` block to that wallet's profile enriches the import: acc pulls the
completed trades from a running `haveno-daemon` (over gRPC via the `grpcurl`
CLI) and books each trade's two on-chain legs — matched to a wallet transfer by
`txid` — as swap entries. The funding leg splits the outgoing XMR into the fee,
the security deposit set aside, and the net XMR traded `@@` the fiat; the payout
leg returns the deposit (and records the fiat paid for a buy). The amounts come
from the trade, while the wallet transaction stays as the `; rpc:` source. Give
`haveno.port`, `haveno.pass` (the daemon's API password), `haveno.proto` (its
`.proto` directory) and the clearing accounts `haveno.deposit` / `haveno.swap`;
the internal wallet-rpc's digest login goes in `wallet.login`. Every other
transfer stays a plain Monero booking.

**Bitcoin & Litecoin (one Core daemon).** `wallet.coin bitcoin` or `litecoin`
targets a Bitcoin Core-family daemon instead — bitcoind, litecoind and their
forks speak identical JSON-RPC, so one backend serves them all. A single daemon
hosts every wallet by URL path, so there is no port scan: give its base URL
(`wallet.rpc http://127.0.0.1:8332`), the wallet name (`wallet.name main`,
which is also its transit leaf) and the cookie file (`wallet.cookie
~/.bitcoin/.cookie`, or `wallet.user` + `wallet.pass`). acc reads
`listtransactions`, books a receive/send/self-send the same way, and drops
transactions the daemon reports as replaced (RBF, negative confirmations) or
abandoned — they never settled. Rule fields are `category`, `address`, `label`,
`txid`.

**Own↔own transfers between wallets.** Run one profile per wallet and a
transfer from one of your wallets to another nets automatically: acc matches
the two legs by shared `txid` across your other wallets — for Monero the
running wallet-rpc endpoints, for Bitcoin/Litecoin the same daemon's other
loaded wallets — so it works even when the sending wallet cached no
destination. It books a directional transit account whose leaf for each wallet
is `<wallet.coin>-<last 4 of its address>` (Monero) or `<wallet.coin>-<wallet
name>` (Bitcoin Core), derived purely from RPC — no other conf is read. Set
`transit.self <prefix>`; for an account NOT on RPC (an exchange), map its
address manually with `transit <address> <leaf>`.

Re-importing an overlapping export is safe: each transaction embeds its
source row as a `; csv:` (or `; rpc:`) comment, and rows already present
(matched on the `identity`) are skipped. The write is
append-only — existing entries are never rewritten. Appended transactions
are aligned by the same in-memory formatter as `acc format`, so they match
every other file; a thousands-separator comma in an amount (`1,190.00`) is
stripped first, since acc's decimal parser rejects it.

### Environment variables

| Variable                    | Used by           | Description |
|-----------------------------|-------------------|-------------|
| `PRICES`            | main pipeline, `update` | Directory of rate files. When `-X` is set, the `.ledger` files under it are loaded before your own `-f` paths — selectively, keeping only the pairs a report's conversions need, including the bridge pairs on a multi-hop path (e.g. `XMR → $ → €`). `acc update` writes here. |
| `OPENEXCHANGERATES_API_KEY` | `update` (fiat)   | API key from [openexchangerates.org](https://openexchangerates.org). Required for fiat fetching. |

### Exit codes

| Code | Meaning                                                  |
|------|----------------------------------------------------------|
| `0`  | Success.                                                 |
| `1`  | Load failure (parse / resolve / book / IO error) or invalid CLI argument. Error message on stderr. |

---

## Examples

The two reports most users run first:

```
$ acc -f journal.ledger bal
 $7441.80 assets
 $7441.80   checking
$-5000.00 equity
$-5000.00   opening
   $58.20 expenses
   $58.20   food
$-2500.00 income
$-2500.00   salary
---------
        0
```

```
$ acc -f journal.ledger reg
2024-01-01 initial balances  assets:checking   $5000.00  $5000.00
                             equity:opening   $-5000.00         0
2024-01-05 Groceries         expenses:food       $58.20    $58.20
                             assets:checking    $-58.20         0
2024-01-10 * paycheck        assets:checking   $2500.00  $2500.00
                             income:salary    $-2500.00         0
```

Everything else — `print`, `accounts`, `commodities`, `codes`,
`lint`, filter patterns, `-X` currency conversion,
slippage gain/loss, CTA, lot annotations, balance assertions — is
covered in topic-specific walkthroughs with journal inline and
verbatim output:

- [`examples/01-basics.md`](examples/01-basics.md) — all the list-
  and print-style commands
- [`examples/02-filters.md`](examples/02-filters.md) — the filter
  DSL, `-r`, `-R`, multi-`-p`, date ranges
- [`examples/03-currency-conversion.md`](examples/03-currency-conversion.md) —
  `-X`, multi-hop
- [`examples/04-slippage.md`](examples/04-slippage.md) —
  realising gain/loss on multi-commodity trades
- [`examples/05-cta.md`](examples/05-cta.md) — IAS 21 / ASC 830
  Cumulative Translation Adjustment
- [`examples/06-lots-and-costs.md`](examples/06-lots-and-costs.md) —
  `@` / `@@` / `{COST}` lot tracking
- [`examples/07-assertions.md`](examples/07-assertions.md) —
  balance assertions and assignments
- [`examples/08-diff.md`](examples/08-diff.md) — `acc diff`
  every input combination (file/dir, explicit / `--snapshot`)
  with verbatim outputs

### `acc format`

Before — misaligned, mixed commodity-glue, whitespace noise:

```
2024-03-01 * Equipment purchase
    assets:bank    USD-5000.00
    expenses:hardware   USD 5000.00

2024-04-15 * Monthly rent (annual contract split)
    expenses:rent   USD 1000.00 @ (USD 12000/12)
    assets:bank USD-1000.00
```

After `acc format journal.ledger`:

```
2024-03-01 * Equipment purchase
	assets:bank              USD-5000.00
	expenses:hardware         USD5000.00

2024-04-15 * Monthly rent (annual contract split)
	expenses:rent             USD1000.00 @ (USD 12000/12)
	assets:bank              USD-1000.00
```

Account column left-aligned, amount column right-aligned, the
`(USD 12000/12)` expression survives the round-trip as-is. Commodity
and number are glued (`USD -1000` → `USD-1000`) — so the digits
line up on the right edge and `USD` floats to wherever the number
pushes it.

**Vim integration** — drop this in your `ftplugin/ledger.vim`:

```vim
autocmd FileType ledger nnoremap <leader>f :%!acc format -<cr>
```

Then in any ledger buffer, `<leader>f` pipes the buffer through
`acc format` and replaces it with the aligned output. Undo
history stays intact (it's a buffer edit, not a file reload), and
because only the parser runs, format works mid-edit on a journal
whose balance doesn't yet compute.

### `acc diff`

Useful for checking that a format pass — or any other edit —
didn't drop content. Given a `.bak` that saved the pre-format
state:

```
$ acc diff journal.ledger.bak journal.ledger
--- journal.ledger.bak
+++ journal.ledger
@@ -1,5 +1,5 @@
 2024-03-01 * Equipment purchase
-    assets:bank    USD-5000.00
-    expenses:hardware   USD 5000.00
+	assets:bank              USD-5000.00
+	expenses:hardware         USD5000.00

1 files compared, 0 with differences
```

Note: the tab vs 4-space indent and the `USD 5000 → USD5000`
glue both show up as changed lines (because `-w` strips them for
comparison, but the display still shows them). Exit code is `0`
because no **token-level** difference — so in a CI check, `acc
format` + `acc diff` proves the round-trip is safe.

When the working tree and a backup share the same layout under
different roots, skip the full path:

```
cd ~/journals/cash
acc diff --snapshot /path/to/backup journal.ledger
```

acc walks `journal.ledger`'s absolute path from the right and
matches against the longest suffix that exists under the
snapshot root — no config, no env var, works with any backup
layout.

### Automated transactions (`= /pattern/`)

Keep `assets:cash` at zero by auto-booking every cash inflow to
`expenses:cash` (so the physical cash you pull from the bank is
immediately treated as spent — the classic "all cash counts as
expense" policy):

```
= /^assets:cash/
	[assets:cash]          -1
	[expenses:cash]         1

2024-05-10 * ATM withdrawal
	assets:cash             $100
	assets:bank            $-100
```

Expanded at load time to:

```
2024-05-10 * ATM withdrawal
	assets:cash             $100
	assets:bank            $-100
	[assets:cash]          $-100
	[expenses:cash]         $100
```

Net effect: `assets:cash` back to zero, `assets:bank` down $100,
`expenses:cash` up $100.

The amount after each account is a factor on the triggering amount: a
bare number, or `amount` / `-amount` as readable synonyms for `1` /
`-1`. A posting with **no** amount is the balancing leg — the expander
fills it with the negated pool sum, like the bare last posting of a
hand-written transaction (so the flush above can also be written
`[assets:cash] -amount` + a bare `[expenses:cash]`).

Each balance pool must balance: a bare leg fills it, or the factors sum
to zero — real postings on their own, balanced-virtual `[...]` on their
own (unbalanced `(...)` postings are exempt); the resolver validates
this. A VAT-split variant:

```
= /^income:gross/
	[income:gross]          -1
	[income:net]          0.81
	[taxes:vat19]         0.19
```

Matching `income:gross $1000` injects `-$1000` flush,
`+$810` net, `+$190` vat.

**`$account` — refer to the matched account.** Inside an injected
posting, `$account` is replaced with the account of the posting that
triggered the rule (ledger's `[$account]`). So one rule flushes each of
several accounts to its *own* leg instead of a hard-coded parent — e.g.
per-currency cash:

```
= /^assets:cash-/
	[$account]             -1
	[expenses:cash]         1
```

A transaction touching `assets:cash-eur $5` and `assets:cash-usd $3`
injects `[assets:cash-eur] $-5` and `[assets:cash-usd] $-3` — each to its
own specific account — with `expenses:cash` collecting the total. The
substitution is textual, so `$account` works as the whole account or
embedded (`budget:$account`).

**`$segment` — match any one account segment.** A `$segment` in the
pattern stands for exactly one segment (a run without `:`, i.e. `[^:]+`).
It anchors a rule to a segment *position* rather than matching at any
depth:

```
= /^$segment:cash-/
	[$account]             -1
	[expenses:cash]         1
```

This matches `personal:cash-eur` and `business:cash-usd` — any single
leading segment followed by `:cash-` — but not `a:b:cash-eur` (two
segments before `:cash-`) or `cash-eur` (none). `$segment` is acc's own
placeholder, not ledger's: acc auto-patterns are *not* a regex engine —
the only metacharacters are the `^` / `$` anchors and the literal
`$segment` token (no ranges, classes or quantifiers). It may appear more
than once and in any position (`:cash:$segment:eur`); each occurrence
consumes exactly one segment. Pair it with `$account` to flush every
matched account to its own leg regardless of its leading segment.

**`$year` / `$month` / `$day` — the transaction's date.** In any posting
account (hand-written or auto-injected), `$year` / `$month` / `$day` are
replaced with the parts of that transaction's own date — `assets:budget:$year`
on a 2026 entry becomes `assets:budget:2026`. A plain textual replace, so they
work embedded anywhere in the account.

**Named templates + `[key]` lookups.** A named rule `= NAME :: /pattern/` is
a *template* — it does nothing on its own, and is fired by instantiating it
with a pair, `= NAME a b`. Positional `$1` / `$2` placeholders in the pattern
and posting accounts are filled from the two arguments; because the pair is
unordered, each instantiation emits *both* directions (one rule each), so a
single `= NAME a b` mirrors `a→b` and `b→a`. A lookup table is a set of
`= NAME[key] :: value` entries — a string→string map on the same
auto-transaction level (leading `=`), referenced as `NAME[key]` inside a
posting account to expand a key to its value (unknown key → error). Together
they track a per-pair "who owes whom" position:

```
= fullname[a] :: alpha-corp
= fullname[b] :: beta-llc

= reconcile :: /^transit:$1-$segment:$2-$segment$/ amount > 0
	($1:owed:fullname[$2])    1
	($2:owed:fullname[$1])   -1

= reconcile a b
```

`= reconcile a b` expands to two concrete rules, matching `transit:a-…:b-…`
and `transit:b-…:a-…`. Only *listed* pairs fire; deleting the `= reconcile`
line removes the position.

**Unbalanced `(...)` postings and the `amount` clause.** Injected postings
follow the normal balance rules: real and balanced-virtual `[...]` postings
must each sum to zero across the rule (validated per pool), while unbalanced
`(...)` postings take part in no balance — a lone `(...)` posting is valid. An
optional `amount <op> N` clause after the pattern (`op` one of `>` `<` `>=`
`<=` `==` `!=`) fires the rule only when the matched posting's amount satisfies
it: `amount > 0` above counts a positive outflow (a send) but skips the
negative counter-posting that clears it. There is no boolean expression
language — AND is more clauses, OR is more rules, NOT flips the operator.

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
	assets:btc         BTC0.1 {$30000}
	assets:cash        $-3000

; sell part of the lot at a higher price → gain
2024-06-01 sell
	assets:btc         BTC-0.05 {$30000} @ $40000
	assets:cash           $2000
	income:gain           $-500
```

`{COST}` = per-unit lot cost; `{{TOTAL}}` = whole-lot cost (what
`@@` is to `@`). A leading `=` (`{=COST}`) locks the cost so display
semantics don't drift. The booker prefers lot cost over `@`-cost for
balance math and round-trips the exact form you wrote. `[DATE]`
records the lot's acquisition date — display-only, and valid only
next to a `{…}` / `{{…}}` cost (a bare `[date]` is rejected, since a
later FIFO split would silently overwrite it).

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

`-X TARGET` converts every amount into `TARGET` using the price DB.

### Per-posting conversion at `tx.date`

```
acc -f journal.ledger bal -X €
```

Each posting is converted using the latest `P` rate on or before
its transaction's own date. A $5 coffee from 2020 always shows as
its 2020 € equivalent, regardless of when the report runs. Reports
are historically reproducible — same journal + same rate files =
same result, forever. acc always values this way; there is no
report-date snapshot mode.

### Multi-hop

If no direct `P BASE QUOTE` rate exists, acc does BFS over the
commodity graph. `TOKEN → STABLECOIN → USD → EUR` resolves
transparently if the intermediate pairs exist. Inverse rates are
computed on demand, so a stored `USD/EUR` also serves `EUR/USD`.

### Missing rates

If no path exists between a posting's commodity and the target,
the posting stays in its original commodity. No error, just a
remainder visible in the report.

### `$PRICES`

When `-X` is set, the `.ledger` files under the directory the env
var points to supply the rates, loaded before your own `-f` paths:

```
export PRICES=~/accounting/prices/
```

You can put both acc-fetched (`acc update`) and hand-written `P`
directives here. No-op when `-X` is absent.

**Selective loading.** The price DB can grow to hundreds of thousands
of `P` directives, but a report only ever needs the handful of pairs
that connect the commodities it actually holds to the `-X` target. So
acc parses your journal first, works out that set (expanded across
commodity aliases, so `$` / `USD` / `USDT` all match), then keeps a
`P` directive only when *both* its commodities are in it — every
other rate is dropped before its date and amount are even parsed. A
report's load stays flat as the DB grows; on a multi-thousand-file DB
this is the difference between a fraction of a second and a noticeable
pause. The directory layout is untouched — nothing about how you keep
prices on disk has to change.

### `slippage gain` / `slippage loss` realisation

Declare the two accounts:

```
account Equity:SlippageGain
    slippage gain

account Equity:SlippageLoss
    slippage loss
```

With `-X TARGET` and both accounts declared, acc converts every
posting of a multi-commodity transaction to the target at the
market rate on `tx.date` and sums them up. If the sum is non-zero,
the transaction's implied rate differed from the market rate — the
difference becomes the realised gain or loss, and acc injects a
real posting to close it out: `slippage gain` when the user came out
ahead of market, `slippage loss` when behind. Differences below the
target's display precision are ignored.

**Example.** Target `€`, market rate `P 2024-06-15 USD EUR 0.90`.

```
2024-06-15 sold USD for EUR
    assets:usd  $-1000
    assets:eur   €920
```

At market rate `$1000` is worth `€900`, but the user got `€920` —
`€20` gain. acc adds:

```
    Equity:SlippageGain  €-20
```

Report on them directly:

```
acc bal Equity:SlippageGain Equity:SlippageLoss -X €    # total realised gains / losses
acc reg Equity:SlippageGain -X €                  # per-transaction breakdown
```

### `holding gain` / `holding loss` — mark to market

`slippage` closes the gap on *trades* — money actually changed
hands at an off-market rate. But a position you still **hold** in a
foreign currency also drifts as rates move, and that drift is
**unrealized** until you convert back. By default acc ignores it: the
default report values every posting historically (rule 1), so an open
`$` balance keeps its acquisition-date `€` value.

`-V` / `--unrealized` turns on the report-date revaluation. Declare
the accounts:

```
account Equity:Holding:Gain
    holding gain

account Equity:Holding:Loss
    holding loss
```

Under `-X TARGET -V`, acc marks every open foreign **balance** to the
**latest available rate** and books the difference — current value
minus historical value — to `holding gain` / `holding
loss`:

```
acc bal ^assets -X €        # historical: open $ at acquisition cost
acc bal ^assets -X € -V     # marked to market: open $ at latest rate
```

acc imposes **no** monetary / non-monetary classification: it
revalues *every* account holding an open foreign-currency balance,
**income and expense included**. That is deliberate — which accounts
are balance-sheet and which are P&L is your call, expressed through
your account structure and the report filter, not something acc
hard-codes. Scope the report to what you want to value: `^assets` for
a balance-sheet snapshot, `^expenses` if you ever want P&L at current
rates. You normally look at one or the other, and since each
revaluation nets to zero, the balances you don't filter in never
disturb the ones you do.

The revaluation is one synthetic transaction per open
`(account, commodity)` — its description carries that commodity
(`holding revaluation $`), so several foreign currencies on one
account stay distinguishable — dated today, so the journal still
reloads 1:1. It is opt-in and
orthogonal to the historical default — without `-V` nothing is
revalued, so the realized / tax-relevant view is untouched: `slippage`
and `cta` book **realized** results; `holding` is the
only place an **un**realized number appears.

### `cta gain` / `cta loss` — Commodity Translation Adjustment

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
`-X USD` shows a `+$500` phantom. Nothing economically happened —
the money passed through — but the account looks like it gained
`$500`.

```
$ acc bal -X USD               # without cta accounts declared
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
exactly like the existing `slippage gain` / `slippage loss` pair:

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

With both `cta gain` and `cta loss` declared and `-X TARGET` set,
acc walks every `(account, commodity)` group chronologically. For
every group whose native amounts sum to exactly zero over the
reporting period — the definition of a transit account — it tracks
running native and running target. At every zero-crossing of the
native balance where the running target is non-zero, a synthetic
transaction is emitted on that date:

```
<date> * commodity translation adjustment
    <transit-account>    TARGET -drift
    <cta-account>        TARGET drift
```

Both postings are real and sum to zero on their own, so the
transaction balances and reloads 1:1 — the transit account's target
sum is driven to zero. Positive drift (target value lost while
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
$ acc bal -X USD
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
$ acc reg -X USD
2024-01-15 * salary arrives          assets:checking        USD11000.00
                                     income:salary         USD-11000.00
2024-06-15 * invoice paid            expenses:services      USD10500.00
                                     assets:checking       USD-10500.00
2024-06-15 * commodity translation adjustment  assets:checking   USD-500.00
                                              equity:cta:loss    USD500.00
```

Auditable, reproducible, name-attributable.

#### Interaction with `slippage gain` / `slippage loss` and `capital gain` / `capital loss`

The three mechanisms measure different things and never double-book.
The realizer's **slippage** is the trade-day execution deviation *within* one
multi-commodity transaction (implied/booked rate vs. market on
`tx.date`). The lotter's **capital** is the disposed asset's market
move *over its holding period*. The translator's **CTA** is the
holding-period drift on a *same-commodity transfer* — a foreign
currency passing through an account with no trade. A traded position
nets to zero through capital + fx, so CTA does not fire on it; CTA is
reserved for genuine pass-through transfers. Every injected
transaction is self-balancing in the target currency, so the three
never overlap.

#### Position in the plaintext-accounting ecosystem

As of this writing, acc is the only plaintext-accounting tool that
implements IAS 21 / ASC 830 translation adjustment automatically:

- **ledger-cli** and **hledger** default to single-rate valuation
  under `-X`, which sidesteps the drift at the cost of historical
  income-statement stability. Neither tool has a CTA concept.
- **beancount** exposes `account_previous_conversions` and
  `account_current_conversions` options but does not populate them
  automatically — they require explicit invocation of
  `summarize.conversions()` at the user's discretion.
- **rustledger** carries the beancount option schema forward but
  the booking logic is not wired into the pipeline.

acc's per-posting-tx.date conversion preserves IAS 21 rule (1)
(historical income/expense). It deliberately omits the report-date
snapshot of rule (2) (current rate for monetary items), valuing
everything historically instead. The `cta gain` / `cta loss` pair
covers rule (3) (translation differences to OCI / equity), routing
the valuation difference on transit accounts to equity rather than
revaluing open balances.

### `capital gain` / `capital loss` — realised gains via FIFO lots

Declare the two accounts:

```
account income:capital:gain
    capital gain

account income:capital:loss
    capital loss
```

With both declared, acc keeps a FIFO lot queue per `(account,
commodity)`. An acquisition (a positive posting carrying its cost via
`@` / `@@`) opens a lot; a disposal (a negative posting) closes lots
oldest-first and books the realised gain or loss. Write the disposal
at its market price with `@`, **not** a `{}` annotation — the leg has
to balance against its proceeds on its own. An explicit `{cost}` on a
disposal means *you* are booking the gain by hand, so acc consumes the
lot for FIFO consistency but injects nothing.

**Example.**

```
2023-06-01 * buy
    assets:crypto:eth   ETH 2 @ EUR 1500
    assets:cash        EUR -3000

2024-06-01 * sell
    assets:crypto:eth   ETH -2 @ EUR 2000
    assets:cash         EUR 4000
```

```
$ acc print
2024-06-01 * sell
    assets:crypto:eth      ETH-2 {EUR1500} [2023-06-01] @ EUR2000
    assets:cash            EUR4000
    income:capital:gain    EUR-1000
```

acc rewrites the disposal leg in place with the lot it closed
(`{cost} [acquisition-date]`) and appends the gain as a real posting:
`2 × (2000 − 1500) = 1000`. A sale spanning several lots splits FIFO
into one leg per lot, each with its own basis and date.

**Under `-X` the realised result decomposes across phases.** Valuation
follows the price DB, and two named figures are booked by separate
phases that run together — so a single trade can show both:

- **capital** (`capital gain` / `capital loss`, the lotter) — the
  disposed lot's *market move* over its holding period: the
  commodity's market value at the sale date minus its market value on
  the acquisition date, the genuine investment performance. The
  disposal leg carries that acquisition-date market value as its `{}`
  cost basis, so the asset enters and leaves at the same value and nets
  to zero.
- **slippage** (`slippage gain` / `slippage loss`, the realizer) — the *execution spread*
  on every trade, buy and sell: the booked rate's deviation from the
  market rate that day. Declare the slippage accounts so a multi-commodity
  trade balances at market (the realizer strips the `@` so each leg
  converts at market, and the gap to market becomes slippage).

A same-commodity *transfer* — a foreign currency passing through an
account across a rate move, with no trade — is not a capital event; its
holding-period drift is booked as **CTA** instead, so a currency
tailwind never masks a poor asset pick. Report:

```
acc bal income:capital -X EUR    # realised gains / losses
acc reg income:capital -X EUR    # per-disposal breakdown
```

---

## Rate updates (`acc update`)

Fetches daily rates into `$PRICES` from two sources:

- **MEXC klines** for crypto (no API key required)
- **openexchangerates.org** for fiat (needs
  `OPENEXCHANGERATES_API_KEY` in the environment; see
  [openexchangerates.org](https://openexchangerates.org) for sign-up
  — free tier covers typical personal use)

Files are stored at:

- Crypto: `$PRICES/crypto/MEXC_{BASE}_{QUOTE}.ledger`
  (one file per pair)
- Fiat: `$PRICES/fiat/{YYYY-MM-DD}.ledger`
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

# Refresh every existing crypto pair in $PRICES/crypto/
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

### `= NAME[key] :: value` (lookup tables)

```
= fullname[a] :: alpha-corp
= fullname[b] :: beta-llc
```

A set of `= NAME[key] :: value` entries is a string→string **lookup table** on
the auto-transaction level (leading `=`): each line maps one key, and
`NAME[key]` — inside an automated-transaction template posting — expands to the
value (an unknown key is an error). Deliberately a lookup *only*; acc has no
expression evaluator. See **Automated transactions** for how a template
references it.

### `~` (periodic transactions)

```
~ 2021 monthly annual-budget
	assets:budget:$year   €1200.00
	income:x
```

A `~ YYYY [monthly|daily] [title]` block expands into **real, ordinary
transactions** — one per occurrence in the year, dated at its start
(`YYYY-01-01`, the 1st of each month, or each day). The written amounts are the
period **total**; a cadence divides them across the occurrences (`monthly` →
÷12, `daily` → ÷ days), with the last occurrence absorbing any rounding
remainder so the slices sum back exactly. `$year` / `$month` / `$day` in an
account are filled from each occurrence's date (so the example above accrues
`assets:budget:2021` at €100/month). Without a cadence keyword it is a single
transaction on `YYYY-01-01`.

Unlike ledger's `~` (an unbounded forecast that *repeats* the amount), acc's are
bounded to the year and *split* the total; the generated transactions are real
— they book, balance, and auto-fill a bare posting like any hand-written entry.

### `account`

```
account Equity:Slippage:Gain
    slippage gain

account Equity:Slippage:Loss
    slippage loss

account Equity:Holding:Gain
    holding gain

account Equity:Holding:Loss
    holding loss

account Equity:CTA:Gain
    cta gain

account Equity:CTA:Loss
    cta loss

account Equity:Capital:Gain
    capital gain

account Equity:Capital:Loss
    capital loss
```

Eight sub-directives, in four pairs:

- `slippage gain` / `slippage loss` — the realiser's per-trade
  execution spread on multi-commodity transactions whose implied
  conversion rate diverges from the market rate. See
  [`slippage gain` / `slippage loss` realisation](#slippage-gain--slippage-loss-realisation).
- `holding gain` / `holding loss` — the revaluator's
  report-date mark-to-market of open foreign balances, booked only
  under `-V` / `--unrealized`. See
  [`holding gain` / `holding loss`](#holding-gain--holding-loss--mark-to-market).
- `cta gain` / `cta loss` — the translator's IAS 21 / ASC 830
  Cumulative Translation Adjustment: the holding-period drift on
  single-commodity transit accounts when rates move between inflow
  and outflow. See
  [`cta gain` / `cta loss`](#cta-gain--cta-loss--commodity-translation-adjustment).
- `capital gain` / `capital loss` — the lotter's FIFO realised
  capital gain on disposed lots (the holding-period market move). See
  [`capital gain` / `capital loss`](#capital-gain--capital-loss--realised-gains-via-fifo-lots).

Each sub-directive must be unique across the journal — declaring
two different accounts with `cta gain` is an error. Both halves of
a pair must be declared for their feature to activate.

A further family of sub-directives attaches cosmetic labels:

```
account 1000
    label foo
```

`label <text>` gives *that* account a dimmed display label, so numbered
chart-of-accounts codes keep sorting nicely while still reading as words.
Each view renders it in its own place, always keeping the number:

- `acc bal` **appends** it after the name — `1000 (foo)` in tree mode,
  `assets:1000 (foo)` in flat mode — so the number still drives the
  tree's sort order.
- `acc reg` inlines it after the labelled segment inside the account
  path — `assets:1000 (foo):sub`.

It is display-only: no inheritance to sub-accounts, and nothing filters
or computes on it.

**Per view.** Bare `label` is the shared fallback for both views;
`label-balance` and `label-register` set a *view-specific* label that
overrides the fallback for that view — so a coded account can read one
way in the balance sheet and another in the register:

```
account 1000
    label          foo          ; both views, unless overridden
    label-register cash inflow   ; register only
```

`bal` shows `1000 (foo)`, `reg` shows `…:1000 (cash inflow):…`. Use only
`label-balance` / `label-register` (no bare `label`) to label one view
and leave the other bare.

The account name may itself carry a [`$segment`](#automated-transactions--pattern)
wildcard — for any of the three keywords — labelling every account that
matches:

```
account $segment:1000
    label foo
```

labels `personal:1000`, `business:1000`, … — any single leading segment
followed by `:1000`. The pattern is anchored to the whole name, so
`personal:1000:sub` is *not* labelled. Precedence within a view: the
view-specific label wins over the shared `label`, and an exact-name entry
wins over a `$segment` pattern.

### `P` — price

```
P 2024-06-15 USD EUR 0.92
P 2024-06-15 BTC USDT 63210.50
```

Date, base commodity, quote commodity, rate. The rate is
units-of-quote per unit-of-base. Populates the price DB that `-X`
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
- `apply` / `end` — scope-block directives.
- `D`, `Y`, `A`, `N` — short-form defaults.
- `tag`, `payee` — metadata directives.

---

## Philosophy

**Plain text, user-owned.** Journal files live where you put them,
edited with whatever editor you already use. No database, no
sync service, no lock-in. `git diff` is your audit log.

**Reproducible reports.** Same journal + same rate files produce
the same output today and a year from now. Every amount converts
at its own transaction date's rate, not at "right now" — last
year's numbers don't silently shift every time the report runs.
There is no rolling-revaluation mode; valuation is always
historical.

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
rate forever. acc has no rolling-revaluation mode — valuation is
always historical, by design.

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

### How do I see realised slippage gain/loss?

Declare `slippage gain` and `slippage loss` accounts (see [Currency
conversion](#currency-conversion)) and run with `-X`:

```
acc bal Equity:SlippageGain Equity:SlippageLoss -X €
```

The realiser automatically injects the gain/loss postings for
multi-commodity transactions whose implied rate diverges from the
market rate.

### Does acc write to my journal files?

No. Your journal is read-only from acc's perspective. The only
thing that writes is `acc update`, and only to `$PRICES`.

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
cargo run -- -f journal.ledger bal # try a build against the bundled demo
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
- `tests/conversion.rs` — `-X`, inverse + multi-hop
  rebalance

Before sending a patch, please `cargo test` and `cargo clippy` locally.

---

## Changelog

See [CHANGELOG.md](CHANGELOG.md).

## License

GPL-3.0. See [LICENSE](LICENSE).
