//! One MCP session bridged to one child process over stdin/stdout.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use futures::StreamExt;
use serde_json::{Value, json};
use tokio::{
    io::AsyncWriteExt,
    process::{ChildStdin, ChildStdout, Command},
    sync::{broadcast, mpsc, oneshot},
};
use tokio_util::{
    codec::{FramedRead, LinesCodec},
    sync::CancellationToken,
};
use zeroize::Zeroizing;

use super::json_rpc_error;
use crate::process::{InstanceOutput, ProcessStop, drain_output};

const STDIN_QUEUE: usize = 64;
const SERVER_MESSAGE_QUEUE: usize = 64;
const INVALID_REQUEST: i64 = -32600;
const REQUEST_TIMED_OUT: i64 = -32001;

type PendingResponses = Arc<Mutex<HashMap<String, oneshot::Sender<Value>>>>;

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("could not start the child process: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("the session's child process has exited")]
    SessionDead,
}

#[derive(Clone, Copy)]
pub struct SessionLimits {
    pub max_line_bytes: usize,
    pub max_pending_requests: usize,
    pub initialize_timeout: Duration,
    pub request_timeout: Duration,
}

pub struct SpawnParams {
    pub session_id: String,
    pub owner_user_id: i64,
    pub instance_id: i64,
    pub command: Command,
    pub scratch_dir: PathBuf,
    pub limits: SessionLimits,
    pub output: InstanceOutput,
    pub secrets: Vec<Zeroizing<String>>,
}

pub struct BridgeSession {
    pub session_id: String,
    pub owner_user_id: i64,
    pub instance_id: i64,
    last_activity: Mutex<Instant>,
    stdin_sender: mpsc::Sender<String>,
    pending: PendingResponses,
    server_messages: broadcast::Sender<Value>,
    stop: ProcessStop,
    limits: SessionLimits,
}

impl BridgeSession {
    /// Starts the child and its tasks. Every task ends when `stop` is cancelled or
    /// the child goes away, and any of those ending stops the whole session.
    pub fn spawn(params: SpawnParams) -> Result<Arc<Self>, BridgeError> {
        let SpawnParams {
            session_id,
            owner_user_id,
            instance_id,
            mut command,
            scratch_dir,
            limits,
            output,
            secrets,
        } = params;

        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");

        let stop = ProcessStop::default();
        let pending = PendingResponses::default();
        let (stdin_sender, stdin_receiver) = mpsc::channel(STDIN_QUEUE);
        let (server_messages, _) = broadcast::channel(SERVER_MESSAGE_QUEUE);

        tokio::spawn(write_stdin(stdin, stdin_receiver, stop.token.clone()));
        tokio::spawn(read_stdout(
            stdout,
            limits.max_line_bytes,
            pending.clone(),
            server_messages.clone(),
            stop.token.clone(),
        ));
        tokio::spawn(drain_output(stderr, output.tail.clone(), Arc::new(secrets)));
        tokio::spawn(output.supervise_child(child, stop.clone(), scratch_dir));

        Ok(Arc::new(Self {
            session_id,
            owner_user_id,
            instance_id,
            last_activity: Mutex::new(Instant::now()),
            stdin_sender,
            pending,
            server_messages,
            stop,
            limits,
        }))
    }

    /// Forwards a request and waits for the child's response. Protocol-level problems
    /// come back as JSON-RPC errors; only a dead child is an `Err`.
    pub async fn request(&self, message: Value) -> Result<Value, BridgeError> {
        self.touch();
        let id = message["id"].clone();
        let method = message["method"].as_str().unwrap_or_default().to_owned();
        let is_initialize = method == "initialize";
        let timeout = if is_initialize {
            self.limits.initialize_timeout
        } else {
            self.limits.request_timeout
        };

        let key = id.to_string();
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self
                .pending
                .lock()
                .expect("no code panics holding this lock");
            if self.is_dead() {
                return Err(BridgeError::SessionDead);
            }
            if pending.contains_key(&key) {
                return Ok(json_rpc_error(&id, INVALID_REQUEST, "duplicate request id"));
            }
            if pending.len() >= self.limits.max_pending_requests {
                return Ok(json_rpc_error(
                    &id,
                    INVALID_REQUEST,
                    "too many pending requests",
                ));
            }
            pending.insert(key.clone(), sender);
        }

        // Runs however this function ends, including the client disconnecting.
        let mut guard = PendingGuard {
            session: self,
            key,
            id: id.clone(),
            finished: false,
            // The protocol forbids cancelling `initialize`.
            send_cancel: !is_initialize,
        };

