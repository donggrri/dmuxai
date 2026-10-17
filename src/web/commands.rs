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
/// `NotFound` and `Missing` -> 404, `Invalid` -> 400, `Internal` -> 500.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunTodoError {
    Busy(String),
    Forbidden(String),
    UnknownAgent(String),
    AlreadyRunning,
    NotFound,
    /// A well-formed request with an unusable value (SPEC-dashboard-parity FR-781).
    Invalid(String),
    /// A referenced item other than a to-do does not exist (for example a
    /// conversation id or model id).
    Missing(String),
    Internal(String),
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PromptAccepted {
    pub conversation_id: String,
    pub agent: String,
}

/// A picker or lifecycle action from `POST /api/command`
/// (SPEC-dashboard-parity FR-781). `agent` defaults to the selected agent.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionCommand {
    SelectAgent {
        agent: String,
    },
    SetMode {
        #[serde(default)]
        agent: Option<String>,
        mode: String,
    },
    LoadModels {
        #[serde(default)]
        agent: Option<String>,
    },
    SelectModel {
        #[serde(default)]
        agent: Option<String>,
        model: String,
    },
    SetEffort {
        #[serde(default)]
        agent: Option<String>,
        effort: String,
    },
    Cancel {
        #[serde(default)]
        agent: Option<String>,
    },
    NewConversation {
        #[serde(default)]
        title: Option<String>,
    },
    LoadConversation {
        id: String,
    },
    Share {
        target: String,
    },
    Status,
    ToggleDetails,
    Quit,
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
    Session {
        command: SessionCommand,
        reply: oneshot::Sender<Result<crate::web::SessionView, RunTodoError>>,
    },
}
