//! Owns every running child: registries, limits, eviction, the idle reaper, shutdown.

use std::{
    collections::{HashMap, VecDeque},
    net::TcpListener,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::{
    bridge::{
        http::{UpstreamParams, UpstreamProcess},
        stdio::{BridgeError, BridgeSession, SessionLimits, SpawnParams},
    },
    catalog::{HttpSettings, RuntimeKind, install::ResolvedServer},
    config::{Config, RuntimeSettings},
    process::{ChildSpec, EnvValue, InstanceOutput, build_child_command, create_scratch_dir},
};

const REAPER_INTERVAL: Duration = Duration::from_secs(30);
const SPAWN_WINDOW: Duration = Duration::from_secs(60);
const SHUTDOWN_GRACE: Duration = Duration::from_millis(500);
const MAX_CRASH_BACKOFF_SECS: u64 = 60;
/// An upstream that lived this long wasn't crash-looping.
const STABLE_UPTIME: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("the hub is running its maximum number of processes")]
    TooManyProcesses,
    #[error("too many processes started recently")]
    SpawnRateLimited,
    #[error("the upstream process keeps crashing; retry in {retry_after_secs} seconds")]
    CrashBackoff { retry_after_secs: u64 },
    #[error("no free port in the configured range")]
    NoFreePort,
    #[error("could not create a scratch directory: {0}")]
    Scratch(std::io::Error),
    #[error(transparent)]
    Bridge(#[from] BridgeError),
}

/// What's needed to start a stdio session. `env` must come from `declared_process_env`.
pub struct StartRequest<'a> {
    pub user_id: i64,
    pub instance_id: i64,
    pub runtime: RuntimeKind,
    pub server: &'a ResolvedServer,
    pub args: &'a [String],
    pub env: Vec<EnvValue>,
    /// The instance's secret values, so that output can be redacted against them.
    pub secrets: Vec<Zeroizing<String>>,
}

/// What's needed to start an http upstream. `env` must come from `declared_process_env`.
pub struct StartUpstream<'a> {
    pub user_id: i64,
    pub instance_id: i64,
    pub runtime: RuntimeKind,
    pub server: &'a ResolvedServer,
    pub args: &'a [String],
    pub http: &'a HttpSettings,
    pub env: Vec<EnvValue>,
    /// The instance's secret values, so that output can be redacted against them.
    pub secrets: Vec<Zeroizing<String>>,
}

#[derive(Clone, Debug, Default)]
pub struct InstanceStatus {
    pub live_processes: usize,
    pub last_exit: Option<String>,
    pub output_tail: String,
}

#[derive(Clone, Copy)]
struct CrashRecord {
    consecutive: u32,
    retry_at: Instant,
}

#[derive(Default)]
struct Registry {
    sessions: HashMap<String, Arc<BridgeSession>>,
    upstreams: HashMap<i64, Arc<UpstreamProcess>>,
    recent_spawns: HashMap<i64, VecDeque<Instant>>,
    outputs: HashMap<i64, InstanceOutput>,
    crashes: HashMap<i64, CrashRecord>,
}

impl Registry {
    fn remove_dead(&mut self) {
        self.sessions.retain(|_, session| !session.is_dead());

        let crashed: Vec<Arc<UpstreamProcess>> = self
            .upstreams
            .values()
            .filter(|upstream| upstream.is_dead() && !upstream.was_stopped_by_hub())
            .cloned()
            .collect();
        for upstream in crashed {
            self.record_crash(&upstream);
        }
        self.upstreams.retain(|_, upstream| !upstream.is_dead());
    }

    fn record_crash(&mut self, upstream: &UpstreamProcess) {
        let instance_id = upstream.instance_id;
        if upstream.lived_for() >= STABLE_UPTIME {
            self.crashes.remove(&instance_id);
            return;
        }
        let consecutive = self
            .crashes
            .get(&instance_id)
            .map_or(1, |crash| crash.consecutive + 1);
        let delay = 2u64
            .saturating_pow(consecutive - 1)
            .min(MAX_CRASH_BACKOFF_SECS);
        self.crashes.insert(
            instance_id,
            CrashRecord {
                consecutive,
                retry_at: Instant::now() + Duration::from_secs(delay),
            },
        );
    }

    fn process_count(&self) -> usize {
        self.sessions.len() + self.upstreams.len()
    }

    fn user_process_count(&self, user_id: i64) -> usize {
        let sessions = self
            .sessions
            .values()
            .filter(|session| session.owner_user_id == user_id)
            .count();
        let upstreams = self
            .upstreams
            .values()
            .filter(|upstream| upstream.owner_user_id == user_id)
            .count();
        sessions + upstreams
    }

