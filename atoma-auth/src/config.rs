use config::{Config, File};
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;
use validator::Validate;

#[derive(Error, Debug)]
pub enum AuthConfigError {
    #[error("Invalid auth configuration: {0}")]
    InvalidConfig(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Configuration file error: {0}")]
    FileError(#[from] config::ConfigError),

    #[error("Validation error: {0}")]
    ValidationError(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, Validate)]
pub struct AtomaAuthConfig {
    /// Access token validity duration in minutes
    #[validate(range(min = 1, message = "access token lifetime must be at least 1 minute"))]
    pub access_token_lifetime: usize,

    /// Google OAuth client ID (required only when google-oauth feature is enabled)
    #[validate(length(
        min = 1,
        message = "google client ID cannot be empty when google-oauth is enabled"
    ))]
    #[cfg(feature = "google-oauth")]
    pub google_client_id: String,

    /// Refresh token validity duration in days
    #[validate(range(min = 1, message = "refresh token lifetime must be at least 1 day"))]
    pub refresh_token_lifetime: usize,

    /// JWT signing key for token generation
    #[validate(length(min = 1, message = "secret key cannot be empty"))]
    pub secret_key: String,
}

impl AtomaAuthConfig {
    #[must_use]
    pub const fn new(
        secret_key: String,
        access_token_lifetime: usize,
        refresh_token_lifetime: usize,
        #[cfg(feature = "google-oauth")] google_client_id: String,
    ) -> Self {
        Self {
            access_token_lifetime,
            #[cfg(feature = "google-oauth")]
            google_client_id,
            refresh_token_lifetime,
            secret_key,
        }
    }

    /// Validates the auth configuration
    ///
    /// # Errors
    ///
    /// Returns `AuthConfigError::ValidationError` if any validation rules fail
    pub fn validate(&self) -> Result<(), AuthConfigError> {
        Validate::validate(self).map_err(|e| AuthConfigError::ValidationError(e.to_string()))
    }

    /// Loads configuration from a file path
    ///
    /// # Errors
    ///
    /// Returns `AuthConfigError` if the configuration file cannot be read or parsed
    ///
    /// # Panics
    ///
    /// Panics if the path cannot be converted to a string
    pub fn from_file_path<P: AsRef<Path>>(config_file_path: P) -> Result<Self, AuthConfigError> {
        let builder = Config::builder()
            .add_source(File::with_name(config_file_path.as_ref().to_str().unwrap()))
            .add_source(
                config::Environment::with_prefix("ATOMA_AUTH")
                    .keep_prefix(true)
                    .separator("__"),
            );
        let config = builder.build()?;
        let config = config.get::<Self>("atoma_auth")?;

        // Validate the configuration
        config.validate()?;

        Ok(config)
    }
}
