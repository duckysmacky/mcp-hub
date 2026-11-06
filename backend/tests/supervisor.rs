use std::{sync::Arc, time::Duration};

use mcp_hub::{
    bridge::stdio::BridgeSession,
    catalog::RuntimeKind,
    catalog::install::ResolvedServer,
    config::{LimitSettings, Settings},
    supervisor::{StartRequest, Supervisor, SupervisorError},
};
use serde_json::json;
use tempfile::TempDir;

mod common;

fn supervisor(limits: LimitSettings) -> (Arc<Supervisor>, TempDir) {
    let scratch = tempfile::tempdir().unwrap();
    let mut settings = Settings::default();
    settings.runtimes.bun = common::fake_mcp_path();
    settings.paths.scratch = scratch.path().to_owned();
    settings.limits = limits;
    (Supervisor::new(common::config(settings)), scratch)
}

async fn start(
    supervisor: &Supervisor,
    user_id: i64,
    instance_id: i64,
) -> Result<Arc<BridgeSession>, SupervisorError> {
    supervisor
        .start_session(StartRequest {
            user_id,
            instance_id,
            runtime: RuntimeKind::Bun,
            server: &ResolvedServer {
                script: "ignored.js".into(),
            },
            args: &[],
            env: Vec::new(),
            secrets: Vec::new(),
        })
        .await
}

fn generous() -> LimitSettings {
    LimitSettings {
        max_spawns_per_user_per_minute: 1000,
        ..LimitSettings::default()
    }
}

#[tokio::test]
async fn session_lookup_requires_the_matching_user_and_instance() {
    let (supervisor, _scratch) = supervisor(generous());
    let session = start(&supervisor, 1, 10).await.unwrap();
    let id = &session.session_id;

    assert!(supervisor.session(id, 1, 10).is_some());
    assert!(supervisor.session(id, 2, 10).is_none());
    assert!(supervisor.session(id, 1, 11).is_none());
    assert!(supervisor.session("unknown", 1, 10).is_none());

    assert!(!supervisor.end_session(id, 2, 10));
    assert!(supervisor.session(id, 1, 10).is_some());
    assert!(supervisor.end_session(id, 1, 10));
    assert!(supervisor.session(id, 1, 10).is_none());
}

#[tokio::test]
async fn session_ids_are_128_random_bits() {
    let (supervisor, _scratch) = supervisor(generous());
    let first = start(&supervisor, 1, 10).await.unwrap();
    let second = start(&supervisor, 1, 11).await.unwrap();
    assert_eq!(first.session_id.len(), 22);
    assert_ne!(first.session_id, second.session_id);
}

