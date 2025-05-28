use std::time::{Duration, Instant};

use crate::server::handlers::{update_state_manager_fiat, STREAM_KEEP_ALIVE_INTERVAL_IN_SECONDS};
use crate::server::streamer::ClientStreamer;
use crate::server::types::{
    ConfidentialComputeRequest, ConfidentialComputeResponse, ConfidentialComputeStreamResponse,
};
use crate::server::{
    error::AtomaProxyError, http_server::ProxyState, middleware::RequestMetadataExtension,
    streamer::Streamer,
};
use atoma_state::types::AtomaAtomaStateManagerEvent;
use atoma_utils::constants::REQUEST_ID;
use axum::body::Body;
use axum::http::HeaderValue;
use axum::response::{IntoResponse, Response, Sse};
use axum::Extension;
use axum::{extract::State, http::HeaderMap, Json};
use futures::StreamExt;
use openai_api_completions::{
    CompletionTokensDetails, CompletionsPrompt, CompletionsRequest, CompletionsResponse,
    CompletionsStreamResponse, CreateCompletionsStreamRequest, LogProbs, PromptTokensDetails,
    Usage,
};
use opentelemetry::KeyValue;
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::Value;
use sqlx::types::chrono::{DateTime, Utc};
use tracing::instrument;
use utoipa::OpenApi;

use super::metrics::{
    CHAT_COMPLETIONS_COMPLETIONS_TOKENS, CHAT_COMPLETIONS_COMPLETIONS_TOKENS_PER_USER,
    CHAT_COMPLETIONS_CONFIDENTIAL_NUM_REQUESTS, CHAT_COMPLETIONS_INPUT_TOKENS,
    CHAT_COMPLETIONS_INPUT_TOKENS_PER_USER, CHAT_COMPLETIONS_LATENCY_METRICS,
    CHAT_COMPLETIONS_NUM_REQUESTS, CHAT_COMPLETIONS_TOTAL_TOKENS,
    CHAT_COMPLETIONS_TOTAL_TOKENS_PER_USER, CHAT_COMPLETION_REQUESTS_PER_USER,
    INTENTIONALLY_CANCELLED_CHAT_COMPLETION_STREAMING_REQUESTS, TOTAL_BAD_REQUESTS,
    TOTAL_COMPLETED_REQUESTS, TOTAL_FAILED_CHAT_CONFIDENTIAL_REQUESTS, TOTAL_FAILED_CHAT_REQUESTS,
    TOTAL_FAILED_REQUESTS, TOTAL_LOCKED_REQUESTS, TOTAL_TOO_EARLY_REQUESTS,
    TOTAL_TOO_MANY_REQUESTS, TOTAL_UNAUTHORIZED_REQUESTS,
    UNSUCCESSFUL_CHAT_COMPLETION_REQUESTS_PER_USER,
};
use super::request_model::{ComputeUnitsEstimate, RequestModel};
use super::{
    handle_status_code_error, update_state_manager, verify_response_hash_and_signature,
    COMPLETION_TOKENS, PROMPT_TOKENS, STOP_STREAMER_PATH, STREAM, TOTAL_TOKENS, USAGE, USER_ID,
};
use crate::server::{Result, DEFAULT_MAX_TOKENS, MAX_COMPLETION_TOKENS, MAX_TOKENS, MODEL};

/// Path for the completions endpoint.
///
/// This endpoint follows the OpenAI API format for legacy completions
/// and is used to process chat-based requests for AI model inference.
pub const COMPLETIONS_PATH: &str = "/v1/completions";

/// Path for the confidential completions endpoint.
pub const CONFIDENTIAL_COMPLETIONS_PATH: &str = "/v1/confidential/completions";

/// The key for the prompt in the request.
const PROMPT: &str = "prompt";

/// The model key
const MODEL_KEY: &str = "model";

/// The user id key
const USER_ID_KEY: &str = "user_id";

