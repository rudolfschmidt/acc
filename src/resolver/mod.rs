//! Resolve phase.
//!
//! Consumes the raw `Vec<Located<Entry>>` produced by the parser and
//! returns the data shape the later phases (pricedb build, balance)
//! expect:
//!
//! - commodity aliases are applied to every Price and every Posting
//!   Amount slot (amount, costs, balance_assertion);
//! - `fx gain` / `fx loss` account declarations are extracted;
//! - transactions and prices are split into separate, date-sorted vecs;
//! - all other entries (Commodity/Account scaffolds, Comment) are
//!   dropped — their information has been extracted.
//!
//! Errors on alias conflicts (`$ → USD` and later `$ → EUR`) and on
//! duplicate fx-gain / fx-loss declarations.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::parser::entry::Entry;
use crate::parser::located::Located;
use crate::parser::posting::{Costs, Posting};
use crate::parser::transaction::Transaction;
use crate::parser::entry::Price;

pub mod error;

pub use error::ResolveError;

/// Output of normalization. Transactions and prices are in date order;
/// declarations are extracted into their own fields.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub transactions: Vec<Located<Transaction>>,
    pub prices: Vec<Located<Price>>,
    pub fx_gain: Option<String>,
    pub fx_loss: Option<String>,
    pub cta_gain: Option<String>,
    pub cta_loss: Option<String>,
    /// Explicit `precision N` values from `commodity` directives.
    /// The loader merges these over the amount-derived `Journal.precisions`
    /// so declared commodities render with exactly N fractional digits,
    /// regardless of what the posting amounts contain.
    pub precisions: HashMap<String, usize>,
    /// `alias → canonical` map collected from `commodity` directives.
    /// Handed downstream so CLI targets like `-x EUR` can be resolved
    /// to `€` before they reach the rebalancer or the price DB.
    pub aliases: HashMap<String, String>,
    /// Automated-transaction rules collected from `= /pattern/` blocks.
    /// The expander phase applies these after the booker — for every
    /// posting account that matches a rule, the rule's extra postings
    /// are injected into the same transaction, scaled by the
    /// triggering amount.
    pub auto_rules: Vec<crate::parser::entry::AutoRule>,
}

pub fn resolve(entries: Vec<Located<Entry>>) -> Result<Resolved, ResolveError> {
    let (aliases, fx_gain, fx_loss, cta_gain, cta_loss, precisions) =
        collect_declarations(&entries)?;

    // Parallel Arc-based alias table for the Price path. Each alias
    // maps to an interned primary `Arc<str>`; the same interner is
    // reused for every commodity symbol that flows through Price so
    // that ~200 unique symbols back ~780k price directives with just
    // ~200 live Arc allocations (instead of ~1.56M fresh String heaps).
    let mut interner: HashSet<Arc<str>> = HashSet::new();
    let mut arc_aliases: HashMap<String, Arc<str>> = HashMap::new();
    for (alias, primary) in &aliases {
        let primary_arc = intern_str(&mut interner, primary.as_str());
        arc_aliases.insert(alias.clone(), primary_arc);
    }

    let mut transactions = Vec::new();
    let mut prices = Vec::new();
    let mut auto_rules = Vec::new();

    for Located { file, line, value } in entries {
        match value {
            Entry::Price(mut p) => {
                p.base = resolve_arc(&mut interner, &arc_aliases, p.base);
                p.quote = resolve_arc(&mut interner, &arc_aliases, p.quote);
                prices.push(Located { file, line, value: p });
            }
            Entry::Transaction(mut tx) => {
                if tx.postings.len() < 2 {
                    return Err(ResolveError::new(
                        file.clone(),
                        line,
                        format!(
                            "transaction `{}` must have at least two postings, got {}",
                            tx.description.trim(),
                            tx.postings.len()
                        ),
                    ));
                }
                for lp in &mut tx.postings {
                    apply_to_posting(&mut lp.value, &aliases);
                }
                transactions.push(Located { file, line, value: tx });
            }
            Entry::AutoRule(mut rule) => {
                // Apply commodity aliases to the injected postings'
                // account names? No — aliases are commodity aliases,
                // not account aliases. Accounts aren't renamed. Just
                // collect the rule for the expander.
                // But: an empty rule (no postings) is useless; reject.
                if rule.postings.is_empty() {
                    return Err(ResolveError::new(
                        file.clone(),
                        line,
                        "auto-rule has no postings",
                    ));
                }
                // Sanity: multipliers must sum to zero for the expanded
                // postings to balance. Reject otherwise early so the
                // booker won't get confused downstream.
                let mut total = crate::decimal::Decimal::zero();
                for p in &rule.postings {
                    total = total + p.multiplier;
                }
                if !total.is_zero() {
                    return Err(ResolveError::new(
                        file.clone(),
                        line,
                        format!(
                            "auto-rule multipliers must sum to zero, got {}",
                            total
                        ),
                    ));
                }
                // Strip any aliases that resolve in posting accounts —
                // not relevant for auto-rules (account names are
                // literal), just store as-is.
                for _ in &mut rule.postings {
                    // Placeholder: no per-posting alias work needed.
                }
                auto_rules.push(rule);
            }
            // Commodity/Account scaffolds and Comment entries carry no
            // data we need past this point — drop them.
            _ => {}
        }
    }

    transactions.sort_by(|a, b| a.value.date.cmp(&b.value.date));
    prices.sort_by(|a, b| a.value.date.cmp(&b.value.date));

    Ok(Resolved {
        transactions,
        prices,
        fx_gain,
        fx_loss,
        cta_gain,
        cta_loss,
        precisions,
        aliases,
        auto_rules,
    })
}

