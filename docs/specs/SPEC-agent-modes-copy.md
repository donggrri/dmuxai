# SPEC-agent-modes-copy

Status: Superseded

**Status:** Superseded by the single **agent turn** model (`TurnPolicy::Agent` /
`TurnPolicy::ReadOnly` on `AgentRequest.policy`) and by
[SPEC-acp-runners.md](./SPEC-acp-runners.md) for Cursor/grok transport.

User-facing Ask / Plan / Implement selection (`/mode`, mode picker, per-agent
mode maps) has been removed. Internal judge, plan, and replan turns use
`TurnPolicy::ReadOnly`; user turns use `TurnPolicy::Agent`.

Do not implement requirements from earlier drafts of this spec unless they are
re-stated in the baseline or a current feature spec.

## 4. Requirements

| ID | Priority | Requirement |
| --- | --- | --- |
| FR-1889 | Must | User-facing Ask/Plan/Implement mode selection is superseded by `TurnPolicy::Agent` for user turns and `TurnPolicy::ReadOnly` for internal turns |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Test |
| --- | --- | --- | --- |
| TC-1889 | FR-1889 | unit | `src/agent/mod.rs` `legacy_ask_and_plan_are_read_only`, `legacy_implement_is_agent` |