#[tokio::test]
async fn instance_cap_evicts_its_least_recently_used_session() {
    let (supervisor, _scratch) = supervisor(LimitSettings {
        max_sessions_per_instance: 2,
        ..generous()
    });
    let oldest = start(&supervisor, 1, 10).await.unwrap();
    let newer = start(&supervisor, 1, 10).await.unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    // Activity moves the first session ahead of the second.
    oldest
        .request(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .await
        .unwrap();

    let third = start(&supervisor, 1, 10).await.unwrap();

    assert!(newer.is_dead());
    assert!(!oldest.is_dead());
    assert!(!third.is_dead());
    assert_eq!(supervisor.instance_status(10).live_processes, 2);
}

#[tokio::test]
async fn user_cap_only_evicts_that_users_sessions() {
    let (supervisor, _scratch) = supervisor(LimitSettings {
        max_processes_per_user: 1,
        ..generous()
    });
    let other_user = start(&supervisor, 2, 20).await.unwrap();
    let first = start(&supervisor, 1, 10).await.unwrap();

    let second = start(&supervisor, 1, 11).await.unwrap();

    assert!(first.is_dead());
    assert!(!second.is_dead());
    assert!(!other_user.is_dead());
}

#[tokio::test]
async fn global_cap_rejects_instead_of_evicting_other_users() {
    let (supervisor, _scratch) = supervisor(LimitSettings {
        max_processes_total: 1,
        ..generous()
    });
    let other_user = start(&supervisor, 2, 20).await.unwrap();

    let result = start(&supervisor, 1, 10).await;

    assert!(matches!(result, Err(SupervisorError::TooManyProcesses)));
    assert!(!other_user.is_dead());
}

#[tokio::test]
async fn spawn_rate_is_limited_per_user() {
    let (supervisor, _scratch) = supervisor(LimitSettings {
        max_spawns_per_user_per_minute: 2,
        ..LimitSettings::default()
    });
    start(&supervisor, 1, 10).await.unwrap();
    start(&supervisor, 1, 10).await.unwrap();

    assert!(matches!(
        start(&supervisor, 1, 10).await,
        Err(SupervisorError::SpawnRateLimited)
    ));
    assert!(start(&supervisor, 2, 20).await.is_ok());
}

#[tokio::test]
async fn reaper_stops_idle_sessions_but_not_busy_ones() {
    let (supervisor, _scratch) = supervisor(LimitSettings {
        idle_timeout_secs: 0,
        ..generous()
    });
    let idle = start(&supervisor, 1, 10).await.unwrap();
    let busy = start(&supervisor, 1, 11).await.unwrap();
    let request = {
        let busy = busy.clone();
        tokio::spawn(async move {
            busy.request(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/call",
                "params": { "name": "slow", "arguments": { "ms": 300 } },
            }))
            .await
        })
    };
    while !busy.has_requests_in_flight() {
        tokio::task::yield_now().await;
    }

    supervisor.reap_idle();

    assert!(idle.is_dead());
    assert!(!busy.is_dead());
    assert!(request.await.unwrap().is_ok());
}

#[tokio::test]
async fn reaper_keeps_recently_active_sessions() {
    let (supervisor, _scratch) = supervisor(generous());
    let session = start(&supervisor, 1, 10).await.unwrap();
    supervisor.reap_idle();
    assert!(!session.is_dead());
}

#[tokio::test]
async fn dead_sessions_disappear_and_leave_a_status() {
    let (supervisor, _scratch) = supervisor(generous());
    let session = start(&supervisor, 1, 10).await.unwrap();
    let id = session.session_id.clone();
    assert_eq!(supervisor.instance_status(10).live_processes, 1);

    let _ = session
        .request(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "crash", "arguments": {} },
        }))
        .await;

    assert!(supervisor.session(&id, 1, 10).is_none());
    let mut status = supervisor.instance_status(10);
    for _ in 0..100 {
        if status.last_exit.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        status = supervisor.instance_status(10);
    }
    assert_eq!(status.live_processes, 0);
    assert!(status.last_exit.unwrap().contains('3'));
}

#[tokio::test]
async fn stop_instance_only_kills_that_instance() {
    let (supervisor, _scratch) = supervisor(generous());
    let first = start(&supervisor, 1, 10).await.unwrap();
    let second = start(&supervisor, 1, 10).await.unwrap();
    let other = start(&supervisor, 1, 11).await.unwrap();

    supervisor.stop_instance(10);

    assert!(first.is_dead() && second.is_dead());
    assert!(!other.is_dead());
}

#[tokio::test]
async fn shutdown_kills_everything_and_refuses_new_processes() {
    let (supervisor, _scratch) = supervisor(generous());
    let session = start(&supervisor, 1, 10).await.unwrap();

    supervisor.shutdown().await;

    assert!(session.is_dead());
    assert!(start(&supervisor, 1, 10).await.is_err());
}

#[tokio::test]
async fn a_missing_runtime_is_an_error_and_leaves_no_scratch_dir() {
    let scratch = tempfile::tempdir().unwrap();
    let mut settings = Settings::default();
    settings.runtimes.bun = "/nonexistent/runtime".into();
    settings.paths.scratch = scratch.path().to_owned();
    let supervisor = Supervisor::new(common::config(settings));

    assert!(start(&supervisor, 1, 10).await.is_err());
    assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
}
