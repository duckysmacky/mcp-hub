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
        json!({ "signup_enabled": false })
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
