use std::time::{Duration, Instant};

use crate::server::streamer::ClientStreamer;
use crate::server::types::{ConfidentialComputeResponse, ConfidentialComputeStreamResponse};
use crate::server::{
    error::AtomaProxyError, http_server::ProxyState, middleware::RequestMetadataExtension,
    streamer::Streamer, types::ConfidentialComputeRequest,
};
use atoma_state::types::AtomaAtomaStateManagerEvent;
use atoma_utils::constants::REQUEST_ID;
use axum::body::Body;
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response, Sse};
use axum::Extension;
use axum::{extract::State, http::HeaderMap, Json};
use base64::engine::{general_purpose::STANDARD, Engine};
use futures::StreamExt;
use openai_api::message::Role;
use openai_api::tools::ToolFunction;
use openai_api::{
    completion_choice::{
        ChatCompletionChoice, ChatCompletionChunkChoice, ChatCompletionChunkDelta,
    },
    logprobs::ChatCompletionLogProb,
    logprobs::{ChatCompletionLogProbs, ChatCompletionLogProbsContent},
    message::ChatCompletionMessage,
    message_content::{MessageContent, MessageContentPart},
    stop_reason::StopReason,
    token_details::PromptTokensDetails,
    tools::{ChatCompletionChunkDeltaToolCall, ChatCompletionChunkDeltaToolCallFunction},
    tools::{Tool, ToolCall, ToolCallFunction},
    usage::CompletionUsage,
    ChatCompletionChunk, ChatCompletionRequest, ChatCompletionResponse,
};
use openai_api::{CreateChatCompletionRequest, CreateChatCompletionStreamRequest};
use opentelemetry::KeyValue;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::types::chrono::{DateTime, Utc};
use tokenizers::Tokenizer;
use tracing::instrument;
use utoipa::OpenApi;

use super::metrics::{
    CHAT_COMPLETIONS_COMPLETIONS_TOKENS, CHAT_COMPLETIONS_INPUT_TOKENS,
    CHAT_COMPLETIONS_LATENCY_METRICS, CHAT_COMPLETIONS_NUM_REQUESTS, CHAT_COMPLETIONS_TOTAL_TOKENS,
    TOTAL_COMPLETED_REQUESTS, TOTAL_FAILED_CHAT_REQUESTS, TOTAL_FAILED_REQUESTS,
};
use super::request_model::{ComputeUnitsEstimate, RequestModel};
use super::{
    handle_status_code_error, update_state_manager, verify_response_hash_and_signature,
    RESPONSE_HASH_KEY,
};
use crate::server::{Result, DEFAULT_MAX_TOKENS, MAX_COMPLETION_TOKENS, MAX_TOKENS, MODEL};

/// Path for the confidential chat completions endpoint.
///
/// This endpoint follows the OpenAI API format for chat completions, with additional
/// confidential processing (through AEAD encryption and TEE hardware).
pub const CONFIDENTIAL_CHAT_COMPLETIONS_PATH: &str = "/v1/confidential/chat/completions";

/// The key for the content field in the request payload.
///
/// This is used to represent the content of a message in the chat completion request.
/// It can be either a text or an array of content parts.
pub const CONTENT_KEY: &str = "content";

/// Path for the chat completions endpoint.
///
/// This endpoint follows the OpenAI API format for chat completions
/// and is used to process chat-based requests for AI model inference.
pub const CHAT_COMPLETIONS_PATH: &str = "/v1/chat/completions";

/// Path for the completions endpoint.
///
/// This endpoint follows the OpenAI API format for legacy completions
/// and is used to process chat-based requests for AI model inference.
pub const COMPLETIONS_PATH: &str = "/v1/completions";

/// The interval for the keep-alive message in the SSE stream.
const STREAM_KEEP_ALIVE_INTERVAL_IN_SECONDS: u64 = 15;

/// The messages field in the request payload.
const MESSAGES: &str = "messages";

/// The stream field in the request payload.
const STREAM: &str = "stream";

/// The path for the stop streamer endpoint.
const STOP_STREAMER_PATH: &str = "/v1/stop-streamer";

#[derive(OpenApi)]
#[openapi(
    paths(chat_completions_create, chat_completions_create_stream),
    components(schemas(
        ChatCompletionRequest,
        ChatCompletionMessage,
        ChatCompletionResponse,
        ChatCompletionChoice,
        CompletionUsage,
        PromptTokensDetails,
        ChatCompletionChunk,
        ChatCompletionChunkChoice,
        ChatCompletionChunkDelta,
        ToolCall,
        Tool,
        ToolCallFunction,
        ToolFunction,
        MessageContent,
        MessageContentPart,
        StopReason,
        ChatCompletionChunkDeltaToolCall,
        ChatCompletionChunkDeltaToolCallFunction,
        ChatCompletionLogProbs,
        ChatCompletionLogProbsContent,
        ChatCompletionLogProb,
        Role,
    ))
)]
pub struct ChatCompletionsOpenApi;

/// Create chat completion
///
/// This function processes chat completion requests by determining whether to use streaming
/// or non-streaming response handling based on the request payload. For streaming requests,
/// it configures additional options to track token usage.
///
/// ## Returns
///
/// Returns a Response containing either:
/// - A streaming SSE connection for real-time completions
/// - A single JSON response for non-streaming completions
///
/// ## Errors
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
    let endpoint = metadata.endpoint.clone();
    tokio::spawn(async move {
        // TODO: We should allow cancelling the request if the client disconnects
        let is_streaming = payload
            .get(STREAM)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or_default();

        match handle_chat_completions_request(&state, &metadata, headers, payload, is_streaming)
            .await
        {
            Ok(response) => {
                TOTAL_COMPLETED_REQUESTS.add(1, &[KeyValue::new("model", metadata.model_name)]);
                Ok(response)
            }
            Err(e) => {
                let model_label: String = metadata.model_name.clone();
                TOTAL_FAILED_CHAT_REQUESTS.add(1, &[KeyValue::new("model", model_label)]);
                update_state_manager(
                    &state.state_manager_sender,
                    metadata.selected_stack_small_id,
                    metadata.max_total_num_compute_units as i64,
                    0,
                    &metadata.endpoint,
                )?;
                Err(e)
            }
        }
    })
    .await
    .map_err(|e| AtomaProxyError::InternalError {
        message: format!("Failed to spawn image generation task: {e:?}"),
        client_message: None,
        endpoint,
    })?
}

