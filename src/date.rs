//! `Date` — calendar date as days-since-1970, UTC, no time-of-day.
//!
//! A ledger is date-indexed, not timestamped: every entry belongs to a
//! whole day, not a specific instant. `Date` stores that day as a
//! single `u32` (≈ 11.7 million years of range after 1970), so
//! comparisons are one CPU op, sorting is trivial, and date arithmetic
//! (next day / next month / next year) reduces to integer math.
//!
//! The canonical textual form is `YYYY-MM-DD`. `Display` renders that
//! form; `Date::parse` accepts it strictly.
//!
//! A handful of legacy free functions at the bottom still deal in
//! string-encoded dates — used by the HTTP-fetching `update` command
//! that thinks in unix-ms timestamps. They stay as low-level helpers;
//! new code should use the `Date` type.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// Calendar date at day granularity.
/// Internal representation: `u32` days since `1970-01-01` (UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Date(u32);

impl Date {
    /// Parse the canonical `YYYY-MM-DD` form. Rejects any other shape.
    pub fn parse(s: &str) -> Result<Self, String> {
        Ok(Date(date_to_days(s)?))
    }

    /// Today's date (UTC midnight).
    pub fn today() -> Self {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Date((secs / 86_400) as u32)
    }

    /// Tomorrow's date (UTC).
    pub fn tomorrow() -> Self {
        Date(Self::today().0 + 1)
    }

    /// Build directly from a days-since-epoch count.
    pub fn from_days(days: u32) -> Self {
        Date(days)
    }

    /// Raw days-since-1970-01-01 accessor.
    pub fn days(self) -> u32 {
        self.0
    }

    /// Year component (e.g. `2024`).
    pub fn year(self) -> u16 {
        days_to_date(self.0 as u64).0 as u16
    }

    /// Month component (`1..=12`).
    pub fn month(self) -> u8 {
        days_to_date(self.0 as u64).1 as u8
    }

    /// Day-of-month component (`1..=31`).
    pub fn day(self) -> u8 {
        days_to_date(self.0 as u64).2 as u8
    }

    /// The day after this one.
    pub fn next_day(self) -> Self {
        Date(self.0 + 1)
    }

    /// First day of the month after this one.
    pub fn next_month_start(self) -> Self {
        let (y, m, _) = days_to_date(self.0 as u64);
        let (ny, nm) = if m >= 12 { (y + 1, 1) } else { (y, m + 1) };
        Date(civil_to_days(ny as i64, nm as i64, 1) as u32)
    }

    /// January 1st of the year after this one.
    pub fn next_year_start(self) -> Self {
        let (y, _, _) = days_to_date(self.0 as u64);
        Date(civil_to_days(y as i64 + 1, 1, 1) as u32)
    }

    /// Number of days between `self` and `other` (signed).
    pub fn days_until(self, other: Date) -> i64 {
        other.0 as i64 - self.0 as i64
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d) = days_to_date(self.0 as u64);
        write!(f, "{:04}-{:02}-{:02}", y, m, d)
    }
}

// ==================== Legacy string-based helpers ====================
// Used by `update/` (MEXC / OXR fetching) which deals in unix-ms. New
// code should prefer `Date` above.

/// Current wall-clock time as milliseconds since Unix epoch (UTC).
pub fn current_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Convert a `YYYY-MM-DD` date to milliseconds since Unix epoch.
pub fn date_to_ms(date: &str) -> Result<u64, String> {
    let days = date_to_days(date)? as u64;
    Ok(days * 86_400_000)
}

/// Convert milliseconds since Unix epoch to `YYYY-MM-DD` (UTC).
pub fn ms_to_date(ms: u64) -> String {
    let days = ms / 86_400_000;
    let (y, m, d) = days_to_date(days);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// Return the day after `date` as `YYYY-MM-DD`.
pub fn day_after(date: &str) -> Result<String, String> {
    let days = date_to_days(date)? as u64 + 1;
    let (y, m, d) = days_to_date(days);
    Ok(format!("{:04}-{:02}-{:02}", y, m, d))
}

/// Return the first day of the next month after `date`.
pub fn next_month_start(date: &str) -> Result<String, String> {
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return Err(format!("invalid date format: {}", date));
    }
    let y: i64 = parts[0].parse().map_err(|_| format!("invalid year: {}", parts[0]))?;
    let m: i64 = parts[1].parse().map_err(|_| format!("invalid month: {}", parts[1]))?;
    let (ny, nm) = if m >= 12 { (y + 1, 1) } else { (y, m + 1) };
    Ok(format!("{:04}-{:02}-01", ny, nm))
}

/// Return January 1st of the year after `date`.
pub fn next_year_start(date: &str) -> Result<String, String> {
    let parts: Vec<&str> = date.split('-').collect();
    if parts.is_empty() {
        return Err(format!("invalid date format: {}", date));
    }
    let y: i64 = parts[0].parse().map_err(|_| format!("invalid year: {}", parts[0]))?;
    Ok(format!("{:04}-01-01", y + 1))
}

