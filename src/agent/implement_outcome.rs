use std::process::ExitStatus;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::{AgentError, TurnPolicy};

const AGY_WRITE_TOOL_NAMES: &[&str] = &[
    "write_to_file",
    "replace_file_content",
    "multi_replace_file_content",
    "sed_file",
    "notebook_edit",
    "edit_file",
    "write_file",
];

const AGY_PLAN_ARTIFACT_TOOL_NAMES: &[&str] = &[
    "plan",
    "artifact",
    "create_plan",
    "write_plan",
    "update_plan",
    "save_plan",
    "plan_artifact",
    "create_artifact",
    "write_artifact",
];

pub(crate) struct ImplementTurnTracker {
    policy: TurnPolicy,
    writes_observed: bool,
    plan_artifact_observed: bool,
}

impl ImplementTurnTracker {
    pub(crate) fn new(policy: TurnPolicy) -> Self {
        Self {
            policy,
            writes_observed: false,
            plan_artifact_observed: false,
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
        if self.policy == TurnPolicy::Agent {
            if agy_message_indicates_write(message) {
                self.writes_observed = true;
            }
            if agy_message_indicates_plan_artifact(message) {
                self.plan_artifact_observed = true;
            }
        }
    }

    pub(crate) fn writes_observed(&self) -> bool {
        self.writes_observed
    }

    /// FR-2121: a structural plan/artifact signal from the AGY stream. It is
    /// an additional positive signal for the headless exit-7 marker, never a
    /// requirement.
    pub(crate) fn plan_artifact_observed(&self) -> bool {
        self.plan_artifact_observed
    }

    pub(crate) fn finish(
        self,
        status: &ExitStatus,
        stderr: &str,
        protocol_completed: bool,
        require_writes: bool,
        exit_forced_after_result: bool,
    ) -> Result<(), AgentError> {
        if self.policy != TurnPolicy::Agent {
            if !exit_forced_after_result {
                if !status.success() && !stderr.trim().is_empty() {
                    return Err(AgentError::process_exit(stderr.trim()));
                }
                if !status.success() {
                    return Err(AgentError::process_exit(format!(
                        "provider exited with {status}"
                    )));
                }
            }
            return Ok(());
        }

        if stderr_indicates_permission_denial(stderr) {
            return Err(AgentError::policy(stderr.trim()));
        }
        if !status.success() && !exit_forced_after_result {
            return Err(if stderr.trim().is_empty() {
                AgentError::process_exit(format!(
                    "agent turn failed: provider exited with {status}"
                ))
            } else {
                AgentError::process_exit(stderr.trim())
            });
        }
        if !protocol_completed {
            return Err(if stderr.trim().is_empty() {
                AgentError::process_exit(
                    "agent turn failed: provider exited without a terminal result",
                )
            } else {
                AgentError::process_exit(stderr.trim())
            });
        }
        if require_writes && !self.writes_observed {
            return Err(AgentError::policy(
                "agent turn completed without any observed file changes; stderr had no writes",
            ));
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
    if !AGY_WRITE_TOOL_NAMES.contains(&tool_name) {
        return false;
    }
    if let Some(path) = agy_write_target_path(message)
        && is_agy_private_artifact_path(path)
    {
        return false;
    }
    true
}

fn agy_write_target_path(message: &Value) -> Option<&str> {
    const PATH_KEYS: &[&str] = &["Path", "TargetFile", "path", "file_path"];

    for key in PATH_KEYS {
        let ptr = format!("/step_update/tool_info/parameters/{key}");
        if let Some(path) = message.pointer(&ptr).and_then(Value::as_str) {
            return Some(path);
        }
    }
    for key in PATH_KEYS {
        let ptr = format!("/step_update/parameters/{key}");
        if let Some(path) = message.pointer(&ptr).and_then(Value::as_str) {
            return Some(path);
        }
    }
    None
}

/// FR-2121: AGY keeps plan artifacts there, outside the workspace.
pub(crate) fn is_agy_private_artifact_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized.contains("/.gemini/antigravity-cli/brain/")
        || normalized.contains("/.gemini/antigravity/brain/")
}

/// FR-2121: a structural "the agent produced a plan" signal in the AGY
/// stream. Any of these step shapes means the agent touched a plan artifact,
/// so the reply can be a plan review even when the text does not spell it out.
pub(crate) fn agy_message_indicates_plan_artifact(message: &Value) -> bool {
    if message.get("event").and_then(Value::as_str) != Some("step_update") {
        return false;
    }
    let step_type = message
        .pointer("/step_update/step_type")
        .and_then(Value::as_str)
        .unwrap_or("");
    if matches!(
        step_type,
        "plan" | "plan_step" | "artifact" | "artifact_update"
    ) {
        return true;
    }
    if step_type != "tool" && step_type != "tool_call" {
        return false;
    }
    let tool_name = message
        .pointer("/step_update/tool_name")
        .or_else(|| message.pointer("/step_update/tool_info/tool_name"))
        .or_else(|| message.pointer("/step_update/tool_info/name"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if AGY_PLAN_ARTIFACT_TOOL_NAMES
        .iter()
        .any(|name| tool_name.eq_ignore_ascii_case(name))
    {
        return true;
    }
    if AGY_WRITE_TOOL_NAMES.contains(&tool_name)
        && let Some(path) = agy_write_target_path(message)
        && is_agy_private_artifact_path(path)
    {
        return true;
    }
    false
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

/// FR-2121: the reply states that a plan exists (an artifact the agent wrote
/// or a plan it presents). Anchored so a bare "plan.md", a plan that already
/// existed, or a plan that is only mentioned in passing do not match.
static PLAN_STATEMENT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?im)",
        // the agent wrote a plan: created/wrote/drafted/saved ... plan
        r"\b(?:created|wrote|written|drafted|saved|added|prepared|produced)\b[^.!?\n]{0,60}\bplans?\b",
        // a plan file token is a plan statement even when a verb precedes it
        // across the dot inside the filename (implementation_plan.md)
        r"|\b\w*plans?\w*\.md(?:$|[^A-Za-z0-9_-])",
        // the agent presents a plan: here is / proposed / attached ... plan
        r"|\b(?:here(?:'s|\s+(?:is|are))|proposed|attached)\b[^.!?\n]{0,40}\bplans?\b",
        // the plan is presented below
        r"|\bplans?\b[^.!?\n]{0,40}\bbelow\b",
        // a plan heading introduces a plan ("Plan:")
        r"|^\s*(?:implementation\s+)?plans?\s*:",
        // "implementation plan below/here/created"
        r"|\bimplementation\s+plans?\b[^.!?\n]{0,60}\b(?:created|wrote|written|drafted|saved|added|prepared|produced|below|here)\b",
        // Korean: the agent wrote or laid out a plan (계획 작성/수립/만들/세웠/아래)
        r"|계획[^.\n]{0,20}(?:작성|수립|만들|세웠|아래)",
        r"|(?:작성|수립|만들|세웠|아래)[^.\n]{0,20}계획",
    ))
    .expect("valid plan statement regex")
});

