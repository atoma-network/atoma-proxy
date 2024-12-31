use atoma_state::types::AtomaAtomaStateManagerEvent;
use atoma_utils::constants;
use auth::{authenticate_and_process, ProcessedRequest};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{request::Parts, HeaderValue},
    middleware::Next,
    response::Response,
};
use base64::engine::{general_purpose::STANDARD, Engine};
use reqwest::header::CONTENT_LENGTH;
use serde_json::Value;
use tracing::instrument;

use super::{
    error::AtomaProxyError,
    handlers::{
        chat_completions::{
            RequestModelChatCompletions, CHAT_COMPLETIONS_PATH, CONFIDENTIAL_CHAT_COMPLETIONS_PATH,
        },
        embeddings::{RequestModelEmbeddings, CONFIDENTIAL_EMBEDDINGS_PATH, EMBEDDINGS_PATH},
        image_generations::{
            RequestModelImageGenerations, CONFIDENTIAL_IMAGE_GENERATIONS_PATH,
            IMAGE_GENERATIONS_PATH,
        },
        request_model::RequestModel,
        select_node_public_key::MAX_NUM_TOKENS_FOR_CONFIDENTIAL_COMPUTE,
    },
    http_server::ProxyState,
};
use super::{types::ConfidentialComputeRequest, Result};

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
    pub fn with_node_small_id(mut self, node_small_id: i64) -> Self {
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
    pub fn with_num_compute_units(mut self, num_compute_units: u64) -> Self {
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
    pub fn with_stack_small_id(mut self, stack_small_id: i64) -> Self {
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
    skip_all,
    fields(endpoint = %req.uri().path())
)]
pub async fn authenticate_middleware(
    state: State<ProxyState>,
    req: Request<Body>,
    next: Next,
) -> Result<Response> {
    let (mut req_parts, body) = req.into_parts();
    let body_bytes = axum::body::to_bytes(body, MAX_BODY_SIZE)
        .await
        .map_err(|e| AtomaProxyError::InternalError {
            message: format!("Failed to convert body to bytes: {}", e),
            endpoint: req_parts.uri.path().to_string(),
        })?;
    let body_json: Value =
        serde_json::from_slice(&body_bytes).map_err(|e| AtomaProxyError::InternalError {
            message: format!("Failed to parse body as JSON: {}", e),
            endpoint: req_parts.uri.path().to_string(),
        })?;
    let endpoint = req_parts.uri.path().to_string();
    let ProcessedRequest {
        node_address,
        node_id,
        signature,
        stack_small_id,
        mut headers,
        num_compute_units,
        tx_digest,
    } = utils::process_request(&state, &endpoint, &body_json, &mut req_parts).await?;
    let stack_small_id_header =
        HeaderValue::from_str(&stack_small_id.to_string()).map_err(|e| {
            AtomaProxyError::InternalError {
                message: format!("Failed to convert stack small id to header value: {}", e),
                endpoint: req_parts.uri.path().to_string(),
            }
        })?;
    let signature_header =
        HeaderValue::from_str(&signature).map_err(|e| AtomaProxyError::InternalError {
            message: format!("Failed to convert signature to header value: {}", e),
            endpoint: req_parts.uri.path().to_string(),
        })?;
    let content_length_header = HeaderValue::from_str(&body_json.to_string().len().to_string())
        .map_err(|e| AtomaProxyError::InternalError {
            message: format!("Failed to convert content length to header value: {}", e),
            endpoint: req_parts.uri.path().to_string(),
        })?;
    headers.insert(constants::SIGNATURE, signature_header);
    headers.insert(constants::STACK_SMALL_ID, stack_small_id_header);
    headers.insert(CONTENT_LENGTH, content_length_header);
    if let Some(tx_digest) = tx_digest {
        let tx_digest_header = HeaderValue::from_str(&tx_digest.base58_encode()).map_err(|e| {
            AtomaProxyError::InternalError {
                message: format!("Failed to convert tx digest to header value: {}", e),
                endpoint: req_parts.uri.path().to_string(),
            }
        })?;
        headers.insert(constants::TX_DIGEST, tx_digest_header);
    }
    let request_model =
        body_json
            .get("model")
            .and_then(|m| m.as_str())
            .ok_or(AtomaProxyError::InvalidBody {
                message: "Model not found".to_string(),
                endpoint: req_parts.uri.path().to_string(),
            })?;
    req_parts.extensions.insert(RequestMetadataExtension {
        node_address,
        node_id,
        num_compute_units,
        selected_stack_small_id: stack_small_id,
        endpoint: endpoint.clone(),
        model_name: request_model.to_string(),
    });

    // update headers
    req_parts.headers = headers;
    let req = Request::from_parts(req_parts, Body::from(body_bytes));
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
    skip_all,
    fields(endpoint = %req.uri().path())
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
        .map_err(|e| AtomaProxyError::InternalError {
            message: format!("Failed to convert body to bytes: {}", e),
            endpoint: endpoint.clone(),
        })?;
    let confidential_compute_request: ConfidentialComputeRequest =
        serde_json::from_slice(&body_bytes).map_err(|e| AtomaProxyError::InternalError {
            message: format!("Failed to parse body as JSON: {}", e),
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
        .map_err(|e| AtomaProxyError::InternalError {
            message: format!("Failed to decode plaintext body hash: {}", e),
            endpoint: endpoint.clone(),
        })?;
    let plaintext_body_signature = state
        .sui
        .write()
        .await
        .sign_hash(&plaintext_body_hash)
        .map_err(|e| AtomaProxyError::InternalError {
            message: format!("Failed to get Sui signature: {}", e),
            endpoint: endpoint.clone(),
        })?;
    let signature_header = HeaderValue::from_str(&plaintext_body_signature).map_err(|e| {
        AtomaProxyError::InternalError {
            message: format!("Failed to convert signature to header value: {}", e),
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
            .max_tokens
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

pub(crate) mod auth {
    use std::sync::Arc;

    use atoma_auth::StackEntryResponse;
    use atoma_auth::Sui;
    use atoma_state::{timestamp_to_datetime_or_now, types::AtomaAtomaStateManagerEvent};
    use axum::http::HeaderMap;
    use flume::Sender;
    use reqwest::header::AUTHORIZATION;
    use serde_json::Value;
    use sui_sdk::types::digests::TransactionDigest;
    use tokio::sync::{oneshot, RwLock};
    use tracing::instrument;

    use crate::server::Result;
    use crate::server::{
        error::AtomaProxyError, handlers::request_model::RequestModel, http_server::ProxyState,
    };

    /// Represents the processed and validated request data after authentication and initial processing.
    ///
    /// This struct contains all the necessary information needed to forward a request to an inference node,
    /// including authentication details, routing information, and request metadata.
    #[derive(Debug)]
    pub(crate) struct ProcessedRequest {
        /// The public address of the selected inference node
        pub node_address: String,
        /// The unique identifier for the selected node
        pub node_id: i64,
        /// The authentication signature for the request
        pub signature: String,
        /// The unique identifier for the selected stack
        pub stack_small_id: i64,
        /// HTTP headers to be forwarded with the request, excluding sensitive authentication headers
        pub headers: HeaderMap,
        /// The estimated number of compute units for this request (input + output)
        pub num_compute_units: u64,
        /// Optional transaction digest from stack entry creation, if a new stack was acquired
        pub tx_digest: Option<TransactionDigest>,
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
    #[instrument(level = "info", skip_all)]
    pub(crate) async fn authenticate_and_process(
        request_model: impl RequestModel,
        state: &ProxyState,
        headers: HeaderMap,
        payload: &Value,
        is_confidential: bool,
        endpoint: &str,
    ) -> Result<ProcessedRequest> {
        // Authenticate
        let user_id = check_auth(&state.state_manager_sender, &headers, endpoint).await?;

        // Estimate compute units and the request model
        let model = request_model.get_model()?;
        let total_compute_units = request_model.get_compute_units_estimate(state)?;

        // Get node selection
        let SelectedNodeMetadata {
            stack_small_id: selected_stack_small_id,
            selected_node_id,
            tx_digest,
        } = get_selected_node(
            &model,
            &state.state_manager_sender,
            &state.sui,
            total_compute_units,
            user_id,
            is_confidential,
            endpoint,
        )
        .await?;

        // Get node address
        let (result_sender, result_receiver) = oneshot::channel();
        state
            .state_manager_sender
            .send(AtomaAtomaStateManagerEvent::GetNodePublicAddress {
                node_small_id: selected_node_id,
                result_sender,
            })
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to send GetNodePublicAddress event: {:?}", err),
                endpoint: endpoint.to_string(),
            })?;

        let node_address = result_receiver
            .await
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to receive GetNodePublicAddress result: {:?}", err),
                endpoint: endpoint.to_string(),
            })?
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to get GetNodePublicAddress result: {:?}", err),
                endpoint: endpoint.to_string(),
            })?
            .ok_or_else(|| AtomaProxyError::NotFound {
                message: format!("No node address found for node {}", selected_node_id),
                endpoint: endpoint.to_string(),
            })?;

        // Get signature
        let signature = state
            .sui
            .write()
            .await
            .get_sui_signature(payload)
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to get Sui signature: {:?}", err),
                endpoint: endpoint.to_string(),
            })?;

        // Prepare headers
        let mut headers = headers;
        headers.remove(AUTHORIZATION);

        Ok(ProcessedRequest {
            node_address,
            node_id: selected_node_id,
            signature,
            stack_small_id: selected_stack_small_id,
            headers,
            num_compute_units: total_compute_units,
            tx_digest,
        })
    }

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
                            message: format!("Failed to send IsApiTokenValid event: {:?}", err),
                            endpoint: endpoint.to_string(),
                        })?;
                    return receiver
                        .await
                        .map_err(|err| AtomaProxyError::InternalError {
                            message: format!("Failed to receive IsApiTokenValid result: {:?}", err),
                            endpoint: endpoint.to_string(),
                        })?
                        .map_err(|err| AtomaProxyError::InternalError {
                            message: format!("Failed to get IsApiTokenValid result: {:?}", err),
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

    /// Selects a node for processing a model request by either finding an existing stack or acquiring a new one.
    ///
    /// This function follows a two-step process:
    /// 1. First, it attempts to find existing stacks that can handle the requested model and compute units
    /// 2. If no suitable stacks exist, it acquires a new stack entry by:
    ///    - Finding available tasks for the model
    ///    - Creating a new stack entry with predefined compute units and price
    ///    - Registering the new stack with the state manager
    ///
    /// # Arguments
    ///
    /// * `model` - The name/identifier of the AI model being requested
    /// * `state_manager_sender` - Channel for sending events to the state manager
    /// * `sui` - Reference to the Sui interface for blockchain operations
    /// * `total_tokens` - The total number of compute units (tokens) needed for the request
    ///
    /// # Returns
    ///
    /// Returns a `SelectedNodeMetadata` containing:
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
    #[instrument(level = "info", skip_all, fields(%model))]
    async fn get_selected_node(
        model: &str,
        state_manager_sender: &Sender<AtomaAtomaStateManagerEvent>,
        sui: &Arc<RwLock<Sui>>,
        total_tokens: u64,
        user_id: i64,
        is_confidential: bool,
        endpoint: &str,
    ) -> Result<SelectedNodeMetadata> {
        let (result_sender, result_receiver) = oneshot::channel();

        state_manager_sender
            .send(AtomaAtomaStateManagerEvent::GetStacksForModel {
                model: model.to_string(),
                free_compute_units: total_tokens as i64,
                user_id,
                is_confidential,
                result_sender,
            })
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to send GetStacksForModel event: {:?}", err),
                endpoint: endpoint.to_string(),
            })?;

        let optional_stack = result_receiver
            .await
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to receive GetStacksForModel result: {:?}", err),
                endpoint: endpoint.to_string(),
            })?
            .map_err(|err| AtomaProxyError::InternalError {
                message: format!("Failed to get GetStacksForModel result: {:?}", err),
                endpoint: endpoint.to_string(),
            })?;

        if let Some(stack) = optional_stack {
            Ok(SelectedNodeMetadata {
                stack_small_id: stack.stack_small_id,
                selected_node_id: stack.selected_node_id,
                tx_digest: None,
            })
        } else {
            let (result_sender, result_receiver) = oneshot::channel();
            state_manager_sender
                .send(AtomaAtomaStateManagerEvent::GetCheapestNodeForModel {
                    model: model.to_string(),
                    is_confidential,
                    result_sender,
                })
                .map_err(|err| AtomaProxyError::InternalError {
                    message: format!("Failed to send GetTasksForModel event: {:?}", err),
                    endpoint: endpoint.to_string(),
                })?;
            let node = result_receiver
                .await
                .map_err(|err| AtomaProxyError::InternalError {
                    message: format!("Failed to receive GetTasksForModel result: {:?}", err),
                    endpoint: endpoint.to_string(),
                })?
                .map_err(|err| AtomaProxyError::InternalError {
                    message: format!("Failed to get GetTasksForModel result: {:?}", err),
                    endpoint: endpoint.to_string(),
                })?;
            let node: atoma_state::types::CheapestNode = match node {
                Some(node) => node,
                None => {
                    return Err(AtomaProxyError::NotFound {
                        message: format!("No tasks found for model {}", model),
                        endpoint: endpoint.to_string(),
                    });
                }
            };
            // This will fail if the balance is not enough.
            let (result_sender, result_receiver) = oneshot::channel();
            state_manager_sender
                .send(AtomaAtomaStateManagerEvent::DeductFromUsdc {
                    user_id,
                    amount: node.price_per_one_million_compute_units * node.max_num_compute_units,
                    result_sender,
                })
                .map_err(|err| AtomaProxyError::InternalError {
                    message: format!("Failed to send DeductFromUsdc event: {:?}", err),
                    endpoint: endpoint.to_string(),
                })?;

            result_receiver
                .await
                .map_err(|err| AtomaProxyError::InternalError {
                    message: format!("Failed to receive DeductFromUsdc result: {:?}", err),
                    endpoint: endpoint.to_string(),
                })?
                .map_err(|err| AtomaProxyError::InternalError {
                    message: format!("Failed to get DeductFromUsdc result: {:?}", err),
                    endpoint: endpoint.to_string(),
                })?;
            let StackEntryResponse {
                transaction_digest: tx_digest,
                stack_created_event: event,
                timestamp_ms,
            } = sui
                .write()
                .await
                .acquire_new_stack_entry(
                    node.task_small_id as u64,
                    node.max_num_compute_units as u64,
                    node.price_per_one_million_compute_units as u64,
                )
                .await
                .map_err(|err| AtomaProxyError::InternalError {
                    message: format!("Failed to acquire new stack entry: {:?}", err),
                    endpoint: endpoint.to_string(),
                })?;

            let stack_small_id = event.stack_small_id.inner as i64;
            let selected_node_id = event.selected_node_id.inner as i64;

            // Send the NewStackAcquired event to the state manager, so we have it in the DB.
            state_manager_sender
                .send(AtomaAtomaStateManagerEvent::NewStackAcquired {
                    event,
                    already_computed_units: total_tokens as i64,
                    transaction_timestamp: timestamp_to_datetime_or_now(timestamp_ms),
                    user_id,
                })
                .map_err(|err| AtomaProxyError::InternalError {
                    message: format!("Failed to send NewStackAcquired event: {:?}", err),
                    endpoint: endpoint.to_string(),
                })?;

            Ok(SelectedNodeMetadata {
                stack_small_id,
                selected_node_id,
                tx_digest: Some(tx_digest),
            })
        }
    }
}

