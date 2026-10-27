# REVIEW: v0.3.0 release audit

Date: 2026-10-27 (Asia/Seoul). Base: `main` at 1ce1e97 (#223), plus the release PR. Requested by Dongjin 2026-10-27 12:47 KST.

## Scope

All 72 commits on `main` since v0.2.2 (d51e634). The user-facing list is the `[0.3.0]` section of CHANGELOG.md.

Not in this release (open, not on `main`): #222, #219, #197, #78, #79, #68.

<details><summary>Commits v0.2.2..1ce1e97</summary>

- 1ce1e97 refactor(web): serve the dashboard from a one-entry workspace directory (#223)
- 65818ff refactor(web): route dashboard API requests through one URL helper (#221)
- 99059a9 refactor(supervisor): tag live process groups with an owner id (#220)
- 75813b6 fix(conversation): busy reply save is retried; a lost reply fails the run and keeps the text (#203) (#206)
- b8b1b5c feat(agent-prompt): per-model/per-runner system prompts with built-ins for AGY Gemini flash, Cursor composer-2.5, Cursor grok-4.7 and Grok Build (#210)
- 35ce359 feat(model-policy): Cursor accepts non-fast models such as grok-4.7; fast ids stay blocked (#209)
- efa208e fix(grok): a v1.0.62 `grok models` listing with a grok- id counts as logged in (#207) (#208)
- 5f7912d fix(fallback): runner handover correctness — no stale chain, no stacked prefaces, finish once, apply model, final-runner text, field-wise config merge (F1/F2/F4/F8/F10) (#205)
- d3a9a70 fix(custom-agent): empty reply after workspace edits succeeds; catalog hint no longer forces model_unavailable (#146) (#199)
- 6ed5f92 fix(conversation): retry busy DB open, no write txn on current schema, exit 4 when still locked (#167) (#196)
- 2b8a681 fix(app): one startup runner-selection helper; no panic on removed custom agent (#145, #171) (#187)
- 6cb49c4 fix(gate): run cargo test with --no-fail-fast so one flaky target can't hide the rest (#198) (#200)
- 0dae2e9 fix(gate): run tests/web_*.rs targets and isolate XDG_CONFIG_HOME (#154, #161) (#186)
- 2cf469e feat: runner fallback on provider failure/stall + agy fixes (#151, no sandbox, prompt rules) (PR-10) (#182)
- 81cbd37 refactor: remove MCP propagation to runners (PR-9) (#181)
- ee7db05 docs(skill): make agent-graph-engineering model-agnostic (#180)
- 374ced8 refactor: input_command classifier; headless no longer borrows TUI input (PR-8d; scan #10) (#179)
- 1f63208 refactor: GraphDeferQueue owns graph defer state (PR-8c; scan #5) (#178)
- 7eddd1c refactor: TodoLinks module owns to-do link state (PR-8b; scan #7) (#177)
- b1eb6a4 refactor: internal turn core for summary/triage/plan/replan (PR-8a; scan #1) (#176)
- 18f2913 refactor: MCP injection seam, no workspace files (PR-7; scan #4; #113) (#174)
- a36d5ab refactor: runners on the shared process supervisor (PR-5b; scan #2 #8) (#169)
- 3058c2e refactor: single activity merge rule + one PlanContext builder (R-1; scan #3 #6) (#168)
- 376e86d refactor: turn record — display text + durable activity rows (PR-6; #99 #100 #132) (#165)
- 1e813d8 refactor: shared runner process supervisor (PR-5; #123 #162) (#163)
- 93b1fe6 refactor: headless direct path + progress + flags (PR-4; #134 #137 #114) (#160)
- eea3048 refactor: single model resolver + catalog check + headless --model (PR-3; #135 #139 #136) (#159)
- d504f0a fix(custom-agent): error_path fallback only for an error container, not a success /result object (#156) (#158)
- ab91827 feat: per-agent system prompt (PR-2.5) (#157)
- 856d3fc refactor: typed runner errors + error_path list + graph error order + cursor dedupe (PR-2; #133 #134 #142) (#153)
- de61d1b docs(specs): fill behavior gaps, fix 26 legacy specs, record code-vs-spec mismatches (#152)
- da1f268 test: spec traceability gate and local gate script (#150)
- df1cf46 refactor(env): RuntimeEnv for settings, --config/DMUXAI_CONFIG, one child-env policy, shared test env lock (#140, #138) (#149)
- 27a1b8d feat(web): top bar Chat button to return from Graph/Diff/Memory (#144)
- 70a0d02 fix(dashboard): keep a background turn's activity rows across conversation switches (#132) (#143)
- d0dd84b fix(dashboard): keep a background conversation's graph card live (#131) (#141)
- 75954e1 fix(custom-agent): timeout error names timeout_secs; default 1800s (#117) (#126)
- 36eff61 docs(spec): drop removed --mode from SPEC-agent-api and API docs (#119) (#121)
- f551579 fix(custom-agent): infer jsonl output from JSON-path fields (#116) (#125)
- be68ab1 feat(web): run turns in several conversations at once (FR-1620..1625) (#122)
- d36aff2 fix(api): expose awaiting_approval turn state (#108) (#130)
- 7ed2d71 fix: pin explicit runner, honour allowed_runners on resume, read-only agy questions (#112, #101, #106) (#129)
- 427f16b fix: headless sensitive gate with explicit agent; clear inherited DMUXAI_MCP_CONFIG (#110, #118) (#128)
- c263506 ci: drop Windows target lint and Windows release job (#120)
- 7c943d1 chore: drop Windows support for now (revertable) (#115)
- 39741c0 fix: opencode runner without --dir; short prompts skip the triage judge (#109)
- 5dcdfed feat(web): dmuxai --web runs the dashboard only, no TUI (#105)
- a94d4b5 docs: release-process version bump details (#97)
- 9fdf33d fix(agy): accept bare gemini-3.8-flash; ACP client version from Cargo (#98)
- a89eddd docs(spec): stage-skills — map skills to graph stages (spec only) (#93)
- efa17f4 docs: add release process and manual TC template (#96)
- 27864f8 docs(spec): spec/test coverage audit for #91/#92/#94; Open needs a validated path (#95)
- 1da1c25 fix(web): 'Open in new window' starts a headless dashboard instead of hanging (#94)
- b2d59fe fix: grok follow-ups use per-runner default model; dashboard port fallback (#92)
- 1b9861a feat(acp): cursor/grok ACP round 5 (supersedes #78, #79) (#91)
- 0c6ec26 feat(web): show prompt and progress when a task is selected (#90)
- ee6c896 feat(web): open another workspace dashboard without stopping this one
- bde438f feat(memory): default notes directory to <vault>/memory
- bf64695 Add contributing docs, security policy, templates, and gitleaks CI (#89)
- d514b38 ci: run baseline spec traceability with Python on ubuntu-latest (#88)
- b3e50b3 chore: add MIT LICENSE and note it in README (#87)
- 2e29ced feat(web): send follow-ups while a reply is streaming (#86)
- 3d35480 fix(web): send a chat prompt once and show the user message once / 채팅 메시지 중복 표시 수정 (#82)
- 6d99c0f feat(web): syntax highlighting + line numbers in the Files code view / Files 코드 뷰 문법 강조와 줄 번호 (#83)
- 7b840fa feat(web): composer model menu with search, Default model and effort submenu (#85)
- 9fb19f0 feat(web): copy button on AI replies in the dashboard chat (#84)
- 44f0d55 feat(web): image attachments in the dashboard chat (#81)
- 924d2b9 Remove execution modes, dashboard GFM chat, composer agent picker (#80)
- a1538e3 feat(web): full-width center, ChatGPT-style chat, collapsible Documents (no duplicated replies) (#76)
- 5e60663 fix(graph): keep the implicit task ref on fallback plan revisions (#77)
- a13b7b2 ci: release workflow manual-only (Actions billing-blocked) (#75)
- a4d1b9b feat: plan approval gate + agent-planned dynamic graph (v0.2.3) (#67)

</details>

## Step 1: spec audit

Method: a throwaway script read every `docs/specs/SPEC-*.md` and DMUXAI-BASELINE.md (81 files, 845 requirement rows, 926 TC rows). It checked that each requirement in an implemented SPEC is covered by a TC row, and that each test name in backticks in a TC row exists in `src/` or `tests/`.

Requirements with no TC row (implemented SPECs):

| Requirement | SPEC | Note |
| --- | --- | --- |
| UX-760 | SPEC-agent-api | Manual docs review; the README entry points should be rechecked now that MCP propagation is removed (#181) |
| FR-917 | SPEC-grok-runner | Docs-only (delegation doc) |
| NFR-932 | SPEC-plan-gate | Latency budget; no automated TC |
| FR-1513 | SPEC-stage-skills | Dashboard stage-skills write; no TC row |

Test names in TC rows that do not exist (not marked *proposed*):

| TC | SPEC | Missing name |
| --- | --- | --- |
| TC-431 | SPEC-graph-allowed-runners | `routing_default_not_allowed_falls_back` |
| TC-432 | SPEC-graph-allowed-runners | `empty_allowed_runners_planner_falls_back_to_single` (the second listed test exists) |
| TC-505 | SPEC-implement-plan-only | `finish_accepts_forced_kill_after_result_in_implement_mode` (other listed tests exist) |
| TC-960 | SPEC-plan-gate | `approval_panel_in_headless_chrome` |
| TC-962 | SPEC-plan-gate | `live_judge_and_plan_smoke` (ignored live test) |

Already marked *proposed* in the SPECs (carried over from earlier releases): TC-303, TC-305, TC-315, TC-324. TC-304 is deferred with Windows (#115).

SPECs whose Status is not Implemented: SPEC-acp-runners (in progress, #78/#79), SPEC-agent-graph-orchestration (draft), SPEC-remove-modes-chat-markdown (draft), SPEC-tui-stabilize (draft), SPEC-agent-modes-copy (superseded), and SPEC-plan-gate (marked "implemented in tree", status line not updated).

None of these gaps blocks the release. They are traceability gaps (stale test names, missing TC rows), not behavior gaps found in the code.

## Steps 2 and 5: tests and running the build

| TC / check | Evidence | Result |
| --- | --- | --- |
| `scripts/gate.sh` (fmt, clippy, `cargo test --all-features`) with an empty temp XDG config | 1192 passed, 0 failed, 38 ignored | pass (second run). The first run failed only on the known flaky `tc_1798_group_alive_and_guard_registry` (#175) while a release build ran in parallel. |
| Browser tests `tests/web_*.rs -- --ignored` (headless Chrome) | 30 passed, 0 failed | pass |
| Static musl build `cargo build --release --locked --target x86_64-unknown-linux-musl` | `ldd` shows no dynamic libraries | pass |
| `dmuxai --version` | `dmuxai 0.3.0` | pass |
| `dmuxai --help` without a TTY, `dmuxai init` in a fresh repo, `dmuxai status --json` | exit 0, valid JSON | pass |
| `dmuxai --web` from the release binary | dashboard serves HTTP 200, process stopped afterward | pass |
| Release archive `dmuxai-linux-x86_64.tar.gz` (binary, README, CHANGELOG) and `.sha256` | sha256 `01728e23730f5e40a336ee80c75f86f4ecd17c3847598f9242d58e20d3ef84ef`, `sha256sum -c` OK, extracted binary prints 0.3.0 | pass |

## Step 6: UI screens

Not done yet. The process requires screenshots of the dashboard changes, taken with the Grok Bot desktop agent, for Dongjin to review.

## Step 7: release gate

Waiting for Dongjin's OK on this report and the screens. After the OK: squash-merge `release-candidate-v0.3.0` into `main`, tag `v0.3.0` on the merge commit, publish the release by hand, and attach the locally built Linux archive and checksum. GitHub Actions is not used.
