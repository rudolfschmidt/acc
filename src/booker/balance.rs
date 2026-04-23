//! Transaction-local balance math.
//!
//! Sums explicit amounts per commodity, ensures each commodity's sum
//! is zero, and infers the one allowed omitted amount.
//!
//! Cost annotations (`@` / `@@`) participate: a posting with a cost
//! contributes its cost-converted value in the cost commodity. The
//! posting keeps its original amount on the output side; only the
//! balance arithmetic uses the effective form.

use std::collections::HashMap;
use std::sync::Arc;

use crate::decimal::Decimal;
use crate::parser::posting::{Amount, Costs, Posting};
use crate::parser::transaction::Transaction;

use super::error::{BookError, BookErrorKind, Residual};

/// Balance one transaction. If one posting lacks an amount, it is
/// inferred as the negated sum of the others (in the cost-effective
/// commodity if any `@`/`@@` annotations are in play). If all
/// postings carry amounts, each commodity's sum must be zero.
///
/// Assumes any balance-assignment posting (`= X` without amount) has
/// already been resolved by the caller. Balance-assertion-only
/// postings (`= X` without amount, which the caller could not resolve)
/// are skipped — they contribute nothing and are not the omitted
/// inference target.
pub(super) fn balance_tx(
    tx: &mut Transaction,
    file: &Arc<str>,
    start_line: usize,
    end_line: usize,
) -> Result<(), BookError> {
    let mut sums: HashMap<String, Decimal> = HashMap::new();
    let mut max_decimals: HashMap<String, usize> = HashMap::new();
    let mut missing_idx: Option<usize> = None;

    let err = |kind: BookErrorKind| BookError::new(file.clone(), start_line, end_line, kind);

    for (i, lp) in tx.postings.iter().enumerate() {
        let p = &lp.value;
        if p.is_virtual && !p.balanced {
            continue;
        }
        match effective_amount(p) {
            Some(eff) => {
                *sums.entry(eff.commodity.clone()).or_insert(Decimal::zero()) += eff.value;
                let entry = max_decimals.entry(eff.commodity.clone()).or_insert(0);
                if eff.decimals > *entry {
                    *entry = eff.decimals;
                }
            }
            None => {
                // A posting with no amount AND no assertion is a
                // candidate for inference. One with an assertion is a
                // still-unresolved balance-assignment that carries no
                // information here (the booker's resolution pass
                // should have filled it in, but we're defensive).
                if p.balance_assertion.is_some() {
                    continue;
                }
                if missing_idx.is_some() {
                    return Err(err(BookErrorKind::MultipleMissing));
                }
                missing_idx = Some(i);
            }
        }
    }

    if let Some(idx) = missing_idx {
        match sums.len() {
            0 => return Err(err(BookErrorKind::NoAmountsToInfer)),
            1 => {
                let (commodity, sum) = sums.into_iter().next().unwrap();
                let decimals = max_decimals.get(&commodity).copied().unwrap_or(0);
                tx.postings[idx].value.amount = Some(Amount {
                    commodity,
                    value: -sum,
                    decimals,
                });
            }
            _ => {
                // Multi-commodity inference — the omitted posting is
                // expanded into one posting per non-zero commodity,
                // each balancing its own commodity's sum. This matches
                // ledger-cli's behaviour: a trailing `expense  ` with
                // no amount after multi-commodity entries soaks up
                // every loose commodity.
                //
                // Account, virtual/balanced flags, comments and file
                // provenance of the original posting are preserved on
                // all replacements.
                let template = tx.postings[idx].clone();
                let mut rows: Vec<(String, Decimal)> = sums.into_iter().collect();
                rows.sort_by(|a, b| a.0.cmp(&b.0));
                let replacements: Vec<_> = rows
                    .into_iter()
                    .map(|(commodity, sum)| {
                        let decimals = max_decimals.get(&commodity).copied().unwrap_or(0);
                        let mut lp = template.clone();
                        lp.value.amount = Some(Amount {
                            commodity,
                            value: -sum,
                            decimals,
                        });
                        lp
                    })
                    .collect();
                tx.postings.splice(idx..=idx, replacements);
            }
        }
    } else if sums.len() == 1 {
        // Balance check only applies when — after cost resolution — a
        // single commodity remains. Multi-commodity transactions
        // without costs cannot balance numerically and are accepted
        // as-is; it is up to the author to add `@` / `@@` annotations
        // when cross-commodity balance enforcement is wanted.
        //
        // Tolerance: residuals that round to zero at the commodity's
        // display precision are accepted. Per-unit cost multiplication
        // (e.g. `0.26184800 BTC @ €11292.58`) produces trailing digits
        // beyond what the user wrote; any real bookkeeping error is
        // large enough to show up in the rounded display anyway.
        let (commodity, sum) = sums.into_iter().next().unwrap();
        let decimals = max_decimals.get(&commodity).copied().unwrap_or(0);
        if !sum.is_display_zero(decimals) {
            return Err(err(BookErrorKind::Unbalanced {
                residuals: vec![Residual {
                    commodity,
                    value: sum,
                    decimals,
                }],
            }));
        }
    }

    Ok(())
}

