#![allow(dead_code)] // Each test crate uses a different part of this module.

use std::{collections::HashMap, path::PathBuf, process::Command, sync::Arc, sync::OnceLock};

use mcp_hub::{
    app_state::AppState,
    build_router,
    catalog::Catalog,
    config::{Config, Settings},
    supervisor::Supervisor,
};
use reqwest::Response;
use serde_json::{Value, json};
use sqlx::{PgPool, postgres::PgConnectOptions};

const DEFAULT_PUBLIC_URL: &str = "http://localhost:8080";

pub fn config_at(settings: Settings, public_url: &str, allow_signup: bool) -> Arc<Config> {
    let mut settings = settings;
    // Real Argon2 costs would make the suite crawl.
    settings.password_kdf.memory_kib = 256;
    settings.password_kdf.iterations = 1;
    Arc::new(Config {
        public_url: public_url.parse().unwrap(),
        allow_signup,
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        database: PgConnectOptions::new(),
        settings,
    })
}

pub fn config(settings: Settings) -> Arc<Config> {
    config_at(settings, DEFAULT_PUBLIC_URL, true)
}

pub fn state(pool: PgPool) -> AppState {
    state_with(pool, config(Settings::default()), "")
}

pub fn state_with(pool: PgPool, config: Arc<Config>, catalog: &str) -> AppState {
    AppState {
        supervisor: Supervisor::new(config.clone()),
        config,
        pool,
        catalog: Arc::new(Catalog::parse(catalog).unwrap()),
        bin_scripts: Arc::new(HashMap::new()),
    }
}

/// The real router on a real port, with helpers for the JSON API.
pub struct TestApp {
    pub address: String,
    pub state: AppState,
    client: reqwest::Client,
}

impl TestApp {
    pub async fn spawn(
        pool: PgPool,
        settings: Settings,
        catalog: &str,
        allow_signup: bool,
    ) -> Self {
        Self::spawn_at(pool, settings, catalog, allow_signup, DEFAULT_PUBLIC_URL).await
    }

    pub async fn spawn_at(
        pool: PgPool,
        settings: Settings,
        catalog: &str,
        allow_signup: bool,
        public_url: &str,
    ) -> Self {
        let state = state_with(pool, config_at(settings, public_url, allow_signup), catalog);
        Self::serve(state).await
    }

    pub async fn serve(state: AppState) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let router = build_router(state.clone());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            address,
            state,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
        }
    }

    pub fn public_url(&self) -> &str {
        self.state.config.public_url.as_str()
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub async fn get(&self, path: &str, cookie: Option<&str>) -> Response {
        let mut request = self.client.get(format!("{}{path}", self.address));
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        request.send().await.unwrap()
    }

    pub async fn send_json(
        &self,
        method: reqwest::Method,
        path: &str,
        cookie: Option<&str>,
        body: Value,
    ) -> Response {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.address))
            .header("origin", self.public_url())
            .header("content-type", "application/json")
            .body(body.to_string());
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        request.send().await.unwrap()
    }

    pub async fn post(&self, path: &str, cookie: Option<&str>, body: Value) -> Response {
        self.send_json(reqwest::Method::POST, path, cookie, body)
            .await
    }

    pub async fn signup(&self, username: &str, password: &str) -> Response {
        let body = json!({ "username": username, "password": password });
        self.post("/api/auth/signup", None, body).await
    }

    /// Logs in and returns the `name=value` pair to send as a `Cookie` header.
    pub async fn login(&self, username: &str, password: &str) -> String {
        let body = json!({ "username": username, "password": password });
        let response = self.post("/api/auth/login", None, body).await;
        assert!(
            response.status().is_success(),
            "login failed: {}",
            response.status()
        );
        let set_cookie = response.headers()["set-cookie"].to_str().unwrap();
        set_cookie.split(';').next().unwrap().to_owned()
    }
}

/// Path of the fake MCP server. A plain `cargo test` builds it; it's built here only
/// for a filtered run that skipped examples.
pub fn fake_mcp_path() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let find = || {
            std::env::current_exe()
                .unwrap()
                .ancestors()
                .map(|directory| directory.join("examples/fake_mcp"))
                .find(|candidate| candidate.exists())
        };
        find().unwrap_or_else(|| {
            let status = Command::new(env!("CARGO"))
                .args(["build", "--quiet", "--example", "fake_mcp"])
                .current_dir(env!("CARGO_MANIFEST_DIR"))
                .status()
                .unwrap();
            assert!(status.success());
            find().expect("fake_mcp should exist under the target directory")
        })
    })
    .clone()
}
