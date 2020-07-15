mod balancer;
mod debuger;
mod lexer;
mod model;
mod parser;
mod printer;
mod printer_bal;
mod printer_bal_flat;
mod printer_bal_struc;
mod printer_print;
mod printer_register;
mod reader;

use std::env;

enum Command {
	Print,
	Balance,
	Register,
}

#[derive(PartialEq)]
enum Argument {
	Flat,
	Raw,
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
			"--raw" => arguments.push(Argument::Raw),
			"--debug-lexer" => arguments.push(Argument::DebugLexer),
			"--debug-unbalanced-transactions" => arguments.push(Argument::DebugUnbalancedTransactions),
			"--debug-balanced-transactions" => arguments.push(Argument::DebugBalancedTransactions),
			"balance" | "bal" => command = Some(Command::Balance),
			"print" => command = Some(Command::Print),
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
				let journal = read_file(file, &arguments)?;
				ledger.journals.push(journal);
			}
			execute(ledger, command, &arguments)
		}
	}
}

fn read_file(file: String, arguments: &[Argument]) -> Result<model::Journal, String> {
	let mut journal = model::Journal {
		file: file,
		content: String::new(),
		lexer_tokens: Vec::new(),
		unbalanced_transactions: Vec::new(),
		balanced_transactions: Vec::new(),
	};

	reader::read_file(&journal.file, &mut journal.content)?;

	lexer::read_lines(&journal.file, &journal.content, &mut journal.lexer_tokens)?;

	if arguments.contains(&Argument::DebugLexer) {
		debuger::print_tokens(&journal.lexer_tokens);
	}

	parser::parse_unbalanced_transactions(
		&journal.lexer_tokens,
		&mut journal.unbalanced_transactions,
	)?;

	if arguments.contains(&Argument::DebugUnbalancedTransactions) {
		debuger::print_unbalanced_transactions(&journal.unbalanced_transactions);
	}

	balancer::balance_transactions(
		&journal.file,
		&journal.unbalanced_transactions,
		&mut journal.balanced_transactions,
	)?;

	if arguments.contains(&Argument::DebugBalancedTransactions) {
		debuger::print_balanced_transactions(&journal.balanced_transactions);
	}

	Ok(journal)
}

fn execute(ledger: model::Ledger, command: Command, arguments: &[Argument]) -> Result<(), String> {
	match command {
		Command::Balance => {
			if arguments.contains(&Argument::Flat) {
				printer_bal_flat::print(
					ledger
						.journals
						.iter()
						.flat_map(|j| j.balanced_transactions.iter())
						.collect(),
				)?;
				return Ok(());
			}
			printer_bal_struc::print(
				ledger
					.journals
					.iter()
					.flat_map(|j| j.balanced_transactions.iter())
					.collect(),
			)?;
		}
		Command::Register => {
			printer_register::print(&ledger)?;
		}
		Command::Print => {
			if arguments.contains(&Argument::Raw) {
				printer_print::print_raw(&ledger)?;
				return Ok(());
			}
			printer_print::print(&ledger)?;
		}
	}
	Ok(())
}
