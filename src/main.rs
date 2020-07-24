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

use std::env;

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
				None => return Err(String::from("Error : No argument provided for --file")),
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
				return Err(String::from(
					"Error : No file(s) reselected. Try --file <file> to select a file",
				));
			}

			let mut ledger = model::Ledger {
				journals: Vec::new(),
			};

			for file in files {
				let mut journal = model::Journal {
					file,
					content: String::new(),
					lexer_tokens: Vec::new(),
					transactions: Vec::new(),
				};
				match read_file(&mut journal, &command, &arguments) {
					Err(err) => return Err(err),
					Ok(()) => ledger.journals.push(journal),
				}
			}
			execute_command(ledger, command, arguments)
		}
	}
}

fn read_file(
	journal: &mut model::Journal,
	command: &Command,
	arguments: &[Argument],
) -> Result<(), String> {
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

	match parse_file(journal, command, arguments) {
		Err(err) => {
			return Err(format!(
				"While parsing file \"{}\" at line {}:\n{}",
				journal.file, err.line, err.message
			))
		}
		Ok(()) => Ok(()),
	}
}

fn parse_file(
	journal: &mut model::Journal,
	command: &Command,
	arguments: &[Argument],
) -> Result<(), errors::Error> {
	parser_lexer::read_lines(&journal.content, &mut journal.lexer_tokens)?;

	if let Command::Debug = command {
		if arguments.contains(&Argument::DebugLexer) {
			debuger::print_tokens(&journal.lexer_tokens);
		}
	}

	parser_model::parse_unbalanced_transactions(&journal.lexer_tokens, &mut journal.transactions)?;

	if let Command::Debug = command {
		if arguments.contains(&Argument::DebugUnbalancedTransactions) {
			debuger::print_transactions(&journal.transactions);
		}
	}

	parser_balancer::balance_transactions(&mut journal.transactions)?;

	if let Command::Debug = command {
		if arguments.contains(&Argument::DebugBalancedTransactions) {
			debuger::print_transactions(&journal.transactions);
		}
	}

	Ok(())
}

fn execute_command(
	ledger: model::Ledger,
	command: Command,
	mut arguments: Vec<Argument>,
) -> Result<(), String> {
	match command {
		Command::Balance => {
			if !arguments.contains(&Argument::Flat) {
				arguments.push(Argument::Tree);
			}
			if arguments.contains(&Argument::Flat) {
				cmd_printer_bal_flat::print(
					ledger
						.journals
						.iter()
						.flat_map(|j| j.transactions.iter())
						.collect(),
				)?
			}
			if arguments.contains(&Argument::Tree) {
				cmd_printer_bal_struc::print(
					ledger
						.journals
						.iter()
						.flat_map(|j| j.transactions.iter())
						.collect(),
				)?
			}
		}
		Command::Register => {
			cmd_printer_register::print(&ledger)?;
		}
		Command::Print => {
			if arguments.contains(&Argument::Raw) {
				return cmd_printer_print::print_raw(&ledger);
			}
			if arguments.contains(&Argument::Explicit) {
				return cmd_printer_print::print(&ledger);
			}
			if !arguments.contains(&Argument::Explicit) {
				return cmd_printer_print::print_raw(&ledger);
			}
		}
		Command::Debug => {}
		Command::Accounts => {
			if arguments.contains(&Argument::Flat) {
				return cmd_accounts::print_accounts_flat(&ledger);
			}
			if arguments.contains(&Argument::Tree) {
				return cmd_accounts::print_accounts_tree(&ledger);
			}
			if !arguments.contains(&Argument::Flat) {
				return cmd_accounts::print_accounts_tree(&ledger);
			}
		}
		Command::Codes => cmd_codes::print_codes(&ledger)?,
	}
	Ok(())
}
