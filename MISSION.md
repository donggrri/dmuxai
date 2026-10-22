# DmuxAI mission

DmuxAI is a **Rust + Ratatui** terminal orchestrator. It does not scrape ANSI terminals or assemble shell one-liners; it drives real headless (or selectively interactive) coding-agent CLIs with structured protocols and durable conversations.

## Product direction

1. **Graph-first delivery** — Default workflow follows [agent-graph-engineering](.agents/skills/agent-graph-engineering/SKILL.md): spec → plan → execute → review → test, with work expressed as a directed graph (nodes, roles, edges). DmuxAI should **auto-pick which agent/CLI to invoke per graph node** from routing policy and the active graph definition (see `docs/dmuxai-graph.yaml` and future runtime graph execution).

2. **Primary runners** — **cursor-agent** is the default CLI; **agy** is the second primary (fallback or user-selected alternative). **Codex** and other CLIs remain **optional**, never the default route.

3. **Skills** — Discover and run skills under `.agents/skills/` (each skill provides `SKILL.md` and optional assets). Skills are first-class inputs to turns and graph nodes.

4. **Permissions** — Ask/Plan stay read-only; **Implement/Plan must never silently succeed when writes are blocked**. Fail the turn with a recorded stderr reason when permission is denied or Implement produces no observable writes.

5. **Direct image attach** — Ctrl+V (and Alt+V where terminals use it) attaches clipboard images:
   - **Interactive cursor-agent on Linux/WSL**: pass Ctrl+V through so the CLI’s native paste works when DmuxAI is not managing the turn.
   - **Headless runs and CLIs without paste (agy, Codex, headless cursor-agent)**: DmuxAI reads the clipboard (`wl-paste`, `xclip`, or WSL `powershell.exe` image), saves PNG under the workspace `.dmuxai/attachments/`, shows `[Image #N]` chips, and passes paths via `@path` and/or each CLI’s `--image` flag (verified per provider).

6. **Platforms** — **Linux only** (WSL counts as Linux and keeps working). Native **Windows** support was dropped on 2026-10-22 (see [docs/plans/PLAN-drop-windows.md](docs/plans/PLAN-drop-windows.md)); the removal is revertable in one commit.

7. **Model policy** — **Only `composer-2.5`** may be used or defaulted for any graph node, subagent, or DmuxAI routing default. No haiku, grok, sonnet, gpt, gemini, or `*-fast` models. Guards/tests enforce this in-repo. Exception (2026-10-17, dogfooding): a model the user picks with F2 for a non-Cursor runner is honored under that runner's policy (AGY `gemini-*-flash-*`; custom agents' `allowed_models`); graph defaults stay `composer-2.5`.

8. **Custom headless runners** — DmuxAI must be able to run **any headless agent CLI declared in config** (`[[agents.custom]]` in `dmuxai.toml`), not only the built-in primaries (see [docs/agents/custom-agents.md](docs/agents/custom-agents.md)).

9. **Graph execution reports** — Every product PR includes an honest Korean **Graph execution report** in `docs/graph-runs/<topic>.md` and a matching section in the PR body (graph used, roles, models, edge payloads, retries, timings; no false claims of delegation).

## Non-goals (for now)

- Replacing Ratatui with another UI framework.
- Defaulting to Codex or non–composer-2.5 models.

## Related docs

- [docs/WORKFLOW.md](docs/WORKFLOW.md) — Spec-first delivery gates.
- [docs/ANALYSIS.md](docs/ANALYSIS.md) — Platform and permission analysis.
- [README.md](README.md) — Install, verify, and operator commands.
