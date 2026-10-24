use std::collections::HashMap;
use std::sync::Arc;

use dmuxai::conversation::TurnId as StoredTurnId;
use dmuxai::graph::GraphRun;
use dmuxai::todo::TodoStore;

use crate::agent::Agent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TodoFinish {
    NotLinked,
    Finished,
    Failed(String),
}

#[derive(Default)]
pub(crate) struct TodoLinks {
    pub(crate) store: Option<Arc<TodoStore>>,
    pub(crate) turns: HashMap<(Agent, StoredTurnId), String>,
    pub(crate) graph: Option<(String, String)>,
    pub(crate) pending_graph: Option<String>,
    pub(crate) pending_run_id: Option<String>,
    pub(crate) submitting_run: bool,
}

impl TodoLinks {
    pub(crate) fn attach_store(&mut self, store: Arc<TodoStore>) {
        self.store = Some(store);
    }

    pub(crate) fn store(&self) -> Option<&Arc<TodoStore>> {
        self.store.as_ref()
    }

    pub(crate) fn link_turn(
        &mut self,
        agent: Agent,
        turn_id: StoredTurnId,
        todo_id: impl Into<String>,
    ) {
        self.turns.insert((agent, turn_id), todo_id.into());
    }

    pub(crate) fn finish_turn(
        &mut self,
        agent: Agent,
        turn_id: &StoredTurnId,
        success: bool,
        error: Option<String>,
    ) -> TodoFinish {
        let Some(todo_id) = self.turns.remove(&(agent, turn_id.clone())) else {
            return TodoFinish::NotLinked;
        };
        if let Some(store) = &self.store {
            match store.finish_run(&todo_id, success, error) {
                Ok(_) => TodoFinish::Finished,
                Err(err) => TodoFinish::Failed(err.to_string()),
            }
        } else {
            TodoFinish::Finished
        }
    }

    pub(crate) fn begin_pending_graph(
        &mut self,
        todo_id: impl Into<String>,
        run_ref: impl Into<String>,
    ) {
        self.pending_graph = Some(todo_id.into());
        self.pending_run_id = Some(run_ref.into());
    }

    #[allow(dead_code)]
    pub(crate) fn clear_pending_graph(&mut self) {
        self.pending_graph = None;
        self.pending_run_id = None;
    }

    #[allow(dead_code)]
    pub(crate) fn pending_run_id(&self) -> Option<&str> {
        self.pending_run_id.as_deref()
    }

    #[allow(dead_code)]
    pub(crate) fn pending_graph(&self) -> Option<&str> {
        self.pending_graph.as_deref()
    }

    pub(crate) fn todo_for_new_run(&self, plan_id: impl AsRef<str>) -> Option<String> {
        let plan_id = plan_id.as_ref();
        self.pending_graph.clone().or_else(|| {
            self.graph
                .as_ref()
                .filter(|(bound, _)| bound == plan_id)
                .map(|(_, todo_id)| todo_id.clone())
        })
    }

    pub(crate) fn commit_pending_graph(&mut self, plan_id: impl AsRef<str>, run: &mut GraphRun) {
        let plan_id = plan_id.as_ref();
        let pending = self.pending_graph.take();
        self.pending_run_id = None;
        if run.todo_id.is_none() {
            run.todo_id = pending;
        }
        if self.graph.is_none()
            && let Some(todo_id) = run.todo_id.clone()
        {
            self.graph = Some((plan_id.to_owned(), todo_id));
        }
    }

    pub(crate) fn finish_graph_for_plan(
        &mut self,
        plan_id: impl AsRef<str>,
        success: bool,
        error: Option<String>,
    ) -> TodoFinish {
        let plan_id = plan_id.as_ref();
        let todo_id = match &self.graph {
            Some((bound, id)) if bound == plan_id => Some(id.clone()),
            _ => None,
        };
        let Some(todo_id) = todo_id else {
            return TodoFinish::NotLinked;
        };
        self.graph = None;
        if let Some(store) = &self.store {
            match store.finish_run(&todo_id, success, error) {
                Ok(_) => TodoFinish::Finished,
                Err(err) => TodoFinish::Failed(err.to_string()),
            }
        } else {
            TodoFinish::Finished
        }
    }

