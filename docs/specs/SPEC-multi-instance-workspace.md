# SPEC: Multiple DmuxAI instances in one workspace

Status: complete  
Owner: DmuxAI  
Created: 2026-08-24  
Baseline: `675d9df`  
Related: baseline FR-001, FR-012, FR-015, BR-001, BR-010, BR-011

## 1. Decision summary

Allow more than one DmuxAI process to run at the same time from the same
canonical workspace. Each process receives its own runtime session and, when
another live session already exists, starts in a fresh DmuxAI conversation so
its UI and provider actors are independent. The workspace's durable
conversation list remains shared, while live turns are owned by the session
that started them and are not recovered by a peer that is still alive.

## 2. Scope

### In scope

- Concurrent startup of multiple DmuxAI processes from one canonical workspace.
- A per-process runtime session with liveness heartbeats and clean shutdown.
- A fresh active conversation for a new peer instance, with existing
  conversations still available through the conversation picker.
- Ownership-aware startup recovery and an atomic per-conversation/agent busy
  check across SQLite connections.
- Additive, transactional persistence changes that remain readable by the
  previous schema version.

### Out of scope / non-goals

- Splitting the durable SQLite database into one database per window.
- Sharing one provider child process or native provider session between windows.
- Allowing two active turns for the same agent in the same DmuxAI conversation.
- Creating separate filesystem workspaces, credentials, sandboxes, or provider
  permission policies.
- Adding a terminal multiplexer, window manager, or new command-line flags.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-001 | “Independent window” means independent runtime and default active conversation; completed conversations remain visible in the shared workspace list to preserve existing persistence and resume behavior. | DmuxAI | Execute |

No open questions block implementation.

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Runtime session | A UUID-backed lifetime of one DmuxAI process, registered for one canonical workspace. | A provider-native thread/session or a DmuxAI conversation. |
| Live peer | A different runtime session for the same workspace whose status is active and whose heartbeat has not expired. | A stale database row left by a crashed process. |
| Instance conversation | The conversation selected by a process at startup. A peer process receives a new one; it is still a normal durable DmuxAI conversation. | A private database or an unshareable transcript. |
| Owned turn | A queued or running turn carrying the runtime session ID that created it. | A completed turn, which remains workspace-visible. |

Runtime sessions transition as follows:

