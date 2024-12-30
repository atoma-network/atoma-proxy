use serde_json::Value;

use crate::server::{http_server::ProxyState, Result};

/// A trait for parsing and handling AI model requests across different endpoints (chat, embeddings, images).
/// This trait provides a common interface for processing various types of AI model requests
/// and estimating their computational costs.
pub trait RequestModel {
    /// Constructs a new request model instance by parsing the provided JSON request.
    ///
    /// # Arguments
    /// * `request` - The JSON payload containing the request parameters
    ///
    /// # Returns
    /// * `Ok(Self)` - Successfully parsed request model
    /// * `Err(AtomaProxyError)` - If the request is invalid or malformed
    fn new(request: &Value) -> Result<Self>
    where
        Self: Sized;

    /// Retrieves the target AI model identifier for this request.
    ///
    /// # Returns
    /// * `Ok(String)` - The name/identifier of the AI model to be used
    /// * `Err(AtomaProxyError)` - If the model information is missing or invalid
    fn get_model(&self) -> Result<String>;

    /// Calculates the estimated computational resources required for this request.
    ///
    /// # Arguments
    /// * `state` - The current proxy state containing configuration and metrics
    ///
    /// # Returns
    /// * `Ok(u64)` - The estimated compute units needed
    /// * `Err(AtomaProxyError)` - If the estimation fails or parameters are invalid
    fn get_compute_units_estimate(&self, state: &ProxyState) -> Result<u64>;
}
