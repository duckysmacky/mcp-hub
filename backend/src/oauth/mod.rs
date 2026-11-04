//! The built-in OAuth 2.1 authorization server (OAUTH.md).

pub mod authorize;
pub mod consent;
pub mod register;
pub mod token;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};

use crate::{app_state::AppState, config::PublicUrl, db::instances::is_valid_slug};

pub const SCOPE: &str = "mcp";

/// The routes outside `/api`. These use no cookies, so they need no CSRF checks.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            get(authorization_server_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource/mcp/{slug}",
            get(protected_resource_metadata),
        )
        .route("/oauth/register", post(register::register))
        .route("/oauth/authorize", get(authorize::authorize))
        .route("/oauth/token", post(token::token))
}

/// The canonical resource URL of a slug.
pub fn resource_url(public_url: &PublicUrl, slug: &str) -> String {
    format!("{}/mcp/{slug}", public_url.as_str())
}

/// Returns the slug of a canonical resource URL on this hub, if it is one.
pub fn slug_from_resource<'a>(public_url: &PublicUrl, resource: &'a str) -> Option<&'a str> {
    let slug = resource
        .strip_prefix(public_url.as_str())?
        .strip_prefix("/mcp/")?;
    is_valid_slug(slug).then_some(slug)
}

async fn authorization_server_metadata(State(state): State<AppState>) -> Json<Value> {
    let issuer = state.config.public_url.as_str();
    Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/oauth/authorize"),
        "token_endpoint": format!("{issuer}/oauth/token"),
        "registration_endpoint": format!("{issuer}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": [SCOPE],
        "client_id_metadata_document_supported": false,
    }))
}

/// Answers for any well-formed slug, so nobody can probe which slugs exist.
async fn protected_resource_metadata(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> Response {
    if !is_valid_slug(&slug) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let public_url = &state.config.public_url;
    Json(json!({
        "resource": resource_url(public_url, &slug),
        "authorization_servers": [public_url.as_str()],
        "scopes_supported": [SCOPE],
        "bearer_methods_supported": ["header"],
        "resource_name": "MCP Hub",
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_urls_must_be_canonical() {
        let public_url: PublicUrl = "https://mcp.example.com".parse().unwrap();
        assert_eq!(
            slug_from_resource(&public_url, "https://mcp.example.com/mcp/taiga"),
            Some("taiga")
        );
        for resource in [
            "",
            "https://mcp.example.com/mcp/",
            "https://mcp.example.com/mcp/taiga/",
            "https://mcp.example.com/mcp/taiga?x=1",
            "https://mcp.example.com/mcp/Taiga",
            "https://mcp.example.com/other/taiga",
            "https://evil.example.com/mcp/taiga",
            "https://mcp.example.com.evil.example/mcp/taiga",
            "http://mcp.example.com/mcp/taiga",
        ] {
            assert_eq!(
                slug_from_resource(&public_url, resource),
                None,
                "{resource}"
            );
        }
    }
}
