//! Plain-text judge / triage lines for the TUI and dashboard (UX-930, UX-931, UX-937).

use std::collections::HashSet;

use unicode_width::UnicodeWidthChar;

use super::judge::{FallbackCause, OverrideKind, Route, TriageSource};
use super::plan_doc::PlanStep;
use super::tasks::task_counts;
use super::types::{GraphRun, GraphRunPhase, MAX_PLAN_REVISIONS, PlanRevision, PlanTask, Triage};

const JOB_INDENT: &str = "  ";

/// One rendered row in the plan approval panel (UX-931).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelLine {
    pub kind: PanelLineKind,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelLineKind {
    Header,
    Warning,
    Task,
    Step,
    Job,
    Note,
    Hint,
}

/// Graph status suffix when the run has tasks (UX-937).
pub fn task_counter_text(run: &GraphRun) -> Option<String> {
    if run.plan.tasks.is_empty() {
        return None;
    }
    let (done, total) = task_counts(&run.plan);
    Some(format!("tasks {done}/{total}"))
}

/// Plain-text approval panel rows for `awaiting_approval` (UX-931, FR-975).
pub fn approval_panel_lines(run: &GraphRun, width: usize) -> Vec<PanelLine> {
    if run.phase != GraphRunPhase::AwaitingApproval {
        return Vec::new();
    }
    let width = width.max(1);
    let approval = match run.approval.as_ref() {
        Some(approval) => approval,
        None => return Vec::new(),
    };
    let revision = match approval.revisions.last() {
        Some(revision) => revision,
        None => return Vec::new(),
    };
    let tasks = run
        .pending_replan_plan
        .as_ref()
        .map(|plan| plan.tasks.as_slice())
        .filter(|tasks| !tasks.is_empty())
        .unwrap_or(run.plan.tasks.as_slice());
    let steps = revision.steps.as_slice();

    let mut lines = Vec::new();
    let header = format!(
        "Plan rev {}/{} · {}",
        revision.n, MAX_PLAN_REVISIONS, revision.source
    );
    lines.push(panel_line(
        PanelLineKind::Header,
        ellipsize_to_width(&header, width),
    ));

    if revision.source == "fallback" {
        let cause = run
            .triage
            .as_ref()
            .and_then(|triage| triage.cause)
            .map(fallback_cause_label)
            .unwrap_or("error");
        let runner = run
            .triage
            .as_ref()
            .map(|triage| plain_label_text(&triage.runner))
            .unwrap_or_else(|| "agent".to_owned());
        let warning = format!("Fallback plan (heuristic, {cause}), not written by {runner}");
        lines.push(panel_line(
            PanelLineKind::Warning,
            ellipsize_to_width(&warning, width),
        ));
    }

    if tasks.is_empty() {
        for step in steps {
            push_step_block(&mut lines, revision, step, width);
        }
    } else {
        for task in tasks {
            let task_row = format!("{} {}", task.id, task.title);
            lines.push(panel_line(
                PanelLineKind::Task,
                ellipsize_to_width(&task_row, width),
            ));
            for step in steps_for_task(steps, &task.id) {
                push_step_block(&mut lines, revision, step, width);
            }
        }
        for step in unassigned_steps(steps, tasks) {
            push_step_block(&mut lines, revision, step, width);
        }
    }

    if let Some(notes) = revision.errors.as_ref() {
        for note in notes {
            let trimmed = note.trim();
            if trimmed.is_empty() {
                continue;
            }
            lines.push(panel_line(
                PanelLineKind::Note,
                ellipsize_to_width(trimmed, width),
            ));
        }
    }

    lines.push(panel_line(
        PanelLineKind::Hint,
        ellipsize_to_width(
            "/graph approve · /graph edit <feedback> · /graph reject",
            width,
        ),
    ));
    lines
}

fn panel_line(kind: PanelLineKind, text: String) -> PanelLine {
    PanelLine { kind, text }
}

fn steps_for_task<'a>(steps: &'a [PlanStep], task_id: &str) -> Vec<&'a PlanStep> {
    steps
        .iter()
        .filter(|step| step.task.as_deref() == Some(task_id))
        .collect()
}

