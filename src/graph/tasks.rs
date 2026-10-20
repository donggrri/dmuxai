use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::plan_doc::{PlanContext, PlanDoc, PlanError, PlanStep, validate_plan_with_limit};
use super::types::{GraphNode, GraphNodeStatus, GraphPlan, GraphRun, PlanTask};

/// Local task edit at the approval gate (FR-965).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskEdit {
    Rename {
        task_id: String,
        title: String,
    },
    Delete {
        task_id: String,
    },
    Merge {
        into: String,
        from: String,
    },
    /// `position` is 1-based in the task list.
    Move {
        task_id: String,
        position: usize,
    },
    /// Model edit turn (FR-965); handled in the app, not [`TaskEdit::apply`].
    Split {
        task_id: String,
        note: String,
    },
}

impl TaskEdit {
    pub fn apply(
        self,
        steps: &[PlanStep],
        tasks: &[PlanTask],
        ctx: &PlanContext<'_>,
        max_steps: usize,
    ) -> Result<(Vec<PlanStep>, Vec<PlanTask>), String> {
        match self {
            Self::Rename { task_id, title } => {
                apply_task_rename(steps, tasks, &task_id, &title, ctx, max_steps)
            }
            Self::Delete { task_id } => apply_task_delete(steps, tasks, &task_id, ctx, max_steps),
            Self::Merge { into, from } => {
                apply_task_merge(steps, tasks, &into, &from, ctx, max_steps)
            }
            Self::Move { task_id, position } => {
                apply_task_move(steps, tasks, &task_id, position, ctx, max_steps)
            }
            Self::Split { .. } => Err("split is not a local task edit".to_owned()),
        }
    }
}

fn first_validation_error(errors: Vec<PlanError>) -> String {
    errors
        .first()
        .map(|err| err.to_string())
        .unwrap_or_else(|| "plan validation failed".to_owned())
}

fn revalidate_revision(
    steps: Vec<PlanStep>,
    tasks: Vec<PlanTask>,
    ctx: &PlanContext<'_>,
    max_steps: usize,
) -> Result<(Vec<PlanStep>, Vec<PlanTask>), String> {
    if tasks.is_empty() {
        return Err("zero tasks remaining".to_owned());
    }
    let doc = PlanDoc {
        steps,
        tasks,
        require_tasks: true,
        task_errors: Vec::new(),
    };
    validate_plan_with_limit(&doc, ctx, max_steps)
        .map(|_| (doc.steps.clone(), doc.tasks.clone()))
        .map_err(first_validation_error)
}

fn task_index(tasks: &[PlanTask], id: &str) -> Result<usize, String> {
    tasks
        .iter()
        .position(|task| task.id == id)
        .ok_or_else(|| format!("unknown task '{id}'"))
}

fn renumber_tasks_and_refs(
    tasks: Vec<PlanTask>,
    steps: Vec<PlanStep>,
) -> (Vec<PlanTask>, Vec<PlanStep>) {
    let id_map: HashMap<String, String> = tasks
        .iter()
        .enumerate()
        .map(|(index, task)| (task.id.clone(), format!("task-{}", index + 1)))
        .collect();
    let map_dep = |dep: &String| id_map.get(dep).cloned().unwrap_or_else(|| dep.clone());
    let tasks = tasks
        .into_iter()
        .enumerate()
        .map(|(index, mut task)| {
            task.id = format!("task-{}", index + 1);
            let mut seen_deps = HashSet::new();
            task.depends_on = task
                .depends_on
                .iter()
                .map(map_dep)
                .filter(|dep| dep != &task.id)
                .filter(|dep| seen_deps.insert(dep.clone()))
                .collect();
            task
        })
        .collect();
    let steps = steps
        .into_iter()
        .map(|mut step| {
            if let Some(task) = step.task.clone() {
                step.task = Some(map_dep(&task));
            }
            step
        })
        .collect();
    (tasks, steps)
}

pub fn apply_task_rename(
    steps: &[PlanStep],
    tasks: &[PlanTask],
    task_id: &str,
    title: &str,
    ctx: &PlanContext<'_>,
    max_steps: usize,
) -> Result<(Vec<PlanStep>, Vec<PlanTask>), String> {
    let index = task_index(tasks, task_id)?;
    let mut tasks = tasks.to_vec();
    tasks[index].title = title.to_owned();
    revalidate_revision(steps.to_vec(), tasks, ctx, max_steps)
}

