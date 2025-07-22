use std::hash::Hash;

use atoma_state::types::{NodeAttestation, UpdateNodeAttestation};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, put},
    Json, Router,
};
use blake2::Digest;
use fastcrypto::{
    ed25519::{Ed25519PublicKey, Ed25519Signature},
    secp256k1::{Secp256k1PublicKey, Secp256k1Signature},
    secp256r1::{Secp256r1PublicKey, Secp256r1Signature},
    traits::VerifyingKey,
};
use sui_sdk::types::{
    base_types::SuiAddress,
    crypto::{PublicKey, Signature, SignatureScheme, SuiSignature, ToFromBytes},
};
use tracing::{error, instrument};
use utoipa::OpenApi;

use crate::ProxyServiceState;

type Result<T> = std::result::Result<T, StatusCode>;

/// The path for the update of node attestation endpoint.
pub const UPDATE_NODE_ATTESTATION_PATH: &str = "/update_node_attestation";

/// The path for the get node attestation endpoint.
pub const GET_NODE_ATTESTATION_PATH: &str = "/get_node_attestation";

/// Returns a router with the subscriptions endpoint.
///
/// # Returns
/// * `Router<ProxyServiceState>` - A router with the subscriptions endpoint
pub fn nodes_router() -> Router<ProxyServiceState> {
    Router::new()
        .route(UPDATE_NODE_ATTESTATION_PATH, put(update_node_attestation))
        .route(
            &format!("{GET_NODE_ATTESTATION_PATH}/{{id}}"),
            get(get_node_attestation),
        )
}

/// OpenAPI documentation for the update_node_attestation endpoint.
///
/// This struct is used to generate OpenAPI documentation for the update_node_attestation
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(update_node_attestation))]
pub struct UpdateNodeAttestationOpenApi;

