# TASKS: v0.2.2 dogfood fixes

Status: in progress
Plan: [PLAN-v022-dogfood](../plans/PLAN-v022-dogfood.md)

- [x] T1 FR-980: `src/grok_activity.rs` tracker. Test: `cargo test --lib grok_tool_lines_map_to_activity_items` (TC-980)
- [x] T2 FR-980: wire into `src/agent/custom.rs` + fixture mode. Test: `cargo test --bin dmuxai grok_tool_calls_emit_activity_events` (TC-981)
- [x] T3 FR-981: `AgentView.activity`, chip and pill. Tests: `dashboard_agent_activity_matches_tui_status` (TC-982), `dashboard_shows_agent_activity_detail` (TC-983)
- [x] T4 FR-982: cancel notice. Tests: `cancel_appends_one_cancelled_notice_for_every_runner` (TC-984), `dashboard_cancel_shows_cancelled_notice` (TC-985)
- [x] T5 FR-983: welcome from registry. Test: `welcome_lists_registered_runners` (TC-816)
- [x] T6 FR-984: local-only token notice. Tests: `dashboard_snapshot_never_contains_startup_token` (TC-986), `api_snapshots_do_not_echo_token` (TC-987)
- [ ] T7 CHANGELOG, review doc, gates, live grok-4.7 verification, screenshots
