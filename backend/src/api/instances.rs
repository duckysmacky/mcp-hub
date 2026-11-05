//! Instance management. Env values are write-only: no response ever contains one.

use std::collections::{HashMap, HashSet};

use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post, put},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use time::OffsetDateTime;
use zeroize::Zeroizing;

use crate::{
    app_state::AppState,
    audit,
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
    /// Only for vars the catalog marks `secret = false`. A secret is never returned.
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<Zeroizing<String>>,
}

#[derive(Serialize)]
struct InstanceResponse {
    id: i64,
    slug: String,
    server_id: String,
    /// Fixed by the catalog. `null` once the server has left it.
    runtime: Option<RuntimeKind>,
    package: Option<String>,
    enabled: bool,
    needs_attention: Option<String>,
    url: String,
    env: Vec<EnvStatus>,
    missing_required: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

/// Decrypts the instance's non-secret values. Secrets are left out here, before anything
/// could put them in a response.
async fn readable_values(
    state: &AppState,
    session: &CurrentSession,
    instance_id: i64,
    template: &ServerTemplate,
) -> Result<HashMap<String, Zeroizing<String>>, ApiError> {
    let readable: Vec<_> = db::instances::env_ciphertexts(&state.pool, instance_id)
        .await?
        .into_iter()
        .filter(|row| {
            template
                .env_spec(&row.env_var)
                .is_some_and(|spec| !spec.secret)
        })
        .collect();
    if readable.is_empty() {
        return Ok(HashMap::new());
    }
    let dek = session.dek()?;
    readable
        .into_iter()
        .map(|row| {
            let value = crypto::decrypt_env(&dek, instance_id, &row.env_var, &row.ciphertext)?;
            Ok((row.env_var, value))
        })
        .collect()
}

async fn describe(
    state: &AppState,
    session: &CurrentSession,
    instance: Instance,
) -> Result<InstanceResponse, ApiError> {
    let template = state.catalog.get(&instance.server_id);
    let mut readable = match template {
        Some(template) => readable_values(state, session, instance.id, template).await?,
        None => HashMap::new(),
    };
    let env: Vec<EnvStatus> = db::instances::list_env(&state.pool, instance.id)
        .await?
        .into_iter()
        .map(|entry| EnvStatus {
            in_catalog: template.is_some_and(|server| server.env_spec(&entry.env_var).is_some()),
            value: readable.remove(&entry.env_var),
            var: entry.env_var,
            is_set: true,
            updated_at: entry.updated_at,
        })
        .collect();
    let set_vars: HashSet<&str> = env.iter().map(|entry| entry.var.as_str()).collect();
    let missing_required = template.map_or_else(Vec::new, |server| {
        server
            .missing_required(&set_vars)
            .into_iter()
            .map(str::to_owned)
            .collect()
    });
    Ok(InstanceResponse {
        id: instance.id,
        url: format!("{}/mcp/{}", state.config.public_url.as_str(), instance.slug),
        slug: instance.slug,
        server_id: instance.server_id,
        runtime: template.map(|server| server.runtime),
        package: template.map(|server| server.package.clone()),
        enabled: instance.enabled,
        needs_attention: instance.needs_attention,
        env,
        missing_required,
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
    let template = state
        .catalog
        .get(&instance.server_id)
        .ok_or_else(|| ApiError::Conflict("this server was removed from the catalog".to_owned()))?;
    if !template.enabled {
        return Err(ApiError::Conflict(
            "the operator has turned this server off".to_owned(),
        ));
    }
    Ok(template)
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
        responses.push(describe(&state, &session, instance).await?);
    }
    Ok(Json(responses))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateInstance {
    server_id: String,
    slug: String,
}

async fn create_instance(
    State(state): State<AppState>,
    session: CurrentSession,
    Json(request): Json<CreateInstance>,
) -> Result<(StatusCode, Json<InstanceResponse>), ApiError> {
    validate_slug(&request.slug)?;
    if !state
        .catalog
        .get(&request.server_id)
        .is_some_and(|server| server.enabled)
    {
        return Err(ApiError::BadRequest("unknown server".to_owned()));
    }
    let instance = db::instances::create(
        &state.pool,
        session.user_id,
        &request.slug,
        &request.server_id,
    )
    .await?;
    let detail = json!({ "server_id": instance.server_id, "slug": instance.slug });
    audit::record(
        &state.pool,
        Some(session.user_id),
        Some(instance.id),
        "instance_created",
        Some(detail),
    )
    .await;
    Ok((
        StatusCode::CREATED,
        Json(describe(&state, &session, instance).await?),
    ))
}

async fn get_instance(
    State(state): State<AppState>,
    session: CurrentSession,
    Path(id): Path<i64>,
) -> Result<Json<InstanceResponse>, ApiError> {
    let instance = owned_instance(&state, &session, id).await?;
    Ok(Json(describe(&state, &session, instance).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateInstance {
    slug: Option<String>,
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
    let instance = db::instances::update(&state.pool, session.user_id, id, request.slug.as_deref())
        .await?
        .ok_or(ApiError::NotFound)?;
    // Running processes were started under the old slug.
    state.supervisor.stop_instance(id);
    Ok(Json(describe(&state, &session, instance).await?))
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
    let detail = json!({ "instance_id": id });
    audit::record(
        &state.pool,
        Some(session.user_id),
        None,
        "instance_deleted",
        Some(detail),
    )
    .await;
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
    let spec = template.env_spec(&var).ok_or_else(|| {
        ApiError::BadRequest("this server doesn't allow that variable".to_owned())
    })?;
    if request.value.len() > MAX_ENV_VALUE_BYTES {
        return Err(ApiError::BadRequest("value is too long".to_owned()));
    }
    // A NUL can't exist in a process env, so the child could never be spawned.
    if request.value.contains('\0') {
        return Err(ApiError::BadRequest("value contains a NUL byte".to_owned()));
    }
    spec.validate_value(&request.value)
        .map_err(ApiError::BadRequest)?;
    let existing = db::instances::list_env(&state.pool, id).await?;
    if let Some(conflict) = existing
        .iter()
        .find(|entry| spec.conflicts.contains(&entry.env_var))
    {
        return Err(ApiError::Conflict(format!(
            "{var} can't be set together with {}; remove that first",
            conflict.env_var
        )));
    }
    net_guard::check_env_value(&request.value, &state.config.settings.network)
        .await
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;

    let dek = session.dek()?;
    let ciphertext = crypto::encrypt_env(&dek, id, &var, &request.value)?;
    db::instances::set_env(&state.pool, id, &var, &ciphertext).await?;
    audit::record(
        &state.pool,
        Some(session.user_id),
        Some(id),
        "env_set",
        Some(json!({ "var": var })),
    )
    .await;
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
    audit::record(
        &state.pool,
        Some(session.user_id),
        Some(id),
        "env_removed",
        Some(json!({ "var": var })),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

async fn start_instance(
    State(state): State<AppState>,
    session: CurrentSession,
    Path(id): Path<i64>,
) -> Result<StatusCode, ApiError> {
    let instance = owned_instance(&state, &session, id).await?;
    let template = template_of(&state, &instance)?;
    let existing = db::instances::list_env(&state.pool, id).await?;
    let set_vars = existing
        .iter()
        .map(|entry| entry.env_var.as_str())
        .collect();
    let missing = template.missing_required(&set_vars);
    if !missing.is_empty() {
        return Err(ApiError::Conflict(format!(
            "set these values first: {}",
            missing.join(", ")
        )));
    }
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
        runtime: template.map(|server| server.runtime),
        needs_attention: instance.needs_attention,
        live_processes: status.live_processes,
        last_exit: status.last_exit,
        output_tail: status.output_tail,
    }))
}
