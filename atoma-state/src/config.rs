use config::Config;
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::types::Modalities;

/// Configuration for the Atoma State Manager instance.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AtomaStateManagerConfig {
    /// The URL of the Postgres database.
    pub database_url: String,

    /// The configuration for metrics collection.
    pub metrics_collection: MetricsCollectionConfig,
}

/// Configuration for metrics collection.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MetricsCollectionConfig {
    /// The URL endpoint where metrics can be collected from.
    pub metrics_url: String,

    /// A vector of tuples containing modality types and their corresponding model identifiers.
    pub models: Vec<(Modalities, String)>,

    /// Optional parameter to limit the number of best nodes returned.
    pub top_k: Option<usize>,
}

impl AtomaStateManagerConfig {
    /// Constructor
    #[must_use]
    pub const fn new(database_url: String, metrics_collection: MetricsCollectionConfig) -> Self {
        Self {
            database_url,
            metrics_collection,
        }
    }

    /// Creates a new `AtomaStateManagerConfig` instance from a configuration file.
    ///
    /// # Arguments
    ///
    /// * `config_file_path` - A path-like object representing the location of the configuration file.
    ///
    /// # Returns
    ///
    /// Returns a new `AtomaStateManagerConfig` instance populated with values from the configuration file.
    ///
    /// # Panics
    ///
    /// This method will panic if:
    /// - The configuration file cannot be read or parsed.
    /// - The "atoma-state" section is missing from the configuration file.
    /// - The required fields are missing or have invalid types in the configuration file.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use std::path::Path;
    /// use atoma_node::atoma_state::AtomaStateManagerConfig;
    ///
    /// let config = AtomaStateManagerConfig::from_file_path("path/to/config.toml");
    /// ```
    pub fn from_file_path<P: AsRef<Path>>(config_file_path: P) -> Self {
        let builder = Config::builder()
            .add_source(config::File::with_name(
                config_file_path.as_ref().to_str().unwrap(),
            ))
            .add_source(
                config::Environment::with_prefix("ATOMA_STATE")
                    .keep_prefix(true)
                    .separator("__"),
            );
        let config = builder
            .build()
            .expect("Failed to generate atoma state configuration file");
        config
            .get::<Self>("atoma_state")
            .expect("Failed to generate configuration instance")
    }
}
