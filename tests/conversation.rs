use dmuxai::conversation::{
    AgentKey, BeginTurn, BeginTurnOutcome, BindingStatus, ConversationEngine, ConversationError,
    ConversationSelection, MessageRole, MessageState, SummaryDocument, TurnEvent, TurnOutput,
    TurnStatus,
};
use rusqlite::Connection;
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

fn ready(outcome: BeginTurnOutcome) -> dmuxai::conversation::AgentRequest {
    match outcome {
        BeginTurnOutcome::Ready(request) => request,
        BeginTurnOutcome::NeedsSummary(_) => panic!("test conversation unexpectedly needs summary"),
    }
}

fn complete_turn(
    engine: &mut ConversationEngine,
    request: &dmuxai::conversation::AgentRequest,
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
                    display_text: None,
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
                display_text: None,
            })
            .unwrap(),
    );
    assert_eq!(next.resume_session.as_deref(), Some("thread-1"));
    assert_eq!(next.context.recent.len(), 1);
}

#[test]
fn projections_are_shared_across_agents_and_bindings_remain_provider_scoped() {
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
                display_text: None,
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
                display_text: None,
            })
            .unwrap(),
    );
    assert!(cursor.resume_session.is_none());
    assert_eq!(cursor.context.recent.len(), 1);
    assert_eq!(cursor.context.recent[0].agent, AgentKey::Codex);
    assert_eq!(cursor.context.recent[0].user, "Codex-only secret");
    complete_turn(&mut engine, &cursor, "cursor-session", "Cursor answer");

    let codex_again = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "Continue Codex".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    assert!(codex_again.resume_session.is_none());
    assert_eq!(codex_again.context.recent.len(), 2);
    assert_eq!(codex_again.context.recent[0].user, "Codex-only secret");
    assert_eq!(codex_again.context.recent[0].agent, AgentKey::Codex);
    assert_eq!(codex_again.context.recent[1].user, "What do you know?");
    assert_eq!(codex_again.context.recent[1].agent, AgentKey::Cursor);
}

#[test]
fn shared_projection_is_scoped_to_the_active_conversation() {
    let harness = Harness::new();
    let mut engine = harness.open();
    let conversation_a = engine
        .select(ConversationSelection::New {
            title: Some("A".to_owned()),
        })
        .unwrap()
        .id;
    let first = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "private A".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    complete_turn(&mut engine, &first, "a-thread", "answer A");

    let conversation_b = engine
        .select(ConversationSelection::New {
            title: Some("B".to_owned()),
        })
        .unwrap()
        .id;
    let in_b = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Cursor,
                model_id: None,
                prompt: "question B".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    assert!(in_b.context.recent.is_empty());
    complete_turn(&mut engine, &in_b, "b-session", "answer B");

    engine
        .select(ConversationSelection::Existing(conversation_a))
        .unwrap();
    let in_a = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Cursor,
                model_id: None,
                prompt: "follow A".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    assert_eq!(in_a.context.recent.len(), 1);
    assert_eq!(in_a.context.recent[0].user, "private A");
    assert!(
        !in_a
            .context
            .recent
            .iter()
            .any(|turn| turn.user == "question B")
    );

    engine
        .select(ConversationSelection::Existing(conversation_b))
        .unwrap();
    let back_to_b = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Agy,
                model_id: None,
                prompt: "follow B".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    assert_eq!(back_to_b.context.recent.len(), 1);
    assert_eq!(back_to_b.context.recent[0].user, "question B");
}

#[test]
fn summary_source_includes_completed_turns_from_all_agents() {
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
                prompt: "codex goal".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    complete_turn(&mut engine, &codex, "codex-thread", &"c".repeat(2_000));
    let cursor = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Cursor,
                model_id: None,
                prompt: "cursor constraint".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    complete_turn(&mut engine, &cursor, "cursor-session", &"u".repeat(2_000));

    let job = engine.prepare_summary(AgentKey::Agy).unwrap().unwrap();
    assert_eq!(job.source_turns().len(), 2);
    assert_eq!(job.source_turns()[0].agent, AgentKey::Codex);
    assert_eq!(job.source_turns()[1].agent, AgentKey::Cursor);
}

#[test]
fn selecting_away_from_a_live_turn_and_back_keeps_it_recordable() {
    let harness = Harness::new();
    let mut engine = harness.open();
    let conversation_b = engine
        .select(ConversationSelection::New {
            title: Some("Conversation B".to_owned()),
        })
        .unwrap()
        .id;
    let conversation_a = engine
        .select(ConversationSelection::New {
            title: Some("Conversation A".to_owned()),
        })
        .unwrap()
        .id;
    let request = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "Keep working while I switch views".to_owned(),
                display_text: None,
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
            envelope(TurnEvent::ProviderSessionBound("thread-a".to_owned())),
            envelope(TurnEvent::TextDelta("partial ".to_owned())),
        ])
        .unwrap();

    engine
        .select(ConversationSelection::Existing(conversation_b))
        .unwrap();
    let live = engine
        .select(ConversationSelection::Existing(conversation_a.clone()))
        .unwrap();
    assert_eq!(live.turns[0].status, TurnStatus::Running);
    assert_eq!(live.messages[1].state, MessageState::Streaming);
    assert_eq!(live.bindings[0].status, BindingStatus::Active);

    engine
        .record_turn_outputs(&[
            envelope(TurnEvent::TextDelta("answer".to_owned())),
            envelope(TurnEvent::Completed),
        ])
        .unwrap();
    let completed = engine
        .select(ConversationSelection::Existing(conversation_a))
        .unwrap();
    assert_eq!(completed.turns[0].status, TurnStatus::Completed);
    assert_eq!(completed.messages[1].state, MessageState::Complete);
    assert_eq!(completed.messages[1].text, "partial answer");
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
                    display_text: None,
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
    assert_eq!(reopened.recover_workspace().unwrap(), 1);
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
                display_text: None,
            })
            .unwrap(),
    );
    assert!(next.resume_session.is_none());
    assert!(next.context.recent.is_empty());
}

