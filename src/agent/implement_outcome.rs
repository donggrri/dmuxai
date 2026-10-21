use std::process::ExitStatus;

use serde_json::Value;

use super::TurnPolicy;

const AGY_WRITE_TOOL_NAMES: &[&str] = &[
    "write_to_file",
    "replace_file_content",
    "multi_replace_file_content",
    "sed_file",
    "notebook_edit",
    "edit_file",
    "write_file",
];

pub(crate) struct ImplementTurnTracker {
    policy: TurnPolicy,
    writes_observed: bool,
}

impl ImplementTurnTracker {
    pub(crate) fn new(policy: TurnPolicy) -> Self {
        Self {
            policy,
            writes_observed: false,
        }
    }

    pub(crate) fn observe_provider_line(&mut self, line: &str) {
        if self.policy != TurnPolicy::Agent {
            return;
        }
        if provider_line_suggests_write(line) {
            self.writes_observed = true;
        }
    }

    pub(crate) fn observe_agy_message(&mut self, message: &Value) {
        if self.policy == TurnPolicy::Agent && agy_message_indicates_write(message) {
            self.writes_observed = true;
        }
    }

    pub(crate) fn writes_observed(&self) -> bool {
        self.writes_observed
    }

    pub(crate) fn finish(
        self,
        status: &ExitStatus,
        stderr: &str,
        protocol_completed: bool,
        require_writes: bool,
        exit_forced_after_result: bool,
    ) -> Result<(), String> {
        if self.policy != TurnPolicy::Agent {
            if !exit_forced_after_result {
                if !status.success() && !stderr.trim().is_empty() {
                    return Err(stderr.trim().to_owned());
                }
                if !status.success() {
                    return Err(format!("provider exited with {status}"));
                }
            }
            return Ok(());
        }

        if stderr_indicates_permission_denial(stderr) {
            return Err(stderr.trim().to_owned());
        }
        if !status.success() && !exit_forced_after_result {
            return Err(if stderr.trim().is_empty() {
                format!("agent turn failed: provider exited with {status}")
            } else {
                stderr.trim().to_owned()
            });
        }
        if !protocol_completed {
            return Err(if stderr.trim().is_empty() {
                "agent turn failed: provider exited without a terminal result".to_owned()
            } else {
                stderr.trim().to_owned()
            });
        }
        if require_writes && !self.writes_observed {
            return Err(
                "agent turn completed without any observed file changes; stderr had no writes"
                    .to_owned(),
            );
        }
        Ok(())
    }
}

