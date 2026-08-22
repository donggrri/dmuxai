use std::fmt;

/// An AI provider that can own conversations and produce messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Events emitted by an adapter and consumed by the application.
///
/// Real providers will eventually send these through a bounded async channel.
#[derive(Debug, PartialEq, Eq)]
pub enum AgentEvent {
    TextDelta(String),
    Completed,
    Error(String),
}

/// The small provider boundary needed by the local prototype.
///
/// `send` is deliberately synchronous for now. Phase 3 can replace the return
/// value with a bounded Tokio channel without coupling the UI to a provider.
pub trait AgentAdapter {
    fn agent(&self) -> Agent;
    fn send(&mut self, prompt: &str) -> Vec<AgentEvent>;
}

/// A deterministic local adapter used until a real structured protocol is added.
pub struct FakeAgent {
    agent: Agent,
}

impl FakeAgent {
    pub const fn new(agent: Agent) -> Self {
        Self { agent }
    }
}

impl AgentAdapter for FakeAgent {
    fn agent(&self) -> Agent {
        self.agent
    }

    fn send(&mut self, prompt: &str) -> Vec<AgentEvent> {
        if prompt.trim() == "/error" {
            return vec![AgentEvent::Error(
                "simulated local adapter failure".to_owned(),
            )];
        }

        vec![
            AgentEvent::TextDelta(format!(
                "Local prototype reply from {}: I received \"{prompt}\".",
                self.agent
            )),
            AgentEvent::Completed,
        ]
    }
}
