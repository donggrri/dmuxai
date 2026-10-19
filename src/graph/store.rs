use std::{
    fs, io,
    path::{Path, PathBuf},
};

use super::types::GraphRun;

pub struct GraphStore {
    root: PathBuf,
}

impl GraphStore {
    pub fn new(workspace: &Path) -> Self {
        Self {
            root: workspace.join(".dmuxai").join("graphs"),
        }
    }

    pub fn save(&self, run: &GraphRun) -> io::Result<PathBuf> {
        fs::create_dir_all(&self.root)?;
        let record = cap_run_revision_raw(run);
        let path = self.root.join(format!("{}.json", record.plan.id));
        let json = serde_json::to_string_pretty(&record)?;
        fs::write(&path, json)?;
        Ok(path)
    }

    /// Every stored run record, skipping files that do not parse so one bad
    /// file never hides the rest.
    pub fn list_runs(&self) -> Vec<GraphRun> {
        let Ok(read_dir) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut runs = Vec::new();
        for entry in read_dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            if let Ok(contents) = fs::read_to_string(&path)
                && let Ok(run) = serde_json::from_str::<GraphRun>(&contents)
            {
                runs.push(run);
            }
        }
        runs
    }

    /// FR-954: how many runs are already stored for `conversation_id`.
    pub fn conversation_run_count(&self, conversation_id: &str) -> usize {
        self.list_runs()
            .iter()
            .filter(|run| run.conversation_id == conversation_id)
            .count()
    }
}

/// Replaces every failed-parse `raw` with its 4 KiB char-boundary cap so the
/// persisted record never stores oversized model text (FR-946).
fn cap_run_revision_raw(run: &GraphRun) -> GraphRun {
    let mut record = run.clone();
    if let Some(approval) = &mut record.approval {
        for revision in &mut approval.revisions {
            if let Some(raw) = &mut revision.raw {
                let capped = super::types::cap_revision_raw(raw);
                *raw = capped;
            }
        }
    }
    record
}

pub fn save_run(workspace: &Path, run: &GraphRun) -> io::Result<PathBuf> {
    GraphStore::new(workspace).save(run)
}