#[test]
fn workspace_recovery_handles_all_live_turns_without_touching_other_workspaces() {
    let temp = tempfile::tempdir().unwrap();
    let workspace_a = temp.path().join("workspace-a");
    let workspace_b = temp.path().join("workspace-b");
    std::fs::create_dir(&workspace_a).unwrap();
    std::fs::create_dir(&workspace_b).unwrap();
    let database = temp.path().join("dmuxai.sqlite3");

    let queued_conversation;
    let running_conversation;
    {
        let mut engine = ConversationEngine::open(&database, &workspace_a).unwrap();
        queued_conversation = engine
            .select(ConversationSelection::New {
                title: Some("Queued in A".to_owned()),
            })
            .unwrap()
            .id;
        let queued = ready(
            engine
                .begin_turn(BeginTurn {
                    agent: AgentKey::Cursor,
                    model_id: None,
                    prompt: "queued".to_owned(),
                    display_text: None,
                })
                .unwrap(),
        );
        engine
            .record_turn_outputs(&[TurnOutput {
                conversation_id: queued.conversation_id.clone(),
                turn_id: queued.turn_id.clone(),
                agent: queued.agent,
                event: TurnEvent::ProviderSessionBound("cursor-queued-a".to_owned()),
            }])
            .unwrap();

        running_conversation = engine
            .select(ConversationSelection::New {
                title: Some("Running in A".to_owned()),
            })
            .unwrap()
            .id;
        let running = ready(
            engine
                .begin_turn(BeginTurn {
                    agent: AgentKey::Agy,
                    model_id: None,
                    prompt: "running".to_owned(),
                    display_text: None,
                })
                .unwrap(),
        );
        let envelope = |event| TurnOutput {
            conversation_id: running.conversation_id.clone(),
            turn_id: running.turn_id.clone(),
            agent: running.agent,
            event,
        };
        engine
            .record_turn_outputs(&[
                envelope(TurnEvent::Started),
                envelope(TurnEvent::ProviderSessionBound("agy-running-a".to_owned())),
                envelope(TurnEvent::TextDelta("partial-a".to_owned())),
            ])
            .unwrap();
    }

    let other_conversation;
    {
        let mut engine = ConversationEngine::open(&database, &workspace_b).unwrap();
        other_conversation = engine
            .select(ConversationSelection::New {
                title: Some("Running in B".to_owned()),
            })
            .unwrap()
            .id;
        let running = ready(
            engine
                .begin_turn(BeginTurn {
                    agent: AgentKey::Codex,
                    model_id: None,
                    prompt: "other workspace".to_owned(),
                    display_text: None,
                })
                .unwrap(),
        );
        let envelope = |event| TurnOutput {
            conversation_id: running.conversation_id.clone(),
            turn_id: running.turn_id.clone(),
            agent: running.agent,
            event,
        };
        engine
            .record_turn_outputs(&[
                envelope(TurnEvent::Started),
                envelope(TurnEvent::ProviderSessionBound(
                    "codex-running-b".to_owned(),
                )),
                envelope(TurnEvent::TextDelta("partial-b".to_owned())),
            ])
            .unwrap();
    }

    let mut recovered = ConversationEngine::open(&database, &workspace_a).unwrap();
    assert_eq!(recovered.recover_workspace().unwrap(), 2);
    assert_eq!(recovered.recover_workspace().unwrap(), 0);

    let queued = recovered
        .select(ConversationSelection::Existing(queued_conversation))
        .unwrap();
    assert_eq!(queued.turns[0].status, TurnStatus::Interrupted);
    assert_eq!(queued.messages.len(), 1);
    assert_eq!(queued.bindings[0].status, BindingStatus::Stale);

    let running = recovered
        .select(ConversationSelection::Existing(running_conversation))
        .unwrap();
    assert_eq!(running.turns[0].status, TurnStatus::Interrupted);
    assert_eq!(running.messages[1].state, MessageState::Interrupted);
    assert_eq!(running.messages[1].text, "partial-a");
    assert_eq!(running.bindings[0].status, BindingStatus::Stale);

    let mut untouched = ConversationEngine::open(&database, &workspace_b).unwrap();
    let other = untouched
        .select(ConversationSelection::Existing(other_conversation))
        .unwrap();
    assert_eq!(other.turns[0].status, TurnStatus::Running);
    assert_eq!(other.messages[1].state, MessageState::Streaming);
    assert_eq!(other.messages[1].text, "partial-b");
    assert_eq!(other.bindings[0].status, BindingStatus::Active);
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
            display_text: None,
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
                display_text: None,
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
                    display_text: None,
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
    assert_eq!(reopened.recover_workspace().unwrap(), 1);
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
                display_text: None,
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

#[test]
fn explicit_summary_is_incremental_and_invalidates_native_resume() {
    let harness = Harness::new();
    let mut engine = harness.open();
    let conversation_id = engine
        .select(ConversationSelection::New { title: None })
        .unwrap()
        .id;
    assert!(engine.prepare_summary(AgentKey::Codex).unwrap().is_none());

    let first = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "first prompt".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    complete_turn(&mut engine, &first, "thread-1", "first answer");

    let first_job = engine.prepare_summary(AgentKey::Codex).unwrap().unwrap();
    assert!(first_job.previous_summary().is_none());
    assert_eq!(first_job.covers_through_seq(), 1);
    assert_eq!(first_job.source_turns().len(), 1);
    assert_eq!(first_job.source_turns()[0].seq, 1);
    engine
        .commit_summary(
            first_job,
            SummaryDocument {
                goal: Some("first summary".to_owned()),
                ..SummaryDocument::default()
            },
        )
        .unwrap();

    let after_first_summary = engine
        .select(ConversationSelection::Existing(conversation_id))
        .unwrap();
    assert_eq!(after_first_summary.bindings[0].status, BindingStatus::Stale);
    let second = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "second prompt".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    assert!(second.resume_session.is_none());
    assert!(
        second
            .context
            .summary
            .as_deref()
            .unwrap()
            .contains("first summary")
    );
    complete_turn(&mut engine, &second, "thread-2", "second answer");

    let second_job = engine.prepare_summary(AgentKey::Codex).unwrap().unwrap();
    assert_eq!(second_job.covers_through_seq(), 2);
    assert_eq!(
        second_job.previous_summary().unwrap().goal.as_deref(),
        Some("first summary")
    );
    assert_eq!(second_job.source_turns().len(), 1);
    assert_eq!(second_job.source_turns()[0].seq, 2);
}

#[test]
fn explicit_summary_rejects_a_busy_agent() {
    let harness = Harness::new();
    let mut engine = harness.open();
    engine
        .select(ConversationSelection::New { title: None })
        .unwrap();
    ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Agy,
                model_id: None,
                prompt: "still queued".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );

    assert!(matches!(
        engine.prepare_summary(AgentKey::Agy),
        Err(ConversationError::AgentBusy(AgentKey::Agy))
    ));
}

