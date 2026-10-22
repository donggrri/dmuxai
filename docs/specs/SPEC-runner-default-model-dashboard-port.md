# SPEC: Per-runner default model and dashboard port fallback

Status: approved  
Owner: dmuxai  
Baseline: `main` (`1b9861a`)

## Problem

1. A Grok (or any non-Cursor) turn sent from the TUI or dashboard without an
   explicit model failed with `model composer-2.5 is not allowed for grok`.
   `App::selected_run_model` and `graph::fallback_plan` fell back to
   `model_policy::default_model()` (`composer-2.5`, a Cursor model) for every
   runner, then rejected it for Grok. Headless `run --agent grok` was not
   affected, which is why the round-5 ACP checks did not catch it.
2. `dmuxai --web --port N` with `N` already held by another process left the
   dashboard unstarted (only a chat notice). A browser on `N` reached the other
   process and hung on "connecting" / `GET /api/state`.

## 4. Requirements

| ID | Priority | Given | Then | Edge cases | Tests |
| --- | --- | --- | --- | --- | --- |
| FR-1460 | Must | A turn on runner R with no explicitly selected model | The model is R's own default: `composer-2.5` (non-fast) for Cursor; `provider default` (no `--model`, no select) for every other runner | Never falls back to another runner's model and never to a fast model | TC-1460, TC-1461 |
| FR-1461 | Must | The 1-node fallback plan for runner R with no valid hint | The node model is R's default per FR-1460 instead of an error | A disallowed explicit selection is still rejected | TC-1462 |
| FR-1462 | Must | `--web`/`serve` with an explicit `--port N` already in use | The dashboard binds a free port and reports a warning naming N and the real URL | Non-AddrInUse bind errors still fail as before | TC-1463 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement | Command |
| --- | --- | --- |
| TC-1460 | FR-1460 | `cargo test tc_1460_grok_follow_up_without_model_uses_provider_default` |
| TC-1461 | FR-1460 | `cargo test tc_1461_default_model_for_runner_is_per_runner` |
| TC-1462 | FR-1461 | `cargo test tc_1462_fallback_plan_uses_runner_default` |
| TC-1463 | FR-1462 | `cargo test tc_1463_explicit_port_in_use_falls_back_with_warning` |
| TC-1485 | FR-1460 | Manual: TUI on a grok conversation with no model picked; type a follow-up and press Enter. Pass: grok replies; no "composer-2.5 is not allowed for grok" |
| TC-1486 | FR-1460 | Manual: same follow-up sent from the web dashboard (`POST /api/prompt` with `agent=grok` or the composer). Pass: grok reply appears in the dashboard chat |
| TC-1487 | FR-1462 | Manual: hold port N with another `--web`; start `dmuxai --web --port N`. Pass: warning names N and the new port; the new URL loads and shows "live" |

## Root causes

- FR-1460/1461: `App::selected_run_model` (`src/app.rs`) used
  `model_policy::default_model()` for every runner with no selection, and
  `graph::fallback_plan` did the same. Both now use
  `model_policy::default_model_for_runner`, and `validate_model_for_runner`
  accepts the `provider default` sentinel for non-Cursor runners only.
- FR-1462: `web::server::start_server` only fell back to a free port for the
  default port 7420; an explicit `--port` in use returned an error that the
  TUI showed as one chat notice while the dashboard stayed down.
