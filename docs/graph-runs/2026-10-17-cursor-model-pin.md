# Graph execution report: Cursor model pin (dogfood #18)

| Node | Role | Runner / model | Output |
|---|---|---|---|
| spec+impl | implement | DmuxAI (worktree `dmux-wt/fix-model-pin`, tmux `dmux-pin`) → AGY `gemini-3.8-flash-medium` | SPEC/PLAN/TASKS/REVIEW-cursor-model-pin, pin in cursor adapter argv + app catalog/submit/F2, TC-410..413 |
| fix | review | orchestrator | test called nonexistent `handle_enter` → `submit_prompt`; per-agent catalog test moved to AGY ids (Cursor ids are now coerced) |
| gate | test | cargo | fmt, clippy, test --all-targets green |
