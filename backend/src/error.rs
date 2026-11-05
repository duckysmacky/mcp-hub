use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::crypto::CryptoError;

/// Errors from the JSON API. Messages are written for the client and never contain
/// secrets. Internal details are logged, and the client only sees a generic message.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("not logged in")]
    Unauthorized,
    #[error("invalid username or password")]
    InvalidCredentials,
    #[error("{0}")]
    Forbidden(&'static str),
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("too many attempts; try again later")]
    RateLimited { retry_after_secs: u64 },
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("crypto error: {0}")]
    Crypto(#[from] CryptoError),
    #[error("background task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized | Self::InvalidCredentials => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::RateLimited { retry_after_secs } => {
                let headers = [(header::RETRY_AFTER, retry_after_secs.to_string())];
                let body = Json(json!({ "error": self.to_string() }));
                return (StatusCode::TOO_MANY_REQUESTS, headers, body).into_response();
            }
            Self::Database(_) | Self::Crypto(_) | Self::Task(_) => {
                tracing::error!(error = %self, "request failed");
                let body = Json(json!({ "error": "internal error" }));
                return (StatusCode::INTERNAL_SERVER_ERROR, body).into_response();
            }
        };
        (status, Json(json!({ "error": self.to_string() }))).into_response()
    }
}

/// Maps a unique-constraint failure to a conflict, leaving other errors alone.
pub fn conflict_on_duplicate(error: sqlx::Error, message: &'static str) -> ApiError {
    match error.as_database_error() {
        Some(database_error) if database_error.is_unique_violation() => {
            ApiError::Conflict(message.to_owned())
        }
        _ => error.into(),
    }
}
