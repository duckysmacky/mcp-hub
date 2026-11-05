//! The clients a user has connected, and revoking them.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get},
};
use serde::Serialize;
use serde_json::json;
use time::OffsetDateTime;
use url::Url;

use crate::{app_state::AppState, audit, auth::CurrentSession, db, error::ApiError};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/grants", get(list_grants))
        .route("/grants/{id}", delete(revoke_grant))
}

#[derive(Serialize)]
struct GrantResponse {
    id: i64,
    instance_id: i64,
    slug: String,
    server_id: String,
    /// Whatever the client told us about itself: unverified.
    client_name_self_reported: Option<String>,
    client_uri_self_reported: Option<String>,
    logo_uri_self_reported: Option<String>,
    redirect_hosts: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    last_used_at: Option<OffsetDateTime>,
}

async fn list_grants(
    State(state): State<AppState>,
    session: CurrentSession,
) -> Result<Json<Vec<GrantResponse>>, ApiError> {
    let grants = db::oauth::list_grants(&state.pool, session.user_id)
        .await?
        .into_iter()
        .map(|grant| {
            let mut redirect_hosts: Vec<String> = grant
                .redirect_uris
                .0
                .iter()
                .filter_map(|uri| Url::parse(uri).ok()?.host_str().map(str::to_owned))
                .collect();
            redirect_hosts.sort();
            redirect_hosts.dedup();
            GrantResponse {
                id: grant.id,
                instance_id: grant.instance_id,
                slug: grant.slug,
                server_id: grant.server_id,
                client_name_self_reported: grant.client_name,
                client_uri_self_reported: grant.client_uri,
                logo_uri_self_reported: grant.logo_uri,
                redirect_hosts,
                created_at: grant.created_at,
                last_used_at: grant.last_used_at,
            }
        })
        .collect();
    Ok(Json(grants))
}

async fn revoke_grant(
    State(state): State<AppState>,
    session: CurrentSession,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let instance_id = db::oauth::delete_grant(&state.pool, session.user_id, id)
        .await?
        .ok_or(ApiError::NotFound)?;
    // Nothing should keep running on an approval the user took back. Other clients of the
    // instance get a 404 and re-initialize.
    state.supervisor.stop_instance(instance_id);
    audit::record(
        &state.pool,
        Some(session.user_id),
        Some(instance_id),
        "grant_revoked",
        Some(json!({ "grant_id": id })),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
