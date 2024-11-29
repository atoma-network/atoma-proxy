use std::time::Instant;

use atoma_state::types::AtomaAtomaStateManagerEvent;
use axum::body::Body;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::{extract::State, http::HeaderMap, Json};
use serde_json::Value;
use tracing::{error, instrument};
use utoipa::OpenApi;

use crate::server::http_server::ProxyState;
use crate::server::middleware::RequestMetadataExtension;

use super::request_model::RequestModel;

/// Path for the confidential image generations endpoint.
///
/// This endpoint follows the OpenAI API format for image generations, with additional
/// confidential processing (through AEAD encryption and TEE hardware).
pub const CONFIDENTIAL_IMAGE_GENERATIONS_PATH: &str = "/v1/confidential/images/generations";

/// Path for the image generations endpoint.
///
/// This endpoint follows the OpenAI API format for image generations
pub const IMAGE_GENERATIONS_PATH: &str = "/v1/images/generations";

/// A model representing the parameters for an image generation request.
///
/// This struct encapsulates the required parameters for generating images through
/// the API endpoint.
pub struct RequestModelImageGenerations {
    /// The identifier of the AI model to use for image generation
    model: String,
    /// The number of sampling generation to be performed for this request
    n: u64,
    /// The desired dimensions of the generated images in the format "WIDTHxHEIGHT"
    /// (e.g., "1024x1024")
    size: String,
}

/// OpenAPI documentation for the image generations endpoint.
#[derive(OpenApi)]
#[openapi(paths(image_generations_handler))]
pub(crate) struct ImageGenerationsOpenApi;

impl RequestModel for RequestModelImageGenerations {
    fn new(request: &Value) -> Result<Self, StatusCode> {
        let model = request
            .get("model")
            .and_then(|m| m.as_str())
            .ok_or(StatusCode::BAD_REQUEST)?;
        let n = request
            .get("n")
            .and_then(|n| n.as_u64())
            .ok_or(StatusCode::BAD_REQUEST)?;
        let size = request
            .get("size")
            .and_then(|s| s.as_str())
            .ok_or(StatusCode::BAD_REQUEST)?;

        Ok(Self {
            model: model.to_string(),
            n,
            size: size.to_string(),
        })
    }

    fn get_model(&self) -> Result<String, StatusCode> {
        Ok(self.model.clone())
    }

    fn get_compute_units_estimate(&self, _state: &ProxyState) -> Result<u64, StatusCode> {
        // Parse dimensions from size string (e.g., "1024x1024")
        let dimensions: Vec<u64> = self
            .size
            .split('x')
            .filter_map(|s| s.parse().ok())
            .collect();

        if dimensions.len() != 2 {
            error!("Invalid size format: {}", self.size);
            return Err(StatusCode::BAD_REQUEST);
        }

        let width = dimensions[0];
        let height = dimensions[1];

        // Calculate compute units based on number of images and pixel count
        Ok(self.n * width * height)
    }
}

/// Handles incoming requests for AI image generation.
///
/// This endpoint processes requests to generate images using AI models by forwarding them
/// to the appropriate AI node. The request metadata and compute units have already been
/// validated by middleware before reaching this handler.
///
/// # Arguments
/// * `metadata` - Extension containing pre-processed request metadata (node address, compute units, etc.)
/// * `state` - Application state containing configuration and shared resources
/// * `headers` - HTTP headers from the incoming request
/// * `payload` - JSON payload containing image generation parameters
///
/// # Returns
/// * `Result<Response<Body>, StatusCode>` - The processed response from the AI node or an error status
///
/// # Errors
/// * Returns various status codes based on the underlying `handle_image_generation_response`:
///   - `INTERNAL_SERVER_ERROR` - If there's an error communicating with the AI node
///
/// # Example Payload
/// ```json
/// {
///     "model": "stable-diffusion-v1-5",
///     "n": 1,
///     "size": "1024x1024"
/// }
/// ```
#[utoipa::path(
    post,
    path = IMAGE_GENERATIONS_PATH,
    responses(
        (status = OK, description = "Image generations", body = Value),
        (status = BAD_REQUEST, description = "Bad request"),
        (status = UNAUTHORIZED, description = "Unauthorized"),
        (status = INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
#[instrument(
    level = "info",
    skip_all,
    fields(endpoint = IMAGE_GENERATIONS_PATH, payload = ?payload)
)]
pub async fn image_generations_handler(
    Extension(metadata): Extension<RequestMetadataExtension>,
    State(state): State<ProxyState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<Response<Body>, StatusCode> {
    handle_image_generation_response(
        state,
        metadata.node_address,
        metadata.node_id,
        headers,
        payload,
        metadata.num_compute_units as i64,
    )
    .await
}

/// Handles the response processing for image generation requests.
///
/// This function is responsible for forwarding the image generation request to the appropriate AI node
/// and processing its response. It performs the following steps:
/// 1. Creates an HTTP client
/// 2. Forwards the request to the AI node with appropriate headers
/// 3. Processes the response and handles any errors
///
/// # Arguments
/// * `_state` - Application state containing configuration and shared resources (currently unused)
/// * `node_address` - The base URL of the AI node to send the request to
/// * `node_id` - Unique identifier of the target AI node
/// * `signature` - Authentication signature for the request
/// * `selected_stack_small_id` - Identifier for the billing stack entry
/// * `headers` - HTTP headers to forward with the request
/// * `payload` - The original image generation request payload
/// * `_estimated_total_tokens` - Estimated computational cost (currently unused)
///
/// # Returns
/// * `Result<Response<Body>, StatusCode>` - The processed response from the AI node or an error status
///
/// # Errors
/// * Returns `INTERNAL_SERVER_ERROR` (500) if:
///   - The request to the AI node fails
///   - The response cannot be parsed as valid JSON
///
/// # Note
/// This function is instrumented with tracing to log important metrics and debug information.
/// There is a pending TODO to implement node throughput performance tracking.
#[instrument(
    level = "info",
    skip_all,
    fields(
        path = IMAGE_GENERATIONS_PATH,
        stack_small_id,
        estimated_total_tokens
    )
)]
#[allow(clippy::too_many_arguments)]
async fn handle_image_generation_response(
    state: ProxyState,
    node_address: String,
    selected_node_id: i64,
    headers: HeaderMap,
    payload: Value,
    total_tokens: i64,
) -> Result<Response<Body>, StatusCode> {
    let client = reqwest::Client::new();
    let time = Instant::now();
    // Send the request to the AI node
    let response = client
        .post(format!("{}{}", node_address, IMAGE_GENERATIONS_PATH))
        .headers(headers)
        .json(&payload)
        .send()
        .await
        .map_err(|err| {
            error!("Failed to send image generation request: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .json::<Value>()
        .await
        .map_err(|err| {
            error!("Failed to parse image generation response: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })
        .map(Json)?;

    // Update the node throughput performance
    state
        .state_manager_sender
        .send(
            AtomaAtomaStateManagerEvent::UpdateNodeThroughputPerformance {
                node_small_id: selected_node_id,
                input_tokens: 0,
                output_tokens: total_tokens,
                time: time.elapsed().as_secs_f64(),
            },
        )
        .map_err(|err| {
            error!("Failed to update node throughput performance: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(response.into_response())
}