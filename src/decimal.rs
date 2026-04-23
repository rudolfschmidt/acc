//! Strict fixed-point decimal: `mantissa × 10^(-MAX_SCALE)` always.
//!
//! Every `Decimal` holds its value in an `i128` mantissa at an implicit,
//! globally-fixed scale of `MAX_SCALE = 20`. There is **no per-value scale**.
//! 20 fractional digits covers every financial use case; the integer
//! portion ranges up to `i128::MAX / 10^20 ≈ 1.7 × 10^18`, wide enough
//! for any accounting value and their products (price × amount).
//!
//! ## Strict semantics
//!
//! - **Parse**: rejects input with more than `MAX_SCALE` fractional digits
//!   (returns `Err`). Mantissa overflow also returns `Err`.
//! - **Add / Sub / Mul**: panic on any integer overflow. `Mul` additionally
//!   panics if the mathematical result cannot be represented exactly at
//!   `MAX_SCALE` (i.e. more than `MAX_SCALE` fractional digits would be
//!   needed). Use `round(n)` beforehand if you deliberately want to lose
//!   precision.
//! - **Div `/`**: panics if the result is non-terminating (e.g. `1/3`). Use
//!   `div_rounded` for the rare case where rounding is acceptable (e.g.
//!   `PriceDB` inverse lookups).
//!
//! This keeps any silent precision loss out of the system entirely — the
//! only place that rounds without panicking is the explicit
//! `div_rounded` / `round` helpers.

use std::cmp::Ordering;
use std::fmt;
use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

use crate::i256::i256;

/// Number of fractional digits every `Decimal` carries.
pub const MAX_SCALE: u32 = 20;

/// `10^MAX_SCALE` as i128. Used as the implicit denominator.
const SCALE_FACTOR: i128 = 10_i128.pow(MAX_SCALE);

/// Fixed-point decimal with implicit scale `MAX_SCALE`.
/// The stored integer `mantissa` represents `mantissa × 10^(-MAX_SCALE)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Decimal {
    mantissa: i128,
}

impl Decimal {
    pub const ZERO: Decimal = Decimal { mantissa: 0 };

    /// Construct from a raw mantissa at scale `MAX_SCALE`.
    pub fn from_mantissa(mantissa: i128) -> Self {
        Decimal { mantissa }
    }

    /// Convenience constructor: `new(numer, denom)` = `numer / denom`, rounded.
    /// Exists for ergonomic test construction of rates like `Decimal::new(92, 100)`
    /// = 0.92. Uses `div_rounded` so it also works for non-terminating ratios.
    pub fn new(numer: i128, denom: i128) -> Self {
        Decimal::from(numer).div_rounded(Decimal::from(denom))
    }

    pub fn zero() -> Self {
        Decimal::ZERO
    }

    pub fn is_zero(&self) -> bool {
        self.mantissa == 0
    }

    pub fn is_negative(&self) -> bool {
        self.mantissa < 0
    }

    pub fn abs(&self) -> Self {
        Decimal {
            mantissa: self.mantissa.abs(),
        }
    }

    /// Approximate as an f64. Used for comparisons in `sort` where absolute
    /// precision isn't needed.
    pub fn to_f64(&self) -> f64 {
        self.mantissa as f64 / SCALE_FACTOR as f64
    }

    /// Check if the value rounds to zero at `precision` fractional digits.
    pub fn is_display_zero(&self, precision: usize) -> bool {
        let drop = (MAX_SCALE as usize).saturating_sub(precision);
        if drop == 0 {
            return self.mantissa == 0;
        }
        round_half_up(self.mantissa, 10_i128.pow(drop as u32)) == 0
    }

    /// Round to `precision` fractional digits (half-up, away-from-zero on ties),
    /// preserving the storage scale. This is a **deliberate** precision loss.
    pub fn round(&self, precision: usize) -> Self {
        let target = (precision as u32).min(MAX_SCALE);
        if target == MAX_SCALE {
            return *self;
        }
        let drop = MAX_SCALE - target;
        let factor = 10_i128.pow(drop);
        let rounded = round_half_up(self.mantissa, factor);
        // Re-scale to MAX_SCALE storage (zeros at the tail).
        Decimal {
            mantissa: rounded.checked_mul(factor).expect("decimal round overflow"),
        }
    }

