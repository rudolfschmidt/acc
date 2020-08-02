use super::Tokenizer;

pub(super) fn parse_numeric(tokenizer: &mut Tokenizer, value: &mut String) -> Result<(), String> {
	match tokenizer.chars.get(tokenizer.pos) {
		None => Err(String::from(
			"Unexpected end of line. Expected number instead",
		)),
		Some(c) if c.is_numeric() => {
			value.push(*c);
			tokenizer.pos += 1;
			Ok(())
		}
		Some(c) => Err(format!(
			"Unexpected character \"{}\". Expected number instead",
			c
		)),
	}
}

pub(super) fn expect_char(tokenizer: &mut Tokenizer, char: char) -> Result<(), String> {
	match tokenizer.chars.get(tokenizer.pos) {
		None => Err(format!(
			"Unexpected end of line. Expected \"{}\" instead",
			char
		)),
		Some(&c) if c == char => {
			tokenizer.pos += 1;
			Ok(())
		}
		Some(&c) => Err(format!(
			"Unexpected character \"{}\". Expected \"{}\" instead",
			c, char
		)),
	}
}

pub(super) fn expect_whitespace(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.chars.get(tokenizer.pos) {
		None => Err(String::from(
			"Unexpected end of line. Expected whitespace instead",
		)),
		Some(c) if c.is_whitespace() => {
			tokenizer.pos += 1;
			Ok(())
		}
		Some(&c) => Err(format!(
			"Unexpected character \"{}\". Expected whitespace instead",
			c
		)),
	}
}

pub(super) fn is_any_char(tokenizer: &Tokenizer) -> bool {
	match tokenizer.chars.get(tokenizer.pos) {
		None => false,
		Some(_) => true,
	}
}

pub(super) fn is_char(tokenizer: &Tokenizer, char: char) -> bool {
	match tokenizer.chars.get(tokenizer.pos) {
		Some(&c) if c == char => true,
		_ => false,
	}
}

pub(super) fn is_char_pos(chars: &[char], pos: usize, char: char) -> bool {
	match chars.get(pos) {
		Some(&c) if c == char => true,
		_ => false,
	}
}

pub(super) fn consume_whitespaces(tokenizer: &mut Tokenizer) {
	while let Some(c) = tokenizer.chars.get(tokenizer.pos) {
		if !c.is_whitespace() {
			break;
		}
		tokenizer.pos += 1;
	}
}

pub(super) fn consume_string(tokenizer: &mut Tokenizer, str: &str) -> bool {
	let mut pos = tokenizer.pos;
	for c in str.chars() {
		if is_pos_char(tokenizer, c, &mut pos).is_err() {
			return false;
		}
	}
	tokenizer.pos = pos;
	true
}

fn is_pos_char(tokenizer: &mut Tokenizer, char: char, pos: &mut usize) -> Result<(), ()> {
	match tokenizer.chars.get(*pos) {
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
