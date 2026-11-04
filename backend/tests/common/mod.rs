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

pub const CLIENT_REDIRECT: &str = "http://127.0.0.1:43123/callback";

pub async fn register_client(app: &TestApp) -> String {
    let response = app
        .client()
        .post(format!("{}/oauth/register", app.address))
        .json(&json!({ "client_name": "Test Client", "redirect_uris": [CLIENT_REDIRECT] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    let body: Value = response.json().await.unwrap();
    body["client_id"].as_str().unwrap().to_owned()
}

/// Runs `/oauth/authorize` and returns the pending id from the consent redirect.
pub async fn start_authorization(
    app: &TestApp,
    client_id: &str,
    slug: &str,
    challenge: &str,
) -> String {
    let resource = format!("{}/mcp/{slug}", app.public_url());
    let response = app
        .client()
        .get(format!("{}/oauth/authorize", app.address))
        .query(&[
            ("client_id", client_id),
            ("redirect_uri", CLIENT_REDIRECT),
            ("response_type", "code"),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("scope", "mcp"),
            ("resource", &resource),
            ("state", "client-state"),
        ])
        .send()
        .await
        .unwrap();
    assert!(response.status().is_redirection(), "{}", response.status());
    let location = response.headers()["location"].to_str().unwrap();
    location
        .strip_prefix("/consent?request=")
        .unwrap_or_else(|| panic!("unexpected redirect: {location}"))
        .to_owned()
}

pub const PKCE_VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
pub const PKCE_CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

/// Approves a pending authorization and returns the code from the redirect.
pub async fn approve(app: &TestApp, cookie: &str, pending: &str) -> String {
    let response = app
        .post(
            &format!("/api/oauth/pending/{pending}/approve"),
            Some(cookie),
            json!({}),
        )
        .await;
    assert!(response.status().is_success(), "{}", response.status());
    let body: Value = response.json().await.unwrap();
    let location = url::Url::parse(body["redirect_url"].as_str().unwrap()).unwrap();
    location
        .query_pairs()
        .find(|(name, _)| name == "code")
        .unwrap()
        .1
        .into_owned()
}

pub async fn post_token(app: &TestApp, form: &[(&str, &str)]) -> Response {
    app.client()
        .post(format!("{}/oauth/token", app.address))
        .form(form)
        .send()
        .await
        .unwrap()
}

pub async fn exchange_code(app: &TestApp, client_id: &str, code: &str, slug: &str) -> Response {
    let resource = format!("{}/mcp/{slug}", app.public_url());
    post_token(
        app,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", CLIENT_REDIRECT),
            ("client_id", client_id),
            ("code_verifier", PKCE_VERIFIER),
            ("resource", &resource),
        ],
    )
    .await
}

pub struct IssuedTokens {
    pub client_id: String,
    pub access_token: String,
    pub refresh_token: String,
}

/// The whole OAuth flow, as a client and the user would drive it.
pub async fn connect(app: &TestApp, cookie: &str, slug: &str) -> IssuedTokens {
    let client_id = register_client(app).await;
    let pending = start_authorization(app, &client_id, slug, PKCE_CHALLENGE).await;
    let code = approve(app, cookie, &pending).await;
    let response = exchange_code(app, &client_id, &code, slug).await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    IssuedTokens {
        client_id,
        access_token: body["access_token"].as_str().unwrap().to_owned(),
        refresh_token: body["refresh_token"].as_str().unwrap().to_owned(),
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
