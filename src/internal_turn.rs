use crate::agent::{Agent, AgentOutput};

pub(crate) const MAX_INTERNAL_TURN_BUFFER_CHARS: usize = 80_000;

/// Buffer for internal agent query turns, tracking accumulated text and char count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TurnBuffer {
    text: String,
    chars: usize,
}

impl TurnBuffer {
    pub(crate) fn new() -> Self {
        Self {
            text: String::new(),
            chars: 0,
        }
    }

    pub(crate) fn would_overflow(&self, delta_chars: usize) -> bool {
        self.chars.saturating_add(delta_chars) > MAX_INTERNAL_TURN_BUFFER_CHARS
    }

    pub(crate) fn push(&mut self, delta: &str) {
        self.chars = self.chars.saturating_add(delta.chars().count());
        self.text.push_str(delta);
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.text
    }

    pub(crate) fn chars(&self) -> usize {
        self.chars
    }

    #[allow(dead_code)]
    pub(crate) fn set_chars_for_test(&mut self, count: usize) {
        self.chars = count;
    }
}

/// Identifiers for correlating an internal query turn with streaming agent output.
pub(crate) trait InternalTurn {
    fn agent(&self) -> Agent;
    fn conversation_id(&self) -> &str;
    fn turn_id(&self) -> &str;
}

/// Returns true if the output belongs to the active internal turn.
pub(crate) fn output_belongs<T: InternalTurn>(pending: Option<&T>, output: &AgentOutput) -> bool {
    pending.is_some_and(|pending| {
        pending.agent() == output.agent
            && output
                .conversation_id
                .as_ref()
                .is_some_and(|id| id.as_str() == pending.conversation_id())
            && output
                .turn_id
                .as_ref()
                .is_some_and(|id| id.as_str() == pending.turn_id())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, AgentEvent, AgentOutput, ConversationId, TurnId};

    // TC-2000
    #[test]
    fn tc_2000_turn_buffer_cap() {
        let mut buffer = TurnBuffer::new();
        assert_eq!(buffer.chars(), 0);
        assert_eq!(buffer.as_str(), "");

        // would_overflow at exactly MAX is false, MAX + 1 is true
        assert!(!buffer.would_overflow(MAX_INTERNAL_TURN_BUFFER_CHARS));
        assert!(buffer.would_overflow(MAX_INTERNAL_TURN_BUFFER_CHARS + 1));

        // Setting char count to MAX
        buffer.set_chars_for_test(MAX_INTERNAL_TURN_BUFFER_CHARS);
        assert_eq!(buffer.chars(), MAX_INTERNAL_TURN_BUFFER_CHARS);
        assert!(!buffer.would_overflow(0));
        assert!(buffer.would_overflow(1));

        // Multibyte characters counted as Unicode chars, not bytes
        let mut mb_buffer = TurnBuffer::new();
        let sample = "한글 test 🚀"; // 9 Unicode scalar values, 16 UTF-8 bytes
        mb_buffer.push(sample);
        assert_eq!(mb_buffer.chars(), sample.chars().count());
        assert_eq!(mb_buffer.chars(), 9);
        assert_eq!(mb_buffer.as_str(), sample);

        // Saturating addition check on overflow
        assert!(mb_buffer.would_overflow(usize::MAX));
    }

    struct DummyTurn {
        agent: Agent,
        conversation_id: String,
        turn_id: String,
    }

    impl InternalTurn for DummyTurn {
        fn agent(&self) -> Agent {
            self.agent
        }

        fn conversation_id(&self) -> &str {
            &self.conversation_id
        }

        fn turn_id(&self) -> &str {
            &self.turn_id
        }
    }

    // TC-2001
    #[test]
    fn tc_2001_output_belongs_matches_agent_conversation_turn() {
        let turn = DummyTurn {
            agent: Agent::Codex,
            conversation_id: "conv-42".to_owned(),
            turn_id: "turn-99".to_owned(),
        };

        let matching_output = AgentOutput {
            agent: Agent::Codex,
            conversation_id: Some(ConversationId::new("conv-42")),
            turn_id: Some(TurnId::new("turn-99")),
            event: AgentEvent::Started,
        };

        // None pending returns false
        assert!(!output_belongs::<DummyTurn>(None, &matching_output));

        // All three match -> true
        assert!(output_belongs(Some(&turn), &matching_output));

        // Agent mismatch -> false
        let wrong_agent = AgentOutput {
            agent: Agent::Cursor,
            ..matching_output
        };
        assert!(!output_belongs(Some(&turn), &wrong_agent));

        // Missing conversation_id -> false
        let missing_conv = AgentOutput {
            agent: Agent::Codex,
            conversation_id: None,
            turn_id: Some(TurnId::new("turn-99")),
            event: AgentEvent::Started,
        };
        assert!(!output_belongs(Some(&turn), &missing_conv));

        // Mismatched conversation_id -> false
        let wrong_conv = AgentOutput {
            agent: Agent::Codex,
            conversation_id: Some(ConversationId::new("conv-diff")),
            turn_id: Some(TurnId::new("turn-99")),
            event: AgentEvent::Started,
        };
        assert!(!output_belongs(Some(&turn), &wrong_conv));

        // Missing turn_id -> false
        let missing_turn = AgentOutput {
            agent: Agent::Codex,
            conversation_id: Some(ConversationId::new("conv-42")),
            turn_id: None,
            event: AgentEvent::Started,
        };
        assert!(!output_belongs(Some(&turn), &missing_turn));

        // Mismatched turn_id -> false
        let wrong_turn = AgentOutput {
            agent: Agent::Codex,
            conversation_id: Some(ConversationId::new("conv-42")),
            turn_id: Some(TurnId::new("turn-diff")),
            event: AgentEvent::Started,
        };
        assert!(!output_belongs(Some(&turn), &wrong_turn));
    }
}