#[test]
fn oversized_summary_is_rejected_without_invalidating_the_binding() {
    let harness = Harness::new();
    let mut engine = harness.open();
    engine
        .select(ConversationSelection::New { title: None })
        .unwrap();
    let first = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Cursor,
                model_id: None,
                prompt: "small history".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    complete_turn(&mut engine, &first, "cursor-session", "small answer");
    let job = engine.prepare_summary(AgentKey::Cursor).unwrap().unwrap();

    assert!(matches!(
        engine.commit_summary(
            job,
            SummaryDocument {
                goal: Some("x".repeat(40_000)),
                ..SummaryDocument::default()
            }
        ),
        Err(ConversationError::SummaryTooLarge {
            actual_chars,
            maximum_chars,
        }) if actual_chars > maximum_chars
    ));

    let next = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Cursor,
                model_id: None,
                prompt: "binding remains usable".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    assert_eq!(next.resume_session.as_deref(), Some("cursor-session"));
}

#[test]
fn unicode_summary_uses_the_same_character_budget_as_projection() {
    let harness = Harness::new();
    let mut engine = harness.open();
    engine
        .select(ConversationSelection::New { title: None })
        .unwrap();
    let first = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "한국어 요약".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    complete_turn(&mut engine, &first, "codex-thread", "짧은 답변");
    let job = engine.prepare_summary(AgentKey::Codex).unwrap().unwrap();
    engine
        .commit_summary(
            job,
            SummaryDocument {
                goal: Some("한".repeat(15_000)),
                ..SummaryDocument::default()
            },
        )
        .unwrap();

    assert!(matches!(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "요약 뒤 계속".to_owned(),
                display_text: None,
            })
            .unwrap(),
        BeginTurnOutcome::Ready(_)
    ));
}

#[test]
fn failed_initial_migration_rolls_back_every_schema_change() {
    let harness = Harness::new();
    let connection = Connection::open(&harness.database).unwrap();
    connection
        .execute_batch("CREATE VIEW turns AS SELECT 1 AS id;")
        .unwrap();
    drop(connection);

    assert!(ConversationEngine::open(&harness.database, &harness.workspace).is_err());

    let connection = Connection::open(&harness.database).unwrap();
    let conversations: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
              WHERE type = 'table' AND name = 'conversations'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(conversations, 0);
    assert_eq!(version, 0);
}

