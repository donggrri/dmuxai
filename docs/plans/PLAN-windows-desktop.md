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
11. Implement Windows per-user installation and uninstallation scripts (`packaging/windows/install.ps1`, `packaging/windows/uninstall.ps1`, `packaging/windows/README-windows.txt`). (FR-396)
12. Implement cross-compilation packaging script `scripts/package-windows.sh` and update `.github/workflows/release.yml`. (FR-396, FR-397)
13. Implement automated packaging test `tests/windows_packaging.rs`. (FR-396, TC-405)
14. Define manual W-rows in `docs/TEST-PLAN.md` §3 (W-01..W-06). (FR-395, UX-390, TC-400, TC-404)

## Risks and mitigations

- `wslpath` output or distro list encoding on Windows may vary (UTF-16LE / BOM); decode and trim command output cleanly.
- Argument injection if shell strings were used; strictly adhere to argv vector arrays without shell expansion (NFR-390).
- Corrupt launcher configuration shouldn't panic or block execution; fallback to defaults gracefully.
- Cross-compilation requires MinGW GCC; fail fast with clear instructions if mingw toolchain is missing.
- PowerShell installer must avoid `Invoke-Expression` and always use `-LiteralPath` to prevent injection or path escaping bugs.

## Rollback

Reverting `src/launcher/`, `src/bin/dmuxai-launcher.rs`, and `packaging/windows/` restores the pre-launcher state without affecting the core Linux TUI or database schema.
