# SPEC: `/compact` and `/summarize <focus>`

Status: Implemented  
Owner: DmuxAI  
Created: 2026-10-17  
Baseline: `73e3f4d`  
Related: dogfood plan, conversation-continuity-research

## 1. Decision summary

`/compact` is an alias of `/summarize`. Both accept optional focus text that is added to the summary instruction. The canonical transcript is never deleted; only the context sent to the next request is replaced by the summary plus recent turns.

## 2. Scope

### In scope

- Slash parsing for `/compact [focus]`, `/summarize [focus]`
- Summary prompt builder

### Out of scope / non-goals

- Automatic compaction thresholds changes

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| Q-1 | Proposed TCs (marked *proposed*) have no code yet and must be implemented before this spec is Approved. | DmuxAI | Release gate |

## 3. Terms and state model

- Summary document: constraints, decisions, facts, files, failed attempts.

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-340 | Must | User submits `/compact` with no text. | Same behavior as `/summarize`. | Works only when the agent is idle; otherwise a notice. | TC-341 |
| FR-341 | Must | User submits `/compact <focus>` or `/summarize <focus>`. | Focus text is included in the summary instruction. | Empty focus equals no focus. | TC-342 |
| FR-342 | Must | A summary completes. | Transcript is preserved; next request uses the summary fresh. | Invalid summary does not replace the transcript. | TC-340 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-340 | FR-342 | unit | conversation with turns | `/summarize`, then submit | summary internal; next turn uses it; invalid summary rejected | `cargo test explicit_summary_is_internal_and_the_next_turn_uses_it_fresh`; `invalid_summary_does_not_replace_the_canonical_transcript` |
| TC-341 | FR-340 | unit | idle app | `/compact` | same request as `/summarize` | `compact_is_alias_for_summarize` |
| TC-342 | FR-341 | unit | idle app | `/compact decisions only` | summary prompt contains focus | `summarize_focus_is_included_in_prompt` |

## 6. Data, integration, and safety contracts

- Focus text is user content and is quoted inside the instruction.

## 7. Implementation plan

See status above; proposed tests are tracked in [TEST-PLAN.md](../TEST-PLAN.md).

## 8. Review and test exit criteria

- TC-340..342 pass; status → Implemented.