// TC-052 / FR-051, BR-050: closed and expired owners are recovered, including
// their partial assistant text and provider binding.
#[test]
fn closed_or_expired_session_turns_are_recovered() {
    let harness = Harness::new();
    let mut closed = harness.open();
    let closed_conversation = closed
        .select(ConversationSelection::New { title: None })
        .unwrap()
        .id;
    let closed_request = ready(
        closed
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "closed owner".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    closed
        .record_turn_outputs(&[
            TurnOutput {
                conversation_id: closed_request.conversation_id.clone(),
                turn_id: closed_request.turn_id.clone(),
                agent: closed_request.agent,
                event: TurnEvent::Started,
            },
            TurnOutput {
                conversation_id: closed_request.conversation_id.clone(),
                turn_id: closed_request.turn_id.clone(),
                agent: closed_request.agent,
                event: TurnEvent::ProviderSessionBound("closed-session".to_owned()),
            },
            TurnOutput {
                conversation_id: closed_request.conversation_id.clone(),
                turn_id: closed_request.turn_id.clone(),
                agent: closed_request.agent,
                event: TurnEvent::TextDelta("partial closed".to_owned()),
            },
        ])
        .unwrap();
    closed.close_session().unwrap();

    let mut live = harness.open();
    let expired_conversation = live
        .select(ConversationSelection::New { title: None })
        .unwrap()
        .id;
    let expired_request = ready(
        live.begin_turn(BeginTurn {
            agent: AgentKey::Cursor,
            model_id: None,
            prompt: "expired owner".to_owned(),
            display_text: None,
        })
        .unwrap(),
    );
    live.record_turn_outputs(&[
        TurnOutput {
            conversation_id: expired_request.conversation_id.clone(),
            turn_id: expired_request.turn_id.clone(),
            agent: expired_request.agent,
            event: TurnEvent::Started,
        },
        TurnOutput {
            conversation_id: expired_request.conversation_id.clone(),
            turn_id: expired_request.turn_id.clone(),
            agent: expired_request.agent,
            event: TurnEvent::ProviderSessionBound("expired-session".to_owned()),
        },
        TurnOutput {
            conversation_id: expired_request.conversation_id.clone(),
            turn_id: expired_request.turn_id.clone(),
            agent: expired_request.agent,
            event: TurnEvent::TextDelta("partial expired".to_owned()),
        },
    ])
    .unwrap();

    let connection = Connection::open(&harness.database).unwrap();
    connection
        .execute(
            "UPDATE runtime_sessions SET last_seen_at_ms = 0 WHERE status = 'active'",
            [],
        )
        .unwrap();
    drop(connection);

    let mut recovered = harness.open();
    assert_eq!(recovered.recover_workspace().unwrap(), 2);

    for conversation_id in [closed_conversation, expired_conversation] {
        let snapshot = recovered
            .select(ConversationSelection::Existing(conversation_id))
            .unwrap();
        assert_eq!(snapshot.turns[0].status, TurnStatus::Interrupted);
        assert_eq!(snapshot.messages[1].state, MessageState::Interrupted);
        assert!(snapshot.messages[1].text.starts_with("partial "));
        assert_eq!(snapshot.bindings[0].status, BindingStatus::Stale);
    }
}

// TC-054 / BR-051: the busy invariant is checked inside the write transaction
// and applies across separate runtime-session database connections.
#[test]
fn busy_agent_is_rejected_across_runtime_sessions() {
    let harness = Harness::new();
    let mut first = harness.open();
    let conversation_id = first
        .select(ConversationSelection::New { title: None })
        .unwrap()
        .id;
    ready(
        first
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "first live turn".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );

    let mut second = harness.open();
    second
        .select(ConversationSelection::Existing(conversation_id.clone()))
        .unwrap();
    assert!(matches!(
        second.begin_turn(BeginTurn {
            agent: AgentKey::Codex,
            model_id: None,
            prompt: "must wait".to_owned(),
            display_text: None,
        }),
        Err(ConversationError::AgentBusy(AgentKey::Codex))
    ));

    let snapshot = second
        .select(ConversationSelection::Existing(conversation_id))
        .unwrap();
    assert_eq!(snapshot.turns.len(), 1);
}

// TC-055 / BR-051: separate instance conversations can run the same provider
// concurrently without sharing the per-conversation busy state.
#[test]
fn same_agent_can_run_in_separate_instance_conversations() {
    let harness = Harness::new();
    let mut first = harness.open();
    first
        .select(ConversationSelection::New { title: None })
        .unwrap();
    ready(
        first
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "first window".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );

    let mut second = harness.open();
    second
        .select(ConversationSelection::New { title: None })
        .unwrap();
    assert!(matches!(
        second.begin_turn(BeginTurn {
            agent: AgentKey::Codex,
            model_id: None,
            prompt: "second window".to_owned(),
            display_text: None,
        }),
        Ok(BeginTurnOutcome::Ready(_))
    ));
}

// TC-056 / NFR-050: an old v1-shaped database receives the additive session
// objects transactionally without losing its canonical transcript.
#[test]
fn runtime_session_schema_extension_preserves_existing_data() {
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
                    prompt: "preserve this".to_owned(),
                    display_text: None,
                })
                .unwrap(),
        );
        complete_turn(&mut engine, &request, "agy-preserved", "still here");
    }

    let connection = Connection::open(&harness.database).unwrap();
    connection
        .execute_batch(
            "PRAGMA foreign_keys = OFF;
             DROP INDEX turns_by_owner_status;
             ALTER TABLE turns DROP COLUMN owner_session_id;
             DROP TABLE runtime_sessions;",
        )
        .unwrap();
    drop(connection);

    let mut reopened = harness.open();
    let snapshot = reopened
        .select(ConversationSelection::Existing(conversation_id))
        .unwrap();
    assert_eq!(snapshot.messages[1].text, "still here");
    assert_eq!(snapshot.turns[0].status, TurnStatus::Completed);

    let connection = Connection::open(&harness.database).unwrap();
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    let session_table: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'runtime_sessions'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let owner_column: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('turns') WHERE name = 'owner_session_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        version, 3,
        "v3 migrates to include display_text and activities"
    );
    assert_eq!(session_table, 1);
    assert_eq!(owner_column, 1);
}

