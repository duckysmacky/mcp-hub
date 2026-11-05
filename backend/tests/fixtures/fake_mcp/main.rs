//! A small MCP server for tests, so the suite never needs Bun, Node, or the network.
//! Tests point the catalog's runtime at this binary; argv[1] (the script) is ignored.

use std::{
    collections::BTreeMap,
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

type Stdout = Arc<Mutex<std::io::Stdout>>;

#[derive(Clone, Default)]
struct Server {
    cancelled: Arc<Mutex<Vec<Value>>>,
}

impl Server {
    /// Runs a tool and returns its text output.
    async fn call_tool(&self, name: &str, arguments: &Value, stdout: Option<&Stdout>) -> String {
        match name {
            "echo" | "danger" => arguments.to_string(),
            "exit" => std::process::exit(arguments["code"].as_i64().unwrap_or(1) as i32),
            "env" => {
                let env: BTreeMap<String, String> = std::env::vars().collect();
                serde_json::to_string(&env).unwrap()
            }
            "cwd" => std::env::current_dir().unwrap().display().to_string(),
            "crash" => std::process::exit(3),
            "slow" => {
                let millis = arguments["ms"].as_u64().unwrap_or(1000);
                tokio::time::sleep(Duration::from_millis(millis)).await;
                "done".to_owned()
            }
            "cancelled" => serde_json::to_string(&*self.cancelled.lock().unwrap()).unwrap(),
            "stderr" => {
                eprintln!("{}", arguments["text"].as_str().unwrap_or_default());
                "written".to_owned()
            }
            "stderr_spam" => {
                let line = "x".repeat(1023);
                for _ in 0..arguments["kib"].as_u64().unwrap_or(1024) {
                    eprintln!("{line}");
                }
                "spammed".to_owned()
            }
            "long_line" => {
                let bytes = arguments["bytes"].as_u64().unwrap_or(1024) as usize;
                write_line(stdout.unwrap(), &"x".repeat(bytes));
                "written".to_owned()
            }
            "noise" => {
                write_line(stdout.unwrap(), "this is not json");
                "written".to_owned()
            }
            "notify" => {
                let message = json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/message",
                    "params": { "level": "info", "data": arguments["text"] },
                });
                write_line(stdout.unwrap(), &message.to_string());
                "notified".to_owned()
            }
            other => panic!("unknown tool {other}"),
        }
    }

    /// Answers a JSON-RPC request; notifications return `None`.
    async fn handle(&self, message: &Value, stdout: Option<&Stdout>) -> Option<Value> {
        let method = message["method"].as_str()?;
        if method == "notifications/cancelled" {
            let id = message["params"]["requestId"].clone();
            self.cancelled.lock().unwrap().push(id);
            return None;
        }
        let id = message.get("id")?;
        let result = match method {
            "initialize" => json!({
                "protocolVersion": message["params"]["protocolVersion"],
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "fake-mcp", "version": "1.0.0" },
                "receivedCapabilities": message["params"]["capabilities"],
            }),
            "ping" => json!({}),
            "tools/list" => {
                json!({ "tools": [
                    { "name": "echo", "description": "Says it back", "inputSchema": { "type": "object" } },
                    { "name": "danger", "inputSchema": { "type": "object" } },
                ] })
            }
            "tools/call" => {
                let name = message["params"]["name"].as_str().unwrap_or_default();
                let text = self
                    .call_tool(name, &message["params"]["arguments"], stdout)
                    .await;
                json!({ "content": [{ "type": "text", "text": text }] })
            }
            _ => {
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": "method not found" },
                }));
            }
        };
        Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
    }
}

fn write_line(stdout: &Stdout, line: &str) {
    let mut stdout = stdout.lock().unwrap();
    stdout.write_all(line.as_bytes()).unwrap();
    stdout.write_all(b"\n").unwrap();
    stdout.flush().unwrap();
}

async fn run_stdio(server: Server) {
    let stdout: Stdout = Arc::new(Mutex::new(std::io::stdout()));
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let message: Value = serde_json::from_str(&line).unwrap();
        let (server, stdout) = (server.clone(), stdout.clone());
        tokio::spawn(async move {
            if let Some(response) = server.handle(&message, Some(&stdout)).await {
                write_line(&stdout, &response.to_string());
            }
        });
    }
}

/// HTTP mode: serves `POST`/`GET`/`DELETE /mcp` like a stateful Streamable HTTP server.
mod http {
    use std::{
        collections::HashSet,
        convert::Infallible,
        sync::atomic::{AtomicU64, Ordering},
    };

