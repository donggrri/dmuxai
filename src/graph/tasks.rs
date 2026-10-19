use serde::{Deserialize, Serialize};

use super::types::{GraphNode, GraphNodeStatus, GraphPlan, GraphRun, PlanTask};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::types::{GraphLimits, NodeRole, TaskSize};

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
}
