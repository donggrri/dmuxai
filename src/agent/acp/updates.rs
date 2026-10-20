//! Map ACP `session/update` payloads to [`AgentEvent`] (FR-1302).

use serde_json::Value;

use crate::agent::AgentEvent;

pub fn map_session_update(update: &Value) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    let kind = update
        .get("sessionUpdate")
        .or_else(|| update.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");

    match kind {
        "agent_message_chunk" | "agent_message" => {
            if let Some(text) = extract_text(update) {
                events.push(AgentEvent::TextDelta(text));
            }
        }
        "tool_call_update" => {
            let summary = update
                .pointer("/toolCall/title")
                .or_else(|| update.pointer("/toolCall/summary"))
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let state = update
                .pointer("/toolCall/status")
                .and_then(Value::as_str)
                .unwrap_or("in_progress");
            let item_id = update
                .pointer("/toolCall/toolCallId")
                .or_else(|| update.pointer("/toolCall/id"))
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_owned();
            events.push(AgentEvent::ToolProgress {
                item_id,
                summary: summary.to_owned(),
                state: state.to_owned(),
            });
        }
        "plan_update" => {
            let text = update
                .pointer("/plan/content")
                .or_else(|| update.pointer("/content"))
                .and_then(Value::as_str)
                .unwrap_or("plan updated");
            events.push(AgentEvent::Notice(text.to_owned()));
        }
        "notice" => {
            if let Some(text) = update.get("message").and_then(Value::as_str) {
                events.push(AgentEvent::Notice(text.to_owned()));
            }
        }
        _ => {}
    }
    events
}

fn extract_text(update: &Value) -> Option<String> {
    if let Some(text) = update.get("text").and_then(Value::as_str) {
        return Some(text.to_owned());
    }
    update
        .pointer("/content/text")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            update
                .pointer("/content")
                .and_then(Value::as_array)
                .and_then(|blocks| {
                    blocks
                        .iter()
                        .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                        .filter_map(|b| b.get("text").and_then(Value::as_str))
                        .next()
                        .map(str::to_owned)
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // TC-1302
    #[test]
    fn maps_agent_message_chunk() {
        let update = json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "hello"}
        });
        let events = map_session_update(&update);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], AgentEvent::TextDelta("hello".to_owned()));
    }
}
