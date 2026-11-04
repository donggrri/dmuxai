# PLAN: progress tool events and heartbeats (#166)

Spec: [SPEC-agent-api](../specs/SPEC-agent-api.md) FR-2430..FR-2433.

## Order

1. Add `tool_when`, `tool_name_path` and `tool_title_path` to `[[agents.custom]]`
   with built-in opencode defaults.
2. Parse matching JSONL tool lines in the custom adapter into
   `ActivityItem::Tool`; map edit/write/patch tools to workspace-relative paths.
3. Carry the last stdout `type`/`event` in `AgentEvent::OutputLine` and teach
   `ProgressTracker` to emit 30 s heartbeats while a turn is active.
4. Map tool activity to `tool`/`file_edit` progress kinds and wire heartbeat
   emission into the headless loop only.
5. Add the deterministic unit/CLI tests and update the docs/CHANGELOG.

## Risks

- Extra `OutputLine` events must not enter transcripts or reset watchdogs.
- Summaries must never use content/output fields.
- Heartbeats must stop on terminal events and stay off the event log.

## Rollback

Revert the adapter/progress changes; no persistent schema changes are involved.
