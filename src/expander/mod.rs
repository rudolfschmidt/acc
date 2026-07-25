//! Expander phase — apply automated-transaction rules.
//!
//! Runs between the booker and the filter. For every regular
//! transaction, every posting is matched against every `auto_rules`
//! pattern; when the posting's account matches, the rule's extra
//! postings are appended to the transaction with their amount
//! expressions evaluated against the triggering posting.
//!
//! Each rule's injected postings net to zero per balance pool — either
//! the explicit amounts sum to zero, or a bare `Fill` leg (`amount: None`)
//! is filled here with the negated pool sum — so the transaction stays
//! balanced after expansion (the booker re-checks it). A leg whose amount
//! evaluates to zero injects nothing, so a spent cap (`min(...)` → 0)
//! adds no 0-amount posting. Auto-rule postings don't re-trigger expansion
//! (no recursive matching on injected postings).
//!
//! An injected posting's account may contain `$account`, replaced
//! with the *triggering* posting's account (ledger's `[$account]`:
//! refer to the matched account itself). So one rule can flush each
//! of `assets:cash-eur`, `assets:cash-usd`, … back to its own
//! specific account. The substitution is textual, so `$account`
//! works as the whole account or embedded (`Budget:$account`). Inside a
//! `total(ACCOUNT)` amount, `$self` is the leg's own account.

use crate::parser::entry::{AmountExpr, AutoRule};
use crate::parser::located::Located;
use crate::parser::posting::{Amount, Posting};
use crate::parser::transaction::Transaction;

