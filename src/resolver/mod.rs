//! Resolve phase.
//!
//! Consumes the raw `Vec<Located<Entry>>` produced by the parser and
//! returns the data shape the later phases (pricedb build, balance)
//! expect:
//!
//! - commodity aliases are applied to every Price and every Posting
//!   Amount slot (amount, costs, balance_assertion);
//! - `slippage gain`/`slippage loss`, `cta gain`/`cta loss` and
//!   `capital gain`/`capital loss` account declarations are extracted;
//! - transactions and prices are split into separate, date-sorted vecs;
//! - all other entries (Commodity/Account scaffolds, Comment) are
//!   dropped — their information has been extracted.
//!
//! Errors on alias conflicts (`$ → USD` and later `$ → EUR`) and on
//! duplicate fx / cta / capital account declarations.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::parser::entry::Entry;
use crate::parser::located::Located;
use crate::parser::posting::{Costs, Posting};
use crate::parser::transaction::Transaction;
use crate::parser::entry::Price;

pub mod error;

pub use error::ResolveError;

/// A set of account labels: exact full-name entries plus `$segment`
/// wildcard patterns. One of these per view (plus a shared base); the
/// exact map is consulted first, then the patterns.
#[derive(Debug, Clone, Default)]
pub struct LabelSet {
    pub exact: HashMap<String, String>,
    pub patterns: Vec<(crate::parser::entry::AutoPattern, String)>,
}

impl LabelSet {
    /// The label for `account` in this set: exact match first, then the
    /// first matching `$segment` pattern.
    pub fn get(&self, account: &str) -> Option<&str> {
        if let Some(label) = self.exact.get(account) {
            return Some(label);
        }
        self.patterns
            .iter()
            .find(|(pattern, _)| pattern.matches(account))
            .map(|(_, label)| label.as_str())
    }
}

/// Output of normalization. Transactions and prices are in date order;
/// declarations are extracted into their own fields.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub transactions: Vec<Located<Transaction>>,
    pub prices: Vec<Located<Price>>,
    pub slippage_gain: Option<String>,
    pub slippage_loss: Option<String>,
    pub cta_gain: Option<String>,
    pub cta_loss: Option<String>,
    /// Declared via `account NAME / capital gain` / `capital loss`.
    /// Both must be present for the lot/capital-gains phase to run.
    pub capital_gain: Option<String>,
    pub capital_loss: Option<String>,
    /// Declared via `account NAME / holding gain` / `holding
    /// loss`. Both must be present for the `--unrealized` revaluator to run.
    pub holding_gain: Option<String>,
    pub holding_loss: Option<String>,
    /// Explicit `precision N` values from `commodity` directives.
    /// The loader merges these over the amount-derived `Journal.precisions`
    /// so declared commodities render with exactly N fractional digits,
    /// regardless of what the posting amounts contain.
    pub precisions: HashMap<String, usize>,
    /// `alias → canonical` map collected from `commodity` directives.
    /// Handed downstream so CLI targets like `-X EUR` can be resolved
    /// to `€` before they reach the rebalancer or the price DB.
    pub aliases: HashMap<String, String>,
    /// Automated-transaction rules collected from `= /pattern/` blocks.
    /// The expander phase applies these after the booker — for every
    /// posting account that matches a rule, the rule's extra postings
    /// are injected into the same transaction, scaled by the
    /// triggering amount.
    pub auto_rules: Vec<crate::parser::entry::AutoRule>,
    /// `account NAME / label <text>` declarations — cosmetic display
    /// labels. `labels` is the shared fallback (bare `label`); the two
    /// view-specific sets (`label-balance` / `label-register`) override
    /// it per view. Each set holds exact names and `$segment` patterns.
    pub labels: LabelSet,
    pub labels_balance: LabelSet,
    pub labels_register: LabelSet,
}

