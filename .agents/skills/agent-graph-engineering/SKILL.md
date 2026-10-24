---
name: agent-graph-engineering
description: >-
  Design multi-agent workflows as graph DAGs. Use when orchestrating 3+ roles,
  parallel fan-out/fan-in, conditional routing, or reusable pipelines in Cursor
  IDE, cursor-agent CLI, or Cloud Agents.
---

# Agent Graph Engineering

Design multi-agent workflows as **directed graphs**: agents are nodes, context and data flow are edges. The graph topology is the orchestration logic — get the topology right and the rest follows.

**Platforms:** Linux and macOS (Cursor IDE, `cursor-agent` CLI, Cloud Agents).

## When to Use

- A task needs **3 or more distinct roles** (e.g., research + drafting + review).
- You want **parallelism** — independent subtasks that can run simultaneously.
- You need **conditional branching** — different agent paths based on a decision.
- You're building a **reusable pipeline** that runs repeatedly with different inputs.
- A single agent context is overflowing — split into focused nodes with trimmed edges.

**Don't use when:** The task is 1–2 steps and fits in one agent turn. A graph adds overhead; only use it when the structure pays for itself.

## Core Principle: 5 Rules

1. **Agent = Node, Communication = Edge.** Every agent has one job. Every edge carries explicit context. No implicit coupling.
2. **Single Responsibility Nodes.** A node does ONE thing well. If you can't describe a node's job in one sentence, split it.
3. **Explicit Context on Edges.** Context flows along edges, not through shared global state. Each edge defines exactly what data enters and exits. No telepathy between agents.
4. **Stateless Nodes, Stateful Graph.** Nodes don't remember — the graph state object does. A node receives input, produces output, moves on. Retry is safe because there's no hidden state.
5. **Fail at Edges, Not Nodes.** Retry, fallback, and timeout live on edges. A node either succeeds or fails; the edge decides what happens next.

## Graph Patterns (5 Archetypes)

### 1. Linear Chain — Sequential Pipeline

```
[A: Research] → [B: Draft] → [C: Review] → [D: Publish]
```

- **When:** Steps have strict ordering and each depends on the previous output.
- **Key:** Each edge passes only what the next node needs, not the entire accumulated context. Trim aggressively.
- **Cursor:** Run subagents **sequentially** via the Task tool (`subagent_type` matching a project agent under `.cursor/agents/`, or built-in types like `explore`). Pass each child's return value into the next task's `prompt`. Keep the parent as orchestrator.

### 2. Fan-Out / Fan-In — Parallel Scatter-Gather

```
        ┌→ [B: Backend]    ┐
[A: Plan] →├→ [C: Frontend]   ├→ [E: Integrate]
        └→ [D: Database]   ┘
```

- **When:** Independent subtasks can run simultaneously, then results merge.
- **Key:** Fan-out nodes must be truly independent (no shared dependencies). The fan-in node receives a structured collection, not interleaved text.
- **Cursor:** Issue **multiple Task tool calls in one message** so children run in parallel. Collect structured results, then run the integrator (parent or one subagent). Batch map-reduce work in groups of 3–5.

### 3. Conditional Router — Branch on Output

```
                ┌→ [B: Hot Path]   ┐
[A: Classify] →┤                    ├→ [E: Merge]
                └→ [C: Cold Path]  ┘
```

- **When:** The next step depends on a decision from the current node.
- **Key:** The routing node outputs a structured label, not free text. The graph engine (you) reads the label and dispatches.
- **Cursor:** Delegate classification to `graph-router` (or similar), parse structured output in the parent, then spawn only the matching branch via Task. Do not hide routing inside a single opaque prompt.

### 4. Map-Reduce — Bulk Parallel

```
[A: Chunker] → [B₁, B₂, ..., Bₙ: Workers] → [C: Reducer]
```

- **When:** One operation applied to N items, then aggregated.
- **Key:** Chunker produces a uniform list. Workers are identical logic with different inputs. Reducer combines into a single artifact.
- **Cursor:** Build the item list in the parent (code or a planner subagent). Spawn N Task calls in batches of 3–5 (e.g. `graph-worker` roles). Collect outputs, then run the reducer.

### 5. Hierarchical Delegation — Manager-Worker

```
[A: Manager] →┬→ [B: Worker 1]
              ├→ [C: Worker 2]
              └→ [D: Worker 3]
```

- **When:** A complex task needs decomposition by a coordinator who tracks overall state.
- **Key:** Manager decomposes, workers execute, manager re-integrates. Manager must NOT do worker work — it only plans and assembles.
- **Cursor:** You (or a planner subagent) are the manager. Use Task for workers. Keep integration logic in the manager context.

## Design Procedure (4 Steps)

