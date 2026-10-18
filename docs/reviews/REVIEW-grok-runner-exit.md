# REVIEW: grok runner exit and Grok Build cost (fix/grok-runner-exit)

Date: 2026-10-19 (KST)
Spec: [SPEC-grok-runner](../specs/SPEC-grok-runner.md) section 9, FR-913..FR-917, TC-913..TC-918
grok CLI: 1.0.62 (`~/.grok/bin/grok`)

## Symptoms

- A. Long `grok -p` runs print the final summary and the `end` line, then never exit. DmuxAI waited for EOF until the 900 s timeout.
- B. Grok Build is slow and token-heavy. Implementation session `01a14f3a`: 136 model calls, 16.37M input tokens (15.88M cached, 97%), 178k output, about $10.81. Spec session `01a14f1e`: 7.73M input, 64 calls.
- C. Grok text before and after a tool call ran together ("...next.Finished").

## Root cause A (evidence)

Repro prompt: start `sleep 6; echo BG_FINISHED` with `block_until_ms` 1000 (moves to the background), run a foreground `sleep 12`, then reply. Four variants ran in parallel at 00:19 KST with `</dev/null` and stdout to a file.

| Variant | Result |
| --- | --- |
| default | hangs after `end` |
| `--background-wait-timeout 15` | hangs after `end` |
| `--no-wait-for-background` | exits after 23 s with `end` |
| `GROK_AUTO_WAKE=0` | exits after 27 s with `end`, no wake prompt queued |

Hung process (default variant), 2 minutes after its `end` line:

- `ps`: state `Sl`, wchan `futex_wait_queue`, 0 children.
- `lsof`: fd 0 `/dev/null`, fd 1 a regular file, so neither stdin nor a child holding a pipe is involved.
- `strace -c` over 6 s: 57 threads, all idle in `futex`/`epoll_pwait`.

grok's own log (`~/.grok/logs/unified.jsonl`) for the hung pid:

```text
00:19:12.976 shell.prompt.queued {"prompt_id": "task-completed-call-1027…-0", "queue_depth": 1}
00:19:24.346 shell.handle_prompt.done {"prompt_id": "371f…", "ok": true}
(no handle_prompt.start for the task-completed prompt, no session_end.worker_join)
```

The variants that exit log `session_end.worker_join` right after `handle_prompt.done`. So: when a backgrounded command finishes while the user turn is active, auto-wake queues a synthetic `task-completed-<id>` prompt. After the turn ends, headless mode never starts that prompt and never reaches session end. `--background-wait-timeout` only limits waiting on running tasks, and nothing is running. The hidden flag `--no-wait-for-background` is documented in `~/.grok/docs/user-guide/14-headless-mode.md` ("Workflows and Background Work"). `GROK_AUTO_WAKE` is the env form of `features.auto_wake`.

In the Phase 1/2 grok-runner sessions, cargo commands were started with `block_until_ms` 0. Every cargo run was a background task, so long sessions hit this almost every time.

## Root cause B (evidence)

From the implementation session's events and stdout:

- About 90% of wall time was model inference (2369 s of 2630 s).
- Context per call grew to 120k-204k tokens (median 122k). Each of 136 sequential calls re-sent it, with four compactions. Caching keeps the price down but not the latency.
- Tools: `read_file` 165 calls (2.1 MB of output). The 13.6k-line `src/app.rs` alone was read 53 times in chunks. `grep` was called 95 times (1 MB).
- `reasoning_effort` high on every call.
- Background cargo runs (above) also added wake-ups and hangs.

Before/after on the same read-only question ("how does a custom-agent turn end…"), in two detached worktrees at `a4c50ae`, run in parallel at 00:28 KST. Numbers are from `grok usage <session_id>`.

| Run | Flags and prompt | Wall | Model calls | Input tokens (cached) | Output | Tool calls | Cost |
| --- | --- | --- | --- | --- | --- | --- | --- |
| before | default effort, open question | 221 s | 9 | 369,172 (309,888) | 16,106 | 27 | $0.37 |
| after | `--effort medium --max-turns 15 --no-wait-for-background`, `GROK_AUTO_WAKE=0`, prompt names the file and functions, rg first then ranged reads, at most 150 words | 40 s | 3 | 52,125 (19,072) | 2,834 | 2 | $0.09 |

Both answers were correct. The open run also noticed the unbounded stderr drain, which this PR fixes too (FR-914). Scoped turns trade breadth for 5.5x faster and 7x fewer input tokens. Guidance: [grok-build-delegation.md](../agents/grok-build-delegation.md).

## Fix