        let exchange = async {
            self.send_line(&message).await?;
            receiver.await.map_err(|_| BridgeError::SessionDead)
        };
        match tokio::time::timeout(timeout, exchange).await {
            Ok(result) => {
                guard.finished = true;
                self.touch();
                result
            }
            Err(_) => {
                tracing::warn!(session_id = %self.session_id, %method, "request timed out");
                Ok(json_rpc_error(&id, REQUEST_TIMED_OUT, "request timed out"))
            }
        }
    }

    /// Forwards a notification, or a response to a server-initiated request.
    pub async fn notify(&self, message: &Value) -> Result<(), BridgeError> {
        self.touch();
        tokio::time::timeout(self.limits.request_timeout, self.send_line(message))
            .await
            .map_err(|_| BridgeError::SessionDead)?
    }

    async fn send_line(&self, message: &Value) -> Result<(), BridgeError> {
        self.stdin_sender
            .send(message.to_string())
            .await
            .map_err(|_| BridgeError::SessionDead)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.server_messages.subscribe()
    }

    /// Resolves when the session has ended, so that open streams can finish.
    pub async fn stopped(&self) {
        self.stop.token.cancelled().await;
    }

    pub fn stop(&self) {
        self.stop.request();
    }

    pub fn is_dead(&self) -> bool {
        self.stop.token.is_cancelled()
    }

    pub fn last_activity(&self) -> Instant {
        *self
            .last_activity
            .lock()
            .expect("no code panics holding this lock")
    }

    pub fn has_requests_in_flight(&self) -> bool {
        !self
            .pending
            .lock()
            .expect("no code panics holding this lock")
            .is_empty()
    }

    fn touch(&self) {
        *self
            .last_activity
            .lock()
            .expect("no code panics holding this lock") = Instant::now();
    }
}

impl Drop for BridgeSession {
    fn drop(&mut self) {
        self.stop.request();
    }
}

struct PendingGuard<'a> {
    session: &'a BridgeSession,
    key: String,
    id: Value,
    finished: bool,
    send_cancel: bool,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.session
            .pending
            .lock()
            .expect("no code panics holding this lock")
            .remove(&self.key);
        if !self.finished && self.send_cancel {
            let cancel = json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": { "requestId": self.id, "reason": "client went away or the request timed out" },
            });
            // Best effort: with a full queue the child is already not keeping up.
            let _ = self.session.stdin_sender.try_send(cancel.to_string());
        }
    }
}

/// The only writer to the child's stdin.
async fn write_stdin(
    mut stdin: ChildStdin,
    mut messages: mpsc::Receiver<String>,
    stop: CancellationToken,
) {
    loop {
        let message = tokio::select! {
            () = stop.cancelled() => return,
            message = messages.recv() => message,
        };
        let Some(mut line) = message else { return };
        line.push('\n');
        let written = async {
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await
        };
        let result = tokio::select! {
            () = stop.cancelled() => return,
            result = written => result,
        };
        if result.is_err() {
            stop.cancel();
            return;
        }
    }
}

async fn read_stdout(
    stdout: ChildStdout,
    max_line_bytes: usize,
    pending: PendingResponses,
    server_messages: broadcast::Sender<Value>,
    stop: CancellationToken,
) {
    let mut lines = FramedRead::new(stdout, LinesCodec::new_with_max_length(max_line_bytes));
    loop {
        let next = tokio::select! {
            () = stop.cancelled() => break,
            next = lines.next() => next,
        };
        let line = match next {
            Some(Ok(line)) => line,
            // EOF, an over-long line, or invalid UTF-8: the stream can't be trusted now.
            Some(Err(_)) | None => break,
        };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            tracing::warn!("skipping a non-JSON line from a child's stdout");
            continue;
        };
        route_message(message, &pending, &server_messages);
    }

    stop.cancel();
    // Hold the lock while clearing: `request` checks `stop` under it, so nothing can
    // be added afterwards and wait forever. Dropped senders wake every waiter.
    pending
        .lock()
        .expect("no code panics holding this lock")
        .clear();
}

fn route_message(
    message: Value,
    pending: &PendingResponses,
    server_messages: &broadcast::Sender<Value>,
) {
    let is_response = message.get("id").is_some() && message.get("method").is_none();
    if is_response {
        let key = message["id"].to_string();
        let waiter = pending
            .lock()
            .expect("no code panics holding this lock")
            .remove(&key);
        if let Some(waiter) = waiter {
            let _ = waiter.send(message);
        }
    } else {
        // An error only means nobody is listening right now.
        let _ = server_messages.send(message);
    }
}
