#[path = "../src/conversation.rs"]
mod conversation;

use conversation::{
    AgentKey, BeginTurn, BeginTurnOutcome, BindingStatus, ConversationEngine,
    ConversationSelection, MessageRole, MessageState, SummaryDocument, TurnEvent, TurnOutput,
    TurnStatus,
};
use tempfile::TempDir;

struct Harness {
    _temp: TempDir,
    workspace: std::path::PathBuf,
    database: std::path::PathBuf,
}

impl Harness {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let database = temp.path().join("dmuxai.sqlite3");
        Self {
            _temp: temp,
            workspace,
            database,
        }
    }

    fn open(&self) -> ConversationEngine {
        ConversationEngine::open(&self.database, &self.workspace).unwrap()
    }
}

fn ready(outcome: BeginTurnOutcome) -> conversation::AgentRequest {
    match outcome {
        BeginTurnOutcome::Ready(request) => request,
        BeginTurnOutcome::NeedsSummary(_) => panic!("test conversation unexpectedly needs summary"),
    }
}

fn complete_turn(
    engine: &mut ConversationEngine,
    request: &conversation::AgentRequest,
    provider_session_id: &str,
    answer: &str,
) {
    let envelope = |event| TurnOutput {
        conversation_id: request.conversation_id.clone(),
        turn_id: request.turn_id.clone(),
        agent: request.agent,
        event,
    };
    engine
        .record_turn_outputs(&[
            envelope(TurnEvent::Started),
            envelope(TurnEvent::ProviderSessionBound(
                provider_session_id.to_owned(),
            )),
            envelope(TurnEvent::TextDelta(answer.to_owned())),
            envelope(TurnEvent::Notice("provider notice".to_owned())),
            envelope(TurnEvent::Completed),
        ])
        .unwrap();
}

#[test]
fn completed_conversation_and_binding_survive_restart() {
    let harness = Harness::new();
    let conversation_id;
    {
        let mut engine = harness.open();
        let snapshot = engine
            .select(ConversationSelection::New { title: None })
            .unwrap();
        conversation_id = snapshot.id;
        let request = ready(
            engine
                .begin_turn(BeginTurn {
                    agent: AgentKey::Codex,
                    model_id: Some("codex-model".to_owned()),
                    prompt: "Explain ownership".to_owned(),
                })
                .unwrap(),
        );
        assert!(request.resume_session.is_none());
        assert!(request.context.recent.is_empty());
        complete_turn(&mut engine, &request, "thread-1", "Ownership is explicit.");
    }

    let mut reopened = harness.open();
    let cards = reopened.list().unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].id, conversation_id);
    assert_eq!(cards[0].turn_count, 1);
    assert_eq!(cards[0].last_agent, Some(AgentKey::Codex));

    let snapshot = reopened
        .select(ConversationSelection::Existing(conversation_id))
        .unwrap();
    assert_eq!(snapshot.turns[0].status, TurnStatus::Completed);
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(snapshot.messages[0].role, MessageRole::User);
    assert_eq!(snapshot.messages[1].role, MessageRole::Assistant);
    assert_eq!(snapshot.messages[1].text, "Ownership is explicit.");
    assert_eq!(snapshot.bindings[0].status, BindingStatus::Active);
    assert_eq!(snapshot.bindings[0].provider_session_id, "thread-1");

    let next = ready(
        reopened
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "Continue".to_owned(),
            })
            .unwrap(),
    );
    assert_eq!(next.resume_session.as_deref(), Some("thread-1"));
    assert_eq!(next.context.recent.len(), 1);
}

#[test]
fn projections_and_bindings_are_isolated_by_agent() {
    let harness = Harness::new();
    let mut engine = harness.open();
    engine
        .select(ConversationSelection::New { title: None })
        .unwrap();
    let codex = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "Codex-only secret".to_owned(),
            })
            .unwrap(),
    );
    complete_turn(&mut engine, &codex, "codex-thread", "Codex-only answer");

    let cursor = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Cursor,
                model_id: None,
                prompt: "What do you know?".to_owned(),
            })
            .unwrap(),
    );
    assert!(cursor.resume_session.is_none());
    assert!(cursor.context.recent.is_empty());
    complete_turn(&mut engine, &cursor, "cursor-session", "Cursor answer");

    let codex_again = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "Continue Codex".to_owned(),
            })
            .unwrap(),
    );
    assert_eq!(codex_again.resume_session.as_deref(), Some("codex-thread"));
    assert_eq!(codex_again.context.recent.len(), 1);
    assert_eq!(codex_again.context.recent[0].user, "Codex-only secret");
}

