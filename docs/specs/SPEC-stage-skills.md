# SPEC: stage-skills (map skills to graph stages)

Status: Draft (spec only, not implemented). Requested by Dongjin 2026-10-22 KST.
Owner: donggrri
Created: 2026-10-22
Baseline: `1b9861a` (origin/main)
Related: SPEC-plan-gate (judge, plan, approval), SPEC-agent-graph-orchestration (NodeRole), SPEC-acp-runners, SPEC-grok-runner, SPEC-model-policy, `src/skills.rs`, design note `ACP-DEFAULT-AND-QUESTIONS` (question card)

## 1. Decision summary

Users can attach any Agent Skill (a folder with `SKILL.md`) to a dmuxai stage, for example Matt Pocock's `grill-me` to planning. When a turn for that stage starts, dmuxai hands the skill to the runner, either through the runner's native skill folder or by putting the SKILL.md text into the stage prompt. Runners are decided: Cursor legacy (headless), agy headless, Grok ACP, opencode headless, codex.

## 2. Today on main (`1b9861a`)

- **Stages.** The graph pre-steps are `judge` (simple or complex verdict) and `planner` (strict JSON plan), then `approval`, which runs no model. After that come the node roles in `NodeRole` (`src/graph/types.rs`): `orchestrate`, `research`, `explore`, `test`, `implement`, `draft`, `plan`, `route`, `review`. A simple prompt runs as a single node with no plan.
- **Config.** `dmuxai.toml` in the workspace and the user config, parsed in `src/settings.rs` (`DmuxToml`). There is no stage-level prompt or skill setting.
- **Skills.** `src/skills.rs` discovers `<workspace>/.agents/skills/<id>/SKILL.md`. When the user picks a skill, `app.rs:9778` adds only `[Skill: name]\ndescription` before the prompt. The SKILL.md body is not sent.
- **CLI discovery seen on the box:**
  - cursor-agent 2026.10.19 has slash and skill expansion in print mode (`--disable-slash-commands` turns it off).
  - The grok 1.0.67 binary mentions `.grok/skills`, `.agents/skills` and `.claude/skills`.
  - The agy 1.2.2 binary mentions `.agents/skills` and `~/.gemini/config/skills`.
  - opencode 1.19.3 is listed by its docs as reading `.opencode/skill`, `.claude/skills` and `.agents/skills`. Not yet verified on the box.

## 3. Config

```toml
# dmuxai.toml (workspace) or the user config
[skills.sources]
grill-me = { git = "https://github.com/mattpocock/skills", path = "grill-me", rev = "<commit-sha>" }  # pinned
local-review = { path = "./skills/review-checklist" }

[stages.planner]          # graph pre-step
skills = ["grill-me"]
[stages.judge]
skills = []               # judge stays strict JSON; skills are rejected here unless forced
[stages.review]           # NodeRole
skills = ["local-review"]
[stages.implement]
skills = ["tdd"]          # resolved from discovered dirs
delivery = "auto"         # auto | native | inject
```

Per-task override: `/plan --skills grill-me,-tdd <prompt>`. The plan JSON step can also carry `skills: [...]`, which the approval card shows and lets the user edit.

## 4. Requirements

