use serde_json::Value;
use tokenizers::Tokenizer;

use crate::server::Result;

/// A struct that contains the estimated compute units needed for a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputeUnitsEstimate {
    /// The number of compute units needed for the input tokens.
    pub num_input_tokens: u64,
    /// The number of compute units needed for the output tokens.
    pub max_output_tokens: u64,
}

/// A trait for parsing and handling AI model requests across different endpoints (chat, embeddings, images).
/// This trait provides a common interface for processing various types of AI model requests
/// and estimating their computational costs.
pub trait RequestModel: Clone {
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
    /// * `String` - The name/identifier of the AI model to be used
    fn get_model(&self) -> String;

    /// Calculates the estimated computational resources required for this request.
    ///
    /// # Arguments
    /// * `tokenizer` - The tokenizer to use for the request
    ///
    /// # Returns
    /// * `Ok(u64)` - The estimated compute units needed
    /// * `Err(AtomaProxyError)` - If the estimation fails or parameters are invalid
    ///
    /// # Warning
    /// This method assumes that the tokenizer has been correctly retrieved from the `ProxyState` for
    /// the associated model, as obtained by calling `get_model` on `Self`.
    fn get_compute_units_estimate(
        &self,
        tokenizer: Option<&Tokenizer>,
    ) -> Result<ComputeUnitsEstimate>;
}
