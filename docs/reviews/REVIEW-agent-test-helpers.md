# REVIEW: Agent test helpers

Spec: [SPEC-agent-test-helpers.md](../specs/SPEC-agent-test-helpers.md)  
Date: 2026-10-17

## Spec compliance

| ID | Met | Notes |
| --- | --- | --- |
| FR-510–FR-514 | yes | `AGENTS.md` § Agent test helpers + `agents_md_documents_agent_test_helpers` |
| FR-515 | yes | `mod tests` module doc + `app_tests_module_doc_lists_helpers` |
| FR-516 | yes | `handle_enter_alias_submits_prompt` |
| FR-517 | yes | dogfood #23 marked fixed |
| FR-518 | yes | TEST-PLAN §1 rows added |
| FR-519 | yes | `spec_ids_are_unique_across_specs` |

## Regression risk

Low: `handle_enter` is `#[cfg(test)]` only; production Enter path unchanged.

## Test evidence

```text
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
bash scripts/spec-index.sh | rg agent-test-helpers
```

## Residual risk

Agents may still prefer the alias name in new tests; docs steer to `submit_prompt`.

## Rollback

Revert documentation and remove `handle_enter` alias; no migrations.
