# SPEC: Disable Cursor Implement mode on Windows

Status: complete  
Owner: DmuxAI  
Created: 2026-08-24  
Baseline: `675d9df`  
Related: baseline `INT-002`, `UX-002`, `INV-005`

## 1. Decision summary

Cursor's Implement mode currently launches with `--sandbox enabled`, but Cursor
does not provide that sandbox on Windows. DmuxAI will reject Cursor Implement
mode on Windows before a mode change or provider process can start. The UI will
show an actionable platform-specific error, while Cursor Ask and Plan and all
non-Cursor agent modes retain their existing behavior.

## 2. Scope

### In scope

- Validate Cursor mode capability before changing the selected mode.
- Guard the Cursor runtime before command construction and process spawning.
- Preserve existing Cursor Ask/Plan and supported-platform Implement flags.
- Add deterministic Windows and cross-platform regression coverage.
- Document the platform limitation in the baseline and README.

### Out of scope / non-goals

- Replacing Cursor sandbox mode with an unsandboxed or allowlist mode.
- Changing Codex or AGY Implement behavior.
- Changing Cursor Ask or Plan behavior.
- Installing, configuring, or authenticating Cursor Agent.
- Making Windows support Cursor Implement through a provider upgrade.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-001 | None. The reported Cursor error identifies sandbox mode as unavailable on Windows, so rejecting only Cursor Implement is the smallest safe behavior change. | DmuxAI | N/A |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Cursor Implement | Cursor's write-capable mode represented by `--force --sandbox enabled`. | DmuxAI Implement mode for Codex or AGY |
| Mode validation | A side-effect-free capability check performed before UI persistence or command queueing. | Provider process execution |
| Blocked mode | Cursor Implement on Windows; it has no provider process or active mode transition. | A failed provider turn after a process starts |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| INT-060 | Must | Given Windows and Cursor is idle, when the user attempts to select Implement. | DmuxAI rejects the mode before changing Cursor's selected mode or retiring its provider binding, and displays that Cursor Implement requires macOS or Linux sandbox support. | The mode picker remains available so the user can choose Ask or Plan; no provider process starts. | TC-060 |
| NFR-060 | Must | Given any Cursor Implement request reaches the adapter runtime. | The request is rejected before `cursor_command`, command construction, or child-process spawning; Windows never invokes Cursor with `--sandbox enabled`. | The correlated request receives a terminal error and the runtime remains usable for later Ask/Plan requests. | TC-061 |
| INT-061 | Must | Given a supported non-Windows platform and Cursor is selected. | Cursor Implement continues to use `--force --sandbox enabled`; Ask continues to use `--mode ask` and Plan uses `--mode plan`. | No mode gains broader permissions or changes provider protocol shape. | TC-062, TC-063 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-060 | INT-060 | unit | Construct a Cursor adapter mode request on the current target platform. | Validate and attempt Cursor Implement selection. | Windows returns the platform-specific `UnsupportedMode` error and does not queue a mode-change event; supported platforms accept it. The UI does not retire the binding or change mode state. | `src/agent/cursor.rs`: `cursor_mode_validation_matches_platform`; Windows `windows_cursor_implement_mode_is_rejected_before_queueing`; `src/app.rs`: `unsupported_mode_is_rejected_before_binding_retirement` |
| TC-061 | NFR-060 | unit | Construct a Cursor Implement turn request on Windows with an unusable working directory. | Call the adapter's turn runner directly. | It returns the platform error before any provider event or process-spawn error can occur. | `src/agent/cursor.rs`: `windows_cursor_implement_turn_is_rejected_before_process_spawn` |
| TC-062 | INT-061 | unit | Build Cursor command arguments for Ask, Plan, and Implement. | Inspect the constructed arguments on a supported target. | Existing flags remain exact, including Implement's `--force --sandbox enabled`. | `cargo test cursor_execution_modes_use_native_cli_flags` |
| TC-063 | INT-061 | boundary | Validate Cursor Ask and Plan on every target. | Select each mode through the adapter capability check. | Both modes remain accepted on Windows and non-Windows targets. | `src/agent/cursor.rs`: `cursor_mode_validation_matches_platform` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No schema change; a rejected mode does not retire a provider binding. | Code/docs-only revert; existing conversations and bindings remain readable. | TC-060 |
| Provider protocol | Windows must not launch the Cursor Implement command; supported platforms keep the existing command flags and NDJSON protocol. | No provider protocol migration. | TC-061, TC-062 |
| Security / permissions | Do not fall back to unsandboxed Cursor execution or broaden permissions. | The safe fallback is user-visible rejection; Ask/Plan remain available. | TC-060, TC-061 |
| Platform | Cursor Implement is unavailable on Windows because Cursor sandbox mode requires macOS or Linux. | A future Cursor Windows sandbox capability can remove the guard in a separately specified change. | TC-060, TC-061 |

## 7. Implementation plan

See [PLAN-cursor-windows-implement-guard.md](../plans/PLAN-cursor-windows-implement-guard.md) and
[TASKS-cursor-windows-implement-guard.md](../tasks/TASKS-cursor-windows-implement-guard.md).

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Add provider mode validation and an actionable unsupported-mode error. | `src/agent/mod.rs`, `src/agent/cursor.rs` | INT-060, INT-061 | TC-060, TC-063 |
| 2 | Add the runtime pre-spawn guard and preserve supported command construction. | `src/agent/cursor.rs` | NFR-060, INT-061 | TC-061, TC-062 |
| 3 | Make the UI validate before retiring bindings or committing a mode change. | `src/app.rs` | INT-060 | TC-060 |
| 4 | Update product documentation and baseline traceability. | `README.md`, `docs/specs/DMUXAI-BASELINE.md` | all | spec check |

## 8. Review and test exit criteria

- [ ] All Must requirements map to passing deterministic evidence.
- [ ] Windows rejects Cursor Implement before provider process launch.
- [ ] Cursor Ask/Plan and supported-platform Implement behavior are unchanged.
- [ ] The baseline and README describe the Windows limitation.
- [ ] Required Rust format, all-target tests, and clippy checks pass.
- [ ] External-provider smoke tests are recorded as skipped unless authenticated CLIs are available.
- [ ] `../reviews/REVIEW-cursor-windows-implement-guard.md` records commands, results, and residual risk.
