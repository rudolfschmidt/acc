# 05 — CTA (Currency Translation Adjustment)

The flagship feature. Implements **IAS 21** (*The Effects of
Changes in Foreign Exchange Rates*) and **US-GAAP ASC 830**
(*Foreign Currency Matters*) automatically.

## The problem

Under per-posting historical conversion (`tx.date` rate, the
default `-X` mode), a transit account that receives money in one
currency and pays the same amount out later is **empty in native**
but **non-zero in target** because rates moved between inflow and
outflow.

Example without CTA accounts declared:

```
commodity $
    alias USD
commodity €
    alias EUR

P 2024-01-15 EUR USD 1.10
P 2024-06-15 EUR USD 1.05
P 2025-01-15 EUR USD 1.15
P 2025-06-15 EUR USD 1.02

2024-01-15 * salary A
    assets:checking    10000 EUR
    income:salary     -10000 EUR

2024-06-15 * invoice paid A
    expenses:services  10000 EUR
    assets:checking   -10000 EUR

2025-01-15 * salary B
    assets:checking    10000 EUR
    income:salary     -10000 EUR

2025-06-15 * invoice paid B
    expenses:services  10000 EUR
    assets:checking   -10000 EUR
```

Native:

```
$ acc -f journal.ledger bal
 €20000 expenses
 €20000   services
€-20000 income
€-20000   salary
-------
      0
```

`assets:checking` is gone from the balance (empty in €, hidden by
default). Now with `-X USD`:

```
$ acc -f journal.ledger bal -X USD
  $1800.00 assets
  $1800.00   checking         ← phantom drift, checking was empty
 $20700.00 expenses
 $20700.00   services
$-22500.00 income
$-22500.00   salary
```

`assets:checking` shows **`$1800` of phantom drift** — the money
passed through and out, but because 2024's EUR weakened against
USD and 2025's EUR also weakened, the $-value of the outflows was
less than the $-value of the inflows. Nothing really happened, but
the account looks like it gained `$1800`.

This is factually misleading for:
- **Audit** — transit accounts should reflect their real state.
- **Period comparability** — the same cash flow should net to zero
  in any reporting currency.
- **Balance-sheet correctness** — asset accounts misrepresent
  where value sits.

## Regulatory framing

| Framework | Reference | Rule |
|-----------|-----------|------|
| IFRS | **IAS 21** §§ 39–48 | Translation differences are recognised in **other comprehensive income** (OCI), never in profit or loss. |
| US-GAAP | **ASC 830-30** | Translation adjustments accumulate in a separate component of equity — the **Cumulative Translation Adjustment** account. |

Both agree: the drift is real, but it's not an income event and it
doesn't belong on the asset that happened to transit the currency.
It belongs on a dedicated equity / OCI account.

## Enabling CTA

Declare the two accounts:

```
account equity:cta:gain
    cta gain

account equity:cta:loss
    cta loss
```

Names are up to you; the sub-directives are what acc looks for.
Both must be declared — if only one is present, the translator
phase doesn't run.

## Same journal with CTA declared

```
$ acc -f journal.ledger bal -X USD
  $1800.00 equity
  $1800.00   cta
  $1800.00     loss
 $20700.00 expenses
 $20700.00   services
$-22500.00 income
$-22500.00   salary
```

`assets:checking` is gone (genuinely zero now). The `$1800` is
named on `equity:cta:loss` — translation **loss**, because holding
EUR during a weakening period cost the USD-reporting entity value.

## Register shows the injected adjustments

```
$ acc -f journal.ledger reg -X USD
2024-01-15 * salary A                assets:checking     $11000.00  $11000.00
                                     income:salary      $-11000.00          0
2024-06-15 * invoice paid A          expenses:services   $10500.00  $10500.00
                                     assets:checking    $-10500.00          0
2024-06-15 * translation adjustment  [assets:checking]    $-500.00   $-500.00
                                     [equity:cta:loss]     $500.00          0
2025-01-15 * salary B                assets:checking     $11500.00  $11500.00
                                     income:salary      $-11500.00          0
2025-06-15 * invoice paid B          expenses:services   $10200.00  $10200.00
                                     assets:checking    $-10200.00          0
2025-06-15 * translation adjustment  [assets:checking]   $-1300.00  $-1300.00
                                     [equity:cta:loss]    $1300.00          0
```

Two synthetic **translation adjustment** transactions, one per
zero-crossing of `assets:checking`'s native balance:

1. After the first in/out cycle (2024-01-15 → 2024-06-15): `$500`
   of drift booked to `equity:cta:loss`.
2. After the second cycle (2025-01-15 → 2025-06-15): `$1300` of
   drift booked to `equity:cta:loss`.

The `[…]` brackets mark the postings as **bracket-virtual**:
virtual (automatic, injected) but balance-contributing — so the
transit account's target sum actually reaches zero while the drift
is named elsewhere.

## Reports on CTA

```
$ acc -f journal.ledger bal equity:cta -X USD
$1800.00 equity
$1800.00   cta
$1800.00     loss
--------
$1800.00
```

## `-R` — hide the automatic labelling

If you want to see only the flows you typed, without the injected
adjustments:

```
$ acc -f journal.ledger bal -X USD -R
  $1800.00 assets
  $1800.00   checking
 $20700.00 expenses
 $20700.00   services
$-22500.00 income
$-22500.00   salary
```

The translator still ran and computed the drift, but `-R` drops
every virtual posting (paren-virtual realizer labels and
bracket-virtual translator labels). The `$1800` goes back to
`assets:checking` as the raw arithmetic product of rate movement.

Use `-R` when you want to audit the "just the transactions I
entered" view without automated bookkeeping overlaid.

## Interaction with `-V` / `--unrealized`

CTA and mark-to-market (`-V`) are **complementary and never
overlap**. CTA books the *realized* drift on a transit once it has
**closed** — native balance back to zero. `-V` does the other end: it
skips zero-native accounts entirely (they are CTA's job) and marks
accounts that still hold an **open** foreign balance to the latest
rate.

On this journal the transit has fully closed, so CTA nets it to zero
— and adding `-V` changes nothing for it:

```
$ acc -f journal.ledger bal ^assets -X USD        # CTA active
    0

$ acc -f journal.ledger bal ^assets -X USD -V     # -V skips the closed transit
    0
```

A still-open foreign balance is the mirror image: untouched by CTA
(it never closed), marked to market by `-V`. The two split the work
cleanly — realized drift on closed transits to CTA, unrealized
revaluation of open balances to `-V` — so nothing is double-counted.

## Sign convention

- **Positive drift** (transit retained target-value lost while
  holding native → the target-currency observer lost purchasing
  power) → `cta loss`.
- **Negative drift** (target-value gained during holding period)
  → `cta gain`.

A user-side mental model: CTA gain / loss mirrors "was the foreign
currency I was holding strengthening (gain) or weakening (loss)
during the time I held it". The ledger conventions follow:

- `income:...` and `equity` credit accounts: **negative** values =
  gain (credit increases equity / income).
- `expenses:...` debit accounts: **positive** values = loss.

## vs. other tools

- **ledger-cli**, **hledger**: default to single-rate `-V`
  revaluation, which sidesteps the drift at the cost of
  income-statement stability. No CTA account.
- **beancount**: has `account_previous_conversions` and
  `account_current_conversions` options, but booking requires
  explicit user invocation of `summarize.conversions()`.
- **rustledger**: carries beancount's option schema; booking logic
  is not wired up.
- **acc**: fully automatic. Declare the accounts, that's it.