pub fn apply_task_delete(
    steps: &[PlanStep],
    tasks: &[PlanTask],
    task_id: &str,
    ctx: &PlanContext<'_>,
    max_steps: usize,
) -> Result<(Vec<PlanStep>, Vec<PlanTask>), String> {
    task_index(tasks, task_id)?;
    let removed_step_ids: HashSet<String> = steps
        .iter()
        .filter(|step| step.task.as_deref() == Some(task_id))
        .map(|step| step.id.clone())
        .collect();
    let tasks: Vec<PlanTask> = tasks
        .iter()
        .filter(|task| task.id != task_id)
        .map(|task| {
            let mut task = task.clone();
            task.depends_on.retain(|dep| dep != task_id);
            task
        })
        .collect();
    if tasks.is_empty() {
        return Err("zero tasks remaining".to_owned());
    }
    let steps: Vec<PlanStep> = steps
        .iter()
        .filter(|step| step.task.as_deref() != Some(task_id))
        .map(|step| {
            let mut step = step.clone();
            step.depends_on
                .retain(|dep| !removed_step_ids.contains(dep));
            step
        })
        .collect();
    let (tasks, steps) = renumber_tasks_and_refs(tasks, steps);
    revalidate_revision(steps, tasks, ctx, max_steps)
}

pub fn apply_task_merge(
    steps: &[PlanStep],
    tasks: &[PlanTask],
    into: &str,
    from: &str,
    ctx: &PlanContext<'_>,
    max_steps: usize,
) -> Result<(Vec<PlanStep>, Vec<PlanTask>), String> {
    if into == from {
        return Err("cannot merge a task into itself".to_owned());
    }
    let into_index = task_index(tasks, into)?;
    let from_index = task_index(tasks, from)?;
    let into_task = &tasks[into_index];
    let from_task = &tasks[from_index];
    let mut merged_deps = Vec::new();
    let mut seen_deps = HashSet::new();
    for dep in into_task
        .depends_on
        .iter()
        .chain(from_task.depends_on.iter())
    {
        if dep == into || dep == from {
            continue;
        }
        if seen_deps.insert(dep.clone()) {
            merged_deps.push(dep.clone());
        }
    }
    let tasks: Vec<PlanTask> = tasks
        .iter()
        .filter(|task| task.id != from)
        .map(|task| {
            let mut task = task.clone();
            if task.id == into {
                task.depends_on = merged_deps.clone();
            } else {
                let mut deps = Vec::new();
                let mut seen = HashSet::new();
                for dep in &task.depends_on {
                    let mapped = if dep == from {
                        into.to_owned()
                    } else {
                        dep.clone()
                    };
                    if mapped == from || !seen.insert(mapped.clone()) {
                        continue;
                    }
                    deps.push(mapped);
                }
                task.depends_on = deps;
            }
            task
        })
        .collect();
    let steps: Vec<PlanStep> = steps
        .iter()
        .map(|step| {
            let mut step = step.clone();
            if step.task.as_deref() == Some(from) {
                step.task = Some(into.to_owned());
            }
            step
        })
        .collect();
    let (tasks, steps) = renumber_tasks_and_refs(tasks, steps);
    revalidate_revision(steps, tasks, ctx, max_steps)
}

pub fn apply_task_move(
    steps: &[PlanStep],
    tasks: &[PlanTask],
    task_id: &str,
    position: usize,
    ctx: &PlanContext<'_>,
    max_steps: usize,
) -> Result<(Vec<PlanStep>, Vec<PlanTask>), String> {
    let from_index = task_index(tasks, task_id)?;
    if position == 0 || position > tasks.len() {
        return Err(format!("position must be 1 to {}", tasks.len()));
    }
    let to_index = position - 1;
    let mut tasks = tasks.to_vec();
    let moved = tasks.remove(from_index);
    tasks.insert(to_index, moved);
    let (tasks, steps) = renumber_tasks_and_refs(tasks, steps.to_vec());
    revalidate_revision(steps, tasks, ctx, max_steps)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Pending,
    Running,
    Done,
    Failed,
}

