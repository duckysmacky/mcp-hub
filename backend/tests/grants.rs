use common::{McpApp, connect, initialize_message};
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
"#;

async fn revoke_token(app: &McpApp, token: &str, client_id: Option<&str>) -> StatusCode {
    let mut form = vec![("token", token)];
    if let Some(client_id) = client_id {
        form.push(("client_id", client_id));
    }
    app.client()
        .post(format!("{}/oauth/revoke", app.address))
        .form(&form)
        .send()
        .await
        .unwrap()
        .status()
}

async fn mcp_status(app: &McpApp, token: &str) -> StatusCode {
    app.mcp_post("demo", Some(token), None, &initialize_message())
        .await
        .status()
}

async fn refresh_status(app: &McpApp, client_id: &str, refresh_token: &str) -> StatusCode {
    common::post_token(
        app,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ],
    )
    .await
    .status()
}

#[sqlx::test]
async fn metadata_advertises_revocation(pool: PgPool) {
    let app = McpApp::spawn(pool, CATALOG, |_| {}).await;
    let metadata: Value = app
        .get("/.well-known/oauth-authorization-server", None)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        metadata["revocation_endpoint"],
        format!("{}/oauth/revoke", app.public_url())
    );
}

#[sqlx::test]
async fn revoking_an_access_token_leaves_the_grant(pool: PgPool) {
    let app = McpApp::spawn(pool, CATALOG, |_| {}).await;
    let (cookie, _) = app.user_with_instance("alice", "demo", "demo", &[]).await;
    let tokens = connect(&app, &cookie, "demo").await;

    assert_eq!(
        revoke_token(&app, &tokens.access_token, Some(&tokens.client_id)).await,
        StatusCode::OK
    );
    assert_eq!(
        mcp_status(&app, &tokens.access_token).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        refresh_status(&app, &tokens.client_id, &tokens.refresh_token).await,
        StatusCode::OK
    );
}

#[sqlx::test]
async fn revoking_a_refresh_token_ends_the_grant(pool: PgPool) {
    let app = McpApp::spawn(pool, CATALOG, |_| {}).await;
    let (cookie, _) = app.user_with_instance("alice", "demo", "demo", &[]).await;
    let tokens = connect(&app, &cookie, "demo").await;

    assert_eq!(
        revoke_token(&app, &tokens.refresh_token, Some(&tokens.client_id)).await,
        StatusCode::OK
    );
    assert_eq!(
        mcp_status(&app, &tokens.access_token).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        refresh_status(&app, &tokens.client_id, &tokens.refresh_token).await,
        StatusCode::BAD_REQUEST
    );
    let grants: Value = app
        .get("/api/grants", Some(&cookie))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(grants, json!([]));
}

#[sqlx::test]
async fn only_the_issued_client_can_revoke(pool: PgPool) {
    let app = McpApp::spawn(pool, CATALOG, |_| {}).await;
    let (cookie, _) = app.user_with_instance("alice", "demo", "demo", &[]).await;
    let tokens = connect(&app, &cookie, "demo").await;

    for client in [Some("someone-else"), None] {
        assert_eq!(
            revoke_token(&app, &tokens.access_token, client).await,
            StatusCode::OK
        );
    }
    assert_eq!(mcp_status(&app, &tokens.access_token).await, StatusCode::OK);
}

