# TASKS: Dashboard full parity and TUI retirement (SPEC-dashboard-full-parity)

Status: Draft (v0.2.4). Nothing below starts until Dongjin approves the gap list (SPEC §3) and OQ-1000..1004.

| Done | Owner | Task | Requirement IDs | Test command |
| --- | --- | --- | --- | --- |
| [x] | Grok Build (grok-4.7) + maintainer | Gap audit (`docs/research/dashboard-parity-audit.md`), spec, plan, tasks, ROADMAP v0.2.4 entry | all | `bash scripts/spec-index.sh`; `pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath docs/specs/SPEC-dashboard-full-parity.md`; `cargo test spec_ids_are_unique_across_specs every_spec_has_status_line` |
| [x] | opencode free (deepseek-v4.1-flash-free, glm-5.3-flash-free) | Spec review | all | `docs/reviews/REVIEW-dashboard-full-parity.md` |
| [ ] | Dongjin | Review the gap list; answer OQ-1000..1004 | — | — |
| [ ] | Grok Build (grok-4.7) | Slice 1: `App::step`, `session_host`, interactive `serve`, `serve --read-only`, single instance, signals, workspace-switch re-exec into `serve` (`main.rs:678-724`) | FR-1000..1002, NFR-1000 | `cargo test serve_ session_host_step_matches_tui_loop` (TC-1000..1003) |
| [ ] | Grok Build (grok-4.7) | Slice 1: command channel without the to-do store; `/quit` under serve | FR-1003, FR-1004 | `cargo test command_channel_without_todo_store serve_quit_stops_server` (TC-1004, TC-1005) |
| [ ] | Grok Build (grok-4.7) | Slice 2: `TUI_FEATURES` and the parity guard | FR-1020, FR-1021 | `cargo test tui_features_have_dashboard_controls` (TC-1020) |
| [ ] | Grok Build (grok-4.7) | Slice 3: image upload route, `pending_images`, attach UI | FR-1010 | `cargo test image_upload_contract session_module_has_image_attach` (TC-1010, TC-1011) |
| [ ] | Grok Build (grok-4.7) | Slice 4: core activity groups, `[ui]` settings, toggle group/last | FR-1011 | `cargo test dashboard_activity_groups_match_tui` (TC-1012) |
| [ ] | Grok Build (grok-4.7) | Slice 5: graph gate Continue/Override controls | FR-1014 | `cargo test graph_gate_controls_contract` (TC-1015) |
| [ ] | Grok Build (grok-4.7) | Slice 6: status header from core `status_segments` | FR-1013 | `cargo test dashboard_status_segments_match_tui` (TC-1014) |
| [ ] | Grok Build (grok-4.7) | Slice 7: transcript links into the Docs preview | FR-1012 | `cargo test dashboard_transcript_links_confined` (TC-1013) |
| [ ] | Grok Build (grok-4.7) | Slice 8: keyboard shortcuts | UX-1000 | `cargo test session_module_has_shortcuts` (TC-1016) |
| [ ] | maintainer | Check #66 grok runner and #67 plan gate dashboard rows, plus diff review sidebar and world model | FR-1021 | their own specs' TCs (SPEC-grok-runner FR-898, SPEC-plan-gate UX-932/933); the guard only confirms the rows exist |
| [ ] | Grok Build (grok-4.7) | Slice 9: TUI deprecation notice | FR-1030 | `cargo test tui_deprecation_notice` (TC-1030) |
| [ ] | Grok Build (grok-4.7) | Slice 10: remove the TUI; deprecate TUI settings; docs | FR-1031, FR-1032 | `cargo tree -i ratatui`; `cargo test tui_theme_settings_deprecated` (TC-1031, TC-1032) |
| [ ] | opencode free models | Review each slice; REVIEW doc; light screenshots | all | `docs/reviews/REVIEW-dashboard-full-parity.md` |

Slices 3, 4, 7 and 8 put their page code in new modules (`assets/attach.js`, `assets/activity.js`, `assets/links.js`, `assets/shortcuts.js`) and only add one hook each to `session.js`, so they can run in parallel worktrees without conflicts.
