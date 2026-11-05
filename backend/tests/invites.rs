use std::sync::Arc;

use common::{TestApp, config};
use mcp_hub::{
    app_state::AppState,
    catalog::Catalog,
    cli,
    config::{Settings, SignupMode},
    db,
    supervisor::Supervisor,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;

mod common;

const PASSWORD: &str = "correct horse battery";

async fn invite_only_app(pool: PgPool) -> TestApp {
    let mut base = config(Settings::default());
    let settings = &mut Arc::get_mut(&mut base).unwrap().settings;
    settings.server.signup = SignupMode::InviteOnly;
    TestApp::serve(AppState {
        supervisor: Supervisor::new(base.clone()),
        config: base,
        pool,
        catalog: Arc::new(Catalog::parse("").unwrap()),
        bin_scripts: Arc::default(),
    })
    .await
}

async fn signup_with(app: &TestApp, username: &str, code: Option<&str>) -> StatusCode {
    let mut body = json!({ "username": username, "password": PASSWORD });
    if let Some(code) = code {
        body["invite_code"] = json!(code);
    }
    app.post("/api/auth/signup", None, body).await.status()
}

#[sqlx::test]
async fn the_config_reports_the_mode(pool: PgPool) {
    let app = invite_only_app(pool).await;
    let config: Value = app
        .get("/api/auth/config", None)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(config, json!({ "signup": "invite-only" }));
}

#[sqlx::test]
async fn sign_up_needs_a_valid_code(pool: PgPool) {
    let app = invite_only_app(pool).await;
    assert_eq!(
        signup_with(&app, "alice", None).await,
        StatusCode::FORBIDDEN
    );
    for bad in ["", "nonsense", "mcphub_inv_AAAA"] {
        assert_eq!(
            signup_with(&app, "alice", Some(bad)).await,
            StatusCode::FORBIDDEN,
            "{bad}"
        );
    }
}

#[sqlx::test]
async fn a_code_creates_exactly_one_account(pool: PgPool) {
    let app = invite_only_app(pool.clone()).await;
    let code = cli::create_invite(&pool, 7, Some("for alice"))
        .await
        .unwrap();

    // A rejected sign-up doesn't use the invite up.
    let body =
        json!({ "username": "bad name", "password": PASSWORD, "invite_code": code.as_str() });
    assert_eq!(
        app.post("/api/auth/signup", None, body).await.status(),
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        signup_with(&app, "alice", Some(&code)).await,
        StatusCode::CREATED
    );
    app.login("alice", PASSWORD).await;
    assert_eq!(
        signup_with(&app, "bob", Some(&code)).await,
        StatusCode::FORBIDDEN
    );

    let invites = db::invites::list(&pool).await.unwrap();
    assert_eq!(invites.len(), 1);
    assert_eq!(invites[0].note.as_deref(), Some("for alice"));
    assert!(invites[0].used_at.is_some());
}

#[sqlx::test]
async fn a_taken_username_leaves_the_invite_unused(pool: PgPool) {
    let app = invite_only_app(pool.clone()).await;
    let first = cli::create_invite(&pool, 7, None).await.unwrap();
    let second = cli::create_invite(&pool, 7, None).await.unwrap();
    assert_eq!(
        signup_with(&app, "alice", Some(&first)).await,
        StatusCode::CREATED
    );
    assert_eq!(
        signup_with(&app, "ALICE", Some(&second)).await,
        StatusCode::CONFLICT
    );
    assert_eq!(
        signup_with(&app, "bob", Some(&second)).await,
        StatusCode::CREATED
    );
}

#[sqlx::test]
async fn expired_and_revoked_codes_are_refused(pool: PgPool) {
    let app = invite_only_app(pool.clone()).await;
    let expired = cli::create_invite(&pool, 7, None).await.unwrap();
    sqlx::query!("UPDATE invites SET expires_at = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        signup_with(&app, "alice", Some(&expired)).await,
        StatusCode::FORBIDDEN
    );

    let revoked = cli::create_invite(&pool, 7, None).await.unwrap();
    let id = db::invites::list(&pool).await.unwrap().last().unwrap().id;
    assert!(db::invites::revoke(&pool, id).await.unwrap());
    assert!(!db::invites::revoke(&pool, id).await.unwrap());
    assert_eq!(
        signup_with(&app, "alice", Some(&revoked)).await,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test]
async fn codes_are_stored_only_as_hashes(pool: PgPool) {
    let code = cli::create_invite(&pool, 7, None).await.unwrap();
    let stored = sqlx::query_scalar!("SELECT code_hash FROM invites")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored.len(), 32);
    assert!(!String::from_utf8_lossy(&stored).contains(code.as_str()));
}

#[sqlx::test]
async fn used_invites_cannot_be_revoked(pool: PgPool) {
    let app = invite_only_app(pool.clone()).await;
    let code = cli::create_invite(&pool, 7, None).await.unwrap();
    signup_with(&app, "alice", Some(&code)).await;
    let id = db::invites::list(&pool).await.unwrap()[0].id;
    assert!(!db::invites::revoke(&pool, id).await.unwrap());
}
