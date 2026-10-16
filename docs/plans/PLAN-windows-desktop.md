# PLAN: Windows desktop launcher (DmuxAI inside WSL)

Status: in-progress  
Spec: [SPEC-windows-desktop.md](../specs/SPEC-windows-desktop.md)

1. Define `Probe` trait and fake probe for deterministic testing. (FR-390, TC-390, TC-391)
2. Implement `LauncherConfig` with JSON persistence and corrupt/missing file fallback to default. (FR-391, FR-392, TC-392)
3. Implement `choose_distro` selection logic (auto, remembered, picker, blocked). (FR-391, TC-392, TC-393)
4. Implement `windows_to_wsl_path` conversion rejecting UNC `\\wsl$` and `\\wsl.localhost` inputs. (FR-392, TC-394)
5. Implement `run_checks` first-run diagnostics: WSL status, distro presence, in-distro `dmuxai`, and non-blocking agent CLIs. (FR-393, TC-395..397)
6. Implement `mnt_gate` for `/mnt/` performance warning and confirmation bypass. (FR-394, TC-398)
7. Implement `version_gate` against `MIN_DMUXAI_VERSION` with fix-it for stale builds and non-blocking warning for unverified. (FR-398, TC-402)
8. Implement `build_launch_command` generating fixed argv arrays with Windows Terminal or fallback. (FR-390, NFR-390, TC-390, TC-391, TC-403)
9. Provide `RealProbe` and `src/bin/dmuxai-launcher.rs` entry point. (FR-390)
10. Verify via unit tests TC-390..398, TC-402, TC-403.

## Risks and mitigations

- `wslpath` output or distro list encoding on Windows may vary (UTF-16LE / BOM); decode and trim command output cleanly.
- Argument injection if shell strings were used; strictly adhere to argv vector arrays without shell expansion (NFR-390).
- Corrupt launcher configuration shouldn't panic or block execution; fallback to defaults gracefully.

## Rollback

Reverting `src/launcher/` and `src/bin/dmuxai-launcher.rs` restores the pre-launcher state without affecting the core Linux TUI or database schema.
