use thiserror::Error;
use tracing::{error, instrument, trace};

use crate::timer::Modalities;

#[derive(Error, Debug)]
pub enum GpuMetricError {
    #[error("GPU memory used: expected {expected}, found {found}")]
    MemoryUsedMismatch { expected: usize, found: usize },
    #[error("GPU memory total: expected {expected}, found {found}")]
    MemoryTotalMismatch { expected: usize, found: usize },
    #[error("GPU memory free: expected {expected}, found {found}")]
    MemoryFreeMismatch { expected: usize, found: usize },
    #[error("GPU read/write time: expected {expected}, found {found}")]
    ReadWriteTimeMismatch { expected: usize, found: usize },
    #[error("GPU execution time: expected {expected}, found {found}")]
    ExecutionTimeMismatch { expected: usize, found: usize },
    #[error("GPU temperatures: expected {expected}, found {found}")]
    TemperatureMismatch { expected: usize, found: usize },
    #[error("GPU power usage: expected {expected}, found {found}")]
    PowerUsageMismatch { expected: usize, found: usize },
}

#[derive(Debug, thiserror::Error)]
pub enum MetricsServiceError {
    #[error("IO error: {0}")]
    Io(std::io::Error),
    #[error("Prometheus error: {0}")]
    Prometheus(#[from] prometheus::Error),
    #[error("Reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("Failed to trigger new metrics collection task: {0}")]
    TimerError(String),
    #[error("Failed to send best available nodes to channel: {0}")]
    FailedToSendBestAvailableNodesToChannel(
        #[from] tokio::sync::mpsc::error::SendError<Vec<(Modalities, Vec<i64>)>>,
    ),
}

#[derive(Error, Debug)]
pub enum QuoteVerificationError {
    #[error("Invalid quote format: {0}")]
    InvalidQuoteFormat(String),

    #[error("Collateral retrieval timeout after {timeout_secs} seconds")]
    CollateralRetrievalTimeout { timeout_secs: u64 },

    #[error("Report data mismatch - expected: {expected:?}, got: {actual:?}")]
    InvalidReportData { expected: Vec<u8>, actual: Vec<u8> },

    #[error("Unsupported report type: {0}")]
    UnsupportedReportType(String),

