use std::str::FromStr;

use atoma_state::types::AtomaAtomaStateManagerEvent;
use atoma_utils::verify_signature;
use axum::http::HeaderMap;
use axum::{extract::State, Json};
use base64::engine::{general_purpose::STANDARD, Engine};
use blake2::digest::consts::U32;
use blake2::digest::generic_array::GenericArray;
use blake2::{Blake2b, Digest};
use serde::{Deserialize, Serialize};
use sui_sdk::types::base_types::SuiAddress;
use sui_sdk::types::crypto::{PublicKey as SuiPublicKey, Signature, SuiSignature};
use tokio::sync::oneshot;
use tracing::instrument;
use utoipa::{OpenApi, ToSchema};

use crate::server::check_auth;
use crate::server::error::AtomaProxyError;
use crate::server::http_server::ProxyState;
use crate::server::middleware::acquire_stack_lock;
use crate::server::middleware::auth::{
    acquire_new_stack, get_stack_if_locked, SelectedNodeMetadata,
};

pub const NODES_PATH: &str = "/v1/nodes";
pub const NODES_CREATE_PATH: &str = "/v1/nodes";
pub const NODES_CREATE_LOCK_PATH: &str = "/v1/nodes/lock";

/// Size of the blake2b hash in bytes
const BODY_HASH_SIZE: usize = 32;

/// The maximum number of tokens to be processed for confidential compute.
/// Since requests are encrypted, the proxy is not able to determine the number of tokens
/// in the request. We set a default value here to be used for node selection, as a upper
/// bound for the number of tokens for each request.
/// TODO: In the future, this number can be dynamically adjusted based on the model.
pub const MAX_NUM_TOKENS_FOR_CONFIDENTIAL_COMPUTE: i64 = 8_192;

#[derive(OpenApi)]
#[openapi(paths(nodes_create, nodes_create_lock))]
/// OpenAPI documentation for the node public address registration endpoint.
///
/// This struct is used to generate OpenAPI documentation for the node public address
/// registration endpoint. It uses the `utoipa` crate's derive macro to automatically
/// generate the OpenAPI specification from the code.
pub struct NodesOpenApi;

/// Represents the payload for the node public address registration request.
///
/// This struct represents the payload for the node public address registration request.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct NodePublicAddressAssignment {
    /// Unique small integer identifier for the node
    node_small_id: u64,
    /// The public address of the node
    public_address: String,
    /// The country of the node
    country: String,
}

/// Represents the payload for the node public address registration request.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct NodesCreateRequest {
    /// The data required to register a node's public address
    data: NodePublicAddressAssignment,
    /// The signature of the data base 64 encoded
    signature: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct NodesCreateResponse {
    /// The message of the response
    message: String,
}

