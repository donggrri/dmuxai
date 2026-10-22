# SPEC: Automatic cross-agent conversation sharing

Status: complete  
Owner: DmuxAI maintainers  
Created: 2026-08-24  
Baseline: `v0.1.0` / `ca66284`  
Related: `FR-020`–`FR-024`, `BR-002`, `BR-003`, `BR-011`

## 1. Decision summary

The current product stores one canonical transcript but only projects the
selected agent's history, so changing agents does not carry the conversation
forward. This feature makes completed turns in a DmuxAI conversation common
context for Codex, Cursor, and AGY without requiring `/share`. Each subsequent
request receives a bounded shared projection whenever its provider-native
session does not already cover the complete common history. Native sessions
remain provider-scoped; no unsolicited request is sent to an idle or busy
agent.

## 2. Scope

### In scope

- Include completed turns from every agent in the active conversation's
  context projection.
- Preserve the source agent on projected turns and render it in restored
  context so an agent can distinguish Codex, Cursor, and AGY contributions.
- Reuse the existing bounded summary/recent-tail mechanism for the common
  transcript and keep the canonical transcript unchanged.
- Exclude prior synthetic `[DmuxAI explicit context import]` turns from the
  automatic projection so the compatibility command cannot recursively nest
  its own payload.
- Invalidate native resumption implicitly when the binding's known sequence is
  behind a newly added turn from any agent.
- Keep `/share <agent>` available for explicit manual replay, but make it
  optional and document that ordinary turns are already shared automatically.

### Out of scope / non-goals

- Sending unsolicited prompts or transcript events to provider processes.
- Sharing turns from another DmuxAI conversation or workspace.
- Replacing provider-native sessions or making one native session serve
  multiple agents.
- Changing the SQLite schema or deleting existing summaries.
- Sharing queued/running/failed partial turns as completed context.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-001 | “Shared automatically” means the next request to each agent receives the common projection; providers are not contacted until the user submits a request. | DmuxAI maintainers | Spec approval |

OQ-001 is resolved by the user's request and is part of the acceptance
criteria below.

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| Common transcript | Completed user/assistant turns from all agents in one DmuxAI conversation, ordered by conversation sequence. | A provider-native thread |
| Shared projection | Bounded rolling summary plus recent common turns rendered into a provider request when native resume is unavailable. | The durable canonical transcript |
| Provider binding | Native session ID owned by one `(conversation, agent)` pair. | A DmuxAI conversation ID |

