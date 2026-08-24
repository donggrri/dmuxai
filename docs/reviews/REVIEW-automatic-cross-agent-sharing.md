# REVIEW: Automatic cross-agent conversation sharing

Status: complete  
Spec: `../specs/SPEC-automatic-cross-agent-sharing.md`
Plan: `../plans/PLAN-automatic-cross-agent-sharing.md`
Review date: 2026-08-24

## Spec compliance

- FR-025/FR-026: `ConversationEngine` now projects every completed turn in the
  active conversation, retains the source `AgentKey`, and the TUI renders the
  source label in the shared-context envelope.
- FR-027: the existing `known_through_seq` check now compares provider bindings
  with the common projection head, so a turn from another agent prevents an
  incorrect native resume.
- FR-028: existing summary/checkpoint logic now hashes the common projected
  turn set. The canonical transcript and existing character limits remain
  unchanged; synthetic explicit-import turns are filtered to prevent recursive
  payloads.
- UX-015: switching via `/agent` or the picker requires no `/share`; the legacy
  explicit command remains available.
- NFR-005: projection queries remain scoped by active conversation and the
  engine workspace; no schema or cross-workspace behavior changed.

## Automated evidence

Commands run from `C:\Users\tlsfmswls\Desktop\DmuxAI`:

- `cargo fmt --check` — passed.
- `cargo test --test conversation` — 15 passed, 0 failed.
- `cargo test --bin dmuxai switching_agents_automatically_shares_the_completed_context` — passed.
- `cargo test --all-targets` — 7 library tests passed; 68 binary tests passed,
  5 provider-authentication tests ignored; 15 conversation integration tests
  passed.
- `cargo clippy --all-targets --all-features -- -D warnings` — passed.
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/SPEC-automatic-cross-agent-sharing.md` — passed.
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/DMUXAI-BASELINE.md` — passed.
- `git diff --check` — passed (only existing Windows line-ending warnings).

Live provider smoke tests were not run; the deterministic suite keeps those
tests ignored because authenticated Codex, Cursor, and AGY CLIs are not a
required local dependency.

## Regression and residual risks

- Provider processes are not proactively sent another agent's conversation;
  sharing occurs on the next user request. This is intentional to avoid
  unsolicited work and provider queue contention.
- Common history reaches the existing projection budget sooner than a
  same-agent projection, so mixed-agent work may trigger summaries earlier.
- `/share <agent>` still creates its explicit compatibility turn; automatic
  projection excludes that synthetic payload, and the existing repeated-share
  regression test passes.

## Rollback

Revert the code/docs change. No SQLite migration or destructive data operation
was introduced, so canonical transcripts and existing checkpoints remain
recoverable.
