//! Map grok streaming-json tool lines onto [`ActivityItem`]s (FR-980).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::Value;

use crate::activity::{ActivityItem, ToolStatus};

const TARGET_MAX_CHARS: usize = 80;
const DETAIL_MAX_BYTES: usize = 4096;

const TARGET_KEYS: &[&str] = &[
    "target_file",
    "file_path",
    "target_directory",
    "command",
    "pattern",
    "query",
    "url",
];

/// Remembers open grok tool calls so a later update can finish the same row.
pub struct GrokToolTracker {
    workspace: PathBuf,
    open: HashMap<String, (String, String, String, Instant)>,
}

impl GrokToolTracker {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            open: HashMap::new(),
        }
    }

    pub fn observe(&mut self, line: &str) -> Option<ActivityItem> {
        self.observe_at(line, Instant::now())
    }

    pub fn observe_at(&mut self, line: &str, now: Instant) -> Option<ActivityItem> {
        let value: Value = serde_json::from_str(line).ok()?;
        match value.get("type").and_then(Value::as_str)? {
            "tool_call" => Some(self.on_tool_call(&value, now)),
            "tool_call_update" => self.on_tool_update(&value, now),
            _ => None,
        }
    }

    fn on_tool_call(&mut self, value: &Value, now: Instant) -> ActivityItem {
        let name = verb_name(
            value.get("kind").and_then(Value::as_str),
            value.get("toolName").and_then(Value::as_str),
        );
        let raw = value.get("rawInput").filter(|raw| !raw.is_null());
        let target = target_from(raw, &self.workspace);
        let detail = detail_from(raw);
        if let Some(id) = value.get("toolCallId").and_then(Value::as_str)
            && !id.is_empty()
        {
            self.open.insert(
                id.to_string(),
                (name.clone(), target.clone(), detail.clone(), now),
            );
        }
        ActivityItem::Tool {
            name,
            target,
            status: ToolStatus::Running,
            duration_ms: None,
            detail,
        }
    }

    fn on_tool_update(&mut self, value: &Value, now: Instant) -> Option<ActivityItem> {
        let status = match value.get("status")? {
            Value::String(status) => status.as_str(),
            _ => return None,
        };
        let status = match status {
            "completed" => ToolStatus::Ok,
            "failed" => ToolStatus::Failed,
            _ => return None,
        };
        let id = value.get("toolCallId").and_then(Value::as_str)?;
        let (name, target, detail, started) = self.open.remove(id)?;
        let duration_ms =
            u64::try_from(now.saturating_duration_since(started).as_millis()).unwrap_or(u64::MAX);
        Some(ActivityItem::Tool {
            name,
            target,
            status,
            duration_ms: Some(duration_ms),
            detail,
        })
    }
}

fn verb_name(kind: Option<&str>, tool_name: Option<&str>) -> String {
    let verb = match kind {
        Some("read") => Some("read"),
        Some("edit") => Some("edit"),
        Some("write") => Some("write"),
        Some("execute") => Some("run"),
        Some("search") => Some("search"),
        Some("list") => Some("list"),
        Some("plan") => Some("todo"),
        _ => None,
    };
    if let Some(verb) = verb {
        return verb.to_string();
    }
    match tool_name {
        Some(name) if !name.is_empty() => name.to_string(),
        _ => "tool".to_string(),
    }
}

fn target_from(raw: Option<&Value>, workspace: &Path) -> String {
    let Some(object) = raw.and_then(Value::as_object) else {
        return String::new();
    };
    for key in TARGET_KEYS {
        let Some(text) = object.get(*key).and_then(Value::as_str) else {
            continue;
        };
        if text.is_empty() {
            continue;
        }
        let stripped = strip_workspace_prefix(text, &workspace.to_string_lossy());
        return cut_chars(&collapse_whitespace(&stripped), TARGET_MAX_CHARS);
    }
    String::new()
}

fn strip_workspace_prefix(target: &str, workspace: &str) -> String {
    let workspace = workspace.trim_end_matches(['/', '\\']);
    if workspace.is_empty() {
        return target.to_string();
    }
    if target == workspace {
        return String::new();
    }
    let prefix = format!("{workspace}/");
    target.strip_prefix(&prefix).unwrap_or(target).to_string()
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Keep at most `max` characters. A longer string is cut so the result is `max`
/// characters and ends with `…`.
fn cut_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let take = max.saturating_sub(1);
    let mut out: String = text.chars().take(take).collect();
    out.push('…');
    out
}

fn detail_from(raw: Option<&Value>) -> String {
    let Some(raw) = raw else {
        return String::new();
    };
    let pretty = serde_json::to_string_pretty(raw).unwrap_or_default();
    cut_bytes(&pretty, DETAIL_MAX_BYTES)
}

