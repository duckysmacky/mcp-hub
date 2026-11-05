//! `/mcp/{slug}` for `stdio` servers: one child process per MCP session.

use std::{convert::Infallible, sync::Arc};

use axum::{
    Json,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures::stream;
use serde_json::Value;
use tokio::sync::broadcast::{self, error::RecvError};

use super::{
    McpError, Message, SESSION_HEADER, Target, json_error, no_buffering, remember_tools,
    required_session_id, start_failure, tool_list_page,
};
use crate::{
    bridge::{
        stdio::{BridgeError, BridgeSession},
        tools::filter_tool_list,
    },
    process::{declared_process_env, secret_values},
    supervisor::StartRequest,
};

pub async fn post(
    target: &Target<'_>,
    headers: &HeaderMap,
    kind: Message,
    message: Value,
) -> Result<Response, McpError> {
    if matches!(kind, Message::Initialize) {
        return initialize(target, message).await;
    }
    let session = find_session(target, headers)?;
    let tool_list = tool_list_page(&message);
    match kind {
        Message::Request => match session.request(message).await {
            Ok(mut response) => {
                if let Some(page) = tool_list {
                    let disabled = &target.instance.disabled_tools.0;
                    if let Some(seen) = filter_tool_list(&mut response, disabled) {
                        remember_tools(&target.state.pool, target.instance.id, &seen, page).await;
                    }
                }
                Ok(Json(response).into_response())
            }
            Err(_) => Err(McpError::NotFound),
        },
        _ => match session.notify(&message).await {
            Ok(()) => Ok(StatusCode::ACCEPTED.into_response()),
            Err(_) => Err(McpError::NotFound),
        },
    }
}

/// Starts a child for a new session and forwards `initialize` to it.
async fn initialize(target: &Target<'_>, message: Value) -> Result<Response, McpError> {
    let id = message["id"].clone();
    let values = target.decrypt_env().await?;
    if let Err(reason) = target.check_values(&values).await {
        return Ok(json_error(&id, &reason));
    }
    let server = match target.resolved_server().await {
        Ok(server) => server,
        Err(reason) => return Ok(json_error(&id, &reason)),
    };
    let secrets = secret_values(&target.template, &values);

    let started = target
        .state
        .supervisor
        .start_session(StartRequest {
            user_id: target.auth.user_id,
            instance_id: target.instance.id,
            runtime: target.template.runtime,
            server: &server,
            args: &target.template.bin_args,
            env: declared_process_env(&target.template, values),
            secrets,
        })
        .await;
    let session = match started {
        Ok(session) => session,
        Err(error) => return start_failure(&id, error),
    };
    target.audit("process_spawned", None).await;

    let response = match session.request(message).await {
        Ok(response) => response,
        Err(BridgeError::SessionDead | BridgeError::Spawn(_)) => {
            return Ok(json_error(
                &id,
                "the server exited during startup; check the instance status",
            ));
        }
    };
    // A failed initialize leaves no session behind for the client to use.
    if response.get("result").is_none() {
        target.state.supervisor.end_session(
            &session.session_id,
            target.auth.user_id,
            target.instance.id,
        );
        return Ok(Json(response).into_response());
    }

    let session_header = HeaderValue::from_str(&session.session_id)
        .expect("session ids are URL-safe base64, which is a valid header value");
    Ok(([(SESSION_HEADER, session_header)], Json(response)).into_response())
}

fn find_session(target: &Target<'_>, headers: &HeaderMap) -> Result<Arc<BridgeSession>, McpError> {
    let session_id = required_session_id(headers)?;
    target
        .state
        .supervisor
        .session(session_id, target.auth.user_id, target.instance.id)
        .ok_or(McpError::NotFound)
}

/// Streams server-initiated messages until the session ends or the client leaves.
pub fn get(target: &Target<'_>, headers: &HeaderMap) -> Result<Response, McpError> {
    let accepts_events = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/event-stream"));
    if !accepts_events {
        return Err(McpError::NotAcceptable);
    }
    let session = find_session(target, headers)?;
    let messages = session.subscribe();
    let events = stream::unfold((session, messages), |(session, messages)| {
        next_event(session, messages)
    });
    Ok((
        [no_buffering()],
        Sse::new(events).keep_alive(KeepAlive::default()),
    )
        .into_response())
}

async fn next_event(
    session: Arc<BridgeSession>,
    mut messages: broadcast::Receiver<Value>,
) -> Option<(
    Result<Event, Infallible>,
    (Arc<BridgeSession>, broadcast::Receiver<Value>),
)> {
    loop {
        let message = tokio::select! {
            () = session.stopped() => return None,
            message = messages.recv() => message,
        };
        match message {
            Ok(message) => {
                let event = Event::default().event("message").data(message.to_string());
                return Some((Ok(event), (session, messages)));
            }
            Err(RecvError::Lagged(skipped)) => {
                tracing::warn!(skipped, "an SSE client fell behind and missed messages");
            }
            Err(RecvError::Closed) => return None,
        }
    }
}

pub fn delete(target: &Target<'_>, headers: &HeaderMap) -> Result<Response, McpError> {
    let session_id = required_session_id(headers)?;
    let ended =
        target
            .state
            .supervisor
            .end_session(session_id, target.auth.user_id, target.instance.id);
    if ended {
        Ok(StatusCode::NO_CONTENT.into_response())
    } else {
        Err(McpError::NotFound)
    }
}
