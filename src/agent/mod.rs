mod agy;
mod codex;
mod cursor;
mod custom;
mod implement_outcome;

use std::{error::Error, fmt, path::PathBuf};

use tokio::{sync::mpsc, task::JoinHandle};

pub use agy::spawn_agy_adapter;
pub use codex::spawn_codex_adapter;
pub use cursor::spawn_cursor_adapter;
pub use custom::spawn_custom_adapter;

/// An AI provider that can own conversations and produce messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    Codex,
    Cursor,
    Agy,
    /// A `[[agents.custom]]` headless agent, by registry index.
    Custom(u8),
}

impl Agent {
    /// Built-in runners in UI and spawn order: primary runners first, optional Codex last.
    pub const BUILTIN: [Self; 3] = [Self::Cursor, Self::Agy, Self::Codex];

    /// Built-in runners followed by every configured custom agent.
    pub fn all() -> Vec<Self> {
        let mut agents = Self::BUILTIN.to_vec();
        agents.extend(
            (0..dmuxai::custom_agent::registry().len())
                .filter_map(|index| u8::try_from(index).ok())
                .map(Self::Custom),
        );
        agents
    }

    #[allow(dead_code)]
    pub const PRIMARY: [Self; 2] = [Self::Cursor, Self::Agy];

    pub const fn default() -> Self {
        Self::Cursor
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::Agy => "AGY",
            Self::Custom(index) => dmuxai::custom_agent::spec_at(index)
                .map(|spec| spec.label())
                .unwrap_or("custom"),
        }
    }
}

impl fmt::Display for Agent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// A provider-independent description of what an agent may do during a turn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ExecutionMode {
    #[default]
    Ask,
    Plan,
    Implement,
}

impl ExecutionMode {
    pub const ALL: [Self; 3] = [Self::Ask, Self::Plan, Self::Implement];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Ask => "Ask",
            Self::Plan => "Plan",
            Self::Implement => "Implement",
        }
    }
}

impl fmt::Display for ExecutionMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Provider-independent events emitted by a runtime and consumed by the UI.
#[derive(Debug, PartialEq, Eq)]
pub enum AgentEvent {
    Started,
    ProviderSessionBound(String),
    TextDelta(String),
    Notice(String),
    ModelsListed {
        models: Vec<ModelInfo>,
        selected: Option<String>,
        selected_effort: Option<String>,
    },
    ModelSelected(String),
    EffortSelected(String),
    ModeSelected(ExecutionMode),
    ToolProgress {
        item_id: String,
        summary: String,
        state: String,
    },
    Status(String),
    Completed,
    Cancelled,
    Error(String),
}

/// A DmuxAI-owned identifier for a conversation spanning provider sessions.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConversationId(String);

impl ConversationId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ConversationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A DmuxAI-owned identifier used to correlate one request and its outputs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TurnId(String);

impl TurnId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TurnId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRequest {
    pub conversation_id: ConversationId,
    pub turn_id: TurnId,
    pub prompt: String,
    pub resume_session: Option<String>,
    pub image_paths: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
struct OutputContext {
    conversation_id: ConversationId,
    turn_id: TurnId,
}

impl From<&AgentRequest> for OutputContext {
    fn from(request: &AgentRequest) -> Self {
        Self {
            conversation_id: request.conversation_id.clone(),
            turn_id: request.turn_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
}

impl ModelInfo {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            efforts: Vec::new(),
            default_effort: None,
        }
    }

    pub fn with_efforts(
        id: impl Into<String>,
        label: impl Into<String>,
        efforts: Vec<String>,
        default_effort: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            efforts,
            default_effort,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct AgentOutput {
    pub agent: Agent,
    pub conversation_id: Option<ConversationId>,
    pub turn_id: Option<TurnId>,
    pub event: AgentEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterError {
    CommandQueueFull,
    RuntimeStopped,
    Unsupported,
    UnsupportedMode { agent: Agent, mode: ExecutionMode },
}

impl fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommandQueueFull => formatter.write_str("agent command queue is full"),
            Self::RuntimeStopped => formatter.write_str("agent runtime has stopped"),
            Self::Unsupported => formatter.write_str("agent does not support this command"),
            Self::UnsupportedMode { agent, mode }
                if *agent == Agent::Cursor && *mode == ExecutionMode::Implement && cfg!(windows) =>
            {
                formatter.write_str(
                    "Cursor Implement mode is unavailable on Windows unless cursor.windows_unsandboxed = true in dmuxai.toml or DMUXAI_CURSOR_WINDOWS_UNSANDBOXED is set (unsandboxed; not recommended)",
                )
            }
            Self::UnsupportedMode { agent, mode } => formatter.write_str(
                &format!("{agent} {mode} mode is unavailable on this platform"),
            ),
        }
    }
}

impl Error for AdapterError {}

/// The synchronous UI-facing handle for an asynchronous agent runtime.
///
/// Commands are deliberately non-blocking: the UI either enqueues one or gets
/// a visible backpressure error. The runtime owns the process and emits events
/// through a separate bounded channel.
pub trait AgentAdapter: Send {
    fn agent(&self) -> Agent;
    fn send(&self, request: AgentRequest) -> Result<(), AdapterError>;
    fn cancel(&self) -> Result<(), AdapterError>;
    fn request_models(&self) -> Result<(), AdapterError>;
    fn select_model(&self, model: String) -> Result<(), AdapterError>;
    fn select_effort(&self, _effort: String) -> Result<(), AdapterError> {
        Err(AdapterError::Unsupported)
    }
    fn validate_mode(&self, _mode: ExecutionMode) -> Result<(), AdapterError> {
        Ok(())
    }
    fn select_mode(&self, mode: ExecutionMode) -> Result<(), AdapterError>;
    fn request_status(&self) -> Result<(), AdapterError> {
        Err(AdapterError::Unsupported)
    }
}

/// Cursor Agent and AGY have no `--image` CLI flag; image attachments are
/// passed as `@<absolute path>` tokens appended to the prompt text.
pub(crate) fn prompt_with_image_tokens(prompt: &str, image_paths: &[PathBuf]) -> String {
    if image_paths.is_empty() {
        return prompt.to_owned();
    }
    let refs = image_paths
        .iter()
        .map(|path| format!("@{}", path.display()))
        .collect::<Vec<_>>()
        .join(" ");
    if prompt.trim().is_empty() {
        refs
    } else {
        format!("{prompt} {refs}")
    }
}

pub type SpawnedAdapter = (Box<dyn AgentAdapter>, JoinHandle<()>);

pub const EVENT_CHANNEL_CAPACITY: usize = 64;

pub fn event_channel() -> (mpsc::Sender<AgentOutput>, mpsc::Receiver<AgentOutput>) {
    mpsc::channel(EVENT_CHANNEL_CAPACITY)
}
