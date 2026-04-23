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
pub use journal::Journal;

use std::collections::HashMap;
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
        fx_gain: resolved.fx_gain,
        fx_loss: resolved.fx_loss,
        precisions,
    })
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
    fn extracts_fx_accounts() {
        let src = "account Equity:FxGain\n    fx gain\naccount Equity:FxLoss\n    fx loss\n";
        with_tmp("fx", src, |path| {
            let journal = load(&[path]).unwrap();
            assert_eq!(journal.fx_gain.as_deref(), Some("Equity:FxGain"));
            assert_eq!(journal.fx_loss.as_deref(), Some("Equity:FxLoss"));
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
}
