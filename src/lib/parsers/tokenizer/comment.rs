use super::super::super::model::Token;
use super::chars;
use super::Tokenizer;

pub(super) fn tokenize(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Ok(()),
		Some(&c) => {
			if c == ';' {
				tokenizer.line_pos += 1;

				chars::consume_whitespaces(tokenizer);

				let mut value = String::new();

				while let Some(&c) = tokenizer.line_chars.get(tokenizer.line_pos) {
					value.push(c);
					tokenizer.line_pos += 1;
				}

				tokenizer
					.tokens
					.push(Token::Comment(tokenizer.line_index, value));
			}

			Ok(())
		}
	}
}