pub(crate) fn agy_message_indicates_write(message: &Value) -> bool {
    if message.get("event").and_then(Value::as_str) != Some("step_update") {
        return false;
    }
    let step_type = message
        .pointer("/step_update/step_type")
        .and_then(Value::as_str);
    if step_type != Some("tool") && step_type != Some("tool_call") {
        return false;
    }
    let tool_name = message
        .pointer("/step_update/tool_name")
        .or_else(|| message.pointer("/step_update/tool_info/tool_name"))
        .or_else(|| message.pointer("/step_update/tool_info/name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    AGY_WRITE_TOOL_NAMES.contains(&tool_name)
}

pub(crate) fn is_plan_review_response(response: &str) -> bool {
    let response = response.to_ascii_lowercase();
    let mentions_plan = response.contains("plan.md")
        || response.contains("implementation plan")
        || response.contains("plan artifact")
        || response.contains("here is the plan")
        || response.contains("proposed plan")
        || response.contains("plan-only")
        || (response.contains("plan") && response.contains("step-by-step"));
    let requests_review = response.contains("approve")
        || response.contains("review")
        || response.contains("proceed")
        || response.contains("confirm")
        || response.contains("before i edit")
        || response.contains("before editing")
        || response.contains("shall i");
    mentions_plan && (requests_review || response.contains("plan.md"))
}

pub(crate) fn is_plan_only_implement_outcome(writes_observed: bool, response: &str) -> bool {
    !writes_observed && is_plan_review_response(response)
}

pub(crate) fn stderr_indicates_permission_denial(stderr: &str) -> bool {
    let haystack = stderr.to_ascii_lowercase();
    [
        "permission denied",
        "not permitted",
        "approval required",
        "requires approval",
        "read-only",
        "read only",
        "sandbox",
        "eacces",
        "operation not permitted",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

fn provider_line_suggests_write(line: &str) -> bool {
    let haystack = line.to_ascii_lowercase();
    [
        "\"filechange\"",
        "file_change",
        "apply_patch",
        "strreplace",
        "create_file",
        "\"type\":\"write\"",
        "edit_file",
        "write_file",
        "accept-edits",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;

    use serde_json::json;

    #[test]
    fn permission_denial_is_detected_in_stderr() {
        assert!(stderr_indicates_permission_denial(
            "error: permission denied: sandbox"
        ));
        assert!(!stderr_indicates_permission_denial("ok"));
    }

    #[test]
    #[cfg(unix)]
    fn implement_without_writes_fails() {
        let tracker = ImplementTurnTracker::new(TurnPolicy::Agent);
        let status = ExitStatus::from_raw(0);
        let error = tracker
            .finish(&status, "", true, true, false)
            .expect_err("should require writes");
        assert!(error.contains("without any observed file changes"));
    }

    #[test]
    #[cfg(unix)]
    fn read_only_does_not_require_writes() {
        let tracker = ImplementTurnTracker::new(TurnPolicy::ReadOnly);
        let status = ExitStatus::from_raw(0);
        assert!(tracker.finish(&status, "", true, true, false).is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn finish_accepts_forced_kill_after_result_in_agent_mode() {
        let killed = ExitStatus::from_raw(9);

        let mut tracker = ImplementTurnTracker::new(TurnPolicy::Agent);
        tracker.observe_provider_line(r#"{"name":"create_file"}"#);
        assert!(tracker.writes_observed());
        assert!(
            tracker.finish(&killed, "", true, true, true).is_ok(),
            "forced kill after a successful result should finish Ok"
        );

        let mut tracker = ImplementTurnTracker::new(TurnPolicy::Agent);
        tracker.observe_provider_line(r#"{"name":"run_command"}"#);
        assert!(tracker.finish(&killed, "", true, true, true).is_err());

        let mut tracker = ImplementTurnTracker::new(TurnPolicy::Agent);
        tracker.observe_provider_line(r#"{"name":"create_file"}"#);
        assert!(
            tracker.finish(&killed, "", true, true, false).is_err(),
            "without the forced-kill flag a signal exit should still fail"
        );
    }

    #[test]
    fn agy_write_tools_are_detected_from_step_updates() {
        let write = json!({
            "event": "step_update",
            "step_update": {
                "step_type": "tool",
                "tool_name": "write_to_file",
                "state": "ACTIVE"
            }
        });
        let init = json!({
            "event": "init",
            "tools": ["write_to_file", "replace_file_content", "run_command"]
        });
        let run = json!({
            "event": "step_update",
            "step_update": {
                "step_type": "tool",
                "tool_name": "run_command",
                "state": "ACTIVE"
            }
        });
        assert!(agy_message_indicates_write(&write));
        assert!(!agy_message_indicates_write(&init));
        let mut tracker = ImplementTurnTracker::new(TurnPolicy::Agent);
        tracker.observe_provider_line(&init.to_string());
        assert!(!tracker.writes_observed());
        assert!(!agy_message_indicates_write(&run));

        let mut tracker = ImplementTurnTracker::new(TurnPolicy::Agent);
        tracker.observe_agy_message(&write);
        assert!(tracker.writes_observed());
    }

    #[test]
    fn init_line_does_not_count_as_write_via_text_scan() {
        let init_line = r#"{"event":"init","tools":["write_to_file","replace_file_content"]}"#;
        let mut tracker = ImplementTurnTracker::new(TurnPolicy::Agent);
        tracker.observe_provider_line(init_line);
        assert!(!tracker.writes_observed());
    }

    #[test]
    fn provider_line_write_needles_cover_cursor_and_custom() {
        let cursor_edit = r#"{"type":"tool_call","tool":{"name":"edit_file","args":{}}}"#;
        let cursor_write = r#"{"type":"tool_call","tool":{"name":"write_file","args":{}}}"#;
        let assistant_text = r#"{"type":"assistant","text":"Updated the configuration file"}"#;

        let mut tracker = ImplementTurnTracker::new(TurnPolicy::Agent);
        tracker.observe_provider_line(cursor_edit);
        assert!(tracker.writes_observed(), "edit_file should mark a write");

        let mut tracker = ImplementTurnTracker::new(TurnPolicy::Agent);
        tracker.observe_provider_line(cursor_write);
        assert!(tracker.writes_observed(), "write_file should mark a write");

        let mut tracker = ImplementTurnTracker::new(TurnPolicy::Agent);
        tracker.observe_provider_line(assistant_text);
        assert!(
            !tracker.writes_observed(),
            "plain assistant text should not mark a write"
        );

        let mut tracker = ImplementTurnTracker::new(TurnPolicy::ReadOnly);
        tracker.observe_provider_line(cursor_edit);
        tracker.observe_provider_line(cursor_write);
        assert!(
            !tracker.writes_observed(),
            "read-only turns never record writes"
        );
    }

    #[test]
    fn plan_review_response_detection_covers_implement_plan_only() {
        assert!(is_plan_review_response(
            "I drafted the implementation plan in plan.md. Please review before I apply edits."
        ));
        assert!(is_plan_review_response(
            "Here is the plan. Shall I proceed with the file changes?"
        ));
        assert!(!is_plan_review_response(
            "Applied edits to src/lib.rs successfully."
        ));
    }

    #[test]
    fn plan_only_outcome_requires_missing_writes_and_plan_like_text() {
        assert!(is_plan_only_implement_outcome(
            false,
            "Created plan.md — approve to proceed."
        ));
        assert!(!is_plan_only_implement_outcome(true, "Created plan.md"));
        assert!(!is_plan_only_implement_outcome(false, "Done."));
    }
}
