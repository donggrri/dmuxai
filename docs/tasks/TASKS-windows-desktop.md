# TASKS: Windows desktop launcher (DmuxAI inside WSL)

Status: in-progress  
Spec: [SPEC-windows-desktop.md](../specs/SPEC-windows-desktop.md)

- [x] T1 - Implement `Probe` trait and fake probe. Requirements: FR-390. Verify: `cargo test launcher_builds_windows_terminal_command`.
- [x] T2 - Implement `LauncherConfig` serialization/deserialization and error recovery. Requirements: FR-391, FR-392. Verify: `cargo test launcher_remembers_distro_and_workspace`.
- [x] T3 - Implement distro selection logic (`choose_distro`). Requirements: FR-391. Verify: `cargo test launcher_prompts_when_distro_ambiguous`.
- [x] T4 - Implement `windows_to_wsl_path` with UNC rejection. Requirements: FR-392. Verify: `cargo test windows_path_maps_to_wsl_path`.
- [x] T5 - Implement first-run diagnostics (`run_checks`). Requirements: FR-393. Verify: `cargo test launcher_reports_missing_wsl`, `cargo test launcher_reports_missing_dmuxai_in_distro`, and `cargo test launcher_reports_missing_agent_clis_non_blocking`.
- [x] T6 - Implement `/mnt/` gate and warning check (`mnt_gate`). Requirements: FR-394. Verify: `cargo test mnt_path_shows_performance_warning`.
- [x] T7 - Implement distro version verification (`version_gate`). Requirements: FR-398. Verify: `cargo test launcher_detects_stale_wsl_dmuxai_version`.
- [x] T8 - Implement launch argv builder (`build_launch_command`). Requirements: FR-390, NFR-390. Verify: `cargo test launcher_builds_windows_terminal_command`, `cargo test launcher_falls_back_without_windows_terminal`, and `cargo test launcher_never_assembles_shell_command_string`.
- [x] T9 - Create `src/bin/dmuxai-launcher.rs` with `RealProbe` on Windows and unsupported stub elsewhere. Requirements: FR-390. Verify: compilation of `dmuxai-launcher`.
- [x] T10 - Implement `packaging/windows/install.ps1`, `packaging/windows/uninstall.ps1`, and `packaging/windows/README-windows.txt`. Requirements: FR-396. Verify: `cargo test --test windows_packaging`.
- [x] T11 - Implement `scripts/package-windows.sh` and update `.github/workflows/release.yml`. Requirements: FR-396, FR-397. Verify: `cargo test --test windows_packaging`.
- [x] T12 - Add static assertion test suite in `tests/windows_packaging.rs`. Requirements: FR-396. Verify: `cargo test --test windows_packaging`.
- [x] T13 - Add manual W-01..W-06 checklist rows to `docs/TEST-PLAN.md` §3. Requirements: FR-395, FR-396, UX-390. Verify: manual inspection in `docs/TEST-PLAN.md`.
