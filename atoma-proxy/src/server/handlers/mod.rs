use atoma_utils::encryption::decrypt_ciphertext;
use serde_json::Value;
use tracing::{info, instrument};
use x25519_dalek::SharedSecret;

use super::{
    error::AtomaProxyError, http_server::NODE_PUBLIC_ADDRESS_REGISTRATION_PATH,
    middleware::NodeEncryptionMetadata,
};
use crate::server::Result;

pub mod chat_completions;
pub mod embeddings;
pub mod image_generations;
pub mod request_model;
pub mod select_node_public_key;

/// Extracts encryption metadata from a JSON response containing encrypted data.
///
/// This function processes a JSON response that contains encrypted data and its associated nonce,
/// converting them from JSON array representations into byte vectors suitable for cryptographic operations.
///
/// # Arguments
///
/// * `response` - A JSON Value that must contain two fields:
///   * `ciphertext`: An array of numbers representing encrypted bytes
///   * `nonce`: An array of numbers representing the cryptographic nonce
///
/// # Returns
///
/// * `Ok(NodeEncryptionMetadata)` - A struct containing the extracted ciphertext and nonce as byte vectors
/// * `Err(AtomaProxyError)` - Returns INTERNAL_SERVER_ERROR (500) if:
///   * The ciphertext or nonce fields are missing from the response
///   * The fields are not arrays
///   * Array values cannot be converted to bytes
///   * The nonce cannot be converted to the required fixed-size array
///
/// # Example JSON Structure
///
/// ```json
/// {
///     "ciphertext": [1, 2, 3, ...],
///     "nonce": [4, 5, 6, ...]
/// }
/// ```
#[instrument(
    level = "info",
    skip(response),
    fields(event = "extract-node-encryption-metadata")
)]
pub(crate) fn extract_node_encryption_metadata(response: Value) -> Result<NodeEncryptionMetadata> {
    let ciphertext = response
        .get("ciphertext")
        .and_then(|ciphertext| ciphertext.as_array())
        .ok_or_else(|| AtomaProxyError::InternalError {
            message: "Failed to extract ciphertext from response".to_string(),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        })?;
    let ciphertext = ciphertext
        .iter()
        .map(|value| {
            value
                .as_u64()
                .map(|value| value as u8)
                .ok_or_else(|| AtomaProxyError::InternalError {
                    message: format!("Failed to extract ciphertext from response: {:?}", response),
                    endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
                })
        })
        .collect::<Result<Vec<u8>>>()?;
    let nonce = response
        .get("nonce")
        .and_then(|nonce| nonce.as_array())
        .ok_or_else(|| AtomaProxyError::InternalError {
            message: "Failed to extract nonce from response".to_string(),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        })?;
    let nonce = nonce
        .iter()
        .map(|value| {
            value
                .as_u64()
                .map(|value| value as u8)
                .ok_or_else(|| AtomaProxyError::InternalError {
                    message: format!("Failed to extract nonce from response: {:?}", response),
                    endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
                })
        })
        .collect::<Result<Vec<u8>>>()?;
    let nonce = nonce
        .try_into()
        .map_err(|e| AtomaProxyError::InternalError {
            message: format!("Failed to convert nonce to array, with error: {:?}", e),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        })?;
    Ok(NodeEncryptionMetadata { ciphertext, nonce })
}

