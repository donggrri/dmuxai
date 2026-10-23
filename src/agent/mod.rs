pub(crate) mod acp;
mod acp_runner;
mod agy;
mod codex;
mod cursor;
mod custom;
mod implement_outcome;

use std::{error::Error, fmt, path::PathBuf};

use tokio::{sync::mpsc, task::JoinHandle};

pub use agy::spawn_agy_adapter;
pub use codex::spawn_codex_adapter;
pub use cursor::{CursorAdapterConfig, spawn_cursor_adapter};
pub use custom::spawn_custom_adapter;

#[cfg(test)]
#[cfg(unix)]
pub(crate) use cursor::tests::FakeCursorFixture;

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

/// Per-turn policy for adapter runs. User-facing Ask/Plan/Implement modes are
/// removed; user turns use [`Agent`], internal judge/plan/replan turns use
/// [`ReadOnly`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum TurnPolicy {
    #[default]
    Agent,
    ReadOnly,
}

impl TurnPolicy {
    #[allow(dead_code)]
    pub const fn is_read_only(self) -> bool {
        matches!(self, Self::ReadOnly)
    }
}

impl fmt::Display for TurnPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Agent => formatter.write_str("agent"),
            Self::ReadOnly => formatter.write_str("read-only"),
        }
    }
}

/// Legacy dashboard/MCP mode strings (`ask`, `plan`, `implement`). Omitted mode
/// means the default write-capable agent policy.
/// Maps legacy mode strings from the dashboard/MCP/CLI to a [`TurnPolicy`].
pub fn turn_policy_from_legacy_mode(mode: &str) -> Result<TurnPolicy, String> {
    match mode.trim().to_ascii_lowercase().as_str() {
        "ask" | "plan" => Ok(TurnPolicy::ReadOnly),
        "implement" | "agent" => Ok(TurnPolicy::Agent),
        other => Err(format!(
            "invalid mode '{other}'; use ask or plan (read-only), implement (agent), or omit mode"
        )),
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
    ToolProgress {
        item_id: String,
        summary: String,
        state: String,
    },
    Activity(dmuxai::activity::ActivityItem),
    Status(String),
    Completed,
    Cancelled,
    Error(AgentError),
}

/// The high-level category of an agent or runner error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentErrorKind {
    Provider,
    ModelUnavailable,
    ProcessExit,
    Timeout,
    Idle,
    Cancelled,
    Policy,
}

impl AgentErrorKind {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::ModelUnavailable => "model_unavailable",
            Self::ProcessExit => "process_exit",
            Self::Timeout => "timeout",
            Self::Idle => "idle",
            Self::Cancelled => "cancelled",
            Self::Policy => "policy",
        }
    }
}

impl fmt::Display for AgentErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A structured error emitted by an agent runner.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentError {
    pub kind: AgentErrorKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl AgentError {
    pub fn new(kind: AgentErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            hint: None,
        }
    }

    pub fn provider(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::Provider, message)
    }

    pub fn model_unavailable(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::ModelUnavailable, message)
    }

    pub fn process_exit(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::ProcessExit, message)
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::Timeout, message)
    }

    pub fn idle(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::Idle, message)
    }

    pub fn cancelled(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::Cancelled, message)
    }

    pub fn policy(message: impl Into<String>) -> Self {
        Self::new(AgentErrorKind::Policy, message)
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    #[allow(dead_code)]
    pub fn kind(&self) -> AgentErrorKind {
        self.kind
    }

    #[allow(dead_code)]
    pub fn message(&self) -> &str {
        &self.message
    }

    #[allow(dead_code)]
    pub fn hint(&self) -> Option<&str> {
        self.hint.as_deref()
    }
}

impl fmt::Display for AgentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.hint {
            Some(hint) => {
                let trimmed = self.message.trim_end();
                if trimmed.ends_with(';') || trimmed.ends_with(':') {
                    write!(formatter, "{trimmed} {hint}")
                } else {
                    write!(formatter, "{}; {hint}", self.message)
                }
            }
            None => formatter.write_str(&self.message),
        }
    }
}

impl Error for AgentError {}

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
    pub policy: TurnPolicy,
    /// When true (graph implement steps), AGY may retry once if the model replies
    /// with plan-only text and no writes. Ordinary user questions leave this false.
    pub require_file_edits: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct OutputContext {
    pub conversation_id: ConversationId,
    pub turn_id: TurnId,
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
}
impl fmt::Display for AdapterError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommandQueueFull => formatter.write_str("agent command queue is full"),
            Self::RuntimeStopped => formatter.write_str("agent runtime has stopped"),
            Self::Unsupported => formatter.write_str("agent does not support this command"),
        }
    }
}

impl Error for AdapterError {}

