use atoma_state::types::SetPriceForUserForModel;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use reqwest::header;
use tracing::{error, instrument};
use utoipa::OpenApi;

use crate::ProxyServiceState;

type Result<T> = std::result::Result<T, StatusCode>;

/// The path for the settings endpoint.
pub const SET_PRICE_FOR_USER_PATH: &str = "/set_pricing_for_user";

/// Returns a router with the tasks endpoint.
///
/// # Returns
/// * `Router<ProxyServiceState>` - A router with the settings endpoint
pub fn settings_router() -> Router<ProxyServiceState> {
    Router::new().route(SET_PRICE_FOR_USER_PATH, post(set_pricing_for_user))
}

/// OpenAPI documentation for the set_pricing_for_user endpoint.
///
/// This struct is used to generate OpenAPI documentation for the set_pricing_for_user
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(set_pricing_for_user))]
pub struct SetPricingForUserOpenApi;

/// Sets the pricing for a user for a specific model.
///
/// # Arguments
///
/// * `State(proxy_service_state)` - The state of the proxy service.
/// * `body` - The JSON body containing the user ID, model name, and pricing information.
///
/// # Returns
///
/// * `Result<Json<()>>` - A result containing an empty JSON response on success,
///   or an error status code on failure.
///
/// # Errors
/// * Returns `StatusCode::INTERNAL_SERVER_ERROR` if the pricing update fails.
#[utoipa::path(
    post,
    path = "",
    responses(
        (status = OK, description = "Pricing updated successfully"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to update pricing")
    )
)]
#[instrument(level = "trace", skip_all)]
pub async fn set_pricing_for_user(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
    body: Json<SetPriceForUserForModel>,
) -> Result<Json<()>> {
    let bearer_token = headers
        .get(header::AUTHORIZATION)
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_str()
        .map_err(|_| StatusCode::UNAUTHORIZED)?
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if bearer_token != proxy_service_state.settings_password {
        error!("Invalid password for settings endpoint");
        return Err(StatusCode::UNAUTHORIZED);
    }
    proxy_service_state
        .atoma_state
        .set_custom_pricing(
            body.user_id,
            &body.model_name,
            body.price_per_one_million_input_compute_units,
            body.price_per_one_million_output_compute_units,
        )
        .await
        .map_err(|_| {
            error!("Failed to update user pricing");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(()))
}