pub(crate) mod utils {
    use super::*;

    /// Processes an incoming API request by validating the request model and performing authentication.
    ///
    /// This function handles three types of requests:
    /// - Chat completions (both standard and confidential)
    /// - Embeddings (both standard and confidential)
    /// - Image generations (both standard and confidential)
    ///
    /// For each request type, it:
    /// 1. Parses and validates the request body into the appropriate model type
    /// 2. Authenticates the request and performs initial processing
    ///
    /// # Arguments
    ///
    /// * `state` - Server state containing shared resources and configuration
    /// * `endpoint` - The API endpoint path being accessed
    /// * `body_json` - The parsed JSON body of the request
    /// * `req_parts` - Mutable reference to the request parts containing headers and other metadata
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing:
    /// - `Ok(ProcessedRequest)`: Successfully processed request with node selection and authentication details
    /// - `Err(AtomaProxyError)`: Appropriate HTTP error status if processing fails
    ///
    /// # Errors
    ///
    /// Returns various status codes for different failure scenarios:
    /// * `BAD_REQUEST` (400):
    ///   - Invalid request model format
    ///   - Failed to parse request body
    /// * `UNAUTHORIZED` (401):
    ///   - Authentication failure
    /// * `NOT_FOUND` (404):
    ///   - Invalid or unsupported endpoint
    /// * `INTERNAL_SERVER_ERROR` (500):
    ///   - State manager communication failures
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let processed = process_request(
    ///     state,
    ///     CHAT_COMPLETIONS_PATH,
    ///     body_json,
    ///     &mut req_parts
    /// ).await?;
    /// ```
    pub(crate) async fn process_request(
        state: &State<ProxyState>,
        endpoint: &str,
        body_json: &Value,
        req_parts: &mut Parts,
    ) -> Result<ProcessedRequest> {
        match endpoint {
            CHAT_COMPLETIONS_PATH | CONFIDENTIAL_CHAT_COMPLETIONS_PATH => {
                let request_model = RequestModelChatCompletions::new(body_json).map_err(|e| {
                    AtomaProxyError::InvalidBody {
                        message: format!(
                            "Failed to parse body as chat completions request model: {}",
                            e
                        ),
                        endpoint: endpoint.to_string(),
                    }
                })?;
                authenticate_and_process(
                    request_model,
                    &state.0,
                    req_parts.headers.clone(),
                    body_json,
                    endpoint == CONFIDENTIAL_CHAT_COMPLETIONS_PATH,
                    endpoint,
                )
                .await
            }
            EMBEDDINGS_PATH | CONFIDENTIAL_EMBEDDINGS_PATH => {
                let request_model = RequestModelEmbeddings::new(body_json).map_err(|e| {
                    AtomaProxyError::InvalidBody {
                        message: format!("Failed to parse body as embeddings request model: {}", e),
                        endpoint: endpoint.to_string(),
                    }
                })?;
                authenticate_and_process(
                    request_model,
                    &state.0,
                    req_parts.headers.clone(),
                    body_json,
                    endpoint == CONFIDENTIAL_EMBEDDINGS_PATH,
                    endpoint,
                )
                .await
            }
            IMAGE_GENERATIONS_PATH | CONFIDENTIAL_IMAGE_GENERATIONS_PATH => {
                let request_model = RequestModelImageGenerations::new(body_json).map_err(|e| {
                    AtomaProxyError::InvalidBody {
                        message: format!(
                            "Failed to parse body as image generations request model: {}",
                            e
                        ),
                        endpoint: endpoint.to_string(),
                    }
                })?;
                authenticate_and_process(
                    request_model,
                    &state.0,
                    req_parts.headers.clone(),
                    body_json,
                    endpoint == CONFIDENTIAL_IMAGE_GENERATIONS_PATH,
                    endpoint,
                )
                .await
            }
            _ => Err(AtomaProxyError::NotFound {
                message: format!("Invalid or unsupported endpoint: {}", endpoint),
                endpoint: endpoint.to_string(),
            }),
        }
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
        )
    )]
    pub(crate) async fn verify_stack_for_confidential_compute(
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
                message: format!("Failed to send GetNodePublicAddress event: {}", e),
                endpoint: endpoint.to_string(),
            })?;
        let is_valid = result_receiver
            .await
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!(
                    "Failed to receive VerifyStackForConfidentialComputeRequest result: {}",
                    e
                ),
                endpoint: endpoint.to_string(),
            })?
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to verify stack for confidential compute: {}", e),
                endpoint: endpoint.to_string(),
            })?;
        if !is_valid {
            return Err(AtomaProxyError::InternalError {
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
        )
    )]
    pub(crate) async fn lock_compute_units_for_stack(
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
                message: format!("Failed to send LockComputeUnitsForStack event: {}", e),
                endpoint: endpoint.to_string(),
            })?;
        result_receiver
            .await
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to receive LockComputeUnitsForStack result: {}", e),
                endpoint: endpoint.to_string(),
            })?
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to lock compute units for stack: {}", e),
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
        fields(%endpoint, %stack_small_id)
    )]
    pub(crate) async fn get_node_address(
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
                message: format!("Failed to send GetNodePublicAddress event: {}", e),
                endpoint: endpoint.to_string(),
            })?;
        let (node_address, node_small_id) = result_receiver
            .await
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to receive GetNodePublicAddress result: {}", e),
                endpoint: endpoint.to_string(),
            })?
            .map_err(|e| AtomaProxyError::InternalError {
                message: format!("Failed to get node public address: {}", e),
                endpoint: endpoint.to_string(),
            })?;
        if let Some(node_address) = node_address {
            return Ok((node_address, node_small_id));
        }
        Err(AtomaProxyError::InternalError {
            message: "Failed to get node public address".to_string(),
            endpoint: endpoint.to_string(),
        })
    }
}