/// Create completion stream
#[utoipa::path(
    post,
    path = "#stream",
    security(
        ("bearerAuth" = [])
    ),
    request_body = CompletionRequest,
    responses(
        (status = OK, description = "Chat completions", content(
            (CompletionResponse = "text/event-stream")
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
pub async fn completions_create(
    Extension(metadata): Extension<RequestMetadataExtension>,
    State(state): State<ProxyState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<Response<Body>> {
    // Transform the payload
    if let Some(prompt) = payload.get("prompt") {
        let messages = match prompt {
            Value::String(single_prompt) => {
                // Single string prompt
                vec![json!({ "role": "user", "content": single_prompt })]
            }
            Value::Array(prompts) => {
                // Array of string prompts
                prompts
                    .iter()
                    .filter_map(|p| p.as_str().map(|s| json!({ "role": "user", "content": s })))
                    .collect()
            }
            _ => {
                return Err(AtomaProxyError::RequestError {
                    message: "Invalid 'prompt' field".to_string(),
                    endpoint: COMPLETIONS_PATH.to_string(),
                });
            }
        };

        // Replace "prompt" with "messages"
        payload.as_object_mut().unwrap().remove("prompt");
        payload
            .as_object_mut()
            .unwrap()
            .insert("messages".to_string(), Value::Array(messages));
    } else {
        return Err(AtomaProxyError::RequestError {
            message: "Missing 'prompt' field".to_string(),
            endpoint: COMPLETIONS_PATH.to_string(),
        });
    }

    // Forward the transformed payload to /v1/chat/completions
    chat_completions_create(metadata, state, headers, Json(payload)).await
}

/// Routes chat completion requests to either streaming or non-streaming handlers based on the request type.
///
/// This function serves as a router that directs incoming chat completion requests to the appropriate
/// handler based on whether streaming is requested. It handles both regular and confidential chat
/// completion requests.
///
/// # Arguments
///
/// * `state` - Reference to the application's shared state containing service configuration
/// * `metadata` - Request metadata containing:
///   * `node_address` - Address of the inference node
///   * `node_id` - Identifier of the selected node
///   * `num_compute_units` - Available compute units
///   * `selected_stack_small_id` - Stack identifier
///   * `endpoint` - The API endpoint being accessed
///   * `model_name` - Name of the AI model being used
/// * `headers` - HTTP request headers to forward to the inference service
/// * `payload` - The JSON payload containing the chat completion request
/// * `is_streaming` - Boolean flag indicating whether to use streaming response
///
/// # Returns
///
/// Returns a `Result` containing either:
/// * A streaming SSE response for real-time completions
/// * A single JSON response for non-streaming completions
///
/// # Errors
///
/// Returns an error if either the streaming or non-streaming handler encounters an error:
/// * Network communication failures
/// * Invalid response formats
/// * State management errors
///
/// # Example
///
/// ```rust,ignore
/// let response = handle_chat_completions_request(
///     &state,
///     &metadata,
///     headers,
///     payload,
///     true // for streaming
/// ).await?;
/// ```
#[instrument(
    level = "info",
    skip_all,
    fields(
        path = metadata.endpoint,
    )
)]
async fn handle_chat_completions_request(
    state: &ProxyState,
    metadata: &RequestMetadataExtension,
    headers: HeaderMap,
    payload: Value,
    is_streaming: bool,
) -> Result<Response<Body>> {
    // Record the request in the chat completions num requests metric
    CHAT_COMPLETIONS_NUM_REQUESTS.add(1, &[KeyValue::new("model", metadata.model_name.clone())]);

    if is_streaming {
        handle_streaming_response(
            state,
            &metadata.node_address,
            metadata.node_id,
            headers,
            &payload,
            metadata.num_input_tokens.map(|v| v as i64),
            metadata.max_total_num_compute_units as i64,
            metadata.selected_stack_small_id,
            metadata.endpoint.clone(),
            metadata.model_name.clone(),
        )
        .await
    } else {
        handle_non_streaming_response(
            state,
            &metadata.node_address,
            metadata.node_id,
            headers,
            &payload,
            metadata.max_total_num_compute_units as i64,
            metadata.selected_stack_small_id,
            metadata.endpoint.clone(),
            metadata.model_name.clone(),
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
    request_body = openai_api::CreateChatCompletionStreamRequest,
    responses(
        (status = OK, description = "Chat completions", content(
            (openai_api::ChatCompletionStreamResponse = "text/event-stream")
        )),
        (status = BAD_REQUEST, description = "Bad request"),
        (status = UNAUTHORIZED, description = "Unauthorized"),
        (status = INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
#[allow(dead_code)]
pub fn chat_completions_create_stream(
    Extension(_metadata): Extension<RequestMetadataExtension>,
    State(_state): State<ProxyState>,
    _headers: HeaderMap,
    Json(_payload): Json<CreateChatCompletionStreamRequest>,
) -> Result<Response<Body>> {
    // This endpoint exists only for OpenAPI documentation
    // Actual streaming is handled by chat_completions_create
    Err(AtomaProxyError::NotImplemented {
        message: "This is a mock endpoint for OpenAPI documentation".to_string(),
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
    paths(
        confidential_chat_completions_create,
        confidential_chat_completions_create_stream
    ),
    components(schemas(ConfidentialComputeRequest))
)]
pub struct ConfidentialChatCompletionsOpenApi;

/// Create confidential chat completion
///
/// This handler processes chat completion requests in a confidential manner, providing additional
/// encryption and security measures for sensitive data processing. It supports both streaming and
/// non-streaming responses while maintaining data confidentiality through AEAD encryption and TEE hardware,
/// for full private AI compute.
///
/// ## Returns
///
/// Returns a `Result` containing either:
/// * An HTTP response with the chat completion result
/// * A streaming SSE connection for real-time completions
/// * An `AtomaProxyError` error if the request processing fails
///
/// ## Errors
///
/// Returns `AtomaProxyError::InvalidBody` if:
/// * The 'stream' field is missing or invalid in the payload
///
/// Returns `AtomaProxyError::InternalError` if:
/// * The inference service request fails
/// * Response processing encounters errors
/// * State manager updates fail
///
/// ## Security Features
///
/// * Utilizes AEAD encryption for request/response data
/// * Supports TEE (Trusted Execution Environment) processing
/// * Implements secure key exchange using X25519
/// * Maintains confidentiality throughout the request lifecycle
#[utoipa::path(
    post,
    path = "",
    request_body = ConfidentialComputeRequest,
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "Confidential chat completions", body = ConfidentialComputeResponse),
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
    let endpoint = metadata.endpoint.clone();
    tokio::spawn(async move {
        // TODO: We should allow cancelling the request if the client disconnects
        let is_streaming = payload
            .get(STREAM)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or_default();

        match handle_chat_completions_request(&state, &metadata, headers, payload, is_streaming)
            .await
        {
            Ok(response) => {
                TOTAL_COMPLETED_REQUESTS.add(1, &[KeyValue::new("model", metadata.model_name)]);
                Ok(response)
            }
            Err(e) => {
                let model_label: String = metadata.model_name.clone();
                TOTAL_FAILED_CHAT_REQUESTS.add(1, &[KeyValue::new("model", model_label.clone())]);

                // Record the failed request in the total failed requests metric
                TOTAL_FAILED_REQUESTS.add(1, &[KeyValue::new("model", model_label)]);

                update_state_manager(
                    &state.state_manager_sender,
                    metadata.selected_stack_small_id,
                    metadata.max_total_num_compute_units as i64,
                    0,
                    &metadata.endpoint,
                )?;
                Err(e)
            }
        }
    })
    .await
    .map_err(|e| AtomaProxyError::InternalError {
        message: format!("Failed to spawn image generation task: {e:?}"),
        client_message: None,
        endpoint,
    })?
}

#[utoipa::path(
    post,
    path = "#stream",
    security(
        ("bearerAuth" = [])
    ),
    request_body = ConfidentialComputeRequest,
    responses(
        (status = OK, description = "Chat completions", content(
            (ConfidentialComputeStreamResponse = "text/event-stream")
        )),
        (status = BAD_REQUEST, description = "Bad request"),
        (status = UNAUTHORIZED, description = "Unauthorized"),
        (status = INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
#[allow(dead_code)]
pub fn confidential_chat_completions_create_stream(
    Extension(_metadata): Extension<RequestMetadataExtension>,
    State(_state): State<ProxyState>,
    _headers: HeaderMap,
    Json(_payload): Json<ConfidentialComputeStreamResponse>,
) -> Result<Response<Body>> {
    // This endpoint exists only for OpenAPI documentation
    // Actual streaming is handled by chat_completions_create
    Err(AtomaProxyError::NotImplemented {
        message: "This is a mock endpoint for OpenAPI documentation".to_string(),
        endpoint: CHAT_COMPLETIONS_PATH.to_string(),
    })
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
    state: &ProxyState,
    node_address: &String,
    selected_node_id: i64,
    headers: HeaderMap,
    payload: &Value,
    estimated_total_tokens: i64,
    selected_stack_small_id: i64,
    endpoint: String,
    model_name: String,
) -> Result<Response<Body>> {
    let client = reqwest::Client::new();
    let time = Instant::now();

    let model_label = model_name.clone();

    let response = client
        .post(format!("{node_address}{endpoint}"))
        .headers(headers)
        .json(&payload)
        .send()
        .await
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to send OpenAI API request: {err:?}"),
            client_message: Some("Failed to connect to the node.".to_string()),
            endpoint: endpoint.to_string(),
        })?;

    if !response.status().is_success() {
        let error = response
            .status()
            .canonical_reason()
            .unwrap_or("Unknown error");
        handle_status_code_error(response.status(), &endpoint, error)?;
    }

    let response = response
        .json::<Value>()
        .await
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to parse OpenAI API response: {err:?}"),
            client_message: None,
            endpoint: endpoint.to_string(),
        })
        .map(Json)?;

    // Extract the response total number of tokens
    let total_tokens = response
        .get("usage")
        .and_then(|usage| usage.get("total_tokens"))
        .and_then(serde_json::Value::as_u64)
        .map_or(0, |n| n as i64);

    let input_tokens = response
        .get("usage")
        .and_then(|usage| usage.get("completion_tokens"))
        .and_then(serde_json::Value::as_u64)
        .map_or(0, |n| n as i64);

    let output_tokens = response
        .get("usage")
        .and_then(|usage| usage.get("prompt_tokens"))
        .and_then(serde_json::Value::as_u64)
        .map_or(0, |n| n as i64);

    // Record the total tokens in the chat completions tokens metrics
    CHAT_COMPLETIONS_TOTAL_TOKENS.add(
        total_tokens as u64,
        &[KeyValue::new("model", model_name.clone())],
    );
    CHAT_COMPLETIONS_INPUT_TOKENS.add(
        input_tokens as u64,
        &[KeyValue::new("model", model_name.clone())],
    );
    CHAT_COMPLETIONS_COMPLETIONS_TOKENS.add(
        output_tokens as u64,
        &[KeyValue::new("model", model_name.clone())],
    );

    let verify_hash = endpoint != CONFIDENTIAL_CHAT_COMPLETIONS_PATH;
    verify_response_hash_and_signature(&response.0, verify_hash)?;

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
            message: format!("Error updating node throughput performance: {err:?}"),
            client_message: None,
            endpoint: endpoint.to_string(),
        })?;

    // NOTE: It is not very secure to rely on the node's computed response hash,
    // if the node is not running in a TEE, but for now it suffices
    let total_hash = response
        .get(RESPONSE_HASH_KEY)
        .and_then(|hash| hash.as_str())
        .map(|hash| STANDARD.decode(hash).unwrap_or_default())
        .unwrap_or_default()
        .try_into()
        .map_err(|e: Vec<u8>| AtomaProxyError::InternalError {
            message: format!(
                "Error converting response hash to array, received array of length {}",
                e.len()
            ),
            client_message: None,
            endpoint: endpoint.to_string(),
        })?;

    state
        .state_manager_sender
        .send(AtomaAtomaStateManagerEvent::UpdateStackTotalHash {
            stack_small_id: selected_stack_small_id,
            total_hash,
        })
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Error updating stack total hash: {err:?}"),
            client_message: None,
            endpoint: endpoint.to_string(),
        })?;

    // NOTE: We need to update the stack num tokens, because the inference response might have produced
    // less tokens than estimated what we initially estimated, from the middleware.
    if let Err(e) = update_state_manager(
        &state.state_manager_sender,
        selected_stack_small_id,
        estimated_total_tokens,
        total_tokens,
        &endpoint,
    ) {
        return Err(AtomaProxyError::InternalError {
            message: format!("Error updating state manager: {e:?}"),
            client_message: None,
            endpoint: endpoint.to_string(),
        });
    }

    CHAT_COMPLETIONS_LATENCY_METRICS.record(
        time.elapsed().as_secs_f64(),
        &[KeyValue::new("model", model_label)],
    );

    Ok(response.into_response())
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
    state: &ProxyState,
    node_address: &String,
    node_id: i64,
    mut headers: HeaderMap,
    payload: &Value,
    num_input_tokens: Option<i64>,
    estimated_total_tokens: i64,
    selected_stack_small_id: i64,
    endpoint: String,
    model_name: String,
) -> Result<Response<Body>> {
    let client = reqwest::Client::new();
    let start = Instant::now();

    let request_id = uuid::Uuid::new_v4().to_string();
    headers.insert(REQUEST_ID, HeaderValue::from_str(&request_id).unwrap());
    let response = client
        .post(format!("{node_address}{endpoint}"))
        .headers(headers)
        .json(&payload)
        .send()
        .await
        .map_err(|e| AtomaProxyError::InternalError {
            message: format!("Error sending request to inference service: {e:?}"),
            client_message: Some("Failed to connect to the node.".to_string()),
            endpoint: endpoint.to_string(),
        })?;

    if !response.status().is_success() {
        let error = response
            .status()
            .canonical_reason()
            .unwrap_or("Unknown error");
        handle_status_code_error(response.status(), &endpoint, error)?;
    }

    tracing::info!(
        target = "atoma-service-chat-completions",
        "Streaming response received"
    );
    let stream = response.bytes_stream();

    let (event_sender, event_receiver) = flume::unbounded();
    let (kill_signal_sender, kill_signal_receiver) = flume::unbounded();
    let state_manager_sender = state.state_manager_sender.clone();
    let node_address_clone = node_address.clone();
    let request_id_clone = request_id.clone();
    let client_clone = client.clone();
    tokio::spawn(async move {
        tracing::info!(
            target = "atoma-service-chat-completions",
            "Starting streamer"
        );
        let mut streamer = Streamer::new(
            stream,
            state_manager_sender,
            selected_stack_small_id,
            num_input_tokens.unwrap_or(0),
            estimated_total_tokens,
            start,
            node_id,
            model_name,
            endpoint,
        );
        loop {
            tokio::select! {
                event = streamer.next() => {
                    match event {
                        Some(Ok(maybe_chunk)) => {
                            tracing::info!(target = "atoma-service-chat-completions", "Sending chunk to event sender");
                            if let Err(e) = event_sender.send(maybe_chunk) {
                                tracing::error!(
                                    target = "atoma-service-chat-completions",
                                    level = "error",
                                    "Error sending chunk: {e}"
                                );
                                // We continue the loop, to allow the streamer to finish with updated usage from the node
                                continue;
                            }
                        }
                        Some(Err(e)) => {
                            tracing::error!(
                                target = "atoma-service-chat-completions",
                                level = "error",
                                "Error sending chunk for the inner streamer with error: {e}"
                            );
                            continue;
                        }
                        None => {
                            break;
                        }
                    }
                }
                _ = kill_signal_receiver.recv_async() => {
                    tracing::info!(target = "atoma-service-streamer", "Received kill signal, stopping streamer");
                    let stop_response = client_clone
                        .post(format!("{node_address_clone}{STOP_STREAMER_PATH}"))
                        .header("X-Request-ID", request_id_clone.clone())
                        .send()
                        .await;

                    if let Err(e) = stop_response {
                        tracing::error!(
                            target = "atoma-service-streamer",
                            level = "error",
                            "Failed to notify node of client disconnect: {}",
                            e
                        );
                    }
                    // We continue the loop, to allow the streamer to finish with updated usage from the node
                    continue;
                }
            }
        }
        tracing::info!(
            target = "atoma-service-chat-completions",
            "Streamer finished for request id: {request_id}"
        );
    });

    // Create the SSE stream
    // NOTE: The number of input tokens is only available for non-confidential requests,
    // for confidential requests, the input message is encrypted. In that case, we set it to `0`,
    // and we allow for the proxy to underestimate the total tokens, relative to the node, if
    // the last stream chunk is not received (containing full usage). This situation is fine,
    // for two reasons:
    // 1. The node is running in confidential compute mode, so we can trust the real usage information by node
    //    (say when claiming stacks from the blockchain).
    // 2. It only affects requests whose connection is dropped before the final chunk is processed, by the client.
    let stream = Sse::new(ClientStreamer::new(event_receiver, kill_signal_sender)).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_millis(STREAM_KEEP_ALIVE_INTERVAL_IN_SECONDS))
            .text("keep-alive"),
    );

    Ok(stream.into_response())
}

