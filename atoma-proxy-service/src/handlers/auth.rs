use atoma_auth::AuthError;
use atoma_state::types::{
    AuthResponse, CreateTokenRequest, LoginAuthRequest, ProofRequest, RegisterAuthRequest,
    RevokeApiTokenRequest, TokenResponse, UsdcPaymentRequest, UserProfile,
};
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};

use base64::{prelude::BASE64_STANDARD, Engine};
use tracing::{error, instrument};
use utoipa::OpenApi;

use crate::ProxyServiceState;
use rand::{Rng, SeedableRng};

/// The path for the register endpoint.
pub const REGISTER_PATH: &str = "/register";

/// The path for the login endpoint.
pub const LOGIN_PATH: &str = "/login";

/// The path for the generate_api_token endpoint.
pub const GENERATE_API_TOKEN_PATH: &str = "/generate_api_token";

/// The path for the revoke_api_token endpoint.
pub const REVOKE_API_TOKEN_PATH: &str = "/revoke_api_token";

/// The path for the api_tokens endpoint.
pub const GET_ALL_API_TOKENS_PATH: &str = "/api_tokens";

/// The path for the update_sui_address endpoint.
pub const UPDATE_SUI_ADDRESS_PATH: &str = "/update_sui_address";

/// The path for the usdc payment endpoint.
pub const USDC_PAYMENT_PATH: &str = "/usdc_payment";

/// The path for the get_sui_address endpoint.
pub const GET_SUI_ADDRESS_PATH: &str = "/get_sui_address";

/// The path for the balance endpoint.
pub const GET_BALANCE_PATH: &str = "/balance";

/// Get user profile endpoint
pub const GET_USER_PROFILE_PATH: &str = "/user_profile";

/// Set user's salt endpoint.
pub const GET_ZK_SALT_PATH: &str = "/zk_salt";

#[cfg(feature = "google-oauth")]
/// The path for the google_oauth endpoint.
pub const GOOGLE_OAUTH_PATH: &str = "/google_oauth";

type Result<T> = std::result::Result<T, StatusCode>;

/// OpenAPI documentation for the get_all_api_tokens endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_all_api_tokens
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_all_api_tokens))]
pub struct GetAllApiTokensOpenApi;

/// Returns a router with the auth endpoints.
///
/// # Returns
/// * `Router<ProxyServiceState>` - A router with the auth endpoints
pub fn auth_router() -> Router<ProxyServiceState> {
    let router = Router::new()
        .route(GET_ALL_API_TOKENS_PATH, get(get_all_api_tokens))
        .route(GENERATE_API_TOKEN_PATH, post(generate_api_token))
        .route(REVOKE_API_TOKEN_PATH, post(revoke_api_token))
        .route(REGISTER_PATH, post(register))
        .route(LOGIN_PATH, post(login))
        .route(UPDATE_SUI_ADDRESS_PATH, post(update_sui_address))
        .route(USDC_PAYMENT_PATH, post(usdc_payment))
        .route(GET_SUI_ADDRESS_PATH, get(get_sui_address))
        .route(GET_BALANCE_PATH, get(get_balance))
        .route(GET_USER_PROFILE_PATH, get(get_user_profile))
        .route(GET_ZK_SALT_PATH, get(get_zk_salt));
    #[cfg(feature = "google-oauth")]
    let router = router.route(GOOGLE_OAUTH_PATH, post(google_oauth));
    router
}