/// Return the interned `Arc<str>` for `s`, inserting it on first sight.
fn intern_str(interner: &mut HashSet<Arc<str>>, s: &str) -> Arc<str> {
    if let Some(existing) = interner.get(s) {
        return existing.clone();
    }
    let arc: Arc<str> = Arc::from(s);
    interner.insert(arc.clone());
    arc
}

/// Resolve a commodity Arc to its canonical interned form, applying
/// aliases and deduplicating against the interner. The input `arc`
/// gets dropped when a shared copy already exists — this is the core
/// of the memory win: per-directive Arcs collapse into ~200 shared
/// references.
fn resolve_arc(
    interner: &mut HashSet<Arc<str>>,
    aliases: &HashMap<String, Arc<str>>,
    arc: Arc<str>,
) -> Arc<str> {
    if let Some(primary) = aliases.get(arc.as_ref()) {
        return primary.clone();
    }
    if let Some(existing) = interner.get(arc.as_ref()) {
        return existing.clone();
    }
    interner.insert(arc.clone());
    arc
}

/// First pass: walk entries, build the alias table and capture the
/// fx-gain / fx-loss / cta-gain / cta-loss accounts. Errors on any
/// conflict.
fn collect_declarations(
    entries: &[Located<Entry>],
) -> Result<
    (
        HashMap<String, String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        HashMap<String, usize>,
    ),
    ResolveError,
> {
    let mut aliases: HashMap<String, String> = HashMap::new();
    let mut fx_gain: Option<Declaration> = None;
    let mut fx_loss: Option<Declaration> = None;
    let mut cta_gain: Option<Declaration> = None;
    let mut cta_loss: Option<Declaration> = None;
    let mut precisions: HashMap<String, usize> = HashMap::new();

    for e in entries {
        match &e.value {
            Entry::Commodity { symbol, aliases: list, precision } => {
                for a in list {
                    if let Some(existing) = aliases.get(a) {
                        if existing != symbol {
                            return Err(ResolveError::new(
                                e.file.clone(),
                                e.line,
                                format!(
                                    "alias `{}` already maps to `{}`, cannot remap to `{}`",
                                    a, existing, symbol
                                ),
                            ));
                        }
                    }
                    aliases.insert(a.clone(), symbol.clone());
                }
                if let Some(p) = precision {
                    precisions.insert(symbol.clone(), *p);
                }
            }
            Entry::FxGainAccount(name) => {
                if let Some(prev) = &fx_gain {
                    if prev.name != *name {
                        return Err(ResolveError::new(
                            e.file.clone(),
                            e.line,
                            format!(
                                "fx gain account already set to `{}` at line {}",
                                prev.name, prev.line
                            ),
                        ));
                    }
                }
                fx_gain = Some(Declaration { line: e.line, name: name.clone() });
            }
            Entry::FxLossAccount(name) => {
                if let Some(prev) = &fx_loss {
                    if prev.name != *name {
                        return Err(ResolveError::new(
                            e.file.clone(),
                            e.line,
                            format!(
                                "fx loss account already set to `{}` at line {}",
                                prev.name, prev.line
                            ),
                        ));
                    }
                }
                fx_loss = Some(Declaration { line: e.line, name: name.clone() });
            }
            Entry::CtaGainAccount(name) => {
                if let Some(prev) = &cta_gain {
                    if prev.name != *name {
                        return Err(ResolveError::new(
                            e.file.clone(),
                            e.line,
                            format!(
                                "cta gain account already set to `{}` at line {}",
                                prev.name, prev.line
                            ),
                        ));
                    }
                }
                cta_gain = Some(Declaration { line: e.line, name: name.clone() });
            }
            Entry::CtaLossAccount(name) => {
                if let Some(prev) = &cta_loss {
                    if prev.name != *name {
                        return Err(ResolveError::new(
                            e.file.clone(),
                            e.line,
                            format!(
                                "cta loss account already set to `{}` at line {}",
                                prev.name, prev.line
                            ),
                        ));
                    }
                }
                cta_loss = Some(Declaration { line: e.line, name: name.clone() });
            }
            _ => {}
        }
    }

    Ok((
        aliases,
        fx_gain.map(|d| d.name),
        fx_loss.map(|d| d.name),
        cta_gain.map(|d| d.name),
        cta_loss.map(|d| d.name),
        precisions,
    ))
}

