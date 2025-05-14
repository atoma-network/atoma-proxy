use atoma_state::types::NodeDistribution;
use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;
use serde_json::Value;
use tracing::{error, instrument};
use utoipa::OpenApi;

use crate::{
    components::grafana::{self},
    ProxyServiceState,
};

type Result<T> = std::result::Result<T, StatusCode>;

/// Panel response is part of the DashboardResponse.
#[derive(Serialize)]
pub struct PanelResponse {
    /// The title of the panel.
    pub title: String,
    /// Description of the panel.
    pub description: Option<String>,
    /// The fieldConfig of the panel.
    pub field_config: Value,
    /// The query for the panel.
    #[serde(skip)]
    pub query: grafana::Query,
    /// The interval of the panel.
    #[serde(skip)]
    pub interval: Value,
    /// The data of the panel.
    pub data: Value,
    /// Type of the graph.
    #[serde(rename = "type")]
    pub graph_type: String,
    /// Start time of the panel.
    pub from: String,
}

/// Dashboard response is part of the GraphsResponse.
#[derive(Serialize)]
pub struct DashboardResponse {
    /// The title of the dashboard.
    pub title: String,
    /// The panels of the dashboard.
    pub panels: Vec<PanelResponse>,
}

/// Response for getting grafana graphs.
///
/// This struct represents the response for the get_grafana_graphs endpoint.
/// It's vector of tuples where the first element is the name of the dashboard and the second element tuple of panels.
/// Each panel has a title and a graph data.
pub type GraphsResponse = Vec<DashboardResponse>;

/// The path for the get_nodes_distribution endpoint.
pub const GET_NODES_DISTRIBUTION_PATH: &str = "/get_nodes_distribution";
/// The path for the get_graphs endpoint.
pub const GET_GRAPHS_PATH: &str = "/get_graphs";
/// The path for the get_stats endpoint.
pub const GET_STATS_PATH: &str = "/get_stats";

/// Returns a router with the stats endpoint.
///
/// # Returns
/// * `Router<ProxyServiceState>` - A router with the stacks endpoint
pub fn stats_router() -> Router<ProxyServiceState> {
    Router::new()
        .route(GET_NODES_DISTRIBUTION_PATH, get(get_nodes_distribution))
        .route(GET_GRAPHS_PATH, get(get_graphs))
        .route(GET_STATS_PATH, get(get_stats))
}

/// OpenAPI documentation for the get_nodes_distribution endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_nodes_distribution
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_nodes_distribution))]
pub struct GetNodeDistribution;

/// Get nodes distribution.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
///
/// # Returns
///
/// * `Result<Json<Vec<NodeDistribution>>` - A JSON response containing a list of nodes distribution
///  - `Ok(Json<Vec<NodeDistribution>>)` - Successfully retrieved nodes distribution
/// - `Err(StatusCode::INTERNAL_SERVER_ERROR)` - Failed to retrieve nodes distribution from state manager
///
/// # Example Response
///
/// Returns a JSON array of nodes distribution
/// ```json
/// [
///  {
///   "country": "US",
///   "count": 2
///  }
/// ]
/// ```
#[utoipa::path(
    get,
    path = "",
    responses(
        (status = OK, description = "Retrieves nodes distribution", body = Value),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get node distribution")
    )
)]
#[instrument(level = "trace", skip_all)]
async fn get_nodes_distribution(
    State(proxy_service_state): State<ProxyServiceState>,
) -> Result<Json<Vec<NodeDistribution>>> {
    Ok(Json(
        proxy_service_state
            .atoma_state
            .get_nodes_distribution()
            .await
            .map_err(|_| {
                error!("Failed to get nodes distribution");
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}

/// OpenAPI documentation for the get_graphs endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_graphs
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_graphs))]
pub struct GetGraphs;

/// Get graphs.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
///
/// # Returns
///
/// * `Result<Json<Value>>` - A JSON response containing a list of graphs
///   - `Ok(Json<Value>)` - Successfully retrieved graphs
///   - `Err(StatusCode::INTERNAL_SERVER_ERROR)` - Failed to retrieve graphs from state manager
#[utoipa::path(
    get,
    path = "",
    responses(
        (status = OK, description = "Retrieves all graphs", body = Value),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get graphs")
    )
)]
#[instrument(level = "trace", skip_all)]
async fn get_graphs(
    State(proxy_service_state): State<ProxyServiceState>,
) -> Result<Json<GraphsResponse>> {
    let grafana = &proxy_service_state.grafana;
    Ok(Json(
        grafana
            .get_dashboards(proxy_service_state.dashboard_tag)
            .await
            .map_err(|e| {
                error!("Failed to get dashboard uids: {:?}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}

#[derive(OpenApi)]
#[openapi(paths(get_stats))]
pub struct GetStats;

/// Get stats.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// # Returns
///
/// * `Result<Json<Value>>` - A JSON response containing a list of graphs
///   - `Ok(Json<Value>)` - Successfully retrieved graphs
///   - `Err(StatusCode::INTERNAL_SERVER_ERROR)` - Failed to retrieve graphs from state manager
#[utoipa::path(
    get,
    path = "",
    responses(
        (status = OK, description = "Retrieves all graphs", body = Value),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get graphs")
    )
)]
#[instrument(level = "trace", skip_all)]
async fn get_stats(
    State(proxy_service_state): State<ProxyServiceState>,
) -> Result<Json<GraphsResponse>> {
    let grafana = &proxy_service_state.grafana;
    Ok(Json(
        grafana
            .get_dashboards(proxy_service_state.stats_tag)
            .await
            .map_err(|e| {
                error!("Failed to get dashboard uids: {:?}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}
