use std::{path::Path, sync::Arc, time::Duration};

use mcp_hub::{
    bridge::stdio::{BridgeError, BridgeSession, SessionLimits, SpawnParams},
    process::{ChildSpec, EnvValue, InstanceOutput, build_child_command, create_scratch_dir},
};
use serde_json::{Value, json};
use tempfile::TempDir;
use zeroize::Zeroizing;

mod common;

const LIMITS: SessionLimits = SessionLimits {
    max_line_bytes: 4096,
    max_pending_requests: 4,
    initialize_timeout: Duration::from_secs(5),
    request_timeout: Duration::from_secs(5),
};

struct Harness {
    session: Arc<BridgeSession>,
    output: InstanceOutput,
    scratch: std::path::PathBuf,
    _root: TempDir,
}

async fn start(limits: SessionLimits, env: Vec<EnvValue>) -> Harness {
    let root = tempfile::tempdir().unwrap();
    let scratch = create_scratch_dir(root.path(), "session").await.unwrap();
    let binary = common::fake_mcp_path();
    let command = build_child_command(&ChildSpec {
        runtime_binary: &binary,
        script: Path::new("ignored.js"),
        args: &[],
        env: &env,
        scratch_dir: &scratch,
    });
    let output = InstanceOutput::new(64 * 1024);
    let session = BridgeSession::spawn(SpawnParams {
        session_id: "session".to_owned(),
        owner_user_id: 1,
        instance_id: 1,
        command,
        scratch_dir: scratch.clone(),
        limits,
        output: output.clone(),
        secrets: env.into_iter().map(|(_, value)| value).collect(),
    })
    .unwrap();
    Harness {
        session,
        output,
        scratch,
        _root: root,
    }
}

fn call(id: i64, tool: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments },
    })
}

fn tool_text(response: &Value) -> &str {
    response["result"]["content"][0]["text"].as_str().unwrap()
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    for _ in 0..200 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("condition never became true");
}

#[tokio::test]
async fn forwards_requests_and_keeps_string_and_number_ids_apart() {
    let harness = start(LIMITS, Vec::new()).await;
    let session = &harness.session;

    let number = session
        .request(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }))
        .await
        .unwrap();
    let string = session
        .request(json!({ "jsonrpc": "2.0", "id": "1", "method": "ping" }))
        .await
        .unwrap();
    assert_eq!(number["id"], 1);
    assert_eq!(string["id"], "1");

    let echoed = session
        .request(call(2, "echo", json!({ "value": 7 })))
        .await
        .unwrap();
    assert_eq!(tool_text(&echoed), r#"{"value":7}"#);
}

#[tokio::test]
async fn child_env_is_only_what_was_declared() {
    let env = vec![("TOKEN".to_owned(), Zeroizing::new("abc".to_owned()))];
    let harness = start(LIMITS, env).await;

    let response = harness
        .session
        .request(call(1, "env", json!({})))
        .await
        .unwrap();
    let child_env: Value = serde_json::from_str(tool_text(&response)).unwrap();
    let mut names: Vec<&String> = child_env.as_object().unwrap().keys().collect();
    names.sort();
    assert_eq!(names, ["HOME", "PATH", "TOKEN"]);
    assert_eq!(child_env["HOME"], harness.scratch.display().to_string());

    let cwd = harness
        .session
        .request(call(2, "cwd", json!({})))
        .await
        .unwrap();
    assert_eq!(tool_text(&cwd), harness.scratch.display().to_string());
}

#[tokio::test]
async fn duplicate_in_flight_id_is_rejected() {
    let harness = start(LIMITS, Vec::new()).await;
    let session = harness.session.clone();
    let slow =
        tokio::spawn(async move { session.request(call(1, "slow", json!({ "ms": 300 }))).await });
    wait_until(|| harness.session.has_requests_in_flight()).await;

    let duplicate = harness
        .session
        .request(call(1, "echo", json!({})))
        .await
        .unwrap();

    assert_eq!(duplicate["error"]["code"], -32600);
    assert_eq!(tool_text(&slow.await.unwrap().unwrap()), "done");
}

