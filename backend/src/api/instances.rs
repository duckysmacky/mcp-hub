//! Instance management. Env values are write-only: no response ever contains one.

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use zeroize::Zeroizing;

use crate::{
    app_state::AppState,
    auth::CurrentSession,
    catalog::{EnvSpec, RuntimeKind, ServerMode, ServerTemplate},
    crypto, db,
    db::instances::{Instance, is_valid_slug},
    error::ApiError,
    net_guard,
};

const MAX_ENV_VALUE_BYTES: usize = 64 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/catalog", get(list_catalog))
        .route("/instances", get(list_instances).post(create_instance))
        .route(
            "/instances/{id}",
            get(get_instance)
                .patch(update_instance)
                .delete(delete_instance),
        )
        .route(
            "/instances/{id}/env/{var}",
            put(set_env_value).delete(delete_env_value),
        )
        .route("/instances/{id}/start", post(start_instance))
        .route("/instances/{id}/stop", post(stop_instance))
        .route("/instances/{id}/restart", post(restart_instance))
        .route("/instances/{id}/status", get(instance_status))
}

#[derive(Serialize)]
struct CatalogServer {
    id: String,
    mode: ServerMode,
    runtime: RuntimeKind,
    package: String,
    env: Vec<EnvSpec>,
}

async fn list_catalog(
    State(state): State<AppState>,
    _session: CurrentSession,
) -> Json<Vec<CatalogServer>> {
    let servers = state
        .catalog
        .servers()
        .filter(|server| server.enabled)
        .map(|server| CatalogServer {
            id: server.id.clone(),
            mode: server.mode,
            runtime: server.runtime,
            package: server.package.clone(),
            env: server.env.clone(),
        })
        .collect();
    Json(servers)
}

#[derive(Serialize)]
struct EnvStatus {
    var: String,
    is_set: bool,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
    in_catalog: bool,
}

#[derive(Serialize)]
struct InstanceResponse {
    id: i64,
    slug: String,
    server_id: String,
    runtime: Option<String>,
    enabled: bool,
    needs_attention: Option<String>,
    url: String,
    env: Vec<EnvStatus>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

async fn describe(state: &AppState, instance: Instance) -> Result<InstanceResponse, ApiError> {
    let template = state.catalog.get(&instance.server_id);
    let env = db::instances::list_env(&state.pool, instance.id)
        .await?
        .into_iter()
        .map(|entry| EnvStatus {
            in_catalog: template.is_some_and(|server| server.env_spec(&entry.env_var).is_some()),
            var: entry.env_var,
            is_set: true,
            updated_at: entry.updated_at,
        })
        .collect();
    Ok(InstanceResponse {
        id: instance.id,
        url: format!("{}/mcp/{}", state.config.public_url.as_str(), instance.slug),
        slug: instance.slug,
        server_id: instance.server_id,
        runtime: instance.runtime,
        enabled: instance.enabled,
        needs_attention: instance.needs_attention,
        env,
        created_at: instance.created_at,
        updated_at: instance.updated_at,
    })
}

async fn owned_instance(
    state: &AppState,
    session: &CurrentSession,
    id: i64,
) -> Result<Instance, ApiError> {
    db::instances::get(&state.pool, session.user_id, id)
        .await?
        .ok_or(ApiError::NotFound)
}

fn template_of<'a>(
    state: &'a AppState,
    instance: &Instance,
) -> Result<&'a ServerTemplate, ApiError> {
    state
        .catalog
        .get(&instance.server_id)
        .ok_or(ApiError::Conflict(
            "this server was removed from the catalog",
        ))
}

fn validate_slug(slug: &str) -> Result<(), ApiError> {
    if is_valid_slug(slug) {
        Ok(())
    } else {
        Err(ApiError::BadRequest(
            "slug must be lowercase letters, digits, and dashes, starting with a letter or digit"
                .to_owned(),
        ))
    }
}

async fn list_instances(
    State(state): State<AppState>,
    session: CurrentSession,
) -> Result<Json<Vec<InstanceResponse>>, ApiError> {
    let mut responses = Vec::new();
    for instance in db::instances::list(&state.pool, session.user_id).await? {
        responses.push(describe(&state, instance).await?);
    }
    Ok(Json(responses))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateInstance {
    server_id: String,
    slug: String,
    runtime: Option<RuntimeKind>,
}

async fn create_instance(
    State(state): State<AppState>,
    session: CurrentSession,
    Json(request): Json<CreateInstance>,
) -> Result<(StatusCode, Json<InstanceResponse>), ApiError> {
    validate_slug(&request.slug)?;
    if state.catalog.get(&request.server_id).is_none() {
        return Err(ApiError::BadRequest("unknown server".to_owned()));
    }
    let instance = db::instances::create(
        &state.pool,
        session.user_id,
        &request.slug,
        &request.server_id,
        request.runtime.map(RuntimeKind::as_str),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(describe(&state, instance).await?)))
}

