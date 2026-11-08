# REVIEW: dashboard-token-docs

Spec: [SPEC-dashboard-token-docs](../specs/SPEC-dashboard-token-docs.md)
Plan: [PLAN-dashboard-token-docs](../plans/PLAN-dashboard-token-docs.md)

## Spec compliance

| Requirement | Status | Evidence |
| --- | --- | --- |
| FR-2513 | Met | `tc_2513_readme_documents_dashboard_token` |
| FR-2514 | Met | `tc_2514_security_documents_dashboard_token` |
| FR-2515 | Met | `tc_2515_bug_report_reminds_redact_dashboard_tokens` |

## ID renumber note

The three requirement IDs and three test IDs were originally drafted as
FR-2450..FR-2452 / TC-2450..TC-2452 and collided with the already merged
#258 spec (SPEC-opencode-workspace-cwd); they were renumbered to
FR-2513..FR-2515 / TC-2513..TC-2515 per the coordinator reservations.

## Residual risks

- Users may still paste the startup line without reading the docs; the bug-report template only nudges reporters who use the form.

## Test evidence

Refreshed in this completion pass (see "Validation" below).

## Validation (refreshed 2026-11-08, deterministic only)

Model attribution: originally drafted with Composer (dmuxai); this completion
pass (spec-ID renumber and review refresh only) was performed by the GLM free
model through dmuxai.

- `cargo fmt --all -- --check`: pass
- `cargo test --test dashboard_token_docs`: 3 passed (TC-2513, TC-2514, TC-2515)
- `bash scripts/check-spec.sh docs/specs/SPEC-dashboard-token-docs.md`: pass
- `python3 scripts/check-spec.py --all`: pass, no collisions for FR/TC-2513..2515
- `python3 scripts/check-tc-links.py`: pass, 0 orphan / 0 undefined-test / 0 stale
- `bash scripts/gate.sh --quick`: pass (fmt, `cargo test --test spec_index` incl.
  `spec_ids_are_unique_across_specs`, check-spec --all, check-tc-links)

Not rerun in this pass (kept from the original PR; docs-only change, no runtime
effects): full `cargo test --all-targets`, clippy, live-provider smoke, browser
tests.
