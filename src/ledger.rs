use super::cmd_accounts;
use super::cmd_codes;
use super::cmd_printer_bal_flat;
use super::cmd_printer_bal_struc;
use super::cmd_printer_print;
use super::cmd_printer_register;
use super::debuger;
use super::errors::Error;
use super::model::Token;
use super::model::Transaction;
use super::parser_balancer;
use super::parser_lexer;
use super::parser_model;

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
	pub fn read_content(&mut self, file: &str) -> Result<(), String> {
		let content = self.read_file(file)?;
		if let Err(err) = self.parse_content(&content) {
			return Err(format!(
				"While parsing file \"{}\" at line {}:\n{}",
				&file, err.line, err.message
			));
		}
		Ok(())
	}

	pub fn read_tokens(&mut self, file: &str) -> Result<(), String> {
		let content = self.read_file(file)?;
		if let Err(err) = parser_lexer::read_lines(self, &content) {
			return Err(format!(
				"While parsing file \"{}\" at line {}:\n{}",
				&file, err.line, err.message
			));
		}
		Ok(())
	}

	fn read_file(&self, file: &str) -> Result<String, String> {
		match std::fs::read_to_string(file) {
			Err(err) => Err(format!("While parsing \"{}\"\nError : {}", &file, err)),
			Ok(content) => Ok(content),
		}
	}

	fn parse_content(&mut self, content: &str) -> Result<(), Error> {
		self.parse_tokens(content)?;
		self.parse_unbalanced_transactions()?;
		self.parse_balance_transactions()?;
		Ok(())
	}

	fn parse_tokens(&mut self, content: &str) -> Result<(), Error> {
		parser_lexer::read_lines(self, content)?;
		if let Command::Debug = self.command {
			if self.arguments.contains(&Argument::DebugLexer) {
				debuger::print_tokens(&self.tokens);
			}
		}
		Ok(())
	}

	fn parse_unbalanced_transactions(&mut self) -> Result<(), Error> {
		parser_model::parse_unbalanced_transactions(&self.tokens, &mut self.transactions)?;
		if let Command::Debug = self.command {
			if self
				.arguments
				.contains(&Argument::DebugUnbalancedTransactions)
			{
				debuger::print_transactions(&self.transactions);
			}
		}
		Ok(())
	}

	fn parse_balance_transactions(&mut self) -> Result<(), Error> {
		parser_balancer::balance_transactions(&mut self.transactions)?;
		if let Command::Debug = self.command {
			if self
				.arguments
				.contains(&Argument::DebugBalancedTransactions)
			{
				debuger::print_transactions(&self.transactions);
			}
		}
		Ok(())
	}

	pub fn execute_command(&self) -> Result<(), String> {
		match self.command {
			Command::Balance => {
				if self.arguments.contains(&Argument::Flat) {
					return cmd_printer_bal_flat::print(&self.transactions);
				}
				if self.arguments.contains(&Argument::Tree) {
					return cmd_printer_bal_struc::print(&self.transactions);
				}
				return cmd_printer_bal_struc::print(&self.transactions);
			}
			Command::Register => {
				cmd_printer_register::print(&self.transactions)?;
			}
			Command::Print => {
				if self.arguments.contains(&Argument::Explicit) {
					return cmd_printer_print::print_explicit(&self.transactions);
				}
				if self.arguments.contains(&Argument::Raw) {
					return cmd_printer_print::print_raw(&self.transactions);
				}
				return cmd_printer_print::print_raw(&self.transactions);
			}
			Command::Debug => {}
			Command::Accounts => {
				if self.arguments.contains(&Argument::Flat) {
					return cmd_accounts::print_accounts_flat(&self.transactions);
				}
				if self.arguments.contains(&Argument::Tree) {
					return cmd_accounts::print_accounts_tree(&self.transactions);
				}
				return cmd_accounts::print_accounts_tree(&self.transactions);
			}
			Command::Codes => cmd_codes::print_codes(&self.transactions)?,
		}
		Ok(())
	}
}