pub fn resolve(entries: Vec<Located<Entry>>) -> Result<Resolved, ResolveError> {
    let Declarations {
        aliases,
        roles,
        precisions,
        labels,
        labels_balance,
        labels_register,
    } = collect_declarations(&entries)?;

    // The pipeline phases consume specific roles by name; this is the one
    // place those semantic keys live. Everything else — parsing, conflict
    // checks, `$role:slot` resolution — stays generic over `roles`.
    let slippage_gain = roles.get("slippage gain").cloned();
    let slippage_loss = roles.get("slippage loss").cloned();
    let cta_gain = roles.get("cta gain").cloned();
    let cta_loss = roles.get("cta loss").cloned();
    let capital_gain = roles.get("capital gain").cloned();
    let capital_loss = roles.get("capital loss").cloned();
    let holding_gain = roles.get("holding gain").cloned();
    let holding_loss = roles.get("holding loss").cloned();

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
                    if let Some(name) = resolve_role_account(&lp.value.account, &roles) {
                        lp.value.account = name;
                    }
                }
                transactions.push(Located { file, line, value: tx });
            }
            Entry::AutoRule(rule) => {
                // Commodity aliases are not applied to auto-rule account
                // names — aliases rename commodities, not accounts. An
                // empty rule (no postings) is useless; reject it.
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
                    total += p.multiplier;
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
                auto_rules.push(rule);
            }
            // Commodity/Account scaffolds and Comment entries carry no
            // data we need past this point — drop them.
            _ => {}
        }
    }

    // Transactions must be date-sorted: the booker validates balance
    // assertions in chronological order.
    transactions.sort_by_key(|a| a.value.date);
    // Prices are NOT sorted here: the indexer stores each pair's series
    // in a `BTreeMap<day, rate>` that orders itself, and a same-day
    // collision resolves to the last directive in file order either way
    // (a stable sort wouldn't change it). Sorting ~800k price directives
    // is pure overhead — skip it.

    Ok(Resolved {
        transactions,
        prices,
        slippage_gain,
        slippage_loss,
        cta_gain,
        cta_loss,
        capital_gain,
        capital_loss,
        holding_gain,
        holding_loss,
        precisions,
        aliases,
        auto_rules,
        labels,
        labels_balance,
        labels_register,
    })
}