pub fn expand(transactions: &mut [Located<Transaction>], auto_rules: &[AutoRule]) {
    if auto_rules.is_empty() {
        return;
    }
    // Running per-account balance, accumulated in the transactions' (date-sorted)
    // order and including the postings injected here — a `total(...)` leg reads
    // it to size against an account's current balance.
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
                let ctx = MatchContext {
                    trigger: &trigger_amount,
                    account: &trigger_account,
                    date: &date_str,
                    balances: &balances,
                };
                for posting in inject_rule(rule, &ctx) {
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
        // balances so a later `total(...)` sees their effect. Real and
        // balanced-virtual `[...]` postings count toward an account balance;
        // `(...)` don't.
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

/// What a firing auto-rule sees about its trigger — the value context every
/// injected amount expression is evaluated against (see [`eval_amount`]).
struct MatchContext<'a> {
    /// The matched (triggering) posting's amount — value and commodity.
    trigger: &'a Amount,
    /// The matched posting's account (substituted for `$account`).
    account: &'a str,
    /// The triggering transaction's date as `YYYY-MM-DD`
    /// (`$year` / `$month` / `$day`).
    date: &'a str,
    /// Running per-account balance up to (not including) this transaction —
    /// `total(ACCOUNT)` reads an account's current balance from it.
    balances: &'a std::collections::HashMap<String, crate::decimal::Decimal>,
}

/// Build the postings one matching rule injects for a single trigger. Every leg
/// with an amount expression is evaluated against the trigger; a bare `Fill` leg
/// (`amount: None`) takes the negated sum of its own balance pool (real or
/// balanced-virtual `[...]`), so the injected set nets to zero on its own — like
/// the bare last posting of a hand-written transaction. A leg that evaluates to
/// zero is dropped, so a spent cap injects nothing rather than a 0-amount
/// posting. `$account` in an account resolves to the matched posting's account,
/// date vars to its date.
fn inject_rule(rule: &AutoRule, ctx: &MatchContext) -> Vec<Posting> {
    use crate::decimal::Decimal;
    // First pass: resolve accounts and value every non-`Fill` leg, summing per
    // pool so a `Fill` leg can absorb the rest in pass two.
    let mut real_sum = Decimal::zero();
    let mut virt_sum = Decimal::zero();
    let mut legs: Vec<(String, bool, bool, Option<Decimal>)> =
        Vec::with_capacity(rule.postings.len());
    for ap in &rule.postings {
        // Resolve the account: the matched account for `$account`, then the
        // triggering transaction's date parts for `$year`/`$month`/`$day`.
        let mut account = ap.account.replace("$account", ctx.account);
        crate::resolver::substitute_date_vars(&mut account, ctx.date);
        // `amount: None` is the bare Fill leg — valued from the pool in pass two.
        let value = ap.amount.as_ref().map(|expr| eval_amount(expr, &account, ctx));
        if let Some(v) = value {
            if !ap.is_virtual {
                real_sum += v;
            } else if ap.balanced {
                virt_sum += v;
            }
        }
        legs.push((account, ap.is_virtual, ap.balanced, value));
    }
    // Second pass: each bare `Fill` leg takes the negated sum of its own pool;
    // drop any leg (explicit or fill) that comes out zero.
    legs.into_iter()
        .filter_map(|(account, is_virtual, balanced, value)| {
            let value = value.unwrap_or_else(|| {
                let pool_sum = if !is_virtual { real_sum } else { virt_sum };
                -pool_sum
            });
            if value.is_zero() {
                return None;
            }
            Some(Posting {
                account,
                amount: Some(Amount {
                    commodity: ctx.trigger.commodity.clone(),
                    value,
                    decimals: ctx.trigger.decimals,
                }),
                costs: None,
                lot_cost: None,
                lot_date: None,
                balance_assertion: None,
                is_virtual,
                balanced,
                comments: Vec::new(),
            })
        })
        .collect()
}

/// Evaluate one injected posting's amount expression against the match context.
/// `account` is the leg's already-resolved own account, read for `$self` inside
/// a `total(ACCOUNT)`. Arithmetic uses the decimal's rounding helpers so a
/// `total * factor` can't overflow the scale limit.
fn eval_amount(expr: &AmountExpr, account: &str, ctx: &MatchContext) -> crate::decimal::Decimal {
    use crate::decimal::Decimal;
    match expr {
        AmountExpr::Amount => ctx.trigger.value,
        AmountExpr::Literal(d) => *d,
        // `$n` params, `:=` bindings (`Var`) and value lookups are all rewritten
        // to literals by the resolver, so none of these reaches a matched rule —
        // value them as zero defensively.
        AmountExpr::Param(_) | AmountExpr::Var(_) | AmountExpr::Lookup(_) => Decimal::zero(),
        AmountExpr::Total(text) => {
            // `$self` = this leg's own account; `$account` = the trigger; then
            // the date parts. `$1`/`$2` were resolved by the resolver already.
            let mut acct = text.replace("$self", account).replace("$account", ctx.account);
            crate::resolver::substitute_date_vars(&mut acct, ctx.date);
            ctx.balances.get(&acct).copied().unwrap_or_else(Decimal::zero)
        }
        AmountExpr::Neg(a) => -eval_amount(a, account, ctx),
        AmountExpr::Min(a, b) => eval_amount(a, account, ctx).min(eval_amount(b, account, ctx)),
        AmountExpr::Max(a, b) => eval_amount(a, account, ctx).max(eval_amount(b, account, ctx)),
        AmountExpr::Add(a, b) => eval_amount(a, account, ctx) + eval_amount(b, account, ctx),
        AmountExpr::Sub(a, b) => eval_amount(a, account, ctx) - eval_amount(b, account, ctx),
        AmountExpr::Mul(a, b) => {
            eval_amount(a, account, ctx).mul_rounded(eval_amount(b, account, ctx))
        }
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
            = /^assets:cash/\n\
            \t[assets:cash]  -amount\n\
            \t[expenses:cash]       amount\n\
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
            \t[assets:cash]  -amount\n\
            \t[expenses:cash]       amount\n\
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
    fn fractional_multiplier_split() {
        // Gross income split into a 10% fee and the net.
        let src = "\
            = /^income:gross/\n\
            \t[income:gross]  -amount\n\
            \t[expenses:fee]  amount * 0.1\n\
            \t[income:net]    amount * 0.9\n\
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
        // Fee: -1000 * 0.1 = -100
        let fee = &tx.postings[3].value;
        assert_eq!(fee.account, "expenses:fee");
        assert_eq!(
            fee.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(-100)
        );
        // Net: -1000 * 0.9 = -900
        let net = &tx.postings[4].value;
        assert_eq!(net.account, "income:net");
        assert_eq!(
            net.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(-900)
        );
    }

    #[test]
    fn account_variable_refers_to_the_matched_account() {
        // `$account` is replaced with the *specific* matched account, so one
        // rule flushes each per-currency cash account to its own leg.
        let src = "\
            = /^assets:cash/\n\
            \t[$account]  -amount\n\
            \t[expenses:cash]  amount\n\
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
            \t[assets:cash]  -amount\n\
            \t[expenses:cash]       amount\n\
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
    fn min_expression_limits_to_account_balance_then_drops() {
        // `-min(amount, max(0, total($self)))` is the old `clamp`: a +100 balance
        // is seeded on `budget`, a 300 trigger fills only the remaining 100, and a
        // later 50 trigger sees 0 balance — the leg evaluates to zero and is
        // dropped, injecting nothing rather than a 0-amount posting.
        let src = "\
            = /^inter:send/\n\
            \t[budget]  -min(amount, max(0, total($self)))\n\
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
        // Big: fill to the 100 balance, bare leg absorbs +100.
        let big = &transactions[1].value;
        assert_eq!(big.postings.len(), 4);
        assert_eq!(big.postings[2].value.account, "budget");
        assert_eq!(
            big.postings[2].value.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(-100)
        );
        assert_eq!(
            big.postings[3].value.amount.as_ref().unwrap().value,
            crate::decimal::Decimal::from(100)
        );
        // Over: balance exhausted → both legs value to zero and are dropped, so
        // nothing is injected (2 original postings, unchanged).
        let over = &transactions[2].value;
        assert_eq!(over.postings.len(), 2);
    }
}
