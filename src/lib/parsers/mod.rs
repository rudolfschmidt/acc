pub mod balancer;
pub mod tokenizer;

use super::model::BalancedPosting;
use super::model::Transaction;
use std::fs::read_to_string;
use std::path::Path;

pub struct Error {
	line: usize,
	message: String,
}

pub fn parse(
	file: &Path,
	transactions: &mut Vec<Transaction<BalancedPosting>>,
) -> Result<(), String> {
	match read_to_string(file) {
		Err(err) => Err(format!(
			"While parsing file \"{}\"\n{}",
			file.display(),
			err
		)),
		Ok(content) => match tokenizer::tokenize(file, &content, transactions) {
			Err(err) => Err(format!(
				"While parsing file \"{}\" at line {}:\n{}",
				file.display(),
				err.line,
				err.message
			)),
			Ok(()) => Ok(()),
		},
	}
}
