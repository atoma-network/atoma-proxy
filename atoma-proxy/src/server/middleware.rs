use atoma_state::types::AtomaAtomaStateManagerEvent;
use atoma_utils::{constants, encryption::encrypt_plaintext};
use auth::{authenticate_and_process, ProcessedRequest};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{request::Parts, HeaderMap, HeaderValue},
    middleware::Next,
    response::Response,
};
use base64::engine::{general_purpose::STANDARD, Engine};
use reqwest::header::CONTENT_LENGTH;
use reqwest::StatusCode;
use serde_json::Value;
use tracing::{error, instrument};
use x25519_dalek::PublicKey;

use super::{
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
    },
    http_server::ProxyState,
};

/// Maximum size of the body in bytes.
/// This is to prevent DoS attacks by limiting the size of the request body.
const MAX_BODY_SIZE: usize = 1024 * 1024; // 1MB

/// Size of the salt in bytes used for encryption.
/// A 16-byte (128-bit) salt provides sufficient randomness
/// for cryptographic operations.
const SALT_SIZE: usize = 16;

/// Size of the x25519 public key in bytes.
const X25519_PUBLIC_KEY_SIZE: usize = 32;

/// Metadata containing encryption details for secure node communication.
///
/// This struct holds the cryptographic parameters needed for encrypted communication
/// with inference nodes, specifically the public key for asymmetric encryption and
/// the nonce for symmetric encryption.
///
/// # Security Considerations
///
/// - The public key is used in X25519 key exchange to establish a shared secret
/// - The nonce should be unique for each encryption operation
/// - Both values are Base64-encoded for safe transmission in HTTP headers
pub struct NodeEncryptionMetadata {
    /// Base64-encoded X25519 public key used for key exchange
    pub ciphertext: Vec<u8>,
    /// Base64-encoded random nonce used for AES-GCM encryption
    pub nonce: [u8; constants::NONCE_SIZE],
}

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

    /// Optional salt used for encrypting this this request.
    pub salt: Option<[u8; SALT_SIZE]>,

    /// Optional node x25519 public key used for encrypting this this request.
    pub node_x25519_public_key: Option<PublicKey>,
}

impl RequestMetadataExtension {
    /// Adds a salt value to the request metadata.
    ///
    /// This method is used in confidential computing scenarios to attach
    /// a cryptographic salt that will be used for request encryption.
    ///
    /// # Arguments
    ///
    /// * `salt` - A 16-byte array containing the random salt value
    ///
    /// # Returns
    ///
    /// Returns self with the salt field populated, enabling method chaining
    ///
    /// # Example
    ///
    /// ```
    /// let metadata = RequestMetadataExtension {
    ///     // ... other fields ...
    /// }.with_salt([0u8; 16]);
    /// ```
    pub fn with_salt(mut self, salt: [u8; SALT_SIZE]) -> Self {
        self.salt = Some(salt);
        self
    }

