# 06 — Lots and cost annotations

`@` / `@@` annotate the price paid for a commodity at transaction
time. `{COST}` attaches a lot basis to held positions, which the
booker uses when you sell part of a lot later.

## `@` per-unit cost

```
commodity $
    alias USD
commodity €
    alias EUR

2024-06-15 * euro purchase at today's rate
    assets:eur      €1000.00 @ $1.08
    assets:usd     $-1080.00

2024-07-01 * euro purchase total-cost form
    assets:eur       €500.00 @@ $540.00
    assets:usd      $-540.00
```

`@ $1.08` means *per unit*: 1000 EUR × $1.08 = $1080 effective for
balance. `@@ $540.00` means *total*: the whole EUR amount cost
$540 regardless of how it's split per unit.

```
$ acc -f journal.ledger bal
$-1620.00
 €1500.00 assets
 €1500.00   eur
$-1620.00   usd
---------
$-1620.00
 €1500.00
```

Account balances in native commodities. The cost annotations only
affect balance *validation* during parsing — the booker uses
`1000 × 1.08 = 1080` to confirm the first transaction balances
(1080 − 1080 = 0) without needing each side to already be in the
same commodity.

## `{COST}` — lot basis tracking

For holdings you'll sell later (stocks, crypto, real estate), tag
each acquisition with its cost basis so sell-from-lot math works.

```
commodity $
    alias USD
commodity BTC
    precision 8

2024-01-15 * buy BTC at 30k
    assets:btc        BTC 0.5 {$30000}
    assets:cash        $-15000.00

2024-06-15 * buy more BTC at 40k
    assets:btc        BTC 0.5 {$40000}
    assets:cash        $-20000.00

2024-12-15 * sell 0.3 BTC from 30k lot at 60k per
    assets:btc       BTC -0.3 {$30000} @ $60000
    assets:cash       $18000.00
    income:gain       $-9000.00
```

- Two buys build up 1 BTC total across two lots: 0.5 @ $30k and
  0.5 @ $40k.
- The sell posting specifies `{$30000}` — pulls from that lot, not
  FIFO-implicit. `@ $60000` is the sale price.
- Explicit `income:gain $-9000.00` records the realised gain
  (0.3 × (60000 − 30000) = $9000).

```
$ acc -f journal.ledger bal
   $-17000.00
BTC0.70000000 assets
BTC0.70000000   btc
   $-17000.00   cash
    $-9000.00 income
    $-9000.00   gain
```

`assets:btc` holds **0.7 BTC** — 0.2 left from the 30k lot plus
0.5 from the 40k lot. `income:gain` carries the realised capital
gain.

```
$ acc -f journal.ledger reg
2024-01-15 * buy BTC at 30k           assets:btc    BTC0.50000000  BTC0.50000000
                                      assets:cash      $-15000.00     $-15000.00
                                                                   BTC0.50000000
2024-06-15 * buy more BTC at 40k      assets:btc    BTC0.50000000     $-15000.00
                                                                   BTC1.00000000
                                      assets:cash      $-20000.00     $-35000.00
                                                                   BTC1.00000000
2024-12-15 * sell 0.3 BTC from 30k …  assets:btc   BTC-0.30000000     $-35000.00
                                                                   BTC0.70000000
                                      assets:cash       $18000.00     $-17000.00
                                                                   BTC0.70000000
                                      income:gain       $-9000.00     $-26000.00
                                                                   BTC0.70000000
```

Running totals in the rightmost column show the BTC position
accumulating then partially liquidating.

## `{=COST}` — fixed vs floating lot

- `{$30000}` — **floating** lot cost. Reports that revalue can
  update the cost basis.
- `{=$30000}` — **fixed** lot cost. Pinned; revaluation can't
  touch it. Use this when you want the cost basis to stay stable
  forever for gain calculations.

The booker uses lot cost (when present) as the balance-effective
cost, overriding any `@`-cost on the same posting. This matches
Ledger's sell-from-lot semantics.

## Cost annotations that parse but aren't yet modelled

- `{{TOTAL}}` — total-cost variant of `{}`. Parser consumes it;
  the booker treats it like the per-unit form (doesn't scale by
  amount).
- `[DATE]` — acquisition date tag after a lot cost. Parsed and
  discarded.

These exist for format compatibility with journals written for
ledger-cli; they don't yet drive acc-specific report behaviour.
