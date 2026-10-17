# REVIEW: Multiple DmuxAI instances in one workspace

Status: complete  
Spec: [SPEC-multi-instance-workspace.md](../specs/SPEC-multi-instance-workspace.md)  
Plan: [PLAN-multi-instance-workspace.md](../plans/PLAN-multi-instance-workspace.md)  
Tasks: [TASKS-multi-instance-workspace.md](../tasks/TASKS-multi-instance-workspace.md)  
Reviewed: 2026-08-24

## 1. Result

Each `ConversationEngine` registers a UUID-backed runtime session, refreshes
its lease from the TUI loop, and closes the session on normal shutdown. New
turns persist their owning session. Startup recovery interrupts only turns
owned by missing, closed, or expired sessions. A live peer in the same
canonical workspace causes a new app instance to select a fresh conversation;
without a peer, the latest conversation restore behavior remains unchanged.

The same-conversation/agent busy check runs inside the immediate write
transaction, so separate SQLite connections cannot create duplicate active
turns. The runtime-session schema is additive and preserves the existing
SQLite user-version contract.

## 2. Requirement review

| Requirement | Evidence | Result |
| --- | --- | --- |
| FR-050 | Live-peer startup selection and `multiple_live_instances_start_in_separate_conversations` | Met |
| FR-051 | Owner-aware recovery, live-peer preservation, and `closed_or_expired_session_turns_are_recovered` | Met |
| FR-052 | Session registration, `touch_session`, clean close, and `a_new_app_instance_loads_the_latest_conversation` | Met |
| BR-050 | Persisted owner IDs, lease expiry, heartbeat path, and recovery tests | Met |
| BR-051 | Transactional busy check and separate-conversation same-agent test | Met |
| NFR-050 | Additive schema extension and preservation test; provider request/event shapes unchanged | Met |

The implementation does not add provider permission, credential, workspace
filesystem, or provider-session sharing behavior.

## 3. Test evidence

Commands were run from `C:\path\to\DmuxAI` with the bundled
Rust toolchain path because Cargo is not on the PowerShell `PATH`.

| Command | Result |
| --- | --- |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/SPEC-multi-instance-workspace.md` | Pass |
| `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/DMUXAI-BASELINE.md` | Pass |
| `%USERPROFILE%\.cargo\bin\cargo.exe fmt --check` | Pass |
| `%USERPROFILE%\.cargo\bin\cargo.exe test --all-targets --target-dir target\\release-verify` | Pass: 8 library, 93 binary, and 31 integration tests; 5 live-provider tests ignored |
| `%USERPROFILE%\.cargo\bin\cargo.exe clippy --all-targets --all-features --target-dir target\\release-verify -- -D warnings` | Pass |

The full all-target run includes the acceptance tests for live peer startup,
latest-conversation restore, closed/expired recovery, cross-session busy
rejection, separate-conversation concurrency, and additive schema extension.
Live Codex, Cursor, and AGY smoke tests were not run because the provider CLIs
and credentials were not established.

## 4. Regression and safety review

- Session ownership is scoped to the canonical workspace and does not create a
  process-wide lock; multiple windows remain independently usable.
- The 30-second lease is refreshed every five seconds by the UI loop. A crashed
  process is recoverable after lease expiry; normal shutdown closes immediately.
- Recovery is transactional and preserves partial streaming text while marking
  the orphaned turn and binding appropriately.
- The busy invariant remains conversation- and agent-scoped, so two instance
  conversations can run the same agent concurrently.
- Provider protocols and safety/mode mappings are unchanged.

## 5. Rollback and residual risk

Reverting the runtime-session code and associated docs/tests restores the old
workspace-wide recovery behavior. The additive schema objects can remain in a
database because the user-version is unchanged; no destructive downgrade is
required.

Residual risk is the intentional lease tradeoff: a suspended process may be
treated as expired after 30 seconds. An interactive two-terminal smoke test was
not run; deterministic application and persistence tests cover the ownership
and recovery behavior.