    /// Adds an X25519 public key to the request metadata.
    ///
    /// This method is used in confidential computing scenarios to attach
    /// the node's public key that will be used for establishing secure
    /// end-to-end encrypted communication.
    ///
    /// # Arguments
    ///
    /// * `node_x25519_public_key` - A 32-byte array containing the node's X25519 public key
    ///
    /// # Returns
    ///
    /// Returns self with the public key field populated, enabling method chaining
    ///
    /// # Example
    ///
    /// ```
    /// let metadata = RequestMetadataExtension {
    ///     // ... other fields ...
    /// }.with_node_x25519_public_key([0u8; 32]);
    /// ```
    pub fn with_node_x25519_public_key(mut self, node_x25519_public_key: PublicKey) -> Self {
        self.node_x25519_public_key = Some(node_x25519_public_key);
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
) -> Result<Response, StatusCode> {
    let (mut req_parts, body) = req.into_parts();
    let body_bytes = axum::body::to_bytes(body, MAX_BODY_SIZE)
        .await
        .map_err(|_| {
            error!("Failed to convert body to bytes");
            StatusCode::BAD_REQUEST
        })?;
    let body_json: Value = serde_json::from_slice(&body_bytes).map_err(|_| {
        error!("Failed to parse body as JSON");
        StatusCode::BAD_REQUEST
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
            error!("Failed to convert stack small id to header value: {}", e);
            StatusCode::BAD_REQUEST
        })?;
    let signature_header = HeaderValue::from_str(&signature).map_err(|e| {
        error!("Failed to convert signature to header value: {}", e);
        StatusCode::BAD_REQUEST
    })?;
    let content_length_header = HeaderValue::from_str(&body_json.to_string().len().to_string())
        .map_err(|e| {
            error!("Failed to convert content length to header value: {}", e);
            StatusCode::BAD_REQUEST
        })?;
    headers.insert(constants::SIGNATURE, signature_header);
    headers.insert(constants::STACK_SMALL_ID, stack_small_id_header);
    headers.insert(CONTENT_LENGTH, content_length_header);
    if let Some(tx_digest) = tx_digest {
        let tx_digest_header = HeaderValue::from_str(&tx_digest.base58_encode()).map_err(|e| {
            error!("Failed to convert tx digest to header value: {}", e);
            StatusCode::BAD_REQUEST
        })?;
        headers.insert(constants::TX_DIGEST, tx_digest_header);
    }
    req_parts.extensions.insert(RequestMetadataExtension {
        node_address,
        node_id,
        num_compute_units,
        selected_stack_small_id: stack_small_id,
        endpoint: endpoint.clone(),
        ..Default::default()
    });
    utils::handle_confidential_compute_content(state, &mut headers, &endpoint, node_id).await?;
    // update headers
    req_parts.headers = headers;
    let req = Request::from_parts(req_parts, Body::from(body_bytes));
    Ok(next.run(req).await)
}

/// Middleware that handles confidential computing by encrypting request bodies using X25519 key exchange.
///
/// This middleware performs the following steps:
/// 1. Extracts the client's X25519 public key from the request headers
/// 2. Generates random salt and nonce values
/// 3. Computes a shared secret using the client's public key and server's private key
/// 4. Encrypts the request body using AES-GCM with the shared secret
/// 5. Adds encryption-related headers to the request
///
/// # Arguments
/// * `state` - Server state containing cryptographic keys
/// * `req` - Incoming HTTP request
/// * `next` - Next middleware in the chain
///
/// # Returns
/// Returns the processed response from downstream handlers, or a `BAD_REQUEST` status
/// if any cryptographic operations fail.
///
/// # Security Considerations
/// - Maximum body size is limited to 1MB to prevent DoS attacks
/// - Uses 128-bit salt and 96-bit nonce for AES-GCM encryption
/// - Implements X25519 key exchange for perfect forward secrecy
///
/// # Headers
/// ## Required Input Headers
/// - `X-Node-X25519-PublicKey`: Client's base64-encoded X25519 public key
///
/// ## Added Headers
/// - `X-Nonce`: Base64-encoded encryption nonce
/// - `X-Salt`: Base64-encoded salt
/// - `X-Node-X25519-PublicKey`: Server's base64-encoded X25519 public key
///
/// # Errors
/// Returns `StatusCode::BAD_REQUEST` (400) if:
/// - Required headers are missing or malformed
/// - Request body exceeds maximum size
/// - Any cryptographic operations fail
#[instrument(
    level = "info",
    skip_all,
    fields(endpoint = %req.uri().path())
)]
pub async fn confidential_compute_middleware(
    state: State<ProxyState>,
    req: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let (mut req_parts, body) = req.into_parts();
    let body_bytes = axum::body::to_bytes(body, MAX_BODY_SIZE)
        .await
        .map_err(|_| {
            error!("Failed to convert body to bytes");
            StatusCode::BAD_REQUEST
        })?;
    let body_json: Value = serde_json::from_slice(&body_bytes).map_err(|e| {
        error!("Failed to parse body as JSON: {}", e);
        StatusCode::BAD_REQUEST
    })?;
    let is_streaming = body_json
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let x25519_public_key_header = req_parts
        .headers
        .get(constants::NODE_X25519_PUBLIC_KEY)
        .ok_or_else(|| {
            error!("Missing x25519-public-key header");
            StatusCode::BAD_REQUEST
        })?;
    let x25519_public_key_str = x25519_public_key_header.to_str().map_err(|_| {
        error!("Invalid x25519-public-key header");
        StatusCode::BAD_REQUEST
    })?;
    let x25519_public_key_bytes: [u8; X25519_PUBLIC_KEY_SIZE] = STANDARD
        .decode(x25519_public_key_str)
        .map_err(|_| {
            error!("Invalid x25519-public-key header");
            StatusCode::BAD_REQUEST
        })?
        .try_into()
        .map_err(|_| {
            error!("Invalid x25519-public-key header");
            StatusCode::BAD_REQUEST
        })?;
    let x25519_public_key = PublicKey::from(x25519_public_key_bytes);
    let salt = rand::random::<[u8; SALT_SIZE]>();
    let shared_secret = state.compute_shared_secret(&x25519_public_key);

    let (encrypted_plaintext, nonce) = encrypt_plaintext(&body_bytes, &shared_secret, &salt)
        .map_err(|_| {
            error!("Failed to encrypt plaintext");
            StatusCode::BAD_REQUEST
        })?;
    let nonce_str = STANDARD.encode(nonce);
    let salt_str = STANDARD.encode(salt);
    let nonce_header = HeaderValue::from_str(&nonce_str).map_err(|e| {
        error!("Invalid nonce header: {}", e);
        StatusCode::BAD_REQUEST
    })?;
    let salt_header = HeaderValue::from_str(&salt_str).map_err(|e| {
        error!("Invalid salt header: {}", e);
        StatusCode::BAD_REQUEST
    })?;
    req_parts.headers.insert(constants::NONCE, nonce_header);
    req_parts.headers.insert(constants::SALT, salt_header);
    let proxy_x25519_public_key_header =
        HeaderValue::from_str(&STANDARD.encode(state.public_key().as_bytes())).map_err(|e| {
            error!("Invalid proxy x25519-public-key header: {}", e);
            StatusCode::BAD_REQUEST
        })?;
    req_parts.headers.insert(
        constants::PROXY_X25519_PUBLIC_KEY,
        proxy_x25519_public_key_header,
    );
    let body_json = serde_json::json!({
        "ciphertext": encrypted_plaintext,
        "stream": is_streaming,
    });
    let content_length_header = HeaderValue::from_str(&body_json.to_string().len().to_string())
        .map_err(|e| {
            error!("Failed to convert content length to header value: {}", e);
            StatusCode::BAD_REQUEST
        })?;
    req_parts
        .headers
        .insert(CONTENT_LENGTH, content_length_header);
    let request_metadata = req_parts
        .extensions
        .get::<RequestMetadataExtension>()
        .cloned()
        .unwrap_or_default()
        .with_salt(salt)
        .with_node_x25519_public_key(x25519_public_key);
    req_parts.extensions.insert(request_metadata);
    let req = Request::from_parts(req_parts, Body::from(body_json.to_string()));
    Ok(next.run(req).await)
}

