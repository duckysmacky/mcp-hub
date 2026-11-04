//! One long-running upstream HTTP MCP process per instance. It holds no credentials in
//! its env: the hub injects them as request headers on every proxied request.

use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    process::Stdio,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::{
    net::TcpStream,
    process::Command,
    sync::watch,
    time::{sleep, timeout_at},
};
use zeroize::Zeroizing;

use crate::process::{InstanceOutput, ProcessStop, drain_output};

const READINESS_POLL_INTERVAL: Duration = Duration::from_millis(100);
const MAX_RECORDED_SESSIONS: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Readiness {
    Starting,
    Ready,
    Failed,
}

pub struct UpstreamParams {
    pub owner_user_id: i64,
    pub instance_id: i64,
    pub port: u16,
    pub path: String,
    pub command: Command,
    pub scratch_dir: PathBuf,
    pub output: InstanceOutput,
    pub secrets: Vec<Zeroizing<String>>,
    pub start_timeout: Duration,
}

/// The upstream session ids this hub has seen. Only these may be used by clients.
// ponytail: a capped FIFO, so a long-lived process can't grow it forever. A session
// pushed out gets 404 and re-initializes; track upstream expiry if that ever bites.
#[derive(Default)]
struct RecordedSessions {
    ids: HashSet<String>,
    order: VecDeque<String>,
}

pub struct UpstreamProcess {
    pub owner_user_id: i64,
    pub instance_id: i64,
    pub port: u16,
    path: String,
    started_at: Instant,
    last_activity: Mutex<Instant>,
    in_flight: Arc<AtomicUsize>,
    sessions: Mutex<RecordedSessions>,
    readiness: watch::Receiver<Readiness>,
    stop: ProcessStop,
}

impl UpstreamProcess {
    /// Starts the process and its tasks. Every task ends when `stop` fires or the
    /// process goes away.
    pub fn spawn(params: UpstreamParams) -> std::io::Result<Arc<Self>> {
        let UpstreamParams {
            owner_user_id,
            instance_id,
            port,
            path,
            mut command,
            scratch_dir,
            output,
            secrets,
            start_timeout,
        } = params;

        command.stdin(Stdio::piped());
        let mut child = command.spawn()?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");

        let stop = ProcessStop::default();
        let secrets = Arc::new(secrets);
        tokio::spawn(drain_output(stdout, output.tail.clone(), secrets.clone()));
        tokio::spawn(drain_output(stderr, output.tail.clone(), secrets));
        tokio::spawn(output.supervise_child(child, stop.clone(), scratch_dir));
        // Some servers exit when stdin closes, so it stays open for the process's life.
        let stop_token = stop.token.clone();
        tokio::spawn(async move {
            stop_token.cancelled().await;
            drop(stdin);
        });

        let (readiness_sender, readiness) = watch::channel(Readiness::Starting);
        tokio::spawn(watch_readiness(
            port,
            start_timeout,
            stop.clone(),
            readiness_sender,
        ));

        let now = Instant::now();
        Ok(Arc::new(Self {
            owner_user_id,
            instance_id,
            port,
            path,
            started_at: now,
            last_activity: Mutex::new(now),
            in_flight: Arc::default(),
            sessions: Mutex::default(),
            readiness,
            stop,
        }))
    }

    /// Waits until the process accepts connections. False if it never will.
    pub async fn wait_ready(&self) -> bool {
        let mut readiness = self.readiness.clone();
        let result = readiness
            .wait_for(|state| *state != Readiness::Starting)
            .await
            .map(|state| *state == Readiness::Ready);
        result.unwrap_or(false)
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}{}", self.port, self.path)
    }

    pub fn is_dead(&self) -> bool {
        self.stop.token.is_cancelled()
    }

    pub fn stop(&self) {
        self.stop.request();
    }

    pub fn was_stopped_by_hub(&self) -> bool {
        self.stop.requested_by_hub()
    }

    pub fn lived_for(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub fn touch(&self) {
        *self
            .last_activity
            .lock()
            .expect("no code panics holding this lock") = Instant::now();
    }

    pub fn last_activity(&self) -> Instant {
        *self
            .last_activity
            .lock()
            .expect("no code panics holding this lock")
    }

    /// Marks a request as running until the guard drops, so the reaper leaves it alone.
    pub fn begin_request(&self) -> InFlightRequest {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        InFlightRequest(self.in_flight.clone())
    }

    pub fn has_requests_in_flight(&self) -> bool {
        self.in_flight.load(Ordering::SeqCst) > 0
    }

    pub fn record_session(&self, session_id: &str) {
        let mut sessions = self
            .sessions
            .lock()
            .expect("no code panics holding this lock");
        if sessions.ids.insert(session_id.to_owned()) {
            sessions.order.push_back(session_id.to_owned());
        }
        while sessions.order.len() > MAX_RECORDED_SESSIONS {
            if let Some(oldest) = sessions.order.pop_front() {
                sessions.ids.remove(&oldest);
            }
        }
    }

    pub fn has_session(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .expect("no code panics holding this lock")
            .ids
            .contains(session_id)
    }

    pub fn forget_session(&self, session_id: &str) {
        let mut sessions = self
            .sessions
            .lock()
            .expect("no code panics holding this lock");
        if sessions.ids.remove(session_id) {
            sessions.order.retain(|id| id != session_id);
        }
    }
}

impl Drop for UpstreamProcess {
    fn drop(&mut self) {
        self.stop.request();
    }
}

/// Keeps a process marked as busy for as long as a request or its response body lives.
pub struct InFlightRequest(Arc<AtomicUsize>);

impl Drop for InFlightRequest {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn watch_readiness(
    port: u16,
    start_timeout: Duration,
    stop: ProcessStop,
    readiness: watch::Sender<Readiness>,
) {
    let deadline = tokio::time::Instant::now() + start_timeout;
    let poll = async {
        loop {
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                return;
            }
            sleep(READINESS_POLL_INTERVAL).await;
        }
    };
    let state = tokio::select! {
        result = timeout_at(deadline, poll) => match result {
            Ok(()) => Readiness::Ready,
            Err(_) => {
                tracing::warn!(port, "upstream process never started listening");
                // Not a hub stop: this counts as a crash for the restart backoff.
                stop.token.cancel();
                Readiness::Failed
            }
        },
        () = stop.token.cancelled() => Readiness::Failed,
    };
    let _ = readiness.send(state);
}
