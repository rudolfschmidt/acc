use crate::error::Error;

use super::env::load_api_key;
use super::file;

const OXR_BASE: &str = "https://openexchangerates.org/api/historical";
const KEY_VAR: &str = "OPENEXCHANGERATES_API_KEY";

/// Entry point: update fiat rate files.
/// Fetches **all** currencies OXR returns for USD base — no symbol filter.
/// - `date` (if Some): fetch only that single day.
/// - `since` (if Some): fetch range [since, today].
/// - else: fetch (day after latest existing file) to today.
///
/// `cadence` controls step size (daily / monthly / yearly) for the loop.
pub fn run(
    since: Option<&str>,
    date: Option<&str>,
    cadence: super::Cadence,
    skip: bool,
) -> Result<(), Error> {
    let app_id = load_api_key(KEY_VAR)?;

    if let Some(d) = date {
        if skip && file::fiat_path_for(d)?.exists() {
            println!("fiat {}: exists, skipping", d);
            return Ok(());
        }
        return fetch_and_write(&app_id, d);
    }

    let (latest, _symbols) = file::scan_fiat()?;
    let start = match since {
        Some(d) => d.to_string(),
        None => match latest {
            Some(d) => advance(&d, cadence)?,
            None => {
                return Err(Error::new(
                    "fiat: no existing files found — provide --since DATE or --date DATE",
                ));
            }
        },
    };
    let today = crate::date::ms_to_date(crate::date::current_ms());
    if start.as_str() > today.as_str() {
        println!("fiat: already up to date ({})", today);
        return Ok(());
    }

    let mut cursor = start.clone();
    let mut written = 0;
    let mut skipped = 0;
    loop {
        if cursor.as_str() > today.as_str() {
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
        println!("fiat: {} written, {} skipped (existing)", written, skipped);
    } else {
        println!("fiat: {} entries written", written);
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
            println!("fiat {}: {} rates", date, rates.len());
            Ok(())
        }
        Err(e) => {
            eprintln!("fiat {}: {}", date, e);
            Err(e)
        }
    }
}

/// Fetch one day's rates for **all** currencies OXR exposes.
/// Base is always USD on free/paid tiers.
fn fetch_day(app_id: &str, date: &str) -> Result<Vec<(String, String)>, Error> {
    let url = format!("{}/{}.json?app_id={}&base=USD", OXR_BASE, date, app_id);
    let body = match ureq::get(&url).call() {
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
                "AED": 3.673,
                "EUR": 0.816,
                "CHF": 0.89
            }
        }"#;
        let r = parse_response(body).unwrap();
        assert_eq!(r.len(), 3);
        // sorted alphabetically
        assert_eq!(r[0].0, "AED");
        assert_eq!(r[1].0, "CHF");
        assert_eq!(r[2].0, "EUR");
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