| State | Allowed transition | Trigger |
| --- | --- | --- |
| active | active -> closed | Normal application shutdown. |
| active | active -> closed | Lease expiry detected by a later startup. |
| closed | terminal | Historical ownership remains available for recovery/audit. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-050 | Must | Given one DmuxAI process is live in a canonical workspace, when a second process starts from that same workspace. | The second process starts successfully, creates/selects a different fresh conversation, and remains independently usable; the first process remains running. | If session registration or the database cannot be opened, startup returns the existing actionable persistence error and does not claim a usable session. | TC-050, TC-051 |
| FR-051 | Must | Given a live peer owns a queued or running turn, when another DmuxAI process starts in the same workspace. | Startup recovery leaves the peer's turn and provider binding untouched; the owning process can continue receiving and persisting output. | A turn from a closed or expired session is recovered as interrupted using the existing recovery behavior. | TC-051, TC-052 |
| FR-052 | Must | Given no live peer exists, when DmuxAI starts in a workspace with persisted conversations. | It retains baseline behavior and restores the latest unarchived conversation; when a live peer exists, the new process starts a fresh conversation instead. | Existing conversations remain selectable from `/conversations`/`F3`; startup does not delete or archive them. | TC-050, TC-053 |
| BR-050 | Must | Given multiple processes use the same SQLite database. | Each new turn stores its runtime-session owner, session heartbeats are refreshed while the UI runs, and normal shutdown marks the session closed. | A missing/closed/expired owner is treated as recoverable; a live owner is never interrupted by peer startup. | TC-051, TC-052 |
| BR-051 | Must | Given two processes select the same conversation and agent. | At most one queued or running turn is committed for that `(conversation, agent)` pair, even when the processes use separate SQLite connections. | The losing request returns `AgentBusy`, preserves its prompt at the UI boundary, and does not create a second durable turn. Different instance conversations may run the same agent concurrently. | TC-054, TC-055 |
| NFR-050 | Must | During startup against a new or existing conversation database. | The session/ownership storage is installed transactionally and additively; existing conversations, messages, bindings, and checkpoints remain readable, and provider request/event protocols are unchanged. | A failed schema extension rolls back its own changes; the previous binary can still open the additive schema, though it retains its original single-instance recovery semantics. | TC-056 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-050 | FR-050, FR-052 | application integration | A temporary database has no live session, then one app runtime is opened and kept alive. | Open a second app runtime for the same workspace. | Both runtimes initialize; their active conversation IDs differ; the first conversation is still present in the shared list. | `cargo test multiple_live_instances_start_in_separate_conversations` |
| TC-051 | FR-050, FR-051, BR-050 | application integration | The first runtime has a queued/running turn and an active session heartbeat. | Start the second runtime, then deliver completion output through the first runtime. | The second startup does not interrupt/rewrite the first turn; the first turn completes and its transcript remains durable. | `cargo test multiple_live_instances_start_in_separate_conversations` |
| TC-052 | FR-051, BR-050 | persistence integration | A turn is owned by a session that is explicitly closed; repeat with an expired active session row. | Open a new engine and run startup recovery. | Both orphaned turns become interrupted, streaming text remains visible, and bindings become stale. | `cargo test closed_or_expired_session_turns_are_recovered` |
| TC-053 | FR-052 | application integration | A prior app runtime is dropped cleanly and its database contains a completed conversation. | Open a new app runtime with no live peer. | The latest persisted conversation is restored, preserving baseline restart behavior. | `cargo test a_new_app_instance_loads_the_latest_conversation` |
| TC-054 | BR-051 | persistence integration | Two engines use separate connections, select the same conversation, and engine A owns a Codex turn. | Engine B attempts a Codex turn in that same conversation. | Engine B gets `ConversationError::AgentBusy`; no second turn row is inserted. | `cargo test busy_agent_is_rejected_across_runtime_sessions` |
| TC-055 | BR-051 | boundary/concurrency | Two independent instance conversations are active in one workspace. | Both engines begin a turn for the same agent. | Both requests are accepted because their conversation ownership boundaries differ. | `cargo test same_agent_can_run_in_separate_instance_conversations` |
| TC-056 | NFR-050 | persistence migration | A v1 database without runtime-session objects and a fresh database. | Open each with the new engine, then inspect schema/data and reopen with the old-shaped tables. | The additive schema is installed atomically, old durable rows remain readable, and no provider protocol fields change. | `cargo test runtime_session_schema_extension_preserves_existing_data`; `scripts/check-spec.ps1` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | Add `runtime_sessions` plus nullable `turns.owner_session_id`; use a transactional additive extension and keep the existing SQLite user-version contract. | Existing rows with no owner are recoverable on next startup. The old binary can ignore the extra table/column; reverting code restores the old recovery behavior without requiring a destructive downgrade. | TC-052, TC-056 |
| Provider protocol | No Codex, Cursor, or AGY request/event shape changes; each runtime continues to own its own adapter and child process. | Provider smoke tests remain ignored unless installed/authenticated; code rollback is provider-neutral. | TC-051 |
| Security / permissions | Multiple instances do not expand sandbox, approval, credential, or workspace permissions. | Existing Ask/Plan/Implement safety mapping is unchanged. | TC-051 |
| Platform | Session identity and recovery use the existing SQLite/WAL path and work on Windows and Linux; no OS-specific singleton lock is introduced. | If a process crashes, lease expiry is the bounded fallback; normal shutdown closes immediately. | TC-052; Rust gates |

## 7. Implementation plan

See [PLAN-multi-instance-workspace.md](../plans/PLAN-multi-instance-workspace.md) and
[TASKS-multi-instance-workspace.md](../tasks/TASKS-multi-instance-workspace.md).

## 8. Review and test exit criteria

- [x] All Must requirements map to passing evidence.
- [x] The diff is reviewed against the multiple-instance scope and non-goals.
- [x] The spec traceability check and required Rust format, test, and clippy
  gates pass.
- [x] External-provider smoke tests are marked skipped with a reason unless
  explicitly run with authenticated CLIs.
- [x] [REVIEW-multi-instance-workspace.md](../reviews/REVIEW-multi-instance-workspace.md)
  records commands, results, residual risk, and rollback notes.
