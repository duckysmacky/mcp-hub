//! Dynamic Client Registration (RFC 7591). Every client is public: it proves itself
//! with PKCE, so any requested auth method is answered with `none`.

use axum::{Json, extract::State, http::StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

use crate::{app_state::AppState, config::host_is_loopback, crypto::Token, db};

const MAX_REDIRECT_URIS: usize = 10;
const MAX_REDIRECT_URI_BYTES: usize = 2048;
const MAX_CLIENT_NAME_CHARS: usize = 200;

/// Fields beyond these are accepted and ignored, as RFC 7591 asks.
#[derive(Deserialize)]
pub struct RegistrationRequest {
    client_name: Option<String>,
    #[serde(default)]
    redirect_uris: Vec<String>,
}

fn invalid_metadata(code: &str, description: &str) -> (StatusCode, Json<Value>) {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": code, "error_description": description })),
    )
}

/// A redirect target must be `https`, or `http` on a loopback host.
fn is_allowed_redirect_uri(uri: &str) -> bool {
    if uri.len() > MAX_REDIRECT_URI_BYTES {
        return false;
    }
    let Ok(url) = Url::parse(uri) else {
        return false;
    };
    let scheme_is_allowed =
        url.scheme() == "https" || (url.scheme() == "http" && host_is_loopback(&url));
    scheme_is_allowed && url.fragment().is_none() && url.has_host()
}

pub async fn register(
    State(state): State<AppState>,
    Json(request): Json<RegistrationRequest>,
) -> Result<(StatusCode, Json<Value>), (StatusCode, Json<Value>)> {
    if request.redirect_uris.is_empty() || request.redirect_uris.len() > MAX_REDIRECT_URIS {
        return Err(invalid_metadata(
            "invalid_redirect_uri",
            "between 1 and 10 redirect_uris are required",
        ));
    }
    if !request
        .redirect_uris
        .iter()
        .all(|uri| is_allowed_redirect_uri(uri))
    {
        return Err(invalid_metadata(
            "invalid_redirect_uri",
            "redirect_uris must be https, or http on a loopback host",
        ));
    }
    let client_name = request
        .client_name
        .filter(|name| !name.is_empty())
        .map(|name| name.chars().take(MAX_CLIENT_NAME_CHARS).collect::<String>());

    let client_id = Token::generate().encode("");
    db::oauth::create_client(
        &state.pool,
        &client_id,
        client_name.as_deref(),
        &request.redirect_uris,
    )
    .await
    .map_err(|error| {
        tracing::error!(%error, "client registration failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "server_error" })),
        )
    })?;

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "client_id": client_id.as_str(),
            "client_name": client_name,
            "redirect_uris": request.redirect_uris,
            "token_endpoint_auth_method": "none",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
        })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_uris_are_limited_to_https_and_loopback() {
        for allowed in [
            "https://claude.ai/api/mcp/auth_callback",
            "http://localhost:3000/callback",
            "http://127.0.0.1:33333/callback",
            "http://[::1]:33333/callback",
        ] {
            assert!(is_allowed_redirect_uri(allowed), "{allowed}");
        }
        for rejected in [
            "http://example.com/callback",
            "http://localhost.evil.example/callback",
            "javascript:alert(1)",
            "cursor://callback",
            "data:text/html,x",
            "https://example.com/callback#fragment",
            "not a url",
            "",
        ] {
            assert!(!is_allowed_redirect_uri(rejected), "{rejected}");
        }
    }
}
