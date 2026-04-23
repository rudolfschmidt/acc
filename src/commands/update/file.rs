use crate::error::Error;

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

const PRICES_DIR_ENV: &str = "ACC_PRICES_DIR";

/// Base directory for all price files. Read from `ACC_PRICES_DIR`.
/// Returns `Err` if the env var is not set.
fn prices_dir() -> Result<PathBuf, Error> {
    std::env::var(PRICES_DIR_ENV)
        .map(PathBuf::from)
        .map_err(|_| {
            Error::new(format!(
                "environment variable '{}' is not set",
                PRICES_DIR_ENV
            ))
        })
}

fn crypto_dir() -> Result<PathBuf, Error> {
    let mut dir = prices_dir()?;
    dir.push("crypto");
    Ok(dir)
}

/// Absolute path for a given pair's crypto price file.
/// `$ACC_PRICES_DIR/crypto/MEXC_{BASE}_{QUOTE}.ledger`
pub fn path_for(base: &str, quote: &str) -> Result<PathBuf, Error> {
    let mut path = crypto_dir()?;
    path.push(format!("MEXC_{}_{}.ledger", base, quote));
    Ok(path)
}

pub fn fiat_dir() -> Result<PathBuf, Error> {
    let mut dir = prices_dir()?;
    dir.push("fiat");
    Ok(dir)
}

/// Path for a fiat price file (one per day).
pub fn fiat_path_for(date: &str) -> Result<PathBuf, Error> {
    let mut dir = fiat_dir()?;
    dir.push(format!("{}.ledger", date));
    Ok(dir)
}

/// Scan existing fiat files, find the latest date and the set of currency codes used.
/// Reads all `*.ledger` files in the fiat dir; for each line matching
/// `P DATE USD SYMBOL RATE`, collect SYMBOL. Returns (latest_date, symbols).
pub fn scan_fiat() -> Result<(Option<String>, Vec<String>), Error> {
    let dir = fiat_dir()?;
    if !dir.exists() {
        return Ok((None, Vec::new()));
    }
    let mut latest: Option<String> = None;
    let mut symbols: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(date) = name.strip_suffix(".ledger") else {
            continue;
        };
        // Track latest date by string compare (YYYY-MM-DD is lexicographic).
        latest = Some(match latest {
            None => date.to_string(),
            Some(prev) if prev.as_str() < date => date.to_string(),
            Some(prev) => prev,
        });
        // Parse symbols from the file content.
        if let Ok(content) = fs::read_to_string(&path) {
            for line in content.lines() {
                let tokens: Vec<&str> = line.split_whitespace().collect();
                // Expect: P DATE BASE TARGET RATE (5 tokens)
                if tokens.len() >= 5 && tokens[0] == "P" {
                    symbols.insert(tokens[3].to_string());
                }
            }
        }
    }
    Ok((latest, symbols.into_iter().collect()))
}

/// Scan the crypto price directory and return all (base, quote) pairs derived
/// from file names matching the `MEXC_{BASE}_{QUOTE}.ledger` pattern.
/// Returns empty Vec if the directory does not exist.
pub fn discover_crypto_pairs() -> Result<Vec<(String, String)>, Error> {
    let dir = crypto_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut pairs = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        // MEXC_{BASE}_{QUOTE}.ledger
        let Some(stem) = name.strip_suffix(".ledger") else {
            continue;
        };
        let parts: Vec<&str> = stem.split('_').collect();
        if parts.len() != 3 || parts[0] != "MEXC" {
            continue;
        }
        pairs.push((parts[1].to_string(), parts[2].to_string()));
    }
    pairs.sort();
    Ok(pairs)
}

/// Read an existing price file and return (date, raw-rate-string) tuples.
/// Returns empty Vec if the file does not exist.
/// Lines that do not match `P DATE BASE QUOTE RATE` are silently skipped.
pub fn read_existing(path: &PathBuf) -> Result<Vec<(String, String)>, Error> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(path)?;
    let mut out = Vec::new();
    for line in content.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        // Format: P DATE BASE QUOTE RATE  → 5 tokens
        if tokens.len() != 5 || tokens[0] != "P" {
            continue;
        }
        out.push((tokens[1].to_string(), tokens[4].to_string()));
    }
    Ok(out)
}

/// Merge existing + fetched into a deduplicated, date-sorted list.
/// Fetched values win on duplicate dates.
pub fn merge_and_sort(
    existing: Vec<(String, String)>,
    fetched: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    for (d, r) in existing {
        map.insert(d, r);
    }
    for (d, r) in fetched {
        map.insert(d, r); // overwrites
    }
    map.into_iter().collect()
}

/// Write sorted entries to `path` in `P DATE BASE QUOTE RATE` format.
/// The rate string is written verbatim — no rounding, no re-formatting.
/// Creates parent directories as needed. Atomic via temp+rename.
pub fn write_sorted(
    path: &PathBuf,
    base: &str,
    quote: &str,
    entries: &[(String, String)],
) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut content = String::new();
    for (date, rate) in entries {
        content.push_str(&format!("P {} {} {} {}\n", date, base, quote, rate));
    }
    let tmp = path.with_extension("ledger.tmp");
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_dedupe() {
        let existing = vec![
            ("2020-01-01".to_string(), "0.01".to_string()),
            ("2020-01-02".to_string(), "0.02".to_string()),
        ];
        let fetched = vec![
            ("2020-01-02".to_string(), "9.99".to_string()),
            ("2020-01-03".to_string(), "0.03".to_string()),
        ];
        let merged = merge_and_sort(existing, fetched);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[1].1, "9.99");
        assert_eq!(merged[2].0, "2020-01-03");
    }

    #[test]
    fn test_merge_sorted() {
        let existing = vec![
            ("2020-03-01".to_string(), "3".to_string()),
            ("2020-01-01".to_string(), "1".to_string()),
        ];
        let fetched = vec![("2020-02-01".to_string(), "2".to_string())];
        let merged = merge_and_sort(existing, fetched);
        assert_eq!(merged[0].0, "2020-01-01");
        assert_eq!(merged[1].0, "2020-02-01");
        assert_eq!(merged[2].0, "2020-03-01");
    }

    #[test]
    fn test_read_nonexistent_returns_empty() {
        let path = PathBuf::from("/tmp/definitely-does-not-exist-acc-xyz.ledger");
        assert!(read_existing(&path).unwrap().is_empty());
    }

    #[test]
    fn test_write_preserves_raw_string_no_rounding() {
        let tmp = std::env::temp_dir().join("acc-test-no-round.ledger");
        let _ = fs::remove_file(&tmp);
        // Contrived long decimal — must survive verbatim.
        let entries = vec![(
            "2020-01-01".to_string(),
            "0.123456789012345678901234567890".to_string(),
        )];
        write_sorted(&tmp, "BTC", "USDT", &entries).unwrap();
        let content = fs::read_to_string(&tmp).unwrap();
        assert!(content.contains("0.123456789012345678901234567890"));
        let _ = fs::remove_file(&tmp);
    }
}
