use atoma_state::types::NodeSubscription;
use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use tracing::{error, instrument};
use utoipa::OpenApi;

use crate::ProxyServiceState;

type Result<T> = std::result::Result<T, StatusCode>;

/// The path for the subscriptions endpoint.
pub const SUBSCRIPTIONS_PATH: &str = "/subscriptions";

/// Returns a router with the subscriptions endpoint.
///
/// # Returns
/// * `Router<ProxyServiceState>` - A router with the subscriptions endpoint
pub fn subscriptions_router() -> Router<ProxyServiceState> {
    Router::new().route(SUBSCRIPTIONS_PATH, get(get_all_subscriptions))
}

/// OpenAPI documentation for the get_all_subscriptions endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_all_subscriptions
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_all_subscriptions))]
pub struct GetAllSubscriptionsOpenApi;

/// Retrieves all subscriptions.
///
/// # Arguments
/// * `proxy_service_state` - The shared state containing the state manager
///
/// # Returns
/// * `Result<Json<Vec<NodeSubscription>>>` - A JSON response containing a list of subscriptions
///   - `Ok(Json<Vec<NodeSubscription>>)` - Successfully retrieved subscriptions
///   - `Err(StatusCode::INTERNAL_SERVER_ERROR)` - Failed to retrieve subscriptions from state manager
///
/// # Example Response
/// Returns a JSON array of NodeSubscription objects, which may include:
/// ```json
/// [
///     {
///         "node_small_id": 123,
///         "model_name": "example_model",
///         "echelon_id": 1,
///         "subscription_time": "2024-03-21T12:00:00Z"
///     }
/// ]
/// ```
#[utoipa::path(
    get,
    path = "",
    responses(
        (status = OK, description = "Retrieves all subscriptions for all nodes"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get nodes subscriptions")
    )
)]
#[instrument(level = "trace", skip_all)]
pub async fn get_all_subscriptions(
    State(proxy_service_state): State<ProxyServiceState>,
) -> Result<Json<Vec<NodeSubscription>>> {
    Ok(Json(
        proxy_service_state
            .atoma_state
            .get_all_node_subscriptions()
            .await
            .map_err(|_| {
                error!("Failed to get nodes subscriptions");
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}
