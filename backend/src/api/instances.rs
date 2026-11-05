//! Instance management. Env values are write-only: no response ever contains one.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap, HashSet},
};

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
    catalog::{
        CUSTOM_SERVER_ID, CustomServer, EnvSpec, RuntimeKind, ServerMode, ServerTemplate, install,
    },
    crypto, db,
    db::instances::{Instance, is_valid_slug},
    error::ApiError,
    net_guard,
};

const MAX_ENV_VALUE_BYTES: usize = 64 * 1024;
const MAX_DISABLED_TOOLS: usize = 1000;
const MAX_TOOL_NAME_BYTES: usize = 1024;
const CUSTOM_SERVERS_OFF: &str = "the operator has turned custom servers off";

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/catalog", get(list_catalog))
        .route("/catalog/settings", get(catalog_settings))
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
        .route("/instances/{id}/tools", get(instance_tools))
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
struct CatalogSettingsResponse {
    custom_servers: bool,
}

async fn catalog_settings(
    State(state): State<AppState>,
    _session: CurrentSession,
) -> Json<CatalogSettingsResponse> {
    Json(CatalogSettingsResponse {
        custom_servers: state.config.settings.catalog.allow_custom_servers,
    })
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
    /// Fixed by the catalog or the custom definition. `null` once the server has left it.
    runtime: Option<RuntimeKind>,
    package: Option<String>,
    enabled: bool,
    needs_attention: Option<String>,
    url: String,
    env: Vec<EnvStatus>,
    missing_required: Vec<String>,
    disabled_tools: Vec<String>,
    /// The owner's own definition. It holds no values: those are in `env`.
    custom: Option<CustomServer>,
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
    let template = state.template_for(&instance);
    let template = template.as_deref();
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
        disabled_tools: instance.disabled_tools.0,
        custom: instance.custom.map(|custom| custom.0),
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
) -> Result<Cow<'a, ServerTemplate>, ApiError> {
    let template = state.template_for(instance).ok_or_else(|| {
        ApiError::Conflict(if instance.custom.is_some() {
            CUSTOM_SERVERS_OFF.to_owned()
        } else {
            "this server was removed from the catalog".to_owned()
        })
    })?;
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
    /// A catalog server, or `custom` for a definition of the user's own.
    #[serde(default)]
    server_id: Option<String>,
    slug: String,
    #[serde(default)]
    custom: Option<CustomServer>,
}

/// Checks a definition sent by a user, and that this hub accepts them at all.
fn check_custom(state: &AppState, custom: &CustomServer) -> Result<(), ApiError> {
    if !state.config.settings.catalog.allow_custom_servers {
        return Err(ApiError::BadRequest(CUSTOM_SERVERS_OFF.to_owned()));
    }
    custom.validate().map_err(ApiError::BadRequest)
}

