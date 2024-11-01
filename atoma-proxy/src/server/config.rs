use std::path::Path;

use serde::Deserialize;

use config::{Config, File};

/// Configuration for the Atoma Service.
///
/// This struct holds the configuration options for the Atoma Service,
/// including URLs for various services and a list of models.
#[derive(Debug, Deserialize)]
pub struct AtomaServiceConfig {
    /// Bind address for the Atoma Proxy Server.
    ///
    /// This field specifies the address and port on which the Atoma Proxy Server will bind.
    pub service_bind_address: String,
}

impl AtomaServiceConfig {
    /// Creates a new `AtomaServiceConfig` instance from a configuration file.
    ///
    /// # Arguments
    ///
    /// * `config_file_path` - Path to the configuration file. The file should be in a format
    ///   supported by the `config` crate (e.g., YAML, JSON, TOML) and contain an "atoma-service"
    ///   section with the required configuration fields.
    ///
    /// # Returns
    ///
    /// Returns a new `AtomaServiceConfig` instance populated with values from the config file.
    ///
    /// # Panics
    ///
    /// This method will panic if:
    /// * The configuration file cannot be read or parsed
    /// * The "atoma-service" section is missing from the configuration
    /// * The configuration format doesn't match the expected structure
    pub fn from_file_path<P: AsRef<Path>>(config_file_path: P) -> Self {
        let builder = Config::builder()
            .add_source(File::with_name(config_file_path.as_ref().to_str().unwrap()));
        let config = builder
            .build()
            .expect("Failed to generate atoma-service configuration file");
        config
            .get::<Self>("atoma-service")
            .expect("Failed to generate configuration instance")
    }
}
