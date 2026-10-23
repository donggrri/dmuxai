# SPEC-spec-index

Status: Implemented · Dogfood: #14

## Decision summary
Every parallel worktree appended a row to the hand-maintained SPEC table in `docs/TEST-PLAN.md` §0.1, so merges kept conflicting there. The table is removed. Each SPEC carries its own metadata, a generator prints the index on demand, and a test enforces the metadata and the uniqueness of IDs across SPECs. As a result, a new SPEC touches only its own file.

## Requirements
| ID | Pri | Requirement |
|---|---|---|
| FR-470 | Must | Every `docs/specs/SPEC-*.md` has a `Status:` line within its first 10 lines. |
| FR-471 | Must | A requirement or TC ID (`FR-`, `NFR-`, `UX-`, `INT-`, `TC-` + digits) is defined (as the first cell of a table row) in at most one SPEC file. |
| FR-472 | Must | `scripts/spec-index.sh` prints a markdown table (SPEC link, Status, ID ranges) generated from the SPEC files. |
| FR-473 | Must | TEST-PLAN §0.1 no longer has per-spec rows. It points to the generator and the rule. |

## Acceptance
| TC | Covers | Test |
|---|---|---|
| TC-470 | FR-470 | `every_spec_has_status_line` |
| TC-471 | FR-471 | `spec_ids_are_unique_across_specs` |
| TC-472 | FR-472 | `spec_index_script_lists_every_spec` |
| TC-473 | FR-473 | manual review |

## Exit criteria
TCs pass in CI. Dogfood #14 is downgraded from friction to resolved.

## Feature specs (generated index)

See `bash scripts/spec-index.sh` for the full table. Runner safety: [SPEC-runner-safety.md](SPEC-runner-safety.md) (FR-1640..1648, TC-1640..1648).
