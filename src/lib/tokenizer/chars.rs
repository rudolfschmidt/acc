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
	for char in str.chars() {
		match tokenizer.line_characters.get(pos) {
			None => return false,
			Some(&c) if c == char => pos += 1,
			_ => return false,
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

pub(super) fn expect<F>(tokenizer: &mut Tokenizer, condition: F) -> Result<(), String>
where
	F: Fn(char) -> bool,
{
	match tokenizer.line_characters.get(tokenizer.line_position) {
		Some(&c) if condition(c) => {
			tokenizer.line_position += 1;
			Ok(())
		}
		_ => Err(String::from("unexpected value")),
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

pub(super) fn consume_whitespaces(tokenizer: &mut Tokenizer) {
	while let Some(c) = tokenizer.line_characters.get(tokenizer.line_position) {
		if !c.is_whitespace() {
			break;
		}
		tokenizer.line_position += 1;
	}
}
