use atoma_sui::events::{
    StackAttestationDisputeEvent, StackCreatedEvent, StackTrySettleEvent, TaskRegisteredEvent,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tokio::sync::oneshot;
use utoipa::ToSchema;

use crate::state_manager::Result;

/// Represents the pricing for a model
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow, ToSchema)]
pub struct Pricing {
    /// Price per one million input compute units
    pub price_per_one_million_input_compute_units: i64,
    /// Price per one million output compute units
    pub price_per_one_million_output_compute_units: i64,
}

/// Request payload for setting custom pricing for a user for a specific model
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow, ToSchema)]
pub struct SetPriceForUserForModel {
    /// The user id for which the pricing is set
    pub user_id: i64,
    /// The model name for which the pricing is set
    pub model_name: String,
    /// Price per one million input compute units
    pub price_per_one_million_input_compute_units: i64,
    /// Price per one million output compute units
    pub price_per_one_million_output_compute_units: i64,
}

/// The modalities that can be used to collect metrics, for each of the
/// currently supported modalities.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub enum Modalities {
    #[serde(rename = "Chat Completions")]
    ChatCompletions,
    #[serde(rename = "Embeddings")]
    Embeddings,
    #[serde(rename = "Images Generations")]
    ImagesGenerations,
}

/// Request payload for revoking an API token
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow, ToSchema)]
pub struct RevokeApiTokenRequest {
    /// The API token id to be revoked
    pub api_token_id: i64,
}

/// Request payload for user authentication
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow, ToSchema)]
pub struct RegisterAuthRequest {
    /// The user's unique identifier
    pub user_profile: UserProfile,
    /// The user's password
    pub password: String,
}

/// Request payload for user authentication
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow, ToSchema)]
pub struct LoginAuthRequest {
    /// The user's unique identifier
    pub email: String,
    /// The user's password
    pub password: String,
}

/// Response returned after successful authentication
///
/// Contains both an access token and a refresh token for implementing token-based authentication:
/// - The access token is used to authenticate API requests
/// - The refresh token is used to obtain new access tokens when they expire
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct AuthResponse {
    /// JWT token used to authenticate API requests
    pub access_token: String,
    /// Long-lived token used to obtain new access tokens
    pub refresh_token: String,
}

/// Request payload for creating a new API token
///
/// Contains the name of the token
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CreateTokenRequest {
    /// The name of the token
    pub name: String,
}

/// After requesting api tokens vec of these will be returned
///
/// Contains the id of the token, the last 4 digits of the token, the name of the token and the creation date of the token
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct TokenResponse {
    /// The id of the token
    pub id: i64,
    /// The last 4 chars of the token
    pub token_last_4: String,
    /// The last used timestamp of the token
    pub last_used_timestamp: Option<DateTime<Utc>>,
    /// The creation timestamp of the token
    pub created_at: DateTime<Utc>,
    /// The name of the token
    pub name: String,
}

/// Request payload for updating the sui address for the user.
///
/// Contains the signature of the user to prove ownership of the sui address.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProofRequest {
    /// The signature of the user to prove ownership of the sui address
    pub signature: String,
}

/// Request payload for acknowledging a usdc payment.
///
/// Contains the transaction digest of the payment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UsdcPaymentRequest {
    /// The transaction digest of the payment
    pub transaction_digest: String,
    /// The proof signature of the payment
    pub proof_signature: Option<String>,
}

