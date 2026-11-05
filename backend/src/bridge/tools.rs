//! Per-instance tool filtering (BRIDGE.md). A disabled tool is taken out of `tools/list`
//! results, in plain JSON or in an SSE stream. A list this can't inspect is never passed on.

use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_RECORDED_TOOLS: usize = 1000;
const MAX_TOOL_NAME_CHARS: usize = 256;
const MAX_TOOL_DESCRIPTION_CHARS: usize = 2048;

/// What the UI needs to know about a tool. Comes from the server, so it's only displayed.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ToolSummary {
    pub name: String,
    pub description: Option<String>,
}

/// Removes disabled tools from a `tools/list` response, and returns every tool it listed
/// before that. `None` if the message has no tool list, such as a JSON-RPC error.
pub fn filter_tool_list(message: &mut Value, disabled: &[String]) -> Option<Vec<ToolSummary>> {
    let tools = message
        .get_mut("result")?
        .get_mut("tools")?
        .as_array_mut()?;
    let seen = tools
        .iter()
        .filter_map(|tool| {
            Some(ToolSummary {
                name: bounded(tool.get("name")?.as_str()?, MAX_TOOL_NAME_CHARS),
                description: tool
                    .get("description")
                    .and_then(Value::as_str)
                    .map(|text| bounded(text, MAX_TOOL_DESCRIPTION_CHARS)),
            })
        })
        .take(MAX_RECORDED_TOOLS)
        .collect();
    tools.retain(|tool| {
        tool.get("name")
            .and_then(Value::as_str)
            .is_none_or(|name| !disabled.iter().any(|disabled_name| disabled_name == name))
    });
    Some(seen)
}

