use common::TestApp;
use mcp_hub::{config::Settings, db};
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
env_vars = ["BASE_URL", "TOKEN"]

[[server]]
id = "other"
mode = "stdio"
runtime = "node"
package = "other@1.0.0"
env_vars = ["TOKEN"]
"#;

struct Setup {
    app: TestApp,
    alice: String,
    bob: String,
}

async fn setup(pool: PgPool) -> Setup {
    let app = TestApp::spawn(pool, Settings::default(), CATALOG, true).await;
    app.signup("alice", PASSWORD).await;
    app.signup("bob", PASSWORD).await;
    let alice = app.login("alice", PASSWORD).await;
    let bob = app.login("bob", PASSWORD).await;
    Setup { app, alice, bob }
}

async fn create(app: &TestApp, cookie: &str, slug: &str) -> i64 {
    let response = app
        .post(
            "/api/instances",
            Some(cookie),
            json!({ "server_id": "demo", "slug": slug }),
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    response.json::<Value>().await.unwrap()["id"]
        .as_i64()
        .unwrap()
}

async fn put_env(app: &TestApp, cookie: &str, id: i64, var: &str, value: &str) -> StatusCode {
    app.send_json(
        Method::PUT,
        &format!("/api/instances/{id}/env/{var}"),
        Some(cookie),
        json!({ "value": value }),
    )
    .await
    .status()
}

#[sqlx::test]
async fn everything_requires_a_login(pool: PgPool) {
    let Setup { app, .. } = setup(pool).await;
    for path in [
        "/api/catalog",
        "/api/instances",
        "/api/instances/1",
        "/api/instances/1/status",
    ] {
        assert_eq!(
            app.get(path, None).await.status(),
            StatusCode::UNAUTHORIZED,
            "{path}"
        );
    }
}

#[sqlx::test]
async fn catalog_lists_servers(pool: PgPool) {
    let Setup { app, alice, .. } = setup(pool).await;
    let catalog: Value = app
        .get("/api/catalog", Some(&alice))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        catalog[0],
        json!({
            "id": "demo",
            "mode": "stdio",
            "default_runtime": "bun",
            "env_vars": ["BASE_URL", "TOKEN"],
        })
    );
}

#[sqlx::test]
async fn create_view_update_delete(pool: PgPool) {
    let Setup { app, alice, .. } = setup(pool).await;
    let id = create(&app, &alice, "my-taiga").await;

    let path = format!("/api/instances/{id}");
    let instance: Value = app.get(&path, Some(&alice)).await.json().await.unwrap();
    assert_eq!(instance["slug"], "my-taiga");
    assert_eq!(instance["enabled"], false);
    assert_eq!(instance["runtime"], Value::Null);
    assert_eq!(instance["url"], "http://localhost:8080/mcp/my-taiga");

    let updated: Value = app
        .send_json(
            Method::PATCH,
            &path,
            Some(&alice),
            json!({ "slug": "renamed", "runtime": "node" }),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(updated["slug"], "renamed");
    assert_eq!(updated["runtime"], "node");

    let list: Value = app
        .get("/api/instances", Some(&alice))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);

    let deleted = app
        .send_json(Method::DELETE, &path, Some(&alice), json!({}))
        .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        app.get(&path, Some(&alice)).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test]
async fn slugs_are_validated_and_unique_per_user(pool: PgPool) {
    let Setup { app, alice, bob } = setup(pool).await;
    for slug in ["", "UPPER", "-dash", "has space", "a/b", &"x".repeat(64)] {
        let response = app
            .post(
                "/api/instances",
                Some(&alice),
                json!({ "server_id": "demo", "slug": slug }),
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{slug}");
    }

    create(&app, &alice, "shared").await;
    let duplicate = app
        .post(
            "/api/instances",
            Some(&alice),
            json!({ "server_id": "demo", "slug": "shared" }),
        )
        .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);

    create(&app, &bob, "shared").await;
}

#[sqlx::test]
async fn unknown_server_and_extra_fields_are_rejected(pool: PgPool) {
    let Setup { app, alice, .. } = setup(pool).await;
    let unknown = app
        .post(
            "/api/instances",
            Some(&alice),
            json!({ "server_id": "nope", "slug": "a" }),
        )
        .await;
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);

    // Users can't smuggle in a command, arguments, or a package.
    let command = app
        .post(
            "/api/instances",
            Some(&alice),
            json!({ "server_id": "demo", "slug": "a", "command": "sh" }),
        )
        .await;
    assert!(command.status().is_client_error());
}

