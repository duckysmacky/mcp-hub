//! `/mcp/{slug}`: the MCP Streamable HTTP endpoint (BRIDGE.md). Every request is
//! authenticated first, then resolved through the token's user to one instance.

mod auth;
mod stdio;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::Value;

use crate::{
    app_state::AppState,
    bridge::json_rpc_error,
    catalog::{ServerMode, ServerTemplate},
    crypto, db,
    db::instances::Instance,
    error::ApiError,
    net_guard,
    process::EnvValue,
    supervisor::SupervisorError,
};
use auth::Authenticated;

const SESSION_HEADER: HeaderName = HeaderName::from_static("mcp-session-id");
const INTERNAL_ERROR: i64 = -32603;
const RATE_LIMIT_RETRY_SECS: &str = "60";

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/mcp/{slug}",
        post(handle_post).get(handle_get).delete(handle_delete),
    )
}

/// Transport and auth failures. MCP-level problems are JSON-RPC errors in a 200 instead.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("missing or invalid bearer token")]
    Unauthorized { challenge: Option<HeaderValue> },
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(&'static str),
    #[error("Accept must include text/event-stream")]
    NotAcceptable,
    #[error("too many processes started recently")]
    RateLimited,
    #[error("the hub is at its process limit")]
    Unavailable,
    #[error(transparent)]
    Internal(#[from] ApiError),
}

impl From<sqlx::Error> for McpError {
    fn from(error: sqlx::Error) -> Self {
        Self::Internal(error.into())
    }
}

impl From<crypto::CryptoError> for McpError {
    fn from(error: crypto::CryptoError) -> Self {
        Self::Internal(error.into())
    }
}

impl IntoResponse for McpError {
    fn into_response(self) -> Response {
        let text = self.to_string();
        match self {
            Self::Unauthorized { challenge } => {
                let mut response = StatusCode::UNAUTHORIZED.into_response();
                if let Some(challenge) = challenge {
                    response
                        .headers_mut()
                        .insert(header::WWW_AUTHENTICATE, challenge);
                }
                response
            }
            Self::NotFound => StatusCode::NOT_FOUND.into_response(),
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, text).into_response(),
            Self::NotAcceptable => (StatusCode::NOT_ACCEPTABLE, text).into_response(),
            Self::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, RATE_LIMIT_RETRY_SECS)],
                text,
            )
                .into_response(),
            Self::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, text).into_response(),
            Self::Internal(error) => error.into_response(),
        }
    }
}

/// How a parsed POST body should be treated. `id` + `method` is the only shape that
/// gets a response; notifications and responses to the server are just passed on.
enum Message {
    Initialize,
    Request,
    NoResponseExpected,
}

fn classify(message: &Value) -> Result<Message, McpError> {
    if message.is_array() {
        return Err(McpError::BadRequest("JSON-RPC batches are not supported"));
    }
    if !message.is_object() {
        return Err(McpError::BadRequest("body must be a JSON-RPC message"));
    }
    let id = message.get("id");
    if id.is_some_and(Value::is_null) {
        return Err(McpError::BadRequest("id must not be null"));
    }
    match (id, message.get("method").and_then(Value::as_str)) {
        (Some(_), Some("initialize")) => Ok(Message::Initialize),
        (None, Some("initialize")) => Err(McpError::BadRequest("initialize must have an id")),
        (Some(_), Some(_)) => Ok(Message::Request),
        _ => Ok(Message::NoResponseExpected),
    }
}

fn session_id(headers: &HeaderMap) -> Option<&str> {
    headers.get(SESSION_HEADER)?.to_str().ok()
}

fn required_session_id(headers: &HeaderMap) -> Result<&str, McpError> {
    match headers.get(SESSION_HEADER) {
        None => Err(McpError::BadRequest("Mcp-Session-Id header is required")),
        // A value that can't be a real id can't match any session.
        Some(_) => session_id(headers).ok_or(McpError::NotFound),
    }
}

/// The resolved target of a request. Only reachable through a valid token.
struct Target<'a> {
    state: &'a AppState,
    auth: Authenticated,
    instance: Instance,
    template: &'a ServerTemplate,
}

async fn resolve<'a>(
    state: &'a AppState,
    headers: &HeaderMap,
    slug: &str,
) -> Result<Target<'a>, McpError> {
    let auth = auth::authenticate(state, headers, slug).await?;
    // Bound by id as well as URL, so a recreated slug doesn't revive old tokens.
    let instance = db::instances::get(&state.pool, auth.user_id, auth.instance_id)
        .await?
        .filter(|instance| instance.enabled && instance.slug == slug)
        .ok_or(McpError::NotFound)?;
    let template = state
        .catalog
        .get(&instance.server_id)
        .ok_or(McpError::NotFound)?;
    Ok(Target {
        state,
        auth,
        instance,
        template,
    })
}

