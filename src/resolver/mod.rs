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
use crate::parser::entry::{
    AmountExpr, AutoPattern, AutoPosting, AutoRule, Cadence, Entry, TemplateRule,
};
use crate::parser::located::Located;
use crate::parser::posting::{Costs, Posting};
use crate::parser::transaction::{State, Transaction};
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

/// A named auto-rule template from a `= NAME ::` block — its one or more rules,
/// each carrying positional `$1`/`$2` placeholders and `lookup[key]` references
/// in the pattern and accounts; [`expand_instance`] substitutes a pair into
/// every rule to produce concrete `AutoRule`s.
struct Template {
    rules: Vec<TemplateRule>,
}

pub fn resolve(entries: Vec<Located<Entry>>) -> Result<Resolved, ResolveError> {
    let Declarations {
        aliases,
        roles,
        precisions,
        labels,
        labels_balance,
        labels_register,
        lookups,
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
                finalize_tx_postings(&mut tx, &aliases, &roles);
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
                // Injected postings must keep the transaction balanced: each pool
                // (real, balanced-virtual `[...]`) has at most one bare leg that
                // fills to balance it; `(...)` postings are exempt, like
                // everywhere in acc.
                if let Err(msg) = check_injected_balance(&rule.postings) {
                    return Err(ResolveError::new(file.clone(), line, format!("auto-rule: {msg}")));
                }
                // Expand `:=` bindings and resolve value lookups; no `$1`/`$2`
                // params (an anonymous rule has no arguments — a stray `$n`
                // surfaces here as an unbound-parameter error).
                let postings =
                    resolve_postings(&[], &rule.bindings, &rule.postings, &lookups, &file, line)?;
                auto_rules.push(AutoRule {
                    pattern: rule.pattern,
                    postings,
                    condition: rule.condition,
                    bindings: Vec::new(),
                });
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
                // Instantiate a template into concrete auto-rules — one per rule
                // in the template, with `$1`/`$2` bound to the pair. Templates/
                // lookups were gathered in the first pass, so ordering across
                // files doesn't matter.
                let rules = expand_instance(&name, &args, &templates, &lookups, &file, line)?;
                auto_rules.extend(rules);
            }
            Entry::Periodic { period, cadence, description, postings } => {
                // A `~ PERIOD [cadence]` block expands into real, ordinary
                // transactions — one per occurrence in the year, the written
                // total split across them. Each then gets the same alias / date /
                // role finalization as any transaction, and the booker balances
                // and auto-fills it downstream.
                if postings.len() < 2 {
                    return Err(ResolveError::new(
                        file.clone(),
                        line,
                        format!(
                            "periodic block `~ {period}` must have at least two postings, got {}",
                            postings.len()
                        ),
                    ));
                }
                let occurrences =
                    expand_periodic(&period, cadence, &description, postings, &file, line)?;
                for mut tx in occurrences {
                    finalize_tx_postings(&mut tx, &aliases, &roles);
                    transactions.push(Located { file: file.clone(), line, value: tx });
                }
            }
            // Account/Lookup/AutoTemplate scaffolds and Comment entries carry
            // no data we still need here — the first pass already consumed the
            // lookup tables and templates. Drop them.
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

/// An auto-rule's injected postings must keep the transaction balanced. Each
/// balance pool (real, balanced-virtual `[...]`) may carry at most one bare
/// `Fill` leg (`amount: None`), which balances the pool by absorbing the rest.
/// The amount *expressions* are dynamic — they read the trigger and running
/// balances — so a pool with no `Fill` can't be summed to zero statically; that
/// it nets to zero is enforced when the expanded transaction is booked.
/// Unbalanced-virtual `(...)` postings take part in no balance — exempt — so a
/// bare `(...)` leg has nothing to fill and is rejected. Returns a
/// human-readable reason on failure.
fn check_injected_balance(postings: &[AutoPosting]) -> Result<(), String> {
    if postings.iter().any(|p| p.is_virtual && !p.balanced && p.amount.is_none()) {
        return Err("a bare `(...)` posting has no pool to balance".to_string());
    }
    check_pool(postings, false, "real")?;
    check_pool(postings, true, "balanced-virtual `[...]`")?;
    Ok(())
}

/// Validate one balance pool: at most one bare `Fill` leg (`amount: None`), which
/// fills the pool by absorbing the rest. `balanced_virtual = false` selects the
/// real pool (non-virtual postings), `true` the balanced-virtual `[...]` pool.
/// Whether the pool nets to zero is left to the booker — the amounts are only
/// known at expansion time.
fn check_pool(postings: &[AutoPosting], balanced_virtual: bool, pool: &str) -> Result<(), String> {
    let fills = postings
        .iter()
        .filter(|p| {
            let in_pool = if balanced_virtual { p.is_virtual && p.balanced } else { !p.is_virtual };
            in_pool && p.amount.is_none()
        })
        .count();
    if fills > 1 {
        return Err(format!("more than one bare posting in the {pool} pool"));
    }
    Ok(())
}

/// Expand a `= NAME a b …` instantiation into concrete `AutoRule`s — one per
/// rule in template `NAME`, with `$1`/`$2`/… bound to the args in the pattern,
/// the posting accounts, and the amount expressions. The rules are independent
/// (a second transfer direction is just another rule in the template), so this
/// is a straight per-rule map; any `table[key]` lookup in an account is resolved
/// against the lookup tables.
fn expand_instance(
    name: &str,
    args: &[String],
    templates: &HashMap<String, Template>,
    lookups: &HashMap<String, HashMap<String, String>>,
    file: &Arc<str>,
    line: usize,
) -> Result<Vec<AutoRule>, ResolveError> {
    let template = templates.get(name).ok_or_else(|| {
        ResolveError::new(file.clone(), line, format!("no auto-rule template named `{name}`"))
    })?;
    // An instance supplies one arg per positional `$n` the template references
    // (the highest index used). `$1`/`$2` are typically the entity pair; a later
    // `$3` is a number (e.g. a share-capital cap) read in an amount expression.
    let arity = template_arity(&template.rules);
    if args.len() != arity {
        return Err(ResolveError::new(
            file.clone(),
            line,
            format!(
                "`= {name}` takes {arity} argument{}, got {}",
                if arity == 1 { "" } else { "s" },
                args.len()
            ),
        ));
    }
    let bindings: Vec<(usize, &str)> =
        args.iter().enumerate().map(|(i, a)| (i + 1, a.as_str())).collect();
    template
        .rules
        .iter()
        .map(|rule| build_instance_rule(rule, &bindings, lookups, file, line))
        .collect()
}

/// The number of positional args a template needs: the highest `$n` referenced
/// across every rule's pattern, posting account, and amount expression. A
/// template with no `$n` has arity 0.
fn template_arity(rules: &[TemplateRule]) -> usize {
    let mut params: Vec<usize> = Vec::new();
    for rule in rules {
        params.extend(param_refs(&rule.pattern).iter().filter_map(|r| r.parse::<usize>().ok()));
        for (_, expr) in &rule.bindings {
            collect_expr_params(expr, &mut params);
        }
        for tp in &rule.postings {
            params
                .extend(param_refs(&tp.account).iter().filter_map(|r| r.parse::<usize>().ok()));
            if let Some(expr) = &tp.amount {
                collect_expr_params(expr, &mut params);
            }
        }
    }
    params.into_iter().max().unwrap_or(0)
}

/// Collect every positional parameter index `$n` referenced in an amount
/// expression — the `Param` atoms plus any `$n` inside a `total(ACCOUNT)` or a
/// `table[key]` lookup. `Var` references are ignored: the binding they name is
/// walked separately, so its own `$n` are already counted.
fn collect_expr_params(expr: &AmountExpr, out: &mut Vec<usize>) {
    match expr {
        AmountExpr::Param(n) => out.push(*n),
        AmountExpr::Total(s) | AmountExpr::Lookup(s) => {
            out.extend(param_refs(s).iter().filter_map(|r| r.parse::<usize>().ok()));
        }
        AmountExpr::Amount | AmountExpr::Literal(_) | AmountExpr::Var(_) => {}
        AmountExpr::Neg(a) => collect_expr_params(a, out),
        AmountExpr::Min(a, b)
        | AmountExpr::Max(a, b)
        | AmountExpr::Add(a, b)
        | AmountExpr::Sub(a, b)
        | AmountExpr::Mul(a, b) => {
            collect_expr_params(a, out);
            collect_expr_params(b, out);
        }
    }
}

/// Build one concrete `AutoRule` from a template rule: substitute the `$1`/`$2`/…
/// `bindings` into the pattern, then resolve the rule's `:=` bindings and every
/// posting (account + amount) via [`resolve_postings`].
fn build_instance_rule(
    rule: &TemplateRule,
    bindings: &[(usize, &str)],
    lookups: &HashMap<String, HashMap<String, String>>,
    file: &Arc<str>,
    line: usize,
) -> Result<AutoRule, ResolveError> {
    let pattern = AutoPattern::parse_inner(&substitute_params(&rule.pattern, bindings));
    let postings = resolve_postings(bindings, &rule.bindings, &rule.postings, lookups, file, line)?;
    Ok(AutoRule { pattern, postings, condition: rule.condition.clone(), bindings: Vec::new() })
}

/// Resolve a rule's postings for one concrete instance: expand the `:=`
/// bindings (`\def`-style, in declaration order) into every posting, then
/// resolve each posting's account (params + `table[key]` lookups) and amount
/// expression (params, value lookups, `total(…)` account lookups). `param_bindings`
/// is empty for an anonymous rule.
fn resolve_postings(
    param_bindings: &[(usize, &str)],
    let_bindings: &[(String, AmountExpr)],
    postings: &[AutoPosting],
    lookups: &HashMap<String, HashMap<String, String>>,
    file: &Arc<str>,
    line: usize,
) -> Result<Vec<AutoPosting>, ResolveError> {
    // Resolve `:=` bindings among themselves first, in order — each may
    // reference an earlier one.
    let mut vars: HashMap<String, AmountExpr> = HashMap::new();
    for (name, expr) in let_bindings {
        if vars.contains_key(name) {
            return Err(ResolveError::new(
                file.clone(),
                line,
                format!("binding `{name}` is defined more than once"),
            ));
        }
        let resolved = substitute_vars(expr, &vars, file, line)?;
        vars.insert(name.clone(), resolved);
    }
    let mut out = Vec::with_capacity(postings.len());
    for tp in postings {
        let account =
            resolve_lookup_calls(&substitute_params(&tp.account, param_bindings), lookups, file, line)?;
        let amount = match &tp.amount {
            Some(e) => {
                let expanded = substitute_vars(e, &vars, file, line)?;
                Some(resolve_expr(&expanded, param_bindings, lookups, file, line)?)
            }
            None => None,
        };
        out.push(AutoPosting { account, amount, is_virtual: tp.is_virtual, balanced: tp.balanced });
    }
    Ok(out)
}

/// Replace every `Var` (`:=` binding reference) in an amount expression with the
/// binding's already-resolved expression — `\def`-style substitution. An unknown
/// name is an error.
fn substitute_vars(
    expr: &AmountExpr,
    vars: &HashMap<String, AmountExpr>,
    file: &Arc<str>,
    line: usize,
) -> Result<AmountExpr, ResolveError> {
    let recur = |e: &AmountExpr| substitute_vars(e, vars, file, line);
    Ok(match expr {
        AmountExpr::Var(name) => vars.get(name).cloned().ok_or_else(|| {
            ResolveError::new(
                file.clone(),
                line,
                format!("unknown binding `{name}` — define it with `{name} := …` above"),
            )
        })?,
        AmountExpr::Amount
        | AmountExpr::Literal(_)
        | AmountExpr::Param(_)
        | AmountExpr::Total(_)
        | AmountExpr::Lookup(_) => expr.clone(),
        AmountExpr::Neg(a) => AmountExpr::Neg(Box::new(recur(a)?)),
        AmountExpr::Min(a, b) => AmountExpr::Min(Box::new(recur(a)?), Box::new(recur(b)?)),
        AmountExpr::Max(a, b) => AmountExpr::Max(Box::new(recur(a)?), Box::new(recur(b)?)),
        AmountExpr::Add(a, b) => AmountExpr::Add(Box::new(recur(a)?), Box::new(recur(b)?)),
        AmountExpr::Sub(a, b) => AmountExpr::Sub(Box::new(recur(a)?), Box::new(recur(b)?)),
        AmountExpr::Mul(a, b) => AmountExpr::Mul(Box::new(recur(a)?), Box::new(recur(b)?)),
    })
}

/// Look up a `table[key]` reference as a *value* — returns the raw string, or
/// `None` if the table or key is undeclared. Unlike [`resolve_lookup_calls`]
/// (account context, a missing key is an error) a value lookup is lenient: the
/// caller defaults a miss to `0`.
fn lookup_value(text: &str, lookups: &HashMap<String, HashMap<String, String>>) -> Option<String> {
    let open = text.find('[')?;
    let close = text.rfind(']')?;
    lookups.get(&text[..open])?.get(text[open + 1..close].trim()).cloned()
}

/// Resolve a template amount expression for one instance: rewrite each `$n`
/// [`Param`](AmountExpr::Param) to a [`Literal`](AmountExpr::Literal) from the
/// bound argument, and substitute `$1`/`$2` / `table[key]` inside every
/// `total(ACCOUNT)`. `$self`, `$account` and the date placeholders survive —
/// they resolve per posting at expansion time.
fn resolve_expr(
    expr: &AmountExpr,
    bindings: &[(usize, &str)],
    lookups: &HashMap<String, HashMap<String, String>>,
    file: &Arc<str>,
    line: usize,
) -> Result<AmountExpr, ResolveError> {
    Ok(match expr {
        AmountExpr::Amount => AmountExpr::Amount,
        AmountExpr::Literal(d) => AmountExpr::Literal(*d),
        AmountExpr::Param(n) => {
            let value = bindings.iter().find(|(k, _)| k == n).map(|(_, v)| *v).ok_or_else(|| {
                ResolveError::new(file.clone(), line, format!("template parameter `${n}` is not bound"))
            })?;
            let d = Decimal::parse(value).map_err(|e| {
                ResolveError::new(
                    file.clone(),
                    line,
                    format!("template parameter `${n}` = `{value}` is not a number: {e}"),
                )
            })?;
            AmountExpr::Literal(d)
        }
        // A `:=` binding reference — already substituted away in
        // `resolve_postings`; reaching here means a stray one.
        AmountExpr::Var(name) => {
            return Err(ResolveError::new(file.clone(), line, format!("unresolved binding `{name}`")));
        }
        // `table[key]` as a value — resolve to a literal (missing table/key → 0).
        AmountExpr::Lookup(text) => {
            let resolved = substitute_params(text, bindings);
            let d = match lookup_value(&resolved, lookups) {
                Some(v) => Decimal::parse(&v).map_err(|e| {
                    ResolveError::new(
                        file.clone(),
                        line,
                        format!("lookup `{resolved}` = `{v}` is not a number: {e}"),
                    )
                })?,
                None => Decimal::zero(),
            };
            AmountExpr::Literal(d)
        }
        AmountExpr::Total(s) => AmountExpr::Total(resolve_lookup_calls(
            &substitute_params(s, bindings),
            lookups,
            file,
            line,
        )?),
        AmountExpr::Neg(a) => {
            AmountExpr::Neg(Box::new(resolve_expr(a, bindings, lookups, file, line)?))
        }
        AmountExpr::Min(a, b) => AmountExpr::Min(
            Box::new(resolve_expr(a, bindings, lookups, file, line)?),
            Box::new(resolve_expr(b, bindings, lookups, file, line)?),
        ),
        AmountExpr::Max(a, b) => AmountExpr::Max(
            Box::new(resolve_expr(a, bindings, lookups, file, line)?),
            Box::new(resolve_expr(b, bindings, lookups, file, line)?),
        ),
        AmountExpr::Add(a, b) => AmountExpr::Add(
            Box::new(resolve_expr(a, bindings, lookups, file, line)?),
            Box::new(resolve_expr(b, bindings, lookups, file, line)?),
        ),
        AmountExpr::Sub(a, b) => AmountExpr::Sub(
            Box::new(resolve_expr(a, bindings, lookups, file, line)?),
            Box::new(resolve_expr(b, bindings, lookups, file, line)?),
        ),
        AmountExpr::Mul(a, b) => AmountExpr::Mul(
            Box::new(resolve_expr(a, bindings, lookups, file, line)?),
            Box::new(resolve_expr(b, bindings, lookups, file, line)?),
        ),
    })
}

/// Replace each positional placeholder `$n` with its bound value. `$segment`
/// (the match wildcard) and any other `$word` are left alone — a numbered
/// `$1`/`$2` is never a substring of `$segment`, so the replace can't touch it.
fn substitute_params(s: &str, bindings: &[(usize, &str)]) -> String {
    let mut out = s.to_string();
    for (n, value) in bindings {
        out = out.replace(&format!("${n}"), value);
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

/// Resolve every `table[key]` reference in `account` against the lookup tables,
/// leftmost first, until none remain. Only *declared* table names are matched,
/// so an incidental bracketed fragment is left alone; an unknown *key* for a
/// known table is an error — a typo in an instantiation pair should surface.
fn resolve_lookup_calls(
    account: &str,
    lookups: &HashMap<String, HashMap<String, String>>,
    file: &Arc<str>,
    line: usize,
) -> Result<String, ResolveError> {
    let mut result = account.to_string();
    loop {
        // Leftmost `table[` across all declared lookup tables.
        let mut hit: Option<(usize, String)> = None;
        for tname in lookups.keys() {
            if let Some(start) = result.find(&format!("{tname}[")) {
                match &hit {
                    Some((s, _)) if *s <= start => {}
                    _ => hit = Some((start, tname.clone())),
                }
            }
        }
        let Some((start, tname)) = hit else { break };
        let after = start + tname.len() + 1; // past `table[`
        let rel_close = result[after..].find(']').ok_or_else(|| {
            ResolveError::new(file.clone(), line, format!("unclosed `[` in `{tname}[…]` lookup"))
        })?;
        let close = after + rel_close;
        let key = result[after..close].trim().to_string();
        let value = lookups
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
    /// Lookup tables from `= NAME[key] :: value` entries: table → (key → value).
    lookups: HashMap<String, HashMap<String, String>>,
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
    // Lookup tables (`= NAME[key] :: value`) and `= NAME :: /pattern/`
    // templates, both gathered here so an instantiation can reference either
    // regardless of source order.
    let mut lookups: HashMap<String, HashMap<String, String>> = HashMap::new();
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
            Entry::Lookup { table, key, value } => {
                // Each line is one entry; entries sharing a table name merge
                // here. A duplicate key within one table is a conflict.
                if lookups
                    .entry(table.clone())
                    .or_default()
                    .insert(key.clone(), value.clone())
                    .is_some()
                {
                    return Err(ResolveError::new(
                        e.file.clone(),
                        e.line,
                        format!("lookup table `{table}` has a duplicate key `{key}`"),
                    ));
                }
            }
            Entry::AutoTemplate { name, rules } => {
                if rules.is_empty() {
                    return Err(ResolveError::new(
                        e.file.clone(),
                        e.line,
                        format!("auto-rule template `{name}` has no rules (no `/pattern/` blocks)"),
                    ));
                }
                for rule in rules {
                    if rule.postings.is_empty() {
                        return Err(ResolveError::new(
                            e.file.clone(),
                            e.line,
                            format!("template `{name}`: a `/pattern/` rule has no postings"),
                        ));
                    }
                    // Each balance pool needs at most one bare leg to fill it;
                    // `(...)` postings are exempt. The pool nets to zero at
                    // expansion time (the amounts are dynamic), checked by the
                    // booker. Positional `$n` are validated per instance in
                    // `expand_instance`, once the argument count is known.
                    if let Err(msg) = check_injected_balance(&rule.postings) {
                        return Err(ResolveError::new(
                            e.file.clone(),
                            e.line,
                            format!("template `{name}`: {msg}"),
                        ));
                    }
                }
                let template = Template { rules: rules.clone() };
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
        lookups,
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

/// The per-posting resolution every transaction gets: commodity aliases,
/// date-variable substitution in the account (`$year`/`$month`/`$day` → the
/// transaction's own date parts), then `$role:slot` account resolution.
/// Shared by hand-written transactions and expanded `~` periodic ones.
fn finalize_tx_postings(
    tx: &mut Transaction,
    aliases: &HashMap<String, String>,
    roles: &HashMap<String, String>,
) {
    let date_str = tx.date.to_string();
    for lp in &mut tx.postings {
        apply_to_posting(&mut lp.value, aliases);
        substitute_date_vars(&mut lp.value.account, &date_str);
        if let Some(name) = resolve_role_account(&lp.value.account, roles) {
            lp.value.account = name;
        }
    }
}

/// Replace `$year` / `$month` / `$day` in an account with the fields of
/// `date_str` (a `YYYY-MM-DD` string), exactly as they appear (zero-padded).
/// A plain textual replace; accounts without `$` are skipped. Lets a posting
/// derive a dated sub-account from its own transaction date
/// (`assets:budget:$year` on a 2021 entry → `assets:budget:2021`). Shared with
/// the expander, which fills the same vars in auto-rule-injected accounts using
/// the triggering transaction's date.
pub(crate) fn substitute_date_vars(account: &mut String, date_str: &str) {
    // Fast path: most accounts carry no `$` at all. `str::replace` always
    // allocates a fresh String even when the pattern is absent, so skip it.
    if !account.contains('$') {
        return;
    }
    // A `$` alone doesn't mean a date variable — `$role:slot` accounts also
    // have one, and a date account may carry only some of the three. Guard each
    // replace so it allocates only for the variables actually present.
    // `date_str` is always `YYYY-MM-DD` (from Date::to_string), ASCII, so the
    // byte slices are exactly the year / month / day fields.
    if account.contains("$year") {
        *account = account.replace("$year", &date_str[0..4]);
    }
    if account.contains("$month") {
        *account = account.replace("$month", &date_str[5..7]);
    }
    if account.contains("$day") {
        *account = account.replace("$day", &date_str[8..10]);
    }
}

/// Expand a `~ PERIOD` block into a real transaction. Only a bare year is
/// supported: the transaction is dated `YYYY-01-01`. Its postings are ordinary
/// (the bare last one auto-fills in the booker); any `$year`/`$month`/`$day` in
/// their accounts is filled by [`finalize_tx_postings`] like any transaction's.
fn expand_periodic(
    period: &str,
    cadence: Cadence,
    description: &str,
    postings: Vec<Located<Posting>>,
    file: &Arc<str>,
    line: usize,
) -> Result<Vec<Transaction>, ResolveError> {
    let year: i32 = period.parse().map_err(|_| {
        ResolveError::new(
            file.clone(),
            line,
            format!("periodic `~ {period}` — only a bare year `YYYY` is supported"),
        )
    })?;
    let jan1 = Date::parse(&format!("{year:04}-01-01"))
        .map_err(|e| ResolveError::new(file.clone(), line, format!("periodic `~ {period}`: {e}")))?;
    let dates = occurrence_dates(jan1, cadence);
    let n = dates.len();

    let txs = dates
        .into_iter()
        .enumerate()
        .map(|(i, date)| {
            // Each occurrence clones the source postings and takes its slice of
            // every explicit amount; a bare leg stays bare and auto-fills.
            let occ_postings = postings
                .iter()
                .map(|lp| {
                    let mut p = lp.value.clone();
                    if let Some(amount) = &mut p.amount {
                        amount.value = split_amount(amount.value, amount.decimals, i, n);
                    }
                    Located { file: lp.file.clone(), line: lp.line, value: p }
                })
                .collect();
            Transaction {
                date,
                state: State::Uncleared,
                code: None,
                description: description.to_string(),
                postings: occ_postings,
                comments: Vec::new(),
            }
        })
        .collect();
    Ok(txs)
}

/// The occurrence dates of a `~ YYYY` block within the year, under its cadence:
/// one at `YYYY-01-01` for yearly, the start of each month for monthly, each day
/// for daily.
fn occurrence_dates(jan1: Date, cadence: Cadence) -> Vec<Date> {
    match cadence {
        Cadence::Yearly => vec![jan1],
        Cadence::Monthly => step_dates(jan1, Date::next_month_start),
        Cadence::Daily => step_dates(jan1, Date::next_day),
    }
}

/// Walk from `jan1` up to (excluding) the next year's start, stepping with
/// `step`, collecting each occurrence date within the year.
fn step_dates(jan1: Date, step: fn(Date) -> Date) -> Vec<Date> {
    let end = jan1.next_year_start().days();
    let mut out = Vec::new();
    let mut d = jan1;
    while d.days() < end {
        out.push(d);
        d = step(d);
    }
    out
}

/// The `i`-th of `n` slices of `total`, at `decimals` display precision: an even
/// split rounded to `decimals`, with the last slice taking the remainder so the
/// `n` slices sum back to exactly `total`.
fn split_amount(total: Decimal, decimals: usize, i: usize, n: usize) -> Decimal {
    if n <= 1 {
        return total;
    }
    let per = total.div_rounded(Decimal::from(n as i128)).round(decimals);
    if i + 1 < n {
        per
    } else {
        total - per.mul_rounded(Decimal::from((n - 1) as i128))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn parsed(src: &str) -> Vec<Located<Entry>> {
        parser::parse(src).unwrap()
    }

    fn loc(value: Entry) -> Located<Entry> {
        Located { file: std::sync::Arc::from("<test>"), line: 0, value }
    }

    /// A synthetic commodity entry — as `CONFIG_DIR` TOML would produce it.
    fn commodity(
        symbol: &str,
        aliases: &[&str],
        parities: &[&str],
        precision: Option<usize>,
    ) -> Located<Entry> {
        loc(Entry::Commodity {
            symbol: symbol.to_string(),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            parities: parities.iter().map(|s| s.to_string()).collect(),
            precision,
        })
    }

    /// A synthetic role / label account entry (`slippage gain`, `label X`, …).
    fn role(role: &str, account: &str) -> Located<Entry> {
        loc(Entry::RoleAccount { role: role.to_string(), account: account.to_string() })
    }

    /// Config entries (commodities / roles) followed by a parsed ledger — the
    /// exact order `read_and_parse` builds in production.
    fn with(config: Vec<Located<Entry>>, ledger: &str) -> Vec<Located<Entry>> {
        let mut entries = config;
        entries.extend(parser::parse(ledger).unwrap());
        entries
    }

    #[test]
    fn applies_alias_to_price() {
        let entries = with(vec![commodity("USD", &["$"], &[], None)], "P 2024-06-15 $ EUR 0.92\n");
        let out = resolve(entries).unwrap();
        assert_eq!(out.prices.len(), 1);
        assert_eq!(&*out.prices[0].value.base, "USD");
        assert_eq!(&*out.prices[0].value.quote, "EUR");
    }

    #[test]
    fn parity_emits_synthetic_one_to_one_price_without_folding() {
        // `commodity USDC / parity $` values USDC 1:1 as $ but keeps USDC's
        // own symbol (unlike alias, which folds). A held-USDC posting stays
        // USDC, and a synthetic USDC→$ price of 1 is emitted.
        let entries = with(
            vec![commodity("$", &["USD"], &[], None), commodity("USDC", &[], &["$"], None)],
            "2024-06-15 * x\n    assets:exchange  USDC5\n    equity  USDC-5\n",
        );
        let out = resolve(entries).unwrap();
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
        let entries = with(
            vec![commodity("USD", &["$"], &[], None)],
            "2024-06-15 * X\n    expenses:food   $5\n    assets:cash  $-5\n",
        );
        let out = resolve(entries).unwrap();
        let amt = out.transactions[0].value.postings[0].value.amount.as_ref().unwrap();
        assert_eq!(amt.commodity, "USD");
    }

    #[test]
    fn extracts_slippage_accounts() {
        let out = resolve(vec![
            role("slippage gain", "Equity:SlippageGain"),
            role("slippage loss", "Equity:SlippageLoss"),
        ])
        .unwrap();
        assert_eq!(out.slippage_gain.as_deref(), Some("Equity:SlippageGain"));
        assert_eq!(out.slippage_loss.as_deref(), Some("Equity:SlippageLoss"));
    }

    #[test]
    fn splits_exact_and_segment_labels() {
        let out = resolve(vec![
            role("label liquid", "assets:cash"),
            role("label tag", "$segment:baz"),
        ])
        .unwrap();
        // Exact name and `$segment` pattern both resolve via the base set;
        // the pattern is anchored to the whole account name.
        assert_eq!(out.labels.get("assets:cash"), Some("liquid"));
        assert_eq!(out.labels.get("foo:baz"), Some("tag"));
        assert_eq!(out.labels.get("foo:baz:sub"), None);
    }

    #[test]
    fn view_labels_and_multiple_sub_directives() {
        let out = resolve(vec![
            role("label base", "a:1"),
            role("label-register reg-only", "a:1"),
            role("label-balance bal-only", "a:2"),
        ])
        .unwrap();
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
        let err = resolve(vec![
            commodity("USD", &["$"], &[], None),
            commodity("EUR", &["$"], &[], None),
        ])
        .unwrap_err();
        assert!(err.message.contains("alias"));
        assert!(err.message.contains("$"));
    }

    #[test]
    fn conflicting_slippage_gain_error() {
        let err = resolve(vec![
            role("slippage gain", "Equity:A"),
            role("slippage gain", "Equity:B"),
        ])
        .unwrap_err();
        assert!(err.message.contains("slippage gain"));
    }

    #[test]
    fn resolves_role_account_references() {
        let entries = with(
            vec![
                role("capital gain", "income:cap:market"),
                role("capital loss", "expenses:cap:market"),
                role("cta gain", "income:cap:cta"),
            ],
            "2024-06-15 * sell\n    assets:eth  -6 EUR\n    $capital:gain  2 EUR\n    $capital:loss  2 EUR\n    $cta:gain  2 EUR\n",
        );
        let out = resolve(entries).unwrap();
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
        let entries = with(
            vec![role("capital gain", "income:cap")],
            "2024-06-15 * x\n    a  -2 EUR\n    $unknown:gain  1 EUR\n    $captial:gain  1 EUR\n",
        );
        let out = resolve(entries).unwrap();
        let acct = |i: usize| out.transactions[0].value.postings[i].value.account.as_str();
        assert_eq!(acct(1), "$unknown:gain");
        assert_eq!(acct(2), "$captial:gain");
    }

    #[test]
    fn plain_account_and_commodity_are_dropped() {
        let out = resolve(vec![
            commodity("USD", &[], &[], None),
            loc(Entry::Account("Assets:Bank".to_string())),
        ])
        .unwrap();
        assert!(out.transactions.is_empty());
        assert!(out.prices.is_empty());
        assert!(out.slippage_gain.is_none());
        assert!(out.slippage_loss.is_none());
    }

    const TEMPLATE_SRC: &str = "= long[foo] :: foo-long\n= long[bar] :: bar-long\n\
        = mirror ::\n\
        \t/^x:$1:$segment:$2:$segment$/\n\
        \t\t[$1:z:long[$2]]  -1\n\t\t[$2:z:long[$1]]  1\n";

    #[test]
    fn instantiation_substitutes_the_pair_into_the_rule() {
        let out = resolve(parsed(&format!("{TEMPLATE_SRC}= mirror foo bar\n"))).unwrap();
        // One rule in the template → one concrete rule, `$1`=foo/`$2`=bar.
        assert_eq!(out.auto_rules.len(), 1);
        let rule = &out.auto_rules[0];
        let accts: Vec<&str> = rule.postings.iter().map(|p| p.account.as_str()).collect();
        assert_eq!(accts, vec!["foo:z:bar-long", "bar:z:foo-long"]);
        assert!(rule.pattern.matches("x:foo:acct-a:bar:acct-b"));
        // Only the written direction — the reverse would be a second rule block.
        assert!(!rule.pattern.matches("x:bar:acct-b:foo:acct-a"));
    }

    #[test]
    fn multi_rule_template_instantiates_every_rule() {
        // A two-rule template (forward + mirrored reverse, both written out) →
        // two concrete rules from one instance, `$1`=foo/`$2`=bar in both.
        let src = "= long[foo] :: foo-long\n= long[bar] :: bar-long\n\
                   = both ::\n\
                   \t/^x:$1:$segment:$2:$segment$/\n\t\t[$1:z:long[$2]]  amount\n\t\t[$2:z:long[$1]]\n\
                   \t/^x:$2:$segment:$1:$segment$/\n\t\t[$1:z:long[$2]]  -amount\n\t\t[$2:z:long[$1]]\n\
                   = both foo bar\n";
        let out = resolve(parsed(src)).unwrap();
        assert_eq!(out.auto_rules.len(), 2);
        // Rule 0 matches foo-first, rule 1 bar-first; both book foo:z:bar-long.
        assert!(out.auto_rules[0].pattern.matches("x:foo:a:bar:b"));
        assert!(out.auto_rules[1].pattern.matches("x:bar:b:foo:a"));
        assert_eq!(out.auto_rules[0].postings[0].account, "foo:z:bar-long");
        assert_eq!(out.auto_rules[1].postings[0].account, "foo:z:bar-long");
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
    fn template_pool_rejects_two_fill_legs() {
        // A balance pool may carry at most one bare (Fill) leg — two is
        // ambiguous. (The old "multipliers sum to zero" check is gone: amounts
        // are dynamic expressions now, so imbalance is caught when booked.)
        let src = "= mirror ::\n\t/^x:$1:$segment:$2:$segment$/\n\
                   \t\t[$1:z:$2]\n\t\t[$2:z:$1]\n";
        assert!(resolve(parsed(src)).is_err());
    }

    #[test]
    fn non_numeric_argument_for_amount_param_is_rejected() {
        // `$2` is used as a number in an amount, but the instance passes a
        // non-number for it — surfaced when the expression is resolved.
        let src = "= cap ::\n\t/^x:$1:$segment$/\n\
                   \t\t[$1:z:held]  min(amount, $2)\n\t\t[$1:z:rest]\n\
                   = cap foo notanumber\n";
        assert!(resolve(parsed(src)).is_err());
    }

    #[test]
    fn value_lookup_resolves_and_missing_is_zero() {
        // `cap[$2]` reads the array as a value; a missing key resolves to 0. A
        // `:=` binding is substituted into the posting before resolution.
        let src = "= cap[bar] :: 7\n\
                   = t ::\n\t/^x:$1:$2$/\n\t\tstake := cap[$2]\n\
                   \t\t[$1:held]  stake\n\t\t[$1:rest]\n\
                   = t foo bar\n= t foo other\n";
        let out = resolve(parsed(src)).unwrap();
        assert_eq!(out.auto_rules.len(), 2);
        let held = |rule: &AutoRule| match &rule.postings[0].amount {
            Some(AmountExpr::Literal(d)) => *d,
            other => panic!("expected a resolved literal, got {:?}", other),
        };
        assert_eq!(held(&out.auto_rules[0]), Decimal::from(7)); // cap[bar]
        assert_eq!(held(&out.auto_rules[1]), Decimal::zero()); // cap[other] missing → 0
    }

    #[test]
    fn unbalanced_virtual_posting_and_amount_clause_pass() {
        // A lone `(...)` unbalanced posting is allowed (no `[...]` pool to
        // balance); the `amount > 0` clause carries onto the concrete rule.
        let src = "= mirror ::\n\t/^x:$1-$segment:$2-$segment$/ amount > 0\n\
                   \t\t($1:z:$2)  1\n\
                   = mirror foo bar\n";
        let out = resolve(parsed(src)).unwrap();
        assert_eq!(out.auto_rules.len(), 1);
        for rule in &out.auto_rules {
            assert!(rule.condition.is_some());
            assert_eq!(rule.postings.len(), 1);
            assert!(rule.postings[0].is_virtual && !rule.postings[0].balanced);
        }
    }

    #[test]
    fn periodic_monthly_splits_the_total() {
        let out =
            resolve(parsed("~ 2021 monthly budget\n\tassets:b:$year  $1200.00\n\tin:x\n")).unwrap();
        // 12 monthly transactions, dated the 1st of each month.
        assert_eq!(out.transactions.len(), 12);
        assert_eq!(out.transactions[0].value.date.to_string(), "2021-01-01");
        // Each slice is 1200 / 12 = 100, and the account's `$year` is filled.
        let p = &out.transactions[5].value.postings[0].value;
        assert_eq!(p.account, "assets:b:2021");
        assert_eq!(p.amount.as_ref().unwrap().value, Decimal::from(100));
    }

    #[test]
    fn periodic_split_preserves_the_total_with_remainder() {
        // 100 / 12 does not divide evenly; the 12 slices must still sum to 100
        // (the last occurrence absorbs the rounding remainder).
        let out = resolve(parsed("~ 2021 monthly x\n\ta  $100.00\n\tb\n")).unwrap();
        assert_eq!(out.transactions.len(), 12);
        let sum = out
            .transactions
            .iter()
            .map(|t| t.value.postings[0].value.amount.as_ref().unwrap().value)
            .fold(Decimal::zero(), |acc, v| acc + v);
        assert_eq!(sum, Decimal::from(100));
    }
}
