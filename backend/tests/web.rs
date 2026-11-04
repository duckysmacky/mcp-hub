use common::TestApp;
use mcp_hub::config::Settings;
use reqwest::StatusCode;
use sqlx::PgPool;

mod common;

async fn app(pool: PgPool) -> (TestApp, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("index.html"), "<title>spa</title>").unwrap();
    std::fs::create_dir(directory.path().join("_app")).unwrap();
    std::fs::write(directory.path().join("_app/app.js"), "console.log(1)").unwrap();

    let mut settings = Settings::default();
    settings.paths.r#static = directory.path().to_owned();
    (TestApp::spawn(pool, settings, "", true).await, directory)
}

#[sqlx::test]
async fn pages_and_assets_are_served_with_protective_headers(pool: PgPool) {
    let (app, _directory) = app(pool).await;

    for path in ["/", "/instances/5", "/consent?request=abc"] {
        let page = app.get(path, None).await;
        assert_eq!(page.status(), StatusCode::OK, "{path}");
        assert_eq!(
            page.headers()["content-security-policy"],
            "frame-ancestors 'none'"
        );
        assert_eq!(page.headers()["x-content-type-options"], "nosniff");
        assert_eq!(page.headers()["referrer-policy"], "no-referrer");
        assert_eq!(page.text().await.unwrap(), "<title>spa</title>", "{path}");
    }

    let asset = app.get("/_app/app.js", None).await;
    assert_eq!(asset.text().await.unwrap(), "console.log(1)");
}

#[sqlx::test]
async fn server_paths_never_fall_back_to_the_page(pool: PgPool) {
    let (app, _directory) = app(pool).await;

    for path in [
        "/api/unknown",
        "/oauth/unknown",
        "/mcp/a/b",
        "/.well-known/unknown",
    ] {
        let response = app.get(path, None).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_ne!(
            response.text().await.unwrap(),
            "<title>spa</title>",
            "{path}"
        );
    }
}

#[sqlx::test]
async fn real_routes_win_over_the_page(pool: PgPool) {
    let (app, _directory) = app(pool).await;

    let health = app.get("/health", None).await;
    assert_eq!(health.text().await.unwrap(), r#"{"status":"ok"}"#);
    let config = app.get("/api/auth/config", None).await;
    assert_eq!(config.status(), StatusCode::OK);
}

#[sqlx::test]
async fn the_hub_still_runs_without_a_build(pool: PgPool) {
    let app = TestApp::spawn(pool, Settings::default(), "", true).await;
    assert_eq!(app.get("/", None).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(app.get("/health", None).await.status(), StatusCode::OK);
}
