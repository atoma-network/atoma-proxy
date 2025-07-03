use config::{Config, File};
use serde::Deserialize;
use std::path::Path;
use thiserror::Error;
use url::Url;

#[derive(Error, Debug)]
pub enum ProxyServiceConfigError {
    #[error("Invalid service bind address: {0}")]
    InvalidBindAddress(String),

    #[error("Invalid Grafana URL: {0}")]
    InvalidGrafanaUrl(String),

    #[error("Missing required field: {0}")]
    MissingField(String),

    #[error("Configuration file error: {0}")]
    FileError(#[from] config::ConfigError),
}

/// Configuration for the Atoma proxy service
///
/// This struct holds the configuration parameters needed to run the Atoma Proxy Service,
/// including service binding information and node badge definitions.
#[derive(Debug, Deserialize)]
pub struct AtomaProxyServiceConfig {
    /// The address and port where the service will listen for connections
    /// Format: "host:port" (e.g., "127.0.0.1:8080")
    pub service_bind_address: String,

    /// Grafana URL
    pub grafana_url: String,

    /// Grafana api token (read only access is sufficient)
    pub grafana_api_token: String,

    /// Only dashboards tagged with this tag will be proxied as graphs
    pub grafana_dashboard_tag: String,

    /// Only dashboards tagged with this tag will be proxied as stats
    pub grafana_stats_tag: String,

    /// Enable Grafana dashboard and stats proxy
    pub enable_grafana_proxy: Option<bool>,

    /// Password for the settings page
    pub settings_password: String,
}

impl AtomaProxyServiceConfig {
    /// Validates the proxy service configuration
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the configuration is valid, or a `ProxyServiceConfigError` if there are any validation errors.
    ///
    /// # Errors
    ///
    /// Returns a `ProxyServiceConfigError` if:
    /// * The service bind address is empty
    /// * The Grafana URL is empty or invalid
    /// * The Grafana API token is empty
    /// * The Grafana dashboard tag is empty
    /// * The Grafana stats tag is empty
    pub fn validate(&self) -> Result<(), ProxyServiceConfigError> {
        // Validate service bind address
        if self.service_bind_address.is_empty() {
            return Err(ProxyServiceConfigError::MissingField(
                "service_bind_address".to_string(),
            ));
        }

        // Validate Grafana URL
        if self.grafana_url.is_empty() {
            return Err(ProxyServiceConfigError::MissingField(
                "grafana_url".to_string(),
            ));
        }
        if Url::parse(&self.grafana_url).is_err() {
            return Err(ProxyServiceConfigError::InvalidGrafanaUrl(
                self.grafana_url.clone(),
            ));
        }

        // Validate Grafana API token
        if self.grafana_api_token.is_empty() {
            return Err(ProxyServiceConfigError::MissingField(
                "grafana_api_token".to_string(),
            ));
        }

        // Validate Grafana dashboard tag
        if self.grafana_dashboard_tag.is_empty() {
            return Err(ProxyServiceConfigError::MissingField(
                "grafana_dashboard_tag".to_string(),
            ));
        }

        // Validate Grafana stats tag
        if self.grafana_stats_tag.is_empty() {
            return Err(ProxyServiceConfigError::MissingField(
                "grafana_stats_tag".to_string(),
            ));
        }

        Ok(())
    }

    /// Creates a new AtomaProxyServiceConfig instance from a configuration file
    ///
    /// # Arguments
    ///
    /// * `config_file_path` - Path to the configuration file. The file should be in a format
    ///   supported by the `config` crate (e.g., TOML, JSON, YAML) and
    ///   contain an "atoma-proxy-service" section with the required configuration
    ///   parameters.
    ///
    /// # Returns
    ///
    /// Returns a new `AtomaProxyServiceConfig` instance populated with values from the config file.
    ///
    /// # Errors
    ///
    /// Returns a `ProxyServiceConfigError` if:
    /// * The configuration file cannot be read or parsed
    /// * The "atoma-proxy-service" section is missing from the configuration
    /// * The configuration format doesn't match the expected structure
    /// * The configuration fails validation
    ///
    /// # Panics
    ///
    /// Panics if the path cannot be converted to a string.
    pub fn from_file_path<P: AsRef<Path>>(
        config_file_path: P,
    ) -> Result<Self, ProxyServiceConfigError> {
        let builder = Config::builder()
            .add_source(File::with_name(config_file_path.as_ref().to_str().unwrap()))
            .add_source(
                config::Environment::with_prefix("ATOMA_PROXY_SERVICE")
                    .keep_prefix(true)
                    .separator("__"),
            );
        let config = builder.build()?;
        let config = config.get::<Self>("atoma_proxy_service")?;

        // Validate the configuration
        config.validate()?;

        Ok(config)
    }
}
