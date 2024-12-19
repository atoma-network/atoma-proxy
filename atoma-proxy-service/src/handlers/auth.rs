use atoma_state::types::{AuthRequest, AuthResponse, ProofRequest, RevokeApiTokenRequest};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};

use tracing::{error, instrument};
use utoipa::OpenApi;

use crate::ProxyServiceState;

/// The path for the register endpoint.
pub(crate) const REGISTER_PATH: &str = "/register";

/// The path for the login endpoint.
pub(crate) const LOGIN_PATH: &str = "/login";

/// The path for the generate_api_token endpoint.
pub(crate) const GENERATE_API_TOKEN_PATH: &str = "/generate_api_token";

/// The path for the revoke_api_token endpoint.
pub(crate) const REVOKE_API_TOKEN_PATH: &str = "/revoke_api_token";

/// The path for the api_tokens endpoint.
pub(crate) const GET_ALL_API_TOKENS_PATH: &str = "/api_tokens";

/// The path for the update_sui_address endpoint.
pub(crate) const UPDATE_SUI_ADDRESS_PATH: &str = "/update_sui_address";

type Result<T> = std::result::Result<T, StatusCode>;

/// OpenAPI documentation for the get_all_api_tokens endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_all_api_tokens
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_all_api_tokens))]
pub(crate) struct GetAllApiTokensOpenApi;

/// Returns a router with the auth endpoints.
///
/// # Returns
/// * `Router<ProxyServiceState>` - A router with the auth endpoints
pub(crate) fn auth_router() -> Router<ProxyServiceState> {
    Router::new()
        .route(GET_ALL_API_TOKENS_PATH, get(get_all_api_tokens))
        .route(GENERATE_API_TOKEN_PATH, get(generate_api_token))
        .route(REVOKE_API_TOKEN_PATH, post(revoke_api_token))
        .route(REGISTER_PATH, post(register))
        .route(LOGIN_PATH, post(login))
        .route(UPDATE_SUI_ADDRESS_PATH, post(update_sui_address))
}

