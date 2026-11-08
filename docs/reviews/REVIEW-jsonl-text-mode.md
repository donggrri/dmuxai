# REVIEW: JSONL text_mode and lifecycle-based turn completion (#184)

Status: complete

## Compliance

| Requirement | Met | Evidence |
| --- | --- | --- |
| FR-2530 | yes | `tc_2531_last_mode_keeps_final_text_block_after_tools` |
| FR-2531 | yes | `tc_2530_concat_mode_joins_text_parts_with_newlines` |
| FR-2532 | yes | `tc_2532_last_finish_reason_not_ok_is_unfinished`, `tc_2534_missing_finish_event_is_unfinished`, `tc_2535_short_final_answer_with_stop_succeeds` |
| FR-2533 | yes | `tc_2533_opencode_preset_finish_settings` |
| FR-2534 | yes | `tc_2536_done_when_missing_is_unfinished` |
| FR-2535 | yes | `tc_2535_short_final_answer_with_stop_succeeds` (no reply-length or tool-count heuristic) |

## Residual risks

- A custom runner configuring `done_when` that exits 0 silently without ever matching the pattern now fails with `error_kind = "unfinished"`. This is intended lifecycle enforcement per FR-2534.
- Runners configuring `finish_when` must emit a terminal line with a reason in `finish_ok` to succeed when exiting 0.

## Tests run

- `cargo build`
- `cargo test tc_253`
- `cargo test --lib`
- `cargo clippy --all-targets -- -D warnings`
- `cargo fmt`
- `python3 scripts/check-spec.py --all`
