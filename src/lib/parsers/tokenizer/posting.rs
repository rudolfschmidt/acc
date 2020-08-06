use super::super::super::model::MixedAmount;
use super::super::super::model::PostingHead;
use super::super::super::model::UnbalancedPosting;
use super::super::Error;
use super::chars;
use super::mixed_amount;
use super::Tokenizer;

pub(super) fn tokenize(tokenizer: &mut Tokenizer) -> Result<(), Error> {
	let virtual_posting = chars::consume(tokenizer, |c| c == '(');
	tokenize_posting(tokenizer, virtual_posting)
}

fn tokenize_posting(tokenizer: &mut Tokenizer, virtual_posting: bool) -> Result<(), Error> {
	match tokenizer.line_characters.get(tokenizer.line_position) {
		None => Ok(()),
		Some(_) => {
			chars::consume_whitespaces(tokenizer);

			let mut virtual_closed = false;
			let mut account = String::new();

			while let Some(&c) = tokenizer.line_characters.get(tokenizer.line_position) {
				if chars::consume(tokenizer, |c| c == '\t') || chars::consume_string(tokenizer, "  ") {
					if virtual_posting && !virtual_closed {
						return Err(Error::LexerError(String::from(
							"virtual posting not closed",
						)));
					}
					chars::consume_whitespaces(tokenizer);
					let amount = mixed_amount::tokenize(tokenizer)?.map(|(commodity, value)| MixedAmount {
						commodity: commodity,
						value: create_rational(&value),
					});
					let balance_assertion =
						balance_assertion(tokenizer)?.map(|(commodity, value)| MixedAmount {
							commodity: commodity,
							value: create_rational(&value),
						});
					tokenizer
						.unbalanced_transactions
						.last_mut()
						.expect("last transaction not found")
						.postings
						.push(UnbalancedPosting {
							header: PostingHead {
								line: tokenizer.line_index + 1,
								account: account,
								balance_assertion: balance_assertion,
								comments: Vec::new(),
								virtual_posting: virtual_posting,
							},
							amount: amount,
						});
					return Ok(());
				}
				if virtual_posting && chars::consume(tokenizer, |c| c == ')') {
					virtual_closed = true;
					continue;
				}
				account.push(c);
				tokenizer.line_position += 1;
			}

			tokenizer
				.unbalanced_transactions
				.last_mut()
				.expect("last transaction not found")
				.postings
				.push(UnbalancedPosting {
					header: PostingHead {
						line: tokenizer.line_index + 1,
						account: account,
						balance_assertion: None,
						comments: Vec::new(),
						virtual_posting: virtual_posting,
					},
					amount: None,
				});
			Ok(())
		}
	}
}

fn balance_assertion(tokenizer: &mut Tokenizer) -> Result<Option<(String, String)>, Error> {
	match tokenizer.line_characters.get(tokenizer.line_position) {
		Some('=') => {
			tokenizer.line_position += 1;
			chars::consume(tokenizer, char::is_whitespace);
			match tokenizer.line_characters.get(tokenizer.line_position) {
				None => Err(Error::LexerError(String::from("invalid balance assertion"))),
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