Completed turns remain terminal and immutable. A provider binding may be
active only when its `known_through_seq` covers the common projection head;
otherwise the next request starts a fresh native session with the shared
projection.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-025 | Must | Given a conversation with a completed turn from any agent, when another agent receives a later request in that conversation | The later request contains the earlier turn in its shared context, without `/share` | A queued/running or non-terminal turn is not projected until it reaches a terminal completed state | TC-001, TC-002 |
| FR-026 | Must | Given completed turns from at least two agents, when either agent is requested after the other agent's turn | The projection contains both turns in sequence order and labels each source agent | Invalid or mismatched output is rejected by existing correlation rules; synthetic prior explicit-import turns are omitted to prevent recursive payloads | TC-002, TC-003, TC-006 |
| FR-027 | Must | Given a provider binding whose known sequence is behind a newly completed turn from another agent | The request does not resume that stale native session and instead starts a fresh request with the shared projection | A binding that already covers the common head may continue to resume normally | TC-004 |
| FR-028 | Must | Given common history larger than the existing projection budget | Automatic summary/retry uses all-agent source turns, preserves the full transcript, and respects the 32,000-character projection and 20,000-character summary limits | Failed, malformed, stale, or oversized summary leaves the canonical transcript intact and restores the deferred prompt | TC-005 |
| UX-015 | Must | Given the user switches agents with `/agent` or the picker | No `/share` command is required; the next prompt to the new agent visibly uses the common context | `/share <agent>` remains accepted as an optional explicit replay and does not alter automatic sharing semantics | TC-001, TC-006 |
| NFR-005 | Must | Given any workspace or conversation selection | Shared projection reads only completed turns in the active conversation and canonical workspace | No turn from another conversation/workspace is included | TC-007 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-001 | FR-025, UX-015 | app integration | Complete a Codex turn, select Cursor, leave `/share` unused | Submit a Cursor prompt | Cursor request is fresh and its prompt contains the Codex user/assistant text and shared-context marker | `src/app.rs` test |
| TC-002 | FR-026 | conversation integration | Complete Codex then Cursor turns in one conversation | Begin a later Codex turn | Context contains both source agents in seq order; Codex native binding is not resumed after Cursor's turn | `tests/conversation.rs` test |
| TC-003 | FR-026, NFR-005 | conversation integration | Create a second conversation and add turns to both | Begin turns in each conversation | Each projection contains only its own conversation's turns | `tests/conversation.rs` test |
| TC-004 | FR-027 | conversation integration | Bind an agent at seq N, then complete another agent at seq N+1 | Begin the first agent again | `resume_session` is `None`; the context contains the new cross-agent turn | `tests/conversation.rs` test |
| TC-005 | FR-028 | conversation integration | Build long mixed-agent history beyond 32,000 characters | Begin another agent turn and commit a summary fixture | Summary source includes mixed-agent turns, projection stays within limits, and canonical messages remain | `tests/conversation.rs` test |
| TC-006 | UX-015 | app unit | Use `/share cursor` after automatic sharing is available | Submit the command | The legacy explicit command remains accepted; ordinary switching still works without it | `src/app.rs` test |
| TC-007 | NFR-005 | persistence integration | Use two workspaces and two conversations | Begin a request in each | No cross-workspace or cross-conversation text appears | `tests/conversation.rs` test |

Coverage includes the happy path (TC-001), ordering and multiple agents
(TC-002), isolation (TC-003/TC-007), native-session boundary (TC-004), and
summary/failure limits (TC-005).

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Persistence | No schema change. Existing checkpoints remain valid only when their source hash matches the common-turn set. | Reverting the code restores same-agent projection; stored canonical turns and checkpoints remain readable. | TC-003, TC-005 |
| Provider protocol | Requests keep the existing structured `resume_session` field. A stale common-head binding uses `None` and the existing restored-context envelope. | No provider CLI changes; live provider tests remain ignored. | TC-001, TC-004 |
| Security / permissions | Shared context is bounded, marked as untrusted historical data, and never interpolated into shell commands. | Existing sandbox/mode policy is unchanged. | TC-005, default Rust gates |
| Platform | No platform-specific behavior change. | Existing Linux process launch paths are unchanged. | Default Rust gates |

## 7. Implementation plan

See [PLAN-automatic-cross-agent-sharing.md](../plans/PLAN-automatic-cross-agent-sharing.md)
and [TASKS-automatic-cross-agent-sharing.md](../tasks/TASKS-automatic-cross-agent-sharing.md).

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Update projection data and query to include all completed agent turns and source labels | `src/conversation.rs` | FR-025, FR-026, FR-028, NFR-005 | TC-002, TC-003, TC-005 |
| 2 | Render the shared projection and preserve native-resume boundary | `src/app.rs` | FR-025, FR-027, UX-015 | TC-001, TC-004, TC-006 |
| 3 | Update deterministic tests and product documentation | `tests/conversation.rs`, `src/app.rs`, `README.md`, baseline spec | all | TC-001–TC-007, spec gate |

## 8. Review and test exit criteria

- [ ] All Must requirements map to passing evidence.
- [ ] The diff has been reviewed against scope and non-goals.
- [ ] Required Rust checks pass: format, all targets test, and clippy.
- [ ] The specification traceability gate passes.
- [ ] `../reviews/REVIEW-automatic-cross-agent-sharing.md` records commands, results,
  residual risk, and rollback notes.
