use atoma_state::types::AtomaAtomaStateManagerEvent;
use atoma_utils::constants;
use auth::{GetSelectedNodeArgs, ProcessedRequest, SelectedNodeMetadata, StackMetadata};
use axum::{
    body::Body,
    extract::{rejection::LengthLimitError, Request, State},
    http::{request::Parts, HeaderValue},
    middleware::Next,
    response::Response,
};
use base64::engine::{general_purpose::STANDARD, Engine};
use reqwest::{
    header::{AUTHORIZATION, CONTENT_LENGTH},
    StatusCode,
};
use serde_json::Value;
use tracing::instrument;
use utils::is_confidential_compute_endpoint;

use super::{
    check_auth,
    error::AtomaProxyError,
    handlers::{
        image_generations::CONFIDENTIAL_IMAGE_GENERATIONS_PATH, models::MODELS_PATH,
        nodes::MAX_NUM_TOKENS_FOR_CONFIDENTIAL_COMPUTE, update_state_manager,
    },
    http_server::ProxyState,
};
use super::{types::ConfidentialComputeRequest, Result};

/// The size of the stack to buy in compute units.
///
/// NOTE: Right now, we buy the maximum number of compute units that a node supports
/// as hardcoded in Atoma's smart contract.
pub const STACK_SIZE_TO_BUY: i64 = 1_000_000;

/// Default image resolution for image generations, in pixels.
const DEFAULT_IMAGE_RESOLUTION: u64 = 1024 * 1024;

/// Maximum size of the body in bytes.
/// This is to prevent DoS attacks by limiting the size of the request body.
const MAX_BODY_SIZE: usize = 1024 * 1024; // 1MB

/// Metadata extension for tracking request-specific information about the selected inference node.
///
/// This extension is attached to requests during authentication middleware processing
/// and contains essential information about the node that will process the request.
#[derive(Clone, Debug, Default)]
pub struct RequestMetadataExtension {
    /// The public address/endpoint of the selected inference node.
    /// This is typically a URL where the request will be forwarded to.
    pub node_address: String,

    /// Unique identifier for the selected node in the system.
    /// This ID is used to track and manage node-specific operations and state.
    pub node_id: i64,

    /// Number of input tokens for the request.
    /// This is only set for non-confidential requests, for confidential requests,
    /// the input tokens are not known, so we don't count it.
    pub num_input_tokens: Option<u64>,

    /// Estimated compute units required for this request.
    /// This represents the total computational resources needed for both input and output processing.
    pub num_compute_units: u64,

    /// Selected stack small id for this request.
    pub selected_stack_small_id: i64,

    /// The endpoint path for this request.
    pub endpoint: String,

    /// Model name
    pub model_name: String,
}

impl RequestMetadataExtension {
    /// Adds a node address to the request metadata.
    ///
    /// This method is used to set the node address that will be used for the request.
    ///
    /// # Arguments
    ///
    /// * `node_address` - The node address to set
    ///
    /// # Returns
    ///
    /// Returns self with the node address field populated, enabling method chaining
    pub fn with_node_address(mut self, node_address: String) -> Self {
        self.node_address = node_address;
        self
    }

    /// Adds a node small id to the request metadata.
    ///
    /// This method is used to set the node small id that will be used for the request.
    ///
    /// # Arguments
    ///
    /// * `node_small_id` - The node small id to set
    ///
    /// # Returns
    ///
    /// Returns self with the node small id field populated, enabling method chaining
    pub const fn with_node_small_id(mut self, node_small_id: i64) -> Self {
        self.node_id = node_small_id;
        self
    }

    /// Adds a num compute units to the request metadata.
    ///
    /// This method is used to set the num compute units that will be used for the request.
    ///
    /// # Arguments
    ///
    /// * `num_compute_units` - The num compute units to set
    ///
    /// # Returns
    ///
    /// Returns self with the num compute units field populated, enabling method chaining
    pub const fn with_num_compute_units(mut self, num_compute_units: u64) -> Self {
        self.num_compute_units = num_compute_units;
        self
    }

    /// Adds a stack small id to the request metadata.
    ///
    /// This method is used to set the stack small id that will be used for the request.
    ///
    /// # Arguments
    ///
    /// * `stack_small_id` - The stack small id to set
    ///
    /// # Returns
    ///
    /// Returns self with the stack small id field populated, enabling method chaining
    pub const fn with_stack_small_id(mut self, stack_small_id: i64) -> Self {
        self.selected_stack_small_id = stack_small_id;
        self
    }

    /// Adds a model name to the request metadata.
    ///
    /// This method is used to set the model name that will be used for the request.
    ///
    /// # Arguments
    ///
    /// * `model_name` - The model name to set
    ///
    /// # Returns
    ///
    /// Returns self with the model name field populated, enabling method chaining
    pub fn with_model_name(mut self, model_name: String) -> Self {
        self.model_name = model_name;
        self
    }

    /// Adds an endpoint to the request metadata.
    ///
    /// This method is used to set the endpoint that will be used for the request.
    ///
    /// # Arguments
    ///
    /// * `endpoint` - The endpoint to set
    ///
    /// # Returns
    ///
    /// Returns self with the endpoint field populated, enabling method chaining
    pub fn with_endpoint(mut self, endpoint: String) -> Self {
        self.endpoint = endpoint;
        self
    }
}

/// Middleware that handles request authentication, node selection, and request processing setup.
///
/// This middleware performs several key functions:
/// 1. Authenticates incoming requests using bearer token authentication
/// 2. Parses and validates the request body based on the endpoint type (chat, embeddings, or image generation)
/// 3. Selects an appropriate inference node to handle the request
/// 4. Sets up necessary headers and metadata for request forwarding
/// 5. Handles confidential computing setup when required
///
/// # Arguments
/// * `state` - Server state containing authentication, node management, and other shared resources
/// * `req` - Incoming HTTP request
/// * `next` - Next middleware in the chain
///
/// # Returns
/// Returns the processed response from downstream handlers, or an appropriate error status code.
///
/// # Request Flow
/// 1. Extracts and validates request body (limited to 1MB)
/// 2. Determines endpoint type and creates appropriate request model
/// 3. Authenticates request and processes initial setup via `authenticate_and_process`
/// 4. Sets required headers for node communication:
///    - `X-Signature`: Authentication signature
///    - `X-Stack-Small-Id`: Selected stack identifier
///    - `Content-Length`: Updated body length
///    - `X-Tx-Digest`: Transaction digest (if new stack created)
/// 5. For confidential endpoints, adds X25519 public key information
///
/// # Errors
/// Returns various status codes for different failure scenarios:
/// * `BAD_REQUEST` (400):
///   - Body exceeds size limit
///   - Invalid JSON format
///   - Invalid request model
///   - Header conversion failures
/// * `UNAUTHORIZED` (401):
///   - Authentication failure
/// * `NOT_FOUND` (404):
///   - Invalid endpoint
///   - No X25519 public key found for node
/// * `INTERNAL_SERVER_ERROR` (500):
///   - State manager communication failures
///   - Public key retrieval failures
///
/// # Security Considerations
/// - Implements bearer token authentication
/// - Enforces 1MB maximum body size
/// - Supports confidential computing paths with X25519 key exchange
/// - Sanitizes headers before forwarding
///
/// # Example
/// ```no_run
/// let app = Router::new()
///     .route("/", get(handler))
///     .layer(middleware::from_fn(authenticate_middleware));
/// ```
#[instrument(
    level = "info",
    name = "authenticate_middleware",
    skip_all,
    fields(endpoint = %req.uri().path()),
    err
)]
pub async fn authenticate_middleware(
    state: State<ProxyState>,
    req: Request<Body>,
    next: Next,
) -> Result<Response> {
    let (mut req_parts, body) = req.into_parts();
    let endpoint = req_parts.uri.path().to_string();
    if endpoint == MODELS_PATH {
        check_auth(&state.state_manager_sender, &req_parts.headers, &endpoint).await?;
        let req = Request::from_parts(req_parts, body);
        return Ok(next.run(req).await);
    }
    let body_bytes = axum::body::to_bytes(body, MAX_BODY_SIZE)
        .await
        .map_err(|e| {
            if let Some(source) = std::error::Error::source(&e) {
                if source.is::<LengthLimitError>() {
                    return AtomaProxyError::RequestError {
                        message: format!("The body is too big: {e}"),
                        endpoint: req_parts.uri.path().to_string(),
                    };
                }
            }
            AtomaProxyError::InternalError {
                message: format!("Failed to convert body to bytes: {e}"),
                client_message: None,
                endpoint: req_parts.uri.path().to_string(),
            }
        })?;
    let body_json: Value =
        serde_json::from_slice(&body_bytes).map_err(|e| AtomaProxyError::RequestError {
            message: format!("Failed to parse body as JSON: {e}"),
            endpoint: req_parts.uri.path().to_string(),
        })?;

    // Authenticate request and lock compute units for a Stack.
    //
    // NOTE: If this method succeeds and the `optional_stack` is Some, this means that the proxy has locked
    // enough compute units for the request, within the state manager. Otherwise, this has not been the case.
    let StackMetadata {
        optional_stack,
        num_input_compute_units,
        max_total_compute_units,
        model,
        user_id,
    } = auth::handle_authenticate_and_lock_compute_units(
        &state,
        &req_parts.headers,
        &body_json,
        &endpoint,
    )
    .await?;

    // Selects an appropriate node to process the request (if there is no available node for the stacks the proxy holds, it buys a new stack)
    //
    // NOTE: IF `optional_stack` is Some, this means that the proxy has locked enough compute units for the request, within the state manager, already.
    // In this case, this method cannot error (as it just returns the underlying stack data). Otherwise, it will try to buy a new stack.
    // If this method succeeds, this means that the proxy has locked enough compute units for the request, within the state manager.
    // Otherwise, we are safe to assume that the proxy has not locked enough compute units for the request, within the state manager, and we will not be able to process the request.
    let SelectedNodeMetadata {
        stack_small_id,
        selected_node_id,
        tx_digest,
    } = auth::get_selected_node(GetSelectedNodeArgs {
        model: &model,
        state: &state,
        body_json: &body_json,
        sui: &state.sui,
        optional_stack,
        total_tokens: max_total_compute_units,
        user_id,
        endpoint: &endpoint,
    })
    .await?;

    // Validates the stack for the request.
    //
    // NOTE: If this method fails, we need to rollback the compute units that we locked for the stack, back to 0. Otherwise,
    // the proxy will be in an inconsistent state for the current stack.
    let req = match utils::try_validate_stack_for_request(
        &state,
        &body_json,
        &mut req_parts,
        selected_node_id,
        stack_small_id,
        num_input_compute_units,
        max_total_compute_units,
        tx_digest,
        user_id,
        &endpoint,
    )
    .await
    {
        Ok(req) => req,
        Err(e) => {
            update_state_manager(
                &state.state_manager_sender,
                stack_small_id,
                max_total_compute_units as i64,
                0,
                &endpoint,
            )?;
            return Err(e);
        }
    };
    Ok(next.run(req).await)
}