    fn instance_session_count(&self, instance_id: i64) -> usize {
        self.sessions
            .values()
            .filter(|session| session.instance_id == instance_id)
            .count()
    }

    fn evict_session(&mut self, session_id: &str) {
        if let Some(session) = self.sessions.remove(session_id) {
            tracing::info!(session_id, "evicting a session at its limit");
            session.stop();
        }
    }

    fn evict_oldest_instance_session(&mut self, instance_id: i64) {
        let oldest = self
            .sessions
            .values()
            .filter(|session| session.instance_id == instance_id)
            .min_by_key(|session| session.last_activity())
            .map(|session| session.session_id.clone());
        if let Some(session_id) = oldest {
            self.evict_session(&session_id);
        }
    }

    /// Evicts the user's least recently used session or upstream process.
    fn evict_oldest_user_process(&mut self, user_id: i64) {
        let oldest_session = self
            .sessions
            .values()
            .filter(|session| session.owner_user_id == user_id)
            .min_by_key(|session| session.last_activity())
            .map(|session| (session.last_activity(), session.session_id.clone()));
        let oldest_upstream = self
            .upstreams
            .values()
            .filter(|upstream| upstream.owner_user_id == user_id)
            .min_by_key(|upstream| upstream.last_activity())
            .map(|upstream| (upstream.last_activity(), upstream.instance_id));

        match (oldest_session, oldest_upstream) {
            (Some((session_time, _)), Some((upstream_time, instance_id)))
                if upstream_time < session_time =>
            {
                self.evict_upstream(instance_id);
            }
            (Some((_, session_id)), _) => self.evict_session(&session_id),
            (None, Some((_, instance_id))) => self.evict_upstream(instance_id),
            (None, None) => {}
        }
    }

    fn evict_upstream(&mut self, instance_id: i64) {
        if let Some(upstream) = self.upstreams.remove(&instance_id) {
            tracing::info!(instance_id, "evicting an upstream process at its limit");
            upstream.stop();
        }
    }

    fn output_for(&mut self, instance_id: i64, capacity: usize) -> InstanceOutput {
        self.outputs
            .entry(instance_id)
            .or_insert_with(|| InstanceOutput::new(capacity))
            .clone()
    }
}

pub struct Supervisor {
    config: Arc<Config>,
    registry: Mutex<Registry>,
    // ponytail: serializes every spawn across all users. A spawn takes milliseconds,
    // so this is fine at this scale; use per-instance locks if startups ever queue up.
    spawn_lock: tokio::sync::Mutex<()>,
    shutting_down: CancellationToken,
    upstream_client: reqwest::Client,
}

impl Supervisor {
    pub fn new(config: Arc<Config>) -> Arc<Self> {
        let upstream_client = reqwest::Client::builder()
            // A redirect must never carry injected credentials anywhere else.
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .expect("a client without TLS or custom settings always builds");
        Arc::new(Self {
            config,
            registry: Mutex::default(),
            spawn_lock: tokio::sync::Mutex::new(()),
            shutting_down: CancellationToken::new(),
            upstream_client,
        })
    }

    fn registry(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .expect("no code panics holding this lock")
    }

    pub fn upstream_client(&self) -> &reqwest::Client {
        &self.upstream_client
    }

