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

fn read_file<'a>(file: String, arguments: &[Argument]) -> Result<model::Journal<'a>, String> {
	let content = reader::read_file(&file)?;

	let lexer_tokens = lexer::read_lines(&file, &content)?;

	if arguments.contains(&Argument::DebugLexer) {
		debuger::print_tokens(&lexer_tokens)
	}

	let unbalanced_transactions = parser::parse_tokens(&lexer_tokens)?;

	if arguments.contains(&Argument::DebugUnbalancedTransactions) {
		debuger::print_unbalanced_transactions(&unbalanced_transactions);
	}

	let balanced_transactions = balancer::balance_transactions(&file, &unbalanced_transactions)?;

	if arguments.contains(&Argument::DebugBalancedTransactions) {
		debuger::print_balanced_transactions(&balanced_transactions)
	}

	Ok(model::Journal {
		file: file,
		content: content,
		lexer_tokens: lexer_tokens,
		unbalanced_transactions: unbalanced_transactions,
		balanced_transactions: balanced_transactions,
	})
}

fn execute<'a>(
	ledger: model::Ledger<'a>,
	command: Command,
	arguments: &[Argument],
) -> Result<(), String> {
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
			printer_register::print(
				ledger
					.journals
					.iter()
					.flat_map(|j| j.balanced_transactions.iter())
					.collect(),
			)?;
		}
		Command::Print => {
			if arguments.contains(&Argument::Raw) {
				printer_print::print_raw(
					ledger
						.journals
						.iter()
						.flat_map(|j| j.balanced_transactions.iter())
						.collect(),
				)?;
				return Ok(());
			}
			printer_print::print(
				ledger
					.journals
					.iter()
					.flat_map(|j| j.balanced_transactions.iter())
					.collect(),
			)?;
		}
	}
	Ok(())
}
