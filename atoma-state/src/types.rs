use atoma_sui::events::{
    StackAttestationDisputeEvent, StackCreatedEvent, StackTrySettleEvent, TaskRegisteredEvent,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tokio::sync::oneshot;
use utoipa::ToSchema;

use crate::state_manager::Result;

/// Request payload for revoking an API token
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, FromRow, ToSchema)]
pub struct RevokeApiTokenRequest {
    /// The API token to be revoked
    pub api_token: String,
}

/// Request payload for user authentication
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, FromRow, ToSchema)]
pub struct AuthRequest {
    /// The user's unique identifier
    pub username: String,
    /// The user's password
    pub password: String,
}

/// Response returned after successful authentication
///
/// Contains both an access token and a refresh token for implementing token-based authentication:
/// - The access token is used to authenticate API requests
/// - The refresh token is used to obtain new access tokens when they expire
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, FromRow)]
pub struct AuthResponse {
    /// JWT token used to authenticate API requests
    pub access_token: String,
    /// Long-lived token used to obtain new access tokens
    pub refresh_token: String,
}

/// Request payload for updating the sui address for the user.
///
/// Contains the signature of the user to prove ownership of the sui address.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ProofRequest {
    pub signature: String,
}

/// Represents a computed units processed response
/// This struct is used to represent the response for the get_compute_units_processed endpoint.
/// The timestamp of the computed units processed measurement. We measure the computed units processed on hourly basis. We do these measurements for each model.
/// So the timestamp is the hour for which it is measured.
/// The amount is the sum of all computed units processed in that hour. The requests is the total number of requests in that hour.
/// And the time is the time taken to process all computed units in that hour.
///
/// E.g. you have two requests in the hour, one with 10 computed units and the other with 20 computed units.
/// The amount will be 30, the requests will be 2 and the time will be the sum of the time taken to process the 10 and 20 computed units.
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

/// Represents a latency response
/// This struct is used to represent the response for the get_latency endpoint.
/// The timestamp of the latency measurement. We measure the latency on hourly basis. So the timestamp is the hour for which it is measured.
/// The latency is the sum of all latencies in that hour. And the number of requests is the total number of requests in that hour.
///
/// E.g. you have two requests in the hour, one with 1 second latency and the other with 2 seconds latency.
/// The latency will be 3 seconds and the requests will be 2.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, FromRow)]
pub struct LatencyResponse {
    /// Timestamp of the latency measurement
    pub timestamp: DateTime<Utc>,
    /// Sum of all latencies (in seconds) in that hour
    pub latency: f64,
    /// Total number of requests in that hour
    pub requests: i64,
}

/// Represents a stats stacks response
/// This struct is used to represent the response for the get_stats_stacks endpoint.
/// The timestamp of the stats stacks measurement. We measure the stats stacks on hourly basis. So the timestamp is the hour for which it is measured.
/// The number of compute units is the total number of compute units in the system. The settled number of compute units is the total number of settled compute units in the system.
///
/// E.g. you have new stack for 10 compute units and stack settled with 5 settled compute units during the hour.
/// The number of compute units will be 10 and the settled number of compute units will be 5.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, FromRow)]
pub struct StatsStackResponse {
    pub timestamp: DateTime<Utc>,
    pub num_compute_units: i64,
    pub settled_num_compute_units: i64,
}

/// Represents a task in the system
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, FromRow)]
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

impl From<TaskRegisteredEvent> for Task {
    fn from(event: TaskRegisteredEvent) -> Self {
        Task {
            task_id: event.task_id,
            task_small_id: event.task_small_id.inner as i64,
            role: event.role.inner as i16,
            model_name: event.model_name,
            is_deprecated: false,
            valid_until_epoch: None,
            deprecated_at_epoch: None,
            security_level: event.security_level.inner as i32,
            minimum_reputation_score: event.minimum_reputation_score.map(|score| score as i16),
        }
    }
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

/// Response for getting the node distribution
/// This struct is used to represent the response for the get_node_distribution endpoint.
/// The country is the country of the node and the count is the number of nodes in that country.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, FromRow)]
pub struct NodeDistribution {
    pub country: Option<String>,
    pub count: i64,
}

/// Represents a stack of compute units for a specific task
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, FromRow)]
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
    /// Indicates whether the stack is currently in the settle period
    pub in_settle_period: bool,
    /// Joint concatenation of Blake2b hashes of each payload and response pairs that was already processed
    /// by the node for this stack.
    pub total_hash: Vec<u8>,
    /// Number of payload requests that were received by the node for this stack.
    pub num_total_messages: i64,
}

impl From<StackCreatedEvent> for Stack {
    fn from(event: StackCreatedEvent) -> Self {
        Stack {
            owner: event.owner,
            stack_id: event.stack_id,
            stack_small_id: event.stack_small_id.inner as i64,
            task_small_id: event.task_small_id.inner as i64,
            selected_node_id: event.selected_node_id.inner as i64,
            num_compute_units: event.num_compute_units as i64,
            price_per_one_million_compute_units: event.price_per_one_million_compute_units as i64,
            already_computed_units: 0,
            in_settle_period: false,
            total_hash: vec![],
            num_total_messages: 0,
        }
    }
}

/// Represents a settlement ticket for a compute stack
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, FromRow)]
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