pub fn date_to_days(date: &str) -> Result<u32, String> {
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return Err(format!("invalid date format: {}", date));
    }
    let y: i64 = parts[0].parse().map_err(|_| format!("invalid year: {}", parts[0]))?;
    let m: i64 = parts[1].parse().map_err(|_| format!("invalid month: {}", parts[1]))?;
    let d: i64 = parts[2].parse().map_err(|_| format!("invalid day: {}", parts[2]))?;
    Ok(civil_to_days(y, m, d) as u32)
}

/// Howard Hinnant's days-from-civil algorithm (inverse of days_to_date).
fn civil_to_days(y: i64, m: i64, d: i64) -> u64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = (y - era * 400) as u64; // 0..=399
    let m = m as u64;
    let d = d as u64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    ((era * 146097) as u64 + doe).saturating_sub(719468)
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_date(days: u64) -> (u64, u64, u64) {
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u64, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Date type ----

    #[test]
    fn parse_and_display_roundtrip() {
        let d = Date::parse("2024-06-15").unwrap();
        assert_eq!(d.to_string(), "2024-06-15");
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(Date::parse("not-a-date").is_err());
        assert!(Date::parse("2020/01/01").is_err());
        assert!(Date::parse("2020-01").is_err());
    }

    #[test]
    fn components() {
        let d = Date::parse("2024-06-15").unwrap();
        assert_eq!(d.year(), 2024);
        assert_eq!(d.month(), 6);
        assert_eq!(d.day(), 15);
    }

    #[test]
    fn ordering() {
        let a = Date::parse("2024-01-01").unwrap();
        let b = Date::parse("2024-12-31").unwrap();
        assert!(a < b);
    }

    #[test]
    fn next_day_wraps_months() {
        assert_eq!(Date::parse("2020-02-28").unwrap().next_day().to_string(), "2020-02-29"); // leap
        assert_eq!(Date::parse("2021-02-28").unwrap().next_day().to_string(), "2021-03-01");
        assert_eq!(Date::parse("2020-12-31").unwrap().next_day().to_string(), "2021-01-01");
    }

    #[test]
    fn next_month_start_rollover() {
        assert_eq!(Date::parse("2020-01-15").unwrap().next_month_start().to_string(), "2020-02-01");
        assert_eq!(Date::parse("2020-12-31").unwrap().next_month_start().to_string(), "2021-01-01");
    }

    #[test]
    fn next_year_start_rollover() {
        assert_eq!(Date::parse("2020-06-15").unwrap().next_year_start().to_string(), "2021-01-01");
        assert_eq!(Date::parse("2020-12-31").unwrap().next_year_start().to_string(), "2021-01-01");
    }

    #[test]
    fn days_until() {
        let a = Date::parse("2024-01-01").unwrap();
        let b = Date::parse("2024-01-31").unwrap();
        assert_eq!(a.days_until(b), 30);
        assert_eq!(b.days_until(a), -30);
    }

    #[test]
    fn epoch_is_zero() {
        let d = Date::parse("1970-01-01").unwrap();
        assert_eq!(d.days(), 0);
    }

    // ---- Legacy string helpers ----

    #[test]
    fn test_date_to_ms_unix_epoch() {
        assert_eq!(date_to_ms("1970-01-01").unwrap(), 0);
    }

    #[test]
    fn test_date_to_ms_known_date() {
        assert_eq!(date_to_ms("2020-01-01").unwrap(), 1_577_836_800_000);
    }

    #[test]
    fn test_ms_to_date_roundtrip() {
        assert_eq!(ms_to_date(1_577_836_800_000), "2020-01-01");
        assert_eq!(ms_to_date(0), "1970-01-01");
    }

    #[test]
    fn test_day_after() {
        assert_eq!(day_after("2020-01-01").unwrap(), "2020-01-02");
        assert_eq!(day_after("2020-02-28").unwrap(), "2020-02-29");
        assert_eq!(day_after("2021-02-28").unwrap(), "2021-03-01");
        assert_eq!(day_after("2020-12-31").unwrap(), "2021-01-01");
    }

    #[test]
    fn test_date_to_ms_invalid() {
        assert!(date_to_ms("not-a-date").is_err());
        assert!(date_to_ms("2020/01/01").is_err());
    }

    #[test]
    fn test_next_month_start_str() {
        assert_eq!(next_month_start("2020-01-15").unwrap(), "2020-02-01");
        assert_eq!(next_month_start("2020-12-31").unwrap(), "2021-01-01");
        assert_eq!(next_month_start("2020-02-01").unwrap(), "2020-03-01");
    }

    #[test]
    fn test_next_year_start_str() {
        assert_eq!(next_year_start("2020-06-15").unwrap(), "2021-01-01");
        assert_eq!(next_year_start("2020-01-01").unwrap(), "2021-01-01");
    }
}
