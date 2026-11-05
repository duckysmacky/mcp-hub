//! The consent decision, under `/api/oauth/pending` with the logged-in user's cookie.

use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{get, post},
};
use serde::Serialize;
use url::Url;

use super::{
    authorize::{error_redirect, redirect_with},
    slug_from_resource,
};
use crate::{
    app_state::AppState,
    auth::CurrentSession,
    config::host_is_loopback,
    crypto::{self, Token, WrapKind},
    db,
    db::{
        instances::Instance,
        oauth::{NewCode, Pending},
    },
    error::ApiError,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/oauth/pending/{id}", get(pending_details))
        .route("/oauth/pending/{id}/approve", post(approve))
        .route("/oauth/pending/{id}/deny", post(deny))
}

#[derive(Serialize)]
struct InstanceSummary {
    slug: String,
    server_id: String,
}

#[derive(Serialize)]
struct PendingDetails {
    /// Whatever the client told us about itself: unverified.
    client_name_self_reported: Option<String>,
    redirect_host: Option<String>,
    redirect_is_loopback: bool,
    scope: String,
    requested_slug: Option<String>,
    /// `null` when the logged-in user has no instance with this slug.
    instance: Option<InstanceSummary>,
}

#[derive(Serialize)]
struct Decision {
    redirect_url: String,
}

async fn requested_instance(
    state: &AppState,
    user_id: i64,
    pending: &Pending,
) -> Result<Option<Instance>, ApiError> {
    let Some(slug) = slug_from_resource(&state.config.public_url, &pending.resource) else {
        return Ok(None);
    };
    Ok(db::instances::find_by_slug(&state.pool, user_id, slug).await?)
}

async fn pending_details(
    State(state): State<AppState>,
    session: CurrentSession,
    Path(id): Path<String>,
) -> Result<Json<PendingDetails>, ApiError> {
    let pending = db::oauth::find_pending(&state.pool, &id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let client = db::oauth::find_client(&state.pool, &pending.client_id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let instance = requested_instance(&state, session.user_id, &pending).await?;
    let redirect = Url::parse(&pending.redirect_uri).ok();
    let requested_slug =
        slug_from_resource(&state.config.public_url, &pending.resource).map(str::to_owned);

    Ok(Json(PendingDetails {
        client_name_self_reported: client.client_name,
        redirect_host: redirect
            .as_ref()
            .and_then(|url| url.host_str().map(str::to_owned)),
        redirect_is_loopback: redirect.as_ref().is_some_and(host_is_loopback),
        scope: pending.scope,
        requested_slug,
        instance: instance.map(|instance| InstanceSummary {
            slug: instance.slug,
            server_id: instance.server_id,
        }),
    }))
}

async fn approve(
    State(state): State<AppState>,
    session: CurrentSession,
    Path(id): Path<String>,
) -> Result<Json<Decision>, ApiError> {
    // Checked before claiming, so the user can create a missing instance and retry.
    let unclaimed = db::oauth::find_pending(&state.pool, &id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let instance = requested_instance(&state, session.user_id, &unclaimed)
        .await?
        .ok_or_else(|| ApiError::Conflict("you have no instance with this slug".to_owned()))?;
    let pending = db::oauth::consume_pending(&state.pool, &id)
        .await?
        .ok_or(ApiError::NotFound)?;

    let grant_id = db::oauth::find_or_create_grant(
        &state.pool,
        session.user_id,
        &pending.client_id,
        instance.id,
        &pending.resource,
        &pending.scope,
    )
    .await?;

    let code = Token::generate();
    let code_kek = code.kek(WrapKind::AuthCode)?;
    let dek = session.dek()?;
    db::oauth::create_code(
        &state.pool,
        NewCode {
            code_hash: &code.hash(),
            grant_id,
            redirect_uri: &pending.redirect_uri,
            code_challenge: &pending.code_challenge,
            resource: &pending.resource,
            ttl_secs: state.config.settings.oauth.auth_code_ttl_secs,
        },
        |code_id| {
            crypto::wrap_dek(
                &code_kek,
                &dek,
                WrapKind::AuthCode,
                code_id,
                session.user_id,
            )
        },
    )
    .await?;

    let redirect_url = redirect_with(
        &pending.redirect_uri,
        &[("code", &code.encode(""))],
        pending.state.as_deref(),
        state.config.public_url.as_str(),
    );
    Ok(Json(Decision { redirect_url }))
}

async fn deny(
    State(state): State<AppState>,
    _session: CurrentSession,
    Path(id): Path<String>,
) -> Result<Json<Decision>, ApiError> {
    let pending = db::oauth::consume_pending(&state.pool, &id)
        .await?
        .ok_or(ApiError::NotFound)?;
    let redirect_url = error_redirect(
        &pending.redirect_uri,
        "access_denied",
        pending.state.as_deref(),
        state.config.public_url.as_str(),
    );
    Ok(Json(Decision { redirect_url }))
}
