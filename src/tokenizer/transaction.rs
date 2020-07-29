use super::super::model::State;
use super::super::model::Token;
use super::Tokenizer;

pub(super) fn tokenize(tokenizer: &mut Tokenizer) -> Result<(), String> {
	tokenize_date(tokenizer)
}

fn tokenize_date(tokenizer: &mut Tokenizer) -> Result<(), String> {
	if let Some(&c) = tokenizer.line_chars.get(tokenizer.line_pos) {
		if c.is_numeric() {
			let mut value = String::new();

			parse_number(tokenizer, &mut value)?;
			parse_number(tokenizer, &mut value)?;
			parse_number(tokenizer, &mut value)?;
			parse_number(tokenizer, &mut value)?;
			parse_dash(tokenizer, &mut value)?;
			parse_number(tokenizer, &mut value)?;
			parse_number(tokenizer, &mut value)?;
			parse_dash(tokenizer, &mut value)?;
			parse_number(tokenizer, &mut value)?;
			parse_number(tokenizer, &mut value)?;
			expect_whitespace(tokenizer)?;

			tokenizer
				.ledger
				.tokens
				.push(Token::TransactionDate(tokenizer.line_index, value));

			toknize_transaction_state(tokenizer)?;
			toknize_transaction_code(tokenizer)?;
			toknize_transaction_description(tokenizer)?;
		}
	}
	Ok(())
}

fn parse_number(tokenizer: &mut Tokenizer, value: &mut String) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Err(format!("Unexpected end of line. Expected number instead")),
		Some(c) => {
			if !c.is_numeric() {
				Err(format!(
					"Unexpected character \"{}\". Expected number instead",
					c
				))
			} else {
				value.push(*c);
				tokenizer.line_pos += 1;
				Ok(())
			}
		}
	}
}

fn parse_dash(tokenizer: &mut Tokenizer, value: &mut String) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Err(format!("Unexpected end of line. Expected \"-\" instead")),
		Some(c) => {
			if '-' != *c {
				Err(format!(
					"Unexpected character \"{}\". Expected \"-\" instead",
					c
				))
			} else {
				value.push(*c);
				tokenizer.line_pos += 1;
				Ok(())
			}
		}
	}
}

fn expect_whitespace(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Err(format!("Unexpected end of line. Expected \"-\" instead")),
		Some(&c) => {
			if !c.is_whitespace() {
				return Err(format!(
					"Unexpected character \"{}\". Expected \"-\" instead",
					c
				));
			}
			tokenizer.line_pos += 1;
			Ok(())
		}
	}
}

fn toknize_transaction_state(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Err(format!("Unexpected end of line")),
		Some(&c) => {
			tokenizer.consume_whitespaces();
			match c {
				'*' => {
					let state = Token::TransactionState(tokenizer.line_index, State::Cleared);
					tokenizer.ledger.tokens.push(state);
					tokenizer.line_pos += 1;
				}
				'!' => {
					let state = Token::TransactionState(tokenizer.line_index, State::Pending);
					tokenizer.ledger.tokens.push(state);
					tokenizer.line_pos += 1;
				}
				_ => {
					let state = Token::TransactionState(tokenizer.line_index, State::Uncleared);
					tokenizer.ledger.tokens.push(state);
				}
			}
			Ok(())
		}
	}
}

fn toknize_transaction_code(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Ok(()),
		Some(&c) => {
			tokenizer.consume_whitespaces();
			if c == '(' {
				tokenizer.line_pos += 1;

				let mut value = String::new();

				match tokenizer.line_chars.get(tokenizer.line_pos) {
					None => {
						return Err(String::from(""));
					}
					Some(&c) => {
						value.push(c);
						tokenizer.line_pos += 1;
					}
				}

				while let Some(&c) = tokenizer.line_chars.get(tokenizer.line_pos) {
					if c == ')' {
						tokenizer.line_pos += 1;
						break;
					}
					value.push(c);
					tokenizer.line_pos += 1;
				}

				tokenizer
					.ledger
					.tokens
					.push(Token::TransactionCode(tokenizer.line_index, value));
			}
			Ok(())
		}
	}
}

fn toknize_transaction_description(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Err(format!("Unexpected end of line")),
		Some(_) => {
			tokenizer.consume_whitespaces();
			let mut value = String::new();
			while let Some(&c) = tokenizer.line_chars.get(tokenizer.line_pos) {
				value.push(c);
				tokenizer.line_pos += 1;
			}
			tokenizer
				.ledger
				.tokens
				.push(Token::TransactionDescription(tokenizer.line_index, value));
			Ok(())
		}
	}
}