/// Middleware that handles routing and setup for confidential compute requests.
///
/// This middleware performs several key operations for confidential compute requests:
/// 1. Validates and deserializes the confidential compute request
/// 2. Verifies that the specified stack is valid for confidential computing
/// 3. Generates and adds a signature for the plaintext body hash
/// 4. Locks the required compute units for the stack
///
/// # Arguments
///
/// * `state` - Shared server state containing Sui interface and other resources
/// * `req` - The incoming HTTP request
/// * `next` - The next middleware in the chain
///
/// # Returns
///
/// Returns the processed response from downstream handlers, wrapped in a `Result`.
///
/// # Request Flow
///
/// 1. Extracts and validates request body (limited to 1MB)
/// 2. Deserializes the body into a `ConfidentialComputeRequest`
/// 3. Verifies stack eligibility for confidential compute
/// 4. Generates Sui signature for plaintext body hash
/// 5. Locks compute units for the stack
/// 6. Adds signature header to request
/// 7. Forwards modified request to next handler
///
/// # Errors
///
/// Returns `AtomaProxyError` in the following cases:
/// * `InternalError`:
///   - Body size exceeds limit
///   - JSON parsing fails
///   - Stack verification fails
///   - Signature generation fails
///   - Header conversion fails
///   - Compute unit locking fails
///
/// # Security Considerations
///
/// - Enforces maximum body size limit
/// - Verifies stack eligibility before processing
/// - Uses cryptographic signatures for request validation
/// - Ensures compute units are properly locked
///
/// # Example
///
/// ```rust,ignore
/// let app = Router::new()
///     .route("/confidential/*", post(handler))
///     .layer(middleware::from_fn(confidential_compute_router_middleware));
/// ```
#[instrument(
    level = "info",
    name = "confidential_compute_middleware",
    skip_all,
    fields(endpoint = %req.uri().path()),
    err
)]
pub async fn confidential_compute_middleware(
    state: State<ProxyState>,
    req: Request<Body>,
    next: Next,
) -> Result<Response> {
    let (mut req_parts, body) = req.into_parts();
    let endpoint = req_parts.uri.path().to_string();
    let body_bytes = axum::body::to_bytes(body, MAX_BODY_SIZE)
        .await
        .map_err(|e| {
            if let Some(source) = std::error::Error::source(&e) {
                if source.is::<LengthLimitError>() {
                    return AtomaProxyError::RequestError {
                        message: format!("The body is too big: {e}"),
                        endpoint: req_parts.uri.path().to_string(),
                    };
                }
            }
            AtomaProxyError::InternalError {
                message: format!("Failed to convert body to bytes: {e}"),
                client_message: None,
                endpoint: req_parts.uri.path().to_string(),
            }
        })?;
    let confidential_compute_request: ConfidentialComputeRequest =
        serde_json::from_slice(&body_bytes).map_err(|e| AtomaProxyError::RequestError {
            message: format!("Failed to parse body as JSON: {e}"),
            endpoint: req_parts.uri.path().to_string(),
        })?;

    utils::verify_stack_for_confidential_compute(
        &state,
        confidential_compute_request.stack_small_id as i64,
        MAX_NUM_TOKENS_FOR_CONFIDENTIAL_COMPUTE,
        &endpoint,
    )
    .await?;

    let plaintext_body_hash = STANDARD
        .decode(confidential_compute_request.plaintext_body_hash)
        .map_err(|e| AtomaProxyError::RequestError {
            message: format!("Hash is not base64: {e}"),
            endpoint: endpoint.clone(),
        })?;
    let plaintext_body_signature = state
        .sui
        .write()
        .await
        .sign_hash(&plaintext_body_hash)
        .map_err(|e| AtomaProxyError::InternalError {
            message: format!("Failed to get Sui signature: {e}"),
            client_message: None,
            endpoint: endpoint.clone(),
        })?;
    let signature_header = HeaderValue::from_str(&plaintext_body_signature).map_err(|e| {
        AtomaProxyError::RequestError {
            message: format!("Signed hash is not present as header value: {e}"),
            endpoint: endpoint.clone(),
        }
    })?;

    let (node_address, node_small_id) = utils::get_node_address(
        &state,
        confidential_compute_request.stack_small_id as i64,
        &endpoint,
    )
    .await?;

    let num_compute_units = if endpoint == CONFIDENTIAL_IMAGE_GENERATIONS_PATH {
        confidential_compute_request
            .num_compute_units
            .unwrap_or(DEFAULT_IMAGE_RESOLUTION) as i64
    } else {
        MAX_NUM_TOKENS_FOR_CONFIDENTIAL_COMPUTE
    };

    utils::lock_compute_units_for_stack(
        &state,
        confidential_compute_request.stack_small_id as i64,
        num_compute_units,
        &endpoint,
    )
    .await?;

    req_parts
        .headers
        .insert(constants::SIGNATURE, signature_header);
    let request_metadata = req_parts
        .extensions
        .get::<RequestMetadataExtension>()
        .cloned()
        .unwrap_or_default()
        .with_node_address(node_address)
        .with_node_small_id(node_small_id)
        .with_stack_small_id(confidential_compute_request.stack_small_id as i64)
        .with_num_compute_units(num_compute_units as u64)
        .with_model_name(confidential_compute_request.model_name)
        .with_endpoint(endpoint);
    req_parts.extensions.insert(request_metadata);
    let req = Request::from_parts(req_parts, Body::from(body_bytes));
    Ok(next.run(req).await)
}