/// The amount a posting contributes to the balance sum.
///
/// Priority (highest first):
///
/// 1. **Lot cost** `{COST}` — the posting has a lot annotation, so
///    the effective value is `amount × lot_cost` in the lot's
///    commodity. This is Ledger's sell-from-lot semantics: the books
///    move at cost basis, not at the current market. Any `@` on the
///    same posting is the market sale price and participates in the
///    realizer (for gain tracking), not in the balance.
/// 2. **Per-unit cost** `@ UNIT` — effective is `amount × UNIT`.
/// 3. **Total cost** `@@ TOTAL` — effective is `TOTAL`, carrying
///    the posting's sign.
/// 4. No annotation — posting contributes its own amount.
///
/// Returns `None` only when the posting has no amount.
fn effective_amount(p: &Posting) -> Option<Amount> {
    let amt = p.amount.as_ref()?;
    // `decimals` on the effective amount drives the balance-check
    // tolerance (via `is_display_zero`). Cost-derived effective
    // amounts carry no user-visible precision in the target
    // commodity — the user wrote e.g. `0.26184800 BTC`, not the
    // resulting `€...`. So cost-paths set `decimals: 0`; the
    // max-decimals tracker then picks up the tolerance from any
    // *direct* posting in the same commodity (e.g. a plain `€44.06`
    // in the same tx), defaulting to 0 if the commodity only ever
    // appears via cost conversion.
    if let Some(lot) = &p.lot_cost {
        let cost = lot.amount();
        return Some(Amount {
            commodity: cost.commodity.clone(),
            value: amt.value.mul_rounded(cost.value),
            decimals: 0,
        });
    }
    Some(match &p.costs {
        None => amt.clone(),
        Some(Costs::PerUnit(cost)) => Amount {
            commodity: cost.commodity.clone(),
            value: amt.value.mul_rounded(cost.value),
            decimals: 0,
        },
        Some(Costs::Total(cost)) => {
            let signed = if amt.value.is_negative() {
                -cost.value
            } else {
                cost.value
            };
            Amount {
                commodity: cost.commodity.clone(),
                value: signed,
                decimals: 0,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use crate::parser::located::Located;
    use crate::resolver;

    /// Isolates the tx-local balance step from booker's cross-tx
    /// machinery. The source must parse into exactly one transaction.
    fn balance_one(src: &str) -> Result<Transaction, BookError> {
        let entries = parser::parse(src).unwrap();
        let resolved = resolver::resolve(entries).unwrap();
        let mut first: Located<Transaction> = resolved.transactions.into_iter().next().unwrap();
        let end = first
            .value
            .postings
            .iter()
            .map(|p| p.line)
            .max()
            .unwrap_or(first.line);
        balance_tx(&mut first.value, &first.file, first.line, end)?;
        Ok(first.value)
    }

    #[test]
    fn infers_single_missing_amount() {
        let src = "2024-06-15 * Coffee\n    expenses:food   5 USD\n    assets:cash\n";
        let tx = balance_one(src).unwrap();
        let inferred = tx.postings[1].value.amount.as_ref().unwrap();
        assert_eq!(inferred.commodity, "USD");
        assert_eq!(inferred.value, Decimal::from(-5));
    }

    #[test]
    fn inferred_amount_inherits_max_decimals() {
        let src = "2024-06-15 * X\n    expenses:food   5.00 USD\n    assets:cash\n";
        let tx = balance_one(src).unwrap();
        assert_eq!(tx.postings[1].value.amount.as_ref().unwrap().decimals, 2);
    }

    #[test]
    fn accepts_already_balanced() {
        let src = "2024-06-15 * X\n    expenses:food   5 USD\n    assets:cash  -5 USD\n";
        assert!(balance_one(src).is_ok());
    }

    #[test]
    fn errors_on_multiple_missing() {
        let src = "2024-06-15 * X\n    expenses:food\n    assets:cash\n";
        let err = balance_one(src).unwrap_err();
        assert!(matches!(err.kind, BookErrorKind::MultipleMissing));
    }

    #[test]
    fn errors_on_unbalanced_sum() {
        let src = "2024-06-15 * X\n    expenses:food   5 USD\n    assets:cash  -3 USD\n";
        let err = balance_one(src).unwrap_err();
        assert!(matches!(err.kind, BookErrorKind::Unbalanced { .. }));
    }

    #[test]
    fn missing_in_multi_commodity_expands_to_per_commodity_postings() {
        // Trailing posting without an amount soaks up every commodity
        // in the transaction — ledger-cli semantics. `assets:other`
        // expands into two postings: one in USD, one in EUR.
        let src = "2024-06-15 * X\n    expenses:food   5 USD\n    assets:eur  -5 EUR\n    assets:other\n";
        let tx = balance_one(src).unwrap();
        // Original 3 postings → 4 after expansion.
        assert_eq!(tx.postings.len(), 4);
        // Last two are both `assets:other`, one per commodity.
        let last_two = &tx.postings[2..];
        assert!(last_two.iter().all(|lp| lp.value.account == "assets:other"));
        let commodities: Vec<&str> = last_two
            .iter()
            .map(|lp| lp.value.amount.as_ref().unwrap().commodity.as_str())
            .collect();
        assert!(commodities.contains(&"EUR"));
        assert!(commodities.contains(&"USD"));
        // Values negate each commodity's sum.
        for lp in last_two {
            let amt = lp.value.amount.as_ref().unwrap();
            match amt.commodity.as_str() {
                "USD" => assert_eq!(amt.value, Decimal::from(-5)),
                "EUR" => assert_eq!(amt.value, Decimal::from(5)),
                _ => panic!("unexpected commodity"),
            }
        }
    }

    #[test]
    fn multi_commodity_balanced_is_accepted() {
        let src = "2024-06-15 * X\n    a:x  5 USD\n    a:y  -5 USD\n    a:z  10 EUR\n    a:w  -10 EUR\n";
        assert!(balance_one(src).is_ok());
    }

    #[test]
    fn parens_virtual_excluded_from_balance_check() {
        let src = "2024-06-15 * X\n    (virtual:off)  -5 USD\n    expenses:food   5 USD\n    assets:cash  -5 USD\n";
        assert!(balance_one(src).is_ok());
    }

    #[test]
    fn bracket_virtual_participates_in_balance() {
        let src = "2024-06-15 * X\n    [virtual:on]  5 USD\n    assets:cash  -5 USD\n";
        assert!(balance_one(src).is_ok());
    }

    #[test]
    fn per_unit_cost_balances_across_commodities() {
        let src = "2024-06-15 * X\n    expenses:food  5 USD @ 0.92 EUR\n    assets:eur    -4.60 EUR\n";
        assert!(balance_one(src).is_ok());
    }

    #[test]
    fn total_cost_balances() {
        let src = "2024-06-15 * X\n    expenses:food  5 USD @@ 4.60 EUR\n    assets:eur    -4.60 EUR\n";
        assert!(balance_one(src).is_ok());
    }

    #[test]
    fn per_unit_cost_infers_missing_in_cost_commodity() {
        let src = "2024-06-15 * X\n    expenses:food  5 USD @ 0.92 EUR\n    assets:eur\n";
        let tx = balance_one(src).unwrap();
        let inferred = tx.postings[1].value.amount.as_ref().unwrap();
        assert_eq!(inferred.commodity, "EUR");
        assert_eq!(inferred.value, Decimal::parse("-4.60").unwrap());
    }

    #[test]
    fn per_unit_cost_unbalanced_errors() {
        let src = "2024-06-15 * X\n    expenses:food  5 USD @ 0.92 EUR\n    assets:eur    -4 EUR\n";
        assert!(balance_one(src).is_err());
    }

    #[test]
    fn total_cost_with_negative_posting() {
        let src = "2024-06-15 * X\n    assets:usd    -5 USD @@ 4.60 EUR\n    assets:eur     4.60 EUR\n";
        assert!(balance_one(src).is_ok());
    }

    #[test]
    fn lot_cost_overrides_at_cost_for_balance() {
        // ETH -1 {BTC 0.0904} @ BTC 0.0907 + BTC 0.0907 + in:trade BTC -0.0003.
        // Booker must use the lot cost (0.0904) for the ETH-side, so
        // sum = -0.0904 + 0.0907 + (-0.0003) = 0. Without lot-cost
        // priority it would use @-cost (0.0907) and report residual.
        let src = "2018-01-18 * sell\n\
                   \tassets:eth  ETH-1 {BTC 0.0904} @ BTC 0.0907\n\
                   \tassets:btc  BTC 0.0907\n\
                   \tin:trade    BTC -0.0003\n";
        assert!(balance_one(src).is_ok());
    }

    #[test]
    fn lot_cost_fixed_variant_also_used() {
        // `{=COST}` (fixed) must work the same as `{COST}` for balance.
        let src = "2018-01-18 * sell\n\
                   \tassets:eth  ETH-1 {=BTC 0.0904} @ BTC 0.0907\n\
                   \tassets:btc  BTC 0.0907\n\
                   \tin:trade    BTC -0.0003\n";
        assert!(balance_one(src).is_ok());
    }

    #[test]
    fn high_precision_lot_cost_does_not_tighten_tolerance() {
        // `CZK -7524 {=€0.0380117}` computes a lot cost of
        // €-286.0000308 against `€286.00` → residual 0.0000308 €.
        // That must round to display-zero at the amount's 0 decimals
        // (and the journal-wide 2 decimals for €), not at the cost's
        // 7 decimals.
        let src = "2020-09-02 * wizzair\n\
                   \tassets:czk   CZK-7524 {=€0.0380117}\n\
                   \texpenses:t   €286.00\n";
        assert!(balance_one(src).is_ok());
    }

    #[test]
    fn high_precision_source_does_not_tighten_tolerance() {
        // `0.26184800 BTC @ €11292.58` converts to €2956.9394878400.
        // Against -3001.00 + 44.06 € the residual is ~0.00051216 €,
        // which must round to display-zero at the explicit €-posting
        // decimals (2), not inherit BTC's 8 decimals.
        let src = "2018-01-11 * rs\n\
                   \tassets:btc   BTC 0.26184800 @ €11292.58\n\
                   \tassets:eur   €-3001.00\n\
                   \texpenses:t   €44.06\n";
        assert!(balance_one(src).is_ok());
    }

    #[test]
    fn residual_within_display_precision_is_accepted() {
        // 0.26184800 BTC @ €11292.58 = 2956.9394878400 €
        // against -3001.00 + 44.06 = -2956.94 → residual -0.0005 €
        // Rounded to 2 decimals that's 0.00, so the tx balances.
        let src = "2024-06-15 * rounding\n\
                   \tassets:btc   0.26184800 BTC @ €11292.58\n\
                   \tassets:eur   €-3001.00\n\
                   \texpenses:fee €44.06\n";
        assert!(balance_one(src).is_ok());
    }

    #[test]
    fn assertion_only_posting_does_not_participate() {
        let src = "2024-06-15 * X\n    assets:bank    = 100 USD\n    assets:bank    5 USD\n    expenses:food -5 USD\n";
        let tx = balance_one(src).unwrap();
        assert!(tx.postings[0].value.amount.is_none());
    }
}
