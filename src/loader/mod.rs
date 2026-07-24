//! Loader phase.
//!
//! Orchestrates the full pipeline end-to-end: reads the input files,
//! runs every earlier phase in order, and assembles a [`Journal`] for
//! downstream report commands.
//!
//! ```text
//! files ─► parser ─► resolver ─┬─► booker  ─┐
//!                              └─► indexer ─┤
//!                                           ▼
//!                                        Journal
//! ```

pub mod error;
pub mod journal;

pub use error::LoadError;
pub use journal::{Journal, LabelView};

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::parser::entry::Entry;
use crate::parser::located::Located;
use crate::parser::posting::{Amount, Posting};
use crate::parser::transaction::Transaction;
use crate::{booker, indexer, parser, resolver};

/// Load one or more journal files and build a complete `Journal`.
///
/// Every phase runs unconditionally, so a journal with balance errors,
/// alias conflicts or failed assertions is rejected here — callers
/// that need to bypass validation (e.g. `print --raw`) should call
/// [`parser::parse`] directly on the file contents and skip `load`.
pub fn load<P>(files: &[P]) -> Result<Journal, LoadError>
where
    P: AsRef<Path> + Sync,
{
    let entries = read_and_parse(files)?;
    finish_load(entries)
}

/// Load the journal, then load only the price-DB pairs the report can use.
///
/// `journal_files` are parsed first to learn the held commodities + the
/// `-X` target; the `price_files` (the `PRICES` star, ~800k
/// directives) are then parsed with a filter that keeps a `P` directive
/// only when both its commodities can take part in a conversion the report
/// needs. Every conversion routes through the `$` hub (`X → $ → target`),
/// so a pair with one un-needed side is dead weight — dropping it skips the
/// expensive per-price work for the bulk of the DB.
pub fn load_selective<P>(
    journal_files: &[P],
    price_files: &[P],
    target: Option<&str>,
) -> Result<Journal, LoadError>
where
    P: AsRef<Path> + Sync,
{
    let journal_entries = read_and_parse(journal_files)?;
    let mut needed = needed_commodities(&journal_entries, target);
    // Widen `needed` with the bridge commodities on each conversion path, so a
    // multi-hop `X → $ → target` survives the both-sides price filter even when
    // the hub (`$`) is neither a posting commodity nor the target.
    if let Some(t) = target {
        add_bridge_commodities(&mut needed, &journal_entries, price_files, t);
    }
    let price_entries = read_and_parse_filtered(price_files, &needed)?;
    // Prices first so journal declarations win on resolution — matches the
    // eager order where the price-dir files precede the user files.
    let mut entries = price_entries;
    entries.extend(journal_entries);
    finish_load(entries)
}

/// Resolve → book → index a parsed entry stream into a `Journal`.
fn finish_load(entries: Vec<Located<Entry>>) -> Result<Journal, LoadError> {
    let resolved = resolver::resolve(entries)?;
    let transactions = booker::book(resolved.transactions)?;
    let prices = indexer::index(resolved.prices);
    let mut precisions = precisions_per_commodity(&transactions);
    // Explicit `precision N` under `commodity` directives wins over
    // whatever the posting amounts happened to contain. Users pin
    // fiat currencies to 2 decimals even if a raw `$13123.12312`
    // exists somewhere in the source.
    for (commodity, p) in resolved.precisions {
        precisions.insert(commodity, p);
    }

    Ok(Journal {
        transactions,
        prices,
        slippage_gain: resolved.slippage_gain,
        slippage_loss: resolved.slippage_loss,
        cta_gain: resolved.cta_gain,
        cta_loss: resolved.cta_loss,
        capital_gain: resolved.capital_gain,
        capital_loss: resolved.capital_loss,
        holding_gain: resolved.holding_gain,
        holding_loss: resolved.holding_loss,
        precisions,
        aliases: resolved.aliases,
        auto_rules: resolved.auto_rules,
        labels: resolved.labels,
        labels_balance: resolved.labels_balance,
        labels_register: resolved.labels_register,
    })
}

