//! Map ACP `session/update` payloads to [`AgentEvent`] (FR-1302).

use serde_json::Value;

use crate::agent::AgentEvent;
use dmuxai::activity::{ActivityItem, ToolStatus};

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
        "tool_call" => {
            if let Some(item) = map_tool_call(update) {
                events.push(AgentEvent::Activity(item));
            }
        }
        "tool_call_update" => {
            if let Some(item) = map_tool_call_update(update) {
                events.push(AgentEvent::Activity(item));
            }
            let summary = tool_summary(update);
            let state = update
                .get("status")
                .or_else(|| update.pointer("/toolCall/status"))
                .and_then(Value::as_str)
                .unwrap_or("in_progress");
            let item_id = update
                .get("toolCallId")
                .or_else(|| update.pointer("/toolCall/toolCallId"))
                .or_else(|| update.pointer("/toolCall/id"))
                .and_then(Value::as_str)
                .unwrap_or("tool")
                .to_owned();
            events.push(AgentEvent::ToolProgress {
                item_id,
                summary,
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

fn tool_summary(update: &Value) -> String {
    update
        .get("title")
        .or_else(|| update.pointer("/toolCall/title"))
        .or_else(|| update.get("kind"))
        .or_else(|| update.pointer("/toolCall/kind"))
        .and_then(Value::as_str)
        .unwrap_or("tool")
        .to_owned()
}

fn map_tool_call(update: &Value) -> Option<ActivityItem> {
    let title = tool_summary(update);
    let kind = update
        .get("kind")
        .or_else(|| update.pointer("/toolCall/kind"))
        .and_then(Value::as_str)
        .unwrap_or("tool");
    let status = tool_status_from_str(
        update
            .get("status")
            .or_else(|| update.pointer("/toolCall/status"))
            .and_then(Value::as_str)
            .unwrap_or("running"),
    );
    let location = update
        .pointer("/locations/0/path")
        .or_else(|| update.pointer("/toolCall/locations/0/path"))
        .or_else(|| update.get("path"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(ActivityItem::Tool {
        name: kind.to_owned(),
        target: location.unwrap_or_else(|| title.clone()),
        status,
        duration_ms: None,
        detail: title,
    })
}

fn map_tool_call_update(update: &Value) -> Option<ActivityItem> {
    let title = tool_summary(update);
    let status = tool_status_from_str(
        update
            .get("status")
            .or_else(|| update.pointer("/toolCall/status"))
            .and_then(Value::as_str)
            .unwrap_or("running"),
    );
    let location = update
        .pointer("/locations/0/path")
        .or_else(|| update.pointer("/toolCall/locations/0/path"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let diff = update
        .pointer("/diff")
        .or_else(|| update.pointer("/toolCall/diff"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let kind = update
        .get("kind")
        .or_else(|| update.pointer("/toolCall/kind"))
        .and_then(Value::as_str)
        .unwrap_or("tool");
    Some(ActivityItem::Tool {
        name: kind.to_owned(),
        target: location.unwrap_or_else(|| title.clone()),
        status,
        duration_ms: None,
        detail: diff.unwrap_or(title),
    })
}

fn tool_status_from_str(raw: &str) -> ToolStatus {
    match raw.to_ascii_lowercase().as_str() {
        "completed" | "complete" | "done" | "success" => ToolStatus::Ok,
        "failed" | "error" => ToolStatus::Failed,
        _ => ToolStatus::Running,
    }
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

    // TC-1315 / FR-1302: tool_call_update reads top-level fields.
    #[test]
    fn maps_tool_call_update_fields() {
        let update = json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc-1",
            "title": "Read file",
            "kind": "read",
            "status": "completed",
            "locations": [{"path": "src/main.rs"}]
        });
        let events = map_session_update(&update);
        assert!(events.iter().any(
            |e| matches!(e, AgentEvent::ToolProgress { summary, .. } if summary == "Read file")
        ));
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Activity(_))));
    }
}