/// Represents a chat completion request model following the OpenAI API format
#[derive(Debug, Clone)]
pub struct RequestModelChatCompletions {
    /// The identifier of the model to use for the completion
    /// (e.g., "gpt-3.5-turbo", "meta-llama/Llama-3.3-70B-Instruct", etc.)
    model: String,

    /// Array of message objects that represent the conversation history
    /// Each message should contain a "role" (system/user/assistant) and "content"
    /// The content can be a string or an array of content parts.
    messages: Vec<Value>,

    /// The maximum number of tokens to generate in the completion
    /// This limits the length of the model's response
    max_completion_tokens: u64,
}

impl RequestModel for RequestModelChatCompletions {
    fn new(request: &Value) -> Result<Self> {
        let model = request.get(MODEL).and_then(|m| m.as_str()).ok_or_else(|| {
            AtomaProxyError::RequestError {
                message: "Missing or invalid 'model' field".to_string(),
                endpoint: CHAT_COMPLETIONS_PATH.to_string(),
            }
        })?;

        let messages = request
            .get(MESSAGES)
            .and_then(|m| m.as_array())
            .ok_or_else(|| AtomaProxyError::RequestError {
                message: "Missing or invalid 'messages' field".to_string(),
                endpoint: CHAT_COMPLETIONS_PATH.to_string(),
            })?;

        let max_completion_tokens = request
            .get(MAX_COMPLETION_TOKENS)
            .or_else(|| request.get(MAX_TOKENS))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(DEFAULT_MAX_TOKENS);

        Ok(Self {
            model: model.to_string(),
            messages: messages.clone(),
            max_completion_tokens,
        })
    }

