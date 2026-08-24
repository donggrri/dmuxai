# SPEC: <feature name>

Status: draft
Owner: <name>
Created: YYYY-MM-DD
Baseline: `<commit or tag>`
Related: <issue, design, or baseline requirement IDs>

## 1. Decision summary

State the user problem, desired user-visible outcome, and the decision being
made in at most five sentences. Do not describe code structure here.

## 2. Scope

### In scope

- <observable capability>

### Out of scope / non-goals

- <explicitly excluded capability>

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-001 | <question> | <name> | Spec approval |

No implementation starts while a Must requirement depends on an unanswered
question.

## 3. Terms and state model

Define domain terms once. For each stateful object, name allowed states,
transitions, and terminal states. Include a small transition table if useful.

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| <term> | <definition> | <nearby term> |

## 4. Requirements

Each row must specify one externally observable contract. Use `FR` for a user
feature, `BR` for a business/domain rule, `NFR` for a quality constraint, `UX`
for an interaction contract, and `INT` for an integration contract.

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-001 | Must | Given …, when … | Then … | If …, then … | TC-001, TC-002 |

Rules:

- IDs are stable: never repurpose a removed ID.
- Write **must** statements, not implementation hints such as "use a HashMap".
- Include durable storage, process, security, and error consequences when they
  are observable or safety-critical.
- Give each Must requirement at least one automated `TC-*` test unless the
  table explicitly explains why manual verification is the only viable option.

## 5. Acceptance scenarios and test design

Use deterministic fixtures, a temporary database, or state doubles before an
authenticated external provider. One scenario may cover multiple requirement
IDs, but every requirement must appear in the mapping below.

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-001 | FR-001 | unit | <reproducible state> | <single trigger> | <precise result> | `src/...` |
| TC-002 | FR-001, NFR-001 | integration | … | … | … | `cargo test <name>` |

Required coverage prompts:

- Happy path: does the primary result occur?
- Boundary: empty, maximum, minimum, duplicate, ordering, or Unicode boundary
  as applicable.
- Failure: malformed provider input, persistence error, rejected request, or
  unavailable dependency.
- State transition: cancellation, interruption, restart, stale state, or retry
  as applicable.
- Isolation: conversation, agent, workspace, or user data cannot leak across
  its ownership boundary.

## 6. Data, integration, and safety contracts

Only include rows that apply. A no-change statement is valid and prevents
accidental scope expansion.

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | <schema/invariant or "No schema change"> | <migration/recovery behavior> | TC-… |
| Provider protocol | <request/event contract> | <unknown/malformed event handling> | TC-… |
| Security / permissions | <sandbox, approval, sensitive data rule> | <must not weaken…> | TC-… |
| Platform | <Windows/Linux behavior> | <fallback or limitation> | TC-… |

## 7. Implementation plan

Link or add the detailed `PLAN-<slug>.md` and `TASKS-<slug>.md`. The plan must
reference requirement and test IDs; it must not introduce new behavior.

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | <change> | <paths> | FR-001 | TC-001 |

## 8. Review and test exit criteria

- [ ] All Must requirements map to passing evidence.
- [ ] The diff has been reviewed against scope and non-goals.
- [ ] Required Rust checks pass: format, all targets test, and clippy.
- [ ] External-provider smoke tests are marked run, skipped, or blocked with a
  reason; skipped is never reported as passing.
- [ ] `REVIEW-<slug>.md` records commands, result, residual risk, and rollback
  notes.
