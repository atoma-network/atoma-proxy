use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Response structure for API errors
///
/// This struct is used to provide a consistent error response format across the API.
/// It wraps [`ErrorDetails`] in an `error` field to maintain a standard JSON structure
/// like `{"error": {...}}`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: ErrorDetails,
}

/// Details of an API error response
///
/// This struct contains the specific details of an error that occurred during
/// API request processing. It is wrapped in [`ErrorResponse`] to maintain a
/// consistent JSON structure.
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorDetails {
    /// A machine-readable error code string (e.g., "MISSING_HEADER", "AUTH_ERROR")
    pub code: String,
    /// A human-readable error message describing what went wrong
    pub message: String,
}

/// Represents all possible errors that can occur within the Atoma service
///
/// This enum implements [`std::error::Error`] and provides structured error types
/// for various failure scenarios in the API. Each variant includes the number of
/// tokens that were processed before the error occurred.
#[derive(Debug, Error)]
pub enum AtomaProxyError {
    /// Error returned when the request body is malformed or contains invalid data
    #[error("Invalid request: {message}")]
    RequestError {
        /// Description of why the request body is invalid
        message: String,
        /// The endpoint that the error occurred on
        endpoint: String,
    },

    /// Error returned when authentication fails
    #[error("Authentication error: {auth_error}")]
    AuthError {
        /// Description of the authentication error
        auth_error: String,
        /// The endpoint that the error occurred on
        endpoint: String,
    },

    /// Error returned for unexpected internal server errors
    #[error("Internal server error: {message}")]
    InternalError {
        /// Description of the internal error
        message: String,
        /// The message to be sent to the client, if None then default message will be used
        client_message: Option<String>,
        /// The endpoint that the error occurred on
        endpoint: String,
    },

    /// Error returned when there is not enough balance to complete a transaction
    #[error("Insufficient balance: {message}")]
    BalanceError {
        /// Description of the balance error
        message: String,
        /// The endpoint that the error occurred on
        endpoint: String,
    },

    /// Error returned when a resource is not found
    #[error("Resource not found: {message}")]
    NotFound {
        /// Description of the resource not found
        message: String,
        /// The endpoint that the error occurred on
        endpoint: String,
    },

    /// Error returned when an endpoint is not implemented
    #[error("Endpoint not implemented: {message}")]
    NotImplemented {
        /// Description of the endpoint not implemented
        message: String,
        /// The endpoint that the error occurred on
        endpoint: String,
    },

    /// Error returned when a service is unavailable
    #[error("Service unavailable: {message}")]
    ServiceUnavailable {
        /// Description of the service unavailable
        message: String,
        /// The endpoint that the error occurred on
        endpoint: String,
    },

    /// Error returned when a service is unavailable
    #[error("Locked: {message}")]
    Locked {
        /// Description of the locked
        message: String,
        /// The endpoint that the error occurred on
        endpoint: String,
    },

    /// Error returned when a specific stack is unavailable
    #[error("Stack unavailable: {message}")]
    UnavailableStack {
        /// Description of the stack unavailable
        message: String,
        /// The endpoint that the error occurred on
        endpoint: String,
    },

    #[error("Too many requests: {message}")]
    TooManyRequests {
        /// Description of the too many requests
        message: String,
        /// The endpoint that the error occurred on
        endpoint: String,
    },

    #[error("Only fiat payments are supported")]
    FiatPaymentsOnly {
        /// Description of the fiat payments only error
        message: String,
        /// The endpoint that the error occurred on
        endpoint: String,
    },
}