#[sqlx::test]
async fn unknown_tokens_look_revoked_and_a_missing_one_is_an_error(pool: PgPool) {
    let app = McpApp::spawn(pool, CATALOG, |_| {}).await;
    for token in ["nonsense", "mcphub_at_AAAA", ""] {
        assert_eq!(revoke_token(&app, token, Some("x")).await, StatusCode::OK);
    }
    let response = app
        .client()
        .post(format!("{}/oauth/revoke", app.address))
        .form(&[("client_id", "x")])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn grants_are_listed_with_their_use_and_client(pool: PgPool) {
    let app = McpApp::spawn(pool, CATALOG, |_| {}).await;
    let (cookie, id) = app.user_with_instance("alice", "demo", "demo", &[]).await;
    let tokens = connect(&app, &cookie, "demo").await;

    let listed: Value = app
        .get("/api/grants", Some(&cookie))
        .await
        .json()
        .await
        .unwrap();
    let grant = &listed[0];
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(grant["instance_id"], id);
    assert_eq!(grant["slug"], "demo");
    assert_eq!(grant["server_id"], "demo");
    assert_eq!(grant["client_name_self_reported"], "Test Client");
    assert_eq!(grant["redirect_hosts"], json!(["127.0.0.1"]));
    assert_eq!(grant["last_used_at"], Value::Null);

    app.initialize("demo", &tokens.access_token).await;
    let listed: Value = app
        .get("/api/grants", Some(&cookie))
        .await
        .json()
        .await
        .unwrap();
    assert!(listed[0]["last_used_at"].is_string());
}

#[sqlx::test]
async fn self_reported_urls_are_stored_only_if_https(pool: PgPool) {
    let app = McpApp::spawn(pool, CATALOG, |_| {}).await;
    let body = json!({
        "client_name": "Client",
        "client_uri": "https://client.example.com",
        "logo_uri": "javascript:alert(1)",
        "redirect_uris": [common::CLIENT_REDIRECT],
    });
    let registered: Value = app
        .client()
        .post(format!("{}/oauth/register", app.address))
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(registered["client_uri"], "https://client.example.com");
    assert_eq!(registered["logo_uri"], Value::Null);
}

#[sqlx::test]
async fn revoking_a_grant_deletes_its_tokens_and_stops_processes(pool: PgPool) {
    let app = McpApp::spawn(pool.clone(), CATALOG, |_| {}).await;
    let (cookie, id) = app.user_with_instance("alice", "demo", "demo", &[]).await;
    let tokens = connect(&app, &cookie, "demo").await;
    app.initialize("demo", &tokens.access_token).await;
    assert_eq!(app.state.supervisor.instance_status(id).live_processes, 1);

    let listed: Value = app
        .get("/api/grants", Some(&cookie))
        .await
        .json()
        .await
        .unwrap();
    let grant_id = listed[0]["id"].as_i64().unwrap();
    let revoked = app
        .send_json(
            Method::DELETE,
            &format!("/api/grants/{grant_id}"),
            Some(&cookie),
            json!({}),
        )
        .await;
    assert_eq!(revoked.status(), StatusCode::NO_CONTENT);

    assert_eq!(app.state.supervisor.instance_status(id).live_processes, 0);
    assert_eq!(
        mcp_status(&app, &tokens.access_token).await,
        StatusCode::UNAUTHORIZED
    );
    let wraps = sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM oauth_tokens"#)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(wraps, 0);
}

#[sqlx::test]
async fn other_users_grants_look_like_they_dont_exist(pool: PgPool) {
    let app = McpApp::spawn(pool, CATALOG, |_| {}).await;
    let (alice, _) = app.user_with_instance("alice", "demo", "demo", &[]).await;
    let tokens = connect(&app, &alice, "demo").await;
    let listed: Value = app
        .get("/api/grants", Some(&alice))
        .await
        .json()
        .await
        .unwrap();
    let grant_id = listed[0]["id"].as_i64().unwrap();

    app.signup("mallory", PASSWORD).await;
    let mallory = app.login("mallory", PASSWORD).await;
    let theirs: Value = app
        .get("/api/grants", Some(&mallory))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(theirs, json!([]));
    let refused = app
        .send_json(
            Method::DELETE,
            &format!("/api/grants/{grant_id}"),
            Some(&mallory),
            json!({}),
        )
        .await;
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    assert_eq!(mcp_status(&app, &tokens.access_token).await, StatusCode::OK);

    assert_eq!(
        app.get("/api/grants", None).await.status(),
        StatusCode::UNAUTHORIZED
    );
}
