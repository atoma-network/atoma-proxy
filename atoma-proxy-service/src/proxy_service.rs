use atoma_auth::Auth;
use atoma_state::{
    types::{AuthRequest, AuthResponse, NodeSubscription, RevokeApiTokenRequest, Stack, Task},
    AtomaState,
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, Method, StatusCode},
    routing::{get, post},
    Json, Router,
};

use tokio::{net::TcpListener, sync::watch::Receiver};
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, instrument};

type Result<T> = std::result::Result<T, StatusCode>;

/// State container for the Atoma proxy service that manages node operations and interactions.
///
/// The `ProxyServiceState` struct serves as the central state management component for the Atoma proxy service,
/// containing essential components for interacting with the Sui blockchain and managing node state.
/// It is designed to be shared across multiple request handlers and maintains thread-safe access
/// to shared resources.
///
/// # Thread Safety
///
/// This struct is designed to be safely shared across multiple threads:
/// - Implements `Clone` for easy sharing across request handlers
/// - Uses `Arc<RwLock>` for thread-safe access to the Sui client
/// - State manager and node badges vector use interior mutability patterns
///
/// # Example
///
/// ```rust,ignore
/// // Create a new proxy_service state instance
/// let proxy_service_state = ProxyServiceState {
///     client: Arc::new(RwLock::new(AtomaSuiClient::new())),
///     state_manager: AtomaStateManager::new(),
///     node_badges: vec![(ObjectID::new([0; 32]), 1)],
/// };
///
/// // Clone the state for use in different handlers
/// let handler_state = proxy_service_state.clone();
/// ```
#[derive(Clone)]
pub struct ProxyServiceState {
    /// Manages the persistent state of nodes, tasks, and other system components.
    /// Handles database operations and state synchronization.
    pub atoma_state: AtomaState,

    /// The authentication manager for the proxy service.
    pub auth: Auth,
}

/// Starts and runs the Atoma proxy service service, handling HTTP requests and graceful shutdown.
/// This function initializes and runs the main proxy_service service that handles node operations,
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state container for the proxy service service, containing the Sui client,
///   state manager, and node badge information
/// * `tcp_listener` - A pre-configured TCP listener that the HTTP server will bind to
///
/// # Returns
///
/// * `anyhow::Result<()>` - Ok(()) on successful shutdown, or an error if
///   server initialization or shutdown fails
///
/// # Shutdown Behavior
///
/// The server implements graceful shutdown by:
/// 1. Listening for a Ctrl+C signal
/// 2. Logging shutdown initiation
/// 3. Waiting for existing connections to complete
///
/// # Example
///
/// ```rust,ignore
/// use tokio::net::TcpListener;
/// use tokio::sync::watch;
/// use atoma_proxy_service::{ProxyServiceState, run_proxy_service};
///
/// async fn start_server() -> Result<(), Box<dyn std::error::Error>> {
///     let proxy_service_state = ProxyServiceState::new(/* ... */);
///     let listener = TcpListener::bind("127.0.0.1:3000").await?;
///     
///     run_proxy_service(proxy_service_state, listener).await
/// }
/// ```
pub async fn run_proxy_service(
    proxy_service_state: ProxyServiceState,
    tcp_listener: TcpListener,
    mut shutdown_receiver: Receiver<bool>,
) -> anyhow::Result<()> {
    let proxy_service_router = create_proxy_service_router(proxy_service_state);
    let server = axum::serve(tcp_listener, proxy_service_router.into_make_service())
        .with_graceful_shutdown(async move {
            shutdown_receiver
                .changed()
                .await
                .expect("Error receiving shutdown signal")
        });
    server.await?;
    Ok(())
}

/// Creates and configures the main router for the Atoma proxy service HTTP API.
///
/// # Arguments
/// * `proxy_service_state` - The shared state container that will be available to all route handlers
///
/// # Returns
/// * `Router` - A configured axum Router instance with all API routes and shared state
///
/// # API Endpoints
///
/// ## Subscription Management
/// * `GET /subscriptions` - Get all subscriptions for registered nodes
/// * `GET /subscriptions/:id` - Get subscriptions for a specific node
/// * `POST /model_subscribe` - Subscribe a node to a model
/// * `POST /task_subscribe` - Subscribe a node to a task
/// * `POST /task_update_subscription` - Updates an already existing subscription to a task
/// * `POST /task_unsubscribe` - Unsubscribe a node from a task
///
/// ## Task Management
/// * `GET /tasks` - Get all available tasks
///
/// ## Stack Operations
/// * `GET /stacks` - Get all stacks for registered nodes
/// * `GET /stacks/:id` - Get stacks for a specific node
/// * `GET /almost_filled_stacks/:fraction` - Get stacks filled above specified fraction
/// * `GET /almost_filled_stacks/:id/:fraction` - Get node's stacks filled above fraction
/// * `GET /claimed_stacks` - Get all claimed stacks
/// * `GET /claimed_stacks/:id` - Get claimed stacks for a specific node
/// * `POST /try_settle_stack_ids` - Attempt to settle specified stacks
/// * `POST /submit_stack_settlement_attestations` - Submit attestations for stack settlement
/// * `POST /claim_funds` - Claim funds from completed stacks
///
/// ## Attestation Disputes
/// * `GET /against_attestation_disputes` - Get disputes against registered nodes
/// * `GET /against_attestation_disputes/:id` - Get disputes against a specific node
/// * `GET /own_attestation_disputes` - Get disputes initiated by registered nodes
/// * `GET /own_attestation_disputes/:id` - Get disputes initiated by a specific node
///
/// ## Node Registration
/// * `POST /register` - Register a new node
///
/// # Example
/// ```rust,ignore
/// use atoma_proxy_service::ProxyServiceState;
///
/// let proxy_service_state = ProxyServiceState::new(/* ... */);
/// let app = create_proxy_service_router(proxy_service_state);
/// // Start the server with the configured router
/// axum::Server::bind(&"0.0.0.0:3000".parse().unwrap())
///     .serve(app.into_make_service())
///     .await?;
/// ```
pub fn create_proxy_service_router(proxy_service_state: ProxyServiceState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(vec![Method::GET, Method::POST])
        .allow_headers(Any);
    Router::new()
        .route("/subscriptions", get(get_all_subscriptions))
        .route("/tasks", get(get_all_tasks))
        .route("/task/:id", get(get_nodes_for_tasks))
        .route("/stacks/:id", get(get_node_stacks))
        .route("/get_stacks", get(get_current_stacks))
        .route("/register", post(register))
        .route("/login", post(login))
        .route("/api_tokens", get(get_all_api_tokens))
        .route("/generate_api_token", get(generate_api_token))
        .route("/revoke_api_token", post(revoke_api_token))
        .layer(cors)
        .with_state(proxy_service_state)
        .route("/health", get(health))
}