    pub(crate) fn finish_graph(&mut self, success: bool, error: Option<String>) -> TodoFinish {
        let Some((_plan_id, todo_id)) = self.graph.take() else {
            return TodoFinish::NotLinked;
        };
        if let Some(store) = &self.store {
            match store.finish_run(&todo_id, success, error) {
                Ok(_) => TodoFinish::Finished,
                Err(err) => TodoFinish::Failed(err.to_string()),
            }
        } else {
            TodoFinish::Finished
        }
    }

    pub(crate) fn finish_linked(
        &mut self,
        success: bool,
        error: Option<String>,
        last_run_todo: Option<String>,
    ) -> TodoFinish {
        let todo_id = self
            .graph
            .as_ref()
            .map(|(_, id)| id.clone())
            .or_else(|| self.pending_graph.clone())
            .or(last_run_todo);
        let Some(todo_id) = todo_id else {
            return TodoFinish::NotLinked;
        };
        self.graph = None;
        self.pending_graph = None;
        self.pending_run_id = None;
        if let Some(store) = &self.store {
            match store.finish_run(&todo_id, success, error) {
                Ok(_) => TodoFinish::Finished,
                Err(err) => TodoFinish::Failed(err.to_string()),
            }
        } else {
            TodoFinish::Finished
        }
    }

    pub(crate) fn finish_all(&mut self, reason: impl Into<String>) -> Vec<String> {
        if self.store.is_none() {
            return Vec::new();
        }
        let reason = reason.into();
        let mut ids: std::collections::HashSet<String> =
            self.turns.drain().map(|(_, id)| id).collect();
        if let Some((_plan_id, todo_id)) = &self.graph {
            ids.insert(todo_id.clone());
        }
        self.graph = None;
        let mut errors = Vec::new();
        for todo_id in ids {
            if let Some(store) = &self.store
                && let Err(err) = store.finish_run(&todo_id, false, Some(reason.clone()))
            {
                errors.push(err.to_string());
            }
        }
        errors
    }

    pub(crate) fn set_submitting_run(&mut self, value: bool) {
        self.submitting_run = value;
    }