| Area | Change | Requirement |
| --- | --- | --- |
| grok preset | env `GROK_AUTO_WAKE=0`; argv `--no-wait-for-background` before the mode flag; init example updated | FR-913 |
| custom agents | `done_when`: on the matching line, stop reading; 3 s grace for exit; then TERM to the process group, KILL after 2 s more; TERM to the group after a clean exit too; success as if exit 0; stderr drained at most 2 s | FR-914 |
| custom agents | no `done_when` or no matching line: EOF and exit as before; 900 s timeout stays as last resort | FR-915 |
| custom agents | `text_break_when` (grok `tool_call`): the next text gets a blank line when needed | FR-916 |
| docs | delegation guide | FR-917 |

Process groups: commands that grok runs get their own session and group (observed `pgid == sid == pid` for `sleep 92` and `sleep 15` under grok pid 311084). DmuxAI's group signal does not reach them. With `--no-wait-for-background`, grok stopped a 90 s background `sleep` itself when it exited (checked twice, under DmuxAI and raw). The group signal covers children that stay in the runner's group, which is the case the mock tests.

## Tests and gates

- New: TC-913 `grok_preset_avoids_stranded_auto_wake`, TC-914 `grok_end_line_finishes_turn_and_reaps_group`, TC-915 `grok_orphan_holding_stdout_does_not_block_turn`, TC-916 `grok_turn_without_end_line_completes_at_exit`, TC-917 `append_reply_text_breaks_paragraphs_once`, TC-918 `grok_text_after_tool_call_starts_new_paragraph`.
- The mock gained `linger` (end line, then idle while a child holds stdout), `orphan`, and `noend`. Piping the mock's `orphan` output into `head` hangs the shell, which shows the mock really holds the pipe.
- `cargo fmt --check`, `cargo clippy --all-targets -D warnings` (Linux and `x86_64-pc-windows-gnu`), `cargo test`: pass.

## Real smoke (dmuxai `run --agent grok --mode implement`, isolated config and state, grok-4.7)

| Turn | Result |
| --- | --- |
| Background `sleep 15; echo BG_DONE > bg.txt` (`block_until_ms` 1000) finishing during a foreground `sleep 20` (the exact hang trigger) | exit 0 after 37 s; `bg.txt` written; reply `Starting the check.\n\nThe first command is in the background. Running the foreground wait next.\n\nFinished the check.\nSMOKE_DONE`; no grok or sleep left |
| Background `sleep 90` left running, reply at once | exit 0 after 8 s; reply `…end the turn.\n\nBG_LEFT`; the `sleep 90` was gone afterwards |

## Reviewer input and decisions

Reviewers: opencode free models only. `nemotron-3-ultra-free` answered both rounds. `space-bunny-free` answered neither: it was killed after 10 min in round 1 and timed out at 7 min (exit 124) in round 2. `deepseek-v4.1-flash-free`, `glm-5.3-flash-free`, and `beam-free` were unresponsive or IP-flagged earlier the same night, so they were not retried. The cursor-agent `sonnet-5.5-high` fallback was not needed.

### Round 1: root-cause second opinion (nemotron-3-ultra-free)

Input: this evidence (ps/lsof/strace, the unified log excerpts, the session statistics) and both hypotheses.

| Point raised | Decision |
| --- | --- |
| A and B are strongly supported. Gap: is it one wake per background command? | Agreed. Each repro variant logged one `task-completed` prompt for its one background command. |
| Killing the process group will not reach grok's background commands, because grok puts them in a separate group | **Confirmed by experiment** (each command gets `setsid`). Documented in FR-914 and above. With `--no-wait-for-background`, grok stops its own background tasks when it exits (verified). The group signal remains for children in the runner's own group. |
| Lower the effort, avoid background cargo, scope reads, trigger compaction earlier | Adopted in the delegation guide, then measured (before/after table). DmuxAI does not change the preset's effort level; that is left to the user. |
| Have DmuxAI cache or summarize tool output | Rejected: grok's tool loop runs inside grok, and DmuxAI only sees streaming-json. |

### Round 2: diff review (nemotron-3-ultra-free)

| Finding | Decision |
| --- | --- |
| H: a second `child.wait()` after a timed-out one may be unsafe | Rejected: tokio documents `Child::wait` as cancel-safe and callable again. |
| H: on Windows the group kill does nothing | Pre-existing (FR-897 non-Unix cancel matches the other runners). After the end line, `start_kill` still stops the leader. The comment now says so. Windows is not a supported platform for v0.2.x runtime checks. |
| M: timing thresholds are tight | Accepted: linger is now under 8 s; orphan is under 2.5 s, still below the 3 s grace so the test proves no grace wait. |
| M: the 2 s stderr cap may lose output | Rejected: stderr is read from spawn onward, and the cap only starts after the process has exited, when its stderr is already in the pipe. |
| M: `json_matches` never matches non-string values | Same semantics as the existing `text_when`. `text_when` now calls `json_matches`, so there is one rule. |
| L: tests are Linux-only; `start_kill` is redundant; the `pgid_value` closure | Linux-only, like TC-898, because they read `/proc`. `start_kill` is kept for non-Unix. The closure stays (readability). |