    /// Starts a child for a new stdio session, making room within the limits first.
    pub async fn start_session(
        &self,
        request: StartRequest<'_>,
    ) -> Result<Arc<BridgeSession>, SupervisorError> {
        let limits = &self.config.settings.limits;
        let _spawn_guard = self.spawn_lock.lock().await;
        if self.shutting_down.is_cancelled() {
            return Err(SupervisorError::TooManyProcesses);
        }
        let output = self.make_room(request.user_id, request.instance_id, true)?;

        let session_id = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 16]>());
        let scratch_dir = create_scratch_dir(&self.config.settings.paths.scratch, &session_id)
            .await
            .map_err(SupervisorError::Scratch)?;
        let command = build_child_command(&ChildSpec {
            runtime_binary: runtime_binary(&self.config.settings.runtimes, request.runtime),
            script: &request.server.script,
            python_path: request.server.python_path.as_deref(),
            args: request.args,
            env: &request.env,
            scratch_dir: &scratch_dir,
        });

        let spawned = BridgeSession::spawn(SpawnParams {
            session_id: session_id.clone(),
            owner_user_id: request.user_id,
            instance_id: request.instance_id,
            command,
            scratch_dir: scratch_dir.clone(),
            limits: SessionLimits {
                max_line_bytes: limits.max_line_bytes,
                max_pending_requests: limits.max_pending_requests_per_session,
                initialize_timeout: Duration::from_secs(limits.initialize_timeout_secs),
                request_timeout: Duration::from_secs(limits.request_timeout_secs),
            },
            output,
            secrets: request.secrets,
        });
        let session = match spawned {
            Ok(session) => session,
            Err(error) => {
                let _ = tokio::fs::remove_dir_all(&scratch_dir).await;
                return Err(error.into());
            }
        };

        self.registry().sessions.insert(session_id, session.clone());
        Ok(session)
    }

    /// Returns the instance's running upstream, starting it if there is none. It may
    /// still be starting: wait on `UpstreamProcess::wait_ready`, outside any lock.
    pub async fn ensure_upstream(
        &self,
        request: StartUpstream<'_>,
    ) -> Result<Arc<UpstreamProcess>, SupervisorError> {
        let limits = &self.config.settings.limits;
        let _spawn_guard = self.spawn_lock.lock().await;
        if self.shutting_down.is_cancelled() {
            return Err(SupervisorError::TooManyProcesses);
        }
        if let Some(upstream) = self.upstream(request.instance_id) {
            return Ok(upstream);
        }
        self.check_crash_backoff(request.instance_id)?;
        let port = self.allocate_port()?;
        let output = self.make_room(request.user_id, request.instance_id, false)?;

        let mut env = request.env;
        env.push((
            request.http.port_env.clone(),
            Zeroizing::new(port.to_string()),
        ));
        let directory_name = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 16]>());
        let scratch_dir = create_scratch_dir(&self.config.settings.paths.scratch, &directory_name)
            .await
            .map_err(SupervisorError::Scratch)?;
        let command = build_child_command(&ChildSpec {
            runtime_binary: runtime_binary(&self.config.settings.runtimes, request.runtime),
            script: &request.server.script,
            python_path: request.server.python_path.as_deref(),
            args: request.args,
            env: &env,
            scratch_dir: &scratch_dir,
        });

        let spawned = UpstreamProcess::spawn(UpstreamParams {
            owner_user_id: request.user_id,
            instance_id: request.instance_id,
            port,
            path: request.http.path.clone(),
            command,
            scratch_dir: scratch_dir.clone(),
            output,
            secrets: request.secrets,
            start_timeout: Duration::from_secs(limits.upstream_start_timeout_secs),
        });
        let upstream = match spawned {
            Ok(upstream) => upstream,
            Err(error) => {
                let _ = tokio::fs::remove_dir_all(&scratch_dir).await;
                return Err(SupervisorError::Bridge(BridgeError::Spawn(error)));
            }
        };

        self.registry()
            .upstreams
            .insert(request.instance_id, upstream.clone());
        Ok(upstream)
    }

    /// The instance's live upstream process, if it has one.
    pub fn upstream(&self, instance_id: i64) -> Option<Arc<UpstreamProcess>> {
        let mut registry = self.registry();
        registry.remove_dead();
        registry.upstreams.get(&instance_id).cloned()
    }

    fn check_crash_backoff(&self, instance_id: i64) -> Result<(), SupervisorError> {
        let registry = self.registry();
        let Some(crash) = registry.crashes.get(&instance_id) else {
            return Ok(());
        };
        let remaining = crash.retry_at.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        Err(SupervisorError::CrashBackoff {
            retry_after_secs: remaining.as_secs() + 1,
        })
    }

    /// Finds a port that no upstream holds and that nothing else is listening on.
    fn allocate_port(&self) -> Result<u16, SupervisorError> {
        let [first, last] = self.config.settings.http_mode.port_range;
        let registry = self.registry();
        (first..=last)
            .find(|&port| {
                !registry
                    .upstreams
                    .values()
                    .any(|upstream| upstream.port == port)
                    && TcpListener::bind(("127.0.0.1", port)).is_ok()
            })
            .ok_or(SupervisorError::NoFreePort)
    }

    /// Applies the limits for one more process, and returns the instance's output buffer.
    fn make_room(
        &self,
        user_id: i64,
        instance_id: i64,
        is_session: bool,
    ) -> Result<InstanceOutput, SupervisorError> {
        let limits = &self.config.settings.limits;
        let mut registry = self.registry();
        registry.remove_dead();

        let spawns = registry.recent_spawns.entry(user_id).or_default();
        while spawns
            .front()
            .is_some_and(|spawned| spawned.elapsed() >= SPAWN_WINDOW)
        {
            spawns.pop_front();
        }
        if spawns.len() >= limits.max_spawns_per_user_per_minute {
            return Err(SupervisorError::SpawnRateLimited);
        }
        spawns.push_back(Instant::now());

        if is_session {
            while registry.instance_session_count(instance_id) >= limits.max_sessions_per_instance {
                registry.evict_oldest_instance_session(instance_id);
            }
        }
        while registry.user_process_count(user_id) >= limits.max_processes_per_user {
            registry.evict_oldest_user_process(user_id);
        }
        if registry.process_count() >= limits.max_processes_total {
            return Err(SupervisorError::TooManyProcesses);
        }
        Ok(registry.output_for(instance_id, limits.output_tail_bytes))
    }

    /// Finds a live session. All three parts must match: a session id alone grants nothing.
    pub fn session(
        &self,
        session_id: &str,
        user_id: i64,
        instance_id: i64,
    ) -> Option<Arc<BridgeSession>> {
        let mut registry = self.registry();
        let session = registry.sessions.get(session_id)?;
        if session.is_dead() {
            registry.sessions.remove(session_id);
            return None;
        }
        (session.owner_user_id == user_id && session.instance_id == instance_id)
            .then(|| session.clone())
    }

    /// Ends one session. Returns false unless it belongs to this user and instance.
    pub fn end_session(&self, session_id: &str, user_id: i64, instance_id: i64) -> bool {
        let Some(session) = self.session(session_id, user_id, instance_id) else {
            return false;
        };
        self.registry().sessions.remove(session_id);
        session.stop();
        true
    }

    /// Kills every process of an instance. Clients get 404 and re-initialize.
    pub fn stop_instance(&self, instance_id: i64) {
        let mut registry = self.registry();
        registry.sessions.retain(|_, session| {
            if session.instance_id == instance_id {
                session.stop();
            }
            session.instance_id != instance_id
        });
        if let Some(upstream) = registry.upstreams.remove(&instance_id) {
            upstream.stop();
        }
        // The operator or user acted, so the next start gets a clean slate.
        registry.crashes.remove(&instance_id);
    }

    /// Kills an instance's processes and drops what's kept about it, once it's deleted.
    pub fn forget_instance(&self, instance_id: i64) {
        self.stop_instance(instance_id);
        self.registry().outputs.remove(&instance_id);
    }

    pub fn instance_status(&self, instance_id: i64) -> InstanceStatus {
        let mut registry = self.registry();
        registry.remove_dead();
        let live_processes = registry.instance_session_count(instance_id)
            + usize::from(registry.upstreams.contains_key(&instance_id));
        let output = registry.outputs.get(&instance_id);
        InstanceStatus {
            live_processes,
            last_exit: output.and_then(InstanceOutput::last_exit),
            output_tail: output.map_or_else(String::new, |output| output.tail.contents()),
        }
    }

    /// Stops processes idle past the limit. Never one with a request still running.
    pub fn reap_idle(&self) {
        let idle_timeout = Duration::from_secs(self.config.settings.limits.idle_timeout_secs);
        let mut registry = self.registry();
        registry.remove_dead();
        registry.sessions.retain(|_, session| {
            let idle = session.last_activity().elapsed() >= idle_timeout
                && !session.has_requests_in_flight();
            if idle {
                tracing::info!(session_id = %session.session_id, "reaping an idle session");
                session.stop();
            }
            !idle
        });
        registry.upstreams.retain(|_, upstream| {
            let idle = upstream.last_activity().elapsed() >= idle_timeout
                && !upstream.has_requests_in_flight();
            if idle {
                tracing::info!(
                    instance_id = upstream.instance_id,
                    "reaping an idle upstream"
                );
                upstream.stop();
            }
            !idle
        });
    }

    /// Runs the reaper until shutdown.
    pub fn spawn_reaper(self: &Arc<Self>) {
        let supervisor = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(REAPER_INTERVAL);
            loop {
                tokio::select! {
                    () = supervisor.shutting_down.cancelled() => return,
                    _ = interval.tick() => supervisor.reap_idle(),
                }
            }
        });
    }

    /// Kills everything and refuses new processes. Ends open SSE streams too.
    pub async fn shutdown(&self) {
        // Waiting for the lock means no spawn is halfway through and missed below.
        let _spawn_guard = self.spawn_lock.lock().await;
        self.shutting_down.cancel();
        {
            let mut registry = self.registry();
            for (_, session) in registry.sessions.drain() {
                session.stop();
            }
            for (_, upstream) in registry.upstreams.drain() {
                upstream.stop();
            }
        }
        tokio::time::sleep(SHUTDOWN_GRACE).await;
    }
}

/// `None` for Python: an installed entry point runs itself through its shebang.
fn runtime_binary(runtimes: &RuntimeSettings, runtime: RuntimeKind) -> Option<&Path> {
    match runtime {
        RuntimeKind::Bun => Some(&runtimes.bun),
        RuntimeKind::Node => Some(&runtimes.node),
        RuntimeKind::Uv | RuntimeKind::Pip => None,
    }
}
