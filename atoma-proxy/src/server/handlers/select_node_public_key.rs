use atoma_state::types::AtomaAtomaStateManagerEvent;
use axum::http::StatusCode;
use axum::Extension;
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tracing::{error, instrument};
use utoipa::{OpenApi, ToSchema};

use crate::server::{http_server::ProxyState, middleware::RequestMetadataExtension};

/// The maximum number of tokens to be processed for confidential compute.
/// Since requests are encrypted, the proxy is not able to determine the number of tokens
/// in the request. We set a default value here to be used for node selection, as a upper
/// bound for the number of tokens for each request.
/// TODO: In the future, this number can be dynamically adjusted based on the model.
const MAX_NUM_TOKENS_FOR_CONFIDENTIAL_COMPUTE: i64 = 128_000;

/// The endpoint for selecting a node's public key for encryption
pub const ENCRYPTION_PUBLIC_KEY_ENDPOINT: &str = "/v1/encryption/public-key";

/// OpenAPI documentation structure for the node public key selection endpoint.
/// This struct is used to generate OpenAPI/Swagger documentation for the
/// `/v1/encryption/public-key` endpoint, which handles the selection of a node's
/// public key for encryption in confidential compute scenarios.
#[derive(OpenApi)]
#[openapi(paths(select_node_public_key))]
pub(crate) struct SelectNodePublicKeyOpenApi;

/// The request body for selecting a node's public key for encryption
/// from a client.
#[derive(Deserialize, Serialize, ToSchema)]
pub struct SelectNodePublicKeyRequest {
    /// The request model name
    model_name: String,
}

/// The response body for selecting a node's public key for encryption
/// from a client. The client will use the provided public key to encrypt
/// the request and send it back to the proxy. The proxy will then route this
/// request to the selected node.
#[derive(Deserialize, Serialize, ToSchema)]
pub struct SelectNodePublicKeyResponse {
    /// The public key for the selected node, base64 encoded
    public_key: Vec<u8>,
    /// The node small id for the selected node
    node_small_id: u64,
    /// Transaction digest for the transaction that acquires the stack entry, if any
    stack_entry_digest: Option<String>,
}

/// Handles requests to select a node's public key for confidential compute operations.
///
/// This endpoint attempts to find a suitable node and retrieve its public key for encryption
/// through a two-step process:
///
/// 1. First, it tries to select an existing node with a public key directly.
/// 2. If no node is immediately available, it falls back to finding the cheapest compatible node
///    and acquiring a new stack entry for it.
///
/// # Parameters
/// - `state`: The shared proxy state containing connections to the state manager and Sui
/// - `metadata`: Request metadata from middleware
/// - `request`: JSON payload containing the requested model name
///
/// # Returns
/// Returns a `Result` containing either:
/// - `Json<SelectNodePublicKeyResponse>` with:
///   - The selected node's public key (base64 encoded)
///   - The node's small ID
///   - Optional stack entry digest (if a new stack entry was acquired)
/// - `StatusCode` error if:
///   - `INTERNAL_SERVER_ERROR` - Communication errors or missing node public keys
///   - `SERVICE_UNAVAILABLE` - No nodes available for confidential compute
///
/// # Example Response
/// ```json
/// {
///     "public_key": [base64_encoded_bytes],
///     "node_small_id": 123,
///     "stack_entry_digest": "transaction_digest_string"
/// }
/// ```
///
/// This endpoint is specifically designed for confidential compute scenarios where
/// requests need to be encrypted before being processed by nodes.
#[utoipa::path(
    get,
    path = "",
    responses(
        (status = OK, description = "Node DH public key requested successfully", body = Value),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to request node DH public key"),
        (status = SERVICE_UNAVAILABLE, description = "No node found for model with confidential compute enabled for requested model")
    )
)]
#[instrument(
    level = "info",
    skip_all,
    fields(endpoint = metadata.endpoint)
)]
pub(crate) async fn select_node_public_key(
    State(state): State<ProxyState>,
    Extension(metadata): Extension<RequestMetadataExtension>,
    Json(request): Json<SelectNodePublicKeyRequest>,
) -> Result<Json<SelectNodePublicKeyResponse>, StatusCode> {
    let (sender, receiver) = oneshot::channel();
    state
        .state_manager_sender
        .send(
            AtomaAtomaStateManagerEvent::SelectNodePublicKeyForEncryption {
                model: request.model_name.clone(),
                max_num_tokens: MAX_NUM_TOKENS_FOR_CONFIDENTIAL_COMPUTE,
                result_sender: sender,
            },
        )
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let node_public_key = receiver.await.map_err(|_| {
        tracing::error!(
            target = "atoma-proxy",
            event = "select_node_public_key",
            "Failed to receive node public key"
        );
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    if let Some(node_public_key) = node_public_key {
        Ok(Json(SelectNodePublicKeyResponse {
            public_key: node_public_key.public_key,
            node_small_id: node_public_key.node_small_id as u64,
            stack_entry_digest: None,
        }))
    } else {
        let (sender, receiver) = oneshot::channel();
        state
            .state_manager_sender
            .send(AtomaAtomaStateManagerEvent::GetCheapestNodeForModel {
                model: request.model_name.clone(),
                is_confidential: true, // NOTE: This endpoint is only required for confidential compute
                result_sender: sender,
            })
            .map_err(|e| {
                error!("Failed to send GetCheapestNodeForModel event: {:?}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        let node = receiver
            .await
            .map_err(|_| {
                error!("Failed to receive GetCheapestNodeForModel result");
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .map_err(|e| {
                error!("Failed to get GetCheapestNodeForModel result: {:?}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        if let Some(node) = node {
            let stack_entry_resp = state
                .sui
                .write()
                .await
                .acquire_new_stack_entry(
                    node.task_small_id as u64,
                    node.max_num_compute_units as u64,
                    node.price_per_one_million_compute_units as u64,
                )
                .await
                .map_err(|e| {
                    error!("Failed to acquire new stack entry: {:?}", e);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
            // NOTE: The contract might select a different node than the one we used to extract
            // the price per one million compute units. In this case, we need to update the value of the `node_small_id``
            // to be the one selected by the contract, that we can query from the `StackCreatedEvent`.
            let node_small_id = stack_entry_resp.stack_created_event.selected_node_id.inner;
            // NOTE: We need to get the public key for the selected node for the acquired stack.
            let (sender, receiver) = oneshot::channel();
            state
                .state_manager_sender
                .send(
                    AtomaAtomaStateManagerEvent::SelectNodePublicKeyForEncryptionForNode {
                        node_small_id: node_small_id as i64,
                        result_sender: sender,
                    },
                )
                .map_err(|e| {
                    error!(
                        "Failed to send GetNodePublicKeyForEncryption event: {:?}",
                        e
                    );
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
            let node_public_key = receiver.await.map_err(|e| {
                error!(
                    "Failed to receive GetNodePublicKeyForEncryption result: {:?}",
                    e
                );
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
            if let Some(node_public_key) = node_public_key {
                Ok(Json(SelectNodePublicKeyResponse {
                    public_key: node_public_key.public_key,
                    node_small_id: node_public_key.node_small_id as u64,
                    stack_entry_digest: Some(stack_entry_resp.transaction_digest.to_string()),
                }))
            } else {
                error!("No node public key found for node {}", node.node_small_id);
                Err(StatusCode::INTERNAL_SERVER_ERROR)
            }
        } else {
            error!(
                "No node found for model {} with confidential compute enabled",
                request.model_name
            );
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
    }
}
