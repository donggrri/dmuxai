# TASKS: open model lists

Spec: [SPEC-open-model-lists.md](../specs/SPEC-open-model-lists.md)

- [ ] T1 - Open agy policy, `validate_model_for_runner_with`, process-wide `allow_fast`. Requirements: FR-3000..FR-3003. Verify: `cargo test tc_3000 tc_3001`.
- [ ] T2 - User-level `[models] allow_fast` setting and entry-point wiring. Requirements: FR-3003. Verify: `cargo test tc_3008`.
- [ ] T3 - Dynamic Cursor/AGY listing with timeout, cache, static fallback. Requirements: FR-3004..FR-3007, NFR-3000. Verify: `cargo test tc_3002 tc_3003 tc_3004 tc_3005 tc_3006`.
- [ ] T4 - Status text, graph, picker and Cursor argv coverage. Requirements: FR-3008..FR-3010. Verify: `cargo test tc_3009 tc_3010 tc_3011 tc_3012`.
- [ ] T5 - Update superseded tests, CHANGELOG, docs, review. Requirements: all. Verify: `python3 scripts/check-spec.py --all`, `python3 scripts/check-tc-links.py`, `scripts/gate.sh`.