/// Create node
///
/// This endpoint allows nodes to register or update their public address in the system.
/// When a node comes online or changes its address, it can use this endpoint to ensure
/// the system has its current address for routing requests.
///
/// ## Errors
///
/// Returns various `AtomaProxyError` variants:
/// * `MissingHeader` - If the signature header is missing
/// * `InvalidHeader` - If the signature header is malformed
/// * `InvalidBody` - If:
///   - The request body cannot be read
///   - The signature is invalid
///   - The body cannot be parsed
///   - The sui address doesn't match the signature
/// * `InternalError` - If:
///   - The state manager channel is closed
///   - The registration event cannot be sent
///   - Node Sui address lookup fails
#[utoipa::path(
    post,
    path = "",
    responses(
        (status = OK, description = "Node public address registered successfully", body = NodesCreateResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to register node public address")
    )
)]
#[instrument(level = "info", skip_all)]
pub async fn nodes_create(
    State(state): State<ProxyState>,
    Json(payload): Json<NodesCreateRequest>,
) -> Result<Json<NodesCreateResponse>, AtomaProxyError> {
    let base64_signature = &payload.signature;
    let body_bytes =
        serde_json::to_vec(&payload.data).map_err(|e| AtomaProxyError::RequestError {
            message: format!("Failed to serialize payload to bytes, with error: {e}"),
            endpoint: NODES_CREATE_PATH.to_string(),
        })?;

    let signature =
        Signature::from_str(base64_signature).map_err(|e| AtomaProxyError::RequestError {
            message: format!("Failed to parse signature, with error: {e}"),
            endpoint: NODES_CREATE_PATH.to_string(),
        })?;

    let public_key_bytes = signature.public_key_bytes();
    let public_key =
        SuiPublicKey::try_from_bytes(signature.scheme(), public_key_bytes).map_err(|e| {
            AtomaProxyError::RequestError {
                message: format!("Failed to extract public key from bytes, with error: {e}"),
                endpoint: NODES_CREATE_PATH.to_string(),
            }
        })?;
    let sui_address = SuiAddress::from(&public_key);

    let mut blake2b_hash = Blake2b::new();
    blake2b_hash.update(&body_bytes);
    let body_blake2b_hash: GenericArray<u8, U32> = blake2b_hash.finalize();
    let body_blake2b_hash_bytes: [u8; BODY_HASH_SIZE] = body_blake2b_hash
        .as_slice()
        .try_into()
        .map_err(|e| AtomaProxyError::RequestError {
            message: format!("Failed to convert blake2b hash to bytes, with error: {e}"),
            endpoint: NODES_CREATE_PATH.to_string(),
        })?;
    verify_signature(base64_signature, &body_blake2b_hash_bytes).map_err(|e| {
        AtomaProxyError::RequestError {
            message: format!("Failed to verify signature, with error: {e}"),
            endpoint: NODES_CREATE_PATH.to_string(),
        }
    })?;

    let (result_sender, result_receiver) = oneshot::channel();

    state
        .state_manager_sender
        .send(AtomaAtomaStateManagerEvent::GetNodeSuiAddress {
            node_small_id: payload.data.node_small_id as i64,
            result_sender,
        })
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to send GetNodeSuiAddress event: {err:?}"),
            client_message: None,
            endpoint: NODES_CREATE_PATH.to_string(),
        })?;

    let node_sui_address = result_receiver
        .await
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to receive GetNodeSuiAddress result: {err:?}"),
            client_message: None,
            endpoint: NODES_CREATE_PATH.to_string(),
        })?
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to get node Sui address: {err:?}"),
            client_message: None,
            endpoint: NODES_CREATE_PATH.to_string(),
        })?
        .ok_or_else(|| AtomaProxyError::NotFound {
            message: "Node Sui address not found".to_string(),
            endpoint: NODES_CREATE_PATH.to_string(),
        })?;

    // Check if the address associated with the small ID in the request matches the Sui address in the signature.
    if node_sui_address != sui_address.to_string() {
        return Err(AtomaProxyError::RequestError {
            message: "The sui address associated with the node small ID does not match the signature sui address".to_string(),
            endpoint: NODES_CREATE_PATH.to_string(),
        });
    }

    state
        .state_manager_sender
        .send(AtomaAtomaStateManagerEvent::UpsertNodePublicAddress {
            node_small_id: payload.data.node_small_id as i64,
            public_address: payload.data.public_address.clone(),
            country: payload.data.country.clone(),
        })
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to send UpsertNodePublicAddress event: {err:?}"),
            client_message: None,
            endpoint: NODES_CREATE_PATH.to_string(),
        })?;

    Ok(Json(NodesCreateResponse {
        message: "Success".to_string(),
    }))
}

/// The response body for selecting a node's public key for encryption
/// from a client. The client will use the provided public key to encrypt
/// the request and send it back to the proxy. The proxy will then route this
/// request to the selected node.
#[derive(Deserialize, Serialize, ToSchema)]
pub struct NodesCreateLockResponse {
    /// The public key for the selected node, base64 encoded
    public_key: String,

    /// The node small id for the selected node
    node_small_id: u64,

    /// Transaction digest for the transaction that acquires the stack entry, if any
    stack_entry_digest: Option<String>,

    /// The stack small id to which an available stack entry was acquired, for the selected node
    stack_small_id: u64,
}

