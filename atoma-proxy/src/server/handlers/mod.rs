use std::str::FromStr;

use atoma_state::types::AtomaAtomaStateManagerEvent;
use base64::engine::{general_purpose::STANDARD, Engine};
use blake2::Digest;
use fastcrypto::{
    ed25519::{Ed25519PublicKey, Ed25519Signature},
    secp256k1::{Secp256k1PublicKey, Secp256k1Signature},
    secp256r1::{Secp256r1PublicKey, Secp256r1Signature},
    traits::{ToFromBytes, VerifyingKey},
};
use flume::Sender;
use reqwest::StatusCode;
use sui_sdk::types::crypto::{PublicKey, Signature, SignatureScheme, SuiSignature};
use tracing::instrument;

use super::{error::AtomaProxyError, ONE_MILLION};
use crate::server::Result;

pub mod chat_completions;
pub mod completions;
pub mod embeddings;
pub mod image_generations;
pub mod metrics;
pub mod models;
pub mod nodes;
pub mod request_model;

/// Key for the response hash in the payload
pub const RESPONSE_HASH_KEY: &str = "response_hash";

/// Key for the signature in the payload
pub const SIGNATURE_KEY: &str = "signature";

/// The stream field in the request payload.
pub const STREAM: &str = "stream";

/// The usage field in the response payload.
pub const USAGE: &str = "usage";

/// The total number of tokens field in the response payload.
pub const TOTAL_TOKENS: &str = "total_tokens";

/// The output tokens field in the response payload.
pub const COMPLETION_TOKENS: &str = "completion_tokens";

/// The prompt tokens field in the response payload.
pub const PROMPT_TOKENS: &str = "prompt_tokens";

/// The user id field in the response payload.
pub const USER_ID: &str = "user_id";

/// The path for the stop streamer endpoint.
const STOP_STREAMER_PATH: &str = "/v1/stop-streamer";

/// The interval for the keep-alive message in the SSE stream.
const STREAM_KEEP_ALIVE_INTERVAL_IN_SECONDS: u64 = 15;

/// Updates the state manager with token usage and hash information for a stack.
///
/// This function performs two main operations:
/// 1. Updates the token count for the stack with both estimated and actual usage
/// 2. Computes and updates a total hash combining the payload and response hashes
///
/// # Arguments
///
/// * `state` - Reference to the application state containing the state manager sender
/// * `stack_small_id` - Unique identifier for the stack
/// * `estimated_total_tokens` - The estimated number of tokens before processing
/// * `total_tokens` - The actual number of tokens used
/// * `payload_hash` - Hash of the request payload
/// * `response_hash` - Hash of the response data
///
/// # Returns
///
/// Returns `Ok(())` if both updates succeed, or a `AtomaProxyError::InternalError` if either update fails.
///
/// # Errors
///
/// This function will return an error if:
/// - The state manager channel is closed
/// - Either update operation fails to complete
#[instrument(
    level = "info",
    skip_all,
    fields(stack_small_id, estimated_total_tokens, total_tokens, endpoint)
)]
pub fn update_state_manager(
    state_manager_sender: &Sender<AtomaAtomaStateManagerEvent>,
    stack_small_id: i64,
    estimated_total_tokens: i64,
    total_tokens: i64,
    endpoint: &str,
) -> Result<()> {
    // Update stack num tokens
    state_manager_sender
        .send(AtomaAtomaStateManagerEvent::UpdateStackNumTokens {
            stack_small_id,
            estimated_total_tokens,
            total_tokens,
        })
        .map_err(|e| AtomaProxyError::InternalError {
            message: format!("Error updating stack num tokens: {e}"),
            client_message: None,
            endpoint: endpoint.to_string(),
        })?;
    Ok(())
}

