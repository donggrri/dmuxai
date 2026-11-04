# TASKS: progress tool events and heartbeats (#166)

- [x] FR-2430 / TC-2430: custom JSONL tool mapping + opencode defaults.
  `cargo test -j 1 tc_2430`
- [x] FR-2431 / TC-2431: edit tools emit `file_edit` with a relative path.
  `cargo test -j 1 tc_2431`
- [x] FR-2432 / TC-2432: 30 s heartbeat with last-output age/type.
  `cargo test -j 1 tc_2432`
- [x] FR-2433 / TC-2433: safe, bounded summaries without content.
  `cargo test -j 1 tc_2433`
- [x] CLI integration: fake JSONL runner emits a tool progress line.
  `cargo test -j 1 tc_2430_cli_progress_emits_tool_line_for_jsonl_runner`
- [x] Docs/spec gates: `python3 scripts/check-spec.py --all`,
  `bash scripts/spec-index.sh`.
