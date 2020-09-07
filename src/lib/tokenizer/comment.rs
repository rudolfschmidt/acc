use super::super::model::Comment;
use super::super::model::Item;
use super::super::model::Posting;
use super::chars;
use super::Tokenizer;

pub(super) fn tokenize_journal_comment(tokenizer: &mut Tokenizer) {
	match tokenize_comment(tokenizer) {
		None => {}
		Some(comment) => {
			tokenizer.items.push(Item::Comment {
				line: tokenizer.line_index + 1,
				comment,
			});
		}
	}
}

pub(super) fn tokenize_indented_comment(tokenizer: &mut Tokenizer) {
	match tokenize_comment(tokenizer) {
		None => {}
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
		}
	}
}

fn tokenize_comment(tokenizer: &mut Tokenizer) -> Option<String> {
	if chars::try_consume_char(tokenizer, |c| c == ';') {
		chars::try_consume_char(tokenizer, char::is_whitespace);

		let mut comment = String::new();

		while let Some(&c) = tokenizer.line_characters.get(tokenizer.line_position) {
			comment.push(c);
			tokenizer.line_position += 1;
		}

		Some(comment)
	} else {
		None
	}
}
