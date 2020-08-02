use super::super::super::model::Token;
use super::chars;
use super::mixed_amount;
use super::Tokenizer;

pub(super) fn tokenize(tokenizer: &mut Tokenizer) -> Result<(), String> {
	if chars::is_char(tokenizer, '(') {
		tokenizer.pos += 1;

		tokenize_virtual_account(tokenizer)
	} else if chars::is_any_char(tokenizer) {
		tokenize_normal_account(tokenizer)
	} else {
		Ok(())
	}
}

fn tokenize_virtual_account(tokenizer: &mut Tokenizer) -> Result<(), String> {
	let mut virtual_closed = false;
	let mut account = String::new();

	while let Some(&c) = tokenizer.chars.get(tokenizer.pos) {
		if chars::is_char(tokenizer, '\t')
			|| (chars::is_char_pos(&tokenizer.chars, tokenizer.pos, ' ')
				&& chars::is_char_pos(&tokenizer.chars, tokenizer.pos + 1, ' '))
		{
			if !virtual_closed {
				return Err(format!("Virtual account not closed"));
			}

			tokenizer
				.tokens
				.push(Token::PostingVirtualAccount(tokenizer.index, account));

			chars::consume_whitespaces(tokenizer);
			mixed_amount::tokenize(tokenizer)?;

			return balance_assertion(tokenizer);
		}

		if !chars::is_char(tokenizer, ')') {
			virtual_closed = true;
			account.push(c);
		}
		tokenizer.pos += 1;
	}

	tokenizer
		.tokens
		.push(Token::PostingVirtualAccount(tokenizer.index, account));

	Ok(())
}

fn tokenize_normal_account(tokenizer: &mut Tokenizer) -> Result<(), String> {
	let mut account = String::new();

	while let Some(&c) = tokenizer.chars.get(tokenizer.pos) {
		if chars::is_char(tokenizer, '\t')
			|| (chars::is_char_pos(&tokenizer.chars, tokenizer.pos, ' ')
				&& chars::is_char_pos(&tokenizer.chars, tokenizer.pos + 1, ' '))
		{
			tokenizer
				.tokens
				.push(Token::PostingAccount(tokenizer.index, account));

			chars::consume_whitespaces(tokenizer);
			mixed_amount::tokenize(tokenizer)?;

			return balance_assertion(tokenizer);
		}

		account.push(c);
		tokenizer.pos += 1;
	}

	tokenizer
		.tokens
		.push(Token::PostingAccount(tokenizer.index, account));

	Ok(())
}

fn balance_assertion(tokenizer: &mut Tokenizer) -> Result<(), String> {
	if let Some(&c) = tokenizer.chars.get(tokenizer.pos) {
		if c == '=' {
			tokenizer.pos += 1;

			tokenizer
				.tokens
				.push(Token::BalanceAssertion(tokenizer.index));

			if tokenizer.chars.get(tokenizer.pos).is_none() {
				return Err(String::from(""));
			} else {
				chars::consume_whitespaces(tokenizer);
				return mixed_amount::tokenize(tokenizer);
			};
		}
	}
	Ok(())
}
