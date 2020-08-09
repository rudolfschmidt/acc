use super::super::super::model::State;
use super::super::super::model::Transaction;
use super::super::Error;
use super::chars;
use super::Tokenizer;

pub(super) fn tokenize(tokenizer: &mut Tokenizer) -> Result<(), Error> {
	match tokenizer.line_characters.get(tokenizer.line_position) {
		Some(c) if c.is_numeric() => {
			super::balance_last_transaction(tokenizer)?;
			tokenize_transaction(tokenizer)
		}
		_ => Ok(()),
	}
}

fn tokenize_transaction(tokenizer: &mut Tokenizer) -> Result<(), Error> {
	let mut year = String::new();
	let mut month = String::new();
	let mut day = String::new();
	parse_date(tokenizer, &mut year, &mut month, &mut day)?;

	if chars::try_consume_char(tokenizer, |c| c == '=') {
		let mut year = String::new();
		let mut month = String::new();
		let mut day = String::new();
		parse_date(tokenizer, &mut year, &mut month, &mut day)?;
	}

	chars::expect(tokenizer, char::is_whitespace)?;

	let state = tokenize_state(tokenizer)?;
	let code = tokenize_code(tokenizer)?;
	let description = tokenize_description(tokenizer)?;

	let transaction = Transaction {
		line: tokenizer.line_index + 1,
		date: format!("{}-{}-{}", year, month, day),
		state,
		code,
		description,
		comments: Vec::new(),
		postings: Vec::new(),
	};
	tokenizer.transactions.push(transaction);

	Ok(())
}

fn parse_date(
	tokenizer: &mut Tokenizer,
	year: &mut String,
	month: &mut String,
	day: &mut String,
) -> Result<(), Error> {
	year.push(chars::extract(tokenizer, char::is_numeric)?);
	year.push(chars::extract(tokenizer, char::is_numeric)?);
	year.push(chars::extract(tokenizer, char::is_numeric)?);
	year.push(chars::extract(tokenizer, char::is_numeric)?);

	if chars::try_consume_char(tokenizer, |c| c == '-') {
		month.push(chars::extract(tokenizer, char::is_numeric)?);
		month.push(chars::extract(tokenizer, char::is_numeric)?);
		chars::expect(tokenizer, |c| c == '-')?;
		day.push(chars::extract(tokenizer, char::is_numeric)?);
		day.push(chars::extract(tokenizer, char::is_numeric)?);
	}

	if chars::try_consume_char(tokenizer, |c| c == '/') {
		month.push(chars::extract(tokenizer, char::is_numeric)?);
		month.push(chars::extract(tokenizer, char::is_numeric)?);
		chars::expect(tokenizer, |c| c == '/')?;
		day.push(chars::extract(tokenizer, char::is_numeric)?);
		day.push(chars::extract(tokenizer, char::is_numeric)?);
	}

	if chars::try_consume_char(tokenizer, |c| c == '.') {
		month.push(chars::extract(tokenizer, char::is_numeric)?);
		month.push(chars::extract(tokenizer, char::is_numeric)?);
		chars::expect(tokenizer, |c| c == '.')?;
		day.push(chars::extract(tokenizer, char::is_numeric)?);
		day.push(chars::extract(tokenizer, char::is_numeric)?);
	}

	Ok(())
}

fn tokenize_state(tokenizer: &mut Tokenizer) -> Result<State, Error> {
	match tokenizer.line_characters.get(tokenizer.line_position) {
		None => Err(Error::LexerError(String::from("unexpected end of line"))),
		Some(&c) => {
			chars::try_consume_char(tokenizer, char::is_whitespace);
			match c {
				'*' => {
					tokenizer.line_position += 1;
					Ok(State::Cleared)
				}
				'!' => {
					tokenizer.line_position += 1;
					Ok(State::Pending)
				}
				_ => Ok(State::Uncleared),
			}
		}
	}
}

fn tokenize_code(tokenizer: &mut Tokenizer) -> Result<Option<String>, Error> {
	chars::try_consume_char(tokenizer, char::is_whitespace);
	match tokenizer.line_characters.get(tokenizer.line_position) {
		Some('(') => {
			tokenizer.line_position += 1;
			match tokenizer.line_characters.get(tokenizer.line_position) {
				None => Err(Error::LexerError(String::from(
					"code has to be closed with \")\"",
				))),
				Some(&c) if c == ')' => Err(Error::LexerError(String::from("null code not allowed"))),
				Some(&c) => {
					let mut code = String::new();
					code.push(c);
					tokenizer.line_position += 1;
					loop {
						match tokenizer.line_characters.get(tokenizer.line_position) {
							None => {
								return Err(Error::LexerError(String::from(
									"code has to be closed with \")\"",
								)))
							}
							Some(&c) if c == ')' => {
								tokenizer.line_position += 1;
								break;
							}
							Some(&c) => {
								code.push(c);
								tokenizer.line_position += 1;
							}
						}
					}
					Ok(Some(code))
				}
			}
		}
		_ => Ok(None),
	}
}

fn tokenize_description(tokenizer: &mut Tokenizer) -> Result<String, Error> {
	match tokenizer.line_characters.get(tokenizer.line_position) {
		None => Err(Error::LexerError(String::from(
			"empty description not allowed",
		))),
		Some(_) => {
			chars::try_consume_char(tokenizer, char::is_whitespace);
			let mut description = String::new();
			while let Some(&c) = tokenizer.line_characters.get(tokenizer.line_position) {
				description.push(c);
				tokenizer.line_position += 1;
			}
			Ok(description)
		}
	}
}
