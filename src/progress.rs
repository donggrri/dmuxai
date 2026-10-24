//! Live progress formatting and text coalescing for headless runs (issue #137).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::agent::AgentEvent;
use dmuxai::activity::ActivityItem;

pub const MAX_SUMMARY_CHARS: usize = 160;
pub const TEXT_COALESCE_INTERVAL_MS: u64 = 2000;

/// Live progress line written to STDERR as JSON Lines when `--progress` is enabled.
///
/// Shape: `{"type":"progress","ts_ms":<u64>,"agent":"<runner id>","kind":"<kind>","summary":"<text>"}`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressLine {
    #[serde(rename = "type")]
    pub line_type: String,
    pub ts_ms: u64,
    pub agent: String,
    pub kind: String,
    pub summary: String,
}

impl ProgressLine {
    pub fn new(
        ts_ms: u64,
        agent: impl Into<String>,
        kind: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            line_type: "progress".to_owned(),
            ts_ms,
            agent: agent.into(),
            kind: kind.into(),
            summary: summary.into(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// Sanitizes a summary: single-line (newlines replaced by spaces) and at most
/// `max_chars` unicode characters. If longer than `max_chars`, truncates to
/// `max_chars - 1` and appends `…` (horizontal ellipsis).
pub fn sanitize_summary(s: &str, max_chars: usize) -> String {
    let single_line = s.replace("\r\n", " ").replace(['\n', '\r'], " ");
    let count = single_line.chars().count();
    if count <= max_chars {
        single_line
    } else {
        let cut_len = max_chars.saturating_sub(1);
        let mut truncated: String = single_line.chars().take(cut_len).collect();
        truncated.push('…');
        truncated
    }
}

/// Sanitizes coalesced text: single-line (newlines replaced by spaces) taking the
/// last `max_chars` unicode characters received.
pub fn sanitize_text_summary(s: &str, max_chars: usize) -> String {
    let single_line = s.replace("\r\n", " ").replace(['\n', '\r'], " ");
    let count = single_line.chars().count();
    if count <= max_chars {
        single_line
    } else {
        single_line.chars().skip(count - max_chars).collect()
    }
}

/// Extracts a short label/summary string from an [`ActivityItem`].
pub fn activity_item_summary(item: &ActivityItem) -> String {
    match item {
        ActivityItem::Tool { name, target, .. } => {
            if target.is_empty() {
                name.clone()
            } else if target.starts_with(name) {
                target.clone()
            } else {
                format!("{name} {target}")
            }
        }
        ActivityItem::Thinking { text, .. } => {
            if text.is_empty() {
                "thinking".to_owned()
            } else {
                text.clone()
            }
        }
    }
}

/// Returns true if the [`ActivityItem`] represents a file-edit operation.
pub fn is_file_edit_activity(item: &ActivityItem) -> bool {
    match item {
        ActivityItem::Tool { name, .. } => {
            let n = name.trim().to_ascii_lowercase();
            n == "edit" || n == "file_edit" || n == "write_to_file" || n == "replace_file_content"
        }
        _ => false,
    }
}

/// Maps an [`AgentEvent`] to a [`ProgressLine`] if applicable.
/// TextDelta is handled via [`ProgressThrottle`].
/// Ignored events return `None`.
pub fn format_progress_event(agent: &str, event: &AgentEvent, now_ms: u64) -> Option<ProgressLine> {
    match event {
        AgentEvent::Started => Some(ProgressLine::new(now_ms, agent, "started", "started")),
        AgentEvent::ToolProgress { summary, state, .. } => {
            let text = format!("{state} {summary}");
            Some(ProgressLine::new(
                now_ms,
                agent,
                "tool",
                sanitize_summary(&text, MAX_SUMMARY_CHARS),
            ))
        }
        AgentEvent::Activity(item) => {
            let text = activity_item_summary(item);
            Some(ProgressLine::new(
                now_ms,
                agent,
                "activity",
                sanitize_summary(&text, MAX_SUMMARY_CHARS),
            ))
        }
        AgentEvent::Status(status) => Some(ProgressLine::new(
            now_ms,
            agent,
            "status",
            sanitize_summary(status, MAX_SUMMARY_CHARS),
        )),
        AgentEvent::Notice(notice) => Some(ProgressLine::new(
            now_ms,
            agent,
            "notice",
            sanitize_summary(notice, MAX_SUMMARY_CHARS),
        )),
        AgentEvent::Completed => Some(ProgressLine::new(now_ms, agent, "completed", "completed")),
        AgentEvent::Cancelled => Some(ProgressLine::new(now_ms, agent, "cancelled", "cancelled")),
        AgentEvent::Error(err) => Some(ProgressLine::new(
            now_ms,
            agent,
            "error",
            sanitize_summary(&err.message, MAX_SUMMARY_CHARS),
        )),
        AgentEvent::TextDelta(_) => None,
        AgentEvent::ProviderSessionBound(_)
        | AgentEvent::ModelsListed { .. }
        | AgentEvent::ModelSelected(_)
        | AgentEvent::EffortSelected(_) => None,
    }
}

/// Coalesces [`AgentEvent::TextDelta`] deltas per agent every 2 seconds, and
/// flushes remaining text before terminal events.
#[derive(Debug, Default)]
pub struct ProgressThrottle {
    states: HashMap<String, AgentTextState>,
}

#[derive(Debug, Default)]
struct AgentTextState {
    pending_text: String,
    last_emitted_ms: Option<u64>,
}

impl ProgressThrottle {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds streaming delta text for an agent at timestamp `now_ms`.
    /// Emits at most one `text` line per agent every 2 seconds.
    pub fn observe_text(&mut self, agent: &str, text: &str, now_ms: u64) -> Option<ProgressLine> {
        let state = self.states.entry(agent.to_owned()).or_default();
        state.pending_text.push_str(text);
        let should_emit = match state.last_emitted_ms {
            None => {
                state.last_emitted_ms = Some(now_ms);
                false
            }
            Some(last_ms) => now_ms.saturating_sub(last_ms) >= TEXT_COALESCE_INTERVAL_MS,
        };
        if should_emit && !state.pending_text.is_empty() {
            let summary = sanitize_text_summary(&state.pending_text, MAX_SUMMARY_CHARS);
            state.pending_text.clear();
            state.last_emitted_ms = Some(now_ms);
            Some(ProgressLine::new(now_ms, agent, "text", summary))
        } else {
            None
        }
    }

    /// Flushes any pending text for an agent before `completed`/`error`/`cancelled`.
    pub fn flush_agent(&mut self, agent: &str, now_ms: u64) -> Option<ProgressLine> {
        let state = self.states.get_mut(agent)?;
        if state.pending_text.is_empty() {
            return None;
        }
        let summary = sanitize_text_summary(&state.pending_text, MAX_SUMMARY_CHARS);
        state.pending_text.clear();
        state.last_emitted_ms = Some(now_ms);
        Some(ProgressLine::new(now_ms, agent, "text", summary))
    }

    /// Resets agent throttle state at timestamp `now_ms`.
    pub fn reset_agent(&mut self, agent: &str, now_ms: u64) {
        self.states.insert(
            agent.to_owned(),
            AgentTextState {
                pending_text: String::new(),
                last_emitted_ms: Some(now_ms),
            },
        );
    }
}

/// Tracks agent events and produces [`ProgressLine`]s for a progress sink.
#[derive(Debug, Default)]
pub struct ProgressTracker {
    throttle: ProgressThrottle,
}

impl ProgressTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observes an [`AgentEvent`] at `now_ms` and returns progress lines to emit.
    pub fn observe(&mut self, agent: &str, event: &AgentEvent, now_ms: u64) -> Vec<ProgressLine> {
        let mut lines = Vec::new();
        match event {
            AgentEvent::Started => {
                self.throttle.reset_agent(agent, now_ms);
                if let Some(line) = format_progress_event(agent, event, now_ms) {
                    lines.push(line);
                }
            }
            AgentEvent::TextDelta(text) => {
                if let Some(line) = self.throttle.observe_text(agent, text, now_ms) {
                    lines.push(line);
                }
            }
            AgentEvent::Completed | AgentEvent::Cancelled | AgentEvent::Error(_) => {
                if let Some(flushed) = self.throttle.flush_agent(agent, now_ms) {
                    lines.push(flushed);
                }
                if let Some(line) = format_progress_event(agent, event, now_ms) {
                    lines.push(line);
                }
            }
            AgentEvent::Notice(_)
            | AgentEvent::ToolProgress { .. }
            | AgentEvent::Activity(_)
            | AgentEvent::Status(_) => {
                if let Some(line) = format_progress_event(agent, event, now_ms) {
                    lines.push(line);
                }
            }
            AgentEvent::ProviderSessionBound(_)
            | AgentEvent::ModelsListed { .. }
            | AgentEvent::ModelSelected(_)
            | AgentEvent::EffortSelected(_) => {}
        }
        lines
    }

    #[allow(dead_code)]
    pub fn throttle(&mut self) -> &mut ProgressThrottle {
        &mut self.throttle
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentError, AgentEvent};
    use dmuxai::activity::{ActivityItem, ToolStatus};

    // TC-1772 / FR-1772: each AgentEvent kind maps to the documented JSON keys;
    // long/multiline summaries become one line <=160 chars cut with "…".
    #[test]
    fn tc_1772_progress_line_shape_and_cut() {
        let long_text = "a".repeat(200);
        let multiline_text = "line1\nline2\r\nline3";

        let events = vec![
            (AgentEvent::Started, "started", "started"),
            (
                AgentEvent::ToolProgress {
                    item_id: "t1".to_owned(),
                    summary: long_text.clone(),
                    state: "running".to_owned(),
                },
                "tool",
                "running aaaaaaaaa",
            ),
            (
                AgentEvent::Activity(ActivityItem::Tool {
                    name: "edit".to_owned(),
                    target: multiline_text.to_owned(),
                    status: ToolStatus::Running,
                    duration_ms: None,
                    detail: String::new(),
                }),
                "activity",
                "edit line1 line2 line3",
            ),
            (
                AgentEvent::Status("busy analyzing\nfiles".to_owned()),
                "status",
                "busy analyzing files",
            ),
            (
                AgentEvent::Notice(format!("notice:\n{long_text}")),
                "notice",
                "notice: aaaaa",
            ),
            (AgentEvent::Completed, "completed", "completed"),
            (AgentEvent::Cancelled, "cancelled", "cancelled"),
            (
                AgentEvent::Error(AgentError::provider(format!("failed:\n{long_text}"))),
                "error",
                "failed: aaaaa",
            ),
        ];

        for (event, expected_kind, prefix_check) in events {
            let line = format_progress_event("cursor", &event, 1729756800000)
                .expect("event should produce progress line");
            assert_eq!(line.line_type, "progress");
            assert_eq!(line.ts_ms, 1729756800000);
            assert_eq!(line.agent, "cursor");
            assert_eq!(line.kind, expected_kind);
            assert!(
                line.summary.starts_with(prefix_check),
                "summary: {}",
                line.summary
            );
            assert!(
                !line.summary.contains('\n'),
                "contains newline: {}",
                line.summary
            );
            assert!(
                !line.summary.contains('\r'),
                "contains carriage return: {}",
                line.summary
            );
            assert!(
                line.summary.chars().count() <= MAX_SUMMARY_CHARS,
                "length {} > 160",
                line.summary.chars().count()
            );

            // If long, verify it was cut with ellipsis
            if line.summary.chars().count() == MAX_SUMMARY_CHARS {
                assert!(line.summary.ends_with('…'));
            }

            // Verify serialization round-trips with documented keys:
            // {"type":"progress","ts_ms":<u64>,"agent":"<runner id>","kind":"<kind>","summary":"<text>"}
            let json_str = line.to_json();
            let val: serde_json::Value = serde_json::from_str(&json_str).unwrap();
            assert_eq!(val["type"], "progress");
            assert_eq!(val["ts_ms"], 1729756800000u64);
            assert_eq!(val["agent"], "cursor");
            assert_eq!(val["kind"], expected_kind);
            assert!(val["summary"].is_string());
        }

        // Ignored events
        assert!(
            format_progress_event(
                "cursor",
                &AgentEvent::ProviderSessionBound("s1".into()),
                100
            )
            .is_none()
        );
        assert!(
            format_progress_event(
                "cursor",
                &AgentEvent::ModelsListed {
                    models: vec![],
                    selected: None,
                    selected_effort: None
                },
                100
            )
            .is_none()
        );
        assert!(
            format_progress_event("cursor", &AgentEvent::ModelSelected("m".into()), 100).is_none()
        );
        assert!(
            format_progress_event("cursor", &AgentEvent::EffortSelected("e".into()), 100).is_none()
        );
    }

    // TC-1773 / FR-1773: many TextDeltas within 2 s -> one text line; flush before completed.
    #[test]
    fn tc_1773_progress_text_coalesced_every_2s() {
        let mut tracker = ProgressTracker::new();

        // Start agent at t = 1000ms
        let start_lines = tracker.observe("agy", &AgentEvent::Started, 1000);
        assert_eq!(start_lines.len(), 1);
        assert_eq!(start_lines[0].kind, "started");

        // 20 TextDeltas within 1500 ms (all within 2s of start)
        for i in 0..20 {
            let delta = format!("part{i} ");
            let lines = tracker.observe("agy", &AgentEvent::TextDelta(delta), 1000 + (i * 70));
            assert!(
                lines.is_empty(),
                "delta {i} should be coalesced without emitting immediately"
            );
        }

        // Now Completed arrives at t = 2400 (< 2000ms since last check, or before 2s boundary)
        let complete_lines = tracker.observe("agy", &AgentEvent::Completed, 2400);
        // Must emit the flushed text line first, then the completed line
        assert_eq!(complete_lines.len(), 2);
        assert_eq!(complete_lines[0].kind, "text");
        assert!(complete_lines[0].summary.contains("part0"));
        assert!(complete_lines[0].summary.contains("part19"));
        assert_eq!(complete_lines[1].kind, "completed");

        // Test long text coalescing: summary is the last 160 chars
        tracker.observe("agy", &AgentEvent::Started, 3000);
        let big_chunk = "0123456789".repeat(30); // 300 chars
        tracker.observe("agy", &AgentEvent::TextDelta(big_chunk), 3100);
        let flushed = tracker.observe("agy", &AgentEvent::Completed, 4000);
        assert_eq!(flushed.len(), 2);
        assert_eq!(flushed[0].kind, "text");
        assert_eq!(flushed[0].summary.chars().count(), 160);
        // It must be the last 160 characters
        assert!(flushed[0].summary.ends_with("0123456789"));
    }
}