    fn get_model(&self) -> String {
        self.model.clone()
    }

    /// Computes the total number of tokens for the chat completion request.
    ///
    /// This is used to estimate the cost of the chat completion request, on the proxy side.
    /// We support either string or array of content parts. We further assume that all content messages
    /// share the same previous messages. That said, we further assume that content parts formatted into arrays
    /// are to be concatenated and treated as a single message, by the model and from the estimate point of view.
    fn get_compute_units_estimate(
        &self,
        tokenizer: Option<&Tokenizer>,
    ) -> Result<ComputeUnitsEstimate> {
        // In order to account for the possibility of not taking into account possible additional special tokens,
        // which might not be considered by the tokenizer, we add a small overhead to the total number of tokens, per message.
        const MESSAGE_OVERHEAD_TOKENS: u64 = 3;
        let Some(tokenizer) = tokenizer else {
            return Err(AtomaProxyError::InternalError {
                client_message: Some("No available tokenizer found for current model, try again later or open a ticket".to_string()),
                message: "Tokenizer not found for current model".to_string(),
                endpoint: CHAT_COMPLETIONS_PATH.to_string(),
            });
        };
        // Helper function to count tokens for a text string
        let count_text_tokens = |text: &str| -> Result<u64> {
            Ok(tokenizer
                .encode(text, true)
                .map_err(|err| AtomaProxyError::InternalError {
                    message: format!("Failed to encode message: {err:?}"),
                    client_message: Some(
                        "Failed to encode message using the model's tokenizer".to_string(),
                    ),
                    endpoint: CHAT_COMPLETIONS_PATH.to_string(),
                })?
                .get_ids()
                .len() as u64)
        };

        let mut total_num_tokens = 0;

        for message in &self.messages {
            let content = message
                .get(CONTENT_KEY)
                .and_then(|content| MessageContent::deserialize(content).ok())
                .ok_or_else(|| AtomaProxyError::RequestError {
                    message: "Missing or invalid message content".to_string(),
                    endpoint: CHAT_COMPLETIONS_PATH.to_string(),
                })?;

            match content {
                MessageContent::Text(text) => {
                    let num_tokens = count_text_tokens(&text)?;
                    total_num_tokens += num_tokens + MESSAGE_OVERHEAD_TOKENS;
                }
                MessageContent::Array(parts) => {
                    if parts.is_empty() {
                        tracing::error!(
                            "Received empty array of message parts for chat completion request"
                        );
                        return Err(AtomaProxyError::RequestError {
                            message: "Missing or invalid message content".to_string(),
                            endpoint: CHAT_COMPLETIONS_PATH.to_string(),
                        });
                    }
                    for part in parts {
                        match part {
                            MessageContentPart::Text { text, .. } => {
                                let num_tokens = count_text_tokens(&text)?;
                                total_num_tokens += num_tokens + MESSAGE_OVERHEAD_TOKENS;
                            }
                            MessageContentPart::Image { .. } => {
                                // TODO: Ensure that for image content parts, we have a way to estimate the number of tokens,
                                // which can depend on the size of the image and the output description.
                                continue;
                            }
                        }
                    }
                }
            }
        }
        // add the max completion tokens, to account for the response
        Ok(ComputeUnitsEstimate {
            num_input_compute_units: total_num_tokens,
            max_total_compute_units: total_num_tokens + self.max_completion_tokens,
        })
    }
}

pub mod openai_api {
    use serde::{Deserialize, Deserializer, Serialize};
    use serde_json::Value;
    use utoipa::ToSchema;

    /// Represents the create chat completion request.
    ///
    /// This is used to represent the create chat completion request in the chat completion request.
    /// It can be either a chat completion or a chat completion stream.
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

