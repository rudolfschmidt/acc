use super::super::super::model::Costs;
use super::super::super::model::Item;
use super::super::super::model::MixedAmount;
use super::super::super::model::Posting;
use super::super::Error;
use super::chars;
use super::mixed_amount;
use super::Tokenizer;

pub(super) fn tokenize(tokenizer: &mut Tokenizer) -> Result<(), Error> {
	let virtual_posting = chars::try_consume_char(tokenizer, |c| c == '(');
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
				if chars::try_consume_char(tokenizer, |c| c == '\t')
					|| chars::try_consume_string(tokenizer, "  ")
				{
					if virtual_posting && !virtual_closed {
						return Err(Error::LexerError(String::from(
							"virtual posting not closed",
						)));
					}
					chars::consume_whitespaces(tokenizer);
					let amount =
						mixed_amount::tokenize_decimal(tokenizer)?.map(|(commodity, value)| MixedAmount {
							commodity: commodity,
							value: create_rational(&value),
						});
					let balance_assertion =
						balance_assertion(tokenizer)?.map(|(commodity, value)| MixedAmount {
							commodity: commodity,
							value: create_rational(&value),
						});
					let costs = costs(tokenizer)?;

					for item in tokenizer.items.iter_mut().rev() {
						match item {
							Item::Transaction { postings, .. } => {
								postings.push(Posting::UnbalancedPosting {
									line: tokenizer.line_index + 1,
									account: account,
									comments: Vec::new(),
									balance_assertion: balance_assertion,
									costs: costs,
									unbalanced_amount: amount,
								});
								break;
							}
							_ => {}
						}
					}
					return Ok(());
				}
				if virtual_posting && chars::try_consume_char(tokenizer, |c| c == ')') {
					virtual_closed = true;
					continue;
				}
				account.push(c);
				tokenizer.line_position += 1;
			}

			if virtual_posting && !virtual_closed {
				return Err(Error::LexerError(String::from(
					"virtual posting not closed",
				)));
			}

			for item in tokenizer.items.iter_mut().rev() {
				match item {
					Item::Transaction { postings, .. } => {
						postings.push(Posting::UnbalancedPosting {
							line: tokenizer.line_index + 1,
							account: account,
							balance_assertion: None,
							comments: Vec::new(),
							costs: None,
							unbalanced_amount: None,
						});
						break;
					}
					_ => {}
				}
			}

			Ok(())
		}
	}
}

fn costs(tokenizer: &mut Tokenizer) -> Result<Option<Costs>, Error> {
	chars::try_consume_char(tokenizer, char::is_whitespace);
	if chars::try_consume_char(tokenizer, |c| c == '@') {
		return if chars::try_consume_char(tokenizer, |c| c == '@') {
			chars::try_consume_char(tokenizer, char::is_whitespace);
			Ok(None)
		} else {
			chars::try_consume_char(tokenizer, char::is_whitespace);
			return if chars::try_consume_char(tokenizer, |c| c == '(') {
				Ok(
					mixed_amount::tokenize_expression(tokenizer)?
						.map(|(commodity, value)| MixedAmount {
							commodity: commodity,
							value: value,
						})
						.map(|amount| Costs::PerUnit(amount)),
				)
			} else {
				Ok(
					mixed_amount::tokenize_decimal(tokenizer)?
						.map(|(commodity, value)| MixedAmount {
							commodity: commodity,
							value: create_rational(&value),
						})
						.map(|amount| Costs::PerUnit(amount)),
				)
			};
		};
	}
	Ok(None)
}

fn balance_assertion(tokenizer: &mut Tokenizer) -> Result<Option<(String, String)>, Error> {
	if chars::try_consume_char(tokenizer, |c| c == '=') {
		chars::try_consume_char(tokenizer, char::is_whitespace);
		match tokenizer.line_characters.get(tokenizer.line_position) {
			None => return Err(Error::LexerError(String::from("invalid balance assertion"))),
			Some(_) => {
				chars::consume_whitespaces(tokenizer);
				return mixed_amount::tokenize_decimal(tokenizer);
			}
		}
	}
	Ok(None)
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
