use super::super::super::model::Token;
use super::chars;
use super::Tokenizer;
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;

pub(super) fn is_include(tokenizer: &mut Tokenizer) -> Result<(), String> {
	match tokenizer.chars.get(tokenizer.pos) {
		None => Ok(()),
		Some(_) => {
			if chars::consume_string(tokenizer, "include ") {
				let mut file = String::new();
				while let Some(&c) = tokenizer.chars.get(tokenizer.pos) {
					file.push(c);
					tokenizer.pos += 1;
				}
				let mut files: Vec<PathBuf> = Vec::new();
				if file.starts_with('/') {
					files.push(PathBuf::from("/"));
				}
				for token in file.split('/') {
					if token == "**.*" {
						let mut inc: Vec<PathBuf> = Vec::new();
						for file in files {
							if let Err(err) = add_files(&file, &mut inc, true, |p| p.is_file()) {
								return Err(format!("{}", err));
							}
						}
						files = inc;
					} else if token == "*.*" {
						let mut inc = Vec::new();
						for file in files {
							if let Err(err) = add_files(&file, &mut inc, false, |p| p.is_file()) {
								return Err(format!("{}", err));
							}
						}
						files = inc;
					} else if token.starts_with("*.") {
						let mut inc = Vec::new();
						for file in files {
							if let Err(err) = add_files(&file, &mut inc, false, |p| {
								p.is_file() && p.extension() == Path::new(token).extension()
							}) {
								return Err(format!("{}", err));
							}
						}
						files = inc;
					} else if token == "*" {
						let mut inc: Vec<PathBuf> = Vec::new();
						for file in files {
							if let Err(err) = add_directories(&file, &mut inc, false) {
								return Err(format!("{}", err));
							}
						}
						files = inc;
					} else if token == "**" {
						let mut inc: Vec<PathBuf> = Vec::new();
						for file in files {
							if let Err(err) = add_directories(&file, &mut inc, true) {
								return Err(format!("{}", err));
							}
						}
						files = inc;
					} else {
						match files.last_mut() {
							None => {
								let parent = tokenizer.file.parent().unwrap_or_else(|| {
									panic!(
										"file \"{}\" has no parent directory",
										tokenizer.file.display()
									)
								});
								let mut file = PathBuf::from(parent);
								file.push(token);
								files.push(file)
							}
							Some(file) => {
								file.push(Path::new(token));
							}
						}
					}
				}
				for file in files {
					super::super::parse_file(&file, tokenizer.transactions)?;
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

fn add_files<P>(
	base: &Path,
	files: &mut Vec<PathBuf>,
	resurive: bool,
	predicate: P,
) -> Result<(), io::Error>
where
	P: FnOnce(&Path) -> bool + Copy,
{
	let mut paths = fs::read_dir(&base)?
		.map(|res| res.map(|e| e.path()))
		.collect::<Result<Vec<PathBuf>, io::Error>>()?;
	paths.sort();
	for path in paths {
		if resurive && path.is_dir() {
			add_files(&path, files, resurive, predicate)?;
		}
		if predicate(&path) {
			files.push(path);
		}
	}
	Ok(())
}

pub(super) fn is_alias(tokenizer: &mut Tokenizer) -> Result<(), String> {
	// check if is some check removable
	if tokenizer.chars.get(tokenizer.pos).is_some() {
		if chars::consume_string(tokenizer, "alias ") {
			let mut alias = String::new();
			while let Some(&c) = tokenizer.chars.get(tokenizer.pos) {
				alias.push(c);
				tokenizer.pos += 1;
			}
			tokenizer.tokens.push(Token::Alias(tokenizer.index, alias));
		}
	}
	Ok(())
}
