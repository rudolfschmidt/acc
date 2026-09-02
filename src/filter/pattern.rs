//! The pattern language: parsing (recursive-descent [`Parser`]) and
//! evaluation ([`Query`] AST, one tree walk per posting). Syntax and
//! precedence are documented on [`crate::filter`].

use crate::parser::located::Located;
use crate::parser::posting::Posting;

/// Parsed pattern expression as a list of `with`-separated conjuncts. An empty
/// list means "match anything". Each conjunct is a per-posting [`Query`]; a
/// transaction is selected when *every* conjunct matches *some* posting — so
/// `A with B` needs one posting for `A` and a sibling posting for `B` in the
/// *same* transaction. A single conjunct (no `with`) is the ordinary
/// "any posting matches" case.
#[derive(Debug, Clone)]
pub struct PatternMatcher {
    conjuncts: Vec<Query>,
}

impl PatternMatcher {
    /// Parse a token list as delivered by the CLI.
    pub fn from_parts(parts: &[String]) -> Self {
        Self { conjuncts: Parser::new(parts).parse_with() }
    }

    /// Convenience: parse a single pattern string.
    pub fn new(pattern: &str) -> Self {
        Self::from_parts(&[pattern.to_string()])
    }

    /// Whether a whole transaction is selected: every `with`-conjunct must be
    /// satisfied by at least one of the transaction's postings.
    pub(crate) fn matches_transaction(
        &self,
        postings: &[Located<Posting>],
        desc_lower: &str,
        code_lower: &str,
    ) -> bool {
        self.conjuncts
            .iter()
            .all(|q| postings.iter().any(|lp| q.eval(&lp.value, desc_lower, code_lower)))
    }

    /// Whether this one posting satisfies any conjunct — used to project which
    /// postings count as "matched" (for `-r` / display) once a transaction has
    /// been selected. No conjuncts (empty pattern) matches everything.
    pub(crate) fn matches_full(
        &self,
        posting: &Posting,
        desc_lower: &str,
        code_lower: &str,
    ) -> bool {
        self.conjuncts.is_empty()
            || self.conjuncts.iter().any(|q| q.eval(posting, desc_lower, code_lower))
    }

    /// Account-only check: non-account dimensions are treated as
    /// unconstrained (always match). Suitable for post-filter use
    /// where only a bare account name is available.
    pub fn matches(&self, account: &str) -> bool {
        let account_lower = account.to_lowercase();
        self.conjuncts.is_empty()
            || self.conjuncts.iter().any(|q| q.eval_account_only(&account_lower))
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
/// with_expr := or_expr ( "with" or_expr )*     # transaction-wide: each part on a sibling posting
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
            Some(t) => {
                !t.eq_ignore_ascii_case("and")
                    && !t.eq_ignore_ascii_case("or")
                    && !t.eq_ignore_ascii_case("with")
            }
        }
    }

    /// Top level: `or_expr ( "with" or_expr )*`. Each `with`-separated part is a
    /// per-posting query that must be satisfied by *some* posting of the
    /// transaction — so `with` pairs a posting with a sibling, looser than the
    /// per-posting `and` / `or`.
    fn parse_with(&mut self) -> Vec<Query> {
        let mut conjuncts = Vec::new();
        if let Some(q) = self.parse_or() {
            conjuncts.push(q);
        }
        while self.peek_kw("with") {
            self.advance();
            if let Some(q) = self.parse_or() {
                conjuncts.push(q);
            }
        }
        conjuncts
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
