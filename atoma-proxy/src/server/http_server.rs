use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;
use std::time::Instant;

use atoma_auth::Sui;
use atoma_state::types::AtomaAtomaStateManagerEvent;
use axum::middleware::from_fn_with_state;
use axum::{
    routing::{get, post},
    Json, Router,
};
use dashmap::DashMap;
use flume::Sender;
use reqwest::Method;
use serde::Serialize;
use tokenizers::Tokenizer;
use tokio::sync::watch;
use tokio::{net::TcpListener, sync::RwLock};
use tower::ServiceBuilder;
use tower_http::cors::{Any, CorsLayer};
use tracing::instrument;

pub use components::openapi::openapi_routes;
use utoipa::{OpenApi, ToSchema};

use crate::server::{
    handlers::{
        chat_completions::chat_completions_create,
        chat_completions::CHAT_COMPLETIONS_PATH,
        embeddings::embeddings_create,
        embeddings::EMBEDDINGS_PATH,
        image_generations::image_generations_create,
        image_generations::IMAGE_GENERATIONS_PATH,
        models::{models_list, MODELS_PATH},
    },
    Result,
};

use super::components;
use super::handlers::chat_completions::{
    confidential_chat_completions_create, CONFIDENTIAL_CHAT_COMPLETIONS_PATH,
};
use super::handlers::completions::{
    completions_create, confidential_completions_create, COMPLETIONS_PATH,
    CONFIDENTIAL_COMPLETIONS_PATH,
};
use super::handlers::embeddings::{confidential_embeddings_create, CONFIDENTIAL_EMBEDDINGS_PATH};
use super::handlers::image_generations::{
    confidential_image_generations_create, CONFIDENTIAL_IMAGE_GENERATIONS_PATH,
};
use super::handlers::models::{open_router_models_list, OPEN_ROUTER_MODELS_PATH};
use super::handlers::nodes::{
    nodes_create, nodes_create_lock, NODES_CREATE_LOCK_PATH, NODES_CREATE_PATH,
};
use super::middleware::{
    authenticate_middleware, confidential_compute_middleware, handle_locked_stack_middleware,
};
use super::AtomaServiceConfig;

/// Path for health check endpoint.
///
/// This endpoint is used to check the health of the atoma proxy service.
pub const HEALTH_PATH: &str = "/health";

pub type UserId = i64;

pub type TaskId = i64;

pub type StackSmallId = i64;

pub type Timeout = u64;

pub type LockedComputeUnits = i64;

/// Represents the details of a locked compute unit.
///
/// This struct holds the expiration time and the maximum number of tokens
/// for a locked compute unit.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LockedDetails {
    /// The expiration time of the locked compute unit.
    pub expires_at: Instant,

    /// The maximum number of tokens for the locked compute unit.
    pub max_num_tokens: LockedComputeUnits,
}

/// Represents the shared state of the application.
///
/// This struct holds various components and configurations that are shared
/// across different parts of the application, enabling efficient resource
/// management and communication between components.
#[derive(Clone)]
pub struct ProxyState {
    /// Channel sender for managing application events.
    ///
    /// This sender is used to communicate events and state changes to the
    /// state manager, allowing for efficient handling of application state
    /// updates and notifications across different components.
    pub state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,

    /// Map of user ids to their stack lock status.
    ///
    /// This map is used to prevent race conditions when multiple requests
    /// try to acquire the same stack for a user.
    pub users_buy_stack_lock_map: Arc<DashMap<(UserId, TaskId), bool>>,

    /// Map of stack ids to their locked compute units for confidential compute mode.
    ///
    /// This map is used to prevent race conditions when multiple requests
    /// try to acquire the same stack for a user.
    pub stack_locked_compute_units: Arc<DashMap<StackSmallId, BinaryHeap<Reverse<LockedDetails>>>>,

    /// `Sui` struct for handling Sui-related operations.
    ///
    /// This struct is used to interact with the Sui component of the application,
    /// enabling communication with the Sui service and handling Sui-related operations
    /// such as acquiring new stack entries.
    pub sui: Arc<RwLock<Sui>>,

    /// Tokenizer used for processing text input.
    ///
    /// The tokenizer is responsible for breaking down text input into
    /// manageable tokens, which are then used in various natural language
    /// processing tasks.
    pub tokenizers: Arc<Vec<Arc<Tokenizer>>>,

    /// List of available AI models.
    ///
    /// This list contains the names or identifiers of AI models that
    /// the application can use for inference tasks. It allows the
    /// application to dynamically select and switch between different
    /// models as needed.
    pub models: Arc<Vec<String>>,

    /// Open router models file.
    pub open_router_models_file: String,
}

#[derive(OpenApi)]
#[openapi(paths(health))]
/// OpenAPI documentation for the health check endpoint.
///
/// This struct is used to generate OpenAPI documentation for the health check
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
///
/// The health check endpoint is accessible at `/health` and returns a simple
/// JSON response indicating the service status.
pub struct HealthOpenApi;

#[derive(Serialize, ToSchema)]
pub struct HealthResponse {
    /// The status of the service
    message: String,
}

