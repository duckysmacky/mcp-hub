use common::{McpApp, connect, exchange_code, register_client, start_authorization};
use reqwest::{Method, StatusCode};
use serde_json::json;
use sqlx::PgPool;

mod common;

const SECRET_VALUE: &str = "secret-env-value";
const SECRET_ARGUMENT: &str = "secret-tool-argument";
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

async fn events(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar!("SELECT event FROM audit_events ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

#[sqlx::test]
async fn activity_is_recorded_without_any_secret(pool: PgPool) {
    let app = McpApp::spawn(pool.clone(), CATALOG, |_| {}).await;
    let (cookie, id) = app
        .user_with_instance("alice", "demo", "demo", &[("TOKEN", SECRET_VALUE)])
        .await;
    let bad_login = app
        .post(
            "/api/auth/login",
            None,
            json!({ "username": "alice", "password": "the wrong password" }),
        )
        .await;
    assert_eq!(bad_login.status(), StatusCode::UNAUTHORIZED);
    let tokens = connect(&app, &cookie, "demo").await;
    let (session, _) = app.initialize("demo", &tokens.access_token).await;
    app.call_tool(
        "demo",
        &tokens.access_token,
        &session,
        "echo",
        json!({ "text": SECRET_ARGUMENT }),
    )
    .await;
    app.send_json(
        Method::DELETE,
        &format!("/api/instances/{id}/env/TOKEN"),
        Some(&cookie),
        json!({}),
    )
    .await;
    app.send_json(
        Method::DELETE,
        &format!("/api/instances/{id}"),
        Some(&cookie),
        json!({}),
    )
    .await;

    let recorded = events(&pool).await;
    for expected in [
        "signup",
        "login",
        "login_failed",
        "instance_created",
        "env_set",
        "grant_approved",
        "process_spawned",
        "tool_call",
        "env_removed",
        "instance_deleted",
    ] {
        assert!(
            recorded.iter().any(|event| event == expected),
            "{expected}: {recorded:?}"
        );
    }

    let everything = sqlx::query_scalar!(
        r#"SELECT string_agg(event || ' ' || COALESCE(detail::text, ''), '\n') AS "text!"
           FROM audit_events"#
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(everything.contains("\"tool\": \"echo\""), "{everything}");
    assert!(everything.contains("\"var\": \"TOKEN\""), "{everything}");
    for secret in [
        SECRET_VALUE,
        SECRET_ARGUMENT,
        PASSWORD,
        "the wrong password",
        tokens.access_token.as_str(),
        tokens.refresh_token.as_str(),
    ] {
        assert!(!everything.contains(secret), "{secret}: {everything}");
    }
}

#[sqlx::test]
async fn replayed_codes_are_recorded(pool: PgPool) {
    let app = McpApp::spawn(pool.clone(), CATALOG, |_| {}).await;
    let (cookie, _) = app
        .user_with_instance("alice", "demo", "demo", &[("TOKEN", "value")])
        .await;
    let client_id = register_client(&app).await;
    let pending = start_authorization(&app, &client_id, "demo", common::PKCE_CHALLENGE).await;
    let code = common::approve(&app, &cookie, &pending).await;
    assert_eq!(
        exchange_code(&app, &client_id, &code, "demo")
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        exchange_code(&app, &client_id, &code, "demo")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert!(
        events(&pool)
            .await
            .iter()
            .any(|event| event == "code_replayed")
    );
}
