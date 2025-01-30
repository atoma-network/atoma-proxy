use std::collections::HashMap;

use atoma_auth::Auth;
use atoma_state::AtomaState;
use axum::{
    http::{Method, StatusCode},
    routing::get,
    Router,
};

use tokio::{net::TcpListener, sync::watch::Receiver};
use tower_http::cors::{Any, CorsLayer};
use tracing::instrument;
use utoipa::OpenApi;

use crate::{
    components::openapi::openapi_router,
    handlers::{
        auth::auth_router, stacks::stacks_router, stats::stats_router,
        subscriptions::subscriptions_router, tasks::tasks_router,
    },
    ModelModality,
};

/// The path for the health check endpoint.
pub const HEALTH_PATH: &str = "/health";

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

    /// List of models and their modalities.
    pub models_with_modalities: HashMap<String, Vec<ModelModality>>,
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
/// # Errors
/// Returns an error if:
/// - Failed to accept new connections
/// - Failed to spawn worker tasks
/// - Failed to handle client requests
/// - Failed to shut down gracefully
///
/// # Panics
/// Panics if:
/// - Failed to receive shutdown signal
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
                .expect("Error receiving shutdown signal");
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
        .merge(auth_router())
        .merge(stacks_router())
        .merge(subscriptions_router())
        .merge(tasks_router())
        .merge(stats_router())
        .layer(cors)
        .with_state(proxy_service_state)
        .route(HEALTH_PATH, get(health))
        .merge(openapi_router())
}

/// OpenAPI documentation for the health endpoint.
///
/// This struct is used to generate OpenAPI documentation for the health
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(health))]
pub struct HealthOpenApi;

/// Health check endpoint for the proxy service.
///
/// # Returns
/// * `StatusCode::OK` - Always returns OK
#[utoipa::path(
    get,
    path = "",
    responses(
        (status = OK, description = "Service is healthy"),
        (status = INTERNAL_SERVER_ERROR, description = "Service is unhealthy")
    )
)]
#[instrument(level = "trace", skip_all)]
pub async fn health() -> StatusCode {
    StatusCode::OK
}
