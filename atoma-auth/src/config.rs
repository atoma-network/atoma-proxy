use config::Config;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Configuration for Postgres database connection.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AtomaAuthConfig {
    /// The secret key for JWT authentication.
    pub secret_key: String,
    /// The access token lifetime in minutes.
    pub access_token_lifetime: usize,
    /// The refresh token lifetime in days.
    pub refresh_token_lifetime: usize,
}

impl AtomaAuthConfig {
    /// Constructor
    pub fn new(
        secret_key: String,
        access_token_lifetime: usize,
        refresh_token_lifetime: usize,
    ) -> Self {
        Self {
            secret_key,
            access_token_lifetime,
            refresh_token_lifetime,
        }
    }

    /// Creates a new `AtomaAuthConfig` instance from a configuration file.
    ///
    /// # Arguments
    ///
    /// * `config_file_path` - A path-like object representing the location of the configuration file.
    ///
    /// # Returns
    ///
    /// Returns a new `AtomaAuthConfig` instance populated with values from the configuration file.
    ///
    /// # Panics
    ///
    /// This method will panic if:
    /// - The configuration file cannot be read or parsed.
    /// - The "atoma-auth" section is missing from the configuration file.
    /// - The required fields are missing or have invalid types in the configuration file.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use std::path::Path;
    /// use atoma_node::atoma_state::AtomaAuthConfig;
    ///
    /// let config = AtomaAuthConfig::from_file_path("path/to/config.toml");
    /// ```
    pub fn from_file_path<P: AsRef<Path>>(config_file_path: P) -> Self {
        let builder = Config::builder()
            .add_source(config::File::with_name(
                config_file_path.as_ref().to_str().unwrap(),
            ))
            .add_source(
                config::Environment::with_prefix("ATOMA_AUTH")
                    .keep_prefix(true)
                    .separator("__"),
            );
        let config = builder
            .build()
            .expect("Failed to generate atoma state configuration file");
        config
            .get::<Self>("atoma_auth")
            .expect("Failed to generate configuration instance")
    }
}
