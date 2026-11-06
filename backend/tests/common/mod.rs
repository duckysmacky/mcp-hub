#![allow(dead_code)] // Each test crate uses a different part of this module.

use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    process::Command,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU16, Ordering},
    },
};

use mcp_hub::{
    app_state::AppState,
    build_router,
    catalog::{Catalog, RuntimeKind, install::ResolvedServer},
    config::{Config, Settings, SignupMode},
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
    settings.server.signup = if allow_signup {
        SignupMode::Enabled
    } else {
        SignupMode::Disabled
    };
    Arc::new(Config {
        public_url: public_url.parse().unwrap(),
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
    AppState::new(
        config.clone(),
        pool,
        Catalog::parse(catalog).unwrap(),
        HashMap::new(),
        Supervisor::new(config),
    )
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
        let service = router.into_make_service_with_connect_info::<SocketAddr>();
        tokio::spawn(async move { axum::serve(listener, service).await.unwrap() });
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

/// An app whose catalog `runtime` is the fake MCP server, so nothing real is needed.
pub struct McpApp {
    pub app: TestApp,
    _scratch: tempfile::TempDir,
}

impl std::ops::Deref for McpApp {
    type Target = TestApp;

    fn deref(&self) -> &TestApp {
        &self.app
    }
}

impl McpApp {
    pub async fn spawn(pool: PgPool, catalog: &str, customize: impl FnOnce(&mut Settings)) -> Self {
        let scratch = tempfile::tempdir().unwrap();
        let mut settings = Settings::default();
        settings.runtimes.bun = fake_mcp_path();
        settings.runtimes.node = fake_mcp_path();
        settings.paths.scratch = scratch.path().to_owned();
        settings.limits.max_spawns_per_user_per_minute = 1000;
        // Tests in one binary run concurrently, so each app gets its own ports.
        static NEXT_PORT_BLOCK: AtomicU16 = AtomicU16::new(0);
        let block = NEXT_PORT_BLOCK.fetch_add(1, Ordering::SeqCst);
        let first_port = 43000 + block * 4;
        settings.http_mode.port_range = [first_port, first_port + 3];
        customize(&mut settings);

        let parsed = Catalog::parse(catalog).unwrap();
        let bin_scripts = parsed
            .servers()
            .map(|server| {
                let script = match server.runtime {
                    RuntimeKind::Binary => PathBuf::from(&server.package),
                    _ => PathBuf::from("ignored.js"),
                };
                let resolved = ResolvedServer { script };
                (server.id.clone(), resolved)
            })
            .collect();
        let mut state = state_with(pool, config(settings), catalog);
        state.bin_scripts = Arc::new(bin_scripts);
        Self {
            app: TestApp::serve(state).await,
            _scratch: scratch,
        }
    }

    /// Signs up a user and creates a started instance with the given env values.
    /// Returns the user's cookie and the instance id.
    pub async fn user_with_instance(
        &self,
        username: &str,
        slug: &str,
        server_id: &str,
        env: &[(&str, &str)],
    ) -> (String, i64) {
        let cookie = match self
            .signup(username, "correct horse battery")
            .await
            .status()
        {
            reqwest::StatusCode::CREATED | reqwest::StatusCode::CONFLICT => {
                self.login(username, "correct horse battery").await
            }
            status => panic!("signup failed: {status}"),
        };
        let created = self
            .post(
                "/api/instances",
                Some(&cookie),
                json!({ "server_id": server_id, "slug": slug }),
            )
            .await;
        assert_eq!(created.status(), reqwest::StatusCode::CREATED);
        let id = created.json::<Value>().await.unwrap()["id"]
            .as_i64()
            .unwrap();
        for (var, value) in env {
            let response = self
                .send_json(
                    reqwest::Method::PUT,
                    &format!("/api/instances/{id}/env/{var}"),
                    Some(&cookie),
                    json!({ "value": value }),
                )
                .await;
            assert_eq!(response.status(), reqwest::StatusCode::NO_CONTENT, "{var}");
        }
        self.post(
            &format!("/api/instances/{id}/start"),
            Some(&cookie),
            json!({}),
        )
        .await;
        (cookie, id)
    }

    pub async fn mcp_post(
        &self,
        slug: &str,
        token: Option<&str>,
        session: Option<&str>,
        body: &Value,
    ) -> Response {
        let mut request = self
            .client()
            .post(format!("{}/mcp/{slug}", self.address))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(body.to_string());
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(session) = session {
            request = request.header("mcp-session-id", session);
        }
        request.send().await.unwrap()
    }

    /// Initializes a session and returns its id and the `initialize` result.
    pub async fn initialize(&self, slug: &str, token: &str) -> (String, Value) {
        let response = self
            .mcp_post(slug, Some(token), None, &initialize_message())
            .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let session = response.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        (session, read_json_rpc(response).await)
    }

    /// Calls a tool of the fake server and returns its text output.
    pub async fn call_tool(
        &self,
        slug: &str,
        token: &str,
        session: &str,
        tool: &str,
        arguments: Value,
    ) -> Response {
        self.mcp_post(
            slug,
            Some(token),
            Some(session),
            &tool_call(tool, arguments),
        )
        .await
    }
}

pub fn initialize_message() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": { "sampling": {}, "elicitation": {}, "roots": {}, "experimental": {} },
            "clientInfo": { "name": "test", "version": "1" },
        },
    })
}

pub fn tool_call(tool: &str, arguments: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": { "name": tool, "arguments": arguments },
    })
}

/// Reads a JSON-RPC message from a JSON body or from an SSE `data:` line.
pub async fn read_json_rpc(response: Response) -> Value {
    let text = response.text().await.unwrap();
    let payload = text
        .lines()
        .find_map(|line| line.strip_prefix("data:"))
        .unwrap_or(&text);
    serde_json::from_str(payload.trim()).unwrap()
}

pub fn tool_text(message: &Value) -> &str {
    message["result"]["content"][0]["text"].as_str().unwrap()
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
