use atoma_state::types::{ComputedUnitsProcessedResponse, LatencyResponse};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use tracing::{error, instrument};
use utoipa::OpenApi;

use crate::{ComputeUnitsProcessedQuery, LatencyQuery, ProxyServiceState};

type Result<T> = std::result::Result<T, StatusCode>;

/// The path for the compute_units_processed endpoint.
pub(crate) const COMPUTE_UNITS_PROCESSED_PATH: &str = "/compute_units_processed";
/// The path for the compute_units_processed endpoint.
pub(crate) const LATENCY_PATH: &str = "/latency";

/// Returns a router with the stats endpoint.
///
/// # Returns
/// * `Router<ProxyServiceState>` - A router with the stacks endpoint
pub(crate) fn stats_router() -> Router<ProxyServiceState> {
    Router::new()
        .route(
            COMPUTE_UNITS_PROCESSED_PATH,
            get(get_compute_units_processed),
        )
        .route(LATENCY_PATH, get(get_latency))
}

/// OpenAPI documentation for the get_compute_units_processed endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_compute_units_processed
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_compute_units_processed))]
pub(crate) struct GetComputeUnitsProcessed;

/// Get compute unit processed in the last `hours` hours per model.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `query` - The query containing the number of hours to look back
///
/// # Returns
///
/// * `Result<Json<Vec<ComputedUnitsProcessedResponse>>` - A JSON response containing a list of computed units processed
///   - `Ok(Json<Vec<ComputedUnitsProcessedResponse>>)` - Successfully retrieved computed units processed
///   - `Err(StatusCode::INTERNAL_SERVER_ERROR)` - Failed to retrieve computed units processed from state manager
///
/// # Example Response
///
/// Returns a JSON array of ComputedUnitsProcessedResponse objects for the specified hours
/// ```json
/// [
///    {
///        timestamp: "2024-03-21T12:00:00Z",
///        model_name: "example_model",
///        amount: 123,
///        requests: 2,
///        time: 45
///   }
///]
///```
#[utoipa::path(
  get,
  path = "",
  responses(
      (status = OK, description = "Retrieves all computed units processed", body = Value),
      (status = INTERNAL_SERVER_ERROR, description = "Failed to get performance")
  )
)]
#[instrument(level = "trace", skip_all)]
async fn get_compute_units_processed(
    State(proxy_service_state): State<ProxyServiceState>,
    Query(query): Query<ComputeUnitsProcessedQuery>,
) -> Result<Json<Vec<ComputedUnitsProcessedResponse>>> {
    Ok(Json(
        proxy_service_state
            .atoma_state
            .get_compute_units_processed(query.hours)
            .await
            .map_err(|_| {
                error!("Failed to get performance");
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}

/// OpenAPI documentation for the get_latency endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_latency
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_latency))]
pub(crate) struct GetLatency;

/// Get latency performance in the last `hours` hours.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `query` - The query containing the number of hours to look back
///
/// # Returns
///
/// * `Result<Json<Vec<LatencyResponse>>` - A JSON response containing a list of latency performance
///   - `Ok(Json<Vec<LatencyResponse>>)` - Successfully retrieved latency performance
///   - `Err(StatusCode::INTERNAL_SERVER_ERROR)` - Failed to retrieve latency performance from state manager
///
/// # Example Response
///
/// Returns a JSON array of LatencyResponse objects for the specified hours
/// ```json
/// [
///   {
///      timestamp: "2024-03-21T12:00:00Z",
///      latency: 123,
///      requests: 2,
///      time: 45
///   }
/// ]
/// ```
#[utoipa::path(
  get,
  path = "",
  responses(
      (status = OK, description = "Retrieves all latency performance", body = Value),
      (status = INTERNAL_SERVER_ERROR, description = "Failed to get performance")
  )
)]
#[instrument(level = "trace", skip_all)]
async fn get_latency(
    State(proxy_service_state): State<ProxyServiceState>,
    Query(query): Query<LatencyQuery>,
) -> Result<Json<Vec<LatencyResponse>>> {
    Ok(Json(
        proxy_service_state
            .atoma_state
            .get_latency_performance(query.hours)
            .await
            .map_err(|_| {
                error!("Failed to get performance");
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}