/// Commodities a `-X` report can touch: every commodity that appears in a
/// posting (amount, cost, lot cost, assertion) plus the target — each
/// expanded to all its alias spellings, so `$` / `USD` / `USDT` all match
/// the raw symbols the price files are written in.
fn needed_commodities(
    entries: &[Located<Entry>],
    target: Option<&str>,
) -> HashSet<String> {
    use crate::parser::posting::Costs;
    let mut needed: HashSet<String> = HashSet::new();
    let mut alias_pairs: Vec<(String, String)> = Vec::new();
    for located in entries {
        match &located.value {
            Entry::Transaction(tx) => {
                for lp in &tx.postings {
                    let p = &lp.value;
                    if let Some(a) = &p.amount {
                        needed.insert(a.commodity.clone());
                    }
                    if let Some(Costs::Total(a) | Costs::PerUnit(a)) = &p.costs {
                        needed.insert(a.commodity.clone());
                    }
                    if let Some(lc) = &p.lot_cost {
                        needed.insert(lc.amount.commodity.clone());
                    }
                    if let Some(a) = &p.balance_assertion {
                        needed.insert(a.commodity.clone());
                    }
                }
            }
            Entry::Commodity { symbol, aliases, .. } => {
                for a in aliases {
                    alias_pairs.push((symbol.clone(), a.clone()));
                }
            }
            _ => {}
        }
    }
    if let Some(t) = target {
        needed.insert(t.to_string());
    }
    // Pull in every alias form: a `$ / USD / USDT` chain links all three.
    expand_aliases(&mut needed, &alias_pairs);
    needed
}

