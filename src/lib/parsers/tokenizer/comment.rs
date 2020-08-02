use super::super::super::model::Token;
use super::chars;
use super::Tokenizer;

pub(super) fn tokenize(tokenizer: &mut Tokenizer) -> Result<(), String> {
	if let Some(';') = tokenizer.chars.get(tokenizer.pos) {
		tokenizer.pos += 1;

		chars::consume_whitespaces(tokenizer);

		let mut value = String::new();

		while let Some(&c) = tokenizer.chars.get(tokenizer.pos) {
			value.push(c);
			tokenizer.pos += 1;
		}

		tokenizer
			.tokens
			.push(Token::Comment(tokenizer.index, value));
	}

	Ok(())
}