pub fn load_run(workspace: &Path, id: &str) -> io::Result<GraphRun> {
    let path = workspace
        .join(".dmuxai")
        .join("graphs")
        .join(format!("{id}.json"));
    let contents = fs::read_to_string(path)?;
    serde_json::from_str(&contents)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::super::judge::{FallbackCause, Route, TriageSource};
    use super::super::types::{
        Approval, ApprovalState, DecisionBy, GraphPlan, GraphRunPhase, PlanChange, PlanChangeKind,
        PlanChangeTrigger, PlanRevision, PlannerCalls, Triage,
    };
    use super::*;
    use tempfile::tempdir;

    fn run(id: &str) -> GraphRun {
        GraphRun {
            plan: GraphPlan {
                schema: "dmuxai.graph/v1".to_owned(),
                id: id.to_owned(),
                orchestrator_model: "m".to_owned(),
                user_prompt: "p".to_owned(),
                nodes: Vec::new(),
                edges: Vec::new(),
                limits: Default::default(),
                tasks: Vec::new(),
            },
            conversation_id: "conv".to_owned(),
            phase: GraphRunPhase::Active,
            waiting_gate_node: None,
            ..Default::default()
        }
    }

    // TC-953 (FR-946): a record written before triage/approval persistence
    // loads with defaults; a record with triage and a revision round-trips and
    // the failed-parse raw text stays at most 4 KiB cut on a char boundary.
    #[test]
    fn run_record_triage_and_approval_round_trip() {
        let dir = tempdir().unwrap();
        let legacy_json = r#"{
            "plan": {
                "schema": "dmuxai.graph/v1",
                "id": "legacy-run",
                "orchestrator_model": "m",
                "user_prompt": "p",
                "nodes": [],
                "edges": [],
                "limits": { "max_parallel": 2, "max_retries": 2 }
            },
            "conversation_id": "conv",
            "phase": "active",
            "waiting_gate_node": null
        }"#;
        let root = dir.path();
        let dir_path = root.join(".dmuxai").join("graphs");
        fs::create_dir_all(&dir_path).unwrap();
        fs::write(dir_path.join("legacy-run.json"), legacy_json).unwrap();
        let legacy = load_run(root, "legacy-run").unwrap();
        assert_eq!(legacy.turn_index, 0);
        assert!(legacy.triage.is_none());
        assert!(legacy.approval.is_none());
        assert!(legacy.plan_history.is_empty());
        assert_eq!(legacy.planner_calls, PlannerCalls::default());
        assert_eq!(legacy.plan.limits.max_replans, 3);
        assert_eq!(legacy.plan.limits.replan_budget_chars, 24_000);
        assert_eq!(legacy.plan.limits.max_nodes, 12);

        let long_text = "a".repeat(4100) + "é" + &"b".repeat(4100);
        let mut run = run("triaged-run");
        run.triage = Some(Triage {
            route: Route::Complex,
            reason: "multi step".to_owned(),
            source: TriageSource::Fallback,
            cause: Some(FallbackCause::Timeout),
            override_kind: None,
            runner: "codex".to_owned(),
            model: "m".to_owned(),
            elapsed_ms: 123,
        });
        run.approval = Some(Approval {
            state: ApprovalState::Rejected,
            revisions: vec![PlanRevision {
                n: 1,
                source: "fallback".to_owned(),
                steps: vec![Default::default()],
                feedback: None,
                errors: Some(vec!["not json".to_owned()]),
                raw: Some(long_text.clone()),
            }],
            decided_by: Some(DecisionBy::Dashboard),
            decided_at_ms: Some(1_700_000_000_000),
        });
        save_run(root, &run).unwrap();
        let loaded = load_run(root, "triaged-run").unwrap();
        assert_eq!(loaded.conversation_id, run.conversation_id);
        assert_eq!(loaded.triage, run.triage);
        let raw = loaded.approval.as_ref().unwrap().revisions[0]
            .raw
            .as_ref()
            .unwrap();
        assert!(raw.len() <= 4096, "raw must be cut to 4 KiB bytes");
        let expected = crate::graph::cap_revision_raw(&long_text.clone());
        assert_eq!(raw, &expected);
        assert_eq!(raw.len(), 4096);
        let raw = loaded.approval.unwrap().revisions[0].raw.take().unwrap();
        assert!(raw.len() <= 4096, "raw must be cut to 4 KiB bytes");
        assert!(
            raw.chars().all(|c| c.is_ascii_alphanumeric() || c == 'é'),
            "cut must not split a UTF-8 code point"
        );
    }

    // TC-977 store part (FR-960): plan_history round-trips with applied and
    // rejected entries, an old record without the field loads empty, and
    // pushes beyond 50 drop the oldest entry only.
    #[test]
    fn plan_history_round_trip() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let legacy_json = r#"{
            "plan": {
                "schema": "dmuxai.graph/v1",
                "id": "history-legacy",
                "orchestrator_model": "m",
                "user_prompt": "p",
                "nodes": [],
                "edges": [],
                "limits": { "max_parallel": 2, "max_retries": 2 }
            },
            "conversation_id": "conv",
            "phase": "active",
            "waiting_gate_node": null
        }"#;
        let dir_path = root.join(".dmuxai").join("graphs");
        fs::create_dir_all(&dir_path).unwrap();
        fs::write(dir_path.join("history-legacy.json"), legacy_json).unwrap();
        let legacy = load_run(root, "history-legacy").unwrap();
        assert!(legacy.plan_history.is_empty());

        let mut run = run("history-run");
        let applied = PlanChange {
            rev: 2,
            kind: PlanChangeKind::Replan,
            at_ms: 1_700_000_000_000,
            after_node: Some("node-a".to_owned()),
            reason: "step node-a failed".to_owned(),
            trigger: Some(PlanChangeTrigger::Failure),
            prompt_chars: Some(120),
            response_chars: Some(340),
            added: vec!["node-b".to_owned()],
            changed: vec!["node-a".to_owned()],
            removed: vec!["node-c".to_owned()],
            applied: true,
            error: None,
            decided_by: None,
        };
        let rejected = PlanChange {
            rev: 3,
            kind: PlanChangeKind::Replan,
            at_ms: 1_700_000_010_000,
            after_node: None,
            reason: "user asked".to_owned(),
            trigger: Some(PlanChangeTrigger::User),
            prompt_chars: None,
            response_chars: None,
            added: vec![],
            changed: vec![],
            removed: vec![],
            applied: false,
            error: Some("duplicate step id node-b".to_owned()),
            decided_by: Some(DecisionBy::Tui),
        };
        run.push_plan_change(applied.clone());
        run.push_plan_change(rejected.clone());
        save_run(root, &run).unwrap();
        let loaded = load_run(root, "history-run").unwrap();
        assert_eq!(loaded.plan_history, vec![applied, rejected]);

        run.push_plan_change(PlanChange {
            rev: 4,
            kind: PlanChangeKind::Edit,
            at_ms: 0,
            after_node: None,
            reason: "feedback".to_owned(),
            trigger: None,
            prompt_chars: None,
            response_chars: None,
            added: vec![],
            changed: vec![],
            removed: vec![],
            applied: true,
            error: None,
            decided_by: Some(DecisionBy::Cli),
        });
        let mut extra = run.clone();
        for rev in 5..=55 {
            extra.push_plan_change(PlanChange {
                rev,
                kind: PlanChangeKind::Replan,
                at_ms: 0,
                after_node: None,
                reason: "keep".to_owned(),
                trigger: None,
                prompt_chars: None,
                response_chars: None,
                added: vec![],
                changed: vec![],
                removed: vec![],
                applied: true,
                error: None,
                decided_by: None,
            });
        }
        assert_eq!(extra.plan_history.len(), 50);
        assert_eq!(
            extra.plan_history[0].rev,
            extra.plan_history.last().unwrap().rev.saturating_sub(49)
        );
        save_run(root, &extra).unwrap();
        let reloaded = load_run(root, "history-run").unwrap();
        assert_eq!(reloaded.plan_history.len(), 50);
        assert_eq!(reloaded.plan_history[0].rev, extra.plan_history[0].rev);
    }

    // TC-998 / FR-967: plan tasks round-trip; legacy records without task fields load.
    #[test]
    fn run_with_tasks_round_trips() {
        use super::super::types::{PlanTask, TaskSize};

        let dir = tempdir().unwrap();
        let root = dir.path();
        let mut run = run("tasks-run");
        run.plan.tasks = vec![
            PlanTask {
                id: "task-1".to_owned(),
                title: "first".to_owned(),
                done_check: "d1".to_owned(),
                areas: Vec::new(),
                size: TaskSize::Small,
                depends_on: Vec::new(),
                implicit: false,
            },
            PlanTask {
                id: "task-2".to_owned(),
                title: "second".to_owned(),
                done_check: "d2".to_owned(),
                areas: Vec::new(),
                size: TaskSize::Medium,
                depends_on: vec!["task-1".to_owned()],
                implicit: false,
            },
        ];
        save_run(root, &run).unwrap();
        let loaded = load_run(root, "tasks-run").unwrap();
        assert_eq!(loaded.plan.tasks, run.plan.tasks);

        let legacy_json = r#"{
            "plan": {
                "schema": "dmuxai.graph/v1",
                "id": "legacy-no-tasks",
                "orchestrator_model": "m",
                "user_prompt": "p",
                "nodes": [{
                    "id": "step-1",
                    "role": "implement",
                    "job": "work",
                    "runner": "cursor",
                    "model_id": "composer-2.5",
                    "requires_gate": false,
                    "status": "pending",
                    "attempt": 0
                }],
                "edges": [],
                "limits": { "max_parallel": 2, "max_retries": 2 }
            },
            "conversation_id": "conv",
            "phase": "active"
        }"#;
        let dir_path = root.join(".dmuxai").join("graphs");
        fs::write(dir_path.join("legacy-no-tasks.json"), legacy_json).unwrap();
        let legacy = load_run(root, "legacy-no-tasks").unwrap();
        assert!(legacy.plan.tasks.is_empty());
        assert!(legacy.plan.nodes[0].task.is_none());
    }
}