/// Middleware that handles locked stack requests.
///
/// This middleware checks if the current request is for a locked stack and, if so,
/// attempts to lock a new stack for the request. It then forwards the request to the
/// next middleware in the chain.
///
/// # Arguments
///
/// * `state` - The state of the proxy server.
/// * `req` - The incoming HTTP request.
/// * `next` - The next middleware in the chain.
///
/// # Returns
///
/// Returns the processed response from downstream handlers, wrapped in a `Result`.
///
/// # Request Flow
///
/// 1. Extracts and validates request body (limited to 1MB)
/// 2. Deserializes the body into a `ConfidentialComputeRequest`
/// 3. Verifies stack eligibility for confidential compute
/// 4. Locks the compute units for the stack
/// 5. Forwards the request to the next middleware in the chain
///
/// # Errors
///
/// Returns `AtomaProxyError` in the following cases:
/// * `InternalError`:
///   - Failed to convert body to bytes
///   - Failed to send LockStack event
///   - Failed to convert stack small id to string
///   - Failed to parse stack small id
///   - Failed to parse stack small id
///   - Failed to convert body to bytes
///   - Failed to send LockStack event
///   - Failed to convert stack small id to string
#[instrument(level = "info", name = "handle_locked_stack_middleware", skip_all, err)]
pub async fn handle_locked_stack_middleware(
    state: State<ProxyState>,
    req: Request<Body>,
    next: Next,
) -> Result<Response> {
    let endpoint = req.uri().path().to_string();
    let (mut req_parts, body) = req.into_parts();
    // Remove the Authorization header from the request parts, before sending it to the node
    let authorization_header =
        req_parts
            .headers
            .remove(AUTHORIZATION)
            .ok_or_else(|| AtomaProxyError::RequestError {
                message: "Authorization header not found, this should never happen".to_string(),
                endpoint: endpoint.to_string(),
            })?;
    let body_bytes = axum::body::to_bytes(body, MAX_BODY_SIZE)
        .await
        .map_err(|e| {
            if let Some(source) = std::error::Error::source(&e) {
                if source.is::<LengthLimitError>() {
                    return AtomaProxyError::RequestError {
                        message: format!("The body is too big: {e}"),
                        endpoint: endpoint.to_string(),
                    };
                }
            }
            AtomaProxyError::InternalError {
                message: format!("Failed to convert body to bytes: {e}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            }
        })?;
    let original_req = Request::from_parts(req_parts.clone(), Body::from(body_bytes.clone()));
    let response = next.clone().run(original_req).await;
    if response.status() == StatusCode::LOCKED {
        // Lock the current stack, in the Proxy's internal state
        utils::lock_stack(&state.state_manager_sender, &mut req_parts, &endpoint)?;
        req_parts
            .headers
            .insert(AUTHORIZATION, authorization_header);
        let req = Request::from_parts(req_parts, Body::from(body_bytes));
        if is_confidential_compute_endpoint(&endpoint) {
            confidential_compute_middleware(state, req, next).await
        } else {
            authenticate_middleware(state, req, next).await
        }
    } else {
        Ok(response)
    }
}

pub mod auth {
    use std::sync::Arc;
    use std::time::Duration;

    use atoma_auth::StackEntryResponse;
    use atoma_auth::Sui;
    use atoma_state::types::Stack;
    use atoma_state::{timestamp_to_datetime_or_now, types::AtomaAtomaStateManagerEvent};
    use axum::http::HeaderMap;
    use flume::Sender;
    use serde_json::Value;
    use sui_sdk::types::digests::TransactionDigest;
    use tokio::sync::{oneshot, RwLock};
    use tracing::instrument;

    use crate::server::handlers::chat_completions::RequestModelChatCompletions;
    use crate::server::handlers::chat_completions::CHAT_COMPLETIONS_PATH;
    use crate::server::handlers::embeddings::RequestModelEmbeddings;
    use crate::server::handlers::embeddings::EMBEDDINGS_PATH;
    use crate::server::handlers::image_generations::RequestModelImageGenerations;
    use crate::server::handlers::image_generations::CONFIDENTIAL_IMAGE_GENERATIONS_PATH;
    use crate::server::handlers::image_generations::IMAGE_GENERATIONS_PATH;
    use crate::server::handlers::request_model::ComputeUnitsEstimate;
    use crate::server::http_server::UserId;
    use crate::server::{
        check_auth, error::AtomaProxyError, handlers::request_model::RequestModel,
        http_server::ProxyState, Result, ONE_MILLION,
    };

    use super::acquire_stack_lock;
    use super::acquire_stack_lock::LockGuard;
    use super::STACK_SIZE_TO_BUY;

    /// The maximum time to wait for a stack to be created, on the Sui blockchain,
    /// and the corresponding event to be emitted by the Sui blockchain and captured
    /// by the Proxy's state manager.
    const MAX_STACK_WAIT_TIME: Duration = Duration::from_millis(300);

    /// The maximum number of attempts to wait for a stack to be created.
    const MAX_STACK_WAIT_ATTEMPTS: usize = 10;

    /// Metadata about the stack that was selected for the request.
    /// This is used to update the stack's num_tokens after the request is processed.  
    #[derive(Clone, Debug)]
    pub struct StackMetadata {
        /// The stack that was selected for the request.
        pub optional_stack: Option<Stack>,
        /// The number of input compute units for the request.
        pub num_input_compute_units: u64,
        /// The maximum total compute units for the request.
        pub max_total_compute_units: u64,
        /// The model that was selected for the request.
        pub model: String,
        /// The user ID that made the request.
        pub user_id: i64,
    }

    /// Handles authentication and compute unit locking for incoming API requests.
    ///
    /// This function serves as a routing layer that processes different types of API requests
    /// (chat completions, embeddings, and image generations) by:
    /// 1. Validating the request body against the appropriate model type
    /// 2. Authenticating the request
    /// 3. Locking the required compute units for processing
    ///
    /// # Arguments
    ///
    /// * `state` - Reference to the proxy server state containing shared resources
    /// * `headers` - HTTP headers from the incoming request, used for authentication
    /// * `body_json` - The parsed JSON body of the request
    /// * `endpoint` - The API endpoint path being accessed (e.g., "/v1/chat/completions")
    ///
    /// # Returns
    ///
    /// Returns a `Result<StackMetadata>` where:
    /// * `Ok(StackMetadata)` - Authentication succeeded and compute units were locked
    /// * `Err(AtomaProxyError)` - Processing failed with specific error details
    ///
    /// # Errors
    ///
    /// Returns `AtomaProxyError` in the following cases:
    /// * `InvalidBody` - Request body doesn't match the expected model format
    /// * `InternalError` - Unexpected endpoint or internal processing failure
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use axum::http::HeaderMap;
    /// use serde_json::json;
    ///
    /// async fn process_chat_request(state: &ProxyState) -> Result<Option<Stack>> {
    ///     let headers = HeaderMap::new();
    ///     let body = json!({
    ///         "model": "gpt-4",
    ///         "messages": [{"role": "user", "content": "Hello"}]
    ///     });
    ///
    ///     handle_authenticate_and_lock_compute_units(
    ///         state,
    ///         &headers,
    ///         &body,
    ///         "/v1/chat/completions"
    ///     ).await
    /// }
    /// ```
    ///
    /// # Supported Endpoints
    ///
    /// * `CHAT_COMPLETIONS_PATH` - For chat completion requests
    /// * `EMBEDDINGS_PATH` - For text embedding requests
    /// * `IMAGE_GENERATIONS_PATH` - For image generation requests
    ///
    /// # Request Flow
    ///
    /// 1. Matches the endpoint to determine request type
    /// 2. Parses request body into appropriate model struct
    /// 3. Authenticates request and locks compute units
    /// 4. Returns stack information if successful
    ///
    /// # Security Considerations
    ///
    /// * Ensures all requests are properly authenticated
    /// * Validates request body format before processing
    /// * Locks compute units to prevent resource exhaustion
    #[instrument(
        level = "info",
        skip_all,
        fields(endpoint = %endpoint),
        err
    )]
    pub async fn handle_authenticate_and_lock_compute_units(
        state: &ProxyState,
        headers: &HeaderMap,
        body_json: &Value,
        endpoint: &str,
    ) -> Result<StackMetadata> {
        match endpoint {
            CHAT_COMPLETIONS_PATH => {
                let request_model = RequestModelChatCompletions::new(body_json).map_err(|e| {
                    AtomaProxyError::RequestError {
                        message: format!(
                            "Failed to parse body as chat completions request model: {e}"
                        ),
                        endpoint: endpoint.to_string(),
                    }
                })?;
                authenticate_and_lock_compute_units(state, headers, request_model, endpoint).await
            }
            EMBEDDINGS_PATH => {
                let request_model = RequestModelEmbeddings::new(body_json).map_err(|e| {
                    AtomaProxyError::RequestError {
                        message: format!("Failed to parse body as embeddings request model: {e}"),
                        endpoint: endpoint.to_string(),
                    }
                })?;
                authenticate_and_lock_compute_units(state, headers, request_model, endpoint).await
            }
            IMAGE_GENERATIONS_PATH => {
                let request_model = RequestModelImageGenerations::new(body_json).map_err(|e| {
                    AtomaProxyError::RequestError {
                        message: format!(
                            "Failed to parse body as image generations request model: {e}"
                        ),
                        endpoint: endpoint.to_string(),
                    }
                })?;
                authenticate_and_lock_compute_units(state, headers, request_model, endpoint).await
            }
            _ => {
                return Err(AtomaProxyError::InternalError {
                    message: format!(
                        "Invalid endpoint for current middleware, this should never happen: {endpoint}"
                    ),
                    client_message: None,
                    endpoint: endpoint.to_string(),
                });
            }
        }
    }

    /// Authenticates a request and attempts to lock compute units for model execution.
    ///
    /// This function performs several key operations in sequence:
    /// 1. Authenticates the user using provided headers
    /// 2. Estimates required compute units for the request
    /// 3. Attempts to find and lock available compute units from existing stacks
    ///
    /// # Arguments
    ///
    /// * `state` - Server state containing authentication and resource management components
    /// * `headers` - HTTP request headers containing authentication information
    /// * `request_model` - The parsed request model implementing the `RequestModel` trait
    /// * `endpoint` - The API endpoint path being accessed
    ///
    /// # Returns
    ///
    /// Returns a `Result<StackMetadata>` where:
    /// * `Ok(StackMetadata)` - Authentication succeeded and compute units were successfully locked
    /// * `Err(AtomaProxyError)` - Authentication or compute unit locking failed
    ///
    /// # Errors
    ///
    /// Returns `AtomaProxyError` in the following cases:
    /// * Authentication failure
    /// * Failed to estimate compute units
    /// * Failed to communicate with state manager
    /// * Failed to lock compute units
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use axum::http::HeaderMap;
    ///
    /// async fn process_request(state: &ProxyState, headers: &HeaderMap) -> Result<()> {
    ///     let request_model = ChatCompletionsModel::new(&body)?;
    ///     let result = authenticate_and_lock_compute_units(
    ///         state,
    ///         headers,
    ///         request_model,
    ///         "/v1/chat/completions"
    ///     ).await?;
    ///
    ///     match result {
    ///         Some(stack) => println!("Compute units locked on stack {}", stack.id),
    ///         None => println!("No suitable stack found"),
    ///     }
    ///     Ok(())
    /// }
    /// ```
    ///
    /// # Implementation Notes
    ///
    /// * The function is instrumented with tracing at the info level
    /// * Non-confidential compute is assumed (is_confidential is hardcoded to false)
    /// * Compute units are estimated based on the specific request model implementation
    #[instrument(
        level = "info",
        skip_all,
        fields(endpoint = %endpoint),
        err
    )]
    pub async fn authenticate_and_lock_compute_units(
        state: &ProxyState,
        headers: &HeaderMap,
        request_model: impl RequestModel + Send,
        endpoint: &str,
    ) -> Result<StackMetadata> {
        let user_id = check_auth(&state.state_manager_sender, headers, endpoint).await?;

        // Retrieve the model and the appropriate tokenizer
        let model = request_model.get_model();
        let ComputeUnitsEstimate {
            num_input_compute_units,
            max_total_compute_units,
        } = if [IMAGE_GENERATIONS_PATH, CONFIDENTIAL_IMAGE_GENERATIONS_PATH].contains(&endpoint) {
            request_model.get_compute_units_estimate(None)?
        } else {
            let tokenizer_index =
                state
                    .models
                    .iter()
                    .position(|m| m == &model)
                    .ok_or_else(|| AtomaProxyError::RequestError {
                        message: "Model not supported".to_string(),
                        endpoint: CHAT_COMPLETIONS_PATH.to_string(),
                    })?;
            let tokenizer = state.tokenizers[tokenizer_index].clone();
            request_model.get_compute_units_estimate(Some(&tokenizer))?
        };

        let (result_sender, result_receiver) = oneshot::channel();

        state
            .state_manager_sender
            .send(AtomaAtomaStateManagerEvent::GetStacksForModel {
                model: model.to_string(),
                free_compute_units: max_total_compute_units as i64,
                user_id,
                is_confidential: false, // NOTE: This method is only used for non-confidential compute
                result_sender,
            })
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to send GetStacksForModel event: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;

        let optional_stack = result_receiver
            .await
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to receive GetStacksForModel result: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to get GetStacksForModel result: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;

        Ok(StackMetadata {
            optional_stack,
            num_input_compute_units,
            max_total_compute_units,
            model,
            user_id,
        })
    }

    /// Attempts to retrieve stack metadata for a user's stack based on the endpoint type
    ///
    /// This function serves as a routing layer that handles different API endpoints
    /// (chat completions, embeddings, and image generations) by parsing their specific
    /// request models and delegating to a common stack retrieval implementation.
    ///
    /// # Arguments
    /// * `state` - Reference to the ProxyState containing application state
    /// * `user_id` - The ID of the user requesting the stack
    /// * `task_small_id` - The small ID of the task to be fetched
    /// * `request_model` - Implementation of RequestModel trait containing request details
    /// * `endpoint` - The API endpoint being accessed
    ///
    /// # Returns
    /// * `Result<Option<SelectedNodeMetadata>>` - Stack metadata if successful
    ///
    /// # Error Conditions
    /// * Returns `AtomaProxyError::RequestError` if:
    ///   - The endpoint is not supported
    ///   - Request body parsing fails for any endpoint type
    ///   - Underlying stack retrieval fails
    #[instrument(level = "info", skip_all, fields(user_id = %user_id, endpoint = %endpoint), err)]
    async fn try_get_stack_for_user_id(
        state: &ProxyState,
        user_id: UserId,
        task_small_id: i64,
        request_model: impl RequestModel + Send,
        endpoint: &str,
        total_tokens: u64,
    ) -> Result<Option<SelectedNodeMetadata>> {
        let (result_sender, result_receiver) = oneshot::channel();

        state
            .state_manager_sender
            .send(AtomaAtomaStateManagerEvent::GetStacksForTask {
                task_small_id,
                free_compute_units: total_tokens as i64,
                user_id,
                result_sender,
            })
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to send GetStacksForTask event: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;

        let optional_stack = result_receiver
            .await
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to receive GetStacksForTask result: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to get GetStacksForTask result: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;
        if let Some(stack) = optional_stack {
            Ok(Some(SelectedNodeMetadata {
                stack_small_id: stack.stack_small_id,
                selected_node_id: stack.selected_node_id,
                tx_digest: None,
            }))
        } else {
            Ok(None)
        }
    }

    /// Represents the processed and validated request data after authentication and initial processing.
    ///
    /// This struct contains all the necessary information needed to forward a request to an inference node,
    /// including authentication details, routing information, and request metadata.
    #[derive(Debug)]
    pub struct ProcessedRequest {
        /// The public address of the selected inference node
        pub node_address: String,
        /// The authentication signature for the request
        pub signature: String,
    }

    /// Authenticates the request and processes initial steps up to signature creation.
    ///
    /// # Arguments
    ///
    /// * `state` - The proxy state containing models, and other shared state
    /// * `headers` - Request headers containing authorization
    /// * `payload` - Request payload containing model and token information
    ///
    /// # Returns
    ///
    /// Returns a `ProcessedRequest` containing:
    /// - `node_address`: Public address of the selected inference node
    /// - `node_id`: Unique identifier for the selected node
    /// - `signature`: Sui signature for request authentication
    /// - `stack_small_id`: Identifier for the selected processing stack
    /// - `headers`: Sanitized headers for forwarding (auth headers removed)
    /// - `total_tokens`: Estimated total token usage
    /// - `tx_digest`: Optional transaction digest if a new stack was created
    ///
    /// # Errors
    ///
    /// Returns `AtomaProxyError` error in the following cases:
    /// - `UNAUTHORIZED`: Invalid or missing authentication
    /// - `BAD_REQUEST`: Invalid payload format or unsupported model
    /// - `NOT_FOUND`: No available node address found
    /// - `INTERNAL_SERVER_ERROR`: Various internal processing failures
    #[instrument(level = "info", skip_all, err)]
    pub async fn process_selected_stack(
        state: &ProxyState,
        payload: &Value,
        selected_node_id: i64,
        endpoint: &str,
    ) -> Result<ProcessedRequest> {
        // Get node address
        let (result_sender, result_receiver) = oneshot::channel();
        state
            .state_manager_sender
            .send(AtomaAtomaStateManagerEvent::GetNodePublicAddress {
                node_small_id: selected_node_id,
                result_sender,
            })
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to send GetNodePublicAddress event: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;

        let node_address = result_receiver
            .await
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to receive GetNodePublicAddress result: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to get GetNodePublicAddress result: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?
            .ok_or_else(|| AtomaProxyError::NotFound {
                message: format!("No node address found for node {selected_node_id}"),
                endpoint: endpoint.to_string(),
            })?;

        // Get signature
        let signature = state
            .sui
            .write()
            .await
            .get_sui_signature(payload)
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to get Sui signature: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;

        Ok(ProcessedRequest {
            node_address,
            signature,
        })
    }

    /// Metadata returned when selecting a node for processing a model request
    #[derive(Debug)]
    pub struct SelectedNodeMetadata {
        /// The small ID of the stack
        pub stack_small_id: i64,
        /// The small ID of the selected node
        pub selected_node_id: i64,
        /// The transaction digest of the stack entry creation transaction
        pub tx_digest: Option<TransactionDigest>,
    }

    /// The result of acquiring a new stack entry.
    #[derive(Debug)]
    pub struct NewStackResult {
        /// The small ID of the stack
        pub stack_small_id: i64,
        /// The small ID of the selected node
        pub selected_node_id: i64,
        /// The transaction digest of the stack entry creation transaction
        pub tx_digest: TransactionDigest,
    }

    /// Acquires a new stack entry for the cheapest node.
    ///
    /// This function acquires for the given node.
    /// We spawn a tokio task to make sure that the function finishes in case the main task is killed.
    /// The new tokio task also captures the lock_guard, so it will be released when the task finishes,
    /// and the new stack acquisition operation has been completed.
    ///
    /// #Arguments
    ///
    /// * `node` - The cheapest node to acquire a stack for
    ///
    /// #Returns
    ///
    /// Returns a `NewStackResult` containing:
    /// * `stack_small_id` - The identifier for the selected/created stack
    /// * `selected_node_id` - The identifier for the node that will process the request
    #[instrument(level = "info", skip_all, err)]
    async fn acquire_new_stack(
        state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
        user_id: i64,
        lock_guard: LockGuard,
        endpoint: String,
        total_tokens: u64,
        sui: Arc<RwLock<Sui>>,
        node: atoma_state::types::CheapestNode,
    ) -> Result<NewStackResult> {
        // NOTE: This method is called only if there was no prior lock to an already existing stack
        // for the user. For this reason, it is safe to try to modify the underlying `DashMap
        let endpoint_clone = endpoint.clone();
        tokio::spawn(async move {
            // NOTE: Move the lock_guard into the spawned task.
            // This ensures the lock is held for the entire duration of this background task,
            // even if the original request handler that called `acquire_new_stack` is cancelled
            // (e.g., due to client disconnection). The `LockGuard`'s `Drop` implementation
            // will run only when this spawned task finishes, releasing the lock reliably
            // and preventing race conditions where multiple requests might try to acquire
            // a stack concurrently after premature lock release.
            let _moved_lock_guard = lock_guard;
            // 1. Deduct USDC from the user's balance. This will fail if the balance is not enough.
            deduct_usdc(
                state_manager_sender.clone(),
                user_id,
                node.price_per_one_million_compute_units as u64,
                STACK_SIZE_TO_BUY as u64,
                endpoint_clone.clone(),
            )
            .await?;
            // 2. Acquire a new stack on USDC deduction and send the NewStackAcquired event to the state manager.
            // NOTE: If acquiring a new stack fails, we will refund the USDC to the user's balance.
            acquire_new_stack_on_usdc_deduction_wrapper(AcquireNewStackArgs {
                state_manager_sender,
                sui,
                user_id,
                task_small_id: node.task_small_id as u64,
                stack_size_to_buy: STACK_SIZE_TO_BUY as u64,
                price_per_million_compute_units: node.price_per_one_million_compute_units as u64,
                endpoint: endpoint_clone.clone(),
                total_tokens,
            })
            .await
        })
        .await
        .map_err(|e| AtomaProxyError::InternalError {
            message: format!("Failed to acquire new stack: {e}"),
            client_message: None,
            endpoint,
        })?
    }

    /// Arguments for acquiring a new stack on USDC deduction.
    struct AcquireNewStackArgs {
        /// The sender for the state manager event.
        state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
        /// The Sui interface for blockchain operations.
        sui: Arc<RwLock<Sui>>,
        /// The user ID of the request.
        user_id: UserId,
        /// The small ID of the task that the user is requesting.
        task_small_id: u64,
        /// The size of the stack to buy.
        stack_size_to_buy: u64,
        /// The price per million compute units for the stack.
        price_per_million_compute_units: u64,
        /// The endpoint of the request.
        endpoint: String,
        /// The total number of tokens for the request.
        total_tokens: u64,
    }

    /// Wrapper for acquiring a new stack on USDC deduction.
    ///
    /// This function acquires a new stack on USDC deduction.
    /// If the stack is not acquired, it refunds the USDC to the user.
    ///
    /// # Arguments
    ///
    /// * `args` - The arguments for acquiring a new stack on USDC deduction.
    ///
    /// # Returns
    ///
    /// Returns a `NewStackResult` containing:
    /// * `stack_small_id` - The identifier for the selected/created stack
    /// * `selected_node_id` - The identifier for the node that will process the request
    /// * `tx_digest` - The transaction digest of the stack entry creation transaction
    ///
    /// # Errors
    ///
    /// Returns an error if the stack is not acquired or if the state manager event fails to store the stack creation event.
    #[instrument(level = "info", skip_all, fields(user_id = %args.user_id, endpoint = %args.endpoint), err)]
    async fn acquire_new_stack_on_usdc_deduction_wrapper(
        args: AcquireNewStackArgs,
    ) -> Result<NewStackResult> {
        let endpoint = args.endpoint.clone();
        let user_id = args.user_id;
        let state_manager_sender = args.state_manager_sender.clone();
        let price_per_million_compute_units = args.price_per_million_compute_units;
        match acquire_new_stack_on_usdc_deduction(args).await {
            Ok(result) => Ok(result),
            Err(e) => {
                tracing::error!("Failed to acquire new stack: {e}");
                match refund_usdc(
                    state_manager_sender,
                    user_id,
                    price_per_million_compute_units,
                    STACK_SIZE_TO_BUY as u64,
                    endpoint,
                )
                .await
                {
                    Ok(()) => (),
                    Err(e) => {
                        tracing::error!("Failed to refund USDC: {e}");
                    }
                }
                Err(e)
            }
        }
    }

    /// Acquires a new stack on USDC deduction.
    ///
    /// This function acquires a new stack on USDC deduction.
    ///
    /// # Arguments
    ///
    /// * `args` - The arguments for acquiring a new stack on USDC deduction.
    ///
    /// # Returns
    ///
    /// Returns a `NewStackResult` containing:
    /// * `stack_small_id` - The identifier for the selected/created stack
    /// * `selected_node_id` - The identifier for the node that will process the request
    /// * `tx_digest` - The transaction digest of the stack entry creation transaction
    ///
    /// # Errors
    ///
    /// Returns an error if the stack is not acquired or if the state manager event fails to store the stack creation event.
    #[instrument(level = "info", skip_all, fields(user_id = %args.user_id, endpoint = %args.endpoint), err)]
    async fn acquire_new_stack_on_usdc_deduction(
        args: AcquireNewStackArgs,
    ) -> Result<NewStackResult> {
        let AcquireNewStackArgs {
            state_manager_sender,
            sui,
            user_id,
            task_small_id,
            stack_size_to_buy,
            price_per_million_compute_units,
            endpoint,
            total_tokens,
        } = args;

        let StackEntryResponse {
            transaction_digest: tx_digest,
            stack_created_event: event,
            timestamp_ms,
        } = sui
            .write()
            .await
            .acquire_new_stack_entry(
                task_small_id,
                stack_size_to_buy,
                price_per_million_compute_units,
            )
            .await
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to acquire new stack entry: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;

        let stack_small_id = event.stack_small_id.inner as i64;
        let selected_node_id = event.selected_node_id.inner as i64;

        // Send the NewStackAcquired event to the state manager, so we have it in the DB.
        let (result_sender, result_receiver) = oneshot::channel();
        state_manager_sender
            .send(AtomaAtomaStateManagerEvent::NewStackAcquired {
                event,
                already_computed_units: total_tokens as i64,
                transaction_timestamp: timestamp_to_datetime_or_now(timestamp_ms),
                user_id,
                result_sender,
            })
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to send NewStackAcquired event: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;
        let _ = result_receiver
            .await
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to receive NewStackAcquired result: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;
        Ok(NewStackResult {
            stack_small_id,
            selected_node_id,
            tx_digest,
        })
    }

    /// Deducts USDC from the user's balance.
    ///
    /// This function deducts USDC from the user's balance.
    ///
    /// # Arguments
    ///
    /// * `state_manager_sender` - The sender for the state manager event.
    /// * `user_id` - The user ID of the request.
    /// * `amount` - The amount to deduct.
    /// * `price_per_one_million_compute_units` - The price per one million compute units for the stack.
    /// * `stack_size_to_buy` - The size of the stack to buy.
    /// * `endpoint` - The endpoint of the request.
    ///
    /// # Returns
    ///
    /// Returns a `Result<()>` indicating success or failure.
    ///
    /// # Errors
    ///
    /// Returns an error if the deduct fails.
    ///
    /// This function will return an error if:
    ///
    /// - The database query fails to execute.
    #[instrument(level = "info", skip_all, fields(user_id = %user_id, endpoint = %endpoint), err)]
    async fn deduct_usdc(
        state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
        user_id: UserId,
        price_per_one_million_compute_units: u64,
        stack_size_to_buy: u64,
        endpoint: String,
    ) -> Result<()> {
        let (result_sender, result_receiver) = oneshot::channel();
        state_manager_sender
            .send(AtomaAtomaStateManagerEvent::DeductFromUsdc {
                user_id,
                amount: (price_per_one_million_compute_units * stack_size_to_buy / ONE_MILLION)
                    as i64,
                result_sender,
            })
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to send DeductFromUsdc event: {err:?}"),
                client_message: None,
                endpoint: endpoint.clone(),
            })?;
        result_receiver
            .await
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to receive DeductFromUsdc result: {err:?}"),
                client_message: None,
                endpoint: endpoint.clone(),
            })?
            .map_err(|err| AtomaProxyError::BalanceError {
                message: format!("Balance error : {err:?}"),
                endpoint,
            })?;
        Ok(())
    }

    /// Refunds a USDC payment.
    ///
    /// This function refunds a USDC payment to the user in the `balance` table.
    ///
    /// # Arguments
    ///
    /// * `state_manager_sender` - The sender for the state manager event.
    /// * `user_id` - The user ID of the request.
    /// * `amount` - The amount to refund.
    /// * `endpoint` - The endpoint of the request.
    ///
    /// # Returns
    ///
    /// Returns a `Result<()>` indicating success or failure.
    ///
    /// # Errors
    ///
    /// Returns an error if the refund fails.
    ///
    /// This function will return an error if:
    ///
    /// - The database query fails to execute.
    #[instrument(level = "info", skip_all, fields(user_id = %user_id, endpoint = %endpoint), err)]
    async fn refund_usdc(
        state_manager_sender: Sender<AtomaAtomaStateManagerEvent>,
        user_id: UserId,
        price_per_one_million_compute_units: u64,
        stack_size_to_buy: u64,
        endpoint: String,
    ) -> Result<()> {
        let (result_sender, result_receiver) = oneshot::channel();
        state_manager_sender
            .send(AtomaAtomaStateManagerEvent::RefundUsdc {
                user_id,
                amount: (price_per_one_million_compute_units * stack_size_to_buy / ONE_MILLION)
                    as i64,
                result_sender,
            })
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to send RefundUsdc event: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;
        result_receiver
            .await
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to receive RefundUsdc result: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to refund USDC: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })
    }

    /// Retrieves stack metadata for a locked user stack based on the endpoint type
    ///
    /// This function serves as a routing layer that handles different API endpoints
    /// (chat completions, embeddings, and image generations) by parsing their specific
    /// request models and delegating to a common stack retrieval implementation.
    ///
    /// # Arguments
    /// * `state` - Reference to the ProxyState containing application state
    /// * `user_id` - The ID of the user requesting the stack
    /// * `task_small_id` - The small ID of the task that the user is requesting
    /// * `body_json` - The raw JSON request body as a serde_json Value
    /// * `endpoint` - The API endpoint being accessed
    ///
    /// # Returns
    /// * `Result<SelectedNodeMetadata>` - Stack metadata if successful
    ///
    /// # Error Conditions
    /// * Returns `AtomaProxyError::RequestError` if:
    ///   - The endpoint is not supported
    ///   - Request body parsing fails for any endpoint type
    ///   - Underlying stack retrieval fails
    ///
    /// # Supported Endpoints
    /// * `CHAT_COMPLETIONS_PATH` - Handles chat completion requests
    /// * `EMBEDDINGS_PATH` - Handles embedding generation requests
    /// * `IMAGE_GENERATIONS_PATH` - Handles image generation requests
    ///
    /// # Implementation Details
    /// * Matches on the endpoint type to determine the appropriate request model
    /// * Parses the request body into the corresponding model type
    /// * Delegates to get_stack_if_locked_with_request_model for actual stack retrieval
    #[instrument(level = "info", skip_all, fields(user_id = %user_id, endpoint = %endpoint), err)]
    async fn get_stack_if_locked(
        state: &ProxyState,
        user_id: i64,
        task_small_id: i64,
        body_json: &Value,
        endpoint: &str,
        total_tokens: u64,
    ) -> Result<SelectedNodeMetadata> {
        match endpoint {
            CHAT_COMPLETIONS_PATH => {
                let request_model = RequestModelChatCompletions::new(body_json).map_err(|err| {
                    AtomaProxyError::RequestError {
                        message: format!("Failed to parse chat completions request: {err:?}"),
                        endpoint: endpoint.to_string(),
                    }
                })?;
                get_stack_if_locked_with_request_model(
                    state,
                    user_id,
                    task_small_id,
                    request_model,
                    endpoint,
                    total_tokens,
                )
                .await
            }
            EMBEDDINGS_PATH => {
                let request_model = RequestModelEmbeddings::new(body_json).map_err(|err| {
                    AtomaProxyError::RequestError {
                        message: format!("Failed to parse embeddings request: {err:?}"),
                        endpoint: endpoint.to_string(),
                    }
                })?;
                get_stack_if_locked_with_request_model(
                    state,
                    user_id,
                    task_small_id,
                    request_model,
                    endpoint,
                    total_tokens,
                )
                .await
            }
            IMAGE_GENERATIONS_PATH => {
                let request_model =
                    RequestModelImageGenerations::new(body_json).map_err(|err| {
                        AtomaProxyError::RequestError {
                            message: format!("Failed to parse image generations request: {err:?}"),
                            endpoint: endpoint.to_string(),
                        }
                    })?;
                get_stack_if_locked_with_request_model(
                    state,
                    user_id,
                    task_small_id,
                    request_model,
                    endpoint,
                    total_tokens,
                )
                .await
            }
            _ => {
                return Err(AtomaProxyError::RequestError {
                    message: format!("Unsupported endpoint: {endpoint}"),
                    endpoint: endpoint.to_string(),
                })
            }
        }
    }

    /// Attempts to retrieve stack metadata if a user's stack is currently locked during purchase
    ///
    /// This function handles concurrent stack purchase requests by implementing a retry mechanism
    /// when a user's stack is locked. It's designed to wait for an ongoing stack purchase
    /// transaction to complete on the Sui blockchain.
    ///
    /// # Arguments
    /// * `state` - Reference to the ProxyState containing application state
    /// * `user_id` - The ID of the user requesting the stack
    /// * `task_small_id` - The small ID of the task that the user is requesting
    /// * `request_model` - Implementation of RequestModel trait containing request details
    /// * `endpoint` - The API endpoint being accessed
    ///
    /// # Returns
    /// * `Result<SelectedNodeMetadata>` - Stack metadata if successful
    ///
    /// # Error Conditions
    /// * Returns `AtomaProxyError::RequestError` if the maximum number of retry attempts is
    ///   exceeded and the stack is still not available
    ///
    /// # Implementation Details
    /// * Checks if the user's stack is locked using the users_buy_stack_lock_map
    /// * If locked, retries up to MAX_STACK_WAIT_ATTEMPTS times with MAX_STACK_WAIT_TIME delay
    /// * Each retry attempts to fetch the stack metadata via try_get_stack_for_user_id
    /// * We don't wait for the Sui blockchain to finalize the stack creation, as we store the stack creation event
    ///   right after the call to the Sui blockchain, and we don't wait to catch the event from the Sui blockchain,
    ///   as this can take extra time (roughly 300ms following Sui's Mysticeti finality times).
    #[instrument(level = "info", skip_all, fields(user_id = %user_id, endpoint = %endpoint), err)]
    async fn get_stack_if_locked_with_request_model(
        state: &ProxyState,
        user_id: i64,
        task_small_id: i64,
        request_model: impl RequestModel + Send,
        endpoint: &str,
        total_tokens: u64,
    ) -> Result<SelectedNodeMetadata> {
        let stack_is_locked = {
            state
                .users_buy_stack_lock_map
                .get(&(user_id, task_small_id))
                .is_some_and(|lock| *lock)
        };
        if stack_is_locked {
            // NOTE: This means a concurrent request is already buying a stack, so we wait for it to finish,
            // and for the stack creation event to be stored on the AtomaStateManager's internal state, as
            // we store it right after the call to the Sui blockchain and don't wait to catch the event
            // from the Sui blockchain, as this can take extra time (roughly 300ms following Sui's Mysticeti
            // finality times). We will try again, for a fixed number of times. If the stack is still not created,
            // we return an error.
            for _ in 0..MAX_STACK_WAIT_ATTEMPTS {
                let stack_metadata = try_get_stack_for_user_id(
                    state,
                    user_id,
                    task_small_id,
                    request_model.clone(),
                    endpoint,
                    total_tokens,
                )
                .await?;
                if let Some(stack_metadata) = stack_metadata {
                    return Ok(stack_metadata);
                }
                tokio::time::sleep(MAX_STACK_WAIT_TIME).await;
            }
        }
        Err(AtomaProxyError::RequestError {
            message: format!("User {user_id} has already bought a stack"),
            endpoint: endpoint.to_string(),
        })
    }

    /// Arguments for the get_selected_node function.
    pub struct GetSelectedNodeArgs<'a> {
        /// The name/identifier of the AI model being requested
        pub model: &'a str,
        /// The state of the proxy
        pub state: &'a ProxyState,
        /// The raw JSON request body as a serde_json Value
        pub body_json: &'a Value,
        /// The Sui interface for blockchain operations
        pub sui: &'a Arc<RwLock<Sui>>,
        /// The optional stack to use for the request
        pub optional_stack: Option<Stack>,
        /// The total number of compute units (tokens) needed for the request
        pub total_tokens: u64,
        /// The user ID of the request
        pub user_id: i64,
        /// The endpoint of the request
        pub endpoint: &'a str,
    }

    /// Selects a node for processing a model request by either finding an existing stack or acquiring a new one.
    ///
    /// This function acquires for the given node.
    /// We spawn a tokio thread to make sure that the function finishes in case the thread is killed.
    ///
    /// #Arguments
    ///
    /// * `node` - The cheapest node to acquire a stack for
    ///
    /// #Returns
    ///
    /// Returns a `NewStackResult` containing:
    /// * `stack_small_id` - The identifier for the selected/created stack
    /// * `selected_node_id` - The identifier for the node that will process the request
    /// * `tx_digest` - Optional transaction digest if a new stack was created
    ///
    /// # Errors
    ///
    /// Returns a `AtomaProxyError` error in the following cases:
    /// * `INTERNAL_SERVER_ERROR` - Communication errors with state manager or Sui interface
    /// * `NOT_FOUND` - No tasks available for the requested model
    /// * `BAD_REQUEST` - Requested compute units exceed the maximum allowed limit
    ///
    /// # Example
    ///
    /// ```no_run
    /// let metadata = get_selected_node(
    ///     "gpt-4",
    ///     &state_manager_sender,
    ///     &sui,
    ///     1000
    /// ).await?;
    /// println!("Selected stack ID: {}", metadata.stack_small_id);
    /// ```
    #[instrument(level = "info", skip_all, fields(model =%args.model), err)]
    pub async fn get_selected_node<'a>(
        args: GetSelectedNodeArgs<'a>,
    ) -> Result<SelectedNodeMetadata> {
        let GetSelectedNodeArgs {
            model,
            state,
            body_json,
            sui,
            optional_stack,
            total_tokens,
            user_id,
            endpoint,
        } = args;
        if let Some(stack) = optional_stack {
            return Ok(SelectedNodeMetadata {
                stack_small_id: stack.stack_small_id,
                selected_node_id: stack.selected_node_id,
                tx_digest: None,
            });
        }
        // WARN: This temporary check is to prevent users from trying to buy more compute units than the allowed stack size,
        // by the smart contract. If we update the smart contract to not force a maximum stack size, we SHOULD revision this check constraint.
        if total_tokens > STACK_SIZE_TO_BUY as u64 {
            return Err(AtomaProxyError::RequestError {
                    message: format!(
                        "Total tokens {total_tokens} exceed the maximum stack size of {STACK_SIZE_TO_BUY}"
                    ),
                    endpoint: endpoint.to_string(),
                });
        }
        let (result_sender, result_receiver) = oneshot::channel();
        state
            .state_manager_sender
            .send(AtomaAtomaStateManagerEvent::GetCheapestNodeForModel {
                model: model.to_string(),
                is_confidential: false,
                result_sender,
            })
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to send GetTasksForModel event: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;
        let node = result_receiver
            .await
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to receive GetTasksForModel result: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to get GetTasksForModel result: {err:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;
        let node: atoma_state::types::CheapestNode = match node {
            Some(node) => node,
            None => {
                return Err(AtomaProxyError::RequestError {
                    message: format!("No node found for model {model}"),
                    endpoint: endpoint.to_string(),
                });
            }
        };
        let Some(lock_guard) = acquire_stack_lock::LockGuard::try_lock(
            &state.users_buy_stack_lock_map,
            (user_id, node.task_small_id),
        ) else {
            // NOTE: Failed to acquire stack lock (meaning, we are in a race condition scenario)
            // so we try to get the stack from the state manager, and if it is not found, we return an error.
            return get_stack_if_locked(
                state,
                user_id,
                node.task_small_id,
                body_json,
                endpoint,
                total_tokens,
            )
            .await;
        };
        // NOTE: At this point, we have an acquired stack lock, so we can safely acquire a new stack.
        let NewStackResult {
            stack_small_id,
            selected_node_id,
            tx_digest,
        } = acquire_new_stack(
            state.state_manager_sender.clone(),
            user_id,
            lock_guard,
            endpoint.to_string(),
            total_tokens,
            Arc::clone(sui),
            node,
        )
        .await?;
        // NOTE: The `acquire_new_stack` method will emit a stack creation event, and it will stored it
        // in the AtomaStateManager's internal state, therefore any new request querying the state manager after this
        // lock guard release will see the new stack.
        // NOTE: When the `lock_guard` goes out of scope, it ensures that the `DashMap` entry is removed,
        // even if the `acquire_new_stack` returned an error, previously, as this is handled at drop time.

        Ok(SelectedNodeMetadata {
            stack_small_id,
            selected_node_id,
            tx_digest: Some(tx_digest),
        })
    }
}

