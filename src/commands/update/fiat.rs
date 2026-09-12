use colored::Colorize;

use crate::error::Error;

use super::file;

const OXR_BASE: &str = "https://openexchangerates.org/api/historical";

/// Entry point: update fiat rate files.
/// Fetches **all** currencies OXR returns for USD base — no symbol filter.
/// - `begin` (if Some): the first day to fetch, else the latest existing
///   file, which is refetched because it may hold a provisional rate.
/// - `end` (if Some): stop before this day, else run up to today.
///
/// `cadence` controls step size (daily / monthly / yearly) for the loop.
pub fn run(
    begin: Option<&str>,
    end: Option<&str>,
    cadence: super::Cadence,
    skip: bool,
) -> Result<(), Error> {
    let app_id = crate::loader::config::openexchangerates().ok_or_else(|| {
        Error::new("no `[exchange] openexchangerates` key in the CONFIG_DIR config".to_string())
    })?;

    let (latest, _symbols) = file::scan_fiat()?;
    let start = match begin {
        Some(d) => d.to_string(),
        None => match latest {
            // The latest file is refetched, not skipped. OXR dates a day
            // at 23:59:59 UTC, so a file written while its day was still
            // running holds an intra-day snapshot; starting one step
            // later would freeze that snapshot forever.
            Some(d) => d,
            None => {
                return Err(Error::new(
                    "fiat: no existing files found — provide -b DATE or -p PERIOD",
                ));
            }
        },
    };
    let today = crate::date::ms_to_date(crate::date::current_ms());
    if start.as_str() > today.as_str() {
        println!("{} fiat: already up to date ({})", "✓".green(), today);
        return Ok(());
    }

    let mut cursor = start.clone();
    let mut written = 0;
    let mut skipped = 0;
    loop {
        // Today caps every run, since a day the world has not reached
        // has no rate; `end` is exclusive and cuts in before that.
        if cursor.as_str() > today.as_str() || end.is_some_and(|e| cursor.as_str() >= e) {
            break;
        }
        if skip && file::fiat_path_for(&cursor)?.exists() {
            skipped += 1;
        } else if fetch_and_write(&app_id, &cursor).is_err() {
            // Stop: rate-limit or auth failure would keep failing.
            break;
        } else {
            written += 1;
        }
        cursor = advance(&cursor, cadence)?;
    }
    if skip {
        println!("{} fiat: {} written, {} skipped (existing)", "✓".green(), written, skipped);
    } else {
        println!("{} fiat: {} entries written", "✓".green(), written);
    }
    Ok(())
}

fn advance(date: &str, cadence: super::Cadence) -> Result<String, Error> {
    Ok(match cadence {
        super::Cadence::Daily => crate::date::day_after(date)?,
        super::Cadence::Monthly => crate::date::next_month_start(date)?,
        super::Cadence::Yearly => crate::date::next_year_start(date)?,
    })
}

fn fetch_and_write(app_id: &str, date: &str) -> Result<(), Error> {
    match fetch_day(app_id, date) {
        Ok(rates) => {
            let path = file::fiat_path_for(date)?;
            write_day(&path, date, &rates)?;
            println!("{} fiat {}: {} rates", "✓".green(), date, rates.len());
            Ok(())
        }
        Err(e) => {
            eprintln!("{} fiat {}: {}", "✗".red(), date, e);
            Err(e)
        }
    }
}

/// Fetch one day's rates for **all** currencies OXR exposes.
/// Base is always USD on free/paid tiers.
fn fetch_day(app_id: &str, date: &str) -> Result<Vec<(String, String)>, Error> {
    let url = format!("{}/{}.json?app_id={}&base=USD", OXR_BASE, date, app_id);
    let body = match super::agent().get(&url).call() {
        Ok(resp) => resp.into_string()?,
        Err(ureq::Error::Status(code, resp)) => {
            let msg = resp.into_string().unwrap_or_default();
            return Err(Error::new(format!("HTTP {}: {}", code, msg)));
        }
        Err(e) => return Err(Error::new(e.to_string())),
    };
    parse_response(&body)
}

/// Parse the OXR response and return (symbol, rate) tuples with the raw rate
/// string exactly as the API returned it — no rounding, no re-formatting.
pub fn parse_response(body: &str) -> Result<Vec<(String, String)>, Error> {
    let value: serde_json::Value = serde_json::from_str(body)?;
    // Error response has an "error" field.
    if value.get("error").and_then(|v| v.as_bool()).unwrap_or(false) {
        let msg = value
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("openexchangerates error");
        return Err(Error::new(msg.to_string()));
    }
    let rates = value
        .get("rates")
        .and_then(|v| v.as_object())
        .ok_or_else(|| Error::new("response missing 'rates' object".to_string()))?;
    let mut out = Vec::with_capacity(rates.len());
    for (sym, val) in rates {
        let rate_str = match val {
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => s.clone(),
            _ => continue,
        };
        out.push((sym.clone(), rate_str));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn write_day(
    path: &std::path::PathBuf,
    date: &str,
    rates: &[(String, String)],
) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut content = String::new();
    for (sym, rate) in rates {
        content.push_str(&format!("P {} USD {} {}\n", date, sym, rate));
    }
    std::fs::write(path, content)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_happy() {
        let body = r#"{
            "disclaimer": "x",
            "license": "y",
            "timestamp": 1609459200,
            "base": "USD",
            "rates": {
                "AUD": 1.532,
                "GBP": 0.816,
                "CHF": 0.89
            }
        }"#;
        let r = parse_response(body).unwrap();
        assert_eq!(r.len(), 3);
        // sorted alphabetically
        assert_eq!(r[0].0, "AUD");
        assert_eq!(r[1].0, "CHF");
        assert_eq!(r[2].0, "GBP");
    }

    #[test]
    fn test_parse_error_payload() {
        let body = r#"{"error": true, "status": 401, "description": "invalid_app_id"}"#;
        assert!(parse_response(body).is_err());
    }

    #[test]
    fn test_parse_missing_rates() {
        let body = r#"{"disclaimer": "x"}"#;
        assert!(parse_response(body).is_err());
    }
}
