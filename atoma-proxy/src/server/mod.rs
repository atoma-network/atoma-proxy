pub(crate) mod components;
mod config;
pub mod error;
pub mod handlers;
pub mod http_server;
pub mod middleware;
pub mod streamer;

pub use config::AtomaServiceConfig;
pub use http_server::start_server;

pub type Result<T> = std::result::Result<T, error::AtomaProxyError>;