async fn get_all_api_tokens(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
) -> Result<Json<Vec<String>>> {
    let auth_header = headers
        .get("Authorization")
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_str()
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let jwt = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(Json(
        proxy_service_state
            .auth
            .get_all_api_tokens(jwt)
            .await
            .map_err(|e| {
                error!("Failed to get all api tokens: {:?}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}

async fn generate_api_token(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
) -> Result<Json<String>> {
    let auth_header = headers
        .get("Authorization")
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_str()
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let jwt = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(Json(
        proxy_service_state
            .auth
            .generate_api_token(jwt)
            .await
            .map_err(|e| {
                error!("Failed to generate api token: {:?}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}

async fn revoke_api_token(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
    body: Json<RevokeApiTokenRequest>,
) -> Result<Json<()>> {
    let auth_header = headers
        .get("Authorization")
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_str()
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let jwt = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;
    proxy_service_state
        .auth
        .revoke_api_token(jwt, &body.api_token)
        .await
        .map_err(|e| {
            error!("Failed to revoke api token: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(()))
}

async fn register(
    State(proxy_service_state): State<ProxyServiceState>,
    body: Json<AuthRequest>,
) -> Result<Json<AuthResponse>> {
    let (refresh_token, access_token) = proxy_service_state
        .auth
        .register(&body.username, &body.password)
        .await
        .map_err(|e| {
            error!("Failed to register user: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(AuthResponse {
        access_token,
        refresh_token,
    }))
}

async fn login(
    State(proxy_service_state): State<ProxyServiceState>,
    body: Json<AuthRequest>,
) -> Result<Json<AuthResponse>> {
    let (refresh_token, access_token) = proxy_service_state
        .auth
        .check_user_password(&body.username, &body.password)
        .await
        .map_err(|e| {
            error!("Failed to register user: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(AuthResponse {
        access_token,
        refresh_token,
    }))
}

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
        (status = OK, description = "Retrieves all stacks that are not settled", body = Value),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get all stacks")
    )
)]
#[instrument(level = "trace", skip_all)]
async fn get_current_stacks(
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
        (status = OK, description = "Retrieves all subscriptions for all nodes", body = Value),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get nodes subscriptions")
    )
)]
#[instrument(level = "trace", skip_all)]
async fn get_all_subscriptions(
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
    path = "",
    responses(
        (status = OK, description = "Retrieves all subscriptions for a specific task", body = Value),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get node subscriptions")
    )
)]
#[instrument(level = "trace", skip_all)]
async fn get_nodes_for_tasks(
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
        (status = OK, description = "Retrieves all tasks", body = Value),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get all tasks")
    )
)]
#[instrument(level = "trace", skip_all)]
async fn get_all_tasks(
    State(proxy_service_state): State<ProxyServiceState>,
) -> Result<Json<Vec<Task>>> {
    let all_tasks = proxy_service_state
        .atoma_state
        .get_all_tasks()
        .await
        .map_err(|_| {
            error!("Failed to get all tasks");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(all_tasks))
}

/// Health check endpoint for the proxy service.
///
/// # Returns
/// * `StatusCode::OK` - Always returns OK
#[utoipa::path(
    get,
    path = "",
    responses(
        (status = OK, description = "Service is healthy", body = Value),
        (status = INTERNAL_SERVER_ERROR, description = "Service is unhealthy")
    )
)]
#[instrument(level = "trace", skip_all)]
async fn health() -> StatusCode {
    StatusCode::OK
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
    path = "",
    responses(
        (status = OK, description = "Retrieves all stacks for a specific node", body = Value),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get node stack")
    )
)]
#[instrument(level = "trace", skip_all)]
async fn get_node_stacks(
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
