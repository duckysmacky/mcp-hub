//! Bearer tokens on `/mcp/*`: only from the `Authorization` header, never a query or cookie.

use axum::http::{HeaderMap, HeaderValue, header};

use super::McpError;
use crate::{
    app_state::AppState,
    crypto::{self, ACCESS_TOKEN_PREFIX, Dek, Token, WrapKind},
    db,
    db::instances::is_valid_slug,
    error::ApiError,
    oauth::{SCOPE, resource_url},
};

/// A verified token whose audience is this request's URL.
pub struct Authenticated {
    pub user_id: i64,
    pub instance_id: i64,
    token: Token,
    token_row_id: i64,
    wrapped_dek: Vec<u8>,
}

impl Authenticated {
    /// Unwraps the DEK for the moment it's needed. Drop it straight after.
    pub fn dek(&self) -> Result<Dek, ApiError> {
        let kek = self.token.kek(WrapKind::AccessToken)?;
        Ok(crypto::unwrap_dek(
            &kek,
            &self.wrapped_dek,
            WrapKind::AccessToken,
            self.token_row_id,
            self.user_id,
        )?)
    }
}

pub async fn authenticate(
    state: &AppState,
    headers: &HeaderMap,
    slug: &str,
) -> Result<Authenticated, McpError> {
    let reject = |token_was_sent| McpError::Unauthorized {
        challenge: challenge(state, slug, token_was_sent),
    };
    let Some(text) = bearer_token(headers) else {
        return Err(reject(false));
    };
    let token = Token::parse(ACCESS_TOKEN_PREFIX, text).ok_or_else(|| reject(true))?;
    let record = db::oauth::find_access_token(&state.pool, &token.hash())
        .await?
        .ok_or_else(|| reject(true))?;
    // This looks the same whether or not the slug exists for anyone.
    if record.resource != resource_url(&state.config.public_url, slug) {
        return Err(reject(true));
    }
    db::oauth::touch_grant(&state.pool, record.grant_id).await?;
    Ok(Authenticated {
        user_id: record.user_id,
        instance_id: record.instance_id,
        token,
        token_row_id: record.id,
        wrapped_dek: record.wrapped_dek,
    })
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
}

/// Builds the 401 challenge that sends a client to the OAuth discovery documents.
fn challenge(state: &AppState, slug: &str, token_was_sent: bool) -> Option<HeaderValue> {
    // A malformed slug must not end up inside a header.
    if !is_valid_slug(slug) {
        return HeaderValue::from_str(&format!("Bearer scope=\"{SCOPE}\"")).ok();
    }
    let public_url = state.config.public_url.as_str();
    let error = if token_was_sent {
        ", error=\"invalid_token\""
    } else {
        ""
    };
    HeaderValue::from_str(&format!(
        "Bearer resource_metadata=\"{public_url}/.well-known/oauth-protected-resource/mcp/{slug}\", scope=\"{SCOPE}\"{error}"
    ))
    .ok()
}