// TC-1221 / SPEC-dashboard-fullwidth FR-1225: other workspaces' sessions are
// read from their own per-workspace databases, read-only, without the copies
// the legacy migration left in other databases.
#[test]
fn state_dir_listing_reads_other_workspaces() {
    let temp = tempfile::tempdir().unwrap();
    let repos = temp.path().join("repos");
    let open = |name: &str| {
        let ws = temp.path().join(name);
        std::fs::create_dir_all(&ws).unwrap();
        let key = dmuxai::workspace::normalize_workspace_path(&ws).unwrap();
        let db = repos
            .join(dmuxai::state_path::repo_key_for_canonical(&key))
            .join("conversations.sqlite3");
        (ConversationEngine::open(&db, &ws).unwrap(), key, db)
    };
    let (mut current, current_key, current_db) = open("current");
    let (mut other, other_key, other_db) = open("other");
    current
        .select(ConversationSelection::New {
            title: Some("mine".to_owned()),
        })
        .unwrap();
    let first = other
        .select(ConversationSelection::New {
            title: Some("older".to_owned()),
        })
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let second = other
        .select(ConversationSelection::New {
            title: Some("newer".to_owned()),
        })
        .unwrap();
    drop(other);
    // A legacy copy of the other workspace's row inside the current database.
    let copy = Connection::open(&current_db).unwrap();
    copy.execute(
        "INSERT INTO conversations (id, workspace_root, title, created_at_ms, updated_at_ms)
         VALUES ('legacy-copy', ?1, 'stale copy', 1, 1)",
        [&other_key],
    )
    .unwrap();
    drop(copy);
    let before = std::fs::read(&other_db).unwrap();

    let rows = dmuxai::conversation::list_state_dir_conversations(&repos, &current_key, 50);
    let titles: Vec<(&str, &str)> = rows
        .iter()
        .map(|(ws, card)| (ws.as_str(), card.title.as_str()))
        .collect();
    assert_eq!(
        titles,
        [(other_key.as_str(), "newer"), (other_key.as_str(), "older")]
    );
    assert_eq!(rows[0].1.id, second.id);
    assert_eq!(rows[1].1.id, first.id);
    assert_eq!(
        dmuxai::conversation::list_state_dir_conversations(&repos, &current_key, 1).len(),
        1
    );
    assert!(
        dmuxai::conversation::list_state_dir_conversations(
            &temp.path().join("none"),
            &current_key,
            5
        )
        .is_empty()
    );
    assert_eq!(std::fs::read(&other_db).unwrap(), before);
    drop(current);
}

// TC-1900 / FR-1900: v2 -> v3 migration adds display_text to messages and activities table.
#[test]
fn tc_1900_v2_to_v3_migration_adds_display_text_and_activities() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path().join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    let db_path = temp.path().join("v2_test.sqlite3");
    let ws_key = dmuxai::workspace::normalize_workspace_path(&ws).unwrap();

    // 1. Build a v2 database by hand
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE runtime_sessions (
                id TEXT PRIMARY KEY,
                workspace_root TEXT NOT NULL,
                status TEXT NOT NULL CHECK (status IN ('active', 'closed')),
                created_at_ms INTEGER NOT NULL,
                last_seen_at_ms INTEGER NOT NULL,
                closed_at_ms INTEGER
            );
            CREATE TABLE conversations (
                id TEXT PRIMARY KEY,
                workspace_root TEXT NOT NULL,
                title TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                archived_at_ms INTEGER
            );
            CREATE TABLE turns (
                id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                seq INTEGER NOT NULL,
                requested_agent TEXT NOT NULL CHECK (length(requested_agent) > 0),
                model_id TEXT,
                status TEXT NOT NULL,
                error_text TEXT,
                context_through_seq INTEGER NOT NULL DEFAULT 0,
                created_at_ms INTEGER NOT NULL,
                started_at_ms INTEGER,
                completed_at_ms INTEGER,
                owner_session_id TEXT REFERENCES runtime_sessions(id),
                UNIQUE (conversation_id, seq)
            );
            CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                turn_id TEXT NOT NULL REFERENCES turns(id) ON DELETE CASCADE,
                ordinal INTEGER NOT NULL CHECK (ordinal IN (0, 1)),
                role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
                text TEXT NOT NULL DEFAULT '',
                state TEXT NOT NULL CHECK (state IN ('complete', 'streaming', 'interrupted')),
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                UNIQUE (turn_id, ordinal),
                UNIQUE (turn_id, role)
            );
            CREATE TABLE provider_bindings (
                conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                agent TEXT NOT NULL CHECK (length(agent) > 0),
                provider_session_id TEXT NOT NULL,
                status TEXT NOT NULL,
                known_through_seq INTEGER NOT NULL DEFAULT 0,
                bound_by_turn_id TEXT REFERENCES turns(id),
                created_at_ms INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL,
                PRIMARY KEY (conversation_id, agent),
                UNIQUE (agent, provider_session_id)
            );
            CREATE TABLE context_checkpoints (
                conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                agent TEXT NOT NULL CHECK (length(agent) > 0),
                covers_through_seq INTEGER NOT NULL,
                source_hash BLOB NOT NULL CHECK (length(source_hash) = 32),
                prompt_version TEXT NOT NULL,
                summary_json TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                PRIMARY KEY (conversation_id, agent, covers_through_seq)
            );
            PRAGMA user_version = 2;
            "#,
        )
        .unwrap();

        conn.execute(
            "INSERT INTO conversations (id, workspace_root, title, created_at_ms, updated_at_ms)
             VALUES ('conv-v2', ?1, 'v2 conversation', 1000, 1000)",
            [&ws_key],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO turns (id, conversation_id, seq, requested_agent, status, created_at_ms)
             VALUES ('turn-v2', 'conv-v2', 1, 'codex', 'completed', 1000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, turn_id, ordinal, role, text, state, created_at_ms, updated_at_ms)
             VALUES ('msg-u', 'turn-v2', 0, 'user', 'v2 user text', 'complete', 1000, 1000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id, turn_id, ordinal, role, text, state, created_at_ms, updated_at_ms)
             VALUES ('msg-a', 'turn-v2', 1, 'assistant', 'v2 assistant reply', 'complete', 1000, 1000)",
            [],
        )
        .unwrap();
    }

    // 2. Open with ConversationEngine
    let mut engine = ConversationEngine::open(&db_path, &ws).unwrap();
    let snapshot = engine
        .select(ConversationSelection::Existing(
            dmuxai::conversation::ConversationId::new("conv-v2"),
        ))
        .unwrap();

    // Check version
    let conn = Connection::open(&db_path).unwrap();
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, 3);

    // messages.display_text column exists and is NULL for old rows
    let display_text: Option<String> = conn
        .query_row(
            "SELECT display_text FROM messages WHERE id = 'msg-u'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(display_text.is_none());

    // activities table exists
    let activities_table: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'activities'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(activities_table, 1);

    // Old transcript loads unchanged
    assert_eq!(snapshot.messages.len(), 2);
    assert_eq!(snapshot.messages[0].text, "v2 user text");
    assert_eq!(snapshot.messages[0].display_text, None);
    assert_eq!(snapshot.messages[0].shown_text(), "v2 user text");
    assert_eq!(snapshot.messages[1].text, "v2 assistant reply");
    assert_eq!(snapshot.messages[1].display_text, None);
    assert_eq!(snapshot.messages[1].shown_text(), "v2 assistant reply");
    drop(conn);
    drop(engine);

    // Opening again is a no-op
    let mut reopened = ConversationEngine::open(&db_path, &ws).unwrap();
    let snapshot2 = reopened
        .select(ConversationSelection::Existing(
            dmuxai::conversation::ConversationId::new("conv-v2"),
        ))
        .unwrap();
    assert_eq!(snapshot2.messages[0].shown_text(), "v2 user text");
}

