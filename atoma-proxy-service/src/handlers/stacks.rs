use atoma_state::types::Stack;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use tracing::{error, instrument};
use utoipa::OpenApi;

use crate::ProxyServiceState;

type Result<T> = std::result::Result<T, StatusCode>;

/// The path for the get_current_stacks endpoint.
pub(crate) const GET_CURRENT_STACKS_PATH: &str = "/current_stacks";

pub(crate) const GET_ALL_STACKS_FOR_USER_PATH: &str = "/all_stacks";

/// Returns a router with the stacks endpoint.
///
/// # Returns
/// * `Router<ProxyServiceState>` - A router with the stacks endpoint
pub(crate) fn stacks_router() -> Router<ProxyServiceState> {
    Router::new()
        .route(
            &format!("{GET_CURRENT_STACKS_PATH}/:id"),
            get(get_node_stacks),
        )
        .route(GET_CURRENT_STACKS_PATH, get(get_current_stacks))
        .route(GET_ALL_STACKS_FOR_USER_PATH, get(get_all_stacks_for_user))
}

/// OpenAPI documentation for the get_current_stacks endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_current_stacks
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_current_stacks))]
pub(crate) struct GetCurrentStacksOpenApi;

/// Retrieves all stacks that are not settled.
///
/// # Arguments
/// * `proxy_service_state` - The shared state containing the state manager
///
/// # Returns
/// * `Result<Json<Vec<Stack>>>` - A JSON response containing a list of stacks
///   - `Ok(Json<Vec<Stack>>)` - Successfully retrieved stacks
///   - `Err(StatusCode::INTERNAL_SERVER_ERROR)` - Failed to retrieve stacks from state manager
#[utoipa::path(
    get,
    path = "",
    responses(
        (status = OK, description = "Retrieves all stacks that are not settled"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get all stacks")
    )
)]
#[instrument(level = "trace", skip_all)]
pub(crate) async fn get_current_stacks(
    State(proxy_service_state): State<ProxyServiceState>,
) -> Result<Json<Vec<Stack>>> {
    Ok(Json(
        proxy_service_state
            .atoma_state
            .get_current_stacks()
            .await
            .map_err(|_| {
                error!("Failed to get all stacks");
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}

/// Retrieves all stacks for a specific node identified by its small ID.
///
/// # Arguments
/// * `proxy_service_state` - The shared state containing the state manager
/// * `node_small_id` - The small ID of the node whose stacks should be retrieved
///
/// # Returns
/// * `Result<Json<Vec<Stack>>>` - A JSON response containing a list of stacks
///   - `Ok(Json<Vec<Stack>>)` - Successfully retrieved stacks
///   - `Err(StatusCode::INTERNAL_SERVER_ERROR)` - Failed to retrieve stacks from state manager
///
/// # Example Response
/// Returns a JSON array of Stack objects for the specified node
#[utoipa::path(
    get,
    path = "/{node_small_id}",
    params(
        ("node_small_id" = i64, description = "The small ID of the node whose stacks should be retrieved")
    ),
    responses(
        (status = OK, description = "Retrieves all stacks for a specific node"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get node stack")
    )
)]
#[instrument(level = "trace", skip_all)]
pub(crate) async fn get_node_stacks(
    State(proxy_service_state): State<ProxyServiceState>,
    Path(node_small_id): Path<i64>,
) -> Result<Json<Vec<Stack>>> {
    Ok(Json(
        proxy_service_state
            .atoma_state
            .get_stack_by_id(node_small_id)
            .await
            .map_err(|_| {
                error!("Failed to get node stack");
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}

/// OpenAPI documentation for the get_stacks_by_user_id endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_stacks_by_user_id
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_all_stacks_for_user))]
pub(crate) struct GetStacksByUserId;

/// Retrieves all stacks for the user based on the access token.
///
/// # Arguments
/// * `proxy_service_state` - The shared state containing the state manager
/// * `headers` - The headers of the request
///
/// # Returns
/// * `Result<Json<Vec<Stack>>>` - A JSON response containing a list of stacks
///   - `Ok(Json<Vec<Stack>>)` - Successfully retrieved stacks
///   - `Err(StatusCode::INTERNAL_SERVER_ERROR)` - Failed to retrieve stacks from state manager
///
/// # Example Response
/// Returns a JSON array of Stack objects for the specified user
#[utoipa::path(
    get,
    path = "",
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "Retrieves all stacks for the user"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get user stack")
    )
)]
#[instrument(level = "trace", skip_all)]
pub(crate) async fn get_all_stacks_for_user(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Stack>>> {
    let auth_header = headers
        .get("Authorization")
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_str()
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let jwt = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let user_id = proxy_service_state
        .auth
        .get_user_id_from_token(jwt)
        .await
        .map_err(|_| {
            error!("Failed to get user ID from token");
            StatusCode::UNAUTHORIZED
        })?;
    Ok(Json(
        proxy_service_state
            .atoma_state
            .get_stacks_by_user_id(user_id)
            .await
            .map_err(|_| {
                error!("Failed to get user stack");
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}
