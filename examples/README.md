# Examples

Each file walks through one feature-area with an inline journal,
the exact commands that work against it, and the output acc
produces. Copy-paste the journal into a `.ledger` file to follow
along, or just read through — every journal and output is
verbatim.

| File | Topic |
|------|-------|
| [01-basics.md](01-basics.md) | `balance`, `register`, `print`, `accounts`, `commodities`, `codes` on a single-currency journal |
| [02-filters.md](02-filters.md) | Account / description / code / commodity patterns, combinators, `-r`, `-R`, multi-`-p`, date ranges |
| [03-currency-conversion.md](03-currency-conversion.md) | `-x`, per-tx.date default, `--market [DATE]`, multi-hop rate lookups |
| [04-fx-gain-loss.md](04-fx-gain-loss.md) | Realising gain/loss on multi-commodity trades via `fx gain` / `fx loss` |
| [05-cta.md](05-cta.md) | **CTA** — IAS 21 / ASC 830 Cumulative Translation Adjustment for transit accounts |
| [06-lots-and-costs.md](06-lots-and-costs.md) | `@` / `@@` cost annotations, `{COST}` lots, sell-from-lot math |
| [07-assertions.md](07-assertions.md) | Balance assertions `= AMT` and balance assignments |
