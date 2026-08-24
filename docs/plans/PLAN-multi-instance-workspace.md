# PLAN: Multiple DmuxAI instances in one workspace

Status: complete  
Spec: [SPEC-multi-instance-workspace.md](../specs/SPEC-multi-instance-workspace.md)

1. Extend the existing transactional SQLite schema additively with runtime
   sessions and nullable turn ownership, keeping the current user-version
   compatibility and adding deterministic session lifecycle methods. (BR-050,
   NFR-050, TC-052, TC-056)
2. Register one UUID session per `ConversationEngine`, implement heartbeat,
   clean-close, live-peer detection, and ownership-aware recovery. A later
   startup recovers only turns whose owner is missing, closed, or lease-expired.
   (FR-051, BR-050, TC-051, TC-052)
3. Change application startup selection so a live peer gets a fresh active
   conversation while a workspace without a live peer still restores the latest
   conversation. Keep the shared conversation picker and provider boundaries.
   (FR-050, FR-052, TC-050, TC-053)
4. Make the per-conversation/agent busy check part of the same immediate
   transaction that inserts a turn, so separate SQLite connections cannot race
   into duplicate active turns. (BR-051, TC-054, TC-055)
5. Add deterministic application/persistence tests, update the README and
   baseline traceability, run the spec check and Rust gates, and record review
   evidence. (all requirements, TC-050..TC-056)

## Risks and mitigations

- A crashed process leaves a session row active until the lease expires. Keep
  the lease bounded and refresh it from the non-blocking UI loop; test explicit
  expiry without waiting in real time.
- A peer that intentionally selects the same conversation still shares its
  canonical transcript and must obey the existing per-agent busy rule. Keep
  startup isolation as the default and preserve `/conversations` for explicit
  selection.
- Additive schema setup must not leave a half-created table/column. Perform the
  extension in one SQLite transaction and test an existing v1 database.
- A provider output arriving after its owner is recovered must be rejected by
  existing terminal-state correlation; the provider protocol itself is not
  changed.

## Rollback

The schema extension is additive and keeps the existing SQLite user-version, so
an older binary can ignore `runtime_sessions` and `owner_session_id`. Reverting
the code and docs restores the former workspace-wide recovery behavior; no
destructive database downgrade is required. A failed extension transaction is
rolled back by SQLite.
