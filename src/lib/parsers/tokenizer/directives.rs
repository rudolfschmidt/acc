use super::super::super::model::Token;
use super::chars;
use super::Tokenizer;
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;

pub(super) fn is_include(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Ok(()),
		Some(_) => {
			if chars::is_string(tokenizer, "include ") {
				let mut file = String::new();
				while let Some(&c) = tokenizer.line_chars.get(tokenizer.line_pos) {
					file.push(c);
					tokenizer.line_pos += 1;
				}
				let mut files: Vec<PathBuf> = Vec::new();
				if file.starts_with("/") {
					files.push(PathBuf::from("/"));
				}
				for token in file.split('/') {
					if token == "*" {
						let mut inc_dirs: Vec<PathBuf> = Vec::new();
						for file in files {
							if let Err(err) = add_directories(&file, &mut inc_dirs, false) {
								return Err(format!("{}", err));
							}
						}
						files = inc_dirs;
					} else if token == "**" {
						let mut included: Vec<PathBuf> = Vec::new();
						for file in files {
							if let Err(err) = add_directories(&file, &mut included, true) {
								return Err(format!("{}", err));
							}
						}
						files = included;
					} else if token == "*.*" {
						let mut inc_dirs = Vec::new();
						for file in files {
							if let Err(err) = add_files(&file, &mut inc_dirs, |p| p.is_file()) {
								return Err(format!("{}", err));
							}
						}
						files = inc_dirs;
					} else if token.starts_with("*.") {
						let mut inc_files = Vec::new();
						for file in files {
							if let Err(err) = add_files(&file, &mut inc_files, |p| {
								p.is_file() && p.extension() == Path::new(token).extension()
							}) {
								return Err(format!("{}", err));
							}
						}
						files = inc_files;
					} else {
						match files.last_mut() {
							None => files.push(PathBuf::from(token)),
							Some(p) => p.push(Path::new(token)),
						}
					}
				}
				for file in files {
					tokenizer.ledger.read_tokens(&file)?;
				}
			}
			Ok(())
		}
	}
}

fn add_directories(
	base: &Path,
	files: &mut Vec<PathBuf>,
	resursive: bool,
) -> Result<(), io::Error> {
	let mut paths = fs::read_dir(base)?
		.map(|res| res.map(|e| e.path()))
		.collect::<Result<Vec<PathBuf>, io::Error>>()?;
	paths.sort();
	for path in paths {
		if path.is_dir() {
			if resursive {
				add_directories(&path, files, resursive)?;
			}
			files.push(path);
		}
	}
	Ok(())
}

fn add_files<P>(base: &Path, files: &mut Vec<PathBuf>, predicate: P) -> Result<(), io::Error>
where
	P: Fn(&Path) -> bool,
{
	let mut paths = fs::read_dir(&base)?
		.map(|res| res.map(|e| e.path()))
		.collect::<Result<Vec<PathBuf>, io::Error>>()?;
	paths.sort();
	for path in paths {
		if predicate(&path) {
			files.push(path);
		}
	}
	Ok(())
}

pub(super) fn is_alias(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.line_chars.get(tokenizer.line_pos) {
		None => Ok(()),
		Some(_) => {
			if chars::is_string(tokenizer, "alias ") {
				let mut alias = String::new();
				while let Some(&c) = tokenizer.line_chars.get(tokenizer.line_pos) {
					alias.push(c);
					tokenizer.line_pos += 1;
				}
				tokenizer
					.ledger
					.tokens
					.push(Token::Alias(tokenizer.line_index, alias));
			}
			Ok(())
		}
	}
}
