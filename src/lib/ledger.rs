use super::commands;
use super::errors;
use super::model::Token;
use super::model::Transaction;
use super::parsers;

pub enum Command {
	Print,
	Balance,
	Register,
	Debug,
	Accounts,
	Codes,
}

#[derive(PartialEq)]
pub enum Argument {
	Flat,
	Tree,
	Raw,
	Explicit,
	DebugLexer,
	DebugUnbalancedTransactions,
	DebugBalancedTransactions,
}

pub struct Ledger {
	pub command: Command,
	pub arguments: Vec<Argument>,
	pub tokens: Vec<Token>,
	pub transactions: Vec<Transaction>,
}

impl Ledger {
	pub fn read_content(&mut self, file: &std::path::Path) -> Result<(), String> {
		let content = read_file(file)?;
		if let Err(err) = self.parse_content(&content) {
			return cannot_parse_file(err, file);
		}
		Ok(())
	}

	pub fn read_tokens(&mut self, file: &std::path::Path) -> Result<(), String> {
		let content = read_file(file)?;
		if let Err(err) = parsers::tokenizer::read_lines(self, &content) {
			return cannot_parse_file(err, file);
		}
		Ok(())
	}

	fn parse_content(&mut self, content: &str) -> Result<(), errors::Error> {
		self.build_tokens(content)?;
		self.build_transactions()?;
		self.balance_transactions()?;
		Ok(())
	}

	fn build_tokens(&mut self, content: &str) -> Result<(), errors::Error> {
		parsers::tokenizer::read_lines(self, content)?;
		if let Command::Debug = self.command {
			if self.arguments.contains(&Argument::DebugLexer) {
				commands::debuger::print_tokens(&self.tokens);
			}
		}
		Ok(())
	}

	fn build_transactions(&mut self) -> Result<(), errors::Error> {
		parsers::modeler::build_transactions(&self.tokens, &mut self.transactions)?;
		if let Command::Debug = self.command {
			if self
				.arguments
				.contains(&Argument::DebugUnbalancedTransactions)
			{
				commands::debuger::print_transactions(&self.transactions);
			}
		}
		Ok(())
	}

	fn balance_transactions(&mut self) -> Result<(), errors::Error> {
		parsers::balancer::balance_transactions(&mut self.transactions)?;
		if let Command::Debug = self.command {
			if self
				.arguments
				.contains(&Argument::DebugBalancedTransactions)
			{
				commands::debuger::print_transactions(&self.transactions);
			}
		}
		Ok(())
	}

	pub fn execute_command(&self) -> Result<(), String> {
		match self.command {
			Command::Balance => {
				if self.arguments.contains(&Argument::Flat) {
					return commands::balance::print_flat(&self.transactions);
				}
				if self.arguments.contains(&Argument::Tree) {
					return commands::balance::print_tree(&self.transactions);
				}
				return commands::balance::print_tree(&self.transactions);
			}
			Command::Register => commands::register::print(&self.transactions)?,
			Command::Print => {
				if self.arguments.contains(&Argument::Explicit) {
					return commands::print::print_explicit(&self.transactions);
				}
				if self.arguments.contains(&Argument::Raw) {
					return commands::print::print_raw(&self.transactions);
				}
				return commands::print::print_raw(&self.transactions);
			}
			Command::Debug => {}
			Command::Accounts => {
				if self.arguments.contains(&Argument::Flat) {
					return commands::accounts::print_flat(&self.transactions);
				}
				if self.arguments.contains(&Argument::Tree) {
					return commands::accounts::print_tree(&self.transactions);
				}
				return commands::accounts::print_tree(&self.transactions);
			}
			Command::Codes => commands::codes::print(&self.transactions)?,
		}
		Ok(())
	}
}

fn read_file(path: &std::path::Path) -> Result<String, String> {
	match std::fs::read_to_string(path) {
		Err(err) => Err(format!(
			"While parsing \"{}\"\nError : {}",
			path.display(),
			err
		)),
		Ok(content) => Ok(content),
	}
}

fn cannot_parse_file(err: errors::Error, file: &std::path::Path) -> Result<(), String> {
	Err(format!(
		"While parsing file \"{}\" at line {}:\n{}",
		file.display(),
		err.line,
		err.message
	))
}
