//! Connects MCP clients to child processes. JSON-RPC is forwarded raw: only the
//! fields the hub needs are read, so new protocol methods and fields pass through.

pub mod http;
pub mod stdio;
pub mod tools;

use serde_json::{Value, json};

/// Removes capabilities the bridge can't serve yet. A server-to-client request during
/// a tool call would have nowhere to go, and the server would wait forever (BRIDGE.md).
pub fn strip_client_capabilities(initialize: &mut Value) {
    if let Some(capabilities) = initialize
        .pointer_mut("/params/capabilities")
        .and_then(Value::as_object_mut)
    {
        for capability in ["sampling", "elicitation", "roots"] {
            capabilities.remove(capability);
        }
    }
}

pub fn json_rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_only_unsupported_capabilities() {
        let mut message = json!({
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {
                    "sampling": {},
                    "elicitation": {},
                    "roots": { "listChanged": true },
                    "experimental": { "kept": true },
                },
            },
        });

        strip_client_capabilities(&mut message);

        assert_eq!(
            message["params"]["capabilities"],
            json!({ "experimental": { "kept": true } })
        );
        assert_eq!(message["params"]["protocolVersion"], "2025-11-25");
    }

    #[test]
    fn tolerates_a_message_without_capabilities() {
        let mut message = json!({ "method": "initialize" });
        strip_client_capabilities(&mut message);
        assert_eq!(message, json!({ "method": "initialize" }));
    }
}