/// Represents a computed units processed response
///
/// This struct is used to represent the response for the get_compute_units_processed endpoint.
/// The timestamp of the computed units processed measurement. We measure the computed units processed on hourly basis. We do these measurements for each model.
/// So the timestamp is the hour for which it is measured.
/// The amount is the sum of all computed units processed in that hour. The requests is the total number of requests in that hour.
/// And the time is the time taken to process all computed units in that hour.
/// Tracks hourly measurements
/// of compute unit processing for each model.
/// Each measurement includes:
/// - Total compute units processed
/// - Number of requests handled
/// - Processing time
/// - Timestamp (hourly basis)
///
/// # Example
/// For two requests in an hour:
/// - Request 1: 10 compute units
/// - Request 2: 20 compute units
/// - Total compute units = 30
/// - Total requests = 2
/// - Time = sum of processing time for both requests
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, FromRow)]
pub struct ComputedUnitsProcessedResponse {
    /// Timestamp of the computed units processed measurement
    pub timestamp: DateTime<Utc>,
    /// Name of the model
    pub model_name: String,
    /// Amount of all computed units processed
    pub amount: i64,
    /// Number of requests
    pub requests: i64,
    /// Time (in seconds) taken to process all computed units
    pub time: f64,
}

/// Represents a user profile
/// This struct is used to represent the response for the get_user_profile endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow, ToSchema)]
pub struct UserProfile {
    /// The user's email
    pub email: String,
}

/// Represents a latency response.
///
/// Tracks hourly latency measurements for the system.
/// Each measurement includes:
/// - Total latency for all requests in the hour
/// - Number of requests processed
/// - Timestamp of the measurement
///
/// # Example
/// For two requests in an hour:
/// - Request 1: 1 second latency
/// - Request 2: 2 seconds latency
/// - Total latency = 3 seconds
/// - Total requests = 2
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, FromRow)]
pub struct LatencyResponse {
    /// Timestamp of the latency measurement
    pub timestamp: DateTime<Utc>,
    /// Sum of all latencies (in seconds) in that hour
    pub latency: f64,
    /// Total number of requests in that hour
    pub requests: i64,
}

/// Represents a stats stacks response.
///
/// This struct tracks hourly statistics about compute units in the system.
/// Includes both total and settled compute units measured each hour.
///
/// Measurements include:
/// - Total compute units in the system
/// - Number of settled compute units
/// - Timestamp of the measurement (hourly basis)
///
/// # Example
/// If you have a new stack with 10 compute units and 5 units are settled:
/// - Total compute units = 10
/// - Settled compute units = 5
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct StatsStackResponse {
    /// Timestamp of the measurement (hourly basis)
    pub timestamp: DateTime<Utc>,
    /// Total compute units in the system
    pub num_compute_units: i64,
    /// Number of settled compute units
    pub settled_num_compute_units: i64,
}

/// Represents a task in the system
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct Task {
    /// Unique small integer identifier for the task
    pub task_small_id: i64,
    /// Unique string identifier for the task
    pub task_id: String,
    /// Role associated with the task (encoded as an integer)
    pub role: i16,
    /// Optional name of the model used for the task
    pub model_name: Option<String>,
    /// Indicates whether the task is deprecated
    pub is_deprecated: bool,
    /// Optional epoch timestamp until which the task is valid
    pub valid_until_epoch: Option<i64>,
    /// Optional epoch timestamp when the task was deprecated
    pub deprecated_at_epoch: Option<i64>,
    /// Security level of the task (encoded as an integer)
    pub security_level: i32,
    /// Optional minimum reputation score required for the task
    pub minimum_reputation_score: Option<i16>,
}

/// Represents system metrics collected from a node in the network
///
/// This struct contains detailed hardware metrics including CPU, RAM, network,
/// and GPU usage statistics. For GPU metrics, each vector field contains one entry
/// per GPU device present on the node.
///
/// # Fields
///
/// All memory/storage values are in bytes unless otherwise specified.
/// All percentage values are between 0.0 and 100.0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, FromRow)]
pub struct NodeMetrics {
    /// Unique small integer identifier for the node
    pub node_small_id: i64,

    /// Unix timestamp when metrics were collected
    pub timestamp: i64,

    /// CPU usage as a percentage (0.0-100.0)
    pub cpu_usage: f32,

    /// Number of CPU cores available
    pub num_cpus: i32,

    /// Current RAM usage in bytes
    pub ram_used: i64,