/// Grow `needed` to a fixpoint over the alias graph: if either side of an
/// alias pair is needed, so is the other.
fn expand_aliases(needed: &mut HashSet<String>, alias_pairs: &[(String, String)]) {
    loop {
        let mut grew = false;
        for (canonical, alias) in alias_pairs {
            if needed.contains(canonical) && needed.insert(alias.clone()) {
                grew = true;
            }
            if needed.contains(alias) && needed.insert(canonical.clone()) {
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
}

/// Add the intermediate ("bridge") commodities that lie on a conversion path
/// from a journal commodity to `target`. Without this, a two-hop route like
/// `XMR → $ → €` dies: `$` is neither a posting commodity nor the target, so
/// the both-sides filter drops `XMR/USDT` and `USD/EUR` alike. The path is
/// found over a graph built from the price DB's PAIR structure only — cheap
/// next to the ~800k dated rates.
fn add_bridge_commodities<P: AsRef<Path>>(
    needed: &mut HashSet<String>,
    journal_entries: &[Located<Entry>],
    price_files: &[P],
    target: &str,
) {
    // Alias map (any spelling → canonical) and the journal's own commodities.
    let mut alias_pairs: Vec<(String, String)> = Vec::new();
    let mut parity_pairs: Vec<(String, String)> = Vec::new();
    let mut sources: HashSet<String> = HashSet::new();
    for located in journal_entries {
        match &located.value {
            Entry::Transaction(tx) => {
                for lp in &tx.postings {
                    if let Some(a) = &lp.value.amount {
                        sources.insert(a.commodity.clone());
                    }
                }
            }
            Entry::Commodity { symbol, aliases, parities, .. } => {
                for a in aliases {
                    alias_pairs.push((symbol.clone(), a.clone()));
                }
                for t in parities {
                    parity_pairs.push((symbol.clone(), t.clone()));
                }
            }
            _ => {}
        }
    }
    let mut canonical: HashMap<String, String> = HashMap::new();
    for (sym, alias) in &alias_pairs {
        canonical.insert(alias.clone(), sym.clone());
    }
    let canon = |c: &str| canonical.get(c).cloned().unwrap_or_else(|| c.to_string());

    // Undirected commodity graph from the price-pair structure, canonicalized.
    let mut adj: HashMap<String, HashSet<String>> = HashMap::new();
    for (base, quote) in price_pair_graph(price_files) {
        let (b, q) = (canon(&base), canon(&quote));
        if b == q {
            continue;
        }
        adj.entry(b.clone()).or_default().insert(q.clone());
        adj.entry(q).or_default().insert(b);
    }
    // Fixed parity edges (`commodity USDC / parity $`) live in the config,
    // not the price DB — add them so a route like USDC → $ → € is found and
    // the hub `$` is pulled into `needed`, keeping `$→€` past the filter.
    for (sym, t) in &parity_pairs {
        let (a, b) = (canon(sym), canon(t));
        if a == b {
            continue;
        }
        adj.entry(a.clone()).or_default().insert(b.clone());
        adj.entry(b).or_default().insert(a);
    }

    let tgt = canon(target);
    let mut on_path: HashSet<String> = HashSet::new();
    for src in &sources {
        let s = canon(src);
        if s == tgt {
            continue;
        }
        if let Some(path) = shortest_path(&adj, &s, &tgt) {
            on_path.extend(path);
        }
    }
    for c in on_path {
        needed.insert(c);
    }
    // The graph is canonical; pull the raw alias spellings back in so the
    // filter matches the symbols the price files are actually written in.
    expand_aliases(needed, &alias_pairs);
}

/// Shortest commodity path `src … tgt` (inclusive) over the pair graph, or
/// `None` if the two are not connected.
fn shortest_path(
    adj: &HashMap<String, HashSet<String>>,
    src: &str,
    tgt: &str,
) -> Option<Vec<String>> {
    use std::collections::VecDeque;
    if src == tgt {
        return Some(vec![src.to_string()]);
    }
    let mut prev: HashMap<String, String> = HashMap::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    queue.push_back(src.to_string());
    prev.insert(src.to_string(), src.to_string());
    while let Some(node) = queue.pop_front() {
        let Some(neighbors) = adj.get(&node) else {
            continue;
        };
        for n in neighbors {
            if prev.contains_key(n) {
                continue;
            }
            prev.insert(n.clone(), node.clone());
            if n == tgt {
                let mut path = vec![tgt.to_string()];
                let mut cur = tgt.to_string();
                while cur != *src {
                    cur = prev[&cur].clone();
                    path.push(cur.clone());
                }
                return Some(path);
            }
            queue.push_back(n.clone());
        }
    }
    None
}

/// The distinct commodity PAIRS in the price DB, read cheaply: each file is
/// opened once and scanned only until its pairs stop being new (a single-pair
/// crypto file stops almost at once; the first daily fiat file yields its full
/// quote set; every later fiat file, sharing a base already scanned, stops on
/// line one) — so this touches a few thousand lines, not the ~800k rates.
fn price_pair_graph<P: AsRef<Path>>(files: &[P]) -> Vec<(String, String)> {
    use std::io::{BufRead, BufReader};
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut bases_done: HashSet<String> = HashSet::new();
    for file in files {
        let Ok(f) = std::fs::File::open(file.as_ref()) else {
            continue;
        };
        let mut reader = BufReader::new(f);
        let mut line = String::new();
        let mut base: Option<String> = None;
        let mut stale = 0;
        while stale < 16 {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let Some((b, q)) = parse_price_pair(&line) else {
                continue;
            };
            if base.is_none() {
                base = Some(b.to_string());
                if bases_done.contains(b) {
                    break; // a later daily file for an already-scanned base
                }
            }
            if seen.insert((b.to_string(), q.to_string())) {
                stale = 0;
            } else {
                stale += 1;
            }
        }
        if let Some(b) = base {
            bases_done.insert(b);
        }
    }
    seen.into_iter().collect()
}

/// `P DATE BASE QUOTE RATE` → `(BASE, QUOTE)`; `None` for any other line.
fn parse_price_pair(line: &str) -> Option<(&str, &str)> {
    let mut it = line.split_whitespace();
    if it.next()? != "P" {
        return None;
    }
    it.next()?; // date
    Some((it.next()?, it.next()?))
}

/// Parse price files in parallel, keeping only the `P` directives whose
/// commodities are both in `needed`.
fn read_and_parse_filtered<P>(
    files: &[P],
    needed: &HashSet<String>,
) -> Result<Vec<Located<Entry>>, LoadError>
where
    P: AsRef<Path> + Sync,
{
    use rayon::prelude::*;

    let per_file: Result<Vec<Vec<Located<Entry>>>, LoadError> = files
        .par_iter()
        .map(|file| {
            let path = file.as_ref().display().to_string();
            let source = std::fs::read_to_string(file.as_ref()).map_err(|e| LoadError::Io {
                path: path.clone(),
                source: e,
            })?;
            let file_arc: Arc<str> = Arc::from(path.as_str());
            parser::parse_with_file_filtered(&source, file_arc, needed)
                .map_err(|e| LoadError::Parse { path, source: e })
        })
        .collect();
    Ok(per_file?.into_iter().flatten().collect())
}

/// Walk every posting and record, per commodity, the maximum
/// fractional-digit count the user wrote. Reports render every amount
/// of a commodity with this many decimals so that `$5`, `$5.00` and
/// `$5.0000` in the same journal all print as `$5.0000`.
fn precisions_per_commodity(
    txs: &[Located<Transaction>],
) -> HashMap<String, usize> {
    let mut map: HashMap<String, usize> = HashMap::new();
    for located in txs {
        for p in &located.value.postings {
            visit_posting(&p.value, &mut map);
        }
    }
    map
}

fn visit_posting(p: &Posting, map: &mut HashMap<String, usize>) {
    // Only the posting's real amount contributes to display precision.
    // `@` / `@@` cost annotations and `{…}` lot costs can carry many
    // trailing digits (e.g. `€0.0047169811320755`) that the user never
    // meant to see rendered on a balance line; `= X` assertions are
    // internal checks, not user-facing output.
    if let Some(a) = &p.amount {
        bump(map, a);
    }
}

fn bump(map: &mut HashMap<String, usize>, a: &Amount) {
    let entry = map.entry(a.commodity.clone()).or_insert(0);
    if a.decimals > *entry {
        *entry = a.decimals;
    }
}

/// Read every file and concatenate their parsed entries. Files are
/// read and parsed in parallel via `rayon`; the final `Vec` still
/// preserves the input file order (and source order within each
/// file), because `par_iter().collect()` is order-preserving.
fn read_and_parse<P>(files: &[P]) -> Result<Vec<Located<Entry>>, LoadError>
where
    P: AsRef<Path> + Sync,
{
    use rayon::prelude::*;

    let per_file: Result<Vec<Vec<Located<Entry>>>, LoadError> = files
        .par_iter()
        .map(|file| read_and_parse_one(file.as_ref()))
        .collect();
    Ok(per_file?.into_iter().flatten().collect())
}

fn read_and_parse_one(file: &Path) -> Result<Vec<Located<Entry>>, LoadError> {
    let path = file.display().to_string();
    let source = if path == "-" {
        let mut s = String::new();
        use std::io::Read as _;
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| LoadError::Io {
                path: path.clone(),
                source: e,
            })?;
        s
    } else {
        std::fs::read_to_string(file).map_err(|e| LoadError::Io {
            path: path.clone(),
            source: e,
        })?
    };
    let file_arc: Arc<str> = Arc::from(path.as_str());
    parser::parse_with_file(&source, file_arc).map_err(|e| LoadError::Parse {
        path,
        source: e,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn with_tmp(name: &str, contents: &str, f: impl FnOnce(&Path)) {
        let dir = std::env::temp_dir().join(format!(
            "acc-loader-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{}.ledger", name));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        f(&path);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn loads_a_simple_journal() {
        let src = "2024-06-15 * Coffee\n    expenses:food   5 USD\n    assets:cash\n";
        with_tmp("simple", src, |path| {
            let journal = load(&[path]).unwrap();
            assert_eq!(journal.transactions.len(), 1);
            assert_eq!(
                journal.transactions[0].value.description,
                "Coffee"
            );
        });
    }

    #[test]
    fn loads_prices_into_the_index() {
        let src = "P 2024-06-15 USD EUR 0.92\n";
        with_tmp("prices", src, |path| {
            let journal = load(&[path]).unwrap();
            assert!(journal.prices.find("USD", "EUR", "2024-06-16").is_some());
        });
    }

    #[test]
    fn shortest_path_finds_the_hub() {
        let mut adj: HashMap<String, HashSet<String>> = HashMap::new();
        for (a, b) in [("XMR", "$"), ("$", "€"), ("$", "£")] {
            adj.entry(a.into()).or_default().insert(b.into());
            adj.entry(b.into()).or_default().insert(a.into());
        }
        assert_eq!(shortest_path(&adj, "XMR", "€").unwrap(), vec!["€", "$", "XMR"]);
        assert!(shortest_path(&adj, "XMR", "¥").is_none());
    }

    #[test]
    fn bridge_commodities_pull_in_the_hub() {
        // A journal holding only XMR, valued in € (`-X €`), needs the two-hop
        // route XMR → $ → €. The hub `$` (and its raw spellings USD / USDT) is
        // neither a posting commodity nor the target, so it must be pulled in
        // explicitly or the bridge rates get filtered out.
        let journal = "commodity $\n    alias USD\n    alias USDT\n\
                       commodity €\n    alias EUR\n\
                       2026-07-19 * x\n    a  1 XMR\n    b\n";
        let xmr = "P 2026-07-19 XMR USDT 335.22\n";
        let fiat = "P 2026-07-19 USD EUR 0.874355\n";
        with_tmp("bridge-j", journal, |j| {
            with_tmp("bridge-x", xmr, |x| {
                with_tmp("bridge-e", fiat, |e| {
                    let entries = read_and_parse(&[j]).unwrap();
                    let mut needed = needed_commodities(&entries, Some("€"));
                    assert!(!needed.contains("$"), "hub must not be needed yet");
                    add_bridge_commodities(
                        &mut needed,
                        &entries,
                        &[x.to_path_buf(), e.to_path_buf()],
                        "€",
                    );
                    assert!(needed.contains("$"), "hub $ pulled onto the path");
                    assert!(needed.contains("USDT"), "raw alias pulled in for the filter");
                    assert!(needed.contains("USD"), "raw alias pulled in for the filter");
                });
            });
        });
    }

    #[test]
    fn parity_edge_bridges_usdc_to_the_hub() {
        // A journal holding USDC, valued in € (`-X €`). USDC is not an alias
        // of $ — it is `parity $`, its own commodity — and there is no USDC/…
        // pair in the price DB. The parity edge must feed the bridge graph so
        // the route USDC → $ → € is found and the hub $ pulled into `needed`.
        let journal = "commodity $\n    alias USD\n\
                       commodity USDC\n    parity $\n\
                       commodity €\n    alias EUR\n\
                       2026-07-19 * x\n    a  1 USDC\n    b\n";
        let fiat = "P 2026-07-19 USD EUR 0.874355\n";
        with_tmp("parity-j", journal, |j| {
            with_tmp("parity-e", fiat, |e| {
                let entries = read_and_parse(&[j]).unwrap();
                let mut needed = needed_commodities(&entries, Some("€"));
                assert!(!needed.contains("$"), "hub must not be needed yet");
                add_bridge_commodities(&mut needed, &entries, &[e.to_path_buf()], "€");
                assert!(needed.contains("$"), "hub $ pulled onto the parity path");
                assert!(needed.contains("USD"), "raw alias pulled in for the filter");
            });
        });
    }

    #[test]
    fn extracts_slippage_accounts() {
        let src = "account Equity:SlippageGain\n    slippage gain\naccount Equity:SlippageLoss\n    slippage loss\n";
        with_tmp("slippage", src, |path| {
            let journal = load(&[path]).unwrap();
            assert_eq!(journal.slippage_gain.as_deref(), Some("Equity:SlippageGain"));
            assert_eq!(journal.slippage_loss.as_deref(), Some("Equity:SlippageLoss"));
        });
    }

    #[test]
    fn errors_on_unbalanced_transaction() {
        let src = "2024-06-15 * X\n    a  5 USD\n    b  -3 USD\n";
        with_tmp("unbalanced", src, |path| {
            let err = load(&[path]).unwrap_err();
            match err {
                LoadError::Book(_) => {}
                other => panic!("expected Book error, got {:?}", other),
            }
        });
    }

    #[test]
    fn errors_on_alias_conflict() {
        let src = "commodity USD\n    alias $\ncommodity EUR\n    alias $\n";
        with_tmp("alias_conflict", src, |path| {
            let err = load(&[path]).unwrap_err();
            match err {
                LoadError::Resolve(_) => {}
                other => panic!("expected Resolve error, got {:?}", other),
            }
        });
    }

    #[test]
    fn errors_on_missing_file() {
        let err = load(&[Path::new("/this/does/not/exist.ledger")]).unwrap_err();
        match err {
            LoadError::Io { .. } => {}
            other => panic!("expected Io error, got {:?}", other),
        }
    }

    #[test]
    fn selective_keeps_reachable_pairs_and_drops_noise() {
        // A journal holding ABC bought with $; the price star carries the
        // useful USD/EUR + ABC/USDT pairs plus a noise pair (XYZ/USDT) for a
        // commodity the journal never touches. Eager keeps everything;
        // selective must drop the noise but reach the same valuations.
        let config = "commodity $\n    alias USD\n    alias USDT\ncommodity €\n    alias EUR\n";
        let journal = "2024-01-10 * buy\n    assets:crypto  ABC2 @ $30000\n    assets:bank\n";
        let prices = "P 2024-01-10 USD EUR 0.9\nP 2024-01-10 ABC USDT 30000\nP 2024-01-10 XYZ USDT 2000\n";

        with_tmp("sel_cfg", config, |cfg| {
            with_tmp("sel_jrn", journal, |jrn| {
                with_tmp("sel_prc", prices, |prc| {
                    let eager = load(&[cfg, prc, jrn]).unwrap();
                    let sel = load_selective(&[cfg, jrn], &[prc], Some("€")).unwrap();

                    // Both reach the held commodity's valuation chain. The
                    // index stores resolved alias forms, so the `USD EUR`
                    // price lives under `$`/`€`.
                    assert!(eager.prices.find("ABC", "$", "2024-01-11").is_some());
                    assert!(sel.prices.find("ABC", "$", "2024-01-11").is_some());
                    assert!(sel.prices.find("$", "€", "2024-01-11").is_some());

                    // The noise pair is present eagerly but filtered out
                    // selectively — XYZ never appears in the journal.
                    assert!(eager.prices.find("XYZ", "$", "2024-01-11").is_some());
                    assert!(sel.prices.find("XYZ", "$", "2024-01-11").is_none());
                });
            });
        });
    }

    #[test]
    fn selective_expands_target_aliases() {
        // The `-X` target is given as the canonical `€`, but the price file
        // spells the pair `USD EUR`. Alias expansion of the target must pull
        // `EUR` into the needed set so the pair survives the filter.
        let config = "commodity $\n    alias USD\ncommodity €\n    alias EUR\n";
        let journal = "2024-01-10 * spend\n    expenses:x  $10\n    assets:bank\n";
        let prices = "P 2024-01-10 USD EUR 0.9\n";

        with_tmp("ali_cfg", config, |cfg| {
            with_tmp("ali_jrn", journal, |jrn| {
                with_tmp("ali_prc", prices, |prc| {
                    let sel = load_selective(&[cfg, jrn], &[prc], Some("€")).unwrap();
                    assert!(sel.prices.find("$", "€", "2024-01-11").is_some());
                });
            });
        });
    }

    #[test]
    fn selective_parity_values_usdc_via_hub() {
        // USDC declared `parity $`; the DB has only USD/EUR. `-X €` must reach
        // USDC → € by chaining the synthetic 1:1 parity edge (USDC→$) with the
        // fiat hub ($→€), and the selective loader must keep the $→€ pair.
        let config = "commodity $\n    alias USD\ncommodity USDC\n    parity $\ncommodity €\n    alias EUR\n";
        let journal = "2024-01-10 * in\n    assets:kraken  USDC10\n    equity\n";
        let prices = "P 2024-01-10 USD EUR 0.9\n";
        with_tmp("par_cfg", config, |cfg| {
            with_tmp("par_jrn", journal, |jrn| {
                with_tmp("par_prc", prices, |prc| {
                    let sel = load_selective(&[cfg, jrn], &[prc], Some("€")).unwrap();
                    // USDC → € chains: USDC→$ (parity, ×1) then $→€ (×0.9).
                    assert_eq!(
                        sel.prices.find("USDC", "€", "2024-01-11"),
                        Some(crate::decimal::Decimal::parse("0.9").unwrap())
                    );
                });
            });
        });
    }
}
