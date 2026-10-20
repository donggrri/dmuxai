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

For a feature or behavior change, create these files under `docs/` using an
ASCII kebab-case `<slug>`:

1. `docs/specs/SPEC-<slug>.md` — requirements, boundaries, acceptance scenarios, and test
   mapping. Start from `docs/templates/FEATURE-SPEC.md`.
2. `docs/plans/PLAN-<slug>.md` — ordered implementation plan, risks, and rollback notes.
3. `docs/tasks/TASKS-<slug>.md` — independently verifiable tasks. Every code task must
   name the requirement IDs and the test command it will satisfy.
4. `docs/reviews/REVIEW-<slug>.md` — spec compliance, regression risk, and test evidence.

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
  in `docs/reviews/REVIEW-<slug>.md`.

### 5. Test

- Run the narrow tests from the spec first, then repository gates appropriate to
  the changed area.
- For every changed `docs/specs/SPEC-*.md` or baseline spec, run
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

## Agent test helpers

When writing or extending `src/app.rs` integration tests, use the helpers in the
`#[cfg(test)] mod tests` block. **There is no `App::handle_enter` in production**
— submitting a prompt is [`submit_prompt`](src/app.rs) (bound to Enter in the
TUI). A `#[cfg(test)]` `handle_enter` alias exists only so mistaken generated
calls still compile.

| Helper | Returns | Use when |
| --- | --- | --- |
| `test_app()` | `(App, SentRequests)` | Default: `:memory:` DB, recording adapters, Codex selected. |
| `test_app_with_engine(engine)` | `(App, SentRequests)` | You need a specific [`ConversationEngine`](src/conversation.rs) (e.g. on-disk temp DB). |
| `test_app_with_engine_and_failure(engine, agent)` | `(App, SentRequests)` | Simulate `AdapterError::CommandQueueFull` on send for one agent. |
| `test_app_runtime(engine, failing_agent)` | `(App, SentRequests, CancelRequests)` | Assert cancel paths as well as sent requests. |

Typical flow:

```rust
let (mut app, sent) = test_app();
app.input = "hello".to_owned();
app.submit_prompt(); // not handle_enter (test alias only)
let requests = sent.lock().unwrap();
assert_eq!(requests.len(), 1);
```

Set `app.selected = adapter_index(Agent::Cursor)` (or another runner) before
submitting when the test depends on the active agent. Inspect captured traffic
via `sent.lock().unwrap()`; each entry is `(Agent, AgentRequest)`.
