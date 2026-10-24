use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// Display mode for tool calls in the chat view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShowTools {
    #[default]
    Collapsed,
    Expanded,
    Hidden,
}

impl<'de> Deserialize<'de> for ShowTools {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = ShowTools;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(r#""collapsed", "expanded", or "hidden""#)
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value.trim().to_ascii_lowercase().as_str() {
                    "expanded" => Ok(ShowTools::Expanded),
                    "hidden" => Ok(ShowTools::Hidden),
                    _ => Ok(ShowTools::Collapsed),
                }
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}

impl Serialize for ShowTools {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Collapsed => serializer.serialize_str("collapsed"),
            Self::Expanded => serializer.serialize_str("expanded"),
            Self::Hidden => serializer.serialize_str("hidden"),
        }
    }
}

/// Display mode for model reasoning/thinking streams in the chat view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShowThinking {
    #[default]
    Live,
    Collapsed,
    Hidden,
}

impl<'de> Deserialize<'de> for ShowThinking {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = ShowThinking;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(r#""live", "collapsed", or "hidden""#)
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                match value.trim().to_ascii_lowercase().as_str() {
                    "collapsed" => Ok(ShowThinking::Collapsed),
                    "hidden" => Ok(ShowThinking::Hidden),
                    _ => Ok(ShowThinking::Live),
                }
            }
        }

        deserializer.deserialize_str(Visitor)
    }
}

impl Serialize for ShowThinking {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Live => serializer.serialize_str("live"),
            Self::Collapsed => serializer.serialize_str("collapsed"),
            Self::Hidden => serializer.serialize_str("hidden"),
        }
    }
}

/// UI presentation settings for tool calls and thinking display under `[ui]` in `dmuxai.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct UiSettings {
    #[serde(default)]
    pub show_tools: ShowTools,
    #[serde(default)]
    pub show_thinking: ShowThinking,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            show_tools: ShowTools::Collapsed,
            show_thinking: ShowThinking::Live,
        }
    }
}

/// Execution status of a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Ok,
    Failed,
}

/// Ephemeral activity item representing a tool invocation or model thinking stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ActivityItem {
    Tool {
        name: String,
        target: String,
        status: ToolStatus,
        duration_ms: Option<u64>,
        detail: String,
    },
    Thinking {
        text: String,
        started_ms: u64,
        finished_ms: Option<u64>,
    },
}

impl ActivityItem {
    pub fn is_tool(&self) -> bool {
        matches!(self, Self::Tool { .. })
    }

    pub fn is_thinking(&self) -> bool {
        matches!(self, Self::Thinking { .. })
    }
}

/// Merges an incoming activity item into an existing collection of items of type `T`.
///
/// Searches from the end for the matching element using `view` (elements where `view` returns `None`
/// are skipped):
/// - A `Tool` replaces the most recent `Running` tool with the same `name`, else is pushed via `wrap(item)`.
/// - A `Thinking` merges into the most recent `Thinking` whose `finished_ms` is `None`
///   (setting `finished_ms` if incoming has it, setting `started_ms` if existing is 0 and
///   incoming is non-zero, and appending incoming text), else is pushed via `wrap(item)`.
///
/// Returns the index in `items` that was updated or pushed.
pub fn merge_activity_in<T>(
    items: &mut Vec<T>,
    item: ActivityItem,
    mut view: impl FnMut(&mut T) -> Option<&mut ActivityItem>,
    wrap: impl FnOnce(ActivityItem) -> T,
) -> usize {
    match item {
        ActivityItem::Tool { ref name, .. } => {
            let found = items
                .iter_mut()
                .enumerate()
                .rev()
                .find_map(|(idx, el)| match view(el) {
                    Some(ActivityItem::Tool {
                        name: existing_name,
                        status: ToolStatus::Running,
                        ..
                    }) if existing_name == name => Some(idx),
                    _ => None,
                });

            if let Some(index) = found {
                if let Some(target) = view(&mut items[index]) {
                    *target = item;
                }
                index
            } else {
                items.push(wrap(item));
                items.len() - 1
            }
        }
        ActivityItem::Thinking {
            text: incoming_text,
            started_ms: incoming_started,
            finished_ms: incoming_finished,
        } => {
            let found = items
                .iter_mut()
                .enumerate()
                .rev()
                .find_map(|(idx, el)| match view(el) {
                    Some(ActivityItem::Thinking {
                        finished_ms: None, ..
                    }) => Some(idx),
                    _ => None,
                });

            if let Some(index) = found {
                if let Some(ActivityItem::Thinking {
                    text: existing_text,
                    started_ms: existing_started,
                    finished_ms: existing_finished,
                }) = view(&mut items[index])
                {
                    if incoming_finished.is_some() {
                        *existing_finished = incoming_finished;
                    }
                    if *existing_started == 0 && incoming_started != 0 {
                        *existing_started = incoming_started;
                    }
                    if !incoming_text.is_empty() {
                        existing_text.push_str(&incoming_text);
                    }
                }
                index
            } else {
                items.push(wrap(ActivityItem::Thinking {
                    text: incoming_text,
                    started_ms: incoming_started,
                    finished_ms: incoming_finished,
                }));
                items.len() - 1
            }
        }
    }
}

