# 01 — Basics

The minimal journey: a single-currency journal, the core report
commands, and what each produces.

## Journal

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

Three transactions, one commodity (`$`), one omitted amount — the
booker fills in `assets:checking $-58.20` on the Groceries tx by
inferring the missing side from the other posting.

## `bal` — balances grouped by account

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

Hierarchical tree by default. `--flat` gives one line per account.
`-E` / `--empty` adds zero-balance accounts to the output (hidden
by default).

## `reg` — transaction register with running total

```
$ acc -f journal.ledger reg
2024-01-01 initial balances  assets:checking   $5000.00  $5000.00
                             equity:opening   $-5000.00         0
2024-01-05 Groceries         expenses:food       $58.20    $58.20
                             assets:checking    $-58.20         0
2024-01-10 * paycheck        assets:checking   $2500.00  $2500.00
                             income:salary    $-2500.00         0
```

Per-tx running total in the rightmost column, zeros out after each
balanced transaction.

## `print` — normalised vs raw

Normal mode fills in computed amounts:

```
$ acc -f journal.ledger print
2024-01-01   (opening) initial balances
    assets:checking     $5000.00
    equity:opening     $-5000.00

2024-01-05   (42) Groceries
    expenses:food         $58.20
    assets:checking      $-58.20

2024-01-10 * paycheck
    assets:checking     $2500.00
    income:salary      $-2500.00
```

`--raw` dumps the original source bytes unchanged:

```
$ acc -f journal.ledger print --raw
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

## `accounts`, `commodities`, `codes`

List views of the journal's dimensions:

```
$ acc -f journal.ledger accounts
assets:checking
equity:opening
expenses:food
income:salary
```

```
$ acc -f journal.ledger commodities
$
```

```
$ acc -f journal.ledger codes
42
opening
```