/// Retrieves all API tokens for the user.
///
/// # Arguments
/// * `proxy_service_state` - The shared state containing the state manager
/// * `headers` - The headers of the request
///
/// # Returns
///
/// * `Result<Json<Vec<String>>>` - A JSON response containing a list of API tokens
#[utoipa::path(
    get,
    path = "",
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "Retrieves all API tokens for the user"),
        (status = UNAUTHORIZED, description = "Unauthorized request"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get all api tokens")
    )
)]
#[instrument(level = "info", skip_all)]
pub(crate) async fn get_all_api_tokens(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
) -> Result<Json<Vec<String>>> {
    let auth_header = headers
        .get("Authorization")
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_str()
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let jwt = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(Json(
        proxy_service_state
            .auth
            .get_all_api_tokens(jwt)
            .await
            .map_err(|e| {
                error!("Failed to get all api tokens: {:?}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}

/// OpenAPI documentation for the generate_api_token endpoint.
///
/// This struct is used to generate OpenAPI documentation for the generate_api_token
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(generate_api_token))]
pub(crate) struct GenerateApiTokenOpenApi;

/// Generates an API token for the user.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `headers` - The headers of the request
///
/// # Returns
///
/// * `Result<Json<String>>` - A JSON response containing the generated API token
#[utoipa::path(
    get,
    path = "",
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "Generates an API token for the user"),
        (status = UNAUTHORIZED, description = "Unauthorized request"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to generate api token")
    )
)]
#[instrument(level = "info", skip_all)]
pub(crate) async fn generate_api_token(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
) -> Result<Json<String>> {
    let auth_header = headers
        .get("Authorization")
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_str()
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let jwt = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(Json(
        proxy_service_state
            .auth
            .generate_api_token(jwt)
            .await
            .map_err(|e| {
                error!("Failed to generate api token: {:?}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}

/// OpenAPI documentation for the revoke_api_token endpoint.
///
/// This struct is used to generate OpenAPI documentation for the revoke_api_token
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(revoke_api_token))]
pub(crate) struct RevokeApiTokenOpenApi;

/// Revokes an API token for the user.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `headers` - The headers of the request
/// * `body` - The request body containing the API token to revoke
///
/// # Returns
///
/// * `Result<Json<()>>` - A JSON response indicating the success of the operation
#[utoipa::path(
    post,
    path = "",
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "Revokes an API token for the user", body = RevokeApiTokenRequest),
        (status = UNAUTHORIZED, description = "Unauthorized request"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to revoke api token")
    )
)]
#[instrument(level = "info", skip_all)]
pub(crate) async fn revoke_api_token(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
    body: Json<RevokeApiTokenRequest>,
) -> Result<Json<()>> {
    let auth_header = headers
        .get("Authorization")
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_str()
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let jwt = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;
    proxy_service_state
        .auth
        .revoke_api_token(jwt, &body.api_token)
        .await
        .map_err(|e| {
            error!("Failed to revoke api token: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(()))
}

/// OpenAPI documentation for the register endpoint.
///
/// This struct is used to generate OpenAPI documentation for the register
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(register))]
pub(crate) struct RegisterOpenApi;

/// Registers a new user with the proxy service.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `body` - The request body containing the username and password of the new user
///
/// # Returns
///
/// * `Result<Json<AuthResponse>>` - A JSON response containing the access and refresh tokens
#[utoipa::path(
    post,
    path = "",
    responses(
        (status = OK, description = "Registers a new user", body = AuthRequest),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to register user")
    )
)]
#[instrument(level = "trace", skip_all)]
pub(crate) async fn register(
    State(proxy_service_state): State<ProxyServiceState>,
    body: Json<AuthRequest>,
) -> Result<Json<AuthResponse>> {
    let (refresh_token, access_token) = proxy_service_state
        .auth
        .register(&body.username, &body.password)
        .await
        .map_err(|e| {
            error!("Failed to register user: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(AuthResponse {
        access_token,
        refresh_token,
    }))
}

/// OpenAPI documentation for the login endpoint.
///
/// This struct is used to generate OpenAPI documentation for the login
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(login))]
pub(crate) struct LoginOpenApi;

/// Logs in a user with the proxy service.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `body` - The request body containing the username and password of the user
///
/// # Returns
///
/// * `Result<Json<AuthResponse>>` - A JSON response containing the access and refresh tokens
#[utoipa::path(
    post,
    path = "",
    responses(
        (status = OK, description = "Logs in a user", body = AuthRequest),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to login user")
    )
)]
#[instrument(level = "trace", skip_all)]
pub(crate) async fn login(
    State(proxy_service_state): State<ProxyServiceState>,
    body: Json<AuthRequest>,
) -> Result<Json<AuthResponse>> {
    let (refresh_token, access_token) = proxy_service_state
        .auth
        .check_user_password(&body.username, &body.password)
        .await
        .map_err(|e| {
            error!("Failed to register user: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(AuthResponse {
        access_token,
        refresh_token,
    }))
}

/// OpenAPI documentation for the update_sui_address endpoint.
///
/// This struct is used to generate OpenAPI documentation for the update_sui_address
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(update_sui_address))]
pub(crate) struct UpdateSuiAddress;

/// Updates the sui address for the user.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `headers` - The headers of the request
/// * `body` - The request body containing the signature of the proof of address
///
/// # Returns
///
/// * `Result<Json<()>>` - A JSON response indicating the success of the operation
#[utoipa::path(
    post,
    path = "",
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "Proof of address request"),
        (status = UNAUTHORIZED, description = "Unauthorized request"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to proof of address request")
    )
)]
#[instrument(level = "info", skip_all)]
pub(crate) async fn update_sui_address(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
    body: Json<ProofRequest>,
) -> Result<Json<()>> {
    let auth_header = headers
        .get("Authorization")
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_str()
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    let jwt = auth_header
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)?;
    proxy_service_state
        .auth
        .update_sui_address(jwt, &body.signature)
        .await
        .map_err(|e| {
            error!("Failed to update sui address request: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(()))
}