async fn get_instance(
    State(state): State<AppState>,
    session: CurrentSession,
    Path(id): Path<i64>,
) -> Result<Json<InstanceResponse>, ApiError> {
    let instance = owned_instance(&state, &session, id).await?;
    Ok(Json(describe(&state, instance).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateInstance {
    slug: Option<String>,
    runtime: Option<RuntimeKind>,
}

async fn update_instance(
    State(state): State<AppState>,
    session: CurrentSession,
    Path(id): Path<i64>,
    Json(request): Json<UpdateInstance>,
) -> Result<Json<InstanceResponse>, ApiError> {
    if let Some(slug) = &request.slug {
        validate_slug(slug)?;
    }
    let instance = db::instances::update(
        &state.pool,
        session.user_id,
        id,
        request.slug.as_deref(),
        request.runtime.map(RuntimeKind::as_str),
    )
    .await?
    .ok_or(ApiError::NotFound)?;
    // Running processes were started under the old slug or runtime.
    state.supervisor.stop_instance(id);
    Ok(Json(describe(&state, instance).await?))
}

async fn delete_instance(
    State(state): State<AppState>,
    session: CurrentSession,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if !db::instances::delete(&state.pool, session.user_id, id).await? {
        return Err(ApiError::NotFound);
    }
    state.supervisor.forget_instance(id);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetEnvValue {
    value: Zeroizing<String>,
}

async fn set_env_value(
    State(state): State<AppState>,
    session: CurrentSession,
    Path((id, var)): Path<(i64, String)>,
    Json(request): Json<SetEnvValue>,
) -> Result<StatusCode, ApiError> {
    let instance = owned_instance(&state, &session, id).await?;
    let template = template_of(&state, &instance)?;
    if template.env_spec(&var).is_none() {
        return Err(ApiError::BadRequest(
            "this server doesn't allow that variable".to_owned(),
        ));
    }
    if request.value.len() > MAX_ENV_VALUE_BYTES {
        return Err(ApiError::BadRequest("value is too long".to_owned()));
    }
    // A NUL can't exist in a process env, so the child could never be spawned.
    if request.value.contains('\0') {
        return Err(ApiError::BadRequest("value contains a NUL byte".to_owned()));
    }
    net_guard::check_env_value(&request.value, &state.config.settings.network)
        .await
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;

    let dek = session.dek()?;
    let ciphertext = crypto::encrypt_env(&dek, id, &var, &request.value)?;
    db::instances::set_env(&state.pool, id, &var, &ciphertext).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_env_value(
    State(state): State<AppState>,
    session: CurrentSession,
    Path((id, var)): Path<(i64, String)>,
) -> Result<StatusCode, ApiError> {
    owned_instance(&state, &session, id).await?;
    if !db::instances::delete_env(&state.pool, id, &var).await? {
        return Err(ApiError::NotFound);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn start_instance(
    State(state): State<AppState>,
    session: CurrentSession,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let instance = owned_instance(&state, &session, id).await?;
    template_of(&state, &instance)?;
    db::instances::set_enabled(&state.pool, session.user_id, id, true).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn stop_instance(
    State(state): State<AppState>,
    session: CurrentSession,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    if !db::instances::set_enabled(&state.pool, session.user_id, id, false).await? {
        return Err(ApiError::NotFound);
    }
    state.supervisor.stop_instance(id);
    Ok(StatusCode::NO_CONTENT)
}

async fn restart_instance(
    State(state): State<AppState>,
    session: CurrentSession,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    owned_instance(&state, &session, id).await?;
    state.supervisor.stop_instance(id);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct StatusResponse {
    enabled: bool,
    needs_attention: Option<String>,
    mode: Option<ServerMode>,
    runtime: Option<RuntimeKind>,
    live_processes: usize,
    last_exit: Option<String>,
    output_tail: String,
}

async fn instance_status(
    State(state): State<AppState>,
    session: CurrentSession,
    Path(id): Path<i64>,
) -> Result<Json<StatusResponse>, ApiError> {
    let instance = owned_instance(&state, &session, id).await?;
    let status = state.supervisor.instance_status(id);
    let template = state.catalog.get(&instance.server_id);
    Ok(Json(StatusResponse {
        enabled: instance.enabled,
        mode: template.map(|server| server.mode),
        runtime: template.map(|server| instance.effective_runtime(server)),
        needs_attention: instance.needs_attention,
        live_processes: status.live_processes,
        last_exit: status.last_exit,
        output_tail: status.output_tail,
    }))
}
