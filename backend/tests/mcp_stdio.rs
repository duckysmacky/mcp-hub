use std::time::Duration;

use common::{
    McpApp, connect, initialize_message, post_token, read_json_rpc, tool_call, tool_text,
};
use futures::StreamExt;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;

mod common;

const CATALOG: &str = r#"
[[server]]
id = "demo"
mode = "stdio"
runtime = "bun"
package = "demo@1.0.0"

[[server.env]]
name = "TOKEN"

[[server.env]]
name = "BASE_URL"

[[server.env]]
name = "FAKE_EXIT_AT_START"
"#;

async fn app(pool: PgPool) -> McpApp {
    McpApp::spawn(pool, CATALOG, |_| {}).await
}

/// One user with a started instance and a token for it.
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

#[sqlx::test]
async fn missing_and_invalid_tokens_get_a_discovery_challenge(pool: PgPool) {
    let app = app(pool).await;
    let body = initialize_message();

    let missing = app.mcp_post("demo", None, None, &body).await;
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        missing.headers()["www-authenticate"],
        "Bearer resource_metadata=\"http://localhost:8080/.well-known/oauth-protected-resource/mcp/demo\", scope=\"mcp\""
    );

    for token in ["garbage", "mcphub_at_AAAA", "mcphub_rt_AAAA"] {
        let invalid = app.mcp_post("demo", Some(token), None, &body).await;
        assert_eq!(invalid.status(), StatusCode::UNAUTHORIZED, "{token}");
        let challenge = invalid.headers()["www-authenticate"].to_str().unwrap();
        assert!(challenge.contains("error=\"invalid_token\""), "{challenge}");
    }
}

