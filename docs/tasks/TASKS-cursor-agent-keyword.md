# TASKS: cursor-agent keyword

Spec: [SPEC-cursor-agent-keyword.md](../specs/SPEC-cursor-agent-keyword.md)

- [ ] T1 - `canonical_runner_id`. Requirements: FR-3100. Verify: `cargo test tc_3100`.
- [ ] T2 - CLI, settings, graph, TUI, MCP, web boundaries and reserved name. Requirements: FR-3101..FR-3105. Verify: `cargo test tc_3101 tc_3102 tc_3103 tc_3104 tc_3105 tc_3106`.
- [ ] T3 - `aliases` in `runners --json`. Requirements: FR-3106. Verify: `cargo test tc_3107`.
- [ ] T4 - Help, README, docs, CHANGELOG, review. Requirements: UX-3100. Verify: `cargo test tc_3108`, `python3 scripts/check-spec.py --all`, `scripts/gate.sh`.