pub(crate) mod auth {
    use std::sync::Arc;

    use atoma_state::types::AtomaAtomaStateManagerEvent;
    use axum::http::HeaderMap;
    use flume::Sender;
    use reqwest::{header::AUTHORIZATION, StatusCode};
    use serde_json::Value;
    use sui_sdk::types::digests::TransactionDigest;
    use tokio::sync::{oneshot, RwLock};
    use tracing::{error, instrument};

    use crate::{
        server::{handlers::request_model::RequestModel, http_server::ProxyState},
        sui::{StackEntryResponse, Sui},
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
    /// * `state` - The proxy state containing password, models, and other shared state
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
    /// Returns `StatusCode` error in the following cases:
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
    ) -> Result<ProcessedRequest, StatusCode> {
        // Authenticate
        if !check_auth(&state.password, &headers) {
            return Err(StatusCode::UNAUTHORIZED);
        }

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

        // Get signature
        let signature = state
            .sui
            .write()
            .await
            .get_sui_signature(payload)
            .map_err(|err| {
                error!("Failed to get Sui signature: {:?}", err);
                StatusCode::INTERNAL_SERVER_ERROR
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
    /// provided password with the `Authorization` header in the request.
    ///
    /// # Arguments
    ///
    /// * `password`: The password to check against.
    /// * `headers`: The headers of the request.
    ///
    /// # Returns
    ///
    /// Returns `true` if the authentication is successful, `false` otherwise.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let mut headers = HeaderMap::new();
    /// headers.insert("Authorization", "Bearer password".parse().unwrap());
    /// let password = "password";
    ///
    /// assert_eq!(check_auth(password, &headers), true);
    /// assert_eq!(check_auth("wrong_password", &headers), false);
    /// ```
    #[instrument(level = "info", skip_all)]
    fn check_auth(password: &str, headers: &HeaderMap) -> bool {
        if let Some(auth) = headers.get("Authorization") {
            if let Ok(auth) = auth.to_str() {
                if auth == format!("Bearer {}", password) {
                    return true;
                }
            }
        }
        error!("Invalid or missing password for request");
        false
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
    /// Returns a `StatusCode` error in the following cases:
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
    ) -> Result<SelectedNodeMetadata, StatusCode> {
        let (result_sender, result_receiver) = oneshot::channel();

        state_manager_sender
            .send(AtomaAtomaStateManagerEvent::GetStacksForModel {
                model: model.to_string(),
                free_compute_units: total_tokens as i64,
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

        if stacks.is_empty() {
            let (result_sender, result_receiver) = oneshot::channel();
            state_manager_sender
                .send(AtomaAtomaStateManagerEvent::GetCheapestNodeForModel {
                    model: model.to_string(),
                    result_sender,
                })
                .map_err(|err| {
                    error!("Failed to send GetTasksForModel event: {:?}", err);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
            let node = result_receiver
                .await
                .map_err(|err| {
                    error!("Failed to receive GetTasksForModel result: {:?}", err);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?
                .map_err(|err| {
                    error!("Failed to get GetTasksForModel result: {:?}", err);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
            let node: atoma_state::types::CheapestNode = match node {
                Some(node) => node,
                None => {
                    error!("No tasks found for model {}", model);
                    return Err(StatusCode::NOT_FOUND);
                }
            };
            let StackEntryResponse {
                transaction_digest: tx_digest,
                stack_created_event: event,
            } = sui
                .write()
                .await
                .acquire_new_stack_entry(
                    node.task_small_id as u64,
                    node.max_num_compute_units as u64,
                    node.price_per_compute_unit as u64,
                )
                .await
                .map_err(|err| {
                    error!("Failed to acquire new stack entry: {:?}", err);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;

            let stack_small_id = event.stack_small_id.inner as i64;
            let selected_node_id = event.selected_node_id.inner as i64;

            // Send the NewStackAcquired event to the state manager, so we have it in the DB.
            state_manager_sender
                .send(AtomaAtomaStateManagerEvent::NewStackAcquired {
                    event,
                    already_computed_units: total_tokens as i64,
                })
                .map_err(|err| {
                    error!("Failed to send NewStackAcquired event: {:?}", err);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;

            Ok(SelectedNodeMetadata {
                stack_small_id,
                selected_node_id,
                tx_digest: Some(tx_digest),
            })
        } else {
            Ok(SelectedNodeMetadata {
                stack_small_id: stacks[0].stack_small_id,
                selected_node_id: stacks[0].selected_node_id,
                tx_digest: None,
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
    /// - `Err(StatusCode)`: Appropriate HTTP error status if processing fails
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
    /// ```no_run
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
    ) -> Result<ProcessedRequest, StatusCode> {
        match endpoint {
            CHAT_COMPLETIONS_PATH | CONFIDENTIAL_CHAT_COMPLETIONS_PATH => {
                let request_model = RequestModelChatCompletions::new(body_json).map_err(|_| {
                    error!("Failed to parse body as chat completions request model");
                    StatusCode::BAD_REQUEST
                })?;
                authenticate_and_process(
                    request_model,
                    &state.0,
                    req_parts.headers.clone(),
                    body_json,
                )
                .await
            }
            EMBEDDINGS_PATH | CONFIDENTIAL_EMBEDDINGS_PATH => {
                let request_model = RequestModelEmbeddings::new(body_json).map_err(|_| {
                    error!("Failed to parse body as embeddings request model");
                    StatusCode::BAD_REQUEST
                })?;
                authenticate_and_process(
                    request_model,
                    &state.0,
                    req_parts.headers.clone(),
                    body_json,
                )
                .await
            }
            IMAGE_GENERATIONS_PATH | CONFIDENTIAL_IMAGE_GENERATIONS_PATH => {
                let request_model = RequestModelImageGenerations::new(body_json).map_err(|_| {
                    error!("Failed to parse body as image generations request model");
                    StatusCode::BAD_REQUEST
                })?;
                authenticate_and_process(
                    request_model,
                    &state.0,
                    req_parts.headers.clone(),
                    body_json,
                )
                .await
            }
            _ => Err(StatusCode::NOT_FOUND),
        }
    }

    /// Handles the setup of confidential computing headers for secure endpoints.
    ///
    /// This function checks if the request is targeting a confidential endpoint and, if so,
    /// retrieves and adds the node's X25519 public key to the request headers. This key
    /// is used for establishing secure end-to-end encrypted communication.
    ///
    /// # Arguments
    ///
    /// * `state` - Server state containing access to the state manager
    /// * `headers` - Mutable reference to request headers where the public key will be added
    /// * `endpoint` - The API endpoint path being accessed
    /// * `node_id` - The ID of the selected inference node
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the headers were successfully processed, or an appropriate
    /// `StatusCode` error if any step fails.
    ///
    /// # Headers Added
    ///
    /// For confidential endpoints, adds:
    /// * `X-Node-X25519-PublicKey`: Base64-encoded X25519 public key of the node
    ///
    /// # Errors
    ///
    /// Returns various status codes for different failure scenarios:
    /// * `INTERNAL_SERVER_ERROR` (500):
    ///   - Failed to communicate with state manager
    ///   - Failed to receive public key response
    /// * `NOT_FOUND` (404):
    ///   - No X25519 public key found for the specified node
    /// * `BAD_REQUEST` (400):
    ///   - Failed to convert public key to header value
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut headers = HeaderMap::new();
    /// handle_confidential_compute_content(
    ///     state,
    ///     &mut headers,
    ///     CONFIDENTIAL_CHAT_COMPLETIONS_PATH,
    ///     node_id
    /// ).await?;
    /// ```
    pub(crate) async fn handle_confidential_compute_content(
        state: State<ProxyState>,
        headers: &mut HeaderMap,
        endpoint: &str,
        node_id: i64,
    ) -> Result<(), StatusCode> {
        if [
            CONFIDENTIAL_CHAT_COMPLETIONS_PATH,
            CONFIDENTIAL_EMBEDDINGS_PATH,
            CONFIDENTIAL_IMAGE_GENERATIONS_PATH,
        ]
        .contains(&endpoint)
        {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            state
                .state_manager_sender
                .send(
                    AtomaAtomaStateManagerEvent::GetSelectedNodeX25519PublicKey {
                        selected_node_id: node_id,
                        result_sender: sender,
                    },
                )
                .map_err(|err| {
                    error!("Failed to get server x25519 public key: {}", err);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
            let x25519_dalek_public_key = receiver
                .await
                .map_err(|err| {
                    error!("Failed to receive server x25519 public key: {}", err);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?
                .map_err(|err| {
                    error!("Failed to get server x25519 public key: {}", err);
                    StatusCode::INTERNAL_SERVER_ERROR
                })?
                .ok_or_else(|| {
                    error!("No x25519 public key found for node {}", node_id);
                    StatusCode::NOT_FOUND
                })?;
            let x25519_dalek_public_key_str = STANDARD.encode(x25519_dalek_public_key);
            headers.insert(
                constants::NODE_X25519_PUBLIC_KEY,
                HeaderValue::from_str(&x25519_dalek_public_key_str).map_err(|e| {
                    error!("Failed to convert x25519 public key to header value: {}", e);
                    StatusCode::BAD_REQUEST
                })?,
            );
        }
        Ok(())
    }
}
