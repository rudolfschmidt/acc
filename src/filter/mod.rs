//! Transaction filter.
//!
//! Accepts a user-provided pattern expression and an optional date
//! range, drops transactions and postings that do not match. Runs
//! between the loader and the commander. Pure transform — no I/O.
//!
//! ## Pattern syntax
//!
//! ```text
//! account            contains, case-insensitive (default dim)
//! ^account           starts-with
//! account$           ends-with
//! ^account$          exact
//! @foo               description contains "foo" (case-insensitive)
//! desc <foo>         same as @foo (keyword form for values with spaces)
//! #XYZ               transaction-code equals "XYZ" (case-insensitive)
//! code <XYZ>         same as #XYZ
//! com <GBP>          posting commodity equals "GBP" (case-insensitive)
//! not <pat>          negate the following single pattern
//! and / or           combinators. Default between bare tokens is OR.
//! ```
//!
//! Precedence: `or` (lowest) < `and` < `not` < primary.
//! Consecutive primaries with no explicit combinator are OR'd by
//! default. The parser is recursive-descent; the resulting `Query`
//! AST evaluates in a single tree walk per posting.
//!
//! Quoting for values containing spaces relies on the shell: e.g.
//! `desc "foo bar"` arrives as two args `["desc", "foo bar"]`;
//! `@"foo bar"` arrives as one arg `"@foo bar"`.

use crate::loader::Journal;
use crate::parser::located::Located;
use crate::parser::posting::Posting;
use crate::parser::transaction::Transaction;

mod amount;
mod pattern;

pub use amount::{AmountFilter, SignFilter};
pub use pattern::PatternMatcher;

/// Filter phase. Applies `patterns` and an optional `begin` / `end`
/// date range to the journal. Transactions outside the date range are
/// dropped; surviving transactions keep only postings that match the
/// pattern. Transactions that end up empty are dropped too.
///
/// `whole_transactions` flips that posting reduction off: a matched
/// transaction keeps *all* of its postings. This is what `print`
/// wants — show the complete entry whenever it matches — as opposed
/// to `reg` / `bal`, which show only the matched postings.
///
/// The non-transaction fields of `Journal` (prices, role accounts,
/// precisions) pass through unchanged — they are either global
/// metadata or derived before the filter runs.
pub fn filter(journal: Journal, selection: &Selection) -> Journal {
    // Only `transactions` is transformed; every other field passes
    // through unchanged, so `..journal` carries them — including any
    // field added to `Journal` later, with no edit needed here.
    let transactions = filter_transactions(journal.transactions, selection);
    Journal {
        transactions,
        ..journal
    }
}

/// Everything a report command asks the filter phase for. `Default` is
/// "keep it all": no patterns, no date range, matched postings only.
#[derive(Default)]
pub struct Selection<'a> {
    pub patterns: &'a [String],
    pub begin: Option<&'a str>,
    pub end: Option<&'a str>,
    pub related: bool,
    pub whole_transactions: bool,
    pub sign: SignFilter,
    pub display: Option<&'a str>,
    pub amount: Option<&'a AmountFilter>,
}

