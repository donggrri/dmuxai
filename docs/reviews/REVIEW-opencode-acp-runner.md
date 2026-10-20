# Review: opencode ACP runner (stacked on PR #78)

Status: Ready for review
Spec: `docs/specs/SPEC-acp-runners.md` (FR-1320..FR-1322, TC-1320..TC-1322)

## Compliance

| Requirement | Met | Evidence |
| --- | --- | --- |
| FR-1320 ACP argv + model via `session/set_config_option` | Yes | `acp_runner.rs`, `acp/client.rs`, `custom.rs` |
| FR-1321 ACP permissions / single agent mode | Yes | Same `PermissionContext` path as grok ACP |
| FR-1322 detection / preset registration | Yes | Existing FR-810 path + `find_opencode_bin_from` |
| Legacy fallback | Yes | `opencode_protocol = legacy` → jsonl `run` |
| No merge to PR #78 branch | Yes | Separate branch `cursor/opencode-acp-runner-b005` |

## Tests run

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets`

## Residual risks

- Live opencode ACP model/config behavior not exercised in CI (fake agent only).
- MCP propagation for opencode remains on legacy path only; ACP uses opencode project config.

## Open questions

- OQ-1320: document operator patterns for restricting providers/models outside dmuxai `allowed_models`.