/// The OpenAPI schema for the completions endpoint.
#[derive(OpenApi)]
#[openapi(
    paths(completions_create, completions_create_stream),
    components(schemas(
        CompletionsRequest,
        CompletionsResponse,
        LogProbs,
        Usage,
        CompletionTokensDetails,
        PromptTokensDetails,
    ))
)]
pub struct CompletionsOpenApi;

/// Create completions
///
/// This function processes completion requests by using the chat completions endpoint.
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
    request_body = CompletionsRequest,
    responses(
        (status = OK, description = "Chat completions", content(
            (CompletionsResponse = "application/json"),
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
        path = COMPLETIONS_PATH,
    )
)]
pub async fn completions_create(
    Extension(metadata): Extension<RequestMetadataExtension>,
    State(state): State<ProxyState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<Response<Body>> {
    let endpoint = metadata.endpoint.clone();
    tokio::spawn(async move {
        let is_streaming = payload
            .get(STREAM)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or_default();

        match handle_completions_request(&state, &metadata, headers, payload, is_streaming).await {
            Ok(response) => {
                if !is_streaming {
                    // The streaming metric is recorded in the streamer (final chunk)
                    TOTAL_COMPLETED_REQUESTS.add(1, &[KeyValue::new("model", metadata.model_name)]);
                }
                Ok(response)
            }
            Err(e) => {
                let model = metadata.model_name.clone();
                match e.status_code() {
                    StatusCode::TOO_MANY_REQUESTS => {
                        TOTAL_TOO_MANY_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model.clone())]);
                    }
                    StatusCode::BAD_REQUEST => {
                        TOTAL_BAD_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model.to_owned())]);
                    }
                    StatusCode::LOCKED => {
                        TOTAL_LOCKED_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model.to_owned())]);
                    }
                    StatusCode::TOO_EARLY => {
                        TOTAL_TOO_EARLY_REQUESTS
                            .add(1, &[KeyValue::new(MODEL_KEY, model.to_owned())]);
                    }
                    StatusCode::UNAUTHORIZED => {
                        TOTAL_UNAUTHORIZED_REQUESTS
                            .add(1, &[KeyValue::new(MODEL_KEY, model.to_owned())]);
                    }
                    _ => {
                        TOTAL_FAILED_CHAT_REQUESTS
                            .add(1, &[KeyValue::new(MODEL_KEY, model.to_owned())]);
                        TOTAL_FAILED_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model.to_owned())]);

                        UNSUCCESSFUL_CHAT_COMPLETION_REQUESTS_PER_USER
                            .add(1, &[KeyValue::new(USER_ID_KEY, metadata.user_id)]);
                    }
                }

                if let Some(stack_small_id) = metadata.selected_stack_small_id {
                    update_state_manager(
                        &state.state_manager_sender,
                        stack_small_id,
                        (metadata.num_input_tokens.unwrap_or_default() + metadata.max_output_tokens)
                            as i64,
                        0,
                        &metadata.endpoint,
                    )?;
                } else {
                    update_state_manager_fiat(
                        &state.state_manager_sender,
                        metadata.user_id,
                        metadata.num_input_tokens.unwrap_or_default() as i64,
                        0,
                        metadata.max_output_tokens as i64,
                        0,
                        metadata.price_per_million,
                        metadata.model_name,
                        &metadata.endpoint,
                    )?;
                }
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

