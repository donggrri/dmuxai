## Summary

<!-- What changed and why? Link docs/specs/SPEC-<slug>.md and any GitHub issue. -->

## Spec-first checklist

- [ ] Behavior change includes `docs/specs/SPEC-<slug>.md` (or only docs/markdown with no behavior change)
- [ ] Plan/tasks/review artifacts added under `docs/` when required by [AGENTS.md](../AGENTS.md)
- [ ] `bash scripts/check-spec.sh` run for any changed `docs/specs/SPEC-*.md` or baseline spec
- [ ] Tests map to requirement or scenario IDs from the spec

## Test evidence

<!-- Commands you ran and their outcome. CI must pass on this branch. -->

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
```

## Graph execution report (product PRs only)

<!-- Per MISSION.md: graph used, roles, models, edge payloads, retries, timings. Omit for docs-only or non-graph changes. -->

## Risk and rollback

<!-- Migrations, provider protocol, persistence, or platform-specific behavior? How to roll back? -->