/// Resolve a `$role:slot` account reference (e.g. `$capital:gain`) to the
/// account declared for that role. The token after `$` is matched
/// generically — colons become spaces (`capital:gain` → `capital gain`)
/// and the result is looked up among the declared role directives. No
/// role names are baked in, so a role is referenceable the moment it is
/// declared.
///
/// Returns `None` — leave the account verbatim — both for a plain account
/// and for a `$` reference whose role no `account` directive declares. The
/// latter is deliberately lenient: `acc format` (and any single-file run)
/// must round-trip a `$role:slot` reference without the central config
/// that declares the role. `acc lint` warns on any `$…` account that
/// survives unresolved, so a genuine typo still surfaces.
fn resolve_role_account(account: &str, roles: &HashMap<String, String>) -> Option<String> {
    let token = account.strip_prefix('$')?;
    roles.get(&token.replace(':', " ")).cloned()
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

/// First pass: walk entries, build the alias table, index every role
/// account by its directive text, and collect precision overrides.
/// Errors on a conflicting re-declaration (same role, different account).
/// Declarations gathered in a first pass over the entry stream, before
/// the transactions are resolved: commodity aliases, the role → account
/// index, per-commodity precisions, and cosmetic account labels.
struct Declarations {
    aliases: HashMap<String, String>,
    roles: HashMap<String, String>,
    precisions: HashMap<String, usize>,
    labels: LabelSet,
    labels_balance: LabelSet,
    labels_register: LabelSet,
}

fn collect_declarations(entries: &[Located<Entry>]) -> Result<Declarations, ResolveError> {
    let mut aliases: HashMap<String, String> = HashMap::new();
    // Role directives indexed by their verbatim text (`capital gain`,
    // `cta loss`, …) → declared account. One generic map in place of the
    // former per-role fields: a new role needs no change here.
    let mut roles: HashMap<String, Declaration> = HashMap::new();
    let mut precisions: HashMap<String, usize> = HashMap::new();
    // `label` / `label-balance` / `label-register` display labels.
    // `labels` is the shared fallback; the view-specific sets override it.
    let mut labels = LabelSet::default();
    let mut labels_balance = LabelSet::default();
    let mut labels_register = LabelSet::default();

    for e in entries {
        match &e.value {
            Entry::Commodity { symbol, aliases: list, precision } => {
                for a in list {
                    if let Some(existing) = aliases.get(a)
                        && existing != symbol {
                            return Err(ResolveError::new(
                                e.file.clone(),
                                e.line,
                                format!(
                                    "alias `{}` already maps to `{}`, cannot remap to `{}`",
                                    a, existing, symbol
                                ),
                            ));
                        }
                    aliases.insert(a.clone(), symbol.clone());
                }
                if let Some(p) = precision {
                    precisions.insert(symbol.clone(), *p);
                }
            }
            Entry::RoleAccount { role, account } => {
                // Display-label sub-directives, not roles: bare `label`
                // (shared fallback) or the view-specific `label-balance` /
                // `label-register`, which override it per view. A `$segment`
                // in the account name makes it a wildcard (anchored to the
                // whole name); otherwise it is an exact full-name entry.
                let target = if let Some(t) = role.strip_prefix("label-balance ") {
                    Some((&mut labels_balance, t))
                } else if let Some(t) = role.strip_prefix("label-register ") {
                    Some((&mut labels_register, t))
                } else {
                    role.strip_prefix("label ").map(|t| (&mut labels, t))
                };
                if let Some((set, text)) = target {
                    let text = text.trim().to_string();
                    if account.contains("$segment") {
                        let parts = account.split("$segment").map(str::to_string).collect();
                        set.patterns.push((
                            crate::parser::entry::AutoPattern::Segmented {
                                parts,
                                anchored_start: true,
                                anchored_end: true,
                            },
                            text,
                        ));
                    } else {
                        set.exact.insert(account.clone(), text);
                    }
                    continue;
                }
                if let Some(prev) = roles.get(role)
                    && prev.name != *account {
                        return Err(ResolveError::new(
                            e.file.clone(),
                            e.line,
                            format!(
                                "`{}` account already set to `{}` at line {}",
                                role, prev.name, prev.line
                            ),
                        ));
                    }
                roles.insert(role.clone(), Declaration { line: e.line, name: account.clone() });
            }
            _ => {}
        }
    }

    Ok(Declarations {
        aliases,
        roles: roles.into_iter().map(|(role, d)| (role, d.name)).collect(),
        precisions,
        labels,
        labels_balance,
        labels_register,
    })
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
    fn extracts_slippage_accounts() {
        let src = "account Equity:SlippageGain\n    slippage gain\naccount Equity:SlippageLoss\n    slippage loss\n";
        let out = resolve(parsed(src)).unwrap();
        assert_eq!(out.slippage_gain.as_deref(), Some("Equity:SlippageGain"));
        assert_eq!(out.slippage_loss.as_deref(), Some("Equity:SlippageLoss"));
    }

    #[test]
    fn splits_exact_and_segment_labels() {
        let src = "account assets:cash\n    label liquid\n\
                   account $segment:baz\n    label tag\n";
        let out = resolve(parsed(src)).unwrap();
        // Exact name and `$segment` pattern both resolve via the base set;
        // the pattern is anchored to the whole account name.
        assert_eq!(out.labels.get("assets:cash"), Some("liquid"));
        assert_eq!(out.labels.get("foo:baz"), Some("tag"));
        assert_eq!(out.labels.get("foo:baz:sub"), None);
    }

    #[test]
    fn view_labels_and_multiple_sub_directives() {
        let src = "account a:1\n    label base\n    label-register reg-only\n\
                   account a:2\n    label-balance bal-only\n";
        let out = resolve(parsed(src)).unwrap();
        // a:1 declares two sub-directives: a shared fallback and a
        // register-specific override; no balance-specific label.
        assert_eq!(out.labels.get("a:1"), Some("base"));
        assert_eq!(out.labels_register.get("a:1"), Some("reg-only"));
        assert_eq!(out.labels_balance.get("a:1"), None);
        // a:2 is balance-only, no shared fallback.
        assert_eq!(out.labels_balance.get("a:2"), Some("bal-only"));
        assert_eq!(out.labels.get("a:2"), None);
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
    fn conflicting_slippage_gain_error() {
        let src = "account Equity:A\n    slippage gain\naccount Equity:B\n    slippage gain\n";
        let err = resolve(parsed(src)).unwrap_err();
        assert!(err.message.contains("slippage gain"));
    }

    #[test]
    fn resolves_role_account_references() {
        let src = "account income:cap:market\n    capital gain\n\
                   account expenses:cap:market\n    capital loss\n\
                   account income:cap:cta\n    cta gain\n\
                   2024-06-15 * sell\n    assets:eth  -6 EUR\n    $capital:gain  2 EUR\n    $capital:loss  2 EUR\n    $cta:gain  2 EUR\n";
        let out = resolve(parsed(src)).unwrap();
        let acct = |i: usize| out.transactions[0].value.postings[i].value.account.as_str();
        assert_eq!(acct(1), "income:cap:market"); // $capital:gain
        assert_eq!(acct(2), "expenses:cap:market"); // $capital:loss
        assert_eq!(acct(3), "income:cap:cta"); // $cta:gain
    }

    #[test]
    fn unresolved_role_reference_passes_through() {
        // A `$ref` to a role no `account` declares (here `unknown gain`, and a
        // typo) is left verbatim, not an error — so `acc format` can
        // round-trip a single file without the central config. `acc lint`
        // is what flags the leftover `$…` account.
        let src = "account income:cap\n    capital gain\n\
                   2024-06-15 * x\n    a  -2 EUR\n    $unknown:gain  1 EUR\n    $captial:gain  1 EUR\n";
        let out = resolve(parsed(src)).unwrap();
        let acct = |i: usize| out.transactions[0].value.postings[i].value.account.as_str();
        assert_eq!(acct(1), "$unknown:gain");
        assert_eq!(acct(2), "$captial:gain");
    }

    #[test]
    fn plain_account_and_commodity_are_dropped() {
        let src = "commodity USD\naccount Assets:Bank\n";
        let out = resolve(parsed(src)).unwrap();
        assert!(out.transactions.is_empty());
        assert!(out.prices.is_empty());
        assert!(out.slippage_gain.is_none());
        assert!(out.slippage_loss.is_none());
    }
}
