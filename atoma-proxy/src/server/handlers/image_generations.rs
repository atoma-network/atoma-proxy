use std::time::Instant;

use atoma_state::types::AtomaAtomaStateManagerEvent;
use atoma_utils::constants;
use axum::body::Body;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::{extract::State, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::types::chrono::{DateTime, Utc};
use tracing::{error, instrument};
use utoipa::{OpenApi, ToSchema};
use x25519_dalek::PublicKey;

use crate::server::{
    handlers::{extract_node_encryption_metadata, handle_confidential_compute_decryption_response},
    http_server::ProxyState,
    middleware::{NodeEncryptionMetadata, RequestMetadataExtension},
};

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

/// The model field in the request payload.
const MODEL: &str = "model";

/// The n field in the request payload.
const N: &str = "n";

/// The size field in the request payload.
const SIZE: &str = "size";

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
#[openapi(
    paths(image_generations_create),
    components(schemas(CreateImageRequest, CreateImageResponse, ImageData))
)]
pub(crate) struct ImageGenerationsOpenApi;

impl RequestModel for RequestModelImageGenerations {
    fn new(request: &Value) -> Result<Self, StatusCode> {
        let model = request
            .get(MODEL)
            .and_then(|m| m.as_str())
            .ok_or(StatusCode::BAD_REQUEST)?;
        let n = request
            .get(N)
            .and_then(|n| n.as_u64())
            .ok_or(StatusCode::BAD_REQUEST)?;
        let size = request
            .get(SIZE)
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

/// Create image generation
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
    path = "",
    request_body = CreateImageRequest,
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "Image generations", body = CreateImageResponse),
        (status = BAD_REQUEST, description = "Bad request"),
        (status = UNAUTHORIZED, description = "Unauthorized"),
        (status = INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
#[instrument(
    level = "info",
    skip_all,
    fields(endpoint = metadata.endpoint)
)]
pub async fn image_generations_create(
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
        metadata.endpoint,
        metadata.salt,
        metadata.node_x25519_public_key,
        metadata.model_name,
    )
    .await
}

/// OpenAPI documentation for the image generations endpoint.
#[derive(OpenApi)]
#[openapi(
    paths(confidential_image_generations_create),
    components(schemas(CreateImageRequest, CreateImageResponse, ImageData))
)]
pub(crate) struct ConfidentialImageGenerationsOpenApi;

/// Create confidential image generations
///
/// This endpoint follows the OpenAI API format for generating images,
/// but with confidential processing (through AEAD encryption and TEE hardware).
/// The handler receives pre-processed metadata from middleware and forwards the request to
/// the selected node.
///
/// Note: Authentication, node selection, initial request validation and encryption
/// are handled by middleware before this handler is called.
///
/// # Arguments
/// * `metadata` - Pre-processed request metadata containing node information and compute units
/// * `state` - The shared proxy state containing configuration and runtime information
/// * `headers` - HTTP headers from the incoming request
/// * `payload` - The JSON request body containing the model and input text
///
/// # Returns
/// * `Ok(Response)` - The image generations response from the processing node
/// * `Err(StatusCode)` - An error status code if any step fails
///
/// # Errors
/// * `INTERNAL_SERVER_ERROR` - Processing or node communication failures
#[utoipa::path(
    post,
    path = "",
    request_body = CreateImageRequest,
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "Image generations", body = CreateImageResponse),
        (status = BAD_REQUEST, description = "Bad request"),
        (status = UNAUTHORIZED, description = "Unauthorized"),
        (status = INTERNAL_SERVER_ERROR, description = "Internal server error")
    )
)]
#[instrument(
    level = "info",
    skip_all,
    fields(endpoint = metadata.endpoint)
)]
pub async fn confidential_image_generations_create(
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
        metadata.endpoint,
        metadata.salt,
        metadata.node_x25519_public_key,
        metadata.model_name,
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
        path = endpoint,
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
    endpoint: String,
    salt: Option<[u8; constants::SALT_SIZE]>,
    node_x25519_public_key: Option<PublicKey>,
    model_name: String,
) -> Result<Response<Body>, StatusCode> {
    let client = reqwest::Client::new();
    let time = Instant::now();
    // Send the request to the AI node
    let response = client
        .post(format!("{}{}", node_address, endpoint))
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

    // Update the node throughput performance
    state
        .state_manager_sender
        .send(
            AtomaAtomaStateManagerEvent::UpdateNodeThroughputPerformance {
                timestamp: DateTime::<Utc>::from(std::time::SystemTime::now()),
                model_name,
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

    Ok(Json(response).into_response())
}

/// Request body for image generation
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateImageRequest {
    /// A text description of the desired image(s). The maximum length is 1000 characters.
    pub prompt: String,

    /// The model to use for image generation.
    pub model: String,

    /// The number of images to generate. Must be between 1 and 10.
    pub n: u32,

    /// The quality of the image that will be generated.
    /// `hd` creates images with finer details and greater consistency across the image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,

    /// The format in which the generated images are returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,

    /// The size of the generated images.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,

    /// The style of the generated images.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,

    /// A unique identifier representing your end-user, which can help OpenAI to monitor and detect abuse.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

//TODO: Add support for b64_json format
/// Response format for image generation
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateImageResponse {
    pub created: i64,
    pub data: Vec<ImageData>,
}

/// Individual image data in the response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ImageData {
    pub revised_prompt: String,
    pub url: String,
}
