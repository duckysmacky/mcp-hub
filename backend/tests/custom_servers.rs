use std::os::unix::fs::PermissionsExt;

use common::{McpApp, connect, fake_mcp_path, initialize_message, read_json_rpc, tool_text};
use mcp_hub::db;
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

fn binary_definition() -> Value {
    json!({
        "mode": "stdio",
        "runtime": "binary",
        "package": fake_mcp_path(),
        "bin_args": ["--flag"],
        "env": [{ "name": "MY_TOKEN" }, { "name": "MY_MODE", "secret": false }],
    })
}

async fn app(pool: PgPool) -> McpApp {
    McpApp::spawn(pool, CATALOG, |settings| {
        settings.catalog.allow_custom_servers = true;
    })
    .await
}

async fn user(app: &McpApp, username: &str) -> String {
    assert_eq!(
        app.signup(username, PASSWORD).await.status(),
        StatusCode::CREATED
    );
    app.login(username, PASSWORD).await
}

async fn create_custom(app: &McpApp, cookie: &str, slug: &str, custom: Value) -> Value {
    let response = app
        .post(
            "/api/instances",
            Some(cookie),
            json!({ "slug": slug, "custom": custom }),
        )
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    response.json().await.unwrap()
}

async fn start(app: &McpApp, cookie: &str, id: i64) {
    let response = app
        .post(
            &format!("/api/instances/{id}/start"),
            Some(cookie),
            json!({}),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

async fn instances(app: &McpApp, cookie: &str) -> Vec<Value> {
    app.get("/api/instances", Some(cookie))
        .await
        .json()
        .await
        .unwrap()
}

#[sqlx::test]
async fn custom_servers_are_off_unless_the_operator_opts_in(pool: PgPool) {
    let app = McpApp::spawn(pool, CATALOG, |_| {}).await;
    let alice = user(&app, "alice").await;

    let settings: Value = app
        .get("/api/catalog/settings", Some(&alice))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(settings["custom_servers"], false);

    let response = app
        .post(
            "/api/instances",
            Some(&alice),
            json!({ "slug": "mine", "custom": binary_definition() }),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(instances(&app, &alice).await.is_empty());
}

#[sqlx::test]
async fn the_settings_need_a_session(pool: PgPool) {
    let app = app(pool).await;
    let response = app.get("/api/catalog/settings", None).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn a_custom_server_runs_with_its_args_and_declared_env(pool: PgPool) {
    let app = app(pool).await;
    let alice = user(&app, "alice").await;
    let settings: Value = app
        .get("/api/catalog/settings", Some(&alice))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(settings["custom_servers"], true);

    let instance = create_custom(&app, &alice, "mine", binary_definition()).await;
    let id = instance["id"].as_i64().unwrap();
    assert_eq!(instance["server_id"], "custom");
    assert_eq!(instance["runtime"], "binary");
    assert_eq!(instance["custom"]["bin_args"], json!(["--flag"]));

    for (var, value) in [("MY_TOKEN", "hunter2"), ("MY_MODE", "fast")] {
        let response = app
            .send_json(
                Method::PUT,
                &format!("/api/instances/{id}/env/{var}"),
                Some(&alice),
                json!({ "value": value }),
            )
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT, "{var}");
    }
    start(&app, &alice, id).await;

    let described: Value = app
        .get(&format!("/api/instances/{id}"), Some(&alice))
        .await
        .json()
        .await
        .unwrap();
    let values: Vec<&Value> = described["env"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| &entry["value"])
        .collect();
    // Sorted by name: the non-secret comes back, the secret never does.
    assert_eq!(values, [&json!("fast"), &Value::Null]);
    assert!(!described.to_string().contains("hunter2"));

    let token = connect(&app, &alice, "mine").await.access_token;
    let (session, _) = app.initialize("mine", &token).await;
    let response = app
        .call_tool("mine", &token, &session, "env", json!({}))
        .await;
    let text = read_json_rpc(response).await;
    let env: Value = serde_json::from_str(tool_text(&text)).unwrap();
    assert_eq!(env["MY_TOKEN"], "hunter2");
    assert_eq!(env["MY_MODE"], "fast");
    assert!(env.get("DATABASE_URL").is_none());
}

#[sqlx::test]
async fn other_users_never_see_a_custom_server(pool: PgPool) {
    let app = app(pool).await;
    let alice = user(&app, "alice").await;
    let bob = user(&app, "bob").await;
    let instance = create_custom(&app, &alice, "mine", binary_definition()).await;
    let id = instance["id"].as_i64().unwrap();
    start(&app, &alice, id).await;
    let path = format!("/api/instances/{id}");

    assert!(instances(&app, &bob).await.is_empty());
    let catalog: Value = app
        .get("/api/catalog", Some(&bob))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(catalog.as_array().unwrap().len(), 1);
    assert_eq!(catalog[0]["id"], "demo");

    let read = app.get(&path, Some(&bob)).await;
    assert_eq!(read.status(), StatusCode::NOT_FOUND);
    let edit = app
        .send_json(
            Method::PATCH,
            &path,
            Some(&bob),
            json!({ "custom": binary_definition() }),
        )
        .await;
    assert_eq!(edit.status(), StatusCode::NOT_FOUND);
    let delete = app
        .send_json(Method::DELETE, &path, Some(&bob), json!({}))
        .await;
    assert_eq!(delete.status(), StatusCode::NOT_FOUND);

    // Bob's own valid token doesn't reach Alice's slug.
    let created = app
        .post(
            "/api/instances",
            Some(&bob),
            json!({ "server_id": "demo", "slug": "his" }),
        )
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let bob_token = connect(&app, &bob, "his").await.access_token;
    let response = app
        .mcp_post("mine", Some(&bob_token), None, &initialize_message())
        .await;
    assert!(
        matches!(
            response.status(),
            StatusCode::NOT_FOUND | StatusCode::UNAUTHORIZED
        ),
        "{}",
        response.status()
    );
    assert_eq!(instances(&app, &alice).await.len(), 1);
}

#[sqlx::test]
async fn bad_definitions_are_refused_and_leave_nothing_behind(pool: PgPool) {
    let app = app(pool).await;
    let alice = user(&app, "alice").await;
    let with = |field: &str, value: Value| {
        let mut definition = binary_definition();
        definition[field] = value;
        definition
    };
    let bodies = [
        json!({ "slug": "x", "custom": with("package", json!("relative/path")) }),
        json!({ "slug": "x", "custom": with("package", json!("/nonexistent/server")) }),
        json!({ "slug": "x", "custom": with("env", json!([{ "name": "LD_PRELOAD" }])) }),
        json!({ "slug": "x", "custom": with("env", json!([{ "name": "PATH" }])) }),
        json!({ "slug": "x", "custom": with("bin_args", json!(vec!["a"; 65])) }),
        json!({ "slug": "x", "custom": {
            "mode": "stdio", "runtime": "bun", "package": "demo@latest"
        } }),
        json!({ "slug": "x", "server_id": "custom" }),
        json!({ "slug": "x" }),
        json!({ "slug": "x", "server_id": "demo", "custom": binary_definition() }),
    ];
    for body in bodies {
        let response = app.post("/api/instances", Some(&alice), body.clone()).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
    }
    assert!(instances(&app, &alice).await.is_empty());
}

#[sqlx::test]
async fn editing_a_definition_stops_its_processes(pool: PgPool) {
    let app = app(pool).await;
    let alice = user(&app, "alice").await;
    let instance = create_custom(&app, &alice, "mine", binary_definition()).await;
    let id = instance["id"].as_i64().unwrap();
    start(&app, &alice, id).await;
    let token = connect(&app, &alice, "mine").await.access_token;
    let (session, _) = app.initialize("mine", &token).await;

    let mut edited = binary_definition();
    edited["bin_args"] = json!(["--other"]);
    let response = app
        .send_json(
            Method::PATCH,
            &format!("/api/instances/{id}"),
            Some(&alice),
            json!({ "custom": edited }),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["custom"]["bin_args"], json!(["--other"]));

    let gone = app
        .call_tool("mine", &token, &session, "echo", json!({}))
        .await;
    assert_eq!(gone.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn a_bad_edit_keeps_the_old_definition(pool: PgPool) {
    let app = app(pool).await;
    let alice = user(&app, "alice").await;
    let instance = create_custom(&app, &alice, "mine", binary_definition()).await;
    let id = instance["id"].as_i64().unwrap();
    let path = format!("/api/instances/{id}");

    let mut broken = binary_definition();
    broken["package"] = json!("/nonexistent/server");
    let response = app
        .send_json(
            Method::PATCH,
            &path,
            Some(&alice),
            json!({ "custom": broken }),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body: Value = app.get(&path, Some(&alice)).await.json().await.unwrap();
    assert_eq!(body["custom"]["package"], binary_definition()["package"]);
}

#[sqlx::test]
async fn only_a_custom_instance_takes_a_definition(pool: PgPool) {
    let app = app(pool).await;
    let alice = user(&app, "alice").await;
    let created: Value = app
        .post(
            "/api/instances",
            Some(&alice),
            json!({ "server_id": "demo", "slug": "catalog" }),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(created["custom"], Value::Null);

    let id = created["id"].as_i64().unwrap();
    let response = app
        .send_json(
            Method::PATCH,
            &format!("/api/instances/{id}"),
            Some(&alice),
            json!({ "custom": binary_definition() }),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn turning_the_setting_off_stops_custom_servers(pool: PgPool) {
    let on = app(pool.clone()).await;
    let alice = user(&on, "alice").await;
    let instance = create_custom(&on, &alice, "mine", binary_definition()).await;
    let id = instance["id"].as_i64().unwrap();
    start(&on, &alice, id).await;
    let token = connect(&on, &alice, "mine").await.access_token;
    on.initialize("mine", &token).await;

    // Refused from the first request, even before the startup sync flags the instance.
    let off = McpApp::spawn(pool.clone(), CATALOG, |_| {}).await;
    let response = off
        .mcp_post("mine", Some(&token), None, &initialize_message())
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    db::instances::sync_custom_servers(&pool, false)
        .await
        .unwrap();
    let path = format!("/api/instances/{id}");
    let alice_off = off.login("alice", PASSWORD).await;
    let body: Value = off.get(&path, Some(&alice_off)).await.json().await.unwrap();
    assert_eq!(body["needs_attention"], "custom_disabled");
    assert_eq!(body["enabled"], false);
    assert_eq!(body["runtime"], Value::Null);
    let refused = off
        .post(&format!("{path}/start"), Some(&alice_off), json!({}))
        .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);

    db::instances::sync_custom_servers(&pool, true)
        .await
        .unwrap();
    let body: Value = on.get(&path, Some(&alice)).await.json().await.unwrap();
    assert_eq!(body["needs_attention"], Value::Null);
    assert_eq!(body["enabled"], false);
}

/// A fake bun: "installs" on `install`, and runs the fake MCP server otherwise.
fn write_fake_bun(path: &std::path::Path) {
    std::fs::write(
        path,
        format!(
            r#"#!/bin/sh
case "$1" in
  install)
    echo "$@" >> args.txt
    echo tree > bun.lock
    mkdir -p node_modules/demo
    echo '{{"bin":"cli.js"}}' > node_modules/demo/package.json
    touch node_modules/demo/cli.js
    ;;
  *) exec {} "$@" ;;
esac
"#,
            fake_mcp_path().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[sqlx::test]
async fn a_package_installs_with_scripts_off_reinstalls_if_lost_and_goes_on_delete(pool: PgPool) {
    let root = tempfile::tempdir().unwrap();
    let fake_bun = root.path().join("fake-bun.sh");
    write_fake_bun(&fake_bun);
    let packages = root.path().join("packages");
    let app = McpApp::spawn(pool, CATALOG, |settings| {
        settings.catalog.allow_custom_servers = true;
        settings.runtimes.bun = fake_bun.clone();
        settings.paths.packages = packages.clone();
    })
    .await;
    let alice = user(&app, "alice").await;

    let definition = json!({ "mode": "stdio", "runtime": "bun", "package": "demo@1.2.3" });
    let instance = create_custom(&app, &alice, "mine", definition).await;
    let id = instance["id"].as_i64().unwrap();
    let directory = packages.join(".custom").join(id.to_string());
    let args = std::fs::read_to_string(directory.join("args.txt")).unwrap();
    assert_eq!(args.trim(), "install --frozen-lockfile --ignore-scripts");
    assert_eq!(instance["package"], "demo@1.2.3");

    std::fs::remove_dir_all(&directory).unwrap();
    start(&app, &alice, id).await;
    let token = connect(&app, &alice, "mine").await.access_token;
    app.initialize("mine", &token).await;
    assert!(directory.join("node_modules/demo/cli.js").exists());

    let deleted = app
        .send_json(
            Method::DELETE,
            &format!("/api/instances/{id}"),
            Some(&alice),
            json!({}),
        )
        .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert!(!directory.exists());
}