impl From<StackTrySettleEvent> for StackSettlementTicket {
    fn from(event: StackTrySettleEvent) -> Self {
        let num_attestation_nodes = event.requested_attestation_nodes.len();
        let expanded_size = 32 * num_attestation_nodes;

        let mut expanded_proofs = event.committed_stack_proof;
        expanded_proofs.resize(expanded_size, 0);

        let mut expanded_leaves = event.stack_merkle_leaf;
        expanded_leaves.resize(expanded_size, 0);

        StackSettlementTicket {
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
        }
    }
}

/// Represents a dispute in the stack attestation process
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, FromRow)]
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
        StackAttestationDispute {
            stack_small_id: event.stack_small_id.inner as i64,
            attestation_commitment: event.attestation_commitment,
            attestation_node_id: event.attestation_node_id.inner as i64,
            original_node_id: event.original_node_id.inner as i64,
            original_commitment: event.original_commitment,
        }
    }
}

/// Represents a node subscription to a task
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, FromRow)]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, FromRow)]
pub struct NodePublicKey {
    /// Unique small integer identifier for the node
    pub node_small_id: i64,
    /// Public key of the node
    pub public_key: Vec<u8>,
}

pub enum AtomaAtomaStateManagerEvent {
    /// Represents an update to the number of tokens in a stack
    UpdateStackNumTokens {
        /// Unique small integer identifier for the stack
        stack_small_id: i64,
        /// Estimated total number of tokens in the stack
        estimated_total_tokens: i64,
        /// Total number of tokens in the stack
        total_tokens: i64,
    },
    /// Represents an update to the total hash of a stack
    UpdateStackTotalHash {
        /// Unique small integer identifier for the stack
        stack_small_id: i64,
        /// Total hash of the stack
        total_hash: [u8; 32],
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
    /// Selects a node's public key for encryption
    SelectNodePublicKeyForEncryption {
        /// The name/identifier of the model to query the cheapest node for
        model: String,
        /// The maxinum number of tokens to be processed
        max_num_tokens: i64,
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
        /// Number of compute units already processed
        already_computed_units: i64,
        /// Timestamp of the transaction that created the stack
        transaction_timestamp: DateTime<Utc>,
        /// User id of the stack owner (referencing local user table)
        user_id: i64,
    },
    /// Records statistics about a node's throughput performance
    UpdateNodeThroughputPerformance {
        /// Timestamp of the transaction that created the stack
        timestamp: DateTime<Utc>,
        /// The name/identifier of the model
        model_name: String,
        /// Unique small integer identifier for the node
        node_small_id: i64,
        /// Number of input tokens
        input_tokens: i64,
        /// Number of output tokens
        output_tokens: i64,
        /// Time taken to process the tokens
        time: f64,
    },
    /// Records statistics about a node's prefill performance
    UpdateNodePrefillPerformance {
        /// Unique small integer identifier for the node
        node_small_id: i64,
        /// Number of tokens
        tokens: i64,
        /// Time taken to process the tokens
        time: f64,
    },
    /// Records statistics about a node's decode performance
    UpdateNodeDecodePerformance {
        /// Unique small integer identifier for the node
        node_small_id: i64,
        /// Number of tokens
        tokens: i64,
        /// Time taken to process the tokens
        time: f64,
    },
    /// Records statistics about a node's latency performance
    UpdateNodeLatencyPerformance {
        /// Timestamp of the transaction that created the stack
        timestamp: DateTime<Utc>,
        /// Unique small integer identifier for the node
        node_small_id: i64,
        /// Latency in seconds
        latency: f64,
    },
    /// Retrieves the X25519 public key for a selected node
    GetSelectedNodeX25519PublicKey {
        /// Unique small integer identifier for the node
        selected_node_id: i64,
        /// Channel to send back the X25519 public key
        /// Returns Ok(Option<Vec<u8>>) with the public key or an error if the query fails
        result_sender: oneshot::Sender<Result<Option<Vec<u8>>>>,
    },
    /// Registers a new user with a password
    RegisterUserWithPassword {
        /// The username of the user
        username: String,
        /// The password of the user
        password: String,
        /// Channel to send back the user ID
        /// Returns Ok(Option<i64>) with the user ID or an error if the query fails
        result_sender: oneshot::Sender<Result<Option<i64>>>,
    },
    /// Retrieves the user ID by username and password
    GetUserIdByUsernamePassword {
        /// The username of the user
        username: String,
        /// The password of the user
        password: String,
        /// Channel to send back the user ID
        /// Returns Ok(Option<i64>) with the user ID or an error if the query fails
        result_sender: oneshot::Sender<Result<Option<i64>>>,
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
        /// The API token
        api_token: String,
    },
    /// Stores a new API token for a user
    StoreNewApiToken {
        /// The user ID
        user_id: i64,
        /// The API token
        api_token: String,
    },
    /// Retrieves all API tokens for a user
    GetApiTokensForUser {
        /// The user ID
        user_id: i64,
        /// Channel to send back the list of API tokens
        /// Returns Ok(Vec<String>) with the list of API tokens or an error if the query fails
        result_sender: oneshot::Sender<Result<Vec<String>>>,
    },
    /// Stores the sui_address with proven ownership
    UpdateSuiAddress {
        /// The user ID
        user_id: i64,
        /// Proven Sui address
        sui_address: String,
    },
}
