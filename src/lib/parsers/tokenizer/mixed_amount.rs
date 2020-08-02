use super::super::super::model::Token;
use super::Tokenizer;

pub(super) fn tokenize(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.chars.get(tokenizer.pos) {
		None => Ok(()),
		_ => {
			let mut commodity = String::new();
			while let Some(&c) = tokenizer.chars.get(tokenizer.pos) {
				if c == '-' || c.is_numeric() {
					break;
				}
				if c.is_whitespace() {
					tokenizer.pos += 1;
					continue;
				}
				commodity.push(c);
				tokenizer.pos += 1;
			}
			if !commodity.is_empty() {
				tokenizer
					.tokens
					.push(Token::PostingCommodity(tokenizer.index, commodity));
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
				.push(Token::PostingAmount(tokenizer.index, amount));
			Ok(())
		})
		.unwrap_or(Ok(()))
}

fn parse_amount(tokenizer: &mut Tokenizer) -> Result<Option<(char, String)>, String> {
	match tokenizer.chars.get(tokenizer.pos) {
		None => Err(String::from("Unexpected end of line")),
		Some(_) => {
			let mut amount = String::new();

			if let Some('-') = tokenizer.chars.get(tokenizer.pos) {
				amount.push('-');
				tokenizer.pos += 1;
			}

			match tokenizer.chars.get(tokenizer.pos) {
				None => return Err(String::from("Unexpected end of line")),
				Some(c) if !c.is_numeric() => {
					return Err(format!("received \"{}\", but expected number", c))
				}
				Some(&c) => {
					amount.push(c);
					tokenizer.pos += 1;
				}
			}

			while let Some(&c) = tokenizer.chars.get(tokenizer.pos) {
				if !c.is_numeric() && c != '.' {
					break;
				}
				amount.push(c);
				tokenizer.pos += 1;
			}

			while let Some(&c) = tokenizer.chars.get(tokenizer.pos) {
				if c == '=' {
					break;
				} else if c.is_whitespace() {
					tokenizer.pos += 1;
					continue;
				} else if !c.is_numeric() {
					return Ok(Some((c, amount)));
				}
				amount.push(c);
				tokenizer.pos += 1;
			}

			tokenizer
				.tokens
				.push(Token::PostingAmount(tokenizer.index, amount));

			Ok(None)
		}
	}
}

fn tokenize_commodity(tokenizer: &mut Tokenizer) -> Result<(), String> {
	if tokenizer.chars.get(tokenizer.pos).is_some() {
		let mut commodity = String::new();
		while let Some(&c) = tokenizer.chars.get(tokenizer.pos) {
			if c == '-' || c.is_numeric() {
				break;
			}
			if c.is_whitespace() {
				tokenizer.pos += 1;
				continue;
			}
			commodity.push(c);
			tokenizer.pos += 1;
		}
		tokenizer
			.tokens
			.push(Token::PostingCommodity(tokenizer.index, commodity));
	}
	Ok(())
}
