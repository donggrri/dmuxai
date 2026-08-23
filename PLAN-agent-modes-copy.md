# PLAN: Agent modes and chat copy

Status: complete
Branch: `main`
Baseline: `9d554ff52e85adb52c926eebc37ac4cfbe49c2dc`

## Goal

Provide per-agent Ask/Plan/Implement modes, slash-command previews, and mouse-drag chat copying on Windows and Linux.

## Scope boundaries

- Implement mode may write only inside the workspace through each provider's sandboxed mode.
- Existing provider conversation/session continuity must not be reset by mode selection.
- Conversation sharing and persistence belong to the separate referenced task and are not implemented here.

## Implementation order

1. Add provider-independent execution modes and map them to each CLI.
2. Add slash-command preview and the `/mode` picker.
3. Add terminal-cell-aware chat selection and cross-platform clipboard support.
4. Run automated checks and exercise the real TUI.

## Verification

- `cargo test --all-targets --target-dir target\agent-modes-copy`
- `cargo clippy --all-targets --all-features --target-dir target\agent-modes-copy -- -D warnings`
- Real Windows TUI: slash preview, mode selection, agent display, drag-copy, clean exit.
- Linux cross-target clippy was attempted; the host lacks `x86_64-linux-gnu-gcc`, which is required by the concurrently added bundled SQLite dependency.
- Mutation testing is unavailable for this Rust workspace.
