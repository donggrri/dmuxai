mod agy;
mod codex;
mod cursor;

use std::{error::Error, fmt};

use tokio::{sync::mpsc, task::JoinHandle};

pub use agy::spawn_agy_adapter;
pub use codex::spawn_codex_adapter;
pub use cursor::spawn_cursor_adapter;

/// An AI provider that can own conversations and produce messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    Codex,
    Cursor,
    Agy,
}

impl Agent {
    pub const ALL: [Self; 3] = [Self::Codex, Self::Cursor, Self::Agy];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::Agy => "AGY",
        }
    }
}

impl fmt::Display for Agent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Provider-independent events emitted by a runtime and consumed by the UI.
#[derive(Debug, PartialEq, Eq)]
pub enum AgentEvent {
    Started,
    TextDelta(String),
    Notice(String),
    ModelsListed {
        models: Vec<ModelInfo>,
        selected: Option<String>,
    },
    ModelSelected(String),
    Completed,
    Cancelled,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
}

impl ModelInfo {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct AgentOutput {
    pub agent: Agent,
    pub event: AgentEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterError {
    CommandQueueFull,
    RuntimeStopped,
}

impl fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommandQueueFull => formatter.write_str("agent command queue is full"),
            Self::RuntimeStopped => formatter.write_str("agent runtime has stopped"),
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
    fn send(&self, prompt: String) -> Result<(), AdapterError>;
    fn cancel(&self) -> Result<(), AdapterError>;
    fn request_models(&self) -> Result<(), AdapterError>;
    fn select_model(&self, model: String) -> Result<(), AdapterError>;
}

pub type SpawnedAdapter = (Box<dyn AgentAdapter>, JoinHandle<()>);

pub const EVENT_CHANNEL_CAPACITY: usize = 64;

pub fn event_channel() -> (mpsc::Sender<AgentOutput>, mpsc::Receiver<AgentOutput>) {
    mpsc::channel(EVENT_CHANNEL_CAPACITY)
}
