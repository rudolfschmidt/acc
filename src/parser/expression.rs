//! Ledger-style valuation expressions — parse amounts written as
//! `(COMMODITY EXPR)` or `(EXPR COMMODITY)`.
//!
//! The parser is a tiny recursive-descent evaluator for `+ - * /` with
//! parenthesised sub-expressions and unary minus. Commodity symbols
//! are stripped out before arithmetic so `(€11784.00 / 12)` works the
//! same as `(11784.00 / 12)`; the commodity then rides along as the
//! result's commodity label.
//!
//! Multiplication and division use `Decimal::mul_rounded` /
//! `div_rounded` so expressions like `(1 / 3)` don't overflow the
//! decimal's scale limit — the result is rounded at the Decimal's
//! own MAX_SCALE.

use crate::decimal::Decimal;

/// Parse a valuation expression starting with `(`. Consumes through
/// the matching `)` and returns the extracted commodity, the computed
/// value, and the number of bytes consumed including the closing
/// paren. The input must begin with `(` — callers check this first.
pub fn parse(text: &str) -> Result<(String, Decimal, usize), String> {
    let close = find_close(text)?;
    let inner = &text[1..close];
    let (commodity, math) = split_commodity(inner)?;
    if math.is_empty() {
        return Err(format!("empty expression: `{}`", inner));
    }
    let value = Expr::new(&math).eval()?;
    Ok((commodity, value, close + 1))
}

/// Count the maximum number of digits after any decimal point in the
/// expression source. Kept for potential future use (e.g. a flag to
/// preserve source precision for expressions), but not used by the
/// current parser — expression amounts default to `decimals = 0` so
/// that the commodity's display precision is driven by direct
/// postings rather than intermediate computation terms.
#[allow(dead_code)]
pub fn max_decimals(text: &str) -> usize {
    let mut max = 0;
    let mut count: Option<usize> = None;
    for c in text.chars() {
        match c {
            '.' => count = Some(0),
            _ if c.is_ascii_digit() => {
                if let Some(n) = count.as_mut() {
                    *n += 1;
                    if *n > max {
                        max = *n;
                    }
                }
            }
            _ => count = None,
        }
    }
    max
}

// ---------------------------------------------------------------------
// matching paren + commodity/math split
// ---------------------------------------------------------------------

/// Index of the `)` that matches the opening `(` at position 0.
fn find_close(text: &str) -> Result<usize, String> {
    if !text.starts_with('(') {
        return Err("expression must start with `(`".into());
    }
    let mut depth = 0usize;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
            }
            _ => {}
        }
    }
    Err("unclosed `(` in expression".into())
}

/// Separate commodity characters from math characters. Commodities
/// are collected as a list of whitespace-/math-delimited runs; having
/// more than one distinct commodity inside a single expression is a
/// hard error (`1 EUR + 1 USD` makes no numeric sense). Zero
/// commodities is fine — the result carries an empty commodity.
fn split_commodity(text: &str) -> Result<(String, String), String> {
    let mut commodities: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut math = String::new();

    let flush = |current: &mut String, commodities: &mut Vec<String>| {
        if !current.is_empty() {
            commodities.push(std::mem::take(current));
        }
    };

    for c in text.chars() {
        if is_math_char(c) {
            flush(&mut current, &mut commodities);
            math.push(c);
        } else if c.is_whitespace() {
            flush(&mut current, &mut commodities);
            if !math.is_empty() && !commodities.is_empty() {
                math.push(c);
            }
        } else {
            current.push(c);
        }
    }
    flush(&mut current, &mut commodities);

    match commodities.len() {
        0 => Ok((String::new(), math.trim().to_string())),
        1 => Ok((commodities.remove(0), math.trim().to_string())),
        _ => Err(format!(
            "expression may contain at most one commodity, found {}",
            commodities.join(", "),
        )),
    }
}

fn is_math_char(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '.' | '+' | '-' | '*' | '/' | '(' | ')')
}

// ---------------------------------------------------------------------
// recursive-descent evaluator
// ---------------------------------------------------------------------