const ADAPTER_BUSY_TURN_SUFFIX: &str =
    " is already running a turn; cancel it before sending another prompt";

/// Typed adapter refusal when a second prompt is sent before the prior turn ends.
pub fn is_adapter_turn_busy_rejection(message: &str) -> bool {
    match message.strip_suffix(ADAPTER_BUSY_TURN_SUFFIX) {
        Some(prefix) => !prefix.is_empty() && !prefix.contains('\n'),
        None => false,
    }
}

/// The synchronous UI-facing handle for an asynchronous agent runtime.
pub trait AgentAdapter: Send {
    fn agent(&self) -> Agent;
    fn send(&self, request: AgentRequest) -> Result<(), AdapterError>;
    fn cancel(&self) -> Result<(), AdapterError>;
    fn request_models(&self) -> Result<(), AdapterError>;
    fn select_model(&self, model: String) -> Result<(), AdapterError>;
    fn select_effort(&self, _effort: String) -> Result<(), AdapterError> {
        Err(AdapterError::Unsupported)
    }
    fn send_graph_node(&self, request: AgentRequest) -> Result<(), AdapterError> {
        self.send(request)
    }
    fn prewarm(&self) -> Result<(), AdapterError> {
        Ok(())
    }
    fn reset(&self) -> Result<(), AdapterError> {
        Ok(())
    }
    fn tick(&self) -> Result<(), AdapterError> {
        Ok(())
    }
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

#[cfg(test)]
mod legacy_mode_tests {
    use super::{TurnPolicy, turn_policy_from_legacy_mode};

    #[test]
    fn legacy_ask_and_plan_are_read_only() {
        assert_eq!(
            turn_policy_from_legacy_mode("ask").unwrap(),
            TurnPolicy::ReadOnly
        );
        assert_eq!(
            turn_policy_from_legacy_mode("plan").unwrap(),
            TurnPolicy::ReadOnly
        );
    }

    #[test]
    fn legacy_implement_is_agent() {
        assert_eq!(
            turn_policy_from_legacy_mode("implement").unwrap(),
            TurnPolicy::Agent
        );
    }
}

#[cfg(test)]
mod typed_error_tests {
    use super::{AgentError, AgentErrorKind};

    #[test]
    fn agent_error_kind_strings_and_display() {
        let cases = [
            (AgentErrorKind::Provider, "provider"),
            (AgentErrorKind::ModelUnavailable, "model_unavailable"),
            (AgentErrorKind::ProcessExit, "process_exit"),
            (AgentErrorKind::Timeout, "timeout"),
            (AgentErrorKind::Idle, "idle"),
            (AgentErrorKind::Cancelled, "cancelled"),
            (AgentErrorKind::Policy, "policy"),
        ];

        for (kind, expected) in cases {
            assert_eq!(kind.as_str(), expected);
            assert_eq!(kind.to_string(), expected);
            let serialized = serde_json::to_string(&kind).unwrap();
            assert_eq!(serialized, format!("\"{expected}\""));
        }
    }

    #[test]
    fn agent_error_constructors_and_display() {
        let err = AgentError::provider("connection failed");
        assert_eq!(err.kind(), AgentErrorKind::Provider);
        assert_eq!(err.message(), "connection failed");
        assert_eq!(err.hint(), None);
        assert_eq!(err.to_string(), "connection failed");

        let err = AgentError::model_unavailable("model grok-99 not found");
        assert_eq!(err.kind(), AgentErrorKind::ModelUnavailable);
        assert_eq!(err.to_string(), "model grok-99 not found");

        let err = AgentError::process_exit("runner exited with 127");
        assert_eq!(err.kind(), AgentErrorKind::ProcessExit);
        assert_eq!(err.to_string(), "runner exited with 127");

        let err = AgentError::timeout("runner timed out after 30s")
            .with_hint("raise timeout_secs in dmuxai.toml");
        assert_eq!(err.kind(), AgentErrorKind::Timeout);
        assert_eq!(err.hint(), Some("raise timeout_secs in dmuxai.toml"));
        assert_eq!(
            err.to_string(),
            "runner timed out after 30s; raise timeout_secs in dmuxai.toml"
        );

        let err = AgentError::idle("silent child killed");
        assert_eq!(err.kind(), AgentErrorKind::Idle);
        assert_eq!(err.to_string(), "silent child killed");

        let err = AgentError::cancelled("turn was cancelled");
        assert_eq!(err.kind(), AgentErrorKind::Cancelled);
        assert_eq!(err.to_string(), "turn was cancelled");

        let err = AgentError::policy("permission denied");
        assert_eq!(err.kind(), AgentErrorKind::Policy);
        assert_eq!(err.to_string(), "permission denied");
    }
}
