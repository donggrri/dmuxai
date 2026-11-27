# TASKS: cursor-agent keyword

Spec: [SPEC-cursor-agent-keyword.md](../specs/SPEC-cursor-agent-keyword.md)

- [ ] T1 - `canonical_runner_id`. Requirements: FR-3200. Verify: `cargo test tc_3200`.
- [ ] T2 - CLI, settings, graph, TUI, MCP, web boundaries and reserved name. Requirements: FR-3201..FR-3205. Verify: `cargo test tc_3201 tc_3202 tc_3203 tc_3204 tc_3205 tc_3206`.
- [ ] T3 - `aliases` in `runners --json`. Requirements: FR-3206. Verify: `cargo test tc_3207`.
- [ ] T3b - Canonical id flip to `cursor-agent` and legacy `cursor` normalization on read. Requirements: FR-3207..FR-3209. Verify: `cargo test tc_3209 tc_3210 tc_3211 tc_3212 tc_3213 tc_3214`.
- [ ] T4 - Help, README, docs, CHANGELOG, review. Requirements: UX-3200. Verify: `cargo test tc_3208`, `python3 scripts/check-spec.py --all`, `scripts/gate.sh`.
