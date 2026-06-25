# 04 — Realised slippage gain / loss

Multi-commodity transactions — where you trade one commodity for
another — happen at the **implied rate** of the transaction (the
ratio of the two amounts). If that implied rate differs from the
market rate at the same date, you realised a gain or a loss.

acc's **realizer** phase books this automatically, **when the
journal declares both `slippage gain` and `slippage loss` accounts** and `-X`
is set.

## Setup

```
commodity $
    alias USD
commodity €
    alias EUR

account income:slippage
    slippage gain
account expenses:slippage
    slippage loss

P 2024-06-15 USD EUR 0.90
P 2024-12-15 USD EUR 0.92

2024-06-15 * sold USD for EUR at favourable rate
    assets:usd       $-1000.00
    assets:eur          €920.00

2024-12-15 * sold EUR for USD below market
    assets:eur         €-500.00
    assets:usd          $540.00
```

Trade 1: sold `$1000` for `€920`. Implied rate **0.92**. Market at
that date: **0.90**. Got more EUR than market → **gain**.

Trade 2: sold `€500` for `$540`. Implied rate **0.926**
(540/500 × 0.92 market for conversion). At market `0.92`: `€500`
= `$543.48`. Got `$540` → **loss of `$3.48`**.

## Under `-X €`

```
$ acc -f journal.ledger bal -X €
  €16.80 assets
 €420.00   eur
€-403.20   usd
   €3.20 expenses
   €3.20   slippage
 €-20.00 income
 €-20.00   slippage
--------
       0
```

Balanced. `€-20.00` on `income:slippage` (a credit, representing a
realised gain) and `€3.20` on `expenses:slippage` (a debit,
realised loss). Total effect: `€16.80` of real value captured, now
sitting on `assets:eur` / `assets:usd`.

## Register view — the injected slippage postings

```
$ acc -f journal.ledger reg -X €
2024-06-15 * sold USD for EUR at favourabl…  assets:usd       €-900.00  €-900.00
                                             assets:eur        €920.00    €20.00
                                             income:slippage     €-20.00         0
2024-12-15 * sold EUR for USD below market   assets:eur       €-500.00  €-500.00
                                             assets:usd        €496.80    €-3.20
                                             expenses:slippage     €3.20         0
```

The realizer adds a **real** third posting per trade:
`income:slippage` or `expenses:slippage`. Under historical conversion the
two traded legs don't balance in EUR — that's the whole point, the
implied rate diverged from market — so the injected posting absorbs
the difference, naming *what* the residual is and *where* it belongs
while leaving the transaction balanced and reloadable 1:1.

## Just the gain/loss view

```
$ acc -f journal.ledger bal income:slippage expenses:slippage -X €
  €3.20 expenses
  €3.20   slippage
€-20.00 income
€-20.00   slippage
-------
€-16.80
```

Combined P&L impact from foreign exchange trading.

## When the realizer runs

- Both `slippage gain` **and** `slippage loss` accounts must be declared.
- `-X TARGET` must be set.
- The transaction must have ≥2 distinct commodities.
- Market rate for every posting's commodity pair must be
  findable — if any rate is missing, the transaction is skipped
  (the realizer can't label what it can't price).
- The delta must exceed the target commodity's display precision
  (rounding noise is silently dropped).

## Realizer vs translator (CTA)

Different events:

| Scenario | Handled by | Mechanism |
|----------|------------|-----------|
| Multi-commodity trade where implied rate ≠ market rate | Realizer (`slippage gain` / `slippage loss`) | Real posting names the per-tx gain/loss |
| Single-commodity transit: money in at one rate, out at another | Translator ([05-cta.md](05-cta.md)) — `cta gain` / `cta loss` | Synthetic adjustment tx with real postings |

They never co-fire. acc's translator excludes any account+commodity
pair touched by a multi-commodity transaction — that's realizer
territory.