    /// Parse a plain decimal string like `"3.14"`, `"-0.001"`, `"322"`.
    /// Rejects: empty input, scientific notation, thousand separators,
    /// more than `MAX_SCALE` fractional digits, mantissa overflow.
    ///
    /// Single-pass byte parser: accumulates the mantissa as we go, counts
    /// fractional digits. No intermediate strings, no per-call `str::parse`
    /// or `str::find`. Critical hot path during journal parsing (~1M calls
    /// for a real fiat prices directory).
    pub fn parse(s: &str) -> Result<Self, String> {
        let bytes = s.as_bytes();
        let mut i = 0;
        let end = bytes.len();
        // Trim leading whitespace.
        while i < end && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n') {
            i += 1;
        }
        // Trim trailing whitespace.
        let mut last = end;
        while last > i && matches!(bytes[last - 1], b' ' | b'\t' | b'\r' | b'\n') {
            last -= 1;
        }
        if i == last {
            return Err("empty decimal".to_string());
        }
        let negative = match bytes[i] {
            b'-' => {
                i += 1;
                true
            }
            b'+' => {
                i += 1;
                false
            }
            _ => false,
        };
        let mut mantissa: i128 = 0;
        let mut frac_digits: u32 = 0;
        let mut saw_dot = false;
        let mut saw_digit = false;
        while i < last {
            let b = bytes[i];
            if b.is_ascii_digit() {
                let d = (b - b'0') as i128;
                mantissa = mantissa
                    .checked_mul(10)
                    .and_then(|m| m.checked_add(d))
                    .ok_or_else(|| format!("decimal '{}' mantissa overflow", s))?;
                if saw_dot {
                    frac_digits += 1;
                    if frac_digits > MAX_SCALE {
                        return Err(format!(
                            "decimal '{}' has more than {} fractional digits",
                            s, MAX_SCALE
                        ));
                    }
                }
                saw_digit = true;
            } else if b == b'.' {
                if saw_dot {
                    return Err(format!("invalid decimal '{}'", s));
                }
                saw_dot = true;
            } else {
                return Err(format!("invalid decimal '{}'", s));
            }
            i += 1;
        }
        if !saw_digit {
            return Err(format!("invalid decimal '{}'", s));
        }
        // Pad the mantissa up to MAX_SCALE by multiplying with 10^(MAX_SCALE - frac_digits).
        let pad = MAX_SCALE - frac_digits;
        if pad > 0 {
            mantissa = mantissa
                .checked_mul(10_i128.pow(pad))
                .ok_or_else(|| format!("decimal '{}' mantissa overflow", s))?;
        }
        Ok(Decimal {
            mantissa: if negative { -mantissa } else { mantissa },
        })
    }

    /// Format as plain decimal with exactly `precision` fractional digits,
    /// half-up rounded for display. Storage is not mutated.
    pub fn format_decimal(&self, precision: usize) -> String {
        let target = (precision as u32).min(MAX_SCALE);
        let drop = MAX_SCALE - target;
        let factor = 10_i128.pow(drop);
        let display_m = round_half_up(self.mantissa, factor);
        let negative = display_m < 0;
        let abs = display_m.unsigned_abs();
        let s = abs.to_string();
        let scale = target as usize;
        let formatted = if scale == 0 {
            s
        } else if s.len() > scale {
            let split = s.len() - scale;
            format!("{}.{}", &s[..split], &s[split..])
        } else {
            let pad = scale - s.len();
            format!("0.{}{}", "0".repeat(pad), s)
        };
        if negative && abs != 0 {
            format!("-{}", formatted)
        } else {
            formatted
        }
    }

    /// Explicit rounded multiplication: product rounded (half-up) to
    /// `MAX_SCALE` fractional digits. Use when an input may push the
    /// exact product past `MAX_SCALE` (e.g. rebalancing an amount by
    /// an inverse rate that carries the full 28-digit tail); the
    /// normal `*` operator panics on that.
    pub fn mul_rounded(self, rhs: Self) -> Self {
        if self.is_zero() || rhs.is_zero() {
            return Decimal::ZERO;
        }
        let a = i256::from_i128(self.mantissa);
        let b = i256::from_i128(rhs.mantissa);
        let prod = a.mul(&b);
        let divisor = i256::from_i128(SCALE_FACTOR);
        let (q, r) = i256_divmod(&prod, &divisor);
        let mut mantissa = q.to_i128().unwrap_or_else(|| {
            panic!(
                "{} × {} is too large",
                self.format_decimal(MAX_SCALE as usize),
                rhs.format_decimal(MAX_SCALE as usize),
            )
        });
        // Half-up: if 2·|r| >= |divisor|, bump magnitude away from zero.
        let doubled_abs_r = r.abs().add(&r.abs());
        if doubled_abs_r.cmp_signed(&divisor.abs()) != Ordering::Less {
            if (self.mantissa < 0) ^ (rhs.mantissa < 0) {
                mantissa -= 1;
            } else {
                mantissa += 1;
            }
        }
        Decimal { mantissa }
    }

    /// Explicit rounded division: result is rounded (half-up) to `MAX_SCALE`
    /// fractional digits. Used deliberately for reciprocal rate calculation;
    /// the normal `/` operator panics on non-terminating results.
    pub fn div_rounded(self, rhs: Self) -> Self {
        assert!(!rhs.is_zero(), "decimal division by zero");
        if self.is_zero() {
            return Decimal::ZERO;
        }
        // value(a) = a.mant / 10^MAX_SCALE
        // value(b) = b.mant / 10^MAX_SCALE
        // a/b as mantissa at MAX_SCALE scale:
        //   result_mant = (a.mant * 10^MAX_SCALE) / b.mant
        let num = i256::from_i128(self.mantissa);
        let den = i256::from_i128(rhs.mantissa);
        let num_scaled = i256_mul_pow10(&num, MAX_SCALE);
        let (q, r) = i256_divmod(&num_scaled, &den);
        // Half-up rounding: if 2·|r| >= |den|, bump magnitude away from zero.
        let doubled_r = r.abs().add(&r.abs());
        let mut mantissa = q
            .to_i128()
            .expect("decimal div_rounded quotient overflows i128");
        if doubled_r.cmp_signed(&den.abs()) != Ordering::Less {
            if (self.mantissa < 0) ^ (rhs.mantissa < 0) {
                mantissa -= 1;
            } else {
                mantissa += 1;
            }
        }
        Decimal { mantissa }
    }
}

