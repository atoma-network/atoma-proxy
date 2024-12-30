use std::time::{Duration, Instant};

use crate::server::{
    error::AtomaProxyError,
    handlers::{extract_node_encryption_metadata, handle_confidential_compute_decryption_response},
    http_server::ProxyState,
    middleware::{NodeEncryptionMetadata, RequestMetadataExtension},
    streamer::Streamer,
};
use atoma_state::types::AtomaAtomaStateManagerEvent;
use atoma_utils::constants;
use axum::body::Body;
use axum::response::{IntoResponse, Response, Sse};
use axum::Extension;
use axum::{extract::State, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::types::chrono::{DateTime, Utc};
use tracing::instrument;
use utoipa::{OpenApi, ToSchema};
use x25519_dalek::PublicKey;

use super::request_model::RequestModel;
use crate::server::Result;

/// Path for the confidential chat completions endpoint.
///
/// This endpoint follows the OpenAI API format for chat completions, with additional
/// confidential processing (through AEAD encryption and TEE hardware).
pub const CONFIDENTIAL_CHAT_COMPLETIONS_PATH: &str = "/v1/confidential/chat/completions";

// TODO: Create a new endpoint for the confidential chat completions

/// Path for the chat completions endpoint.
///
/// This endpoint follows the OpenAI API format for chat completions
/// and is used to process chat-based requests for AI model inference.
pub const CHAT_COMPLETIONS_PATH: &str = "/v1/chat/completions";

/// The interval for the keep-alive message in the SSE stream.
const STREAM_KEEP_ALIVE_INTERVAL_IN_SECONDS: u64 = 15;

/// The model field in the request payload.
const MODEL: &str = "model";

/// The messages field in the request payload.
const MESSAGES: &str = "messages";

/// The max_tokens field in the request payload.
const MAX_TOKENS: &str = "max_tokens";

#[derive(OpenApi)]
#[openapi(
    paths(chat_completions_create, chat_completions_create_stream),
    components(schemas(
        ChatCompletionRequest,
        ChatCompletionMessage,
        ChatCompletionResponse,
        ChatCompletionChoice,
        CompletionUsage,
        ChatCompletionChunk,
        ChatCompletionChunkChoice,
        ChatCompletionChunkDelta
    ))
)]
pub(crate) struct ChatCompletionsOpenApi;

/// Create chat completion
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
    security(
        ("bearerAuth" = [])
    ),
    request_body = CreateChatCompletionRequest,
    responses(
        (status = OK, description = "Chat completions", content(
            (ChatCompletionResponse = "application/json"),
        )),
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
    )
)]
pub async fn chat_completions_create(
    Extension(metadata): Extension<RequestMetadataExtension>,
    State(state): State<ProxyState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<Response<Body>> {
    let is_streaming = payload
        .get("stream")
        .ok_or_else(|| AtomaProxyError::InvalidBody {
            message: "Missing or invalid 'stream' field".to_string(),
            endpoint: CHAT_COMPLETIONS_PATH.to_string(),
        })?
        .as_bool()
        .ok_or_else(|| AtomaProxyError::InvalidBody {
            message: "Invalid 'stream' field".to_string(),
            endpoint: CHAT_COMPLETIONS_PATH.to_string(),
        })?;

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
            metadata.model_name,
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
            metadata.model_name,
        )
        .await
    }
}