/// FR-2121: the reply asks the user for an explicit approval or permission.
/// Anchored so a bare "review", "proceed", "confirm" or an instruction to the
/// user ("to proceed, run ...") does not count as an approval request.
static APPROVAL_REQUEST_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)",
        // please approve / confirm
        r"\bplease\s+(?:approve|confirm)\b",
        // please review it/this/the plan (not an unrelated "review the CI logs")
        r"|\bplease\s+review\s+(?:and\s+(?:approve|confirm)\s+)?(?:it|this|them|the\s+(?:plan|proposal|changes?))\b",
        // approve it / confirm this
        r"|\b(?:approve|confirm)\s+(?:it|this|them|the\s+plan)\b",
        // shall I ... / should I proceed
        r"|\bshall\s+i\b",
        r"|\bshould\s+i\b[^.!?\n]{0,40}\b(?:proceed|continue|start|begin|apply|make|edit|go)\b",
        // let me know if I should proceed / if it looks good
        r"|\blet\s+me\s+know\s+(?:if|whether)\s+i\s+should\b",
        r"|\blet\s+me\s+know\s+if\s+(?:it|this|that)\s+looks?\s+(?:good|fine|ok(?:ay)?)\b",
        // before I edit / before making changes
        r"|\bbefore\s+(?:i\s+)?(?:edit(?:s|ing)?|appl(?:y|ying)|mak(?:e|ing)\s+changes|chang(?:e|ing|es)|modif(?:y|ying)|writ(?:e|ing)|start(?:ing)?)\b",
        // once you approve
        r"|\bonce\s+you\s+(?:approve|confirm)\b",
        // waiting for (your) approval
        r"|\b(?:waiting|awaiting)\s+(?:for\s+)?(?:your\s+)?(?:approval|confirmation)\b",
        // Korean: explicit approval requests only (a bare 승인 is not enough)
        r"|승인해\s*주세요",
        r"|승인\s*(?:부탁|후|해\s*주시면)",
        r"|검토\s*후",
        r"|진행\s*(?:할까요|해도\s*될까요)",
        r"|확인해\s*주세요",
    ))
    .expect("valid approval request regex")
});

