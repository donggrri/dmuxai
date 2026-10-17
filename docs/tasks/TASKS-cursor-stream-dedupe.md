# TASKS: cursor-stream-dedupe

| Done | Owner | Task | Requirement IDs | Test command |
| --- | --- | --- | --- | --- |
| [x] | maintainer | Record a real cursor-agent stream; find the root cause | FR-800 | — |
| [x] | maintainer | Fix `normalize_event`; fixture and tests | FR-800, FR-801 | `cargo test --bin dmuxai cursor` (TC-800, TC-801) |
| [x] | maintainer | End-to-end check: cursor, agy, opencode-haiku | FR-800 | `dmuxai run … --json` |
| [x] | opencode-haiku | Review (REQUEST_CHANGES → fixed, re-review) | all | docs/reviews/REVIEW-cursor-stream-dedupe.md |
