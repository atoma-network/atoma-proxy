use config::Config;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Configuration for Postgres database connection.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AtomaStateManagerConfig {
    /// The URL of the Postgres database.
    pub database_url: String,

    /// Component score weights (0.0 to 1.0)
    pub component_weights: ComponentsWeights,

    /// GPU-specific thresholds and weights
    pub gpu: GpuMetricsWeights,

    /// RAM-specific thresholds and weights
    pub ram: RamMetricsWeights,

    /// Moving average configuration
    pub moving_average: MovingAverageConfig,
}

/// Overall component weights in performance calculation
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ComponentsWeights {
    /// Weight factor for GPU performance
    pub gpu_score_weight: f64,

    /// Weight factor for CPU performance
    pub cpu_score_weight: f64,

    /// Weight factor for RAM performance
    pub ram_score_weight: f64,

    /// Weight factor for swap RAM performance
    pub swap_ram_score_weight: f64,

    /// Weight factor for network performance
    pub network_score_weight: f64,
}

/// Metrics for GPU components
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GpuMetricsWeights {
    /// Weight factor for VRAM usage
    pub gpu_vram_weight: f64,

    /// Weight factor for GPU execution
    pub gpu_execution_weight: f64,

    /// Temperature threshold for GPU
    pub gpu_temp_threshold: f64,

    /// Maximum temperature for GPU
    pub gpu_temp_max: f64,

    /// Power threshold for GPU
    pub gpu_power_threshold: f64,

    /// Maximum power for GPU
    pub gpu_power_max: f64,
}

/// Metrics for RAM usage
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RamMetricsWeights {
    /// Weight factor for RAM usage
    pub ram_usage_weight: f64,

    /// Weight factor for swap RAM usage
    pub swap_ram_usage_weight: f64,
}

/// Moving average configuration
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MovingAverageConfig {
    /// Number of samples to use for the moving average
    pub moving_avg_window_size: i32,

    /// Smooth factor for the moving average
    pub moving_avg_smooth_factor: f64,
}

impl AtomaStateManagerConfig {
    /// Constructor
    #[must_use]
    pub const fn new(
        database_url: String,
        component_weights: ComponentsWeights,
        gpu: GpuMetricsWeights,
        ram: RamMetricsWeights,
        moving_average: MovingAverageConfig,
    ) -> Self {
        Self {
            database_url,
            component_weights,
            gpu,
            ram,
            moving_average,
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
