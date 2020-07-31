mod lib;

use lib::ledger::Argument;
use lib::ledger::Command;
use lib::ledger::Ledger;

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
				command,
				arguments,
				tokens: Vec::new(),
				transactions: Vec::new(),
			};

			for file in files {
				ledger.read_content(std::path::Path::new(&file))?;
			}

			ledger.execute_command()
		}
	}
}