    pub(crate) fn take_submitting_run(&mut self) -> bool {
        let value = self.submitting_run;
        self.submitting_run = false;
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use dmuxai::todo::{NewTodo, TodoSource, TodoStatus, TodoStore};
    use std::sync::Arc;
    use tempfile::tempdir;

    fn make_store() -> Arc<TodoStore> {
        let dir = tempdir().unwrap();
        Arc::new(TodoStore::open(&dir.path().join("todos.db")).unwrap())
    }

    fn create_todo(store: &Arc<TodoStore>) -> String {
        store
            .create(NewTodo {
                title: "t".to_owned(),
                body: String::new(),
                source: TodoSource::Manual,
                source_ref: None,
            })
            .unwrap()
            .id
    }

    // TC-2020
    #[test]
    fn tc_2020_todo_links_turn_finish() {
        let store = make_store();
        let mut links = TodoLinks::default();
        links.attach_store(store.clone());
        let todo_id = create_todo(&store);
        let turn_id = StoredTurnId::new("turn-1");
        links.link_turn(Agent::Codex, turn_id.clone(), todo_id.clone());
        store.mark_running(&todo_id, "agent", "codex").unwrap();

        let finish = links.finish_turn(Agent::Codex, &turn_id, true, None);
        assert!(matches!(finish, TodoFinish::Finished));
        assert_eq!(store.get(&todo_id).unwrap().status, TodoStatus::Done);

        let finish2 = links.finish_turn(Agent::Codex, &turn_id, true, None);
        assert!(matches!(finish2, TodoFinish::NotLinked));
    }

    // TC-2021
    #[test]
    fn tc_2021_pending_graph_commit_and_new_run_link() {
        let store = make_store();
        let mut links = TodoLinks::default();
        links.attach_store(store.clone());
        let todo_id = create_todo(&store);
        links.begin_pending_graph(todo_id.clone(), "run-ref".to_owned());

        assert_eq!(links.todo_for_new_run("plan-1"), Some(todo_id.clone()));
        assert_eq!(links.pending_graph(), Some(todo_id.as_str()));
        assert_eq!(links.pending_run_id(), Some("run-ref"));

        let mut run = dmuxai::graph::GraphRun {
            plan: dmuxai::graph::GraphPlan {
                schema: "s".to_owned(),
                id: "plan-1".to_owned(),
                orchestrator_model: "m".to_owned(),
                user_prompt: "p".to_owned(),
                nodes: Vec::new(),
                edges: Vec::new(),
                limits: dmuxai::graph::GraphLimits::default(),
                tasks: Vec::new(),
            },
            conversation_id: "c".to_owned(),
            phase: dmuxai::graph::GraphRunPhase::Active,
            waiting_gate_node: None,
            triage: None,
            approval: None,
            turn_index: 0,
            plan_history: Vec::new(),
            planner_calls: dmuxai::graph::PlannerCalls::default(),
            orchestrate_started_ms: None,
            plan_fixed_ms: None,
            replans_used: 0,
            replan_chars_used: 0,
            pending_replan_plan: None,
            planner_io: Vec::new(),
            todo_id: None,
        };
        links.commit_pending_graph("plan-1", &mut run);
        assert_eq!(run.todo_id, Some(todo_id.clone()));
        assert!(links.pending_graph().is_none());
        assert!(links.pending_run_id().is_none());
        assert_eq!(links.graph, Some(("plan-1".to_owned(), todo_id.clone())));
        assert_eq!(links.todo_for_new_run("plan-1"), Some(todo_id));
    }

    // TC-2022
    #[test]
    fn tc_2022_finish_linked_resolution_order() {
        let store = make_store();
        let mut links = TodoLinks::default();
        links.attach_store(store.clone());
        let graph_todo = create_todo(&store);
        let pending_todo = create_todo(&store);
        let last_todo = create_todo(&store);
        links.graph = Some(("plan-x".to_owned(), graph_todo.clone()));
        links.pending_graph = Some(pending_todo.clone());

        // graph takes priority
        let finish = links.finish_linked(true, None, Some(last_todo.clone()));
        assert!(matches!(finish, TodoFinish::Finished));
        assert_eq!(store.get(&graph_todo).unwrap().status, TodoStatus::Done);
        assert!(links.graph.is_none());
        assert!(links.pending_graph.is_none());
        assert!(links.pending_run_id.is_none());
        // pending untouched? It was cleared by finish_linked (all three). Yes.
        // last not finished
        assert_eq!(store.get(&pending_todo).unwrap().status, TodoStatus::Open);
        assert_eq!(store.get(&last_todo).unwrap().status, TodoStatus::Open);

        // pending priority when no graph
        let mut links2 = TodoLinks::default();
        links2.attach_store(store.clone());
        links2.pending_graph = Some(pending_todo.clone());
        let finish2 = links2.finish_linked(false, Some("err".to_owned()), Some(last_todo.clone()));
        assert!(matches!(finish2, TodoFinish::Finished));
        assert_eq!(store.get(&pending_todo).unwrap().status, TodoStatus::Open);
        assert_eq!(
            store.get(&pending_todo).unwrap().last_error.as_deref(),
            Some("err")
        );

        // last run priority
        let mut links3 = TodoLinks::default();
        links3.attach_store(store.clone());
        let finish3 = links3.finish_linked(true, None, Some(last_todo.clone()));
        assert!(matches!(finish3, TodoFinish::Finished));
        assert_eq!(store.get(&last_todo).unwrap().status, TodoStatus::Done);
    }

    // TC-2023
    #[test]
    fn tc_2023_finish_all_on_shutdown() {
        let store = make_store();
        let mut links = TodoLinks::default();
        links.attach_store(store.clone());
        let t1 = create_todo(&store);
        let t2 = create_todo(&store);
        let turn_id = StoredTurnId::new("turn-1");
        links.link_turn(Agent::Codex, turn_id.clone(), t1.clone());
        links.graph = Some(("plan-1".to_owned(), t2.clone()));
        store.mark_running(&t1, "agent", "codex").unwrap();
        store.mark_running(&t2, "graph", "plan-1").unwrap();

        let errors = links.finish_all("dmuxai exited".to_owned());
        assert!(errors.is_empty());
        assert_eq!(store.get(&t1).unwrap().status, TodoStatus::Open);
        assert_eq!(store.get(&t2).unwrap().status, TodoStatus::Open);
        assert_eq!(
            store.get(&t1).unwrap().last_error.as_deref(),
            Some("dmuxai exited")
        );
        assert!(links.turns.is_empty());
        assert!(links.graph.is_none());
    }
}
