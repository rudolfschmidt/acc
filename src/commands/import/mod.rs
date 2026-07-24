//! `import` command — dispatch a per-profile import to its source backend
//! (`fiat` CSV files, a `monero` wallet RPC, or a `bitcoin`/`litecoin` Bitcoin
//! Core-family RPC) and append the new, deduped transactions to a `@cash` file.
//! This module holds the dispatcher plus the vocabulary EVERY source shares:
//! the categorization `Rule` grammar, own↔own `Transit`, the diff preview, and
//! the small IO helpers. Anything used by only the wallet-RPC backends (their
//! tx model and rendering) lives in `crypto_lib.rs`, not here.

mod bitcoin_lib;
mod bitcoin_rpc;
mod crypto_csv;
mod crypto_lib;
mod exchange_lib;
mod fiat_csv;
mod kraken_api;
mod litecoin_rpc;
mod reto_rpc;
mod monero_rpc;
mod render_lib;
mod rpc_lib;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::Error;

pub fn run(csvs: &[String], conf_path: &str, write: bool) -> Result<(), Error> {
    let conf = read(conf_path)?;
    // A `wallet.coin` directive routes to a wallet-RPC backend by coin.
    if let Some(coin) = directive(&conf, "wallet.coin") {
        return match coin.as_str() {
            "monero" => monero_rpc::run(conf_path, write),
            // bitcoind and litecoind speak the identical RPC; each coin has its
            // own thin entry point that forwards to the shared bitcoin_lib.
            "bitcoin" => bitcoin_rpc::run(conf_path, write),
            "litecoin" => litecoin_rpc::run(conf_path, write),
            other => Err(Error::from(format!("import: unknown wallet.coin '{}'", other))),
        };
    }
    // An `exchange` directive routes to an exchange backend; kraken pulls its
    // multi-asset ledger live from the REST API.
    if let Some(exchange) = directive(&conf, "exchange") {
        return match exchange.as_str() {
            "kraken" => kraken_api::run(conf_path, write),
            // crypto.com exports statement CSVs — pass one or more files or a directory.
            "crypto" => {
                if csvs.is_empty() {
                    return Err(Error::from("import: crypto reads statement CSVs — pass one or more files or a directory"));
                }
                crypto_csv::run(csvs, conf_path, write)
            }
            other => Err(Error::from(format!("import: unknown exchange '{}'", other))),
        };
    }
    let csv_path = csvs.first().ok_or_else(|| {
        Error::from("import: this profile reads a CSV — pass the CSV file as the argument")
    })?;
    fiat_csv::run(csv_path, conf_path, write)
}

/// Read a single-word directive's value from a profile (skips `#` comments
/// and `=>` rules). Used to peek at `wallet.coin` before committing to a backend.
fn directive(src: &str, key: &str) -> Option<String> {
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.contains("=>") {
            continue;
        }
        if let Some((k, v)) = line.split_once(char::is_whitespace)
            && k.trim() == key
        {
            return Some(v.trim().to_string());
        }
    }
    None
}

/// How a rule condition matches its field (case-insensitive). A bare value
/// matches anywhere; `^` anchors the start, `$` the end, `^…$` the whole
/// field — mirroring the report filter and `rename`. Shared by every source.
#[derive(Debug, Clone, Copy)]
enum Match {
    Contains,
    StartsWith,
    EndsWith,
    Exact,
}

impl Match {
    /// Split a raw value into its anchor mode and the core text (anchors
    /// stripped). `^` / `$` are ASCII, so byte-slicing keeps UTF-8 valid.
    fn parse(value: &str) -> (Match, &str) {
        let start = value.starts_with('^');
        let end = value.ends_with('$');
        let core = &value[start as usize..value.len() - end as usize];
        let mode = match (start, end) {
            (true, true) => Match::Exact,
            (true, false) => Match::StartsWith,
            (false, true) => Match::EndsWith,
            (false, false) => Match::Contains,
        };
        (mode, core)
    }

