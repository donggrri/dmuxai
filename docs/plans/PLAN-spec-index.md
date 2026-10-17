# PLAN: Generated SPEC index and cross-SPEC ID uniqueness

Status: complete
Spec: [SPEC-spec-index.md](../specs/SPEC-spec-index.md)

Delivery as an agent graph (nodes = one job, edges = explicit payload):

```
[1 Collision inventory] → [2 ID renumber] → [3 Generator + tests] → [4 Doc rewire] → [5 Gates/review]
```

1. **Collision inventory.** Scan `docs/specs/SPEC-*.md` for first-cell ID
   definitions (the `check-spec.ps1` row shape) and list IDs defined in more
   than one file. Baseline overlap (`DMUXAI-BASELINE.md`) is out of scope
   because FR-471 scopes uniqueness to `SPEC-*` files.
2. **ID renumber.** Three collision groups found; renumber the smaller side of
   each into a free range, preserving each file's internal numbering scheme:
   - `SPEC-cursor-windows-implement-guard`: `INT-060/061, NFR-060,
     TC-060..063` → `INT-480/461, NFR-480, TC-480..463` (free range; also
     de-shadows baseline `INT-060/061`, `NFR-060`).
   - `SPEC-tui-stabilize` tail: `FR-390, TC-390..391` → `FR-490, TC-490..441`
     (`SPEC-windows-desktop` owns 390..405). Also repair the "through …"
     range phrases on its exit-criteria line.
   Update every living reference: TEST-PLAN §1 guard rows, `src` test
   comments, and the guard's PLAN/TASKS/REVIEW artifacts.
3. **Generator + tests.** Add `scripts/spec-index.sh` (FR-472) printing one
   markdown row per `SPEC-*.md`: link, `Status:` value, ID ranges (min–max per
   prefix; `—` when the file defines no IDs). Add `tests/spec_index.rs`
   implementing TC-470/471/472 exactly as named in the spec's acceptance
   table; the ranges assertion re-derives expected min–max per prefix from
   the file itself, mirroring the generator contract rather than its code.
4. **Doc rewire.** TEST-PLAN §0.1 loses its per-spec table (FR-473) and points
   to the generator and the rule; the §1.1 overlap bullet is refreshed.
   `docs/dogfood` #14 is downgraded friction → resolved (exit criterion).
5. **Gates/review.** Rust gates plus spec checks for every changed SPEC;
   record results and residual risk in the review.

## Data-quality finds during inventory (prerequisites forced by FR-470)

- `docs/specs/SPEC-input-redraw.md` was committed **empty** by PR #18 (git
  blob `e69de29`); the spec text was lost. Reconstructed on 2026-10-17 from
  TEST-PLAN §1 rows (FR-420..425 / TC-420..426 with verified `src/app.rs`
  test names), `docs/dogfood` #15/#17/#19, and the recorded fix summaries.
  IDs are unchanged.
- `docs/specs/SPEC-image-attachment.md` had no `Status:` line; a minimal
  header (`Status: Implemented`, Related pointer) was added. No requirement
  rows were touched.

## Risks and mitigations

- Renumbering could break a reference missed by the inventory → mitigated by
  `rg` sweeps for every renumbered token across `docs/` and `src/`, plus the
  new uniqueness test that fails on any future collision.
- The generator's `Status:` and range parsing could drift from the tests'
  interpretation → the test re-derives the expected cell values from the
  files, so drift fails TC-472 immediately.
- `bash` is required to run the generator (TC-472). CI and Linux/WSL dev
  shells provide it; the Windows launcher path does not depend on it.

## Rollback

Documentation and test-only change: revert the touched docs, `src` test
comments, `scripts/spec-index.sh`, and `tests/spec_index.rs`. No persistence,
provider-protocol, or UI behavior changes exist, so no data migration is
involved.
