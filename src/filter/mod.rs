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
//! com <EUR>          posting commodity equals "EUR" (case-SENSITIVE —
//!                    $ and USD are distinct)
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

/// Filter phase. Applies `patterns` and an optional `begin` / `end`
/// date range to the journal. Transactions outside the date range are
/// dropped; surviving transactions keep only postings that match the
/// pattern. Transactions that end up empty are dropped too.
///
/// The non-transaction fields of `Journal` (prices, fx accounts,
/// precisions) pass through unchanged — they are either global
/// metadata or derived before the filter runs.
pub fn filter(
    journal: Journal,
    patterns: &[String],
    begin: Option<&str>,
    end: Option<&str>,
) -> Journal {
    Journal {
        transactions: filter_transactions(journal.transactions, patterns, begin, end),
        prices: journal.prices,
        fx_gain: journal.fx_gain,
        fx_loss: journal.fx_loss,
        precisions: journal.precisions,
    }
}

/// Core transform — kept separate so tests can exercise filter logic
/// without constructing a full `Journal`.
fn filter_transactions(
    transactions: Vec<Located<Transaction>>,
    patterns: &[String],
    begin: Option<&str>,
    end: Option<&str>,
) -> Vec<Located<Transaction>> {
    let matcher = (!patterns.is_empty()).then(|| PatternMatcher::from_parts(patterns));

    let begin_d = begin.and_then(|s| crate::date::Date::parse(s).ok());
    let end_d = end.and_then(|s| crate::date::Date::parse(s).ok());

    transactions
        .into_iter()
        .filter_map(|mut lt| {
            if let Some(b) = begin_d {
                if lt.value.date < b {
                    return None;
                }
            }
            if let Some(e) = end_d {
                if lt.value.date >= e {
                    return None;
                }
            }
            if let Some(m) = &matcher {
                let desc_lower = lt.value.description.to_lowercase();
                let code_lower = lt.value.code.as_deref().unwrap_or("").to_lowercase();
                lt.value
                    .postings
                    .retain(|lp| m.matches_full(&lp.value, &desc_lower, &code_lower));
                if lt.value.postings.is_empty() {
                    return None;
                }
            }
            Some(lt)
        })
        .collect()
}

/// Parsed pattern expression. `None` query means "match anything" —
/// empty input produces this, so callers do not need to special-case
/// an absent pattern.
#[derive(Debug, Clone)]
pub struct PatternMatcher {
    query: Option<Query>,
}

impl PatternMatcher {
    /// Parse a token list as delivered by the CLI.
    pub fn from_parts(parts: &[String]) -> Self {
        if parts.is_empty() {
            return Self { query: None };
        }
        let mut parser = Parser::new(parts);
        Self {
            query: parser.parse_or(),
        }
    }

    /// Convenience: parse a single pattern string.
    pub fn new(pattern: &str) -> Self {
        Self::from_parts(&[pattern.to_string()])
    }

    /// Full posting match — used during the filter pass where all
    /// context (posting + transaction description + code) is
    /// available.
    pub(crate) fn matches_full(
        &self,
        posting: &Posting,
        desc_lower: &str,
        code_lower: &str,
    ) -> bool {
        match &self.query {
            None => true,
            Some(q) => q.eval(posting, desc_lower, code_lower),
        }
    }

    /// Account-only check: non-account dimensions are treated as
    /// unconstrained (always match). Suitable for post-filter use
    /// where only a bare account name is available.
    pub fn matches(&self, account: &str) -> bool {
        match &self.query {
            None => true,
            Some(q) => q.eval_account_only(&account.to_lowercase()),
        }
    }
}

// ==================== Query AST ====================

