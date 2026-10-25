# REVIEW: Runner fallback handover correctness

Status: complete  
Spec: [SPEC-runner-fallback.md](../specs/SPEC-runner-fallback.md)  
Reviewed: 2026-10-25  
Target: Linux (`x86_64-unknown-linux-gnu`); no provider credentials needed

## Spec compliance

- FR-2130 (no stale handover state): the handover state lives in
  `App::pending_fallback` (`PendingFallback`: chain plus original prompt).
  `try_submit_to_agent` takes it at the top of the function, before every
  early return (busy, summary, deferred, send error), and
  `try_runner_fallback` clears it when it starts and on the failed-submit
  path. The exhausted tail leaves nothing pending.
- FR-2131 (one preface, original text): `ActiveTurn.fallback_original_prompt`
  carries the user's prompt across hops; every hop builds
  `preface(this hop) + original prompt`, and the transcript always shows the
  original user text as the user message (`pending_display_text` is set to the
  original text for the handover).
- FR-2132 (one handover record, one error): the Notice + Failed "handed over
  to ...", the `fallback` event, the graph-node rebind and the retirement of the
  failed turn happen only after `try_submit_to_agent` returned `Ok`. A target
  that fails to start restores the active turn, the busy flag, the selection
  and the graph-node mapping, then the next target is tried. When every target
  fails, the exhausted `FallbackRecord` is written once and the original error
  is left to the normal error path, which records it once (`retired_internal_turns`
  guards the same-runner case) and finishes the to-do and the graph node once.
- FR-2133 (unusable target skipped): `apply_fallback_target` applies the
  runner and the model before the submit; a failing `select_model` pushes a
  system note and skips the target, so the turn never runs with the wrong
  model.
- FR-2134 (headless answer text): `App::headless_answer_text` keeps only the
  messages of the final runner when a handover happened; the failed runner's
  partial output stays in `messages`. The CLI JSON now prints the run's
  `fallbacks` array (omitted when empty), as FR-2119 requires.
- FR-2101 (config merge): `FallbackSettings::merge` merges per runner id and
  for `default` field-wise. `RunnerFallbackConfig` keeps `Option` fields so a
  table that sets only `stall_timeout_secs` no longer drops the user's chain.
  `effective_stall_timeout_secs()` / `effective_stall_api_retries()` keep the
  previous defaults (240 s, 3).

## Regression and safety review

- The classification of failures, the stall detector, the default timeouts and
  the fact that the fallback target becomes the selected runner are unchanged
  (no behaviour outside the reported bugs).
- Provider-neutral boundaries are untouched: the adapters, the conversation
  persistence and the process launching are not modified; only the App's
  handover bookkeeping, the CLI JSON payload and the config merge change.
- No persistence migration: the turn records keep their shape, only their
  order changes (the failed turn is failed once instead of twice).
- The graph-node rebind is symmetric: `remap_graph_node_for_fallback` returns
  the previous mapping and `restore_graph_node_for_fallback` puts it back when
  the target does not start, so the graph node keeps the failed runner when
  the handover fails.
- The workspace/user config merge changes precedence only for fields the
  workspace table does not set; an explicit workspace chain (including
  `fallback = []`) still replaces the user chain.
- The test fixture `tests/fixtures/fake-agent.sh` gained a `rate-limit`
  behavior and JSON escaping for the prompt, so a multi-line handover prompt
  still produces valid JSONL. Existing fixture output for single-line prompts
  is unchanged.

## Test evidence

- `cargo fmt --all -- --check`: passed.
- `cargo check --all-targets`: passed (no warnings).
- `cargo test --bin dmuxai app::tests`: passed, 351 tests, 0 failures
  (includes `tc_2118_*`, `tc_2119_*` and the new `tc_2130_*` .. `tc_2134_*`).
- `cargo test --lib tc_2135`: passed (2 tests; field-wise merge keeps the user
  chain and an explicit workspace chain still replaces it).
- `cargo test --test agent_api_cli`: passed, 16 tests, 0 failures, including
  `tc_2136_cli_fallback_handover_uses_the_second_agent` (exit 0, one
  `fallbacks` record from `first` to `second`, text of the second agent only).
- `cargo test --test cli`: passed, 9 tests, 0 failures.
- `cargo test --lib settings`: passed, 53 tests, 0 failures.
- `cargo test --test graph_orchestration`: passed, 4 tests, 0 failures.
- Regression proof: each new test was re-run against a temporary
  re-introduction of the reported bug (stale chain kept, preface stacked, the
  failed turn recorded before the submit, `select_model` error ignored, text
  filter removed) and failed as expected; the fixed code passes.

## Residual risks and untested paths

- The browser/TUI rendering of the handover system line is unchanged and not
  re-tested here; the headless and TUI message paths share the same text.
- A same-runner handover (same runner, other model) still retires the failed
  turn before the new turn starts, because the conversation engine refuses two
  running turns of one runner. That path is covered by the existing
  `tc_2119_same_runner_model_fallback`.
- Live provider behaviour was not exercised; every test here is deterministic
  and needs no credentials.