/// A single-fact declaration that lives only long enough to catch a
/// conflicting re-declaration. The `line` is carried along for the
/// error message; the final `Resolved` struct only keeps `name`.
struct Declaration {
    line: usize,
    name: String,
}

fn apply_alias(commodity: &mut String, aliases: &HashMap<String, String>) {
    if let Some(primary) = aliases.get(commodity) {
        *commodity = primary.clone();
    }
}

fn apply_to_posting(p: &mut Posting, aliases: &HashMap<String, String>) {
    if let Some(a) = &mut p.amount {
        apply_alias(&mut a.commodity, aliases);
    }
    if let Some(c) = &mut p.costs {
        let a = match c {
            Costs::Total(a) | Costs::PerUnit(a) => a,
        };
        apply_alias(&mut a.commodity, aliases);
    }
    if let Some(a) = &mut p.balance_assertion {
        apply_alias(&mut a.commodity, aliases);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn parsed(src: &str) -> Vec<Located<Entry>> {
        parser::parse(src).unwrap()
    }

    #[test]
    fn applies_alias_to_price() {
        let src = "commodity USD\n    alias $\nP 2024-06-15 $ EUR 0.92\n";
        let out = resolve(parsed(src)).unwrap();
        assert_eq!(out.prices.len(), 1);
        assert_eq!(&*out.prices[0].value.base, "USD");
        assert_eq!(&*out.prices[0].value.quote, "EUR");
    }

    #[test]
    fn applies_alias_to_posting_amount() {
        let src = "commodity USD\n    alias $\n2024-06-15 * X\n    expenses:food   $5\n    assets:cash  $-5\n";
        let out = resolve(parsed(src)).unwrap();
        let amt = out.transactions[0].value.postings[0].value.amount.as_ref().unwrap();
        assert_eq!(amt.commodity, "USD");
    }

    #[test]
    fn extracts_fx_accounts() {
        let src = "account Equity:FxGain\n    fx gain\naccount Equity:FxLoss\n    fx loss\n";
        let out = resolve(parsed(src)).unwrap();
        assert_eq!(out.fx_gain.as_deref(), Some("Equity:FxGain"));
        assert_eq!(out.fx_loss.as_deref(), Some("Equity:FxLoss"));
    }

    #[test]
    fn sorts_transactions_by_date() {
        let src = "2024-06-15 * Later\n    assets:cash  1 USD\n    equity  -1 USD\n\
                   2024-06-14 * Earlier\n    assets:cash  2 USD\n    equity  -2 USD\n";
        let out = resolve(parsed(src)).unwrap();
        assert_eq!(out.transactions[0].value.description, "Earlier");
        assert_eq!(out.transactions[1].value.description, "Later");
    }

    #[test]
    fn conflicting_aliases_error() {
        let src = "commodity USD\n    alias $\ncommodity EUR\n    alias $\n";
        let err = resolve(parsed(src)).unwrap_err();
        assert!(err.message.contains("alias"));
        assert!(err.message.contains("$"));
    }

    #[test]
    fn conflicting_fx_gain_error() {
        let src = "account Equity:A\n    fx gain\naccount Equity:B\n    fx gain\n";
        let err = resolve(parsed(src)).unwrap_err();
        assert!(err.message.contains("fx gain"));
    }

    #[test]
    fn plain_account_and_commodity_are_dropped() {
        let src = "commodity USD\naccount Assets:Bank\n";
        let out = resolve(parsed(src)).unwrap();
        assert!(out.transactions.is_empty());
        assert!(out.prices.is_empty());
        assert!(out.fx_gain.is_none());
        assert!(out.fx_loss.is_none());
    }
}
