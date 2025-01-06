mod components;
mod config;
mod handlers;
mod proxy_service;
mod query;

pub use config::AtomaProxyServiceConfig;
pub use proxy_service::*;
pub use query::*;
use serde::{Deserialize, Serialize};

/// Model Capabilities.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ModelCapabilities {
    #[serde(rename = "Chat Completions")]
    ChatCompletions,
    #[serde(rename = "Images Generations")]
    ImagesGenerations,
    Embeddings,
}
