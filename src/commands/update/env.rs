use crate::error::Error;

/// Read an API key from an environment variable.
pub fn load_api_key(name: &str) -> Result<String, Error> {
    std::env::var(name).map_err(|_| {
        Error::new(format!("environment variable '{}' is not set", name))
    })
}