/// Core transform — kept separate so tests can exercise filter logic
/// without constructing a full `Journal`.
fn filter_transactions(
    transactions: Vec<Located<Transaction>>,
    selection: &Selection,
) -> Vec<Located<Transaction>> {
    let &Selection { patterns, begin, end, related, whole_transactions, sign, display, amount } =
        selection;
    let matcher = (!patterns.is_empty()).then(|| PatternMatcher::from_parts(patterns));
    let display_matcher = display.map(PatternMatcher::new);

    let begin_d = begin.and_then(|s| crate::date::Date::parse(s).ok());
    let end_d = end.and_then(|s| crate::date::Date::parse(s).ok());

    transactions
        .into_iter()
        .filter_map(|mut lt| {
            if let Some(b) = begin_d
                && lt.value.date < b {
                    return None;
                }
            if let Some(e) = end_d
                && lt.value.date >= e {
                    return None;
                }
            // Lowercased tx-wide fields for pattern matching; only needed
            // when a positional pattern is present.
            let (desc_lower, code_lower) = if matcher.is_some() {
                (
                    lt.value.description.to_lowercase(),
                    lt.value.code.as_deref().unwrap_or("").to_lowercase(),
                )
            } else {
                (String::new(), String::new())
            };

            // Transaction selection: every `with`-conjunct must be satisfied by
            // some posting (a lone conjunct is the ordinary "at least one posting
            // matches"; `A with B` needs a posting for each on sibling legs). The
            // pattern may match a tx-wide field shared by every posting. Sign/
            // amount narrow further below, gating survival via the empty check.
            if let Some(m) = &matcher
                && !m.matches_transaction(&lt.value.postings, &desc_lower, &code_lower)
            {
                return None;
            }

            // A posting is "matched" by the query when it satisfies every
            // active posting-level criterion at once: the positional
            // pattern, the sign filter (`--pos` / `--neg`) and the amount
            // filter (`--amount`). `-r` relates to this whole set, so an
            // amount or sign search composes with `related` exactly like a
            // pattern search does.
            let matched = |lp: &Located<Posting>| {
                matcher
                    .as_ref()
                    .is_none_or(|m| m.matches_full(&lp.value, &desc_lower, &code_lower))
                    && sign.keeps(&lp.value)
                    && amount.is_none_or(|af| af.keeps(&lp.value))
            };

            // Project which postings to show:
            if let Some(d) = &display_matcher {
                // `--display` / `-d`: from the full posting set of each
                // selected transaction, keep the account matches, then let
                // the sign/amount secondary projections narrow that set.
                // The positional pattern selects *which transactions*; this
                // picks *which of their postings* — so `--related-all` is
                // not needed to widen first.
                lt.value.postings.retain(|lp| d.matches(&lp.value.account));
                if sign != SignFilter::Any {
                    lt.value.postings.retain(|lp| sign.keeps(&lp.value));
                }
                if let Some(af) = amount {
                    lt.value.postings.retain(|lp| af.keeps(&lp.value));
                }
            } else if whole_transactions {
                // `print` / `--related-all`: keep the entry intact, but the
                // sign/amount secondary projections still narrow it.
                if sign != SignFilter::Any {
                    lt.value.postings.retain(|lp| sign.keeps(&lp.value));
                }
                if let Some(af) = amount {
                    lt.value.postings.retain(|lp| af.keeps(&lp.value));
                }
            } else if related {
                // `-r`: show the siblings of the matched postings. Needs at
                // least one match, else there is nothing to relate to.
                if !lt.value.postings.iter().any(&matched) {
                    return None;
                }
                lt.value.postings.retain(|lp| !matched(lp));
            } else {
                // Default (reg / bal): show the matched postings.
                lt.value.postings.retain(|lp| matched(lp));
            }

            // A transaction whose postings were all pruned away carries
            // nothing to show. (A well-formed entry always starts with at
            // least one posting, so this only fires after pruning.)
            if lt.value.postings.is_empty() {
                return None;
            }
            Some(lt)
        })
        .collect()
}