#[sqlx::test]
async fn other_users_instances_look_like_they_dont_exist(pool: PgPool) {
    let Setup { app, alice, bob } = setup(pool).await;
    let id = create(&app, &alice, "private").await;
    let path = format!("/api/instances/{id}");

    assert_eq!(
        app.get(&path, Some(&bob)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get(&format!("{path}/status"), Some(&bob))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        put_env(&app, &bob, id, "TOKEN", "x").await,
        StatusCode::NOT_FOUND
    );
    for action in ["start", "stop", "restart"] {
        let response = app
            .post(&format!("{path}/{action}"), Some(&bob), json!({}))
            .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{action}");
    }
    for (method, target) in [
        (Method::PATCH, path.clone()),
        (Method::DELETE, path.clone()),
        (Method::DELETE, format!("{path}/env/TOKEN")),
    ] {
        let response = app.send_json(method, &target, Some(&bob), json!({})).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{target}");
    }
    assert_eq!(app.get(&path, Some(&alice)).await.status(), StatusCode::OK);
}

#[sqlx::test]
async fn env_values_are_write_only_and_encrypted(pool: PgPool) {
    let Setup { app, alice, .. } = setup(pool.clone()).await;
    let id = create(&app, &alice, "demo").await;
    let secret = "super-secret-token-value";

    assert_eq!(
        put_env(&app, &alice, id, "TOKEN", secret).await,
        StatusCode::NO_CONTENT
    );

    let body = app
        .get(&format!("/api/instances/{id}"), Some(&alice))
        .await
        .text()
        .await
        .unwrap();
    assert!(!body.contains(secret));
    let instance: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(instance["env"][0]["var"], "TOKEN");
    assert_eq!(instance["env"][0]["is_set"], true);
    assert_eq!(instance["env"][0]["in_catalog"], true);
    assert!(instance["env"][0].get("value").is_none());

    let row = sqlx::query!("SELECT ciphertext FROM instance_env")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        !row.ciphertext
            .windows(secret.len())
            .any(|part| part == secret.as_bytes())
    );

    let removed = app
        .send_json(
            Method::DELETE,
            &format!("/api/instances/{id}/env/TOKEN"),
            Some(&alice),
            json!({}),
        )
        .await;
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
}

#[sqlx::test]
async fn only_catalog_vars_can_be_set(pool: PgPool) {
    let Setup { app, alice, .. } = setup(pool).await;
    let id = create(&app, &alice, "demo").await;

    for var in ["NODE_OPTIONS", "LD_PRELOAD", "PATH", "UNLISTED"] {
        assert_eq!(
            put_env(&app, &alice, id, var, "x").await,
            StatusCode::BAD_REQUEST,
            "{var}"
        );
    }
}

#[sqlx::test]
async fn url_values_go_through_net_guard_and_errors_hide_the_value(pool: PgPool) {
    let Setup { app, alice, .. } = setup(pool).await;
    let id = create(&app, &alice, "demo").await;

    for value in [
        "https://127.0.0.1:5432",
        "https://169.254.169.254/latest",
        "http://1.1.1.1",
    ] {
        let response = app
            .send_json(
                Method::PUT,
                &format!("/api/instances/{id}/env/BASE_URL"),
                Some(&alice),
                json!({ "value": value }),
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{value}");
        assert!(!response.text().await.unwrap().contains(value));
    }
    assert_eq!(
        put_env(&app, &alice, id, "BASE_URL", "https://1.1.1.1").await,
        StatusCode::NO_CONTENT
    );
}

#[sqlx::test]
async fn hostile_values_are_rejected(pool: PgPool) {
    let Setup { app, alice, .. } = setup(pool).await;
    let id = create(&app, &alice, "demo").await;
    assert_eq!(
        put_env(&app, &alice, id, "TOKEN", "a\0b").await,
        StatusCode::BAD_REQUEST
    );
    let huge = "x".repeat(70 * 1024);
    assert_eq!(
        put_env(&app, &alice, id, "TOKEN", &huge).await,
        StatusCode::BAD_REQUEST
    );
}

#[sqlx::test]
async fn start_stop_and_status(pool: PgPool) {
    let Setup { app, alice, .. } = setup(pool).await;
    let id = create(&app, &alice, "demo").await;
    let status_path = format!("/api/instances/{id}/status");

    let status: Value = app
        .get(&status_path, Some(&alice))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(status["enabled"], false);
    assert_eq!(status["mode"], "stdio");
    assert_eq!(status["runtime"], "bun");
    assert_eq!(status["live_processes"], 0);

    app.post(
        &format!("/api/instances/{id}/start"),
        Some(&alice),
        json!({}),
    )
    .await;
    let started: Value = app
        .get(&status_path, Some(&alice))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(started["enabled"], true);

    app.post(
        &format!("/api/instances/{id}/stop"),
        Some(&alice),
        json!({}),
    )
    .await;
    let stopped: Value = app
        .get(&status_path, Some(&alice))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(stopped["enabled"], false);
}

#[sqlx::test]
async fn removed_catalog_server_flags_and_disables_instances(pool: PgPool) {
    let Setup { app, alice, .. } = setup(pool.clone()).await;
    let id = create(&app, &alice, "demo").await;
    let path = format!("/api/instances/{id}");
    app.post(&format!("{path}/start"), Some(&alice), json!({}))
        .await;

    db::instances::sync_with_catalog(&pool, &["other".to_owned()])
        .await
        .unwrap();
    let flagged: Value = app.get(&path, Some(&alice)).await.json().await.unwrap();
    assert_eq!(flagged["needs_attention"], "server_removed");
    assert_eq!(flagged["enabled"], false);

    db::instances::sync_with_catalog(&pool, &["demo".to_owned()])
        .await
        .unwrap();
    let back: Value = app.get(&path, Some(&alice)).await.json().await.unwrap();
    assert_eq!(back["needs_attention"], Value::Null);
    assert_eq!(back["enabled"], false);
}

#[sqlx::test]
async fn vars_removed_from_the_catalog_are_marked(pool: PgPool) {
    let Setup { app, alice, .. } = setup(pool.clone()).await;
    let id = create(&app, &alice, "demo").await;
    db::instances::set_env(&pool, id, "OLD_VAR", b"ciphertext")
        .await
        .unwrap();

    let instance: Value = app
        .get(&format!("/api/instances/{id}"), Some(&alice))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(instance["env"][0]["var"], "OLD_VAR");
    assert_eq!(instance["env"][0]["in_catalog"], false);
}
