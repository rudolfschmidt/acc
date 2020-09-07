// use super::super::super::model::Token;
// use super::chars;
// use super::Tokenizer;

// pub(super) fn is_alias(tokenizer: &mut Tokenizer) -> Result<(), String> {
// 	if chars::consume_string(tokenizer, "alias ") {
// 		let mut alias = String::new();
// 		while let Some(&c) = tokenizer.line_characters.get(tokenizer.line_position) {
// 			alias.push(c);
// 			tokenizer.line_position += 1;
// 		}
// 		tokenizer
// 			.tokens
// 			.push(Token::Alias(tokenizer.line_index, alias));
// 	}
// 	Ok(())
// }
