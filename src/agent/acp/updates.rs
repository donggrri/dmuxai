//! Map ACP `session/update` payloads to [`AgentEvent`] (FR-1302).

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::agent::AgentEvent;
use dmuxai::activity::{ActivityItem, ToolStatus};

#[derive(Debug, Default)]
pub struct ToolCallCache {
    entries: HashMap<String, CachedToolCall>,
}

#[derive(Debug, Clone)]
struct CachedToolCall {
    kind: String,
    title: String,
    location: Option<String>,
    command: Option<String>,
    activity_emitted: bool,
}

pub fn map_session_update(update: &Value, cache: &mut ToolCallCache) -> Vec<AgentEvent> {
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
        "tool_call" | "tool_call_update" => {
            let merged = cache.merge(update);
            let status = tool_status_str(&merged);
            if let Some(item) = map_tool_call_update(&merged) {
                let id = tool_call_id(&merged).unwrap_or_default();
                let terminal = matches!(
                    status.to_ascii_lowercase().as_str(),
                    "completed" | "complete" | "done" | "success" | "failed" | "error"
                );
                if terminal && cache.mark_activity_emitted(&id) {
                    events.push(AgentEvent::Activity(item));
                }
            } else if should_emit_tool_progress(&merged, cache) {
                let summary = tool_summary(&merged);
                let item_id = tool_call_id(&merged).unwrap_or_else(|| "tool".to_owned());
                events.push(AgentEvent::ToolProgress {
                    item_id,
                    summary,
                    state: status,
                });
            }
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

impl ToolCallCache {
    fn merge(&mut self, update: &Value) -> Value {
        let id = tool_call_id(update).unwrap_or_else(|| "tool".to_owned());
        let entry = self
            .entries
            .entry(id.clone())
            .or_insert_with(|| CachedToolCall {
                kind: "tool".to_owned(),
                title: "tool".to_owned(),
                location: None,
                command: None,
                activity_emitted: false,
            });
        let kind = read_field(update, "kind");
        if !kind.is_empty() && !kind.eq_ignore_ascii_case("tool") {
            entry.kind = kind;
        }
        let title = tool_summary(update);
        if !title.eq_ignore_ascii_case("tool") {
            entry.title = title;
        }
        let location = update
            .pointer("/locations/0/path")
            .or_else(|| update.pointer("/toolCall/locations/0/path"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        if location.is_some() {
            entry.location = location;
        }
        if let Some(command) = raw_command(update) {
            entry.command = Some(command);
        }
        let status = tool_status_str(update);
        json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": id,
            "kind": entry.kind,
            "title": entry.title,
            "status": status,
            "locations": entry.location.as_ref().map(|path| json!([{"path": path}])),
            "command": entry.command
        })
    }

    fn mark_activity_emitted(&mut self, id: &str) -> bool {
        let entry = self.entries.get_mut(id);
        if let Some(entry) = entry {
            if entry.activity_emitted {
                return false;
            }
            entry.activity_emitted = true;
            return true;
        }
        true
    }

    fn cached_title(&self, id: &str) -> Option<String> {
        self.entries
            .get(id)
            .filter(|e| !e.title.eq_ignore_ascii_case("tool"))
            .map(|e| e.title.clone())
    }
}

fn read_field(update: &Value, field: &str) -> String {
    update
        .get(field)
        .or_else(|| update.pointer(&format!("/toolCall/{field}")))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

/// `rawInput.command` as a string (joined when the agent sends argv).
fn raw_command(update: &Value) -> Option<String> {
    let raw = update
        .pointer("/rawInput/command")
        .or_else(|| update.pointer("/toolCall/rawInput/command"))?;
    let command = match raw {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(" "),
        _ => return None,
    };
    let command = command.trim().to_owned();
    (!command.is_empty()).then_some(command)
}

fn tool_call_id(update: &Value) -> Option<String> {
    update
        .get("toolCallId")
        .or_else(|| update.pointer("/toolCall/toolCallId"))
        .or_else(|| update.pointer("/toolCall/id"))
        .and_then(Value::as_str)
        .map(str::to_owned)
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

fn tool_status_str(update: &Value) -> String {
    update
        .get("status")
        .or_else(|| update.pointer("/toolCall/status"))
        .and_then(Value::as_str)
        .unwrap_or("in_progress")
        .to_owned()
}

fn should_emit_tool_progress(update: &Value, cache: &ToolCallCache) -> bool {
    let id = tool_call_id(update).unwrap_or_else(|| "tool".to_owned());
    if cache.cached_title(&id).is_some() {
        return false;
    }
    let summary = tool_summary(update);
    let kind = update
        .get("kind")
        .or_else(|| update.pointer("/toolCall/kind"))
        .and_then(Value::as_str)
        .unwrap_or("tool");
    !summary.eq_ignore_ascii_case("tool")
        || !kind.eq_ignore_ascii_case("tool")
        || update.pointer("/locations/0/path").is_some()
        || update.pointer("/toolCall/locations/0/path").is_some()
}

fn map_tool_call_update(update: &Value) -> Option<ActivityItem> {
    let title = tool_summary(update);
    if title.eq_ignore_ascii_case("tool") {
        return None;
    }
    let status = tool_status_from_str(tool_status_str(update).as_str());
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
    // FR-1417: execute rows show the command, never the cwd location.
    let command = update
        .get("command")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let target = if kind.eq_ignore_ascii_case("execute") {
        command.unwrap_or_else(|| title.clone())
    } else {
        location.unwrap_or_else(|| title.clone())
    };
    Some(ActivityItem::Tool {
        name: kind.to_owned(),
        target,
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
        let mut cache = ToolCallCache::default();
        let events = map_session_update(&update, &mut cache);
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
        let mut cache = ToolCallCache::default();
        let events = map_session_update(&update, &mut cache);
        assert!(
            !events.iter().any(
                |e| matches!(e, AgentEvent::ToolProgress { summary, .. } if summary == "tool")
            ),
            "generic tool progress should be suppressed"
        );
        assert!(events.iter().any(|e| matches!(e, AgentEvent::Activity(_))));
    }

    // TC-1332: merge generic completion into earlier titled tool row.
    #[test]
    fn merges_tool_call_updates_by_id() {
        let mut cache = ToolCallCache::default();
        let first = json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc-2",
            "title": "Read NOTES.md",
            "kind": "read",
            "status": "in_progress"
        });
        assert!(map_session_update(&first, &mut cache).is_empty());
        let second = json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc-2",
            "title": "tool",
            "kind": "tool",
            "status": "completed"
        });
        let events = map_session_update(&second, &mut cache);
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], AgentEvent::Activity(_)));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolProgress { .. }))
        );
    }

    // TC-1422 / FR-1417: execute activity shows the command, not the cwd.
    #[test]
    fn tc_1422_execute_row_shows_command() {
        let mut cache = ToolCallCache::default();
        let start = json!({
            "sessionUpdate": "tool_call", "toolCallId": "t1", "kind": "execute",
            "title": "Terminal", "status": "pending",
            "locations": [{"path": "/proj"}],
            "rawInput": {"command": "cargo test --lib"}
        });
        map_session_update(&start, &mut cache);
        let done =
            json!({"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed"});
        let events = map_session_update(&done, &mut cache);
        let target = events.iter().find_map(|e| match e {
            AgentEvent::Activity(ActivityItem::Tool { target, .. }) => Some(target.clone()),
            _ => None,
        });
        assert_eq!(target.as_deref(), Some("cargo test --lib"));
    }
}
