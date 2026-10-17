# PLAN: Agent test helpers (dogfood #23)

Status: complete  
Spec: [SPEC-agent-test-helpers.md](../specs/SPEC-agent-test-helpers.md)

1. Document helpers in `AGENTS.md` (FR-510–FR-514, TC-510).
2. Add module docs and `#[cfg(test)] handle_enter` alias in `src/app.rs` (FR-515–FR-516, TC-511–TC-512).
3. Add `tests/agent_test_helpers.rs` for documentation oracles (TC-510, TC-512).
4. Add `docs/specs/SPEC-agent-test-helpers.md` with FR/TC 510–519 (FR-519, TC-513–TC-515).
5. Update `docs/TEST-PLAN.md` and mark dogfood #23 fixed (FR-517–FR-518).
6. Write `docs/reviews/REVIEW-agent-test-helpers.md` after `cargo test` and clippy.

## Risks and rollback

- **Risk:** Test alias encourages `handle_enter` in new tests. **Mitigation:** docs and module doc steer to `submit_prompt`; alias is `cfg(test)` only.
- **Rollback:** Revert doc commit and remove alias; no persistence impact.