#[derive(Debug, Clone)]
enum Query {
    Match(Dim, Pattern),
    Not(Box<Query>),
    And(Box<Query>, Box<Query>),
    Or(Box<Query>, Box<Query>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dim {
    Account,
    Description,
    Code,
    Commodity,
}

#[derive(Debug, Clone)]
struct Pattern {
    text: String,
    mode: MatchMode,
}

#[derive(Debug, Clone, Copy)]
enum MatchMode {
    Contains,
    StartsWith,
    EndsWith,
    Exact,
}

impl Pattern {
    fn test(&self, value: &str) -> bool {
        match self.mode {
            MatchMode::Contains => value.contains(&self.text),
            MatchMode::StartsWith => value.starts_with(&self.text),
            MatchMode::EndsWith => value.ends_with(&self.text),
            MatchMode::Exact => value == self.text,
        }
    }
}

impl Query {
    fn eval(&self, p: &Posting, desc_lower: &str, code_lower: &str) -> bool {
        match self {
            Query::Match(dim, pat) => match dim {
                Dim::Account => pat.test(&p.account.to_lowercase()),
                Dim::Description => pat.test(desc_lower),
                Dim::Code => pat.test(code_lower),
                // Commodity is case-sensitive and only exists when the
                // posting carries an amount.
                Dim::Commodity => p.amount.as_ref().is_some_and(|a| pat.test(&a.commodity)),
            },
            Query::Not(q) => !q.eval(p, desc_lower, code_lower),
            Query::And(a, b) => {
                a.eval(p, desc_lower, code_lower) && b.eval(p, desc_lower, code_lower)
            }
            Query::Or(a, b) => {
                a.eval(p, desc_lower, code_lower) || b.eval(p, desc_lower, code_lower)
            }
        }
    }

    /// Evaluate with only an account available. Matches on other
    /// dimensions return `true` (no constraint from this side).
    fn eval_account_only(&self, account_lower: &str) -> bool {
        match self {
            Query::Match(Dim::Account, pat) => pat.test(account_lower),
            Query::Match(_, _) => true,
            Query::Not(q) => !q.eval_account_only(account_lower),
            Query::And(a, b) => {
                a.eval_account_only(account_lower) && b.eval_account_only(account_lower)
            }
            Query::Or(a, b) => {
                a.eval_account_only(account_lower) || b.eval_account_only(account_lower)
            }
        }
    }
}

// ==================== Parser ====================

/// Recursive-descent parser over the pre-tokenised CLI arg list.
///
/// Grammar (EBNF-ish):
///
/// ```text
/// or_expr  := and_expr ( ("or" | ε) and_expr )*
/// and_expr := not_expr ( "and" not_expr )*
/// not_expr := "not" not_expr | primary
/// primary  := "desc" VALUE
///           | "code" VALUE
///           | "com"  VALUE
///           | "@" REST                  # description
///           | "#" REST                  # code
///           | "^" REST "$"              # account exact
///           | "^" REST                  # account starts-with
///           | REST "$"                  # account ends-with
///           | REST                      # account contains
/// ```
///
/// `ε` in `or_expr` is the "implicit OR" case: when the next token is
/// not a bare combinator (`and` / `or`) and no explicit combinator
/// precedes it, the parser still treats it as the right operand of an
/// OR. This preserves the convention where bare tokens without a
/// combinator are OR'd.
struct Parser<'a> {
    tokens: &'a [String],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(tokens: &'a [String]) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.pos).map(String::as_str)
    }

    fn advance(&mut self) -> Option<&'a str> {
        let t = self.tokens.get(self.pos)?.as_str();
        self.pos += 1;
        Some(t)
    }

    fn peek_kw(&self, kw: &str) -> bool {
        self.peek().is_some_and(|t| t.eq_ignore_ascii_case(kw))
    }

    /// True if the next token *could* start a primary (i.e. it is not
    /// a bare combinator keyword). Used for the implicit-OR case.
    fn at_primary_start(&self) -> bool {
        match self.peek() {
            None => false,
            Some(t) => !t.eq_ignore_ascii_case("and") && !t.eq_ignore_ascii_case("or"),
        }
    }

    fn parse_or(&mut self) -> Option<Query> {
        let mut left = self.parse_and()?;
        loop {
            if self.peek_kw("or") {
                self.advance();
            } else if !self.at_primary_start() {
                break;
            }
            let Some(right) = self.parse_and() else { break };
            left = Query::Or(Box::new(left), Box::new(right));
        }
        Some(left)
    }

    fn parse_and(&mut self) -> Option<Query> {
        let mut left = self.parse_not()?;
        while self.peek_kw("and") {
            self.advance();
            let Some(right) = self.parse_not() else { break };
            left = Query::And(Box::new(left), Box::new(right));
        }
        Some(left)
    }

    fn parse_not(&mut self) -> Option<Query> {
        if self.peek_kw("not") {
            self.advance();
            let inner = self.parse_not()?;
            Some(Query::Not(Box::new(inner)))
        } else {
            self.parse_primary()
        }
    }

    fn parse_primary(&mut self) -> Option<Query> {
        let tok = self.advance()?;
        let lower = tok.to_ascii_lowercase();
        match lower.as_str() {
            "desc" => {
                let val = self.advance()?;
                Some(Query::Match(
                    Dim::Description,
                    Pattern {
                        text: val.to_lowercase(),
                        mode: MatchMode::Contains,
                    },
                ))
            }
            "code" => {
                let val = self.advance()?;
                Some(Query::Match(
                    Dim::Code,
                    Pattern {
                        text: val.to_lowercase(),
                        mode: MatchMode::Exact,
                    },
                ))
            }
            "com" => {
                let val = self.advance()?;
                Some(Query::Match(
                    Dim::Commodity,
                    Pattern {
                        text: val.to_string(),
                        mode: MatchMode::Exact,
                    },
                ))
            }
            _ => Some(parse_bare_token(tok)),
        }
    }
}

