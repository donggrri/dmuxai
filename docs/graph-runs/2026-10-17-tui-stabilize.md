# Graph execution report: TUI stabilize (dogfood #1, #4, #5, #6)

| Node | Role | Runner / model | Output |
|---|---|---|---|
| spec | plan | DmuxAI (worktree `dmux-wt/tui-stabilize`, tmux `dmux-stab`) → AGY `gemini-3.8-flash-medium` | `docs/specs/SPEC-tui-stabilize.md` |
| cli | implement | same | `--help`, `--version`, unknown flag exit 2, clear no-TTY error (exit 1); `tests/cli.rs` |
| status/cache/paste | implement | same | prioritized status segments, AGY model cache (memory + disk, 24h TTL), Enter after paste burst submits |
| fix | review | orchestrator + AGY | missing `};` in agy.rs, `display_name()`→`name()`, tests moved into `mod tests` (AGY); kept other-agent activity in narrow status fallbacks (orchestrator) |
| gate | test | cargo | fmt, clippy -D warnings, test --all-targets green |