/// Handles the completions request.
///
/// This function processes completion requests by using the chat completions endpoint.
///
/// # Arguments
///
/// * `state` - Reference to the application's shared state containing service configuration
/// * `metadata` - Request metadata containing:
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
/// * Unexpected response content
/// * Internal server errors
/// * Invalid request parameters
/// * Unsupported model or endpoint
#[instrument(
    level = "info",
    skip_all,
    fields(
        path = metadata.endpoint,
    )
)]
async fn handle_completions_request(
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
            metadata.user_id,
            headers,
            &payload,
            metadata.num_input_tokens.unwrap_or_default() as i64,
            metadata.max_output_tokens as i64,
            metadata.price_per_million,
            metadata.selected_stack_small_id,
            metadata.endpoint.clone(),
            metadata.model_name.clone(),
        )
        .await
    } else {
        handle_non_streaming_response(
            state,
            &metadata.node_address,
            metadata.user_id,
            headers,
            &payload,
            metadata.num_input_tokens.unwrap_or_default() as i64,
            metadata.max_output_tokens as i64,
            metadata.price_per_million,
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
    request_body = CreateCompletionsStreamRequest,
    responses(
        (status = OK, description = "Completions", content(
            (CompletionsStreamResponse = "text/event-stream")
        )),
        (status = BAD_REQUEST, description = "Bad request"),
        (status = UNAUTHORIZED, description = "Unauthorized"),
        (status = INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
#[allow(dead_code)]
pub fn completions_create_stream(
    Extension(_metadata): Extension<RequestMetadataExtension>,
    State(_state): State<ProxyState>,
    _headers: HeaderMap,
    Json(_payload): Json<CompletionsStreamResponse>,
) -> Result<Response<Body>> {
    // This endpoint exists only for OpenAPI documentation
    // Actual streaming is handled by chat_completions_create
    Err(AtomaProxyError::NotImplemented {
        message: "This is a mock endpoint for OpenAPI documentation".to_string(),
        endpoint: COMPLETIONS_PATH.to_string(),
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
        confidential_completions_create,
        confidential_completions_create_stream
    ),
    components(schemas(ConfidentialComputeRequest))
)]
pub struct ConfidentialCompletionsOpenApi;

/// Create confidential completions
///
/// This handler processes completions requests in a confidential manner, providing additional
/// encryption and security measures for sensitive data processing. It supports both streaming and
/// non-streaming responses while maintaining data confidentiality through AEAD encryption and TEE hardware,
/// for full private AI compute.
///
/// ## Returns
///
/// Returns a `Result` containing either:
/// * An HTTP response with the completions result
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
pub async fn confidential_completions_create(
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

        match handle_completions_request(&state, &metadata, headers, payload, is_streaming).await {
            Ok(response) => {
                if !is_streaming {
                    // The streaming metric is recorded in the streamer (final chunk)
                    CHAT_COMPLETIONS_CONFIDENTIAL_NUM_REQUESTS
                        .add(1, &[KeyValue::new(MODEL_KEY, metadata.model_name)]);
                }
                Ok(response)
            }
            Err(e) => {
                let model = metadata.model_name.clone();
                match e.status_code() {
                    StatusCode::TOO_MANY_REQUESTS => {
                        TOTAL_TOO_MANY_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model.clone())]);
                    }
                    StatusCode::BAD_REQUEST => {
                        TOTAL_BAD_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model.to_owned())]);
                    }
                    StatusCode::LOCKED => {
                        TOTAL_LOCKED_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model.to_owned())]);
                    }
                    StatusCode::TOO_EARLY => {
                        TOTAL_TOO_EARLY_REQUESTS
                            .add(1, &[KeyValue::new(MODEL_KEY, model.to_owned())]);
                    }
                    StatusCode::UNAUTHORIZED => {
                        TOTAL_UNAUTHORIZED_REQUESTS
                            .add(1, &[KeyValue::new(MODEL_KEY, model.to_owned())]);
                    }
                    _ => {
                        TOTAL_FAILED_CHAT_CONFIDENTIAL_REQUESTS
                            .add(1, &[KeyValue::new(MODEL_KEY, model.to_owned())]);
                        TOTAL_FAILED_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model.to_owned())]);

                        UNSUCCESSFUL_CHAT_COMPLETION_REQUESTS_PER_USER
                            .add(1, &[KeyValue::new(USER_ID_KEY, metadata.user_id)]);
                    }
                }
                if let Some(stack_small_id) = metadata.selected_stack_small_id {
                    update_state_manager(
                        &state.state_manager_sender,
                        stack_small_id,
                        (metadata.num_input_tokens.unwrap_or_default() + metadata.max_output_tokens)
                            as i64,
                        0,
                        &metadata.endpoint,
                    )?;
                } else {
                    update_state_manager_fiat(
                        &state.state_manager_sender,
                        metadata.user_id,
                        metadata.num_input_tokens.unwrap_or_default() as i64,
                        0,
                        metadata.max_output_tokens as i64,
                        0,
                        metadata.price_per_million,
                        metadata.model_name,
                        &metadata.endpoint,
                    )?;
                }
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
pub fn confidential_completions_create_stream(
    Extension(_metadata): Extension<RequestMetadataExtension>,
    State(_state): State<ProxyState>,
    _headers: HeaderMap,
    Json(_payload): Json<ConfidentialComputeStreamResponse>,
) -> Result<Response<Body>> {
    // This endpoint exists only for OpenAPI documentation
    // Actual streaming is handled by chat_completions_create
    Err(AtomaProxyError::NotImplemented {
        message: "This is a mock endpoint for OpenAPI documentation".to_string(),
        endpoint: COMPLETIONS_PATH.to_string(),
    })
}

