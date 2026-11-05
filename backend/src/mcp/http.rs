//! `/mcp/{slug}` for `http` servers: a reverse proxy that injects the user's
//! credentials as headers. The client's own `Authorization` never reaches the upstream.

use std::{sync::Arc, time::Duration};

use axum::{
    Json,
    body::Body,
    http::{HeaderMap, HeaderName, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::{StreamExt, stream};
use serde_json::Value;
use sqlx::PgPool;
use zeroize::Zeroizing;

use super::{
    McpError, Message, SESSION_HEADER, Target, ToolListPage, json_error, no_buffering,
    remember_tools, required_session_id, start_failure, tool_list_page,
};
use crate::{
    bridge::{
        http::{InFlightRequest, UpstreamProcess},
        tools::{SseToolFilter, filter_tool_list},
    },
    catalog::{HttpSettings, render_template},
    crypto, db,
    process::{EnvValue, declared_process_env, secret_values},
    supervisor::StartUpstream,
};

const FORWARDED_REQUEST_HEADERS: [HeaderName; 5] = [
    header::CONTENT_TYPE,
    header::ACCEPT,
    HeaderName::from_static("mcp-protocol-version"),
    SESSION_HEADER,
    HeaderName::from_static("last-event-id"),
];
const FORWARDED_RESPONSE_HEADERS: [HeaderName; 5] = [
    header::CONTENT_TYPE,
    header::CACHE_CONTROL,
    header::RETRY_AFTER,
    HeaderName::from_static("mcp-protocol-version"),
    SESSION_HEADER,
];

pub async fn post(
    target: &Target<'_>,
    headers: &HeaderMap,
    kind: Message,
    message: Value,
) -> Result<Response, McpError> {
    if matches!(kind, Message::Initialize) {
        return initialize(target, headers, message).await;
    }
    let session_id = required_session_id(headers)?;
    let upstream = find_session_upstream(target, session_id)?;
    let in_flight = upstream.begin_request();
    let id = message.get("id").cloned();
    let tool_list = tool_list_page(&message);
    let forwarded = forward(
        target,
        &upstream,
        Forward {
            method: Method::POST,
            headers,
            body: Some(message.to_string()),
            timeout: request_timeout(target),
        },
    )
    .await;
    let response = match forwarded {
        Ok(response) => response,
        // The process went away between the lookup and the request.
        Err(ForwardError::Unreachable) => return Err(McpError::NotFound),
        Err(ForwardError::Failed(error)) => return Err(error),
    };
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        upstream.forget_session(session_id);
    }
    match (tool_list, &id) {
        (Some(page), Some(id)) => respond_tool_list(target, response, id, in_flight, page).await,
        _ => respond(response, id.as_ref(), Some(in_flight)),
    }
}

/// Starts the instance's process if needed, forwards `initialize`, records the session.
async fn initialize(
    target: &Target<'_>,
    headers: &HeaderMap,
    message: Value,
) -> Result<Response, McpError> {
    let id = message["id"].clone();
    let http = http_settings(target)?;
    let values = target.decrypt_env().await?;
    if let Err(reason) = target.check_values(&values).await {
        return Ok(json_error(&id, &reason));
    }

    let secrets = secret_values(target.template, &values);
    let already_running = target
        .state
        .supervisor
        .upstream(target.instance.id)
        .is_some();
    let started = target
        .state
        .supervisor
        .ensure_upstream(StartUpstream {
            user_id: target.auth.user_id,
            instance_id: target.instance.id,
            runtime: target.template.runtime,
            server: target.resolved_server()?,
            args: &target.template.bin_args,
            http,
            env: declared_process_env(target.template, values),
            secrets,
        })
        .await;
    let upstream = match started {
        Ok(upstream) => upstream,
        Err(error) => return start_failure(&id, error),
    };
    if !already_running {
        target.audit("process_spawned", None).await;
    }
    if !upstream.wait_ready().await {
        return Ok(json_error(
            &id,
            "the server did not start; check the instance status",
        ));
    }

    let in_flight = upstream.begin_request();
    let timeout = Duration::from_secs(target.state.config.settings.limits.initialize_timeout_secs);
    let forwarded = forward(
        target,
        &upstream,
        Forward {
            method: Method::POST,
            headers,
            body: Some(message.to_string()),
            timeout,
        },
    )
    .await;
    let response = match forwarded {
        Ok(response) => response,
        Err(ForwardError::Unreachable) => {
            return Ok(json_error(
                &id,
                "the server stopped responding; check the instance status",
            ));
        }
        Err(ForwardError::Failed(error)) => return Err(error),
    };
    if let Some(session_id) = response
        .headers()
        .get(SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
    {
        upstream.record_session(session_id);
    }
    respond(response, Some(&id), Some(in_flight))
}

/// Streams server-initiated messages. An open stream doesn't count as activity.
pub async fn get(target: &Target<'_>, headers: &HeaderMap) -> Result<Response, McpError> {
    let session_id = required_session_id(headers)?;
    let upstream = find_session_upstream(target, session_id)?;
    let forwarded = forward(
        target,
        &upstream,
        Forward {
            method: Method::GET,
            headers,
            body: None,
            timeout: request_timeout(target),
        },
    )
    .await;
    match forwarded {
        Ok(response) => respond(response, None, None),
        Err(ForwardError::Unreachable) => Err(McpError::NotFound),
        Err(ForwardError::Failed(error)) => Err(error),
    }
}

pub async fn delete(target: &Target<'_>, headers: &HeaderMap) -> Result<Response, McpError> {
    let session_id = required_session_id(headers)?;
    let upstream = find_session_upstream(target, session_id)?;
    let forwarded = forward(
        target,
        &upstream,
        Forward {
            method: Method::DELETE,
            headers,
            body: None,
            timeout: request_timeout(target),
        },
    )
    .await;
    upstream.forget_session(session_id);
    match forwarded {
        Ok(response) if response.status().is_success() => {
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        Ok(response) => respond(response, None, None),
        Err(ForwardError::Unreachable) => Err(McpError::NotFound),
        Err(ForwardError::Failed(error)) => Err(error),
    }
}

fn request_timeout(target: &Target<'_>) -> Duration {
    Duration::from_secs(target.state.config.settings.limits.request_timeout_secs)
}

fn http_settings<'a>(target: &Target<'a>) -> Result<&'a HttpSettings, McpError> {
    target.template.http.as_ref().ok_or(McpError::NotFound)
}

/// The session must be one this hub recorded on this instance's process, and the instance
/// was already resolved through the token's user: an id from the request isn't enough.
fn find_session_upstream(
    target: &Target<'_>,
    session_id: &str,
) -> Result<Arc<UpstreamProcess>, McpError> {
    let upstream = target
        .state
        .supervisor
        .upstream(target.instance.id)
        .filter(|upstream| upstream.owner_user_id == target.auth.user_id)
        .ok_or(McpError::NotFound)?;
    if !upstream.has_session(session_id) {
        return Err(McpError::NotFound);
    }
    upstream.touch();
    Ok(upstream)
}

struct Forward<'a> {
    method: Method,
    headers: &'a HeaderMap,
    body: Option<String>,
    timeout: Duration,
}

