# PLAN: Automatic cross-agent conversation sharing

Status: complete  
Spec: `../specs/SPEC-automatic-cross-agent-sharing.md`
Baseline: `v0.1.0` / `ca66284`

## Goal

Make the durable common transcript available to every provider on its next
request while retaining provider-specific native session ownership.

## Ordered changes

1. Extend `ProjectedTurn` with its source `AgentKey`; load all completed turns
   for the active conversation, ordered by `seq`, and include the source agent
   in the source hash. Keep checkpoint validation and character budgets
   transactional and schema-compatible. (FR-025, FR-026, FR-028, NFR-005)
2. Make `begin_turn`, automatic summary preparation/commit, and active-binding
   validation use the common projection. A binding resumes only when its
   known sequence covers the common head. (FR-027, FR-028)
3. Update the restored-context envelope to say that it is shared history and
   label each recent turn by source agent. Preserve `/share <agent>` as a
   compatibility command, but document it as optional. (UX-015)
4. Add mixed-agent, ordering, isolation, and app-level no-`/share` tests;
   update baseline and README wording; run the spec traceability script.

## Risks and mitigations

- Cross-agent projections can reach the existing context budget sooner. The
  current summary job and 32,000/20,000/12,000 character limits remain the
  single enforcement point; long mixed history defers once and restores input
  on failure.
- A provider native session cannot know a turn added by another provider. The
  common projection head is included in the binding validity check, forcing a
  fresh structured request when the head advances.
- Existing serialized checkpoints were produced from same-agent source sets.
  Source hashes are recomputed over the new common set; mismatches are ignored,
  so old checkpoints cannot silently omit another agent's history.
- The explicit `/share` path can still create a synthetic import turn. It is
  retained for compatibility and is not used by automatic switching; repeated
  imports continue to be filtered as before.

## Rollback

Revert the code and documentation commit. No migration or destructive data
operation is introduced; canonical turns and existing SQLite rows remain
available to the previous projection implementation.

## Verification order

1. `cargo test --lib` and `cargo test --test conversation`.
2. `cargo fmt --check`.
3. `cargo test --all-targets`.
4. `cargo clippy --all-targets --all-features -- -D warnings`.
5. `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/SPEC-automatic-cross-agent-sharing.md`.
6. `git diff --check`.