/// Handles the non-streaming response for completions.
///
/// This function processes non-streaming completion requests by using the chat completions endpoint.
///
/// # Arguments
///
/// * `state` - Reference to the application's shared state containing service configuration
/// * `node_address` - The address of the node to forward the request to
/// * `user_id` - The ID of the user making the request
/// * `headers` - HTTP request headers to forward to the inference service
/// * `payload` - The JSON payload containing the chat completion request
/// * `num_input_tokens` - The number of input tokens
/// * `estimated_output_tokens` - The estimated total number of tokens for the completion
/// * `fiat_estimated_amount` - The estimated amount in fiat currency for the completion
/// * `selected_stack_small_id` - The ID of the stack small to update
/// * `endpoint` - The endpoint to forward the request to
/// * `model_name` - The name of the model to use
///
/// # Returns
///
/// Returns a `Result` containing either:
/// * A JSON response for non-streaming completions
/// * An error if the request fails
///
/// # Errors
///
/// Returns an error if the request fails
/// * Network communication failures
/// * Invalid response formats
/// * Unexpected response content
/// * Internal server errors
/// * Invalid request parameters
/// * Unsupported model or endpoint
#[instrument(
    level = "info",
    skip_all,
    fields(
        path = endpoint,
        completion_type = "non-streaming",
        stack_small_id,
        estimated_total_tokens = num_input_tokens + estimated_output_tokens,
        payload_hash
    )
)]
#[allow(clippy::too_many_arguments)]
async fn handle_non_streaming_response(
    state: &ProxyState,
    node_address: &String,
    user_id: i64,
    headers: HeaderMap,
    payload: &Value,
    num_input_tokens: i64,
    estimated_output_tokens: i64,
    price_per_million: i64,
    selected_stack_small_id: Option<i64>,
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
        .get(USAGE)
        .and_then(|usage| usage.get(TOTAL_TOKENS))
        .and_then(serde_json::Value::as_u64)
        .map_or(0, |n| n as i64);

    let input_tokens = response
        .get(USAGE)
        .and_then(|usage| usage.get(PROMPT_TOKENS))
        .and_then(serde_json::Value::as_u64)
        .map_or(0, |n| n as i64);

    let output_tokens = response
        .get(USAGE)
        .and_then(|usage| usage.get(COMPLETION_TOKENS))
        .and_then(serde_json::Value::as_u64)
        .map_or(0, |n| n as i64);

    // Record the total tokens in the chat completions tokens metrics
    CHAT_COMPLETIONS_TOTAL_TOKENS.add(
        total_tokens as u64,
        &[KeyValue::new(MODEL, model_name.clone())],
    );
    CHAT_COMPLETIONS_INPUT_TOKENS.add(
        input_tokens as u64,
        &[KeyValue::new(MODEL, model_name.clone())],
    );
    CHAT_COMPLETIONS_COMPLETIONS_TOKENS.add(
        output_tokens as u64,
        &[KeyValue::new(MODEL, model_name.clone())],
    );
    CHAT_COMPLETIONS_TOTAL_TOKENS_PER_USER
        .add(total_tokens as u64, &[KeyValue::new(USER_ID, user_id)]);
    CHAT_COMPLETIONS_INPUT_TOKENS_PER_USER
        .add(input_tokens as u64, &[KeyValue::new(USER_ID, user_id)]);
    CHAT_COMPLETIONS_COMPLETIONS_TOKENS_PER_USER
        .add(output_tokens as u64, &[KeyValue::new(USER_ID, user_id)]);

    CHAT_COMPLETION_REQUESTS_PER_USER.add(1, &[KeyValue::new(USER_ID, user_id)]);

    let verify_hash = endpoint != CONFIDENTIAL_COMPLETIONS_PATH;
    verify_response_hash_and_signature(&response.0, verify_hash)?;

    state
        .state_manager_sender
        .send(
            AtomaAtomaStateManagerEvent::UpdateNodeThroughputPerformance {
                timestamp: DateTime::<Utc>::from(std::time::SystemTime::now()),
                model_name: model_name.clone(),
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

    // NOTE: We need to update the stack num tokens, because the inference response might have produced
    // less tokens than estimated what we initially estimated, from the middleware.
    match selected_stack_small_id {
        Some(stack_small_id) => {
            if let Err(e) = update_state_manager(
                &state.state_manager_sender,
                stack_small_id,
                num_input_tokens + estimated_output_tokens,
                total_tokens,
                &endpoint,
            ) {
                return Err(AtomaProxyError::InternalError {
                    message: format!("Error updating state manager: {e:?}"),
                    client_message: None,
                    endpoint: endpoint.to_string(),
                });
            }
        }
        None => {
            if let Err(e) = update_state_manager_fiat(
                &state.state_manager_sender,
                user_id,
                num_input_tokens,
                0,
                estimated_output_tokens,
                total_tokens,
                price_per_million,
                model_name,
                &endpoint,
            ) {
                return Err(AtomaProxyError::InternalError {
                    message: format!("Error updating fiat state manager: {e:?}"),
                    client_message: None,
                    endpoint: endpoint.to_string(),
                });
            }
        }
    }

    CHAT_COMPLETIONS_LATENCY_METRICS.record(
        time.elapsed().as_secs_f64(),
        &[KeyValue::new("model", model_label)],
    );

    Ok(response.into_response())
}

/// Handles a streaming response from the inference service for the completions endpoint
///
/// This function is used to handle a streaming response from the inference service for the completions endpoint.
///
/// # Arguments
///
/// * `state` - The state of the proxy
/// * `node_address` - The address of the node
/// * `user_id` - The user id
/// * `headers` - The headers of the request
/// * `payload` - The payload of the request
/// * `num_input_tokens` - The number of input tokens
/// * `estimated_output_tokens` - The estimated output tokens
/// * `price_per_million` - The price per million
/// * `selected_stack_small_id` - The selected stack small id
/// * `endpoint` - The endpoint of the request
/// * `model_name` - The name of the model
///
/// # Returns
/// * `Result<Response<Body>>` - The response from the inference service
///
/// # Errors
/// * `AtomaProxyError` - If the request fails
/// * `reqwest::Error` - If the request fails
/// * `serde_json::Error` - If the request fails
/// * `flume::Error` - If the request fails
/// * `tokio::Error` - If the request fails
///
#[instrument(
    level = "info",
    skip_all,
    fields(
        path = endpoint,
        completion_type = "streaming",
        stack_small_id,
        estimated_total_tokens = num_input_tokens + estimated_output_tokens,
        payload_hash
    )
)]
#[allow(clippy::too_many_arguments)]
async fn handle_streaming_response(
    state: &ProxyState,
    node_address: &String,
    user_id: i64,
    mut headers: HeaderMap,
    payload: &Value,
    num_input_tokens: i64,
    estimated_output_tokens: i64,
    price_per_million: i64,
    selected_stack_small_id: Option<i64>,
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
            num_input_tokens,
            estimated_output_tokens,
            price_per_million,
            start,
            user_id,
            model_name.clone(),
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
                                    "Error sending chunk for model {model_name}: {e}",
                                );
                                // We continue the loop, to allow the streamer to finish with updated usage from the node
                            }
                        }
                        Some(Err(e)) => {
                            tracing::error!(
                                target = "atoma-service-chat-completions",
                                level = "error",
                                "Error sending chunk for the inner streamer with error: {e}"
                            );
                        }
                        None => {
                            break;
                        }
                    }
                }
                Ok(()) = kill_signal_receiver.recv_async() => {
                    INTENTIONALLY_CANCELLED_CHAT_COMPLETION_STREAMING_REQUESTS.add(1, &[KeyValue::new(USER_ID, user_id), KeyValue::new(MODEL, model_name.clone())]);
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
pub struct RequestModelCompletions {
    /// The identifier of the model to use for the completion
    /// (e.g., "gpt-3.5-turbo", "meta-llama/Llama-3.3-70B-Instruct", etc.)
    model: String,

    /// Completions prompt following the OpenAI API format
    prompt: CompletionsPrompt,

    /// The maximum number of tokens to generate in the completion
    /// This limits the length of the model's response
    max_tokens: u64,
}

impl RequestModel for RequestModelCompletions {
    fn new(request: &Value) -> Result<Self>
    where
        Self: Sized,
    {
        let model = request.get(MODEL).and_then(|m| m.as_str()).ok_or_else(|| {
            AtomaProxyError::RequestError {
                message: "Missing or invalid 'model' field for `RequestModelCompletions`"
                    .to_string(),
                endpoint: COMPLETIONS_PATH.to_string(),
            }
        })?;

        let prompt = request
            .get(PROMPT)
            .map(CompletionsPrompt::deserialize)
            .transpose()
            .map_err(|e| AtomaProxyError::RequestError {
                message: format!("Invalid 'prompt' field for `RequestModelCompletions`: {e}"),
                endpoint: COMPLETIONS_PATH.to_string(),
            })?
            .ok_or_else(|| AtomaProxyError::RequestError {
                message: "Missing or invalid 'prompt' field for `RequestModelCompletions`"
                    .to_string(),
                endpoint: COMPLETIONS_PATH.to_string(),
            })?;

        let max_tokens = request
            .get(MAX_COMPLETION_TOKENS)
            .or_else(|| request.get(MAX_TOKENS))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(DEFAULT_MAX_TOKENS);

        Ok(Self {
            model: model.to_string(),
            prompt,
            max_tokens,
        })
    }

    fn get_model(&self) -> String {
        self.model.clone()
    }

    fn get_compute_units_estimate(
        &self,
        tokenizer: Option<&tokenizers::Tokenizer>,
    ) -> Result<ComputeUnitsEstimate> {
        // Helper function to count the number of tokens in a text prompt
        let count_text_tokens = |text: &str, tokenizer: &tokenizers::Tokenizer| -> Result<u64> {
            Ok(tokenizer
                .encode(text, true)
                .map_err(|err| AtomaProxyError::InternalError {
                    message: format!("Failed to encode message: {err:?}"),
                    client_message: Some(
                        "Failed to encode message using the model's tokenizer".to_string(),
                    ),
                    endpoint: COMPLETIONS_PATH.to_string(),
                })?
                .get_ids()
                .len() as u64)
        };
        match &self.prompt {
            CompletionsPrompt::Single(prompt) => {
                let tokenizer = tokenizer.ok_or_else(|| AtomaProxyError::RequestError {
                    message: "Tokenizer is required for `RequestModelCompletions`".to_string(),
                    endpoint: COMPLETIONS_PATH.to_string(),
                })?;
                let num_input_compute_units =
                    count_text_tokens(prompt, tokenizer).map_err(|err| {
                        AtomaProxyError::RequestError {
                            message: format!("Failed to count text tokens: {err:?}"),
                            endpoint: COMPLETIONS_PATH.to_string(),
                        }
                    })?;
                Ok(ComputeUnitsEstimate {
                    num_input_tokens: num_input_compute_units,
                    max_output_tokens: self.max_tokens,
                })
            }
            CompletionsPrompt::List(prompts) => {
                let tokenizer = tokenizer.ok_or_else(|| AtomaProxyError::RequestError {
                    message: "Tokenizer is required for `RequestModelCompletions`".to_string(),
                    endpoint: COMPLETIONS_PATH.to_string(),
                })?;
                let num_input_compute_units = prompts
                    .iter()
                    .map(|prompt| count_text_tokens(prompt, tokenizer).unwrap_or(0))
                    .sum();
                Ok(ComputeUnitsEstimate {
                    num_input_tokens: num_input_compute_units,
                    max_output_tokens: self.max_tokens,
                })
            }
            CompletionsPrompt::Tokens(tokens) => {
                let num_input_compute_units = tokens.len() as u64;
                Ok(ComputeUnitsEstimate {
                    num_input_tokens: num_input_compute_units,
                    max_output_tokens: self.max_tokens,
                })
            }
            CompletionsPrompt::TokenArrays(token_arrays) => {
                let num_input_compute_units =
                    token_arrays.iter().map(|tokens| tokens.len() as u64).sum();
                Ok(ComputeUnitsEstimate {
                    num_input_tokens: num_input_compute_units,
                    max_output_tokens: self.max_tokens,
                })
            }
        }
    }
}

pub mod openai_api_completions {
    use std::collections::HashMap;

    use serde::{Deserialize, Serialize};
    use utoipa::ToSchema;

    use crate::server::handlers::chat_completions::openai_api::stream_options;

    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    pub struct CompletionsRequest {
        /// ID of the model to use
        #[schema(example = "meta-llama/Llama-3.3-70B-Instruct")]
        pub model: String,

        /// The prompt to generate completions for
        #[schema(example = json!(["Hello!"]))]
        pub prompt: CompletionsPrompt,

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
        pub logit_bias: Option<std::collections::HashMap<String, f32>>,

        /// An integer between 0 and 20 specifying the number of most likely tokens to return at each token position, each with an associated log probability.
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

    #[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
    #[serde(untagged)]
    pub enum CompletionsPrompt {
        /// A single string prompt
        #[serde(rename = "single")]
        Single(String),

        /// An array of strings prompts
        #[serde(rename = "list")]
        List(Vec<String>),

        /// An array of tokens
        #[serde(rename = "tokens")]
        Tokens(Vec<u32>),

        /// An array of token arrays
        #[serde(rename = "token_arrays")]
        TokenArrays(Vec<Vec<u32>>),
    }

    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    pub struct CompletionsResponse {
        /// Array of completion choices response
        #[schema(example = json!([
            {
                "text": "This is a test",
                "index": 0,
                "logprobs": null,
                "finish_reason": "stop"
            }
        ]))]
        pub choices: Vec<CompletionChoice>,

        /// The usage information for the request
        #[schema(example = json!({
            "prompt_tokens": 10,
            "completion_tokens": 10,
            "total_tokens": 20
        }))]
        pub usage: Usage,

        /// The creation time of the request
        #[schema(example = "2021-01-01T00:00:00.000Z")]
        pub created: i64,

        /// The ID of the request
        #[schema(example = "cmpl-1234567890")]
        pub id: String,

        /// The model used for the request
        #[schema(example = "meta-llama/Llama-3.3-70B-Instruct")]
        pub model: String,

        /// The object type
        #[schema(example = "text_completion")]
        pub object: String,

        /// The system fingerprint
        #[schema(example = "system-fingerprint")]
        pub system_fingerprint: String,
    }

    /// A completion choice response
    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    pub struct CompletionChoice {
        /// The generated text
        #[schema(example = "This is a test")]
        pub text: String,

        /// The index of the choice in the list of choices
        #[schema(example = 0)]
        pub index: i32,

        /// The log probabilities of the chosen tokens
        #[schema(example = "null")]
        pub logprobs: Option<LogProbs>,

        /// The reason the model stopped generating tokens
        #[schema(example = "stop")]
        pub finish_reason: String,
    }

    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    pub struct LogProbs {
        /// The tokens
        #[schema(example = json!([
            "Hello ",
            "world"
        ]))]
        pub tokens: Vec<String>,

        /// The log probabilities of the tokens
        #[schema(example = json!([
            0.5,
            -0.5
        ]))]
        pub token_logprobs: Vec<f32>,

        /// The top log probabilities
        #[schema(example = json!([
            {
                "Hello ": -0.2,
                "world": -0.8
            }
        ]))]
        pub top_logprobs: Vec<HashMap<String, f32>>,

        /// The text offset of the tokens
        #[schema(example = json!([
            0,
            10
        ]))]
        pub text_offset: Vec<u32>,
    }

    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    pub struct Usage {
        /// The number of prompt tokens used
        #[schema(example = 10)]
        pub prompt_tokens: u32,

        /// The number of completion tokens used
        #[schema(example = 10)]
        pub completion_tokens: u32,

        /// The total number of tokens used
        #[schema(example = 20)]
        pub total_tokens: u32,

        /// The details of the completion tokens
        #[schema(example = json!({
            "accepted_prediction_tokens": 10,
            "audio_tokens": 0,
            "reasoning_tokens": 10,
            "rejected_prediction_tokens": 0
        }))]
        pub completion_tokens_details: CompletionTokensDetails,

        /// The details of the prompt tokens
        #[schema(example = json!({
            "audio_tokens": 0,
            "cached_tokens": 10,
        }))]
        pub prompt_tokens_details: PromptTokensDetails,
    }

    /// The details of the completion tokens
    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    #[allow(clippy::struct_field_names)]
    pub struct CompletionTokensDetails {
        /// The number of tokens in the completion
        #[schema(example = 10)]
        pub accepted_prediction_tokens: u32,

        /// The number of audio tokens
        #[schema(example = 0)]
        pub audio_tokens: u32,

        /// The number of reasoning tokens
        #[schema(example = 10)]
        pub reasoning_tokens: u32,

        /// The number of rejected prediction tokens
        #[schema(example = 0)]
        pub rejected_prediction_tokens: u32,
    }

    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    pub struct PromptTokensDetails {
        /// The number of audio tokens
        #[schema(example = 0)]
        pub audio_tokens: u32,

        /// The number of cached tokens
        #[schema(example = 10)]
        pub cached_tokens: u32,
    }

    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    pub struct CreateCompletionsStreamRequest {
        #[serde(flatten)]
        pub completion_request: CompletionsRequest,

        /// Whether to stream back partial progress. Must be true for this request type.
        #[schema(default = true)]
        pub stream: bool,
    }

    #[derive(Debug, Serialize, Deserialize, ToSchema)]
    pub struct CompletionsStreamResponse {
        /// Array of completion choices response
        #[schema(example = json!([
            {
                "text": "This is a test",
                "index": 0,
                "logprobs": null,
                "finish_reason": "stop"
            }
        ]))]
        pub choices: Vec<CompletionChoice>,

        /// The creation time of the request
        #[schema(example = "2021-01-01T00:00:00.000Z")]
        pub created: i64,

        /// The ID of the request
        #[schema(example = "cmpl-1234567890")]
        pub id: String,

        /// The model used for the request
        #[schema(example = "meta-llama/Llama-3.3-70B-Instruct")]
        pub model: String,

        /// The object type
        #[schema(example = "text_completion")]
        pub object: String,

        /// The system fingerprint
        #[schema(example = "system-fingerprint")]
        pub system_fingerprint: String,
    }
}