#[sqlx::test]
async fn tokens_only_count_in_the_authorization_header(pool: PgPool) {
    let app = app(pool).await;
    let (cookie, _, token) = connected(&app, &[]).await;
    let url = format!("{}/mcp/demo", app.address);

    let in_query = app
        .client()
        .post(&url)
        .query(&[("access_token", &token)])
        .body(initialize_message().to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(in_query.status(), StatusCode::UNAUTHORIZED);

    let web_cookie = app
        .client()
        .post(&url)
        .header("cookie", &cookie)
        .body(initialize_message().to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(web_cookie.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn a_full_session_works(pool: PgPool) {
    let app = app(pool).await;
    let (_, _, token) = connected(&app, &[]).await;

    let (session, initialized) = app.initialize("demo", &token).await;
    assert_eq!(initialized["result"]["serverInfo"]["name"], "fake-mcp");
    assert_eq!(initialized["result"]["protocolVersion"], "2025-11-25");

    let notified = app
        .mcp_post(
            "demo",
            Some(&token),
            Some(&session),
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        )
        .await;
    assert_eq!(notified.status(), StatusCode::ACCEPTED);

    let listed = app
        .mcp_post(
            "demo",
            Some(&token),
            Some(&session),
            &json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/list" }),
        )
        .await;
    assert_eq!(listed.headers()["content-type"], "application/json");
    let listed = read_json_rpc(listed).await;
    assert_eq!(listed["id"], 5);
    assert_eq!(listed["result"]["tools"][0]["name"], "echo");

    let called = read_json_rpc(
        app.call_tool("demo", &token, &session, "echo", json!({ "a": 1 }))
            .await,
    )
    .await;
    assert_eq!(tool_text(&called), r#"{"a":1}"#);

    let response_to_server = app
        .mcp_post(
            "demo",
            Some(&token),
            Some(&session),
            &json!({ "jsonrpc": "2.0", "id": 99, "result": {} }),
        )
        .await;
    assert_eq!(response_to_server.status(), StatusCode::ACCEPTED);

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
async fn unsupported_capabilities_never_reach_the_server(pool: PgPool) {
    let app = app(pool).await;
    let (_, _, token) = connected(&app, &[]).await;

    let (_, initialized) = app.initialize("demo", &token).await;

    assert_eq!(
        initialized["result"]["receivedCapabilities"],
        json!({ "experimental": {} })
    );
}

#[sqlx::test]
async fn malformed_requests_are_rejected(pool: PgPool) {
    let app = app(pool).await;
    let (_, _, token) = connected(&app, &[]).await;
    let (session, _) = app.initialize("demo", &token).await;

    let batch = app
        .mcp_post("demo", Some(&token), Some(&session), &json!([]))
        .await;
    assert_eq!(batch.status(), StatusCode::BAD_REQUEST);

    let no_session = app
        .mcp_post("demo", Some(&token), None, &tool_call("echo", json!({})))
        .await;
    assert_eq!(no_session.status(), StatusCode::BAD_REQUEST);

    let unknown_session = app
        .mcp_post(
            "demo",
            Some(&token),
            Some("unknown"),
            &tool_call("echo", json!({})),
        )
        .await;
    assert_eq!(unknown_session.status(), StatusCode::NOT_FOUND);

    let init_in_session = app
        .mcp_post("demo", Some(&token), Some(&session), &initialize_message())
        .await;
    assert_eq!(init_in_session.status(), StatusCode::BAD_REQUEST);

    let not_json = app
        .client()
        .post(format!("{}/mcp/demo", app.address))
        .bearer_auth(&token)
        .body("not json")
        .send()
        .await
        .unwrap();
    assert_eq!(not_json.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn oversized_body_is_rejected(pool: PgPool) {
    let app = McpApp::spawn(pool, CATALOG, |settings| {
        settings.limits.max_body_bytes = 1024;
    })
    .await;
    let (_, _, token) = connected(&app, &[]).await;
    let (session, _) = app.initialize("demo", &token).await;

    let response = app
        .call_tool(
            "demo",
            &token,
            &session,
            "echo",
            json!({ "text": "x".repeat(2048) }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[sqlx::test]
async fn child_gets_decrypted_declared_env_and_no_hub_secrets(pool: PgPool) {
    let app = app(pool).await;
    let (_, _, token) = connected(&app, &[("TOKEN", "upstream-secret")]).await;
    let (session, _) = app.initialize("demo", &token).await;

    let response = read_json_rpc(
        app.call_tool("demo", &token, &session, "env", json!({}))
            .await,
    )
    .await;

    let env: Value = serde_json::from_str(tool_text(&response)).unwrap();
    assert_eq!(env["TOKEN"], "upstream-secret");
    let mut names: Vec<&String> = env.as_object().unwrap().keys().collect();
    names.sort();
    assert_eq!(names, ["HOME", "PATH", "TOKEN"]);
    assert!(!env.to_string().contains(&token), "no token passthrough");
}

#[sqlx::test]
async fn audience_is_enforced_and_looks_the_same_for_any_slug(pool: PgPool) {
    let app = app(pool).await;
    let (cookie, _, token) = connected(&app, &[]).await;
    app.post(
        "/api/instances",
        Some(&cookie),
        json!({ "server_id": "demo", "slug": "second" }),
    )
    .await;

    let own_other_instance = app
        .mcp_post("second", Some(&token), None, &initialize_message())
        .await;
    let nobodys_slug = app
        .mcp_post("does-not-exist", Some(&token), None, &initialize_message())
        .await;

    assert_eq!(own_other_instance.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(nobodys_slug.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn users_with_the_same_slug_stay_separate(pool: PgPool) {
    let app = app(pool).await;
    let (_, _, alice_token) = connected(&app, &[("TOKEN", "alice-secret")]).await;
    let (bob_cookie, _) = app
        .user_with_instance("bob", "demo", "demo", &[("TOKEN", "bob-secret")])
        .await;
    let bob_token = connect(&app, &bob_cookie, "demo").await.access_token;

    let (alice_session, _) = app.initialize("demo", &alice_token).await;
    let (bob_session, _) = app.initialize("demo", &bob_token).await;

    let alice_env = read_json_rpc(
        app.call_tool("demo", &alice_token, &alice_session, "env", json!({}))
            .await,
    )
    .await;
    let bob_env = read_json_rpc(
        app.call_tool("demo", &bob_token, &bob_session, "env", json!({}))
            .await,
    )
    .await;
    assert!(tool_text(&alice_env).contains("alice-secret"));
    assert!(tool_text(&bob_env).contains("bob-secret"));

    // A session id alone grants nothing, even for the same slug.
    let stolen = app
        .call_tool("demo", &bob_token, &alice_session, "echo", json!({}))
        .await;
    assert_eq!(stolen.status(), StatusCode::NOT_FOUND);
    let stolen_delete = app
        .client()
        .delete(format!("{}/mcp/demo", app.address))
        .bearer_auth(&bob_token)
        .header("mcp-session-id", &alice_session)
        .send()
        .await
        .unwrap();
    assert_eq!(stolen_delete.status(), StatusCode::NOT_FOUND);
    let still_alive = app
        .call_tool("demo", &alice_token, &alice_session, "echo", json!({}))
        .await;
    assert_eq!(still_alive.status(), StatusCode::OK);
}

#[sqlx::test]
async fn stopped_instance_returns_404_and_restart_ends_sessions(pool: PgPool) {
    let app = app(pool).await;
    let (cookie, id, token) = connected(&app, &[]).await;
    let (session, _) = app.initialize("demo", &token).await;

    app.post(
        &format!("/api/instances/{id}/restart"),
        Some(&cookie),
        json!({}),
    )
    .await;
    let after_restart = app
        .call_tool("demo", &token, &session, "echo", json!({}))
        .await;
    assert_eq!(after_restart.status(), StatusCode::NOT_FOUND);
    let (session, _) = app.initialize("demo", &token).await;

    app.post(
        &format!("/api/instances/{id}/stop"),
        Some(&cookie),
        json!({}),
    )
    .await;
    let stopped = app
        .call_tool("demo", &token, &session, "echo", json!({}))
        .await;
    assert_eq!(stopped.status(), StatusCode::NOT_FOUND);
    let new_session = app
        .mcp_post("demo", Some(&token), None, &initialize_message())
        .await;
    assert_eq!(new_session.status(), StatusCode::NOT_FOUND);

    app.post(
        &format!("/api/instances/{id}/start"),
        Some(&cookie),
        json!({}),
    )
    .await;
    app.initialize("demo", &token).await;
}

#[sqlx::test]
async fn crash_gives_404_and_the_client_can_reinitialize(pool: PgPool) {
    let app = app(pool).await;
    let (cookie, id, token) = connected(&app, &[]).await;
    let (session, _) = app.initialize("demo", &token).await;

    let crashed = app
        .call_tool("demo", &token, &session, "crash", json!({}))
        .await;
    assert_eq!(crashed.status(), StatusCode::NOT_FOUND);
    let after = app
        .call_tool("demo", &token, &session, "echo", json!({}))
        .await;
    assert_eq!(after.status(), StatusCode::NOT_FOUND);

    let mut current = status(&app, &cookie, id).await;
    for _ in 0..100 {
        if !current["last_exit"].is_null() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        current = status(&app, &cookie, id).await;
    }
    assert!(current["last_exit"].as_str().unwrap().contains('3'));
    assert_eq!(current["live_processes"], 0);

    app.initialize("demo", &token).await;
}

#[sqlx::test]
async fn server_that_dies_at_startup_gives_a_json_rpc_error_and_no_session(pool: PgPool) {
    let app = app(pool).await;
    let (cookie, id, token) = connected(&app, &[("FAKE_EXIT_AT_START", "7")]).await;

    let response = app
        .mcp_post("demo", Some(&token), None, &initialize_message())
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("mcp-session-id").is_none());
    let message = read_json_rpc(response).await;
    assert!(
        message["error"]["message"]
            .as_str()
            .unwrap()
            .contains("status")
    );

    let mut current = status(&app, &cookie, id).await;
    for _ in 0..100 {
        if current["output_tail"].as_str().unwrap().contains("exiting") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        current = status(&app, &cookie, id).await;
    }
    assert!(
        current["output_tail"]
            .as_str()
            .unwrap()
            .contains("exiting at start")
    );
}

#[sqlx::test]
async fn secrets_are_redacted_from_the_status_tail(pool: PgPool) {
    let app = app(pool).await;
    let (cookie, id, token) = connected(&app, &[("TOKEN", "upstream-secret")]).await;
    let (session, _) = app.initialize("demo", &token).await;

    let written = app
        .call_tool(
            "demo",
            &token,
            &session,
            "stderr",
            json!({ "text": "login failed for upstream-secret" }),
        )
        .await;
    assert_eq!(written.status(), StatusCode::OK);

    let mut tail = String::new();
    for _ in 0..100 {
        tail = status(&app, &cookie, id).await["output_tail"]
            .as_str()
            .unwrap()
            .to_owned();
        if tail.contains("login failed") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(tail.contains("login failed for [redacted]"), "{tail}");
    assert!(!tail.contains("upstream-secret"));
}

#[sqlx::test]
async fn url_values_are_checked_again_at_spawn(pool: PgPool) {
    let saving_app = McpApp::spawn(pool.clone(), CATALOG, |settings| {
        settings.network.self_addresses = vec!["203.0.113.10".parse().unwrap()];
    })
    .await;
    let (cookie, _) = saving_app
        .user_with_instance(
            "alice",
            "demo",
            "demo",
            &[("BASE_URL", "https://203.0.113.10")],
        )
        .await;
    let token = connect(&saving_app, &cookie, "demo").await.access_token;

    // The same data under rules that no longer allow that address.
    let app = app(pool).await;
    let response = app
        .mcp_post("demo", Some(&token), None, &initialize_message())
        .await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("mcp-session-id").is_none());
    let message = read_json_rpc(response).await;
    let reason = message["error"]["message"].as_str().unwrap();
    assert!(reason.contains("BASE_URL"), "{reason}");
    assert!(!reason.contains("203.0.113.10"), "{reason}");
}

#[sqlx::test]
async fn expired_token_is_rejected_and_refreshed_token_can_spawn(pool: PgPool) {
    let app = app(pool.clone()).await;
    let (cookie, _) = app
        .user_with_instance("alice", "demo", "demo", &[("TOKEN", "upstream-secret")])
        .await;
    let tokens = connect(&app, &cookie, "demo").await;

    sqlx::query!(
        "UPDATE oauth_tokens SET expires_at = now() - interval '1 second' WHERE kind = 'access'"
    )
    .execute(&pool)
    .await
    .unwrap();
    let expired = app
        .mcp_post(
            "demo",
            Some(&tokens.access_token),
            None,
            &initialize_message(),
        )
        .await;
    assert_eq!(expired.status(), StatusCode::UNAUTHORIZED);

    let refreshed: Value = post_token(
        &app,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &tokens.refresh_token),
            ("client_id", &tokens.client_id),
        ],
    )
    .await
    .json()
    .await
    .unwrap();
    let new_token = refreshed["access_token"].as_str().unwrap();

    let (session, _) = app.initialize("demo", new_token).await;
    let env = read_json_rpc(
        app.call_tool("demo", new_token, &session, "env", json!({}))
            .await,
    )
    .await;
    assert!(tool_text(&env).contains("upstream-secret"));
}

#[sqlx::test]
async fn recreating_a_slug_does_not_revive_old_tokens(pool: PgPool) {
    let app = app(pool).await;
    let (cookie, id, token) = connected(&app, &[]).await;
    let (session, _) = app.initialize("demo", &token).await;

    let deleted = app
        .send_json(
            Method::DELETE,
            &format!("/api/instances/{id}"),
            Some(&cookie),
            json!({}),
        )
        .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let gone = app
        .call_tool("demo", &token, &session, "echo", json!({}))
        .await;
    assert_eq!(gone.status(), StatusCode::UNAUTHORIZED);

    let created = app
        .post(
            "/api/instances",
            Some(&cookie),
            json!({ "server_id": "demo", "slug": "demo" }),
        )
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let new_id = created.json::<Value>().await.unwrap()["id"]
        .as_i64()
        .unwrap();
    app.post(
        &format!("/api/instances/{new_id}/start"),
        Some(&cookie),
        json!({}),
    )
    .await;

    let revived = app
        .mcp_post("demo", Some(&token), None, &initialize_message())
        .await;
    assert_eq!(revived.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn renaming_a_slug_invalidates_its_tokens(pool: PgPool) {
    let app = app(pool).await;
    let (cookie, id, token) = connected(&app, &[]).await;

    app.send_json(
        Method::PATCH,
        &format!("/api/instances/{id}"),
        Some(&cookie),
        json!({ "slug": "renamed" }),
    )
    .await;

    let old_slug = app
        .mcp_post("demo", Some(&token), None, &initialize_message())
        .await;
    assert_eq!(old_slug.status(), StatusCode::NOT_FOUND);
    let new_slug = app
        .mcp_post("renamed", Some(&token), None, &initialize_message())
        .await;
    assert_eq!(new_slug.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn sse_stream_delivers_server_messages_and_ends_with_the_session(pool: PgPool) {
    let app = app(pool).await;
    let (_, _, token) = connected(&app, &[]).await;
    let (session, _) = app.initialize("demo", &token).await;

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
    assert_eq!(stream.headers()["content-type"], "text/event-stream");
    assert_eq!(stream.headers()["x-accel-buffering"], "no");
    let mut body = stream.bytes_stream();

    app.call_tool(
        "demo",
        &token,
        &session,
        "notify",
        json!({ "text": "hello" }),
    )
    .await;

    let chunk = tokio::time::timeout(Duration::from_secs(5), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&chunk);
    assert!(text.contains("event: message"), "{text}");
    assert!(text.contains("notifications/message"), "{text}");
    assert!(text.contains("hello"), "{text}");

    app.client()
        .delete(format!("{}/mcp/demo", app.address))
        .bearer_auth(&token)
        .header("mcp-session-id", &session)
        .send()
        .await
        .unwrap();
    let end = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(chunk) = body.next().await {
            chunk.unwrap();
        }
    })
    .await;
    assert!(end.is_ok(), "the stream must end with the session");
}

#[sqlx::test]
async fn sse_needs_the_right_accept_header_and_a_session(pool: PgPool) {
    let app = app(pool).await;
    let (_, _, token) = connected(&app, &[]).await;
    let (session, _) = app.initialize("demo", &token).await;
    let url = format!("{}/mcp/demo", app.address);

    let wrong_accept = app
        .client()
        .get(&url)
        .bearer_auth(&token)
        .header("accept", "application/json")
        .header("mcp-session-id", &session)
        .send()
        .await
        .unwrap();
    assert_eq!(wrong_accept.status(), StatusCode::NOT_ACCEPTABLE);

    let no_session = app
        .client()
        .get(&url)
        .bearer_auth(&token)
        .header("accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(no_session.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn spawn_limits_become_http_errors(pool: PgPool) {
    let app = McpApp::spawn(pool, CATALOG, |settings| {
        settings.limits.max_spawns_per_user_per_minute = 1;
    })
    .await;
    let (_, _, token) = connected(&app, &[]).await;
    app.initialize("demo", &token).await;

    let limited = app
        .mcp_post("demo", Some(&token), None, &initialize_message())
        .await;

    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.headers()["retry-after"], "60");
}

#[sqlx::test]
async fn chosen_runtime_is_used(pool: PgPool) {
    let app = McpApp::spawn(pool, CATALOG, |settings| {
        settings.runtimes.bun = "/nonexistent/bun".into();
    })
    .await;
    let (cookie, id, token) = connected(&app, &[]).await;

    let broken = app
        .mcp_post("demo", Some(&token), None, &initialize_message())
        .await;
    assert!(broken.headers().get("mcp-session-id").is_none());

    app.send_json(
        Method::PATCH,
        &format!("/api/instances/{id}"),
        Some(&cookie),
        json!({ "runtime": "node" }),
    )
    .await;
    app.initialize("demo", &token).await;
}

const REQUIRED_CATALOG: &str = r#"
[[server]]
id = "demo"
mode = "stdio"
runtime = "bun"
package = "demo@1.0.0"

[[server.env]]
name = "TOKEN"
required = true
"#;

#[sqlx::test]
async fn initialize_reports_a_required_value_removed_after_start(pool: PgPool) {
    let app = McpApp::spawn(pool, REQUIRED_CATALOG, |_| {}).await;
    let (cookie, id) = app
        .user_with_instance("alice", "demo", "demo", &[("TOKEN", "value")])
        .await;
    let tokens = connect(&app, &cookie, "demo").await;
    let removed = app
        .send_json(
            reqwest::Method::DELETE,
            &format!("/api/instances/{id}/env/TOKEN"),
            Some(&cookie),
            json!({}),
        )
        .await;
    assert_eq!(removed.status(), reqwest::StatusCode::NO_CONTENT);

    let response = app
        .mcp_post(
            "demo",
            Some(&tokens.access_token),
            None,
            &initialize_message(),
        )
        .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(response.headers().get("mcp-session-id").is_none());
    let message = read_json_rpc(response).await;
    assert!(
        message["error"]["message"]
            .as_str()
            .unwrap()
            .contains("TOKEN")
    );
}
