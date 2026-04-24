# 03 — Currency conversion

`-x TARGET` converts every amount into `TARGET` using the price DB
assembled from `P` directives. Two modes: per-posting historical
(default, stable) and `--market [DATE]` snapshot (current-value).

## Journal

Multi-currency, multi-date. US salary in January, EU rent in June,
cross-currency transfer in December.

```
commodity $
    alias USD
    precision 2
commodity €
    alias EUR
    precision 2

P 2024-01-15 USD EUR 0.92
P 2024-06-15 USD EUR 0.93
P 2024-12-15 USD EUR 0.95

2024-01-15 * us salary
    assets:checking        $3000.00
    income:salary         $-3000.00

2024-06-15 * eu rent
    expenses:rent           €1500.00
    assets:eu-bank         €-1500.00

2024-12-15 * cross-currency transfer
    assets:checking        $-1000.00
    assets:eu-bank           €930.00
```

Note `commodity € / alias EUR` — P directives come from
openexchangerates.org as `USD EUR`, but the journal uses the `€`
symbol. The alias normalises `EUR` → `€` at parse time so both
meet in the price DB.

## Native balance (no conversion)

```
$ acc -f journal.ledger bal
 $2000.00
 €-570.00 assets
 $2000.00   checking
 €-570.00   eu-bank
 €1500.00 expenses
 €1500.00   rent
$-3000.00 income
$-3000.00   salary
---------
$-1000.00
  €930.00
```

Each account shows every commodity it holds.

## `-x €` — per-posting at `tx.date` (the default)

```
$ acc -f journal.ledger bal -x €
 €1240.00 assets
 €1810.00   checking
 €-570.00   eu-bank
 €1500.00 expenses
 €1500.00   rent
€-2760.00 income
€-2760.00   salary
---------
  €-20.00
```

Each posting converts at its own transaction's date. A January
posting uses the 2024-01-15 rate, a June posting uses 2024-06-15,
and so on. **This is historically stable** — the same journal plus
the same rate files will always produce the same report, next year
or five years from now. Past expenses never retroactively revalue
when today's rate moves.

The grand total `€-20.00` is the translation drift from the
cross-currency transfer: the `$-1000` and `€930` postings converted
independently at the same-day rate but the transaction's own
implied rate (930/1000 = 0.93) matched the market (0.93) — no
drift on this specific tx. The `€-20` comes from the rest of the
journal's per-account drift under historical conversion. See
[05-cta.md](05-cta.md) for how to absorb that cleanly.

## `-x $` — same journal, opposite direction

```
$ acc -f journal.ledger bal -x $
 $1366.04 assets
 $2000.00   checking
 $-633.96   eu-bank
 $1612.90 expenses
 $1612.90   rent
$-3000.00 income
$-3000.00   salary
---------
  $-21.05
```

USD postings stay, EUR postings convert to USD via the inverse rate.
Different grand total because rate movement distributes the drift
differently in the opposite direction.

## `--market [DATE]` — snapshot revaluation

Convert every posting at **one** fixed date's rate:

```
$ acc -f journal.ledger bal -x € --market 2024-12-15
 €1330.00 assets
 €1900.00   checking
 €-570.00   eu-bank
 €1500.00 expenses
 €1500.00   rent
€-2850.00 income
€-2850.00   salary
---------
  €-20.00
```

Good for:
- Year-end financial statements at a consolidation rate.
- "What's my portfolio worth *today*?" reports.
- Matching how ledger-cli's `-V` default works.

Trade-off: the income statement no longer stable. Rerun the report
next year with a different `--market` date and `income:salary`
shows a different number — the 2024 salary wasn't physically
different, only its current-rate-translated value moved.

Without a date argument, `--market` = today:

```
acc bal -x € --market
```

## Multi-hop rate lookups

If no direct `P BASE QUOTE` exists, acc runs BFS over the commodity
graph. Inverse rates are computed on demand.

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

P 2024-06-15 USD EUR 0.93
P 2024-06-15 BTC USDT 60000

2024-06-15 * bitcoin purchase
    assets:wallet       BTC 0.1
    assets:cash          $-6000.00
```

```
$ acc -f journal.ledger bal -x €
€-5580.00 cash
 €5580.00 wallet
---------
        0
```

`BTC → €` works via `BTC → $ → €`. Both `alias USD` and `alias
USDT` route to `$` during resolve, so the P-directive's `USDT`
quote end merges into the same node as the posting's `$`.
