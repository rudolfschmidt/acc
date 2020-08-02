use super::super::super::model::State;
use super::super::super::model::Token;
use super::chars;
use super::Tokenizer;

pub(super) fn tokenize(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.chars.get(tokenizer.pos) {
		None => Ok(()),
		Some(c) if c.is_numeric() => tokenize_transaction(tokenizer),
		Some(_) => Ok(()),
	}
}

fn tokenize_transaction(tokenizer: &mut Tokenizer) -> Result<(), String> {
	tokenize_date(tokenizer)?;
	if chars::is_char(tokenizer, '=') {
		chars::expect_char(tokenizer, '=')?;
		let mut year = String::new();
		let mut month = String::new();
		let mut day = String::new();
		parse_date(tokenizer, &mut year, &mut month, &mut day)?;
	}
	chars::expect_whitespace(tokenizer)?;
	tokenize_state(tokenizer)?;
	tokenize_code(tokenizer)?;
	tokenize_description(tokenizer)?;

	Ok(())
}

fn tokenize_date(tokenizer: &mut Tokenizer) -> Result<(), String> {
	let mut year = String::new();
	let mut month = String::new();
	let mut day = String::new();

	parse_date(tokenizer, &mut year, &mut month, &mut day)?;

	tokenizer
		.tokens
		.push(Token::TransactionDateYear(tokenizer.index, year));
	tokenizer
		.tokens
		.push(Token::TransactionDateMonth(tokenizer.index, month));
	tokenizer
		.tokens
		.push(Token::TransactionDateDay(tokenizer.index, day));

	Ok(())
}

fn parse_date(
	tokenizer: &mut Tokenizer,
	year: &mut String,
	month: &mut String,
	day: &mut String,
) -> Result<(), String> {
	chars::parse_numeric(tokenizer, year)?;
	chars::parse_numeric(tokenizer, year)?;
	chars::parse_numeric(tokenizer, year)?;
	chars::parse_numeric(tokenizer, year)?;

	if chars::is_char(tokenizer, '-') {
		chars::expect_char(tokenizer, '-')?;
		chars::parse_numeric(tokenizer, month)?;
		chars::parse_numeric(tokenizer, month)?;
		chars::expect_char(tokenizer, '-')?;
		chars::parse_numeric(tokenizer, day)?;
		chars::parse_numeric(tokenizer, day)?;
	}

	if chars::is_char(tokenizer, '/') {
		chars::expect_char(tokenizer, '/')?;
		chars::parse_numeric(tokenizer, month)?;
		chars::parse_numeric(tokenizer, month)?;
		chars::expect_char(tokenizer, '/')?;
		chars::parse_numeric(tokenizer, day)?;
		chars::parse_numeric(tokenizer, day)?;
	}

	if chars::is_char(tokenizer, '.') {
		chars::expect_char(tokenizer, '.')?;
		chars::parse_numeric(tokenizer, month)?;
		chars::parse_numeric(tokenizer, month)?;
		chars::expect_char(tokenizer, '.')?;
		chars::parse_numeric(tokenizer, day)?;
		chars::parse_numeric(tokenizer, day)?;
	}

	Ok(())
}

fn tokenize_state(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.chars.get(tokenizer.pos) {
		None => Err(String::from("Unexpected end of line")),
		Some(&c) => {
			chars::consume_whitespaces(tokenizer);
			match c {
				'*' => {
					let state = Token::TransactionState(tokenizer.index, State::Cleared);
					tokenizer.tokens.push(state);
					tokenizer.pos += 1;
				}
				'!' => {
					let state = Token::TransactionState(tokenizer.index, State::Pending);
					tokenizer.tokens.push(state);
					tokenizer.pos += 1;
				}
				_ => {
					let state = Token::TransactionState(tokenizer.index, State::Uncleared);
					tokenizer.tokens.push(state);
				}
			}
			Ok(())
		}
	}
}

fn tokenize_code(tokenizer: &mut Tokenizer) -> Result<(), String> {
	chars::consume_whitespaces(tokenizer);

	if chars::is_char(tokenizer, '(') {
		tokenizer.pos += 1;

		let mut value = String::new();

		match tokenizer.chars.get(tokenizer.pos) {
			None => {
				return Err(String::from(""));
			}
			Some(&c) => {
				value.push(c);
				tokenizer.pos += 1;
			}
		}

		while let Some(&c) = tokenizer.chars.get(tokenizer.pos) {
			if c == ')' {
				tokenizer.pos += 1;
				break;
			}
			value.push(c);
			tokenizer.pos += 1;
		}

		tokenizer
			.tokens
			.push(Token::TransactionCode(tokenizer.index, value));
	}

	Ok(())
}

fn tokenize_description(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.chars.get(tokenizer.pos) {
		None => Err(String::from("Unexpected end of line")),
		Some(_) => {
			chars::consume_whitespaces(tokenizer);

			let mut value = String::new();

			while let Some(&c) = tokenizer.chars.get(tokenizer.pos) {
				value.push(c);
				tokenizer.pos += 1;
			}

			tokenizer
				.tokens
				.push(Token::TransactionDescription(tokenizer.index, value));

			Ok(())
		}
	}
}
