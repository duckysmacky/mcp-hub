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
            "echo" => arguments.to_string(),
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
                json!({ "tools": [{ "name": "echo", "inputSchema": { "type": "object" } }] })
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

#[tokio::main]
async fn main() {
    if let Ok(code) = std::env::var("FAKE_EXIT_AT_START") {
        eprintln!("exiting at start as asked");
        std::process::exit(code.parse().unwrap());
    }
    run_stdio(Server::default()).await;
}
