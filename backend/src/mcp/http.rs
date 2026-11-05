//! `/mcp/{slug}` for `http` servers: a reverse proxy that injects the user's
//! credentials as headers. The client's own `Authorization` never reaches the upstream.

use std::{sync::Arc, time::Duration};

use axum::{
    Json,
    body::Body,
    http::{HeaderMap, HeaderName, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use serde_json::Value;
use zeroize::Zeroizing;

use super::{
    McpError, Message, SESSION_HEADER, Target, json_error, no_buffering, required_session_id,
    start_failure,
};
use crate::{
    bridge::http::{InFlightRequest, UpstreamProcess},
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
    respond(response, id.as_ref(), Some(in_flight))
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

/// Passes the upstream's response on as it arrives, JSON or SSE.
fn respond(
    response: reqwest::Response,
    id: Option<&Value>,
    in_flight: Option<InFlightRequest>,
) -> Result<Response, McpError> {
    // To an MCP client a 401 means "the hub's token is bad" and starts a new OAuth flow,
    // when it's really the user's upstream credential that was refused.
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        const MESSAGE: &str = "the server refused the credentials; check this instance's values";
        return Ok(match id {
            Some(id) => json_error(id, MESSAGE),
            None => (StatusCode::BAD_GATEWAY, Json(MESSAGE)).into_response(),
        });
    }

    let mut builder = Response::builder().status(status.as_u16());
    for name in &FORWARDED_RESPONSE_HEADERS {
        for value in response.headers().get_all(name) {
            builder = builder.header(name, value);
        }
    }
    let is_event_stream = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"));
    if is_event_stream {
        let (name, value) = no_buffering();
        builder = builder.header(name, value);
    }

    // The guard lives inside the stream: a streamed response is still in flight.
    let stream = response.bytes_stream().map(move |chunk| {
        let _keep_alive = &in_flight;
        chunk
    });
    builder
        .body(Body::from_stream(stream))
        .map_err(|_| McpError::BadRequest("the upstream sent an invalid response"))
}
