//! `POST /oauth/revoke` (RFC 7009). Answers 200 whether or not the token existed, so it
//! tells nobody anything about a token they don't already hold.

use axum::{Form, Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::Deserialize;
use serde_json::json;
use zeroize::Zeroizing;

use crate::{
    app_state::AppState,
    audit,
    crypto::{ACCESS_TOKEN_PREFIX, REFRESH_TOKEN_PREFIX, Token},
    db,
    error::ApiError,
};

#[derive(Deserialize)]
pub struct RevokeRequest {
    token: Option<Zeroizing<String>>,
    client_id: Option<String>,
}

pub async fn revoke(
    State(state): State<AppState>,
    Form(request): Form<RevokeRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let Some(text) = &request.token else {
        let body = Json(json!({ "error": "invalid_request" }));
        return Ok((StatusCode::BAD_REQUEST, body).into_response());
    };
    let token = Token::parse(ACCESS_TOKEN_PREFIX, text)
        .or_else(|| Token::parse(REFRESH_TOKEN_PREFIX, text));
    let Some(token) = token else {
        return Ok(StatusCode::OK.into_response());
    };
    let Some(record) = db::oauth::find_token(&state.pool, &token.hash()).await? else {
        return Ok(StatusCode::OK.into_response());
    };
    // A token is only revoked by the client it was issued to.
    if request.client_id.as_deref() != Some(record.client_id.as_str()) {
        return Ok(StatusCode::OK.into_response());
    }

    if record.kind == "refresh" {
        // The access tokens issued alongside it go too: the whole grant ends.
        db::oauth::revoke_grant(&state.pool, record.grant_id).await?;
    } else {
        db::oauth::revoke_token(&state.pool, record.id).await?;
    }
    audit::record(
        &state.pool,
        Some(record.user_id),
        Some(record.instance_id),
        "token_revoked",
        Some(json!({ "grant_id": record.grant_id, "kind": record.kind })),
    )
    .await;
    Ok(StatusCode::OK.into_response())
}
