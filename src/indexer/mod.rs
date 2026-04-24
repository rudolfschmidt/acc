//! Indexer phase.
//!
//! Consumes the date-sorted `Vec<Located<Price>>` produced by the resolver
//! and returns an `Index` ready for fast exchange-rate lookups.
//!
//! This is a bulk-load pass: the input is date-sorted so each per-pair
//! `Vec<(day, rate)>` is built by plain `push`. Commodity symbols are
//! already interned as `Arc<str>` by the resolver, so the indexer just
//! stores the shared references. The resulting DB answers
//! `find(base, quote, date)` queries via BFS over the commodity graph,
//! with reciprocal edges computed on demand.

pub mod index;

pub use index::Index;

use crate::parser::entry::Price;
use crate::parser::located::Located;

/// Build an `Index` from resolved price directives. The input is
/// expected to come from the resolver already date-sorted and with
/// aliases applied, but the DB tolerates arbitrary order — only the
/// resulting lookup behaviour depends on stored values, not insertion
/// order.
pub fn index(prices: Vec<Located<Price>>) -> Index {
    let mut db = Index::new();
    for located in prices {
        let Price { date, base, quote, rate, .. } = located.value;
        db.add(base, quote, date.days(), rate);
    }
    db
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decimal::Decimal;
    use crate::parser;

    fn build(src: &str) -> Index {
        let entries = parser::parse(src).unwrap();
        let resolved = crate::resolver::resolve(entries).unwrap();
        index(resolved.prices)
    }

    #[test]
    fn builds_direct_lookup() {
        let db = build("P 2024-06-15 USD EUR 0.92\n");
        assert_eq!(db.find("USD", "EUR", "2024-06-16"), Some(Decimal::parse("0.92").unwrap()));
    }

    #[test]
    fn inverse_is_computed_on_the_fly() {
        let db = build("P 2024-06-15 USD EUR 0.5\n");
        assert_eq!(db.find("EUR", "USD", "2024-06-16"), Some(Decimal::from(2)));
    }

    #[test]
    fn multi_hop_path() {
        let src = "P 2024-06-15 USD CHF 0.9\nP 2024-06-15 CHF EUR 1.02\n";
        let db = build(src);
        // USD → CHF (0.9) → EUR (1.02) = 0.918
        let rate = db.find("USD", "EUR", "2024-06-16").unwrap();
        assert_eq!(rate, Decimal::parse("0.9").unwrap() * Decimal::parse("1.02").unwrap());
    }

    #[test]
    fn same_commodity_returns_one() {
        let db = Index::new();
        assert_eq!(db.find("USD", "USD", "2024-06-15"), Some(Decimal::from(1)));
    }

    #[test]
    fn latest_before_date() {
        let src = "P 2024-01-01 USD EUR 0.9\nP 2024-06-01 USD EUR 0.92\n";
        let db = build(src);
        assert_eq!(db.find("USD", "EUR", "2024-03-01"), Some(Decimal::parse("0.9").unwrap()));
        assert_eq!(db.find("USD", "EUR", "2024-07-01"), Some(Decimal::parse("0.92").unwrap()));
    }

    #[test]
    fn missing_pair_returns_none() {
        let db = Index::new();
        assert_eq!(db.find("USD", "XYZ", "2024-06-15"), None);
    }

    #[test]
    fn zero_rate_ignored() {
        let src = "P 2024-06-15 USD EUR 0\n";
        let db = build(src);
        assert!(db.is_empty());
    }

    #[test]
    fn applies_aliases_from_resolver() {
        let src = "commodity USD\n    alias $\nP 2024-06-15 $ EUR 0.92\n";
        let db = build(src);
        // Resolver should have replaced $ with USD before indexing
        assert_eq!(db.find("USD", "EUR", "2024-06-16"), Some(Decimal::parse("0.92").unwrap()));
    }

}