impl Target<'_> {
    /// Decrypts every value of this instance that the catalog still allows.
    async fn decrypt_env(&self) -> Result<Vec<EnvValue>, McpError> {
        let dek = self.auth.dek()?;
        let rows = db::instances::env_ciphertexts(&self.state.pool, self.instance.id).await?;
        let mut values = Vec::with_capacity(rows.len());
        for row in rows {
            if self.template.env_vars.contains(&row.env_var) {
                let value =
                    crypto::decrypt_env(&dek, self.instance.id, &row.env_var, &row.ciphertext)?;
                values.push((row.env_var, value));
            }
        }
        Ok(values)
    }

    /// Re-checks URL values at spawn time, because DNS may have changed since they were
    /// saved. Returns the reason for the client; it never includes the value.
    async fn check_urls(&self, values: &[EnvValue]) -> Result<(), String> {
        for (name, value) in values {
            net_guard::check_env_value(value, &self.state.config.settings.network)
                .await
                .map_err(|error| format!("{name} was rejected: {error}"))?;
        }
        Ok(())
    }

    fn script(&self) -> Result<&std::path::Path, McpError> {
        self.state
            .bin_scripts
            .get(&self.template.id)
            .map(std::path::PathBuf::as_path)
            .ok_or(McpError::NotFound)
    }
}

/// A failed start, as MCP sees it: the client shows the message to the user.
fn start_failure(id: &Value, error: SupervisorError) -> Result<Response, McpError> {
    match error {
        SupervisorError::SpawnRateLimited => Err(McpError::RateLimited),
        SupervisorError::TooManyProcesses => Err(McpError::Unavailable),
        SupervisorError::Scratch(_) | SupervisorError::Bridge(_) => {
            tracing::error!(%error, "could not start a child process");
            Ok(json_error(id, "could not start the server"))
        }
    }
}

fn json_error(id: &Value, message: &str) -> Response {
    Json(json_rpc_error(id, INTERNAL_ERROR, message)).into_response()
}

fn no_buffering() -> (HeaderName, HeaderValue) {
    (
        HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    )
}

async fn handle_post(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, McpError> {
    let target = resolve(&state, &headers, &slug).await?;
    let mut message: Value = serde_json::from_slice(&body)
        .map_err(|_| McpError::BadRequest("body must be valid JSON"))?;
    let kind = classify(&message)?;
    if matches!(kind, Message::Initialize) {
        if headers.contains_key(SESSION_HEADER) {
            return Err(McpError::BadRequest(
                "initialize must not carry an Mcp-Session-Id",
            ));
        }
        crate::bridge::strip_client_capabilities(&mut message);
    }
    tracing::info!(
        user_id = target.auth.user_id,
        instance_id = target.instance.id,
        method = message["method"].as_str().unwrap_or("response"),
        "mcp request"
    );
    match target.template.mode {
        ServerMode::Stdio => stdio::post(&target, &headers, kind, message).await,
        ServerMode::Http => Err(McpError::NotFound),
    }
}

async fn handle_get(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Result<Response, McpError> {
    let target = resolve(&state, &headers, &slug).await?;
    match target.template.mode {
        ServerMode::Stdio => stdio::get(&target, &headers),
        ServerMode::Http => Err(McpError::NotFound),
    }
}

async fn handle_delete(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
) -> Result<Response, McpError> {
    let target = resolve(&state, &headers, &slug).await?;
    match target.template.mode {
        ServerMode::Stdio => stdio::delete(&target, &headers),
        ServerMode::Http => Err(McpError::NotFound),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn classifies_message_shapes() {
        assert!(matches!(
            classify(&json!({ "id": 1, "method": "initialize" })),
            Ok(Message::Initialize)
        ));
        assert!(matches!(
            classify(&json!({ "id": 1, "method": "tools/list" })),
            Ok(Message::Request)
        ));
        assert!(matches!(
            classify(&json!({ "method": "notifications/initialized" })),
            Ok(Message::NoResponseExpected)
        ));
        assert!(matches!(
            classify(&json!({ "id": 1, "result": {} })),
            Ok(Message::NoResponseExpected)
        ));
        for invalid in [
            json!([]),
            json!("text"),
            json!(1),
            json!({ "method": "initialize" }),
            json!({ "id": null, "method": "ping" }),
        ] {
            assert!(classify(&invalid).is_err(), "{invalid}");
        }
    }
}
