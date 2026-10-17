# TASKS: Dashboard parity with the TUI (SPEC-dashboard-parity)

| Done | Owner | Task | Requirement IDs | Test command |
| --- | --- | --- | --- | --- |
| [x] | maintainer | Spec, parity matrix (§3), plan | all | `bash scripts/spec-index.sh`; `cargo test --test spec_index` |
| [x] | maintainer | Snapshot `session` view, transcript limits, `DashboardHub::set_session` | FR-780 | `cargo test dashboard_session_` (TC-780, TC-781) |
| [x] | maintainer | Dashboard prompt into the live session | FR-782 | `cargo test dashboard_prompt_streams_into_transcript` (TC-782) |
| [x] | maintainer | `SessionCommand`, `POST /api/command`, error mapping; App handlers on the TUI paths and `allowed_runners` | FR-781 | `cargo test command_route_contract dashboard_commands_` (TC-783) |
| [x] | maintainer | New/load conversation from the dashboard | FR-781 | `cargo test dashboard_new_and_load_conversation` (TC-784) |
| [x] | maintainer | Slash commands from the dashboard prompt | FR-783 | `cargo test dashboard_slash_commands_via_prompt` (TC-785) |
| [x] | maintainer | `assets/session.js` and `assets/session.css`: prompt box, transcript, pickers, Ctrl+K palette | FR-782, FR-783, UX-780, NFR-780 | `cargo test session_module_has_parity_controls session_assets_are_served_hardened` (TC-786) |
| [x] | maintainer | Parity matrix test | FR-784 | `cargo test tui_commands_have_dashboard_controls parity_matrix_controls_exist_in_session_js` (TC-787) |
| [x] | maintainer | `docs/API.md`, README, screenshots | UX-780 | manual review |
| [x] | cursor (composer-2.5) | Cross-review, then re-review after fixes (APPROVE) | all | `docs/reviews/REVIEW-dashboard-parity.md` |