enum ForwardError {
    Unreachable,
    Failed(McpError),
}

impl<E: Into<McpError>> From<E> for ForwardError {
    fn from(error: E) -> Self {
        Self::Failed(error.into())
    }
}

/// Sends one request upstream. The body is streamed later, so `timeout` only covers
/// the time until the response headers arrive.
async fn forward(
    target: &Target<'_>,
    upstream: &UpstreamProcess,
    request: Forward<'_>,
) -> Result<reqwest::Response, ForwardError> {
    let mut outgoing = target
        .state
        .supervisor
        .upstream_client()
        .request(request.method, upstream.url());
    for name in &FORWARDED_REQUEST_HEADERS {
        for value in request.headers.get_all(name) {
            outgoing = outgoing.header(name, value);
        }
    }

    let rendered = render_credential_headers(target).await?;
    for (name, value) in &rendered {
        let mut value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|_| McpError::BadRequest("an env value can't be sent as a request header"))?;
        value.set_sensitive(true);
        outgoing = outgoing.header(name, value);
    }
    drop(rendered);

    if let Some(body) = request.body {
        outgoing = outgoing.body(body);
    }
    match tokio::time::timeout(request.timeout, outgoing.send()).await {
        Ok(Ok(response)) => Ok(response),
        // Errors aren't logged in full: they could include the request's URL or headers.
        Ok(Err(_)) | Err(_) => Err(ForwardError::Unreachable),
    }
}

/// Decrypts the values used in header templates for this one request. A template whose
/// var isn't set is skipped, and the upstream reports the missing credential itself.
async fn render_credential_headers(
    target: &Target<'_>,
) -> Result<Vec<(String, Zeroizing<String>)>, McpError> {
    let http = http_settings(target)?;
    let header_vars = http.header_vars();
    let dek = target.auth.dek()?;
    let mut values: Vec<EnvValue> = Vec::new();
    for row in db::instances::env_ciphertexts(&target.state.pool, target.instance.id).await? {
        if header_vars.contains(row.env_var.as_str()) {
            let value =
                crypto::decrypt_env(&dek, target.instance.id, &row.env_var, &row.ciphertext)?;
            values.push((row.env_var, value));
        }
    }
    Ok(http
        .headers
        .iter()
        .filter_map(|(name, template)| {
            render_template(template, &values).map(|value| (name.clone(), value))
        })
        .collect())
}

