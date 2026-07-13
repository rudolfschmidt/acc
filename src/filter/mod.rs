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
//! com <EUR>          posting commodity equals "EUR" (case-insensitive)
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

/// Posting-level sign filter, driven by `--pos` / `--neg`. Applied as a
/// secondary projection *after* transaction selection: it narrows which
/// postings are shown, by the sign of their amount. Zero counts as both
/// non-negative and non-positive, so a zero posting survives either flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignFilter {
    /// No sign constraint.
    Any,
    /// `--pos`: keep postings whose amount is `>= 0`.
    NonNegative,
    /// `--neg`: keep postings whose amount is `<= 0`.
    NonPositive,
}

impl SignFilter {
    /// Resolve the two CLI flags. Both set (or neither) means no
    /// constraint — `>= 0` OR `<= 0` already covers every amount, so
    /// asking for both is the same as asking for nothing.
    pub fn from_flags(pos: bool, neg: bool) -> Self {
        match (pos, neg) {
            (true, false) => SignFilter::NonNegative,
            (false, true) => SignFilter::NonPositive,
            _ => SignFilter::Any,
        }
    }

    /// Whether a posting passes the sign constraint. A posting without an
    /// amount has no sign, so it is dropped when a constraint is active.
    ///
    /// `>= 0` is `!is_negative()` (already true for zero, since
    /// `is_negative` is strictly `< 0`); `<= 0` needs the explicit
    /// `is_zero()` because `is_negative()` excludes zero. The two
    /// overlap on zero, which is exactly why it shows under both flags.
    fn keeps(&self, p: &Posting) -> bool {
        match self {
            SignFilter::Any => true,
            SignFilter::NonNegative => p.amount.as_ref().is_some_and(|a| !a.value.is_negative()),
            SignFilter::NonPositive => {
                p.amount.as_ref().is_some_and(|a| a.value.is_negative() || a.value.is_zero())
            }
        }
    }
}

/// Comparison operator for the amount filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CmpOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
}

/// Posting-level amount filter, driven by `--amount EXPR`. Like the sign
/// filter, it is a secondary projection applied *after* transaction
/// selection: it narrows which postings survive by comparing each
/// posting's (signed) amount against a threshold. A posting without an
/// amount has nothing to compare, so it is dropped.
///
/// `EXPR` is `[op]number` with `op` one of `>`, `<`, `>=`, `<=`, `=` and
/// `<>` ("not equal"); a bare number is `=`. The comparison is signed and
/// value-based, so `-A 100` keeps `100` but not `-100`, `-A -50` keeps
/// exactly `-50`, `-A '>100'` keeps amounts strictly above `100`, and
/// `-A '<>100'` keeps everything except `100`. Under `-X` the postings are
/// already valued in the target commodity, so the threshold is read in
/// that commodity too.
#[derive(Debug, Clone)]
pub struct AmountFilter {
    op: CmpOp,
    threshold: crate::decimal::Decimal,
}

impl AmountFilter {
    /// Parse an `EXPR` (`[op]number`). The two-char `>=` / `<=` / `<>` must
    /// be tested before the one-char `>` / `<`. The number is a signed
    /// decimal parsed by `Decimal::parse`, so `-A -50` and `-A '<=1.5'`
    /// both work. `Decimal` stores every value at one fixed scale, so the
    /// threshold compares equal to `100`, `100.00`, … alike.
    pub fn parse(expr: &str) -> Result<Self, String> {
        let s = expr.trim();
        let (op, rest) = if let Some(r) = s.strip_prefix(">=") {
            (CmpOp::Ge, r)
        } else if let Some(r) = s.strip_prefix("<=") {
            (CmpOp::Le, r)
        } else if let Some(r) = s.strip_prefix("<>") {
            (CmpOp::Ne, r)
        } else if let Some(r) = s.strip_prefix('>') {
            (CmpOp::Gt, r)
        } else if let Some(r) = s.strip_prefix('<') {
            (CmpOp::Lt, r)
        } else if let Some(r) = s.strip_prefix('=') {
            (CmpOp::Eq, r)
        } else {
            (CmpOp::Eq, s)
        };
        let threshold = crate::decimal::Decimal::parse(rest.trim())
            .map_err(|e| format!("invalid --amount `{expr}`: {e}"))?;
        Ok(Self { op, threshold })
    }

