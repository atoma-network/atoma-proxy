use std::path::Path;

use atoma_proxy_service::ModelModality;
use serde::Deserialize;
use thiserror::Error;
use validator::{Validate, ValidationError};

use config::{Config, File};

#[derive(Error, Debug)]
pub enum ServiceConfigError {
    #[error("Invalid service bind address: {0}")]
    InvalidBindAddress(String),

    #[error("Invalid heartbeat URL: {0}")]
    InvalidHeartbeatUrl(String),

    #[error("Invalid model configuration: {0}")]
    InvalidModelConfig(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Configuration file error: {0}")]
    FileError(#[from] config::ConfigError),

    #[error("Validation error: {0}")]
    ValidationError(String),
}

/// Configuration for the Atoma Service.
///
/// This struct holds the configuration options for the Atoma Service,
/// including URLs for various services and a list of models.
#[derive(Debug, Deserialize, Validate)]
pub struct AtomaServiceConfig {
    /// Bind address for the Atoma Proxy Server.
    ///
    /// This field specifies the address and port on which the Atoma Proxy Server will bind.
    #[validate(custom(function = "validate_bind_address"))]
    pub service_bind_address: String,

    /// List of model names.
    ///
    /// This field contains a list of model names that are deployed by the Atoma Service,
    /// on behalf of the node.
    #[validate(length(min = 1, message = "at least one model must be specified"))]
    pub models: Vec<String>,

    /// List of model revisions.
    ///
    /// This field contains a list of the associated model revisions, for each
    /// model that is currently supported by the Atoma Service.
    #[validate(length(min = 1, message = "at least one revision must be specified"))]
    pub revisions: Vec<String>,

    /// List of model modalities.
    ///
    /// This field contains a list of the associated model modalities, for each
    /// model that is currently supported by the Atoma Service.
    #[validate(length(min = 1, message = "at least one modality must be specified"))]
    #[validate(custom(function = "validate_modalities"))]
    pub modalities: Vec<Vec<ModelModality>>,

    /// Hugging face api token.
    ///
    /// This field contains the Hugging Face API token that is used to authenticate
    /// requests to the Hugging Face API.
    #[validate(length(min = 1, message = "HF token cannot be empty"))]
    pub hf_token: String,

    /// Path to open router json.
    #[validate(length(min = 1, message = "open router models file path cannot be empty"))]
    pub open_router_models_file: String,

    /// Heartbeat URL.
    #[validate(url(message = "heartbeat_url must be a valid URL"))]
    pub heartbeat_url: String,

    /// Sentry DSN for error reporting
    pub sentry_dsn: Option<String>,

    /// Environment
    pub environment: Option<String>,
}
/// Validates the bind address format.
///
/// This function checks if the provided bind address is valid by ensuring:
/// - The address is not empty
/// - The address follows the format "host:port"
/// - The port is a valid number between 0 and 65535
///
/// # Arguments
///
/// * `addr` - The bind address string to validate
///
/// # Returns
///
/// Returns `Ok(())` if the address is valid, or a `ValidationError` if:
/// - The address is empty
/// - The address format is invalid (not host:port)
/// - The port is not a valid number
fn validate_bind_address(addr: &str) -> Result<(), ValidationError> {
    if addr.is_empty() {
        return Err(ValidationError::new("empty_bind_address"));
    }

    // Basic format validation for bind address (host:port)
    let parts: Vec<&str> = addr.split(':').collect();
    if parts.len() != 2 {
        return Err(ValidationError::new("invalid_bind_address_format"));
    }

    // Validate port is a number
    if parts[1].parse::<u16>().is_err() {
        return Err(ValidationError::new("invalid_port_number"));
    }

    Ok(())
}

/// Validates the modalities configuration.
///
/// This function checks if the provided modalities configuration is valid by ensuring:
/// - Each model has at least one modality specified
///
/// # Arguments
///
/// * `modalities` - A slice of vectors containing model modalities to validate
///
/// # Returns
///
/// Returns `Ok(())` if the modalities configuration is valid, or a `ValidationError` if:
/// - Any model has an empty modalities list
fn validate_modalities(modalities: &[Vec<ModelModality>]) -> Result<(), ValidationError> {
    for model_modalities in modalities {
        if model_modalities.is_empty() {
            return Err(ValidationError::new("empty_modalities"));
        }
    }
    Ok(())
}

impl AtomaServiceConfig {
    /// Validates the service configuration
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the configuration is valid, or a `ServiceConfigError` if there are any validation errors.
    ///
    /// # Errors
    ///
    /// Returns a `ServiceConfigError` if:
    /// - The service bind address is invalid
    /// - The heartbeat URL is invalid
    /// - The models configuration is invalid
    /// - The HF token is empty
    /// - The open router models file path is empty
    pub fn validate(&self) -> Result<(), ServiceConfigError> {
        Validate::validate(self).map_err(|e| ServiceConfigError::ValidationError(e.to_string()))
    }

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
    /// # Errors
    ///
    /// Returns a `ServiceConfigError` if:
    /// * The configuration file cannot be read or parsed
    /// * The "atoma-service" section is missing from the configuration
    /// * The configuration format doesn't match the expected structure
    /// * The configuration fails validation
    ///
    /// # Panics
    ///
    /// This method will panic if the path cannot be converted to a string.
    pub fn from_file_path<P: AsRef<Path>>(config_file_path: P) -> Result<Self, ServiceConfigError> {
        let builder = Config::builder()
            .add_source(File::with_name(config_file_path.as_ref().to_str().unwrap()))
            .add_source(
                config::Environment::with_prefix("ATOMA_SERVICE")
                    .keep_prefix(true)
                    .separator("__"),
            );
        let config = builder.build()?;
        let config = config.get::<Self>("atoma_service")?;

        // Validate the configuration
        config.validate()?;

        Ok(config)
    }
}
