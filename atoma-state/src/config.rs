use config::Config;
use serde::{Deserialize, Serialize};
use std::path::Path;
use url::Url;
use validator::{Validate, ValidationError};

use crate::config_error::ConfigError;
use crate::types::Modalities;

/// Configuration for the Atoma State Manager instance.
#[derive(Clone, Debug, Deserialize, Serialize, Validate)]
pub struct AtomaStateManagerConfig {
    /// The URL of the Postgres database.
    #[validate(custom(function = "validate_postgres_url"))]
    pub database_url: String,

    /// The configuration for metrics collection.
    #[validate(nested)]
    pub metrics_collection: MetricsCollectionConfig,
}

/// Configuration for metrics collection.
#[derive(Debug, Clone, Deserialize, Serialize, Validate)]
pub struct MetricsCollectionConfig {
    /// The URL endpoint where metrics can be collected from.
    #[validate(url(message = "metrics_url must be a valid URL"))]
    pub metrics_url: String,

    /// A vector of tuples containing modality types and their corresponding model identifiers.
    #[validate(length(min = 1, message = "at least one model must be specified"))]
    #[validate(custom(function = "validate_models"))]
    pub models: Vec<(Modalities, String)>,

    /// Optional parameter to limit the number of best nodes returned.
    #[validate(range(min = 1, message = "top_k must be greater than 0"))]
    pub top_k: Option<usize>,
}

fn validate_postgres_url(url: &str) -> Result<(), ValidationError> {
    if url.is_empty() {
        return Err(ValidationError::new("empty_database_url"));
    }

    // Basic PostgreSQL URL format validation
    if !url.starts_with("postgresql://") && !url.starts_with("postgres://") {
        return Err(ValidationError::new("invalid_postgres_url_format"));
    }

    // Try to parse as URL
    if Url::parse(url).is_err() {
        return Err(ValidationError::new("invalid_url_format"));
    }

    Ok(())
}

fn validate_models(models: &[(Modalities, String)]) -> Result<(), ValidationError> {
    for (modality, model_id) in models {
        if model_id.is_empty() {
            return Err(ValidationError::new("empty_model_id"));
        }
        match modality {
            Modalities::ChatCompletions
            | Modalities::Embeddings
            | Modalities::ImagesGenerations => (),
        }
    }
    Ok(())
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

    /// Validates the configuration
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the configuration is valid, or a `ConfigError` if there are any validation errors.
    ///
    /// # Errors
    ///
    /// Returns a `ConfigError` if:
    /// - The database URL is invalid or empty
    /// - The metrics URL is invalid or empty
    /// - The models configuration is invalid
    /// - The top_k value is invalid
    pub fn validate(&self) -> Result<(), ConfigError> {
        Validate::validate(self).map_err(|e| ConfigError::ValidationError(e.to_string()))
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
    /// # Errors
    ///
    /// Returns a `ConfigError` if:
    /// - The configuration file cannot be read or parsed
    /// - The "atoma-state" section is missing from the configuration file
    /// - The required fields are missing or have invalid types
    /// - The configuration fails validation
    ///
    /// # Panics
    ///
    /// This method will panic if the path cannot be converted to a string.
    pub fn from_file_path<P: AsRef<Path>>(config_file_path: P) -> Result<Self, ConfigError> {
        let builder = Config::builder()
            .add_source(config::File::with_name(
                config_file_path.as_ref().to_str().unwrap(),
            ))
            .add_source(
                config::Environment::with_prefix("ATOMA_STATE")
                    .keep_prefix(true)
                    .separator("__"),
            );
        let config = builder.build()?;
        let config = config.get::<Self>("atoma_state")?;

        // Validate the configuration
        config.validate()?;

        Ok(config)
    }
}