// TC-1901 / FR-1901: begin_turn stores display_text and preserves prompt in context projection.
#[test]
fn tc_1901_begin_turn_stores_display_text_and_preserves_prompt_projection() {
    let harness = Harness::new();
    let mut engine = harness.open();
    let conversation_id = engine
        .select(ConversationSelection::New { title: None })
        .unwrap()
        .id;

    // 1. begin_turn with custom display_text
    let req1 = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "Actual prompt sent to LLM with full context prefix".to_owned(),
                display_text: Some("Short User Question".to_owned()),
            })
            .unwrap(),
    );
    assert_eq!(
        req1.prompt,
        "Actual prompt sent to LLM with full context prefix"
    );

    let snapshot1 = engine
        .select(ConversationSelection::Existing(conversation_id.clone()))
        .unwrap();
    assert_eq!(snapshot1.messages.len(), 1);
    assert_eq!(
        snapshot1.messages[0].text,
        "Actual prompt sent to LLM with full context prefix"
    );
    assert_eq!(
        snapshot1.messages[0].display_text.as_deref(),
        Some("Short User Question")
    );
    assert_eq!(snapshot1.messages[0].shown_text(), "Short User Question");

    complete_turn(&mut engine, &req1, "thread-1", "LLM Answer");

    // 2. Second turn: verify context projection uses `text` (the sent prompt), NOT display_text
    let req2 = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "Follow-up question".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    assert_eq!(req2.context.recent.len(), 1);
    assert_eq!(
        req2.context.recent[0].user,
        "Actual prompt sent to LLM with full context prefix"
    );

    // 3. display_text None stores NULL
    let snapshot2 = engine
        .select(ConversationSelection::Existing(conversation_id.clone()))
        .unwrap();
    let user_msg2 = snapshot2
        .messages
        .iter()
        .find(|m| m.text == "Follow-up question")
        .unwrap();
    assert_eq!(user_msg2.display_text, None);
    assert_eq!(user_msg2.shown_text(), "Follow-up question");

    // 4. display_text equal to prompt stores NULL
    complete_turn(&mut engine, &req2, "thread-1", "Answer 2");
    let req3 = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "Same text".to_owned(),
                display_text: Some("Same text".to_owned()),
            })
            .unwrap(),
    );
    let snapshot3 = engine
        .select(ConversationSelection::Existing(conversation_id))
        .unwrap();
    let user_msg3 = snapshot3
        .messages
        .iter()
        .find(|m| m.text == "Same text")
        .unwrap();
    assert_eq!(user_msg3.display_text, None);
    assert_eq!(user_msg3.shown_text(), "Same text");
    complete_turn(&mut engine, &req3, "thread-1", "Answer 3");
}

// TC-1902 / FR-1902: begin_turn sets conversation title using display_text when untitled.
#[test]
fn tc_1902_begin_turn_sets_conversation_title_from_display_text() {
    let harness = Harness::new();
    let mut engine = harness.open();
    let conversation_id = engine
        .select(ConversationSelection::New { title: None })
        .unwrap()
        .id;

    // First turn with injected long prompt and short display_text
    let req = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "System instruction header... Long prompt that should not be title"
                    .to_owned(),
                display_text: Some("Feature Implementation Plan".to_owned()),
            })
            .unwrap(),
    );
    complete_turn(&mut engine, &req, "thread-1", "Done");

    let snapshot = engine
        .select(ConversationSelection::Existing(conversation_id.clone()))
        .unwrap();
    assert_eq!(snapshot.title, "Feature Implementation Plan");

    // Subsequent turn does not overwrite the title
    let req2 = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "Next prompt".to_owned(),
                display_text: Some("Different Title".to_owned()),
            })
            .unwrap(),
    );
    complete_turn(&mut engine, &req2, "thread-1", "Done 2");

    let snapshot2 = engine
        .select(ConversationSelection::Existing(conversation_id))
        .unwrap();
    assert_eq!(snapshot2.title, "Feature Implementation Plan");
}

