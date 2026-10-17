# Graph execution report: input + redraw (dogfood #15, #17, #19)

| Node | Role | Runner / model | Output |
|---|---|---|---|
| spec+impl | implement | DmuxAI (worktree `dmux-wt/fix-input-redraw`, tmux `dmux-redraw`) → AGY `gemini-3.8-flash-medium` | SPEC/PLAN/TASKS/REVIEW-input-redraw, slash-command parser with args, unknown-command guard, `//` escape, 1s busy tick, forced draw on completion, last-event reset on activity |
| fix | review | AGY | struct inside impl, borrow and test type errors |
| fix | review | orchestrator | absolute paths (`/tmp/x.png explain`) were rejected as unknown commands and the relative image-path attach was dropped → `first_token_is_path` guard + attach restored + regression test; test asserted wrong default agent |
| live | test | box tmux | fast `/agent agy`+Enter switches without sending; turn end repaints IDLE without a key press |
| gate | test | cargo | fmt, clippy, test --all-targets green |
