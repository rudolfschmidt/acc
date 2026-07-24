//! Shared helpers for the exchange import backends (`kraken_api`, `crypto_csv`).
//!
//! Signed decimal-string arithmetic — so a fee nets exactly at each amount's
//! own natural precision (no fixed 4dp/8dp) — plus the commodity-alias reader
//! (a currency code → its ledger symbol, EUR→€ / USD→$). A `parity` commodity
//! (USDC/USDT) is deliberately NOT folded: parity is a report-time valuation,
//! never a source substitution.

use std::collections::HashMap;
use std::path::Path;

use super::crypto_lib::money;

/// Natural decimal precision of a decimal string ("74.0004" → 4, "5" → 0).
pub(super) fn dp_of(s: &str) -> u32 {
    s.rsplit_once('.').map(|(_, f)| f.len() as u32).unwrap_or(0)
}

/// Flip the sign of a decimal string ("74.0004" → "-74.0004", "-5" → "5").
pub(super) fn neg(s: &str) -> String {
    match s.strip_prefix('-') {
        Some(rest) => rest.to_string(),
        None => format!("-{}", s),
    }
}

/// Magnitude of a signed decimal string (drop a leading '-').
pub(super) fn mag(s: &str) -> &str {
    s.strip_prefix('-').unwrap_or(s)
}

/// Whether a decimal string is zero — no non-zero digit.
pub(super) fn is_zero(s: &str) -> bool {
    !s.bytes().any(|b| b.is_ascii_digit() && b != b'0')
}

/// Parse a signed decimal string into i128 atomic units at `dp` places.
pub(super) fn atomic(s: &str, dp: u32) -> i128 {
    let s = s.trim();
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    let mut frac = frac.to_string();
    frac.truncate(dp as usize);
    while (frac.len() as u32) < dp {
        frac.push('0');
    }
    let int: i128 = int.trim().parse().unwrap_or(0);
    let frac: i128 = if dp == 0 { 0 } else { frac.parse().unwrap_or(0) };
    let v = int * 10i128.pow(dp) + frac;
    if neg { -v } else { v }
}

/// A signed atomic amount at `dp` places (magnitude via the shared `money`).
pub(super) fn signed(v: i128, dp: u32) -> String {
    let m = money(v.abs(), dp);
    if v < 0 { format!("-{}", m) } else { m }
}

/// Parse a `commodities.ledger` for its `alias` declarations only: inside a
/// `commodity SYM` block each `alias CODE` maps CODE→SYM (EUR→€, USD→$).
/// `parity` / `precision` lines are ignored — a parity commodity keeps its own
/// symbol (it values 1:1 at report time), so folding it here would be wrong.
/// Best-effort: a missing or unreadable file yields an empty map.
pub(super) fn load_aliases(path: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(src) = std::fs::read_to_string(path) else {
        return map;
    };
    let mut current: Option<String> = None;
    for line in src.lines() {
        let t = line.trim();
        if let Some(sym) = t.strip_prefix("commodity ") {
            current = Some(sym.trim().to_string());
        } else if let Some(code) = t.strip_prefix("alias ")
            && let Some(sym) = &current
        {
            map.insert(code.trim().to_string(), sym.clone());
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_and_signed_roundtrip() {
        assert_eq!(atomic("1037.1000", 2), 103710);
        assert_eq!(atomic("-1063.7500", 2), -106375);
        assert_eq!(atomic("23.2869965000", 8), 2_328_699_650);
        assert_eq!(atomic("0.0020000000", 8), 200_000);
        assert_eq!(signed(-2_328_775_680, 8), "-23.28775680");
        assert_eq!(signed(103710, 2), "1037.10");
    }

    #[test]
    fn decimal_string_helpers() {
        assert_eq!(dp_of("74.0004"), 4);
        assert_eq!(dp_of("5"), 0);
        assert_eq!(neg("74.0004"), "-74.0004");
        assert_eq!(neg("-5"), "5");
        assert_eq!(mag("-23.22"), "23.22");
        assert!(is_zero("0.0000"));
        assert!(!is_zero("0.0020000000"));
    }

    #[test]
    fn load_aliases_reads_only_alias_lines_not_parity() {
        use std::io::Write as _;
        let dir = std::env::temp_dir().join(format!("acc-exlib-alias-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("commodities.ledger");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"commodity $\n\talias USD\ncommodity USDC\n\tparity $\n\tprecision 2\ncommodity \xe2\x82\xac\n\talias EUR\n").unwrap();
        let map = load_aliases(&path);
        assert_eq!(map.get("USD").map(String::as_str), Some("$"));
        assert_eq!(map.get("EUR").map(String::as_str), Some("€"));
        assert!(!map.contains_key("USDC")); // parity, not an alias
        std::fs::remove_file(&path).ok();
    }
}