async fn create_instance(
    State(state): State<AppState>,
    session: CurrentSession,
    Json(request): Json<CreateInstance>,
) -> Result<(StatusCode, Json<InstanceResponse>), ApiError> {
    validate_slug(&request.slug)?;
    let server_id = match (&request.server_id, &request.custom) {
        (Some(id), None) if state.catalog.get(id).is_some_and(|server| server.enabled) => {
            id.as_str()
        }
        (None, Some(custom)) => {
            check_custom(&state, custom)?;
            CUSTOM_SERVER_ID
        }
        _ => return Err(ApiError::BadRequest("unknown server".to_owned())),
    };
    // ponytail: two concurrent creates can pass this together. It bounds abuse, not exactly.
    let max_instances = state.config.settings.limits.max_instances_per_user;
    if db::instances::count(&state.pool, session.user_id).await? >= max_instances {
        return Err(ApiError::Conflict(format!(
            "you can have at most {max_instances} instances"
        )));
    }
    let instance = db::instances::create(
        &state.pool,
        session.user_id,
        &request.slug,
        server_id,
        request.custom.as_ref(),
    )
    .await?;
    let mut detail = json!({ "server_id": instance.server_id, "slug": instance.slug });
    if let Some(custom) = &request.custom {
        // Installed after the insert, as its directory is named after the instance id.
        if let Err(error) = state.install_custom(&custom.to_template(instance.id)).await {
            db::instances::delete(&state.pool, session.user_id, instance.id).await?;
            return Err(ApiError::BadRequest(error.to_string()));
        }
        detail["custom"] = json!(true);
        detail["package"] = json!(custom.package);
    }
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
    disabled_tools: Option<Vec<String>>,
    /// Only on a custom instance.
    custom: Option<CustomServer>,
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
    if let Some(tools) = &request.disabled_tools {
        let is_bounded = tools.len() <= MAX_DISABLED_TOOLS
            && tools
                .iter()
                .all(|name| !name.is_empty() && name.len() <= MAX_TOOL_NAME_BYTES);
        if !is_bounded {
            return Err(ApiError::BadRequest(
                "disabled_tools has too many names or an invalid one".to_owned(),
            ));
        }
    }
    if let Some(custom) = &request.custom {
        update_custom(&state, &session, id, custom).await?;
    }
    let instance = db::instances::update(
        &state.pool,
        session.user_id,
        id,
        request.slug.as_deref(),
        request.disabled_tools.as_deref(),
    )
    .await?
    .ok_or(ApiError::NotFound)?;
    // Running processes were started under the old slug. The tool filter needs no restart:
    // it's applied to every request.
    if request.slug.is_some() {
        state.supervisor.stop_instance(id);
    }
    Ok(Json(describe(&state, &session, instance).await?))
}

/// Replaces a definition. Running processes read the files being replaced, so they stop first,
/// and a failed install keeps the old definition, which installs again when next needed.
async fn update_custom(
    state: &AppState,
    session: &CurrentSession,
    id: i64,
    custom: &CustomServer,
) -> Result<(), ApiError> {
    let instance = owned_instance(state, session, id).await?;
    let Some(current) = &instance.custom else {
        return Err(ApiError::BadRequest(
            "only a custom instance has a definition to change".to_owned(),
        ));
    };
    check_custom(state, custom)?;
    state.supervisor.stop_instance(id);
    if current.0.runtime != custom.runtime || current.0.package != custom.package {
        state
            .install_custom(&custom.to_template(id))
            .await
            .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    }
    db::instances::update_custom(&state.pool, session.user_id, id, custom).await?;
    audit::record(
        &state.pool,
        Some(session.user_id),
        Some(id),
        "custom_server_updated",
        Some(json!({ "package": custom.package })),
    )
    .await;
    Ok(())
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
    if let Err(error) = install::remove_custom(id, &state.config.settings).await {
        tracing::warn!(%error, instance_id = id, "could not remove a custom server's files");
    }
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
    db::instances::clear_secrets_reset(&state.pool, id).await?;
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
    let template = state.template_for(&instance);
    let template = template.as_deref();
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

#[derive(Serialize)]
struct ToolStatus {
    name: String,
    description: Option<String>,
    disabled: bool,
}

/// The tools seen in the last `tools/list`, plus disabled ones that weren't: a filter the
/// user set must stay visible and removable.
async fn instance_tools(
    State(state): State<AppState>,
    session: CurrentSession,
    Path(id): Path<i64>,
) -> Result<Json<Vec<ToolStatus>>, ApiError> {
    let instance = owned_instance(&state, &session, id).await?;
    let disabled = instance.disabled_tools.0;
    let mut tools: BTreeMap<String, ToolStatus> = BTreeMap::new();
    for tool in db::instances::last_tools(&state.pool, id).await? {
        tools
            .entry(tool.name.clone())
            .or_insert_with(|| ToolStatus {
                disabled: disabled.contains(&tool.name),
                name: tool.name,
                description: tool.description,
            });
    }
    for name in disabled {
        tools.entry(name.clone()).or_insert(ToolStatus {
            name,
            description: None,
            disabled: true,
        });
    }
    Ok(Json(tools.into_values().collect()))
}
