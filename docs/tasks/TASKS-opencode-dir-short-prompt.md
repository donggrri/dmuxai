# TASKS: opencode without `--dir`, judge skipped for short prompts

- [x] T1 — FR-1610. Test: `cargo test tc_1610_opencode_preset_has_no_dir_flag`
- [x] T2 — FR-1611. Test: `cargo test tc_1611_opencode_dir_pair_dropped`
- [x] T3 — FR-1613. Test: `cargo test tc_1613_short_prompt_skips_judge`
- [x] T4 — Gates: `cargo fmt --check`, `cargo test --all-targets`, `cargo clippy --all-targets --all-features -- -D warnings` (Linux and `x86_64-pc-windows-gnu`). Manual: TC-1612 (dashboard, opencode glm free model behind a wrapper that rejects `--dir`) and TC-1614 (dashboard, agy `hi`).