/// FR-966: derived task status in plan task list order.
pub fn task_status(plan: &GraphPlan) -> Vec<(PlanTask, TaskState)> {
    plan.tasks
        .iter()
        .map(|task| {
            let nodes: Vec<&GraphNode> = plan
                .nodes
                .iter()
                .filter(|node| node.task.as_deref() == Some(task.id.as_str()))
                .collect();
            (task.clone(), derive_task_state(&nodes))
        })
        .collect()
}

/// Task list deltas for a local approval-gate edit (FR-965).
pub fn diff_plan_tasks(previous: &[PlanTask], next: &[PlanTask]) -> (usize, usize, usize) {
    let previous_ids: HashMap<&str, &PlanTask> = previous
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect();
    let next_ids: HashSet<&str> = next.iter().map(|task| task.id.as_str()).collect();
    let mut added = 0usize;
    let mut changed = 0usize;
    for task in next.iter() {
        match previous_ids.get(task.id.as_str()) {
            None => added += 1,
            Some(old) if **old != *task => changed += 1,
            _ => {}
        }
    }
    let removed = previous
        .iter()
        .filter(|task| !next_ids.contains(task.id.as_str()))
        .count();
    (added, changed, removed)
}

pub fn task_counts(plan: &GraphPlan) -> (usize, usize) {
    let statuses = task_status(plan);
    let total = statuses.len();
    let done = statuses
        .iter()
        .filter(|(_, state)| *state == TaskState::Done)
        .count();
    (done, total)
}

fn derive_task_state(nodes: &[&GraphNode]) -> TaskState {
    if nodes.is_empty() {
        return TaskState::Pending;
    }
    if nodes.iter().any(|node| {
        matches!(
            node.status,
            GraphNodeStatus::Failed | GraphNodeStatus::Skipped
        )
    }) {
        return TaskState::Failed;
    }
    if nodes.iter().any(|node| node_is_active(node)) {
        return TaskState::Running;
    }
    if nodes
        .iter()
        .all(|node| node.status == GraphNodeStatus::Completed)
    {
        return TaskState::Done;
    }
    TaskState::Pending
}

fn node_is_active(node: &GraphNode) -> bool {
    matches!(
        node.status,
        GraphNodeStatus::Running | GraphNodeStatus::WaitingGate
    ) || (node.status == GraphNodeStatus::Ready && node.started_at_ms.is_some())
}

impl GraphRun {
    pub fn task_status(&self) -> Vec<(PlanTask, TaskState)> {
        task_status(&self.plan)
    }
}

/// Path for FR-967 tasks markdown: `<workspace>/.dmuxai/tasks/TASKS-<run id>.md`.
pub fn tasks_file_path(workspace: &Path, run: &GraphRun) -> PathBuf {
    workspace
        .join(".dmuxai")
        .join("tasks")
        .join(format!("TASKS-{}.md", run.plan.id))
}

/// FR-969: 1-node implicit-only runs do not get a tasks file.
pub fn should_write_tasks_file(run: &GraphRun) -> bool {
    run.plan.tasks.iter().any(|task| !task.implicit)
}

fn task_state_label(state: TaskState) -> &'static str {
    match state {
        TaskState::Pending => "pending",
        TaskState::Running => "running",
        TaskState::Done => "done",
        TaskState::Failed => "failed",
    }
}

/// UTF-8 checklist markdown for a run (FR-967); never includes prompt or node output.
/// Collapses whitespace and line breaks so each task stays one markdown line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn render_tasks_markdown(run: &GraphRun) -> String {
    let mut out = format!("# Tasks {}\n", run.plan.id);
    for (task, state) in run.task_status() {
        if task.implicit {
            continue;
        }
        let checkbox = if state == TaskState::Done {
            "[x]"
        } else {
            "[ ]"
        };
        out.push_str(&format!(
            "- {} {} {} — done: {} ({})\n",
            checkbox,
            task.id,
            one_line(&task.title),
            one_line(&task.done_check),
            task_state_label(state)
        ));
    }
    out
}

/// When `enabled` is false or the run has only implicit tasks, does nothing.
pub fn write_tasks_file_if_enabled(
    workspace: &Path,
    enabled: bool,
    run: &GraphRun,
) -> io::Result<Option<PathBuf>> {
    if !enabled || !should_write_tasks_file(run) {
        return Ok(None);
    }
    write_tasks_file(workspace, run).map(Some)
}

