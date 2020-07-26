mod cmd_accounts;
mod cmd_codes;
mod cmd_printer;
mod cmd_printer_bal;
mod cmd_printer_bal_flat;
mod cmd_printer_bal_struc;
mod cmd_printer_print;
mod cmd_printer_register;
mod debuger;
mod errors;
mod model;
mod parser_balancer;
mod parser_lexer;
mod parser_model;

use model::Argument;
use model::Command;
use model::Journal;
use model::Ledger;
use std::env;

fn main() {
	if let Err(e) = start() {
		eprintln!("{}", e);
	}
}

fn start() -> Result<(), String> {
	let mut args_it = env::args().skip(1);

	let mut files: Vec<String> = Vec::new();
	let mut command: Option<Command> = None;
	let mut arguments: Vec<Argument> = Vec::new();

	while let Some(arg) = args_it.next() {
		match arg.as_str() {
			"--file" | "-f" => match args_it.next() {
				None => return Err(format!("Error : No argument provided for --file")),
				Some(file_path) => files.push(file_path),
			},
			"--flat" => arguments.push(Argument::Flat),
			"--tree" => arguments.push(Argument::Tree),
			"--raw" => arguments.push(Argument::Raw),
			"--explicit" | "-x" => arguments.push(Argument::Explicit),
			"--lexer" => arguments.push(Argument::DebugLexer),
			"--unbalanced-transactions" => arguments.push(Argument::DebugUnbalancedTransactions),
			"--balanced-transactions" => arguments.push(Argument::DebugBalancedTransactions),
			"balance" | "bal" => command = Some(Command::Balance),
			"register" | "reg" => command = Some(Command::Register),
			"print" => command = Some(Command::Print),
			"debug" => command = Some(Command::Debug),
			"accounts" => command = Some(Command::Accounts),
			"codes" => command = Some(Command::Codes),
			_ => {}
		}
	}

	match command {
		None => Err(String::from("Error : No command selected")),
		Some(command) => {
			if files.is_empty() {
				return Err(format!(
					"Error : No file(s) reselected. Try --file <file> to select a file",
				));
			}

			let mut ledger = Ledger {
				journals: Vec::new(),
				command: command,
				arguments: arguments,
			};

			for file in files {
				match read(file, &mut ledger) {
					Err(err) => return Err(err),
					Ok(journal) => ledger.journals.insert(0, journal),
				}
			}

			execute_command(ledger)
		}
	}
}

fn read(file: String, ledger: &mut Ledger) -> Result<Journal, String> {
	let mut journal = model::Journal {
		file,
		content: String::new(),
		lexer_tokens: Vec::new(),
		transactions: Vec::new(),
	};
	match std::fs::read_to_string(&journal.file) {
		Err(err) => {
			return Err(format!(
				"While parsing \"{}\"\nError: {}",
				&journal.file, err
			))
		}
		Ok(data) => {
			journal.content = data;
		}
	}
	if let Err(err) = parse_file(ledger, &mut journal) {
		return Err(format!(
			"While parsing file \"{}\" at line {}:\n{}",
			journal.file, err.line, err.message
		));
	}
	Ok(journal)
}

fn parse_file(ledger: &mut Ledger, journal: &mut Journal) -> Result<(), errors::Error> {
	parser_lexer::read_lines(ledger, &journal.content, &mut journal.lexer_tokens)?;

	if let Command::Debug = ledger.command {
		if ledger.arguments.contains(&Argument::DebugLexer) {
			debuger::print_tokens(&journal.lexer_tokens);
		}
	}

	parser_model::parse_unbalanced_transactions(&journal.lexer_tokens, &mut journal.transactions)?;

	if let Command::Debug = ledger.command {
		if ledger
			.arguments
			.contains(&Argument::DebugUnbalancedTransactions)
		{
			debuger::print_transactions(&journal.transactions);
		}
	}

	parser_balancer::balance_transactions(&mut journal.transactions)?;

	if let Command::Debug = ledger.command {
		if ledger
			.arguments
			.contains(&Argument::DebugBalancedTransactions)
		{
			debuger::print_transactions(&journal.transactions);
		}
	}

	Ok(())
}

fn execute_command(ledger: model::Ledger) -> Result<(), String> {
	match ledger.command {
		Command::Balance => {
			if ledger.arguments.contains(&Argument::Flat) {
				return cmd_printer_bal_flat::print(
					ledger
						.journals
						.iter()
						.flat_map(|j| j.transactions.iter())
						.collect(),
				);
			}
			if ledger.arguments.contains(&Argument::Tree) {
				return cmd_printer_bal_struc::print(
					ledger
						.journals
						.iter()
						.flat_map(|j| j.transactions.iter())
						.collect(),
				);
			}
			return cmd_printer_bal_struc::print(
				ledger
					.journals
					.iter()
					.flat_map(|j| j.transactions.iter())
					.collect(),
			);
		}
		Command::Register => {
			cmd_printer_register::print(&ledger)?;
		}
		Command::Print => {
			if ledger.arguments.contains(&Argument::Explicit) {
				return cmd_printer_print::print_explicit(&ledger);
			}
			if ledger.arguments.contains(&Argument::Raw) {
				return cmd_printer_print::print_raw(&ledger);
			}
			return cmd_printer_print::print_raw(&ledger);
		}
		Command::Debug => {}
		Command::Accounts => {
			if ledger.arguments.contains(&Argument::Flat) {
				return cmd_accounts::print_accounts_flat(&ledger);
			}
			if ledger.arguments.contains(&Argument::Tree) {
				return cmd_accounts::print_accounts_tree(&ledger);
			}
			return cmd_accounts::print_accounts_tree(&ledger);
		}
		Command::Codes => cmd_codes::print_codes(&ledger)?,
	}
	Ok(())
}