    /// Test an already-lowercased field against an already-lowercased
    /// needle under this mode.
    fn test(&self, haystack: &str, needle: &str) -> bool {
        match self {
            Match::Contains => haystack.contains(needle),
            Match::StartsWith => haystack.starts_with(needle),
            Match::EndsWith => haystack.ends_with(needle),
            Match::Exact => haystack == needle,
        }
    }
}

struct Rule {
    /// (field name, lowercased needle, anchor mode) — all must match.
    conds: Vec<(String, String, Match)>,
    account: String,
}

/// The first rule whose every condition matches — its account template to
/// book to, or `None` to fall through to the profile's default. `get`
/// returns a field's value; that lookup is all that differs between a CSV
/// row (by column) and an RPC transfer (by name), so the loop is shared.
fn match_account<'a>(rules: &'a [Rule], get: impl Fn(&str) -> String) -> Option<&'a str> {
    rules
        .iter()
        .find(|rule| {
            rule.conds
                .iter()
                .all(|(field, needle, mode)| mode.test(&get(field).to_lowercase(), needle))
        })
        .map(|rule| rule.account.as_str())
}


fn slug(s: &str) -> String {
    s.to_lowercase().replace(' ', "-")
}

/// Build a directional in-transit account `prefix:sender:receiver` for an
/// own↔own transfer. The order encodes the money flow: `outgoing` books
/// own → other, an incoming one other → own. Both legs of the same transfer
/// therefore produce the identical string and net to 0, and the order is
/// computed (never typed), so two profiles can't disagree. Shared by every
/// source (a CSV counterparty IBAN, a wallet's destination address).
fn directional_account(prefix: &str, own: &str, other: &str, outgoing: bool) -> String {
    let (from, to) = if outgoing { (own, other) } else { (other, own) };
    format!("{}:{}:{}", prefix, from, to)
}

/// Own↔own transit configuration, shared by every source. `transit.self` gives
/// the account prefix and this profile's own leaf; each `transit <key> <leaf>`
/// maps a counterparty key (a CSV IBAN, a wallet address) to the other
/// account's leaf. `account()` turns a match into a directional netting
/// account so both legs cancel to 0.
struct Transit {
    entries: Vec<(String, String)>, // (match key, other leaf)
    prefix: Option<String>,         // account prefix; Some = transit enabled
    own_leaf: String,               // this profile's leaf (last transit.self segment)
}

impl Transit {
    /// Parse `transit.self <prefix>:<leaf>` plus the already-collected
    /// `transit <key> <leaf>` entries. `transit.self` is required once any
    /// `transit` entry is present.
    fn parse(
        directives: &HashMap<String, String>,
        entries: Vec<(String, String)>,
    ) -> Result<Transit, Error> {
        let (prefix, own_leaf) = match directives.get("transit.self") {
            Some(s) => {
                let (p, own) = s.trim().rsplit_once(':').ok_or_else(|| {
                    Error::from(format!(
                        "import: transit.self '{}' must be <prefix>:<name>, e.g. assets:transit:main",
                        s.trim()
                    ))
                })?;
                (Some(p.to_string()), own.to_string())
            }
            None => (None, String::new()),
        };
        if !entries.is_empty() && prefix.is_none() {
            return Err(Error::from(
                "import: transit mappings need a 'transit.self' directive",
            ));
        }
        Ok(Transit { entries, prefix, own_leaf })
    }

    /// The directional netting account for a transfer to `other` — `None`
    /// when transit isn't configured (no `transit.self`).
    fn account(&self, other: &str, outgoing: bool) -> Option<String> {
        self.prefix
            .as_ref()
            .map(|p| directional_account(p, &self.own_leaf, other, outgoing))
    }
}

fn read(path: &str) -> Result<String, Error> {
    std::fs::read_to_string(expand(path))
        .map_err(|e| Error::from(format!("import: read {}: {}", path, e)))
}

