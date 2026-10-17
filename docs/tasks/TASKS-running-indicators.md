# TASKS: running-indicators

Status: complete
Spec: [SPEC-running-indicators](../specs/SPEC-running-indicators.md)
Plan: [PLAN-running-indicators](../plans/PLAN-running-indicators.md)

- [x] T1 — `RunState` enum, turn event transitions, and `DashboardSnapshot` serialization.
  Requirements: FR-820, NFR-820. Verify:
  `cargo test --locked --bin dmuxai run_state_transitions_follow_turn_events`
- [x] T2 — TUI braille spinner frames, status bar elapsed format, and end marks.
  Requirements: FR-821, FR-822, UX-820, UX-821. Verify:
  `cargo test --locked --bin dmuxai status_bar_spinner_and_end_marks`,
  `cargo test --locked --bin dmuxai spinner_frames_and_reduced_motion`
- [x] T3 — Web status pill `#run-status`, agent chip spinners, graph node `.station-spinner`, in-place elapsed updates, and reduced-motion fallback.
  Requirements: FR-823, UX-821, NFR-820. Verify:
  `cargo test --locked --lib web::assets::tests::dashboard_has_run_status_indicators`,
  `node --check src/web/assets/app.js`
- [x] T4 — Browser tab title prefix `⏳`/`✓`/`✗`, focus restore, and same-origin SVG favicon swapping under CSP.
  Requirements: FR-824. Verify:
  `cargo test --locked --test web_running -- --ignored`
- [x] T5 — Agent API `status_json` mapping `pending`/`stalled` to `running` with preserved start time.
  Requirements: FR-820. Verify:
  `cargo test --locked --lib agent_api::tests::status_json_maps_pending_and_stalled_to_running_preserving_start_time`
- [x] T6 — Theme reduced motion configuration (`[theme] reduced_motion` and `DMUXAI_REDUCED_MOTION`).
  Requirements: UX-821. Verify:
  `cargo test --locked --lib settings::builtin_opencode_tests::theme_reduced_motion_config_and_env_var_are_honored`
- [x] T7 — Gates. Verify: `cargo fmt --check`,
  `cargo clippy --locked --all-targets -- -D warnings`,
  `cargo test --locked --all-targets`,
  `cargo test --locked --test web_running -- --ignored`.
