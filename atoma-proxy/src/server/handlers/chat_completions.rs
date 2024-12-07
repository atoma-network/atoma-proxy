use std::time::{Duration, Instant};

use crate::server::{
    handlers::{extract_node_encryption_metadata, handle_confidential_compute_decryption_response},
    http_server::ProxyState,
    middleware::{NodeEncryptionMetadata, RequestMetadataExtension},
    streamer::Streamer,
};
use atoma_state::types::AtomaAtomaStateManagerEvent;
use atoma_utils::constants;
use axum::body::Body;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response, Sse};
use axum::Extension;
use axum::{extract::State, http::HeaderMap, Json};
use serde_json::Value;
use tracing::{error, instrument};
use utoipa::OpenApi;
use x25519_dalek::PublicKey;

use super::request_model::RequestModel;

/// Path for the confidential chat completions endpoint.
///
/// This endpoint follows the OpenAI API format for chat completions, with additional
/// confidential processing (through AEAD encryption and TEE hardware).
pub const CONFIDENTIAL_CHAT_COMPLETIONS_PATH: &str = "/v1/confidential/chat/completions";

/// Path for the chat completions endpoint.
///
/// This endpoint follows the OpenAI API format for chat completions
/// and is used to process chat-based requests for AI model inference.
pub const CHAT_COMPLETIONS_PATH: &str = "/v1/chat/completions";

/// The interval for the keep-alive message in the SSE stream.
const STREAM_KEEP_ALIVE_INTERVAL_IN_SECONDS: u64 = 15;

/// Represents a chat completion request model following the OpenAI API format
pub struct RequestModelChatCompletions {
    /// The identifier of the model to use for the completion
    /// (e.g., "gpt-3.5-turbo", "gpt-4", etc.)
    model: String,

    /// Array of message objects that represent the conversation history
    /// Each message should contain a "role" (system/user/assistant) and "content"
    messages: Vec<Value>,

    /// The maximum number of tokens to generate in the completion
    /// This limits the length of the model's response
    max_tokens: u64,
}

/// OpenAPI documentation for the chat completions endpoint.
///
/// This struct is used to generate OpenAPI documentation for the chat completions
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(chat_completions_handler))]
pub(crate) struct ChatCompletionsOpenApi;

impl RequestModel for RequestModelChatCompletions {
    fn new(request: &Value) -> Result<Self, StatusCode> {
        let model = request
            .get("model")
            .and_then(|m| m.as_str())
            .ok_or(StatusCode::BAD_REQUEST)?;
        let messages = request
            .get("messages")
            .and_then(|m| m.as_array())
            .ok_or(StatusCode::BAD_REQUEST)?;
        let max_tokens = request
            .get("max_tokens")
            .and_then(|m| m.as_u64())
            .ok_or(StatusCode::BAD_REQUEST)?;
        Ok(Self {
            model: model.to_string(),
            messages: messages.to_vec(),
            max_tokens,
        })
    }

    fn get_model(&self) -> Result<String, StatusCode> {
        Ok(self.model.clone())
    }

