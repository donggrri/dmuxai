# REVIEW: opencode without `--dir`, judge skipped for short prompts

FR-1610/FR-1611: the opencode preset and its documented examples no longer pass `--dir`; the workspace is the child's cwd, and `build_argv` drops a user-configured `--dir {workspace}` pair for programs named `opencode`/`opencode.exe` only. Other programs and other `--dir` values are unchanged. FR-1613: a one-line prompt of at most 3 words and 40 chars that is not a slash command skips the judge turn and runs at once with source `heuristic` and reason `short prompt`; longer prompts still go to the judge and `/plan` still forces planning.

Automated: TC-1610 (`tc_1610_opencode_preset_has_no_dir_flag`), TC-1611 (`tc_1611_opencode_dir_pair_dropped`) and TC-1613 (`tc_1613_short_prompt_skips_judge`) pass together with the full `cargo test --all-targets` suite; `cargo fmt --check` is clean and `cargo clippy --all-targets --all-features -- -D warnings` is clean on Linux and `x86_64-pc-windows-gnu`.

Manual TC-1612 PASS: dashboard, opencode glm free model behind a wrapper that rejects `--dir` exactly like the reported version; `pwd` was answered with the workspace path. Manual TC-1614 PASS: agy `hi` now starts one agy process instead of two and was answered in about 9 s on the dashboard; a follow-up question took about 24 s with 6 tool calls, versus the 2+ minutes and 18 tool calls reported earlier.

Residual risk: agy cold-starts a new process per turn, and each non-trivial prompt still pays one extra judge turn. Ask-like questions in Implement mode run with `--dangerously-skip-permissions` and may use many tools.
