use super::super::super::model::Comment;
use super::super::super::model::Item;
use super::super::super::model::Posting;
use super::super::Error;
use super::chars;
use super::Tokenizer;

pub(super) fn tokenize_journal_comment(tokenizer: &mut Tokenizer) -> Result<(), Error> {
	match tokenize_comment(tokenizer)? {
		None => Ok(()),
		Some(comment) => {
			tokenizer.items.push(Item::Comment {
				line: tokenizer.line_index + 1,
				comment,
			});
			Ok(())
		}
	}
}

pub(super) fn tokenize_indented_comment(tokenizer: &mut Tokenizer) -> Result<(), Error> {
	match tokenize_comment(tokenizer)? {
		None => Ok(()),
		Some(comment) => {
			for item in tokenizer.items.iter_mut().rev() {
				match item {
					Item::Transaction {
						comments, postings, ..
					} => {
						match postings.last_mut() {
							None => comments.push(Comment {
								line: tokenizer.line_index + 1,
								comment,
							}),
							Some(Posting::UnbalancedPosting { comments, .. }) => comments.push(Comment {
								line: tokenizer.line_index + 1,
								comment,
							}),
							_ => {}
						}
						break;
					}
					_ => {}
				}
			}
			Ok(())
		}
	}
}

fn tokenize_comment(tokenizer: &mut Tokenizer) -> Result<Option<String>, Error> {
	if chars::try_consume_char(tokenizer, |c| c == ';') {
		chars::try_consume_char(tokenizer, char::is_whitespace);

		let mut comment = String::new();

		while let Some(&c) = tokenizer.line_characters.get(tokenizer.line_position) {
			comment.push(c);
			tokenizer.line_position += 1;
		}

		return Ok(Some(comment));
	}
	Ok(None)
}
