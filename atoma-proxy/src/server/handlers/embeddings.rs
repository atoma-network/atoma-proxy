use std::time::Instant;

use atoma_state::types::AtomaAtomaStateManagerEvent;
use axum::{
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::Value;
use sui_sdk::types::digests::TransactionDigest;
use tracing::{error, instrument};
use utoipa::OpenApi;

use crate::server::http_server::ProxyState;

use super::{authenticate_and_process, request_model::RequestModel, ProcessedRequest};

// A model representing an embeddings request payload.
///
/// This struct encapsulates the necessary fields for processing an embeddings request
/// following the OpenAI API format.
pub struct RequestModelEmbeddings {
    /// The name of the model to use for generating embeddings (e.g., "text-embedding-ada-002")
    model: String,
    /// The input text to generate embeddings for
    input: String,
}

/// Path for the embeddings endpoint.
///
/// This endpoint follows the OpenAI API format for embeddings
/// and is used to generate vector embeddings for input text.
pub const EMBEDDINGS_PATH: &str = "/v1/embeddings";

/// OpenAPI documentation for the embeddings endpoint.
#[derive(OpenApi)]
#[openapi(paths(embeddings_handler))]
pub(crate) struct EmbeddingsOpenApi;

impl RequestModel for RequestModelEmbeddings {
    fn new(request: &Value) -> Result<Self, StatusCode> {
        let model = request
            .get("model")
            .and_then(|m| m.as_str())
            .ok_or(StatusCode::BAD_REQUEST)?;
        let input = request
            .get("input")
            .and_then(|i| i.as_str())
            .ok_or(StatusCode::BAD_REQUEST)?;

        Ok(Self {
            model: model.to_string(),
            input: input.to_string(),
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

        let num_tokens = tokenizer
            .encode(self.input.as_str(), true)
            .map_err(|err| {
                error!("Failed to encode input: {:?}", err);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .get_ids()
            .len() as u64;

        Ok(num_tokens)
    }
}

/// Handles incoming embeddings requests by authenticating, processing, and forwarding them to the appropriate node.
///
/// This endpoint follows the OpenAI API format for generating vector embeddings from input text.
/// The handler performs several key operations:
///
/// 1. Authentication and validation:
///    - Validates the API key and permissions
///    - Ensures the request payload is properly formatted
///
/// 2. Request processing:
///    - Extracts and validates the model name
///    - Parses the input text for embedding generation
///    - Estimates token usage for billing purposes
///
/// 3. Node selection and routing:
///    - Selects an appropriate processing node based on load and availability
///    - Forwards the request to the chosen node
///    - Handles node communication and response processing
///
/// # Arguments
/// * `state` - The shared proxy state containing configuration and runtime information
/// * `headers` - HTTP headers from the incoming request, including authentication
/// * `payload` - The JSON request body containing the model and input text
///
/// # Returns
/// * `Ok(Response)` - The embeddings response from the processing node
/// * `Err(StatusCode)` - An error status code if any step fails
///
/// # Errors
/// * `BAD_REQUEST` - Invalid payload format or unsupported model
/// * `UNAUTHORIZED` - Invalid or missing authentication
/// * `INTERNAL_SERVER_ERROR` - Processing or node communication failures
#[utoipa::path(
    post,
    path = "",
    responses(
        (status = OK, description = "Embeddings generated successfully", body = Value),
        (status = BAD_REQUEST, description = "Bad request"),
        (status = UNAUTHORIZED, description = "Unauthorized"),
        (status = INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
#[instrument(
    level = "info",
    skip_all,
    fields(endpoint = EMBEDDINGS_PATH, payload = ?payload)
)]
pub async fn embeddings_handler(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<Response<Body>, StatusCode> {
    let request_model = RequestModelEmbeddings::new(&payload)?;

    let ProcessedRequest {
        node_address,
        node_id,
        signature,
        stack_small_id: selected_stack_small_id,
        headers,
        num_compute_units: num_input_compute_units,
        tx_digest,
    } = authenticate_and_process(request_model, &state, headers, &payload).await?;

    handle_embeddings_response(
        state,
        node_address,
        node_id,
        signature,
        selected_stack_small_id,
        headers,
        payload,
        num_input_compute_units as i64,
        tx_digest,
    )
    .await
}

/// Handles the response processing for embeddings requests by forwarding them to AI nodes and managing performance metrics.
///
/// This function is responsible for:
/// 1. Forwarding the embeddings request to the selected AI node
/// 2. Processing the node's response
/// 3. Updating performance metrics for the node
///
/// # Arguments
/// * `state` - The shared proxy state containing configuration and runtime information
/// * `node_address` - The URL of the selected AI node
/// * `selected_node_id` - The unique identifier of the selected node
/// * `signature` - Authentication signature for the node request
/// * `selected_stack_small_id` - The identifier for the selected processing stack
/// * `headers` - HTTP headers to forward with the request
/// * `payload` - The JSON request body containing the embeddings request
/// * `num_input_compute_units` - The number of compute units (tokens) in the input
///
/// # Returns
/// * `Ok(Response<Body>)` - The processed embeddings response from the AI node
/// * `Err(StatusCode)` - An error status code if any step fails
///
/// # Errors
/// * Returns `INTERNAL_SERVER_ERROR` if:
///   - The request to the AI node fails
///   - The response parsing fails
///   - Updating node performance metrics fails
#[instrument(
    level = "info",
    skip_all,
    fields(
        path = EMBEDDINGS_PATH,
        stack_small_id,
        estimated_total_tokens
    )
)]
#[allow(clippy::too_many_arguments)]
async fn handle_embeddings_response(
    state: ProxyState,
    node_address: String,
    selected_node_id: i64,
    signature: String,
    selected_stack_small_id: i64,
    headers: HeaderMap,
    payload: Value,
    num_input_compute_units: i64,
    tx_digest: Option<TransactionDigest>,
) -> Result<Response<Body>, StatusCode> {
    let client = reqwest::Client::new();
    let time = Instant::now();
    // Send the request to the AI node
    let req_builder = client
        .post(format!("{}{}", node_address, EMBEDDINGS_PATH))
        .headers(headers)
        .header("X-Signature", signature)
        .header("X-Stack-Small-Id", selected_stack_small_id);
    let req_builder = if let Some(tx_digest) = tx_digest {
        req_builder.header("X-Tx-Digest", tx_digest.base58_encode())
    } else {
        req_builder
    };
    let response = req_builder
        .header("Content-Length", payload.to_string().len())
        .json(&payload)
        .send()
        .await
        .map_err(|err| {
            error!("Failed to send embeddings request: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .json::<Value>()
        .await
        .map_err(|err| {
            error!("Failed to parse embeddings response: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })
        .map(Json)?;

    // Update the node throughput performance
    state
        .state_manager_sender
        .send(
            AtomaAtomaStateManagerEvent::UpdateNodeThroughputPerformance {
                node_small_id: selected_node_id,
                input_tokens: num_input_compute_units,
                output_tokens: 0,
                time: time.elapsed().as_secs_f64(),
            },
        )
        .map_err(|err| {
            error!("Failed to update node throughput performance: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(response.into_response())
}