/// To an MCP client a 401 means "the hub's token is bad" and starts a new OAuth flow,
/// when it's really the user's upstream credential that was refused.
fn credentials_refused(status: reqwest::StatusCode, id: Option<&Value>) -> Option<Response> {
    if status != reqwest::StatusCode::UNAUTHORIZED && status != reqwest::StatusCode::FORBIDDEN {
        return None;
    }
    const MESSAGE: &str = "the server refused the credentials; check this instance's values";
    Some(match id {
        Some(id) => json_error(id, MESSAGE),
        None => (StatusCode::BAD_GATEWAY, Json(MESSAGE)).into_response(),
    })
}

fn response_builder(response: &reqwest::Response) -> axum::http::response::Builder {
    let mut builder = Response::builder().status(response.status().as_u16());
    for name in &FORWARDED_RESPONSE_HEADERS {
        for value in response.headers().get_all(name) {
            builder = builder.header(name, value);
        }
    }
    if is_event_stream(response) {
        let (name, value) = no_buffering();
        builder = builder.header(name, value);
    }
    builder
}

fn is_event_stream(response: &reqwest::Response) -> bool {
    has_content_type(response, "text/event-stream")
}

fn has_content_type(response: &reqwest::Response, media_type: &str) -> bool {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with(media_type))
}

/// Passes the upstream's response on as it arrives, JSON or SSE.
fn respond(
    response: reqwest::Response,
    id: Option<&Value>,
    in_flight: Option<InFlightRequest>,
) -> Result<Response, McpError> {
    if let Some(refused) = credentials_refused(response.status(), id) {
        return Ok(refused);
    }
    let builder = response_builder(&response);
    // The guard lives inside the stream: a streamed response is still in flight.
    let stream = response.bytes_stream().map(move |chunk| {
        let _keep_alive = &in_flight;
        chunk
    });
    builder
        .body(Body::from_stream(stream))
        .map_err(|_| McpError::BadRequest("the upstream sent an invalid response"))
}

/// Filters a `tools/list` response. It must never reach the client uninspected, so
/// anything that can't be read or is too large ends in an error instead.
async fn respond_tool_list(
    target: &Target<'_>,
    response: reqwest::Response,
    id: &Value,
    in_flight: InFlightRequest,
    page: ToolListPage,
) -> Result<Response, McpError> {
    if let Some(refused) = credentials_refused(response.status(), Some(id)) {
        return Ok(refused);
    }
    if !response.status().is_success() {
        return respond(response, Some(id), Some(in_flight));
    }
    let disabled = target.instance.disabled_tools.0.clone();
    let max_bytes = target.state.config.settings.limits.max_line_bytes;
    let builder = response_builder(&response);

    if is_event_stream(&response) {
        let filter = SseToolFilter::new(disabled, max_bytes);
        let body = filtered_events(
            response,
            filter,
            target.state.pool.clone(),
            target.instance.id,
            page,
            in_flight,
        );
        return builder
            .body(Body::from_stream(body))
            .map_err(|_| McpError::BadRequest("the upstream sent an invalid response"));
    }
    if !has_content_type(&response, "application/json") {
        return Ok(json_error(id, "the server sent an unreadable tool list"));
    }

    let Some(body) = read_bounded(response, max_bytes).await else {
        return Ok(json_error(
            id,
            "the server's tool list is too large to check",
        ));
    };
    let Ok(mut message) = serde_json::from_slice::<Value>(&body) else {
        return Ok(json_error(id, "the server sent an unreadable tool list"));
    };
    if let Some(seen) = filter_tool_list(&mut message, &disabled) {
        remember_tools(&target.state.pool, target.instance.id, &seen, page).await;
    }
    builder
        .body(Body::from(message.to_string()))
        .map_err(|_| McpError::BadRequest("the upstream sent an invalid response"))
}

/// Reads a whole body, or `None` if it's over the limit or the read fails.
async fn read_bounded(response: reqwest::Response, max_bytes: usize) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        body.extend_from_slice(&chunk.ok()?);
        if body.len() > max_bytes {
            return None;
        }
    }
    Some(body)
}

/// An SSE stream with every tool list filtered. Too large an event ends the stream with
/// an error, so the client sees a failed request and never an unfiltered list.
fn filtered_events(
    response: reqwest::Response,
    filter: SseToolFilter,
    pool: PgPool,
    instance_id: i64,
    page: ToolListPage,
    in_flight: InFlightRequest,
) -> impl futures::Stream<Item = std::io::Result<Vec<u8>>> {
    let chunks = Box::pin(response.bytes_stream());
    stream::try_unfold(
        (chunks, filter, pool, in_flight),
        move |(mut chunks, mut filter, pool, in_flight)| async move {
            loop {
                let Some(chunk) = chunks.next().await else {
                    return Ok(None);
                };
                let chunk = chunk.map_err(std::io::Error::other)?;
                let output = filter
                    .push(&chunk)
                    .map_err(|_| std::io::Error::other("a tool list event is too large"))?;
                if let Some(seen) = &output.seen {
                    remember_tools(&pool, instance_id, seen, page).await;
                }
                if !output.bytes.is_empty() {
                    return Ok(Some((output.bytes, (chunks, filter, pool, in_flight))));
                }
            }
        },
    )
}
