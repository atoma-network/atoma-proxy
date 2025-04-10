use atoma_state::types::{NodeSubscription, Task};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use tracing::{error, instrument};
use utoipa::OpenApi;

use crate::{ModelModality, ProxyServiceState};

type Result<T> = std::result::Result<T, StatusCode>;
type TasksWithModalities = Vec<(Task, Vec<ModelModality>)>;

/// The path for the tasks endpoint.
pub const TASKS_PATH: &str = "/tasks";

/// Returns a router with the tasks endpoint.
///
/// # Returns
/// * `Router<ProxyServiceState>` - A router with the tasks endpoint
pub fn tasks_router() -> Router<ProxyServiceState> {
    Router::new()
        .route(&format!("{TASKS_PATH}/{{id}}"), get(get_nodes_for_tasks))
        .route(TASKS_PATH, get(get_all_tasks))
}

/// Retrieves all subscriptions for a specific task identified by its task_id.
///
/// # Arguments
/// * `proxy_service_state` - The shared state containing the state manager
/// * `task_id` - The ID of the task whose subscriptions should be retrieved
///
/// # Returns
/// * `Result<Json<Vec<NodeSubscription>>` - A JSON response containing a list of subscriptions
///   - `Ok(Json<Vec<NodeSubscription>>)` - Successfully retrieved subscriptions
///   - `Err(StatusCode::INTERNAL_SERVER_ERROR)` - Failed to retrieve subscriptions from state manager
///
/// # Example Response
/// Returns a JSON array of NodeSubscription objects for the specified task, which may include:
/// ```json
/// [
///    {
///       "node_small_id": 123,
///       "model_name": "example_model",
///       "echelon_id": 1,
///       "subscription_time": "2024-03-21T12:00:00Z"
/// }
/// ]
/// ```
#[utoipa::path(
    get,
    path = "/{task_id}",
    params(
        ("task_id" = i64, description = "The ID of the task whose subscriptions should be retrieved")
    ),
    responses(
        (status = OK, description = "Retrieves all subscriptions for a specific task"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get node subscriptions")
    )
)]
#[instrument(level = "trace", skip_all)]
pub async fn get_nodes_for_tasks(
    State(proxy_service_state): State<ProxyServiceState>,
    Path(task_id): Path<i64>,
) -> Result<Json<Vec<NodeSubscription>>> {
    Ok(Json(
        proxy_service_state
            .atoma_state
            .get_all_node_subscriptions_for_task(task_id)
            .await
            .map_err(|_| {
                error!("Failed to get node subscriptions");
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}

/// OpenAPI documentation for the get_all_tasks endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_all_tasks
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_all_tasks))]
pub struct GetAllTasksOpenApi;

/// Retrieves all tasks from the state manager.
///
/// # Arguments
/// * `proxy_service_state` - The shared state containing the state manager
///
/// # Returns
/// * `Result<Json<Vec<Task>>>` - A JSON response containing a list of tasks
///   - `Ok(Json<Vec<Task>>)` - Successfully retrieved tasks
///   - `Err(StatusCode::INTERNAL_SERVER_ERROR)` - Failed to retrieve tasks from state manager
///
/// # Example Response
/// Returns a JSON array of Task objects representing all tasks in the system
#[utoipa::path(
    get,
    path = "",
    responses(
        (status = OK, description = "Retrieves all tasks"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get all tasks")
    )
)]
#[instrument(level = "trace", skip_all)]
pub async fn get_all_tasks(
    State(proxy_service_state): State<ProxyServiceState>,
) -> Result<Json<TasksWithModalities>> {
    let all_tasks = proxy_service_state
        .atoma_state
        .get_all_tasks()
        .await
        .map_err(|_| {
            error!("Failed to get all tasks");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(
        all_tasks
            .into_iter()
            .map(|task| {
                (
                    task.clone(),
                    task.model_name
                        .and_then(|model| proxy_service_state.models_with_modalities.get(&model))
                        .cloned()
                        .unwrap_or_default(),
                )
            })
            .collect(),
    ))
}
