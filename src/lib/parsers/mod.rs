pub mod balancer;
mod debug;
pub mod modeler;
pub mod tokenizer;

use super::errors;
use super::model::Token;
use super::model::Transaction;
use std::fs::read_to_string;
use std::path::Path;

pub fn parse_file(
	file: &Path,
	tokens: &mut Vec<Token>,
	transactions: &mut Vec<Transaction>,
) -> Result<(), String> {
	match read_to_string(file) {
		Err(err) => Err(format!(
			"While parsing file \"{}\"\n{}",
			file.display(),
			err
		)),
		Ok(content) => match build_transactions(file, &content, tokens, transactions) {
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
	tokens: &mut Vec<Token>,
	transactions: &mut Vec<Transaction>,
) -> Result<(), errors::Error> {
	tokenizer::tokenize(file, content, tokens, transactions)?;
	// debug::print_tokens(&tokens);
	modeler::build(tokens, transactions)?;
	// debug::print_transactions(transactions);
	balancer::balance(transactions)?;
	Ok(())
}