    /// Total RAM available in bytes
    pub ram_total: i64,

    /// Current swap memory usage in bytes
    pub ram_swap_used: i64,

    /// Total swap memory available in bytes
    pub ram_swap_total: i64,

    /// Total bytes received over network
    pub network_rx: i64,

    /// Total bytes transmitted over network
    pub network_tx: i64,

    /// Number of GPU devices present
    pub num_gpus: i32,

    /// Memory used per GPU in bytes
    pub gpu_memory_used: Vec<i64>,

    /// Total memory available per GPU in bytes
    pub gpu_memory_total: Vec<i64>,

    /// Free memory available per GPU in bytes
    pub gpu_memory_free: Vec<i64>,

    /// Percentage of time each GPU spent on memory operations (0.0-100.0)
    pub gpu_percentage_time_read_write: Vec<f64>,

    /// Percentage of time each GPU spent executing compute tasks (0.0-100.0)
    pub gpu_percentage_time_execution: Vec<f64>,

    /// Temperature of each GPU in degrees Celsius
    pub gpu_temperatures: Vec<f64>,

    /// Power consumption of each GPU in watts
    pub gpu_power_usages: Vec<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, FromRow)]
pub struct NodePerformanceScore {
    /// Unique identifier for the performance score
    pub id: i64,

    /// Reference to the weights configuration used for this score
    pub weights_id: i32,

    /// Small integer identifier for the node
    pub node_small_id: i32,

    /// Unix timestamp in seconds when this performance score was recorded
    pub timestamp_secs: i32,

    /// The aggregate performance score of the node
    pub performance_score: f64,
}

impl From<TaskRegisteredEvent> for Task {
    fn from(event: TaskRegisteredEvent) -> Self {
        Self {
            task_id: event.task_id,
            task_small_id: event.task_small_id.inner as i64,
            role: event.role.inner as i16,
            model_name: event.model_name,
            is_deprecated: false,
            valid_until_epoch: None,
            deprecated_at_epoch: None,
            security_level: i32::from(event.security_level.inner),
            minimum_reputation_score: event.minimum_reputation_score.map(i16::from),
        }
    }
}

/// Represents weights used to calculate overall node performance scores
///
/// Each weight is a coefficient between 0 and 1 that determines how much
/// each hardware metric contributes to the final performance score.
///
/// The weights should sum to 1.0 to ensure proper score normalization.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, FromRow)]
pub struct PerformanceWeights {
    /// Weight coefficient for GPU performance metrics (0.0 to 1.0)
    pub gpu_score_weight: f64,

    /// Weight coefficient for CPU performance metrics (0.0 to 1.0)
    pub cpu_score_weight: f64,

    /// Weight coefficient for RAM usage metrics (0.0 to 1.0)
    pub ram_score_weight: f64,

    /// Weight coefficient for swap RAM usage metrics (0.0 to 1.0)
    pub swap_ram_score_weight: f64,

    /// Weight coefficient for network performance metrics (0.0 to 1.0)
    pub network_score_weight: f64,

    /// Weight coefficient for GPU VRAM usage (0.0 to 1.0)
    pub gpu_vram_weight: f64,

    /// Weight coefficient for GPU execution availability (0.0 to 1.0)
    pub gpu_exec_avail_weight: f64,

    /// Weight coefficient for GPU temperature (0.0 to 1.0)
    pub gpu_temp_weight: f64,

    /// Weight coefficient for GPU power usage (0.0 to 1.0)
    pub gpu_power_weight: f64,

    /// Temperature threshold for GPU (0.0 to 1.0)
    pub gpu_temp_threshold: f64,

    /// Maximum temperature for GPU (0.0 to 1.0)
    pub gpu_temp_max: f64,

    /// Power threshold for GPU (0.0 to 1.0)
    pub gpu_power_threshold: f64,

    /// Maximum power usage for GPU (0.0 to 1.0)
    pub gpu_power_max: f64,

    /// Moving average window size for the time series performance score calculation
    pub moving_avg_window_size: i32,

