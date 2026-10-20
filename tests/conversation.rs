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
    assert_eq!(version, 2, "v2 drops the built-in-only agent CHECKs");
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
