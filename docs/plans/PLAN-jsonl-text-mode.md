# PLAN: JSONL text_mode and lifecycle-based turn completion (#184)

Status: approved
Specs: [SPEC-jsonl-text-mode.md](../specs/SPEC-jsonl-text-mode.md)

1. Add `TextMode` (`last` | `concat`) and `text_mode` on `CustomAgentSpec`
   (FR-2530, FR-2531); extend `ReplyCollector` folding: `last` restarts the
   stored reply block after `tool_when` / `text_break_when` lines, `concat`
   joins parts with `\n` when no paragraph break is pending.
2. Add `finish_when` / `finish_reason_path` / `finish_ok` on
   `CustomAgentSpec`; `ReplyCollector` remembers the last `finish_when`
   reason and whether `done_when` ever matched. In `src/agent/custom.rs`
   `run_turn`, a JSONL turn with a clean exit is unfinished when the last
   reason is not in `finish_ok`, or (without `finish_when`) the `done_when`
   line never matched. New `AgentErrorKind::Unfinished` (`"unfinished"`,
   FR-2532, FR-2534). No reply-length or tool-count heuristic (FR-2535).
3. Opencode preset: `text_mode = concat`, `text_break_when` on `tool_use`,
   `finish_when` on `step_finish`, `finish_reason_path = /part/reason`,
   `finish_ok = ["stop"]` (FR-2533). Grok preset keeps `done_when` and uses
   `text_mode = concat`.
4. Add `unfinished` to the non-runner-fallback kinds in
   `src/runner_fallback.rs`.
5. Tests TC-2530..TC-2536 (unit in `src/custom_agent.rs`, integration on the
   fake agent in `src/agent/custom.rs`); save opencode stream fixtures with
   `step_finish` reasons; fake-opencode/grok fixtures keep representing
   completed turns with a terminal `step_finish`/`end` line.
6. Document `text_mode` and the finish fields in `README.md`,
   `docs/agents/custom-agents.md`, `SPEC-custom-agent-provider.md`
   (FR-2442), and `SPEC-grok-runner.md` (TC-916 now fails `unfinished` at a
   clean exit without the end line; TC-918 notes the groove preset `text_mode`).

## Risks

- A runner that exits 0 without its terminal line now fails (`unfinished`)
  instead of succeeding. Documented in both specs; fixtures updated to keep
  representing completed turns.
- Legacy users with `done_when` agents that exit 0 silently: same change.

## Rollback

Remove the `unfinished` check from `run_turn` (revert to exit/reply-only
failure classification) and `ReplyCollector` folding (revert to
unconditional append); the new spec fields then float unused.