#[tokio::test]
async fn pending_requests_are_capped() {
    let harness = start(LIMITS, Vec::new()).await;
    let mut slow_requests = Vec::new();
    for id in 0..4 {
        let session = harness.session.clone();
        slow_requests.push(tokio::spawn(async move {
            session
                .request(call(id, "slow", json!({ "ms": 300 })))
                .await
        }));
    }
    tokio::time::sleep(Duration::from_millis(50)).await;

    let rejected = harness
        .session
        .request(call(99, "echo", json!({})))
        .await
        .unwrap();

    assert_eq!(rejected["error"]["code"], -32600);
    for request in slow_requests {
        request.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn timeout_returns_an_error_and_cancels_in_the_child() {
    let limits = SessionLimits {
        request_timeout: Duration::from_millis(100),
        ..LIMITS
    };
    let harness = start(limits, Vec::new()).await;

    let timed_out = harness
        .session
        .request(call(7, "slow", json!({ "ms": 5000 })))
        .await
        .unwrap();
    assert_eq!(timed_out["error"]["code"], -32001);
    assert!(!harness.session.has_requests_in_flight());

    let cancelled = harness
        .session
        .request(call(8, "cancelled", json!({})))
        .await
        .unwrap();
    assert_eq!(tool_text(&cancelled), "[7]");
}

#[tokio::test]
async fn client_disconnect_cancels_in_the_child() {
    let harness = start(LIMITS, Vec::new()).await;
    let session = harness.session.clone();
    let request = tokio::spawn(async move {
        session
            .request(call(5, "slow", json!({ "ms": 5000 })))
            .await
    });
    wait_until(|| harness.session.has_requests_in_flight()).await;

    request.abort();
    let _ = request.await;

    wait_until(|| !harness.session.has_requests_in_flight()).await;
    let cancelled = harness
        .session
        .request(call(6, "cancelled", json!({})))
        .await
        .unwrap();
    assert_eq!(tool_text(&cancelled), "[5]");
}

#[tokio::test]
async fn crash_wakes_waiters_cleans_up_and_records_the_exit() {
    let harness = start(LIMITS, Vec::new()).await;

    let result = harness.session.request(call(1, "crash", json!({}))).await;

    assert!(matches!(result, Err(BridgeError::SessionDead)));
    assert!(harness.session.is_dead());
    assert!(matches!(
        harness.session.request(call(2, "echo", json!({}))).await,
        Err(BridgeError::SessionDead)
    ));
    wait_until(|| harness.output.last_exit().is_some()).await;
    assert!(harness.output.last_exit().unwrap().contains('3'));
    wait_until(|| !harness.scratch.exists()).await;
}

#[tokio::test]
async fn over_long_stdout_line_kills_the_session() {
    let harness = start(LIMITS, Vec::new()).await;

    let result = harness
        .session
        .request(call(1, "long_line", json!({ "bytes": 8192 })))
        .await;

    assert!(matches!(result, Err(BridgeError::SessionDead)));
    wait_until(|| harness.output.last_exit().is_some()).await;
}

#[tokio::test]
async fn non_json_stdout_lines_are_skipped() {
    let harness = start(LIMITS, Vec::new()).await;

    let response = harness
        .session
        .request(call(1, "noise", json!({})))
        .await
        .unwrap();

    assert_eq!(tool_text(&response), "written");
    assert!(!harness.session.is_dead());
}

#[tokio::test]
async fn heavy_stderr_does_not_block_the_child_and_secrets_are_redacted() {
    let env = vec![("TOKEN".to_owned(), Zeroizing::new("hunter2".to_owned()))];
    let harness = start(LIMITS, env).await;

    let spam = harness
        .session
        .request(call(1, "stderr_spam", json!({ "kib": 1024 })))
        .await
        .unwrap();
    assert_eq!(tool_text(&spam), "spammed");

    harness
        .session
        .request(call(
            2,
            "stderr",
            json!({ "text": "auth failed for hunter2" }),
        ))
        .await
        .unwrap();
    wait_until(|| harness.output.tail.contents().contains("auth failed")).await;
    let tail = harness.output.tail.contents();
    assert!(tail.contains("auth failed for [redacted]"));
    assert!(!tail.contains("hunter2"));
    assert!(tail.len() <= 64 * 1024);
}

#[tokio::test]
async fn server_notifications_reach_subscribers() {
    let harness = start(LIMITS, Vec::new()).await;
    let mut messages = harness.session.subscribe();

    harness
        .session
        .request(call(1, "notify", json!({ "text": "hello" })))
        .await
        .unwrap();

    let message = messages.recv().await.unwrap();
    assert_eq!(message["method"], "notifications/message");
    assert_eq!(message["params"]["data"], "hello");
}

#[tokio::test]
async fn stop_kills_the_child_and_removes_scratch() {
    let harness = start(LIMITS, Vec::new()).await;
    harness
        .session
        .request(call(1, "echo", json!({})))
        .await
        .unwrap();

    harness.session.stop();
    harness.session.stopped().await;

    wait_until(|| harness.output.last_exit().as_deref() == Some("stopped by the hub")).await;
    wait_until(|| !harness.scratch.exists()).await;
}
