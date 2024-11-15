use std::sync::Arc;
use std::time::{Duration, Instant};

use atoma_state::types::AtomaAtomaStateManagerEvent;
use axum::body::Body;
use axum::http::{header::AUTHORIZATION, StatusCode};
use axum::response::{IntoResponse, Response, Sse};
use axum::{extract::State, http::HeaderMap, Json};
use flume::Sender;
use serde_json::Value;
use tokenizers::Tokenizer;
use tokio::sync::{oneshot, RwLock};
use tracing::{error, instrument};
use utoipa::OpenApi;

use crate::sui::Sui;

use super::http_server::ProxyState;
use super::streamer::Streamer;

/// Path for the chat completions endpoint.
///
/// This endpoint follows the OpenAI API format for chat completions
/// and is used to process chat-based requests for AI model inference.
pub const CHAT_COMPLETIONS_PATH: &str = "/v1/chat/completions";

// The default value when creating a new stack entry.
// TODO: Make this configurable or compute from the available stacks subscriptions.
const STACK_ENTRY_COMPUTE_UNITS: u64 = 1000;

// The default price for a new stack entry.
// TODO: Make this configurable or compute from the available stacks subscriptions.
const STACK_ENTRY_PRICE: u64 = 100;

/// The interval for the keep-alive message in the SSE stream.
const STREAM_KEEP_ALIVE_INTERVAL_IN_SECONDS: u64 = 15;

/// OpenAPI documentation for the chat completions endpoint.
///
/// This struct is used to generate OpenAPI documentation for the chat completions
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(chat_completions_handler))]
pub(crate) struct ChatCompletionsOpenApi;