    /// Moving average smooth factor for the time series performance score calculation
    pub moving_avg_smooth_factor: f64,
}

/// Represents the cheapest node settings for a specific model
#[derive(FromRow)]
pub struct CheapestNode {
    /// Unique small integer identifier for the task
    pub task_small_id: i64,

    /// Price per one million compute units for the task that is offered by some node
    pub price_per_one_million_compute_units: i64,

    /// Maximum number of compute units for the task that is offered by the cheapest node
    pub max_num_compute_units: i64,

    /// Unique small integer identifier for the node
    pub node_small_id: i64,
}

/// Response for getting the node distribution.
///
/// This struct represents the response for the get_node_distribution endpoint.
/// Contains the country of the node and the count of nodes in that country.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct NodeDistribution {
    /// The country of the node
    pub country: Option<String>,

    /// The count of nodes in that country
    pub count: i64,
}

/// Represents a stack of compute units for a specific task
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct Stack {
    /// Address of the owner of the stack
    pub owner: String,

    /// Unique small integer identifier for the stack
    pub stack_small_id: i64,

    /// Unique string identifier for the stack
    pub stack_id: String,
    /// Small integer identifier of the associated task
    pub task_small_id: i64,

    /// Identifier of the selected node for computation
    pub selected_node_id: i64,

    /// Total number of compute units in this stack
    pub num_compute_units: i64,

    /// Price per one million compute units for the stack (likely in smallest currency unit)
    pub price_per_one_million_compute_units: i64,

    /// Number of compute units already processed
    pub already_computed_units: i64,

    /// Number of locked compute units for processing
    pub locked_compute_units: i64,

    /// Indicates whether the stack is currently in the settle period
    pub in_settle_period: bool,

    /// Number of payload requests that were received by the node for this stack.
    pub num_total_messages: i64,
}

impl From<StackCreatedEvent> for Stack {
    fn from(event: StackCreatedEvent) -> Self {
        Self {
            owner: event.owner,
            stack_id: event.stack_id,
            stack_small_id: event.stack_small_id.inner as i64,
            task_small_id: event.task_small_id.inner as i64,
            selected_node_id: event.selected_node_id.inner as i64,
            num_compute_units: event.num_compute_units as i64,
            price_per_one_million_compute_units: event.price_per_one_million_compute_units as i64,
            already_computed_units: 0,
            locked_compute_units: 0,
            in_settle_period: false,
            num_total_messages: 0,
        }
    }
}

/// Represents a settlement ticket for a compute stack
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct StackSettlementTicket {
    /// Unique small integer identifier for the stack
    pub stack_small_id: i64,
    /// Identifier of the node selected for computation
    pub selected_node_id: i64,
    /// Number of compute units claimed to be processed
    pub num_claimed_compute_units: i64,
    /// Comma-separated list of node IDs requested for attestation
    pub requested_attestation_nodes: String,
    /// Cryptographic proof of the committed stack state
    pub committed_stack_proofs: Vec<u8>,
    /// Merkle leaf representing the stack in a larger tree structure
    pub stack_merkle_leaves: Vec<u8>,
    /// Optional epoch timestamp when a dispute was settled
    pub dispute_settled_at_epoch: Option<i64>,
    /// Comma-separated list of node IDs that have already attested
    pub already_attested_nodes: String,
    /// Indicates whether the stack is currently in a dispute
    pub is_in_dispute: bool,
    /// Amount to be refunded to the user (likely in smallest currency unit)
    pub user_refund_amount: i64,
    /// Indicates whether the settlement ticket has been claimed
    pub is_claimed: bool,
}

impl TryFrom<StackTrySettleEvent> for StackSettlementTicket {
    type Error = crate::errors::AtomaStateManagerError;

