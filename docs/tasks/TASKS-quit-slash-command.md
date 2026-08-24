# TASKS: Slash-command quit

Status: complete  
Spec: [SPEC-quit-slash-command.md](../specs/SPEC-quit-slash-command.md)

- [x] T1 - Add `/quit` to slash preview and exact submission routing.
  Requirements: UX-011. Verify: `cargo test exact_quit_command_exits_without_starting_a_turn`,
  `cargo test slash_preview_can_execute_quit_with_enter`, and
  `cargo test slash_preview_can_execute_quit_with_tab`.
- [x] T2 - Remove only the main-prompt `Esc` exit transition; preserve overlay
  and preview dismissal. Requirements: UX-012. Verify:
  `cargo test escape_never_exits_the_application`.
- [x] T3 - Protect the whitespace boundary and update user-facing controls and
  baseline traceability. Requirements: UX-011, UX-012. Verify:
  `cargo test quit_with_whitespace_is_regular_prompt_text` and the spec check.
- [x] T4 - Run all required deterministic Rust gates and write the review.
  Requirements: UX-011, UX-012. Verify: `cargo fmt --check`,
  `cargo test --all-targets`, and `cargo clippy --all-targets --all-features -- -D warnings`.
