use std::sync::Arc;

use common::{TestApp, config_at};
use mcp_hub::{
    app_state::AppState,
    catalog::Catalog,
    config::{Cidr, Settings},
    supervisor::Supervisor,
};
use reqwest::StatusCode;
use serde_json::json;
use sqlx::PgPool;

mod common;

const PASSWORD: &str = "correct horse battery";
const LOCAL_URL: &str = "http://localhost:8080";

async fn app(pool: PgPool, customize: impl FnOnce(&mut Settings)) -> TestApp {
    let mut settings = Settings::default();
    customize(&mut settings);
    TestApp::spawn(pool, settings, "", true).await
}

async fn login_from(
    app: &TestApp,
    username: &str,
    forwarded_for: Option<&str>,
) -> reqwest::Response {
    let mut request = app
        .client()
        .post(format!("{}/api/auth/login", app.address))
        .header("origin", app.public_url())
        .header("content-type", "application/json")
        .body(json!({ "username": username, "password": PASSWORD }).to_string());
    if let Some(address) = forwarded_for {
        request = request.header("x-forwarded-for", address);
    }
    request.send().await.unwrap()
}

#[sqlx::test]
async fn login_is_limited_per_address(pool: PgPool) {
    let app = app(pool, |settings| settings.rate_limits.login = 2).await;
    for username in ["a", "b"] {
        assert_eq!(
            login_from(&app, username, None).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let limited = login_from(&app, "c", None).await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        limited.headers()["retry-after"]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            <= 61
    );
}

#[sqlx::test]
async fn login_is_limited_per_username_across_addresses(pool: PgPool) {
    let app = app(pool, |settings| {
        settings.rate_limits.login_per_username = 2;
        settings.network.trusted_proxies = vec![Cidr::try_from("127.0.0.0/8".to_owned()).unwrap()];
    })
    .await;
    for (username, address) in [("alice", "198.51.100.1"), ("ALICE", "198.51.100.2")] {
        assert_eq!(
            login_from(&app, username, Some(address)).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        login_from(&app, "alice", Some("198.51.100.3"))
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        login_from(&app, "bob", Some("198.51.100.3")).await.status(),
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test]
async fn forwarded_addresses_count_only_from_a_trusted_proxy(pool: PgPool) {
    // Trusted: every client address gets its own window.
    let trusted = app(pool.clone(), |settings| {
        settings.rate_limits.login = 1;
        settings.network.trusted_proxies = vec![Cidr::try_from("127.0.0.0/8".to_owned()).unwrap()];
    })
    .await;
    for address in ["198.51.100.1", "198.51.100.2"] {
        assert_eq!(
            login_from(&trusted, "a", Some(address)).await.status(),
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        login_from(&trusted, "a", Some("198.51.100.1"))
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    // A client can't dodge it by forging the left of the header.
    assert_eq!(
        login_from(&trusted, "a", Some("203.0.113.5, 198.51.100.1"))
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );

    // Not trusted: the header is ignored, so changing it gets nobody a new window.
    let untrusted = app(pool, |settings| settings.rate_limits.login = 1).await;
    assert_eq!(
        login_from(&untrusted, "a", Some("198.51.100.1"))
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        login_from(&untrusted, "a", Some("198.51.100.2"))
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[sqlx::test]
async fn sign_up_and_password_changes_are_limited(pool: PgPool) {
    let app = app(pool, |settings| {
        settings.rate_limits.signup = 1;
        settings.rate_limits.password_change = 1;
    })
    .await;
    assert_eq!(
        app.signup("alice", PASSWORD).await.status(),
        StatusCode::CREATED
    );
    assert_eq!(
        app.signup("bob", PASSWORD).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );

    let cookie = app.login("alice", PASSWORD).await;
    let change = json!({ "current_password": "wrong", "new_password": "new password here" });
    assert_eq!(
        app.post("/api/auth/password", Some(&cookie), change.clone())
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.post("/api/auth/password", Some(&cookie), change)
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[sqlx::test]
async fn oauth_endpoints_are_limited(pool: PgPool) {
    let app = app(pool, |settings| {
        settings.rate_limits.oauth_register = 1;
        settings.rate_limits.oauth_authorize = 1;
        settings.rate_limits.oauth_token = 1;
        settings.rate_limits.oauth_revoke = 1;
    })
    .await;
    let post = |path: &str| {
        app.client()
            .post(format!("{}{path}", app.address))
            .form(&[("token", "x"), ("grant_type", "x")])
    };

    common::register_client(&app).await;
    let register = app
        .client()
        .post(format!("{}/oauth/register", app.address))
        .json(&json!({ "redirect_uris": [common::CLIENT_REDIRECT] }))
        .send()
        .await
        .unwrap();
    assert_eq!(register.status(), StatusCode::TOO_MANY_REQUESTS);

    for path in ["/oauth/token", "/oauth/revoke"] {
        assert_ne!(
            post(path).send().await.unwrap().status(),
            StatusCode::TOO_MANY_REQUESTS,
            "{path}"
        );
        assert_eq!(
            post(path).send().await.unwrap().status(),
            StatusCode::TOO_MANY_REQUESTS,
            "{path}"
        );
    }

    let authorize = format!("{}/oauth/authorize", app.address);
    assert_ne!(
        app.client().get(&authorize).send().await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        app.client().get(&authorize).send().await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[sqlx::test]
async fn instances_per_user_are_capped(pool: PgPool) {
    let catalog = r#"
        [[server]]
        id = "demo"
        mode = "stdio"
        runtime = "bun"
        package = "demo@1.0.0"
    "#;
    let mut settings = Settings::default();
    settings.limits.max_instances_per_user = 1;
    let app = TestApp::spawn(pool, settings, catalog, true).await;
    app.signup("alice", PASSWORD).await;
    let cookie = app.login("alice", PASSWORD).await;
    let create = |slug: &str| json!({ "server_id": "demo", "slug": slug });
    assert_eq!(
        app.post("/api/instances", Some(&cookie), create("one"))
            .await
            .status(),
        StatusCode::CREATED
    );
    assert_eq!(
        app.post("/api/instances", Some(&cookie), create("two"))
            .await
            .status(),
        StatusCode::CONFLICT
    );
}

#[sqlx::test]
async fn password_hashing_is_bounded_without_failing_requests(pool: PgPool) {
    let mut settings = Settings::default();
    settings.limits.max_concurrent_password_hashes = 1;
    let config = config_at(settings, LOCAL_URL, true);
    let state = AppState::new(
        Arc::clone(&config),
        pool,
        Catalog::parse("").unwrap(),
        Default::default(),
        Supervisor::new(config),
    );
    let slot = state.password_hash_slot().await;
    let waiter = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        state.password_hash_slot(),
    );
    assert!(waiter.await.is_err());
    drop(slot);
    let _second = state.password_hash_slot().await;
}