#[utoipa::path(
    post,
    path = "#stream",
    security(
        ("bearerAuth" = [])
    ),
    request_body = CreateChatCompletionStreamRequest,
    responses(
        (status = OK, description = "Chat completions", content(
            (ChatCompletionStreamResponse = "text/event-stream")
        )),
        (status = BAD_REQUEST, description = "Bad request"),
        (status = UNAUTHORIZED, description = "Unauthorized"),
        (status = INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
#[allow(dead_code)]
pub async fn chat_completions_create_stream(
    Extension(_metadata): Extension<RequestMetadataExtension>,
    State(_state): State<ProxyState>,
    _headers: HeaderMap,
    Json(_payload): Json<CreateChatCompletionStreamRequest>,
) -> Result<Response<Body>> {
    // This endpoint exists only for OpenAPI documentation
    // Actual streaming is handled by chat_completions_create
    Err(AtomaProxyError::NotImplemented {
        message: "Streaming is not implemented".to_string(),
        endpoint: CHAT_COMPLETIONS_PATH.to_string(),
    })
}

/// OpenAPI documentation structure for confidential chat completions endpoint.
///
/// This structure defines the OpenAPI (Swagger) documentation for the confidential chat completions
/// API endpoint. It includes all the relevant request/response schemas and path definitions for
/// secure, confidential chat interactions.
///
/// The confidential chat completions endpoint provides the same functionality as the regular
/// chat completions endpoint but with additional encryption and security measures for
/// sensitive data processing.
///
/// # Components
///
/// Includes schemas for:
/// * `ChatCompletionRequest` - The structure of incoming chat completion requests
/// * `ChatCompletionMessage` - Individual messages in the chat history
/// * `ChatCompletionResponse` - The response format for completed requests
/// * `ChatCompletionChoice` - Available completion choices in responses
/// * `CompletionUsage` - Token usage statistics
/// * `ChatCompletionChunk` - Streaming response chunks
/// * `ChatCompletionChunkChoice` - Choices within streaming chunks
/// * `ChatCompletionChunkDelta` - Incremental updates in streaming responses
#[derive(OpenApi)]
#[openapi(
    paths(confidential_chat_completions_create),
    components(schemas(
        ChatCompletionRequest,
        ChatCompletionMessage,
        ChatCompletionResponse,
        ChatCompletionChoice,
        CompletionUsage,
        ChatCompletionChunk,
        ChatCompletionChunkChoice,
        ChatCompletionChunkDelta
    ))
)]
pub(crate) struct ConfidentialChatCompletionsOpenApi;

/// Create confidential chat completion
///
/// This handler processes chat completion requests in a confidential manner, providing additional
/// encryption and security measures for sensitive data processing. It supports both streaming and
/// non-streaming responses while maintaining data confidentiality through AEAD encryption and TEE hardware,
/// for full private AI compute.
///
/// # Arguments
///
/// * `metadata` - Extension containing request metadata including:
///   * `endpoint` - The API endpoint being accessed
///   * `node_address` - Address of the inference node
///   * `node_id` - Identifier of the selected node
///   * `num_compute_units` - Available compute units
///   * `selected_stack_small_id` - Stack identifier
///   * `salt` - Optional salt for encryption
///   * `node_x25519_public_key` - Optional public key for encryption
///   * `model_name` - Name of the AI model being used
/// * `state` - Shared application state (ProxyState)
/// * `headers` - HTTP request headers
/// * `payload` - The chat completion request body
///
/// # Returns
///
/// Returns a `Result` containing either:
/// * An HTTP response with the chat completion result
/// * A streaming SSE connection for real-time completions
/// * An `AtomaProxyError` error if the request processing fails
///
/// # Errors
///
/// Returns `AtomaProxyError::InvalidBody` if:
/// * The 'stream' field is missing or invalid in the payload
///
/// Returns `AtomaProxyError::InternalError` if:
/// * The inference service request fails
/// * Response processing encounters errors
/// * State manager updates fail
///
/// # Security Features
///
/// * Utilizes AEAD encryption for request/response data
/// * Supports TEE (Trusted Execution Environment) processing
/// * Implements secure key exchange using X25519
/// * Maintains confidentiality throughout the request lifecycle
///
/// # Example
///
/// ```rust,ignore
/// let response = confidential_chat_completions_handler(
///     Extension(metadata),
///     State(state),
///     headers,
///     Json(payload)
/// ).await?;
/// ```
#[utoipa::path(
    post,
    path = "",
    request_body = CreateChatCompletionRequest,
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "Confidential chat completions", body = ChatCompletionResponse),
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
    )
)]
pub async fn confidential_chat_completions_create(
    Extension(metadata): Extension<RequestMetadataExtension>,
    State(state): State<ProxyState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<Response<Body>> {
    let is_streaming = payload
        .get("stream")
        .ok_or_else(|| AtomaProxyError::InvalidBody {
            message: "Missing or invalid 'stream' field".to_string(),
            endpoint: CONFIDENTIAL_CHAT_COMPLETIONS_PATH.to_string(),
        })?
        .as_bool()
        .ok_or_else(|| AtomaProxyError::InvalidBody {
            message: "Invalid 'stream' field".to_string(),
            endpoint: CONFIDENTIAL_CHAT_COMPLETIONS_PATH.to_string(),
        })?;

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
            metadata.model_name,
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
            metadata.model_name,
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
/// Returns a `Result` containing the HTTP response from the inference service, or an `AtomaProxyError` error.
///
/// # Errors
///
/// Returns `AtomaProxyError::InternalError` if:
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
    model_name: String,
) -> Result<Response<Body>> {
    let client = reqwest::Client::new();
    let time = Instant::now();

    let response = client
        .post(format!("{}{}", node_address, endpoint))
        .headers(headers)
        .json(&payload)
        .send()
        .await
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to send OpenAI API request: {:?}", err),
            endpoint: endpoint.to_string(),
        })?
        .json::<Value>()
        .await
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to parse OpenAI API response: {:?}", err),
            endpoint: endpoint.to_string(),
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
        return Err(AtomaProxyError::InternalError {
            message: format!("Error updating state manager: {}", e),
            endpoint: endpoint.to_string(),
        });
    }

    state
        .state_manager_sender
        .send(
            AtomaAtomaStateManagerEvent::UpdateNodeThroughputPerformance {
                timestamp: DateTime::<Utc>::from(std::time::SystemTime::now()),
                model_name,
                node_small_id: selected_node_id,
                input_tokens,
                output_tokens,
                time: time.elapsed().as_secs_f64(),
            },
        )
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Error updating node throughput performance: {}", err),
            endpoint: endpoint.to_string(),
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
/// Returns a `Result` containing an SSE stream response, or a `AtomaProxyError` error.
///
/// # Errors
///
/// Returns `AtomaProxyError::InternalError` if:
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
    model_name: String,
) -> Result<Response<Body>> {
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
        .map_err(|e| AtomaProxyError::InternalError {
            message: format!("Error sending request to inference service: {}", e),
            endpoint: endpoint.to_string(),
        })?;

    if !response.status().is_success() {
        return Err(AtomaProxyError::InternalError {
            message: format!("Inference service returned error: {}", response.status()),
            endpoint: endpoint.to_string(),
        });
    }

    let stream = response.bytes_stream();

    let shared_secret = node_x25519_public_key
        .map(|node_x25519_public_key| state.compute_shared_secret(&node_x25519_public_key));

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
        model_name,
    ))
    .keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_millis(STREAM_KEEP_ALIVE_INTERVAL_IN_SECONDS))
            .text("keep-alive"),
    );

    Ok(stream.into_response())
}

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