/// Updates the node attestation for a specific node.
///
///# Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `node_small_id` - The small ID of the node for which to update the attestation
/// * `attestation` - The new attestation data for the node
///
/// # Returns
///
/// * `Result<StatusCode>` - A status code indicating the result of the operation
///  - `Ok(StatusCode::OK)` - Successfully updated node attestation
/// - `Err(StatusCode::INTERNAL_SERVER_ERROR)` - Failed to update node attestation in state manager
///
/// # Example Request
///
/// ```json
/// {
///     "node_small_id": 123,
///     "attestation": [0,1,2,3],
///     "signature": [4,5,6,7]
/// }
/// ```
#[utoipa::path(
    put,
    path = "",
    request_body = UpdateNodeAttestation,
    responses(
        (status = OK, description = "Successfully updated node attestation"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to update node attestation")
    )
)]
#[instrument(level = "trace", skip_all)]
pub async fn update_node_attestation(
    State(proxy_service_state): State<ProxyServiceState>,
    Json(update_attestation): Json<UpdateNodeAttestation>,
) -> Result<StatusCode> {
    let signature = Signature::from_bytes(&update_attestation.signature).map_err(|_| {
        error!("Invalid signature format");
        StatusCode::BAD_REQUEST
    })?;

    let signature_bytes = signature.signature_bytes();
    let public_key_bytes = signature.public_key_bytes();
    let signature_scheme = signature.scheme();
    let public_key =
        PublicKey::try_from_bytes(signature_scheme, public_key_bytes).map_err(|e| {
            error!("Failed to extract public key from bytes, with error: {e}");
            StatusCode::BAD_REQUEST
        })?;
    let mut hasher = blake2::Blake2b::new();
    hasher.update(update_attestation.attestation.node_small_id.to_le_bytes());
    hasher.update(&update_attestation.attestation.attestation);
    let attestation_hash: [u8; 32] = hasher.finalize().into();

    match signature_scheme {
        SignatureScheme::ED25519 => {
            let public_key = Ed25519PublicKey::from_bytes(public_key.as_ref()).unwrap();
            let signature = Ed25519Signature::from_bytes(signature_bytes).unwrap();
            public_key
                .verify(&attestation_hash, &signature)
                .map_err(|_| {
                    error!("Failed to verify signature");
                    StatusCode::UNAUTHORIZED
                })?;
        }
        SignatureScheme::Secp256k1 => {
            let public_key = Secp256k1PublicKey::from_bytes(public_key.as_ref()).unwrap();
            let signature = Secp256k1Signature::from_bytes(signature_bytes).unwrap();
            public_key
                .verify(&attestation_hash, &signature)
                .map_err(|_| {
                    error!("Failed to verify signature");
                    StatusCode::UNAUTHORIZED
                })?;
        }
        SignatureScheme::Secp256r1 => {
            let public_key = Secp256r1PublicKey::from_bytes(public_key.as_ref()).unwrap();
            let signature = Secp256r1Signature::from_bytes(signature_bytes).unwrap();
            public_key
                .verify(&attestation_hash, &signature)
                .map_err(|_| {
                    error!("Failed to verify signature");
                    StatusCode::UNAUTHORIZED
                })?;
        }
        _ => {
            error!("Currently unsupported signature scheme");
            return Err(StatusCode::BAD_REQUEST);
        }
    }

    let public_address = proxy_service_state
        .atoma_state
        .get_node_public_address(update_attestation.attestation.node_small_id)
        .await
        .map_err(|_| {
            error!("Failed to get node public address");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or_else(|| {
            error!(
                "Node public address not found for node_small_id: {}",
                update_attestation.attestation.node_small_id
            );
            StatusCode::NOT_FOUND
        })?;

    let sui_address = SuiAddress::from(&public_key);

    if public_address != sui_address.to_string() {
        error!(
            "Public key does not match the sui address for node_small_id: {}",
            update_attestation.attestation.node_small_id
        );
        return Err(StatusCode::UNAUTHORIZED);
    }

    proxy_service_state
        .atoma_state
        .update_node_attestation(update_attestation.attestation)
        .await
        .map_err(|_| {
            error!("Failed to update node attestation");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(StatusCode::OK)
}

/// OpenAPI documentation for the get_node_attestation endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_node_attestation
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_node_attestation))]
pub struct GetNodeAttestationOpenApi;

/// Retrieves the node attestation for a specific node.
///
/// # Arguments
/// * `proxy_service_state` - The shared state containing the state manager
/// * `node_small_id` - The small ID of the node for which to retrieve the attestation
///
/// # Returns
/// * `Result<Json<NodeAttestation>>` - A JSON response containing the node attestation
///   - `Ok(Json<NodeAttestation>)` - Successfully retrieved node attestation
///   - `Err(StatusCode::INTERNAL_SERVER_ERROR)` - Failed to retrieve node attestation from state manager
///
/// # Example Response
/// Returns a JSON object representing the node attestation:
/// ```json
/// {
///     "node_small_id": 123,
///     "attestation": [0,1,2,3]
/// }
#[utoipa::path(
    get,
    path = "/{node_small_id}",
    params(
        ("node_small_id" = i64, description = "The small ID of the node for which to retrieve the attestation")
    ),
    responses(
        (status = OK, description = "Retrieves node attestation for a specific node"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get node attestation")
    )
)]
#[instrument(level = "trace", skip_all)]
pub async fn get_node_attestation(
    State(proxy_service_state): State<ProxyServiceState>,
    Path(node_small_id): Path<i64>,
) -> Result<Json<NodeAttestation>> {
    Ok(Json(
        proxy_service_state
            .atoma_state
            .get_node_attestation(node_small_id)
            .await
            .map_err(|_| {
                error!("Failed to get nodes subscriptions");
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .ok_or_else(|| {
                error!(
                    "Node attestation not found for node_small_id: {}",
                    node_small_id
                );
                StatusCode::NOT_FOUND
            })?,
    ))
}