/// Parse a bare (non-keyword) token into a `Query::Match` node.
fn parse_bare_token(tok: &str) -> Query {
    if let Some(rest) = tok.strip_prefix('@') {
        return Query::Match(
            Dim::Description,
            Pattern {
                text: rest.to_lowercase(),
                mode: MatchMode::Contains,
            },
        );
    }
    if let Some(rest) = tok.strip_prefix('#') {
        return Query::Match(
            Dim::Code,
            Pattern {
                text: rest.to_lowercase(),
                mode: MatchMode::Exact,
            },
        );
    }
    // Account patterns — `^` prefix, `$` suffix, both, or neither.
    let (text, mode) = if let Some(inner) = tok.strip_prefix('^').and_then(|r| r.strip_suffix('$')) {
        (inner.to_lowercase(), MatchMode::Exact)
    } else if let Some(rest) = tok.strip_prefix('^') {
        (rest.to_lowercase(), MatchMode::StartsWith)
    } else if let Some(rest) = tok.strip_suffix('$') {
        (rest.to_lowercase(), MatchMode::EndsWith)
    } else {
        (tok.to_lowercase(), MatchMode::Contains)
    };
    Query::Match(Dim::Account, Pattern { text, mode })
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
        filter_transactions(txs, &pats, None, None)
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
            vec![posting("ex:coffee", "EUR", -5), posting("as:cash", "EUR", 5)],
        )];
        let out = run(&["^ex"], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value.postings.len(), 1);
        assert_eq!(account(&out[0], 0), "ex:coffee");
    }

    #[test]
    fn account_suffix() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![posting("ex:coffee", "EUR", -5), posting("as:cash", "EUR", 5)],
        )];
        let out = run(&["cash$"], txs);
        assert_eq!(out[0].value.postings.len(), 1);
        assert_eq!(account(&out[0], 0), "as:cash");
    }

    #[test]
    fn account_exact() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![posting("ex:coffee", "EUR", -5), posting("ex:coffee:bar", "EUR", 5)],
        )];
        let out = run(&["^ex:coffee$"], txs);
        assert_eq!(out[0].value.postings.len(), 1);
        assert_eq!(account(&out[0], 0), "ex:coffee");
    }

    #[test]
    fn description_at_and_keyword_equivalent() {
        let mk = || {
            vec![
                tx(
                    "2025-01-01",
                    "Amazon order",
                    vec![posting("ex:x", "EUR", -5), posting("as:cc", "EUR", 5)],
                ),
                tx(
                    "2025-01-02",
                    "Walmart",
                    vec![posting("ex:y", "EUR", -5), posting("as:cc", "EUR", 5)],
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
                    vec![posting("ex:x", "EUR", -5), posting("as:cc", "EUR", 5)],
                ),
                tx_coded(
                    "2025-01-02",
                    "b",
                    Some("INV-43"),
                    vec![posting("ex:y", "EUR", -5), posting("as:cc", "EUR", 5)],
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
    fn commodity_keyword_case_sensitive() {
        let txs = vec![
            tx(
                "2025-01-01",
                "a",
                vec![posting("ex:x", "EUR", -5), posting("as:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-02",
                "b",
                vec![posting("ex:y", "USD", -5), posting("as:cc", "USD", 5)],
            ),
        ];
        let out = run(&["com", "EUR"], txs);
        assert_eq!(out.len(), 1);
        assert!(out[0]
            .value
            .postings
            .iter()
            .all(|lp| lp.value.amount.as_ref().map(|a| a.commodity.as_str()) == Some("EUR")));
    }

    #[test]
    fn commodity_keyword_wrong_case_does_not_match() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![posting("ex:x", "EUR", -5), posting("as:cc", "EUR", 5)],
        )];
        let out = run(&["com", "eur"], txs);
        assert!(out.is_empty());
    }

    #[test]
    fn not_negation_on_account() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![posting("ex:coffee", "EUR", -5), posting("as:cc", "EUR", 5)],
        )];
        let out = run(&["not", "^ex"], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value.postings.len(), 1);
        assert_eq!(account(&out[0], 0), "as:cc");
    }

    #[test]
    fn not_on_desc_keyword() {
        let txs = vec![
            tx(
                "2025-01-01",
                "Amazon",
                vec![posting("ex:x", "EUR", -5), posting("as:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-02",
                "Walmart",
                vec![posting("ex:y", "EUR", -5), posting("as:cc", "EUR", 5)],
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
                vec![posting("ex:x", "EUR", -5), posting("as:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-02",
                "b",
                vec![posting("ex:y", "USD", -5), posting("as:cc", "USD", 5)],
            ),
        ];
        let out = run(&["not", "com", "EUR"], txs);
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
            vec![posting("ex:x", "EUR", -5), posting("as:cc", "USD", 5)],
        )];
        let out = run(&["^ex", "and", "com", "EUR"], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value.postings.len(), 1);
        assert_eq!(account(&out[0], 0), "ex:x");
        assert_eq!(commodity_of(&out[0], 0), "EUR");
    }

    #[test]
    fn and_combines_account_and_description() {
        let txs = vec![
            tx(
                "2025-01-01",
                "Amazon",
                vec![posting("ex:books", "EUR", -5), posting("as:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-02",
                "Walmart",
                vec![posting("ex:food", "EUR", -5), posting("as:cc", "EUR", 5)],
            ),
        ];
        let out = run(&["^ex", "and", "@amazon"], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value.postings.len(), 1);
        assert_eq!(account(&out[0], 0), "ex:books");
    }

    #[test]
    fn or_default_between_bare_tokens() {
        let txs = vec![
            tx(
                "2025-01-01",
                "a",
                vec![posting("ex:coffee", "EUR", -5), posting("as:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-02",
                "b",
                vec![posting("in:salary", "EUR", 5), posting("as:cc", "EUR", -5)],
            ),
            tx(
                "2025-01-03",
                "c",
                vec![posting("li:loan", "EUR", -5), posting("as:cc", "EUR", 5)],
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
                vec![posting("ex:books", "EUR", -5), posting("as:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-02",
                "Walmart",
                vec![posting("ex:food", "EUR", -5), posting("as:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-03",
                "any",
                vec![posting("in:salary", "EUR", 5), posting("as:cc", "EUR", -5)],
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
                vec![posting("ex:x", "EUR", -5), posting("as:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-02",
                "foobar",
                vec![posting("ex:y", "EUR", -5), posting("as:cc", "EUR", 5)],
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
                vec![posting("ex:x", "EUR", -5), posting("as:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-02",
                "foobar",
                vec![posting("ex:y", "EUR", -5), posting("as:cc", "EUR", 5)],
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
                vec![posting("ex:x", "EUR", -5), posting("as:cc", "EUR", 5)],
            ),
            tx(
                "2025-02-01",
                "b",
                vec![posting("ex:y", "EUR", -5), posting("as:cc", "EUR", 5)],
            ),
        ];
        let pats: Vec<String> = Vec::new();
        let out = filter_transactions(txs, &pats, Some("2025-01-15"), Some("2025-02-15"));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value.date.to_string(), "2025-02-01");
    }

    #[test]
    fn matches_account_only_accepts_when_only_non_account_dims_constrain() {
        let m = PatternMatcher::from_parts(&["com".into(), "EUR".into()]);
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
            vec![posting("foo:bar", "EUR", -5), posting("as:cc", "EUR", 5)],
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
            vec![posting("ex:coffee", "EUR", -5), posting("as:cc", "EUR", 5)],
        )];
        let out = run(&["not", "not", "^ex"], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(account(&out[0], 0), "ex:coffee");
    }
}
