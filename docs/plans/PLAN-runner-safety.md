# PLAN: runner-safety

1. Add `is_question_prompt` and question-hint handling in graph judge + app submit path.
2. Clamp restored conversation runner against `allowed_runners` on resume/startup/dashboard load.
3. Skip triage for read-only mode; pin planner nodes unless explicit graph mode.
4. Spec tests `tc_1640`–`tc_1648`; run Rust gates and `check-spec.sh`.

Rollback: revert app/graph changes; no migrations.
