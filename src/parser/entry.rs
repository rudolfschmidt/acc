//! Top-level record variants emitted by the parser.

use std::sync::Arc;

use crate::date::Date;
use crate::decimal::Decimal;

use super::located::Located;
use super::posting::Posting;
use super::transaction::Transaction;

/// One raw record from a journal file.
///
/// Block directives (`commodity`, `account`) carry their indented
/// sub-directives inline: a `commodity` block folds its `alias` children
/// into `Commodity.aliases`. An `account` block with an indented role
/// sub-directive (`slippage gain`, `capital loss`, …) is upgraded to a
/// `RoleAccount` carrying that role. The parser accumulates these by
/// mutating the last emitted entry when a new indented line arrives,
/// which lets it remain state-less between lines.
///
/// Alias resolution and price-DB construction happen in the resolve
/// phase after parsing.
#[derive(Debug, Clone)]
pub enum Entry {
    Transaction(Transaction),
    Price(Price),

    /// `commodity SYMBOL` + any number of indented children:
    /// - `alias OTHER` → adds OTHER to `aliases`
    /// - `parity OTHER` → adds OTHER to `parities`: SYMBOL keeps its own
    ///   display (no alias fold) but converts 1:1 to OTHER — a *fixed*
    ///   rate, unlike a dated `P` price. Resolve emits it as a synthetic
    ///   1:1 price so the normal valuation path can chain it.
    /// - `precision N` → sets the display precision override, overriding
    ///   the precision inferred from posting amounts in reports.
    Commodity {
        symbol: String,
        aliases: Vec<String>,
        parities: Vec<String>,
        precision: Option<usize>,
    },

    /// `account NAME` without (or before) a sub-directive. Acts as a
    /// scaffold the parser upgrades to a `RoleAccount` when a role
    /// sub-directive arrives. If no sub-directive follows, the
    /// entry stays and resolve drops it.
    Account(String),

    /// Produced when `account NAME` is followed by an indented role
    /// sub-directive such as `slippage gain`, `cta loss`, or `capital gain`.
    /// `role` is the directive text verbatim (whitespace-collapsed),
    /// `account` the declared name. The role string is the single source
    /// of truth: the resolver indexes these by role, the pipeline phases
    /// look up the ones they consume, and a `$role:slot` posting
    /// reference resolves against the same index — so a new role costs
    /// no parser/resolver change, only a declaration.
    RoleAccount { role: String, account: String },

    /// A top-level comment line (`;` or `#` at column 0).
    Comment(String),

    /// Automated-transaction rule: a pattern that matches against
    /// posting accounts, plus the extra postings to inject (scaled by
    /// the matching posting's amount) into every matching transaction.
    /// Line-leading `=` at column 0, followed by `/pattern/`; indented
    /// children provide the postings with their multipliers.
    AutoRule(AutoRule),

    /// `= NAME[key] :: value` — one entry of a named string→string lookup
    /// table, declared on the auto-transaction level (leading `=`). Referenced
    /// as `NAME[key]` inside an auto-template posting account to expand a key
    /// to its value. A deliberately restricted lookup — pure map access, no
    /// expressions — so resolving it is a map lookup, not an evaluator. Each
    /// line is one entry; entries sharing a table name are merged in resolve.
    /// The bracket in the name is what tells it apart from an `AutoTemplate`.
    Lookup {
        table: String,
        key: String,
        value: String,
    },

    /// `= NAME :: /pattern/` — a named auto-rule *template*. Its pattern and
    /// posting accounts carry positional `$1` / `$2` placeholders (and
    /// `NAME[key]` lookup calls); an `AutoInstance` substitutes a pair in. Kept
    /// apart from `AutoRule` because it isn't matchable until filled.
    AutoTemplate {
        name: String,
        /// Pattern inner text (no surrounding `/…/`), placeholders intact.
        pattern: String,
        postings: Vec<AutoPosting>,
        /// Optional `amount <op> N` filter, applied to every instantiation.
        condition: Option<AmountCondition>,
    },

    /// `= NAME arg1 arg2` — instantiate template `NAME` with a pair. The
    /// resolver substitutes the args in both orderings (one rule per
    /// direction) and emits concrete `AutoRule`s.
    AutoInstance {
        name: String,
        args: Vec<String>,
    },

    /// `~ PERIOD [CADENCE]` block + indented postings — a *periodic*
    /// transaction. The resolver expands it into real, ordinary transactions:
    /// one per cadence occurrence in the year, dated at the occurrence start,
    /// with the written amounts (the period *total*) divided across them
    /// (`monthly` → ÷12, `daily` → ÷days; the last occurrence absorbs the
    /// rounding remainder). `$year`/`$month`/`$day` in an account are filled
    /// from each occurrence's date. Unlike ledger's `~` (an unbounded
    /// budget/forecast that *repeats* the amount), acc's are bounded to the year
    /// and *split* the total; the generated transactions are real — they book,
    /// balance, and auto-fill a bare posting like any hand-written entry.
    Periodic {
        /// The period token — a bare year `YYYY`.
        period: String,
        /// How to spread the block across the year (`~ 2021 monthly …`).
        cadence: Cadence,
        /// Optional description for the generated transactions (`~ 2021 note`);
        /// empty when the header carries only the period (and cadence).
        description: String,
        postings: Vec<Located<Posting>>,
    },
}

