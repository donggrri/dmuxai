# SPEC: cursor-latency

Status: Draft (v0.2.1)
Created: 2026-10-17
Related: Dongjin dogfood 2026-10-17 ("Cursor is really slow in dmuxai"); SPEC-cursor-stream-dedupe

## 1. Decision summary

The question was whether dmuxai is slow with Cursor, or cursor-agent itself is slow. We measured both, on the same box, model and prompts, with a `PATH` shim around `cursor-agent` that logs the stdin bytes and the time to first JSON line, first text and `result`.

| Path | stdin bytes | first line | first text | result |
| --- | --- | --- | --- | --- |
| raw `cursor-agent -p … --model composer-2.5` (×3) | 38 | 2.6–2.8 s | 5.0–5.5 s | 5.1–5.6 s |
| `dmuxai run --agent cursor` (×3) | 38 | 2.5–2.9 s | 5.0–5.6 s | 5.1–5.7 s |
| TUI turn 1 (another cursor-agent was running at the same time) | 38 | 6.9 s | 9.5 s | 9.6 s |
| TUI turn 2 (`--resume`, prompt "Reply with one word: ok.") | 29 | 1.3 s | 3.9 s | 4.0 s |
| raw, prompt written to stdin right after spawn (×2) | 38 | 2.8 s | 5.2 s | 5.3 s |
| raw, process spawned 8 s before the prompt was written (×2) | 38 | 1.3 s | 3.2–3.8 s | 3.2–3.8 s |

Findings:

- dmuxai adds no measurable overhead. It does not resend history on resumed turns (the stdin size is the prompt only), and nothing in dmuxai polls or sleeps on the cursor path.
- A Cursor turn costs a cold `cursor-agent` start plus model latency. There is no persistent server mode, so dmuxai starts a new process for every turn.
- cursor-agent starts up before it reads stdin. A process that dmuxai has already spawned answers **1.4–2.1 s sooner** (about 30%).
- The wait before the first token has no on-screen marker. This is handled by SPEC-running-indicators.

Decision: pre-warm the next cursor-agent process (FR-805..FR-807) and document what is left as cursor-agent's own latency.

## 2. Requirements

| ID | Priority | Requirement | Tests |
| --- | --- | --- | --- |
| FR-805 | Must | When a Cursor turn ends, or when Cursor becomes the selected agent, the TUI/headless App spawns one warm `cursor-agent` process. It uses the arguments the next turn would use: mode, model, resume session id and workspace. Its stdin stays open and nothing is written to it yet | TC-805 |
| FR-806 | Must | On the next Cursor prompt, the warm process is used only if its arguments equal the arguments for that turn exactly. If they differ (mode, model or session changed), or the process has exited, it is killed and reaped, and a fresh process is spawned as before | TC-805, TC-806 |
| FR-807 | Must | A warm process is killed and reaped on quit, on agent removal, on `/new` and after `cursor.prewarm_idle_secs` (default 300) without use. dmuxai keeps at most one warm process per App. `[cursor] prewarm = false` turns the feature off | TC-806, TC-807 |
| NFR-805 | Should | A warm process never runs a prompt the user did not send. Pre-warm never runs for `dmuxai run` (single turn) or for graph nodes (cwd and arguments are per node) | TC-807 |

## 3. Tests

| ID | Requirement | Level | Fixture | Oracle | Test |
| --- | --- | --- | --- | --- | --- |
| TC-805 | FR-805, FR-806 | unit | fake cursor binary that records spawn time and argv | After a turn completes, a warm process exists with the next-turn argv; the next prompt reuses it (one spawn total for turn 2) | `src/agent/cursor.rs` `prewarm_reused_when_args_match` |
| TC-806 | FR-806, FR-807 | unit | fake binary | Changing the mode or model before the prompt discards the warm process (killed and reaped) and spawns a fresh one; `/new` discards it | `src/agent/cursor.rs` `prewarm_discarded_on_arg_change` |
| TC-807 | FR-807, NFR-805 | unit | fake binary, `prewarm = false` | No warm spawn when disabled, in headless `run`, or for graph nodes; idle expiry kills it | `src/agent/cursor.rs` `prewarm_disabled_and_expiry` |

## 4. Out of scope

cursor-agent's own startup and model latency. They are documented in the README (Cursor section) together with the numbers above.
