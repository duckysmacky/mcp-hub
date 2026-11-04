use std::time::Duration;

use common::{McpApp, connect, initialize_message, read_json_rpc, tool_text};
use futures::StreamExt;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;

mod common;

const CATALOG: &str = r#"
[[server]]
id = "demo"
mode = "http"
runtime = "bun"
package = "demo@1.0.0"
env_vars = ["BASE_URL", "TOKEN", "FAKE_NO_LISTEN", "FAKE_REQUIRE_AUTH"]

[server.http]
port_env = "FAKE_HTTP_PORT"
path = "/mcp"
headers = { Authorization = "Bearer {TOKEN}" }
"#;

async fn app(pool: PgPool) -> McpApp {
    McpApp::spawn(pool, CATALOG, |settings| {
        settings.limits.upstream_start_timeout_secs = 2;
    })
    .await
}

async fn connected(app: &McpApp, env: &[(&str, &str)]) -> (String, i64, String) {
    let (cookie, id) = app.user_with_instance("alice", "demo", "demo", env).await;
    let tokens = connect(app, &cookie, "demo").await;
    (cookie, id, tokens.access_token)
}

async fn status(app: &McpApp, cookie: &str, id: i64) -> Value {
    app.get(&format!("/api/instances/{id}/status"), Some(cookie))
        .await
        .json()
        .await
        .unwrap()
}

