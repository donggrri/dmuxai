# REVIEW: ACP runners and single agent mode

Status: complete
Spec: `docs/specs/SPEC-acp-runners.md`
PR: #78

## Compliance

| Requirement | Evidence |
| --- | --- |
| FR-1300..FR-1305 ACP + legacy | `src/agent/acp/*`, `cursor.rs`, `custom.rs`, `cursor_acp_turn_with_fake_agent` |
| FR-1306..FR-1308 single agent mode | `TurnPolicy`, `/mode` notice-only, `session.js`, settings warnings |
| FR-1309 docs | README, CHANGELOG, superseded notes in baseline/dashboard specs |
| NFR-1301 Linux-only | SPEC §Platform; no new Windows ACP docs in this change |

## Commands

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

All passed on Linux (2026-10-20).

## Residual risk

- Live ACP against real `cursor-agent` / `grok` not run in CI (OQ-1300, OQ-1301).
- Historical docs (TEST-PLAN, dogfood README) may still mention `/mode`; baseline superseded note is the anchor.

## Rollback

Set `[cursor] protocol = "legacy"` and `[agents] grok_protocol = "legacy"`.