#[test]
fn unfinished_turn_is_interrupted_and_its_binding_is_not_resumed() {
    let harness = Harness::new();
    let conversation_id;
    {
        let mut engine = harness.open();
        conversation_id = engine
            .select(ConversationSelection::New { title: None })
            .unwrap()
            .id;
        let request = ready(
            engine
                .begin_turn(BeginTurn {
                    agent: AgentKey::Agy,
                    model_id: None,
                    prompt: "Draft a plan".to_owned(),
                })
                .unwrap(),
        );
        let envelope = |event| TurnOutput {
            conversation_id: request.conversation_id.clone(),
            turn_id: request.turn_id.clone(),
            agent: request.agent,
            event,
        };
        engine
            .record_turn_outputs(&[
                envelope(TurnEvent::Started),
                envelope(TurnEvent::ProviderSessionBound(
                    "agy-conversation".to_owned(),
                )),
                envelope(TurnEvent::TextDelta("partial".to_owned())),
            ])
            .unwrap();
    }

    let mut reopened = harness.open();
    let snapshot = reopened
        .select(ConversationSelection::Existing(conversation_id))
        .unwrap();
    assert_eq!(snapshot.turns[0].status, TurnStatus::Interrupted);
    assert_eq!(snapshot.messages[1].state, MessageState::Interrupted);
    assert_eq!(snapshot.messages[1].text, "partial");
    assert_eq!(snapshot.bindings[0].status, BindingStatus::Stale);

    let next = ready(
        reopened
            .begin_turn(BeginTurn {
                agent: AgentKey::Agy,
                model_id: None,
                prompt: "Try safely again".to_owned(),
            })
            .unwrap(),
    );
    assert!(next.resume_session.is_none());
    assert!(next.context.recent.is_empty());
}

#[test]
fn summary_checkpoint_is_used_with_a_gap_free_recent_tail() {
    let harness = Harness::new();
    let mut engine = harness.open();
    engine
        .select(ConversationSelection::New { title: None })
        .unwrap();

    let large_answer = "x".repeat(2_000);
    let mut committed_summary = false;
    for index in 0..24 {
        let input = BeginTurn {
            agent: AgentKey::Codex,
            model_id: None,
            prompt: format!("prompt-{index}"),
        };
        let request = loop {
            match engine.begin_turn(input.clone()).unwrap() {
                BeginTurnOutcome::Ready(request) => break request,
                BeginTurnOutcome::NeedsSummary(job) => {
                    assert_eq!(job.agent(), AgentKey::Codex);
                    assert!(job.covers_through_seq() > 0);
                    let _rolling_summary = job.previous_summary();
                    assert!(!job.source_turns().is_empty());
                    engine
                        .commit_summary(
                            job,
                            SummaryDocument {
                                goal: Some("Preserve the long-running goal".to_owned()),
                                decisions: vec!["Keep completed turns canonical".to_owned()],
                                ..SummaryDocument::default()
                            },
                        )
                        .unwrap();
                    committed_summary = true;
                }
            }
        };
        complete_turn(&mut engine, &request, "codex-thread", &large_answer);
    }
    assert!(committed_summary);

    let next = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "after-summary".to_owned(),
            })
            .unwrap(),
    );
    assert!(
        next.context
            .summary
            .as_deref()
            .unwrap()
            .contains("Preserve the long-running goal")
    );
    assert!(!next.context.recent.is_empty());
    assert!(
        next.context
            .recent
            .windows(2)
            .all(|window| window[0].seq < window[1].seq)
    );
    assert_eq!(next.context.through_seq, 24);
}

#[test]
fn a_mismatched_output_rolls_back_the_entire_batch() {
    let harness = Harness::new();
    let conversation_id;
    let request;
    {
        let mut engine = harness.open();
        conversation_id = engine
            .select(ConversationSelection::New { title: None })
            .unwrap()
            .id;
        request = ready(
            engine
                .begin_turn(BeginTurn {
                    agent: AgentKey::Cursor,
                    model_id: None,
                    prompt: "Atomic batch".to_owned(),
                })
                .unwrap(),
        );
        let result = engine.record_turn_outputs(&[
            TurnOutput {
                conversation_id: request.conversation_id.clone(),
                turn_id: request.turn_id.clone(),
                agent: AgentKey::Cursor,
                event: TurnEvent::TextDelta("must roll back".to_owned()),
            },
            TurnOutput {
                conversation_id: request.conversation_id.clone(),
                turn_id: request.turn_id.clone(),
                agent: AgentKey::Codex,
                event: TurnEvent::Failed {
                    message: "wrong agent".to_owned(),
                    invalidates_binding: true,
                },
            },
        ]);
        assert!(result.is_err());
    }

    let mut reopened = harness.open();
    let snapshot = reopened
        .select(ConversationSelection::Existing(conversation_id))
        .unwrap();
    assert_eq!(snapshot.turns[0].status, TurnStatus::Interrupted);
    assert_eq!(snapshot.messages.len(), 1);
    assert_eq!(snapshot.messages[0].role, MessageRole::User);
}

#[test]
fn cancellation_is_terminal_and_invalidates_the_binding() {
    let harness = Harness::new();
    let mut engine = harness.open();
    let conversation_id = engine
        .select(ConversationSelection::New { title: None })
        .unwrap()
        .id;
    let request = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Cursor,
                model_id: None,
                prompt: "Cancel this".to_owned(),
            })
            .unwrap(),
    );
    let envelope = |event| TurnOutput {
        conversation_id: request.conversation_id.clone(),
        turn_id: request.turn_id.clone(),
        agent: request.agent,
        event,
    };
    engine
        .record_turn_outputs(&[
            envelope(TurnEvent::Started),
            envelope(TurnEvent::ProviderSessionBound("cursor-session".to_owned())),
            envelope(TurnEvent::Cancelled),
        ])
        .unwrap();

    let snapshot = engine
        .select(ConversationSelection::Existing(conversation_id))
        .unwrap();
    assert_eq!(snapshot.turns[0].status, TurnStatus::Cancelled);
    assert_eq!(snapshot.bindings[0].status, BindingStatus::Stale);
}
