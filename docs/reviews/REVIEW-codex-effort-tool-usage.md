# REVIEW: Codex effort, tool progress, and usage status

Status: complete
Spec: [SPEC-codex-effort-tool-usage.md](../specs/SPEC-codex-effort-tool-usage.md)

## Spec compliance

- FR-040 / BR-040: `/effort` is Codex-only, idle-only, populated from the
  selected model's advertised effort list, and emitted on the next `turn/start`.
- FR-041 / BR-041: recognized Codex item start/completion notifications become
  concise transient tool messages only after existing active-turn ownership
  validation. Unknown, malformed, delayed, and mismatched items are ignored.
- FR-042: `/status` reports Codex model/effort with provider usage and
  rate-limit data. Authentication or unsupported-field failures render as
  unavailable status text without starting a conversation turn.

## Regression and safety review

- `approvalPolicy: never` and Ask/Plan `readOnly` behavior remain asserted by
  `codex_turn_request_includes_selected_effort` and existing mode tests.
- No SQLite migration was added. Tool and account data use transient notices;
  the canonical transcript never stores arguments, outputs, or account data.
- Tool summaries are capped at 240 Unicode scalar values and only recognized
  action metadata is rendered. Per-agent active-turn ownership remains the
  gate before rendering.
- Cursor and AGY receive no new commands; their adapters use the provider
  interface's unsupported default for effort/status.

## Test evidence

- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/SPEC-codex-effort-tool-usage.md`: passed.
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/DMUXAI-BASELINE.md`: passed.
- `C:\Users\tlsfmswls\.cargo\bin\cargo.exe fmt --check`: passed.
- `C:\Users\tlsfmswls\.cargo\bin\cargo.exe test --all-targets`: passed: 96 tests; 5 authenticated-provider smoke tests ignored as intended.
- `C:\Users\tlsfmswls\.cargo\bin\cargo.exe clippy --all-targets --all-features -- -D warnings`: passed.
- Live Codex smoke tests: skipped because the task does not require live account use;
  existing tests remain ignored and require a locally authenticated CLI.

## Residual risks and rollback

Codex account status and model metadata are authentication and CLI-version
dependent. The UI labels unavailable data rather than inferring it. Rollback is
code-only; no schema migration or persistent provider setting is introduced.
