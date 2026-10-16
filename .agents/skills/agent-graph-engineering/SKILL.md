---
name: agent-graph-engineering
description: Design and audit Cursor-native agent graphs for DmuxAI (nodes = agents, edges = explicit context contracts). Use when planning multi-agent delivery, Windows+WSL hosting, skills under .agents/skills, or permission/sandbox boundaries. Do not use for one-off reversible edits without a durable workflow.
---

# Agent graph engineering (Cursor / DmuxAI)

Design the smallest graph that matches workflow risk. A graph is a contract file
(`docs/*-graph.yaml` or project-local YAML), not a runtime.

## Model policy (DmuxAI product)

| Role | Model | Notes |
| --- | --- | --- |
| Orchestrate / plan / review | `grok-4.7-medium` | No `*-fast` variants |
| Implement | `composer-2.5` | Workspace writes only in Implement mode |
| Research / test | `claude-haiku-5-5-high` | Read-only or test authoring |

## Create workflow

1. Read repository `AGENTS.md`, baseline spec, and adapter launch flags (`src/agent/*`, `src/process/mod.rs`).
2. Draft Intent Brief: outcome, scope, authority, reversibility, non-goals.
3. Copy `templates/graph.yaml`, set `id`, nodes, edges, and edge context budgets.
4. Map each node to a `.cursor/agents/graph-*.md` role and model.
5. Validate: every node reaches `END`, edges declare guard + context + stop rule.

## Invariants

- Edges carry explicit context payloads; no implicit shared memory between nodes.
- High-risk writes require Implement mode and a human-selected scope.
- Reviewer nodes are read-only and independent from implementer nodes.
- Do not weaken DmuxAI sandbox policy (`approvalPolicy: never` on Codex is product intent; Cursor unsandboxed fallback is forbidden).

## Resources

- `templates/graph.yaml` — graph contract template
- `.cursor/agents/graph-*.md` — role prompts bound to models
- `.cursor/rules/agent-graph-engineering.mdc` — workspace rule hook