fn unassigned_steps<'a>(steps: &'a [PlanStep], tasks: &[PlanTask]) -> Vec<&'a PlanStep> {
    let known: HashSet<&str> = tasks.iter().map(|task| task.id.as_str()).collect();
    steps
        .iter()
        .filter(|step| match step.task.as_deref() {
            None => true,
            Some(id) => !known.contains(id),
        })
        .collect()
}

fn push_step_block(
    lines: &mut Vec<PanelLine>,
    revision: &PlanRevision,
    step: &PlanStep,
    width: usize,
) {
    lines.push(panel_line(
        PanelLineKind::Step,
        ellipsize_to_width(&step_summary_line(revision, step), width),
    ));
    let indent_width = display_width(JOB_INDENT);
    let job_width = width.saturating_sub(indent_width).max(1);
    for chunk in wrap_display_text(&step.job, job_width) {
        let text = if chunk.is_empty() {
            JOB_INDENT.to_owned()
        } else {
            format!("{JOB_INDENT}{chunk}")
        };
        lines.push(panel_line(
            PanelLineKind::Job,
            ellipsize_to_width(&text, width),
        ));
    }
}

fn step_summary_line(revision: &PlanRevision, step: &PlanStep) -> String {
    let marker = step_change_marker(revision, &step.id);
    let model = step.model.as_deref().unwrap_or("");
    format!(
        "{}{} {} {}:{}{}",
        marker,
        step.id,
        step.role,
        step.runner,
        model,
        deps_suffix(&step.depends_on),
    )
}

fn step_change_marker(revision: &PlanRevision, step_id: &str) -> &'static str {
    if revision.added.iter().any(|id| id == step_id) {
        "+ "
    } else if revision.changed.iter().any(|id| id == step_id) {
        "~ "
    } else {
        ""
    }
}

fn deps_suffix(depends_on: &[String]) -> String {
    if depends_on.is_empty() {
        return String::new();
    }
    format!(" ← {}", depends_on.join(", "))
}

fn wrap_display_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![String::new()];
    }
    if text.is_empty() {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for ch in text.chars() {
        let ch = if ch == '\n' { ' ' } else { ch };
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if ch_width == 0 {
            current.push(ch);
            continue;
        }
        if current_width + ch_width > max_width && !current.is_empty() {
            lines.push(current);
            current = String::new();
            current_width = 0;
        }
        if ch_width > max_width {
            if !current.is_empty() {
                lines.push(current);
                current = String::new();
                current_width = 0;
            }
            lines.push(ch.to_string());
            continue;
        }
        current.push(ch);
        current_width += ch_width;
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

/// Full judge line for a finished triage (before width ellipsizing).
pub fn judge_line_text(triage: &Triage) -> String {
    match triage.source {
        TriageSource::Judge => {
            let runner = plain_label_text(&triage.runner);
            let model = plain_label_text(&triage.model);
            let reason = plain_reason_text(&triage.reason);
            if reason.is_empty() {
                format!(
                    "Judge ({} {}): {}",
                    runner,
                    model,
                    route_label(triage.route),
                )
            } else {
                format!(
                    "Judge ({} {}): {} — {}",
                    runner,
                    model,
                    route_label(triage.route),
                    reason,
                )
            }
        }
        TriageSource::Fallback => {
            let cause = triage.cause.map(fallback_cause_label).unwrap_or("error");
            format!(
                "Judge fallback: {} (heuristic) — {}",
                cause,
                route_label(triage.route),
            )
        }
        TriageSource::Override => {
            override_line_text(triage.override_kind, &plain_label_text(&triage.runner))
        }
    }
}

/// Line shown while the judge turn is in flight (UX-930).
pub fn judging_line_text(runner: &str, model: &str) -> String {
    format!(
        "Judging with {} {}…",
        plain_label_text(runner),
        plain_label_text(model),
    )
}

fn display_width(text: &str) -> usize {
    text.chars()
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0))
        .sum()
}

