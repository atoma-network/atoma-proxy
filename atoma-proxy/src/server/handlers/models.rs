use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::instrument;
use utoipa::{OpenApi, ToSchema};

use crate::server::error::AtomaProxyError;
use crate::server::http_server::ProxyState;
use tokio::fs;

/// Path for the models listing endpoint.
///
/// This endpoint follows the OpenAI API format and returns a list
/// of available AI models with their associated metadata and capabilities.
pub const MODELS_PATH: &str = "/v1/models";

/// Path for the OpenRouter models listing endpoint.
pub const OPEN_ROUTER_MODELS_PATH: &str = "/v1/open_router/models";

/// OpenAPI documentation for the models listing endpoint.
///
/// This struct is used to generate OpenAPI documentation for the models listing
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(models_list), components(schemas(ModelList, Model)))]
pub struct ModelsOpenApi;

/// List models
///
/// This endpoint mimics the OpenAI models endpoint format, returning a list of
/// available models with their associated metadata. Each model includes standard
/// OpenAI-compatible fields to ensure compatibility with existing OpenAI client libraries.
#[utoipa::path(
    get,
    path = "",
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "List of available models", body = ModelList),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to retrieve list of available models")
    )
)]
pub async fn models_list(
    State(state): State<ProxyState>,
) -> std::result::Result<Json<ModelList>, AtomaProxyError> {
    let models = state
        .models
        .iter()
        .map(|model| Model {
            id: model.to_string(),
            object: "model".to_string(),
            created: 1_686_935_002,
            owned_by: "atoma".to_string(),
        })
        .collect();

    Ok(Json(ModelList {
        object: "list".to_string(),
        data: models,
    }))
}

/// Response object for the models listing endpoint
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ModelList {
    /// The object type, which is always "list"
    pub object: String,
    /// List of model objects
    pub data: Vec<Model>,
}

/// Individual model object in the response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct Model {
    /// The model identifier
    pub id: String,
    /// The object type, which is always "model"
    pub object: String,
    /// Unix timestamp (in seconds) when this model was created
    pub created: i64,
    /// Organization that owns the model
    pub owned_by: String,
}

/// OpenRouter models listing endpoint
///
/// This endpoint returns a list of available models from the OpenRouter
/// models file. The file is expected to be in JSON format and contains
/// information about the models, including their IDs and other metadata.
#[utoipa::path(
    get,
    path = "/v1/open_router/models",
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "List of available models", body = Value),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to retrieve list of available models")
    )
)]
#[instrument(level = "trace", skip_all)]
pub async fn open_router_models_list(
    State(state): State<ProxyState>,
) -> std::result::Result<Json<Value>, AtomaProxyError> {
    let file_content = fs::read_to_string(state.open_router_models_file)
        .await
        .map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to read OpenRouter models file: {err}"),
            client_message: None,
            endpoint: OPEN_ROUTER_MODELS_PATH.to_string(),
        })?;

    let json_data: Value =
        serde_json::from_str(&file_content).map_err(|err| AtomaProxyError::InternalError {
            message: format!("Failed to parse OpenRouter models file: {err}"),
            client_message: None,
            endpoint: OPEN_ROUTER_MODELS_PATH.to_string(),
        })?;

    Ok(Json(json_data))
}
