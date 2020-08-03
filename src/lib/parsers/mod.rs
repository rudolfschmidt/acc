pub mod balancer;
pub mod debug;
pub mod modeler;
pub mod tokenizer;

use super::model::Transaction;
use std::fs::read_to_string;
use std::path::Path;

pub struct Error {
	line: usize,
	message: String,
}

pub fn parse_file(file: &Path, transactions: &mut Vec<Transaction>) -> Result<(), String> {
	match read_to_string(file) {
		Err(err) => Err(format!(
			"While parsing file \"{}\"\n{}",
			file.display(),
			err
		)),
		Ok(content) => match build_transactions(file, &content, transactions) {
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

fn build_transactions(
	file: &Path,
	content: &str,
	transactions: &mut Vec<Transaction>,
) -> Result<(), Error> {
	let mut tokens = Vec::new();
	tokenizer::tokenize(file, content, &mut tokens, transactions)?;
	modeler::build(&mut tokens, transactions)?;
	balancer::balance(transactions)?;
	Ok(())
}