/// Cuts `text` to at most `max_width` display columns, appending `…` when truncated.
/// Never splits a Unicode scalar value.
pub fn ellipsize_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if display_width(text) <= max_width {
        return text.to_owned();
    }
    if max_width == 1 {
        return "…".to_owned();
    }
    let mut out = String::new();
    let mut used = 0usize;
    let budget = max_width.saturating_sub(1);
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > budget {
            out.push('…');
            break;
        }
        out.push(ch);
        used += ch_width;
    }
    out
}

fn route_label(route: Route) -> &'static str {
    match route {
        Route::Simple => "simple",
        Route::Complex => "complex",
    }
}

fn fallback_cause_label(cause: FallbackCause) -> &'static str {
    match cause {
        FallbackCause::Timeout => "timeout",
        FallbackCause::Error => "error",
        FallbackCause::InvalidVerdict => "invalid_verdict",
        FallbackCause::NoReadOnlyMode => "no_read_only_mode",
    }
}

fn override_line_text(kind: Option<OverrideKind>, runner: &str) -> String {
    match kind {
        Some(OverrideKind::Plan) => "/plan".to_owned(),
        Some(OverrideKind::Direct) => "/direct".to_owned(),
        Some(OverrideKind::Agent) => format!("agent {runner}"),
        Some(OverrideKind::NoAllowedRunners) => "no allowed runners".to_owned(),
        None => "override".to_owned(),
    }
}

/// Collapses control characters and line separators to spaces for plain-text display.
fn plain_reason_text(raw: &str) -> String {
    plain_label_text(raw)
}