/// Merges an incoming activity item into an existing sequence.
///
/// Single source of the merge rule used by App, the dashboard and durable storage:
/// - A `Tool` replaces the most recent `Running` tool with the same `name`, else is pushed.
/// - A `Thinking` merges into the most recent `Thinking` whose `finished_ms` is `None`
///   (setting `finished_ms` if incoming has it, setting `started_ms` if existing is 0 and
///   incoming is non-zero, and appending incoming text), else is pushed.
///
/// Returns the index in `items` that was updated or pushed.
pub fn merge_activity(items: &mut Vec<ActivityItem>, item: ActivityItem) -> usize {
    merge_activity_in(items, item, |existing| Some(existing), |item| item)
}

/// Default line limit when expanding tool details or thought transcripts.
pub const DEFAULT_MAX_EXPANDED_LINES: usize = 40;

/// Render an activity item into a single collapsed line string.
///
/// Formats:
/// - Running tool: `▸ edit src/app.rs (+12 -3)`
/// - Successful tool: `▸ run cargo test ✓ 4.2s`
/// - Failed tool: `▸ run cargo test ✗ 4.2s` (or `▸ run cargo test ✗`)
/// - Live streaming thinking: `thinking… <last 80 chars>`
/// - Completed thinking: `▸ thought for 12s` (or `▸ thought for 0s` if under 1s)
pub fn collapsed_line(item: &ActivityItem) -> String {
    match item {
        ActivityItem::Tool {
            name,
            target,
            status,
            duration_ms,
            ..
        } => {
            let mut line = String::from("▸ ");
            line.push_str(name);
            if !target.is_empty() {
                line.push(' ');
                line.push_str(target);
            }
            match status {
                ToolStatus::Running => {}
                ToolStatus::Ok => {
                    line.push_str(" ✓");
                    if let Some(ms) = duration_ms {
                        line.push(' ');
                        line.push_str(&format_tool_duration(*ms));
                    }
                }
                ToolStatus::Failed => {
                    line.push_str(" ✗");
                    if let Some(ms) = duration_ms {
                        line.push(' ');
                        line.push_str(&format_tool_duration(*ms));
                    }
                }
            }
            line
        }
        ActivityItem::Thinking {
            text,
            started_ms,
            finished_ms,
        } => {
            if let Some(finished) = finished_ms {
                let duration_secs = finished.saturating_sub(*started_ms) / 1000;
                format!("▸ thought for {duration_secs}s")
            } else if text.is_empty() {
                String::new()
            } else {
                let count = text.chars().count();
                let last_80: String = if count <= 80 {
                    text.clone()
                } else {
                    text.chars().skip(count - 80).collect()
                };
                format!("thinking… {last_80}")
            }
        }
    }
}

fn format_tool_duration(ms: u64) -> String {
    let secs = ms as f64 / 1000.0;
    if ms.is_multiple_of(1000) {
        format!("{}s", ms / 1000)
    } else {
        format!("{secs:.1}s")
    }
}

/// Format a slice of tool activity items into a single collapsed summary line.
///
/// Example: `▸ 7 tool calls (edit ×3, run ×4)`
pub fn format_tool_group(tools: &[ActivityItem]) -> String {
    let total = tools.len();
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for item in tools {
        if let ActivityItem::Tool { name, .. } = item {
            if let Some(entry) = counts.iter_mut().find(|(n, _)| *n == name.as_str()) {
                entry.1 += 1;
            } else {
                counts.push((name.as_str(), 1));
            }
        }
    }
    let parts: Vec<String> = counts
        .into_iter()
        .map(|(name, count)| format!("{name} ×{count}"))
        .collect();
    format!("▸ {total} tool calls ({})", parts.join(", "))
}

/// Group items, collapsing any sequence of more than 5 consecutive tool calls
/// into a single summary line: `▸ <N> tool calls (<tool1> ×<count1>, <tool2> ×<count2>)`.
///
/// Sequences of 5 or fewer consecutive tool calls remain individual collapsed lines.
pub fn group_tools(items: &[ActivityItem]) -> Vec<String> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < items.len() {
        if items[i].is_tool() {
            let start = i;
            while i < items.len() && items[i].is_tool() {
                i += 1;
            }
            let chunk = &items[start..i];
            if chunk.len() > 5 {
                result.push(format_tool_group(chunk));
            } else {
                for item in chunk {
                    result.push(collapsed_line(item));
                }
            }
        } else {
            let line = collapsed_line(&items[i]);
            if !line.is_empty() {
                result.push(line);
            }
            i += 1;
        }
    }
    result
}

