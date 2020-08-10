pub mod balancer;
pub mod tokenizer;

use super::model::Item;
use std::fs::read_to_string;
use std::path::Path;

pub enum Error {
	LexerError(String),
	BalanceError {
		range_start: usize,
		range_end: usize,
		message: String,
	},
	ParseError {
		line: usize,
		message: String,
	},
}

pub fn parse(file: &Path, items: &mut Vec<Item>) -> Result<(), String> {
	match read_to_string(file) {
		Err(err) => Err(format!(
			"While parsing file \"{}\"\n{}",
			file.display(),
			err
		)),
		Ok(content) => match tokenizer::tokenize(file, &content, items) {
			Err(err) => match err {
				Error::LexerError(_) => unimplemented!(""),
				Error::BalanceError { .. } => unimplemented!(""),
				Error::ParseError { line, message } => Err(format!(
					"While parsing file \"{}\" at line {}:\n{}",
					file.display(),
					line,
					message
				)),
			},
			Ok(()) => Ok(()),
		},
	}
}
