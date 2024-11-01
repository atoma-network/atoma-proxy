use std::sync::Arc;

use atoma_state::{types::AtomaAtomaStateManagerEvent, AtomaStateManager};
use atoma_sui::client::AtomaSuiClient;
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
    Json, Router,
};
use flume::Sender;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    net::TcpListener,
    sync::{oneshot, RwLock},
};
use tracing::error;

mod config;
pub use config::AtomaServiceConfig;

use crate::sui::Sui;

const CHAT_COMPLETIONS_PATH: &str = "/v1/chat/completions";
const NODE_PUBLIC_ADDRESS_REGISTRATION: &str = "/node/registration";

#[derive(Clone)]
pub struct ProxyState {
    pub _client: Arc<RwLock<AtomaSuiClient>>,
    pub state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
    pub sui: Arc<RwLock<Sui>>,
}

fn check_auth(headers: &HeaderMap) -> bool {
    if let Some(auth) = headers.get("Authorization") {
        if let Ok(auth) = auth.to_str() {
            if auth == "Bearer atoma" {
                return true;
            }
        }
    }
    false
}

pub async fn chat_completions_handler(
    State(state): State<ProxyState>,
    headers: HeaderMap,
    Json(payload): Json<Value>,
) -> Result<Json<Value>, StatusCode> {
    if !check_auth(&headers) {
        return Err(StatusCode::UNAUTHORIZED);
    }

    // Get the model from the payload, so we can find the task.
    let model = if let Some(model) = payload.get("model") {
        if let Some(model) = model.as_str() {
            model.to_string()
        } else {
            return Err(StatusCode::BAD_REQUEST);
        }
    } else {
        return Err(StatusCode::BAD_REQUEST);
    };

    let (result_sender, result_receiver) = oneshot::channel();

    state
        .state_manager_sender
        .send(AtomaAtomaStateManagerEvent::GetStacksForModel {
            model: model.clone(),
            free_compute_units: 1000,
            result_sender,
        })
        .map_err(|err| {
            error!("Failed to send GetStacksForModel event: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let stacks = result_receiver
        .await
        .map_err(|err| {
            error!("Failed to receive GetStacksForModel result: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map_err(|err| {
            error!("Failed to get GetStacksForModel result: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let selected_node_id = if stacks.is_empty() {
        let (result_sender, result_receiver) = oneshot::channel();
        state
            .state_manager_sender
            .send(AtomaAtomaStateManagerEvent::GetTasksForModel {
                model: model.clone(),
                result_sender,
            })
            .map_err(|err| {
                error!("Failed to send GetTasksForModel event: {:?}", err);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        let tasks = result_receiver
            .await
            .map_err(|err| {
                error!("Failed to receive GetTasksForModel result: {:?}", err);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .map_err(|err| {
                error!("Failed to get GetTasksForModel result: {:?}", err);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        if tasks.is_empty() {
            error!("No tasks found for model {}", model);
            return Err(StatusCode::NOT_FOUND);
        }
        state
            .sui
            .write()
            .await
            .acquire_new_stack_entry(tasks[0].task_small_id as u64, 1000, 100)
            .await
            .map_err(|err| {
                error!("Failed to acquire new stack entry: {:?}", err);
                StatusCode::INTERNAL_SERVER_ERROR
            })?
    } else {
        stacks[0].selected_node_id
    };

    let (result_sender, result_receiver) = oneshot::channel();
    state
        .state_manager_sender
        .send(AtomaAtomaStateManagerEvent::GetNodePublicAddress {
            node_small_id: selected_node_id,
            result_sender,
        })
        .map_err(|err| {
            error!("Failed to send GetNodePublicAddress event: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let node_address = result_receiver
        .await
        .map_err(|err| {
            error!("Failed to receive GetNodePublicAddress result: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map_err(|err| {
            error!("Failed to get GetNodePublicAddress result: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or_else(|| {
            error!("No node address found for node {}", selected_node_id);
            StatusCode::NOT_FOUND
        })?;

    state
        .sui
        .write()
        .await
        .send_openai_api_request(payload, node_address)
        .await
        .map_err(|err| {
            error!("Failed to send OpenAI API request: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })
        .map(|response| Json(response))
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
        .state_manager_sender
        .send(AtomaAtomaStateManagerEvent::UpdateNodePublicAddress {
            node_small_id: payload.node_small_id as i64,
            public_address: payload.public_address.clone(),
        })
        .map_err(|err| {
            error!("Failed to send UpdateNodePublicAddress event: {:?}", err);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(Value::Null))
}

pub async fn start_server(
    config: AtomaServiceConfig,
    atoma_sui_client: AtomaSuiClient,
    state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
    sui: Sui,
) {
    let tcp_listener = TcpListener::bind(config.service_bind_address)
        .await
        .unwrap();

    let proxy_state = ProxyState {
        _client: Arc::new(RwLock::new(atoma_sui_client)),
        state_manager_sender: state_manager_sender,
        sui: Arc::new(RwLock::new(sui)),
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