/// Updates the state manager with amount usage for fiat requests.
///
/// Updates the amount count for the user with both estimated and actual usage
///
/// # Arguments
///
/// * `state` - Reference to the application state containing the state manager sender
/// * `user_id` - Unique identifier of the user
/// * `estimated_amount` - The estimated amount before processing
/// * `amount` - The actual amount used
///
/// # Returns
///
/// Returns `Ok(())` if both updates succeed, or a `AtomaProxyError::InternalError` if either update fails.
///
/// # Errors
///
/// This function will return an error if:
/// - The state manager channel is closed
/// - Either update operation fails to complete
#[instrument(
    level = "info",
    skip_all,
    fields(user_id, estimated_amount, amount, endpoint)
)]
#[allow(clippy::too_many_arguments)]
pub fn update_state_manager_fiat(
    state_manager_sender: &Sender<AtomaAtomaStateManagerEvent>,
    user_id: i64,
    estimated_input_tokens: i64,
    input_tokens: i64,
    estimated_output_tokens: i64,
    output_tokens: i64,
    price_per_one_million_input_compute_units: i64,
    price_per_one_million_output_compute_units: i64,
    model_name: String,
    endpoint: &str,
) -> Result<()> {
    let estimated_input_amount = i64::try_from(
        estimated_input_tokens as u128 * price_per_one_million_input_compute_units as u128
            / u128::from(ONE_MILLION),
    )
    .map_err(|e| AtomaProxyError::InternalError {
        message: format!("Error converting estimated amount: {e}"),
        client_message: None,
        endpoint: endpoint.to_string(),
    })?;
    let estimated_output_amount = i64::try_from(
        estimated_output_tokens as u128 * price_per_one_million_output_compute_units as u128
            / u128::from(ONE_MILLION),
    )
    .map_err(|e| AtomaProxyError::InternalError {
        message: format!("Error converting estimated amount: {e}"),
        client_message: None,
        endpoint: endpoint.to_string(),
    })?;
    let input_amount = i64::try_from(
        input_tokens as u128 * price_per_one_million_input_compute_units as u128
            / u128::from(ONE_MILLION),
    )
    .map_err(|e| AtomaProxyError::InternalError {
        message: format!("Error converting amount: {e}"),
        client_message: None,
        endpoint: endpoint.to_string(),
    })?;
    let output_amount = i64::try_from(
        output_tokens as u128 * price_per_one_million_output_compute_units as u128
            / u128::from(ONE_MILLION),
    )
    .map_err(|e| AtomaProxyError::InternalError {
        message: format!("Error converting amount: {e}"),
        client_message: None,
        endpoint: endpoint.to_string(),
    })?;
    // Update stack num tokens
    state_manager_sender
        .send(AtomaAtomaStateManagerEvent::UpdateStackNumTokensFiat {
            user_id,
            estimated_input_amount,
            input_amount,
            input_tokens,
            estimated_output_amount,
            output_amount,
            output_tokens,
            model_name,
        })
        .map_err(|e| AtomaProxyError::InternalError {
            message: format!("Error updating fiat balance: {e}"),
            client_message: None,
            endpoint: endpoint.to_string(),
        })?;
    Ok(())
}