/// Grammar:
///
/// ```text
/// expr    := term   (('+' | '-') term)*
/// term    := factor (('*' | '/') factor)*
/// factor  := '-' factor | primary
/// primary := number | '(' expr ')'
/// number  := digit+ ('.' digit+)?
/// ```
struct Expr<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Expr<'a> {
    fn new(s: &'a str) -> Self {
        Self { src: s.as_bytes(), pos: 0 }
    }

    fn eval(mut self) -> Result<Decimal, String> {
        let v = self.parse_expr()?;
        self.skip_ws();
        if self.pos < self.src.len() {
            return Err(format!(
                "unexpected `{}` in expression",
                self.src[self.pos] as char
            ));
        }
        Ok(v)
    }

    fn parse_expr(&mut self) -> Result<Decimal, String> {
        let mut left = self.parse_term()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'+') => {
                    self.pos += 1;
                    left = left + self.parse_term()?;
                }
                Some(b'-') => {
                    self.pos += 1;
                    left = left - self.parse_term()?;
                }
                _ => return Ok(left),
            }
        }
    }

    fn parse_term(&mut self) -> Result<Decimal, String> {
        let mut left = self.parse_factor()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'*') => {
                    self.pos += 1;
                    left = left.mul_rounded(self.parse_factor()?);
                }
                Some(b'/') => {
                    self.pos += 1;
                    let right = self.parse_factor()?;
                    if right.is_zero() {
                        return Err("division by zero".into());
                    }
                    left = left.div_rounded(right);
                }
                _ => return Ok(left),
            }
        }
    }

    fn parse_factor(&mut self) -> Result<Decimal, String> {
        self.skip_ws();
        match self.peek() {
            Some(b'-') => {
                self.pos += 1;
                Ok(-self.parse_factor()?)
            }
            Some(b'(') => {
                self.pos += 1;
                let v = self.parse_expr()?;
                self.skip_ws();
                if self.peek() != Some(b')') {
                    return Err("expected `)`".into());
                }
                self.pos += 1;
                Ok(v)
            }
            _ => self.parse_number(),
        }
    }

    fn parse_number(&mut self) -> Result<Decimal, String> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_digit() || b == b'.' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            return Err("expected number".into());
        }
        let s = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        Decimal::parse(s).map_err(|e| format!("invalid number `{}`: {}", s, e))
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b) if b.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval(s: &str) -> Decimal {
        let (_, v, _) = parse(s).unwrap();
        v
    }

    #[test]
    fn simple_addition() {
        assert_eq!(eval("(1 + 2)"), Decimal::from(3));
    }

    #[test]
    fn precedence_mul_over_add() {
        assert_eq!(eval("(1 + 2 * 3)"), Decimal::from(7));
    }

    #[test]
    fn nested_parens_override_precedence() {
        assert_eq!(eval("((1 + 2) * 3)"), Decimal::from(9));
    }

    #[test]
    fn unary_minus() {
        assert_eq!(eval("(-5 + 3)"), Decimal::from(-2));
    }

    #[test]
    fn commodity_before_expression() {
        let (commodity, value, _) = parse("(€11784.00 / 12)").unwrap();
        assert_eq!(commodity, "€");
        assert_eq!(value, Decimal::parse("982").unwrap());
    }

    #[test]
    fn commodity_after_expression() {
        let (commodity, value, _) = parse("(100 EUR / 4)").unwrap();
        assert_eq!(commodity, "EUR");
        assert_eq!(value, Decimal::from(25));
    }

    #[test]
    fn no_commodity() {
        let (commodity, value, _) = parse("(100 / 4)").unwrap();
        assert!(commodity.is_empty());
        assert_eq!(value, Decimal::from(25));
    }

    #[test]
    fn non_terminating_division_rounds() {
        // 1/3 should not panic or overflow — it rounds at MAX_SCALE.
        let (_, value, _) = parse("(1 / 3)").unwrap();
        assert_eq!(value, Decimal::from(1).div_rounded(Decimal::from(3)));
    }

    #[test]
    fn division_by_zero_errors() {
        assert!(parse("(5 / 0)").is_err());
    }

    #[test]
    fn unclosed_paren_errors() {
        assert!(parse("(5 + 3").is_err());
    }

    #[test]
    fn empty_expression_errors() {
        assert!(parse("()").is_err());
    }

    #[test]
    fn bytes_consumed_includes_close_paren() {
        let (_, _, n) = parse("(1 + 2) trailing").unwrap();
        assert_eq!(n, "(1 + 2)".len());
    }

    #[test]
    fn two_commodities_error() {
        let err = parse("(1 EUR + 1 USD)").unwrap_err();
        assert!(err.contains("at most one commodity"));
    }

    #[test]
    fn max_decimals_from_source() {
        assert_eq!(max_decimals("11784.00 / 12"), 2);
        assert_eq!(max_decimals("10.5000 / 2"), 4);
        assert_eq!(max_decimals("100 / 4"), 0);
    }
}
