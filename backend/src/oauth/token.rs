//! `POST /oauth/token`: exchanges a code or refresh token for new tokens.

use axum::{
    Form, Json,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use zeroize::Zeroizing;

use crate::{
    app_state::AppState,
    audit,
    crypto::{self, ACCESS_TOKEN_PREFIX, Dek, REFRESH_TOKEN_PREFIX, Token, WrapKind},
    db,
    db::oauth::{CodeRecord, NewToken, RefreshRecord},
    error::ApiError,
};

#[derive(Deserialize)]
pub struct TokenRequest {
    grant_type: Option<String>,
    code: Option<Zeroizing<String>>,
    redirect_uri: Option<String>,
    client_id: Option<String>,
    code_verifier: Option<Zeroizing<String>>,
    refresh_token: Option<Zeroizing<String>>,
    resource: Option<String>,
}

#[derive(Serialize)]
struct TokenResponse {
    access_token: String,
    token_type: &'static str,
    expires_in: u64,
    refresh_token: String,
    scope: String,
}

/// An RFC 6749 error. `invalid_grant` deliberately doesn't say what was wrong.
enum TokenError {
    OAuth(&'static str),
    Internal(ApiError),
}

impl<E: Into<ApiError>> From<E> for TokenError {
    fn from(error: E) -> Self {
        Self::Internal(error.into())
    }
}

impl IntoResponse for TokenError {
    fn into_response(self) -> Response {
        match self {
            Self::OAuth(code) => {
                with_no_store((StatusCode::BAD_REQUEST, Json(json!({ "error": code }))))
            }
            Self::Internal(error) => error.into_response(),
        }
    }
}

const INVALID_GRANT: TokenError = TokenError::OAuth("invalid_grant");
const INVALID_REQUEST: TokenError = TokenError::OAuth("invalid_request");

fn with_no_store(response: impl IntoResponse) -> Response {
    let mut response = response.into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub async fn token(State(state): State<AppState>, Form(request): Form<TokenRequest>) -> Response {
    let result = match request.grant_type.as_deref() {
        Some("authorization_code") => exchange_code(&state, request).await,
        Some("refresh_token") => rotate_refresh_token(&state, request).await,
        Some(_) => Err(TokenError::OAuth("unsupported_grant_type")),
        None => Err(INVALID_REQUEST),
    };
    match result {
        Ok(tokens) => with_no_store(Json(tokens)),
        Err(error) => error.into_response(),
    }
}

async fn exchange_code(
    state: &AppState,
    request: TokenRequest,
) -> Result<TokenResponse, TokenError> {
    let (Some(code_text), Some(redirect_uri), Some(client_id), Some(verifier), Some(resource)) = (
        &request.code,
        &request.redirect_uri,
        &request.client_id,
        &request.code_verifier,
        &request.resource,
    ) else {
        return Err(INVALID_REQUEST);
    };

    let code = Token::parse("", code_text).ok_or(INVALID_GRANT)?;
    let record = db::oauth::find_code(&state.pool, &code.hash())
        .await?
        .ok_or(INVALID_GRANT)?;
    if record.is_used {
        tracing::warn!(grant_id = record.grant_id, "authorization code replayed");
        db::oauth::revoke_tokens_from_code(&state.pool, record.id).await?;
        audit::record(
            &state.pool,
            Some(record.user_id),
            None,
            "code_replayed",
            Some(json!({ "grant_id": record.grant_id })),
        )
        .await;
        return Err(INVALID_GRANT);
    }
    let matches_request = record.client_id == *client_id
        && record.redirect_uri == *redirect_uri
        && record.resource == *resource;
    if record.is_expired
        || record.grant_is_revoked
        || !matches_request
        || !crypto::pkce_matches(verifier, &record.code_challenge)
    {
        return Err(INVALID_GRANT);
    }

    let dek = unwrap_from_code(&code, &record)?;
    let mut transaction = state.pool.begin().await?;
    if !db::oauth::claim_code(&mut transaction, record.id).await? {
        // A concurrent exchange won. The replay handling runs on its next attempt.
        return Err(INVALID_GRANT);
    }
    let tokens = issue_pair(
        state,
        &mut transaction,
        &dek,
        IssueContext {
            grant_id: record.grant_id,
            user_id: record.user_id,
            issued_from_code: Some(record.id),
            rotated_from: None,
            scope: record.scope,
        },
    )
    .await?;
    transaction.commit().await?;
    Ok(tokens)
}

fn unwrap_from_code(code: &Token, record: &CodeRecord) -> Result<Dek, TokenError> {
    let wrapped = record.wrapped_dek.as_deref().ok_or(INVALID_GRANT)?;
    let kek = code.kek(WrapKind::AuthCode)?;
    Ok(crypto::unwrap_dek(
        &kek,
        wrapped,
        WrapKind::AuthCode,
        record.id,
        record.user_id,
    )?)
}

async fn rotate_refresh_token(
    state: &AppState,
    request: TokenRequest,
) -> Result<TokenResponse, TokenError> {
    let (Some(token_text), Some(client_id)) = (&request.refresh_token, &request.client_id) else {
        return Err(INVALID_REQUEST);
    };
    let token = Token::parse(REFRESH_TOKEN_PREFIX, token_text).ok_or(INVALID_GRANT)?;
    let record = db::oauth::find_refresh_token(&state.pool, &token.hash())
        .await?
        .ok_or(INVALID_GRANT)?;

    if record.client_id != *client_id {
        return Err(INVALID_GRANT);
    }
    if record.is_used {
        return reject_reuse(state, &record).await;
    }
    let resource_matches = request
        .resource
        .as_ref()
        .is_none_or(|resource| *resource == record.resource);
    if record.is_expired || record.is_revoked || record.grant_is_revoked || !resource_matches {
        return Err(INVALID_GRANT);
    }

    let kek = token.kek(WrapKind::RefreshToken)?;
    let dek = crypto::unwrap_dek(
        &kek,
        &record.wrapped_dek,
        WrapKind::RefreshToken,
        record.id,
        record.user_id,
    )?;

    let mut transaction = state.pool.begin().await?;
    if !db::oauth::claim_refresh_token(&mut transaction, record.id).await? {
        drop(transaction);
        return reject_reuse(state, &record).await;
    }
    let tokens = issue_pair(
        state,
        &mut transaction,
        &dek,
        IssueContext {
            grant_id: record.grant_id,
            user_id: record.user_id,
            issued_from_code: None,
            rotated_from: Some(record.id),
            scope: record.scope,
        },
    )
    .await?;
    transaction.commit().await?;
    Ok(tokens)
}

async fn reject_reuse(
    state: &AppState,
    record: &RefreshRecord,
) -> Result<TokenResponse, TokenError> {
    tracing::warn!(
        grant_id = record.grant_id,
        "refresh token reused, revoking the grant"
    );
    db::oauth::revoke_grant(&state.pool, record.grant_id).await?;
    audit::record(
        &state.pool,
        Some(record.user_id),
        None,
        "refresh_token_reused",
        Some(json!({ "grant_id": record.grant_id })),
    )
    .await;
    Err(INVALID_GRANT)
}

struct IssueContext {
    grant_id: i64,
    user_id: i64,
    issued_from_code: Option<i64>,
    rotated_from: Option<i64>,
    scope: String,
}

/// Creates an access and refresh token, each with its own wrap of the DEK.
async fn issue_pair(
    state: &AppState,
    connection: &mut sqlx::PgConnection,
    dek: &Dek,
    context: IssueContext,
) -> Result<TokenResponse, TokenError> {
    let settings = &state.config.settings.oauth;
    let (access, refresh) = (Token::generate(), Token::generate());

    for (token, kind_name, kind, ttl_secs) in [
        (
            &access,
            "access",
            WrapKind::AccessToken,
            settings.access_token_ttl_secs,
        ),
        (
            &refresh,
            "refresh",
            WrapKind::RefreshToken,
            settings.refresh_token_ttl_secs,
        ),
    ] {
        let kek = token.kek(kind)?;
        db::oauth::insert_token(
            connection,
            NewToken {
                token_hash: &token.hash(),
                kind: kind_name,
                grant_id: context.grant_id,
                issued_from_code: context.issued_from_code,
                rotated_from: context.rotated_from,
                ttl_secs,
            },
            |row_id| crypto::wrap_dek(&kek, dek, kind, row_id, context.user_id),
        )
        .await?;
    }

    Ok(TokenResponse {
        access_token: access.encode(ACCESS_TOKEN_PREFIX).to_string(),
        token_type: "Bearer",
        expires_in: settings.access_token_ttl_secs,
        refresh_token: refresh.encode(REFRESH_TOKEN_PREFIX).to_string(),
        scope: context.scope,
    })
}
