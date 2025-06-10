use std::time::Instant;

use atoma_state::types::AtomaAtomaStateManagerEvent;
use axum::body::Body;
use axum::response::{IntoResponse, Response};
use axum::Extension;
use axum::{extract::State, http::HeaderMap, Json};
use opentelemetry::KeyValue;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::types::chrono::{DateTime, Utc};
use tokenizers::Tokenizer;
use tracing::instrument;
use utoipa::{OpenApi, ToSchema};

use crate::server::error::AtomaProxyError;
use crate::server::types::{ConfidentialComputeRequest, ConfidentialComputeResponse};
use crate::server::{http_server::ProxyState, middleware::RequestMetadataExtension};

use super::metrics::{
    IMAGE_GENERATION_TOTAL_TOKENS_PER_USER, IMAGE_GEN_LATENCY_METRICS, IMAGE_GEN_NUM_REQUESTS,
    SUCCESSFUL_IMAGE_GENERATION_REQUESTS_PER_USER, TOTAL_BAD_REQUESTS, TOTAL_COMPLETED_REQUESTS,
    TOTAL_FAILED_CONFIDENTIAL_IMAGE_GENERATION_REQUESTS, TOTAL_FAILED_IMAGE_GENERATION_REQUESTS,
    TOTAL_FAILED_REQUESTS, TOTAL_LOCKED_REQUESTS, TOTAL_TOO_EARLY_REQUESTS,
    TOTAL_TOO_MANY_REQUESTS, TOTAL_UNAUTHORIZED_REQUESTS,
    UNSUCCESSFUL_IMAGE_GENERATION_REQUESTS_PER_USER,
};
use super::request_model::ComputeUnitsEstimate;
use super::{
    handle_status_code_error, update_state_manager_fiat, verify_response_hash_and_signature,
};
use super::{request_model::RequestModel, update_state_manager};
use crate::server::{Result, MODEL};

/// Path for the confidential image generations endpoint.
///
/// This endpoint follows the OpenAI API format for image generations, with additional
/// confidential processing (through AEAD encryption and TEE hardware).
pub const CONFIDENTIAL_IMAGE_GENERATIONS_PATH: &str = "/v1/confidential/images/generations";

/// Path for the image generations endpoint.
///
/// This endpoint follows the OpenAI API format for image generations
pub const IMAGE_GENERATIONS_PATH: &str = "/v1/images/generations";

/// The n field in the request payload.
const N: &str = "n";

/// The size field in the request payload.
const SIZE: &str = "size";

/// The model key
const MODEL_KEY: &str = "model";

/// The user id key
const USER_ID_KEY: &str = "user_id";

/// A model representing the parameters for an image generation request.
///
/// This struct encapsulates the required parameters for generating images through
/// the API endpoint.
#[derive(Debug, Clone)]
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
pub struct ImageGenerationsOpenApi;

impl RequestModel for RequestModelImageGenerations {
    fn new(request: &Value) -> Result<Self> {
        let model = request.get(MODEL).and_then(|m| m.as_str()).ok_or_else(|| {
            AtomaProxyError::RequestError {
                message: "Model field   is required".to_string(),
                endpoint: IMAGE_GENERATIONS_PATH.to_string(),
            }
        })?;
        let n = request
            .get(N)
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| AtomaProxyError::RequestError {
                message: "N field is required".to_string(),
                endpoint: IMAGE_GENERATIONS_PATH.to_string(),
            })?;
        let size = request.get(SIZE).and_then(|s| s.as_str()).ok_or_else(|| {
            AtomaProxyError::RequestError {
                message: "Size field is required".to_string(),
                endpoint: IMAGE_GENERATIONS_PATH.to_string(),
            }
        })?;

        Ok(Self {
            model: model.to_string(),
            n,
            size: size.to_string(),
        })
    }

    fn get_model(&self) -> String {
        self.model.clone()
    }

    fn get_compute_units_estimate(
        &self,
        _tokenizer: Option<&Tokenizer>,
    ) -> Result<ComputeUnitsEstimate> {
        // Parse dimensions from size string (e.g., "1024x1024")
        let dimensions: Vec<u64> = self
            .size
            .split('x')
            .filter_map(|s| s.parse().ok())
            .collect();

        if dimensions.len() != 2 {
            return Err(AtomaProxyError::RequestError {
                message: format!("Invalid size format: {}", self.size),
                endpoint: IMAGE_GENERATIONS_PATH.to_string(),
            });
        }

        let width = dimensions[0];
        let height = dimensions[1];

        // Calculate compute units based on number of images and pixel count
        Ok(ComputeUnitsEstimate {
            num_input_tokens: 0,
            max_output_tokens: self.n * width * height,
        })
    }
}

