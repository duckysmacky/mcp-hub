use common::{
    CLIENT_REDIRECT, IssuedTokens, PKCE_CHALLENGE, PKCE_VERIFIER, TestApp, approve, connect,
    exchange_code, post_token, register_client, start_authorization,
};
use mcp_hub::{
    config::Settings,
    crypto::{self, ACCESS_TOKEN_PREFIX, Token, WrapKind},
};
use reqwest::{Method, StatusCode};
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
env_vars = ["TOKEN"]
"#;

async fn setup(pool: PgPool) -> (TestApp, String, i64) {
    let app = TestApp::spawn(pool, Settings::default(), CATALOG, true).await;
    app.signup("alice", PASSWORD).await;
    let cookie = app.login("alice", PASSWORD).await;
    let created: Value = app
        .post(
            "/api/instances",
            Some(&cookie),
            json!({ "server_id": "demo", "slug": "demo" }),
        )
        .await
        .json()
        .await
        .unwrap();
    (app, cookie, created["id"].as_i64().unwrap())
}

async fn refresh(app: &TestApp, tokens: &IssuedTokens, refresh_token: &str) -> reqwest::Response {
    post_token(
        app,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &tokens.client_id),
        ],
    )
    .await
}

async fn active_token_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM oauth_tokens WHERE revoked_at IS NULL"#)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn error_of(response: reqwest::Response) -> String {
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    body["error"].as_str().unwrap().to_owned()
}

#[sqlx::test]
async fn code_exchange_issues_opaque_tokens(pool: PgPool) {
    let (app, cookie, _) = setup(pool.clone()).await;
    let client_id = register_client(&app).await;
    let pending = start_authorization(&app, &client_id, "demo", PKCE_CHALLENGE).await;
    let code = approve(&app, &cookie, &pending).await;

    let response = exchange_code(&app, &client_id, &code, "demo").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    let body: Value = response.json().await.unwrap();
    assert!(
        body["access_token"]
            .as_str()
            .unwrap()
            .starts_with("mcphub_at_")
    );
    assert!(
        body["refresh_token"]
            .as_str()
            .unwrap()
            .starts_with("mcphub_rt_")
    );
    assert_eq!(body["token_type"], "Bearer");
    assert_eq!(body["expires_in"], 3600);
    assert_eq!(body["scope"], "mcp");

    let code_row = sqlx::query!("SELECT used_at, wrapped_dek FROM oauth_codes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(code_row.used_at.is_some());
    assert!(
        code_row.wrapped_dek.is_none(),
        "the code's wrap must be cleared"
    );
}

#[sqlx::test]
async fn tokens_are_stored_hashed_and_unlock_the_users_dek(pool: PgPool) {
    let (app, cookie, instance_id) = setup(pool.clone()).await;
    let response = app
        .send_json(
            Method::PUT,
            &format!("/api/instances/{instance_id}/env/TOKEN"),
            Some(&cookie),
            json!({ "value": "upstream-secret" }),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let tokens = connect(&app, &cookie, "demo").await;

    let token = Token::parse(ACCESS_TOKEN_PREFIX, &tokens.access_token).unwrap();
    let row = sqlx::query!(
        r#"SELECT token.id, token.wrapped_dek, grants.user_id
           FROM oauth_tokens AS token
           JOIN oauth_grants AS grants ON grants.id = token.grant_id
           WHERE token.token_hash = $1 AND token.kind = 'access'"#,
        token.hash(),
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let kek = token.kek(WrapKind::AccessToken).unwrap();
    let dek = crypto::unwrap_dek(
        &kek,
        &row.wrapped_dek,
        WrapKind::AccessToken,
        row.id,
        row.user_id,
    )
    .unwrap();
    let ciphertext = sqlx::query_scalar!("SELECT ciphertext FROM instance_env")
        .fetch_one(&pool)
        .await
        .unwrap();
    let value = crypto::decrypt_env(&dek, instance_id, "TOKEN", &ciphertext).unwrap();
    assert_eq!(value.as_str(), "upstream-secret");
}

#[sqlx::test]
async fn pkce_failure_is_rejected(pool: PgPool) {
    let (app, cookie, _) = setup(pool).await;
    let client_id = register_client(&app).await;
    let pending = start_authorization(&app, &client_id, "demo", PKCE_CHALLENGE).await;
    let code = approve(&app, &cookie, &pending).await;
    let resource = format!("{}/mcp/demo", app.public_url());

    let response = post_token(
        &app,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", CLIENT_REDIRECT),
            ("client_id", &client_id),
            ("code_verifier", &"x".repeat(43)),
            ("resource", &resource),
        ],
    )
    .await;

    assert_eq!(error_of(response).await, "invalid_grant");
    // A failed attempt doesn't burn the code for the real client.
    let real = exchange_code(&app, &client_id, &code, "demo").await;
    assert_eq!(real.status(), StatusCode::OK);
}

#[sqlx::test]
async fn code_must_match_client_redirect_and_resource(pool: PgPool) {
    let (app, cookie, _) = setup(pool).await;
    let client_id = register_client(&app).await;
    let other_client = register_client(&app).await;
    let pending = start_authorization(&app, &client_id, "demo", PKCE_CHALLENGE).await;
    let code = approve(&app, &cookie, &pending).await;
    let resource = format!("{}/mcp/demo", app.public_url());
    let other_resource = format!("{}/mcp/other", app.public_url());

    for (client, redirect, target) in [
        (other_client.as_str(), CLIENT_REDIRECT, resource.as_str()),
        (
            client_id.as_str(),
            "http://127.0.0.1:1/other",
            resource.as_str(),
        ),
        (client_id.as_str(), CLIENT_REDIRECT, other_resource.as_str()),
    ] {
        let response = post_token(
            &app,
            &[
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("redirect_uri", redirect),
                ("client_id", client),
                ("code_verifier", PKCE_VERIFIER),
                ("resource", target),
            ],
        )
        .await;
        assert_eq!(error_of(response).await, "invalid_grant");
    }
}

#[sqlx::test]
async fn replaying_a_code_revokes_what_it_issued(pool: PgPool) {
    let (app, cookie, _) = setup(pool.clone()).await;
    let client_id = register_client(&app).await;
    let pending = start_authorization(&app, &client_id, "demo", PKCE_CHALLENGE).await;
    let code = approve(&app, &cookie, &pending).await;
    let first = exchange_code(&app, &client_id, &code, "demo").await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(active_token_count(&pool).await, 2);

    let replay = exchange_code(&app, &client_id, &code, "demo").await;

    assert_eq!(error_of(replay).await, "invalid_grant");
    assert_eq!(active_token_count(&pool).await, 0);
}

#[sqlx::test]
async fn expired_code_is_rejected(pool: PgPool) {
    let (app, cookie, _) = setup(pool.clone()).await;
    let client_id = register_client(&app).await;
    let pending = start_authorization(&app, &client_id, "demo", PKCE_CHALLENGE).await;
    let code = approve(&app, &cookie, &pending).await;
    sqlx::query!("UPDATE oauth_codes SET expires_at = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();

    let response = exchange_code(&app, &client_id, &code, "demo").await;

    assert_eq!(error_of(response).await, "invalid_grant");
}

#[sqlx::test]
async fn refresh_rotates_and_the_new_wrap_works(pool: PgPool) {
    let (app, cookie, _) = setup(pool.clone()).await;
    let tokens = connect(&app, &cookie, "demo").await;

    let response = refresh(&app, &tokens, &tokens.refresh_token).await;

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    let new_refresh = body["refresh_token"].as_str().unwrap();
    assert_ne!(new_refresh, tokens.refresh_token);
    assert_ne!(body["access_token"].as_str().unwrap(), tokens.access_token);

    let linked = sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!" FROM oauth_tokens WHERE rotated_from IS NOT NULL"#
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(linked, 2);

    // The chain keeps working, so each rotation re-wrapped the same DEK correctly.
    let next = refresh(&app, &tokens, new_refresh).await;
    assert_eq!(next.status(), StatusCode::OK);
}

#[sqlx::test]
async fn reusing_a_rotated_refresh_token_revokes_the_grant(pool: PgPool) {
    let (app, cookie, _) = setup(pool.clone()).await;
    let tokens = connect(&app, &cookie, "demo").await;
    let rotated: Value = refresh(&app, &tokens, &tokens.refresh_token)
        .await
        .json()
        .await
        .unwrap();

    let reuse = refresh(&app, &tokens, &tokens.refresh_token).await;

    assert_eq!(error_of(reuse).await, "invalid_grant");
    assert_eq!(active_token_count(&pool).await, 0);
    let revoked = sqlx::query_scalar!("SELECT revoked_at FROM oauth_grants")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(revoked.is_some());
    // Even the newest token in the chain is dead now.
    let newest = rotated["refresh_token"].as_str().unwrap();
    assert_eq!(
        error_of(refresh(&app, &tokens, newest).await).await,
        "invalid_grant"
    );
}

#[sqlx::test]
async fn refresh_checks_client_resource_and_expiry(pool: PgPool) {
    let (app, cookie, _) = setup(pool.clone()).await;
    let tokens = connect(&app, &cookie, "demo").await;
    let other_client = register_client(&app).await;

    let wrong_client = post_token(
        &app,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &tokens.refresh_token),
            ("client_id", &other_client),
        ],
    )
    .await;
    assert_eq!(error_of(wrong_client).await, "invalid_grant");

    let wrong_resource = post_token(
        &app,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", &tokens.refresh_token),
            ("client_id", &tokens.client_id),
            ("resource", "http://localhost:8080/mcp/other"),
        ],
    )
    .await;
    assert_eq!(error_of(wrong_resource).await, "invalid_grant");

    sqlx::query!("UPDATE oauth_tokens SET expires_at = now() - interval '1 second'")
        .execute(&pool)
        .await
        .unwrap();
    let expired = refresh(&app, &tokens, &tokens.refresh_token).await;
    assert_eq!(error_of(expired).await, "invalid_grant");
}