pub mod utils {
    use flume::Sender;
    use sui_sdk::types::digests::TransactionDigest;

    use crate::server::{
        handlers::{
            chat_completions::CONFIDENTIAL_CHAT_COMPLETIONS_PATH,
            embeddings::CONFIDENTIAL_EMBEDDINGS_PATH,
            image_generations::CONFIDENTIAL_IMAGE_GENERATIONS_PATH,
        },
        MODEL,
    };

    use super::{
        auth, constants, instrument, AtomaAtomaStateManagerEvent, AtomaProxyError, Body,
        HeaderValue, Parts, ProcessedRequest, ProxyState, Request, RequestMetadataExtension,
        Result, State, Value, CONTENT_LENGTH, MAX_NUM_TOKENS_FOR_CONFIDENTIAL_COMPUTE,
    };

    /// Validates and prepares a request for processing by a specific stack and node.
    ///
    /// This function performs several key operations to prepare a request for forwarding:
    /// 1. Processes the selected stack to obtain node address and signature
    /// 2. Sets up required headers for node communication
    /// 3. Configures request metadata for tracking and routing
    ///
    /// # Arguments
    ///
    /// * `state` - Server state containing shared resources and connections
    /// * `body_json` - The parsed JSON body of the request
    /// * `req_parts` - Mutable reference to request parts for header modification
    /// * `selected_node_id` - ID of the node selected to process this request
    /// * `selected_stack_small_id` - ID of the stack allocated for this request
    /// * `total_compute_units` - Total compute units required for this request
    /// * `tx_digest` - Optional transaction digest if a new stack was created
    /// * `user_id` - ID of the user making the request
    /// * `endpoint` - API endpoint path being accessed
    ///
    /// # Returns
    ///
    /// Returns a `Result<Request<Body>>` containing the fully prepared request if successful.
    ///
    /// # Errors
    ///
    /// Returns `AtomaProxyError` in the following cases:
    /// * `InternalError` - Failed to:
    ///   - Convert values to header format
    ///   - Process selected stack
    ///   - Set up required headers
    /// * `InvalidBody` - Request body missing required "model" field
    ///
    /// # Headers Set
    ///
    /// The following headers are set on the request:
    /// * `X-Signature` - Authentication signature for the node
    /// * `X-Stack-Small-Id` - ID of the selected stack
    /// * `Content-Length` - Updated body length
    /// * `X-Tx-Digest` - (Optional) Transaction digest for new stacks
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// let prepared_request = try_validate_stack_for_request(
    ///     &state,
    ///     &body_json,
    ///     &mut req_parts,
    ///     node_id,
    ///     stack_id,
    ///     compute_units,
    ///     Some(tx_digest),
    ///     user_id,
    ///     "/v1/chat/completions"
    /// ).await?;
    /// ```
    ///
    /// # Request Metadata
    ///
    /// The function also sets up `RequestMetadataExtension` with:
    /// * Node address and ID
    /// * Compute units allocation
    /// * Stack ID
    /// * Endpoint path
    /// * Model name
    #[instrument(level = "info", skip_all, fields(
        %endpoint,
        %total_compute_units,
        %user_id
    ), err)]
    #[allow(clippy::too_many_arguments)]
    pub async fn try_validate_stack_for_request(
        state: &State<ProxyState>,
        body_json: &Value,
        req_parts: &mut Parts,
        selected_node_id: i64,
        selected_stack_small_id: i64,
        num_input_tokens: u64,
        total_compute_units: u64,
        tx_digest: Option<TransactionDigest>,
        user_id: i64,
        endpoint: &str,
    ) -> Result<Request<Body>> {
        let ProcessedRequest {
            node_address,
            signature,
        } = auth::process_selected_stack(state, body_json, selected_node_id, endpoint).await?;

        let stack_small_id_header = HeaderValue::from_str(&selected_stack_small_id.to_string())
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to convert stack small id to header value: {e:?}"),
                client_message: None,
                endpoint: req_parts.uri.path().to_string(),
            })?;
        let signature_header =
            HeaderValue::from_str(&signature).map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to convert signature to header value: {e:?}"),
                client_message: None,
                endpoint: req_parts.uri.path().to_string(),
            })?;
        let content_length_header = HeaderValue::from_str(&body_json.to_string().len().to_string())
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to convert content length to header value: {e:?}"),
                client_message: None,
                endpoint: req_parts.uri.path().to_string(),
            })?;
        req_parts
            .headers
            .insert(constants::SIGNATURE, signature_header);
        req_parts
            .headers
            .insert(constants::STACK_SMALL_ID, stack_small_id_header);
        req_parts
            .headers
            .insert(CONTENT_LENGTH, content_length_header);
        if let Some(tx_digest) = tx_digest {
            let tx_digest_header =
                HeaderValue::from_str(&tx_digest.base58_encode()).map_err(|e| {
                    AtomaProxyError::InternalError {
                        message: format!("Failed to convert tx digest to header value: {e:?}"),
                        client_message: None,
                        endpoint: req_parts.uri.path().to_string(),
                    }
                })?;
            req_parts
                .headers
                .insert(constants::TX_DIGEST, tx_digest_header);
        }
        let request_model = body_json
            .get(MODEL)
            .and_then(|m| m.as_str())
            .ok_or_else(|| AtomaProxyError::RequestError {
                message: "{MODEL} not found".to_string(),
                endpoint: req_parts.uri.path().to_string(),
            })?;

        req_parts.extensions.insert(RequestMetadataExtension {
            node_address,
            node_id: selected_node_id,
            num_input_tokens: Some(num_input_tokens),
            num_compute_units: total_compute_units,
            selected_stack_small_id,
            endpoint: endpoint.to_string(),
            model_name: request_model.to_string(),
        });

        // update headers
        let req = Request::from_parts(req_parts.clone(), Body::from(body_json.to_string()));
        Ok(req)
    }

    /// Verifies if a stack is valid for confidential compute operations.
    ///
    /// This function checks whether a given stack has sufficient compute units available
    /// and meets the requirements for confidential computing. It communicates with the
    /// state manager to verify the stack's eligibility.
    ///
    /// # Arguments
    ///
    /// * `state` - The proxy server state containing the state manager sender
    /// * `stack_small_id` - The unique identifier for the stack to verify
    /// * `available_compute_units` - The number of compute units required for the operation
    /// * `endpoint` - The API endpoint path making the verification request
    ///
    /// # Returns
    ///
    /// Returns a `Result<bool>` where:
    /// * `Ok(true)` - The stack is valid for confidential compute
    /// * `Err(AtomaProxyError)` - If verification fails or the stack is invalid
    ///
    /// # Errors
    ///
    /// Returns `AtomaProxyError::InternalError` in the following cases:
    /// * Failed to send verification request to state manager
    /// * Failed to receive verification response
    /// * Failed to process verification result
    /// * Stack is not valid for confidential compute
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use axum::extract::State;
    ///
    /// async fn verify_stack(state: State<ProxyState>) -> Result<()> {
    ///     let is_valid = verify_stack_for_confidential_compute(
    ///         state,
    ///         stack_small_id: 123,
    ///         available_compute_units: 1000,
    ///         endpoint: "/v1/confidential/chat/completions"
    ///     ).await?;
    ///
    ///     if is_valid {
    ///         println!("Stack is valid for confidential compute");
    ///     }
    ///     Ok(())
    /// }
    /// ```
    #[instrument(
        level = "info",
        skip_all,
        fields(
            %endpoint,
            %stack_small_id,
            %available_compute_units
        ),
        err
    )]
    pub async fn verify_stack_for_confidential_compute(
        state: &State<ProxyState>,
        stack_small_id: i64,
        available_compute_units: i64,
        endpoint: &str,
    ) -> Result<bool> {
        let (result_sender, result_receiver) = tokio::sync::oneshot::channel();
        state
            .state_manager_sender
            .send(
                AtomaAtomaStateManagerEvent::VerifyStackForConfidentialComputeRequest {
                    stack_small_id,
                    available_compute_units: MAX_NUM_TOKENS_FOR_CONFIDENTIAL_COMPUTE,
                    result_sender,
                },
            )
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to send GetNodePublicAddress event: {e:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;
        let is_valid = result_receiver
            .await
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!(
                    "Failed to receive VerifyStackForConfidentialComputeRequest result: {e:?}"
                ),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?
            .map_err(|e| AtomaProxyError::RequestError {
                message: format!("Failed to verify stack for confidential compute: {e:?}"),
                endpoint: endpoint.to_string(),
            })?;
        if !is_valid {
            return Err(AtomaProxyError::RequestError {
                message: "Stack is not valid for confidential compute".to_string(),
                endpoint: endpoint.to_string(),
            });
        }
        Ok(true)
    }

    /// Locks a specified number of compute units for a given stack.
    ///
    /// This function reserves compute units for a stack by sending a lock request to the state manager.
    /// The lock ensures that the compute units are exclusively reserved for this stack's use and cannot
    /// be allocated to other requests until released.
    ///
    /// # Arguments
    ///
    /// * `state` - The proxy server state containing the state manager channel
    /// * `stack_small_id` - The unique identifier for the stack requiring compute units
    /// * `available_compute_units` - The number of compute units to lock
    /// * `endpoint` - The API endpoint path making the lock request
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the compute units were successfully locked, or an error if the operation failed.
    ///
    /// # Errors
    ///
    /// Returns `AtomaProxyError::InternalError` in the following cases:
    /// * Failed to send lock request to state manager
    /// * Failed to receive lock response
    /// * Failed to acquire lock (e.g., insufficient available units)
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use axum::extract::State;
    ///
    /// async fn reserve_compute_units(state: State<ProxyState>) -> Result<()> {
    ///     lock_compute_units_for_stack(
    ///         &state,
    ///         stack_small_id: 123,
    ///         available_compute_units: 1000,
    ///         endpoint: "/v1/chat/completions"
    ///     ).await?;
    ///
    ///     // Compute units are now locked for this stack
    ///     Ok(())
    /// }
    /// ```
    #[instrument(
        level = "info",
        skip_all,
        fields(
            %endpoint,
            %stack_small_id,
            %available_compute_units
        ),
        err
    )]
    pub async fn lock_compute_units_for_stack(
        state: &State<ProxyState>,
        stack_small_id: i64,
        available_compute_units: i64,
        endpoint: &str,
    ) -> Result<()> {
        let (result_sender, result_receiver) = tokio::sync::oneshot::channel();
        state
            .state_manager_sender
            .send(AtomaAtomaStateManagerEvent::LockComputeUnitsForStack {
                stack_small_id,
                available_compute_units,
                result_sender,
            })
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to send LockComputeUnitsForStack event: {e:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;
        result_receiver
            .await
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to receive LockComputeUnitsForStack result: {e:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to lock compute units for stack: {e:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })
    }

    /// Retrieves the public URL and small ID for a node associated with a given stack.
    ///
    /// This function communicates with the state manager to fetch the node's public address
    /// and identifier based on the provided stack ID. It's typically used when setting up
    /// request routing to inference nodes.
    ///
    /// # Arguments
    ///
    /// * `state` - Server state containing the state manager channel and other shared resources
    /// * `stack_small_id` - Unique identifier for the stack whose node information is being requested
    /// * `endpoint` - The API endpoint path making the request (used for error context)
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing a tuple of:
    /// * `String` - The node's public URL/address
    /// * `i64` - The node's small ID
    ///
    /// # Errors
    ///
    /// Returns `AtomaProxyError::InternalError` in the following cases:
    /// * Failed to send request to state manager
    /// * Failed to receive response from state manager
    /// * Failed to process state manager response
    /// * No node address found for the given stack
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use axum::extract::State;
    ///
    /// async fn route_request(state: State<ProxyState>) -> Result<()> {
    ///     let (node_url, node_id) = get_node_address(
    ///         &state,
    ///         stack_small_id: 123,
    ///         endpoint: "/v1/chat/completions"
    ///     ).await?;
    ///
    ///     println!("Routing request to node {} at {}", node_id, node_url);
    ///     Ok(())
    /// }
    /// ```
    #[instrument(
        level = "info",
        skip_all,
        fields(%endpoint, %stack_small_id),
        err
    )]
    pub async fn get_node_address(
        state: &State<ProxyState>,
        stack_small_id: i64,
        endpoint: &str,
    ) -> Result<(String, i64)> {
        let (result_sender, result_receiver) = tokio::sync::oneshot::channel();
        state
            .state_manager_sender
            .send(AtomaAtomaStateManagerEvent::GetNodePublicUrlAndSmallId {
                stack_small_id,
                result_sender,
            })
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to send GetNodePublicAddress event: {e:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;
        let (node_address, node_small_id) = result_receiver
            .await
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to receive GetNodePublicAddress result: {e:?}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?
            .map_err(|e| AtomaProxyError::NotFound {
                message: format!("Failed to get node public address: {e:?}"),
                endpoint: endpoint.to_string(),
            })?;
        if let Some(node_address) = node_address {
            return Ok((node_address, node_small_id));
        }
        Err(AtomaProxyError::NotFound {
            message: "Node doesn't have public address".to_string(),
            endpoint: endpoint.to_string(),
        })
    }

    /// Locks the current stack in the Proxy's internal state.
    ///
    /// This function removes the stack small id from the request headers and parses it as an i64.
    /// It then sends a `LockStack` event to the state manager to lock the stack.
    ///
    /// # Arguments
    ///
    /// * `state_manager_sender` - Sender for the state manager channel
    /// * `req_parts` - The request parts containing the headers
    /// * `endpoint` - The API endpoint path making the request (used for error context)
    ///
    /// # Returns
    ///
    /// Returns `AtomaProxyError::InternalError` if the stack small id is not found in the request headers.
    /// Returns `AtomaProxyError::RequestError` if the stack small id is not found in the request headers.
    /// Returns `Ok(())` if the stack is locked successfully.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use axum::extract::State;
    ///
    /// async fn lock_stack(state: State<ProxyState>) -> Result<()> {
    ///     let (node_url, node_id) = get_node_address(
    ///         &state,
    ///         req_parts: &mut Parts,
    ///         endpoint: "/v1/chat/completions"
    ///     ).await?;
    /// }
    /// ```
    #[instrument(
        level = "info",
        skip_all,
        fields(
            endpoint = %endpoint
        ),
        err
    )]
    pub fn lock_stack(
        state_manager_sender: &Sender<AtomaAtomaStateManagerEvent>,
        req_parts: &mut Parts,
        endpoint: &str,
    ) -> Result<()> {
        let stack_small_id = req_parts
            .headers
            .remove(constants::STACK_SMALL_ID)
            .ok_or_else(|| AtomaProxyError::InternalError {
                message: "Stack small id not found".to_string(),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;
        let stack_small_id = stack_small_id
            .to_str()
            .map_err(|_| AtomaProxyError::RequestError {
                message: "Stack small id not found".to_string(),
                endpoint: endpoint.to_string(),
            })?
            .parse::<i64>()
            .map_err(|_| AtomaProxyError::RequestError {
                message: "handle_locked_stack_middleware: Could not parse stack small id"
                    .to_string(),
                endpoint: endpoint.to_string(),
            })?;
        tracing::info!(
            level = "info",
            name = "handle_locked_stack_middleware",
            stack_small_id = %stack_small_id,
            "Stack is in locked state for the requested node, trying to acquire a new stack"
        );
        state_manager_sender
            .send(AtomaAtomaStateManagerEvent::LockStack { stack_small_id })
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to send LockStack event: {e}"),
                client_message: None,
                endpoint: endpoint.to_string(),
            })?;
        Ok(())
    }

    /// Checks if the given endpoint is a confidential compute endpoint.
    ///
    /// This function checks if the provided endpoint string matches any of the
    /// confidential compute endpoints defined in the `handlers` module.
    ///
    /// # Arguments
    ///
    /// * `endpoint` - The endpoint to check
    ///
    /// # Returns
    ///
    /// Returns `true` if the endpoint is a confidential compute endpoint, otherwise `false`.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use crate::server::utils::is_confidential_compute_endpoint;
    ///
    /// let endpoint = "/v1/chat/completions";
    /// assert!(is_confidential_compute_endpoint(endpoint));
    /// ```
    pub fn is_confidential_compute_endpoint(endpoint: &str) -> bool {
        [
            CONFIDENTIAL_CHAT_COMPLETIONS_PATH,
            CONFIDENTIAL_EMBEDDINGS_PATH,
            CONFIDENTIAL_IMAGE_GENERATIONS_PATH,
        ]
        .contains(&endpoint)
    }
}

