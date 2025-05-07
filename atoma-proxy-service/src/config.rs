use config::{Config, File};
use serde::Deserialize;
use std::path::Path;
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

    /// Sentry DSN for error reporting
    pub sentry_dsn: Option<String>,

    /// Environment
    pub environment: Option<String>,
}

impl AtomaProxyServiceConfig {
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
    /// # Panics
    ///
    /// This method will panic if:
    /// * The configuration file cannot be read or parsed
    /// * The "atoma-proxy-service" section is missing from the configuration
    /// * The configuration format doesn't match the expected structure
    pub fn from_file_path<P: AsRef<Path>>(config_file_path: P) -> Self {
        let builder = Config::builder()
            .add_source(File::with_name(config_file_path.as_ref().to_str().unwrap()))
            .add_source(
                config::Environment::with_prefix("ATOMA_PROXY_SERVICE")
                    .keep_prefix(true)
                    .separator("__"),
            );
        let config = builder
            .build()
            .expect("Failed to generate atoma-proxy-service configuration file");
        config
            .get::<Self>("atoma_proxy_service")
            .expect("Failed to generate configuration instance")
    }
}
