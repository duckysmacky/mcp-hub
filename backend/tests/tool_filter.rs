use common::{McpApp, connect, read_json_rpc, tool_text};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sqlx::PgPool;

mod common;

const STDIO_CATALOG: &str = r#"
[[server]]
id = "demo"
mode = "stdio"
runtime = "bun"
package = "demo@1.0.0"
"#;

const HTTP_CATALOG: &str = r#"
[[server]]
id = "demo"
mode = "http"
runtime = "bun"
package = "demo@1.0.0"

[[server.env]]
name = "FAKE_TOOLS_SSE"
secret = false

[server.http]
port_env = "FAKE_HTTP_PORT"
path = "/mcp"
"#;

struct Connected {
    app: McpApp,
    cookie: String,
    id: i64,
    token: String,
    session: String,
}

async fn start(
    pool: PgPool,
    catalog: &str,
    env: &[(&str, &str)],
    max_line_bytes: usize,
) -> Connected {
    let app = McpApp::spawn(pool, catalog, |settings| {
        settings.limits.max_line_bytes = max_line_bytes;
    })
    .await;
    let (cookie, id) = app.user_with_instance("alice", "demo", "demo", env).await;
    let token = connect(&app, &cookie, "demo").await.access_token;
    let (session, _) = app.initialize("demo", &token).await;
    Connected {
        app,
        cookie,
        id,
        token,
        session,
    }
}

impl Connected {
    async fn disable(&self, tools: &[&str]) -> StatusCode {
        self.app
            .send_json(
                Method::PATCH,
                &format!("/api/instances/{}", self.id),
                Some(&self.cookie),
                json!({ "disabled_tools": tools }),
            )
            .await
            .status()
    }

    async fn list_response(&self) -> reqwest::Response {
        let message = json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/list" });
        self.app
            .mcp_post("demo", Some(&self.token), Some(&self.session), &message)
            .await
    }

    async fn listed(&self) -> Vec<String> {
        let message = read_json_rpc(self.list_response().await).await;
        message["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("no tools in {message}"))
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect()
    }

    async fn call(&self, tool: &str) -> Value {
        let response = self
            .app
            .call_tool("demo", &self.token, &self.session, tool, json!({ "a": 1 }))
            .await;
        read_json_rpc(response).await
    }

    async fn status_of_tools(&self) -> Value {
        self.app
            .get(
                &format!("/api/instances/{}/tools", self.id),
                Some(&self.cookie),
            )
            .await
            .json()
            .await
            .unwrap()
    }

    /// Checks the whole behaviour against whichever transport this app uses.
    async fn check_filtering(&self) {
        assert_eq!(self.status_of_tools().await, json!([]));
        assert_eq!(self.listed().await, ["echo", "danger"]);
        assert!(tool_text(&self.call("danger").await).contains("\"a\""));

        // No restart: it applies to the session that's already open.
        assert_eq!(self.disable(&["danger"]).await, StatusCode::OK);
        assert_eq!(self.listed().await, ["echo"]);

        let refused = self.call("danger").await;
        assert_eq!(refused["error"]["code"], -32602);
        assert_eq!(refused["id"], 2);
        assert!(tool_text(&self.call("echo").await).contains("\"a\""));

        // The UI still sees the disabled tool, so it can be turned back on.
        assert_eq!(
            self.status_of_tools().await,
            json!([
                { "name": "danger", "description": null, "disabled": true },
                { "name": "echo", "description": "Says it back", "disabled": false },
            ])
        );

        assert_eq!(self.disable(&[]).await, StatusCode::OK);
        assert_eq!(self.listed().await, ["echo", "danger"]);
    }
}

#[sqlx::test]
async fn stdio_tools_can_be_disabled(pool: PgPool) {
    let connected = start(pool, STDIO_CATALOG, &[], 8_388_608).await;
    connected.check_filtering().await;
}

#[sqlx::test]
async fn http_json_tools_can_be_disabled(pool: PgPool) {
    let connected = start(pool, HTTP_CATALOG, &[], 8_388_608).await;
    connected.check_filtering().await;
}

#[sqlx::test]
async fn http_sse_tools_can_be_disabled(pool: PgPool) {
    let connected = start(pool, HTTP_CATALOG, &[("FAKE_TOOLS_SSE", "1")], 8_388_608).await;
    connected.check_filtering().await;
    let response = connected.list_response().await;
    assert!(
        response.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
}

#[sqlx::test]
async fn an_oversized_json_list_is_refused(pool: PgPool) {
    let connected = start(pool, HTTP_CATALOG, &[], 20).await;
    connected.disable(&["danger"]).await;
    let response = connected.list_response().await;
    let text = response.text().await.unwrap();
    assert!(!text.contains("danger"), "{text}");
    assert!(text.contains("too large"), "{text}");
}

#[sqlx::test]
async fn an_oversized_sse_list_is_cut_off(pool: PgPool) {
    let connected = start(pool, HTTP_CATALOG, &[("FAKE_TOOLS_SSE", "1")], 20).await;
    connected.disable(&["danger"]).await;
    // The request fails or its body ends early, and what arrived holds no tool.
    let sent = connected
        .app
        .client()
        .post(format!("{}/mcp/demo", connected.app.address))
        .bearer_auth(&connected.token)
        .header("mcp-session-id", &connected.session)
        .json(&json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/list" }))
        .send()
        .await;
    let text = match sent {
        Ok(response) => response.text().await.unwrap_or_default(),
        Err(_) => String::new(),
    };
    assert!(!text.contains("danger"), "{text}");
    assert!(!text.contains("echo"), "{text}");
}

#[sqlx::test]
async fn a_disabled_notification_is_dropped(pool: PgPool) {
    let connected = start(pool, STDIO_CATALOG, &[], 8_388_608).await;
    connected.disable(&["danger"]).await;
    let message =
        json!({ "jsonrpc": "2.0", "method": "tools/call", "params": { "name": "danger" } });
    let response = connected
        .app
        .mcp_post(
            "demo",
            Some(&connected.token),
            Some(&connected.session),
            &message,
        )
        .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
}

#[sqlx::test]
async fn filter_settings_are_bounded_and_owner_only(pool: PgPool) {
    let connected = start(pool, STDIO_CATALOG, &[], 8_388_608).await;
    let too_many: Vec<String> = (0..1001).map(|index| format!("tool{index}")).collect();
    let body = json!({ "disabled_tools": too_many });
    let path = format!("/api/instances/{}", connected.id);
    let refused = connected
        .app
        .send_json(Method::PATCH, &path, Some(&connected.cookie), body)
        .await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(connected.disable(&[""]).await, StatusCode::BAD_REQUEST);

    connected
        .app
        .signup("mallory", "correct horse battery")
        .await;
    let mallory = connected
        .app
        .login("mallory", "correct horse battery")
        .await;
    let tools_path = format!("/api/instances/{}/tools", connected.id);
    assert_eq!(
        connected
            .app
            .get(&tools_path, Some(&mallory))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let patched = connected
        .app
        .send_json(
            Method::PATCH,
            &path,
            Some(&mallory),
            json!({ "disabled_tools": [] }),
        )
        .await;
    assert_eq!(patched.status(), StatusCode::NOT_FOUND);
}