/// How a `~` periodic block spreads its amounts across the year. The written
/// amounts are the period *total*; a cadence divides them across its
/// occurrences — the last occurrence absorbs any rounding remainder so the sum
/// stays exact — and dates one transaction at the start of each occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    /// No cadence keyword — one transaction, `YYYY-01-01`, the full amount.
    Yearly,
    /// `monthly` — 12 transactions, `YYYY-MM-01`, each the total ÷ 12.
    Monthly,
    /// `daily` — one transaction per day of the year, each the total ÷ days.
    Daily,
}

/// Comparison operator for an `amount` clause on an auto-rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOp {
    Gt,
    Lt,
    Ge,
    Le,
    Eq,
    Ne,
}

/// An optional `amount <op> <value>` clause after an auto-rule pattern — the
/// rule fires only when the matched posting's amount satisfies it. A composable
/// filter, deliberately limited to one comparison against a bare number (no
/// expression language): AND is more clauses, OR is more rules, NOT flips the op.
#[derive(Debug, Clone)]
pub struct AmountCondition {
    pub op: CompareOp,
    pub value: Decimal,
}

impl AmountCondition {
    /// Whether `value` — a matched posting's amount — satisfies the clause.
    pub fn matches(&self, value: &Decimal) -> bool {
        match self.op {
            CompareOp::Gt => *value > self.value,
            CompareOp::Lt => *value < self.value,
            CompareOp::Ge => *value >= self.value,
            CompareOp::Le => *value <= self.value,
            CompareOp::Eq => *value == self.value,
            CompareOp::Ne => *value != self.value,
        }
    }
}

/// An auto-transaction (`= /pattern/`) block.
#[derive(Debug, Clone)]
pub struct AutoRule {
    pub pattern: AutoPattern,
    pub postings: Vec<AutoPosting>,
    /// Optional `amount <op> N` filter on the matched posting's amount.
    pub condition: Option<AmountCondition>,
}

/// Pattern kinds supported in V1 — a subset of ledger-cli regex
/// semantics, matching what the filter DSL already handles: a
/// `^prefix` anchor, a `suffix$` anchor, an anchored-both `^exact$`,
/// or an unanchored substring. Plus a single placeholder, `$segment`,
/// standing for exactly one account segment (`[^:]+`). It is *not*
/// regex: the only metacharacters are the `^` / `$` anchors and the
/// literal `$segment` token — no ranges, classes or quantifiers.
#[derive(Debug, Clone)]
pub enum AutoPattern {
    Prefix(String),
    Suffix(String),
    Exact(String),
    Contains(String),
    /// Pattern with one or more `$segment` placeholders. `parts` are the
    /// literal chunks the pattern splits into on `$segment`; between each
    /// consecutive pair exactly one account segment must sit. The anchors
    /// apply to the first / last literal.
    Segmented {
        parts: Vec<String>,
        anchored_start: bool,
        anchored_end: bool,
    },
}

impl AutoPattern {
    pub fn matches(&self, account: &str) -> bool {
        match self {
            AutoPattern::Prefix(s) => account.starts_with(s.as_str()),
            AutoPattern::Suffix(s) => account.ends_with(s.as_str()),
            AutoPattern::Exact(s) => account == s,
            AutoPattern::Contains(s) => account.contains(s.as_str()),
            AutoPattern::Segmented {
                parts,
                anchored_start,
                anchored_end,
            } => matches_segments(account, parts, *anchored_start, *anchored_end),
        }
    }

    /// Build a pattern from its inner text — the part between the `/…/`
    /// delimiters, already stripped. `^` anchors the start, `$` the end,
    /// and each `$segment` token stands for exactly one account segment.
    /// Shared by the parser (`= /pattern/`) and the resolver, which calls
    /// it on a template pattern after substituting a pair in. The caller
    /// guarantees `inner` is non-empty.
    pub fn parse_inner(inner: &str) -> AutoPattern {
        let anchored_start = inner.starts_with('^');
        let anchored_end = inner.ends_with('$');
        let core = match (anchored_start, anchored_end) {
            (true, true) => &inner[1..inner.len() - 1],
            (true, false) => &inner[1..],
            (false, true) => &inner[..inner.len() - 1],
            (false, false) => inner,
        };
        // `$segment` placeholder(s): split into literal chunks matched
        // segment-wise. Not regex — the only token is the literal `$segment`.
        if core.contains("$segment") {
            let parts = core.split("$segment").map(str::to_string).collect();
            return AutoPattern::Segmented {
                parts,
                anchored_start,
                anchored_end,
            };
        }
        match (anchored_start, anchored_end) {
            (true, true) => AutoPattern::Exact(core.to_string()),
            (true, false) => AutoPattern::Prefix(core.to_string()),
            (false, true) => AutoPattern::Suffix(core.to_string()),
            (false, false) => AutoPattern::Contains(core.to_string()),
        }
    }
}

