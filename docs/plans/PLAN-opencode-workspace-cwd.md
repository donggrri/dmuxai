# PLAN: opencode workspace cwd

Status: in_progress
Requirement: FR-2450
Tests: TC-2450, TC-2451

## Ordered changes

1. Add `apply_child_cwd` and `apply_child_cwd_tokio` in `src/process/mod.rs`.
   Each sets `Command::current_dir(cwd)` and `env("PWD", cwd)` so the kernel cwd
   and conventional environment value cannot diverge. Evidence: TC-2451.
2. Replace direct `current_dir` calls in the changed runner/custom-child paths
   with the helper: ACP client, AGY turn and model listing, Codex client, Cursor
   warm process/turn/model listing, custom-agent workspace fingerprint,
   `build_custom_command`, and custom model listing. Evidence: TC-2450, TC-2451.
3. Add the Linux integration probe for built-in opencode under `-C`, and extend
   the custom-runner cwd test to assert the explicit `PWD` entry. Evidence:
   TC-2450, TC-2451.
4. Run the narrow tests, then the repository Rust and documentation gates.
   Evidence: review record.

## Risks

- Overriding `PWD` could surprise a runner that deliberately expected the
  parent's value. This is acceptable here because the child cwd was already the
  workspace, and a matching `PWD` is the conventional contract.
- The Linux integration test depends on `/proc/$$/environ` because POSIX shells
  rewrite a mismatched `PWD`. The helper unit test keeps cross-platform coverage
  of command construction.
- Non-runner subprocesses are intentionally untouched; the fix is scoped to the
  agent/custom-runner spawn paths changed in this diff.

## Rollback

Revert the code and these artifacts together. There is no persistence migration
and no provider protocol change to unwind.
