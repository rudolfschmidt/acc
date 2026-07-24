//! Expander phase — apply automated-transaction rules.
//!
//! Runs between the booker and the filter. For every regular
//! transaction, every posting is matched against every `auto_rules`
//! pattern; when the posting's account matches, the rule's extra
//! postings are appended to the transaction with their amounts
//! scaled by the triggering posting's amount.
//!
//! The resolver guarantees each rule's injected postings net to zero
//! per balance pool — either the `Factor` multipliers sum to zero, or a
//! bare `Fill` leg is filled here with the negated pool sum — so the
//! transaction stays balanced after expansion. Auto-rule postings don't
//! re-trigger expansion (no recursive matching on injected postings).
//!
//! An injected posting's account may contain `$account`, replaced
//! with the *triggering* posting's account (ledger's `[$account]`:
//! refer to the matched account itself). So one rule can flush each
//! of `assets:cash-eur`, `assets:cash-usd`, … back to its own
//! specific account. The substitution is textual, so `$account`
//! works as the whole account or embedded (`Budget:$account`).

use crate::parser::entry::{AutoAmount, AutoRule};
use crate::parser::located::Located;
use crate::parser::posting::{Amount, Posting};
use crate::parser::transaction::Transaction;

pub fn expand(transactions: &mut [Located<Transaction>], auto_rules: &[AutoRule]) {
    if auto_rules.is_empty() {
        return;
    }
    // Running per-account balance, accumulated in the transactions' (date-sorted)
    // order and including the postings injected here — a `clamp(...)` leg reads
    // it to fill only the account's remaining headroom to zero.
    let mut balances: std::collections::HashMap<String, crate::decimal::Decimal> =
        std::collections::HashMap::new();
    for lt in transactions.iter_mut() {
        let date_str = lt.value.date.to_string();
        let mut injected: Vec<Located<Posting>> = Vec::new();
        // Snapshot the triggering postings up front so the injected
        // ones (appended below) don't themselves kick off another
        // round of matching within this same transaction.
        let original_count = lt.value.postings.len();
        for idx in 0..original_count {
            let trigger_amount = match &lt.value.postings[idx].value.amount {
                Some(a) => a.clone(),
                None => continue, // no amount = nothing to scale
            };
            let trigger_account = lt.value.postings[idx].value.account.clone();
            for rule in auto_rules {
                if !rule.pattern.matches(&trigger_account) {
                    continue;
                }
                // Optional `amount <op> N` clause: only fire when the matched
                // posting's amount satisfies it — e.g. reconcile's `amount > 0`
                // counts a send but skips the negative transit-clearing posting.
                if let Some(cond) = &rule.condition
                    && !cond.matches(&trigger_amount.value)
                {
                    continue;
                }
                for posting in
                    inject_rule(rule, &trigger_amount, &trigger_account, &date_str, &balances)
                {
                    injected.push(Located {
                        file: lt.file.clone(),
                        line: lt.line,
                        value: posting,
                    });
                }
            }
        }
        lt.value.postings.extend(injected);
        // Fold this transaction's postings (source + injected) into the running
        // balances so a later clamp sees their effect. Real and balanced-virtual
        // `[...]` postings count toward an account balance; `(...)` don't.
        for lp in &lt.value.postings {
            let p = &lp.value;
            if p.is_virtual && !p.balanced {
                continue;
            }
            if let Some(a) = &p.amount {
                *balances.entry(p.account.clone()).or_default() += a.value;
            }
        }
    }
}