/// Append the already-aligned additions to the ledger. A blank line
/// separates them from existing content, but a fresh (empty) file starts
/// straight at the first transaction — no leading blank.
fn append(path: &Path, added: &str) -> Result<(), Error> {
    use std::io::Write as _;
    let has_content = std::fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false);
    let lead = if has_content { "\n" } else { "" };
    let body = format!("{}{}\n", lead, added.trim_end());
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| Error::from(format!("import: open {}: {}", path.display(), e)))?;
    f.write_all(body.as_bytes())
        .map_err(|e| Error::from(format!("import: write {}: {}", path.display(), e)))
}

/// The single tail every import backend ends on: format the rendered blocks
/// (acc `format`, in memory) so imported entries line up like every other file,
/// then append them (when writing) and show the diff preview. Centralised so the
/// format step is applied uniformly and can never be forgotten in a backend.
/// `read`/`noun` word the "nothing new" note when there is nothing to add.
fn emit(
    blocks: &[String],
    read: usize,
    noun: &str,
    existing: &str,
    output: &Path,
    skipped: usize,
    write: bool,
) -> Result<(), Error> {
    use colored::Colorize;
    use std::io::IsTerminal;
    // A pipe (stdout not a terminal) gets pure ledger, not the coloured diff —
    // so `acc import … | acc print -f -` can parse the would-be additions.
    let piped = !std::io::stdout().is_terminal();
    if blocks.is_empty() {
        let msg = format!(
            "{} import: {} {} read, all already present — nothing new.",
            "!".yellow(),
            read,
            noun
        );
        if piped { eprintln!("{}", msg) } else { println!("{}", msg) }
        return Ok(());
    }
    let added = crate::commands::format::format_source(&blocks.join("\n\n"), false)?;
    if write {
        append(output, &added)?;
    }
    if piped {
        // Plain ledger on stdout (parse it with `| acc print -f -`); the human
        // ✓/! summary goes to stderr so it never pollutes the piped ledger.
        println!("{}", added.trim_end());
        eprintln!("{}", render_lib::summary(blocks.len(), output, skipped, write));
    } else {
        render_lib::diff_preview(existing, &added, blocks.len(), output, skipped, write);
    }
    Ok(())
}

/// Expand a leading `~/` to `$HOME`.
fn expand(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return Path::new(&home).join(rest);
    }
    PathBuf::from(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directional_account_orders_by_money_flow() {
        // Outgoing from checking and incoming to savings both describe the same
        // checking→savings flow, so they build the identical account and net to 0.
        assert_eq!(
            directional_account("assets:transit", "checking", "savings", true),
            "assets:transit:checking:savings"
        );
        assert_eq!(
            directional_account("assets:transit", "savings", "checking", false),
            "assets:transit:checking:savings"
        );
        // The reverse flow savings→checking builds the reversed account.
        assert_eq!(
            directional_account("assets:transit", "savings", "checking", true),
            "assets:transit:savings:checking"
        );
    }

    #[test]
    fn transit_parse_and_account() {
        // Entries without a `transit.self` are an error.
        let empty = HashMap::new();
        assert!(Transit::parse(&empty, vec![("x".into(), "y".into())]).is_err());

        // `transit.self` splits its last segment off as the own leaf.
        let mut d = HashMap::new();
        d.insert("transit.self".into(), "assets:transit:main".into());
        let t = Transit::parse(&d, vec![("x".into(), "y".into())]).unwrap();
        assert_eq!(t.prefix.as_deref(), Some("assets:transit"));
        assert_eq!(t.own_leaf, "main");
        assert_eq!(t.account("other", true).unwrap(), "assets:transit:main:other");
        assert_eq!(t.account("other", false).unwrap(), "assets:transit:other:main");

        // No `transit.self` and no entries → disabled; account() is None.
        let off = Transit::parse(&empty, Vec::new()).unwrap();
        assert!(off.account("other", true).is_none());
    }
}
