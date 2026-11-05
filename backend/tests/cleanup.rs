use common::{McpApp, connect, post_token};
use mcp_hub::{cleanup, cli};
use reqwest::StatusCode;
use sqlx::PgPool;

mod common;

const CATALOG: &str = r#"
[[server]]
id = "demo"
mode = "stdio"
runtime = "bun"
package = "demo@1.0.0"
"#;

async fn count(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn age(pool: &PgPool, statement: &'static str) {
    sqlx::query(statement).execute(pool).await.unwrap();
}

#[sqlx::test]
async fn live_rows_survive_and_a_connected_client_keeps_working(pool: PgPool) {
    let app = McpApp::spawn(pool.clone(), CATALOG, |_| {}).await;
    let (cookie, _) = app.user_with_instance("alice", "demo", "demo", &[]).await;
    let tokens = connect(&app, &cookie, "demo").await;
    cli::create_invite(&pool, 7, None).await.unwrap();

    cleanup::run_once(&pool, 90).await.unwrap();

    assert_eq!(
        app.get("/api/auth/me", Some(&cookie)).await.status(),
        StatusCode::OK
    );
    app.initialize("demo", &tokens.access_token).await;
    let refreshed = post_token(
        &app,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &tokens.refresh_token),
            ("client_id", &tokens.client_id),
        ],
    )
    .await;
    assert_eq!(refreshed.status(), StatusCode::OK);
    assert_eq!(count(&pool, "invites").await, 1);
    assert!(count(&pool, "audit_events").await > 0);
}

#[sqlx::test]
async fn expired_rows_are_deleted(pool: PgPool) {
    let app = McpApp::spawn(pool.clone(), CATALOG, |_| {}).await;
    let (cookie, _) = app.user_with_instance("alice", "demo", "demo", &[]).await;
    connect(&app, &cookie, "demo").await;
    cli::create_invite(&pool, 7, None).await.unwrap();
    let client = common::register_client(&app).await;
    common::start_authorization(&app, &client, "demo", common::PKCE_CHALLENGE).await;

    for statement in [
        "UPDATE oauth_tokens SET expires_at = now() - interval '1 second'",
        "UPDATE oauth_codes SET expires_at = now() - interval '2 days'",
        "UPDATE oauth_pending_authorizations SET expires_at = now() - interval '1 second'",
        "UPDATE web_sessions SET expires_at = now() - interval '1 second'",
        "UPDATE invites SET expires_at = now() - interval '1 second'",
        "UPDATE audit_events SET at = now() - interval '91 days'",
    ] {
        age(&pool, statement).await;
    }

    cleanup::run_once(&pool, 90).await.unwrap();

    for table in [
        "oauth_tokens",
        "oauth_codes",
        "oauth_pending_authorizations",
        "web_sessions",
        "invites",
        "audit_events",
    ] {
        assert_eq!(count(&pool, table).await, 0, "{table}");
    }
    // The grant isn't expired: it's only without usable tokens until the client reconnects.
    assert_eq!(count(&pool, "oauth_grants").await, 1);
}

#[sqlx::test]
async fn a_used_invite_stays_as_a_record(pool: PgPool) {
    let app = McpApp::spawn(pool.clone(), CATALOG, |_| {}).await;
    app.signup("alice", "correct horse battery").await;
    cli::create_invite(&pool, 7, None).await.unwrap();
    age(
        &pool,
        "UPDATE invites SET used_at = now(), expires_at = now() - interval '1 second'",
    )
    .await;
    cleanup::run_once(&pool, 90).await.unwrap();
    assert_eq!(count(&pool, "invites").await, 1);
}

#[sqlx::test]
async fn a_recently_expired_code_still_detects_replay(pool: PgPool) {
    let app = McpApp::spawn(pool.clone(), CATALOG, |_| {}).await;
    let (cookie, _) = app.user_with_instance("alice", "demo", "demo", &[]).await;
    let client_id = common::register_client(&app).await;
    let pending =
        common::start_authorization(&app, &client_id, "demo", common::PKCE_CHALLENGE).await;
    let code = common::approve(&app, &cookie, &pending).await;
    assert_eq!(
        common::exchange_code(&app, &client_id, &code, "demo")
            .await
            .status(),
        StatusCode::OK
    );
    age(
        &pool,
        "UPDATE oauth_codes SET expires_at = now() - interval '1 second'",
    )
    .await;
    cleanup::run_once(&pool, 90).await.unwrap();
    assert_eq!(count(&pool, "oauth_codes").await, 1);
    common::exchange_code(&app, &client_id, &code, "demo").await;
    let revoked: i64 =
        sqlx::query_scalar("SELECT count(*) FROM oauth_tokens WHERE revoked_at IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(revoked, 2);
}

#[sqlx::test]
async fn revoked_grants_and_abandoned_clients_age_out(pool: PgPool) {
    let app = McpApp::spawn(pool.clone(), CATALOG, |_| {}).await;
    let (cookie, _) = app.user_with_instance("alice", "demo", "demo", &[]).await;
    let connected = connect(&app, &cookie, "demo").await;
    let abandoned = common::register_client(&app).await;
    let recent = common::register_client(&app).await;

    // A client in use stays, however old it is.
    age(
        &pool,
        "UPDATE oauth_clients SET created_at = now() - interval '2 days'",
    )
    .await;
    sqlx::query("UPDATE oauth_clients SET created_at = now() WHERE client_id = $1")
        .bind(&recent)
        .execute(&pool)
        .await
        .unwrap();
    cleanup::run_once(&pool, 90).await.unwrap();
    let remaining: Vec<String> = sqlx::query_scalar("SELECT client_id FROM oauth_clients")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert!(remaining.contains(&connected.client_id));
    assert!(remaining.contains(&recent));
    assert!(!remaining.contains(&abandoned));

    age(
        &pool,
        "UPDATE oauth_grants SET revoked_at = now() - interval '29 days'",
    )
    .await;
    cleanup::run_once(&pool, 90).await.unwrap();
    assert_eq!(count(&pool, "oauth_grants").await, 1);
    age(
        &pool,
        "UPDATE oauth_grants SET revoked_at = now() - interval '31 days'",
    )
    .await;
    cleanup::run_once(&pool, 90).await.unwrap();
    assert_eq!(count(&pool, "oauth_grants").await, 0);
}