    #[error("Verification failed: {0}")]
    VerificationFailed(String),
}

#[derive(Error, Debug)]
pub enum AtomaStateManagerError {
    #[error("Failed to connect to the database: {0}")]
    DatabaseConnectionError(#[from] sqlx::Error),
    #[error("Database url is malformed")]
    DatabaseUrlError,
    #[error("Stack not found")]
    StackNotFound,
    #[error("Attestation node not found: {0}")]
    AttestationNodeNotFound(i64),
    #[error("Invalid Merkle leaf length")]
    InvalidMerkleLeafLength,
    #[error("Invalid committed stack proof length")]
    InvalidCommittedStackProofLength,
    #[error("Failed to parse JSON: {0}")]
    JsonParseError(#[from] serde_json::Error),
    #[error("Failed to retrieve existing total hash for stack: `{0}`")]
    FailedToRetrieveExistingTotalHash(i64),
    #[error("Failed to send result to channel")]
    ChannelSendError,
    #[error("Invalid timestamp")]
    InvalidTimestamp,
    #[error("Failed to run migrations")]
    FailedToRunMigrations(#[from] sqlx::migrate::MigrateError),
    #[error("Node not found")]
    NodeNotFound,
    #[error("Node small id ownership verification failed")]
    NodeSmallIdOwnershipVerificationFailed,
    #[error("Failed to verify quote: `{0}`")]
    FailedToVerifyQuote(String),
    #[error("Failed to parse quote: `{0}`")]
    FailedToParseQuote(String),
    #[error("Unix time went backwards: `{0}`")]
    UnixTimeWentBackwards(String),
    #[error("Failed to retrieve collateral: `{0}`")]
    FailedToRetrieveCollateral(String),
    #[error("Failed to retrieve fmspc: `{0}`")]
    FailedToRetrieveFmspc(String),
    #[error("Insufficient balance")]
    InsufficientBalance,
    #[error("Country is not a valid ISO 3166-1 alpha-2 code: {0}")]
    InvalidCountry(String),
    #[error("URL is not valid: {0}")]
    InvalidUrl(String),
    #[error("{0}")]
    GpuMetricError(#[from] GpuMetricError),
    #[error("{0}")]
    QuoteVerificationError(#[from] QuoteVerificationError),
}

#[allow(clippy::too_many_arguments)]
#[instrument(level = "trace", skip_all)]
pub fn validate_gpu_metrics(
    num_gpus: u32,
    gpu_memory_used: &[u64],
    gpu_memory_total: &[u64],
    gpu_memory_free: &[u64],
    gpu_percentage_time_read_write: &[u32],
    gpu_percentage_time_execution: &[u32],
    gpu_temperatures: &[u32],
    gpu_power_usages: &[u32],
) -> Result<(), AtomaStateManagerError> {
    trace!(
        target = "atoma-state-handlers",
        event = "validate-gpu-metrics",
        "Validating GPU metrics"
    );

    let expected = num_gpus as usize;
    if num_gpus == 0 {
        return Ok(());
    }

    if gpu_memory_used.len() != expected {
        error!(
            target = "atoma-state-handlers",
            event = "validate-gpu-metrics",
            "Invalid GPU memory used count: expected {}, found {}",
            expected,
            gpu_memory_used.len()
        );
        return Err(GpuMetricError::MemoryUsedMismatch {
            expected,
            found: gpu_memory_used.len(),
        }
        .into());
    }

    if gpu_memory_total.len() != expected {
        error!(
            target = "atoma-state-handlers",
            event = "validate-gpu-metrics",
            "Invalid GPU memory total count: expected {}, found {}",
            expected,
            gpu_memory_total.len()
        );
        return Err(GpuMetricError::MemoryTotalMismatch {
            expected,
            found: gpu_memory_total.len(),
        }
        .into());
    }

    if gpu_memory_free.len() != expected {
        error!(
            target = "atoma-state-handlers",
            event = "validate-gpu-metrics",
            "Invalid GPU memory free count: expected {}, found {}",
            expected,
            gpu_memory_free.len()
        );
        return Err(GpuMetricError::MemoryFreeMismatch {
            expected,
            found: gpu_memory_free.len(),
        }
        .into());
    }

    if gpu_percentage_time_read_write.len() != expected {
        error!(
            target = "atoma-state-handlers",
            event = "validate-gpu-metrics",
            "Invalid GPU read/write time count: expected {}, found {}",
            expected,
            gpu_percentage_time_read_write.len()
        );
        return Err(GpuMetricError::ReadWriteTimeMismatch {
            expected,
            found: gpu_percentage_time_read_write.len(),
        }
        .into());
    }

    if gpu_percentage_time_execution.len() != expected {
        error!(
            target = "atoma-state-handlers",
            event = "validate-gpu-metrics",
            "Invalid GPU execution time count: expected {}, found {}",
            expected,
            gpu_percentage_time_execution.len()
        );
        return Err(GpuMetricError::ExecutionTimeMismatch {
            expected,
            found: gpu_percentage_time_execution.len(),
        }
        .into());
    }

    if gpu_temperatures.len() != expected {
        error!(
            target = "atoma-state-handlers",
            event = "validate-gpu-metrics",
            "Invalid GPU temperature count: expected {}, found {}",
            expected,
            gpu_temperatures.len()
        );
        return Err(GpuMetricError::TemperatureMismatch {
            expected,
            found: gpu_temperatures.len(),
        }
        .into());
    }

    if gpu_power_usages.len() != expected {
        error!(
            target = "atoma-state-handlers",
            event = "validate-gpu-metrics",
            "Invalid GPU power usage count: expected {}, found {}",
            expected,
            gpu_power_usages.len()
        );
        return Err(GpuMetricError::PowerUsageMismatch {
            expected,
            found: gpu_power_usages.len(),
        }
        .into());
    }

    Ok(())
}