fn get_jwt_from_headers(headers: &HeaderMap) -> Result<&str> {
    headers
        .get(header::AUTHORIZATION)
        .ok_or(StatusCode::UNAUTHORIZED)?
        .to_str()
        .map_err(|_| StatusCode::UNAUTHORIZED)?
        .strip_prefix("Bearer ")
        .ok_or(StatusCode::UNAUTHORIZED)
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
pub async fn get_all_api_tokens(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
) -> Result<Json<Vec<TokenResponse>>> {
    let jwt = get_jwt_from_headers(&headers)?;
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
pub struct GenerateApiTokenOpenApi;

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
    post,
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
pub async fn generate_api_token(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
    body: Json<CreateTokenRequest>,
) -> Result<Json<String>> {
    let jwt = get_jwt_from_headers(&headers)?;

    Ok(Json(
        proxy_service_state
            .auth
            .generate_api_token(jwt, body.name.clone())
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
pub struct RevokeApiTokenOpenApi;

/// Revokes an API token for the user.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `headers` - The headers of the request
/// * `body` - The request body containing the API token id to revoke
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
pub async fn revoke_api_token(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
    body: Json<RevokeApiTokenRequest>,
) -> Result<Json<()>> {
    let jwt = get_jwt_from_headers(&headers)?;

    proxy_service_state
        .auth
        .revoke_api_token(jwt, body.api_token_id)
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
pub struct RegisterOpenApi;

/// Registers a new user with the proxy service.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `body` - The request body containing the email and password of the new user
///
/// # Returns
///
/// * `Result<Json<AuthResponse>>` - A JSON response containing the access and refresh tokens
#[utoipa::path(
    post,
    path = "",
    responses(
        (status = OK, description = "Registers a new user", body = RegisterAuthRequest),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to register user")
    )
)]
#[instrument(level = "trace", skip_all)]
pub async fn register(
    State(proxy_service_state): State<ProxyServiceState>,
    body: Json<RegisterAuthRequest>,
) -> Result<Json<AuthResponse>> {
    let (refresh_token, access_token) = proxy_service_state
        .auth
        .register(&body.user_profile, &body.password)
        .await
        .map_err(|e| {
            error!("Failed to register user: {:?}", e);
            match e {
                AuthError::UserAlreadyRegistered => StatusCode::CONFLICT,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            }
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
pub struct LoginOpenApi;

/// Logs in a user with the proxy service.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `body` - The request body containing the email and password of the user
///
/// # Returns
///
/// * `Result<Json<AuthResponse>>` - A JSON response containing the access and refresh tokens
#[utoipa::path(
    post,
    path = "",
    responses(
        (status = OK, description = "Logs in a user", body = RegisterAuthRequest),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to login user")
    )
)]
#[instrument(level = "trace", skip_all)]
pub async fn login(
    State(proxy_service_state): State<ProxyServiceState>,
    body: Json<LoginAuthRequest>,
) -> Result<Json<AuthResponse>> {
    let (refresh_token, access_token) = proxy_service_state
        .auth
        .check_user_password(&body.email, &body.password)
        .await
        .map_err(|e| {
            error!("Failed to login user: {:?}", e);
            match e {
                AuthError::PasswordNotValidOrUserNotFound => StatusCode::UNAUTHORIZED,
                _ => StatusCode::INTERNAL_SERVER_ERROR,
            }
        })?;
    Ok(Json(AuthResponse {
        access_token,
        refresh_token,
    }))
}

/// OpenAPI documentation for the google_oauth endpoint.
///
/// This struct is used to generate OpenAPI documentation for the google_oauth
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[cfg(feature = "google-oauth")]
#[derive(OpenApi)]
#[openapi(paths(google_oauth))]
pub struct GoogleOAuth;

/// Logs in a user with the proxy service using Google OAuth.
/// This endpoint is used to verify a Google ID token and return an access token.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `body` - The request body containing the Google ID token
///
/// # Returns
///
/// * `Result<Json<AuthResponse>>` - A JSON response containing the access and refresh tokens
#[cfg(feature = "google-oauth")]
#[utoipa::path(
    post,
    path = "",
    responses(
        (status = OK, description = "Logs in a user with Google OAuth", body = String),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to verify Google ID token")
    )
)]
#[instrument(level = "trace", skip_all)]
pub async fn google_oauth(
    State(proxy_service_state): State<ProxyServiceState>,
    body: Json<String>,
) -> Result<Json<AuthResponse>> {
    let id_token = body.0;
    let (refresh_token, access_token) = proxy_service_state
        .auth
        .check_google_id_token(&id_token)
        .await
        .map_err(|e| {
            error!("Failed to verify Google ID token: {:?}", e);
            StatusCode::UNAUTHORIZED
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
pub struct UpdateSuiAddress;

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
pub async fn update_sui_address(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
    body: Json<ProofRequest>,
) -> Result<Json<()>> {
    let jwt = get_jwt_from_headers(&headers)?;

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

/// OpenAPI documentation for the usdc_payment endpoint.
///
/// This struct is used to generate OpenAPI documentation for the usdc_payment
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(usdc_payment))]
pub struct UsdcPayment;

/// OpenAPI documentation for the usdc_payment endpoint.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `headers` - The headers of the request
/// * `body` - The request body containing the transaction digest
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
        (status = OK, description = "USDC payment request"),
        (status = UNAUTHORIZED, description = "Unauthorized request"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to usdc payment request")
    )
)]
#[instrument(level = "info", skip_all)]
pub async fn usdc_payment(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
    body: Json<UsdcPaymentRequest>,
) -> Result<Json<()>> {
    let jwt = get_jwt_from_headers(&headers)?;

    proxy_service_state
        .auth
        .usdc_payment(jwt, &body.transaction_digest, body.proof_signature.clone())
        .await
        .map_err(|e| {
            error!("Failed to usdc payment request: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(()))
}

/// OpenAPI documentation for the get_sui_address endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_sui_address
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_sui_address))]
pub struct GetSuiAddress;

/// Retrieves the sui address for the user.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `headers` - The headers of the request
///
/// # Returns
///
/// * `Result<Json<Option<String>>>` - A JSON response containing the sui address
#[utoipa::path(
    get,
    path = "",
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "Retrieves the sui address for the user"),
        (status = UNAUTHORIZED, description = "Unauthorized request"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get sui address")
    )
)]
#[instrument(level = "info", skip_all)]
pub async fn get_sui_address(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
) -> Result<Json<Option<String>>> {
    let jwt = get_jwt_from_headers(&headers)?;

    let sui_address = proxy_service_state
        .auth
        .get_sui_address(jwt)
        .await
        .map_err(|e| {
            error!("Failed to get sui address: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(Json(sui_address))
}

/// OpenAPI documentation for the get_balance endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_balance
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_balance))]
pub struct GetBalance;