/// Expand an activity item into multiple detail lines, truncated to at most `max_lines`.
///
/// If truncated, the final line is replaced with a truncation marker: `… [<N> lines truncated]`.
pub fn expanded_lines(item: &ActivityItem, max_lines: usize) -> Vec<String> {
    let content = match item {
        ActivityItem::Tool { detail, .. } => detail.as_str(),
        ActivityItem::Thinking { text, .. } => text.as_str(),
    };

    if content.is_empty() || max_lines == 0 {
        return Vec::new();
    }

    let all_lines: Vec<&str> = content.lines().collect();
    if all_lines.is_empty() {
        return Vec::new();
    }

    if all_lines.len() <= max_lines {
        all_lines.into_iter().map(String::from).collect()
    } else {
        let keep = max_lines.saturating_sub(1);
        let mut truncated: Vec<String> = all_lines
            .iter()
            .take(keep)
            .map(|s| (*s).to_string())
            .collect();
        let hidden = all_lines.len() - keep;
        truncated.push(format!("… [{hidden} lines truncated]"));
        truncated
    }
}

/// Extract target summary from a tool parameter JSON object or string.
fn extract_target_from_parameters(params: &Value) -> String {
    if let Some(s) = params.as_str() {
        return s.to_owned();
    }
    if let Some(obj) = params.as_object() {
        // 1. Command invocation
        for key in ["command", "command_line", "CommandLine", "cmd"] {
            if let Some(cmd) = obj.get(key).and_then(Value::as_str) {
                return cmd.to_owned();
            }
        }
        // 2. File target with optional diff stats
        let path = [
            "path",
            "file_path",
            "TargetFile",
            "target_file",
            "file",
            "target",
        ]
        .iter()
        .find_map(|key| obj.get(*key).and_then(Value::as_str));
        if let Some(path) = path {
            let diff = ["diff", "diff_stats", "stats"]
                .iter()
                .find_map(|key| obj.get(*key).and_then(Value::as_str));
            return match diff {
                Some(d) if !d.is_empty() => {
                    if d.starts_with('(') && d.ends_with(')') {
                        format!("{path} {d}")
                    } else {
                        format!("{path} ({d})")
                    }
                }
                _ => path.to_owned(),
            };
        }
    }
    String::new()
}