| ID | Requirement |
|---|---|
| FR-1500 | `[stages.<stage>] skills = [..]` is accepted for `judge`, `planner` and every `NodeRole` name. An unknown stage name is a startup config problem, not a crash. |
| FR-1501 | A skill reference resolves in this order: an explicit `[skills.sources]` entry, then a workspace path (`./...`), then discovered dirs `.agents/skills`, `.claude/skills`, `.cursor/skills`, `.grok/skills`, then the user-level dirs `~/.agents/skills` and `~/.claude/skills`. The first match wins, and the resolved path is shown. |
| FR-1502 | A git source needs `rev` as a full commit SHA. dmuxai never downloads an unpinned or branch-only source. A fetch is an explicit `dmuxai skills sync` command, never automatic at run time, and it stores the result under the dmuxai state dir by SHA. |
| FR-1503 | Delivery `auto` uses `inject` for every runner. `native` is opt-in per runner (FR-1504). This keeps behaviour identical across Cursor, agy, Grok, opencode and codex. |
| FR-1504 | `native` delivery links (or copies on Windows) the resolved skill folder into a per-run temp dir that the runner reads (see table in §5). It never writes into the user's repo or home skill dirs. |
| FR-1505 | `inject` puts the full SKILL.md body, without frontmatter, into the stage prompt inside a `<skill name=…>` block, before the task text. Referenced files are listed by path, not inlined. |
| FR-1506 | Limits: 32 KB per SKILL.md and 64 KB per stage in total. If either limit is exceeded, preflight fails with a clear message. Nothing is truncated silently. |
| FR-1507 | Preflight runs before the judge. It lists missing, unpinned, oversize or unreadable skills. A missing skill on a required stage blocks the run. With `optional = true`, the run continues with a warning. |
| FR-1508 | Judge output must stay strict JSON, so skills on `judge` are rejected unless `force = true`. A judge with injected skills still uses the existing JSON fallback rules. |
| FR-1509 | Per-task overrides (`/plan --skills`, a plan step's `skills`) add to or remove from the stage mapping (`-name` removes) for that run only. They are recorded in the run record. |
| FR-1510 | Every node record stores the skill ids, resolved source, SHA or file hash, and delivery mode used. |
| FR-1511 | The existing `.agents/skills` picker (`app.rs:9778`) switches to `inject` of the full body under the same limits. |
| FR-1512 | A skill that asks the user questions (for example `grill-me`) works through the question layer. Grok ACP `_x.ai/ask_user_question` shows a blocking card. On headless runners, text questions are detected at turn end and answered next turn with choice chips. The stage stays open across these turns until the agent marks it done or the user ends it. |
| UX-1500 | TUI `/skills` shows the stage→skill map with source and status (ok, missing, unpinned, oversize). The dashboard Settings page shows and edits the same map, writing `dmuxai.toml` after confirmation. |
| UX-1501 | The plan approval card shows the skills of each step and lets the user remove or add them before approving. |
| NFR-1500 | Skill text is treated as untrusted prompt content. It never changes runner permissions, the model policy, the allowed runners or the sandbox. |

## 5. Per-runner delivery

| Runner | Native skill dir (FR-1504, opt-in) | Default (inject) | Notes |
|---|---|---|---|
| Cursor legacy (`cursor-agent --print`) | `.cursor/skills` / `.agents/skills` in the temp workspace overlay | yes | Native needs an overlay because the cwd is the repo. Keep `--disable-slash-commands` off only when native. |
| Grok ACP | `.grok/skills` or `.agents/skills` | yes | Questions go through `_x.ai/ask_user_question`. |
| agy headless | `~/.gemini/config/skills` (user level) | yes | Native would touch the home dir, so inject only. |
| opencode headless | `.opencode/skill` / `.agents/skills` | yes | To verify on the box. |
| codex | `.agents/skills` (to verify) | yes | |

## 6. Tests

| ID | Test |
|---|---|
| TC-1500 | Config with stages planner, review and an unknown stage parses, and the unknown one is reported (FR-1500). |
| TC-1501 | Resolution order and resolved path with fixture dirs (FR-1501). |
| TC-1502 | A git source without a SHA is rejected. No network at run time (FR-1502). |
| TC-1503 | Inject block contents and order in the planner prompt, frontmatter stripped (FR-1503, FR-1505). |
| TC-1504 | Native mode creates a temp overlay and leaves the repo and home dirs unchanged (FR-1504). |
| TC-1505 | Size limits block the run with a message (FR-1506). |
| TC-1506 | Preflight blocks on a missing required skill and continues on an optional one (FR-1507). |
| TC-1507 | A judge skill is rejected without `force` (FR-1508). |
| TC-1508 | Override add/remove and the run record (FR-1509, FR-1510). |
| TC-1509 | The picker injects the full body (FR-1511). |
| TC-1510 | Hands-on: grill-me on planner with Grok ACP (card) and Cursor legacy (next-turn chips) (FR-1512). |
| TC-1511 | UI map, the edit round-trip and the approval card skills (UX-1500, UX-1501). |
| TC-1512 | Skill text that asks for `--yolo` or a different model changes nothing (NFR-1500). |

## 7. Open questions

- OQ-1500: Should the default stays-open behaviour for question skills (FR-1512) end after N rounds (proposed: 5) or only on explicit user end?
- OQ-1501: Is a pinned git source enough, or should the first use of a new SHA also need a one-time approval in the UI?
- OQ-1502: Should skills attach only to graph stages, or also to a free chat turn (single node) by default?
