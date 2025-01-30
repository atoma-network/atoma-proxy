#![allow(clippy::doc_markdown)]
#![allow(clippy::cast_possible_wrap)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cognitive_complexity)]

pub mod components;
mod config;
pub mod error;
pub mod handlers;
pub mod http_server;
pub mod middleware;
pub mod streamer;
pub mod types;

use atoma_state::types::AtomaAtomaStateManagerEvent;
use axum::http::HeaderMap;
pub use config::AtomaServiceConfig;
use error::AtomaProxyError;
use flume::Sender;
pub use http_server::start_server;
use tokio::sync::oneshot;
use tracing::instrument;

pub type Result<T> = std::result::Result<T, error::AtomaProxyError>;

/// The max_tokens field in the request payload, currently deprecated by the OpenAI API, in favor of max_completion_tokens.
pub const MAX_TOKENS: &str = "max_tokens";

/// The max_completion_tokens field in the request payload.
pub const MAX_COMPLETION_TOKENS: &str = "max_completion_tokens";

/// The default max_tokens value.
pub const DEFAULT_MAX_TOKENS: u64 = 4_096;

/// The one million constant.
pub const ONE_MILLION: u64 = 1_000_000;

/// The model key
pub const MODEL: &str = "model";

/// Checks the authentication of the request.
///
/// This function checks the authentication of the request by comparing the
/// provided Bearer token from the `Authorization` header against the stored tokens.
///
/// # Arguments
///
/// * `state_manager_sender`: The sender for the state manager channel.
/// * `headers`: The headers of the request.
///
/// # Returns
///
/// Returns `true` if the authentication is successful, `false` otherwise.
///
/// # Errors
///
/// Returns a `AtomaProxyError` error if there is an internal server error.
///
/// # Example
///
/// ```rust,ignore
/// let is_authenticated = check_auth(
///     &state_manager_sender,
///     &headers
/// ).await?;
/// println!("Token is : {}", is_authenticated?"Valid":"Invalid");
/// ```
#[instrument(level = "info", skip_all)]
async fn check_auth(
    state_manager_sender: &Sender<AtomaAtomaStateManagerEvent>,
    headers: &HeaderMap,
    endpoint: &str,
) -> Result<i64> {
    if let Some(auth) = headers.get("Authorization") {
        if let Ok(auth) = auth.to_str() {
            if let Some(token) = auth.strip_prefix("Bearer ") {
                let (sender, receiver) = oneshot::channel();
                state_manager_sender
                    .send(AtomaAtomaStateManagerEvent::IsApiTokenValid {
                        api_token: token.to_string(),
                        result_sender: sender,
                    })
                    .map_err(|err| AtomaProxyError::InternalError {
                        message: format!("Failed to send IsApiTokenValid event: {err:?}"),
                        endpoint: endpoint.to_string(),
                    })?;
                return receiver
                    .await
                    .map_err(|err| AtomaProxyError::InternalError {
                        message: format!("Failed to receive IsApiTokenValid result: {err:?}"),
                        endpoint: endpoint.to_string(),
                    })?
                    .map_err(|err| AtomaProxyError::AuthError {
                        auth_error: format!("Invalid or missing api token for request: {err:?}"),
                        endpoint: endpoint.to_string(),
                    });
            }
        }
    }
    Err(AtomaProxyError::AuthError {
        auth_error: "Invalid or missing api token for request".to_string(),
        endpoint: endpoint.to_string(),
    })
}