// TC-1903 / FR-1903: set_title_if_untitled updates only untitled conversations and ignores blank text.
#[test]
fn tc_1903_set_title_if_untitled_updates_only_new_conversation() {
    let harness = Harness::new();
    let mut engine = harness.open();

    // No active conversation returns error
    assert!(matches!(
        engine.set_title_if_untitled("Some Title"),
        Err(ConversationError::NoActiveConversation)
    ));

    // Active new conversation
    let conv = engine
        .select(ConversationSelection::New { title: None })
        .unwrap();
    assert_eq!(conv.title, "New conversation");

    // Blank text returns false and does not change title
    assert!(!engine.set_title_if_untitled("   ").unwrap());
    let snapshot = engine
        .select(ConversationSelection::Existing(conv.id.clone()))
        .unwrap();
    assert_eq!(snapshot.title, "New conversation");

    // Setting valid title returns true
    assert!(engine.set_title_if_untitled("My User Task").unwrap());
    let snapshot2 = engine
        .select(ConversationSelection::Existing(conv.id.clone()))
        .unwrap();
    assert_eq!(snapshot2.title, "My User Task");

    // Subsequent call returns false because title is no longer 'New conversation'
    assert!(!engine.set_title_if_untitled("Another Title").unwrap());
    let snapshot3 = engine
        .select(ConversationSelection::Existing(conv.id))
        .unwrap();
    assert_eq!(snapshot3.title, "My User Task");
}

// TC-1905 / FR-1905: record_activity merges and upserts durable activity rows across reopen.
#[test]
fn tc_1905_record_activity_merges_and_upserts_durable_rows() {
    use dmuxai::activity::{ActivityItem, ToolStatus};
    use dmuxai::conversation::TurnId;

    let harness = Harness::new();
    let mut engine = harness.open();
    let conversation_id = engine
        .select(ConversationSelection::New { title: None })
        .unwrap()
        .id;

    // Unknown turn ID returns error
    assert!(
        engine
            .record_activity(
                &TurnId::new("nonexistent-turn"),
                ActivityItem::Tool {
                    name: "exec".to_string(),
                    target: "ls".to_string(),
                    status: ToolStatus::Running,
                    duration_ms: None,
                    detail: "".to_string(),
                }
            )
            .is_err()
    );

    let req = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "Run tools".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );

    // Record tool running
    engine
        .record_activity(
            &req.turn_id,
            ActivityItem::Tool {
                name: "exec".to_string(),
                target: "ls".to_string(),
                status: ToolStatus::Running,
                duration_ms: None,
                detail: "".to_string(),
            },
        )
        .unwrap();

    // Record live thinking
    engine
        .record_activity(
            &req.turn_id,
            ActivityItem::Thinking {
                text: "Reasoning step 1".to_string(),
                started_ms: 1000,
                finished_ms: None,
            },
        )
        .unwrap();

    // Tool completion merges into ordinal 0
    engine
        .record_activity(
            &req.turn_id,
            ActivityItem::Tool {
                name: "exec".to_string(),
                target: "ls".to_string(),
                status: ToolStatus::Ok,
                duration_ms: Some(200),
                detail: "file1 file2".to_string(),
            },
        )
        .unwrap();

    // Thinking delta merges into ordinal 1
    engine
        .record_activity(
            &req.turn_id,
            ActivityItem::Thinking {
                text: "... reasoning step 2".to_string(),
                started_ms: 0,
                finished_ms: Some(1500),
            },
        )
        .unwrap();

    // Drop engine and reopen
    drop(engine);
    let mut reopened = harness.open();
    let snapshot = reopened
        .select(ConversationSelection::Existing(conversation_id))
        .unwrap();

    // Assert rows survived and are ordered
    assert_eq!(snapshot.activities.len(), 2);
    assert_eq!(snapshot.activities[0].ordinal, 0);
    assert_eq!(
        snapshot.activities[0].item,
        ActivityItem::Tool {
            name: "exec".to_string(),
            target: "ls".to_string(),
            status: ToolStatus::Ok,
            duration_ms: Some(200),
            detail: "file1 file2".to_string(),
        }
    );
    assert_eq!(snapshot.activities[1].ordinal, 1);
    assert_eq!(
        snapshot.activities[1].item,
        ActivityItem::Thinking {
            text: "Reasoning step 1... reasoning step 2".to_string(),
            started_ms: 1000,
            finished_ms: Some(1500),
        }
    );
}

