mod balancer;
mod cmd_accounts;
mod cmd_codes;
mod cmd_printer;
mod cmd_printer_bal;
mod cmd_printer_bal_flat;
mod cmd_printer_bal_struc;
mod cmd_printer_print;
mod cmd_printer_register;
mod debuger;
mod lexer;
mod model;
mod parser;
mod reader;

use std::env;

enum Command {
	Print,
	Balance,
	Register,
	Debug,
	Accounts,
	Codes,
}

#[derive(PartialEq)]
enum Argument {
	Flat,
	Tree,
	Raw,
	Evaluate,
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
			"--evaluate" | "--eval" => arguments.push(Argument::Evaluate),
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
				let journal = read_file(&command, &arguments, file)?;
				ledger.journals.push(journal);
			}
			execute_command(ledger, command, arguments)
		}
	}
}

fn read_file(
	command: &Command,
	arguments: &[Argument],
	file: String,
) -> Result<model::Journal, String> {
	let mut journal = model::Journal {
		file,
		content: String::new(),
		lexer_tokens: Vec::new(),
		unbalanced_transactions: Vec::new(),
		balanced_transactions: Vec::new(),
	};

	reader::read_file(&journal.file, &mut journal.content)?;

	lexer::read_lines(&journal.file, &journal.content, &mut journal.lexer_tokens)?;

	if let Command::Debug = command {
		if arguments.contains(&Argument::DebugLexer) {
			debuger::print_tokens(&journal.lexer_tokens);
		}
	}

	parser::parse_unbalanced_transactions(
		&journal.lexer_tokens,
		&mut journal.unbalanced_transactions,
	)?;

	if let Command::Debug = command {
		if arguments.contains(&Argument::DebugUnbalancedTransactions) {
			debuger::print_unbalanced_transactions(&journal.unbalanced_transactions);
		}
	}

	balancer::balance_transactions(
		&journal.file,
		&journal.unbalanced_transactions,
		&mut journal.balanced_transactions,
	)?;

	if let Command::Debug = command {
		if arguments.contains(&Argument::DebugBalancedTransactions) {
			debuger::print_balanced_transactions(&journal.balanced_transactions);
		}
	}

	Ok(journal)
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
						.flat_map(|j| j.balanced_transactions.iter())
						.collect(),
				)?
			}
			if arguments.contains(&Argument::Tree) {
				cmd_printer_bal_struc::print(
					ledger
						.journals
						.iter()
						.flat_map(|j| j.balanced_transactions.iter())
						.collect(),
				)?
			}
		}
		Command::Register => {
			cmd_printer_register::print(&ledger)?;
		}
		Command::Print => {
			if !arguments.contains(&Argument::Raw) {
				arguments.push(Argument::Evaluate);
			}
			if arguments.contains(&Argument::Raw) {
				cmd_printer_print::print_raw(&ledger)?
			}
			if arguments.contains(&Argument::Evaluate) {
				cmd_printer_print::print(&ledger)?
			}
		}
		Command::Debug => {}
		Command::Accounts => {
			if !arguments.contains(&Argument::Flat) {
				arguments.push(Argument::Tree);
			}
			if arguments.contains(&Argument::Flat) {
				cmd_accounts::print_accounts_flat(&ledger)?
			}
			if arguments.contains(&Argument::Tree) {
				cmd_accounts::print_accounts_tree(&ledger)?
			}
		}
		Command::Codes => cmd_codes::print_codes(&ledger)?,
	}
	Ok(())
}