// ==================== Tests ====================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decimal::Decimal;
    use crate::parser::comment::Comment;
    use crate::parser::posting::Amount;
    use crate::parser::transaction::State;
    use std::sync::Arc;

    fn posting(account: &str, commodity: &str, value: i64) -> Located<Posting> {
        Located {
            file: Arc::from(""),
            line: 0,
            value: Posting {
                account: account.to_string(),
                amount: Some(Amount {
                    commodity: commodity.to_string(),
                    value: Decimal::from(value),
                    decimals: 0,
                }),
                costs: None,
                lot_cost: None,
                lot_date: None,
                balance_assertion: None,
                is_virtual: false,
                balanced: true,
                comments: Vec::<Located<Comment>>::new(),
            },
        }
    }

    fn tx(date: &str, description: &str, postings: Vec<Located<Posting>>) -> Located<Transaction> {
        tx_coded(date, description, None, postings)
    }

    fn tx_coded(
        date: &str,
        description: &str,
        code: Option<&str>,
        postings: Vec<Located<Posting>>,
    ) -> Located<Transaction> {
        Located {
            file: Arc::from(""),
            line: 1,
            value: Transaction {
                date: crate::date::Date::parse(date).unwrap(),
                state: State::Cleared,
                code: code.map(String::from),
                description: description.to_string(),
                postings,
                comments: Vec::new(),
            },
        }
    }

    fn run(patterns: &[&str], txs: Vec<Located<Transaction>>) -> Vec<Located<Transaction>> {
        let pats: Vec<String> = patterns.iter().map(|s| s.to_string()).collect();
        filter_transactions(txs, &Selection { patterns: &pats, ..Default::default() })
    }

    fn run_amount(expr: &str, txs: Vec<Located<Transaction>>) -> Vec<Located<Transaction>> {
        let af = AmountFilter::parse(expr).unwrap();
        filter_transactions(txs, &Selection { amount: Some(&af), ..Default::default() })
    }

    fn run_sign(
        patterns: &[&str],
        whole_transactions: bool,
        sign: SignFilter,
        txs: Vec<Located<Transaction>>,
    ) -> Vec<Located<Transaction>> {
        let pats: Vec<String> = patterns.iter().map(|s| s.to_string()).collect();
        filter_transactions(
            txs,
            &Selection { patterns: &pats, whole_transactions, sign, ..Default::default() },
        )
    }

    fn run_display(
        patterns: &[&str],
        related: bool,
        whole_transactions: bool,
        display: &str,
        txs: Vec<Located<Transaction>>,
    ) -> Vec<Located<Transaction>> {
        let pats: Vec<String> = patterns.iter().map(|s| s.to_string()).collect();
        filter_transactions(
            txs,
            &Selection {
                patterns: &pats,
                related,
                whole_transactions,
                display: Some(display),
                ..Default::default()
            },
        )
    }

    fn account(lt: &Located<Transaction>, idx: usize) -> &str {
        &lt.value.postings[idx].value.account
    }

    fn commodity_of(lt: &Located<Transaction>, idx: usize) -> &str {
        lt.value.postings[idx]
            .value
            .amount
            .as_ref()
            .map(|a| a.commodity.as_str())
            .unwrap_or("")
    }

    #[test]
    fn account_prefix() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![posting("expenses:coffee", "GBP", -5), posting("assets:cash", "GBP", 5)],
        )];
        let out = run(&["^ex"], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value.postings.len(), 1);
        assert_eq!(account(&out[0], 0), "expenses:coffee");
    }

    #[test]
    fn account_suffix() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![posting("expenses:coffee", "GBP", -5), posting("assets:cash", "GBP", 5)],
        )];
        let out = run(&["cash$"], txs);
        assert_eq!(out[0].value.postings.len(), 1);
        assert_eq!(account(&out[0], 0), "assets:cash");
    }

    #[test]
    fn account_exact() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![posting("expenses:coffee", "GBP", -5), posting("expenses:coffee:bar", "GBP", 5)],
        )];
        let out = run(&["^expenses:coffee$"], txs);
        assert_eq!(out[0].value.postings.len(), 1);
        assert_eq!(account(&out[0], 0), "expenses:coffee");
    }

    #[test]
    fn description_at_and_keyword_equivalent() {
        let mk = || {
            vec![
                tx(
                    "2025-01-01",
                    "Amazon order",
                    vec![posting("expenses:x", "GBP", -5), posting("assets:cc", "GBP", 5)],
                ),
                tx(
                    "2025-01-02",
                    "Walmart",
                    vec![posting("expenses:y", "GBP", -5), posting("assets:cc", "GBP", 5)],
                ),
            ]
        };
        let a = run(&["@amazon"], mk());
        let b = run(&["desc", "amazon"], mk());
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].value.description, b[0].value.description);
    }

    #[test]
    fn code_hash_and_keyword_equivalent() {
        let mk = || {
            vec![
                tx_coded(
                    "2025-01-01",
                    "a",
                    Some("INV-42"),
                    vec![posting("expenses:x", "GBP", -5), posting("assets:cc", "GBP", 5)],
                ),
                tx_coded(
                    "2025-01-02",
                    "b",
                    Some("INV-43"),
                    vec![posting("expenses:y", "GBP", -5), posting("assets:cc", "GBP", 5)],
                ),
            ]
        };
        let a = run(&["#INV-42"], mk());
        let b = run(&["code", "INV-42"], mk());
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].value.code, b[0].value.code);
    }

    #[test]
    fn commodity_keyword_matches_exact_symbol() {
        let txs = vec![
            tx(
                "2025-01-01",
                "a",
                vec![posting("expenses:x", "GBP", -5), posting("assets:cc", "GBP", 5)],
            ),
            tx(
                "2025-01-02",
                "b",
                vec![posting("expenses:y", "USD", -5), posting("assets:cc", "USD", 5)],
            ),
        ];
        let out = run(&["com", "GBP"], txs);
        assert_eq!(out.len(), 1);
        assert!(out[0]
            .value
            .postings
            .iter()
            .all(|lp| lp.value.amount.as_ref().map(|a| a.commodity.as_str()) == Some("GBP")));
    }

    #[test]
    fn commodity_keyword_is_case_insensitive() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![posting("expenses:x", "GBP", -5), posting("assets:cc", "GBP", 5)],
        )];
        // Lowercase pattern matches uppercase commodity.
        let out = run(&["com", "gbp"], txs);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn not_negation_on_account() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![posting("expenses:coffee", "GBP", -5), posting("assets:cc", "GBP", 5)],
        )];
        let out = run(&["not", "^ex"], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value.postings.len(), 1);
        assert_eq!(account(&out[0], 0), "assets:cc");
    }

    #[test]
    fn not_on_desc_keyword() {
        let txs = vec![
            tx(
                "2025-01-01",
                "Amazon",
                vec![posting("expenses:x", "GBP", -5), posting("assets:cc", "GBP", 5)],
            ),
            tx(
                "2025-01-02",
                "Walmart",
                vec![posting("expenses:y", "GBP", -5), posting("assets:cc", "GBP", 5)],
            ),
        ];
        let out = run(&["not", "desc", "amazon"], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value.description, "Walmart");
    }

    #[test]
    fn not_on_com_keyword() {
        let txs = vec![
            tx(
                "2025-01-01",
                "a",
                vec![posting("expenses:x", "GBP", -5), posting("assets:cc", "GBP", 5)],
            ),
            tx(
                "2025-01-02",
                "b",
                vec![posting("expenses:y", "USD", -5), posting("assets:cc", "USD", 5)],
            ),
        ];
        let out = run(&["not", "com", "GBP"], txs);
        assert_eq!(out.len(), 1);
        assert!(out[0]
            .value
            .postings
            .iter()
            .all(|lp| lp.value.amount.as_ref().map(|a| a.commodity.as_str()) == Some("USD")));
    }

    #[test]
    fn com_plus_account_and() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![posting("expenses:x", "GBP", -5), posting("assets:cc", "USD", 5)],
        )];
        let out = run(&["^ex", "and", "com", "GBP"], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value.postings.len(), 1);
        assert_eq!(account(&out[0], 0), "expenses:x");
        assert_eq!(commodity_of(&out[0], 0), "GBP");
    }

    #[test]
    fn and_combines_account_and_description() {
        let txs = vec![
            tx(
                "2025-01-01",
                "Amazon",
                vec![posting("expenses:books", "GBP", -5), posting("assets:cc", "GBP", 5)],
            ),
            tx(
                "2025-01-02",
                "Walmart",
                vec![posting("expenses:food", "GBP", -5), posting("assets:cc", "GBP", 5)],
            ),
        ];
        let out = run(&["^ex", "and", "@amazon"], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value.postings.len(), 1);
        assert_eq!(account(&out[0], 0), "expenses:books");
    }

    #[test]
    fn or_default_between_bare_tokens() {
        let txs = vec![
            tx(
                "2025-01-01",
                "a",
                vec![posting("expenses:coffee", "GBP", -5), posting("assets:cc", "GBP", 5)],
            ),
            tx(
                "2025-01-02",
                "b",
                vec![posting("income:salary", "GBP", 5), posting("assets:cc", "GBP", -5)],
            ),
            tx(
                "2025-01-03",
                "c",
                vec![posting("li:loan", "GBP", -5), posting("assets:cc", "GBP", 5)],
            ),
        ];
        let out = run(&["^ex", "^in"], txs);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn mixed_and_or() {
        // (^ex AND @amazon) OR ^in
        let txs = vec![
            tx(
                "2025-01-01",
                "Amazon",
                vec![posting("expenses:books", "GBP", -5), posting("assets:cc", "GBP", 5)],
            ),
            tx(
                "2025-01-02",
                "Walmart",
                vec![posting("expenses:food", "GBP", -5), posting("assets:cc", "GBP", 5)],
            ),
            tx(
                "2025-01-03",
                "any",
                vec![posting("income:salary", "GBP", 5), posting("assets:cc", "GBP", -5)],
            ),
        ];
        let out = run(&["^ex", "and", "@amazon", "or", "^in"], txs);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn desc_with_spaces_via_quoted_token() {
        let txs = vec![
            tx(
                "2025-01-01",
                "foo bar baz",
                vec![posting("expenses:x", "GBP", -5), posting("assets:cc", "GBP", 5)],
            ),
            tx(
                "2025-01-02",
                "foobar",
                vec![posting("expenses:y", "GBP", -5), posting("assets:cc", "GBP", 5)],
            ),
        ];
        let out = run(&["desc", "foo bar"], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value.description, "foo bar baz");
    }

    #[test]
    fn at_prefix_with_spaces_via_quoted_token() {
        let txs = vec![
            tx(
                "2025-01-01",
                "foo bar baz",
                vec![posting("expenses:x", "GBP", -5), posting("assets:cc", "GBP", 5)],
            ),
            tx(
                "2025-01-02",
                "foobar",
                vec![posting("expenses:y", "GBP", -5), posting("assets:cc", "GBP", 5)],
            ),
        ];
        let out = run(&["@foo bar"], txs);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn date_range_preserved() {
        let txs = vec![
            tx(
                "2025-01-01",
                "a",
                vec![posting("expenses:x", "GBP", -5), posting("assets:cc", "GBP", 5)],
            ),
            tx(
                "2025-02-01",
                "b",
                vec![posting("expenses:y", "GBP", -5), posting("assets:cc", "GBP", 5)],
            ),
        ];
        let pats: Vec<String> = Vec::new();
        let out = filter_transactions(
            txs,
            &Selection {
                patterns: &pats,
                begin: Some("2025-01-15"),
                end: Some("2025-02-15"),
                ..Default::default()
            },
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value.date.to_string(), "2025-02-01");
    }

    #[test]
    fn with_selects_transactions_holding_both_accounts() {
        let txs = vec![
            tx(
                "2025-01-01",
                "both",
                vec![posting("foo:aa", "USD", -150), posting("bar:bb", "USD", 150)],
            ),
            tx(
                "2025-02-01",
                "only foo",
                vec![posting("foo:aa", "USD", -50), posting("assets:cash", "USD", 50)],
            ),
            tx(
                "2025-03-01",
                "only bar",
                vec![posting("bar:bb", "USD", 100), posting("income:z", "USD", -100)],
            ),
        ];
        // `foo:aa with bar:bb` keeps only the transaction that has BOTH on
        // sibling postings.
        let out = run(&["foo:aa", "with", "bar:bb"], txs.clone());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value.date.to_string(), "2025-01-01");
        // A bare `foo:aa` keeps both foo transactions — `with` is stricter.
        assert_eq!(run(&["foo:aa"], txs).len(), 2);
    }

    #[test]
    fn matches_account_only_accepts_when_only_non_account_dims_constrain() {
        let m = PatternMatcher::from_parts(&["com".into(), "GBP".into()]);
        assert!(m.matches("any:account"));
    }

    #[test]
    fn matches_account_only_respects_account_constraint() {
        let m = PatternMatcher::from_parts(&["^assets".into()]);
        assert!(m.matches("assets:bank"));
        assert!(!m.matches("expenses:food"));
    }

    #[test]
    fn empty_patterns_accept_everything() {
        let m = PatternMatcher::from_parts(&[]);
        assert!(m.matches("anything"));
    }

    #[test]
    fn trailing_keyword_without_value_is_ignored() {
        let txs = vec![tx(
            "2025-01-01",
            "irrelevant",
            vec![posting("foo:bar", "GBP", -5), posting("assets:cc", "GBP", 5)],
        )];
        let out = run(&["foo", "desc"], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(account(&out[0], 0), "foo:bar");
    }

    #[test]
    fn double_not_cancels() {
        // not not ^ex  ==  ^ex
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![posting("expenses:coffee", "GBP", -5), posting("assets:cc", "GBP", 5)],
        )];
        let out = run(&["not", "not", "^ex"], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(account(&out[0], 0), "expenses:coffee");
    }

    fn accounts(lt: &Located<Transaction>) -> Vec<&str> {
        lt.value.postings.iter().map(|lp| lp.value.account.as_str()).collect()
    }

    #[test]
    fn sign_pos_keeps_nonnegative_and_zero() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![
                posting("assets:in", "GBP", 5),
                posting("assets:out", "GBP", -5),
                posting("assets:zero", "GBP", 0),
            ],
        )];
        let out = run_sign(&[], false, SignFilter::NonNegative, txs);
        assert_eq!(out.len(), 1);
        assert_eq!(accounts(&out[0]), vec!["assets:in", "assets:zero"]);
    }

    #[test]
    fn sign_neg_keeps_nonpositive_and_zero() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![
                posting("assets:in", "GBP", 5),
                posting("assets:out", "GBP", -5),
                posting("assets:zero", "GBP", 0),
            ],
        )];
        let out = run_sign(&[], false, SignFilter::NonPositive, txs);
        assert_eq!(out.len(), 1);
        assert_eq!(accounts(&out[0]), vec!["assets:out", "assets:zero"]);
    }

    #[test]
    fn sign_filter_drops_emptied_transaction() {
        // Every posting is positive; `--neg` leaves nothing, so the
        // transaction is dropped rather than shown empty.
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![posting("assets:a", "GBP", 5), posting("assets:b", "GBP", 5)],
        )];
        let out = run_sign(&[], false, SignFilter::NonPositive, txs);
        assert!(out.is_empty());
    }

    #[test]
    fn sign_filter_from_flags_resolves() {
        assert_eq!(SignFilter::from_flags(true, false), SignFilter::NonNegative);
        assert_eq!(SignFilter::from_flags(false, true), SignFilter::NonPositive);
        assert_eq!(SignFilter::from_flags(false, false), SignFilter::Any);
        // Both at once is the same as no constraint.
        assert_eq!(SignFilter::from_flags(true, true), SignFilter::Any);
    }

    #[test]
    fn sign_filter_composes_with_whole_transactions() {
        // Pattern selects the transaction, `whole_transactions` keeps all
        // its postings, then the sign filter narrows to the non-negative
        // ones — the same secondary-projection mechanism `--related-all`
        // composes with.
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![
                posting("assets:vendor", "GBP", 100),
                posting("expenses:a", "GBP", -60),
                posting("expenses:b", "GBP", -40),
            ],
        )];
        let out = run_sign(&["^assets:vendor"], true, SignFilter::NonNegative, txs);
        assert_eq!(out.len(), 1);
        assert_eq!(accounts(&out[0]), vec!["assets:vendor"]);
    }

    #[test]
    fn display_projects_postings_without_related_all() {
        // Select transactions touching the vendor account, then show only
        // their expense postings — `--related-all` is not needed, the
        // projection runs on the full posting set.
        let txs = vec![tx(
            "2025-01-05",
            "Vendor",
            vec![
                posting("expenses:food", "GBP", 30),
                posting("expenses:fee", "GBP", 5),
                posting("assets:vendor", "GBP", -35),
            ],
        )];
        let out = run_display(&["^assets:vendor"], false, false, "^ex", txs);
        assert_eq!(out.len(), 1);
        assert_eq!(accounts(&out[0]), vec!["expenses:food", "expenses:fee"]);
    }

    #[test]
    fn display_same_result_with_or_without_related_all() {
        let mk = || {
            vec![tx(
                "2025-01-05",
                "Vendor",
                vec![
                    posting("expenses:food", "GBP", 30),
                    posting("assets:vendor", "GBP", -30),
                ],
            )]
        };
        let without = run_display(&["^assets:vendor"], false, false, "^ex", mk());
        let with = run_display(&["^assets:vendor"], false, true, "^ex", mk());
        assert_eq!(accounts(&without[0]), vec!["expenses:food"]);
        assert_eq!(accounts(&without[0]), accounts(&with[0]));
    }

    #[test]
    fn display_without_positional_pattern_keeps_matching_postings() {
        let txs = vec![
            tx(
                "2025-01-05",
                "a",
                vec![
                    posting("expenses:food", "GBP", 30),
                    posting("assets:cash", "GBP", -30),
                ],
            ),
            tx(
                "2025-01-06",
                "b",
                vec![
                    posting("income:job", "GBP", -50),
                    posting("assets:cash", "GBP", 50),
                ],
            ),
        ];
        let out = run_display(&[], false, false, "^ex", txs);
        assert_eq!(out.len(), 1);
        assert_eq!(accounts(&out[0]), vec!["expenses:food"]);
    }

    #[test]
    fn display_drops_transaction_with_no_matching_posting() {
        // Selected by the vendor account, but no expense posting — nothing
        // to project, so the transaction is dropped rather than shown empty.
        let txs = vec![tx(
            "2025-01-05",
            "Vendor",
            vec![
                posting("income:job", "GBP", -50),
                posting("assets:vendor", "GBP", 50),
            ],
        )];
        let out = run_display(&["^assets:vendor"], false, false, "^ex", txs);
        assert!(out.is_empty());
    }

    #[test]
    fn display_ends_with_anchor() {
        let txs = vec![tx(
            "2025-01-05",
            "a",
            vec![
                posting("assets:bank:cash", "GBP", 10),
                posting("assets:bank:savings", "GBP", -10),
            ],
        )];
        let out = run_display(&[], false, false, "cash$", txs);
        assert_eq!(out.len(), 1);
        assert_eq!(accounts(&out[0]), vec!["assets:bank:cash"]);
    }

    #[test]
    fn amount_exact_is_signed() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![
                posting("assets:a", "GBP", 100),
                posting("assets:b", "GBP", -100),
                posting("assets:c", "GBP", 50),
            ],
        )];
        let out = run_amount("100", txs);
        assert_eq!(out.len(), 1);
        assert_eq!(accounts(&out[0]), vec!["assets:a"]); // +100 only, not -100
    }

    #[test]
    fn amount_negative_exact() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![posting("assets:a", "GBP", 100), posting("assets:b", "GBP", -100)],
        )];
        let out = run_amount("-100", txs);
        assert_eq!(accounts(&out[0]), vec!["assets:b"]);
    }

    #[test]
    fn amount_greater_than_is_signed() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![
                posting("assets:a", "GBP", 150),
                posting("assets:b", "GBP", 100),
                posting("assets:c", "GBP", -200),
            ],
        )];
        let out = run_amount(">100", txs);
        assert_eq!(accounts(&out[0]), vec!["assets:a"]); // 150 only; -200 is not > 100
    }

    #[test]
    fn amount_le_includes_boundary() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![
                posting("assets:a", "GBP", 50),
                posting("assets:b", "GBP", 100),
                posting("assets:c", "GBP", 101),
            ],
        )];
        let out = run_amount("<=100", txs);
        assert_eq!(accounts(&out[0]), vec!["assets:a", "assets:b"]);
    }

    #[test]
    fn amount_drops_emptied_transaction() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![posting("assets:a", "GBP", 5), posting("assets:b", "GBP", -5)],
        )];
        // No posting is exactly 999, so the whole transaction is dropped.
        assert!(run_amount("999", txs).is_empty());
    }

    #[test]
    fn amount_not_equal_is_signed() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![
                posting("assets:a", "GBP", 100),
                posting("assets:b", "GBP", -100),
                posting("assets:c", "GBP", 50),
            ],
        )];
        // `<>100` keeps everything except exactly +100 (signed).
        let out = run_amount("<>100", txs);
        assert_eq!(accounts(&out[0]), vec!["assets:b", "assets:c"]);
    }

    #[test]
    fn amount_parse_rejects_garbage() {
        assert!(AmountFilter::parse(">abc").is_err());
        assert!(AmountFilter::parse("").is_err());
    }

    fn run_related_amount(
        expr: &str,
        patterns: &[&str],
        txs: Vec<Located<Transaction>>,
    ) -> Vec<Located<Transaction>> {
        let af = AmountFilter::parse(expr).unwrap();
        let pats: Vec<String> = patterns.iter().map(|s| s.to_string()).collect();
        filter_transactions(
            txs,
            &Selection { patterns: &pats, related: true, amount: Some(&af), ..Default::default() },
        )
    }

    #[test]
    fn related_shows_siblings_of_amount_match() {
        // `-A ">100" -r`: the +150 posting matches the amount filter; `-r`
        // shows its sibling instead of the match itself.
        let txs = vec![tx(
            "2025-02-01",
            "a",
            vec![
                posting("expenses:food", "GBP", 150),
                posting("assets:cash", "GBP", -150),
            ],
        )];
        let out = run_related_amount(">100", &[], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(accounts(&out[0]), vec!["assets:cash"]);
    }

    #[test]
    fn related_amount_drops_transaction_without_match() {
        // No posting exceeds 100, so there is nothing to relate to — the
        // whole transaction is dropped rather than shown in full.
        let txs = vec![tx(
            "2025-02-01",
            "a",
            vec![
                posting("expenses:food", "GBP", 50),
                posting("assets:cash", "GBP", -50),
            ],
        )];
        let out = run_related_amount(">100", &[], txs);
        assert!(out.is_empty());
    }

    #[test]
    fn related_composes_pattern_and_amount() {
        // Pattern AND amount together define the match; `-r` shows the
        // siblings of the posting satisfying both. `expenses:small` matches
        // the pattern but not the amount, so it is a sibling, not a match.
        let txs = vec![tx(
            "2025-02-01",
            "a",
            vec![
                posting("expenses:food", "GBP", 150),
                posting("expenses:small", "GBP", 10),
                posting("assets:cash", "GBP", -160),
            ],
        )];
        let out = run_related_amount(">100", &["^expenses"], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(accounts(&out[0]), vec!["expenses:small", "assets:cash"]);
    }
}
