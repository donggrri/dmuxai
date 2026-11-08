# SPEC: security-reporting

Status: Implemented
Owner: Cloud Agent
Created: 2026-11-07
Baseline: `main`
Related: GitHub issue #188

## 1. Decision summary

`SECURITY.md` told reporters to use GitHub private vulnerability reporting as if it were always
enabled. When the feature is off, that guidance is wrong and invites public disclosure. The
policy now names the **Security** tab **Report a vulnerability** form as the preferred channel,
documents a no-details public-issue fallback, and adds a release step to confirm reporting is
enabled.

## 2. Scope

### In scope

- `SECURITY.md` reporting instructions and fallback behavior.
- Release checklist item in `docs/RELEASE-PROCESS.md`.
- Automated documentation oracles in `tests/security_reporting.rs`.
- Brief `CONTRIBUTING.md` alignment pointing at `SECURITY.md`.

### Out of scope / non-goals

- Enabling private vulnerability reporting on GitHub (maintainer settings).
- Inventing email, chat, or other off-GitHub contact endpoints.
- Changing application security behavior or CI secret scanning.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-001 | Maintainers can enable private vulnerability reporting in repository settings. | Maintainers | Each release |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Private vulnerability reporting | GitHub feature that powers the **Report a vulnerability** form. | A public issue with full reproduction steps. |
| Fallback public issue | Minimal issue asking for a private channel without vulnerability details. | A security advisory or full public report. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2516 | Must | A reporter reads `SECURITY.md` | The doc names **Security** → **Report a vulnerability** as the preferred channel and warns against public disclosure of details | Missing guidance sends reporters to the wrong channel | TC-2516 |
| FR-2517 | Must | The **Report a vulnerability** form is unavailable | `SECURITY.md` instructs a minimal public issue that requests a private channel and forbids vulnerability details; the reporting section does not list mailto links or standalone contact emails | Reporters disclose secrets in public issues | TC-2516 |
| FR-2518 | Must | A maintainer prepares a release using `docs/RELEASE-PROCESS.md` | Step 0 includes verifying private vulnerability reporting is enabled (or updating policy before ship) | Releases ship with broken reporting guidance | TC-2517 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2516 | FR-2516, FR-2517 | unit | Repository checkout | Read `SECURITY.md` reporting section | Preferred form, fallback without details, no mailto/email contacts in that section | `cargo test tc_2516_security_md_documents_reporting_channels` |
| TC-2517 | FR-2518 | unit | Repository checkout | Read `docs/RELEASE-PROCESS.md` step 0 | Release checklist mentions verifying private vulnerability reporting | `cargo test tc_2517_release_process_checks_private_vulnerability_reporting` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No change | N/A | — |
| Provider protocol | No change | N/A | — |
| Security / permissions | Documentation only | Revert doc commit | TC-2516, TC-2517 |
| Platform | GitHub UI availability | Fallback path when form missing | TC-2516 |

## 7. Implementation plan

Link to `../plans/PLAN-security-reporting.md` and `../tasks/TASKS-security-reporting.md`.

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Policy and release checklist | `SECURITY.md`, `docs/RELEASE-PROCESS.md`, `CONTRIBUTING.md` | FR-2516–FR-2518 | TC-2516, TC-2517 |
| 2 | SPEC and review artifacts | `docs/specs/SPEC-security-reporting.md`, plan, tasks, review | FR-2516–FR-2518 | `python3 scripts/check-spec.py --all` |

## 8. Review and test exit criteria

- [x] All Must requirements map to passing evidence.
- [x] The diff has been reviewed against scope and non-goals.
- [x] Doc tests and spec gates pass.
- [x] `cargo fmt` run on touched Rust tests.
