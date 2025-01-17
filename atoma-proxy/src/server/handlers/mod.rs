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
use sui_sdk::types::crypto::{PublicKey, Signature, SignatureScheme, SuiSignature};
use tracing::instrument;

use super::error::AtomaProxyError;
use crate::server::Result;

pub mod chat_completions;
pub mod embeddings;
pub mod image_generations;
pub mod models;
pub mod nodes;
pub mod request_model;

/// Key for the response hash in the payload
pub(crate) const RESPONSE_HASH_KEY: &str = "response_hash";

/// Key for the signature in the payload
pub(crate) const SIGNATURE_KEY: &str = "signature";

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
            message: format!("Error updating stack num tokens: {}", e),
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
                endpoint: "verify_signature".to_string(),
            })?;
    let response_hash = STANDARD.decode(response_hash).unwrap();

    if verify_hash {
        verify_response_hash(payload, &response_hash)?;
    }

    tracing::info!("FLAG: payload: {:?}", payload);

    let node_signature =
        payload[SIGNATURE_KEY]
            .as_str()
            .ok_or_else(|| AtomaProxyError::InternalError {
                message: "Missing signature in payload".to_string(),
                endpoint: "verify_signature".to_string(),
            })?;

    let signature =
        Signature::from_str(node_signature).map_err(|e| AtomaProxyError::InternalError {
            message: format!("Failed to create signature: {}", e),
            endpoint: "verify_signature".to_string(),
        })?;

    let public_key_bytes = signature.public_key_bytes();
    let public_key =
        PublicKey::try_from_bytes(signature.scheme(), public_key_bytes).map_err(|e| {
            AtomaProxyError::InternalError {
                message: format!("Failed to create public key: {}", e),
                endpoint: "verify_signature".to_string(),
            }
        })?;

    match signature.scheme() {
        SignatureScheme::ED25519 => {
            let public_key = Ed25519PublicKey::from_bytes(public_key.as_ref()).map_err(|e| {
                AtomaProxyError::InternalError {
                    message: format!("Failed to create public key: {}", e),
                    endpoint: "verify_signature".to_string(),
                }
            })?;
            let signature =
                Ed25519Signature::from_bytes(signature.signature_bytes()).map_err(|e| {
                    AtomaProxyError::InternalError {
                        message: format!("Failed to create ed25519 signature: {}", e),
                        endpoint: "verify_signature".to_string(),
                    }
                })?;
            public_key
                .verify(response_hash.as_slice(), &signature)
                .map_err(|e| AtomaProxyError::InternalError {
                    message: format!("Failed to verify ed25519 signature: {}", e),
                    endpoint: "verify_signature".to_string(),
                })?;
        }
        SignatureScheme::Secp256k1 => {
            let public_key = Secp256k1PublicKey::from_bytes(public_key.as_ref()).map_err(|e| {
                AtomaProxyError::InternalError {
                    message: format!("Failed to create secp256k1 public key: {}", e),
                    endpoint: "verify_signature".to_string(),
                }
            })?;
            let signature =
                Secp256k1Signature::from_bytes(signature.signature_bytes()).map_err(|e| {
                    AtomaProxyError::InternalError {
                        message: format!("Failed to create secp256k1 signature: {}", e),
                        endpoint: "verify_signature".to_string(),
                    }
                })?;
            public_key
                .verify(response_hash.as_slice(), &signature)
                .map_err(|_| AtomaProxyError::InternalError {
                    message: "Failed to verify secp256k1 signature".to_string(),
                    endpoint: "verify_signature".to_string(),
                })?;
        }
        SignatureScheme::Secp256r1 => {
            let public_key = Secp256r1PublicKey::from_bytes(public_key.as_ref()).map_err(|e| {
                AtomaProxyError::InternalError {
                    message: format!("Failed to create secp256r1 public key: {}", e),
                    endpoint: "verify_signature".to_string(),
                }
            })?;
            let signature =
                Secp256r1Signature::from_bytes(signature.signature_bytes()).map_err(|e| {
                    AtomaProxyError::InternalError {
                        message: format!("Failed to create secp256r1 signature: {}", e),
                        endpoint: "verify_signature".to_string(),
                    }
                })?;
            public_key
                .verify(response_hash.as_slice(), &signature)
                .map_err(|_| AtomaProxyError::InternalError {
                    message: "Failed to verify secp256r1 signature".to_string(),
                    endpoint: "verify_signature".to_string(),
                })?;
        }
        _ => {
            return Err(AtomaProxyError::InternalError {
                message: "Currently unsupported signature scheme".to_string(),
                endpoint: "verify_signature".to_string(),
            });
        }
    }

    Ok(())
}

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
            endpoint: "verify_signature".to_string(),
        });
    }
    Ok(())
}
