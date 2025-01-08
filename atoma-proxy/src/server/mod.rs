pub(crate) mod components;
mod config;
pub mod error;
pub mod handlers;
pub mod http_server;
pub mod middleware;
pub mod streamer;
pub mod types;

pub use config::AtomaServiceConfig;
pub use http_server::start_server;

pub type Result<T> = std::result::Result<T, error::AtomaProxyError>;

/// The max_tokens field in the request payload.
pub(crate) const MAX_TOKENS: &str = "max_tokens";

/// The default max_tokens value.
pub(crate) const DEFAULT_MAX_TOKENS: u64 = 4_096;