/// Decrypts and deserializes an encrypted response from a confidential compute node.
///
/// This function handles the decryption of encrypted response data using the provided shared secret
/// and cryptographic parameters, then deserializes the decrypted data into a JSON value.
///
/// # Arguments
///
/// * `shared_secret` - The shared secret key used for decryption, generated from key exchange
/// * `ciphertext` - The encrypted data as a byte slice
/// * `salt` - Cryptographic salt used in the encryption process
/// * `nonce` - Unique cryptographic nonce (number used once) for this encryption
///
/// # Returns
///
/// * `Ok(Value)` - The decrypted and parsed JSON response
/// * `Err(AtomaProxyError)` - Returns INTERNAL_SERVER_ERROR (500) if:
///   * Decryption fails
///   * The decrypted data cannot be parsed as valid JSON
///
/// # Example
///
/// ```no_run
/// use serde_json::Value;
/// use x25519_dalek::SharedSecret;
///
/// let shared_secret = // ... obtained from key exchange
/// let ciphertext = // ... encrypted response bytes
/// let salt = // ... salt bytes
/// let nonce = // ... nonce bytes
///
/// let decrypted_response = handle_confidential_compute_decryption_response(
///     shared_secret,
///     &ciphertext,
///     &salt,
///     &nonce
/// )?;
/// ```
#[instrument(
    level = "info",
    skip_all,
    fields(event = "confidential-compute-decryption-response")
)]
pub(crate) fn handle_confidential_compute_decryption_response(
    shared_secret: SharedSecret,
    ciphertext: &[u8],
    salt: &[u8],
    nonce: &[u8],
) -> Result<Value> {
    info!(
        target: "atoma-proxy-service",
        event = "confidential-compute-decryption-response",
        "Decrypting new response",
    );
    let plaintext_response_body_bytes = decrypt_ciphertext(&shared_secret, ciphertext, salt, nonce)
        .map_err(|e| AtomaProxyError::InternalError {
            message: format!("Failed to decrypt response: {:?}", e),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        })?;
    let response_body = serde_json::from_slice(&plaintext_response_body_bytes).map_err(|_| {
        AtomaProxyError::InternalError {
            message: "Failed to parse response body as JSON".to_string(),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        }
    })?;
    Ok(response_body)
}

/// Decrypts and deserializes an encrypted streaming chunk from a confidential compute node.
///
/// This function handles the decryption of encrypted streaming data using the provided shared secret
/// and cryptographic parameters, then deserializes the decrypted chunk into a JSON value.
///
/// # Arguments
///
/// * `shared_secret` - A reference to the shared secret key used for decryption
/// * `ciphertext` - The encrypted chunk data as a byte slice
/// * `salt` - Cryptographic salt used in the encryption process
/// * `nonce` - Unique cryptographic nonce (number used once) for this encryption
///
/// # Returns
///
/// * `Ok(Value)` - The decrypted and parsed JSON chunk
/// * `Err(AtomaProxyError)` - Returns INTERNAL_SERVER_ERROR (500) if:
///   * Decryption fails
///   * The decrypted data cannot be parsed as valid JSON
///
/// # Example
///
/// ```rust,ignore
/// use serde_json::Value;
/// use x25519_dalek::SharedSecret;
///
/// let shared_secret = // ... obtained from key exchange
/// let ciphertext = // ... encrypted chunk bytes
/// let salt = // ... salt bytes
/// let nonce = // ... nonce bytes
///
/// let decrypted_chunk = handle_confidential_compute_decryption_streaming_chunk(
///     &shared_secret,
///     &ciphertext,
///     &salt,
///     &nonce
/// )?;
/// ```
pub(crate) fn handle_confidential_compute_decryption_streaming_chunk(
    shared_secret: &SharedSecret,
    ciphertext: &[u8],
    salt: &[u8],
    nonce: &[u8],
) -> Result<Value> {
    info!(
        target: "atoma-proxy-service",
        event = "confidential-compute-decryption-response",
        "Decrypting new response",
    );
    let plaintext_response_body_bytes = decrypt_ciphertext(shared_secret, ciphertext, salt, nonce)
        .map_err(|e| AtomaProxyError::InternalError {
            message: format!("Failed to decrypt response: {:?}", e),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        })?;
    let chunk = serde_json::from_slice::<Value>(&plaintext_response_body_bytes).map_err(|e| {
        AtomaProxyError::InternalError {
            message: format!("Failed to parse response body as JSON: {:?}", e),
            endpoint: NODE_PUBLIC_ADDRESS_REGISTRATION_PATH.to_string(),
        }
    })?;
    Ok(chunk)
}