pub mod acquire_stack_lock {
    use std::sync::Arc;

    use dashmap::DashMap;
    use tracing::{info, instrument};

    use crate::server::http_server::{TaskId, UserId};

    /// A guard that locks a stack for a user id.
    ///
    /// This struct is used to lock a stack for a user id, so that no other concurrent requests can try to acquire a new stack,
    /// to avoid buying multiple redundant stacks, at the same time (that is in a window of 300ms, following Sui's Mysticeti fast finality estimation times).
    pub struct LockGuard {
        /// The map of user id and task id to lock status.
        map: Arc<DashMap<(UserId, TaskId), bool>>,
        /// The user id and task id to lock.
        key: (UserId, TaskId),
        /// Whether the lock is held.
        locked: bool,
    }

    impl LockGuard {
        /// Tries to lock a stack for a user id.
        ///
        /// This method checks if the user id is already locked in the map. If it is, it returns `None`,
        /// indicating that the stack is already locked. Otherwise, it locks the stack and returns a new `LockGuard`.
        ///
        /// # Arguments
        ///
        /// * `map` - The map of user id to lock status.
        /// * `key` - The user id to lock.
        ///
        /// # Returns
        ///
        /// Returns a new `LockGuard` if the stack is not locked, otherwise `None`.
        pub fn try_lock(
            map: &Arc<DashMap<(UserId, TaskId), bool>>,
            key: (UserId, TaskId),
        ) -> Option<Self> {
            match map.entry(key) {
                dashmap::mapref::entry::Entry::Occupied(_) => None, // Already locked
                dashmap::mapref::entry::Entry::Vacant(entry) => {
                    entry.insert(true);
                    Some(Self {
                        map: map.clone(),
                        key,
                        locked: true,
                    })
                }
            }
        }
    }

    impl Drop for LockGuard {
        /// Drops the lock guard and releases the lock.
        ///
        /// This method removes the lock from the map when the lock guard goes out of scope.
        /// It logs a message indicating that the lock has been released for the user id.
        #[instrument(level = "info", skip_all, fields(user_id = %self.key.0, task_small_id = %self.key.1))]
        fn drop(&mut self) {
            if self.locked {
                let previous_lock = self.map.remove(&self.key);
                if previous_lock.is_some() {
                    info!(
                        "Held lock has been released for user id: {} and task small id: {}",
                        self.key.0, self.key.1
                    );
                }
            }
        }
    }
}