/// Verifies a Sui signature from a handler response
///
/// # Arguments
///
/// * `payload` - JSON payload containing the response hash and its signature
/// * `node_public_key` - Public key of the node that signed the response
///
/// # Returns
///
/// Returns `Ok(())` if verification succeeds,
/// or an error if verification fails or signing fails
///
/// # Errors
///
/// This function will return an error if:
/// - The payload format is invalid
/// - The signature verification fails
#[allow(clippy::too_many_lines)]
#[instrument(level = "debug", skip_all)]
pub fn verify_response_hash_and_signature(
    payload: &serde_json::Value,
    verify_hash: bool,
) -> Result<()> {
    // Extract response hash and signature from payload
    let response_hash =
        payload[RESPONSE_HASH_KEY]
            .as_str()
            .ok_or_else(|| AtomaProxyError::InternalError {
                message: "Missing response_hash in payload".to_string(),
                client_message: Some("Invalid response from inference service".to_string()),
                endpoint: "verify_signature".to_string(),
            })?;
    let response_hash = STANDARD.decode(response_hash).unwrap();

    if verify_hash {
        verify_response_hash(payload, &response_hash)?;
    }

    let node_signature =
        payload[SIGNATURE_KEY]
            .as_str()
            .ok_or_else(|| AtomaProxyError::InternalError {
                message: "Missing signature in payload".to_string(),
                client_message: Some("Invalid response from inference service".to_string()),
                endpoint: "verify_signature".to_string(),
            })?;

    let signature =
        Signature::from_str(node_signature).map_err(|e| AtomaProxyError::InternalError {
            message: format!("Failed to create signature: {e}"),
            client_message: Some("Invalid response from inference service".to_string()),
            endpoint: "verify_signature".to_string(),
        })?;

    let public_key_bytes = signature.public_key_bytes();
    let public_key =
        PublicKey::try_from_bytes(signature.scheme(), public_key_bytes).map_err(|e| {
            AtomaProxyError::InternalError {
                message: format!("Failed to create public key: {e}"),
                client_message: Some("Invalid response from inference service".to_string()),
                endpoint: "verify_signature".to_string(),
            }
        })?;

    match signature.scheme() {
        SignatureScheme::ED25519 => {
            let public_key = Ed25519PublicKey::from_bytes(public_key.as_ref()).map_err(|e| {
                AtomaProxyError::InternalError {
                    message: format!("Failed to create public key: {e}"),
                    client_message: Some("Invalid response from inference service".to_string()),
                    endpoint: "verify_signature".to_string(),
                }
            })?;
            let signature =
                Ed25519Signature::from_bytes(signature.signature_bytes()).map_err(|e| {
                    AtomaProxyError::InternalError {
                        message: format!("Failed to create ed25519 signature: {e}"),
                        client_message: Some("Invalid response from inference service".to_string()),
                        endpoint: "verify_signature".to_string(),
                    }
                })?;
            public_key
                .verify(response_hash.as_slice(), &signature)
                .map_err(|e| AtomaProxyError::InternalError {
                    message: format!("Failed to verify ed25519 signature: {e}"),
                    client_message: Some("Invalid response from inference service".to_string()),
                    endpoint: "verify_signature".to_string(),
                })?;
        }
        SignatureScheme::Secp256k1 => {
            let public_key = Secp256k1PublicKey::from_bytes(public_key.as_ref()).map_err(|e| {
                AtomaProxyError::InternalError {
                    message: format!("Failed to create secp256k1 public key: {e}"),
                    client_message: Some("Invalid response from inference service".to_string()),
                    endpoint: "verify_signature".to_string(),
                }
            })?;
            let signature =
                Secp256k1Signature::from_bytes(signature.signature_bytes()).map_err(|e| {
                    AtomaProxyError::InternalError {
                        message: format!("Failed to create secp256k1 signature: {e}"),
                        client_message: Some("Invalid response from inference service".to_string()),
                        endpoint: "verify_signature".to_string(),
                    }
                })?;
            public_key
                .verify(response_hash.as_slice(), &signature)
                .map_err(|_| AtomaProxyError::InternalError {
                    message: "Failed to verify secp256k1 signature".to_string(),
                    client_message: Some("Invalid response from inference service".to_string()),
                    endpoint: "verify_signature".to_string(),
                })?;
        }
        SignatureScheme::Secp256r1 => {
            let public_key = Secp256r1PublicKey::from_bytes(public_key.as_ref()).map_err(|e| {
                AtomaProxyError::InternalError {
                    message: format!("Failed to create secp256r1 public key: {e}"),
                    client_message: Some("Invalid response from inference service".to_string()),
                    endpoint: "verify_signature".to_string(),
                }
            })?;
            let signature =
                Secp256r1Signature::from_bytes(signature.signature_bytes()).map_err(|e| {
                    AtomaProxyError::InternalError {
                        message: format!("Failed to create secp256r1 signature: {e}"),
                        client_message: Some("Invalid response from inference service".to_string()),
                        endpoint: "verify_signature".to_string(),
                    }
                })?;
            public_key
                .verify(response_hash.as_slice(), &signature)
                .map_err(|_| AtomaProxyError::InternalError {
                    message: "Failed to verify secp256r1 signature".to_string(),
                    client_message: Some("Invalid response from inference service".to_string()),
                    endpoint: "verify_signature".to_string(),
                })?;
        }
        _ => {
            return Err(AtomaProxyError::InternalError {
                message: "Currently unsupported signature scheme".to_string(),
                client_message: Some("Invalid response from inference service".to_string()),
                endpoint: "verify_signature".to_string(),
            });
        }
    }

    Ok(())
}