1. **List the jobs.** Write every distinct task as a bullet. One bullet = one node candidate.
2. **Draw the edges.** For each node, ask: "What do I need as input?" and "What do I produce?" Draw arrows. If an arrow has no clear payload, the node boundary is wrong.
3. **Cut the fat.** Merge nodes that always run together and share all context. Split nodes that do two things. Target 3–7 nodes for most workflows.
4. **Assign execution.** For each node pick the simplest Cursor mechanism:
   - **`task_subagent`** — Task tool with a matching `.cursor/agents/*.md` definition (default for focused roles).
   - **`task_builtin`** — Built-in subagent types (`explore`, `generalPurpose`, etc.) when they fit.
   - **`task_background`** — `run_in_background: true` on Task for long branches that should not block the parent.
   - **`task_cloud`** — Task with `environment: "cloud"` for isolated VMs, separate branches, or heavy CI-style work (Cloud Agents / IDE when enabled).
   - **`self`** — Cheap steps done in the parent context without delegation.

## Model choice

This skill is model-agnostic. Use whatever model your runner is configured with; omit `model` on custom agents to inherit the parent model, or set it per role only if your project requires that.

## Edge Contract Template

For every edge in the graph, define:

```
Edge: A → B
Payload: { type: "structured|freeform", content: "...", trim_rule: "keep_only|pass_through|summarize" }
On Failure: { retry: 1, fallback: "skip|default_value|human_escalate" }
```

Document edges in your graph file (see `templates/graph.yaml`) and `templates/edge-contract.md`.

## Context Budget Rules

- **Trim on every edge.** Node B does not need Node A's full prompt — only its output. Pass the result, not the conversation.
- **Summarize between phases.** When moving between major graph phases (research → writing), summarize the accumulated context into a compact brief.
- **Cap fan-out breadth.** More than 5 parallel children creates coordination overhead. Batch in groups of 3–5, collect, then next batch.
- **Children are amnesiac.** A Task subagent sees only what you pass in `prompt`. It does not inherit your conversation. Pack everything it needs.

## Cursor Implementation Map

| Pattern | Mechanism | How |
| --- | --- | --- |
| Linear Chain | Task (sequential) | One Task per node; prior result → next `prompt` |
| Fan-Out / Fan-In | Task (parallel) | Multiple Task calls in **one** message; parent integrates |
| Conditional Router | Task + parse | Classifier subagent → structured label → branch Tasks |
| Map-Reduce | Task batched | List in parent; batch 3–5 Tasks; reducer Task or self |
| Hierarchical | Task | Parent/planner decomposes; worker Tasks; parent integrates |
| Long / heavy branch | `run_in_background` or `environment: "cloud"` | Background Task or cloud subagent; merge on completion |

**Where this runs**

- **Cursor IDE** — Agent chat, Task tool, project subagents in `.cursor/agents/`.
- **cursor-agent CLI** — Same project rules, skills, and agents from the repo.
- **Cloud Agents** — Full runs on a VM; use cloud Tasks for isolated long branches when needed.

## Anti-Patterns

- **Cycle without exit.** A→B→A with no termination condition = infinite loop. Every cycle needs an explicit max-iteration or success check.
- **Implicit state sharing.** Two nodes reading/writing the same file without an edge declaring it. Make the file path an explicit edge payload.
- **God Node.** One agent doing research + drafting + review + publishing. Split it.
- **Context explosion.** Passing full conversation history to every child. Trim to essentials.
- **Premature parallelism.** Fan-out before the plan is stable. Plan first (sequential), then parallelize execution.
- **Over-graphing.** A 2-step task doesn't need a graph. Use this skill when N ≥ 3 nodes.
- **Trusting child self-reports.** A child says "done" but the file is empty. Verify side effects (file exists, content non-empty) before proceeding to the next node.

## Verification Checklist

Before declaring a graph workflow complete:

- [ ] Every node has a one-sentence job description
- [ ] Every edge has a defined payload and trim rule
- [ ] No node does more than one thing
- [ ] Parallel nodes are truly independent (no shared mutation)
- [ ] Failure on any edge has a defined fallback
- [ ] Total context passed to children is minimized (not your full conversation)
- [ ] External side effects (file writes, API calls) are verified by reading back
- [ ] Graph has ≤ 7 nodes (if more, consider hierarchical nesting)

## Quick Start

1. Read this skill (auto-discovered from `.agents/skills/`, or `@agent-graph-engineering` / rule `agent-graph-engineering`).
2. Copy `templates/graph.yaml` into your workspace and fill nodes and edges.
3. Copy or adapt example subagents from `.cursor/agents/graph-*.md`.
4. Execute using the Cursor Implementation Map above.
5. Verify with the checklist.

## Reference

- `.agents/skills/agent-graph-engineering/templates/graph.yaml` — Graph definition template
- `.agents/skills/agent-graph-engineering/templates/edge-contract.md` — Edge contract format
- `.cursor/agents/graph-*.md` — Example role subagents

## Attribution

Adapted from the **Hermes Agent** `agent-graph-engineering` skill (MIT). Hermes-specific APIs (`delegate_task`, terminal/hermes tags) are replaced with Cursor subagents, the Task tool, and background/cloud execution.