#[sqlx::test]
async fn an_access_token_cannot_be_used_as_a_refresh_token(pool: PgPool) {
    let (app, cookie, _) = setup(pool).await;
    let tokens = connect(&app, &cookie, "demo").await;
    let response = refresh(&app, &tokens, &tokens.access_token).await;
    assert_eq!(error_of(response).await, "invalid_grant");
}

#[sqlx::test]
async fn bad_requests_get_oauth_errors(pool: PgPool) {
    let (app, _, _) = setup(pool).await;
    let unsupported = post_token(&app, &[("grant_type", "password")]).await;
    assert_eq!(error_of(unsupported).await, "unsupported_grant_type");
    let missing = post_token(&app, &[("grant_type", "authorization_code")]).await;
    assert_eq!(error_of(missing).await, "invalid_request");
    let empty = post_token(&app, &[]).await;
    assert_eq!(error_of(empty).await, "invalid_request");
}

#[sqlx::test]
async fn deleting_an_instance_removes_its_grants_and_tokens(pool: PgPool) {
    let (app, cookie, instance_id) = setup(pool.clone()).await;
    connect(&app, &cookie, "demo").await;
    assert_eq!(active_token_count(&pool).await, 2);

    app.send_json(
        Method::DELETE,
        &format!("/api/instances/{instance_id}"),
        Some(&cookie),
        json!({}),
    )
    .await;

    assert_eq!(active_token_count(&pool).await, 0);
}