/// Cut to `max` bytes on a char boundary and append `…` when anything was removed.
fn cut_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = text[..end].to_string();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn expect_tool(
        item: Option<ActivityItem>,
    ) -> (String, String, ToolStatus, Option<u64>, String) {
        match item {
            Some(ActivityItem::Tool {
                name,
                target,
                status,
                duration_ms,
                detail,
            }) => (name, target, status, duration_ms, detail),
            other => panic!("expected a tool activity, got {other:?}"),
        }
    }

    // Requirement/TC traceability: FR-980 / TC-980
    #[test]
    fn grok_tool_lines_map_to_activity_items() {
        let mut tracker = GrokToolTracker::new("/ws");
        let start = Instant::now();

        let read = r#"{"type":"tool_call","toolCallId":"a","title":"read_file","kind":"read","status":"pending","toolName":"read_file","rawInput":{"target_file":"/ws/src/app.rs","limit":250},"content":[],"locations":[]}"#;
        let (name, target, status, duration_ms, read_detail) =
            expect_tool(tracker.observe_at(read, start));
        assert_eq!(name, "read");
        assert_eq!(target, "src/app.rs");
        assert_eq!(status, ToolStatus::Running);
        assert_eq!(duration_ms, None);
        assert!(!read_detail.is_empty());

        let execute = r#"{"type":"tool_call","toolCallId":"b","kind":"execute","toolName":"run_terminal_command","rawInput":{"command":"cargo test","description":"Run tests"}}"#;
        let (name, target, status, duration_ms, _) =
            expect_tool(tracker.observe_at(execute, start));
        assert_eq!(name, "run");
        assert_eq!(target, "cargo test");
        assert_eq!(status, ToolStatus::Running);
        assert_eq!(duration_ms, None);

        let command = "x".repeat(200);
        let long = serde_json::json!({
            "type": "tool_call",
            "toolCallId": "long",
            "kind": "execute",
            "toolName": "run_terminal_command",
            "rawInput": {"command": command}
        })
        .to_string();
        let (_, target, _, _, _) = expect_tool(tracker.observe_at(&long, start));
        let mut expected_target: String = "x".repeat(79);
        expected_target.push('…');
        assert_eq!(target, expected_target);
        assert_eq!(target.chars().count(), 80);
        assert!(target.ends_with('…'));

        let edit = serde_json::json!({
            "type": "tool_call",
            "toolCallId": "c",
            "kind": "edit",
            "toolName": "search_replace",
            "rawInput": {
                "file_path": "/ws/src/x.rs",
                "old_string": "x".repeat(5000),
                "new_string": "y"
            }
        })
        .to_string();
        let (name, target, _, _, detail) = expect_tool(tracker.observe_at(&edit, start));
        assert_eq!(name, "edit");
        assert_eq!(target, "src/x.rs");
        assert!(detail.len() <= 4096 + '…'.len_utf8());
        assert!(detail.ends_with('…'));

        let other = r#"{"type":"tool_call","toolCallId":"d","kind":"other","toolName":"web_fetch","rawInput":{"url":"https://example.com"}}"#;
        let (name, target, status, _, _) = expect_tool(tracker.observe_at(other, start));
        assert_eq!(name, "web_fetch");
        assert_eq!(target, "https://example.com");
        assert_eq!(status, ToolStatus::Running);

        let null_status = r#"{"type":"tool_call_update","toolCallId":"a","status":null,"content":[],"rawOutput":null,"locations":[{"path":"/ws/src/app.rs"}]}"#;
        assert!(tracker.observe_at(null_status, start).is_none());
        let in_progress = r#"{"type":"tool_call_update","toolCallId":"a","status":"in_progress","content":[],"rawOutput":null,"locations":[]}"#;
        assert!(tracker.observe_at(in_progress, start).is_none());

        let completed = r#"{"type":"tool_call_update","toolCallId":"a","status":"completed","content":[],"rawOutput":null,"locations":[]}"#;
        let later = start + Duration::from_millis(1500);
        let (name, target, status, duration_ms, detail) =
            expect_tool(tracker.observe_at(completed, later));
        assert_eq!(name, "read");
        assert_eq!(target, "src/app.rs");
        assert_eq!(status, ToolStatus::Ok);
        assert_eq!(duration_ms, Some(1500));
        assert_eq!(detail, read_detail);

        let failed = r#"{"type":"tool_call_update","toolCallId":"b","status":"failed"}"#;
        let (name, target, status, duration_ms, _) = expect_tool(tracker.observe_at(failed, later));
        assert_eq!(name, "run");
        assert_eq!(target, "cargo test");
        assert_eq!(status, ToolStatus::Failed);
        assert_eq!(duration_ms, Some(1500));

        let unknown = r#"{"type":"tool_call_update","toolCallId":"zzz","status":"completed"}"#;
        assert!(tracker.observe_at(unknown, start).is_none());
        assert!(tracker.observe_at("{", start).is_none());
        assert!(
            tracker
                .observe_at(r#"{"type":"text","data":"x"}"#, start)
                .is_none()
        );
    }
}
