use std::time::Duration;

use common::{McpApp, connect, read_json_rpc};
use mcp_hub::{cli, config::Settings, db};
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;

mod common;

const PASSWORD: &str = "correct horse battery";
const CATALOG: &str = r#"
[[server]]
id = "demo"
mode = "stdio"
runtime = "bun"
package = "demo@1.0.0"

[[server.env]]
name = "TOKEN"
"#;

fn test_params() -> mcp_hub::crypto::KdfParams {
    let mut settings = Settings::default();
    settings.password_kdf.memory_kib = 256;
    settings.password_kdf.iterations = 1;
    settings.password_kdf.params()
}

async fn login_status(app: &McpApp, username: &str, password: &str) -> StatusCode {
    let body = json!({ "username": username, "password": password });
    app.post("/api/auth/login", None, body).await.status()
}

#[sqlx::test]
async fn disabling_locks_a_user_out_and_enabling_lets_them_back(pool: PgPool) {
    let app = McpApp::spawn(pool.clone(), CATALOG, |_| {}).await;
    let (cookie, _) = app
        .user_with_instance("alice", "demo", "demo", &[("TOKEN", "value")])
        .await;
    let tokens = connect(&app, &cookie, "demo").await;

    cli::disable_user(&pool, "alice").await.unwrap();

    assert_eq!(
        app.get("/api/auth/me", Some(&cookie)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        login_status(&app, "alice", PASSWORD).await,
        StatusCode::UNAUTHORIZED
    );
    let refused = app
        .mcp_post(
            "demo",
            Some(&tokens.access_token),
            None,
            &common::initialize_message(),
        )
        .await;
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    let wrap_rows = sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM oauth_tokens"#)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(wrap_rows, 0);

    cli::enable_user(&pool, "alice").await.unwrap();
    assert_eq!(login_status(&app, "alice", PASSWORD).await, StatusCode::OK);
    let still_refused = app
        .mcp_post(
            "demo",
            Some(&tokens.access_token),
            None,
            &common::initialize_message(),
        )
        .await;
    assert_eq!(still_refused.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn resetting_a_password_wipes_stored_values_and_access(pool: PgPool) {
    let app = McpApp::spawn(pool.clone(), CATALOG, |_| {}).await;
    let (cookie, id) = app
        .user_with_instance("alice", "demo", "demo", &[("TOKEN", "old-value")])
        .await;
    let tokens = connect(&app, &cookie, "demo").await;
    let (other_cookie, other_id) = app
        .user_with_instance("bob", "demo", "demo", &[("TOKEN", "bob-value")])
        .await;

    let password = cli::reset_password(&pool, "alice", test_params())
        .await
        .unwrap();

    assert_eq!(
        app.get("/api/auth/me", Some(&cookie)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let refused = app
        .mcp_post(
            "demo",
            Some(&tokens.access_token),
            None,
            &common::initialize_message(),
        )
        .await;
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        login_status(&app, "alice", PASSWORD).await,
        StatusCode::UNAUTHORIZED
    );

    let new_cookie = app.login("alice", &password).await;
    let instance: Value = app
        .get(&format!("/api/instances/{id}"), Some(&new_cookie))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(instance["env"], json!([]));
    assert_eq!(instance["enabled"], false);
    assert_eq!(instance["needs_attention"], "secrets_reset");

    // The new DEK works from the new login on, and starting clears the flag.
    let saved = app
        .send_json(
            reqwest::Method::PUT,
            &format!("/api/instances/{id}/env/TOKEN"),
            Some(&new_cookie),
            json!({ "value": "new-value" }),
        )
        .await;
    assert_eq!(saved.status(), StatusCode::NO_CONTENT);
    app.post(
        &format!("/api/instances/{id}/start"),
        Some(&new_cookie),
        json!({}),
    )
    .await;
    let instance: Value = app
        .get(&format!("/api/instances/{id}"), Some(&new_cookie))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(instance["needs_attention"], Value::Null);
    let reconnected = connect(&app, &new_cookie, "demo").await;
    app.initialize("demo", &reconnected.access_token).await;

    // Nobody else was touched.
    let bob: Value = app
        .get(&format!("/api/instances/{other_id}"), Some(&other_cookie))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(bob["enabled"], true);
    assert_eq!(bob["env"][0]["var"], "TOKEN");
}

#[sqlx::test]
async fn unknown_users_are_an_error(pool: PgPool) {
    assert!(cli::disable_user(&pool, "nobody").await.is_err());
    assert!(cli::enable_user(&pool, "nobody").await.is_err());
    assert!(
        cli::reset_password(&pool, "nobody", test_params())
            .await
            .is_err()
    );
}

#[sqlx::test]
async fn usernames_are_listed(pool: PgPool) {
    let app = McpApp::spawn(pool.clone(), CATALOG, |_| {}).await;
    app.signup("alice", PASSWORD).await;
    cli::disable_user(&pool, "ALICE").await.unwrap();
    let users = db::users::list(&pool).await.unwrap();
    assert_eq!(users[0].username, "alice");
    assert!(users[0].disabled_at.is_some());
}

/// Waits for the instance's processes to go, re-sending in case the listener wasn't up yet.
async fn wait_until_stopped(app: &McpApp, pool: &PgPool, id: i64, payload: &str) {
    for _ in 0..100 {
        cli::notify_stop(pool, payload).await.unwrap();
        if app.state.supervisor.instance_status(id).live_processes == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{payload} did not stop instance {id}");
}

#[sqlx::test]
async fn the_running_hub_obeys_stop_requests(pool: PgPool) {
    let app = McpApp::spawn(pool.clone(), CATALOG, |_| {}).await;
    app.state.supervisor.spawn_stop_listener(pool.clone());
    let (cookie, id) = app
        .user_with_instance("alice", "demo", "demo", &[("TOKEN", "value")])
        .await;
    let tokens = connect(&app, &cookie, "demo").await;
    let user_id = sqlx::query_scalar!("SELECT user_id FROM instances WHERE id = $1", id)
        .fetch_one(&pool)
        .await
        .unwrap();

    for payload in [format!("user:{user_id}"), "server:demo".to_owned()] {
        let (session, _) = app.initialize("demo", &tokens.access_token).await;
        assert_eq!(app.state.supervisor.instance_status(id).live_processes, 1);
        wait_until_stopped(&app, &pool, id, &payload).await;
        let gone = app
            .call_tool("demo", &tokens.access_token, &session, "echo", json!({}))
            .await;
        assert_eq!(gone.status(), StatusCode::NOT_FOUND);
    }

    // Other users' processes survive a request about someone else.
    let (session, _) = app.initialize("demo", &tokens.access_token).await;
    cli::notify_stop(&pool, "user:999999").await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let alive = app
        .call_tool("demo", &tokens.access_token, &session, "echo", json!({}))
        .await;
    assert_eq!(read_json_rpc(alive).await["result"]["isError"], Value::Null);
}