    fn try_from(event: StackTrySettleEvent) -> std::result::Result<Self, Self::Error> {
        let num_attestation_nodes = event.requested_attestation_nodes.len();
        let expanded_size = 32 * num_attestation_nodes;

        let mut expanded_proofs = event.committed_stack_proof;
        expanded_proofs.resize(expanded_size, 0);

        let mut expanded_leaves = event.stack_merkle_leaf;
        expanded_leaves.resize(expanded_size, 0);

        Ok(Self {
            stack_small_id: event.stack_small_id.inner as i64,
            selected_node_id: event.selected_node_id.inner as i64,
            num_claimed_compute_units: event.num_claimed_compute_units as i64,
            requested_attestation_nodes: serde_json::to_string(
                &event
                    .requested_attestation_nodes
                    .into_iter()
                    .map(|id| id.inner)
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
            committed_stack_proofs: expanded_proofs,
            stack_merkle_leaves: expanded_leaves,
            dispute_settled_at_epoch: None,
            already_attested_nodes: serde_json::to_string(&Vec::<i64>::new()).unwrap(),
            is_in_dispute: false,
            user_refund_amount: 0,
            is_claimed: false,
        })
    }
}

/// Represents a dispute in the stack attestation process
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct StackAttestationDispute {
    /// Unique small integer identifier for the stack involved in the dispute
    pub stack_small_id: i64,
    /// Cryptographic commitment provided by the attesting node
    pub attestation_commitment: Vec<u8>,
    /// Identifier of the node that provided the attestation
    pub attestation_node_id: i64,
    /// Identifier of the original node that performed the computation
    pub original_node_id: i64,
    /// Original cryptographic commitment provided by the computing node
    pub original_commitment: Vec<u8>,
}

impl From<StackAttestationDisputeEvent> for StackAttestationDispute {
    fn from(event: StackAttestationDisputeEvent) -> Self {
        Self {
            stack_small_id: event.stack_small_id.inner as i64,
            attestation_commitment: event.attestation_commitment,
            attestation_node_id: event.attestation_node_id.inner as i64,
            original_node_id: event.original_node_id.inner as i64,
            original_commitment: event.original_commitment,
        }
    }
}

/// Represents a node subscription to a task
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct NodeSubscription {
    /// Unique small integer identifier for the node subscription
    pub node_small_id: i64,
    /// Unique small integer identifier for the task
    pub task_small_id: i64,
    /// Price per compute unit for the subscription
    pub price_per_one_million_compute_units: i64,
    /// Maximum number of compute units for the subscription
    pub max_num_compute_units: i64,
    /// Indicates whether the subscription is valid
    pub valid: bool,
}

/// Represents a node's Diffie-Hellman public key so that a client
/// can encrypt a message and the selected node can decrypt it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow)]
pub struct NodePublicKey {
    /// Unique small integer identifier for the node
    pub node_small_id: i64,
    /// Public key of the node
    pub public_key: Vec<u8>,
    /// The stack small id that is associated with the selected node
    #[sqlx(default)]
    pub stack_small_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, FromRow, ToSchema)]
pub struct NodeAttestation {
    pub node_small_id: i64,
    pub compressed_evidence: Vec<u8>,
}