fn plain_label_text(raw: &str) -> String {
    let normalized: String = raw
        .chars()
        .map(|ch| {
            if ch.is_control() || ch == '\u{2028}' || ch == '\u{2029}' {
                ' '
            } else {
                ch
            }
        })
        .collect();
    normalized.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::judge::TriageSource;

    fn sample_triage(source: TriageSource, reason: &str) -> Triage {
        Triage {
            route: Route::Simple,
            reason: reason.to_owned(),
            source,
            cause: None,
            override_kind: None,
            runner: "cursor".to_owned(),
            model: "composer".to_owned(),
            elapsed_ms: 0,
        }
    }

    #[test]
    fn judge_line_simple_and_complex() {
        let triage = sample_triage(TriageSource::Judge, "one file edit");
        assert_eq!(
            judge_line_text(&triage),
            "Judge (cursor composer): simple — one file edit"
        );
        let mut complex = triage;
        complex.route = Route::Complex;
        complex.reason = "multi-step".to_owned();
        assert_eq!(
            judge_line_text(&complex),
            "Judge (cursor composer): complex — multi-step"
        );
    }

    #[test]
    fn judge_line_normalizes_reason() {
        let triage = sample_triage(TriageSource::Judge, "line one\nline\ttwo");
        assert_eq!(
            judge_line_text(&triage),
            "Judge (cursor composer): simple — line one line two"
        );
    }

    #[test]
    fn judge_line_empty_reason_omits_separator() {
        let triage = sample_triage(TriageSource::Judge, "");
        assert_eq!(judge_line_text(&triage), "Judge (cursor composer): simple");
    }

    #[test]
    fn judge_line_fallback() {
        let triage = Triage {
            route: Route::Complex,
            reason: "heuristic fallback".to_owned(),
            source: TriageSource::Fallback,
            cause: Some(FallbackCause::Timeout),
            override_kind: None,
            runner: "cursor".to_owned(),
            model: String::new(),
            elapsed_ms: 1,
        };
        assert_eq!(
            judge_line_text(&triage),
            "Judge fallback: timeout (heuristic) — complex"
        );
    }

    #[test]
    fn judge_line_overrides() {
        let base = sample_triage(TriageSource::Override, "override");
        assert_eq!(judge_line_text(&base), "override");
        let mut direct = base.clone();
        direct.override_kind = Some(OverrideKind::Direct);
        assert_eq!(judge_line_text(&direct), "/direct");
        let mut plan = base.clone();
        plan.override_kind = Some(OverrideKind::Plan);
        assert_eq!(judge_line_text(&plan), "/plan");
        let mut agent = base.clone();
        agent.override_kind = Some(OverrideKind::Agent);
        agent.runner = "codex".to_owned();
        assert_eq!(judge_line_text(&agent), "agent codex");
        let mut blocked = base.clone();
        blocked.override_kind = Some(OverrideKind::NoAllowedRunners);
        assert_eq!(judge_line_text(&blocked), "no allowed runners");
    }

    #[test]
    fn judging_line_text_format() {
        assert_eq!(
            judging_line_text("cursor", "composer"),
            "Judging with cursor composer…"
        );
    }

    #[test]
    fn ellipsize_respects_display_width_and_korean() {
        let text = "가나다라";
        assert_eq!(ellipsize_to_width(text, 8), "가나다라");
        assert_eq!(ellipsize_to_width(text, 7), "가나다…");
        assert_eq!(ellipsize_to_width("hello", 3), "he…");
        assert_eq!(ellipsize_to_width("hello", 0), "");
        assert_eq!(ellipsize_to_width("hello", 1), "…");
        assert_eq!(ellipsize_to_width("abc", 2), "a…");
    }

    use super::{PanelLineKind, approval_panel_lines, task_counter_text, wrap_display_text};
    use crate::graph::types::{
        Approval, ApprovalState, GraphLimits, GraphNode, GraphNodeStatus, GraphPlan, GraphRun,
        GraphRunPhase, NodeRole, PlanRevision, PlanTask, TaskSize, Triage,
    };

    fn sample_step(id: &str, task: Option<&str>, job: &str, deps: &[&str]) -> PlanStep {
        PlanStep {
            id: id.to_owned(),
            role: "implement".to_owned(),
            job: job.to_owned(),
            runner: "codex".to_owned(),
            model: Some("composer-2.5".to_owned()),
            depends_on: deps.iter().map(|dep| (*dep).to_owned()).collect(),
            sensitive: None,
            task: task.map(str::to_owned),
            ..PlanStep::default()
        }
    }

    fn sample_task(id: &str, title: &str) -> PlanTask {
        PlanTask {
            id: id.to_owned(),
            title: title.to_owned(),
            done_check: "done".to_owned(),
            areas: Vec::new(),
            size: TaskSize::Small,
            depends_on: Vec::new(),
            implicit: false,
        }
    }

    fn awaiting_run(
        tasks: Vec<PlanTask>,
        _steps: Vec<PlanStep>,
        revision: PlanRevision,
        phase: GraphRunPhase,
    ) -> GraphRun {
        let mut plan = GraphPlan {
            schema: "dmuxai.graph/v1".to_owned(),
            id: "run-1".to_owned(),
            orchestrator_model: "m".to_owned(),
            user_prompt: "prompt".to_owned(),
            nodes: Vec::new(),
            edges: Vec::new(),
            limits: GraphLimits::default(),
            tasks,
        };
        if !plan.tasks.is_empty() {
            plan.nodes.push(GraphNode {
                id: "step-1".to_owned(),
                role: NodeRole::Implement,
                job: "j".to_owned(),
                runner: "codex".to_owned(),
                model_id: "m".to_owned(),
                skill: None,
                requires_gate: false,
                status: GraphNodeStatus::Completed,
                attempt: 0,
                task: Some("task-1".to_owned()),
                started_at_ms: None,
                finished_at_ms: None,
            });
        }
        GraphRun {
            plan,
            conversation_id: "c".to_owned(),
            phase,
            waiting_gate_node: None,
            triage: None,
            approval: Some(Approval {
                state: ApprovalState::Pending,
                revisions: vec![revision],
                decided_by: None,
                decided_at_ms: None,
            }),
            turn_index: 0,
            plan_history: Vec::new(),
            planner_calls: Default::default(),
            planner_io: Vec::new(),
            replans_used: 0,
            replan_chars_used: 0,
            orchestrate_started_ms: None,
            plan_fixed_ms: None,
            pending_replan_plan: None,
            todo_id: None,
        }
    }

    fn line_texts(lines: &[super::PanelLine]) -> Vec<String> {
        lines.iter().map(|line| line.text.clone()).collect()
    }

    // UX-931 / TC-958: header, deps, notes, hint.
    #[test]
    fn approval_panel_flat_steps_deps_notes_and_hint() {
        let revision = PlanRevision {
            n: 1,
            source: "agent".to_owned(),
            steps: vec![
                sample_step("step-1", None, "first job", &[]),
                sample_step("step-2", None, "second", &["step-1"]),
                sample_step("step-3", None, "third", &["step-2"]),
            ],
            added: Vec::new(),
            changed: Vec::new(),
            feedback: None,
            errors: Some(vec!["note one".to_owned(), "note two".to_owned()]),
            raw: None,
        };
        let run = awaiting_run(
            Vec::new(),
            revision.steps.clone(),
            revision,
            GraphRunPhase::AwaitingApproval,
        );
        let lines = approval_panel_lines(&run, 100);
        let texts = line_texts(&lines);
        assert_eq!(texts[0], "Plan rev 1/5 · agent");
        assert!(
            texts
                .iter()
                .any(|row| row.contains("step-2 implement codex:composer-2.5 ← step-1")),
            "{texts:?}"
        );
        assert!(texts.iter().any(|row| row == "note one"));
        assert!(texts.iter().any(|row| row == "note two"));
        assert!(
            texts
                .last()
                .unwrap()
                .contains("/graph approve · /graph edit <feedback> · /graph reject")
        );
        assert!(
            lines.iter().all(|line| {
                line.text.is_empty() || line.text.chars().all(|ch| ch != '\u{fffd}')
            })
        );
    }

    // UX-937 / FR-975: task grouping and Korean title cuts.
    #[test]
    fn approval_panel_groups_steps_under_tasks() {
        let long_title = "설계 문서를 읽고 요약하기";
        let revision = PlanRevision {
            n: 2,
            source: "edit".to_owned(),
            steps: vec![
                sample_step("step-a", Some("task-1"), "job a", &[]),
                sample_step("step-b", Some("task-2"), "job b", &[]),
                sample_step("step-c", None, "ungrouped", &[]),
            ],
            added: Vec::new(),
            changed: Vec::new(),
            feedback: None,
            errors: None,
            raw: None,
        };
        let tasks = vec![
            sample_task("task-1", long_title),
            sample_task("task-2", "테스트 작성"),
        ];
        let run = awaiting_run(
            tasks,
            revision.steps.clone(),
            revision,
            GraphRunPhase::AwaitingApproval,
        );
        let wide = line_texts(&approval_panel_lines(&run, 100));
        let task_one = wide
            .iter()
            .position(|row| row.starts_with("task-1 "))
            .expect("task-1 header");
        let task_two = wide
            .iter()
            .position(|row| row.starts_with("task-2 "))
            .expect("task-2 header");
        let step_a = wide
            .iter()
            .position(|row| row.starts_with("step-a "))
            .expect("step-a");
        let step_b = wide
            .iter()
            .position(|row| row.starts_with("step-b "))
            .expect("step-b");
        let step_c = wide
            .iter()
            .position(|row| row.starts_with("step-c "))
            .expect("ungrouped step");
        assert!(task_one < step_a && step_a < task_two && task_two < step_b && step_b < step_c);

        let at_40 = approval_panel_lines(&run, 40);
        for line in &at_40 {
            assert!(display_width(&line.text) <= 40, "{}", line.text);
        }
        let tight = approval_panel_lines(&run, 16);
        assert!(
            tight
                .iter()
                .any(|line| line.kind == PanelLineKind::Task && line.text.ends_with('…')),
            "{:?}",
            line_texts(&tight)
        );
    }

    #[test]
    fn approval_panel_fallback_warning_and_replan_markers() {
        let revision = PlanRevision {
            n: 1,
            source: "fallback".to_owned(),
            steps: vec![
                sample_step("step-1", None, "job", &[]),
                sample_step("step-2", None, "new", &[]),
            ],
            added: vec!["step-2".to_owned()],
            changed: vec!["step-1".to_owned()],
            feedback: None,
            errors: None,
            raw: None,
        };
        let mut run = awaiting_run(
            Vec::new(),
            revision.steps.clone(),
            revision,
            GraphRunPhase::AwaitingApproval,
        );
        run.triage = Some(Triage {
            route: Route::Complex,
            reason: String::new(),
            source: TriageSource::Fallback,
            cause: Some(FallbackCause::Timeout),
            override_kind: None,
            runner: "cursor".to_owned(),
            model: String::new(),
            elapsed_ms: 0,
        });
        let texts = line_texts(&approval_panel_lines(&run, 80));
        assert_eq!(
            texts[1],
            "Fallback plan (heuristic, timeout), not written by cursor"
        );
        assert!(texts.iter().any(|row| row.starts_with("~ step-1 ")));
        assert!(texts.iter().any(|row| row.starts_with("+ step-2 ")));
    }

    #[test]
    fn approval_panel_wraps_job_lines_and_omits_deps_arrow_when_empty() {
        let revision = PlanRevision {
            n: 1,
            source: "agent".to_owned(),
            steps: vec![sample_step("step-1", None, "abcdefghijklmnop", &[])],
            added: Vec::new(),
            changed: Vec::new(),
            feedback: None,
            errors: None,
            raw: None,
        };
        let run = awaiting_run(
            Vec::new(),
            revision.steps.clone(),
            revision,
            GraphRunPhase::AwaitingApproval,
        );
        let lines = approval_panel_lines(&run, 12);
        let job_lines: Vec<_> = lines
            .iter()
            .filter(|line| line.kind == PanelLineKind::Job)
            .map(|line| line.text.clone())
            .collect();
        assert!(job_lines.len() >= 2, "{job_lines:?}");
        assert!(job_lines.iter().all(|row| row.starts_with(JOB_INDENT)));
        assert!(!line_texts(&lines).iter().any(|row| row.contains("←")));
    }

    #[test]
    fn wrap_display_text_splits_on_char_boundaries() {
        assert_eq!(
            wrap_display_text("가나다라", 4),
            vec!["가나".to_owned(), "다라".to_owned()]
        );
        assert_eq!(
            wrap_display_text("abcd", 2),
            vec!["ab".to_owned(), "cd".to_owned()]
        );
    }

    #[test]
    fn wrap_display_text_keeps_zero_width_and_newlines() {
        let combining = "e\u{0301}";
        assert_eq!(wrap_display_text(combining, 1), vec![combining.to_owned()]);
        assert_eq!(wrap_display_text("a\nb", 4), vec!["a b".to_owned()]);
    }

    // UX-937: counter only when tasks exist.
    #[test]
    fn task_counter_text_tracks_done_tasks() {
        let revision = PlanRevision {
            n: 1,
            source: "agent".to_owned(),
            steps: vec![sample_step("step-1", Some("task-1"), "j", &[])],
            added: Vec::new(),
            changed: Vec::new(),
            feedback: None,
            errors: None,
            raw: None,
        };
        let tasks = vec![
            sample_task("task-1", "one"),
            sample_task("task-2", "two"),
            sample_task("task-3", "three"),
        ];
        let run = awaiting_run(
            tasks,
            revision.steps.clone(),
            revision,
            GraphRunPhase::AwaitingApproval,
        );
        assert_eq!(task_counter_text(&run), Some("tasks 1/3".to_owned()));
        let mut empty = run.clone();
        empty.plan.tasks.clear();
        assert_eq!(task_counter_text(&empty), None);
    }

    #[test]
    fn approval_panel_empty_when_not_awaiting() {
        let revision = PlanRevision {
            n: 1,
            source: "agent".to_owned(),
            steps: vec![sample_step("step-1", None, "j", &[])],
            added: Vec::new(),
            changed: Vec::new(),
            feedback: None,
            errors: None,
            raw: None,
        };
        let run = awaiting_run(
            Vec::new(),
            revision.steps.clone(),
            revision,
            GraphRunPhase::Active,
        );
        assert!(approval_panel_lines(&run, 80).is_empty());
    }
}