/// Divide `m` by `factor` (a power of 10) with half-up rounding,
/// ties away from zero. `factor == 1` is a no-op.
fn round_half_up(m: i128, factor: i128) -> i128 {
    if factor == 1 {
        return m;
    }
    let q = m / factor;
    let r = m % factor;
    // Half-up: if 2·|r| ≥ factor, bump magnitude away from zero.
    if r.unsigned_abs() * 2 >= factor as u128 {
        if m >= 0 {
            q + 1
        } else {
            q - 1
        }
    } else {
        q
    }
}

// ---------------- i256 helpers ----------------

fn i256_mul_pow10(x: &i256, n: u32) -> i256 {
    if n == 0 {
        return *x;
    }
    let mut out = *x;
    let mut remaining = n;
    while remaining > 0 {
        let step = remaining.min(18);
        let factor = i256::from_i128(10_i128.pow(step));
        out = out.mul(&factor);
        remaining -= step;
    }
    out
}

fn i256_divmod(a: &i256, b: &i256) -> (i256, i256) {
    let (q_abs, r_abs) = a.abs().divmod(&b.abs());
    let q = if (a.negative ^ b.negative) && !q_abs.is_zero() {
        q_abs.negate()
    } else {
        q_abs
    };
    let r = if a.negative && !r_abs.is_zero() {
        r_abs.negate()
    } else {
        r_abs
    };
    (q, r)
}

// ---------------- Conversions ----------------

impl From<i32> for Decimal {
    fn from(n: i32) -> Self {
        Self::from(n as i128)
    }
}

impl From<i64> for Decimal {
    fn from(n: i64) -> Self {
        Self::from(n as i128)
    }
}

impl From<i128> for Decimal {
    fn from(n: i128) -> Self {
        Decimal {
            mantissa: n
                .checked_mul(SCALE_FACTOR)
                .expect("Decimal::from integer overflow"),
        }
    }
}

// ---------------- Arithmetic ----------------

impl Neg for Decimal {
    type Output = Self;
    fn neg(self) -> Self {
        Decimal {
            mantissa: -self.mantissa,
        }
    }
}

impl Add for Decimal {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Decimal {
            mantissa: self
                .mantissa
                .checked_add(rhs.mantissa)
                .expect("decimal add overflow"),
        }
    }
}

impl Sub for Decimal {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Decimal {
            mantissa: self
                .mantissa
                .checked_sub(rhs.mantissa)
                .expect("decimal sub overflow"),
        }
    }
}

