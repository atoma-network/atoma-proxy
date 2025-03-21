use atoma_state::types::{
    ComputedUnitsProcessedResponse, LatencyResponse, NodeDistribution, StatsStackResponse,
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use serde_json::Value;
use tracing::{error, instrument};
use utoipa::OpenApi;

use crate::{
    components::grafana::{self},
    ComputeUnitsProcessedQuery, LatencyQuery, ProxyServiceState, StatsStackQuery,
};

type Result<T> = std::result::Result<T, StatusCode>;

/// Panel response is part of the DashboardResponse.
#[derive(Serialize)]
pub struct PanelResponse {
    /// The title of the panel.
    pub title: String,
    /// Description of the panel.
    pub description: Option<String>,
    /// The unit of the panel.
    pub unit: Option<String>,
    /// The query for the panel.
    pub query: grafana::Query,
    /// The interval of the panel.
    pub interval: Option<String>,
    /// Type of the graph.
    #[serde(rename = "type")]
    pub graph_type: String,
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

/// The path for the compute_units_processed endpoint.
pub const COMPUTE_UNITS_PROCESSED_PATH: &str = "/compute_units_processed";
/// The path for the compute_units_processed endpoint.
pub const LATENCY_PATH: &str = "/latency";
/// The path for the get_stats_stacks endpoint.
pub const GET_STATS_STACKS_PATH: &str = "/get_stats_stacks";
/// The path for the get_nodes_distribution endpoint.
pub const GET_NODES_DISTRIBUTION_PATH: &str = "/get_nodes_distribution";
/// The path for the get_graphs endpoint.
pub const GET_GRAPHS_PATH: &str = "/get_graphs";
/// The path for the graph's data endpoint.
pub const GET_GRAPH_DATA_PATH: &str = "/get_graph_data";

/// Returns a router with the stats endpoint.
///
/// # Returns
/// * `Router<ProxyServiceState>` - A router with the stacks endpoint
pub fn stats_router() -> Router<ProxyServiceState> {
    Router::new()
        .route(
            COMPUTE_UNITS_PROCESSED_PATH,
            get(get_compute_units_processed),
        )
        .route(LATENCY_PATH, get(get_latency))
        .route(GET_STATS_STACKS_PATH, get(get_stats_stacks))
        .route(GET_NODES_DISTRIBUTION_PATH, get(get_nodes_distribution))
        .route(GET_GRAPHS_PATH, get(get_graphs))
        .route(GET_GRAPH_DATA_PATH, post(get_graph_data))
}

/// OpenAPI documentation for the get_compute_units_processed endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_compute_units_processed
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_compute_units_processed))]
pub struct GetComputeUnitsProcessed;

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
pub struct GetLatency;

/// Get latency performance of the network for last 'query.hours' hours. E.g. get latency performance for last 2 hours.
/// The response is vector of LatencyResponse objects.
/// For each hour the response contains sum of the latencies (in seconds) and number of requests made in that hour.
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

/// OpenAPI documentation for the get_stats_stacks endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_stats_stacks
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_stats_stacks))]
pub struct GetStatsStacks;

/// Get all stacks.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
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
async fn get_stats_stacks(
    State(proxy_service_state): State<ProxyServiceState>,
    Query(query): Query<StatsStackQuery>,
) -> Result<Json<Vec<StatsStackResponse>>> {
    Ok(Json(
        proxy_service_state
            .atoma_state
            .get_stats_stacks(query.hours)
            .await
            .map_err(|_| {
                error!("Failed to get stats for stacks");
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
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
    let uids = grafana.get_dashboard_uids().await.map_err(|e| {
        error!("Failed to get dashboard uids: {:?}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let mut results = Vec::new();
    for uid in uids {
        let dashboard = grafana.get_dashboard(uid).await.map_err(|e| {
            error!("Failed to get dashboard: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        let dashboard_title = dashboard.title();
        let queries: Vec<PanelResponse> = dashboard.into();
        results.push(DashboardResponse {
            title: dashboard_title,
            panels: queries,
        });
    }

    Ok(Json(results))
}

/// OpenAPI documentation for the get_graph_data endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_graph_data
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_graph_data))]
pub struct GetGraphData;

/// Get graph data.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `query` - The query for grafana
///
/// # Returns
///
/// * `Result<Json<Value>>` - A JSON response containing the graph data
///   - `Ok(Json<Value>)` - Successfully retrieved graph data
///   - `Err(StatusCode::INTERNAL_SERVER_ERROR)` - Failed to retrieve graph data from state manager
///
#[utoipa::path(
    post,
    path = "",
    responses(
        (status = OK, description = "Retrieves graph data", body = Value),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get graph data")
    )
)]
#[axum::debug_handler]
#[instrument(level = "trace", skip_all)]
async fn get_graph_data(
    State(proxy_service_state): State<ProxyServiceState>,
    Json(query): Json<grafana::Query>,
) -> Result<Json<Value>> {
    let grafana = &proxy_service_state.grafana;
    let data = grafana.get_query_data(query).await.map_err(|e| {
        error!("Failed to get graph data: {:?}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(data))
}
