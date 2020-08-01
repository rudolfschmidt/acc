use super::super::super::model::Token;
use super::Tokenizer;

pub(super) fn tokenize(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Ok(()),
		Some(_) => {
			let mut commodity = String::new();
			while let Some(&c) = tokenizer.line_chars.get(tokenizer.line_pos) {
				if c == '-' || c.is_numeric() {
					break;
				}
				if c.is_whitespace() {
					tokenizer.line_pos += 1;
					continue;
				}
				commodity.push(c);
				tokenizer.line_pos += 1;
			}
			if !commodity.is_empty() {
				tokenizer
					.tokens
					.push(Token::PostingCommodity(tokenizer.line_index, commodity));
				return tokenize_amount(tokenizer);
			}
			tokenize_amount_commodity(tokenizer)
		}
	}
}

fn tokenize_amount(tokenizer: &mut Tokenizer) -> Result<(), String> {
	parse_amount(tokenizer)?
		.map(|(c, _)| Err(format!("received \"{}\", but expected number", c)))
		.unwrap_or(Ok(()))
}

fn tokenize_amount_commodity(tokenizer: &mut Tokenizer) -> Result<(), String> {
	parse_amount(tokenizer)?
		.map(|(_, amount)| {
			tokenize_commodity(tokenizer)?;
			tokenizer
				.tokens
				.push(Token::PostingAmount(tokenizer.line_index, amount));
			Ok(())
		})
		.unwrap_or(Ok(()))
}

fn parse_amount(tokenizer: &mut Tokenizer) -> Result<Option<(char, String)>, String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Err(format!("Unexpected end of line")),
		Some(_) => {
			let mut amount = String::new();
			if let Some(&c) = tokenizer.line_chars.get(tokenizer.line_pos) {
				if c == '-' {
					amount.push(c);
					tokenizer.line_pos += 1;
				}
			}
			match tokenizer.line_chars.get(tokenizer.line_pos) {
				None => return Err(format!("Unexpected end of line")),
				Some(c) if !c.is_numeric() => {
					return Err(format!("received \"{}\", but expected number", c))
				}
				Some(&c) => {
					amount.push(c);
					tokenizer.line_pos += 1;
				}
			}
			while let Some(&c) = tokenizer.line_chars.get(tokenizer.line_pos) {
				if !c.is_numeric() && c != '.' {
					break;
				}
				amount.push(c);
				tokenizer.line_pos += 1;
			}
			while let Some(&c) = tokenizer.line_chars.get(tokenizer.line_pos) {
				if c == '=' {
					break;
				} else if c.is_whitespace() {
					tokenizer.line_pos += 1;
					continue;
				} else if !c.is_numeric() {
					return Ok(Some((c, amount)));
				}
				amount.push(c);
				tokenizer.line_pos += 1;
			}
			tokenizer
				.tokens
				.push(Token::PostingAmount(tokenizer.line_index, amount));
			Ok(None)
		}
	}
}

fn tokenize_commodity(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Ok(()),
		Some(_) => {
			let mut commodity = String::new();
			while let Some(&c) = tokenizer.line_chars.get(tokenizer.line_pos) {
				if c == '-' || c.is_numeric() {
					break;
				}
				if c.is_whitespace() {
					tokenizer.line_pos += 1;
					continue;
				}
				commodity.push(c);
				tokenizer.line_pos += 1;
			}
			tokenizer
				.tokens
				.push(Token::PostingCommodity(tokenizer.line_index, commodity));
			Ok(())
		}
	}
}
