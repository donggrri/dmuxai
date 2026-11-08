use dmuxai::conversation::{
    AgentKey, BeginTurn, BeginTurnOutcome, BindingStatus, ContextProjection, ConversationEngine,
    ConversationError, ConversationSelection, MAX_UNSEEN_DELTA_BYTES, MAX_UNSEEN_DELTA_TURNS,
    MessageRole, MessageState, ProjectedTurn, RequestContext, SummaryDocument, TaskSettings,
    TurnEvent, TurnOutput, TurnStatus,
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

fn fresh_projection(request: &dmuxai::conversation::AgentRequest) -> &ContextProjection {
    match &request.context {
        RequestContext::Fresh(projection) => projection,
        RequestContext::Resume { fallback, .. } => fallback,
    }
}

fn resume_delta(request: &dmuxai::conversation::AgentRequest) -> &[ProjectedTurn] {
    match &request.context {
        RequestContext::Resume { delta, .. } => delta,
        RequestContext::Fresh(_) => panic!("expected a resumed delta, got a fresh projection"),
    }
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
        assert!(fresh_projection(&request).recent.is_empty());
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
    assert!(resume_delta(&next).is_empty());
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
    assert_eq!(fresh_projection(&cursor).recent.len(), 1);
    assert_eq!(fresh_projection(&cursor).recent[0].agent, AgentKey::Codex);
    assert_eq!(
        fresh_projection(&cursor).recent[0].user,
        "Codex-only secret"
    );
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
    assert_eq!(codex_again.resume_session.as_deref(), Some("codex-thread"));
    let delta = resume_delta(&codex_again);
    assert_eq!(delta.len(), 1);
    assert_eq!(delta[0].seq, 2);
    assert_eq!(delta[0].user, "What do you know?");
    assert_eq!(delta[0].agent, AgentKey::Cursor);
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
    assert!(fresh_projection(&in_b).recent.is_empty());
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
    assert_eq!(fresh_projection(&in_a).recent.len(), 1);
    assert_eq!(fresh_projection(&in_a).recent[0].user, "private A");
    assert!(
        !fresh_projection(&in_a)
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
    assert_eq!(fresh_projection(&back_to_b).recent.len(), 1);
    assert_eq!(fresh_projection(&back_to_b).recent[0].user, "question B");
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
    assert!(fresh_projection(&next).recent.is_empty());
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

    // Resume with no unseen turns keeps the plain session; retire it so this
    // test exercises the bounded summary/recent projection instead.
    engine.retire_active_binding(AgentKey::Codex).unwrap();
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
    let projection = fresh_projection(&next);
    assert!(
        projection
            .summary
            .as_deref()
            .unwrap()
            .contains("Preserve the long-running goal")
    );
    assert!(!projection.recent.is_empty());
    assert!(
        projection
            .recent
            .windows(2)
            .all(|window| window[0].seq < window[1].seq)
    );
    assert_eq!(projection.through_seq, 24);
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
        fresh_projection(&second)
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
fn tc_052_closed_or_expired_session_turns_are_recovered() {
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
fn tc_054_busy_agent_is_rejected_across_runtime_sessions() {
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
fn tc_055_same_agent_can_run_in_separate_instance_conversations() {
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
fn tc_056_runtime_session_schema_extension_preserves_existing_data() {
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
        version, 4,
        "the current schema includes display_text, activities, and task settings"
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
    assert_eq!(version, 4);

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
    // Force a fresh projection so the test inspects the shared context rather
    // than an empty same-agent resume delta.
    engine.retire_active_binding(AgentKey::Codex).unwrap();

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
    assert_eq!(fresh_projection(&req2).recent.len(), 1);
    assert_eq!(
        fresh_projection(&req2).recent[0].user,
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

// TC-2320 / FR-2320: a conversation is already a task; listing and selecting
// it must not copy or rewrite transcript rows.
#[test]
fn tc_2320_existing_conversation_is_task_without_transcript_migration() {
    let harness = Harness::new();
    let mut engine = harness.open();

    let first = engine
        .select(ConversationSelection::New {
            title: Some("First task".to_owned()),
        })
        .unwrap();
    let first_request = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: Some("codex-model".to_owned()),
                prompt: "First prompt".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    complete_turn(&mut engine, &first_request, "first-session", "First answer");

    let second = engine
        .select(ConversationSelection::New {
            title: Some("Second task".to_owned()),
        })
        .unwrap();
    let second_request = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Cursor,
                model_id: Some("cursor-model".to_owned()),
                prompt: "Second prompt".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    complete_turn(
        &mut engine,
        &second_request,
        "second-session",
        "Second answer",
    );

    let transcript_counts = || {
        let connection = Connection::open(&harness.database).unwrap();
        let turns: i64 = connection
            .query_row("SELECT COUNT(*) FROM turns", [], |row| row.get(0))
            .unwrap();
        let messages: i64 = connection
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .unwrap();
        (turns, messages)
    };
    let before = transcript_counts();

    let cards = engine.list().unwrap();
    assert_eq!(cards.len(), 2);
    let first_card = cards.iter().find(|card| card.id == first.id).unwrap();
    assert_eq!(first_card.title, "First task");
    assert_eq!(first_card.turn_count, 1);
    assert_eq!(first_card.last_agent, Some(AgentKey::Codex));
    assert_eq!(first_card.settings.agent.as_deref(), Some("codex"));
    assert_eq!(first_card.settings.model.as_deref(), Some("codex-model"));
    assert!(first_card.settings.effort.is_none());

    let first_again = engine
        .select(ConversationSelection::Existing(first.id.clone()))
        .unwrap();
    assert_eq!(first_again.id, first.id);
    assert_eq!(first_again.title, "First task");
    assert_eq!(first_again.turns.len(), 1);
    assert_eq!(first_again.turns[0].id, first_request.turn_id);
    assert_eq!(first_again.messages.len(), 2);
    assert_eq!(first_again.messages[1].text, "First answer");
    assert_eq!(first_again.bindings.len(), 1);
    assert_eq!(first_again.bindings[0].provider_session_id, "first-session");

    let second_again = engine
        .select(ConversationSelection::Existing(second.id.clone()))
        .unwrap();
    assert_eq!(second_again.id, second.id);
    assert_eq!(second_again.title, "Second task");
    assert_eq!(second_again.turns.len(), 1);
    assert_eq!(second_again.turns[0].id, second_request.turn_id);
    assert_eq!(second_again.messages.len(), 2);
    assert_eq!(second_again.messages[1].text, "Second answer");
    assert_eq!(second_again.bindings.len(), 1);
    assert_eq!(
        second_again.bindings[0].provider_session_id,
        "second-session"
    );

    assert_eq!(transcript_counts(), before);

    drop(engine);
    let mut reopened = harness.open();
    let cards = reopened.list().unwrap();
    assert_eq!(cards.len(), 2);
    assert_eq!(transcript_counts(), before);

    let first_reopened = reopened
        .select(ConversationSelection::Existing(first.id.clone()))
        .unwrap();
    assert_eq!(first_reopened.turns.len(), 1);
    assert_eq!(first_reopened.messages[1].text, "First answer");
    assert_eq!(
        first_reopened.bindings[0].provider_session_id,
        "first-session"
    );
    let second_reopened = reopened
        .select(ConversationSelection::Existing(second.id.clone()))
        .unwrap();
    assert_eq!(second_reopened.turns.len(), 1);
    assert_eq!(second_reopened.messages[1].text, "Second answer");
    assert_eq!(
        second_reopened.bindings[0].provider_session_id,
        "second-session"
    );
    assert_eq!(transcript_counts(), before);
}

// TC-2321 / FR-2321: opening a v3 database adds the nullable task-settings
// columns once, keeps old rows readable, and leaves the transcript untouched.
#[test]
fn tc_2321_task_settings_migration_is_idempotent() {
    let harness = Harness::new();
    let conversation_id;
    {
        let mut engine = harness.open();
        let snapshot = engine
            .select(ConversationSelection::New {
                title: Some("Migrated task".to_owned()),
            })
            .unwrap();
        conversation_id = snapshot.id;
        let request = ready(
            engine
                .begin_turn(BeginTurn {
                    agent: AgentKey::Codex,
                    model_id: Some("codex-model".to_owned()),
                    prompt: "Before migration".to_owned(),
                    display_text: None,
                })
                .unwrap(),
        );
        complete_turn(&mut engine, &request, "migrated-session", "Kept reply");
        engine
            .set_task_settings(TaskSettings {
                agent: Some("cursor".to_owned()),
                model: Some("cursor-model".to_owned()),
                effort: Some("high".to_owned()),
            })
            .unwrap();
    }

    // Reshape the database as an older binary would have left it: no task
    // settings columns and schema version 3.
    {
        let connection = Connection::open(&harness.database).unwrap();
        connection
            .execute_batch(
                "ALTER TABLE conversations DROP COLUMN last_agent;
                 ALTER TABLE conversations DROP COLUMN last_model_id;
                 ALTER TABLE conversations DROP COLUMN last_reasoning_effort;
                 PRAGMA user_version = 3;",
            )
            .unwrap();
    }

    let transcript = |path: &std::path::Path| {
        let connection = Connection::open(path).unwrap();
        let turns: i64 = connection
            .query_row("SELECT COUNT(*) FROM turns", [], |row| row.get(0))
            .unwrap();
        let messages: i64 = connection
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
            .unwrap();
        let reply: String = connection
            .query_row(
                "SELECT text FROM messages WHERE role = 'assistant'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        (turns, messages, reply)
    };
    let before = transcript(&harness.database);

    let mut engine = harness.open();
    let snapshot = engine
        .select(ConversationSelection::Existing(conversation_id.clone()))
        .unwrap();
    assert_eq!(snapshot.title, "Migrated task");
    assert_eq!(snapshot.settings, TaskSettings::default());
    assert_eq!(snapshot.messages[1].text, "Kept reply");
    assert_eq!(snapshot.bindings[0].provider_session_id, "migrated-session");

    let settings_columns = |path: &std::path::Path| {
        let connection = Connection::open(path).unwrap();
        let mut statement = connection
            .prepare("PRAGMA table_info(conversations)")
            .unwrap();
        let names = statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        names
            .into_iter()
            .filter(|name| {
                name == "last_agent" || name == "last_model_id" || name == "last_reasoning_effort"
            })
            .collect::<Vec<_>>()
    };
    {
        let connection = Connection::open(&harness.database).unwrap();
        let version: i64 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 4);
        let old_row: (Option<String>, Option<String>, Option<String>) = connection
            .query_row(
                "SELECT last_agent, last_model_id, last_reasoning_effort
                   FROM conversations WHERE id = ?1",
                [conversation_id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(old_row, (None, None, None));
        let names: Vec<String> = {
            let mut statement = connection
                .prepare("PRAGMA table_info(conversations)")
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert!(
            !names
                .iter()
                .any(|name| name == "mode" || name == "last_mode")
        );
        assert!(!names.iter().any(|name| name == "last_model"));
        assert!(!names.iter().any(|name| name == "last_effort"));
    }
    assert_eq!(
        settings_columns(&harness.database),
        vec![
            "last_agent".to_owned(),
            "last_model_id".to_owned(),
            "last_reasoning_effort".to_owned()
        ]
    );
    assert_eq!(transcript(&harness.database), before);
    drop(engine);

    // Reopening at version 4 is a no-op: no duplicate columns, same transcript.
    let mut reopened = harness.open();
    let snapshot = reopened
        .select(ConversationSelection::Existing(conversation_id))
        .unwrap();
    assert_eq!(snapshot.settings, TaskSettings::default());
    assert_eq!(snapshot.messages[1].text, "Kept reply");
    assert_eq!(
        settings_columns(&harness.database),
        vec![
            "last_agent".to_owned(),
            "last_model_id".to_owned(),
            "last_reasoning_effort".to_owned()
        ]
    );
    assert_eq!(transcript(&harness.database), before);
}

// TC-2322 / FR-2322: task settings round-trip through selection and the list
// card; NULL, blank, and unknown stored values do not block selection or a turn.
#[test]
fn tc_2322_task_settings_round_trip_and_defaults() {
    let harness = Harness::new();
    let mut engine = harness.open();

    let stored = engine
        .select(ConversationSelection::New {
            title: Some("Stored task".to_owned()),
        })
        .unwrap();
    let settings = TaskSettings {
        agent: Some("cursor".to_owned()),
        model: Some("cursor-model".to_owned()),
        effort: Some("high".to_owned()),
    };
    engine.set_task_settings(settings.clone()).unwrap();

    let restored = engine
        .select(ConversationSelection::Existing(stored.id.clone()))
        .unwrap();
    assert_eq!(restored.settings, settings);
    let cards = engine.list().unwrap();
    assert_eq!(
        cards
            .iter()
            .find(|card| card.id == stored.id)
            .unwrap()
            .settings,
        settings
    );

    // begin_turn stores the agent and model actually used; effort is untouched.
    let request = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Agy,
                model_id: Some("agy-model".to_owned()),
                prompt: "Use the actual picker".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    assert_eq!(request.agent, AgentKey::Agy);
    complete_turn(&mut engine, &request, "agy-session", "Done");
    let after_turn = engine
        .select(ConversationSelection::Existing(stored.id.clone()))
        .unwrap();
    assert_eq!(
        after_turn.settings,
        TaskSettings {
            agent: Some("agy".to_owned()),
            model: Some("agy-model".to_owned()),
            effort: Some("high".to_owned()),
        }
    );

    // Blank values are stored as NULL and fall back to the global default.
    engine
        .set_task_settings(TaskSettings {
            agent: Some("   ".to_owned()),
            model: Some(String::new()),
            effort: None,
        })
        .unwrap();
    let cleared = engine
        .select(ConversationSelection::Existing(stored.id.clone()))
        .unwrap();
    assert_eq!(cleared.settings, TaskSettings::default());

    // A task that never stored settings selects with defaults and starts a turn.
    let defaults = engine
        .select(ConversationSelection::New {
            title: Some("Default task".to_owned()),
        })
        .unwrap();
    assert_eq!(defaults.settings, TaskSettings::default());
    {
        let connection = Connection::open(&harness.database).unwrap();
        let values: (Option<String>, Option<String>, Option<String>) = connection
            .query_row(
                "SELECT last_agent, last_model_id, last_reasoning_effort
                   FROM conversations WHERE id = ?1",
                [defaults.id.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(values, (None, None, None));
    }
    let defaults_card = engine
        .list()
        .unwrap()
        .into_iter()
        .find(|card| card.id == defaults.id)
        .unwrap();
    assert_eq!(defaults_card.settings, TaskSettings::default());
    let default_request = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Codex,
                model_id: None,
                prompt: "Use defaults".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    assert_eq!(default_request.model_id, None);

    // Unknown stored values are exposed without blocking selection or a turn;
    // the UI layer resolves them to a supported default.
    let unknown = engine
        .select(ConversationSelection::New {
            title: Some("Unknown task".to_owned()),
        })
        .unwrap();
    engine
        .set_task_settings(TaskSettings {
            agent: Some("custom-unknown".to_owned()),
            model: Some("unknown-model".to_owned()),
            effort: Some("ultra".to_owned()),
        })
        .unwrap();
    let unknown_snapshot = engine
        .select(ConversationSelection::Existing(unknown.id.clone()))
        .unwrap();
    assert_eq!(
        unknown_snapshot.settings,
        TaskSettings {
            agent: Some("custom-unknown".to_owned()),
            model: Some("unknown-model".to_owned()),
            effort: Some("ultra".to_owned()),
        }
    );
    let unknown_card = engine
        .list()
        .unwrap()
        .into_iter()
        .find(|card| card.id == unknown.id)
        .unwrap();
    assert_eq!(
        unknown_card.settings.model.as_deref(),
        Some("unknown-model")
    );
    let unknown_request = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Cursor,
                model_id: Some("cursor-model".to_owned()),
                prompt: "Unknown stored model must not block".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    assert_eq!(unknown_request.agent, AgentKey::Cursor);
}

fn begin(
    engine: &mut ConversationEngine,
    agent: AgentKey,
    model_id: Option<&str>,
    prompt: &str,
) -> dmuxai::conversation::AgentRequest {
    ready(
        engine
            .begin_turn(BeginTurn {
                agent,
                model_id: model_id.map(str::to_owned),
                prompt: prompt.to_owned(),
                display_text: None,
            })
            .unwrap(),
    )
}

fn fail_turn(
    engine: &mut ConversationEngine,
    request: &dmuxai::conversation::AgentRequest,
    message: &str,
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
            envelope(TurnEvent::Failed {
                message: message.to_owned(),
                invalidates_binding: true,
            }),
        ])
        .unwrap();
}

fn cancel_turn(engine: &mut ConversationEngine, request: &dmuxai::conversation::AgentRequest) {
    let envelope = |event| TurnOutput {
        conversation_id: request.conversation_id.clone(),
        turn_id: request.turn_id.clone(),
        agent: request.agent,
        event,
    };
    engine
        .record_turn_outputs(&[
            envelope(TurnEvent::Started),
            envelope(TurnEvent::ProviderSessionBound(format!(
                "{}-cancel",
                request.agent
            ))),
            envelope(TurnEvent::Cancelled),
        ])
        .unwrap();
}

// TC-2323 / FR-2323: another agent's completed turn keeps A's binding active,
// and A resumes its own native session instead of starting fresh.
#[test]
fn tc_2323_agent_resumes_own_session_after_other_agent_turn() {
    let harness = Harness::new();
    let mut engine = harness.open();
    let conversation_id = engine
        .select(ConversationSelection::New { title: None })
        .unwrap()
        .id;

    let codex = begin(
        &mut engine,
        AgentKey::Codex,
        Some("codex-model"),
        "codex turn",
    );
    complete_turn(&mut engine, &codex, "codex-thread", "codex answer");
    let cursor = begin(
        &mut engine,
        AgentKey::Cursor,
        Some("cursor-model"),
        "cursor turn",
    );
    complete_turn(&mut engine, &cursor, "cursor-session", "cursor answer");

    let codex_again = begin(&mut engine, AgentKey::Codex, None, "codex again");
    assert_eq!(codex_again.resume_session.as_deref(), Some("codex-thread"));
    let snapshot = engine
        .select(ConversationSelection::Existing(conversation_id))
        .unwrap();
    let binding = snapshot
        .bindings
        .iter()
        .find(|binding| binding.agent == AgentKey::Codex)
        .unwrap();
    assert_eq!(binding.status, BindingStatus::Active);
    assert_eq!(binding.known_through_seq, 1);
}

// TC-2324 / FR-2324: the resumed request carries only completed turns from
// other agents after known_through_seq, in order, with agent and model labels.
#[test]
fn tc_2324_delta_contains_only_unseen_active_task_turns() {
    let harness = Harness::new();
    let mut engine = harness.open();
    let conversation_id = engine
        .select(ConversationSelection::New { title: None })
        .unwrap()
        .id;

    let codex = begin(
        &mut engine,
        AgentKey::Codex,
        Some("codex-model"),
        "codex private",
    );
    complete_turn(&mut engine, &codex, "codex-thread", "codex answer");
    let cursor = begin(
        &mut engine,
        AgentKey::Cursor,
        Some("cursor-model"),
        "cursor turn",
    );
    complete_turn(&mut engine, &cursor, "cursor-session", "cursor answer");
    let agy = begin(&mut engine, AgentKey::Agy, Some("agy-model"), "agy turn");
    complete_turn(&mut engine, &agy, "agy-session", "agy answer");

    let failed = begin(&mut engine, AgentKey::Cursor, None, "cursor failed");
    fail_turn(&mut engine, &failed, "provider failed");
    let cancelled = begin(&mut engine, AgentKey::Cursor, None, "cursor cancelled");
    cancel_turn(&mut engine, &cancelled);
    let _queued = begin(&mut engine, AgentKey::Cursor, None, "cursor queued");

    let other = engine
        .select(ConversationSelection::New {
            title: Some("other task".to_owned()),
        })
        .unwrap();
    let other_turn = begin(&mut engine, AgentKey::Codex, None, "other task prompt");
    complete_turn(
        &mut engine,
        &other_turn,
        "other-thread",
        "other task answer",
    );
    assert_eq!(other.id, other_turn.conversation_id);

    engine
        .select(ConversationSelection::Existing(conversation_id))
        .unwrap();
    let codex_again = begin(&mut engine, AgentKey::Codex, None, "codex follow-up");
    assert_eq!(codex_again.resume_session.as_deref(), Some("codex-thread"));

    let delta = resume_delta(&codex_again);
    let seqs: Vec<i64> = delta.iter().map(|turn| turn.seq).collect();
    assert_eq!(seqs, vec![2, 3]);
    assert_eq!(delta[0].agent, AgentKey::Cursor);
    assert_eq!(delta[0].model_id.as_deref(), Some("cursor-model"));
    assert_eq!(delta[1].agent, AgentKey::Agy);
    assert_eq!(delta[1].model_id.as_deref(), Some("agy-model"));
    assert!(delta.iter().all(|turn| turn.user != "codex private"));
    assert!(delta.iter().all(|turn| turn.user != "other task prompt"));
    assert!(delta.iter().all(|turn| !turn.user.contains("queued")));
    assert!(delta.iter().all(|turn| !turn.user.contains("failed")));
    assert!(delta.iter().all(|turn| !turn.user.contains("cancelled")));
}

// TC-2325 / FR-2325: oversized deltas, stale bindings and never-run agents
// fall back to the existing bounded fresh projection with no resume id.
#[test]
fn tc_2325_delta_bounds_and_resume_fallbacks() {
    let harness = Harness::new();
    let mut engine = harness.open();

    // More than 200 unseen turns.
    engine
        .select(ConversationSelection::New {
            title: Some("too many turns".to_owned()),
        })
        .unwrap();
    let codex = begin(&mut engine, AgentKey::Codex, None, "bound codex");
    complete_turn(&mut engine, &codex, "codex-thread-many", "first");
    for index in 0..=MAX_UNSEEN_DELTA_TURNS {
        let cursor = begin(
            &mut engine,
            AgentKey::Cursor,
            None,
            &format!("cursor-{index}"),
        );
        complete_turn(&mut engine, &cursor, "cursor-session-many", "ok");
    }
    let too_many = begin(&mut engine, AgentKey::Codex, None, "after 201");
    assert!(too_many.resume_session.is_none());
    assert!(matches!(too_many.context, RequestContext::Fresh(_)));

    // More than 65,536 UTF-8 bytes of text, but under the character budget.
    engine
        .select(ConversationSelection::New {
            title: Some("too many bytes".to_owned()),
        })
        .unwrap();
    let codex = begin(&mut engine, AgentKey::Codex, None, "bound codex");
    complete_turn(&mut engine, &codex, "codex-thread-bytes", "first");
    let cursor = begin(&mut engine, AgentKey::Cursor, None, "huge cursor");
    let huge = "가".repeat(21_846);
    assert!(huge.len() > MAX_UNSEEN_DELTA_BYTES);
    complete_turn(&mut engine, &cursor, "cursor-session-bytes", &huge);
    let too_many_bytes = begin(&mut engine, AgentKey::Codex, None, "after huge");
    assert!(too_many_bytes.resume_session.is_none());
    assert!(matches!(too_many_bytes.context, RequestContext::Fresh(_)));

    // A retired binding cannot resume.
    engine
        .select(ConversationSelection::New {
            title: Some("retired binding".to_owned()),
        })
        .unwrap();
    let codex = begin(&mut engine, AgentKey::Codex, None, "bound codex");
    complete_turn(&mut engine, &codex, "codex-thread-retired", "first");
    assert!(engine.retire_active_binding(AgentKey::Codex).unwrap());
    let cursor = begin(&mut engine, AgentKey::Cursor, None, "cursor after retire");
    complete_turn(
        &mut engine,
        &cursor,
        "cursor-session-retired",
        "cursor answer",
    );
    let retired = begin(&mut engine, AgentKey::Codex, None, "after retire");
    assert!(retired.resume_session.is_none());
    assert!(matches!(retired.context, RequestContext::Fresh(_)));

    // An agent that never ran in the task gets the summary/recent projection.
    engine
        .select(ConversationSelection::New {
            title: Some("never ran".to_owned()),
        })
        .unwrap();
    let codex = begin(&mut engine, AgentKey::Codex, None, "only codex");
    complete_turn(&mut engine, &codex, "codex-thread-never", "codex answer");
    let agy = begin(&mut engine, AgentKey::Agy, None, "agy first");
    assert!(agy.resume_session.is_none());
    assert!(!fresh_projection(&agy).recent.is_empty());
}

// TC-2326 / FR-2326: known_through_seq advances only for a saved Completed
// turn; failed, cancelled and interrupted attempts re-deliver the same delta.
#[test]
fn tc_2326_delta_is_consumed_only_on_completed() {
    let harness = Harness::new();
    let mut engine = harness.open();
    let conversation_id = engine
        .select(ConversationSelection::New { title: None })
        .unwrap()
        .id;

    let codex = begin(&mut engine, AgentKey::Codex, None, "codex first");
    complete_turn(&mut engine, &codex, "codex-thread-1", "codex answer");
    let cursor = begin(&mut engine, AgentKey::Cursor, None, "cursor first");
    complete_turn(&mut engine, &cursor, "cursor-session", "cursor answer");

    let completed = begin(&mut engine, AgentKey::Codex, None, "consume delta");
    assert_eq!(resume_delta(&completed).len(), 1);
    complete_turn(&mut engine, &completed, "codex-thread-1", "consumed");
    let after_completed = engine
        .select(ConversationSelection::Existing(conversation_id.clone()))
        .unwrap();
    let binding = after_completed
        .bindings
        .iter()
        .find(|binding| binding.agent == AgentKey::Codex)
        .unwrap();
    assert_eq!(binding.status, BindingStatus::Active);
    assert_eq!(binding.known_through_seq, 3);

    // Failed attempt leaves the binding's known seq unchanged.
    let cursor = begin(&mut engine, AgentKey::Cursor, None, "cursor failed case");
    complete_turn(&mut engine, &cursor, "cursor-session", "cursor answer");
    let failed = begin(&mut engine, AgentKey::Codex, None, "will fail");
    assert_eq!(resume_delta(&failed).len(), 1);
    fail_turn(&mut engine, &failed, "provider failed");
    let after_failed = engine
        .select(ConversationSelection::Existing(conversation_id.clone()))
        .unwrap();
    let binding = after_failed
        .bindings
        .iter()
        .find(|binding| binding.agent == AgentKey::Codex)
        .unwrap();
    assert_eq!(binding.status, BindingStatus::Stale);
    assert_eq!(binding.known_through_seq, 3);
    let redelivered = begin(&mut engine, AgentKey::Codex, None, "after failure");
    assert!(redelivered.resume_session.is_none());
    assert!(
        fresh_projection(&redelivered)
            .recent
            .iter()
            .any(|turn| turn.user == "cursor failed case")
    );
    complete_turn(&mut engine, &redelivered, "codex-thread-2", "recovered");

    // Cancelled attempt leaves the known seq unchanged.
    let cursor = begin(&mut engine, AgentKey::Cursor, None, "cursor cancelled case");
    complete_turn(&mut engine, &cursor, "cursor-session", "cursor answer");
    let cancelled = begin(&mut engine, AgentKey::Codex, None, "will cancel");
    assert_eq!(resume_delta(&cancelled).len(), 1);
    cancel_turn(&mut engine, &cancelled);
    let after_cancelled = engine
        .select(ConversationSelection::Existing(conversation_id.clone()))
        .unwrap();
    let binding = after_cancelled
        .bindings
        .iter()
        .find(|binding| binding.agent == AgentKey::Codex)
        .unwrap();
    assert_eq!(binding.status, BindingStatus::Stale);
    assert_eq!(binding.known_through_seq, 6);
    let redelivered = begin(&mut engine, AgentKey::Codex, None, "after cancel");
    assert!(
        fresh_projection(&redelivered)
            .recent
            .iter()
            .any(|turn| turn.user == "cursor cancelled case")
    );
    complete_turn(
        &mut engine,
        &redelivered,
        "codex-thread-3",
        "recovered again",
    );

    // Interrupted attempt after a restart leaves the known seq unchanged.
    let cursor = begin(
        &mut engine,
        AgentKey::Cursor,
        None,
        "cursor interrupted case",
    );
    complete_turn(&mut engine, &cursor, "cursor-session", "cursor answer");
    let interrupted = begin(&mut engine, AgentKey::Codex, None, "will interrupt");
    assert_eq!(resume_delta(&interrupted).len(), 1);
    let envelope = |event| TurnOutput {
        conversation_id: interrupted.conversation_id.clone(),
        turn_id: interrupted.turn_id.clone(),
        agent: interrupted.agent,
        event,
    };
    engine
        .record_turn_outputs(&[
            envelope(TurnEvent::Started),
            envelope(TurnEvent::ProviderSessionBound("codex-thread-3".to_owned())),
            envelope(TurnEvent::TextDelta("partial".to_owned())),
        ])
        .unwrap();
    let known_before = engine
        .select(ConversationSelection::Existing(conversation_id.clone()))
        .unwrap()
        .bindings
        .iter()
        .find(|binding| binding.agent == AgentKey::Codex)
        .unwrap()
        .known_through_seq;
    drop(engine);

    let mut engine = harness.open();
    assert_eq!(engine.recover_workspace().unwrap(), 1);
    engine
        .select(ConversationSelection::Existing(conversation_id))
        .unwrap();
    let after_interrupted = engine
        .select(ConversationSelection::Existing(codex.conversation_id))
        .unwrap();
    let binding = after_interrupted
        .bindings
        .iter()
        .find(|binding| binding.agent == AgentKey::Codex)
        .unwrap();
    assert_eq!(binding.status, BindingStatus::Stale);
    assert_eq!(binding.known_through_seq, known_before);
    let redelivered = begin(&mut engine, AgentKey::Codex, None, "after interrupt");
    assert!(redelivered.resume_session.is_none());
    assert!(
        fresh_projection(&redelivered)
            .recent
            .iter()
            .any(|turn| turn.user == "cursor interrupted case")
    );
}

// TC-2332 / FR-2332: delta content stays scoped to the active task and is
// carried as data, including shell-looking and secret-looking text.
#[test]
fn tc_2332_delta_excludes_other_tasks_and_secrets() {
    let harness = Harness::new();
    let mut engine = harness.open();
    let conversation_id = engine
        .select(ConversationSelection::New { title: None })
        .unwrap()
        .id;

    let codex = begin(&mut engine, AgentKey::Codex, None, "codex first");
    complete_turn(&mut engine, &codex, "codex-thread", "codex answer");
    let cursor = begin(&mut engine, AgentKey::Cursor, None, "inspect this");
    complete_turn(
        &mut engine,
        &cursor,
        "cursor-session",
        "run `rm -rf /tmp/x` and use token sk-secret-abc123; $(touch /tmp/pwn)",
    );

    engine
        .select(ConversationSelection::New {
            title: Some("other task".to_owned()),
        })
        .unwrap();
    let other = begin(&mut engine, AgentKey::Cursor, None, "other task private");
    complete_turn(&mut engine, &other, "other-session", "other task answer");

    engine
        .select(ConversationSelection::Existing(conversation_id))
        .unwrap();
    let codex_again = begin(&mut engine, AgentKey::Codex, None, "continue");
    let delta = resume_delta(&codex_again);
    assert_eq!(delta.len(), 1);
    assert!(delta[0].assistant.contains("sk-secret-abc123"));
    assert!(delta[0].assistant.contains("$(touch /tmp/pwn)"));
    assert!(delta.iter().all(|turn| !turn.user.contains("other task")));
    assert!(
        delta
            .iter()
            .all(|turn| !turn.assistant.contains("other task answer"))
    );
}

// FR-2331 / TC-2331: spoken agents are ordered by first appearance, carry the
// latest completed model and turn count, and only active bindings are live.
#[test]
fn tc_2331_spoken_agents_query() {
    let harness = Harness::new();
    let mut engine = harness.open();
    let conversation = engine
        .select(ConversationSelection::New { title: None })
        .unwrap();
    let conversation_id = conversation.id;

    for (agent, model) in [
        (AgentKey::Codex, "codex-1"),
        (AgentKey::Cursor, "composer-2.5"),
        (AgentKey::Agy, "agy-3"),
        (AgentKey::Cursor, "composer-2.6"),
    ] {
        let request = ready(
            engine
                .begin_turn(BeginTurn {
                    agent,
                    model_id: Some(model.to_owned()),
                    prompt: format!("prompt from {agent}"),
                    display_text: None,
                })
                .unwrap(),
        );
        complete_turn(&mut engine, &request, &format!("session-{model}"), "done");
    }
    assert!(engine.retire_active_binding(AgentKey::Agy).unwrap());

    let spoken = engine.spoken_agents().unwrap();
    assert_eq!(
        spoken.iter().map(|agent| agent.agent).collect::<Vec<_>>(),
        [AgentKey::Codex, AgentKey::Cursor, AgentKey::Agy]
    );
    assert_eq!(spoken[0].model_id.as_deref(), Some("codex-1"));
    assert_eq!(spoken[0].turns, 1);
    assert!(spoken[0].live);
    assert_eq!(spoken[1].model_id.as_deref(), Some("composer-2.6"));
    assert_eq!(spoken[1].turns, 2);
    assert!(spoken[1].live);
    assert_eq!(spoken[2].model_id.as_deref(), Some("agy-3"));
    assert!(!spoken[2].live);

    let cards = engine.list().unwrap();
    let card = cards
        .iter()
        .find(|card| card.id == conversation_id)
        .unwrap();
    let card_agents: Vec<_> = card.spoken_agents.to_vec();
    assert_eq!(
        card_agents,
        [AgentKey::Codex, AgentKey::Cursor, AgentKey::Agy]
    );

    let other = engine
        .select(ConversationSelection::New { title: None })
        .unwrap();
    assert_ne!(other.id, conversation_id);
    assert!(engine.spoken_agents().unwrap().is_empty());
}

// TC-303 / FR-302, FR-300: Turn outputs can complete without writes or fail with failure reason persisted.
#[test]
fn tc_303_turn_completion_and_failure_reason_persisted() {
    let harness = Harness::new();
    let mut engine = harness.open();
    let snapshot = engine
        .select(ConversationSelection::New { title: None })
        .unwrap();

    // 1. A turn that completes normally (read-only/plan style, no writes needed).
    let req1 = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Cursor,
                model_id: None,
                prompt: "plan the migration".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    complete_turn(&mut engine, &req1, "cursor-sess-1", "Plan details here");

    // 2. An implement turn that fails due to permission denial / write error.
    let req2 = ready(
        engine
            .begin_turn(BeginTurn {
                agent: AgentKey::Cursor,
                model_id: None,
                prompt: "implement the changes".to_owned(),
                display_text: None,
            })
            .unwrap(),
    );
    let fail_output = TurnOutput {
        conversation_id: req2.conversation_id.clone(),
        turn_id: req2.turn_id.clone(),
        agent: req2.agent,
        event: TurnEvent::Failed {
            message: "permission denied: sandbox write blocked".to_owned(),
            invalidates_binding: true,
        },
    };
    engine.record_turn_outputs(&[fail_output]).unwrap();

    let reopened = engine
        .select(ConversationSelection::Existing(snapshot.id))
        .unwrap();
    assert_eq!(reopened.turns.len(), 2);
    assert_eq!(reopened.turns[0].status, TurnStatus::Completed);
    assert_eq!(reopened.turns[1].status, TurnStatus::Failed);
    assert!(
        reopened.turns[1]
            .error_text
            .as_deref()
            .unwrap_or_default()
            .contains("permission denied: sandbox write blocked"),
        "expected failure reason persisted on the turn"
    );
}
