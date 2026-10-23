# SPEC: model-resolver

Status: Implemented

Owner: dmuxai
Created: 2026-10-24
Baseline: `d504f0a`
Related: #135, #136, #139, SPEC-model-policy, SPEC-agent-system-prompt

## 1. Decision summary

DmuxAI resolves the model for a turn through one lib-side resolver, `dmuxai::model_policy::ModelResolution::resolve`, instead of duplicated logic scattered across `app.rs`. Sources are tried in precedence order: Explicit (headless `--model` or an explicit request) > Selected (the model picked in the F2/model menu, `ModelCatalog.selected`) > ConfigDefault (a custom agent's `default_model`, or `[runners.<id>] model` from config) > RunnerDefault (`default_model_for_runner`, i.e. Cursor -> `composer-2.5`) > ProviderDefault (no model, provider picks). Every candidate is checked with the existing `validate_model_for_runner`; an invalid Explicit is an error, while an invalid Selected/ConfigDefault falls through to the next source. Triage (`triage.model` in `dmuxai run --json`), the status/judge line, graph assignment lines, the plan runner pin, and headless JSON all read the model from this resolver. Repeated `"provider default"` string literals in non-test code are replaced with `dmuxai::model_policy::PROVIDER_DEFAULT`. Bug #135 (custom agent `default_model` not shown in `triage.model`) and #139 (catalog check warning + `ModelUnavailable` hint) are fixed, and headless `--model M` (#136) plus per-runner `[runners.<id>] model` profiles are added. `validate_model_for_runner`, `validate_model_id`, `default_model_for_runner`, `ALLOWED_MODEL`, the agy flash-only rule, the fast-model block, Cursor's `composer-2.5` pin, and custom `allowed_models`/`validate_custom_model` are unchanged.

## 2. Scope

### In scope

- `ModelSource`, `ModelCandidates`, `ModelResolution::resolve`, `ModelResolution::label`, `CatalogCheck`, `check_catalog` in `src/model_policy.rs`.
- `App::model_resolution_for`, `App::selected_run_model`, `App::selected_run_model_label`, `App::effective_model_for`, `App::effective_model_for_catalogs`, `App::attach_model_catalog_hint`, the catalog check in `App::try_submit_to_agent`, the headless pre-load and `--model` validation in `App::run_headless`.
- Headless `--model` parsing in `src/cli.rs` and validation/exit code 5 handling.
- `[runners.<id>] model` parsing, validation, and warnings in `src/settings.rs` and `src/agent_prompt.rs` (`runner_models`, `runner_prompts`).
- Custom-agent `default_model` surfacing in `triage.model` and status/judge/graph lines.

### Out of scope / non-goals

- Changing `validate_model_for_runner`, `validate_model_id`, `default_model_for_runner`, `ALLOWED_MODEL`, the agy flash-only rule, the fast-model block, Cursor's `composer-2.5` pin, or custom `allowed_models`/`validate_custom_model`.
- Temperature and permission profiles (deferred; see OQ-1733).
- Removing model-selection features (model menu, F2 picker, `/api` model endpoints).
- Redefining TCs belonging to other specs.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-1733 | Temperature and permission profiles are deferred to a later PR. | dmuxai | Spec approval |
| OQ-1757 | A disallowed Selected model on a non-Cursor runner now falls through to the next source in `effective_model_for`, while `selected_run_model` still returns `Err` for it. | dmuxai | Spec approval |
| OQ-1758 | The headless catalog pre-load has no automated test yet because the custom-agent registry is a process-wide `OnceLock`. | dmuxai | Spec approval |
| OQ-1759 | `triage.model` reports the resolved model, but the resolution source (explicit, selected, etc.) is not yet present in the headless JSON. | dmuxai | Spec approval |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Explicit | Model from headless `--model` or an explicit request; invalid value is an error. | Selected |
| Selected | Model the user picked in the dashboard/TUI model menu (`ModelCatalog.selected`). | ConfigDefault |
| ConfigDefault | Custom agent `default_model`, or `[runners.<id>] model` from config. | RunnerDefault |
| RunnerDefault | `default_model_for_runner`: Cursor -> `composer-2.5`, others -> none. | ProviderDefault |
| ProviderDefault | No model (`None`); provider picks; displayed as `PROVIDER_DEFAULT` ("provider default"). | RunnerDefault |
| ModelResolution | `{ model: Option<String>, source: ModelSource }` result of the resolver. | `ModelCandidates` |
| CatalogCheck | `NoCatalog` / `Listed` / `NotListed { hint }` result of comparing a resolved model to a runner's cached model catalog. | Model policy |
| PROVIDER_DEFAULT | Constant `"provider default"` replacing repeated string literals. | `ALLOWED_MODEL` |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-1725 | Must | Given runner plus candidates `explicit`, `selected`, `config_default` | Resolve in order Explicit > Selected > ConfigDefault > RunnerDefault > ProviderDefault; return `ModelResolution { model, source }` | Empty candidate strings are skipped | TC-1734, TC-1737, TC-1755 |
| FR-1726 | Must | Given a candidate and `validate_model_for_runner` | Invalid Explicit returns `Err`; invalid Selected/ConfigDefault fall through to the next source; Cursor falls back to `composer-2.5` | Empty strings are treated as absent | TC-1735, TC-1736, TC-1741, TC-1742, TC-1744 |
| FR-1727 | Must | Given a turn, plan, or headless run | All consumers read the resolver; a custom agent with `default_model` shows that model in `triage.model`, status/judge line, graph assignment lines, and plan runner pin | "provider default" only when no model resolves | TC-1740, TC-1743, TC-1752 |
| FR-1728 | Must | Given non-test code needs the provider-default label | Use `dmuxai::model_policy::PROVIDER_DEFAULT` instead of the literal string | Constant value remains `"provider default"` | TC-1745 |
| FR-1729 | Must | Given a runner with a non-empty cached model catalog and a resolved model | Push a visible warning `model <m> is not offered by '<command>'; pick another model` (or the no-command variant) before the turn; if the turn then fails, attach the hint to the error as `AgentErrorKind::ModelUnavailable` | No catalog / empty catalog / no resolved model -> no check, no warning, no blocking | TC-1738, TC-1739, TC-1746, TC-1747, TC-1748 |
| FR-1730 | Must | Given `dmuxai run` or `dmuxai todo run` with `--model M` (requires `--agent`) | Validate with `validate_model_for_runner` / custom `allowed_models`; disallowed -> exit 5 before any turn; accepted model is Explicit source and echoed in `triage.model` | Missing value / `--model` without `--agent` / with `--graph` -> exit 2 | TC-1750, TC-1751, TC-1752 |
| FR-1731 | Must | Given `[runners.<id>] model = "..."` in config | Provide a ConfigDefault for built-in and custom runners; `[runners.default] model` is ignored with a warning; invalid value -> config warning and ignored | Unknown runner id -> warning and ignored | TC-1753, TC-1754, TC-1755 |
| FR-1732 | Must | Given the existing model policy | `validate_model_for_runner`, `validate_model_id`, `default_model_for_runner`, `ALLOWED_MODEL`, agy flash-only, fast block, Cursor pin, and custom `allowed_models`/`validate_custom_model` are unchanged | No regression in policy tests | TC-1736, TC-1756 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-1734 | FR-1725 | unit | candidate sets | `ModelResolution::resolve` | precedence explicit > selected > config > runner > provider | `src/model_policy.rs` `tc_1734_precedence_chain` |
| TC-1735 | FR-1726 | unit | Cursor candidates | resolve | invalid selected falls through to composer-2.5; invalid explicit errors | `src/model_policy.rs` `tc_1735_cursor_fallthrough_and_explicit_error` |
| TC-1736 | FR-1732 | unit | agy candidates | resolve | agy flash accepted; foreign rejected; fast blocked for all runners | `src/model_policy.rs` `tc_1736_agy_flash_and_fast_block` |
| TC-1737 | FR-1725 | unit | config/provider candidates | resolve | config default accepted; provider default label | `src/model_policy.rs` `tc_1737_config_default_and_provider_label` |
| TC-1738 | FR-1729 | unit | catalog inputs | `check_catalog` | no catalog variants and listed result | `src/model_policy.rs` `tc_1738_catalog_no_catalog_and_listed` |
| TC-1739 | FR-1729 | unit | not-listed inputs | `check_catalog` | exact warning text with and without command | `src/model_policy.rs` `tc_1739_catalog_not_listed_hint` |
| TC-1740 | FR-1727 | integration | custom agent with default_model | app turn helpers | model surfaces in selected_run_model, label, judge line, plan pin | `src/app.rs` `tc_1740_custom_agent_default_model_surfaces` |
| TC-1741 | FR-1726 | unit | selected disallowed model | selected_run_model | returns Err | `src/app.rs` `tc_1741_disallowed_selected_returns_err` |
| TC-1742 | FR-1725 | unit | Cursor, no selected | effective/resolve | resolves to composer-2.5 | `src/app.rs` `tc_1742_cursor_resolves_composer_2_5` |
| TC-1743 | FR-1727 | unit | custom agent | effective_model_for | uses custom default_model | `src/app.rs` `tc_1743_effective_model_custom_default` |
| TC-1744 | FR-1726 | unit | Cursor planned model | effective_model_for | valid planned used; invalid falls to composer-2.5 | `src/app.rs` `tc_1744_cursor_planned_model` |
| TC-1745 | FR-1728 | unit | Codex, no model | selected_run_model/label | None model and PROVIDER_DEFAULT label | `src/app.rs` `tc_1745_provider_default_label` |
| TC-1746 | FR-1729 | integration | missing default model | turn submit | exact hint pushed; turn still sent | `src/app.rs` `tc_1746_catalog_missing_pushes_warning` |
| TC-1747 | FR-1729 | integration | listed/empty catalog | turn submit | no message | `src/app.rs` `tc_1747_catalog_listed_or_empty_no_message` |
| TC-1748 | FR-1729 | integration | not-listed then failure | turn submit/drain | hint attached; error kind ModelUnavailable | `src/app.rs` `tc_1748_notlisted_error_gets_hint`, `src/app.rs` `tc_1748_drain_error_kind_is_model_unavailable` |
| TC-1750 | FR-1730 | unit | CLI args | parse | --model parses for run and todo run; usage errors | `src/cli.rs` `tc_1750_headless_model_parses` |
| TC-1751 | FR-1730 | integration | disallowed headless model | run_headless | policy rejection, exit 5, before any request | `src/app.rs` `tc_1751_headless_disallowed_model_policy` |
| TC-1752 | FR-1730 | integration | allowed headless model | run_headless | succeeds; model in triage.model | `src/app.rs` `tc_1752_headless_allowed_model_triage` |
| TC-1753 | FR-1731 | unit | settings with runner model | parse settings | accepted stored; invalid dropped with warning | `src/settings.rs` `tc_1753_runner_model_accepted_and_invalid_dropped` |
| TC-1754 | FR-1731 | unit | settings edge cases | parse settings | default ignored; custom accepted; unknown dropped | `src/settings.rs` `tc_1754_runner_model_default_custom_unknown` |
| TC-1755 | FR-1731 | integration | custom agent + config model | selected_run_model | precedence explicit > selected > config > custom default > runner default | `src/app.rs` `tc_1755_resolver_precedence_config_model` |
| TC-1756 | FR-1732 | integration | config model present | policy unchanged | validate still enforces policy | `src/app.rs` `tc_1756_policy_unchanged` |

## 6. Data, integration and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No DB schema change; model selection state stays in memory/catalogs and config | Existing conversations unaffected | TC-1740 |
| Provider protocol | No protocol change; resolver only changes which model string is passed | Unknown fields ignored | TC-1748 |
| Config | `[runners.<id>] model` validated at load; invalid -> warning and ignored; `[runners.default] model` ignored with warning | Existing keys unaffected | TC-1753, TC-1754 |
| Headless | `--model` validated before any turn; exit 5 on policy; accepted model echoed in `triage.model` | Usage errors remain exit 2 | TC-1750, TC-1751, TC-1752 |
| Catalog | Missing/empty catalog never blocks a turn; warning only when catalog present and model not listed | Interactive catalogs already loaded by adapter | TC-1746, TC-1747 |
| Security / privacy | Error hint surfaces model/catalog mismatch only; no secrets added | Existing policy unchanged | TC-1748, TC-1756 |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Resolver and catalog check primitives | `src/model_policy.rs` | FR-1725, FR-1726, FR-1728, FR-1729 | TC-1734 to TC-1739 |
| 2 | App wiring through resolver | `src/app.rs` | FR-1725, FR-1726, FR-1727, FR-1729 | TC-1740 to TC-1748 |
| 3 | Headless --model and CLI | `src/cli.rs`, `src/app.rs` | FR-1730 | TC-1750 to TC-1752 |
| 4 | Per-runner model profile | `src/settings.rs`, `src/agent_prompt.rs` | FR-1731 | TC-1753 to TC-1755 |
| 5 | Policy regression guard | `src/model_policy.rs`, `src/app.rs` | FR-1732 | TC-1736, TC-1756 |

## 8. Review and test exit criteria

- [ ] All Must requirements map to passing evidence.
- [ ] The diff has been reviewed against scope and non-goals.
- [ ] Required Rust checks pass: format, all targets test, and clippy.
- [ ] `../reviews/REVIEW-model-resolver.md` records commands, result, residual risk, and rollback notes.
