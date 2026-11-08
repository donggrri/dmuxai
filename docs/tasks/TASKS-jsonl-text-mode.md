# TASKS: JSONL text_mode and lifecycle completion (#184)

- [x] T1 — `TextMode`/`text_mode` + `ReplyCollector` last/concat folding
      (FR-2530, FR-2531). Tests: TC-2530, TC-2531.
      `cargo test tc_2530 tc_2531`
- [x] T2 — `finish_when`/`finish_reason_path`/`finish_ok`, `unfinished`
      error kind, `run_turn` gate (FR-2532, FR-2534, FR-2535). Tests:
      TC-2532, TC-2534, TC-2535, TC-2536. `cargo test tc_2532 tc_2534 tc_2535 tc_2536`
- [x] T3 — Opencode preset finish settings; grok preset `text_mode` (FR-2533).
      Test: TC-2533. `cargo test tc_2533`
- [x] T4 — Fixtures: fake-agent step_finish modes; opencode stream fixtures
      with step_finish reasons; fake-opencode/fake-grok completed-turn lines.
      `cargo test tc_2430 tc_2440 tc_2442 tc_2446 tc_2450_opencode_run tc_916`
- [x] T5 — Docs: README, custom-agents.md, FR-2442, grok FR-915/TC-916/TC-918.
      `bash scripts/check-spec.sh docs/specs/SPEC-jsonl-text-mode.md`
- [x] T6 — Gates: `cargo fmt`, `cargo test --lib`, `cargo clippy
      --all-targets -- -D warnings`, `python3 scripts/check-spec.py --all`.
