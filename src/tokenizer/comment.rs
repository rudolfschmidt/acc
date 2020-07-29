use super::super::model::Token;
use super::Tokenizer;

pub(super) fn tokenize(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Ok(()),
		Some(&c) => {
			if c == ';' {
				tokenizer.line_pos += 1;

				tokenizer.consume_whitespaces();

				let mut value = String::new();

				while let Some(&c) = tokenizer.line_chars.get(tokenizer.line_pos) {
					value.push(c);
					tokenizer.line_pos += 1;
				}

				tokenizer
					.ledger
					.tokens
					.push(Token::Comment(tokenizer.line_index, value));
			}

			Ok(())
		}
	}
}
