//! `-p` / `-b` / `-e` — the date-range vocabulary every command speaks.
//!
//! One period string names a whole span and expands to a half-open
//! range, so `2024` is every day from `2024-01-01` up to but not
//! including `2025-01-01`. `-b` and `-e` name the same kinds of span and
//! take its START, which makes `-e 2025` and `-e 2025-01-01` the same
//! bound and keeps the end exclusive throughout.
//!
//! The reports filter transactions with it, `update` fetches rates for
//! it. Both resolve it here, so a period means the same thing wherever
//! it is written.

/// Expand a period string into half-open `(begin, end)` bounds.
/// Accepts `YYYY`, `YYYY-MM`, or `YYYY-MM-DD`.
///
/// - `2024`       → `2024-01-01` .. `2025-01-01`
/// - `2024-12`    → `2024-12-01` .. `2025-01-01`
/// - `2024-12-06` → `2024-12-06` .. `2024-12-07`
pub fn expand(s: &str) -> Result<(String, String), String> {
    let parts: Vec<&str> = s.split('-').collect();
    match parts.as_slice() {
        [y] => {
            let year: i32 = y.parse().map_err(|_| format!("invalid year: `{}`", s))?;
            Ok((format!("{:04}-01-01", year), format!("{:04}-01-01", year + 1)))
        }
        [y, m] => {
            let year: i32 = y.parse().map_err(|_| format!("invalid year: `{}`", s))?;
            let month: u32 = m.parse().map_err(|_| format!("invalid month: `{}`", s))?;
            if !(1..=12).contains(&month) {
                return Err(format!("month out of range: `{}`", s));
            }
            let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
            Ok((
                format!("{:04}-{:02}-01", year, month),
                format!("{:04}-{:02}-01", ny, nm),
            ))
        }
        [_, _, _] => {
            // Single date: use it as begin, begin+1day as end.
            let date = crate::date::Date::parse(s).map_err(|e| e.to_string())?;
            let next = crate::date::Date::from_days(date.days() + 1);
            Ok((date.to_string(), next.to_string()))
        }
        _ => Err(format!("invalid period: `{}` (expected YYYY, YYYY-MM, or YYYY-MM-DD)", s)),
    }
}

/// The bounds one run works on.
pub struct Range {
    /// Every `-p` expanded, in the order given. A report keeps them
    /// apart and matches against their union; one alone also sets
    /// `begin`/`end`.
    pub periods: Vec<(String, String)>,
    /// Inclusive lower bound.
    pub begin: Option<String>,
    /// Exclusive upper bound.
    pub end: Option<String>,
}

/// Resolve `-b` / `-e` / `-p` into one range. An explicit `-b`/`-e`
/// wins over a period, which is why clap already rejects the two
/// together — the precedence here only decides the leftover case of a
/// period that expands to nothing.
pub fn resolve(
    begin: Option<&str>,
    end: Option<&str>,
    periods: &[String],
) -> Result<Range, String> {
    let periods: Vec<(String, String)> =
        periods.iter().map(|p| expand(p)).collect::<Result<_, _>>()?;
    // Several periods are a union, not a span, so they leave the plain
    // bounds empty and only the report's multi-period filter sees them.
    let (period_begin, period_end) = match periods.as_slice() {
        [only] => (Some(only.0.clone()), Some(only.1.clone())),
        _ => (None, None),
    };
    let explicit_begin = begin.map(expand).transpose()?.map(|(start, _)| start);
    let explicit_end = end.map(expand).transpose()?.map(|(start, _)| start);
    Ok(Range {
        periods,
        begin: explicit_begin.or(period_begin),
        end: explicit_end.or(period_end),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_period_covers_its_whole_span() {
        assert_eq!(expand("2024").expect("year"), ("2024-01-01".into(), "2025-01-01".into()));
        assert_eq!(expand("2024-12").expect("month"), ("2024-12-01".into(), "2025-01-01".into()));
        assert_eq!(expand("2024-12-06").expect("day"), ("2024-12-06".into(), "2024-12-07".into()));
    }

    #[test]
    fn malformed_periods_name_what_was_expected() {
        assert!(expand("24x").is_err());
        assert!(expand("2024-13").is_err());
        assert!(expand("2024-01-02-03").is_err());
    }

    #[test]
    fn bounds_take_the_start_of_their_period() {
        // `-e 2025` and `-e 2025-01-01` are the same exclusive bound.
        let r = resolve(Some("2024"), Some("2025"), &[]).expect("bounds");
        assert_eq!(r.begin.as_deref(), Some("2024-01-01"));
        assert_eq!(r.end.as_deref(), Some("2025-01-01"));
    }

    #[test]
    fn one_period_sets_both_bounds() {
        let r = resolve(None, None, &["2024-02".to_string()]).expect("period");
        assert_eq!(r.begin.as_deref(), Some("2024-02-01"));
        assert_eq!(r.end.as_deref(), Some("2024-03-01"));
    }

    #[test]
    fn several_periods_stay_a_union() {
        // A span would have to swallow the gap between them, so the
        // plain bounds stay empty and the periods travel on their own.
        let r = resolve(None, None, &["2022".to_string(), "2024".to_string()]).expect("periods");
        assert_eq!(r.periods.len(), 2);
        assert!(r.begin.is_none() && r.end.is_none());
    }
}
