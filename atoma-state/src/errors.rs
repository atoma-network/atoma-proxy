use atoma_utils::compression::CompressionError;
use thiserror::Error;
use tracing::error;

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
    #[error("Model not found: {0}")]
    ModelNotFound(String),
    #[error("Flume recv error: {0}")]
    FlumeRecvError(#[from] flume::RecvError),
    #[error("Failed to send best available nodes to channel")]
    ChannelSendError,
    #[error("Node small id label not found")]
    NodeSmallIdLabelNotFound,
}

#[derive(Error, Debug)]
pub enum RemoteAttestationVerificationError {
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

    #[error("Invalid nonce: {0}")]
    InvalidNonce(String),
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
    RemoteAttestationVerificationError(#[from] RemoteAttestationVerificationError),
    #[error("Compression error: {0}")]
    CompressionError(#[from] CompressionError),
    #[error("Remote attestation error: {0}")]
    RemoteAttestationError(#[from] AtomaStateRemoteAttestationError),
    #[error("User not found with id: {0}")]
    UserNotFound(i64),
}

#[derive(Error, Debug)]
pub enum AtomaStateRemoteAttestationError {
    #[error("Failed to attest remote: {0}")]
    FailedToAttestRemote(#[from] remote_attestation_verifier::AttestError),
    #[error("Failed to retrieve contract nonce")]
    FailedToRetrieveContractNonce,
}
