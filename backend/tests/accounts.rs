use common::TestApp;
use mcp_hub::config::Settings;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::PgPool;

mod common;

const PASSWORD: &str = "correct horse battery";

async fn app(pool: PgPool) -> TestApp {
    TestApp::spawn(pool, Settings::default(), "", true).await
}

#[sqlx::test]
async fn signup_login_me_logout(pool: PgPool) {
    let app = app(pool).await;

    let signup = app.signup("alice", PASSWORD).await;
    assert_eq!(signup.status(), StatusCode::CREATED);
    assert_eq!(signup.json::<Value>().await.unwrap()["username"], "alice");

    let cookie = app.login("alice", PASSWORD).await;
    assert!(app.get("/api/auth/me", None).await.status() == StatusCode::UNAUTHORIZED);

    let me = app.get("/api/auth/me", Some(&cookie)).await;
    assert_eq!(me.status(), StatusCode::OK);
    assert_eq!(me.json::<Value>().await.unwrap()["username"], "alice");

    let logout = app.post("/api/auth/logout", Some(&cookie), json!({})).await;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let after = app.get("/api/auth/me", Some(&cookie)).await;
    assert_eq!(after.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn login_cookie_is_protected(pool: PgPool) {
    let app = app(pool).await;
    app.signup("alice", PASSWORD).await;

    let response = app
        .post(
            "/api/auth/login",
            None,
            json!({ "username": "alice", "password": PASSWORD }),
        )
        .await;

    let cookie = response.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.starts_with("mcphub_session="));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Lax"));
    // The test app's public URL is http://localhost, so `Secure` is off here.
    assert!(!cookie.contains("Secure"));
}

#[sqlx::test]
async fn cookie_is_secure_on_an_https_origin(pool: PgPool) {
    let app = TestApp::spawn_at(
        pool,
        Settings::default(),
        "",
        true,
        "https://hub.example.com",
    )
    .await;
    app.signup("alice", PASSWORD).await;

    let response = app
        .post(
            "/api/auth/login",
            None,
            json!({ "username": "alice", "password": PASSWORD }),
        )
        .await;

    assert!(
        response.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("; Secure")
    );
}

#[sqlx::test]
async fn signup_respects_the_toggle(pool: PgPool) {
    let app = TestApp::spawn(pool, Settings::default(), "", false).await;

    let config = app.get("/api/auth/config", None).await;
    assert_eq!(
        config.json::<Value>().await.unwrap(),
        json!({ "signup": "disabled" })
    );
    assert_eq!(
        app.signup("alice", PASSWORD).await.status(),
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test]
async fn signup_validates_input_and_usernames_are_case_insensitive(pool: PgPool) {
    let app = app(pool).await;

    assert_eq!(
        app.signup("bad name", PASSWORD).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.signup("", PASSWORD).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.signup("alice", "short").await.status(),
        StatusCode::BAD_REQUEST
    );
    let huge = "x".repeat(2000);
    assert_eq!(
        app.signup("alice", &huge).await.status(),
        StatusCode::BAD_REQUEST
    );

    assert_eq!(
        app.signup("alice", PASSWORD).await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        app.signup("ALICE", PASSWORD).await.status(),
        StatusCode::CONFLICT
    );
}

#[sqlx::test]
async fn wrong_password_and_unknown_user_look_identical(pool: PgPool) {
    let app = app(pool).await;
    app.signup("alice", PASSWORD).await;

    let wrong = app
        .post(
            "/api/auth/login",
            None,
            json!({ "username": "alice", "password": "wrong password" }),
        )
        .await;
    let unknown = app
        .post(
            "/api/auth/login",
            None,
            json!({ "username": "nobody", "password": "wrong password" }),
        )
        .await;

    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(wrong.text().await.unwrap(), unknown.text().await.unwrap());
}

#[sqlx::test]
async fn state_changes_need_our_origin_and_json(pool: PgPool) {
    let app = app(pool).await;
    let url = format!("{}/api/auth/signup", app.address);
    let body = json!({ "username": "alice", "password": PASSWORD }).to_string();
    let client = reqwest::Client::new();

    let no_origin = client
        .post(&url)
        .header("content-type", "application/json")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(no_origin.status(), StatusCode::FORBIDDEN);

    let other_origin = client
        .post(&url)
        .header("origin", "https://evil.example")
        .header("content-type", "application/json")
        .body(body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(other_origin.status(), StatusCode::FORBIDDEN);

    let form = client
        .post(&url)
        .header("origin", app.public_url())
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(form.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn credentials_are_stored_only_in_derived_form(pool: PgPool) {
    let app = app(pool.clone()).await;
    app.signup("alice", PASSWORD).await;
    let cookie = app.login("alice", PASSWORD).await;

    let user = sqlx::query!("SELECT password_hash, wrapped_dek FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(user.password_hash.starts_with("$argon2id$"));
    assert!(!user.password_hash.contains(PASSWORD));
    assert!(user.wrapped_dek.len() > 32);

    let session = sqlx::query!("SELECT token_hash, wrapped_dek FROM web_sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    let raw_token = cookie.split('=').nth(1).unwrap();
    assert_eq!(session.token_hash.len(), 32);
    assert_ne!(session.token_hash, raw_token.as_bytes());
    assert!(session.wrapped_dek.len() > 32);
}

#[sqlx::test]
async fn expired_session_is_rejected(pool: PgPool) {
    let app = app(pool.clone()).await;
    app.signup("alice", PASSWORD).await;
    let cookie = app.login("alice", PASSWORD).await;

    sqlx::query!("UPDATE web_sessions SET expires_at = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();

    let me = app.get("/api/auth/me", Some(&cookie)).await;
    assert_eq!(me.status(), StatusCode::UNAUTHORIZED);
}

async fn change_password(app: &TestApp, cookie: &str, current: &str, new: &str) -> StatusCode {
    app.post(
        "/api/auth/password",
        Some(cookie),
        json!({ "current_password": current, "new_password": new }),
    )
    .await
    .status()
}

#[sqlx::test]
async fn changing_the_password_logs_out_other_sessions(pool: PgPool) {
    let app = TestApp::spawn(pool, Settings::default(), "", true).await;
    app.signup("alice", "old password here").await;
    let current = app.login("alice", "old password here").await;
    let other = app.login("alice", "old password here").await;

    assert_eq!(
        change_password(&app, &current, "not the password", "new password here").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        change_password(&app, &current, "old password here", "short").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.get("/api/auth/me", Some(&other)).await.status(),
        StatusCode::OK
    );

    assert_eq!(
        change_password(&app, &current, "old password here", "new password here").await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.get("/api/auth/me", Some(&current)).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        app.get("/api/auth/me", Some(&other)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    let old = json!({ "username": "alice", "password": "old password here" });
    assert_eq!(
        app.post("/api/auth/login", None, old).await.status(),
        StatusCode::UNAUTHORIZED
    );
    app.login("alice", "new password here").await;
}

#[sqlx::test]
async fn changing_the_password_requires_a_login(pool: PgPool) {
    let app = TestApp::spawn(pool, Settings::default(), "", true).await;
    let body = json!({ "current_password": "a", "new_password": "new password here" });
    let response = app.post("/api/auth/password", None, body).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn secrets_and_grants_survive_a_password_change(pool: PgPool) {
    let catalog = r#"
        [[server]]
        id = "demo"
        mode = "stdio"
        runtime = "bun"
        package = "demo@1.0.0"

        [[server.env]]
        name = "TOKEN"
    "#;
    let app = common::McpApp::spawn(pool, catalog, |_| {}).await;
    let (cookie, _) = app
        .user_with_instance("alice", "demo", "demo", &[("TOKEN", "stored-value")])
        .await;
    let tokens = common::connect(&app, &cookie, "demo").await;

    assert_eq!(
        change_password(&app, &cookie, "correct horse battery", "new password here").await,
        StatusCode::NO_CONTENT
    );

    // The old token still unwraps the DEK, and the value it decrypts is intact.
    let (session, _) = app.initialize("demo", &tokens.access_token).await;
    let response = app
        .call_tool("demo", &tokens.access_token, &session, "env", json!({}))
        .await;
    let message = common::read_json_rpc(response).await;
    assert!(common::tool_text(&message).contains("stored-value"));

    // So does the password wrap: a new login can issue tokens that still decrypt it.
    let new_cookie = app.login("alice", "new password here").await;
    let reconnected = common::connect(&app, &new_cookie, "demo").await;
    app.initialize("demo", &reconnected.access_token).await;
}