/// Runs a tool that crashes the upstream, then waits until the hub has noticed.
async fn crash_upstream(app: &McpApp, cookie: &str, id: i64, token: &str, session: &str) {
    let _ = app
        .client()
        .post(format!("{}/mcp/demo", app.address))
        .bearer_auth(token)
        .header("mcp-session-id", session)
        .json(&common::tool_call("exit", json!({ "code": 9 })))
        .send()
        .await;
    for _ in 0..200 {
        if status(app, cookie, id).await["live_processes"] == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the upstream never went away");
}

#[sqlx::test]
async fn a_full_session_works_through_the_proxy(pool: PgPool) {
    let app = app(pool).await;
    let (cookie, id, token) = connected(&app, &[("TOKEN", "upstream-secret")]).await;

    let (session, initialized) = app.initialize("demo", &token).await;
    assert!(session.starts_with("upstream-"));
    assert_eq!(initialized["result"]["serverInfo"]["name"], "fake-mcp");
    assert_eq!(status(&app, &cookie, id).await["live_processes"], 1);

    let echoed = read_json_rpc(
        app.call_tool("demo", &token, &session, "echo", json!({ "a": 1 }))
            .await,
    )
    .await;
    assert_eq!(tool_text(&echoed), r#"{"a":1}"#);

    let notified = app
        .mcp_post(
            "demo",
            Some(&token),
            Some(&session),
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )
        .await;
    assert_eq!(notified.status(), StatusCode::ACCEPTED);

    let deleted = app
        .client()
        .delete(format!("{}/mcp/demo", app.address))
        .bearer_auth(&token)
        .header("mcp-session-id", &session)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let gone = app
        .call_tool("demo", &token, &session, "echo", json!({}))
        .await;
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn credentials_are_injected_and_the_hubs_token_never_passes_through(pool: PgPool) {
    let app = app(pool).await;
    let (_, _, token) = connected(&app, &[("TOKEN", "upstream-secret")]).await;
    let (session, _) = app.initialize("demo", &token).await;

    let response = app
        .client()
        .post(format!("{}/mcp/demo", app.address))
        .bearer_auth(&token)
        .header("mcp-session-id", &session)
        .header("cookie", "should=not-pass")
        .header("x-forwarded-for", "1.2.3.4")
        .header("mcp-protocol-version", "2025-11-25")
        .json(&common::tool_call("headers", json!({})))
        .send()
        .await
        .unwrap();

    let message = read_json_rpc(response).await;
    let seen: Value = serde_json::from_str(tool_text(&message)).unwrap();
    assert_eq!(seen["authorization"], "Bearer upstream-secret");
    assert_eq!(seen["mcp-session-id"], session.as_str());
    assert_eq!(seen["mcp-protocol-version"], "2025-11-25");
    assert!(seen.get("cookie").is_none());
    assert!(seen.get("x-forwarded-for").is_none());
    assert!(!seen.to_string().contains(&token), "no token passthrough");
}

#[sqlx::test]
async fn header_vars_stay_out_of_the_process_env(pool: PgPool) {
    let app = app(pool).await;
    let (_, _, token) = connected(
        &app,
        &[
            ("TOKEN", "upstream-secret"),
            ("BASE_URL", "https://1.1.1.1"),
        ],
    )
    .await;
    let (session, _) = app.initialize("demo", &token).await;

    let response = read_json_rpc(
        app.call_tool("demo", &token, &session, "env", json!({}))
            .await,
    )
    .await;

    let env: Value = serde_json::from_str(tool_text(&response)).unwrap();
    let mut names: Vec<&String> = env.as_object().unwrap().keys().collect();
    names.sort();
    assert_eq!(names, ["BASE_URL", "FAKE_HTTP_PORT", "HOME", "PATH"]);
    assert_eq!(env["BASE_URL"], "https://1.1.1.1");
    assert!(!env.to_string().contains("upstream-secret"));
}

#[sqlx::test]
async fn sessions_of_one_instance_share_one_process(pool: PgPool) {
    let app = app(pool).await;
    let (cookie, id, token) = connected(&app, &[("TOKEN", "x")]).await;

    let (first, _) = app.initialize("demo", &token).await;
    let (second, _) = app.initialize("demo", &token).await;

    assert_ne!(first, second);
    assert_eq!(status(&app, &cookie, id).await["live_processes"], 1);
}

#[sqlx::test]
async fn only_recorded_sessions_are_accepted(pool: PgPool) {
    let app = app(pool).await;
    let (_, _, token) = connected(&app, &[("TOKEN", "x")]).await;
    let (bob_cookie, _) = app
        .user_with_instance("bob", "demo", "demo", &[("TOKEN", "y")])
        .await;
    let bob_token = connect(&app, &bob_cookie, "demo").await.access_token;
    // Each fake process numbers its sessions from 1, so this id exists only for Alice.
    app.initialize("demo", &token).await;
    let (session, _) = app.initialize("demo", &token).await;
    app.initialize("demo", &bob_token).await;

    let invented = app
        .call_tool("demo", &token, "upstream-999", "echo", json!({}))
        .await;
    assert_eq!(invented.status(), StatusCode::NOT_FOUND);

    // Bob holds a valid token for his own "demo", but this id belongs to Alice's process.
    let stolen = app
        .call_tool("demo", &bob_token, &session, "echo", json!({}))
        .await;
    assert_eq!(stolen.status(), StatusCode::NOT_FOUND);

    let no_session = app
        .mcp_post(
            "demo",
            Some(&token),
            None,
            &common::tool_call("echo", json!({})),
        )
        .await;
    assert_eq!(no_session.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn upstream_sse_is_streamed_through(pool: PgPool) {
    let app = app(pool).await;
    let (_, _, token) = connected(&app, &[("TOKEN", "x")]).await;
    let (session, _) = app.initialize("demo", &token).await;

    let response = app
        .call_tool("demo", &token, &session, "sse", json!({}))
        .await;
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    let message = read_json_rpc(response).await;
    assert_eq!(tool_text(&message), "streamed");

    let stream = app
        .client()
        .get(format!("{}/mcp/demo", app.address))
        .bearer_auth(&token)
        .header("accept", "text/event-stream")
        .header("mcp-session-id", &session)
        .send()
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    let mut body = stream.bytes_stream();
    let chunk = tokio::time::timeout(Duration::from_secs(5), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&chunk).contains("notifications/message"));
}

#[sqlx::test]
async fn upstream_headers_and_redirects_are_not_passed_on(pool: PgPool) {
    let app = app(pool).await;
    let (_, _, token) = connected(&app, &[("TOKEN", "x")]).await;
    let (session, _) = app.initialize("demo", &token).await;

    let filtered = app
        .call_tool("demo", &token, &session, "response_headers", json!({}))
        .await;
    assert!(filtered.headers().get("set-cookie").is_none());
    assert!(filtered.headers().get("x-internal").is_none());

    let redirect = app
        .call_tool("demo", &token, &session, "redirect", json!({}))
        .await;
    assert_eq!(redirect.status(), StatusCode::FOUND);
    assert!(redirect.headers().get("location").is_none());
}

#[sqlx::test]
async fn refused_upstream_credentials_do_not_look_like_a_bad_hub_token(pool: PgPool) {
    let app = app(pool).await;
    // No TOKEN value, so the upstream gets no Authorization header and refuses.
    let (_, _, token) = connected(&app, &[("FAKE_REQUIRE_AUTH", "1")]).await;

    let response = app
        .mcp_post("demo", Some(&token), None, &initialize_message())
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("www-authenticate").is_none());
    assert!(response.headers().get("mcp-session-id").is_none());
    let message = read_json_rpc(response).await;
    assert!(
        message["error"]["message"]
            .as_str()
            .unwrap()
            .contains("credentials")
    );
}

#[sqlx::test]
async fn upstream_that_never_listens_times_out_with_a_hint(pool: PgPool) {
    let app = app(pool).await;
    let (cookie, id, token) = connected(&app, &[("FAKE_NO_LISTEN", "1")]).await;

    let response = app
        .mcp_post("demo", Some(&token), None, &initialize_message())
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    let message = read_json_rpc(response).await;
    assert!(
        message["error"]["message"]
            .as_str()
            .unwrap()
            .contains("status")
    );
    assert_eq!(status(&app, &cookie, id).await["live_processes"], 0);
}

#[sqlx::test]
async fn upstream_death_drops_its_sessions_and_restart_backs_off(pool: PgPool) {
    let app = app(pool).await;
    let (cookie, id, token) = connected(&app, &[("TOKEN", "x")]).await;
    let (session, _) = app.initialize("demo", &token).await;

    crash_upstream(&app, &cookie, id, &token, &session).await;

    let old_session = app
        .call_tool("demo", &token, &session, "echo", json!({}))
        .await;
    assert_eq!(old_session.status(), StatusCode::NOT_FOUND);

    // First crash: a short wait before the next start.
    let cooling = app
        .mcp_post("demo", Some(&token), None, &initialize_message())
        .await;
    assert_eq!(cooling.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(cooling.headers().contains_key("retry-after"));

    tokio::time::sleep(Duration::from_millis(1100)).await;
    app.initialize("demo", &token).await;
}

#[sqlx::test]
async fn restart_clears_the_backoff(pool: PgPool) {
    let app = app(pool).await;
    let (cookie, id, token) = connected(&app, &[("TOKEN", "x")]).await;
    let (session, _) = app.initialize("demo", &token).await;
    crash_upstream(&app, &cookie, id, &token, &session).await;

    app.post(
        &format!("/api/instances/{id}/restart"),
        Some(&cookie),
        json!({}),
    )
    .await;

    app.initialize("demo", &token).await;
}

#[sqlx::test]
async fn stop_kills_the_upstream_and_returns_404(pool: PgPool) {
    let app = app(pool).await;
    let (cookie, id, token) = connected(&app, &[("TOKEN", "x")]).await;
    let (session, _) = app.initialize("demo", &token).await;

    app.post(
        &format!("/api/instances/{id}/stop"),
        Some(&cookie),
        json!({}),
    )
    .await;

    let response = app
        .call_tool("demo", &token, &session, "echo", json!({}))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(status(&app, &cookie, id).await["live_processes"], 0);
}

#[sqlx::test]
async fn idle_upstream_is_reaped_but_not_during_a_request(pool: PgPool) {
    let app = McpApp::spawn(pool, CATALOG, |settings| {
        settings.limits.idle_timeout_secs = 0;
    })
    .await;
    let (cookie, id, token) = connected(&app, &[("TOKEN", "x")]).await;
    let (session, _) = app.initialize("demo", &token).await;

    let slow = {
        let client = app.client().clone();
        let url = format!("{}/mcp/demo", app.address);
        let (token, session) = (token.clone(), session.clone());
        tokio::spawn(async move {
            client
                .post(url)
                .bearer_auth(token)
                .header("mcp-session-id", session)
                .json(&common::tool_call("slow", json!({ "ms": 500 })))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;

    app.state.supervisor.reap_idle();
    assert_eq!(status(&app, &cookie, id).await["live_processes"], 1);
    assert!(slow.await.unwrap().contains("done"));

    tokio::time::sleep(Duration::from_millis(50)).await;
    app.state.supervisor.reap_idle();
    assert_eq!(status(&app, &cookie, id).await["live_processes"], 0);
    let gone = app
        .call_tool("demo", &token, &session, "echo", json!({}))
        .await;
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn upstream_output_is_kept_and_redacted(pool: PgPool) {
    let app = app(pool).await;
    let (cookie, id, token) = connected(&app, &[("TOKEN", "upstream-secret")]).await;
    let (session, _) = app.initialize("demo", &token).await;

    app.call_tool(
        "demo",
        &token,
        &session,
        "stderr",
        json!({ "text": "bad token upstream-secret" }),
    )
    .await;

    let mut tail = String::new();
    for _ in 0..100 {
        tail = status(&app, &cookie, id).await["output_tail"]
            .as_str()
            .unwrap()
            .to_owned();
        if tail.contains("bad token") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(tail.contains("bad token [redacted]"), "{tail}");
    assert!(!tail.contains("upstream-secret"));
    assert!(tail.contains("listening on"), "stdout is kept too: {tail}");
}

#[sqlx::test]
async fn a_changed_value_reaches_the_next_request_without_a_restart(pool: PgPool) {
    let app = app(pool).await;
    let (cookie, id, token) = connected(&app, &[("TOKEN", "old-token")]).await;
    let (session, _) = app.initialize("demo", &token).await;

    let replaced = app
        .send_json(
            Method::PUT,
            &format!("/api/instances/{id}/env/TOKEN"),
            Some(&cookie),
            json!({ "value": "new-token" }),
        )
        .await;
    assert_eq!(replaced.status(), StatusCode::NO_CONTENT);

    let message = read_json_rpc(
        app.call_tool("demo", &token, &session, "headers", json!({}))
            .await,
    )
    .await;
    let seen: Value = serde_json::from_str(tool_text(&message)).unwrap();
    assert_eq!(seen["authorization"], "Bearer new-token");
}