    /// Whether a posting passes the amount constraint. A posting without
    /// an amount is dropped — there is nothing to compare.
    fn keeps(&self, p: &Posting) -> bool {
        let Some(a) = p.amount.as_ref() else {
            return false;
        };
        match self.op {
            CmpOp::Eq => a.value == self.threshold,
            CmpOp::Ne => a.value != self.threshold,
            CmpOp::Gt => a.value > self.threshold,
            CmpOp::Lt => a.value < self.threshold,
            CmpOp::Ge => a.value >= self.threshold,
            CmpOp::Le => a.value <= self.threshold,
        }
    }
}

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
pub fn filter(
    journal: Journal,
    patterns: &[String],
    begin: Option<&str>,
    end: Option<&str>,
    related: bool,
    whole_transactions: bool,
    sign: SignFilter,
    display: Option<&str>,
    amount: Option<&AmountFilter>,
) -> Journal {
    // Only `transactions` is transformed; every other field passes
    // through unchanged, so `..journal` carries them — including any
    // field added to `Journal` later, with no edit needed here.
    let transactions = filter_transactions(
        journal.transactions,
        patterns,
        begin,
        end,
        related,
        whole_transactions,
        sign,
        display,
        amount,
    );
    Journal {
        transactions,
        ..journal
    }
}

