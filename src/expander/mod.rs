//! Expander phase — apply automated-transaction rules.
//!
//! Runs between the booker and the filter. For every regular
//! transaction, every posting is matched against every `auto_rules`
//! pattern; when the posting's account matches, the rule's extra
//! postings are appended to the transaction with their amounts
//! scaled by the triggering posting's amount.
//!
//! Multipliers are required by the resolver to sum to zero per
//! rule, which means the expanded postings net out on their own —
//! the transaction stays balanced after expansion. Auto-rule
//! postings don't re-trigger expansion (no recursive matching on
//! injected postings).

use crate::parser::entry::AutoRule;
use crate::parser::located::Located;
use crate::parser::posting::{Amount, Posting};
use crate::parser::transaction::Transaction;

pub fn expand(transactions: &mut [Located<Transaction>], auto_rules: &[AutoRule]) {
    if auto_rules.is_empty() {
        return;
    }
    for lt in transactions.iter_mut() {
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
                for auto_posting in &rule.postings {
                    let scaled_value = trigger_amount.value.mul_rounded(auto_posting.multiplier);
                    let new_posting = Posting {
                        account: auto_posting.account.clone(),
                        amount: Some(Amount {
                            commodity: trigger_amount.commodity.clone(),
                            value: scaled_value,
                            decimals: trigger_amount.decimals,
                        }),
                        costs: None,
                        lot_cost: None,
                        balance_assertion: None,
                        is_virtual: auto_posting.is_virtual,
                        balanced: auto_posting.balanced,
                        comments: Vec::new(),
                    };
                    injected.push(Located {
                        file: lt.file.clone(),
                        line: lt.line,
                        value: new_posting,
                    });
                }
            }
        }
        lt.value.postings.extend(injected);
    }
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
            = /^rud:cc:cash/\n\
            \t[rud:cc:cash]  -1\n\
            \t[ex:cash]       1\n\
            \n\
            2024-06-15 * coffee\n\
            \trud:cc:cash   $5\n\
            \tex:food       $-5\n";
        let (mut transactions, rules) = setup(src);
        expand(&mut transactions, &rules);

        let tx = &transactions[0].value;
        // Original 2 postings + 2 injected = 4.
        assert_eq!(tx.postings.len(), 4);
        let cash_flush = &tx.postings[2].value;
        assert_eq!(cash_flush.account, "rud:cc:cash");
        assert_eq!(
            cash_flush.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(-5)
        );
        let ex_cash = &tx.postings[3].value;
        assert_eq!(ex_cash.account, "ex:cash");
        assert_eq!(
            ex_cash.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(5)
        );
    }

    #[test]
    fn rule_without_match_does_nothing() {
        let src = "\
            = /^rud:cc:cash/\n\
            \t[rud:cc:cash]  -1\n\
            \t[ex:cash]       1\n\
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
            = /^in:gross/\n\
            \t[in:gross]  -1\n\
            \t[rud:vat19]  0.19\n\
            \t[rud:net]    0.81\n\
            \n\
            2024-06-15 * invoice\n\
            \tin:gross    $-1000\n\
            \tas:bank      $1000\n";
        let (mut transactions, rules) = setup(src);
        expand(&mut transactions, &rules);
        let tx = &transactions[0].value;
        // Original 2 + injected 3 = 5.
        assert_eq!(tx.postings.len(), 5);
        // `in:gross -1000 * -1 = 1000` back on in:gross.
        let flush = &tx.postings[2].value;
        assert_eq!(flush.account, "in:gross");
        assert_eq!(
            flush.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(1000)
        );
        // VAT: -1000 * 0.19 = -190
        let vat = &tx.postings[3].value;
        assert_eq!(vat.account, "rud:vat19");
        assert_eq!(
            vat.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(-190)
        );
        // Net: -1000 * 0.81 = -810
        let net = &tx.postings[4].value;
        assert_eq!(net.account, "rud:net");
        assert_eq!(
            net.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(-810)
        );
    }

    #[test]
    fn auto_postings_do_not_retrigger_in_same_tx() {
        // Rule matches any posting containing "cash". Without the
        // snapshot, the injected `[ex:cash]` posting would re-match
        // and blow up. The expander must only scan original postings.
        let src = "\
            = /cash/\n\
            \t[rud:cc:cash]  -1\n\
            \t[ex:cash]       1\n\
            \n\
            2024-06-15 * coffee\n\
            \trud:cc:cash   $5\n\
            \tex:food       $-5\n";
        let (mut transactions, rules) = setup(src);
        expand(&mut transactions, &rules);
        // Only one expansion — 2 original + 2 injected = 4. Not
        // 2 + 2 + 2 + … runaway growth.
        assert_eq!(transactions[0].value.postings.len(), 4);
    }
}