/// Health
#[utoipa::path(
    get,
    path = "",
    responses(
        (status = OK, description = "Service is healthy", body = HealthResponse),
        (status = INTERNAL_SERVER_ERROR, description = "Service is unhealthy")
    )
)]
pub async fn health() -> Result<Json<HealthResponse>> {
    Ok(Json(HealthResponse {
        message: "ok".to_string(),
    }))
}

/// Creates a router with the appropriate routes and state for the atoma proxy service.
///
/// This function sets up two sets of routes:
/// 1. Standard routes for public API endpoints
/// 2. Confidential routes for secure processing
///
/// # Routes
///
/// ## Standard Routes
/// - POST `/v1/chat/completions` - Chat completion endpoint
/// - POST `/v1/embeddings` - Text embedding generation
/// - POST `/v1/images/generations` - Image generation
/// - GET `/v1/models` - List available AI models
/// - POST `/node/registration` - Node public address registration
/// - GET `/health` - Service health check
/// - OpenAPI documentation routes
///
/// ## Confidential Routes
/// Secure variants of the processing endpoints:
/// - POST `/v1/confidential/chat/completions`
/// - POST `/v1/confidential/embeddings`
/// - POST `/v1/confidential/images/generations`
///
/// # Arguments
///
/// * `state` - Shared application state containing configuration and resources
///
/// # Returns
///
/// Returns an configured `Router` instance with all routes and middleware set up
pub fn create_router(state: &ProxyState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(vec![Method::GET, Method::POST])
        .allow_headers(Any);

    let confidential_routes = Router::new()
        .route(
            CONFIDENTIAL_CHAT_COMPLETIONS_PATH,
            post(confidential_chat_completions_create),
        )
        .route(
            CONFIDENTIAL_COMPLETIONS_PATH,
            post(confidential_completions_create),
        )
        .route(
            CONFIDENTIAL_EMBEDDINGS_PATH,
            post(confidential_embeddings_create),
        )
        .route(
            CONFIDENTIAL_IMAGE_GENERATIONS_PATH,
            post(confidential_image_generations_create),
        );

    let regular_routes = Router::new()
        .route(MODELS_PATH, get(models_list))
        .route(COMPLETIONS_PATH, post(completions_create))
        .route(CHAT_COMPLETIONS_PATH, post(chat_completions_create))
        .route(EMBEDDINGS_PATH, post(embeddings_create))
        .route(IMAGE_GENERATIONS_PATH, post(image_generations_create));

    let node_routes = Router::new()
        .route(NODES_CREATE_PATH, post(nodes_create))
        .route(NODES_CREATE_LOCK_PATH, post(nodes_create_lock));

    let public_routes = Router::new()
        .route(HEALTH_PATH, get(health))
        .route(OPEN_ROUTER_MODELS_PATH, get(open_router_models_list));

    Router::new()
        .merge(
            confidential_routes.layer(
                ServiceBuilder::new()
                    .layer(from_fn_with_state(
                        state.clone(),
                        confidential_compute_middleware,
                    ))
                    .layer(from_fn_with_state(
                        state.clone(),
                        handle_locked_stack_middleware,
                    )),
            ),
        )
        .merge(
            regular_routes.layer(
                ServiceBuilder::new()
                    .layer(from_fn_with_state(state.clone(), authenticate_middleware))
                    .layer(from_fn_with_state(
                        state.clone(),
                        handle_locked_stack_middleware,
                    )),
            ),
        )
        .merge(node_routes)
        .merge(public_routes)
        .with_state(state.clone())
        .merge(openapi_routes())
        .layer(cors)
}

/// Starts the atoma proxy server.
///
/// This function starts the atoma proxy server by binding to the specified address
/// and routing requests to the appropriate handlers.
///
/// # Arguments
///
/// * `config`: The configuration for the atoma proxy service.
/// * `state_manager_sender`: The sender channel for managing application events.
/// * `sui`: The Sui struct for handling Sui-related operations.
///
/// # Errors
///
/// Returns an error if the tcp listener fails to bind or the server fails to start.
#[instrument(level = "info", skip_all, fields(service_bind_address = %config.service_bind_address))]
pub async fn start_server(
    config: AtomaServiceConfig,
    state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
    sui: Arc<RwLock<Sui>>,
    tokenizers: Vec<Arc<Tokenizer>>,
    mut shutdown_receiver: watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let tcp_listener = TcpListener::bind(config.service_bind_address).await?;

    let proxy_state = ProxyState {
        state_manager_sender,
        users_buy_stack_lock_map: Arc::new(DashMap::new()),
        stack_locked_compute_units: Arc::new(DashMap::new()),
        sui,
        tokenizers: Arc::new(tokenizers),
        models: Arc::new(config.models),
        open_router_models_file: config.open_router_models_file,
    };
    let router = create_router(&proxy_state);
    let server =
        axum::serve(tcp_listener, router.into_make_service()).with_graceful_shutdown(async move {
            shutdown_receiver
                .changed()
                .await
                .expect("Error receiving shutdown signal");
        });
    server.await?;
    Ok(())
}