/// Map an incoming AGY `step_update` event JSON value into an `ActivityItem`.
///
/// Handles:
/// - `step_type == "tool"` (with `tool_name`, `tool_info.parameters`, `state`, `duration_seconds`)
/// - thinking deltas (with `step_type == "thinking"` or `thinking_delta`)
/// - returns `None` for unrecognized event types or non-activity steps.
pub fn from_agy_step(value: &Value) -> Option<ActivityItem> {
    let step = if value.get("event").and_then(Value::as_str) == Some("step_update") {
        value.get("step_update")?
    } else if let Some(su) = value.get("step_update") {
        su
    } else {
        value
    };

    let step_type = step.get("step_type").and_then(Value::as_str);

    // Thinking: step_type == "thinking" / "thought" OR explicit thinking_delta present
    if step_type == Some("thinking")
        || step_type == Some("thought")
        || step.get("thinking_delta").is_some()
        || step.get("thought_delta").is_some()
    {
        let text = step
            .get("thinking_delta")
            .or_else(|| step.get("thought_delta"))
            .or_else(|| step.get("text_delta"))
            .or_else(|| step.get("text"))
            .or_else(|| step.get("content"))
            .and_then(Value::as_str)
            .unwrap_or_default();

        let state = step.get("state").and_then(Value::as_str);
        let duration_seconds = step
            .get("duration_seconds")
            .and_then(Value::as_f64)
            .or_else(|| step.get("duration").and_then(Value::as_f64));

        let finished_ms = if state == Some("DONE") || duration_seconds.is_some() {
            duration_seconds.map(|s| (s * 1000.0).round() as u64)
        } else {
            None
        };

        if finished_ms.is_some() || !text.is_empty() {
            return Some(ActivityItem::Thinking {
                text: text.to_owned(),
                started_ms: 0,
                finished_ms,
            });
        }
        return None;
    }

    // Tool: step_type == "tool" (or "tool_call")
    if step_type == Some("tool") || step_type == Some("tool_call") {
        let tool_name = step
            .get("tool_name")
            .or_else(|| step.pointer("/tool_info/tool_name"))
            .or_else(|| step.pointer("/tool_info/name"))
            .or_else(|| step.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("tool");

        let name = match tool_name {
            "run_command" | "execute_command" => "run",
            "replace_file_content" | "write_to_file" | "edit_file" => "edit",
            other => other,
        }
        .to_owned();

        let params = step
            .pointer("/tool_info/parameters")
            .or_else(|| step.get("parameters"))
            .or_else(|| step.get("args"));

        let (target, detail) = if let Some(p) = params {
            let target = extract_target_from_parameters(p);
            let detail = if let Some(s) = p.as_str() {
                s.to_owned()
            } else {
                serde_json::to_string_pretty(p).unwrap_or_default()
            };
            (target, detail)
        } else {
            (String::new(), String::new())
        };

        let state = step
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("ACTIVE");
        let status = match state.to_ascii_uppercase().as_str() {
            "DONE" | "SUCCESS" | "OK" => {
                if step.get("error").is_some()
                    || step.get("status").and_then(Value::as_str) == Some("ERROR")
                    || step.get("status").and_then(Value::as_str) == Some("FAILED")
                {
                    ToolStatus::Failed
                } else {
                    ToolStatus::Ok
                }
            }
            "FAILED" | "ERROR" => ToolStatus::Failed,
            _ => ToolStatus::Running,
        };

        let duration_ms = step
            .get("duration_seconds")
            .and_then(Value::as_f64)
            .map(|s| (s * 1000.0).round() as u64)
            .or_else(|| step.get("duration_ms").and_then(Value::as_u64));

        return Some(ActivityItem::Tool {
            name,
            target,
            status,
            duration_ms,
            detail,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_settings_defaults_and_toml_deserialization() {
        let empty_toml = r##""##;
        let default_ui: UiSettings = toml::from_str(empty_toml).unwrap();
        assert_eq!(default_ui.show_tools, ShowTools::Collapsed);
        assert_eq!(default_ui.show_thinking, ShowThinking::Live);

        let valid_toml = r##"
            show_tools = "expanded"
            show_thinking = "hidden"
        "##;
        let parsed: UiSettings = toml::from_str(valid_toml).unwrap();
        assert_eq!(parsed.show_tools, ShowTools::Expanded);
        assert_eq!(parsed.show_thinking, ShowThinking::Hidden);

        let collapsed_toml = r##"
            show_tools = "collapsed"
            show_thinking = "collapsed"
        "##;
        let parsed: UiSettings = toml::from_str(collapsed_toml).unwrap();
        assert_eq!(parsed.show_tools, ShowTools::Collapsed);
        assert_eq!(parsed.show_thinking, ShowThinking::Collapsed);

        let invalid_toml = r##"
            show_tools = "bogus"
            show_thinking = "nonexistent"
        "##;
        let parsed: UiSettings = toml::from_str(invalid_toml).unwrap();
        assert_eq!(parsed.show_tools, ShowTools::Collapsed);
        assert_eq!(parsed.show_thinking, ShowThinking::Live);
    }

    #[test]
    fn collapsed_line_tool_running() {
        let item = ActivityItem::Tool {
            name: "edit".to_owned(),
            target: "src/app.rs (+12 -3)".to_owned(),
            status: ToolStatus::Running,
            duration_ms: None,
            detail: String::new(),
        };
        assert_eq!(collapsed_line(&item), "▸ edit src/app.rs (+12 -3)");
    }

    #[test]
    fn collapsed_line_tool_success_with_duration() {
        let item = ActivityItem::Tool {
            name: "run".to_owned(),
            target: "cargo test".to_owned(),
            status: ToolStatus::Ok,
            duration_ms: Some(4200),
            detail: String::new(),
        };
        assert_eq!(collapsed_line(&item), "▸ run cargo test ✓ 4.2s");
    }

    #[test]
    fn collapsed_line_tool_failure_formats() {
        let with_dur = ActivityItem::Tool {
            name: "run".to_owned(),
            target: "cargo test".to_owned(),
            status: ToolStatus::Failed,
            duration_ms: Some(4200),
            detail: String::new(),
        };
        assert_eq!(collapsed_line(&with_dur), "▸ run cargo test ✗ 4.2s");

        let without_dur = ActivityItem::Tool {
            name: "run".to_owned(),
            target: "cargo test".to_owned(),
            status: ToolStatus::Failed,
            duration_ms: None,
            detail: String::new(),
        };
        assert_eq!(collapsed_line(&without_dur), "▸ run cargo test ✗");
    }

    #[test]
    fn collapsed_line_thinking_live_and_finished() {
        let short_live = ActivityItem::Thinking {
            text: "analyzing requirements".to_owned(),
            started_ms: 1000,
            finished_ms: None,
        };
        assert_eq!(
            collapsed_line(&short_live),
            "thinking… analyzing requirements"
        );

        let long_text = "a".repeat(100);
        let long_live = ActivityItem::Thinking {
            text: long_text,
            started_ms: 1000,
            finished_ms: None,
        };
        let expected_last_80 = format!("thinking… {}", "a".repeat(80));
        assert_eq!(collapsed_line(&long_live), expected_last_80);

        let finished = ActivityItem::Thinking {
            text: "some reasoning".to_owned(),
            started_ms: 1000,
            finished_ms: Some(13000),
        };
        assert_eq!(collapsed_line(&finished), "▸ thought for 12s");

        let zero_dur = ActivityItem::Thinking {
            text: "fast thought".to_owned(),
            started_ms: 1000,
            finished_ms: Some(1400),
        };
        assert_eq!(collapsed_line(&zero_dur), "▸ thought for 0s");

        let empty_live = ActivityItem::Thinking {
            text: String::new(),
            started_ms: 1000,
            finished_ms: None,
        };
        assert_eq!(collapsed_line(&empty_live), "");
    }

    #[test]
    fn group_tools_collapses_over_five_consecutive_calls() {
        let tools_5 = (0..5)
            .map(|i| ActivityItem::Tool {
                name: "edit".to_owned(),
                target: format!("file_{i}.rs"),
                status: ToolStatus::Ok,
                duration_ms: None,
                detail: String::new(),
            })
            .collect::<Vec<_>>();
        let lines_5 = group_tools(&tools_5);
        assert_eq!(lines_5.len(), 5);
        assert_eq!(lines_5[0], "▸ edit file_0.rs ✓");

        let mut tools_7 = Vec::new();
        for _ in 0..3 {
            tools_7.push(ActivityItem::Tool {
                name: "edit".to_owned(),
                target: "src/main.rs".to_owned(),
                status: ToolStatus::Ok,
                duration_ms: None,
                detail: String::new(),
            });
        }
        for _ in 0..4 {
            tools_7.push(ActivityItem::Tool {
                name: "run".to_owned(),
                target: "cargo check".to_owned(),
                status: ToolStatus::Ok,
                duration_ms: None,
                detail: String::new(),
            });
        }
        let lines_7 = group_tools(&tools_7);
        assert_eq!(lines_7, vec!["▸ 7 tool calls (edit ×3, run ×4)"]);

        let mixed = vec![
            ActivityItem::Tool {
                name: "run".to_owned(),
                target: "cargo check".to_owned(),
                status: ToolStatus::Ok,
                duration_ms: None,
                detail: String::new(),
            },
            ActivityItem::Thinking {
                text: "pondering".to_owned(),
                started_ms: 0,
                finished_ms: None,
            },
            ActivityItem::Tool {
                name: "edit".to_owned(),
                target: "src/app.rs".to_owned(),
                status: ToolStatus::Running,
                duration_ms: None,
                detail: String::new(),
            },
        ];
        let mixed_lines = group_tools(&mixed);
        assert_eq!(
            mixed_lines,
            vec![
                "▸ run cargo check ✓",
                "thinking… pondering",
                "▸ edit src/app.rs",
            ]
        );
    }

    #[test]
    fn expanded_lines_truncation() {
        let content_60 = (1..=60)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let item = ActivityItem::Tool {
            name: "run".to_owned(),
            target: "long output".to_owned(),
            status: ToolStatus::Ok,
            duration_ms: None,
            detail: content_60,
        };

        let lines_40 = expanded_lines(&item, 40);
        assert_eq!(lines_40.len(), 40);
        assert_eq!(lines_40[0], "line 1");
        assert_eq!(lines_40[38], "line 39");
        assert_eq!(lines_40[39], "… [21 lines truncated]");

        let item_short = ActivityItem::Thinking {
            text: "first\nsecond\nthird".to_owned(),
            started_ms: 0,
            finished_ms: None,
        };
        let lines_short = expanded_lines(&item_short, 40);
        assert_eq!(lines_short, vec!["first", "second", "third"]);

        let empty_item = ActivityItem::Tool {
            name: "empty".to_owned(),
            target: String::new(),
            status: ToolStatus::Running,
            duration_ms: None,
            detail: String::new(),
        };
        assert!(expanded_lines(&empty_item, 40).is_empty());
    }

    #[test]
    fn from_agy_step_tool_fixtures() {
        let edit_active_fixture: Value = serde_json::from_str(
            r##"
        {
            "event": "step_update",
            "step_update": {
                "step_type": "tool",
                "tool_name": "edit",
                "tool_info": {
                    "parameters": {
                        "path": "src/app.rs",
                        "diff": "+12 -3"
                    }
                },
                "state": "ACTIVE"
            }
        }
        "##,
        )
        .unwrap();

        let item = from_agy_step(&edit_active_fixture).expect("should parse tool step");
        assert_eq!(
            item,
            ActivityItem::Tool {
                name: "edit".to_owned(),
                target: "src/app.rs (+12 -3)".to_owned(),
                status: ToolStatus::Running,
                duration_ms: None,
                detail: serde_json::to_string_pretty(
                    &edit_active_fixture["step_update"]["tool_info"]["parameters"]
                )
                .unwrap(),
            }
        );
        assert_eq!(collapsed_line(&item), "▸ edit src/app.rs (+12 -3)");

        let run_done_fixture: Value = serde_json::from_str(
            r##"
        {
            "event": "step_update",
            "step_update": {
                "step_type": "tool",
                "tool_name": "run",
                "tool_info": {
                    "parameters": {
                        "command": "cargo test"
                    }
                },
                "state": "DONE",
                "duration_seconds": 4.2
            }
        }
        "##,
        )
        .unwrap();

        let item = from_agy_step(&run_done_fixture).expect("should parse done tool step");
        assert_eq!(
            item,
            ActivityItem::Tool {
                name: "run".to_owned(),
                target: "cargo test".to_owned(),
                status: ToolStatus::Ok,
                duration_ms: Some(4200),
                detail: serde_json::to_string_pretty(
                    &run_done_fixture["step_update"]["tool_info"]["parameters"]
                )
                .unwrap(),
            }
        );
        assert_eq!(collapsed_line(&item), "▸ run cargo test ✓ 4.2s");

        let run_failed_fixture: Value = serde_json::from_str(
            r##"
        {
            "event": "step_update",
            "step_update": {
                "step_type": "tool",
                "tool_name": "run_command",
                "tool_info": {
                    "parameters": {
                        "CommandLine": "cargo test"
                    }
                },
                "state": "FAILED",
                "duration_seconds": 1.5
            }
        }
        "##,
        )
        .unwrap();

        let item = from_agy_step(&run_failed_fixture).expect("should parse failed tool step");
        assert_eq!(
            item,
            ActivityItem::Tool {
                name: "run".to_owned(),
                target: "cargo test".to_owned(),
                status: ToolStatus::Failed,
                duration_ms: Some(1500),
                detail: serde_json::to_string_pretty(
                    &run_failed_fixture["step_update"]["tool_info"]["parameters"]
                )
                .unwrap(),
            }
        );
        assert_eq!(collapsed_line(&item), "▸ run cargo test ✗ 1.5s");
    }

    #[test]
    fn from_agy_step_thinking_fixtures() {
        let streaming_fixture: Value = serde_json::from_str(
            r##"
        {
            "event": "step_update",
            "step_update": {
                "step_type": "thinking",
                "thinking_delta": "pondering the implementation details"
            }
        }
        "##,
        )
        .unwrap();

        let item = from_agy_step(&streaming_fixture).expect("should parse streaming thinking step");
        assert_eq!(
            item,
            ActivityItem::Thinking {
                text: "pondering the implementation details".to_owned(),
                started_ms: 0,
                finished_ms: None,
            }
        );
        assert_eq!(
            collapsed_line(&item),
            "thinking… pondering the implementation details"
        );

        let finished_fixture: Value = serde_json::from_str(
            r##"
        {
            "event": "step_update",
            "step_update": {
                "step_type": "thinking",
                "state": "DONE",
                "duration_seconds": 12.0
            }
        }
        "##,
        )
        .unwrap();

        let item = from_agy_step(&finished_fixture).expect("should parse finished thinking step");
        assert_eq!(
            item,
            ActivityItem::Thinking {
                text: String::new(),
                started_ms: 0,
                finished_ms: Some(12000),
            }
        );
        assert_eq!(collapsed_line(&item), "▸ thought for 12s");
    }

    #[test]
    fn from_agy_step_unknown_events_return_none() {
        let response_fixture: Value = serde_json::from_str(
            r##"
        {
            "event": "step_update",
            "step_update": {
                "step_type": "agent_response",
                "text_delta": "Hello from AGY"
            }
        }
        "##,
        )
        .unwrap();
        assert!(from_agy_step(&response_fixture).is_none());

        let result_fixture: Value = serde_json::from_str(
            r##"
        {
            "event": "result",
            "result": {
                "status": "SUCCESS",
                "response": "Done"
            }
        }
        "##,
        )
        .unwrap();
        assert!(from_agy_step(&result_fixture).is_none());

        let unknown_fixture: Value = serde_json::from_str(
            r##"
        {
            "event": "custom_event",
            "data": 123
        }
        "##,
        )
        .unwrap();
        assert!(from_agy_step(&unknown_fixture).is_none());
    }

    #[test]
    fn tool_calls_grouped_when_exceeding_threshold() {
        group_tools_collapses_over_five_consecutive_calls();
    }

    #[test]
    fn thinking_live_stream_and_collapsed_summary() {
        collapsed_line_thinking_live_and_finished();
    }

    #[test]
    fn provider_activity_event_normalization() {
        from_agy_step_tool_fixtures();
        from_agy_step_thinking_fixtures();
        from_agy_step_unknown_events_return_none();
    }

    // TC-1904 / FR-1904: ActivityItem and ToolStatus serde round-trips and merge_activity rules.
    #[test]
    fn tc_1904_activity_item_serde_and_merge_rules() {
        // 1. Serde round-trips
        let tool_running = ActivityItem::Tool {
            name: "read_file".to_string(),
            target: "src/main.rs".to_string(),
            status: ToolStatus::Running,
            duration_ms: None,
            detail: "reading file".to_string(),
        };
        let tool_json = serde_json::to_string(&tool_running).expect("serialize tool");
        assert!(tool_json.contains(r#""type":"tool""#));
        assert!(tool_json.contains(r#""status":"running""#));
        let tool_de: ActivityItem = serde_json::from_str(&tool_json).expect("deserialize tool");
        assert_eq!(tool_running, tool_de);

        let thinking_live = ActivityItem::Thinking {
            text: "Initial thought".to_string(),
            started_ms: 1000,
            finished_ms: None,
        };
        let thinking_json = serde_json::to_string(&thinking_live).expect("serialize thinking");
        assert!(thinking_json.contains(r#""type":"thinking""#));
        let thinking_de: ActivityItem =
            serde_json::from_str(&thinking_json).expect("deserialize thinking");
        assert_eq!(thinking_live, thinking_de);

        // Tool status variants
        for (status, expected_str) in [
            (ToolStatus::Running, "\"running\""),
            (ToolStatus::Ok, "\"ok\""),
            (ToolStatus::Failed, "\"failed\""),
        ] {
            let json = serde_json::to_string(&status).expect("serialize status");
            assert_eq!(json, expected_str);
            let de: ToolStatus = serde_json::from_str(&json).expect("deserialize status");
            assert_eq!(de, status);
        }

        // 2. merge_activity rules
        let mut items = Vec::new();

        // Push new tool (Running)
        let idx0 = merge_activity(&mut items, tool_running.clone());
        assert_eq!(idx0, 0);
        assert_eq!(items.len(), 1);

        // Push another distinct tool (Running)
        let tool_exec = ActivityItem::Tool {
            name: "run_command".to_string(),
            target: "cargo test".to_string(),
            status: ToolStatus::Running,
            duration_ms: None,
            detail: "testing".to_string(),
        };
        let idx1 = merge_activity(&mut items, tool_exec);
        assert_eq!(idx1, 1);
        assert_eq!(items.len(), 2);

        // Tool replacement: incoming completed tool replaces the most recent running tool with same name
        let tool_completed = ActivityItem::Tool {
            name: "read_file".to_string(),
            target: "src/main.rs".to_string(),
            status: ToolStatus::Ok,
            duration_ms: Some(150),
            detail: "read 50 lines".to_string(),
        };
        let idx_replace = merge_activity(&mut items, tool_completed.clone());
        assert_eq!(idx_replace, 0);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], tool_completed);

        // A new tool with same name after completion is pushed (no running tool to replace)
        let tool_read_again = ActivityItem::Tool {
            name: "read_file".to_string(),
            target: "src/lib.rs".to_string(),
            status: ToolStatus::Running,
            duration_ms: None,
            detail: "reading lib".to_string(),
        };
        let idx2 = merge_activity(&mut items, tool_read_again);
        assert_eq!(idx2, 2);
        assert_eq!(items.len(), 3);

        // Thinking merging:
        // Push initial live thinking
        let idx3 = merge_activity(&mut items, thinking_live);
        assert_eq!(idx3, 3);
        assert_eq!(items.len(), 4);

        // Incoming thinking delta: merges into live thinking (finished_ms == None)
        let thinking_delta = ActivityItem::Thinking {
            text: "... more thoughts".to_string(),
            started_ms: 0,
            finished_ms: None,
        };
        let idx_delta = merge_activity(&mut items, thinking_delta);
        assert_eq!(idx_delta, 3);
        assert_eq!(items.len(), 4);
        if let ActivityItem::Thinking {
            text,
            started_ms,
            finished_ms,
        } = &items[3]
        {
            assert_eq!(text, "Initial thought... more thoughts");
            assert_eq!(*started_ms, 1000);
            assert_eq!(*finished_ms, None);
        } else {
            panic!("expected thinking at index 3");
        }

        // Incoming thinking completion: updates finished_ms and appends remaining text
        let thinking_finish = ActivityItem::Thinking {
            text: " (done)".to_string(),
            started_ms: 0,
            finished_ms: Some(2500),
        };
        let idx_finish = merge_activity(&mut items, thinking_finish);
        assert_eq!(idx_finish, 3);
        assert_eq!(items.len(), 4);
        if let ActivityItem::Thinking {
            text,
            started_ms,
            finished_ms,
        } = &items[3]
        {
            assert_eq!(text, "Initial thought... more thoughts (done)");
            assert_eq!(*started_ms, 1000);
            assert_eq!(*finished_ms, Some(2500));
        } else {
            panic!("expected thinking at index 3");
        }

        // New thinking after completion is pushed (no unfinished thinking)
        let new_thinking = ActivityItem::Thinking {
            text: "Second reasoning phase".to_string(),
            started_ms: 3000,
            finished_ms: None,
        };
        let idx4 = merge_activity(&mut items, new_thinking.clone());
        assert_eq!(idx4, 4);
        assert_eq!(items.len(), 5);
        assert_eq!(items[4], new_thinking);
    }

    // TC-1950 / FR-1950: merge_activity_in skips non-activity rows and correctly replaces/merges.
    #[test]
    fn tc_1950_merge_activity_in_skips_non_activity_rows() {
        #[derive(Debug, PartialEq, Clone)]
        enum Row {
            Comment(String),
            Activity(ActivityItem),
        }

        let mut rows = vec![Row::Comment("first comment".to_string())];

        let tool_running = ActivityItem::Tool {
            name: "read_file".to_string(),
            target: "src/main.rs".to_string(),
            status: ToolStatus::Running,
            duration_ms: None,
            detail: String::new(),
        };

        // Push new tool (Running). Index should be 1 (after the comment at 0)
        let idx0 = merge_activity_in(
            &mut rows,
            tool_running.clone(),
            |r| match r {
                Row::Activity(a) => Some(a),
                _ => None,
            },
            Row::Activity,
        );
        assert_eq!(idx0, 1);
        assert_eq!(rows.len(), 2);

        // Add an interleaved non-activity element
        rows.push(Row::Comment("interleaved comment".to_string()));
        assert_eq!(rows.len(), 3);

        // Replace tool across non-activity row:
        let tool_completed = ActivityItem::Tool {
            name: "read_file".to_string(),
            target: "src/main.rs".to_string(),
            status: ToolStatus::Ok,
            duration_ms: Some(120),
            detail: "done".to_string(),
        };
        let idx_replace = merge_activity_in(
            &mut rows,
            tool_completed.clone(),
            |r| match r {
                Row::Activity(a) => Some(a),
                _ => None,
            },
            Row::Activity,
        );
        assert_eq!(idx_replace, 1);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1], Row::Activity(tool_completed));

        // Push tool when none running (index 3)
        let tool_another = ActivityItem::Tool {
            name: "read_file".to_string(),
            target: "src/lib.rs".to_string(),
            status: ToolStatus::Running,
            duration_ms: None,
            detail: String::new(),
        };
        let idx_push = merge_activity_in(
            &mut rows,
            tool_another.clone(),
            |r| match r {
                Row::Activity(a) => Some(a),
                _ => None,
            },
            Row::Activity,
        );
        assert_eq!(idx_push, 3);
        assert_eq!(rows.len(), 4);

        // Add another non-activity row
        rows.push(Row::Comment("another comment".to_string())); // index 4
        assert_eq!(rows.len(), 5);

        // Thinking: push live thinking (index 5)
        let thinking_live = ActivityItem::Thinking {
            text: "Thinking part 1".to_string(),
            started_ms: 1000,
            finished_ms: None,
        };
        let idx_think = merge_activity_in(
            &mut rows,
            thinking_live,
            |r| match r {
                Row::Activity(a) => Some(a),
                _ => None,
            },
            Row::Activity,
        );
        assert_eq!(idx_think, 5);
        assert_eq!(rows.len(), 6);

        // Add non-activity row
        rows.push(Row::Comment("thought comment".to_string())); // index 6
        assert_eq!(rows.len(), 7);

        // Merge thinking into live thinking across non-activity rows (updating index 5)
        let thinking_delta = ActivityItem::Thinking {
            text: ", part 2".to_string(),
            started_ms: 0,
            finished_ms: None,
        };
        let idx_think_delta = merge_activity_in(
            &mut rows,
            thinking_delta,
            |r| match r {
                Row::Activity(a) => Some(a),
                _ => None,
            },
            Row::Activity,
        );
        assert_eq!(idx_think_delta, 5);
        assert_eq!(rows.len(), 7);

        // Finish thinking across non-activity rows (updating index 5)
        let thinking_finish = ActivityItem::Thinking {
            text: " (finished)".to_string(),
            started_ms: 0,
            finished_ms: Some(2500),
        };
        let idx_think_finish = merge_activity_in(
            &mut rows,
            thinking_finish,
            |r| match r {
                Row::Activity(a) => Some(a),
                _ => None,
            },
            Row::Activity,
        );
        assert_eq!(idx_think_finish, 5);
        assert_eq!(rows.len(), 7);
        if let Row::Activity(ActivityItem::Thinking {
            text,
            started_ms,
            finished_ms,
        }) = &rows[5]
        {
            assert_eq!(text, "Thinking part 1, part 2 (finished)");
            assert_eq!(*started_ms, 1000);
            assert_eq!(*finished_ms, Some(2500));
        } else {
            panic!("expected thinking at index 5");
        }

        // Add non-activity row
        rows.push(Row::Comment("after finished comment".to_string())); // index 7
        assert_eq!(rows.len(), 8);

        // Push new thinking after finished across non-activity rows (index 8)
        let new_thinking = ActivityItem::Thinking {
            text: "Fresh thought".to_string(),
            started_ms: 3000,
            finished_ms: None,
        };
        let idx_new_think = merge_activity_in(
            &mut rows,
            new_thinking.clone(),
            |r| match r {
                Row::Activity(a) => Some(a),
                _ => None,
            },
            Row::Activity,
        );
        assert_eq!(idx_new_think, 8);
        assert_eq!(rows.len(), 9);
        assert_eq!(rows[8], Row::Activity(new_thinking));
    }
}
