pub fn read_file(file: &str, content: &mut String) -> Result<(), String> {
	match std::fs::read_to_string(file) {
		Err(err) => Err(format!("While parsing \"{}\"\nError: {}", file, err)),
		Ok(data) => {
			*content = data;
			Ok(())
		}
	}
}
