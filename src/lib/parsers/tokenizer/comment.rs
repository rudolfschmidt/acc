use super::super::super::model::Comment;
use super::super::Error;
use super::chars;
use super::Tokenizer;

pub(super) fn tokenize_journal_comment(tokenizer: &mut Tokenizer) -> Result<(), Error> {
	if let Some(comment) = tokenize_comment(tokenizer)? {
		// println!("journal comment : {}", comment);
	}
	Ok(())
}

pub(super) fn tokenize_indented_comment(tokenizer: &mut Tokenizer) -> Result<(), Error> {
	match tokenize_comment(tokenizer)? {
		None => Ok(()),
		Some(comment) => match tokenizer.unbalanced_transactions.last_mut() {
			None => {
				return Err(Error::LexerError(String::from(
					"indented comment need to come after a valid transaction or posting",
				)))
			}
			Some(transaction) => {
				match transaction.postings.last_mut() {
					None => transaction.header.comments.push(Comment {
						line: tokenizer.line_index + 1,
						comment,
					}),
					Some(p) => p.header.comments.push(Comment {
						line: tokenizer.line_index + 1,
						comment,
					}),
				}
				Ok(())
			}
		},
	}
}

fn tokenize_comment(tokenizer: &mut Tokenizer) -> Result<Option<String>, Error> {
	if chars::consume(tokenizer, |c| c == ';') {
		chars::consume(tokenizer, char::is_whitespace);

		let mut comment = String::new();

		while let Some(&c) = tokenizer.line_characters.get(tokenizer.line_position) {
			comment.push(c);
			tokenizer.line_position += 1;
		}

		return Ok(Some(comment));
	}
	Ok(None)
}
