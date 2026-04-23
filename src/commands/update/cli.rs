use crate::error::Error;

/// Parsed "BASE/QUOTE" trading pair, e.g. "BTC/USDT" → ("BTC", "USDT").
pub struct Pair {
    pub base: String,
    pub quote: String,
}

impl Pair {
    pub fn display(&self) -> String {
        format!("{}/{}", self.base, self.quote)
    }
}

pub fn parse_pair(s: &str) -> Result<Pair, Error> {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() != 2 {
        return Err(Error::new(format!(
            "invalid pair '{}': expected BASE/QUOTE (e.g. BTC/USDT)",
            s
        )));
    }
    let base = parts[0].trim();
    let quote = parts[1].trim();
    if base.is_empty() || quote.is_empty() {
        return Err(Error::new(format!(
            "invalid pair '{}': base or quote empty",
            s
        )));
    }
    Ok(Pair {
        base: base.to_string(),
        quote: quote.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_happy() {
        let p = parse_pair("BTC/USDT").unwrap();
        assert_eq!(p.base, "BTC");
        assert_eq!(p.quote, "USDT");
        assert_eq!(p.display(), "BTC/USDT");
    }

    #[test]
    fn test_parse_malformed() {
        assert!(parse_pair("BTC").is_err());
        assert!(parse_pair("BTC/").is_err());
        assert!(parse_pair("/USDT").is_err());
        assert!(parse_pair("BTC/USDT/FOO").is_err());
        assert!(parse_pair("").is_err());
    }

    #[test]
    fn test_parse_trims() {
        let p = parse_pair("  BTC / USDT  ").unwrap();
        assert_eq!(p.base, "BTC");
        assert_eq!(p.quote, "USDT");
    }
}
