use crate::date::{current_ms, ms_to_date};

const MEXC_BASE: &str = "https://api.mexc.com/api/v3/klines";
const BATCH_DAYS: u64 = 500; // MEXC caps at 500 per call regardless of limit=
const MS_PER_DAY: u64 = 86_400_000;

/// Outcome of fetching klines for a pair.
pub enum FetchResult {
    Ok(Vec<(String, String)>),
    NotListed,
    OtherError(String),
}

/// Fetch daily klines for `{base}{quote}` starting from `start_ms` up to today.
/// Paginates internally if MEXC returns 1000 rows.
/// Returns (date, close-price-string) exactly as MEXC delivered it — no parsing,
/// no rounding, no conversion.
pub fn mexc_klines(base: &str, quote: &str, start_ms: u64) -> FetchResult {
    let symbol = format!("{}{}", base, quote);
    let now_ms = current_ms();
    let mut cursor = start_ms;
    let mut out: Vec<(String, String)> = Vec::new();
    loop {
        if cursor > now_ms {
            break;
        }
        let window_end = cursor + BATCH_DAYS * MS_PER_DAY;
        let url = format!(
            "{}?symbol={}&interval=1d&startTime={}&endTime={}&limit={}",
            MEXC_BASE, symbol, cursor, window_end, BATCH_DAYS
        );
        let body = match mexc_klines_raw(&url) {
            Ok(s) => s,
            Err(FetchError::NotFound) => return FetchResult::NotListed,
            Err(FetchError::Other(msg)) => return FetchResult::OtherError(msg),
        };
        let batch = match parse_klines_response(&body) {
            Ok(v) => v,
            Err(msg) => return FetchResult::OtherError(msg),
        };
        if batch.is_empty() {
            // Advance window even if empty — there may be listed data later.
            let next_cursor = window_end + MS_PER_DAY;
            if next_cursor <= cursor {
                break;
            }
            cursor = next_cursor;
            continue;
        }
        let last_ts = batch.last().map(|(ts_ms, _)| *ts_ms).unwrap_or(cursor);
        for (ts_ms, close) in batch {
            out.push((ms_to_date(ts_ms), close));
        }
        let next_cursor = last_ts + MS_PER_DAY;
        if next_cursor <= cursor {
            break;
        }
        cursor = next_cursor;
    }
    FetchResult::Ok(out)
}

enum FetchError {
    NotFound,
    Other(String),
}

fn mexc_klines_raw(url: &str) -> Result<String, FetchError> {
    match ureq::get(url).call() {
        Ok(resp) => resp
            .into_string()
            .map_err(|e| FetchError::Other(e.to_string())),
        Err(ureq::Error::Status(code, resp)) => {
            // MEXC returns 400 with code -1121 for "Invalid symbol"
            let body = resp.into_string().unwrap_or_default();
            if code == 404 || body.contains("Invalid symbol") || body.contains("-1121") {
                Err(FetchError::NotFound)
            } else {
                Err(FetchError::Other(format!("HTTP {}: {}", code, body)))
            }
        }
        Err(e) => Err(FetchError::Other(e.to_string())),
    }
}

/// Parse the MEXC klines JSON response.
/// Format: `[[ts, open, high, low, close, volume, ...], ...]`
/// Returns (ts_ms, close_raw_string) without any numeric conversion.
pub fn parse_klines_response(body: &str) -> Result<Vec<(u64, String)>, String> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let arr = value
        .as_array()
        .ok_or_else(|| "expected top-level array".to_string())?;
    let mut out = Vec::with_capacity(arr.len());
    for row in arr {
        let row_arr = match row.as_array() {
            Some(a) => a,
            None => continue,
        };
        if row_arr.len() < 5 {
            continue;
        }
        let ts = match row_arr[0].as_u64() {
            Some(t) => t,
            None => continue,
        };
        let close_str = match &row_arr[4] {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            _ => continue,
        };
        out.push((ts, close_str));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple() {
        let body = r#"[
            [1577836800000, "7200.00", "7400.00", "7100.00", "7300.00", "123.45"],
            [1577923200000, "7300.00", "7500.00", "7250.00", "7450.00", "234.56"]
        ]"#;
        let r = parse_klines_response(body).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], (1577836800000, "7300.00".to_string()));
        assert_eq!(r[1], (1577923200000, "7450.00".to_string()));
    }

    #[test]
    fn test_parse_preserves_long_decimals_verbatim() {
        let body = r#"[[1, "1", "2", "3", "0.123456789012345678", "5"]]"#;
        let r = parse_klines_response(body).unwrap();
        assert_eq!(r[0].1, "0.123456789012345678");
    }

    #[test]
    fn test_parse_empty_array() {
        let r = parse_klines_response("[]").unwrap();
        assert!(r.is_empty());
    }

    #[test]
    fn test_parse_malformed_returns_err() {
        assert!(parse_klines_response("not json").is_err());
        assert!(parse_klines_response("{\"not\": \"array\"}").is_err());
    }

    #[test]
    fn test_parse_passes_zero_through_verbatim() {
        // Zero values are not filtered here — PriceDB::add() drops them at load time.
        let body = r#"[
            [1000, "1", "2", "3", "0", "5"],
            [2000, "1", "2", "3", "4", "5"]
        ]"#;
        let r = parse_klines_response(body).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(r[0], (1000, "0".to_string()));
        assert_eq!(r[1], (2000, "4".to_string()));
    }

    #[test]
    fn test_parse_numeric_close() {
        // If exchange returns number instead of string
        let body = r#"[[1000, 1, 2, 3, 4.5, 5]]"#;
        let r = parse_klines_response(body).unwrap();
        assert_eq!(r.len(), 1);
    }
}
