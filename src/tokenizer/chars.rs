use super::Tokenizer;

pub(super) fn is_space(chars: &[char], pos: usize) -> bool {
	match chars.get(pos) {
		None => false,
		Some(&c) if c == ' ' => true,
		Some(_) => false,
	}
}

pub(super) fn is_tab(chars: &[char], pos: usize) -> bool {
	match chars.get(pos) {
		None => false,
		Some(&c) if c == '\t' => true,
		Some(_) => false,
	}
}

pub(super) fn parse_numeric(tokenizer: &mut Tokenizer, value: &mut String) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Err(format!("Unexpected end of line. Expected number instead")),
		Some(c) if c.is_numeric() => {
			value.push(*c);
			tokenizer.line_pos += 1;
			Ok(())
		}
		Some(c) => Err(format!(
			"Unexpected character \"{}\". Expected number instead",
			c
		)),
	}
}

pub(super) fn expect_char(tokenizer: &mut Tokenizer, char: char) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Err(format!(
			"Unexpected end of line. Expected \"{}\" instead",
			char
		)),
		Some(&c) if c == char => {
			tokenizer.line_pos += 1;
			Ok(())
		}
		Some(&c) => Err(format!(
			"Unexpected character \"{}\". Expected \"{}\" instead",
			c, char
		)),
	}
}

pub(super) fn expect_whitespace(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Err(String::from(
			"Unexpected end of line. Expected whitespace instead",
		)),
		Some(c) if c.is_whitespace() => {
			tokenizer.line_pos += 1;
			Ok(())
		}
		Some(&c) => Err(format!(
			"Unexpected character \"{}\". Expected whitespace instead",
			c
		)),
	}
}

pub(super) fn is_char(tokenizer: &mut Tokenizer, char: char) -> bool {
	if let Some(&c) = tokenizer.line_chars.get(tokenizer.line_pos) {
		if c == char {
			return true;
		}
	}
	false
}

pub(super) fn consume_whitespaces(tokenizer: &mut Tokenizer) {
	while let Some(c) = tokenizer.line_chars.get(tokenizer.line_pos) {
		if !c.is_whitespace() {
			break;
		}
		tokenizer.line_pos += 1;
	}
}

pub(super) fn is_string(tokenizer: &mut Tokenizer, str: &str) -> bool {
	let mut pos = tokenizer.line_pos;
	for c in str.chars() {
		if let Err(_) = is_pos_char(tokenizer, c, &mut pos) {
			return false;
		}
	}
	tokenizer.line_pos = pos;
	true
}

fn is_pos_char(tokenizer: &mut Tokenizer, char: char, pos: &mut usize) -> Result<(), ()> {
	match tokenizer.line_chars.get(*pos) {
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