/// Request body for creating a node lock
#[derive(Deserialize, Serialize, ToSchema)]
pub struct NodesCreateLockRequest {
    /// The model to lock a node for
    pub model: String,
}

/// Create a node lock for confidential compute
///
/// This endpoint attempts to find a suitable node and retrieve its public key for encryption
/// through a two-step process:
///
/// 1. First, it tries to select an existing node with a public key directly.
/// 2. If no node is immediately available, it falls back to finding the cheapest compatible node
///    and acquiring a new stack entry for it.
///
/// This endpoint is specifically designed for confidential compute scenarios where
/// requests need to be encrypted before being processed by nodes.
///
/// ## Errors
///   - `INTERNAL_SERVER_ERROR` - Communication errors or missing node public keys
///   - `SERVICE_UNAVAILABLE` - No nodes available for confidential compute
#[utoipa::path(
    post,
    path = "/lock",
    security(
        ("bearerAuth"= [])
    ),
    request_body(content = NodesCreateLockRequest, description = "The model to lock a node for", content_type = "application/json"),
    responses(
        (status = OK, description = "Node DH public key requested successfully", body = NodesCreateLockResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to request node DH public key"),
        (status = SERVICE_UNAVAILABLE, description = "No node found for model with confidential compute enabled for requested model")
    )
)]
#[instrument(
    level = "info",
    skip_all,
    fields(endpoint = NODES_CREATE_LOCK_PATH)
)]
pub async fn nodes_create_lock(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    Json(payload): Json<NodesCreateLockRequest>,
) -> Result<Json<NodesCreateLockResponse>, AtomaProxyError> {
    let (sender, receiver) = oneshot::channel();
    let user_id = check_auth(
        &state.state_manager_sender,
        &headers,
        NODES_CREATE_LOCK_PATH,
    )
    .await?;
    state
        .state_manager_sender
        .send(
            AtomaAtomaStateManagerEvent::SelectNodePublicKeyForEncryption {
                model: payload.model.clone(),
                max_num_tokens: MAX_NUM_TOKENS_FOR_CONFIDENTIAL_COMPUTE,
                user_id,
                result_sender: sender,
            },
        )
        .map_err(|_| AtomaProxyError::InternalError {
            message: "Failed to send SelectNodePublicKeyForEncryption event".to_string(),
            client_message: None,
            endpoint: NODES_CREATE_LOCK_PATH.to_string(),
        })?;
    let node_public_key = receiver.await.map_err(|e| AtomaProxyError::InternalError {
        message: format!("Failed to receive node public key: {e}"),
        client_message: None,
        endpoint: NODES_CREATE_LOCK_PATH.to_string(),
    })?;

    if let Some(node_public_key) = node_public_key {
        let stack_small_id =
            node_public_key
                .stack_small_id
                .ok_or_else(|| AtomaProxyError::InternalError {
                    message: "Stack small id not found for node public key".to_string(),
                    client_message: None,
                    endpoint: NODES_CREATE_LOCK_PATH.to_string(),
                })?;
        let public_key = STANDARD.encode(node_public_key.public_key);
        Ok(Json(NodesCreateLockResponse {
            public_key,
            node_small_id: node_public_key.node_small_id as u64,
            stack_entry_digest: None,
            stack_small_id: stack_small_id as u64,
        }))
    } else {
        // NOTE: We need to check the user's balance before acquiring a new stack entry.
        // If this is not the case, we actually do not need authentication from the user.
        let (sender, receiver) = oneshot::channel();
        state
            .state_manager_sender
            .send(AtomaAtomaStateManagerEvent::GetCheapestNodeForModel {
                model: payload.model.clone(),
                is_confidential: true, // NOTE: This endpoint is only required for confidential compute
                result_sender: sender,
            })
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to send GetCheapestNodeForModel event: {e:?}"),
                client_message: None,
                endpoint: NODES_CREATE_LOCK_PATH.to_string(),
            })?;
        let node = receiver
            .await
            .map_err(|_| AtomaProxyError::InternalError {
                message: "Failed to receive GetCheapestNodeForModel result".to_string(),
                client_message: None,
                endpoint: NODES_CREATE_LOCK_PATH.to_string(),
            })?
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to get GetCheapestNodeForModel result: {e:?}"),
                client_message: None,
                endpoint: NODES_CREATE_LOCK_PATH.to_string(),
            })?;
        if let Some(node) = node {
            let task_small_id = node.task_small_id;
            let SelectedNodeMetadata {
                stack_small_id,
                selected_node_id,
                tx_digest,
            } = if let Some(lock_guard) = acquire_stack_lock::LockGuard::try_lock(
                &state.users_buy_stack_lock_map,
                (user_id, task_small_id),
            ) {
                let sui = state.sui.clone();
                // NOTE: At this point, we have an acquired stack lock, so we can safely acquire a new stack.
                acquire_new_stack(
                    state.state_manager_sender.clone(),
                    user_id,
                    lock_guard,
                    NODES_CREATE_LOCK_PATH.to_string(),
                    MAX_NUM_TOKENS_FOR_CONFIDENTIAL_COMPUTE as u64,
                    sui,
                    node,
                )
                .await?
                // NOTE: The `acquire_new_stack` method will emit a stack creation event, and it will stored it
                // in the AtomaStateManager's internal state, therefore any new request querying the state manager after this
                // lock guard release will see the new stack.
                // NOTE: When the `lock_guard` goes out of scope, it ensures that the `DashMap` entry is removed,
                // even if the `acquire_new_stack` returned an error, previously, as this is handled at drop time.
            } else {
                // NOTE: Failed to acquire stack lock (meaning, we are in a race condition scenario)
                // so we try to get the stack from the state manager, and if it is not found, we return an error.
                get_stack_if_locked(
                    &state,
                    user_id,
                    task_small_id,
                    NODES_CREATE_LOCK_PATH,
                    MAX_NUM_TOKENS_FOR_CONFIDENTIAL_COMPUTE as u64,
                )
                .await?
            };

            // NOTE: The contract might select a different node than the one we used to extract
            // the price per one million compute units. In this case, we need to update the value of the `node_small_id``
            // to be the one selected by the contract, that we can query from the `StackCreatedEvent`.
            let node_small_id = selected_node_id;
            // NOTE: We need to get the public key for the selected node for the acquired stack.
            let (sender, receiver) = oneshot::channel();
            state
                .state_manager_sender
                .send(
                    AtomaAtomaStateManagerEvent::SelectNodePublicKeyForEncryptionForNode {
                        node_small_id,
                        result_sender: sender,
                    },
                )
                .map_err(|e| AtomaProxyError::InternalError {
                    message: format!("Failed to send GetNodePublicKeyForEncryption event: {e:?}"),
                    client_message: None,
                    endpoint: NODES_CREATE_LOCK_PATH.to_string(),
                })?;
            let node_public_key = receiver.await.map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to receive GetNodePublicKeyForEncryption result: {e:?}"),
                client_message: None,
                endpoint: NODES_CREATE_LOCK_PATH.to_string(),
            })?;
            if let Some(node_public_key) = node_public_key {
                let public_key = STANDARD.encode(node_public_key.public_key);
                Ok(Json(NodesCreateLockResponse {
                    public_key,
                    node_small_id: node_public_key.node_small_id as u64,
                    stack_entry_digest: tx_digest.map(|tx| tx.to_string()),
                    stack_small_id: stack_small_id as u64,
                }))
            } else {
                Err(AtomaProxyError::InternalError {
                    message: format!("No node public key found for node {node_small_id}"),
                    client_message: Some("Node public key not found".to_string()),
                    endpoint: NODES_CREATE_LOCK_PATH.to_string(),
                })
            }
        } else {
            Err(AtomaProxyError::ServiceUnavailable {
                message: format!(
                    "No node found for model {} with confidential compute enabled",
                    payload.model
                ),
                endpoint: NODES_CREATE_LOCK_PATH.to_string(),
            })
        }
    }
}
