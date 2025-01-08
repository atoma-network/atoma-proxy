use std::str::FromStr;

use atoma_state::types::AtomaAtomaStateManagerEvent;
use atoma_utils::constants::SIGNATURE;
use atoma_utils::verify_signature;
use axum::body::Body;
use axum::extract::Request;
use axum::http::HeaderMap;
use axum::{extract::State, Json};
use blake2::digest::consts::U32;
use blake2::digest::generic_array::GenericArray;
use blake2::{Blake2b, Digest};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sui_sdk::types::base_types::SuiAddress;
use sui_sdk::types::crypto::{PublicKey as SuiPublicKey, Signature, SuiSignature};
use tokio::sync::oneshot;
use tracing::instrument;
use utoipa::{OpenApi, ToSchema};

use crate::server::error::AtomaProxyError;
use crate::server::http_server::ProxyState;

/// Path for the node public address registration endpoint.
///
/// This endpoint is used to register or update the public address of a node
/// in the system, ensuring that the system has the correct address for routing requests.
pub const NODE_PUBLIC_ADDRESS_REGISTRATION_PATH: &str = "/node/registration";

/// Size of the blake2b hash in bytes
const BODY_HASH_SIZE: usize = 32;

/// Body size limit for signature verification (contains the body size of the request)
const MAX_BODY_SIZE: usize = 1024 * 1024; // 1MB

#[derive(OpenApi)]
#[openapi(paths(node_public_address_registration))]
/// OpenAPI documentation for the node public address registration endpoint.
///
/// This struct is used to generate OpenAPI documentation for the node public address
/// registration endpoint. It uses the `utoipa` crate's derive macro to automatically
/// generate the OpenAPI specification from the code.
pub(crate) struct NodePublicAddressRegistrationOpenApi;

/// Register node
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
        (status = OK, description = "Node public address registered successfully", body = Value),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to register node public address")
    )
)]
#[instrument(level = "info", skip_all)]
pub async fn node_public_address_registration(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    request: Request<Body>,
) -> Result<Json<Value>, AtomaProxyError> {
    let base64_signature = headers
        .get(SIGNATURE)
        .ok_or_else(|| AtomaProxyError::MissingHeader {
            header: SIGNATURE.to_string(),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        })?
        .to_str()
        .map_err(|e| AtomaProxyError::InvalidHeader {
            message: format!("Failed to extract base64 signature encoding, with error: {e}"),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        })?;

    let body_bytes = axum::body::to_bytes(request.into_body(), MAX_BODY_SIZE)
        .await
        .map_err(|_| AtomaProxyError::InvalidBody {
            message: "Failed to convert body to bytes".to_string(),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        })?;

    let signature =
        Signature::from_str(base64_signature).map_err(|e| AtomaProxyError::InvalidBody {
            message: format!("Failed to parse signature, with error: {e}"),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        })?;

    let public_key_bytes = signature.public_key_bytes();
    let public_key =
        SuiPublicKey::try_from_bytes(signature.scheme(), public_key_bytes).map_err(|e| {
            AtomaProxyError::InvalidBody {
                message: format!("Failed to extract public key from bytes, with error: {e}"),
                endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
            }
        })?;
    let sui_address = SuiAddress::from(&public_key);

    let mut blake2b_hash = Blake2b::new();
    blake2b_hash.update(&body_bytes);
    let body_blake2b_hash: GenericArray<u8, U32> = blake2b_hash.finalize();
    let body_blake2b_hash_bytes: [u8; BODY_HASH_SIZE] = body_blake2b_hash
        .as_slice()
        .try_into()
        .map_err(|e| AtomaProxyError::InvalidBody {
            message: format!("Failed to convert blake2b hash to bytes, with error: {e}"),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        })?;
    verify_signature(base64_signature, &body_blake2b_hash_bytes).map_err(|e| {
        AtomaProxyError::InvalidBody {
            message: format!("Failed to verify signature, with error: {e}"),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        }
    })?;

    let payload =
        serde_json::from_slice::<NodePublicAddressAssignment>(&body_bytes).map_err(|e| {
            AtomaProxyError::InvalidBody {
                message: format!("Failed to parse request body, with error: {e}"),
                endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
            }
        })?;

    let (result_sender, result_receiver) = oneshot::channel();

    state
        .state_manager_sender
        .send(AtomaAtomaStateManagerEvent::GetNodeSuiAddress {
            node_small_id: payload.node_small_id as i64,
            result_sender,
        })
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to send GetNodeSuiAddress event: {:?}", err),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        })?;

    let node_sui_address = result_receiver
        .await
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to receive GetNodeSuiAddress result: {:?}", err),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        })?
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to get node Sui address: {:?}", err),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        })?
        .ok_or_else(|| AtomaProxyError::NotFound {
            message: "Node Sui address not found".to_string(),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        })?;

    // Check if the address associated with the small ID in the request matches the Sui address in the signature.
    if node_sui_address != sui_address.to_string() {
        return Err(AtomaProxyError::InvalidBody {
            message: "The sui address associated with the node small ID does not match the signature sui address".to_string(),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        });
    }

    state
        .state_manager_sender
        .send(AtomaAtomaStateManagerEvent::UpsertNodePublicAddress {
            node_small_id: payload.node_small_id as i64,
            public_address: payload.public_address.clone(),
            country: payload.country.clone(),
        })
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to send UpsertNodePublicAddress event: {:?}", err),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        })?;

    Ok(Json(Value::Null))
}

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
