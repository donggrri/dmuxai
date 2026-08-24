# DmuxAI delivery workflow

This repository uses a spec-first delivery flow for every behavior change:

```text
Spec -> Plan -> Execute -> Review -> Test
```

Read [docs/WORKFLOW.md](docs/WORKFLOW.md) and the current product baseline at
[docs/specs/DMUXAI-BASELINE.md](docs/specs/DMUXAI-BASELINE.md) before changing
behavior.  The baseline describes what is already shipped; it is not a request
to redesign it.

## Required artifacts and gates

For a feature or behavior change, create these files at the repository root
using an ASCII kebab-case `<slug>`:

1. `SPEC-<slug>.md` — requirements, boundaries, acceptance scenarios, and test
   mapping. Start from `docs/templates/FEATURE-SPEC.md`.
2. `PLAN-<slug>.md` — ordered implementation plan, risks, and rollback notes.
3. `TASKS-<slug>.md` — independently verifiable tasks. Every code task must
   name the requirement IDs and the test command it will satisfy.
4. `REVIEW-<slug>.md` — spec compliance, regression risk, and test evidence.

Documentation-only edits may use a short plan and review in the pull request or
task description, but must still preserve links and render correctly.

Do not start implementation until the spec has unambiguous acceptance criteria.
If a requirement is unclear, list it as an open question instead of inventing
observable behavior. Do not commit unless the user explicitly requests it.

## Phase rules

### 1. Spec

- Give each observable requirement a stable ID: `FR-*`, `BR-*`, `NFR-*`,
  `UX-*`, or `INT-*`.
- Define the actor, preconditions, trigger, expected result, and persistence or
  error behavior. Use Given/When/Then scenarios.
- Separate shipped behavior from proposed behavior and explicitly list
  non-goals.
- Map every requirement to one or more planned test IDs. A requirement without
  a test must have a concrete manual-verification reason.

### 2. Plan

- Identify the smallest modules and tests to change, dependency order, risks,
  and a rollback path when persistent data or provider protocol is involved.
- Preserve DmuxAI's provider-neutral boundaries: UI, conversation persistence,
  adapter protocol, and process launching are separate concerns.

### 3. Execute

- Implement only approved spec requirements and keep the task checklist current.
- Add or update the tests named by the spec in the same change.
- Keep external provider smoke tests `#[ignore]`; deterministic parsing and
  state tests must not require credentials or a live provider.

### 4. Review

- Review the diff against every requirement and non-goal, not merely whether it
  compiles.
- Check persistence migrations, cancellation/error paths, per-agent isolation,
  and platform-dependent process behavior whenever they are in scope.
- Record unmet requirements, residual risks, and intentionally untested paths
  in `REVIEW-<slug>.md`.

### 5. Test

- Run the narrow tests from the spec first, then repository gates appropriate to
  the changed area.
- For every changed `SPEC-*.md` or baseline spec, run
  `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath <path>`
  before review.
- Default Rust gates:

  ```powershell
  cargo fmt --check
  cargo test --all-targets
  cargo clippy --all-targets --all-features -- -D warnings
  ```

- Run live-provider tests only when the relevant CLI is installed, authenticated,
  and the task calls for it. Record the exact command and result separately from
  deterministic tests.
- A failure blocks completion. Do not mark a test passed based only on code
  inspection or a previous run.

## Test-case quality bar

Each automated test must cite the requirement IDs it protects in a nearby
comment, test name, or the feature spec's traceability table. Prefer externally
observable outcomes over private implementation details. Cover at least one
success case, boundary case, and failure/cancellation case for each stateful
requirement.

For a regression fix, first add a test that fails for the reported behavior,
then implement the fix. For a new provider protocol, use saved JSON/NDJSON
fixtures for normal, partial, malformed, and terminal events.
