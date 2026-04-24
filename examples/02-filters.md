# 02 — Filtering

Every report command accepts positional pattern arguments plus
global date flags. This file walks through the pattern DSL and the
filter-related flags (`-r`, `-R`, multi-`-p`).

## Journal

```
2024-01-05 (42) coffee shop
    expenses:food:coffee       $4.50
    assets:cash

2024-01-06 * Sprouts
    expenses:food:groceries   $58.20
    assets:checking

2024-01-10 * paycheck
    assets:checking         $2500.00
    income:salary          $-2500.00

2024-01-15 (42) restaurant
    expenses:food:dining     $42.00
    assets:creditcard

2024-02-01 * electricity
    expenses:utilities       $75.00
    assets:checking

2024-02-15 (42) coffee shop
    expenses:food:coffee      $5.10
    assets:cash
```

## Account patterns

Substring match is the default, case-insensitive. `^` and `$`
anchor start/end:

```
$ acc -f journal.ledger bal food
$109.80 expenses
$109.80   food
  $9.60     coffee
 $42.00     dining
 $58.20     groceries
```

```
$ acc -f journal.ledger bal ^expenses
$184.80 expenses
$109.80   food
  $9.60     coffee
 $42.00     dining
 $58.20     groceries
 $75.00   utilities
```

```
$ acc -f journal.ledger bal coffee$
$9.60 expenses
$9.60   food
$9.60     coffee
```

## Pattern keywords — other dimensions

`@` for description, `#` for transaction code, `com` for commodity.
All case-insensitive.

Description contains `coffee`:

```
$ acc -f journal.ledger reg @coffee
2024-01-05 coffee shop  expenses:food:coffee   $4.50  $4.50
                        assets:cash           $-4.50      0
2024-02-15 coffee shop  expenses:food:coffee   $5.10  $5.10
                        assets:cash           $-5.10      0
```

Code equals `42`:

```
$ acc -f journal.ledger reg '#42'
2024-01-05 coffee shop  expenses:food:coffee    $4.50   $4.50
                        assets:cash            $-4.50       0
2024-01-15 restaurant   expenses:food:dining   $42.00  $42.00
                        assets:creditcard     $-42.00       0
2024-02-15 coffee shop  expenses:food:coffee    $5.10   $5.10
                        assets:cash            $-5.10       0
```

(Quote `#42` in shells that interpret `#` as a comment marker.)

## `-r` / `--related` — show counter-parties

With a pattern filter, `-r` flips the view: instead of the matched
postings, show the *other* postings of the same transactions.
Useful for seeing what balanced against a given account class.

```
$ acc -f journal.ledger reg food -r
2024-01-05 coffee shop  assets:cash         $-4.50    $-4.50
2024-01-06 * Sprouts    assets:checking    $-58.20   $-62.70
2024-01-15 restaurant   assets:creditcard  $-42.00  $-104.70
2024-02-15 coffee shop  assets:cash         $-5.10  $-109.80
```

Read: food expenses were paid from `cash`, `checking`, and
`creditcard`.

## Date filtering: `-p`, `-b`, `-e`

`-p PERIOD` accepts a year (`YYYY`), month (`YYYY-MM`), or day
(`YYYY-MM-DD`):

```
$ acc -f journal.ledger reg -p 2024-01
2024-01-05 coffee shop  expenses:food:coffee         $4.50     $4.50
                        assets:cash                 $-4.50         0
2024-01-06 * Sprouts    expenses:food:groceries     $58.20    $58.20
                        assets:checking            $-58.20         0
2024-01-10 * paycheck   assets:checking           $2500.00  $2500.00
                        income:salary            $-2500.00         0
2024-01-15 restaurant   expenses:food:dining        $42.00    $42.00
                        assets:creditcard          $-42.00         0
```

Repeat `-p` for multiple **discrete** periods. Not a range — each
`-p` stands on its own, transactions match if they fall in any:

```
$ acc -f journal.ledger reg -p 2024-01-05 -p 2024-02-15
2024-01-05 coffee shop  expenses:food:coffee   $4.50  $4.50
                        assets:cash           $-4.50      0
2024-02-15 coffee shop  expenses:food:coffee   $5.10  $5.10
                        assets:cash           $-5.10      0
```

For a contiguous range use `-b` and `-e`:

```
acc bal -b 2024-01 -e 2024-03          # Jan + Feb 2024
acc bal -b 2024-01-06 -e 2024-02-01    # 6-Jan through end-Jan
```

## Combinators

`or`, `and`, `not`. Default between bare tokens is OR. `and`/`not`
apply **per posting**, not per transaction — `food and cash` asks
for postings on an account that contains *both* "food" and
"cash", which nothing does.

```
acc reg coffee or dining           # postings on coffee or dining
acc reg not ^expenses              # non-expense postings
acc reg ^expenses and not food     # expenses that aren't food
```

## Global modifiers

- `--future` off by default; hides transactions dated after today.
- `-R` / `--real` — drop every virtual posting (paren and bracket)
  from the output. Keeps the real movements visible without
  auto-computed fx/cta labels. See [04](04-fx-gain-loss.md) and
  [05](05-cta.md).
- `-S` / `--sort FIELD` — `date` (default), `amount`, `account`,
  `description`. Prefix with `-` for reverse. Repeat `--sort` for
  secondary keys.
