use std::time::Instant;

use atoma_state::types::AtomaAtomaStateManagerEvent;
use axum::{
    body::Body,
    extract::State,
    http::HeaderMap,
    response::{IntoResponse, Response},
    Extension, Json,
};
use base64::engine::{general_purpose::STANDARD, Engine};
use opentelemetry::KeyValue;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::types::chrono::{DateTime, Utc};
use tokenizers::Tokenizer;
use tracing::instrument;
use utoipa::{OpenApi, ToSchema};

use crate::server::{
    error::AtomaProxyError,
    http_server::ProxyState,
    middleware::RequestMetadataExtension,
    types::{ConfidentialComputeRequest, ConfidentialComputeResponse},
    MODEL, ONE_MILLION,
};

use super::{
    handle_status_code_error,
    metrics::{
        EMBEDDING_TOTAL_TOKENS_PER_USER, SUCCESSFUL_TEXT_EMBEDDING_REQUESTS_PER_USER,
        TEXT_EMBEDDINGS_LATENCY_METRICS, TEXT_EMBEDDINGS_NUM_REQUESTS, TOTAL_COMPLETED_REQUESTS,
        TOTAL_FAILED_REQUESTS, TOTAL_FAILED_TEXT_EMBEDDING_REQUESTS,
        UNSUCCESSFUL_TEXT_EMBEDDING_REQUESTS_PER_USER,
    },
    request_model::{ComputeUnitsEstimate, RequestModel},
    update_state_manager, update_state_manager_fiat, verify_response_hash_and_signature,
    RESPONSE_HASH_KEY,
};
use crate::server::Result;

/// Path for the confidential embeddings endpoint.
///
/// This endpoint follows the OpenAI API format for embeddings, with additional
/// confidential processing (through AEAD encryption and TEE hardware).
pub const CONFIDENTIAL_EMBEDDINGS_PATH: &str = "/v1/confidential/embeddings";

/// Path for the embeddings endpoint.
///
/// This endpoint follows the OpenAI API format for embeddings
/// and is used to generate vector embeddings for input text.
pub const EMBEDDINGS_PATH: &str = "/v1/embeddings";

/// The input field in the request payload.
const INPUT: &str = "input";

// A model representing an embeddings request payload.
///
/// This struct encapsulates the necessary fields for processing an embeddings request
/// following the OpenAI API format.
#[derive(Debug, Clone)]
pub struct RequestModelEmbeddings {
    /// The name of the model to use for generating embeddings (e.g., "text-embedding-ada-002")
    model: String,
    /// The input text to generate embeddings for
    input: String,
}

/// OpenAPI documentation for the embeddings endpoint.
#[derive(OpenApi)]
#[openapi(
    paths(embeddings_create),
    components(schemas(
        CreateEmbeddingRequest,
        EmbeddingObject,
        EmbeddingUsage,
        CreateEmbeddingResponse
    ))
)]
pub struct EmbeddingsOpenApi;

impl RequestModel for RequestModelEmbeddings {
    fn new(request: &Value) -> Result<Self> {
        let model = request.get(MODEL).and_then(|m| m.as_str()).ok_or_else(|| {
            AtomaProxyError::RequestError {
                message: "Model field is required".to_string(),
                endpoint: EMBEDDINGS_PATH.to_string(),
            }
        })?;
        let input = request.get(INPUT).and_then(|i| i.as_str()).ok_or_else(|| {
            AtomaProxyError::RequestError {
                message: "Input field is required".to_string(),
                endpoint: EMBEDDINGS_PATH.to_string(),
            }
        })?;

        Ok(Self {
            model: model.to_string(),
            input: input.to_string(),
        })
    }

    fn get_model(&self) -> String {
        self.model.clone()
    }

