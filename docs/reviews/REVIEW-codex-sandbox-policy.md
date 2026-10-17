# REVIEW: Codex sandbox-policy object compatibility

Status: complete
Spec: [SPEC-codex-sandbox-policy.md](../specs/SPEC-codex-sandbox-policy.md)

## Spec compliance

- INT-040: Codex `turn/start.sandboxPolicy` is now a tagged object. Ask and
  Plan send `{ "type": "readOnly" }`; Implement sends
  `{ "type": "workspaceWrite" }`.
- NFR-041: `approvalPolicy: "never"`, selected model, selected effort, and the
  existing Plan prompt continue to be asserted by request-construction tests.

## Regression and safety review

- The original failure was reproduced before the fix: the regression test
  expected `/params/sandboxPolicy/type`, received `None`, and failed.
- The fix changes only the value shape required by the current Codex App Server
  schema; no SQLite data, process launching, approval setting, or other-agent
  adapter behavior changed.
- `thread/start.sandbox` remains unchanged because the reported App Server
  error identifies the `turn/start.sandboxPolicy` legacy string (`readOnly`).

## Test evidence

- Red reproduction: `%USERPROFILE%\.cargo\bin\cargo.exe test codex_execution_modes_set_per_turn_sandbox_and_approval_policy` failed before the fix with `left: None`, `right: Some(String("readOnly"))` for `/params/sandboxPolicy/type`.
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/SPEC-codex-sandbox-policy.md`: passed.
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/DMUXAI-BASELINE.md`: passed.
- `%USERPROFILE%\.cargo\bin\cargo.exe fmt --check`: passed.
- `%USERPROFILE%\.cargo\bin\cargo.exe test --all-targets`: passed; 96 tests passed and 5 authenticated provider smoke tests were ignored as intended.
- `%USERPROFILE%\.cargo\bin\cargo.exe clippy --all-targets --all-features -- -D warnings`: passed.
- Live Codex smoke test: skipped; it needs a locally authenticated Codex CLI and would make a real provider request. The deterministic regression test covers the malformed request shape.

## Residual risks and rollback

Codex App Server schemas can evolve. The request builder is now covered by a
deterministic tagged-object regression test. Rollback is a code-only revert;
there is no persistent migration.
