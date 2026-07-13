mod cli;
mod env;
mod fetch;
mod fiat;
mod file;

use colored::Colorize;

use crate::date::{date_to_ms, day_after};
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
    since: Option<&str>,
    date: Option<&str>,
    cadence: Cadence,
    skip: bool,
    flags: UpdateFlags,
) -> Result<(), Error> {
    if flags.crypto {
        run_crypto(pairs, since, date)?;
    }
    if flags.fiat {
        fiat::run(since, date, cadence, skip)?;
    }
    Ok(())
}

fn run_crypto(pairs: &[String], since: Option<&str>, date: Option<&str>) -> Result<(), Error> {
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
            process_pair(&pair, since, date);
        }
        return Ok(());
    }
    for spec in pairs {
        match parse_pair(spec) {
            Ok(pair) => process_pair(&pair, since, date),
            Err(e) => eprintln!("{} skip '{}': {}", "!".yellow(), spec, e),
        }
    }
    Ok(())
}

fn process_pair(pair: &Pair, since: Option<&str>, date: Option<&str>) {
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

    // --date D takes precedence: replace only that one day.
    let (start_date, end_date) = if let Some(d) = date {
        existing.retain(|(date, _)| date.as_str() != d);
        (d.to_string(), Some(d.to_string()))
    } else if let Some(d) = since {
        let cutoff = d.to_string();
        existing.retain(|(date, _)| date.as_str() < cutoff.as_str());
        (cutoff, None)
    } else if let Some((last_date, _)) = existing.last() {
        match day_after(last_date) {
            Ok(d) => (d, None),
            Err(e) => {
                eprintln!("{} {}: invalid date in cache: {}", "✗".red(), pair.display(), e);
                return;
            }
        }
    } else {
        eprintln!(
            "{} {}: no existing file — provide --since DATE or --date DATE",
            "!".yellow(),
            pair.display()
        );
        return;
    };

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

    let fetched: Vec<(String, String)> = fetched_raw
        .into_iter()
        .filter(|(d, _)| match &end_date {
            // In single-date mode keep only the requested day.
            Some(only) => d.as_str() == only.as_str(),
            None => true,
        })
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
