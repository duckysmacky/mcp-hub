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
    McpError, Message, SESSION_HEADER, Target, json_error, no_buffering, required_session_id,
    start_failure,
};
use crate::{
    bridge::stdio::{BridgeError, BridgeSession},
    process::declared_process_env,
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
    match kind {
        Message::Request => match session.request(message).await {
            Ok(response) => Ok(Json(response).into_response()),
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
    if let Err(reason) = target.check_urls(&values).await {
        return Ok(json_error(&id, &reason));
    }

    let started = target
        .state
        .supervisor
        .start_session(StartRequest {
            user_id: target.auth.user_id,
            instance_id: target.instance.id,
            runtime: target.instance.effective_runtime(target.template),
            script: target.script()?,
            env: declared_process_env(target.template, values),
        })
        .await;
    let session = match started {
        Ok(session) => session,
        Err(error) => return start_failure(&id, error),
    };

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
