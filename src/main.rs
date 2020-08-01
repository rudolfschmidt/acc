mod lib;

use lib::commands;
use lib::model;
use lib::parsers;

use std::env;

enum Command {
	Print,
	Balance,
	Register,
	Accounts,
	Codes,
}

#[derive(PartialEq)]
enum Argument {
	Flat,
	Tree,
	Raw,
	Explicit,
}

fn main() {
	if let Err(e) = start() {
		eprintln!("{}", e);
	}
}

fn start() -> Result<(), String> {
	let mut files: Vec<String> = Vec::new();
	let mut command = None;
	let mut arguments = Vec::new();
	let mut tokens = Vec::new();
	let mut transactions = Vec::new();

	parse_arguments(&mut files, &mut command, &mut arguments)?;

	match command {
		None => Err(String::from("Error : No command selected")),
		Some(command) => {
			if files.is_empty() {
				return Err(String::from(
					"Error : No file(s) reselected. Try --file <file> to select a file",
				));
			}
			for file in files {
				parsers::parse_file(std::path::Path::new(&file), &mut tokens, &mut transactions)?;
			}

			execute_command(command, arguments, &transactions)
		}
	}
}

fn parse_arguments(
	files: &mut Vec<String>,
	command: &mut Option<Command>,
	arguments: &mut Vec<Argument>,
) -> Result<(), String> {
	let mut it = env::args().skip(1);

	while let Some(arg) = it.next() {
		match arg.as_str() {
			"--file" | "-f" => match it.next() {
				None => return Err(String::from("Error : No argument provided for --file")),
				Some(file_path) => files.push(file_path),
			},
			"--flat" => arguments.push(Argument::Flat),
			"--tree" => arguments.push(Argument::Tree),
			"--raw" => arguments.push(Argument::Raw),
			"--explicit" | "-x" => arguments.push(Argument::Explicit),
			"balance" | "bal" => *command = Some(Command::Balance),
			"register" | "reg" => *command = Some(Command::Register),
			"print" => *command = Some(Command::Print),
			"accounts" => *command = Some(Command::Accounts),
			"codes" => *command = Some(Command::Codes),
			_ => {}
		}
	}

	Ok(())
}

fn execute_command(
	command: Command,
	arguments: Vec<Argument>,
	transactions: &[model::Transaction],
) -> Result<(), String> {
	match command {
		Command::Balance => {
			if arguments.contains(&Argument::Flat) {
				return commands::balance::print_flat(transactions);
			}
			if arguments.contains(&Argument::Tree) {
				return commands::balance::print_tree(transactions);
			}
			return commands::balance::print_tree(transactions);
		}
		Command::Register => commands::register::print(transactions)?,
		Command::Print => {
			if arguments.contains(&Argument::Explicit) {
				return commands::print::print_explicit(transactions);
			}
			if arguments.contains(&Argument::Raw) {
				return commands::print::print_raw(transactions);
			}
			return commands::print::print_raw(transactions);
		}
		Command::Accounts => {
			if arguments.contains(&Argument::Flat) {
				return commands::accounts::print_flat(transactions);
			}
			if arguments.contains(&Argument::Tree) {
				return commands::accounts::print_tree(transactions);
			}
			return commands::accounts::print_tree(transactions);
		}
		Command::Codes => commands::codes::print(transactions)?,
	}
	Ok(())
}
