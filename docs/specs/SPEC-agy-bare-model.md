# SPEC-agy-bare-model

Status: Implemented

## 1. Decision summary
AGY lists its models only with an effort suffix (`gemini-3.8-flash-low`, `-medium`, `-high`).
Selecting the bare name `gemini-3.8-flash` (dashboard `select_model`, API) failed with
`unknown model 'gemini-3.8-flash' for agy`. dmuxai now maps a bare AGY model name to its
`-medium` variant when that variant is in the catalog. The ACP client also reports the crate
version instead of a hard-coded string.

## 2. Scope
In scope: AGY model selection (`SessionCommand::SelectModel`), `src/agent/acp/client.rs`
`CLIENT_VERSION`. Out of scope: other runners' model names; model policy (bare names already pass
`validate_model_for_runner`).

## 4. Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-1493 | Must | For the agy runner, a model id with no exact catalog match resolves to `<id>-medium` when that id is in the catalog; the selected model is the `-medium` id. Exact matches win. Other runners are unchanged, and an id with no exact or `-medium` match still fails with `unknown model`. |
| FR-1494 | Must | `CLIENT_VERSION` in the ACP `initialize` request equals `env!("CARGO_PKG_VERSION")`. |

## 5. Acceptance scenarios and test design
| TC | Covers | Test |
|---|---|---|
| TC-1493 | FR-1493 | Automated: `agy_bare_model_resolves_to_medium` (bare -> `-medium`; exact match kept; unknown still errors; non-agy unchanged). Manual: in `dmuxai --web`, `POST /api/command {"command":"select_model","agent":"agy","model":"gemini-3.8-flash"}` succeeds, then an agy prompt runs and replies. Expected: no `unknown model` error; agy model shows `gemini-3.8-flash-medium`. |
| TC-1494 | FR-1494 | Automated: `acp_client_version_matches_crate` |
