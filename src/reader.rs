pub fn read_file(file: &str) -> Result<String, String> {
	match std::fs::read_to_string(file) {
		Err(err) => Err(format!("While parsing \"{}\"\nError: {}", file, err)),
		Ok(content) => Ok(content),
	}
}
