//! Resolve phase.
//!
//! Consumes the raw `Vec<Located<Entry>>` produced by the parser and
//! returns the data shape the later phases (pricedb build, balance)
//! expect:
//!
//! - commodity aliases are applied to every Price and every Posting
//!   Amount slot (amount, costs, balance_assertion);
//! - a `commodity S / parity T` declaration is turned into a synthetic
//!   1:1 `Price` (S T, rate 1, day 0) so the valuation path values S as
//!   T without folding S's display — the price index / BFS chain it;
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

use crate::date::Date;
use crate::decimal::Decimal;
use crate::parser::entry::{AmountCondition, AutoPattern, AutoPosting, AutoRule, Entry};
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

/// A named auto-rule template from `= NAME :: /pattern/`. Its `pattern` and
/// posting accounts carry positional `$1`/`$2` placeholders and `lookup(key)`
/// calls; [`expand_instance`] substitutes a pair in to produce concrete
/// `AutoRule`s.
struct Template {
    pattern: String,
    postings: Vec<AutoPosting>,
    condition: Option<AmountCondition>,
}

pub fn resolve(entries: Vec<Located<Entry>>) -> Result<Resolved, ResolveError> {
    let Declarations {
        aliases,
        roles,
        precisions,
        labels,
        labels_balance,
        labels_register,
        defines,
        templates,
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
                // Injected postings keep the transaction balanced iff each
                // balance pool (real, balanced-virtual `[...]`) sums to zero;
                // `(...)` unbalanced postings are exempt, like everywhere in acc.
                if let Err((pool, sum)) = check_multiplier_balance(&rule.postings) {
                    return Err(ResolveError::new(
                        file.clone(),
                        line,
                        format!("auto-rule {pool} multipliers must sum to zero, got {sum}"),
                    ));
                }
                auto_rules.push(rule);
            }
            Entry::Commodity { symbol, parities, .. } => {
                // A `parity T` on commodity S declares a *fixed* 1 S = 1 T
                // conversion: S keeps its own symbol (it is NOT an alias, so
                // never folded) but values 1:1 to T. Emit it as a synthetic
                // price so the ordinary valuation path handles it — the
                // index/BFS chain it (e.g. USDC → $ → €), and `latest_rate`
                // falls back to the earliest entry, so this single day-0 edge
                // covers every date. Any real dated S→T price is newer and
                // wins. The target is alias-resolved like any price commodity;
                // the base (S) is not an alias, so it stays put.
                for target in parities {
                    let base = resolve_arc(&mut interner, &arc_aliases, Arc::from(symbol.as_str()));
                    let quote = resolve_arc(&mut interner, &arc_aliases, Arc::from(target.as_str()));
                    prices.push(Located {
                        file: file.clone(),
                        line,
                        value: Price { date: Date::from_days(0), base, quote, rate: Decimal::from(1) },
                    });
                }
            }
            Entry::AutoInstance { name, args } => {
                // Instantiate a template into concrete auto-rules (one per
                // transfer direction). Templates/defines were gathered in the
                // first pass, so ordering across files doesn't matter.
                let rules = expand_instance(&name, &args, &templates, &defines, &file, line)?;
                auto_rules.extend(rules);
            }
            // Account/Define/AutoTemplate scaffolds and Comment entries carry
            // no data we still need here — the first pass already consumed the
            // defines and templates. Drop them.
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

/// An auto-rule's injected postings keep the transaction balanced iff its real
/// postings sum to zero AND its balanced-virtual `[...]` postings sum to zero,
/// each pool on its own. Unbalanced-virtual `(...)` postings take part in no
/// balance — like everywhere else in acc — so they're exempt: a lone `(...)`
/// posting is valid. Returns the first offending (pool-name, non-zero sum).
fn check_multiplier_balance(postings: &[AutoPosting]) -> Result<(), (&'static str, Decimal)> {
    let mut real = Decimal::zero();
    let mut balanced_virtual = Decimal::zero();
    for p in postings {
        if !p.is_virtual {
            real += p.multiplier;
        } else if p.balanced {
            balanced_virtual += p.multiplier;
        }
    }
    if !real.is_zero() {
        return Err(("real", real));
    }
    if !balanced_virtual.is_zero() {
        return Err(("balanced-virtual `[...]`", balanced_virtual));
    }
    Ok(())
}

/// Expand a `= NAME arg…` instantiation into concrete `AutoRule`s. The pair is
/// instantiated in both orderings — one rule per transfer direction — so a
/// single `= NAME a b` mirrors both `a→b` and `b→a`. Each ordering
/// substitutes the args into the template pattern and posting accounts, then
/// resolves any `table(key)` lookup call against the `define` tables. The
/// resulting rules are ordinary `AutoRule`s the expander runs unchanged.
fn expand_instance(
    name: &str,
    args: &[String],
    templates: &HashMap<String, Template>,
    defines: &HashMap<String, HashMap<String, String>>,
    file: &Arc<str>,
    line: usize,
) -> Result<Vec<AutoRule>, ResolveError> {
    let template = templates.get(name).ok_or_else(|| {
        ResolveError::new(file.clone(), line, format!("no auto-rule template named `{name}`"))
    })?;
    // A pair template takes exactly two positional args (`$1`/`$2`); the pair
    // is unordered, so both orderings are emitted (one rule per direction).
    if args.len() != 2 {
        return Err(ResolveError::new(
            file.clone(),
            line,
            format!("`= {name}` takes exactly two arguments (an unordered pair), got {}", args.len()),
        ));
    }
    // Both orderings — one rule per direction. A self-pair (a == b) collapses
    // to a single rule.
    let mut orderings: Vec<[&str; 2]> = vec![[args[0].as_str(), args[1].as_str()]];
    if args[0] != args[1] {
        orderings.push([args[1].as_str(), args[0].as_str()]);
    }

    let mut rules = Vec::new();
    for [a, b] in orderings {
        let bindings = [("1", a), ("2", b)];
        let pattern = AutoPattern::parse_inner(&substitute_params(&template.pattern, &bindings));
        let mut postings = Vec::new();
        for tp in &template.postings {
            let account = resolve_lookup_calls(
                &substitute_params(&tp.account, &bindings),
                defines,
                file,
                line,
            )?;
            postings.push(AutoPosting {
                account,
                multiplier: tp.multiplier,
                is_virtual: tp.is_virtual,
                balanced: tp.balanced,
            });
        }
        rules.push(AutoRule { pattern, postings, condition: template.condition.clone() });
    }
    Ok(rules)
}

/// Replace each positional placeholder `$n` with its bound value. `$segment`
/// (the match wildcard) and any other `$word` are left alone — a numbered
/// `$1`/`$2` is never a substring of `$segment`, so the replace can't touch it.
fn substitute_params(s: &str, bindings: &[(&str, &str)]) -> String {
    let mut out = s.to_string();
    for (name, value) in bindings {
        out = out.replace(&format!("${name}"), value);
    }
    out
}

/// The positional placeholders referenced as `$n` (a `$` immediately followed
/// by digits) in `text`, in order — used to check every reference is a declared
/// position. `$segment` and other `$word` tokens are ignored (not `$<digit>`).
fn param_refs(text: &str) -> Vec<&str> {
    let mut refs = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > start {
                refs.push(&text[start..j]);
                i = j;
                continue;
            }
        }
        i += 1;
    }
    refs
}

