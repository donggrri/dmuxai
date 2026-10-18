# REVIEW: v0.2.2 specs

Date: 2026-10-18 (Asia/Seoul). Reviewer: opencode-haiku (read-only, `--mode ask`), four short rounds.
Specs: SPEC-centered-prompt (#54), SPEC-session-sidebar (#55), SPEC-workspace-dir (#56).

## Round 1 — REQUEST_CHANGES (4 findings, all fixed)

1. FR-851: the left dock was 280px and the right 320px, so "both open = viewport center" was false. Both docks are now 320px.
2. FR-874: the busy check was not atomic with the re-exec, and the argv could carry two `-C` flags. Now a switch-pending flag is set under the same App lock as the busy check, and new turns, graph runs and to-do runs get 409. The re-exec drops any existing `-C`/`--workspace-dir` before adding one `-C`.
3. FR-872/NFR-870: the browse scope was undefined and the deny list was short. Browsing is now limited to `$HOME` plus `[workspace] browse_roots`. Each child is canonicalized and dropped if denied. The deny list adds `~/.docker`, `~/.kube`, `~/.password-store`, keyrings, and the runner credential directories.
4. FR-860: the list order was ambiguous. It is now the active conversation first, then `updated_at` descending, asserted in TC-861 and TC-863.

## Rounds 2-3 — REQUEST_CHANGES (fixed)

- Exec failure after the 202 could leave the switch-pending flag stuck, and `--workspace-dir=DIR` was not stripped from the argv. A preflight (`current_exe`, re-validation) now runs before shutdown. A failed preflight clears the flag and shows a notice. A failed `exec` after shutdown exits with code 1 and prints the restart command. TC-875 covers both.

## Round 4 — APPROVE