/// Retrieves the balance for the user.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `headers` - The headers of the request
///
/// # Returns
///
/// * `Result<Json<i64>>` - A JSON response containing the balance
#[utoipa::path(
    get,
    path = "",
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "Retrieves the balance for the user"),
        (status = UNAUTHORIZED, description = "Unauthorized request"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get balance")
    )
)]
#[instrument(level = "info", skip_all)]
pub async fn get_balance(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
) -> Result<Json<i64>> {
    let jwt = get_jwt_from_headers(&headers)?;

    let user_id = proxy_service_state
        .auth
        .get_user_id_from_token(jwt)
        .await
        .map_err(|e| {
            error!("Failed to get user ID from token: {:?}", e);
            StatusCode::UNAUTHORIZED
        })?;
    Ok(Json(
        proxy_service_state
            .atoma_state
            .get_crypto_balance_for_user(user_id)
            .await
            .map_err(|e| {
                error!("Failed to get balance: {:?}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}

/// OpenAPI documentation for the get_user_profile endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_user_profile
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_user_profile))]
pub struct GetUserProfile;

/// Retrieves the user profile for the user.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `headers` - The headers of the request
///
/// # Returns
///
/// * `Result<Json<Value>>` - A JSON response containing the user profile
///
/// # Errors
///
/// * If the user ID cannot be retrieved from the token, returns a 401 Unauthorized error
/// * If the user profile cannot be retrieved, returns a 500 Internal Server Error
#[utoipa::path(
    get,
    path = "",
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "Retrieves the user profile for the user"),
        (status = UNAUTHORIZED, description = "Unauthorized request"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to get user profile")
    )
)]
#[instrument(level = "info", skip_all)]
pub async fn get_user_profile(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
) -> Result<Json<UserProfile>> {
    let jwt = get_jwt_from_headers(&headers)?;

    let user_id = proxy_service_state
        .auth
        .get_user_id_from_token(jwt)
        .await
        .map_err(|e| {
            error!("Failed to get user ID from token: {:?}", e);
            StatusCode::UNAUTHORIZED
        })?;

    Ok(Json(
        proxy_service_state
            .atoma_state
            .get_user_profile(user_id)
            .await
            .map_err(|e| {
                error!("Failed to get user profile: {:?}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?,
    ))
}

/// OpenAPI documentation for the get_zk_salt endpoint.
///
/// This struct is used to generate OpenAPI documentation for the get_zk_salt
/// endpoint. It uses the `utoipa` crate's derive macro to automatically generate
/// the OpenAPI specification from the code.
#[derive(OpenApi)]
#[openapi(paths(get_zk_salt))]
pub struct GetZkSalt;

/// Gets the zk_salt for the user. It creates a new zk_salt if the user does not have one.
///
/// # Arguments
///
/// * `proxy_service_state` - The shared state containing the state manager
/// * `headers` - The headers of the request
///
/// # Returns
///
/// * `Result<Json<()>>` - A JSON response indicating the success of the operation
#[utoipa::path(
    get,
    path = "",
    security(
        ("bearerAuth" = [])
    ),
    responses(
        (status = OK, description = "Sets the zk_salt for the user"),
        (status = UNAUTHORIZED, description = "Unauthorized request"),
        (status = INTERNAL_SERVER_ERROR, description = "Failed to set zk_salt")
    )
)]
#[instrument(level = "info", skip_all)]
pub async fn get_zk_salt(
    State(proxy_service_state): State<ProxyServiceState>,
    headers: HeaderMap,
) -> Result<Json<String>> {
    let jwt = get_jwt_from_headers(&headers)?;

    let user_id = proxy_service_state
        .auth
        .get_user_id_from_token(jwt)
        .await
        .map_err(|e| {
            error!("Failed to get user ID from token: {:?}", e);
            StatusCode::UNAUTHORIZED
        })?;

    let zk_salt = proxy_service_state
        .atoma_state
        .get_zk_salt(user_id)
        .await
        .map_err(|e| {
            error!("Failed to get user profile: {:?}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let zk_salt = if let Some(zk_salt) = zk_salt {
        zk_salt
    } else {
        let mut rng = rand::rngs::StdRng::from_entropy();
        let zk_salt: [u8; 16] = rng.gen();
        let zk_salt = BASE64_STANDARD.encode(zk_salt);
        proxy_service_state
            .atoma_state
            .set_zk_salt(user_id, &zk_salt)
            .await
            .map_err(|e| {
                error!("Failed to set zk_salt: {:?}", e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        zk_salt
    };
    Ok(Json(zk_salt))
}