    fn get_compute_units_estimate(&self, state: &ProxyState) -> Result<u64, StatusCode> {
        let tokenizer_index = state
            .models
            .iter()
            .position(|m| m == &self.model)
            .ok_or_else(|| {
                error!("Model not supported");
                StatusCode::BAD_REQUEST
            })?;
        let tokenizer = &state.tokenizers[tokenizer_index];

        let mut total_num_tokens = 0;

        for message in self.messages.iter() {
            let content = message
                .get("content")
                .and_then(|content| content.as_str())
                .ok_or(StatusCode::BAD_REQUEST)?;
            let num_tokens = tokenizer
                .encode(content, true)
                .map_err(|err| {
                    error!("Failed to encode message: {:?}", err);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?
                .get_ids()
                .len() as u64;
            total_num_tokens += num_tokens;
            // add 2 tokens as a safety margin, for start and end message delimiters
            total_num_tokens += 2;
            // add 1 token as a safety margin, for the role name of the message
            total_num_tokens += 1;
        }
        total_num_tokens += self.max_tokens;
        Ok(total_num_tokens)
    }
}

/// Handles the chat completions request.
///
/// This function processes chat completion requests by determining whether to use streaming
/// or non-streaming response handling based on the request payload. For streaming requests,
/// it configures additional options to track token usage.
///
/// # Arguments
///
/// * `metadata`: Extension containing request metadata (node address, ID, compute units, etc.)
/// * `state`: The shared state of the application
/// * `headers`: The headers of the request
/// * `payload`: The JSON payload containing the chat completion request
///
/// # Returns
///
/// Returns a Response containing either:
/// - A streaming SSE connection for real-time completions
/// - A single JSON response for non-streaming completions
///
/// # Errors
///
/// Returns an error status code if:
/// - The request processing fails
/// - The streaming/non-streaming handlers encounter errors
/// - The underlying inference service returns an error
#[utoipa::path(
    post,
    path = "",
    responses(
        (status = OK, description = "Chat completions", body = Value),
        (status = BAD_REQUEST, description = "Bad request"),
        (status = UNAUTHORIZED, description = "Unauthorized"),
        (status = INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
#[instrument(
    level = "info",
    skip_all,
    fields(
        path = metadata.endpoint,
        payload = ?payload,
    )
)]
pub async fn chat_completions_handler(
    Extension(metadata): Extension<RequestMetadataExtension>,
    State(state): State<ProxyState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<Response<Body>, StatusCode> {
    let is_streaming = payload
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if is_streaming {
        handle_streaming_response(
            state,
            metadata.node_address,
            metadata.node_id,
            headers,
            payload,
            metadata.num_compute_units as i64,
            metadata.selected_stack_small_id,
            metadata.endpoint,
            metadata.salt,
            metadata.node_x25519_public_key,
        )
        .await
    } else {
        handle_non_streaming_response(
            state,
            metadata.node_address,
            metadata.node_id,
            headers,
            payload,
            metadata.num_compute_units as i64,
            metadata.selected_stack_small_id,
            metadata.endpoint,
            metadata.salt,
            metadata.node_x25519_public_key,
        )
        .await
    }
}

/// Handles non-streaming chat completion requests by processing them through the inference service.
///
/// This function performs several key operations:
/// 1. Forwards the request to the inference service with appropriate headers
/// 2. Processes the response and extracts token usage
/// 3. Updates token usage tracking in the state manager
///
/// # Arguments
///
/// * `state` - Application state containing service configuration
/// * `node_address` - The address of the inference node to send the request to
/// * `signature` - Authentication signature for the request
/// * `selected_stack_small_id` - Unique identifier for the selected stack
/// * `headers` - HTTP headers to forward with the request
/// * `payload` - The JSON payload containing the chat completion request
/// * `estimated_total_tokens` - Estimated token count for the request
///
/// # Returns
///
/// Returns a `Result` containing the HTTP response from the inference service, or a `StatusCode` error.
///
/// # Errors
///
/// Returns `StatusCode::INTERNAL_SERVER_ERROR` if:
/// - The inference service request fails
/// - Response parsing fails
/// - State manager updates fail
///
/// # Example Response Structure
///
/// ```json
/// {
///     "choices": [...],
///     "usage": {
///         "total_tokens": 123,
///         "prompt_tokens": 45,
///         "completion_tokens": 78
///     }
/// }
/// ```
#[instrument(
    level = "info",
    skip_all,
    fields(
        path = endpoint,
        completion_type = "non-streaming",
        stack_small_id,
        estimated_total_tokens,
        payload_hash
    )
)]
#[allow(clippy::too_many_arguments)]
async fn handle_non_streaming_response(
    state: ProxyState,
    node_address: String,
    selected_node_id: i64,
    headers: HeaderMap,
    payload: Value,
    estimated_total_tokens: i64,
    selected_stack_small_id: i64,
    endpoint: String,
    salt: Option<[u8; constants::SALT_SIZE]>,
    node_x25519_public_key: Option<PublicKey>,
) -> Result<Response<Body>, StatusCode> {
    let client = reqwest::Client::new();
    let time = Instant::now();

    let response = client
        .post(format!("{}{}", node_address, endpoint))
        .headers(headers)
        .json(&payload)
        .send()
        .await
        .map_err(|err| {
            error!("Failed to send OpenAI API request: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .json::<Value>()
        .await
        .map_err(|err| {
            error!("Failed to parse OpenAI API response: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })
        .map(Json)?;

    let response = if let (Some(node_x25519_public_key), Some(salt)) =
        (node_x25519_public_key, salt)
    {
        let shared_secret = state.compute_shared_secret(&node_x25519_public_key);
        let NodeEncryptionMetadata { ciphertext, nonce } =
            extract_node_encryption_metadata(response.0)?;
        handle_confidential_compute_decryption_response(shared_secret, &ciphertext, &salt, &nonce)?
    } else {
        response.0
    };

    // Extract the response total number of tokens
    let total_tokens = response
        .get("usage")
        .and_then(|usage| usage.get("total_tokens"))
        .and_then(|total_tokens| total_tokens.as_u64())
        .map(|n| n as i64)
        .unwrap_or(0);

    let input_tokens = response
        .get("usage")
        .and_then(|usage| usage.get("completion_tokens"))
        .and_then(|total_tokens| total_tokens.as_u64())
        .map(|n| n as i64)
        .unwrap_or(0);

    let output_tokens = response
        .get("usage")
        .and_then(|usage| usage.get("prompt_tokens"))
        .and_then(|total_tokens| total_tokens.as_u64())
        .map(|n| n as i64)
        .unwrap_or(0);

    // NOTE: We need to update the stack num tokens, because the inference response might have produced
    // less tokens than estimated what we initially estimated, from the middleware.
    if let Err(e) = utils::update_state_manager(
        &state,
        selected_stack_small_id,
        estimated_total_tokens,
        total_tokens,
    )
    .await
    {
        error!("Error updating state manager: {}", e);
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    state
        .state_manager_sender
        .send(
            AtomaAtomaStateManagerEvent::UpdateNodeThroughputPerformance {
                node_small_id: selected_node_id,
                input_tokens,
                output_tokens,
                time: time.elapsed().as_secs_f64(),
            },
        )
        .map_err(|err| {
            error!("Error updating node throughput performance: {}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(response).into_response())
}

/// Handles streaming chat completion requests by establishing a Server-Sent Events (SSE) connection.
///
/// This function processes streaming chat completion requests by:
/// 1. Adding required streaming options to the payload
/// 2. Forwarding the request to the inference service
/// 3. Establishing an SSE connection with keep-alive functionality
/// 4. Setting up a Streamer to handle the response chunks and manage token usage
///
/// # Arguments
///
/// * `state` - Application state containing service configuration and connections
/// * `payload` - The JSON payload containing the chat completion request
/// * `stack_small_id` - Unique identifier for the stack making the request
/// * `estimated_total_tokens` - Estimated token count for the request
/// * `payload_hash` - BLAKE2b hash of the original request payload
///
/// # Returns
///
/// Returns a `Result` containing an SSE stream response, or a `StatusCode` error.
///
/// # Errors
///
/// Returns `StatusCode::INTERNAL_SERVER_ERROR` if:
/// - The inference service request fails
/// - The inference service returns a non-success status code
///
/// # Example Response Stream
///
/// The SSE stream will emit events in the following format:
/// ```text
/// data: {"choices": [...], "usage": null}
/// data: {"choices": [...], "usage": null}
/// data: {"choices": [...], "usage": {"prompt_tokens": 10, "completion_tokens": 20, "total_tokens": 30}}
/// ```
#[instrument(
    level = "info",
    skip_all,
    fields(
        path = endpoint,
        completion_type = "streaming",
        stack_small_id,
        estimated_total_tokens,
        payload_hash
    )
)]
#[allow(clippy::too_many_arguments)]
async fn handle_streaming_response(
    state: ProxyState,
    node_address: String,
    node_id: i64,
    headers: HeaderMap,
    payload: Value,
    estimated_total_tokens: i64,
    selected_stack_small_id: i64,
    endpoint: String,
    salt: Option<[u8; constants::SALT_SIZE]>,
    node_x25519_public_key: Option<PublicKey>,
) -> Result<Response<Body>, StatusCode> {
    // NOTE: If streaming is requested, add the include_usage option to the payload
    // so that the atoma node state manager can be updated with the total number of tokens
    // that were processed for this request.

    let client = reqwest::Client::new();
    let start = Instant::now();
    let response = client
        .post(format!("{}{}", node_address, endpoint))
        .headers(headers)
        .json(&payload)
        .send()
        .await
        .map_err(|e| {
            error!("Error sending request to inference service: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if !response.status().is_success() {
        error!("Inference service returned error: {}", response.status());
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }

    let stream = response.bytes_stream();

    let shared_secret = if let Some(node_x25519_public_key) = node_x25519_public_key {
        Some(state.compute_shared_secret(&node_x25519_public_key))
    } else {
        None
    };

    // Create the SSE stream
    let stream = Sse::new(Streamer::new(
        stream,
        state.state_manager_sender,
        selected_stack_small_id,
        estimated_total_tokens,
        start,
        node_id,
        shared_secret,
        salt,
    ))
    .keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_millis(STREAM_KEEP_ALIVE_INTERVAL_IN_SECONDS))
            .text("keep-alive"),
    );

    Ok(stream.into_response())
}

pub(crate) mod utils {
    use super::*;

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
    /// Returns `Ok(())` if both updates succeed, or a `StatusCode::INTERNAL_SERVER_ERROR` if either update fails.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The state manager channel is closed
    /// - Either update operation fails to complete
    pub(crate) async fn update_state_manager(
        state: &ProxyState,
        stack_small_id: i64,
        estimated_total_tokens: i64,
        total_tokens: i64,
    ) -> Result<(), StatusCode> {
        // Update stack num tokens
        state
            .state_manager_sender
            .send(AtomaAtomaStateManagerEvent::UpdateStackNumTokens {
                stack_small_id,
                estimated_total_tokens,
                total_tokens,
            })
            .map_err(|e| {
                error!("Error updating stack num tokens: {}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        Ok(())
    }
}
