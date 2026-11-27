# REVIEW: open model lists

Spec: [SPEC-open-model-lists.md](../specs/SPEC-open-model-lists.md)

## Compliance

| Requirement | Status | Evidence |
| --- | --- | --- |
| FR-3000, FR-3001 | Met | `tc_3000_agy_and_cursor_accept_any_non_fast_model` |
| FR-3002, FR-3003 | Met | `tc_3001_fast_blocked_unless_user_opt_in`, `tc_3008_allow_fast_is_user_config_only`; Cursor ACP stays blocked via `acp_model_arg` |
| FR-3004..FR-3007 | Met | `tc_3002`..`tc_3007` in `src/agent_api.rs` (fake CLIs, injected program path, short timeouts) |
| FR-3008, FR-3009 | Met | `tc_3009_*` (graph), `tc_3010_*` (pickers), `tc_3011_*` (Cursor argv); Codex/grok/opencode/custom picker behaviour unchanged (`tc_047`) |
| FR-3010 | Met | `tc_3012_status_model_policy_text` (`runners --json`) |
| FR-3011 | Met | `tc_3013_*` in `src/agent/cursor.rs` and `src/agent/agy.rs` |
| NFR-3000 | Met | validation stays pure; listing bounded to 10 s and cached (`tc_3005`) |

## Residual risks

- The static fallbacks are a snapshot (2026-11-26); live CLI listings win whenever the CLI answers.
- Codex stays `composer-2.5` only (OQ-3001); opening it needs a separate decision.
- `allow_fast` is a process-wide switch set once from the user config; unit tests only use `validate_model_for_runner_with`.
- Pickers never offer fast ids even with `allow_fast`; explicit `--model`/config selection is the only route.

## Test evidence

Full local gate (`fullgate.sh`, 2026-11-26): `GATE: PASS gate=1726/0 web=58/0`. `check-spec --all` 101/101.
Perf (#301): `dmuxai models --agent cursor` median ~1.31 s vs raw `cursor-agent --list-models` ~1.27 s (+3%); `models --agent agy` was not slower than raw `agy models`. Validation path adds no process spawn.