/// Handles the chat completions request.
///
/// This function handles the chat completions request by processing the payload
/// and sending the request to the appropriate node for processing.
/// 1) Check the authentication of the request.
/// 2) Get the model from the payload.
/// 3) Get the stacks for the model.
/// 4) In case no stacks are found, get the tasks for the model and acquire a new stack entry.
/// 5) Get the public address of the selected node.
/// 6) Send the OpenAI API request to the selected node.
/// 7) Return the response from the OpenAI API.
///
/// # Arguments
///
/// * `state`: The shared state of the application.
/// * `headers`: The headers of the request.
/// * `payload`: The payload of the request.
///
/// # Returns
///
/// Returns the response from the OpenAI API or an error status code.
///
/// # Errors
///
/// Returns an error status code if the authentication fails, the model is not found, no tasks are found for the model, no node address is found, or an internal server error occurs.
#[utoipa::path(
    post,
    path = CHAT_COMPLETIONS_PATH,
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
    fields(endpoint = CHAT_COMPLETIONS_PATH, payload = ?payload)
)]
pub async fn chat_completions_handler(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<Response<Body>, StatusCode> {
    let is_streaming = payload
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut payload = payload;
    if is_streaming {
        payload["stream_options"] = serde_json::json!({
            "include_usage": true
        });
    }
    let (
        node_address,
        node_id,
        signature,
        selected_stack_small_id,
        headers,
        estimated_total_tokens,
    ) = authenticate_and_process(&state, headers, &payload).await?;
    if is_streaming {
        handle_streaming_response(
            state,
            node_address,
            node_id,
            signature,
            selected_stack_small_id,
            headers,
            payload,
            estimated_total_tokens as i64,
        )
        .await
    } else {
        handle_non_streaming_response(
            state,
            node_address,
            node_id,
            signature,
            selected_stack_small_id,
            headers,
            payload,
            estimated_total_tokens as i64,
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
        path = CHAT_COMPLETIONS_PATH,
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
    node_id: i64,
    signature: String,
    selected_stack_small_id: i64,
    headers: HeaderMap,
    payload: Value,
    estimated_total_tokens: i64,
) -> Result<Response<Body>, StatusCode> {
    let client = reqwest::Client::new();
    let time = Instant::now();
    let response = client
        .post(format!("{}{}", node_address, CHAT_COMPLETIONS_PATH))
        .headers(headers)
        .header("X-Signature", signature)
        .header("X-Stack-Small-Id", selected_stack_small_id)
        .header("Content-Length", payload.to_string().len()) // Set the real length of the payload
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
                node_small_id: node_id,
                input_tokens,
                output_tokens,
                time: time.elapsed().as_secs_f64(),
            },
        )
        .map_err(|err| {
            error!("Error updating node throughput performance: {}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

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
        path = CHAT_COMPLETIONS_PATH,
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
    signature: String,
    selected_stack_small_id: i64,
    headers: HeaderMap,
    payload: Value,
    estimated_total_tokens: i64,
) -> Result<Response<Body>, StatusCode> {
    // NOTE: If streaming is requested, add the include_usage option to the payload
    // so that the atoma node state manager can be updated with the total number of tokens
    // that were processed for this request.

    let client = reqwest::Client::new();
    let start = Instant::now();
    let response = client
        .post(format!("{}{}", node_address, CHAT_COMPLETIONS_PATH))
        .headers(headers)
        .header("X-Signature", signature)
        .header("X-Stack-Small-Id", selected_stack_small_id)
        .header("Content-Length", payload.to_string().len()) // Set the real length of the payload
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

    // Create the SSE stream
    let stream = Sse::new(Streamer::new(
        stream,
        state.state_manager_sender,
        selected_stack_small_id,
        estimated_total_tokens,
        start,
        node_id,
    ))
    .keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_millis(STREAM_KEEP_ALIVE_INTERVAL_IN_SECONDS))
            .text("keep-alive"),
    );

    Ok(stream.into_response())
}

/// Authenticates the request and processes initial steps up to signature creation.
///
/// # Arguments
///
/// * `state` - The proxy state containing password, models, and other shared state
/// * `headers` - Request headers containing authorization
/// * `payload` - Request payload containing model and token information
///
/// # Returns
///
/// Returns a tuple of (node_address, signature, selected_stack_small_id, headers) if successful
#[instrument(level = "info", skip_all)]
async fn authenticate_and_process(
    state: &ProxyState,
    headers: HeaderMap,
    payload: &Value,
) -> Result<(String, i64, String, i64, HeaderMap, u64), StatusCode> {
    // Authentication and payload extraction
    let (model, max_tokens, messages, tokenizer_index) =
        authenticate_and_extract(state, &headers, payload).await?;

    // Token estimation
    let total_tokens =
        get_token_estimate(&messages, max_tokens, &state.tokenizers[tokenizer_index]).await?;

    dbg!(&state.state_manager_sender);
    dbg!(state.state_manager_sender.is_disconnected());
    // Get node selection
    let (selected_stack_small_id, selected_node_id) = get_selected_node(
        &model,
        &state.state_manager_sender,
        &state.sui,
        total_tokens,
    )
    .await?;

    // Get node address
    let (result_sender, result_receiver) = oneshot::channel();
    state
        .state_manager_sender
        .send(AtomaAtomaStateManagerEvent::GetNodePublicAddress {
            node_small_id: selected_node_id,
            result_sender,
        })
        .map_err(|err| {
            error!("Failed to send GetNodePublicAddress event: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let node_address = result_receiver
        .await
        .map_err(|err| {
            error!("Failed to receive GetNodePublicAddress result: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map_err(|err| {
            error!("Failed to get GetNodePublicAddress result: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or_else(|| {
            error!("No node address found for node {}", selected_node_id);
            StatusCode::NOT_FOUND
        })?;

    // Get signature
    let signature = state
        .sui
        .write()
        .await
        .get_sui_signature(payload)
        .map_err(|err| {
            error!("Failed to get Sui signature: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Prepare headers
    let mut headers = headers;
    headers.remove(AUTHORIZATION);

    Ok((
        node_address,
        selected_node_id,
        signature,
        selected_stack_small_id,
        headers,
        total_tokens,
    ))
}

/// Authenticates the request and extracts required fields from the payload.
///
/// # Arguments
///
/// * `state` - The proxy state containing password and models
/// * `headers` - Request headers containing authorization
/// * `payload` - Request payload containing model and token information
///
/// # Returns
///
/// Returns a tuple of (model, max_tokens, messages, tokenizer_index) if successful
#[instrument(level = "info", skip_all)]
async fn authenticate_and_extract(
    state: &ProxyState,
    headers: &HeaderMap,
    payload: &Value,
) -> Result<(String, u64, Value, usize), StatusCode> {
    if !check_auth(&state.password, headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let model = payload
        .get("model")
        .and_then(|model| model.as_str())
        .ok_or(StatusCode::BAD_REQUEST)?
        .to_string();

    let max_tokens = payload
        .get("max_tokens")
        .and_then(|max_tokens| max_tokens.as_u64())
        .ok_or(StatusCode::BAD_REQUEST)?;

    let messages = payload.get("messages").ok_or(StatusCode::BAD_REQUEST)?;

    let tokenizer_index = state
        .models
        .iter()
        .position(|m| m == &model)
        .ok_or_else(|| {
            error!("Model not supported");
            StatusCode::BAD_REQUEST
        })?;

    Ok((model, max_tokens, messages.clone(), tokenizer_index))
}

/// Checks the authentication of the request.
///
/// This function checks the authentication of the request by comparing the
/// provided password with the `Authorization` header in the request.
///
/// # Arguments
///
/// * `password`: The password to check against.
/// * `headers`: The headers of the request.
///
/// # Returns
///
/// Returns `true` if the authentication is successful, `false` otherwise.
///
/// # Examples
///
/// ```rust
/// let mut headers = HeaderMap::new();
/// headers.insert("Authorization", "Bearer password".parse().unwrap());
/// let password = "password";
///
/// assert_eq!(check_auth(password, &headers), true);
/// assert_eq!(check_auth("wrong_password", &headers), false);
/// ```
#[instrument(level = "info", skip_all)]
fn check_auth(password: &str, headers: &HeaderMap) -> bool {
    if let Some(auth) = headers.get("Authorization") {
        if let Ok(auth) = auth.to_str() {
            if auth == format!("Bearer {}", password) {
                return true;
            }
        }
    }
    false
}

/// Estimates the total number of tokens in the messages.
///
/// This function estimates the total number of tokens in the messages by encoding each message
/// and counting the number of tokens in the encoded message.
///
/// # Arguments
///
/// * `messages`: The messages to estimate the number of tokens for.
/// * `max_tokens`: The maximum number of tokens allowed.
/// * `tokenizers`: The tokenizers used to encode the messages.
///
/// # Returns
///
/// Returns the estimated total number of tokens in the messages.
///
/// # Errors
///
/// Returns a bad request status code if the messages are not in the expected format.
/// Returns an internal server error status code if encoding the message fails.
#[instrument(level = "info", skip_all)]
async fn get_token_estimate(
    messages: &Value,
    max_tokens: u64,
    tokenizers: &Arc<Tokenizer>,
) -> Result<u64, StatusCode> {
    let mut total_num_tokens = 0;
    for message in messages.as_array().ok_or(StatusCode::BAD_REQUEST)? {
        let content = message
            .get("content")
            .and_then(|content| content.as_str())
            .ok_or(StatusCode::BAD_REQUEST)?;
        let num_tokens = tokenizers
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
    total_num_tokens += max_tokens;
    Ok(total_num_tokens)
}

/// Selects a node for processing a model request by either finding an existing stack or acquiring a new one.
///
/// This function follows a two-step process:
/// 1. First, it attempts to find existing stacks that can handle the requested model and compute units
/// 2. If no suitable stacks exist, it acquires a new stack entry by:
///    - Finding available tasks for the model
///    - Creating a new stack entry with predefined compute units and price
///    - Registering the new stack with the state manager
///
/// # Arguments
///
/// * `model` - The name/identifier of the AI model being requested
/// * `state_manager_sender` - Channel for sending events to the state manager
/// * `sui` - Reference to the Sui interface for blockchain operations
/// * `total_tokens` - The total number of compute units (tokens) needed for the request
///
/// # Returns
///
/// Returns a tuple of `(stack_small_id, selected_node_id)` where:
/// * `stack_small_id` - The identifier for the selected/created stack
/// * `selected_node_id` - The identifier for the node that will process the request
///
/// # Errors
///
/// Returns a `StatusCode` error in the following cases:
/// * `INTERNAL_SERVER_ERROR` - Communication errors with state manager or Sui interface
/// * `NOT_FOUND` - No tasks available for the requested model
/// * `BAD_REQUEST` - Requested compute units exceed the maximum allowed limit
///
/// # Example
///
/// ```no_run
/// let (stack_id, node_id) = get_selected_node(
///     "gpt-4",
///     &state_manager_sender,
///     &sui,
///     1000
/// ).await?;
/// ```
#[instrument(level = "info", skip_all, fields(%model))]
async fn get_selected_node(
    model: &str,
    state_manager_sender: &Sender<AtomaAtomaStateManagerEvent>,
    sui: &Arc<RwLock<Sui>>,
    total_tokens: u64,
) -> Result<(i64, i64), StatusCode> {
    let (result_sender, result_receiver) = oneshot::channel();

    dbg!(&model);
    dbg!(total_tokens);
    state_manager_sender
        .send(AtomaAtomaStateManagerEvent::GetStacksForModel {
            model: model.to_string(),
            free_compute_units: total_tokens as i64,
            result_sender,
        })
        .map_err(|err| {
            error!("Failed to send GetStacksForModel event: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let stacks = result_receiver
        .await
        .map_err(|err| {
            error!("Failed to receive GetStacksForModel result: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map_err(|err| {
            error!("Failed to get GetStacksForModel result: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    dbg!(&stacks);
    if stacks.is_empty() {
        let (result_sender, result_receiver) = oneshot::channel();
        state_manager_sender
            .send(AtomaAtomaStateManagerEvent::GetTasksForModel {
                model: model.to_string(),
                result_sender,
            })
            .map_err(|err| {
                error!("Failed to send GetTasksForModel event: {:?}", err);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        let tasks = result_receiver
            .await
            .map_err(|err| {
                error!("Failed to receive GetTasksForModel result: {:?}", err);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .map_err(|err| {
                error!("Failed to get GetTasksForModel result: {:?}", err);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        if tasks.is_empty() {
            error!("No tasks found for model {}", model);
            return Err(StatusCode::NOT_FOUND);
        }
        // TODO: What should be the default values for the stack entry/price?
        if total_tokens > STACK_ENTRY_COMPUTE_UNITS {
            error!("Total tokens exceeds maximum limit of {STACK_ENTRY_COMPUTE_UNITS}");
            return Err(StatusCode::BAD_REQUEST);
        }
        let event = sui
            .write()
            .await
            .acquire_new_stack_entry(
                tasks[0].task_small_id as u64,
                STACK_ENTRY_COMPUTE_UNITS,
                STACK_ENTRY_PRICE,
            )
            .await
            .map_err(|err| {
                error!("Failed to acquire new stack entry: {:?}", err);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        let stack_small_id = event.stack_small_id.inner as i64;
        let selected_node_id = event.selected_node_id.inner as i64;

        // Send the NewStackAcquired event to the state manager, so we have it in the DB.
        state_manager_sender
            .send(AtomaAtomaStateManagerEvent::NewStackAcquired {
                event,
                already_computed_units: total_tokens as i64,
            })
            .map_err(|err| {
                error!("Failed to send NewStackAcquired event: {:?}", err);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;

        Ok((stack_small_id, selected_node_id))
    } else {
        Ok((stacks[0].stack_small_id, stacks[0].selected_node_id))
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