impl AtomaProxyError {
    /// Returns the machine-readable error code for this error type
    ///
    /// Each variant of [`AtomaProxyError`] maps to a specific error code string that can be used
    /// by API clients to programmatically handle different error cases. These codes are consistent
    /// across API responses and are included in the [`ErrorDetails`] structure.
    ///
    /// # Returns
    ///
    /// A static string representing the error code, such as:
    /// - `"MISSING_HEADER"` for missing required headers
    /// - `"INVALID_HEADER"` for invalid header values
    /// - `"INVALID_BODY"` for malformed request bodies
    /// - `"MODEL_ERROR"` for ML model errors
    /// - `"AUTH_ERROR"` for authentication failures
    /// - `"INTERNAL_ERROR"` for unexpected server errors
    pub const fn error_code(&self) -> &'static str {
        match self {
            Self::RequestError { .. } => "REQUEST_ERROR",
            Self::AuthError { .. } => "AUTH_ERROR",
            Self::InternalError { .. } => "INTERNAL_ERROR",
            Self::NotFound { .. } => "NOT_FOUND",
            Self::NotImplemented { .. } => "NOT_IMPLEMENTED",
            Self::ServiceUnavailable { .. } => "SERVICE_UNAVAILABLE",
            Self::BalanceError { .. } => "BALANCE_ERROR",
            Self::Locked { .. } => "LOCKED",
            Self::UnavailableStack { .. } => "UNAVAILABLE_STACK",
            Self::TooManyRequests { .. } => "TOO_MANY_REQUESTS",
        }
    }

    /// Returns a user-friendly error message for API responses
    ///
    /// This method generates human-readable error messages that are suitable for
    /// displaying to API clients. The messages are more concise than the full
    /// error details and exclude sensitive internal information.
    ///
    /// # Returns
    ///
    /// A `String` containing a user-friendly error message based on the error type:
    /// - For missing headers: Specifies which header is missing
    /// - For invalid headers: A generic invalid header message
    /// - For invalid body: Includes the specific validation error
    /// - For model errors: Includes the model-specific error message
    /// - For auth errors: A generic authentication failure message
    /// - For internal errors: A generic server error message
    fn client_message(&self) -> String {
        match self {
            Self::RequestError { message, .. } => format!("Request error: {message}"),
            Self::AuthError { .. } => "Authentication failed".to_string(),
            Self::InternalError { client_message, .. } => client_message
                .clone()
                .unwrap_or_else(|| "Internal server error occurred".to_string()),
            Self::NotFound { .. } => "Resource not found".to_string(),
            Self::NotImplemented { .. } => "Endpoint not implemented".to_string(),
            Self::ServiceUnavailable { .. } => "Service unavailable".to_string(),
            Self::BalanceError { .. } => "Insufficient balance".to_string(),
            Self::Locked { .. } => "Locked".to_string(),
            Self::UnavailableStack { .. } => "Stack unavailable".to_string(),
            Self::TooManyRequests { .. } => "Too many requests".to_string(),
        }
    }

    /// Returns the HTTP status code associated with this error
    ///
    /// Maps each error variant to an appropriate HTTP status code:
    /// - `400 Bad Request` for invalid inputs (missing/invalid headers, invalid body, model errors)
    /// - `401 Unauthorized` for authentication failures
    /// - `500 Internal Server Error` for unexpected server errors
    ///
    /// # Returns
    ///
    /// An [`axum::http::StatusCode`] representing the appropriate HTTP response code for this error
    pub const fn status_code(&self) -> StatusCode {
        match self {
            Self::RequestError { .. } => StatusCode::BAD_REQUEST,
            Self::AuthError { .. } => StatusCode::UNAUTHORIZED,
            Self::InternalError { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            Self::NotFound { .. } => StatusCode::NOT_FOUND,
            Self::NotImplemented { .. } => StatusCode::NOT_IMPLEMENTED,
            Self::ServiceUnavailable { .. } => StatusCode::SERVICE_UNAVAILABLE,
            Self::BalanceError { .. } => StatusCode::PAYMENT_REQUIRED,
            Self::Locked { .. } => StatusCode::LOCKED,
            Self::UnavailableStack { .. } => StatusCode::TOO_EARLY,
            Self::TooManyRequests { .. } => StatusCode::TOO_MANY_REQUESTS,
        }
    }

    /// Returns the endpoint where the error occurred
    ///
    /// This method provides access to the endpoint path across all error variants,
    /// which is useful for error tracking, logging, and debugging purposes.
    ///
    /// # Returns
    ///
    /// A `String` containing the API endpoint path where the error was encountered.
    fn endpoint(&self) -> String {
        match self {
            Self::RequestError { endpoint, .. }
            | Self::AuthError { endpoint, .. }
            | Self::InternalError { endpoint, .. }
            | Self::NotFound { endpoint, .. }
            | Self::NotImplemented { endpoint, .. }
            | Self::ServiceUnavailable { endpoint, .. }
            | Self::BalanceError { endpoint, .. }
            | Self::Locked { endpoint, .. }
            | Self::UnavailableStack { endpoint, .. }
            | Self::TooManyRequests { endpoint, .. } => endpoint.clone(),
        }
    }

    /// Returns the full error message with details
    ///
    /// Unlike [`client_message`], this method returns the complete error message including
    /// internal details. This is suitable for internal logging and debugging, but should
    /// not be exposed directly to API clients.
    ///
    /// # Returns
    ///
    /// A `String` containing the detailed error message that includes:
    /// - For missing headers: The name of the missing header
    /// - For invalid headers: The specific validation error
    /// - For invalid body: The detailed validation message
    /// - For model errors: The complete model error message
    /// - For auth errors: The specific authentication failure reason
    /// - For internal errors: The detailed internal error message
    fn message(&self) -> String {
        match self {
            Self::RequestError { message, .. } => format!("Request error: {message}"),
            Self::AuthError { auth_error, .. } => format!("Authentication error: {auth_error}"),
            Self::InternalError { message, .. } => format!("Internal server error: {message}"),
            Self::NotFound { .. } => "Resource not found".to_string(),
            Self::NotImplemented { .. } => "Endpoint not implemented".to_string(),
            Self::ServiceUnavailable { message, .. } => format!("Service unavailable: {message}"),
            Self::BalanceError { message, .. } => format!("Insufficient balance: {message}"),
            Self::Locked { message, .. } => format!("Locked: {message}"),
            Self::UnavailableStack { message, .. } => format!("Stack unavailable: {message}"),
            Self::TooManyRequests { message, .. } => format!("Too many requests: {message}"),
        }
    }
}

impl IntoResponse for AtomaProxyError {
    fn into_response(self) -> Response {
        tracing::error!(
            target = "atoma-service",
            event = "error_occurred",
            endpoint = self.endpoint(),
            error = %self.message(),
        );
        let error_response = ErrorResponse {
            error: ErrorDetails {
                code: self.error_code().to_string(),
                message: self.client_message(),
            },
        };
        (self.status_code(), Json(error_response)).into_response()
    }
}