/// Create image
///
/// This endpoint processes requests to generate images using AI models by forwarding them
/// to the appropriate AI node. The request metadata and compute units have already been
/// validated by middleware before reaching this handler.
///
/// ## Errors
/// * Returns various status codes based on the underlying `handle_image_generation_response`:
///   - `INTERNAL_SERVER_ERROR` - If there's an error communicating with the AI node
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
) -> Result<Response<Body>> {
    let endpoint = metadata.endpoint.clone();
    tokio::spawn(async move {
        IMAGE_GENERATION_TOTAL_TOKENS_PER_USER.add(
            metadata.max_output_tokens,
            &[KeyValue::new("user_id", metadata.user_id)],
        );
        // TODO: We should allow cancelling the request if the client disconnects
        match handle_image_generation_response(
            &state,
            metadata.node_address,
            headers,
            payload,
            metadata.max_output_tokens as i64,
            metadata.endpoint.clone(),
            metadata.model_name.clone(),
        )
        .await
        {
            Ok(response) => {
                TOTAL_COMPLETED_REQUESTS
                    .add(1, &[KeyValue::new("model", metadata.model_name.clone())]);
                match metadata.selected_stack_small_id {
                    Some(stack_small_id) => {
                        update_state_manager(
                            &state.state_manager_sender,
                            stack_small_id,
                            (metadata.num_input_tokens.unwrap_or_default()
                                + metadata.max_output_tokens) as i64,
                            (metadata.num_input_tokens.unwrap_or_default()
                                + metadata.max_output_tokens) as i64,
                            &metadata.endpoint,
                        )?;
                    }
                    None => {
                        update_state_manager_fiat(
                            &state.state_manager_sender,
                            metadata.user_id,
                            metadata.num_input_tokens.unwrap_or_default() as i64,
                            metadata.num_input_tokens.unwrap_or_default() as i64,
                            metadata.max_output_tokens as i64,
                            metadata.max_output_tokens as i64,
                            metadata.price_per_million,
                            metadata.model_name,
                            &metadata.endpoint,
                        )?;
                    }
                }
                Ok(response.into_response())
            }
            Err(e) => {
                // Record the failed request in the image generations num requests metric
                let model: String = metadata.model_name.clone();
                match e.status_code() {
                    StatusCode::TOO_MANY_REQUESTS => {
                        TOTAL_TOO_MANY_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model)]);
                    }
                    StatusCode::BAD_REQUEST => {
                        TOTAL_BAD_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model)]);
                    }
                    StatusCode::LOCKED => {
                        TOTAL_LOCKED_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model)]);
                    }
                    StatusCode::TOO_EARLY => {
                        TOTAL_TOO_EARLY_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model)]);
                    }
                    StatusCode::UNAUTHORIZED => {
                        TOTAL_UNAUTHORIZED_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model)]);
                    }
                    _ => {
                        TOTAL_FAILED_IMAGE_GENERATION_REQUESTS
                            .add(1, &[KeyValue::new(MODEL_KEY, model.clone())]);
                        TOTAL_FAILED_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model)]);

                        UNSUCCESSFUL_IMAGE_GENERATION_REQUESTS_PER_USER
                            .add(1, &[KeyValue::new(USER_ID_KEY, metadata.user_id)]);
                    }
                }

                match metadata.selected_stack_small_id {
                    Some(stack_small_id) => {
                        update_state_manager(
                            &state.state_manager_sender,
                            stack_small_id,
                            (metadata.num_input_tokens.unwrap_or_default()
                                + metadata.max_output_tokens) as i64,
                            0,
                            &metadata.endpoint,
                        )?;
                    }
                    None => {
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

/// OpenAPI documentation for the image generations endpoint.
#[derive(OpenApi)]
#[openapi(
    paths(confidential_image_generations_create),
    components(schemas(ConfidentialComputeRequest))
)]
pub struct ConfidentialImageGenerationsOpenApi;

