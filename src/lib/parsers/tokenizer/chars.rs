use super::Tokenizer;

pub(super) fn expect<F>(tokenizer: &mut Tokenizer, f: F) -> Result<(), String>
where
	F: Fn(char) -> bool,
{
	match tokenizer.line_characters.get(tokenizer.line_position) {
		Some(&c) if f(c) => {
			tokenizer.line_position += 1;
			Ok(())
		}
		_ => Err(String::from("unexpected value")),
	}
}

pub(super) fn consume<F>(tokenizer: &mut Tokenizer, f: F) -> bool
where
	F: Fn(char) -> bool,
{
	match tokenizer.line_characters.get(tokenizer.line_position) {
		Some(&c) if f(c) => {
			tokenizer.line_position += 1;
			true
		}
		_ => false,
	}
}

pub(super) fn expect_next(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_characters.get(tokenizer.line_position) {
		None => Err(String::from("Unexpected end of line")),
		Some(_) => {
			tokenizer.line_position += 1;
			Ok(())
		}
	}
}

pub(super) fn extract<F>(tokenizer: &mut Tokenizer, f: F) -> Result<char, String>
where
	F: Fn(char) -> bool,
{
	match tokenizer.line_characters.get(tokenizer.line_position) {
		None => Err(String::from("unexpected end of line")),
		Some(&c) if f(c) => {
			tokenizer.line_position += 1;
			Ok(c)
		}
		_ => Err(String::from("unexpected value")),
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

pub(super) fn consume_string(tokenizer: &mut Tokenizer, str: &str) -> bool {
	let mut pos = tokenizer.line_position;
	for c in str.chars() {
		if is_pos_char(tokenizer, c, &mut pos).is_err() {
			return false;
		}
	}
	tokenizer.line_position = pos;
	true
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