    use axum::{
        Json, Router,
        body::Bytes,
        extract::State,
        http::{HeaderMap, StatusCode},
        response::{
            IntoResponse, Response,
            sse::{Event, Sse},
        },
        routing::post,
    };
    use futures::{StreamExt, stream};

    use super::*;

    #[derive(Clone, Default)]
    struct App {
        server: Server,
        sessions: Arc<Mutex<HashSet<String>>>,
    }

    static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);

    fn known_session(app: &App, headers: &HeaderMap) -> Result<(), StatusCode> {
        let session = headers
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .ok_or(StatusCode::BAD_REQUEST)?;
        if app.sessions.lock().unwrap().contains(session) {
            Ok(())
        } else {
            Err(StatusCode::NOT_FOUND)
        }
    }

    fn seen_headers(headers: &HeaderMap) -> String {
        let seen: BTreeMap<&str, &str> = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.to_str().unwrap_or("?")))
            .collect();
        serde_json::to_string(&seen).unwrap()
    }

    fn text_result(id: &Value, text: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "content": [{ "type": "text", "text": text }] },
        })
    }

    async fn handle_post(State(app): State<App>, headers: HeaderMap, body: Bytes) -> Response {
        if std::env::var("FAKE_REQUIRE_AUTH").is_ok() && !headers.contains_key("authorization") {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let message: Value = serde_json::from_slice(&body).unwrap();
        if message["method"] == "initialize" {
            let session = format!("upstream-{}", NEXT_SESSION.fetch_add(1, Ordering::SeqCst));
            app.sessions.lock().unwrap().insert(session.clone());
            let response = app.server.handle(&message, None).await.unwrap();
            return ([("mcp-session-id", session)], Json(response)).into_response();
        }
        if let Err(status) = known_session(&app, &headers) {
            return status.into_response();
        }
        if message["method"] == "tools/list" && std::env::var("FAKE_TOOLS_SSE").is_ok() {
            let response = app.server.handle(&message, None).await.unwrap();
            let events = stream::iter([Ok::<_, Infallible>(
                Event::default().event("message").data(response.to_string()),
            )]);
            return Sse::new(events).into_response();
        }
        let tool = message["params"]["name"].as_str().unwrap_or_default();
        match tool {
            "headers" => Json(text_result(&message["id"], &seen_headers(&headers))).into_response(),
            "sse" => {
                let result = text_result(&message["id"], "streamed").to_string();
                let events = stream::iter([Ok::<_, Infallible>(
                    Event::default().event("message").data(result),
                )]);
                Sse::new(events).into_response()
            }
            "redirect" => (
                StatusCode::FOUND,
                [("location", "http://127.0.0.1:1/stolen")],
            )
                .into_response(),
            "response_headers" => (
                [("set-cookie", "upstream=1"), ("x-internal", "secret")],
                Json(text_result(&message["id"], "with headers")),
            )
                .into_response(),
            _ => match app.server.handle(&message, None).await {
                Some(response) => Json(response).into_response(),
                None => StatusCode::ACCEPTED.into_response(),
            },
        }
    }

    async fn handle_get(State(app): State<App>, headers: HeaderMap) -> Response {
        if let Err(status) = known_session(&app, &headers) {
            return status.into_response();
        }
        let message = json!({ "jsonrpc": "2.0", "method": "notifications/message" }).to_string();
        let first = stream::iter([Ok::<_, Infallible>(
            Event::default().event("message").data(message),
        )]);
        Sse::new(first.chain(stream::pending())).into_response()
    }

    async fn handle_delete(State(app): State<App>, headers: HeaderMap) -> Response {
        if let Err(status) = known_session(&app, &headers) {
            return status.into_response();
        }
        let session = headers["mcp-session-id"].to_str().unwrap();
        app.sessions.lock().unwrap().remove(session);
        StatusCode::NO_CONTENT.into_response()
    }

    pub async fn run(port: u16) {
        println!("listening on {port}");
        let router = Router::new()
            .route(
                "/mcp",
                post(handle_post).get(handle_get).delete(handle_delete),
            )
            .with_state(App::default());
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .unwrap();
        axum::serve(listener, router).await.unwrap();
    }
}

#[tokio::main]
async fn main() {
    if std::env::var("FAKE_NO_LISTEN").is_ok() {
        std::future::pending::<()>().await;
    }
    if let Ok(port) = std::env::var("FAKE_HTTP_PORT") {
        http::run(port.parse().unwrap()).await;
        return;
    }
    if let Ok(code) = std::env::var("FAKE_EXIT_AT_START") {
        eprintln!("exiting at start as asked");
        std::process::exit(code.parse().unwrap());
    }
    run_stdio(Server::default()).await;
}
