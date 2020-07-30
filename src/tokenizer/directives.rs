use super::super::model::Token;
use super::chars;
use super::Tokenizer;

pub(super) fn is_include(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Ok(()),
		Some(_) => {
			if chars::is_string(tokenizer, "include ") {
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

pub(super) fn is_alias(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Ok(()),
		Some(_) => {
			if chars::is_string(tokenizer, "alias ") {
				let mut alias = String::new();
				while let Some(&c) = tokenizer.line_chars.get(tokenizer.line_pos) {
					alias.push(c);
					tokenizer.line_pos += 1;
				}
				tokenizer
					.ledger
					.tokens
					.push(Token::Alias(tokenizer.line_index, alias));
			}
			Ok(())
		}
	}
}