/// Creates `.dmuxai/tasks` and atomically writes `TASKS-<run id>.md`.
pub fn write_tasks_file(workspace: &Path, run: &GraphRun) -> io::Result<PathBuf> {
    if !should_write_tasks_file(run) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no exportable tasks",
        ));
    }
    let dir = workspace.join(".dmuxai").join("tasks");
    fs::create_dir_all(&dir)?;
    let path = tasks_file_path(workspace, run);
    let body = render_tasks_markdown(run);
    let temp_path = dir.join(format!(
        ".TASKS-{}.md.tmp.{}",
        run.plan.id,
        std::process::id()
    ));
    let _ = fs::remove_file(&temp_path);
    {
        let mut temp = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        temp.write_all(body.as_bytes())?;
        temp.flush()?;
    }
    if let Err(error) = fs::rename(&temp_path, &path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::types::{GraphLimits, GraphRun, GraphRunPhase, NodeRole, PlanTask, TaskSize};

    fn node(id: &str, task: &str, status: GraphNodeStatus) -> GraphNode {
        GraphNode {
            id: id.to_owned(),
            role: NodeRole::Implement,
            job: "j".to_owned(),
            runner: "cursor".to_owned(),
            model_id: "m".to_owned(),
            skill: None,
            requires_gate: false,
            status,
            attempt: 0,
            task: Some(task.to_owned()),
            started_at_ms: None,
            finished_at_ms: None,
        }
    }

    fn plan(nodes: Vec<GraphNode>, tasks: Vec<PlanTask>) -> GraphPlan {
        GraphPlan {
            schema: "dmuxai.graph/v1".to_owned(),
            id: "p".to_owned(),
            orchestrator_model: "m".to_owned(),
            user_prompt: "u".to_owned(),
            nodes,
            edges: Vec::new(),
            limits: GraphLimits::default(),
            tasks,
        }
    }

    fn task(id: &str) -> PlanTask {
        PlanTask {
            id: id.to_owned(),
            title: id.to_owned(),
            done_check: "d".to_owned(),
            areas: Vec::new(),
            size: TaskSize::Small,
            depends_on: Vec::new(),
            implicit: false,
        }
    }

    fn state_for(plan: &GraphPlan, task_id: &str) -> TaskState {
        task_status(plan)
            .into_iter()
            .find(|(t, _)| t.id == task_id)
            .map(|(_, s)| s)
            .expect("task")
    }

    // TC-995 / FR-966: task status follows node states; task_counts tracks done.
    #[test]
    fn task_status_follows_nodes() {
        let tasks = vec![task("task-1"), task("task-2")];
        let empty = plan(Vec::new(), tasks.clone());
        assert_eq!(state_for(&empty, "task-1"), TaskState::Pending);
        assert_eq!(state_for(&empty, "task-2"), TaskState::Pending);
        assert_eq!(task_counts(&empty), (0, 2));

        let failed = plan(
            vec![node("a", "task-1", GraphNodeStatus::Failed)],
            vec![task("task-1")],
        );
        assert_eq!(state_for(&failed, "task-1"), TaskState::Failed);

        let skipped = plan(
            vec![node("a", "task-1", GraphNodeStatus::Skipped)],
            vec![task("task-1")],
        );
        assert_eq!(state_for(&skipped, "task-1"), TaskState::Failed);

        let running = plan(
            vec![node("a", "task-1", GraphNodeStatus::Running)],
            vec![task("task-1")],
        );
        assert_eq!(state_for(&running, "task-1"), TaskState::Running);

        let gate = plan(
            vec![node("a", "task-1", GraphNodeStatus::WaitingGate)],
            vec![task("task-1")],
        );
        assert_eq!(state_for(&gate, "task-1"), TaskState::Running);

        let ready_started = {
            let mut n = node("a", "task-1", GraphNodeStatus::Ready);
            n.started_at_ms = Some(1);
            plan(vec![n], vec![task("task-1")])
        };
        assert_eq!(state_for(&ready_started, "task-1"), TaskState::Running);

        let ready_fresh = plan(
            vec![node("a", "task-1", GraphNodeStatus::Ready)],
            vec![task("task-1")],
        );
        assert_eq!(state_for(&ready_fresh, "task-1"), TaskState::Pending);

        let done = plan(
            vec![
                node("a", "task-1", GraphNodeStatus::Completed),
                node("b", "task-1", GraphNodeStatus::Completed),
            ],
            vec![task("task-1")],
        );
        assert_eq!(state_for(&done, "task-1"), TaskState::Done);
        assert_eq!(task_counts(&done), (1, 1));

        let mixed_running = plan(
            vec![
                node("a", "task-1", GraphNodeStatus::Completed),
                node("b", "task-1", GraphNodeStatus::Running),
            ],
            vec![task("task-1")],
        );
        assert_eq!(state_for(&mixed_running, "task-1"), TaskState::Running);

        let mixed_pending = plan(
            vec![
                node("a", "task-1", GraphNodeStatus::Completed),
                node("b", "task-1", GraphNodeStatus::Pending),
            ],
            vec![task("task-1")],
        );
        assert_eq!(state_for(&mixed_pending, "task-1"), TaskState::Pending);

        let failed_wins = plan(
            vec![
                node("a", "task-1", GraphNodeStatus::Completed),
                node("b", "task-1", GraphNodeStatus::Failed),
            ],
            vec![task("task-1")],
        );
        assert_eq!(state_for(&failed_wins, "task-1"), TaskState::Failed);
    }

    type RunnerAllowed = Box<dyn Fn(&str) -> bool>;
    type EffectiveModel = Box<dyn Fn(&str) -> Option<String>>;

    struct PlanCtx {
        available: Vec<String>,
        runner_allowed: RunnerAllowed,
        effective_model: EffectiveModel,
    }

    impl PlanCtx {
        fn new(available: &[&str]) -> Self {
            PlanCtx {
                available: available.iter().map(|name| (*name).to_owned()).collect(),
                runner_allowed: Box::new(|_| true),
                effective_model: Box::new(|_| Some("composer-2.5".to_owned())),
            }
        }

        fn context<'a>(&'a self, user_prompt: &'a str) -> PlanContext<'a> {
            PlanContext {
                user_prompt,
                available_runners: &self.available,
                runner_allowed: self.runner_allowed.as_ref(),
                effective_model: self.effective_model.as_ref(),
                validate_model: &crate::model_policy::validate_model_for_runner,
            }
        }
    }

    fn plan_task(id: &str, title: &str, depends_on: &[&str]) -> PlanTask {
        PlanTask {
            id: id.to_owned(),
            title: title.to_owned(),
            done_check: "done".to_owned(),
            areas: Vec::new(),
            size: TaskSize::Small,
            depends_on: depends_on.iter().map(|dep| (*dep).to_owned()).collect(),
            implicit: false,
        }
    }

    fn plan_step(id: &str, task: &str, depends_on: &[&str]) -> PlanStep {
        PlanStep {
            id: id.to_owned(),
            role: "implement".to_owned(),
            job: id.to_owned(),
            runner: "cursor".to_owned(),
            model: None,
            depends_on: depends_on.iter().map(|dep| (*dep).to_owned()).collect(),
            sensitive: None,
            task: Some(task.to_owned()),
            sensitive_invalid: false,
            depends_on_invalid: false,
            model_invalid: false,
            id_invalid: false,
            not_object: false,
        }
    }

    fn sample_revision() -> (Vec<PlanStep>, Vec<PlanTask>) {
        let tasks = vec![
            plan_task("task-1", "One", &[]),
            plan_task("task-2", "Two", &[]),
            plan_task("task-3", "Three", &["task-1", "task-2"]),
        ];
        let steps = vec![
            plan_step("impl-1", "task-1", &[]),
            plan_step("impl-2", "task-2", &[]),
            plan_step("impl-3", "task-3", &["impl-1", "impl-2"]),
        ];
        (steps, tasks)
    }

    // TC-994 / FR-965: rename, delete, merge, move renumber refs; refuse unknown id and delete-all.
    #[test]
    fn task_edits_rebuild_plan() {
        let ctx = PlanCtx::new(&["cursor"]);
        let plan_ctx = ctx.context("build it");
        let max_steps = 8;
        let (steps, tasks) = sample_revision();
        let (steps, tasks) =
            apply_task_move(&steps, &tasks, "task-3", 1, &plan_ctx, max_steps).expect("move");
        assert_eq!(tasks[0].id, "task-1");
        assert_eq!(tasks[0].title, "Three");
        assert_eq!(tasks[1].id, "task-2");
        assert_eq!(tasks[2].id, "task-3");
        assert_eq!(
            tasks[0].depends_on,
            vec!["task-2".to_owned(), "task-3".to_owned()]
        );
        assert_eq!(
            steps
                .iter()
                .find(|step| step.id == "impl-1")
                .and_then(|step| step.task.as_deref()),
            Some("task-2")
        );
        assert_eq!(
            steps
                .iter()
                .find(|step| step.id == "impl-2")
                .and_then(|step| step.task.as_deref()),
            Some("task-3")
        );
        assert_eq!(
            steps
                .iter()
                .find(|step| step.id == "impl-3")
                .and_then(|step| step.task.as_deref()),
            Some("task-1")
        );

        let (steps, tasks) = sample_revision();
        let (_steps, tasks) =
            apply_task_merge(&steps, &tasks, "task-1", "task-2", &plan_ctx, max_steps)
                .expect("merge deps collapse");
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[1].id, "task-2");
        assert_eq!(tasks[1].depends_on, vec!["task-1".to_owned()]);

        let tasks = vec![
            plan_task("task-1", "One", &[]),
            plan_task("task-2", "Two", &["task-1"]),
            plan_task("task-3", "Three", &["task-2"]),
        ];
        let steps = vec![
            plan_step("impl-1", "task-1", &[]),
            plan_step("impl-2", "task-2", &["impl-1"]),
            plan_step("impl-3", "task-3", &["impl-2"]),
        ];
        let (_steps, tasks) =
            apply_task_merge(&steps, &tasks, "task-2", "task-1", &plan_ctx, max_steps)
                .expect("merge into depends on from");
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "task-1");
        assert_eq!(tasks[0].depends_on, Vec::<String>::new());
        assert_eq!(tasks[1].depends_on, vec!["task-1".to_owned()]);

        let (steps, tasks) = sample_revision();
        assert_eq!(
            apply_task_merge(&steps, &tasks, "task-1", "task-1", &plan_ctx, max_steps),
            Err("cannot merge a task into itself".to_owned())
        );

        let (steps, tasks) = sample_revision();

        let (steps, tasks) = apply_task_rename(
            &steps,
            &tasks,
            "task-2",
            "Two renamed",
            &plan_ctx,
            max_steps,
        )
        .expect("rename");
        assert_eq!(tasks[1].title, "Two renamed");

        let (steps, tasks) =
            apply_task_delete(&steps, &tasks, "task-1", &plan_ctx, max_steps).expect("delete");
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().all(|task| task.title != "One"));
        assert!(steps.iter().all(|step| step.id != "impl-1"));
        assert_eq!(tasks[0].id, "task-1");
        assert_eq!(tasks[1].id, "task-2");
        assert_eq!(tasks[1].depends_on, vec!["task-1".to_owned()]);
        assert_eq!(
            steps
                .iter()
                .find(|step| step.id == "impl-3")
                .map(|step| step.depends_on.clone()),
            Some(vec!["impl-2".to_owned()])
        );

        let (steps, tasks) =
            apply_task_merge(&steps, &tasks, "task-1", "task-2", &plan_ctx, max_steps)
                .expect("merge");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "task-1");
        assert!(
            steps
                .iter()
                .all(|step| step.task.as_deref() == Some("task-1"))
        );

        let (steps, tasks) =
            apply_task_move(&steps, &tasks, "task-1", 1, &plan_ctx, max_steps).expect("move");
        assert_eq!(tasks[0].id, "task-1");
        assert!(
            steps
                .iter()
                .all(|step| step.task.as_deref() == Some("task-1"))
        );

        let (steps, tasks) = sample_revision();
        assert_eq!(
            apply_task_rename(&steps, &tasks, "task-9", "nope", &plan_ctx, max_steps),
            Err("unknown task 'task-9'".to_owned())
        );

        let (steps, tasks) = sample_revision();
        let (steps, tasks) =
            apply_task_delete(&steps, &tasks, "task-1", &plan_ctx, max_steps).unwrap();
        let (steps, tasks) =
            apply_task_delete(&steps, &tasks, "task-2", &plan_ctx, max_steps).unwrap();
        assert_eq!(
            apply_task_delete(&steps, &tasks, "task-1", &plan_ctx, max_steps),
            Err("zero tasks remaining".to_owned())
        );
    }

    // TC-998 / FR-967: tasks file path, markdown shape, setting gate, persistence.
    #[test]
    fn tasks_file_setting_and_persistence() {
        use std::fs;

        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();

        fn sample_run() -> GraphRun {
            let tasks = vec![
                PlanTask {
                    id: "task-1".to_owned(),
                    title: "First".to_owned(),
                    done_check: "tests pass".to_owned(),
                    areas: Vec::new(),
                    size: TaskSize::Small,
                    depends_on: Vec::new(),
                    implicit: false,
                },
                PlanTask {
                    id: "task-2".to_owned(),
                    title: "Second".to_owned(),
                    done_check: "docs updated".to_owned(),
                    areas: Vec::new(),
                    size: TaskSize::Small,
                    depends_on: Vec::new(),
                    implicit: false,
                },
            ];
            let nodes = vec![
                node("n1", "task-1", GraphNodeStatus::Pending),
                node("n2", "task-2", GraphNodeStatus::Pending),
            ];
            GraphRun {
                plan: plan(nodes, tasks),
                conversation_id: "c".to_owned(),
                phase: GraphRunPhase::Active,
                waiting_gate_node: None,
                ..Default::default()
            }
        }

        let mut run = sample_run();
        run.plan.id = "run-a1b2".to_owned();
        run.plan.user_prompt = "SECRET_USER_PROMPT_DO_NOT_LEAK".to_owned();

        let path = tasks_file_path(workspace, &run);
        assert_eq!(
            path,
            workspace
                .join(".dmuxai")
                .join("tasks")
                .join("TASKS-run-a1b2.md")
        );

        assert!(
            write_tasks_file_if_enabled(workspace, false, &run)
                .unwrap()
                .is_none()
        );
        assert!(!path.exists());

        let written = write_tasks_file_if_enabled(workspace, true, &run)
            .unwrap()
            .expect("written");
        assert_eq!(written, path);
        let approved = fs::read_to_string(&path).unwrap();
        assert!(approved.contains("# Tasks run-a1b2"));
        assert!(approved.contains("- [ ] task-1 First — done: tests pass (pending)"));
        assert!(approved.contains("- [ ] task-2 Second — done: docs updated (pending)"));
        assert!(!approved.contains("SECRET_USER_PROMPT"));
        assert!(!approved.contains("user_prompt"));

        run.plan.nodes[0].status = GraphNodeStatus::Completed;
        write_tasks_file(workspace, &run).unwrap();
        let after_done = fs::read_to_string(&path).unwrap();
        assert!(after_done.contains("- [x] task-1 First — done: tests pass (done)"));
        assert!(after_done.contains("- [ ] task-2 Second — done: docs updated (pending)"));

        let implicit_only = {
            let mut r = sample_run();
            r.plan.id = "implicit-run".to_owned();
            r.plan.tasks = vec![PlanTask {
                id: "task-1".to_owned(),
                title: "implicit".to_owned(),
                done_check: "the agent turn completes".to_owned(),
                areas: Vec::new(),
                size: TaskSize::Small,
                depends_on: Vec::new(),
                implicit: true,
            }];
            r.plan.nodes = vec![node("n1", "task-1", GraphNodeStatus::Pending)];
            r
        };
        assert!(!should_write_tasks_file(&implicit_only));
        assert!(
            write_tasks_file_if_enabled(workspace, true, &implicit_only)
                .unwrap()
                .is_none()
        );
        assert!(!tasks_file_path(workspace, &implicit_only).exists());

        let err_dir = tempfile::tempdir().unwrap();
        let err_ws = err_dir.path();
        fs::create_dir_all(err_ws.join(".dmuxai")).unwrap();
        fs::write(err_ws.join(".dmuxai").join("tasks"), "blocked").unwrap();
        let err = write_tasks_file(err_ws, &run).unwrap_err();
        assert_ne!(err.kind(), io::ErrorKind::InvalidInput);

        let json = serde_json::to_string(&run).unwrap();
        let loaded: GraphRun = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.plan.tasks, run.plan.tasks);

        let legacy_json = r#"{
            "plan": {
                "schema": "dmuxai.graph/v1",
                "id": "legacy",
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
        let legacy: GraphRun = serde_json::from_str(legacy_json).unwrap();
        assert!(legacy.plan.tasks.is_empty());
    }
}
