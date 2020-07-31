use super::super::super::model::State;
use super::super::super::model::Token;
use super::chars;
use super::Tokenizer;

pub(super) fn tokenize(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Ok(()),
		Some(c) if c.is_numeric() => tokenize_date(tokenizer),
		Some(_) => Ok(()),
	}
}

fn tokenize_date(tokenizer: &mut Tokenizer) -> Result<(), String> {
	let mut year = String::new();
	let mut month = String::new();
	let mut day = String::new();

	chars::parse_numeric(tokenizer, &mut year)?;
	chars::parse_numeric(tokenizer, &mut year)?;
	chars::parse_numeric(tokenizer, &mut year)?;
	chars::parse_numeric(tokenizer, &mut year)?;

	if chars::is_char(tokenizer, '-') {
		chars::expect_char(tokenizer, '-')?;
		chars::parse_numeric(tokenizer, &mut month)?;
		chars::parse_numeric(tokenizer, &mut month)?;
		chars::expect_char(tokenizer, '-')?;
		chars::parse_numeric(tokenizer, &mut day)?;
		chars::parse_numeric(tokenizer, &mut day)?;
	}

	if chars::is_char(tokenizer, '/') {
		chars::expect_char(tokenizer, '/')?;
		chars::parse_numeric(tokenizer, &mut month)?;
		chars::parse_numeric(tokenizer, &mut month)?;
		chars::expect_char(tokenizer, '/')?;
		chars::parse_numeric(tokenizer, &mut day)?;
		chars::parse_numeric(tokenizer, &mut day)?;
	}

	if chars::is_char(tokenizer, '.') {
		chars::expect_char(tokenizer, '.')?;
		chars::parse_numeric(tokenizer, &mut month)?;
		chars::parse_numeric(tokenizer, &mut month)?;
		chars::expect_char(tokenizer, '.')?;
		chars::parse_numeric(tokenizer, &mut day)?;
		chars::parse_numeric(tokenizer, &mut day)?;
	}

	chars::expect_whitespace(tokenizer)?;

	tokenizer
		.ledger
		.tokens
		.push(Token::TransactionDateYear(tokenizer.line_index, year));
	tokenizer
		.ledger
		.tokens
		.push(Token::TransactionDateMonth(tokenizer.line_index, month));
	tokenizer
		.ledger
		.tokens
		.push(Token::TransactionDateDay(tokenizer.line_index, day));

	tokenize_state(tokenizer)?;
	tokenize_code(tokenizer)?;
	tokenize_description(tokenizer)?;

	Ok(())
}

fn tokenize_state(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Err(format!("Unexpected end of line")),
		Some(&c) => {
			chars::consume_whitespaces(tokenizer);
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

fn tokenize_code(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Ok(()),
		Some(&c) => {
			chars::consume_whitespaces(tokenizer);

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

fn tokenize_description(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Err(format!("Unexpected end of line")),
		Some(_) => {
			chars::consume_whitespaces(tokenizer);

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