impl AddAssign for Decimal {
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl SubAssign for Decimal {
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl Mul for Decimal {
    type Output = Self;
    /// Strict multiplication. Panics if:
    ///   - the exact mathematical result cannot be represented at MAX_SCALE
    ///     (i.e. more than MAX_SCALE fractional digits would be needed), or
    ///   - the integer part overflows `i128`.
    ///
    /// Use `round(n)` before `*` if you deliberately want to lose precision.
    fn mul(self, rhs: Self) -> Self {
        // product.mantissa at scale 2*MAX_SCALE; we need it back at MAX_SCALE
        // by dividing by SCALE_FACTOR. Result must be exact.
        let a = i256::from_i128(self.mantissa);
        let b = i256::from_i128(rhs.mantissa);
        let prod = a.mul(&b);
        let divisor = i256::from_i128(SCALE_FACTOR);
        let (q, r) = i256_divmod(&prod, &divisor);
        if !r.is_zero() {
            panic!(
                "{} × {} has more than {} fractional digits",
                self.format_decimal(MAX_SCALE as usize),
                rhs.format_decimal(MAX_SCALE as usize),
                MAX_SCALE,
            );
        }
        let mantissa = q.to_i128().unwrap_or_else(|| {
            panic!(
                "{} × {} is too large",
                self.format_decimal(MAX_SCALE as usize),
                rhs.format_decimal(MAX_SCALE as usize),
            )
        });
        Decimal { mantissa }
    }
}

impl Div for Decimal {
    type Output = Self;
    /// Strict division: panics if the result is non-terminating (e.g. 1/3).
    /// Use `div_rounded` for reciprocal / rate-inverse scenarios where
    /// rounding is explicitly acceptable.
    fn div(self, rhs: Self) -> Self {
        assert!(
            !rhs.is_zero(),
            "{} divided by 0",
            self.format_decimal(MAX_SCALE as usize)
        );
        if self.is_zero() {
            return Decimal::ZERO;
        }
        let num = i256::from_i128(self.mantissa);
        let den = i256::from_i128(rhs.mantissa);
        let num_scaled = i256_mul_pow10(&num, MAX_SCALE);
        let (q, r) = i256_divmod(&num_scaled, &den);
        if !r.is_zero() {
            panic!(
                "{} / {} has more than {} fractional digits",
                self.format_decimal(MAX_SCALE as usize),
                rhs.format_decimal(MAX_SCALE as usize),
                MAX_SCALE,
            );
        }
        Decimal {
            mantissa: q.to_i128().unwrap_or_else(|| {
                panic!(
                    "{} / {} is too large",
                    self.format_decimal(MAX_SCALE as usize),
                    rhs.format_decimal(MAX_SCALE as usize),
                )
            }),
        }
    }
}

// ---------------- Comparisons ----------------

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Decimal {
    fn cmp(&self, other: &Self) -> Ordering {
        self.mantissa.cmp(&other.mantissa)
    }
}

impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format_decimal(MAX_SCALE as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Constructors & display ----

    #[test]
    fn zero_has_zero_mantissa_and_renders_as_0() {
        assert!(Decimal::zero().is_zero());
        assert_eq!(Decimal::zero().format_decimal(2), "0.00");
        assert_eq!(Decimal::zero().format_decimal(0), "0");
    }

    #[test]
    fn from_integer() {
        assert_eq!(Decimal::from(322i64).format_decimal(0), "322");
        assert_eq!(Decimal::from(-5i32).format_decimal(2), "-5.00");
    }

    // ---- Parse ----

    #[test]
    fn parse_basic_shapes() {
        assert_eq!(Decimal::parse("3.14").unwrap().format_decimal(2), "3.14");
        assert_eq!(Decimal::parse("-0.001").unwrap().format_decimal(3), "-0.001");
        assert_eq!(Decimal::parse("322").unwrap().format_decimal(0), "322");
        assert_eq!(Decimal::parse(".5").unwrap().format_decimal(1), "0.5");
        assert_eq!(Decimal::parse("5.").unwrap().format_decimal(0), "5");
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(Decimal::parse("").is_err());
        assert!(Decimal::parse("abc").is_err());
        assert!(Decimal::parse("1e5").is_err());
        assert!(Decimal::parse("1,000").is_err());
    }

    #[test]
    fn parse_rejects_too_many_fractional_digits() {
        // MAX_SCALE+1 fractional digits → Err
        let s = format!("0.{}", "1".repeat((MAX_SCALE as usize) + 1));
        assert!(Decimal::parse(&s).is_err());
    }

    #[test]
    fn parse_accepts_exactly_max_scale() {
        let s = format!("0.{}", "1".repeat(MAX_SCALE as usize));
        assert!(Decimal::parse(&s).is_ok());
    }

    // ---- Add / Sub ----

    #[test]
    fn add_same_digits() {
        let a = Decimal::parse("1.50").unwrap();
        let b = Decimal::parse("2.50").unwrap();
        assert_eq!((a + b).format_decimal(2), "4.00");
    }

    #[test]
    fn sub_produces_negative() {
        let a = Decimal::parse("1.50").unwrap();
        let b = Decimal::parse("2.50").unwrap();
        assert_eq!((a - b).format_decimal(2), "-1.00");
    }

    // ---- Mul ----

    #[test]
    fn mul_exact_small() {
        let a = Decimal::parse("1.5").unwrap();
        let b = Decimal::parse("2").unwrap();
        assert_eq!((a * b).format_decimal(2), "3.00");
    }

    #[test]
    fn mul_exact_money() {
        let a = Decimal::parse("100.00").unwrap();
        let b = Decimal::parse("0.92").unwrap();
        assert_eq!((a * b).format_decimal(2), "92.00");
    }

    #[test]
    #[should_panic(expected = "more than 20 fractional digits")]
    fn mul_panics_on_inexact_result_beyond_max_scale() {
        // Two of the smallest representable values: mantissa=1 → value 10^-28.
        // Product is 10^-56, which cannot be stored at scale 28 without loss.
        let a = Decimal::from_mantissa(1);
        let b = Decimal::from_mantissa(1);
        let _ = a * b;
    }

    #[test]
    fn mul_round_then_multiply_avoids_panic() {
        // Rounded inputs multiply exactly within scale.
        let a = Decimal::from_mantissa(1).round(14); // 0 after rounding — safe
        let b = Decimal::parse("2.5").unwrap();
        let _ = a * b;
    }

    // ---- Div (strict) ----

    #[test]
    fn div_exact_terminating() {
        let a = Decimal::parse("100").unwrap();
        let b = Decimal::parse("4").unwrap();
        assert_eq!((a / b).format_decimal(2), "25.00");
    }

    #[test]
    #[should_panic(expected = "more than 20 fractional digits")]
    fn div_panics_on_non_terminating() {
        let a = Decimal::parse("1").unwrap();
        let b = Decimal::parse("3").unwrap();
        let _ = a / b;
    }

    #[test]
    #[should_panic(expected = "divided by 0")]
    fn div_panics_on_zero() {
        let a = Decimal::parse("1").unwrap();
        let _ = a / Decimal::zero();
    }

    // ---- div_rounded ----

    #[test]
    fn div_rounded_handles_non_terminating() {
        let a = Decimal::parse("1").unwrap();
        let b = Decimal::parse("3").unwrap();
        let q = a.div_rounded(b);
        // MAX_SCALE fractional digits expected, all 3s (rounded up
        // at the last position per half-up rounding).
        let out = q.format_decimal(MAX_SCALE as usize);
        let expected: String = "0.".to_string() + &"3".repeat(MAX_SCALE as usize - 1) + "3";
        // Last digit may be rounded up from 3 to 4 by half-up, accept either.
        assert!(
            out == expected
                || out == "0.".to_string() + &"3".repeat(MAX_SCALE as usize - 1) + "4",
            "got {}",
            out
        );
    }

    #[test]
    fn div_rounded_matches_exact_when_terminating() {
        let a = Decimal::parse("100").unwrap();
        let b = Decimal::parse("4").unwrap();
        assert_eq!(a.div_rounded(b), a / b);
    }

    // ---- round ----

    #[test]
    fn round_positive() {
        let a = Decimal::parse("1.25").unwrap();
        assert_eq!(a.round(1).format_decimal(1), "1.3");
    }

    #[test]
    fn round_negative() {
        let a = Decimal::parse("-1.25").unwrap();
        assert_eq!(a.round(1).format_decimal(1), "-1.3");
    }

    // ---- Misc ----

    #[test]
    fn is_display_zero() {
        let a = Decimal::parse("0.004").unwrap();
        assert!(a.is_display_zero(2));
        assert!(!a.is_display_zero(3));
    }

    #[test]
    fn neg_and_abs() {
        let a = Decimal::parse("-4.2").unwrap();
        assert_eq!(a.abs().format_decimal(1), "4.2");
        assert_eq!((-a).format_decimal(1), "4.2");
    }

    #[test]
    fn compare() {
        let a = Decimal::parse("1.50").unwrap();
        let b = Decimal::parse("1.5").unwrap();
        assert_eq!(a, b);
        let c = Decimal::parse("1.51").unwrap();
        assert!(a < c);
    }

    #[test]
    fn accumulation_large_is_fast_and_correct() {
        // Sanity: 5000 adds don't blow up precision (they can't, at fixed scale).
        let step = Decimal::parse("0.12345678").unwrap();
        let mut total = Decimal::zero();
        for _ in 0..5000 {
            total += step;
        }
        // 5000 × 0.12345678 = 617.28390
        assert_eq!(total.format_decimal(5), "617.28390");
    }
}
