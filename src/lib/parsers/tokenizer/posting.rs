use super::super::super::model::MixedAmount;
use super::super::super::model::UnbalancedPosting;
use super::chars;
use super::mixed_amount;
use super::Tokenizer;

pub(super) fn tokenize(tokenizer: &mut Tokenizer) -> Result<(), String> {
	let virtual_posting = chars::consume(tokenizer, |c| c == '(');
	tokenize_posting(tokenizer, virtual_posting)
}

fn tokenize_posting(tokenizer: &mut Tokenizer, virtual_posting: bool) -> Result<(), String> {
	match tokenizer.line_characters.get(tokenizer.line_position) {
		None => Ok(()),
		Some(_) => {
			chars::consume_whitespaces(tokenizer);

			let mut virtual_closed = false;
			let mut account = String::new();

			while let Some(&c) = tokenizer.line_characters.get(tokenizer.line_position) {
				if chars::consume(tokenizer, |c| c == '\t') || chars::consume_string(tokenizer, "  ") {
					if virtual_posting && !virtual_closed {
						return Err(format!("virtual posting not closed"));
					}
					chars::consume_whitespaces(tokenizer);
					let unbalanced_amount = mixed_amount::tokenize(tokenizer)?.map(|(c, a)| MixedAmount {
						commodity: c,
						value: create_rational(&a),
					});
					let balance_assertion = balance_assertion(tokenizer)?.map(|(c, a)| MixedAmount {
						commodity: c,
						value: create_rational(&a),
					});
					match tokenizer.transactions.last_mut() {
						None => return Err(String::from("invalid posting position")),
						Some(transaction) => transaction.unbalanced_postings.push(UnbalancedPosting {
							line: tokenizer.line_index + 1,
							account: account,
							unbalanced_amount: unbalanced_amount,
							balance_assertion: balance_assertion,
							comments: Vec::new(),
							virtual_posting: virtual_posting,
						}),
					}
					return Ok(());
				}
				if virtual_posting && chars::consume(tokenizer, |c| c == ')') {
					virtual_closed = true;
					continue;
				}
				account.push(c);
				tokenizer.line_position += 1;
			}

			match tokenizer.transactions.last_mut() {
				None => return Err(String::from("invalid posting position")),
				Some(transaction) => transaction.unbalanced_postings.push(UnbalancedPosting {
					line: tokenizer.line_index + 1,
					account: account,
					unbalanced_amount: None,
					balance_assertion: None,
					comments: Vec::new(),
					virtual_posting: virtual_posting,
				}),
			}
			Ok(())
		}
	}
}

fn balance_assertion(tokenizer: &mut Tokenizer) -> Result<Option<(String, String)>, String> {
	match tokenizer.line_characters.get(tokenizer.line_position) {
		Some('=') => {
			tokenizer.line_position += 1;
			chars::consume(tokenizer, char::is_whitespace);
			match tokenizer.line_characters.get(tokenizer.line_position) {
				None => Err(String::from("invalid balance assertion")),
				Some(_) => {
					chars::consume_whitespaces(tokenizer);
					mixed_amount::tokenize(tokenizer)
				}
			}
		}
		_ => Ok(None),
	}
}

fn create_rational(value: &str) -> num::rational::Rational64 {
	let (_, right) = if let Some(index) = value.find('.') {
		let (left, right) = value.split_at(index);
		let right = right.chars().skip(1).collect::<String>();
		(left, right)
	} else {
		(value, "".to_owned())
	};
	let exponent: usize = right.chars().count();
	let numerator: i64 = value.replace('.', "").parse().unwrap();
	let denominator: i64 = 10_usize.pow(exponent as u32) as i64;
	num::rational::Rational64::new(numerator, denominator)
}