/// Match `account` against a `$segment`-templated pattern: the literal
/// `parts` in order, with exactly one account segment (`[^:]+` — a
/// non-empty run without `:`) filling each gap between consecutive
/// parts. A single left-to-right scan, no regex and no backtracking, so
/// each `$segment` is meant to sit between `:` delimiters or an anchor.
fn matches_segments(
    account: &str,
    parts: &[String],
    anchored_start: bool,
    anchored_end: bool,
) -> bool {
    // Leading literal. Anchored: it must start the account; unanchored:
    // it may appear anywhere (its first occurrence).
    let mut rest = if anchored_start {
        match account.strip_prefix(parts[0].as_str()) {
            Some(r) => r,
            None => return false,
        }
    } else {
        match account.find(parts[0].as_str()) {
            Some(i) => &account[i + parts[0].len()..],
            None => return false,
        }
    };

    // Each remaining part is preceded by one `$segment` gap.
    for (i, lit) in parts.iter().enumerate().skip(1) {
        // Consume exactly one non-empty segment (up to the next `:`).
        let seg_end = rest.find(':').unwrap_or(rest.len());
        if seg_end == 0 {
            return false;
        }
        rest = &rest[seg_end..];

        if i == parts.len() - 1 && anchored_end {
            return rest == lit.as_str();
        }
        match rest.strip_prefix(lit.as_str()) {
            Some(r) => rest = r,
            None => return false,
        }
    }
    true
}

/// How much an auto-rule posting injects, relative to the triggering amount.
/// Written in the amount slot after the account:
///
/// - a bare number, or `amount` (= `1`) / `-amount` (= `-1`) → [`Factor`]:
///   inject `factor × trigger`.
/// - `clamp(amount)` / `-clamp(amount)` → [`Clamp`]: inject the trigger clamped
///   to this posting's account's remaining headroom to zero.
/// - nothing at all (a bare `[account]`) → [`Fill`]: the balancing leg, filled
///   by the expander with the negated sum of its pool's other injected legs —
///   like the bare last posting of a hand-written transaction.
///
/// [`Factor`]: AutoAmount::Factor
/// [`Clamp`]: AutoAmount::Clamp
/// [`Fill`]: AutoAmount::Fill
#[derive(Debug, Clone)]
pub enum AutoAmount {
    Factor(Decimal),
    /// `clamp(amount)` / `-clamp(amount)` — inject the trigger, but clamped so
    /// this posting's own account is not driven past zero: it fills only the
    /// remaining headroom (`min(|trigger|, balance)`). `negate` is the
    /// `-clamp(...)` form. Evaluated in the expander against the running account
    /// balance; a pool with a clamp leg needs a `Fill` leg to absorb it.
    Clamp {
        negate: bool,
    },
    Fill,
}

/// One posting inside an auto-rule. Account + amount + virtual flags mirror
/// the posting syntax; the amount is applied to the triggering posting during
/// expansion.
#[derive(Debug, Clone)]
pub struct AutoPosting {
    pub account: String,
    pub amount: AutoAmount,
    pub is_virtual: bool,
    pub balanced: bool,
}

/// A `P DATE BASE QUOTE RATE` directive. Commodities are stored as
/// `Arc<str>` so the resolver (and downstream phases) can intern them
/// without cloning string buffers. Alias resolution is deferred to the
/// resolve phase.
#[derive(Debug, Clone)]
pub struct Price {
    pub date: Date,
    pub base: Arc<str>,
    pub quote: Arc<str>,
    pub rate: Decimal,
}

#[cfg(test)]
mod tests {
    use super::AutoPattern;

    fn seg(parts: &[&str], start: bool, end: bool) -> AutoPattern {
        AutoPattern::Segmented {
            parts: parts.iter().map(|s| s.to_string()).collect(),
            anchored_start: start,
            anchored_end: end,
        }
    }

    #[test]
    fn segment_wildcard_matches_exactly_one_leading_segment() {
        // `^$segment:bar:baz`
        let p = seg(&["", ":bar:baz"], true, false);
        assert!(p.matches("foo:bar:baz"));
        assert!(p.matches("qux:bar:baz-suffix"));
    }

    #[test]
    fn segment_wildcard_rejects_wrong_depth() {
        // `^$segment:bar:baz` must not match a deeper occurrence…
        let p = seg(&["", ":bar:baz"], true, false);
        assert!(!p.matches("foo:extra:bar:baz")); // two segments before :bar:
        assert!(!p.matches("bar:baz")); // no segment before :bar:
    }

    #[test]
    fn segment_wildcard_matches_a_middle_segment() {
        // `:bar:$segment:qux`
        let p = seg(&[":bar:", ":qux"], false, false);
        assert!(p.matches("foo:bar:mid:qux"));
        assert!(!p.matches("foo:bar::qux")); // empty middle segment
    }

    #[test]
    fn segment_wildcard_anchored_both_ends_is_exactly_one_segment() {
        // `^$segment$`
        let p = seg(&["", ""], true, true);
        assert!(p.matches("foo"));
        assert!(!p.matches("foo:bar")); // more than one segment
        assert!(!p.matches("")); // empty
    }
}
