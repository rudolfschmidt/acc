mod cli;
mod fetch;
mod fiat;
pub(crate) mod file;

use colored::Colorize;

use crate::date::date_to_ms;
use crate::error::Error;

use cli::{parse_pair, Pair};
use fetch::{mexc_klines, FetchResult};

/// A shared `ureq` agent wired with the native-tls TLS backend.
///
/// `ureq`'s `native-tls` feature does *not* configure the default agent —
/// the connector has to be built explicitly — so a bare `ureq::get` has no
/// TLS backend and HTTPS fails with "no TLS backend is configured". We
/// build one agent (cheap to clone — it's `Arc` inside) and reuse it for
/// every request.
fn agent() -> ureq::Agent {
    use std::sync::OnceLock;
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            let connector = native_tls::TlsConnector::new()
                .expect("initialise native-tls connector");
            ureq::AgentBuilder::new()
                .tls_connector(std::sync::Arc::new(connector))
                .build()
        })
        .clone()
}

/// Flags controlling which domains are updated.
pub struct UpdateFlags {
    pub crypto: bool,
    pub fiat: bool,
}

/// Fiat fetch cadence: daily (default) or coarser monthly/yearly snapshots.
#[derive(Copy, Clone)]
pub enum Cadence {
    Daily,
    Monthly,
    Yearly,
}

pub fn run(
    pairs: &[String],
    range: &crate::period::Range,
    cadence: Cadence,
    skip: bool,
    flags: UpdateFlags,
) -> Result<(), Error> {
    // A report can hold several periods apart and match against their
    // union; a fetch would have to walk the gap between them, so it
    // takes one span at a time.
    if range.periods.len() > 1 {
        return Err(Error::new(
            "update takes one -p at a time — use -b/-e for a span across periods".to_string(),
        ));
    }
    let (begin, end) = (range.begin.as_deref(), range.end.as_deref());
    if flags.crypto {
        run_crypto(pairs, begin, end)?;
    }
    if flags.fiat {
        fiat::run(begin, end, cadence, skip)?;
    }
    Ok(())
}

fn run_crypto(pairs: &[String], begin: Option<&str>, end: Option<&str>) -> Result<(), Error> {
    if pairs.is_empty() {
        let discovered = file::discover_crypto_pairs()?;
        if discovered.is_empty() {
            eprintln!(
                "{} crypto: no --pair given and no existing files in $PRICES/crypto/",
                "!".yellow()
            );
            return Ok(());
        }
        for (base, quote) in discovered {
            let pair = Pair { base, quote };
            process_pair(&pair, begin, end);
        }
        return Ok(());
    }
    for spec in pairs {
        match parse_pair(spec) {
            Ok(pair) => process_pair(&pair, begin, end),
            Err(e) => eprintln!("{} skip '{}': {}", "!".yellow(), spec, e),
        }
    }
    Ok(())
}

fn process_pair(pair: &Pair, begin: Option<&str>, end: Option<&str>) {
    let path = match file::path_for(&pair.base, &pair.quote) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{} {}: {}", "✗".red(), pair.display(), e);
            return;
        }
    };

    let mut existing = match file::read_existing(&path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{} {}: read error: {}", "✗".red(), pair.display(), e);
            return;
        }
    };

    let start_date = match begin {
        Some(b) => b.to_string(),
        // The last stored day is refetched, not skipped. An exchange's
        // daily candle only closes at 00:00 UTC, so a row written while
        // its day was still running holds the rate of whatever moment
        // the fetch happened to land on. Starting one day later would
        // freeze that provisional rate forever; starting on the day
        // itself replaces it, with the current rate while the day runs
        // and with the real close once it is over. Every earlier day is
        // final and stays untouched.
        None => match existing.last() {
            Some((last_date, _)) => last_date.clone(),
            None => {
                eprintln!(
                    "{} {}: no existing file — provide -b DATE or -p PERIOD",
                    "!".yellow(),
                    pair.display()
                );
                return;
            }
        },
    };
    // Whatever is fetched replaces what is stored for those days, so the
    // range is dropped here and the fetched rows take its place.
    existing.retain(|(d, _)| {
        d.as_str() < start_date.as_str() || end.is_some_and(|e| d.as_str() >= e)
    });

    let start_ms = match date_to_ms(&start_date) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{} {}: {}", "✗".red(), pair.display(), e);
            return;
        }
    };

    let fetched_raw = match mexc_klines(&pair.base, &pair.quote, start_ms) {
        FetchResult::Ok(v) => v,
        FetchResult::NotListed => {
            eprintln!("{} {}: not listed on MEXC, skipping", "!".yellow(), pair.display());
            return;
        }
        FetchResult::OtherError(msg) => {
            eprintln!("{} {}: fetch error: {}", "✗".red(), pair.display(), msg);
            return;
        }
    };

    // MEXC serves from the start day to now, so the exclusive end is
    // applied here rather than in the request.
    let fetched: Vec<(String, String)> = fetched_raw
        .into_iter()
        .filter(|(d, _)| end.is_none_or(|e| d.as_str() < e))
        .collect();
    let new_count = fetched.len();

    let merged = file::merge_and_sort(existing, fetched);

    if let Err(e) = file::write_sorted(&path, &pair.base, &pair.quote, &merged) {
        eprintln!("{} {}: write error: {}", "✗".red(), pair.display(), e);
        return;
    }

    println!(
        "{} {}: {} lines total ({} fetched)",
        "✓".green(),
        pair.display(),
        merged.len(),
        new_count
    );
}