    /// Represents the chat completion request.
    ///
    /// This is used to represent the chat completion request in the chat completion request.
    /// It can be either a chat completion or a chat completion stream.
    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    pub struct ChatCompletionRequest {
        /// ID of the model to use
        #[schema(example = "meta-llama/Llama-3.3-70B-Instruct")]
        pub model: String,

        /// A list of messages comprising the conversation so far
        #[schema(example = json!([
            {
                "role": "system",
                "content": "You are a helpful AI assistant"
            },
            {
                "role": "user",
                "content": "Hello!"
            },
            {
                "role": "assistant",
                "content": "I'm here to help you with any questions you have. How can I assist you today?"
            }
        ]))]
        pub messages: Vec<message::ChatCompletionMessage>,

        /// What sampling temperature to use, between 0 and 2
        #[schema(example = 0.7)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub temperature: Option<f32>,

        /// An alternative to sampling with temperature
        #[schema(example = 1.0)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub top_p: Option<f32>,

        /// How many chat completion choices to generate for each input message
        #[schema(example = 1)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub n: Option<i32>,

        /// Whether to stream back partial progress
        #[schema(example = false)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub stream: Option<bool>,

        /// Up to 4 sequences where the API will stop generating further tokens
        #[schema(example = "json([\"stop\", \"halt\"])", default = "[]")]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub stop: Option<Vec<String>>,

        /// The maximum number of tokens to generate in the chat completion
        #[schema(example = 4096)]
        #[serde(skip_serializing_if = "Option::is_none")]
        #[deprecated = "It is recommended to use max_completion_tokens instead"]
        pub max_tokens: Option<i32>,

        /// The maximum number of tokens to generate in the chat completion
        #[schema(example = 4096)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub max_completion_tokens: Option<i32>,

        /// Number between -2.0 and 2.0. Positive values penalize new tokens based on
        /// whether they appear in the text so far
        #[schema(example = 0.0)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub presence_penalty: Option<f32>,

        /// Number between -2.0 and 2.0. Positive values penalize new tokens based on their
        /// existing frequency in the text so far
        #[schema(example = 0.0)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub frequency_penalty: Option<f32>,

        /// Modify the likelihood of specified tokens appearing in the completion.
        ///
        /// Accepts a JSON object that maps tokens (specified by their token ID in the tokenizer)
        /// to an associated bias value from -100 to 100. Mathematically, the bias is added to the logits
        /// generated by the model prior to sampling. The exact effect will vary per model, but values
        /// between -1 and 1 should decrease or increase likelihood of selection; values like -100 or
        /// 100 should result in a ban or exclusive selection of the relevant token.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[schema(example = json!({
            "1234567890": 0.5,
            "1234567891": -0.5
        }))]
        pub logit_bias: Option<std::collections::HashMap<u32, f32>>,

        /// An integer between 0 and 20 specifying the number of most likely tokens to return at each token position, each with an associated log probability.
        /// logprobs must be set to true if this parameter is used.
        #[schema(example = 1)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub top_logprobs: Option<i32>,

        /// A unique identifier representing your end-user
        #[schema(example = "user-1234")]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub user: Option<String>,

        /// A list of functions the model may generate JSON inputs for
        #[serde(skip_serializing_if = "Option::is_none")]
        #[schema(example = json!([
            {
                "name": "get_current_weather",
                "description": "Get the current weather in a location",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "location": {
                            "type": "string",
                            "description": "The location to get the weather for"
                        }
                    },
                    "required": ["location"]
                }
            }
        ]))]
        pub functions: Option<Vec<Value>>,

        /// Controls how the model responds to function calls
        #[serde(skip_serializing_if = "Option::is_none")]
        #[schema(example = json!("auto"))]
        pub function_call: Option<Value>,

        /// The format to return the response in
        #[serde(skip_serializing_if = "Option::is_none")]
        #[schema(example = json!("json_object"))]
        pub response_format: Option<response_format::ResponseFormat>,

        /// A list of tools the model may call
        #[serde(skip_serializing_if = "Option::is_none")]
        #[schema(example = json!([
            {
                "type": "function",
                "function": {
                    "name": "get_current_weather",
                    "description": "Get the current weather in a location",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "location": {
                                "type": "string",
                                "description": "The location to get the weather for"
                            }
                        },
                        "required": ["location"]
                    }
                }
            }
        ]))]
        pub tools: Option<Vec<tools::ChatCompletionToolsParam>>,

        /// Controls which (if any) tool the model should use
        #[serde(skip_serializing_if = "Option::is_none")]
        #[schema(example = json!("auto"))]
        pub tool_choice: Option<tools::ToolChoice>,

        /// If specified, our system will make a best effort to sample deterministically
        #[schema(example = 123)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub seed: Option<i64>,

        /// Specifies the latency tier to use for processing the request. This parameter is relevant for customers subscribed to the scale tier service:
        ///
        /// If set to 'auto', and the Project is Scale tier enabled, the system will utilize scale tier credits until they are exhausted.
        /// If set to 'auto', and the Project is not Scale tier enabled, the request will be processed using the default service tier with a lower uptime SLA and no latency guarantee.
        /// If set to 'default', the request will be processed using the default service tier with a lower uptime SLA and no latency guarantee.
        /// When not set, the default behavior is 'auto'.
        #[schema(example = "auto")]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub service_tier: Option<String>,

        /// Options for streaming response. Only set this when you set stream: true.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[schema(example = json!({
            "include_usage": true
        }))]
        pub stream_options: Option<stream_options::StreamOptions>,

        /// Whether to enable parallel tool calls.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[schema(example = true)]
        pub parallel_tool_calls: Option<bool>,
    }

    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    pub struct CompletionRequest {
        /// ID of the model to use
        #[schema(example = "meta-llama/Llama-3.3-70B-Instruct")]
        pub model: String,

        /// The prompt to generate completions for
        #[schema(example = json!(["Hello!"]))]
        pub prompt: Vec<String>,

        #[schema(example = 1, default = 1)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub best_of: Option<i32>,

        #[schema(example = false, default = false)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub echo: Option<bool>,

        /// Number between -2.0 and 2.0. Positive values penalize new tokens based on their
        /// existing frequency in the text so far
        #[schema(example = 0.0)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub frequency_penalty: Option<f32>,

        /// Modify the likelihood of specified tokens appearing in the completion.
        ///
        /// Accepts a JSON object that maps tokens (specified by their token ID in the tokenizer)
        /// to an associated bias value from -100 to 100. Mathematically, the bias is added to the logits
        /// generated by the model prior to sampling. The exact effect will vary per model, but values
        /// between -1 and 1 should decrease or increase likelihood of selection; values like -100 or
        /// 100 should result in a ban or exclusive selection of the relevant token.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[schema(example = json!({
            "1234567890": 0.5,
            "1234567891": -0.5
        }))]
        pub logit_bias: Option<std::collections::HashMap<u32, f32>>,

        /// An integer between 0 and 20 specifying the number of most likely tokens to return at each token position, each with an associated log probability.
        /// logprobs must be set to true if this parameter is used.
        #[schema(example = 1)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub logprobs: Option<i32>,

        /// The maximum number of tokens to generate in the chat completion
        #[schema(example = 4096, default = 16)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub max_tokens: Option<i32>,

        /// How many chat completion choices to generate for each input message
        #[schema(example = 1)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub n: Option<i32>,

        /// Number between -2.0 and 2.0. Positive values penalize new tokens based on
        /// whether they appear in the text so far
        #[schema(example = 0.0)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub presence_penalty: Option<f32>,

        /// If specified, our system will make a best effort to sample deterministically
        #[schema(example = 123)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub seed: Option<i64>,

        /// Up to 4 sequences where the API will stop generating further tokens
        #[schema(example = "json([\"stop\", \"halt\"])", default = "[]")]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub stop: Option<Vec<String>>,

        /// Whether to stream back partial progress
        #[schema(example = false)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub stream: Option<bool>,

        /// Options for streaming response. Only set this when you set stream: true.
        #[schema(example = json!({"include_usage": true}))]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub stream_options: Option<stream_options::StreamOptions>,

        /// The suffix that comes after a completion of inserted text.
        #[schema(example = "json(\"\\n\")")]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub suffix: Option<String>,

        /// What sampling temperature to use, between 0 and 2
        #[schema(example = 0.7)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub temperature: Option<f32>,

        /// An alternative to sampling with temperature
        #[schema(example = 1.0)]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub top_p: Option<f32>,

        /// A unique identifier representing your end-user
        #[schema(example = "user-1234")]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub user: Option<String>,
    }

    /// Represents the chat completion response.
    ///
    /// This is used to represent the chat completion response in the chat completion request.
    /// It can be either a chat completion or a chat completion stream.
    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    pub struct ChatCompletionResponse {
        /// A unique identifier for the chat completion.
        #[schema(example = "chatcmpl-123")]
        pub id: String,

        /// The Unix timestamp (in seconds) of when the chat completion was created.
        #[schema(example = 1_677_652_288)]
        pub created: i64,

        /// The model used for the chat completion.
        #[schema(example = "meta-llama/Llama-3.3-70B-Instruct")]
        pub model: String,

        /// A list of chat completion choices.
        #[schema(
            example = "[{\"index\": 0, \"message\": {\"role\": \"assistant\", \"content\": \"Hello! How can you help me today?\"}, \"finish_reason\": null, \"stop_reason\": null}]"
        )]
        pub choices: Vec<completion_choice::ChatCompletionChoice>,

        /// Usage statistics for the completion request.
        #[schema(
            example = "{\"prompt_tokens\": 100, \"completion_tokens\": 200, \"total_tokens\": 300}"
        )]
        pub usage: Option<usage::CompletionUsage>,

        /// The system fingerprint for the completion, if applicable.
        #[schema(example = "fp_44709d6fcb")]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub system_fingerprint: Option<String>,

        /// The object of the chat completion.
        #[schema(example = "chat.completion")]
        pub object: String,

        /// The service tier of the chat completion.
        #[schema(example = "auto")]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub service_tier: Option<String>,
    }

    /// Represents the chat completion stream response.
    ///
    /// This is used to represent the chat completion stream response in the chat completion request.
    /// It can be either a chat completion chunk or a chat completion stream.
    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    pub struct ChatCompletionStreamResponse {
        /// The stream of chat completion chunks.
        pub data: ChatCompletionChunk,
    }

    /// Represents the chat completion chunk.
    ///
    /// This is used to represent the chat completion chunk in the chat completion request.
    /// It can be either a chat completion chunk or a chat completion chunk choice.
    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    pub struct ChatCompletionChunk {
        /// A unique identifier for the chat completion chunk.
        #[schema(example = "chatcmpl-123")]
        pub id: String,

        /// The object of the chat completion chunk (which is always `chat.completion.chunk`)
        #[schema(example = "chat.completion.chunk")]
        pub object: String,

        /// The Unix timestamp (in seconds) of when the chunk was created.
        #[schema(example = 1_677_652_288)]
        pub created: i64,

        /// The model used for the chat completion.
        #[schema(example = "meta-llama/Llama-3.3-70B-Instruct")]
        pub model: String,

        /// A list of chat completion chunk choices.
        #[schema(
            example = "[{\"index\": 0, \"delta\": {\"role\": \"assistant\", \"content\": \"Hello! How can you help me today?\"}, \"logprobs\": null, \"finish_reason\": null, \"stop_reason\": null}]"
        )]
        pub choices: Vec<completion_choice::ChatCompletionChunkChoice>,

        /// Usage statistics for the completion request.
        #[schema(
            example = "{\"prompt_tokens\": 100, \"completion_tokens\": 200, \"total_tokens\": 300}"
        )]
        #[serde(skip_serializing_if = "Option::is_none")]
        pub usage: Option<usage::CompletionUsage>,
    }

    pub mod completion_choice {
        use super::{logprobs, message, stop_reason, tools, Deserialize, Serialize, ToSchema};

        /// Represents the chat completion choice.
        ///
        /// This is used to represent the chat completion choice in the chat completion request.
        /// It can be either a chat completion message or a chat completion chunk.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct ChatCompletionChoice {
            /// The index of this choice in the list of choices.
            #[schema(example = 0)]
            pub index: i32,

            /// The chat completion message.
            #[schema(example = json!({
                "role": "assistant",
                "content": "Hello! How can I help you today?"
            }))]
            pub message: message::ChatCompletionMessage,

            /// The reason the chat completion was finished.
            #[schema(example = "stop")]
            pub finish_reason: Option<String>,

            /// Log probability information for the choice, if applicable.
            #[serde(skip_serializing_if = "Option::is_none")]
            #[schema(example = json!({
                "logprobs": {
                    "tokens": ["Hello", "!", "How", "can", "I", "help", "you", "today?"],
                    "token_logprobs": [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]
                }
            }))]
            pub logprobs: Option<logprobs::ChatCompletionLogProbs>,
        }

        /// Represents the chat completion chunk choice.
        ///
        /// This is used to represent the chat completion chunk choice in the chat completion request.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct ChatCompletionChunkChoice {
            /// The index of this choice in the list of choices.
            #[schema(example = 0)]
            pub index: i32,

            /// The chat completion delta message for streaming.
            pub delta: ChatCompletionChunkDelta,

            /// Log probability information for the choice, if applicable.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub logprobs: Option<logprobs::ChatCompletionLogProbs>,

            /// The reason the chat completion was finished, if applicable.
            #[schema(example = "stop")]
            #[serde(skip_serializing_if = "Option::is_none")]
            pub finish_reason: Option<String>,

            /// The reason the chat completion was stopped, if applicable.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub stop_reason: Option<stop_reason::StopReason>,
        }

        /// Represents the chat completion chunk delta.
        ///
        /// This is used to represent the chat completion chunk delta in the chat completion request.
        /// It can be either a chat completion chunk delta message or a chat completion chunk delta choice.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct ChatCompletionChunkDelta {
            /// The role of the message author, if present in this chunk.
            #[schema(example = "assistant")]
            #[serde(skip_serializing_if = "Option::is_none")]
            pub role: Option<String>,

            /// The content of the message, if present in this chunk.
            #[schema(example = "Hello")]
            #[serde(skip_serializing_if = "Option::is_none")]
            pub content: Option<String>,

            /// The reasoning content, if present in this chunk.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub reasoning_content: Option<String>,

            /// The tool calls information, if present in this chunk.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub tool_calls: Option<Vec<tools::ChatCompletionChunkDeltaToolCall>>,
        }
    }

    pub mod logprobs {
        use super::{Deserialize, Serialize, ToSchema};

        /// Represents the chat completion log probs.
        ///
        /// This is used to represent the chat completion log probs in the chat completion request.
        /// It can be either a chat completion log probs or a chat completion log probs choice.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct ChatCompletionLogProbs {
            /// The log probs of the chat completion.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub content: Option<Vec<ChatCompletionLogProbsContent>>,
        }

        /// Represents the chat completion log probs content.
        ///
        /// This is used to represent the chat completion log probs content in the chat completion request.
        /// It can be either a chat completion log probs content or a chat completion log probs content choice.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct ChatCompletionLogProbsContent {
            top_logprobs: Vec<ChatCompletionLogProb>,
        }

        /// Represents the chat completion log prob.
        ///
        /// This is used to represent the chat completion log prob in the chat completion request.
        /// It can be either a chat completion log prob or a chat completion log prob choice.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct ChatCompletionLogProb {
            /// The log prob of the chat completion.
            pub logprob: f32,

            /// The token of the chat completion.
            pub token: String,

            /// A list of integers representing the UTF-8 bytes representation of the token.
            /// Useful in instances where characters are represented by multiple tokens and their byte
            /// representations must be combined to generate the correct text representation.
            /// Can be null if there is no bytes representation for the token.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub bytes: Option<Vec<i32>>,
        }
    }

    pub mod message {
        use super::{message_content, tools, Deserialize, Serialize, ToSchema};

        /// The role of the message author
        #[derive(Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
        #[serde(rename_all = "snake_case")]
        #[schema(title = "Role")]
        #[schema(example = "system")]
        pub enum Role {
            /// System role for setting behavior
            System,
            /// Assistant role for AI responses
            Assistant,
            /// User role for human messages
            User,
            /// Tool role for function calls
            Tool,
        }

        /// A message that is part of a conversation which is based on the role
        /// of the author of the message.
        ///
        /// This is used to represent the message in the chat completion request.
        /// It can be either a system message, a user message, an assistant message, or a tool message.
        #[derive(Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
        #[serde(tag = "role", rename_all = "snake_case")]
        pub enum ChatCompletionMessage {
            /// The role of the messages author, in this case system.
            #[schema(title = "System")]
            #[serde(rename = "system")]
            System {
                /// The contents of the message.
                #[serde(default, skip_serializing_if = "Option::is_none")]
                #[schema(example = "You are a helpful AI assistant")]
                content: Option<message_content::MessageContent>,
                /// An optional name for the participant. Provides the model information to differentiate between participants of the same role.
                #[serde(default, skip_serializing_if = "Option::is_none")]
                #[schema(example = "AI expert")]
                name: Option<String>,
            },
            /// The role of the messages author, in this case user.
            #[schema(title = "User")]
            #[serde(rename = "user")]
            User {
                /// The contents of the message.
                #[serde(default, skip_serializing_if = "Option::is_none")]
                #[schema(example = "Hello! How can I help you today?")]
                content: Option<message_content::MessageContent>,
                /// An optional name for the participant. Provides the model information to differentiate between participants of the same role.
                #[serde(default, skip_serializing_if = "Option::is_none")]
                #[schema(example = "John Doe")]
                name: Option<String>,
            },
            /// The role of the messages author, in this case assistant.
            #[schema(title = "Assistant")]
            #[serde(rename = "assistant")]
            Assistant {
                /// The contents of the message.
                #[serde(default, skip_serializing_if = "Option::is_none")]
                #[schema(example = "Hello! How can I help you today?")]
                content: Option<message_content::MessageContent>,
                /// An optional name for the participant. Provides the model information to differentiate between participants of the same role.
                #[serde(default, skip_serializing_if = "Option::is_none")]
                #[schema(example = "AI")]
                name: Option<String>,
                /// The refusal message by the assistant.
                #[serde(default, skip_serializing_if = "Option::is_none")]
                refusal: Option<String>,
                /// The tool calls generated by the model, such as function calls.
                #[serde(default, skip_serializing_if = "Vec::is_empty")]
                tool_calls: Vec<tools::ToolCall>,
            },
            /// The role of the messages author, in this case tool.
            #[schema(title = "Tool")]
            #[serde(rename = "tool")]
            Tool {
                /// The contents of the message.
                #[serde(default, skip_serializing_if = "Option::is_none")]
                content: Option<message_content::MessageContent>,
                /// Tool call that this message is responding to.
                #[serde(default, skip_serializing_if = "String::is_empty")]
                tool_call_id: String,
            },
        }
    }

    pub mod message_content {
        use serde_json::Value;

        use super::{Deserialize, Deserializer, Serialize, ToSchema};

        /// Represents the content of a message.
        ///
        /// This is used to represent the content of a message in the chat completion request.
        /// It can be either a text or an array of content parts.
        #[derive(Debug, PartialEq, Eq, Serialize, ToSchema)]
        #[serde(untagged)]
        pub enum MessageContent {
            /// The text contents of the message.
            #[serde(rename(serialize = "text", deserialize = "text"))]
            Text(String),
            /// An array of content parts with a defined type, each can be of type text or image_url when passing in images.
            /// You can pass multiple images by adding multiple image_url content parts. Image input is only supported when using the gpt-4o model.
            #[serde(rename(serialize = "array", deserialize = "array"))]
            Array(Vec<MessageContentPart>),
        }

        /// Represents a part of a message content.
        ///
        /// This is used to represent the content of a message in the chat completion request.
        /// It can be either a text or an image.
        #[derive(Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
        #[serde(untagged)]
        pub enum MessageContentPart {
            #[serde(rename(serialize = "text", deserialize = "text"))]
            Text {
                /// The type of the content part.
                #[serde(rename(serialize = "type", deserialize = "type"))]
                r#type: String,
                /// The text content.
                text: String,
            },
            #[serde(rename(serialize = "image", deserialize = "image"))]
            Image {
                /// The type of the content part.
                #[serde(rename(serialize = "type", deserialize = "type"))]
                r#type: String,
                /// The image URL.
                image_url: MessageContentPartImageUrl,
            },
        }

        impl std::fmt::Display for MessageContent {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Self::Text(text) => write!(f, "{text}"),
                    Self::Array(parts) => {
                        let mut content = String::new();
                        for part in parts {
                            content.push_str(&format!("{part}\n"));
                        }
                        write!(f, "{content}")
                    }
                }
            }
        }

        // We manually implement Deserialize here for more control.
        impl<'de> Deserialize<'de> for MessageContent {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value: Value = Value::deserialize(deserializer)?;

                if let Some(s) = value.as_str() {
                    return Ok(Self::Text(s.to_string()));
                }

                if let Some(arr) = value.as_array() {
                    let parts: std::result::Result<Vec<MessageContentPart>, _> = arr
                        .iter()
                        .map(|v| {
                            serde_json::from_value(v.clone()).map_err(serde::de::Error::custom)
                        })
                        .collect();
                    return Ok(Self::Array(parts?));
                }

                Err(serde::de::Error::custom(
                    "Expected a string or an array of content parts",
                ))
            }
        }

        impl std::fmt::Display for MessageContentPart {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match self {
                    Self::Text { r#type, text } => {
                        write!(f, "{type}: {text}")
                    }
                    Self::Image { r#type, image_url } => {
                        write!(f, "{type}: [Image URL: {image_url}]")
                    }
                }
            }
        }

        /// Represents the image URL of a message content part.
        ///
        /// This is used to represent the image URL of a message content part in the chat completion request.
        /// It can be either a URL or a base64 encoded image data.
        #[derive(Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
        #[serde(rename(serialize = "image_url", deserialize = "image_url"))]
        pub struct MessageContentPartImageUrl {
            /// Either a URL of the image or the base64 encoded image data.
            url: String,
            /// Specifies the detail level of the image.
            detail: Option<String>,
        }

        /// Implementing Display for MessageContentPartImageUrl
        impl std::fmt::Display for MessageContentPartImageUrl {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match &self.detail {
                    Some(detail) => write!(f, "Image URL: {}, Detail: {}", self.url, detail),
                    None => write!(f, "Image URL: {}", self.url),
                }
            }
        }
    }

    pub mod response_format {
        use super::{Deserialize, Serialize, ToSchema};

        /// The format to return the response in.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        #[serde(rename_all = "snake_case")]
        pub enum ResponseFormatType {
            Text,
            JsonObject,
            JsonSchema,
        }

        /// The format to return the response in.
        ///
        /// This is used to represent the format to return the response in in the chat completion request.
        /// It can be either text, json_object, or json_schema.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct JsonSchemaResponseFormat {
            /// The name of the response format.
            pub name: String,

            /// The description of the response format.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub description: Option<String>,

            /// The JSON schema of the response format.
            #[serde(skip_serializing_if = "Option::is_none")]
            #[serde(rename = "schema")]
            pub json_schema: Option<serde_json::Value>,

            /// Whether to strictly validate the JSON schema.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub strict: Option<bool>,
        }

        /// The format to return the response in.
        ///
        /// This is used to represent the format to return the response in in the chat completion request.
        /// It can be either text, json_object, or json_schema.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct ResponseFormat {
            /// The type of the response format.
            #[serde(rename = "type")]
            pub format_type: ResponseFormatType,

            /// The JSON schema of the response format.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub json_schema: Option<JsonSchemaResponseFormat>,
        }
    }

    pub mod stream_options {
        use super::{Deserialize, Serialize, ToSchema};

        /// Specifies the stream options for the request.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct StreamOptions {
            /// If set, an additional chunk will be streamed before the data: [DONE] message.
            /// The usage field on this chunk shows the token usage statistics for the entire request, and the choices field
            /// will always be an empty array. All other chunks will also include a usage field, but with a null value.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub include_usage: Option<bool>,
        }
    }

    pub mod stop_reason {
        use super::{Deserialize, Serialize, ToSchema};
        use serde::Deserializer;
        use serde_json::Value;

        /// Represents the stop reason.
        ///
        /// This is used to represent the stop reason in the chat completion request.
        /// It can be either a stop reason or a stop reason choice.
        #[derive(Debug, ToSchema)]
        pub enum StopReason {
            Int(u32),
            String(String),
        }

        // Add custom implementations for serialization/deserialization if needed
        impl<'de> Deserialize<'de> for StopReason {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = Value::deserialize(deserializer)?;
                value.as_u64().map_or_else(
                    || {
                        value.as_str().map_or_else(
                            || Err(serde::de::Error::custom("Expected string or integer")),
                            |s| Ok(Self::String(s.to_string())),
                        )
                    },
                    |n| {
                        Ok(Self::Int(u32::try_from(n).map_err(|_| {
                            serde::de::Error::custom("Expected integer")
                        })?))
                    },
                )
            }
        }

        impl Serialize for StopReason {
            fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                match self {
                    Self::Int(n) => serializer.serialize_u32(*n),
                    Self::String(s) => serializer.serialize_str(s),
                }
            }
        }
    }

    pub mod token_details {
        use super::{Deserialize, Serialize, ToSchema};

        /// Represents the prompt tokens details.
        ///
        /// This is used to represent the prompt tokens details in the chat completion request.
        /// It can be either a prompt tokens details or a prompt tokens details choice.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct PromptTokensDetails {
            /// Number of tokens in the prompt that were cached.
            #[schema(example = 1)]
            pub cached_tokens: i32,
        }
    }

    pub mod tools {
        use serde_json::Value;
        use std::collections::HashMap;

        use super::{Deserialize, Serialize, ToSchema};

        /// Represents the function that the model called.
        ///
        /// This is used to represent the function that the model called in the chat completion request.
        /// It can be either a function or a tool call.
        #[derive(Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
        pub struct ToolCallFunction {
            /// The name of the function to call.
            name: String,
            /// The arguments to call the function with, as generated by the model in JSON format.
            /// Note that the model does not always generate valid JSON, and may hallucinate parameters not defined by your function schema.
            /// Validate the arguments in your code before calling your function.
            arguments: String,
        }

        /// Represents the tool call that the model made.
        ///
        /// This is used to represent the tool call that the model made in the chat completion request.
        /// It can be either a function or a tool.
        #[derive(Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
        #[serde(rename(serialize = "tool_call", deserialize = "tool_call"))]
        pub struct ToolCall {
            /// The ID of the tool call.
            id: String,
            /// The type of the tool. Currently, only function is supported.
            #[serde(rename(serialize = "type", deserialize = "type"))]
            r#type: String,
            /// The function that the model called.
            function: ToolCallFunction,
        }

        /// Represents the tool that the model called.
        ///
        /// This is used to represent the tool that the model called in the chat completion request.
        /// It can be either a function or a tool.
        #[derive(Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
        #[serde(rename(serialize = "tool", deserialize = "tool"))]
        pub struct Tool {
            /// The type of the tool. Currently, only function is supported.
            #[serde(rename(serialize = "type", deserialize = "type"))]
            r#type: String,
            /// The function that the model called.
            function: ToolFunction,
        }

        /// Represents the function that the model called.
        ///
        /// This is used to represent the function that the model called in the chat completion request.
        /// It can be either a function or a tool.
        #[derive(Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
        pub struct ToolFunction {
            /// Description of the function to call.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            description: Option<String>,
            /// The name of the function to call.
            name: String,
            /// The arguments to call the function with, as generated by the model in JSON format.
            #[serde(default, skip_serializing_if = "Option::is_none")]
            parameters: Option<Value>,
            /// Whether to enable strict schema adherence when generating the function call. If set to true, the
            /// model will follow the exact schema defined in the parameters field. Only a subset of JSON Schema is supported when strict is true
            #[serde(default, skip_serializing_if = "Option::is_none")]
            strict: Option<bool>,
        }

        /// Represents the chat completion chunk delta tool call.
        ///
        /// This is used to represent the chat completion chunk delta tool call in the chat completion request.
        /// It can be either a chat completion chunk delta tool call or a chat completion chunk delta tool call function.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct ChatCompletionChunkDeltaToolCall {
            /// The ID of the tool call.
            pub id: String,

            /// The type of the tool call.
            pub r#type: String,

            /// The index of the tool call.
            pub index: i32,

            /// The function of the tool call.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub function: Option<ChatCompletionChunkDeltaToolCallFunction>,
        }

        /// Represents the chat completion chunk delta tool call function.
        ///
        /// This is used to represent the chat completion chunk delta tool call function in the chat completion request.
        /// It can be either a chat completion chunk delta tool call function or a chat completion chunk delta tool call function arguments.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct ChatCompletionChunkDeltaToolCallFunction {
            /// The name of the tool call function.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub name: Option<String>,

            /// The arguments of the tool call function.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub arguments: Option<String>,
        }

        /// A tool that can be used in a chat completion.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct ChatCompletionToolsParam {
            /// The type of the tool.
            #[serde(rename = "type")]
            pub tool_type: String,

            /// The function that the tool will call.
            pub function: ChatCompletionToolFunctionParam,
        }

        /// A function that can be used in a chat completion.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct ChatCompletionToolFunctionParam {
            /// The name of the function.
            pub name: String,

            /// The description of the function.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub description: Option<String>,

            /// The parameters of the function.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub parameters: Option<HashMap<String, serde_json::Value>>,

            /// Whether to strictly validate the parameters of the function.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub strict: Option<bool>,
        }

        /// A tool choice that can be used in a chat completion.
        ///
        /// This is used to represent the tool choice in the chat completion request.
        /// It can be either a literal tool choice or a named tool choice.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        #[serde(untagged)]
        pub enum ToolChoice {
            Literal(ToolChoiceLiteral),
            Named(ChatCompletionNamedToolChoiceParam),
        }

        /// A literal tool choice that can be used in a chat completion.
        ///
        /// This is used to represent the literal tool choice in the chat completion request.
        /// It can be either none or auto.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        #[serde(rename_all = "lowercase")]
        pub enum ToolChoiceLiteral {
            None,
            Auto,
        }

        /// A named tool choice that can be used in a chat completion.
        ///
        /// This is used to represent the named tool choice in the chat completion request.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct ChatCompletionNamedToolChoiceParam {
            /// The type of the tool choice.
            #[serde(rename = "type")]
            pub type_field: String,

            /// The function of the tool choice.
            pub function: ChatCompletionNamedFunction,
        }

        /// A named function that can be used in a chat completion.
        ///
        /// This is used to represent the named function in the chat completion request.
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct ChatCompletionNamedFunction {
            /// The name of the function.
            pub name: String,
        }
    }

    pub mod usage {
        use super::{token_details, Deserialize, Serialize, ToSchema};

        /// Represents the completion usage.
        ///
        /// This is used to represent the completion usage in the chat completion request.
        /// It can be either a completion usage or a completion chunk usage.
        #[allow(clippy::struct_field_names)]
        #[derive(Debug, Serialize, Deserialize, ToSchema)]
        pub struct CompletionUsage {
            /// Number of tokens in the prompt.
            #[schema(example = 9)]
            pub prompt_tokens: i32,

            /// Number of tokens in the completion.
            #[schema(example = 12)]
            pub completion_tokens: i32,

            /// Total number of tokens used (prompt + completion).
            #[schema(example = 21)]
            pub total_tokens: i32,

            /// Details about the prompt tokens.
            #[serde(skip_serializing_if = "Option::is_none")]
            pub prompt_tokens_details: Option<token_details::PromptTokensDetails>,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;
    use std::str::FromStr;
    use tokenizers::Tokenizer;

    const MODEL: &str = "TinyLlama/TinyLlama-1.1B-Chat-v1.0";

    async fn load_tokenizer() -> Tokenizer {
        let url =
            "https://huggingface.co/TinyLlama/TinyLlama-1.1B-Chat-v1.0/raw/main/tokenizer.json";
        let tokenizer_json = reqwest::get(url).await.unwrap().text().await.unwrap();

        Tokenizer::from_str(&tokenizer_json).unwrap()
    }

    #[tokio::test]
    async fn test_get_compute_units_estimate() {
        let request = RequestModelChatCompletions {
            model: MODEL.to_string(),
            messages: vec![json!({
                "role": "user",
                "content": "Hello from the other side of Mars"
            })],
            max_completion_tokens: 10,
        };
        let tokenizer = load_tokenizer().await;
        let result = request.get_compute_units_estimate(Some(&tokenizer));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().max_total_compute_units, 21); // 8 tokens + 3 overhead + 10 completion
    }

    #[tokio::test]
    async fn test_get_compute_units_estimate_multiple_messages() {
        let request = RequestModelChatCompletions {
            model: MODEL.to_string(),
            messages: vec![
                json!({
                    "role": "user",
                    "content": "Hello from the other side of Mars"
                }),
                json!({
                    "role": "assistant",
                    "content": "Hello from the other side of Mars"
                }),
            ],
            max_completion_tokens: 10,
        };
        let tokenizer = load_tokenizer().await;
        let result = request.get_compute_units_estimate(Some(&tokenizer));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().max_total_compute_units, 32); // (8+8) tokens + (3+3) overhead + 10 completion
    }

    #[tokio::test]
    async fn test_get_compute_units_estimate_array_content() {
        let request = RequestModelChatCompletions {
            model: MODEL.to_string(),
            messages: vec![json!({
                "role": "user",
                "content": [
                    {
                        "type": "text",
                        "text": "Hello from the other side of Mars"
                    },
                    {
                        "type": "text",
                        "text": "Hello from the other side of Mars"
                    }
                ]
            })],
            max_completion_tokens: 10,
        };

        let tokenizer = load_tokenizer().await;
        let result = request.get_compute_units_estimate(Some(&tokenizer));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().max_total_compute_units, 32); // (8+8) tokens  (3 + 3) overhead + 10 completion
    }

    #[tokio::test]
    async fn test_get_compute_units_estimate_empty_message() {
        let request = RequestModelChatCompletions {
            model: MODEL.to_string(),
            messages: vec![json!({
                "role": "user",
                "content": ""
            })],
            max_completion_tokens: 10,
        };
        let tokenizer = load_tokenizer().await;
        let result = request.get_compute_units_estimate(Some(&tokenizer));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().max_total_compute_units, 14); // 1 tokens (special token) + 3 overhead + 10 completion
    }

    #[tokio::test]
    async fn test_get_compute_units_estimate_mixed_content() {
        let request = RequestModelChatCompletions {
            model: MODEL.to_string(),
            messages: vec![
                json!({
                    "role": "system",
                    "content": "Hello from the other side of Mars"
                }),
                json!({
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "Hello from the other side of Mars"
                        },
                        {
                            "type": "image",
                            "image_url": {
                                "url": "http://example.com/image.jpg"
                            }
                        },
                        {
                            "type": "text",
                            "text": "Hello from the other side of Mars"
                        }
                    ]
                }),
            ],
            max_completion_tokens: 15,
        };
        let tokenizer = load_tokenizer().await;
        let result = request.get_compute_units_estimate(Some(&tokenizer));
        assert!(result.is_ok());
        // System message: tokens + 15 completion
        // User message array: (2 text parts tokens) + (15 * 2 for text completion for parts)
        let tokens = result.unwrap();
        assert_eq!(tokens.max_total_compute_units, 48); // 3 * 8 + 3 * 3 overhead + 15
    }

    #[tokio::test]
    async fn test_get_compute_units_estimate_invalid_content() {
        let request = RequestModelChatCompletions {
            model: MODEL.to_string(),
            messages: vec![json!({
                "role": "user",
                // Missing "content" field
            })],
            max_completion_tokens: 10,
        };
        let tokenizer = load_tokenizer().await;
        let result = request.get_compute_units_estimate(Some(&tokenizer));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AtomaProxyError::RequestError { .. }
        ));
    }

    #[tokio::test]
    async fn test_get_compute_units_estimate_empty_array_content() {
        let request = RequestModelChatCompletions {
            model: MODEL.to_string(),
            messages: vec![json!({
                "role": "user",
                "content": []
            })],
            max_completion_tokens: 10,
        };
        let tokenizer = load_tokenizer().await;
        let result = request.get_compute_units_estimate(Some(&tokenizer));
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AtomaProxyError::RequestError { .. }
        ));
    }

    #[tokio::test]
    async fn test_get_compute_units_estimate_special_characters() {
        let request = RequestModelChatCompletions {
            model: MODEL.to_string(),
            messages: vec![json!({
                "role": "user",
                "content": "Hello! 👋 🌍 \n\t Special chars: &*#@"
            })],
            max_completion_tokens: 10,
        };
        let tokenizer = load_tokenizer().await;
        let result = request.get_compute_units_estimate(Some(&tokenizer));
        assert!(result.is_ok());
        let tokens = result.unwrap();
        assert!(tokens.max_total_compute_units > 13); // Should be more than minimum (3 overhead + 10 completion)
    }
}
