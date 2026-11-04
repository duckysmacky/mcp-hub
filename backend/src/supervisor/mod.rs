//! Owns every running child: registries, limits, eviction, the idle reaper, shutdown.

use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::{
    bridge::stdio::{BridgeError, BridgeSession, SessionLimits, SpawnParams},
    catalog::RuntimeKind,
    config::Config,
    process::{ChildSpec, EnvValue, InstanceOutput, build_child_command, create_scratch_dir},
};

const REAPER_INTERVAL: Duration = Duration::from_secs(30);
const SPAWN_WINDOW: Duration = Duration::from_secs(60);
const SHUTDOWN_GRACE: Duration = Duration::from_millis(500);

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("the hub is running its maximum number of processes")]
    TooManyProcesses,
    #[error("too many processes started recently")]
    SpawnRateLimited,
    #[error("could not create a scratch directory: {0}")]
    Scratch(std::io::Error),
    #[error(transparent)]
    Bridge(#[from] BridgeError),
}

/// What's needed to start a process. `env` must come from `declared_process_env`.
pub struct StartRequest<'a> {
    pub user_id: i64,
    pub instance_id: i64,
    pub runtime: RuntimeKind,
    pub script: &'a Path,
    pub env: Vec<EnvValue>,
}

#[derive(Clone, Debug, Default)]
pub struct InstanceStatus {
    pub live_processes: usize,
    pub last_exit: Option<String>,
    pub output_tail: String,
}

#[derive(Default)]
struct Registry {
    sessions: HashMap<String, Arc<BridgeSession>>,
    recent_spawns: HashMap<i64, VecDeque<Instant>>,
    outputs: HashMap<i64, InstanceOutput>,
}

impl Registry {
    fn remove_dead_sessions(&mut self) {
        self.sessions.retain(|_, session| !session.is_dead());
    }

    fn evict_least_recent(&mut self, matches: impl Fn(&BridgeSession) -> bool) {
        let oldest = self
            .sessions
            .values()
            .filter(|session| matches(session))
            .min_by_key(|session| session.last_activity())
            .map(|session| session.session_id.clone());
        if let Some(session) = oldest.and_then(|id| self.sessions.remove(&id)) {
            tracing::info!(session_id = %session.session_id, "evicting a session at its limit");
            session.stop();
        }
    }

    fn count(&self, matches: impl Fn(&BridgeSession) -> bool) -> usize {
        self.sessions
            .values()
            .filter(|session| matches(session))
            .count()
    }
}

pub struct Supervisor {
    config: Arc<Config>,
    registry: Mutex<Registry>,
    // ponytail: serializes every spawn across all users. A spawn takes milliseconds,
    // so this is fine at this scale; use per-instance locks if startups ever queue up.
    spawn_lock: tokio::sync::Mutex<()>,
    shutting_down: CancellationToken,
}

impl Supervisor {
    pub fn new(config: Arc<Config>) -> Arc<Self> {
        Arc::new(Self {
            config,
            registry: Mutex::default(),
            spawn_lock: tokio::sync::Mutex::new(()),
            shutting_down: CancellationToken::new(),
        })
    }

    fn registry(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .expect("no code panics holding this lock")
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
        let output = self.make_room(request.user_id, request.instance_id)?;

        let session_id = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 16]>());
        let scratch_dir = create_scratch_dir(&self.config.settings.paths.scratch, &session_id)
            .await
            .map_err(SupervisorError::Scratch)?;
        let runtimes = &self.config.settings.runtimes;
        let command = build_child_command(&ChildSpec {
            runtime_binary: match request.runtime {
                RuntimeKind::Bun => &runtimes.bun,
                RuntimeKind::Node => &runtimes.node,
            },
            script: request.script,
            args: &[],
            env: &request.env,
            scratch_dir: &scratch_dir,
        });
        let secrets: Vec<Zeroizing<String>> =
            request.env.into_iter().map(|(_, value)| value).collect();

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
            secrets,
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

    /// Applies the limits for one more process, and returns the instance's output buffer.
    fn make_room(&self, user_id: i64, instance_id: i64) -> Result<InstanceOutput, SupervisorError> {
        let limits = &self.config.settings.limits;
        let mut registry = self.registry();
        registry.remove_dead_sessions();

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

        while registry.count(|session| session.instance_id == instance_id)
            >= limits.max_sessions_per_instance
        {
            registry.evict_least_recent(|session| session.instance_id == instance_id);
        }
        while registry.count(|session| session.owner_user_id == user_id)
            >= limits.max_processes_per_user
        {
            registry.evict_least_recent(|session| session.owner_user_id == user_id);
        }
        if registry.sessions.len() >= limits.max_processes_total {
            return Err(SupervisorError::TooManyProcesses);
        }

        Ok(registry
            .outputs
            .entry(instance_id)
            .or_insert_with(|| InstanceOutput::new(limits.output_tail_bytes))
            .clone())
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
        self.registry().sessions.retain(|_, session| {
            if session.instance_id == instance_id {
                session.stop();
            }
            session.instance_id != instance_id
        });
    }

    /// Kills an instance's processes and drops what's kept about it, once it's deleted.
    pub fn forget_instance(&self, instance_id: i64) {
        self.stop_instance(instance_id);
        self.registry().outputs.remove(&instance_id);
    }

    pub fn instance_status(&self, instance_id: i64) -> InstanceStatus {
        let mut registry = self.registry();
        registry.remove_dead_sessions();
        let live_processes = registry.count(|session| session.instance_id == instance_id);
        let output = registry.outputs.get(&instance_id);
        InstanceStatus {
            live_processes,
            last_exit: output.and_then(InstanceOutput::last_exit),
            output_tail: output.map_or_else(String::new, |output| output.tail.contents()),
        }
    }

    /// Stops sessions idle past the limit. Never one with a request still running.
    pub fn reap_idle(&self) {
        let idle_timeout = Duration::from_secs(self.config.settings.limits.idle_timeout_secs);
        self.registry().sessions.retain(|_, session| {
            let idle = session.last_activity().elapsed() >= idle_timeout
                && !session.has_requests_in_flight();
            if idle {
                tracing::info!(session_id = %session.session_id, "reaping an idle session");
                session.stop();
            }
            !idle && !session.is_dead()
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
        for (_, session) in self.registry().sessions.drain() {
            session.stop();
        }
        tokio::time::sleep(SHUTDOWN_GRACE).await;
    }
}