/// Resolve every `table(key)` call in `account` against the `define` tables,
/// leftmost first, until none remain. Only *declared* table names are matched,
/// so an incidental parenthesised fragment is left alone; an unknown *key* for
/// a known table is an error — a typo in an instantiation pair should surface.
fn resolve_lookup_calls(
    account: &str,
    defines: &HashMap<String, HashMap<String, String>>,
    file: &Arc<str>,
    line: usize,
) -> Result<String, ResolveError> {
    let mut result = account.to_string();
    loop {
        // Leftmost `table(` across all defined tables.
        let mut hit: Option<(usize, String)> = None;
        for tname in defines.keys() {
            if let Some(start) = result.find(&format!("{tname}(")) {
                match &hit {
                    Some((s, _)) if *s <= start => {}
                    _ => hit = Some((start, tname.clone())),
                }
            }
        }
        let Some((start, tname)) = hit else { break };
        let after = start + tname.len() + 1; // past `table(`
        let rel_close = result[after..].find(')').ok_or_else(|| {
            ResolveError::new(file.clone(), line, format!("unclosed `(` in `{tname}(…)` lookup"))
        })?;
        let close = after + rel_close;
        let key = result[after..close].trim().to_string();
        let value = defines
            .get(&tname)
            .and_then(|t| t.get(&key))
            .cloned()
            .ok_or_else(|| {
                ResolveError::new(file.clone(), line, format!("`{tname}` has no entry for `{key}`"))
            })?;
        result = format!("{}{}{}", &result[..start], value, &result[close + 1..]);
    }
    Ok(result)
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
    /// `define NAME` lookup tables: name → (key → value).
    defines: HashMap<String, HashMap<String, String>>,
    /// `= NAME :: /pattern/` auto-rule templates, by name.
    templates: HashMap<String, Template>,
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
    // `define NAME` lookup tables and `= NAME :: /pattern/` templates, both
    // gathered here so an instantiation can reference either regardless of
    // source order.
    let mut defines: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut templates: HashMap<String, Template> = HashMap::new();

    for e in entries {
        match &e.value {
            Entry::Commodity { symbol, aliases: list, precision, .. } => {
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
            Entry::Define { name, entries: kvs } => {
                let mut table: HashMap<String, String> = HashMap::new();
                for (k, v) in kvs {
                    if table.insert(k.clone(), v.clone()).is_some() {
                        return Err(ResolveError::new(
                            e.file.clone(),
                            e.line,
                            format!("define `{name}` has a duplicate key `{k}`"),
                        ));
                    }
                }
                if defines.insert(name.clone(), table).is_some() {
                    return Err(ResolveError::new(
                        e.file.clone(),
                        e.line,
                        format!("define `{name}` is declared more than once"),
                    ));
                }
            }
            Entry::AutoTemplate { name, pattern, postings, condition } => {
                if postings.is_empty() {
                    return Err(ResolveError::new(
                        e.file.clone(),
                        e.line,
                        format!("auto-rule template `{name}` has no postings"),
                    ));
                }
                // Each balance pool (real, balanced-virtual `[...]`) must sum to
                // zero; `(...)` unbalanced postings are exempt — validate once.
                if let Err((pool, sum)) = check_multiplier_balance(postings) {
                    return Err(ResolveError::new(
                        e.file.clone(),
                        e.line,
                        format!("template `{name}` {pool} multipliers must sum to zero, got {sum}"),
                    ));
                }
                // Only positional `$1` / `$2` are valid — catch `$3`, a name,
                // or a typo.
                for text in std::iter::once(pattern.as_str())
                    .chain(postings.iter().map(|p| p.account.as_str()))
                {
                    for r in param_refs(text) {
                        if r != "1" && r != "2" {
                            return Err(ResolveError::new(
                                e.file.clone(),
                                e.line,
                                format!(
                                    "template `{name}` uses `${r}` — only positional `$1` and `$2` are valid"
                                ),
                            ));
                        }
                    }
                }
                let template = Template {
                    pattern: pattern.clone(),
                    postings: postings.clone(),
                    condition: condition.clone(),
                };
                if templates.insert(name.clone(), template).is_some() {
                    return Err(ResolveError::new(
                        e.file.clone(),
                        e.line,
                        format!("auto-rule template `{name}` is declared more than once"),
                    ));
                }
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
        defines,
        templates,
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
    fn parity_emits_synthetic_one_to_one_price_without_folding() {
        // `commodity USDC / parity $` values USDC 1:1 as $ but keeps USDC's
        // own symbol (unlike alias, which folds). A held-USDC posting stays
        // USDC, and a synthetic USDC→$ price of 1 is emitted.
        let src = "commodity $\n    alias USD\ncommodity USDC\n    parity $\n\
                   2024-06-15 * x\n    assets:kraken  USDC5\n    equity  USDC-5\n";
        let out = resolve(parsed(src)).unwrap();
        // Posting keeps USDC (not folded to $).
        let amt = out.transactions[0].value.postings[0].value.amount.as_ref().unwrap();
        assert_eq!(amt.commodity, "USDC");
        // A synthetic 1:1 USDC→$ price exists.
        let p = out.prices.iter().find(|p| &*p.value.base == "USDC").expect("parity price");
        assert_eq!(&*p.value.quote, "$");
        assert_eq!(p.value.rate, Decimal::from(1));
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

    const TEMPLATE_SRC: &str = "define long\n\tfoo = foo-long\n\tbar = bar-long\n\
        = mirror :: /^x:$1:$segment:$2:$segment$/\n\
        \t[$1:z:long($2)]  -1\n\t[$2:z:long($1)]  1\n";

    #[test]
    fn instantiation_expands_both_directions_with_lookup() {
        let out = resolve(parsed(&format!("{TEMPLATE_SRC}= mirror foo bar\n"))).unwrap();
        // One rule per direction, in order: (foo→bar), then (bar→foo).
        assert_eq!(out.auto_rules.len(), 2);

        let fwd = &out.auto_rules[0];
        let accts: Vec<&str> = fwd.postings.iter().map(|p| p.account.as_str()).collect();
        assert_eq!(accts, vec!["foo:z:bar-long", "bar:z:foo-long"]);
        assert!(fwd.pattern.matches("x:foo:acct-a:bar:acct-b"));
        assert!(!fwd.pattern.matches("x:bar:acct-b:foo:acct-a"));

        let rev = &out.auto_rules[1];
        let accts: Vec<&str> = rev.postings.iter().map(|p| p.account.as_str()).collect();
        assert_eq!(accts, vec!["bar:z:foo-long", "foo:z:bar-long"]);
        assert!(rev.pattern.matches("x:bar:acct-b:foo:acct-a"));
    }

    #[test]
    fn unlisted_pair_and_removed_instance_emit_no_rules() {
        // No instantiation at all → no auto-rules, even with a template present.
        let out = resolve(parsed(TEMPLATE_SRC)).unwrap();
        assert!(out.auto_rules.is_empty());
    }

    #[test]
    fn instantiation_errors_surface() {
        let bad = |extra: &str| resolve(parsed(&format!("{TEMPLATE_SRC}{extra}"))).is_err();
        assert!(bad("= nope foo bar\n"), "unknown template name");
        assert!(bad("= mirror foo baz\n"), "unknown lookup key `baz`");
        assert!(bad("= mirror foo\n"), "too few args");
        assert!(bad("= mirror foo bar baz\n"), "too many args");
    }

    #[test]
    fn template_multipliers_must_sum_to_zero() {
        let src = "= mirror :: /^x:$1:$segment:$2:$segment$/\n\
                   \t[$1:z:$2]  -1\n\t[$2:z:$1]  2\n";
        assert!(resolve(parsed(src)).is_err());
    }

    #[test]
    fn out_of_range_placeholder_is_rejected() {
        // Only `$1` and `$2` are valid; `$3` has no argument.
        let src = "= mirror :: /^x:$1:$segment:$3:$segment$/\n\
                   \t[$1:z:$2]  -1\n\t[$2:z:$1]  1\n";
        assert!(resolve(parsed(src)).is_err());
    }

    #[test]
    fn unbalanced_virtual_posting_and_amount_clause_pass() {
        // A lone `(...)` unbalanced posting is allowed (no `[...]` pool to
        // balance); the `amount > 0` clause carries onto both concrete rules.
        let src = "= mirror :: /^x:$1-$segment:$2-$segment$/ amount > 0\n\
                   \t($1:z:$2)  1\n\
                   = mirror foo bar\n";
        let out = resolve(parsed(src)).unwrap();
        assert_eq!(out.auto_rules.len(), 2);
        for rule in &out.auto_rules {
            assert!(rule.condition.is_some());
            assert_eq!(rule.postings.len(), 1);
            assert!(rule.postings[0].is_virtual && !rule.postings[0].balanced);
        }
    }
}