    fn get_compute_units_estimate(
        &self,
        tokenizer: Option<&Tokenizer>,
    ) -> Result<ComputeUnitsEstimate> {
        let Some(tokenizer) = tokenizer else {
            return Err(AtomaProxyError::InternalError {
                client_message: Some("No available tokenizer found for current model, try again later or open a ticket".to_string()),
                message: "Tokenizer not found for current model".to_string(),
                endpoint: EMBEDDINGS_PATH.to_string(),
            });
        };
        let num_tokens = tokenizer
            .encode(self.input.as_str(), true)
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to encode input: {err:?}"),
                client_message: Some(
                    "Failed to encode message using the model's tokenizer".to_string(),
                ),
                endpoint: EMBEDDINGS_PATH.to_string(),
            })?
            .get_ids()
            .len() as u64;
        Ok(ComputeUnitsEstimate {
            num_input_compute_units: num_tokens,
            max_total_compute_units: num_tokens,
        })
    }
}

/// Create embeddings
///
/// This endpoint follows the OpenAI API format for generating vector embeddings from input text.
/// The handler receives pre-processed metadata from middleware and forwards the request to
/// the selected node.
///
/// # Returns
/// * `Ok(Response)` - The embeddings response from the processing node
/// * `Err(AtomaProxyError)` - An error status code if any step fails
///
/// ## Errors
/// * `INTERNAL_SERVER_ERROR` - Processing or node communication failures
#[utoipa::path(
    post,
    path = "",
    request_body = CreateEmbeddingRequest,
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "Embeddings generated successfully", body = CreateEmbeddingResponse),
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
pub async fn embeddings_create(
    Extension(metadata): Extension<RequestMetadataExtension>,
    State(state): State<ProxyState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<Response<Body>> {
    let endpoint = metadata.endpoint.clone();
    tokio::spawn(async move {
        // TODO: We should allow cancelling the request if the client disconnects
        let RequestMetadataExtension {
            node_address,
            max_total_num_compute_units: num_input_compute_units,
            ..
        } = metadata;
        EMBEDDING_TOTAL_TOKENS_PER_USER.add(
            num_input_compute_units,
            &[KeyValue::new("user_id", metadata.user_id)],
        );
        match handle_embeddings_response(
            &state,
            node_address,
            headers,
            payload,
            num_input_compute_units as i64,
            metadata.endpoint.clone(),
            metadata.model_name.clone(),
            metadata.selected_stack_small_id,
        )
        .await
        {
            Ok(response) => {
                TOTAL_COMPLETED_REQUESTS.add(1, &[KeyValue::new("model", metadata.model_name)]);
                SUCCESSFUL_TEXT_EMBEDDING_REQUESTS_PER_USER
                    .add(1, &[KeyValue::new("user_id", metadata.user_id)]);
                Ok(Json(response).into_response())
            }
            Err(e) => {
                let model_label: String = metadata.model_name.clone();
                TOTAL_FAILED_REQUESTS.add(1, &[KeyValue::new("model", model_label.clone())]);
                TOTAL_FAILED_TEXT_EMBEDDING_REQUESTS.add(1, &[KeyValue::new("model", model_label)]);
                UNSUCCESSFUL_TEXT_EMBEDDING_REQUESTS_PER_USER
                    .add(1, &[KeyValue::new("user_id", metadata.user_id)]);
                match metadata.selected_stack_small_id {
                    Some(stack_small_id) => {
                        update_state_manager(
                            &state.state_manager_sender,
                            stack_small_id,
                            num_input_compute_units as i64,
                            0,
                            &metadata.endpoint,
                        )?;
                    }
                    None => {
                        update_state_manager_fiat(
                            &state.state_manager_sender,
                            metadata.user_id,
                            metadata.fiat_estimated_amount.unwrap_or_default(),
                            0,
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

/// Atoma's confidential embeddings OpenAPI documentation.
#[derive(OpenApi)]
#[openapi(
    paths(confidential_embeddings_create),
    components(schemas(ConfidentialComputeRequest,))
)]
pub struct ConfidentialEmbeddingsOpenApi;

/// Create confidential embeddings
///
/// This endpoint follows the OpenAI API format for generating vector embeddings from input text,
/// but with confidential processing (through AEAD encryption and TEE hardware).
/// The handler receives pre-processed metadata from middleware and forwards the request to
/// the selected node.
///
/// ## Returns
/// * `Ok(Response)` - The embeddings response from the processing node
/// * `Err(AtomaProxyError)` - An error status code if any step fails
///
/// ## Errors
/// * `INTERNAL_SERVER_ERROR` - Processing or node communication failures
#[utoipa::path(
    post,
    path = "",
    request_body = ConfidentialComputeRequest,
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "Confidential embeddings generated successfully", body = ConfidentialComputeResponse),
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
pub async fn confidential_embeddings_create(
    Extension(metadata): Extension<RequestMetadataExtension>,
    State(state): State<ProxyState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<Response<Body>> {
    let endpoint = metadata.endpoint.clone();
    tokio::spawn(async move {
        // TODO: We should allow cancelling the request if the client disconnects
        let RequestMetadataExtension {
            node_address,
            max_total_num_compute_units: num_input_compute_units,
            ..
        } = metadata;
        EMBEDDING_TOTAL_TOKENS_PER_USER.add(
            num_input_compute_units,
            &[KeyValue::new("user_id", metadata.user_id)],
        );
        match handle_embeddings_response(
            &state,
            node_address,
            headers,
            payload,
            num_input_compute_units as i64,
            metadata.endpoint.clone(),
            metadata.model_name.clone(),
            metadata.selected_stack_small_id,
        )
        .await
        {
            Ok(response) => {
                // NOTE: In this case, we can safely assume that the response is a well-formed JSON object
                // with a "total_tokens" field, which correctly specifies the number of total tokens
                // processed by the node, as the latter is running within a TEE.
                let total_tokens = response
                    .get("usage")
                    .and_then(|usage| usage.get("total_tokens"))
                    .and_then(serde_json::Value::as_u64)
                    .map_or(0, |n| n as i64);
                match metadata.selected_stack_small_id {
                    Some(stack_small_id) => {
                        update_state_manager(
                            &state.state_manager_sender,
                            stack_small_id,
                            num_input_compute_units as i64,
                            total_tokens,
                            &metadata.endpoint,
                        )?;
                    }
                    None => {
                        update_state_manager_fiat(
                            &state.state_manager_sender,
                            metadata.user_id,
                            metadata.fiat_estimated_amount.unwrap_or_default(),
                            total_tokens * metadata.price_per_million.unwrap_or_default()
                                / ONE_MILLION as i64,
                            &metadata.endpoint,
                        )?;
                    }
                }
                TOTAL_COMPLETED_REQUESTS.add(1, &[KeyValue::new("model", metadata.model_name)]);
                SUCCESSFUL_TEXT_EMBEDDING_REQUESTS_PER_USER
                    .add(1, &[KeyValue::new("user_id", metadata.user_id)]);
                Ok(Json(response).into_response())
            }
            Err(e) => {
                let model_label: String = metadata.model_name.clone();
                TOTAL_FAILED_REQUESTS.add(1, &[KeyValue::new("model", model_label.clone())]);
                TOTAL_FAILED_TEXT_EMBEDDING_REQUESTS.add(1, &[KeyValue::new("model", model_label)]);
                UNSUCCESSFUL_TEXT_EMBEDDING_REQUESTS_PER_USER
                    .add(1, &[KeyValue::new("user_id", metadata.user_id)]);

                match metadata.selected_stack_small_id {
                    Some(stack_small_id) => {
                        update_state_manager(
                            &state.state_manager_sender,
                            stack_small_id,
                            num_input_compute_units as i64,
                            0,
                            &metadata.endpoint,
                        )?;
                    }
                    None => {
                        update_state_manager_fiat(
                            &state.state_manager_sender,
                            metadata.user_id,
                            metadata.fiat_estimated_amount.unwrap_or_default(),
                            0,
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
/// * `Err(AtomaProxyError)` - An error status code if any step fails
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
        path = endpoint,
        stack_small_id,
        estimated_total_tokens
    )
)]
#[allow(clippy::too_many_arguments)]
async fn handle_embeddings_response(
    state: &ProxyState,
    node_address: String,
    headers: HeaderMap,
    payload: Value,
    num_input_compute_units: i64,
    endpoint: String,
    model_name: String,
    stack_small_id: Option<i64>,
) -> Result<Value> {
    // Record the request in the total text embedding requests metric
    let model_label: String = model_name.clone();

    // Record the request in the total failed requests metric
    TEXT_EMBEDDINGS_NUM_REQUESTS.add(1, &[KeyValue::new("model", model_label.clone())]);

    let client = reqwest::Client::new();
    let time = Instant::now();
    // Send the request to the AI node
    let response = client
        .post(format!("{node_address}{endpoint}"))
        .headers(headers)
        .json(&payload)
        .send()
        .await
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to send embeddings request: {err:?}"),
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

    let response =
        response
            .json::<Value>()
            .await
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to parse embeddings response: {err:?}"),
                client_message: Some("Failed to parse the node response".to_string()),
                endpoint: endpoint.to_string(),
            })?;

    let verify_hash = endpoint != CONFIDENTIAL_EMBEDDINGS_PATH;
    verify_response_hash_and_signature(&response, verify_hash)?;

    let num_input_compute_units = if endpoint == CONFIDENTIAL_EMBEDDINGS_PATH {
        response
            .get("total_tokens")
            .map_or(0, |u| u.as_u64().unwrap_or(0)) as i64
    } else {
        num_input_compute_units
    };

    // Update the node throughput performance
    state
        .state_manager_sender
        .send(
            AtomaAtomaStateManagerEvent::UpdateNodeThroughputPerformance {
                timestamp: DateTime::<Utc>::from(std::time::SystemTime::now()),
                model_name,
                input_tokens: num_input_compute_units,
                output_tokens: 0,
                time: time.elapsed().as_secs_f64(),
            },
        )
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to update node throughput performance: {err:?}"),
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

    if let Some(stack_small_id) = stack_small_id {
        state
            .state_manager_sender
            .send(AtomaAtomaStateManagerEvent::UpdateStackTotalHash {
                stack_small_id,
                total_hash,
            })
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Error updating stack total hash: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;
    }

    TEXT_EMBEDDINGS_LATENCY_METRICS.record(
        time.elapsed().as_secs_f64(),
        &[KeyValue::new("model", model_label)],
    );

    Ok(response)
}

/// Request object for creating embeddings
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateEmbeddingRequest {
    /// ID of the model to use.
    #[schema(example = "intfloat/multilingual-e5-large-instruct")]
    pub model: String,

    /// Input text to get embeddings for. Can be a string or array of strings.
    /// Each input must not exceed the max input tokens for the model
    pub input: EmbeddingInput,

    /// A unique identifier representing your end-user, which can help OpenAI to monitor and detect abuse.
    #[schema(example = "user-1234")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,

    /// The format to return the embeddings in. Can be "float" or "base64".
    /// Defaults to "float"
    #[schema(example = "float")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encoding_format: Option<String>,

    /// The number of dimensions the resulting output embeddings should have.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum EmbeddingInput {
    #[schema(example = "The quick brown fox jumped over the lazy dog")]
    Single(String),
    #[schema(example = "[\"The quick brown fox\", \"jumped over the lazy dog\"]")]
    Multiple(Vec<String>),
}

/// Response object from creating embeddings
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreateEmbeddingResponse {
    /// The object type, which is always "list"
    #[schema(example = "list")]
    pub object: String,

    /// The model used for generating embeddings
    #[schema(example = "intfloat/multilingual-e5-large-instruct")]
    pub model: String,

    /// List of embedding objects
    pub data: Vec<EmbeddingObject>,

    /// Usage statistics for the request
    pub usage: EmbeddingUsage,
}

/// Individual embedding object in the response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EmbeddingObject {
    /// The object type, which is always "embedding"
    #[schema(example = "embedding")]
    pub object: String,

    /// The embedding vector
    #[schema(example = "[0.0023064255, -0.009327292]")]
    pub embedding: Vec<f32>,

    /// Index of the embedding in the list of embeddings
    #[schema(example = 0)]
    pub index: usize,
}

/// Usage information for the embeddings request
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EmbeddingUsage {
    /// Number of tokens in the prompt
    #[schema(example = 8)]
    pub prompt_tokens: u32,

    /// Total tokens used in the request
    #[schema(example = 8)]
    pub total_tokens: u32,
}