// TC-1906 / FR-1906: record_activity enforces bounds on characters and row count caps.
#[test]
fn tc_1906_record_activity_enforces_bounds_and_caps() {
    use dmuxai::activity::{ActivityItem, ToolStatus};
    use dmuxai::conversation::{MAX_ACTIVITIES_PER_TURN, MAX_ACTIVITY_TEXT_CHARS};

    let harness = Harness::new();
    let mut engine = harness.open();
    let conversation_id = engine
        .select(ConversationSelection::New { title: None })
        .unwrap()
        .id;

    let req = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "Stress test activities".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );

    // 1. Long text is bounded to MAX_ACTIVITY_TEXT_CHARS (16,000)
    let long_text = "T".repeat(MAX_ACTIVITY_TEXT_CHARS + 5000);
    engine
        .record_activity(
            &req.turn_id,
            ActivityItem::Thinking {
                text: long_text,
                started_ms: 100,
                finished_ms: None,
            },
        )
        .unwrap();

    let snapshot = engine
        .select(ConversationSelection::Existing(conversation_id.clone()))
        .unwrap();
    if let ActivityItem::Thinking { text, .. } = &snapshot.activities[0].item {
        assert_eq!(text.chars().count(), MAX_ACTIVITY_TEXT_CHARS);
    } else {
        panic!("expected thinking at index 0");
    }

    // Verify summary in database is bounded to 200 chars
    let conn = Connection::open(&harness.database).unwrap();
    let summary: String = conn
        .query_row(
            "SELECT summary FROM activities WHERE turn_id = ?1 AND ordinal = 0",
            [req.turn_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(summary.chars().count() <= 200);

    // 2. Cap at MAX_ACTIVITIES_PER_TURN (400)
    // We currently have 1 activity (ordinal 0). Add 399 more distinct tools to reach 400.
    for i in 1..MAX_ACTIVITIES_PER_TURN {
        engine
            .record_activity(
                &req.turn_id,
                ActivityItem::Tool {
                    name: format!("tool_{i}"),
                    target: "".to_string(),
                    status: ToolStatus::Ok,
                    duration_ms: None,
                    detail: "".to_string(),
                },
            )
            .unwrap();
    }

    let snapshot_400 = engine
        .select(ConversationSelection::Existing(conversation_id.clone()))
        .unwrap();
    assert_eq!(snapshot_400.activities.len(), MAX_ACTIVITIES_PER_TURN);

    // 401st distinct activity: ignored (returns Ok, no row added)
    engine
        .record_activity(
            &req.turn_id,
            ActivityItem::Tool {
                name: "tool_overflow".to_string(),
                target: "".to_string(),
                status: ToolStatus::Ok,
                duration_ms: None,
                detail: "".to_string(),
            },
        )
        .unwrap();

    let snapshot_still_400 = engine
        .select(ConversationSelection::Existing(conversation_id.clone()))
        .unwrap();
    assert_eq!(snapshot_still_400.activities.len(), MAX_ACTIVITIES_PER_TURN);

    // Existing row updates still apply even at cap
    engine
        .record_activity(
            &req.turn_id,
            ActivityItem::Thinking {
                text: " finished".to_string(),
                started_ms: 0,
                finished_ms: Some(999),
            },
        )
        .unwrap();

    let snapshot_updated = engine
        .select(ConversationSelection::Existing(conversation_id))
        .unwrap();
    if let ActivityItem::Thinking { finished_ms, .. } = &snapshot_updated.activities[0].item {
        assert_eq!(*finished_ms, Some(999));
    } else {
        panic!("expected thinking at index 0");
    }
}

// TC-1907 / FR-1907: snapshot activities are ordered by turn seq then ordinal; malformed rows skipped; cascade delete works.
#[test]
fn tc_1907_conversation_snapshot_activities_ordered_and_cascade() {
    use dmuxai::activity::{ActivityItem, ToolStatus};

    let harness = Harness::new();
    let mut engine = harness.open();
    let conversation_id = engine
        .select(ConversationSelection::New { title: None })
        .unwrap()
        .id;

    // Turn 1
    let req1 = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "Turn 1".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    engine
        .record_activity(
            &req1.turn_id,
            ActivityItem::Tool {
                name: "tool_t1".to_string(),
                target: "".to_string(),
                status: ToolStatus::Ok,
                duration_ms: None,
                detail: "".to_string(),
            },
        )
        .unwrap();
    complete_turn(&mut engine, &req1, "thread-1", "Done 1");

    // Turn 2
    let req2 = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "Turn 2".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    engine
        .record_activity(
            &req2.turn_id,
            ActivityItem::Tool {
                name: "tool_t2_ord0".to_string(),
                target: "".to_string(),
                status: ToolStatus::Ok,
                duration_ms: None,
                detail: "".to_string(),
            },
        )
        .unwrap();
    engine
        .record_activity(
            &req2.turn_id,
            ActivityItem::Tool {
                name: "tool_t2_ord1".to_string(),
                target: "".to_string(),
                status: ToolStatus::Ok,
                duration_ms: None,
                detail: "".to_string(),
            },
        )
        .unwrap();

    // Insert a malformed payload_json row directly via SQLite
    let conn = Connection::open(&harness.database).unwrap();
    conn.execute(
        "INSERT INTO activities (turn_id, ordinal, kind, summary, state, payload_json, created_at_ms, updated_at_ms)
         VALUES (?1, 2, 'tool', 'malformed', 'ok', '{invalid_json', 1000, 1000)",
        [req2.turn_id.as_str()],
    )
    .unwrap();
    drop(conn);

    // Verify ordering and that malformed row is skipped
    let snapshot = engine
        .select(ConversationSelection::Existing(conversation_id.clone()))
        .unwrap();
    assert_eq!(snapshot.activities.len(), 3);
    assert_eq!(snapshot.activities[0].turn_id, req1.turn_id);
    assert_eq!(snapshot.activities[0].ordinal, 0);
    assert_eq!(snapshot.activities[1].turn_id, req2.turn_id);
    assert_eq!(snapshot.activities[1].ordinal, 0);
    assert_eq!(snapshot.activities[2].turn_id, req2.turn_id);
    assert_eq!(snapshot.activities[2].ordinal, 1);

    // Cascade delete: deleting turn 2 removes its activity rows
    let conn = Connection::open(&harness.database).unwrap();
    conn.execute("PRAGMA foreign_keys = ON;", []).unwrap();
    conn.execute("DELETE FROM turns WHERE id = ?1", [req2.turn_id.as_str()])
        .unwrap();
    let remaining_t2: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM activities WHERE turn_id = ?1",
            [req2.turn_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining_t2, 0);
    // Turn 1 activity still remains
    let remaining_t1: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM activities WHERE turn_id = ?1",
            [req1.turn_id.as_str()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining_t1, 1);
}
