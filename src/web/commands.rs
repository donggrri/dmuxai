//! Commands sent from the dashboard HTTP server to the TUI App
//! (SPEC-web-dashboard FR-542). Only `dmuxai --web` has a receiver; in
//! `dmuxai serve` the router has no sender and answers 409.

use tokio::sync::oneshot;

use crate::todo::Todo;

/// How a to-do should be run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunMode {
    /// Submit to one agent by runner id (`agy`, `codex`, `cursor`, or a custom agent name).
    Agent { agent: String },
    /// Submit through the graph planner.
    Graph,
}

/// Why the App refused a run request. HTTP mapping: `Busy` and
/// `AlreadyRunning` -> 409, `Forbidden` -> 403, `UnknownAgent` and
/// `NotFound` -> 404, `Internal` -> 500.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunTodoError {
    Busy(String),
    Forbidden(String),
    UnknownAgent(String),
    AlreadyRunning,
    NotFound,
    Internal(String),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PromptAccepted {
    pub conversation_id: String,
    pub agent: String,
}

#[derive(Debug)]
pub enum DashboardCommand {
    RunTodo {
        todo_id: String,
        mode: RunMode,
        reply: oneshot::Sender<Result<Todo, RunTodoError>>,
    },
    SubmitPrompt {
        text: String,
        agent: Option<String>,
        mode: Option<String>,
        reply: oneshot::Sender<Result<PromptAccepted, RunTodoError>>,
    },
}
