use super::super::Error;
use super::chars;
use super::Tokenizer;

type Rational = num::rational::Rational64;

pub(super) fn tokenize_expression(
	tokenizer: &mut Tokenizer,
) -> Result<Option<(String, Rational)>, Error> {
	match tokenizer.line_characters.get(tokenizer.line_position) {
		None => Ok(None),
		_ => {
			let commodity = tokenize_commodity(tokenizer, |c| {
				c == '-' || c.is_numeric() || c.is_whitespace()
			});
			if commodity.is_empty() {
				let amount = tokenize_rational_amount(tokenizer)?;
				let commodity = tokenize_commodity(tokenizer, |c| c == ')');
				chars::expect(tokenizer, |c| c == ')')?;
				Ok(Some((commodity, amount)))
			} else {
				super::chars::consume_whitespaces(tokenizer);
				let amount = tokenize_rational_amount(tokenizer)?;
				chars::expect(tokenizer, |c| c == ')')?;
				Ok(Some((commodity, amount)))
			}
		}
	}
}

pub(super) fn tokenize_decimal(
	tokenizer: &mut Tokenizer,
) -> Result<Option<(String, String)>, Error> {
	match tokenizer.line_characters.get(tokenizer.line_position) {
		None => Ok(None),
		_ => {
			let commodity = tokenize_commodity(tokenizer, |c| {
				c == '-' || c.is_numeric() || c.is_whitespace()
			});
			if commodity.is_empty() {
				let (amount, _) = parse_decimal_amount(tokenizer)?;
				let commodity = tokenize_commodity(tokenizer, |c| c == '=' || c.is_whitespace());
				Ok(Some((commodity, amount)))
			} else {
				super::chars::consume_whitespaces(tokenizer);
				let amount = tokenize_decimal_amount(tokenizer)?;
				Ok(Some((commodity, amount)))
			}
		}
	}
}
fn tokenize_decimal_amount(tokenizer: &mut Tokenizer) -> Result<String, Error> {
	let (amount, c) = parse_decimal_amount(tokenizer)?;
	if let Some(c) = c {
		return Err(Error::LexerError(format!(
			"received \"{}\", but expected number",
			c
		)));
	}
	Ok(amount)
}

fn tokenize_rational_amount(tokenizer: &mut Tokenizer) -> Result<Rational, Error> {
	match tokenizer.line_characters.get(tokenizer.line_position) {
		None => Err(Error::LexerError(String::from("unexpected end of line"))),
		Some(_) => {
			let numerator = chars::consume_while(tokenizer, |c| c.is_numeric());
			chars::expect(tokenizer, |c| c == '/')?;
			let denominator = chars::consume_while(tokenizer, |c| c.is_numeric());
			Ok(Rational::new(
				numerator.parse().unwrap(),
				denominator.parse().unwrap(),
			))
		}
	}
}

fn parse_decimal_amount(tokenizer: &mut Tokenizer) -> Result<(String, Option<char>), Error> {
	match tokenizer.line_characters.get(tokenizer.line_position) {
		None => Err(Error::LexerError(String::from("unexpected end of line"))),
		Some(_) => {
			let mut amount = String::new();
			if chars::try_consume_char(tokenizer, |c| c == '-') {
				amount.push('-');
			}
			match tokenizer.line_characters.get(tokenizer.line_position) {
				None => return Err(Error::LexerError(String::from("unexpected end of line"))),
				Some(c) if !c.is_numeric() => {
					return Err(Error::LexerError(format!(
						"received \"{}\", but expected number",
						c
					)))
				}
				Some(&c) => {
					amount.push(c);
					tokenizer.line_position += 1;
				}
			}
			while let Some(&c) = tokenizer.line_characters.get(tokenizer.line_position) {
				if !c.is_numeric() && c != '.' {
					break;
				}
				amount.push(c);
				tokenizer.line_position += 1;
			}
			while let Some(&c) = tokenizer.line_characters.get(tokenizer.line_position) {
				if c == '=' {
					break;
				} else if c.is_whitespace() {
					tokenizer.line_position += 1;
					continue;
				} else if !c.is_numeric() {
					return Ok((amount, Some(c)));
				}
				amount.push(c);
				tokenizer.line_position += 1;
			}
			Ok((amount, None))
		}
	}
}

fn tokenize_commodity<F>(tokenizer: &mut Tokenizer, stop_condition: F) -> String
where
	F: Fn(char) -> bool,
{
	let mut commodity = String::new();
	while let Some(&c) = tokenizer.line_characters.get(tokenizer.line_position) {
		if stop_condition(c) {
			break;
		}
		commodity.push(c);
		tokenizer.line_position += 1;
	}
	commodity
}
