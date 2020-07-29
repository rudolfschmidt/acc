use super::Tokenizer;

pub(super) fn is_include(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Ok(()),
		Some(_) => {
			if is_string(tokenizer, "include ") {
				let mut file = String::new();
				while let Some(&c) = tokenizer.line_chars.get(tokenizer.line_pos) {
					file.push(c);
					tokenizer.line_pos += 1;
				}
				tokenizer.ledger.read_tokens(&file)?;
			}
			Ok(())
		}
	}
}
fn is_string(tokenizer: &mut Tokenizer, str: &str) -> bool {
	let mut pos = tokenizer.line_pos;
	for c in str.chars() {
		if let Err(_) = is_char(tokenizer, c, &mut pos) {
			return false;
		}
	}
	tokenizer.line_pos = pos;
	true
}
fn is_char(tokenizer: &mut Tokenizer, char: char, pos: &mut usize) -> Result<(), ()> {
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
