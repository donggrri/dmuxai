# Delegating work to Grok Build

How to run Grok Build (`grok`) for delegated repository work so turns end on
time and use fewer tokens. SPEC-grok-runner FR-917. Measured on grok 1.0.62.

## Invocation

```sh
GROK_AUTO_WAKE=0 grok -p "$(cat prompt.md)" -m grok-4.7 --cwd "$WORKTREE" \
  --output-format streaming-json --no-wait-for-background \
  --effort medium --max-turns 40 --always-approve </dev/null > turn.jsonl
```

- `GROK_AUTO_WAKE=0` and `--no-wait-for-background` stop the hang where a
  background command finished during the turn and queued a prompt that
  headless mode never runs. Without them the process can sit idle forever after
  printing its `end` line. The DmuxAI grok preset sets both.
- Treat the `end` line (`{"type":"end",...,"sessionId":...}`) as the end of
  the turn, not EOF. Keep a wall-clock timeout as a last resort only.
- `</dev/null` keeps grok from waiting on a terminal.
- `--effort medium` (`none`, `low`, `medium`, `high`, `max`): high effort
  spends more output and time per call. Use `high` only for design or tricky
  debugging turns.
- `--max-turns N` caps a turn that wanders. `-r <session_id>` continues a
  session for the next small step.
- Never pass the fast model; DmuxAI's model policy rejects it.

## Why big turns are slow

Each model call re-sends the whole context. In the v0.2.2 grok-runner
implementation session (one `grok -p` turn) `grok usage` reported 136 calls,
16.37M input tokens (15.88M cached, 97%), 178k output tokens, and about
$10.81. Context grew to 120k-200k tokens per call with four compactions.
Model inference was about 90% of the wall time. The tool log shows where the
context came from: 165 `read_file` calls (the 13.6k-line `src/app.rs` alone
read 53 times in chunks, 2.1 MB of output) and 95 `grep` calls (1 MB).
Cargo commands were started in the background, which also triggered the hang
above.

## Writing smaller turns

1. One concern per turn: spec, then one module, then tests, then docs. Resume
   the session with `-r` or start a fresh one per step; a fresh session starts
   with a small context.
2. Name the files, functions, and line ranges to read. Ask for `rg -n` first,
   then `read_file` with offset and limit. Large files such as `src/app.rs`
   should never be read whole.
3. Run cargo in the foreground and filter its output, for example
   `cargo test --lib grok 2>&1 | tail -20` or
   `cargo clippy ... 2>&1 | rg -A5 '^(error|warning)'`. Do not background
   builds.
4. Ask for short answers ("at most 150 words", "diff only").
5. Measure each turn: `grok usage <session_id>` prints totals and every turn;
   `grok usage <session_id> <turn>` prints one turn.

## Before and after

See [REVIEW-grok-runner-exit.md](../reviews/REVIEW-grok-runner-exit.md) for the
measured comparison of a default prompt and a scoped prompt with these flags.
