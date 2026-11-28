# REVIEW: Ask in the work session (hybrid resume)

Spec: [SPEC-board-ask-resume](../specs/SPEC-board-ask-resume.md) ·
Plan: [PLAN-board-ask-resume](../plans/PLAN-board-ask-resume.md) ·
Tasks: [TASKS-board-ask-resume](../tasks/TASKS-board-ask-resume.md)

Scope: FR-3830..FR-3836, NFR-3830.

## Spec compliance

| Requirement | Verdict | Evidence |
| --- | --- | --- |
| FR-3830 | Met | `run --conversation <id> --agent A` selects the stored conversation and sends one agent turn through the existing `begin_turn` path. No judge, planner or spec-first runs: the direct agent path, `headless_no_spec_first`, and no summary jobs. The provider session is resumed when the binding is active and the delta is small; otherwise the turn is fresh. JSON keeps `conversation_id` and adds `resume {mode, agent}`. Rejected resume: the existing FR-2325 retry retires the binding, resends with the summary seed and reports `summary`. Unknown id → exit 3 `not_found`. Malformed id, `--graph`, `--agents` or no `--agent` → exit 2 before any store access. MCP `run` `conversation_id` → `--conversation` (validated first). TC-3830, `tc_3830_run_conversation_flags_and_mcp_forwarding`. |
| FR-3831 | Met | The fresh prompt starts with `Work session summary`. The summary holds the stored summary document, plan steps with status and last reply (≤ 1 500 chars each), Markdown paths and the thread's last 10 messages. It is capped at 12 000 chars and masked with `mask_sensitive_tokens`. Provider session ids are never read into it. TC-3831 (integration and unit). |
| FR-3832 | Met | `board ask --resume` (CLI, MCP `board_ask`, HTTP body):<br>• `auto` runs `run --conversation <work conv> --agent <OQ-3831 agent> --read-only --json --board-item <id> -` in the item workspace.<br>• `native` without an active binding → conflict exit 4, or HTTP 409 with `reason: no_active_binding`.<br>• `fresh` adds `--fresh-session`.<br>• `off` keeps the FR-3132 ask.<br>• No work session: `auto` behaves as `off`; `native`/`fresh` → usage.<br>• `--revise` ignores `--resume`.<br>TC-3832; HTTP part in `board_http_ask_and_run_view_only_vs_web`. |
| FR-3833 | Met | The planner message is prefixed `[native resume · A]` / `[summary resume · A]`. The mode comes from the child's `resume.mode`, else the mode planned from the binding state. CLI/MCP JSON and the HTTP `202` (planned) carry `resume`. The ask link records the work conversation id; if the child prints none, the planned id is used. TC-3833. |
| FR-3834 | Met | Lane `in_progress` with an active run lock: `auto`/`fresh` pass `--fork-summary` (new conversation seeded with the summary; the running conversation and provider session are never touched), and the answer says "(new conversation; a board run is in progress)". `native` → conflict. TC-3834. |
| FR-3835 | Met | Every resume child argv has `--read-only` and never `--allow-sensitive`/`--trust-workspace-config` (argv and env checked). Asks write only thread and session data, never a decision or check. TC-3835. |
| FR-3836 | Met | `#board-chat-resume` offers Work session (auto) / Fresh with summary / Item only, mapped to `auto`/`fresh`/`off`. It is hidden without a work session and in view-only `serve`. It sends `resume` with plain asks only. Answers show a `Native/Summary resume · A` badge. TC-3836 (asset test and headless-Chrome e2e). |
| NFR-3830 | Met | `run` without `--conversation` only adds a few `Option` checks; board list/show are unchanged. A native resume makes exactly one provider call (TC-3830 counts `--print` spawns). Summary jobs are skipped for continuation turns. |
| Non-goals | Respected | No edit turns from the chat, no cross-family session transfer, `.github/` untouched, no `--allow-sensitive`/`--trust-workspace-config`. Spec requirements unchanged; only the Status line was flipped. |

## Commands and results

- `cargo fmt` / `cargo fmt --check` — clean
- `cargo clippy --all-targets --all-features -- -D warnings` — clean
- `cargo test --lib board` — 87 passed
- `cargo test --test run_conversation` — 2 passed
- `cargo test --test spec_index` — 16 passed
- `cargo test --test web_board_e2e -- --ignored tc_3836` — 1 passed (headless Chrome, not skipped)
- Regression checks:
  - `cargo test --test web_board_e2e -- --ignored tc_3601 tc_3825` — 3 passed
  - `cargo test --lib` — 959 passed, 1 ignored
  - `cargo test --bin dmuxai` — 734 passed, 6 ignored
  - `cargo test --test agent_api_cli` — 24 passed
- `bash scripts/check-spec.sh docs/specs/SPEC-board-ask-resume.md` — PASS
- `python3 scripts/check-spec.py --all` — 119/119 pass
- `python3 scripts/check-tc-links.py` — 0 orphan, 0 undefined
- Not run:
  - the full web e2e suite (out of scope);
  - live providers (deterministic fakes only).

## Residual risks

- In summary mode the prompt carries the work-session summary and then the
  existing restored projection, which FR-3830 asks for. The projection is not
  re-masked, the same as any other fresh turn. Only the summary section is
  guaranteed masked.
- The "native" vs "summary" label in the HTTP `202` is predicted from the
  binding at prepare time. The thread message uses the child's actual mode.
- An ask whose child crashes still shows the planned mode in its prefix.
- Asks during a live run create one new conversation each.
- MCP `board_ask` `resume` parsing has no dedicated test. It shares
  `ResumeMode::parse` and `ask_item_report` with the tested CLI path.