/// Build the postings one matching rule injects for a single trigger. `Factor`
/// legs scale the trigger amount; a bare `Fill` leg is filled with the negated
/// sum of its own balance pool (real or balanced-virtual `[...]`), so the
/// injected set nets to zero on its own — exactly like the bare last posting of
/// a hand-written transaction. `$account` in an account resolves to the matched
/// posting's account.
fn inject_rule(
    rule: &AutoRule,
    trigger_amount: &Amount,
    trigger_account: &str,
    date_str: &str,
    balances: &std::collections::HashMap<String, crate::decimal::Decimal>,
) -> Vec<Posting> {
    use crate::decimal::Decimal;
    // First pass: resolve accounts and value the `Factor`/`Clamp` legs, summing
    // per pool.
    let mut real_sum = Decimal::zero();
    let mut virt_sum = Decimal::zero();
    let mut legs: Vec<(String, bool, bool, Option<Decimal>)> =
        Vec::with_capacity(rule.postings.len());
    for ap in &rule.postings {
        // Resolve the account: the matched account for `$account`, then the
        // triggering transaction's date parts for `$year`/`$month`/`$day`.
        let mut account = ap.account.replace("$account", trigger_account);
        crate::resolver::substitute_date_vars(&mut account, date_str);
        let value = match &ap.amount {
            AutoAmount::Factor(f) => Some(trigger_amount.value.mul_rounded(*f)),
            AutoAmount::Clamp { negate } => {
                // Headroom = the account's current positive balance (once it hits
                // zero there is nothing left to absorb). Clamp the trigger
                // magnitude to it; `-clamp(...)` books it negative.
                let balance = balances.get(&account).copied().unwrap_or_else(Decimal::zero);
                let headroom = if balance > Decimal::zero() { balance } else { Decimal::zero() };
                let mag = {
                    let v = trigger_amount.value;
                    if v < Decimal::zero() { -v } else { v }
                };
                let e = if mag < headroom { mag } else { headroom };
                Some(if *negate { -e } else { e })
            }
            AutoAmount::Fill => None,
        };
        if let Some(v) = value {
            if !ap.is_virtual {
                real_sum += v;
            } else if ap.balanced {
                virt_sum += v;
            }
        }
        legs.push((account, ap.is_virtual, ap.balanced, value));
    }
    // Second pass: each bare `Fill` leg takes the negated sum of its own pool.
    legs.into_iter()
        .map(|(account, is_virtual, balanced, value)| {
            let value = value.unwrap_or_else(|| {
                let pool_sum = if !is_virtual { real_sum } else { virt_sum };
                -pool_sum
            });
            Posting {
                account,
                amount: Some(Amount {
                    commodity: trigger_amount.commodity.clone(),
                    value,
                    decimals: trigger_amount.decimals,
                }),
                costs: None,
                lot_cost: None,
                lot_date: None,
                balance_assertion: None,
                is_virtual,
                balanced,
                comments: Vec::new(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use crate::resolver;

    fn setup(src: &str) -> (Vec<Located<Transaction>>, Vec<AutoRule>) {
        let entries = parser::parse(src).unwrap();
        let resolved = resolver::resolve(entries).unwrap();
        let transactions = crate::booker::book(resolved.transactions).unwrap();
        (transactions, resolved.auto_rules)
    }

    #[test]
    fn cash_flush_pattern() {
        let src = "\
            = /^assets:cash/\n\
            \t[assets:cash]  -1\n\
            \t[expenses:cash]       1\n\
            \n\
            2024-06-15 * coffee\n\
            \tassets:cash   $5\n\
            \texpenses:food       $-5\n";
        let (mut transactions, rules) = setup(src);
        expand(&mut transactions, &rules);

        let tx = &transactions[0].value;
        // Original 2 postings + 2 injected = 4.
        assert_eq!(tx.postings.len(), 4);
        let cash_flush = &tx.postings[2].value;
        assert_eq!(cash_flush.account, "assets:cash");
        assert_eq!(
            cash_flush.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(-5)
        );
        let ex_cash = &tx.postings[3].value;
        assert_eq!(ex_cash.account, "expenses:cash");
        assert_eq!(
            ex_cash.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(5)
        );
    }

    #[test]
    fn rule_without_match_does_nothing() {
        let src = "\
            = /^assets:cash/\n\
            \t[assets:cash]  -1\n\
            \t[expenses:cash]       1\n\
            \n\
            2024-06-15 * rent\n\
            \tassets:bank   $-1000\n\
            \texpenses:rent  $1000\n";
        let (mut transactions, rules) = setup(src);
        let before = transactions[0].value.postings.len();
        expand(&mut transactions, &rules);
        assert_eq!(transactions[0].value.postings.len(), before);
    }

    #[test]
    fn fractional_multiplier_vat_split() {
        // Gross income split 19% / 81%.
        let src = "\
            = /^income:gross/\n\
            \t[income:gross]  -1\n\
            \t[liabilities:vat]  0.19\n\
            \t[income:net]    0.81\n\
            \n\
            2024-06-15 * invoice\n\
            \tincome:gross    $-1000\n\
            \tassets:bank      $1000\n";
        let (mut transactions, rules) = setup(src);
        expand(&mut transactions, &rules);
        let tx = &transactions[0].value;
        // Original 2 + injected 3 = 5.
        assert_eq!(tx.postings.len(), 5);
        // `income:gross -1000 * -1 = 1000` back on income:gross.
        let flush = &tx.postings[2].value;
        assert_eq!(flush.account, "income:gross");
        assert_eq!(
            flush.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(1000)
        );
        // VAT: -1000 * 0.19 = -190
        let vat = &tx.postings[3].value;
        assert_eq!(vat.account, "liabilities:vat");
        assert_eq!(
            vat.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(-190)
        );
        // Net: -1000 * 0.81 = -810
        let net = &tx.postings[4].value;
        assert_eq!(net.account, "income:net");
        assert_eq!(
            net.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(-810)
        );
    }

    #[test]
    fn account_variable_refers_to_the_matched_account() {
        // `$account` is replaced with the *specific* matched account, so one
        // rule flushes each per-currency cash account to its own leg.
        let src = "\
            = /^assets:cash/\n\
            \t[$account]  -1\n\
            \t[expenses:cash]  1\n\
            \n\
            2024-06-15 * spend\n\
            \tassets:cash-eur   $5\n\
            \texpenses:food     $-5\n";
        let (mut transactions, rules) = setup(src);
        expand(&mut transactions, &rules);
        let tx = &transactions[0].value;
        assert_eq!(tx.postings.len(), 4);
        // Injected flush carries the matched account, not the literal
        // `$account`, and stays a balanced-virtual `[…]` posting.
        let flush = &tx.postings[2].value;
        assert_eq!(flush.account, "assets:cash-eur");
        assert!(flush.is_virtual);
        assert_eq!(tx.postings[3].value.account, "expenses:cash");
    }

    #[test]
    fn auto_postings_do_not_retrigger_in_same_tx() {
        // Rule matches any posting containing "cash". Without the
        // snapshot, the injected `[expenses:cash]` posting would re-match
        // and blow up. The expander must only scan original postings.
        let src = "\
            = /cash/\n\
            \t[assets:cash]  -1\n\
            \t[expenses:cash]       1\n\
            \n\
            2024-06-15 * coffee\n\
            \tassets:cash   $5\n\
            \texpenses:food       $-5\n";
        let (mut transactions, rules) = setup(src);
        expand(&mut transactions, &rules);
        // Only one expansion — 2 original + 2 injected = 4. Not
        // 2 + 2 + 2 + … runaway growth.
        assert_eq!(transactions[0].value.postings.len(), 4);
    }

    #[test]
    fn bare_leg_fills_to_the_negated_pool_sum() {
        // `-amount` on the flush leg + a bare balancing leg: the bare leg fills
        // to the negation, so the injected pair nets to zero.
        let src = "\
            = /^assets:cash/\n\
            \t[assets:cash]  -amount\n\
            \t[expenses:cash]\n\
            \n\
            2024-06-15 * spend\n\
            \tassets:cash   $5\n\
            \texpenses:food  $-5\n";
        let (mut transactions, rules) = setup(src);
        expand(&mut transactions, &rules);
        let tx = &transactions[0].value;
        assert_eq!(tx.postings.len(), 4);
        assert_eq!(tx.postings[2].value.account, "assets:cash");
        assert_eq!(
            tx.postings[2].value.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(-5)
        );
        // Bare leg filled to +5.
        assert_eq!(tx.postings[3].value.account, "expenses:cash");
        assert_eq!(
            tx.postings[3].value.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(5)
        );
    }

    #[test]
    fn clamp_limits_to_account_headroom() {
        // A +100 balance is seeded on `budget`; a 300 trigger clamps to the
        // remaining 100, and a later 50 trigger sees 0 headroom and injects 0.
        let src = "\
            = /^inter:send/\n\
            \t[budget]  -clamp(amount)\n\
            \t[absorbed]\n\
            \n\
            2024-01-01 * seed\n\
            \tbudget   $100\n\
            \tequity   $-100\n\
            \n\
            2024-02-01 * big\n\
            \tinter:send   $300\n\
            \tassets:bank  $-300\n\
            \n\
            2024-03-01 * over\n\
            \tinter:send   $50\n\
            \tassets:bank  $-50\n";
        let (mut transactions, rules) = setup(src);
        expand(&mut transactions, &rules);
        // Seed: no trigger.
        assert_eq!(transactions[0].value.postings.len(), 2);
        // Big: clamp to the 100 headroom, bare leg absorbs +100.
        let big = &transactions[1].value;
        assert_eq!(big.postings[2].value.account, "budget");
        assert_eq!(
            big.postings[2].value.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(-100)
        );
        assert_eq!(
            big.postings[3].value.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(100)
        );
        // Over: headroom exhausted, so the clamp injects 0.
        let over = &transactions[2].value;
        assert_eq!(
            over.postings[2].value.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(0)
        );
    }
}
