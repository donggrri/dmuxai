# DmuxAI 테스트 계획 (Spec-driven)

Status: draft · 기준 커밋: `73e3f4d` (main, PR #6 이후) · 작성일: 2026-10-17

이 문서는 [MISSION.md](../MISSION.md) → `docs/specs/*.md` → FR/NFR → TC → 실제 테스트 이름으로 이어지는 추적성을 정리하고, 아직 SPEC이나 테스트가 없는 기능에 대한 새 FR/TC를 제안한다. ID와 테스트 이름은 영어로 유지한다.

> CI의 `Spec traceability` 잡은 `scripts/check-spec.ps1 -SpecPath docs/specs/DMUXAI-BASELINE.md` 하나만 검사한다(Must 요구사항마다 TC 매핑 존재, ID 중복 금지). 따라서 아래 갭 제안은 별도 SPEC stub 없이 이 문서 안에 둔다. 제안이 확정되면 `docs/templates/FEATURE-SPEC.md` 형식으로 SPEC을 만든다.

## 0. Mission 목표 ID

| ID | MISSION 항목 |
|---|---|
| M1 | Graph-first delivery (노드별 자동 라우팅) |
| M2 | Primary runners: cursor-agent 기본, agy 보조, Codex 등은 선택 |
| M3 | Skills (`.agents/skills/*/SKILL.md`) |
| M4 | Permissions (Ask/Plan 읽기 전용, Implement 쓰기 차단 시 실패 기록) |
| M5 | Direct image attach (Ctrl+V / Alt+V) |
| M6 | Platforms: Linux TUI 우선, Windows는 WSL 런처 |
| M7 | Model policy: composer-2.5 only (그래프/서브에이전트/기본값) |
| M8 | Graph execution report (`docs/graph-runs/`) |

## 0.1 새 SPEC 문서 (2026-10-17 추가)

§2의 제안 중 오늘 구현/설계된 기능은 아래 SPEC으로 정식화했다. SPEC에서는 CI 형식(`FR-\d+`, `TC-\d+`)에 맞춰 숫자 ID를 쓰고, 이 문서의 접두어 ID는 그 대응표다.

| SPEC | Status | 숫자 ID | 이 문서 제안 ID |
|---|---|---|---|
| [SPEC-permission-hardening](specs/SPEC-permission-hardening.md) | Implemented | FR-300~303, TC-300~305 | FR-PERM-*, TC-PERM-* |
| [SPEC-model-policy](specs/SPEC-model-policy.md) | Implemented | FR-310~314, TC-310~315 | FR-MODEL-*, TC-MODEL-* |
| [SPEC-speaker-colors](specs/SPEC-speaker-colors.md) | Implemented | FR-320~322, UX-320, TC-320~324 | FR-THEME-*, TC-THEME-* |
| [SPEC-custom-agent-provider](specs/SPEC-custom-agent-provider.md) | Implemented | FR-330~335, UX-330, TC-330~337 | FR-CUSTOM-*, TC-CUSTOM-* |
| [SPEC-compact-summarize-focus](specs/SPEC-compact-summarize-focus.md) | Draft | FR-340~342, TC-340~342 | FR-COMPACT-*, TC-COMPACT-* |
| [SPEC-windows-desktop](specs/SPEC-windows-desktop.md) | Draft (tui-stabilize 병합 후 구현) | FR-390~398, NFR-390, UX-390, TC-390~404 | FR-WIN-*, TC-WIN-* |

SPEC 안의 테스트 이름은 `rg 'fn <name>' src tests`로 확인했고, 없는 것은 `*proposed*`로 표시했다. 확인된 실제 테스트: `user_selected_agy_model_wins_over_graph_default`, `graph_default_model_only_applies_to_cursor`, `chat_lines_render_with_speaker_theme_colors`, `explicit_summary_is_internal_and_the_next_turn_uses_it_fresh` 등 (§2 표의 일부 제안 이름은 이 실제 이름으로 대체된다).

## 1. 추적성 매트릭스

상태 기준:
- ✅ SPEC에 적힌 테스트 이름이 `src/`·`tests/`에 `fn <name>`으로 실제 존재 (또는 `cargo test` 필터 문자열로 매칭됨)
- ⚠️ SPEC에는 있으나 이름 붙은 자동 테스트가 없음(수동/이름 없는 증거) 또는 코드에서 찾지 못함
- ❌ SPEC도 테스트도 없음 → §2 갭으로 다룸

검증 방법: 각 SPEC의 `| TC-… |` 행(그래프/이미지 SPEC은 `| FR-… |` 요구사항 행의 Tests 열)에서 백틱 안 식별자를 뽑아 `rg 'fn <name>' src tests`로 확인했다.

**집계: ✅ 71 · ⚠️ 19 · ❌ 0 (기존 SPEC 내부)** — 기존 SPEC 밖의 미커버 기능은 §2에서 ❌ 14건.

검증 명령: 전체 자동 테스트 `cargo test --all-targets`, 단일 TC는 `cargo test <test name>`.

| Mission | SPEC | FR/NFR | TC | Test name(s) | Type | Status | Note |
|---|---|---|---|---|---|---|---|
| M2,M4,M6 | DMUXAI-BASELINE | FR-001, FR-010, FR-011 | TC-001 | `completed_conversation_and_binding_survive_restart`, `a_new_app_instance_loads_the_latest_conversation` | integration | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | FR-002, UX-001, BR-001 | TC-002 | `submitting_routes_to_the_selected_agent`, `another_idle_agent_can_start_after_switching_conversations` | unit | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | INT-001, INT-002, INT-003, INT-060, INT-061, NFR-060, INV-005 | TC-003 | `cursor_mode_validation_matches_platform` | unit | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | UX-003 | TC-004 | `model_catalog_and_selection_are_kept_per_agent` | unit | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | FR-012, FR-013, INV-001, INV-003 | TC-005 | `a_mismatched_output_rolls_back_the_entire_batch` | unit | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | FR-014, FR-020, FR-021, INV-002 | TC-006 | `provider_session_binding_is_reused_by_the_same_agent`, `switching_agents_automatically_shares_the_completed_context` | unit | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | FR-015, FR-016, FR-017, INV-006 | TC-007 | `workspace_recovery_handles_all_live_turns_without_touching_other_workspaces`, `delayed_output_is_saved_to_its_original_conversation_only` | unit+manual | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | FR-022, FR-023, BR-002, BR-003, BR-004 | TC-008 | `summary_checkpoint_is_used_with_a_gap_free_recent_tail`, `automatic_summary_retries_the_deferred_prompt_once`, `oversized_summary_is_rejected_without_invalidating_the_binding` | unit | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | FR-024 | TC-009 | `repeated_share_does_not_nest_previous_import_payloads` | unit | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | UX-002, UX-010, UX-011, UX-012 | TC-010 | — | manual | ⚠️ | no named automated test (manual/unnamed evidence) |
| M2,M4,M6 | DMUXAI-BASELINE | UX-013, NFR-002 | TC-011 | `mouse_scroll_moves_chat_without_changing_agent`, `prompt_resize_keeps_minimum_chat_and_input_heights` | unit | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | UX-014 | TC-012 | — | manual | ⚠️ | no named automated test (manual/unnamed evidence) |
| M2,M4,M6 | DMUXAI-BASELINE | FR-003, FR-004, FR-005, NFR-004, INV-004 | TC-013 | — | manual | ⚠️ | no named automated test (manual/unnamed evidence) |
| M2,M4,M6 | DMUXAI-BASELINE | BR-010, BR-011 | TC-014 | `failed_initial_migration_rolls_back_every_schema_change` | unit | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | NFR-003, FR-030 | TC-015 | — | manual | ⚠️ | no named automated test (manual/unnamed evidence) |
| M2,M4,M6 | DMUXAI-BASELINE | NFR-001 | TC-016 | — | manual | ⚠️ | no named automated test (manual/unnamed evidence) |
| M2,M4,M6 | DMUXAI-BASELINE | FR-040, FR-041, FR-042 | TC-017 | — | manual | ⚠️ | no named automated test (manual/unnamed evidence) |
| M2,M4,M6 | DMUXAI-BASELINE | UX-002, UX-003, FR-043 | TC-018 | `model_picker_tab_selects_codex_model_and_opens_its_effort_picker`, `mode_change_reopens_the_agent_with_shared_context` | unit | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | UX-011 | TC-019 | `exact_quit_command_exits_without_starting_a_turn`, `slash_preview_can_execute_quit_with_enter`, `slash_preview_can_execute_quit_with_tab` | unit | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | UX-012 | TC-020 | `escape_never_exits_the_application` | unit | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | FR-050, FR-052 | TC-050 | `multiple_live_instances_start_in_separate_conversations` | unit+manual | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | FR-050, FR-051, BR-050 | TC-051 | `multiple_live_instances_start_in_separate_conversations` | unit+manual | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | FR-051, BR-050 | TC-052 | `closed_or_expired_session_turns_are_recovered` | integration | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | FR-001, FR-052 | TC-053 | `a_new_app_instance_loads_the_latest_conversation` | unit | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | BR-051 | TC-054 | `busy_agent_is_rejected_across_runtime_sessions` | integration | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | BR-051 | TC-055 | `same_agent_can_run_in_separate_instance_conversations` | integration | ✅ |  |
| M2,M4,M6 | DMUXAI-BASELINE | BR-010, BR-011, NFR-050 | TC-056 | `runtime_session_schema_extension_preserves_existing_data` | integration | ✅ |  |
| M1,M3,M7 | SPEC-agent-graph-orchestration | FR-GRAPH-1 | (req row) | `planner_sets_orchestrator_model` | unit | ✅ |  |
| M1,M3,M7 | SPEC-agent-graph-orchestration | FR-GRAPH-2 | (req row) | `validate_rejects_fast_and_non_composer_models` | unit | ✅ |  |
| M1,M3,M7 | SPEC-agent-graph-orchestration | FR-GRAPH-3 | (req row) | `validate_model_id` | unit | ✅ |  |
| M1,M3,M7 | SPEC-agent-graph-orchestration | FR-GRAPH-4 | (req row) | `simple_prompt_uses_single_agent` | unit | ✅ |  |
| M1,M3,M7 | SPEC-agent-graph-orchestration | UX-GRAPH-1 | (req row) | — | manual | ⚠️ | no named automated test (manual/unnamed evidence) |
| M1,M3,M7 | SPEC-agent-graph-orchestration | FR-GRAPH-5 | (req row) | `gate_pauses_until_continue` | unit | ✅ |  |
| M1,M3,M7 | SPEC-agent-graph-orchestration | NFR-GRAPH-1 | (req row) | — | manual | ⚠️ | no named automated test (manual/unnamed evidence) |
| M1,M3,M7 | SPEC-agent-graph-orchestration | FR-GRAPH-6 | (req row) | `store_round_trip` | unit | ✅ |  |
| M1,M3,M7 | SPEC-agent-graph-orchestration | FR-GRAPH-7 | (req row) | `discover_skills` | unit | ✅ |  |
| M2 | SPEC-automatic-cross-agent-sharing | FR-025, UX-015 | TC-001 | — | integration | ⚠️ | no named automated test (manual/unnamed evidence) |
| M2 | SPEC-automatic-cross-agent-sharing | FR-026 | TC-002 | — | integration | ⚠️ | no named automated test (manual/unnamed evidence) |
| M2 | SPEC-automatic-cross-agent-sharing | FR-026, NFR-005 | TC-003 | — | integration | ⚠️ | no named automated test (manual/unnamed evidence) |
| M2 | SPEC-automatic-cross-agent-sharing | FR-027 | TC-004 | — | integration | ⚠️ | no named automated test (manual/unnamed evidence) |
| M2 | SPEC-automatic-cross-agent-sharing | FR-028 | TC-005 | — | integration | ⚠️ | no named automated test (manual/unnamed evidence) |
| M2 | SPEC-automatic-cross-agent-sharing | UX-015 | TC-006 | — | unit | ⚠️ | no named automated test (manual/unnamed evidence) |
| M2 | SPEC-automatic-cross-agent-sharing | NFR-005 | TC-007 | — | integration | ⚠️ | no named automated test (manual/unnamed evidence) |
| M5 | SPEC-clipboard-image-paste | FR-201 | TC-101 | `first_failed_command_falls_back_to_next` | unit | ✅ |  |
| M5 | SPEC-clipboard-image-paste | FR-202 | TC-102 | `non_png_output_is_rejected` | unit | ✅ |  |
| M5 | SPEC-clipboard-image-paste | FR-204 | TC-103 | `all_failed_commands_return_none` | unit | ✅ |  |
| M5 | SPEC-clipboard-image-paste | FR-205 | TC-104 | `empty_output_is_rejected` | unit | ✅ |  |
| M5 | SPEC-clipboard-image-paste | FR-206 | TC-105 | `base64_powershell_output_decodes_to_png` | unit | ✅ |  |
| M5 | SPEC-clipboard-image-paste | FR-203 | TC-106 | `save_attachment_writes_png_into_workspace_attachments` | unit | ✅ |  |
| M5 | SPEC-clipboard-image-paste | FR-203 | TC-107 | `clipboard_paste_registers_pending_image_chip` | integration | ✅ |  |
| M5 | SPEC-clipboard-image-paste | FR-204, FR-205 | TC-108 | `clipboard_paste_without_png_is_a_noop` | integration | ✅ |  |
| M5 | SPEC-clipboard-image-paste | FR-207 | TC-109 | `cursor_passes_image_paths_to_cli`, `agy_passes_image_paths_to_cli` | unit | ✅ |  |
| M5 | SPEC-clipboard-image-paste | FR-208 | TC-110 | — | manual | ⚠️ | no named automated test (manual/unnamed evidence) |
| M5 | SPEC-clipboard-image-paste | FR-201, FR-206 | TC-111 | — | manual | ⚠️ | no named automated test (manual/unnamed evidence) |
| M2 (optional Codex) | SPEC-codex-effort-tool-usage | FR-040 | TC-040 | `codex_turn_request_includes_selected_effort` | unit | ✅ |  |
| M2 (optional Codex) | SPEC-codex-effort-tool-usage | FR-040, BR-040 | TC-041 | `effort_picker` | unit | ✅ | cargo test filter (substring) |
| M2 (optional Codex) | SPEC-codex-effort-tool-usage | FR-041 | TC-042 | `normalizes_codex_tool_item_lifecycle` | unit | ✅ |  |
| M2 (optional Codex) | SPEC-codex-effort-tool-usage | FR-041, BR-041, NFR-040 | TC-043 | `codex_tool_progress` | integration | ✅ | cargo test filter (substring) |
| M2 (optional Codex) | SPEC-codex-effort-tool-usage | FR-042 | TC-044 | `codex_status` | integration | ✅ | cargo test filter (substring) |
| M2 (optional Codex) | SPEC-codex-effort-tool-usage | FR-042, NFR-040 | TC-045 | `codex_status_reports_unavailable_provider_data` | unit | ⚠️ | missing: `codex_status_reports_unavailable_provider_data` |
| M2 (optional Codex) | SPEC-codex-model-effort-mode-context | UX-040, NFR-042 | TC-047 | `model_picker_tab_selects_codex_model_and_opens_its_effort_picker` | unit | ✅ |  |
| M2 (optional Codex) | SPEC-codex-model-effort-mode-context | FR-043, NFR-042 | TC-048 | `mode_change_reopens_the_agent_with_shared_context` | integration | ✅ |  |
| M2 (optional Codex) | SPEC-codex-model-effort-mode-context | BR-042, NFR-042 | TC-049 | `busy_agent_cannot_open_mode_picker` | unit | ✅ |  |
| M4 | SPEC-codex-sandbox-policy | INT-040, NFR-041 | TC-046 | `codex_execution_modes_set_per_turn_sandbox_and_approval_policy` | unit | ✅ |  |
| M4,M6 | SPEC-cursor-windows-implement-guard | INT-060 | TC-060 | `cursor_mode_validation_matches_platform`, `windows_cursor_implement_mode_is_rejected_before_queueing`, `unsupported_mode_is_rejected_before_binding_retirement` | unit | ✅ |  |
| M4,M6 | SPEC-cursor-windows-implement-guard | NFR-060 | TC-061 | `windows_cursor_implement_turn_is_rejected_before_process_spawn` | unit | ✅ |  |
| M4,M6 | SPEC-cursor-windows-implement-guard | INT-061 | TC-062 | `cursor_execution_modes_use_native_cli_flags` | unit | ✅ |  |
| M4,M6 | SPEC-cursor-windows-implement-guard | INT-061 | TC-063 | `cursor_mode_validation_matches_platform` | unit | ✅ |  |
| M5 | SPEC-image-attachment | FR-IMG-1 | (req row) | `image_slash_command_attaches_supported_file` | unit | ✅ |  |
| M5 | SPEC-image-attachment | FR-IMG-2 | (req row) | `rejects_oversized_images`, `copies_supported_image_into_workspace_store` | unit | ✅ |  |
| M5 | SPEC-image-attachment | FR-IMG-3 | (req row) | `submit_prompt_forwards_image_paths_to_adapter` | unit | ✅ |  |
| M5 | SPEC-image-attachment | FR-IMG-4 | (req row) | `prompt_with_image_references` | unit | ✅ |  |
| M5 | SPEC-image-attachment | FR-IMG-5 | (req row) | `cursor_passes_image_paths_to_cli`, `agy_passes_image_paths_to_cli` | unit | ✅ |  |
| M5 | SPEC-image-attachment | UX-IMG-1 | (req row) | `chip_line`, `clipboard_paste_registers_pending_image_chip` | unit+manual | ✅ |  |
| M6 | SPEC-multi-instance-workspace | FR-050, FR-052 | TC-050 | `multiple_live_instances_start_in_separate_conversations` | integration | ✅ |  |
| M6 | SPEC-multi-instance-workspace | FR-050, FR-051, BR-050 | TC-051 | `multiple_live_instances_start_in_separate_conversations` | integration | ✅ |  |
| M6 | SPEC-multi-instance-workspace | FR-051, BR-050 | TC-052 | `closed_or_expired_session_turns_are_recovered` | integration | ✅ |  |
| M6 | SPEC-multi-instance-workspace | FR-052 | TC-053 | `a_new_app_instance_loads_the_latest_conversation` | integration | ✅ |  |
| M6 | SPEC-multi-instance-workspace | BR-051 | TC-054 | `busy_agent_is_rejected_across_runtime_sessions` | integration | ✅ |  |
| M6 | SPEC-multi-instance-workspace | BR-051 | TC-055 | `same_agent_can_run_in_separate_instance_conversations` | unit | ✅ |  |
| M6 | SPEC-multi-instance-workspace | NFR-050 | TC-056 | `runtime_session_schema_extension_preserves_existing_data` | unit | ✅ |  |
| M6 (TUI) | SPEC-quit-slash-command | UX-011 | TC-019 | `exact_quit_command_exits_without_starting_a_turn` | unit | ✅ |  |
| M6 (TUI) | SPEC-quit-slash-command | UX-011 | TC-020 | `slash_preview_can_execute_quit_with_enter`, `slash_preview_can_execute_quit_with_tab` | unit | ✅ |  |
| M6 (TUI) | SPEC-quit-slash-command | UX-012 | TC-021 | `escape_never_exits_the_application` | unit | ✅ |  |
| M6 (TUI) | SPEC-quit-slash-command | UX-011 | TC-022 | `quit_with_whitespace_is_regular_prompt_text` | unit | ✅ |  |
| M2 | SPEC-task-progress-visibility | FR-060 | TC-060 | `queued_activity_is_visible_after_request_acceptance` | unit | ✅ |  |
| M2 | SPEC-task-progress-visibility | FR-061, UX-030 | TC-061 | `running_activity_reflects_observable_phase` | unit | ✅ |  |
| M2 | SPEC-task-progress-visibility | FR-061, UX-030 | TC-062 | `activity_status_reports_observed_event_age` | unit | ✅ |  |
| M2 | SPEC-task-progress-visibility | UX-031, FR-062 | TC-063 | `active_agents_are_visible_and_terminal_activity_is_cleared` | unit | ✅ |  |
| M2 | SPEC-task-progress-visibility | NFR-020 | TC-064 | — | manual | ⚠️ | no named automated test (manual/unnamed evidence) |


### 1.1 주의할 ⚠️ 항목

- `SPEC-codex-effort-tool-usage` TC-045: `codex_status_reports_unavailable_provider_data`가 코드에 없음 → 테스트 추가 또는 SPEC 수정 필요.
- `SPEC-automatic-cross-agent-sharing` TC-001~007: 증거가 "`src/app.rs` test"처럼 이름 없이 적혀 있음 → 실제 테스트 이름으로 SPEC 갱신 필요.
- `SPEC-agent-graph-orchestration` UX-GRAPH-1, NFR-GRAPH-1: "manual / executor defaults"뿐 → TC-GRAPH-08/09 제안(§2.10).
- `SPEC-agent-graph-orchestration`은 TC 표 없이 요구사항 행에 테스트 이름만 있음. `FR-GRAPH-7`의 `discover_skills`는 실제 이름 `discover_skills_finds_agent_graph_engineering`(필터 매칭).
- `SPEC-image-attachment`도 TC 표가 없음. 두 SPEC 모두 템플릿 형식(§4 Requirements / §5 Acceptance scenarios)으로 옮기는 것을 권장.
- TC ID가 SPEC 사이에서 겹침(예: TC-019, TC-050, TC-060이 BASELINE과 개별 SPEC에 모두 있음). CI는 BASELINE 안에서만 중복을 검사하므로, 새 ID는 기능 접두어(`TC-PERM-1` 등)를 쓴다.

## 2. 갭과 제안 (❌)

형식: 새 FR ID · Priority · TC 행(Given / When / Then · test name · type). 제안 테스트 이름은 아직 코드에 없다.

### 2.1 Permissions (M4) — 부분 구현, SPEC 없음
기존 테스트: `permission_denial_is_detected_in_stderr`, `implement_without_writes_fails`, `ask_mode_does_not_require_writes` (`src/agent/implement_outcome.rs`) — SPEC에 연결 안 됨.

| FR | Pri | 요구사항 |
|---|---|---|
| FR-PERM-1 | Must | Implement 턴에서 권한 거부 stderr가 있거나 관찰 가능한 쓰기가 없으면 턴을 실패로 기록하고 이유를 저장 |
| FR-PERM-2 | Must | Plan/Ask 턴은 쓰기 없음으로 실패 처리되지 않음(Plan은 절대 차단되지 않음) |
| FR-PERM-3 | Must | Windows의 unsandboxed Cursor는 `cursor.windows_unsandboxed = true` 명시적 opt-in일 때만 |

| TC | FR | Given | When | Then | Test name | Type |
|---|---|---|---|---|---|---|
| TC-PERM-1 | FR-PERM-1 | Implement 턴, stderr에 permission denied | 턴 종료 | 상태 Failed, stderr 이유 저장 | `permission_denial_is_detected_in_stderr` (✅ 존재) + `implement_failure_reason_is_persisted` | unit+integration |
| TC-PERM-2 | FR-PERM-1 | Implement 턴, 쓰기 이벤트 없음 | 턴 종료 | Failed("no observable writes") | `implement_without_writes_fails` (✅) | unit |
| TC-PERM-3 | FR-PERM-2 | Plan 모드, 쓰기 없음 | 턴 종료 | Completed | `plan_mode_never_fails_for_missing_writes` | unit |
| TC-PERM-4 | FR-PERM-3 | 설정 없음 / `windows_unsandboxed=true` | Windows Cursor Implement | 기본 거부, opt-in 시 허용 | `windows_unsandboxed_requires_explicit_opt_in` | unit |

### 2.2 화자별 색상 + NO_COLOR/force_color (M6, dogfood #2)
기존 테스트: `color_for_maps_each_speaker`, `no_color_disables_unless_forced`, `parses_theme_section`.

| FR | Pri | 요구사항 |
|---|---|---|
| FR-THEME-1 | Must | You / Cursor / AGY / opencode / DmuxAI 메시지가 서로 다른 색 |
| FR-THEME-2 | Must | `NO_COLOR`면 색 끔, `DMUXAI_FORCE_COLOR=1` 또는 `[theme] force_color=true`면 강제로 켬, `DMUXAI_FORCE_COLOR=0`은 강제로 끔 |
| FR-THEME-3 | Should | NO_COLOR로 색이 꺼졌을 때 시작 안내 메시지 |

| TC | FR | Given | When | Then | Test name | Type |
|---|---|---|---|---|---|---|
| TC-THEME-1 | FR-THEME-1 | 기본 theme | 화자별 색 조회 | 모두 다름 | `color_for_maps_each_speaker` (✅) | unit |
| TC-THEME-2 | FR-THEME-1 | TestBackend, 화자 3명 메시지 | 렌더 | 버퍼 셀 fg 색이 화자별로 다름 | `rendered_buffer_colors_each_speaker` | unit |
| TC-THEME-3 | FR-THEME-2 | env 조합 표 | 설정 해석 | 표대로 on/off | `no_color_disables_unless_forced` (✅) | unit |
| TC-THEME-4 | FR-THEME-3 | NO_COLOR=1, force 없음 | 시작 | 안내 시스템 메시지 1회 | `no_color_startup_notice_is_shown` | unit |
| TC-THEME-5 | FR-THEME-2 | 실제 터미널 NO_COLOR=1 + FORCE=1 | 실행 | 색 보임 | live checklist L-05 | live-TUI |

### 2.3 러너별 모델 선택 (M2, M7, dogfood #3)
기존 테스트: `only_composer_2_5_is_allowed`, `validate_rejects_fast_and_foreign_models`, `runner_aware_policy_allows_agy_flash_only_on_agy`, `tests/model_policy_repo.rs` 3개.

| FR | Pri | 요구사항 |
|---|---|---|
| FR-MODEL-1 | Must | 해당 에이전트에서 F2로 고른 모델이 항상 우선 |
| FR-MODEL-2 | Must | composer-2.5 그래프 기본값은 Cursor 러너에만 적용 |
| FR-MODEL-3 | Must | AGY는 `gemini-*-flash-*`, opencode는 `opencode/*-free`만 허용 (`/graph override` 포함) |
| FR-MODEL-4 | Must | 저장소 그래프/에이전트 정의는 composer-2.5만 |

| TC | FR | Given | When | Then | Test name | Type |
|---|---|---|---|---|---|---|
| TC-MODEL-1 | FR-MODEL-1 | AGY + F2 Gemini Flash, planner가 composer-2.5 제안 | 제출 | 요청 모델 = Gemini Flash | `f2_model_wins_over_planner_default` | unit |
| TC-MODEL-2 | FR-MODEL-2 | Cursor, F2 미선택 | 제출 | 모델 composer-2.5 | `graph_default_applies_only_to_cursor` | unit |
| TC-MODEL-3 | FR-MODEL-3 | 정책 | 러너별 검증 | 표대로 허용/거부 | `runner_aware_policy_allows_agy_flash_only_on_agy` (✅) + `opencode_policy_allows_free_models_only` | unit |
| TC-MODEL-4 | FR-MODEL-4 | 저장소 | 스캔 | 다른 모델 없음 | `graph_agent_definitions_use_composer_only` (✅) | integration |

### 2.4 범용 헤드리스 커스텀 에이전트 (`[[agents.custom]]`, 진행 중)

| FR | Pri | 요구사항 |
|---|---|---|
| FR-CUSTOM-1 | Must | `dmuxai.toml`의 `[[agents.custom]]`(name, command, models_command, output)으로 에이전트 등록 |
| FR-CUSTOM-2 | Must | `{model}`, `{workspace}`, `{prompt}` 자리표시자 치환, 셸 경유 없이 argv 실행 |
| FR-CUSTOM-3 | Must | `output = "jsonl" \| "text"` 파싱, 비정상 종료/실패 패턴이면 턴 실패 |
| FR-CUSTOM-4 | Must | 등록된 에이전트가 `/agent`, F2, 그래프 노드에 나타남 |
| FR-CUSTOM-5 | Should | 이미지 전달 방식, 세션 이어가기, 권한 플래그를 설정으로 지정 |

| TC | FR | Given | When | Then | Test name | Type |
|---|---|---|---|---|---|---|
| TC-CUSTOM-1 | FR-CUSTOM-1 | 유효/필수 필드 누락 toml | 로드 | 정상 등록 / 명확한 오류 | `custom_agent_config_parses_and_validates` | unit |
| TC-CUSTOM-2 | FR-CUSTOM-2 | 공백·따옴표 포함 prompt | argv 생성 | 한 인자로 전달, 셸 해석 없음 | `custom_agent_placeholders_expand_without_shell` | unit |
| TC-CUSTOM-3 | FR-CUSTOM-3 | 가짜 스크립트가 jsonl/exit 1 출력 | 턴 실행 | 텍스트 반영 / Failed | `custom_agent_jsonl_stream_and_failure` | integration |
| TC-CUSTOM-4 | FR-CUSTOM-4 | custom 1개 등록 | `/agent`, F2 | 목록에 표시, models_command 결과 사용 | `custom_agent_listed_in_agent_picker` | unit |
| TC-CUSTOM-5 | FR-CUSTOM-1 | opencode 예시 설정 + glm-5.3-flash-free | 실제 프롬프트 | 응답 수신 | live checklist L-12 | live-TUI |

### 2.5 `/compact` + `/summarize <focus>` (계획)
현재 `/summarize`는 존재, 이름 붙은 테스트/SPEC 없음.

| FR | Pri | 요구사항 |
|---|---|---|
| FR-COMPACT-1 | Must | `/compact`는 `/summarize`의 별칭 |
| FR-COMPACT-2 | Must | `/compact <focus>` / `/summarize <focus>`의 focus가 요약 지시문에 포함 |
| FR-COMPACT-3 | Must | 원본 대화 기록은 삭제되지 않고, 이후 요청에는 요약 + 최근 대화만 전달 |

| TC | FR | Given | When | Then | Test name | Type |
|---|---|---|---|---|---|---|
| TC-COMPACT-1 | FR-COMPACT-1 | 대화 있음 | `/compact` | summarize와 같은 요청 | `compact_is_alias_for_summarize` | unit |
| TC-COMPACT-2 | FR-COMPACT-2 | 대화 있음 | `/compact 결정사항 위주` | 요약 프롬프트에 focus 포함 | `summarize_focus_is_included_in_prompt` | unit |
| TC-COMPACT-3 | FR-COMPACT-3 | 요약 완료 | 다음 제출 | 메시지 수 유지, 요청엔 요약 사용 | `summary_replaces_history_in_next_request_only` | integration |

### 2.6 헤드리스 / `--help` 모드 (dogfood #1)

| FR | Pri | 요구사항 |
|---|---|---|
| FR-CLI-1 | Must | `dmuxai --help`, `--version`은 TTY 없이 출력 후 exit 0 |
| FR-CLI-2 | Must | stdout이 TTY가 아니면 raw mode 진입 없이 명확한 오류(exit≠0) |
| FR-CLI-3 | Should | `dmuxai run --agent <a> --model <m> "<prompt>"` 헤드리스 실행 |

| TC | FR | Given | When | Then | Test name | Type |
|---|---|---|---|---|---|---|
| TC-CLI-1 | FR-CLI-1 | 파이프된 stdout | `dmuxai --help` | 사용법, exit 0 | `help_flag_prints_usage_without_tty` | integration |
| TC-CLI-2 | FR-CLI-2 | 파이프된 stdout | `dmuxai` | "requires a terminal" 류 오류, os error 6 아님 | `non_tty_start_fails_with_clear_message` | integration |
| TC-CLI-3 | FR-CLI-3 | 가짜 어댑터 | `dmuxai run …` | stdout에 응답, exit 0 | `headless_run_prints_agent_response` | integration |

### 2.7 상태줄 잘림 (dogfood #5)

| FR | Pri | 요구사항 |
|---|---|---|
| UX-STATUS-1 | Should | 좁은 폭에서 우선순위 낮은 항목부터 생략, 핵심 키 힌트(`/quit`, Esc)는 유지 |

| TC | FR | Given | When | Then | Test name | Type |
|---|---|---|---|---|---|---|
| TC-STATUS-1 | UX-STATUS-1 | TestBackend 폭 80/120/200 | 렌더 | 상태줄이 폭 안에 들어가고 마지막 힌트 보임 | `status_line_fits_narrow_widths` | unit |

### 2.8 AGY F2 느림 (dogfood #4)

| FR | Pri | 요구사항 |
|---|---|---|
| NFR-MODELS-1 | Should | 모델 목록 캐시: 두 번째 F2는 즉시(<200ms) 표시, 백그라운드 갱신 |

| TC | FR | Given | When | Then | Test name | Type |
|---|---|---|---|---|---|---|
| TC-MODELS-1 | NFR-MODELS-1 | 느린 가짜 models 명령 | F2 두 번 | 두 번째는 캐시에서 즉시 | `model_catalog_is_cached_per_runner` | unit |

### 2.9 Skills discovery (M3)
기존 `FR-GRAPH-7` → `discover_skills_finds_agent_graph_engineering`만 존재.

| FR | Pri | 요구사항 |
|---|---|---|
| FR-SKILL-1 | Must | 프론트매터(name, description) 파싱, 잘못된 SKILL.md는 건너뛰고 경고 |
| FR-SKILL-2 | Must | 노드/턴에 바인딩된 스킬 내용이 요청에 포함 |

| TC | FR | Given | When | Then | Test name | Type |
|---|---|---|---|---|---|---|
| TC-SKILL-1 | FR-SKILL-1 | 정상 1개 + 깨진 1개 | discover | 정상만 반환 | `malformed_skill_is_skipped` | unit |
| TC-SKILL-2 | FR-SKILL-2 | 스킬 바인딩된 노드 | 요청 생성 | 프롬프트에 SKILL.md 참조 | `bound_skill_is_injected_into_node_prompt` | unit |

### 2.10 Graph gates / override / persistence / resume (M1)
기존: `gate_pauses_until_continue`, `store_round_trip`. 미커버: override, 재시작 후 resume(PR #4 리뷰에서 지적), 병렬/재시도 상한.

| FR | Pri | 요구사항 |
|---|---|---|
| FR-GRAPH-8 | Must | `/graph override <node> <runner> <model>`은 러너별 정책 검증 후 적용 |
| FR-GRAPH-9 | Must | 앱 재시작 시 `.dmuxai/graphs/`의 미완료 그래프를 resume 가능 |
| NFR-GRAPH-2 | Must | 병렬 노드 ≤2, 재시도 ≤2 기본값이 실행기에서 강제됨 |

| TC | FR | Given | When | Then | Test name | Type |
|---|---|---|---|---|---|---|
| TC-GRAPH-08 | FR-GRAPH-8 | 계획된 그래프 | 허용/금지 모델로 override | 적용 / 거부 메시지 | `graph_override_validates_runner_policy` | unit |
| TC-GRAPH-09 | FR-GRAPH-9 | 저장된 진행 중 그래프 | 새 App 인스턴스 | resume 제안, 완료 노드 건너뜀 | `graph_resumes_after_restart` | integration |
| TC-GRAPH-10 | NFR-GRAPH-2 | 독립 노드 4개, 항상 실패 노드 | 실행 | 동시 ≤2, 재시도 2회 후 실패 | `executor_caps_parallelism_and_retries` | unit |
| TC-GRAPH-11 | UX-GRAPH-1 | 다단계 프롬프트 | `/graph` | 노드별 러너·모델 요약 표시 | live checklist L-08 | live-TUI |

### 2.11 Windows + WSL 런처 (M6, 미래)

| FR | Pri | 요구사항 |
|---|---|---|
| FR-WIN-1 | Must | Windows 앱이 WSL 배포판에서 dmuxai를 실행, 작업 폴더를 Linux 경로로 변환 |
| FR-WIN-2 | Must | WSL 없음/배포판 없음이면 설치 안내와 함께 실패 |
| FR-WIN-3 | Should | WSL에서 Ctrl+V가 `powershell.exe` 클립보드 이미지로 동작 |

| TC | FR | Given | When | Then | Test name | Type |
|---|---|---|---|---|---|---|
| TC-WIN-1 | FR-WIN-1 | `C:\src\x` | 경로 변환 | `/mnt/c/src/x` 또는 설정된 Linux 경로 | `windows_path_maps_to_wsl_path` | unit |
| TC-WIN-2 | FR-WIN-2 | `wsl.exe` 없음 | 실행 | 안내 메시지, exit≠0 | `launcher_reports_missing_wsl` | unit |
| TC-WIN-3 | FR-WIN-3 | Windows 클립보드에 이미지 | Ctrl+V | `[Image #1]` 칩 | WSL checklist W-03 | live-TUI |

### 2.12 기타 미커버
- **Graph execution report (M8)**: PR마다 `docs/graph-runs/<topic>.md` 존재 여부 검사 없음 → `NFR-REPORT-1`(Should), TC-REPORT-1 `graph_run_report_exists_for_branch`(CI 스크립트, integration).
- **Esc/Ctrl+C 종료 동작**: `escape_never_exits_the_application`은 있으나 Ctrl+C 정책이 SPEC에 없음 → `UX-QUIT-2`: Ctrl+C는 실행 중 턴 취소, 유휴 상태에서는 안내만(종료는 `/quit`). TC-QUIT-2 `ctrl_c_cancels_running_turn_without_exit`(unit).

### 2.13 갭 요약 (❌ 14)
Permissions SPEC 부재 · 색상/NO_COLOR SPEC 부재 · 러너별 모델 선택 · custom agent · /compact · 헤드리스/--help · 상태줄 잘림 · F2 느림 · skills 파싱/주입 · graph override · graph resume · 병렬/재시도 상한 · Windows/WSL 런처 · graph-run report 검사.

## 3. 수동 live-TUI 체크리스트

환경: Linux 박스 xfce4-terminal 또는 `tmux new -s dmux` (캡처: `tmux capture-pane -p -e`), 이후 WSL(Windows Terminal). 각 회차마다 커밋 SHA, 터미널, 폭, `NO_COLOR` 값을 기록한다.

| # | 항목 | 절차 | 기대 결과 | Linux | WSL |
|---|---|---|---|---|---|
| L-01 | 시작 | 임시 workspace에서 `dmuxai` | 오류 없이 TUI, 최근 대화 또는 새 대화, 상태줄 표시 | ☐ | ☐ |
| L-02 | `/mode` | `/mode` → Ask/Plan/Implement 전환 | 상태줄 모드 변경, Plan에서 쓰기 시도 없음 | ☐ | ☐ |
| L-03 | Ctrl+V 이미지 | 클립보드에 PNG(`xclip -selection clipboard -t image/png -i a.png`) 후 Ctrl+V | `Attachments: [Image #1]`, `.dmuxai/attachments/`에 PNG 저장 | ☐ | ☐ |
| L-04 | Alt+V 이미지 | 같은 절차를 Alt+V로 | 동일, 비이미지 클립보드면 안내만 | ☐ | ☐ |
| L-05 | 색상 (NO_COLOR) | `NO_COLOR=1 dmuxai` → 안내 확인, 다시 `NO_COLOR=1 DMUXAI_FORCE_COLOR=1 dmuxai` | 첫 실행: 무색+안내. 두 번째: You 하늘색, Cursor 초록, AGY 자주, 시스템 회색 | ☐ | ☐ |
| L-06 | `/agent` 전환 + F2 유지 | AGY 선택 → F2 Gemini Flash → Cursor → AGY로 복귀 후 전송 | AGY 모델이 Gemini Flash 그대로, composer-2.5로 바뀌지 않음 | ☐ | ☐ |
| L-07 | F2 응답 시간 | AGY에서 F2 | 목록 표시 시간 기록(현재 ~20s, 목표 캐시 후 즉시) | ☐ | ☐ |
| L-08 | `/graph` | 다단계 프롬프트(조사+구현+테스트) 전송 후 `/graph` | 노드·러너·모델 요약, Cursor 노드는 composer-2.5 | ☐ | ☐ |
| L-09 | `/quit` | `/quit` 또는 `/q` 미리보기에서 Enter | 즉시 종료, 터미널 복구(raw mode 해제) | ☐ | ☐ |
| L-10 | Esc / Ctrl+C | 유휴/실행 중 각각 Esc, Ctrl+C | Esc는 종료하지 않고 오버레이만 닫음. Ctrl+C 동작 기록(정책 UX-QUIT-2 확정 전) | ☐ | ☐ |
| L-11 | 다중 인스턴스 | 같은 workspace에서 창 2개 실행 | 서로 다른 대화, 공유 목록에 둘 다 보임, DB 잠금 오류 없음 | ☐ | ☐ |
| L-12 | custom agent (구현 후) | opencode 예시 설정, `opencode/glm-5.3-flash-free`로 "PONG만 답해" | 응답 수신, opencode 전용 색 | ☑ (2026-10-17 tmux, PONG 주황색) | ☐ |
| L-13 | 좁은 폭 | 폭 80으로 줄임 | 상태줄 잘림 기록(#5) | ☐ | ☐ |

판정: 모든 행 Pass(또는 "기록" 항목은 값 기록) → 체크리스트 통과. 실패 시 `docs/dogfood/README.md`에 번호를 붙여 기록.

## 4. Release gate — Windows 앱 착수 조건

다음이 모두 녹색이어야 Windows/WSL 런처 작업(§2.11)을 시작한다.

1. CI `Verify` 전부 통과: `Spec traceability`, `cargo fmt --check`, `cargo test --all-targets`, `cargo clippy -D warnings`.
2. 기존 SPEC의 모든 **Must** 요구사항이 ✅ 자동 테스트에 매핑됨: 현재 ⚠️ 19건 중 Must에 해당하는 항목(특히 cross-agent sharing TC-001~007, TC-045, NFR-GRAPH-1, UX-GRAPH-1)을 해소.
3. §2의 Must 갭 중 Linux 범위를 SPEC화하고 자동 테스트 추가: FR-PERM-*, FR-THEME-1/2, FR-MODEL-*, FR-CUSTOM-1~4, FR-COMPACT-*, FR-CLI-1/2, FR-GRAPH-8/9, NFR-GRAPH-2, FR-SKILL-*.
4. §3 수동 체크리스트 L-01~L-12가 Linux에서 Pass(최근 main 기준, 결과를 `docs/dogfood/` 또는 PR에 기록).
5. 모델 정책: `tests/model_policy_repo.rs` 통과, 그래프 기본값 composer-2.5 유지.
6. dogfood 열린 항목 중 #1(헤드리스/--help)은 해결, #4/#5는 해결 또는 명시적 보류 결정.
