use atoma_state::types::AtomaAtomaStateManagerEvent;
use flume::Sender;

use super::error::AtomaProxyError;
use crate::server::Result;

pub mod chat_completions;
pub mod embeddings;
pub mod image_generations;
pub mod request_model;
pub mod select_node_public_key;

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