/// Verifies that a response hash matches the computed hash of the payload
///
/// This function computes a Blake2b hash of the payload (excluding the response_hash and signature fields)
/// and compares it with the provided response hash to ensure data integrity.
///
/// # Arguments
///
/// * `value` - The JSON payload to verify, containing the full response data
/// * `response_hash` - The expected Blake2b hash to verify against
///
/// # Returns
///
/// Returns `Ok(())` if the computed hash matches the provided response hash,
/// or an error if verification fails.
///
/// # Errors
///
/// Returns `AtomaProxyError::InternalError` if:
/// - The computed Blake2b hash does not match the provided response hash
///
/// # Example
///
/// ```rust,ignore
/// let payload = serde_json::json!({
///     "data": "example",
///     "response_hash": "base64_encoded_hash",
///     "signature": "signature_data"
/// });
/// let response_hash = decode_base64("base64_encoded_hash");
/// verify_response_hash(&payload, &response_hash)?;
/// ```
#[instrument(level = "debug", skip_all)]
fn verify_response_hash(value: &serde_json::Value, response_hash: &[u8]) -> Result<()> {
    let mut value_tmp = value.clone();
    if let Some(obj) = value_tmp.as_object_mut() {
        obj.remove(RESPONSE_HASH_KEY);
        obj.remove(SIGNATURE_KEY);
    }
    let mut hasher = blake2::Blake2b::new();
    hasher.update(value_tmp.to_string().as_bytes());
    let blake2_hash: [u8; 32] = hasher.finalize().into();
    if blake2_hash.as_slice() != response_hash {
        return Err(AtomaProxyError::InternalError {
            message: "Response hash does not match".to_string(),
            client_message: Some("Invalid response from inference service".to_string()),
            endpoint: "verify_signature".to_string(),
        });
    }
    Ok(())
}

/// Handles the status code returned by the inference service.
///
/// This function maps the status code to an appropriate error variant.
///
/// # Arguments
///
/// * `status_code` - The status code returned by the inference service
/// * `endpoint` - The API endpoint path where the request was received
/// * `error` - The error message returned by the inference service
///
/// # Returns
///
/// Returns an `AtomaServiceError` variant based on the status code.
#[instrument(level = "info", skip_all, fields(endpoint))]
pub fn handle_status_code_error(
    status_code: StatusCode,
    endpoint: &str,
    error: &str,
) -> Result<()> {
    match status_code {
        StatusCode::UNAUTHORIZED => Err(AtomaProxyError::AuthError {
            auth_error: error.to_string(), // The message coming here is from node in the format format!("Unauthorized response from inference service: {error}"),
            endpoint: endpoint.to_string(),
        }),
        StatusCode::INTERNAL_SERVER_ERROR => Err(AtomaProxyError::InternalError {
            message: format!("Inference service returned internal server error: {error}"),
            client_message: Some(format!(
                "Inference service returned status code error {status_code}"
            )),
            endpoint: endpoint.to_string(),
        }),
        StatusCode::BAD_REQUEST => Err(AtomaProxyError::InternalError {
            message: format!("Inference service returned bad request error: {error}"),
            client_message: Some(error.to_string()), // The message coming here is from node in the format format!("Inference service returned bad request error: {error}"),
            endpoint: endpoint.to_string(),
        }),
        StatusCode::LOCKED => Err(AtomaProxyError::Locked {
            message: format!("Inference service returned locked error: {error}"),
            endpoint: endpoint.to_string(),
        }),
        StatusCode::TOO_EARLY => Err(AtomaProxyError::UnavailableStack {
            message: format!("Inference service returned too early error: {error}"),
            endpoint: endpoint.to_string(),
        }),
        StatusCode::TOO_MANY_REQUESTS => Err(AtomaProxyError::TooManyRequests {
            message: format!("Inference service returned too many requests error: {error}"),
            endpoint: endpoint.to_string(),
        }),
        _ => Err(AtomaProxyError::InternalError {
            message: format!("Inference service returned non-success error: {error}"),
            client_message: Some(format!(
                "Inference service returned status code error {status_code}"
            )),
            endpoint: endpoint.to_string(),
        }),
    }
}
