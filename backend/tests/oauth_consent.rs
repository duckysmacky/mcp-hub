use common::{CLIENT_REDIRECT, TestApp, register_client, start_authorization};
use mcp_hub::config::Settings;
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
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

async fn app(pool: PgPool) -> TestApp {
    TestApp::spawn(pool, Settings::default(), CATALOG, true).await
}

async fn user_with_instance(app: &TestApp, username: &str, slug: &str) -> String {
    app.signup(username, PASSWORD).await;
    let cookie = app.login(username, PASSWORD).await;
    let response = app
        .post(
            "/api/instances",
            Some(&cookie),
            json!({ "server_id": "demo", "slug": slug }),
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    cookie
}

fn query_of(location: &str) -> Vec<(String, String)> {
    url::Url::parse(location)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

fn param<'a>(query: &'a [(String, String)], name: &str) -> Option<&'a str> {
    query
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

#[sqlx::test]
async fn metadata_documents_describe_the_hub(pool: PgPool) {
    let app = app(pool).await;

    let server: Value = app
        .get("/.well-known/oauth-authorization-server", None)
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(server["issuer"], "http://localhost:8080");
    assert_eq!(
        server["token_endpoint"],
        "http://localhost:8080/oauth/token"
    );
    assert_eq!(server["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(
        server["token_endpoint_auth_methods_supported"],
        json!(["none"])
    );

    // Answered for a slug nobody has, so slugs can't be probed.
    let resource: Value = app
        .get(
            "/.well-known/oauth-protected-resource/mcp/nobody-has-this",
            None,
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        resource["resource"],
        "http://localhost:8080/mcp/nobody-has-this"
    );
    assert_eq!(
        resource["authorization_servers"],
        json!(["http://localhost:8080"])
    );

    let invalid = app
        .get("/.well-known/oauth-protected-resource/mcp/NOT_VALID", None)
        .await;
    assert_eq!(invalid.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn registration_checks_redirect_uris(pool: PgPool) {
    let app = app(pool).await;
    let url = format!("{}/oauth/register", app.address);

    let registered = app
        .client()
        .post(&url)
        .json(&json!({
            "client_name": "Test Client",
            "redirect_uris": [CLIENT_REDIRECT],
            "token_endpoint_auth_method": "client_secret_post",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(registered.status(), StatusCode::CREATED);
    let body: Value = registered.json().await.unwrap();
    assert!(body["client_id"].as_str().unwrap().len() >= 32);
    assert_eq!(body["token_endpoint_auth_method"], "none");
    assert!(body.get("client_secret").is_none());

    for redirect_uris in [
        json!([]),
        json!(["http://evil.example/cb"]),
        json!(["cursor://cb"]),
    ] {
        let rejected = app
            .client()
            .post(&url)
            .json(&json!({ "redirect_uris": redirect_uris }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            rejected.status(),
            StatusCode::BAD_REQUEST,
            "{redirect_uris}"
        );
    }
}

#[sqlx::test]
async fn bad_client_or_redirect_never_redirects(pool: PgPool) {
    let app = app(pool).await;
    let client_id = register_client(&app).await;

    for params in [
        vec![("redirect_uri", CLIENT_REDIRECT)],
        vec![("client_id", "unknown"), ("redirect_uri", CLIENT_REDIRECT)],
        vec![("client_id", client_id.as_str())],
        vec![
            ("client_id", client_id.as_str()),
            ("redirect_uri", "https://evil.example/cb"),
        ],
    ] {
        let response = app
            .client()
            .get(format!("{}/oauth/authorize", app.address))
            .query(&params)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{params:?}");
        assert!(response.headers().get("location").is_none());
    }
}

#[sqlx::test]
async fn bad_parameters_redirect_back_with_an_error(pool: PgPool) {
    let app = app(pool).await;
    let client_id = register_client(&app).await;
    let resource = "http://localhost:8080/mcp/demo";

    for (name, value, expected) in [
        ("response_type", "token", "invalid_request"),
        ("code_challenge_method", "plain", "invalid_request"),
        ("code_challenge", "too-short", "invalid_request"),
        (
            "resource",
            "https://evil.example/mcp/demo",
            "invalid_target",
        ),
        ("resource", "http://localhost:8080/other", "invalid_target"),
        ("scope", "admin", "invalid_scope"),
    ] {
        let mut params = vec![
            ("client_id", client_id.as_str()),
            ("redirect_uri", CLIENT_REDIRECT),
            ("response_type", "code"),
            ("code_challenge", CHALLENGE),
            ("code_challenge_method", "S256"),
            ("resource", resource),
            ("state", "client-state"),
        ];
        params.retain(|(key, _)| *key != name);
        params.push((name, value));

        let response = app
            .client()
            .get(format!("{}/oauth/authorize", app.address))
            .query(&params)
            .send()
            .await
            .unwrap();

        assert!(response.status().is_redirection(), "{name}={value}");
        let location = response.headers()["location"].to_str().unwrap();
        assert!(location.starts_with(CLIENT_REDIRECT), "{location}");
        let query = query_of(location);
        assert_eq!(param(&query, "error"), Some(expected), "{name}={value}");
        assert_eq!(param(&query, "state"), Some("client-state"));
    }
}

#[sqlx::test]
async fn consent_shows_the_request_and_requires_login(pool: PgPool) {
    let app = app(pool).await;
    let cookie = user_with_instance(&app, "alice", "demo").await;
    let client_id = register_client(&app).await;
    let pending = start_authorization(&app, &client_id, "demo", CHALLENGE).await;
    let path = format!("/api/oauth/pending/{pending}");

    assert_eq!(
        app.get(&path, None).await.status(),
        StatusCode::UNAUTHORIZED
    );

    let details: Value = app.get(&path, Some(&cookie)).await.json().await.unwrap();
    assert_eq!(details["client_name_self_reported"], "Test Client");
    assert_eq!(details["redirect_host"], "127.0.0.1");
    assert_eq!(details["redirect_is_loopback"], true);
    assert_eq!(
        details["instance"],
        json!({ "slug": "demo", "server_id": "demo" })
    );

    let missing = app.get("/api/oauth/pending/unknown", Some(&cookie)).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn approve_redirects_with_a_code_and_can_only_happen_once(pool: PgPool) {
    let app = app(pool.clone()).await;
    let cookie = user_with_instance(&app, "alice", "demo").await;
    let client_id = register_client(&app).await;
    let pending = start_authorization(&app, &client_id, "demo", CHALLENGE).await;
    let path = format!("/api/oauth/pending/{pending}/approve");

    let body: Value = app
        .post(&path, Some(&cookie), json!({}))
        .await
        .json()
        .await
        .unwrap();

    let query = query_of(body["redirect_url"].as_str().unwrap());
    let code = param(&query, "code").unwrap();
    assert!(code.len() >= 43);
    assert_eq!(param(&query, "state"), Some("client-state"));
    assert_eq!(param(&query, "iss"), Some("http://localhost:8080"));

    let row = sqlx::query!("SELECT code_hash, wrapped_dek FROM oauth_codes")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.code_hash.len(), 32);
    assert!(row.wrapped_dek.unwrap().len() > 32);

    let again = app.post(&path, Some(&cookie), json!({})).await;
    assert_eq!(again.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn approval_needs_our_origin(pool: PgPool) {
    let app = app(pool).await;
    let cookie = user_with_instance(&app, "alice", "demo").await;
    let client_id = register_client(&app).await;
    let pending = start_authorization(&app, &client_id, "demo", CHALLENGE).await;

    let forged = app
        .client()
        .post(format!(
            "{}/api/oauth/pending/{pending}/approve",
            app.address
        ))
        .header("cookie", &cookie)
        .header("origin", "https://evil.example")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();

    assert_eq!(forged.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn approval_resolves_the_slug_for_the_logged_in_user(pool: PgPool) {
    let app = app(pool.clone()).await;
    let alice = user_with_instance(&app, "alice", "demo").await;
    let bob = user_with_instance(&app, "bob", "bobs-only").await;
    let client_id = register_client(&app).await;
    let pending = start_authorization(&app, &client_id, "demo", CHALLENGE).await;

    // Bob has no "demo", so he can't approve, and it stays open for Alice.
    let details: Value = app
        .get(&format!("/api/oauth/pending/{pending}"), Some(&bob))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(details["instance"], Value::Null);
    assert_eq!(details["requested_slug"], "demo");
    let path = format!("/api/oauth/pending/{pending}/approve");
    let refused = app.post(&path, Some(&bob), json!({})).await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);

    let approved = app.post(&path, Some(&alice), json!({})).await;
    assert_eq!(approved.status(), StatusCode::OK);
    let grant = sqlx::query!(
        "SELECT users.username FROM oauth_grants JOIN users ON users.id = oauth_grants.user_id"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(grant.username, "alice");
}

#[sqlx::test]
async fn deny_redirects_with_access_denied(pool: PgPool) {
    let app = app(pool).await;
    let cookie = user_with_instance(&app, "alice", "demo").await;
    let client_id = register_client(&app).await;
    let pending = start_authorization(&app, &client_id, "demo", CHALLENGE).await;

    let body: Value = app
        .post(
            &format!("/api/oauth/pending/{pending}/deny"),
            Some(&cookie),
            json!({}),
        )
        .await
        .json()
        .await
        .unwrap();

    let query = query_of(body["redirect_url"].as_str().unwrap());
    assert_eq!(param(&query, "error"), Some("access_denied"));
    assert!(param(&query, "code").is_none());
    let details = app
        .get(&format!("/api/oauth/pending/{pending}"), Some(&cookie))
        .await;
    assert_eq!(details.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn expired_pending_authorization_is_gone(pool: PgPool) {
    let app = app(pool.clone()).await;
    let cookie = user_with_instance(&app, "alice", "demo").await;
    let client_id = register_client(&app).await;
    let pending = start_authorization(&app, &client_id, "demo", CHALLENGE).await;
    sqlx::query!(
        "UPDATE oauth_pending_authorizations SET expires_at = now() - interval '1 second'"
    )
    .execute(&pool)
    .await
    .unwrap();

    let response = app
        .post(
            &format!("/api/oauth/pending/{pending}/approve"),
            Some(&cookie),
            json!({}),
        )
        .await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
