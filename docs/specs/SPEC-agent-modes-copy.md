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