pub enum AtomaAtomaStateManagerEvent {
    /// Locks a stack
    LockStack {
        /// Unique small integer identifier for the stack
        stack_small_id: i64,
    },
    /// Represents an update to the number of tokens in a stack
    UpdateStackNumTokens {
        /// Unique small integer identifier for the stack
        stack_small_id: i64,
        /// Estimated total number of tokens in the stack
        estimated_total_tokens: i64,
        /// Total number of tokens in the stack
        total_tokens: i64,
    },
    /// Gets an available stack with enough compute units for a given stack and public key
    GetAvailableStackWithComputeUnits {
        /// Unique small integer identifier for the stack
        stack_small_id: i64,
        /// Public key of the user
        public_key: String,
        /// Total number of tokens
        total_num_tokens: i64,
        /// Oneshot channel to send the result back to the sender channel
        result_sender: oneshot::Sender<Result<Option<Stack>>>,
    },
    /// Retrieves all stacks associated with a specific model that meet compute unit requirements
    GetStacksForModel {
        /// The name/identifier of the model to query stacks for
        model: String,
        /// The minimum number of available compute units required
        free_compute_units: i64,
        /// The user id of the stacks to filter by
        user_id: i64,
        /// Indicates whether the stacks are associated with confidential compute or not
        is_confidential: bool,
        /// Channel to send back the list of matching stacks
        /// Returns Ok(Vec<Stack>) with matching stacks or an error if the query fails
        result_sender: oneshot::Sender<Result<Option<Stack>>>,
    },
    /// Retrieves all stacks associated with a specific task
    GetStacksForTask {
        /// Unique small integer identifier for the task
        task_small_id: i64,
        /// The minimum number of available compute units required
        free_compute_units: i64,
        /// The user id of the stacks to filter by
        user_id: i64,
        /// Channel to send back the list of matching stacks
        /// Returns Ok(Vec<Stack>) with matching stacks or an error if the query fails
        result_sender: oneshot::Sender<Result<Option<Stack>>>,
    },
    /// Locks compute units for a stack
    LockComputeUnitsForStack {
        /// Unique small integer identifier for the stack
        stack_small_id: i64,
        /// Available compute units for the stack
        available_compute_units: i64,
        /// Channel to send back the result
        /// Returns Ok(()) if the stack is valid or an error if it is not
        result_sender: oneshot::Sender<Result<()>>,
    },
    /// Retrieves all tasks associated with a specific model
    GetTasksForModel {
        /// The name/identifier of the model to query tasks for
        model: String,
        /// Channel to send back the list of matching tasks
        /// Returns Ok(Vec<Task>) with matching tasks or an error if the query fails
        result_sender: oneshot::Sender<Result<Vec<Task>>>,
    },
    /// Retrieves the cheapest node for a specific model
    GetCheapestNodeForModel {
        /// The name/identifier of the model to query the cheapest node for
        model: String,
        /// Indicates whether the stacks are associated with confidential compute or not
        is_confidential: bool,
        /// Channel to send back the cheapest node
        /// Returns Ok(Option<CheapestNode>) with the cheapest node or an error if the query fails
        result_sender: oneshot::Sender<Result<Option<CheapestNode>>>,
    },
    GetNodePublicUrlAndSmallId {
        /// Unique small integer identifier for the stack
        stack_small_id: i64,
        /// Channel to send back the public url and small id
        /// Returns Ok(Option<(String, i64)>) with the public url and small id or an error if the query fails
        result_sender: oneshot::Sender<Result<(Option<String>, i64)>>,
    },
    /// Selects a node's public key for encryption
    SelectNodePublicKeyForEncryption {
        /// The name/identifier of the model to query the cheapest node for
        model: String,
        /// The maxinum number of tokens to be processed
        max_num_tokens: i64,
        /// The user id of the stack owner (referencing local user table)
        user_id: i64,
        /// Channel to send back the public key
        /// Returns Ok(Option<NodePublicKey>) with the public key or an error if the query fails
        result_sender: oneshot::Sender<Option<NodePublicKey>>,
    },
    SelectNodePublicKeyForEncryptionForNode {
        /// Unique small integer identifier for the node
        node_small_id: i64,
        /// Channel to send back the public key
        /// Returns Ok(Option<NodePublicKey>) with the public key or an error if the query fails
        result_sender: oneshot::Sender<Option<NodePublicKey>>,
    },
    /// Upserts a node's public address
    UpsertNodePublicAddress {
        /// Unique small integer identifier for the node
        node_small_id: i64,
        /// Public address of the node
        public_address: String,
        country: String,
    },
    /// Retrieves a node's public address
    GetNodePublicAddress {
        /// Unique small integer identifier for the node
        node_small_id: i64,

        /// Channel to send back the public address
        /// Returns Ok(Option<String>) with the public address or an error if the query fails
        result_sender: oneshot::Sender<Result<Option<String>>>,
    },
    /// Retrieves a node's Sui address
    GetNodeSuiAddress {
        /// Unique small integer identifier for the node
        node_small_id: i64,
        /// Channel to send back the Sui address
        /// Returns Ok(Option<String>) with the Sui address or an error if the query fails
        result_sender: oneshot::Sender<Result<Option<String>>>,
    },
    /// Records statistics about a new stack in the database
    NewStackAcquired {
        /// The event that triggered the stack creation
        event: StackCreatedEvent,
        /// Number of locked compute units for processing
        locked_compute_units: i64,
        /// Timestamp of the transaction that created the stack
        transaction_timestamp: DateTime<Utc>,
        /// User id of the stack owner (referencing local user table)
        user_id: i64,
        /// Channel to send back the result
        /// Returns Ok(()) if the stack is valid or an error if it is not
        result_sender: oneshot::Sender<Result<()>>,
    },
    /// Records statistics about a node's throughput performance
    UpdateNodeThroughputPerformance {
        /// Timestamp of the transaction that created the stack
        timestamp: DateTime<Utc>,
        /// The name/identifier of the model
        model_name: String,
        /// Number of input tokens
        input_tokens: i64,
        /// Number of output tokens
        output_tokens: i64,
        /// Time taken to process the tokens
        time: f64,
    },
    /// Registers a new user with a password
    RegisterUserWithPassword {
        /// The email of the user
        user_profile: UserProfile,
        /// The password of the user
        password: String,
        /// Password salt
        password_salt: String,
        /// Channel to send back the user ID
        /// Returns Ok(Option<i64>) with the user ID or an error if the query fails
        result_sender: oneshot::Sender<Result<Option<i64>>>,
    },
    GetPasswordSalt {
        /// The email of the user
        email: String,
        /// Channel to send back the password salt
        /// Returns Ok(Option<String>) with the password salt or an error if the query fails
        result_sender: oneshot::Sender<Result<Option<String>>>,
    },
    /// Retrieves the user ID by email and password
    GetUserIdByEmailPassword {
        /// The email of the user
        email: String,
        /// The password of the user
        password: String,
        /// Channel to send back the user ID
        /// Returns Ok(Option<i64>) with the user ID or an error if the query fails
        result_sender: oneshot::Sender<Result<Option<i64>>>,
    },
    /// Retrieves the user ID by oauth email
    OAuth {
        /// The email of the user
        email: String,
        /// Password salt
        password_salt: String,
        /// The result sender to send back the user ID
        result_sender: oneshot::Sender<Result<i64>>,
    },
    /// Checks if a refresh token is valid for a user
    IsRefreshTokenValid {
        /// The user ID
        user_id: i64,
        /// The hash of the refresh token
        refresh_token_hash: String,
        /// Channel to send back the result
        /// Returns Ok(bool) with true if the refresh token is valid or false if it is not
        result_sender: oneshot::Sender<Result<bool>>,
    },
    /// Stores a refresh token for a user
    StoreRefreshToken {
        /// The user ID
        user_id: i64,
        /// The hash of the refresh token
        refresh_token_hash: String,
    },
    /// Revokes a refresh token for a user
    RevokeRefreshToken {
        /// The user ID
        user_id: i64,
        /// The hash of the refresh token
        refresh_token_hash: String,
    },
    /// Checks if an API token is valid for a user
    IsApiTokenValid {
        /// The API token
        api_token: String,
        /// Channel to send back the result
        /// Returns Ok(bool) with true if the API token is valid or false if it is not
        result_sender: oneshot::Sender<Result<i64>>,
    },
    /// Revokes an API token for a user
    RevokeApiToken {
        /// The user ID
        user_id: i64,
        /// The API token id
        api_token_id: i64,
    },
    /// Stores a new API token for a user
    StoreNewApiToken {
        /// The user ID
        user_id: i64,
        /// The API token
        api_token: String,
        /// Name of the token
        name: String,
    },
    /// Retrieves all API tokens for a user
    GetApiTokensForUser {
        /// The user ID
        user_id: i64,
        /// Channel to send back the list of API tokens
        /// Returns Ok(Vec<String>) with the list of API tokens or an error if the query fails
        result_sender: oneshot::Sender<Result<Vec<TokenResponse>>>,
    },
    /// Stores the sui_address with proven ownership
    UpdateSuiAddress {
        /// The user ID
        user_id: i64,
        /// Proven Sui address
        sui_address: String,
    },
    /// Retrieves the sui_address for a user
    GetSuiAddress {
        /// The user ID
        user_id: i64,
        /// The result sender to send back the Sui address
        result_sender: oneshot::Sender<Result<Option<String>>>,
    },
    /// Retrieves the user ID by Sui address
    ConfirmUser {
        /// The Sui address
        sui_address: String,
        /// The user ID
        user_id: i64,
        /// The result sender to send back the result
        result_sender: oneshot::Sender<Result<bool>>,
    },
    /// Updates the balance of a user
    TopUpBalance {
        /// The user ID
        user_id: i64,
        /// The amount to top up
        amount: i64,
    },
    /// Withdraws the balance of a user
    DeductFromUsdc {
        /// The user ID
        user_id: i64,
        /// The amount to deduct
        amount: i64,
        /// The result sender to send back the result
        result_sender: oneshot::Sender<Result<()>>,
    },
    /// Refunds a USDC payment.
    RefundUsdc {
        /// The user ID
        user_id: i64,
        /// The amount to refund
        amount: i64,
        /// The result sender to send back the result
        result_sender: oneshot::Sender<Result<()>>,
    },
    /// Acknowledges a USDC payment. Fails if the digest has already been acknowledged.
    InsertNewUsdcPaymentDigest {
        /// The digest of the USDC payment
        digest: String,
        /// The result sender to send back the result
        result_sender: oneshot::Sender<Result<()>>,
    },
    /// Retrieves the zk_salt of a user
    GetZkSalt {
        /// The user ID
        user_id: i64,
        /// The result sender to send back the salt
        result_sender: oneshot::Sender<Result<Option<String>>>,
    },
    /// Sets the salt of a user
    SetSalt {
        /// The user ID
        user_id: i64,
        /// The salt
        salt: String,
        /// The result sender to send back the result
        result_sender: oneshot::Sender<Result<()>>,
    },
    LockUserFiatBalance {
        /// The user ID
        user_id: i64,
        /// The amount to lock for input
        input_amount: i64,
        /// The amount to lock for output
        output_amount: i64,
        /// The result sender to send back the success status
        result_sender: oneshot::Sender<Result<bool>>,
    },
    UpdateStackNumTokensFiat {
        /// The user ID
        user_id: i64,
        /// Model name
        model_name: String,
        /// The original estimated input amount
        estimated_input_amount: i64,
        /// The actual input amount
        input_amount: i64,
        /// Number of input tokens,
        input_tokens: i64,
        /// The original estimated output amount
        estimated_output_amount: i64,
        /// The actual output amount
        output_amount: i64,
        /// Number of output tokens
        output_tokens: i64,
    },
    SetCustomPricing {
        /// The user ID
        user_id: i64,
        /// The model name
        model: String,
        /// Price per one million input compute units
        price_per_one_million_input_compute_units: i64,
        /// Price per one million output compute units
        price_per_one_million_output_compute_units: i64,
    },
    GetCustomPricing {
        /// The user ID
        user_id: i64,
        /// The model name
        model: String,
        /// Channel to send back the custom pricing
        result_sender: oneshot::Sender<Result<Option<Pricing>>>,
    },
    RetrieveNodesPublicAddresses {
        /// Channel to send back the list of public addresses
        /// Returns Ok(Vec<String>) with the list of `public_address` or an error if the query fails
        result_sender: oneshot::Sender<Result<Vec<String>>>,
    },
}
