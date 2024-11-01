use std::sync::Arc;

use atoma_state::AtomaStateManager;
use atoma_sui::client::AtomaSuiClient;
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{net::TcpListener, sync::RwLock};
use tracing::error;

mod config;
pub use config::AtomaServiceConfig;

const CHAT_COMPLETIONS_PATH: &str = "/v1/chat/completions";
const NODE_PUBLIC_ADDRESS_REGISTRATION: &str = "/node/registration";

#[derive(Clone)]
pub struct ProxyState {
    pub _client: Arc<RwLock<AtomaSuiClient>>,
    pub state: Arc<RwLock<AtomaStateManager>>,
}

pub async fn chat_completions_handler(
    State(_state): State<ProxyState>,
    Json(_payload): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    todo!("completion handler");
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NodePublicIpAssignment {
    node_small_id: u64,
    public_address: String,
}

#[axum::debug_handler]
pub async fn node_public_address_registration(
    State(state): State<ProxyState>,
    Json(payload): Json<NodePublicIpAssignment>,
) -> Result<Json<Value>, StatusCode> {
    state
        .state
        .write()
        .await
        .state
        .store_node_public_address(payload.node_small_id as i64, payload.public_address)
        .await
        .map_err(|e| {
            dbg!(e);
            error!("Failed to submit node task subscription tx");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(Value::Null))
}

pub async fn start_server(
    config: AtomaServiceConfig,
    atoma_sui_client: AtomaSuiClient,
    state_manager: AtomaStateManager,
) {
    let tcp_listener = TcpListener::bind(config.service_bind_address)
        .await
        .unwrap();

    let proxy_state = ProxyState {
        _client: Arc::new(RwLock::new(atoma_sui_client)),
        state: Arc::new(RwLock::new(state_manager)),
    };
    let router = Router::new()
        .route(CHAT_COMPLETIONS_PATH, post(chat_completions_handler))
        .route(
            NODE_PUBLIC_ADDRESS_REGISTRATION,
            post(node_public_address_registration),
        )
        .with_state(proxy_state);
    let server = axum::serve(tcp_listener, router.into_make_service());
    server.await.unwrap();
}