/// Core transform — kept separate so tests can exercise filter logic
/// without constructing a full `Journal`.
fn filter_transactions(
    transactions: Vec<Located<Transaction>>,
    patterns: &[String],
    begin: Option<&str>,
    end: Option<&str>,
    related: bool,
    whole_transactions: bool,
    sign: SignFilter,
    display: Option<&str>,
    amount: Option<&AmountFilter>,
) -> Vec<Located<Transaction>> {
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

            // Transaction selection: a positional pattern keeps the entry
            // only if at least one posting matches it (the pattern may match
            // a tx-wide field shared by every posting). Sign/amount narrow
            // further below, gating survival via the final empty check.
            if let Some(m) = &matcher {
                let any = lt
                    .value
                    .postings
                    .iter()
                    .any(|lp| m.matches_full(&lp.value, &desc_lower, &code_lower));
                if !any {
                    return None;
                }
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
                if !lt.value.postings.iter().any(|lp| matched(lp)) {
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
                // Commodity is case-insensitive; only exists when the
                // posting carries an amount.
                Dim::Commodity => p.amount.as_ref().is_some_and(|a| pat.test(&a.commodity.to_lowercase())),
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
                        text: val.to_lowercase(),
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
        filter_transactions(txs, &pats, None, None, false, false, SignFilter::Any, None, None)
    }

    fn run_amount(expr: &str, txs: Vec<Located<Transaction>>) -> Vec<Located<Transaction>> {
        let af = AmountFilter::parse(expr).unwrap();
        filter_transactions(txs, &[], None, None, false, false, SignFilter::Any, None, Some(&af))
    }

    fn run_sign(
        patterns: &[&str],
        whole_transactions: bool,
        sign: SignFilter,
        txs: Vec<Located<Transaction>>,
    ) -> Vec<Located<Transaction>> {
        let pats: Vec<String> = patterns.iter().map(|s| s.to_string()).collect();
        filter_transactions(txs, &pats, None, None, false, whole_transactions, sign, None, None)
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
            &pats,
            None,
            None,
            related,
            whole_transactions,
            SignFilter::Any,
            Some(display),
            None,
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
            vec![posting("expenses:coffee", "EUR", -5), posting("assets:cash", "EUR", 5)],
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
            vec![posting("expenses:coffee", "EUR", -5), posting("assets:cash", "EUR", 5)],
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
            vec![posting("expenses:coffee", "EUR", -5), posting("expenses:coffee:bar", "EUR", 5)],
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
                    vec![posting("expenses:x", "EUR", -5), posting("assets:cc", "EUR", 5)],
                ),
                tx(
                    "2025-01-02",
                    "Walmart",
                    vec![posting("expenses:y", "EUR", -5), posting("assets:cc", "EUR", 5)],
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
                    vec![posting("expenses:x", "EUR", -5), posting("assets:cc", "EUR", 5)],
                ),
                tx_coded(
                    "2025-01-02",
                    "b",
                    Some("INV-43"),
                    vec![posting("expenses:y", "EUR", -5), posting("assets:cc", "EUR", 5)],
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
                vec![posting("expenses:x", "EUR", -5), posting("assets:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-02",
                "b",
                vec![posting("expenses:y", "USD", -5), posting("assets:cc", "USD", 5)],
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
    fn commodity_keyword_is_case_insensitive() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![posting("expenses:x", "EUR", -5), posting("assets:cc", "EUR", 5)],
        )];
        // Lowercase pattern matches uppercase commodity.
        let out = run(&["com", "eur"], txs);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn not_negation_on_account() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![posting("expenses:coffee", "EUR", -5), posting("assets:cc", "EUR", 5)],
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
                vec![posting("expenses:x", "EUR", -5), posting("assets:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-02",
                "Walmart",
                vec![posting("expenses:y", "EUR", -5), posting("assets:cc", "EUR", 5)],
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
                vec![posting("expenses:x", "EUR", -5), posting("assets:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-02",
                "b",
                vec![posting("expenses:y", "USD", -5), posting("assets:cc", "USD", 5)],
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
            vec![posting("expenses:x", "EUR", -5), posting("assets:cc", "USD", 5)],
        )];
        let out = run(&["^ex", "and", "com", "EUR"], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value.postings.len(), 1);
        assert_eq!(account(&out[0], 0), "expenses:x");
        assert_eq!(commodity_of(&out[0], 0), "EUR");
    }

    #[test]
    fn and_combines_account_and_description() {
        let txs = vec![
            tx(
                "2025-01-01",
                "Amazon",
                vec![posting("expenses:books", "EUR", -5), posting("assets:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-02",
                "Walmart",
                vec![posting("expenses:food", "EUR", -5), posting("assets:cc", "EUR", 5)],
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
                vec![posting("expenses:coffee", "EUR", -5), posting("assets:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-02",
                "b",
                vec![posting("income:salary", "EUR", 5), posting("assets:cc", "EUR", -5)],
            ),
            tx(
                "2025-01-03",
                "c",
                vec![posting("li:loan", "EUR", -5), posting("assets:cc", "EUR", 5)],
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
                vec![posting("expenses:books", "EUR", -5), posting("assets:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-02",
                "Walmart",
                vec![posting("expenses:food", "EUR", -5), posting("assets:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-03",
                "any",
                vec![posting("income:salary", "EUR", 5), posting("assets:cc", "EUR", -5)],
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
                vec![posting("expenses:x", "EUR", -5), posting("assets:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-02",
                "foobar",
                vec![posting("expenses:y", "EUR", -5), posting("assets:cc", "EUR", 5)],
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
                vec![posting("expenses:x", "EUR", -5), posting("assets:cc", "EUR", 5)],
            ),
            tx(
                "2025-01-02",
                "foobar",
                vec![posting("expenses:y", "EUR", -5), posting("assets:cc", "EUR", 5)],
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
                vec![posting("expenses:x", "EUR", -5), posting("assets:cc", "EUR", 5)],
            ),
            tx(
                "2025-02-01",
                "b",
                vec![posting("expenses:y", "EUR", -5), posting("assets:cc", "EUR", 5)],
            ),
        ];
        let pats: Vec<String> = Vec::new();
        let out = filter_transactions(
            txs,
            &pats,
            Some("2025-01-15"),
            Some("2025-02-15"),
            false,
            false,
            SignFilter::Any,
            None,
            None,
        );
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
            vec![posting("foo:bar", "EUR", -5), posting("assets:cc", "EUR", 5)],
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
            vec![posting("expenses:coffee", "EUR", -5), posting("assets:cc", "EUR", 5)],
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
                posting("assets:in", "EUR", 5),
                posting("assets:out", "EUR", -5),
                posting("assets:zero", "EUR", 0),
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
                posting("assets:in", "EUR", 5),
                posting("assets:out", "EUR", -5),
                posting("assets:zero", "EUR", 0),
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
            vec![posting("assets:a", "EUR", 5), posting("assets:b", "EUR", 5)],
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
                posting("assets:vendor", "EUR", 100),
                posting("expenses:a", "EUR", -60),
                posting("expenses:b", "EUR", -40),
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
                posting("expenses:food", "EUR", 30),
                posting("expenses:fee", "EUR", 5),
                posting("assets:vendor", "EUR", -35),
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
                    posting("expenses:food", "EUR", 30),
                    posting("assets:vendor", "EUR", -30),
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
                    posting("expenses:food", "EUR", 30),
                    posting("assets:cash", "EUR", -30),
                ],
            ),
            tx(
                "2025-01-06",
                "b",
                vec![
                    posting("income:job", "EUR", -50),
                    posting("assets:cash", "EUR", 50),
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
                posting("income:job", "EUR", -50),
                posting("assets:vendor", "EUR", 50),
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
                posting("assets:bank:cash", "EUR", 10),
                posting("assets:bank:savings", "EUR", -10),
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
                posting("assets:a", "EUR", 100),
                posting("assets:b", "EUR", -100),
                posting("assets:c", "EUR", 50),
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
            vec![posting("assets:a", "EUR", 100), posting("assets:b", "EUR", -100)],
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
                posting("assets:a", "EUR", 150),
                posting("assets:b", "EUR", 100),
                posting("assets:c", "EUR", -200),
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
                posting("assets:a", "EUR", 50),
                posting("assets:b", "EUR", 100),
                posting("assets:c", "EUR", 101),
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
            vec![posting("assets:a", "EUR", 5), posting("assets:b", "EUR", -5)],
        )];
        // No posting is exactly 999, so the whole transaction is dropped.
        assert!(run_amount("999", txs).is_empty());
    }

    #[test]
    fn amount_parse_ops_and_precedence() {
        // Two-char ops must win over the one-char prefixes.
        assert_eq!(AmountFilter::parse(">=5").unwrap().op, CmpOp::Ge);
        assert_eq!(AmountFilter::parse("<=5").unwrap().op, CmpOp::Le);
        assert_eq!(AmountFilter::parse("<>5").unwrap().op, CmpOp::Ne);
        assert_eq!(AmountFilter::parse(">5").unwrap().op, CmpOp::Gt);
        assert_eq!(AmountFilter::parse("<5").unwrap().op, CmpOp::Lt);
        assert_eq!(AmountFilter::parse("=5").unwrap().op, CmpOp::Eq);
        assert_eq!(AmountFilter::parse("5").unwrap().op, CmpOp::Eq);
    }

    #[test]
    fn amount_not_equal_is_signed() {
        let txs = vec![tx(
            "2025-01-01",
            "a",
            vec![
                posting("assets:a", "EUR", 100),
                posting("assets:b", "EUR", -100),
                posting("assets:c", "EUR", 50),
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
            &pats,
            None,
            None,
            true,
            false,
            SignFilter::Any,
            None,
            Some(&af),
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
                posting("expenses:food", "EUR", 150),
                posting("assets:cash", "EUR", -150),
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
                posting("expenses:food", "EUR", 50),
                posting("assets:cash", "EUR", -50),
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
                posting("expenses:food", "EUR", 150),
                posting("expenses:small", "EUR", 10),
                posting("assets:cash", "EUR", -160),
            ],
        )];
        let out = run_related_amount(">100", &["^expenses"], txs);
        assert_eq!(out.len(), 1);
        assert_eq!(accounts(&out[0]), vec!["expenses:small", "assets:cash"]);
    }
}