impl RequestModel for RequestModelChatCompletions {
    fn new(request: &Value) -> Result<Self> {
        let model = request.get(MODEL).and_then(|m| m.as_str()).ok_or_else(|| {
            AtomaProxyError::InvalidBody {
                message: "Missing or invalid 'model' field".to_string(),
                endpoint: CHAT_COMPLETIONS_PATH.to_string(),
            }
        })?;

        let messages = request
            .get(MESSAGES)
            .and_then(|m| m.as_array())
            .ok_or_else(|| AtomaProxyError::InvalidBody {
                message: "Missing or invalid 'messages' field".to_string(),
                endpoint: CHAT_COMPLETIONS_PATH.to_string(),
            })?;

        let max_tokens = request
            .get(MAX_TOKENS)
            .and_then(|m| m.as_u64())
            .ok_or_else(|| AtomaProxyError::InvalidBody {
                message: "Missing or invalid 'max_tokens' field".to_string(),
                endpoint: CHAT_COMPLETIONS_PATH.to_string(),
            })?;

        Ok(Self {
            model: model.to_string(),
            messages: messages.to_vec(),
            max_tokens,
        })
    }

    fn get_model(&self) -> Result<String> {
        Ok(self.model.clone())
    }

    fn get_compute_units_estimate(&self, state: &ProxyState) -> Result<u64> {
        let tokenizer_index = state
            .models
            .iter()
            .position(|m| m == &self.model)
            .ok_or_else(|| AtomaProxyError::InvalidBody {
                message: "Model not supported".to_string(),
                endpoint: CHAT_COMPLETIONS_PATH.to_string(),
            })?;
        let tokenizer = &state.tokenizers[tokenizer_index];

        let mut total_num_tokens = 0;

        for message in self.messages.iter() {
            let content = message
                .get("content")
                .and_then(|content| content.as_str())
                .ok_or_else(|| AtomaProxyError::InvalidBody {
                    message: "Missing or invalid message content".to_string(),
                    endpoint: CHAT_COMPLETIONS_PATH.to_string(),
                })?;

            let num_tokens = tokenizer
                .encode(content, true)
                .map_err(|err| AtomaProxyError::InternalError {
                    message: format!("Failed to encode message: {:?}", err),
                    endpoint: CHAT_COMPLETIONS_PATH.to_string(),
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
    /// Returns `Ok(())` if both updates succeed, or a `AtomaProxyError::InternalError` if either update fails.
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
    ) -> Result<()> {
        // Update stack num tokens
        state
            .state_manager_sender
            .send(AtomaAtomaStateManagerEvent::UpdateStackNumTokens {
                stack_small_id,
                estimated_total_tokens,
                total_tokens,
            })
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Error updating stack num tokens: {}", e),
                endpoint: CHAT_COMPLETIONS_PATH.to_string(),
            })?;
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateChatCompletionRequest {
    #[serde(flatten)]
    pub chat_completion_request: ChatCompletionRequest,

    /// Whether to stream back partial progress. Must be false for this request type.
    #[schema(default = false)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateChatCompletionStreamRequest {
    #[serde(flatten)]
    pub chat_completion_request: ChatCompletionRequest,

    /// Whether to stream back partial progress. Must be true for this request type.
    #[schema(default = true)]
    pub stream: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatCompletionRequest {
    /// ID of the model to use
    pub model: String,

    /// A list of messages comprising the conversation so far
    pub messages: Vec<ChatCompletionMessage>,

    /// What sampling temperature to use, between 0 and 2
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,

    /// An alternative to sampling with temperature
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,

    /// How many chat completion choices to generate for each input message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<i32>,

    /// Whether to stream back partial progress
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,

    /// Up to 4 sequences where the API will stop generating further tokens
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<Vec<String>>,

    /// The maximum number of tokens to generate in the chat completion
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<i32>,

    /// Number between -2.0 and 2.0. Positive values penalize new tokens based on
    /// whether they appear in the text so far
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,

    /// Number between -2.0 and 2.0. Positive values penalize new tokens based on their
    /// existing frequency in the text so far
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,

    /// Modify the likelihood of specified tokens appearing in the completion
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logit_bias: Option<std::collections::HashMap<String, f32>>,

    /// A unique identifier representing your end-user
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,

    /// A list of functions the model may generate JSON inputs for
    #[serde(skip_serializing_if = "Option::is_none")]
    pub functions: Option<Vec<Value>>,

    /// Controls how the model responds to function calls
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call: Option<Value>,

    /// The format to return the response in
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<Value>,

    /// A list of tools the model may call
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,

    /// Controls which (if any) tool the model should use
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,

    /// If specified, our system will make a best effort to sample deterministically
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatCompletionMessage {
    /// The role of the message author. One of: "system", "user", "assistant", "tool", or "function"
    pub role: String,

    /// The contents of the message
    pub content: String,

    /// The name of the author of this message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatCompletionResponse {
    /// A unique identifier for the chat completion.
    pub id: String,

    /// The Unix timestamp (in seconds) of when the chat completion was created.
    pub created: i64,

    /// The model used for the chat completion.
    pub model: String,

    /// A list of chat completion choices.
    pub choices: Vec<ChatCompletionChoice>,

    /// Usage statistics for the completion request.
    pub usage: Option<CompletionUsage>,

    /// The system fingerprint for the completion, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_fingerprint: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatCompletionStreamResponse {
    /// The stream of chat completion chunks.
    pub data: ChatCompletionChunk,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatCompletionChoice {
    /// The index of this choice in the list of choices.
    pub index: i32,

    /// The chat completion message.
    pub message: ChatCompletionMessage,

    /// The reason the chat completion was finished.
    pub finish_reason: Option<String>,

    /// Log probability information for the choice, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logprobs: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CompletionUsage {
    /// Number of tokens in the prompt.
    pub prompt_tokens: i32,

    /// Number of tokens in the completion.
    pub completion_tokens: i32,

    /// Total number of tokens used (prompt + completion).
    pub total_tokens: i32,
}

// For streaming responses
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatCompletionChunk {
    /// A unique identifier for the chat completion chunk.
    pub id: String,

    /// The Unix timestamp (in seconds) of when the chunk was created.
    pub created: i64,

    /// The model used for the chat completion.
    pub model: String,

    /// A list of chat completion chunk choices.
    pub choices: Vec<ChatCompletionChunkChoice>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatCompletionChunkChoice {
    /// The index of this choice in the list of choices.
    pub index: i32,

    /// The chat completion delta message for streaming.
    pub delta: ChatCompletionChunkDelta,

    /// The reason the chat completion was finished, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChatCompletionChunkDelta {
    /// The role of the message author, if present in this chunk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,

    /// The content of the message, if present in this chunk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    /// The function call information, if present in this chunk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_call: Option<Value>,

    /// The tool calls information, if present in this chunk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<Value>>,
}
