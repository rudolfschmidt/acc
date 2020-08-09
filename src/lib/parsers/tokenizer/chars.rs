use super::super::Error;
use super::Tokenizer;

pub(super) fn try_consume_char<F>(tokenizer: &mut Tokenizer, condition: F) -> bool
where
	F: Fn(char) -> bool,
{
	match tokenizer.line_characters.get(tokenizer.line_position) {
		Some(&c) if condition(c) => {
			tokenizer.line_position += 1;
			true
		}
		_ => false,
	}
}

pub(super) fn try_consume_string(tokenizer: &mut Tokenizer, str: &str) -> bool {
	let mut pos = tokenizer.line_position;
	for c in str.chars() {
		if is_pos_char(tokenizer, c, &mut pos).is_err() {
			return false;
		}
	}
	tokenizer.line_position = pos;
	true
}

pub(super) fn consume_while<F>(tokenizer: &mut Tokenizer, condition: F) -> String
where
	F: Fn(char) -> bool,
{
	let mut result = String::new();
	while let Some(&c) = tokenizer.line_characters.get(tokenizer.line_position) {
		if !condition(c) {
			break;
		}
		result.push(c);
		tokenizer.line_position += 1;
	}
	result
}

pub(super) fn expect<F>(tokenizer: &mut Tokenizer, condition: F) -> Result<(), Error>
where
	F: Fn(char) -> bool,
{
	match tokenizer.line_characters.get(tokenizer.line_position) {
		Some(&c) if condition(c) => {
			tokenizer.line_position += 1;
			Ok(())
		}
		_ => Err(Error::LexerError(String::from("unexpected value"))),
	}
}

pub(super) fn expect_next(tokenizer: &mut Tokenizer) -> Result<(), Error> {
	match tokenizer.line_characters.get(tokenizer.line_position) {
		None => Err(Error::LexerError(String::from("Unexpected end of line"))),
		Some(_) => {
			tokenizer.line_position += 1;
			Ok(())
		}
	}
}

pub(super) fn extract<F>(tokenizer: &mut Tokenizer, f: F) -> Result<char, Error>
where
	F: Fn(char) -> bool,
{
	match tokenizer.line_characters.get(tokenizer.line_position) {
		None => Err(Error::LexerError(String::from("unexpected end of line"))),
		Some(&c) if f(c) => {
			tokenizer.line_position += 1;
			Ok(c)
		}
		_ => Err(Error::LexerError(String::from("unexpected value"))),
	}
}

pub(super) fn is_any_char(tokenizer: &Tokenizer) -> bool {
	match tokenizer.line_characters.get(tokenizer.line_position) {
		None => false,
		Some(_) => true,
	}
}

pub(super) fn is_char(tokenizer: &Tokenizer, char: char) -> bool {
	match tokenizer.line_characters.get(tokenizer.line_position) {
		Some(&c) if c == char => true,
		_ => false,
	}
}

pub(super) fn consume_whitespaces(tokenizer: &mut Tokenizer) {
	while let Some(c) = tokenizer.line_characters.get(tokenizer.line_position) {
		if !c.is_whitespace() {
			break;
		}
		tokenizer.line_position += 1;
	}
}

fn is_pos_char(tokenizer: &mut Tokenizer, char: char, pos: &mut usize) -> Result<(), ()> {
	match tokenizer.line_characters.get(*pos) {
		None => Err(()),
		Some(&c) => {
			if c == char {
				*pos += 1;
				Ok(())
			} else {
				Err(())
			}
		}
	}
}
