use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Invalid metrics URL: {0}")]
    InvalidMetricsUrl(String),

    #[error("Invalid database URL: {0}")]
    InvalidDatabaseUrl(String),

    #[error("Invalid model configuration: {0}")]
    InvalidModelConfig(String),

    #[error("Invalid top_k value: {0}")]
    InvalidTopK(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Configuration file error: {0}")]
    FileError(#[from] config::ConfigError),

    #[error("Validation error: {0}")]
    ValidationError(String),
}