/// FR-2121: the reply states that a plan exists.
pub(crate) fn mentions_plan_statement(response: &str) -> bool {
    PLAN_STATEMENT_RE.is_match(response)
}

/// FR-2121: the reply asks the user for an explicit approval.
pub(crate) fn asks_for_approval(response: &str) -> bool {
    APPROVAL_REQUEST_RE.is_match(response)
}

/// FR-2121: a plan-only reply for the headless exit-7 marker needs both a
/// plan statement and an explicit approval request. An ordinary answer that
/// only mentions a plan, or only asks for a review, is not a plan-only reply.
pub(crate) fn is_plan_approval_request(response: &str) -> bool {
    mentions_plan_statement(response) && asks_for_approval(response)
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

    // TC-300 / FR-300: Stderr with permission denied triggers denial detection.
    #[test]
    fn tc_300_permission_denial_is_detected_in_stderr() {
        assert!(stderr_indicates_permission_denial(
            "error: permission denied: sandbox"
        ));
        assert!(!stderr_indicates_permission_denial("ok"));
    }

    // TC-301 / FR-301: Implement turn with no observable writes fails when require_writes is active.
    #[test]
    #[cfg(unix)]
    fn tc_301_implement_without_writes_fails() {
        let tracker = ImplementTurnTracker::new(TurnPolicy::Agent);
        let status = ExitStatus::from_raw(0);
        let error = tracker
            .finish(&status, "", true, true, false)
            .expect_err("should require writes");
        assert!(error.message.contains("without any observed file changes"));
    }

    // TC-302 / FR-302: ReadOnly / Ask mode does not require writes.
    #[test]
    #[cfg(unix)]
    fn tc_302_read_only_does_not_require_writes() {
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

    // TC-1860 / FR-1860: Permission denial diagnostics are detected in stderr patterns.
    #[test]
    fn tc_1860_permission_denial_patterns_detected_in_stderr() {
        for pattern in [
            "permission denied",
            "approval required",
            "read-only filesystem",
            "operation not permitted",
        ] {
            assert!(
                stderr_indicates_permission_denial(&format!("error: {pattern}")),
                "expected denial detected for pattern {pattern:?}"
            );
        }
        assert!(!stderr_indicates_permission_denial(
            "build succeeded without issues"
        ));
    }

    // TC-1861 / FR-1861: Missing observable writes ok when require_writes false, fails when true.
    #[test]
    #[cfg(unix)]
    fn tc_1861_implement_turn_tracker_require_writes_scope() {
        let status = ExitStatus::from_raw(0);

        // Interactive / conversational implement turn: require_writes = false -> succeeds
        let tracker_interactive = ImplementTurnTracker::new(TurnPolicy::Agent);
        assert!(
            tracker_interactive
                .finish(&status, "", true, false, false)
                .is_ok(),
            "when require_writes is false, missing writes should succeed"
        );

        // Graph implement step requiring edits: require_writes = true -> fails
        let tracker_graph_step = ImplementTurnTracker::new(TurnPolicy::Agent);
        let error = tracker_graph_step
            .finish(&status, "", true, true, false)
            .expect_err("when require_writes is true, missing writes must fail");
        assert!(error.message.contains("without any observed file changes"));
    }

    // TC-501 (FR-501): parse write tools from AGY step_update tool names.
    #[test]
    fn tc_501_agy_write_tools_are_detected_from_step_updates() {
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
    fn tc_501_init_line_does_not_count_as_write_via_text_scan() {
        let init_line = r#"{"event":"init","tools":["write_to_file","replace_file_content"]}"#;
        let mut tracker = ImplementTurnTracker::new(TurnPolicy::Agent);
        tracker.observe_provider_line(init_line);
        assert!(!tracker.writes_observed());
    }

    #[test]
    fn tc_501_provider_line_write_needles_cover_cursor_and_custom() {
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

    // TC-503 (FR-502): Plan-only classifier detects plan.md / review patterns.
    #[test]
    fn tc_503_plan_review_response_detection_covers_implement_plan_only() {
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
    fn tc_503_plan_only_outcome_requires_missing_writes_and_plan_like_text() {
        assert!(is_plan_only_implement_outcome(
            false,
            "Created plan.md — approve to proceed."
        ));
        assert!(!is_plan_only_implement_outcome(true, "Created plan.md"));
        assert!(!is_plan_only_implement_outcome(false, "Done."));
    }

    // TC-2145 / FR-2121: the exit-7 marker needs a plan statement *and* an
    // explicit approval request. Ordinary answers that merely mention a plan
    // are not plan-only replies.
    #[test]
    fn tc_2145_plan_approval_request_requires_plan_and_approval() {
        let ordinary_answers = [
            "I reviewed docs/plan.md. It has three phases; phase 2 lacks tests.",
            "The implementation plan in PLAN-x.md looks sound. Review notes: the error handling section is thin.",
            "Summary of the review: the proposed plan covers all FRs. No blockers.",
            "plan.md does not exist in this repo.",
            "To proceed, run cargo test. The implementation plan is tracked in issue #164.",
            "Step-by-step plan for deployment: 1) build 2) tag 3) push. You can review the CI logs afterwards.",
            "The repo has a ROADMAP.md and a plan.md under docs/plans.",
            "I could not find the bug. Please confirm which file you mean.",
            "Done. Updated src/conversation.rs and added a test.",
            "The code is fine; I recommend we proceed with merging.",
            // A lone Korean 승인 is not an approval request (issue title/verb usage).
            "PR 승인 방법: gh pr review --approve. 작성된 계획은 없습니다.",
            // The approval request must be about the plan, not another review.
            "The proposed plan in docs/plans/X.md is fine. Please review the CI logs too.",
        ];
        for answer in ordinary_answers {
            assert!(
                !is_plan_approval_request(answer),
                "ordinary answer must not be a plan approval request: {answer}"
            );
        }

        let plan_approval_replies = [
            "Created plan.md - please review and approve it.",
            "I've written an implementation plan to plan.md. Please review it and let me know if I should proceed.",
            "Here is the plan: 1. add the helper 2. wire it 3. test. Shall I proceed with these edits?",
            "I created an implementation plan artifact. Approve it and I will start editing.",
            "Proposed plan below. Before I edit any files, please confirm.",
            "구현 계획을 작성했습니다. 검토 후 승인해 주세요.",
            // Plan file tokens keep the statement despite the dot in the filename.
            "Created implementation_plan.md. Please review and approve it.",
            "구현 계획을 plan.md에 작성했습니다. 검토 후 승인해 주세요.",
            "구현 계획을 implementation_plan.md에 작성했습니다. 승인해 주세요.",
            // Korean plan verb (세웠습니다) plus 진행할까요.
            "다음과 같이 계획을 세웠습니다:\n1. A\n2. B\n진행할까요?",
            // "Let me know if it looks good" is an approval request.
            "I've put together a plan in implementation_plan.md. Let me know if it looks good.",
            // A plan heading is a plan statement.
            "Plan:\n1. A\n2. B\nWaiting for your approval before I start.",
        ];
        for reply in plan_approval_replies {
            assert!(
                is_plan_approval_request(reply),
                "plan approval request must be detected: {reply}"
            );
        }
    }

    // TC-2145 / FR-2121: a plan statement alone or an approval request alone
    // is not enough; both halves are required.
    #[test]
    fn plan_approval_request_needs_both_halves() {
        assert!(!is_plan_approval_request(
            "Created plan.md with the three phases."
        ));
        assert!(!is_plan_approval_request(
            "Please approve the change before I continue."
        ));
        assert!(!is_plan_approval_request(
            "I reviewed the plan and it looks good."
        ));
        assert!(is_plan_approval_request(
            "I drafted the plan in docs/plans/plan.md. Please review it."
        ));
    }

    // TC-2145 / FR-2121: a plan artifact step in the AGY stream is an
    // additional positive signal, never a requirement.
    #[test]
    fn agy_plan_artifact_steps_are_detected_from_step_updates() {
        let plan_step = json!({
            "event": "step_update",
            "step_update": {"step_type": "plan", "state": "DONE"}
        });
        let plan_tool = json!({
            "event": "step_update",
            "step_update": {"step_type": "tool", "tool_name": "create_plan"}
        });
        let read_tool = json!({
            "event": "step_update",
            "step_update": {"step_type": "tool", "tool_name": "read_file"}
        });
        // A tool name that merely *contains* "plan"/"artifact" is not a plan
        // artifact step; only exact plan-artifact tool names count.
        let read_plan_tool = json!({
            "event": "step_update",
            "step_update": {"step_type": "tool", "tool_name": "read_plan_file"}
        });
        let list_artifacts_tool = json!({
            "event": "step_update",
            "step_update": {"step_type": "tool", "tool_name": "list_artifacts"}
        });
        let write_plan_tool = json!({
            "event": "step_update",
            "step_update": {"step_type": "tool", "tool_name": "write_plan"}
        });
        assert!(agy_message_indicates_plan_artifact(&plan_step));
        assert!(agy_message_indicates_plan_artifact(&plan_tool));
        assert!(agy_message_indicates_plan_artifact(&write_plan_tool));
        assert!(!agy_message_indicates_plan_artifact(&read_tool));
        assert!(!agy_message_indicates_plan_artifact(&read_plan_tool));
        assert!(!agy_message_indicates_plan_artifact(&list_artifacts_tool));

        let mut tracker = ImplementTurnTracker::new(TurnPolicy::Agent);
        tracker.observe_agy_message(&read_tool);
        tracker.observe_agy_message(&read_plan_tool);
        assert!(!tracker.plan_artifact_observed());
        tracker.observe_agy_message(&plan_tool);
        assert!(tracker.plan_artifact_observed());
    }

    // FR-2121 / tc_2148
    #[test]
    fn tc_2148_informational_answers_are_not_plan_approval_requests() {
        let informational = [
            "The function parse_args reads argv and returns a Config. To proceed, run cargo test.",
            "I reviewed docs/plan.md. It has three phases; phase 2 lacks tests.",
            "There is no plan.md in this repository.",
            "The release plan is described in ROADMAP.md: v0.3 adds the dashboard.",
            "Yes. The build uses cargo; confirm the toolchain with rustup show.",
            "README.md explains the setup. Review the CI logs for the failing job.",
            "이 함수는 설정 파일을 읽습니다. 계획 문서는 ROADMAP.md에 있습니다.",
        ];
        for reply in informational {
            assert!(
                !is_plan_approval_request(reply),
                "informational answer must not be plan approval request: {reply}"
            );
        }
    }

    #[test]
    fn tc_2148_plan_approval_requests_are_detected() {
        let approval_requests = [
            "Created plan.md - please review and approve it.",
            "I wrote implementation_plan.md. Shall I proceed?",
            "Here is the plan:\n1. add flag\n2. add test\nLet me know if I should proceed.",
            "계획을 작성했습니다. 진행할까요?",
        ];
        for reply in approval_requests {
            assert!(
                is_plan_approval_request(reply),
                "plan approval request must be detected: {reply}"
            );
        }
    }

    #[test]
    fn tc_2148_writes_always_beat_plan_text() {
        assert!(!is_plan_only_implement_outcome(
            true,
            "Created plan.md - please review and approve it."
        ));
        assert!(is_plan_only_implement_outcome(
            false,
            "Created plan.md - please review and approve it."
        ));
    }

    #[test]
    fn tc_2149_private_artifact_writes_count_as_plan_artifact_not_workspace_write() {
        let artifact_line = json!({
            "event": "step_update",
            "step_update": {
                "step_type": "tool",
                "tool_info": {
                    "name": "write_file",
                    "parameters": {
                        "Path": "/home/box/.gemini/antigravity-cli/brain/bcb102cc-dcd5-4224-94f1-f8cf9c59e67c/implementation_plan.md"
                    }
                }
            }
        });

        assert!(!agy_message_indicates_write(&artifact_line));
        assert!(agy_message_indicates_plan_artifact(&artifact_line));

        let mut tracker = ImplementTurnTracker::new(TurnPolicy::Agent);
        tracker.observe_agy_message(&artifact_line);
        assert!(!tracker.writes_observed());
        assert!(tracker.plan_artifact_observed());

        let workspace_line = json!({
            "event": "step_update",
            "step_update": {
                "step_type": "tool",
                "tool_info": {
                    "name": "write_file",
                    "parameters": {
                        "Path": "/tmp/repo/src/main.rs"
                    }
                }
            }
        });
        assert!(agy_message_indicates_write(&workspace_line));
        assert!(!agy_message_indicates_plan_artifact(&workspace_line));

        let no_params_line = json!({
            "event": "step_update",
            "step_update": {
                "step_type": "tool",
                "tool_info": {
                    "name": "write_file"
                }
            }
        });
        assert!(agy_message_indicates_write(&no_params_line));
    }
}