fn bounded(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

/// A `tools/list` response's SSE stream, filtered event by event.
pub struct SseToolFilter {
    buffer: Vec<u8>,
    disabled: Vec<String>,
    max_event_bytes: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct FilteredChunk {
    /// Complete events, safe to send on.
    pub bytes: Vec<u8>,
    pub seen: Option<Vec<ToolSummary>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct EventTooLarge;

impl SseToolFilter {
    pub fn new(disabled: Vec<String>, max_event_bytes: usize) -> Self {
        Self {
            buffer: Vec::new(),
            disabled,
            max_event_bytes,
        }
    }

    /// Takes the next bytes of the stream and returns the events they completed. Anything
    /// left over at the end of the stream is never sent: it can't have been inspected.
    pub fn push(&mut self, chunk: &[u8]) -> Result<FilteredChunk, EventTooLarge> {
        self.buffer.extend_from_slice(chunk);
        let mut output = FilteredChunk::default();
        while let Some(end) = event_end(&self.buffer) {
            if end > self.max_event_bytes {
                return Err(EventTooLarge);
            }
            let event: Vec<u8> = self.buffer.drain(..end).collect();
            let (bytes, seen) = self.filter_event(&event);
            output.bytes.extend_from_slice(&bytes);
            if seen.is_some() {
                output.seen = seen;
            }
        }
        if self.buffer.len() > self.max_event_bytes {
            return Err(EventTooLarge);
        }
        Ok(output)
    }

    /// Rewrites one event if it holds a tool list. Other events go out byte for byte.
    fn filter_event(&self, event: &[u8]) -> (Vec<u8>, Option<Vec<ToolSummary>>) {
        let Ok(text) = std::str::from_utf8(event) else {
            return (Vec::new(), None);
        };
        let mut other_lines = Vec::new();
        let mut data = Vec::new();
        for line in text.lines() {
            match line.strip_prefix("data:") {
                Some(value) => data.push(value.strip_prefix(' ').unwrap_or(value)),
                None if !line.is_empty() => other_lines.push(line),
                None => {}
            }
        }
        if data.is_empty() {
            return (event.to_vec(), None);
        }
        // Can't be read, so it can't be checked: drop it rather than risk passing a list.
        let Ok(mut message) = serde_json::from_str::<Value>(&data.join("\n")) else {
            return (Vec::new(), None);
        };
        let Some(seen) = filter_tool_list(&mut message, &self.disabled) else {
            return (event.to_vec(), None);
        };
        let mut rewritten = String::new();
        for line in other_lines {
            rewritten.push_str(line);
            rewritten.push('\n');
        }
        rewritten.push_str("data: ");
        rewritten.push_str(&message.to_string());
        rewritten.push_str("\n\n");
        (rewritten.into_bytes(), Some(seen))
    }
}

/// Where the first complete event ends. SSE ends one with a blank line.
fn event_end(buffer: &[u8]) -> Option<usize> {
    let newline = buffer
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|start| start + 2);
    let carriage_return = buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|start| start + 4);
    match (newline, carriage_return) {
        (Some(first), Some(second)) => Some(first.min(second)),
        (first, second) => first.or(second),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn tool_list() -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "tools": [
                    { "name": "keep", "description": "stays", "inputSchema": {} },
                    { "name": "drop", "inputSchema": {} },
                ],
                "nextCursor": "more",
            },
        })
    }

    fn disabled() -> Vec<String> {
        vec!["drop".to_owned()]
    }

    #[test]
    fn disabled_tools_leave_the_list_and_stay_in_the_summary() {
        let mut message = tool_list();
        let seen = filter_tool_list(&mut message, &disabled()).unwrap();
        assert_eq!(
            seen,
            [
                ToolSummary {
                    name: "keep".to_owned(),
                    description: Some("stays".to_owned()),
                },
                ToolSummary {
                    name: "drop".to_owned(),
                    description: None,
                },
            ]
        );
        assert_eq!(message["result"]["tools"].as_array().unwrap().len(), 1);
        assert_eq!(message["result"]["tools"][0]["name"], "keep");
        assert_eq!(message["result"]["nextCursor"], "more");
    }

    #[test]
    fn messages_without_a_tool_list_are_left_alone() {
        let mut error = json!({ "id": 1, "error": { "code": 1, "message": "no" } });
        let before = error.clone();
        assert!(filter_tool_list(&mut error, &disabled()).is_none());
        assert_eq!(error, before);
    }

    #[test]
    fn long_text_is_bounded_in_the_summary() {
        let mut message = json!({ "result": { "tools": [
            { "name": "a".repeat(10_000), "description": "b".repeat(10_000) },
        ] } });
        let seen = filter_tool_list(&mut message, &[]).unwrap();
        assert_eq!(seen[0].name.len(), MAX_TOOL_NAME_CHARS);
        assert_eq!(
            seen[0].description.as_ref().unwrap().len(),
            MAX_TOOL_DESCRIPTION_CHARS
        );
    }

    fn event_of(message: &Value) -> Vec<u8> {
        format!("event: message\ndata: {message}\n\n").into_bytes()
    }

    fn data_of(bytes: &[u8]) -> Value {
        let text = std::str::from_utf8(bytes).unwrap();
        let line = text.lines().find_map(|line| line.strip_prefix("data: "));
        serde_json::from_str(line.unwrap()).unwrap()
    }

    #[test]
    fn sse_lists_are_filtered_even_when_split_across_chunks() {
        let event = event_of(&tool_list());
        let mut filter = SseToolFilter::new(disabled(), 4096);
        let (first, second) = event.split_at(event.len() / 2);

        assert_eq!(filter.push(first).unwrap(), FilteredChunk::default());
        let output = filter.push(second).unwrap();

        assert_eq!(output.seen.unwrap().len(), 2);
        let text = String::from_utf8(output.bytes.clone()).unwrap();
        assert!(text.starts_with("event: message\n"), "{text}");
        assert!(!text.contains("drop"), "{text}");
        assert_eq!(data_of(&output.bytes)["result"]["tools"][0]["name"], "keep");
    }

    #[test]
    fn multi_line_data_and_crlf_are_understood() {
        let event = "data: {\"result\":\r\ndata: {\"tools\":[{\"name\":\"drop\"}]}}\r\n\r\n";
        let mut filter = SseToolFilter::new(disabled(), 4096);
        let output = filter.push(event.as_bytes()).unwrap();
        assert_eq!(output.seen.unwrap()[0].name, "drop");
        assert_eq!(data_of(&output.bytes)["result"]["tools"], json!([]));
    }

    #[test]
    fn other_events_pass_byte_for_byte() {
        let event = b"event: ping\n: comment\n\ndata: {\"method\":\"notifications/x\"}\n\n";
        let mut filter = SseToolFilter::new(disabled(), 4096);
        let output = filter.push(event).unwrap();
        assert_eq!(output.bytes, event);
        assert!(output.seen.is_none());
    }

    #[test]
    fn what_cannot_be_inspected_is_never_passed_on() {
        let mut filter = SseToolFilter::new(disabled(), 4096);
        let unparsable = filter
            .push(b"data: {\"result\":{\"tools\":[{\"name\":\"drop\"\n\n")
            .unwrap();
        assert!(unparsable.bytes.is_empty());

        // An event the stream never finishes stays buffered and is dropped with the filter.
        let unfinished = filter.push(&event_of(&tool_list())[..20]).unwrap();
        assert!(unfinished.bytes.is_empty());

        let mut small = SseToolFilter::new(disabled(), 8);
        assert_eq!(small.push(b"data: 0123456789"), Err(EventTooLarge));
        let mut small = SseToolFilter::new(disabled(), 8);
        assert_eq!(small.push(b"data: 0123456789\n\n"), Err(EventTooLarge));
    }
}