/// Create confidential image
///
/// This handler processes image generation requests in a confidential manner, providing additional
/// encryption and security measures for sensitive data processing. It supports both streaming and
/// non-streaming responses while maintaining data confidentiality through AEAD encryption and TEE hardware,
/// for full private AI compute.
#[utoipa::path(
    post,
    path = "",
    request_body = ConfidentialComputeRequest,
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "Image generations", body = ConfidentialComputeResponse),
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
    Json(payload): Json<ConfidentialComputeRequest>,
) -> Result<Response<Body>> {
    let endpoint = metadata.endpoint.clone();
    tokio::spawn(async move {
        // TODO: We should allow cancelling the request if the client disconnects
        let payload =
            serde_json::to_value(payload).map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to serialize payload: {e}"),
                client_message: None,
                endpoint: metadata.endpoint.clone(),
            })?;
        IMAGE_GENERATION_TOTAL_TOKENS_PER_USER.add(
            metadata.num_input_tokens.unwrap_or_default() + metadata.max_output_tokens,
            &[KeyValue::new("user_id", metadata.user_id)],
        );
        match handle_image_generation_response(
            &state,
            metadata.node_address,
            headers,
            payload,
            metadata.max_output_tokens as i64,
            metadata.endpoint.clone(),
            metadata.model_name.clone(),
        )
        .await
        {
            Ok(response) => {
                // NOTE: At this point, we do not need to update the stack num compute units,
                // because the image generation response was correctly generated by a TEE node.
                TOTAL_COMPLETED_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, metadata.model_name)]);
                SUCCESSFUL_IMAGE_GENERATION_REQUESTS_PER_USER
                    .add(1, &[KeyValue::new(USER_ID_KEY, metadata.user_id)]);
                Ok(response.into_response())
            }
            Err(e) => {
                let model: String = metadata.model_name.clone();
                match e.status_code() {
                    StatusCode::TOO_MANY_REQUESTS => {
                        TOTAL_TOO_MANY_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model)]);
                    }
                    StatusCode::BAD_REQUEST => {
                        TOTAL_BAD_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model)]);
                    }
                    StatusCode::LOCKED => {
                        TOTAL_LOCKED_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model)]);
                    }
                    StatusCode::TOO_EARLY => {
                        TOTAL_TOO_EARLY_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model)]);
                    }
                    StatusCode::UNAUTHORIZED => {
                        TOTAL_UNAUTHORIZED_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model)]);
                    }
                    _ => {
                        TOTAL_FAILED_CONFIDENTIAL_IMAGE_GENERATION_REQUESTS
                            .add(1, &[KeyValue::new(MODEL_KEY, model.clone())]);
                        TOTAL_FAILED_REQUESTS.add(1, &[KeyValue::new(MODEL_KEY, model)]);

                        UNSUCCESSFUL_IMAGE_GENERATION_REQUESTS_PER_USER
                            .add(1, &[KeyValue::new(USER_ID_KEY, metadata.user_id)]);
                    }
                }
                match metadata.selected_stack_small_id {
                    Some(stack_small_id) => {
                        update_state_manager(
                            &state.state_manager_sender,
                            stack_small_id,
                            (metadata.num_input_tokens.unwrap_or_default()
                                + metadata.max_output_tokens) as i64,
                            0,
                            &metadata.endpoint,
                        )?;
                    }
                    None => {
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
/// * `Result<Response<Body>, AtomaProxyError>` - The processed response from the AI node or an error status
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
    state: &ProxyState,
    node_address: String,
    headers: HeaderMap,
    payload: Value,
    output_tokens: i64,
    endpoint: String,
    model_name: String,
) -> Result<Response<Body>> {
    // Record the request in the image generations num requests metric
    let model_label: String = model_name.clone();
    IMAGE_GEN_NUM_REQUESTS.add(1, &[KeyValue::new("model", model_label.clone())]);

    let time = Instant::now();
    // Send the request to the AI node
    let response = state
        .client
        .post(format!("{node_address}{endpoint}"))
        .headers(headers)
        .json(&payload)
        .send()
        .await
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to send image generation request: {err:?}"),
            client_message: Some("Failed to connect to the node".to_string()),
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
            message: format!("Failed to parse image generation response: {err:?}"),
            client_message: Some("Failed to parse node's response".to_string()),
            endpoint: endpoint.to_string(),
        })
        .map(Json)?;

    let verify_hash = endpoint != CONFIDENTIAL_IMAGE_GENERATIONS_PATH;
    verify_response_hash_and_signature(&response.0, verify_hash)?;

    // Update the node throughput performance
    state
        .state_manager_sender
        .send(
            AtomaAtomaStateManagerEvent::UpdateNodeThroughputPerformance {
                timestamp: DateTime::<Utc>::from(std::time::SystemTime::now()),
                model_name,
                input_tokens: 0,
                output_tokens,
                time: time.elapsed().as_secs_f64(),
            },
        )
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to update node throughput performance: {err:?}"),
            client_message: None,
            endpoint: endpoint.to_string(),
        })?;

    IMAGE_GEN_LATENCY_METRICS.record(
        time.elapsed().as_secs_f64(),
        &[KeyValue::new("model", model_label)],
    );

    Ok(response.into_response())
}

/// Request body for image generation
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateImageRequest {
    /// A text description of the desired image(s). The maximum length is 1000 characters.
    #[schema(example = "A cute baby sea otter floating on its back")]
    pub prompt: String,

    /// The model to use for image generation.
    #[schema(example = "black-forest-labs/FLUX.1-schnell")]
    pub model: String,

    /// The number of images to generate. Defaults to 1.
    #[schema(example = 1)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub n: Option<u32>,

    /// The quality of the image that will be generated.
    /// `hd` creates images with finer details and greater consistency across the image.
    #[schema(example = "hd")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,

    /// The format in which the generated images are returned.
    #[schema(example = "url")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<String>,

    /// The size of the generated images.
    #[schema(example = "1024x1024")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,

    /// The style of the generated images.
    #[schema(example = "vivid")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,

    /// A unique identifier representing your end-user, which can help OpenAI to monitor and detect abuse.
    #[schema(example = "user-1234")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

//TODO: Add support for b64_json format
/// Response format for image generation
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateImageResponse {
    #[schema(example = 1_677_649_420)]
    pub created: i64,
    pub data: Vec<ImageData>,
}

/// Individual image data in the response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ImageData {
    #[schema(
        example = "A stunning image of a baby sea otter floating on its back in crystal clear blue water, with gentle ripples surrounding it. The otter's fur appears soft and well-detailed, and its expression is peaceful and content."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revised_prompt: Option<String>,

    #[schema(example = "https://oaidalleapiprodscus.blob.core.windows.net/private/image.png")]
    pub url: String,
}
