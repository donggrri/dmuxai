# REVIEW: Generated SPEC index and cross-SPEC ID uniqueness

Status: complete
Spec: [SPEC-spec-index.md](../specs/SPEC-spec-index.md)
Reviewed: 2026-10-17

## Spec compliance

- **FR-470 (TC-470 `every_spec_has_status_line`)** — every
  `docs/specs/SPEC-*.md` now has a non-empty `Status:` line within its first
  10 lines. Two files needed repair to satisfy it:
  - `SPEC-input-redraw.md` was committed empty by PR #18 (git blob
    `e69de29`); reconstructed from TEST-PLAN §1 rows FR-420..425 / TC-420..426
    (all cited `src/app.rs` test names verified present), `docs/dogfood`
    #15/#17/#19, and the recorded fix summaries. IDs unchanged.
  - `SPEC-image-attachment.md` had no `Status:` line; a minimal header was
    added without touching requirement rows.
- **FR-471 (TC-471 `spec_ids_are_unique_across_specs`)** — IDs defined as the
  first cell of a table row are unique across `docs/specs/SPEC-*.md`. Three
  pre-existing collisions were resolved by renumbering the smaller side of
  each group into a free range: guard `060` block → `460`s
  (`INT-480/461`, `NFR-480`, `TC-480..463`; also de-shadows baseline
  `INT-060/061`, `NFR-060`), tui-stabilize tail → `441`-adjacent free range
  (`FR-490`, `TC-490/441`; `SPEC-windows-desktop` owns `390..405`). All living
  references were updated (TEST-PLAN §1 guard rows, plan/tasks/review of the
  guard, and `src/agent/cursor.rs` / `src/app.rs` test comments).
- **FR-472 (TC-472 `spec_index_script_lists_every_spec`)** —
  `scripts/spec-index.sh` prints one markdown row per `SPEC-*.md` with the
  link, the file's own `Status:` value, and ID ranges (min–max per prefix; `—`
  when the file defines no IDs). The test re-derives every Status and range
  cell from the files themselves, so generator/test drift fails the test.
- **FR-473 (TC-473, manual review)** — TEST-PLAN §0.1 no longer contains
  per-spec rows; it now points to `scripts/spec-index.sh`, the rule, and the
  enforcing test names. The §1.1 ID-overlap bullet was refreshed to the
  resolved state, and dogfood #14 is downgraded friction → resolved.

## Interpretation decisions

- FR-471 scopes uniqueness to files matching the FR-470 glob
  (`docs/specs/SPEC-*.md`). `docs/specs/DMUXAI-BASELINE.md` therefore does not
  participate, and intentional baseline echo (e.g. `TC-001..007` in
  SPEC-automatic-cross-agent-sharing, `TC-050..056` in
  SPEC-multi-instance-workspace) remains out of scope. The stricter reading
  (baseline included) would force renumbering ~35 IDs across six shipped
  specs; recorded here as a deliberate boundary, also stated in TEST-PLAN
  §1.1.
- Renumbered IDs follow the template rule "IDs are stable": no ID was
  repurposed. `INT-060/061`, `NFR-060` and `TC-060..063` continue to exist in
  DMUXAI-BASELINE with unchanged meaning; guard's old numeric IDs are simply
  no longer defined in any `SPEC-*.md` file. Historical ANALYSIS/dogfood
  mentions of the old collisions remain as records of the pre-fix state.

## Test evidence

- `cargo test --all-targets`: passed — 76 lib + 143 bin + 4 + 19 + 10 + 3 + 3
  + 4 crate/integration tests, 0 failures, 5 ignored (authenticated provider
  smoke tests, ignored as intended). Includes
  `tests/spec_index.rs` TC-470/471/472.
- `cargo fmt --check`: passed (ran `cargo fmt` once after drafting).
- `cargo clippy --all-targets --all-features -- -D warnings`: passed.
- Spec checks: `pwsh` is not installed in this Linux environment, so
  `scripts/check-spec.ps1` could not be executed directly. A faithful Python
  port of its exact regex/section logic was run instead (fail ⇒ exit 1):
  - `docs/specs/SPEC-tui-stabilize.md`: PASS
  - `docs/specs/SPEC-cursor-windows-implement-guard.md`: PASS
  - `docs/specs/SPEC-input-redraw.md`: PASS
  - `docs/specs/DMUXAI-BASELINE.md`: PASS (pre-existing CI target)
  - `docs/specs/SPEC-spec-index.md`, `docs/specs/SPEC-image-attachment.md`:
    FAIL on heading shape only (`## Requirements` instead of the supported
    `## 4. Requirements` variants) — both files' shapes pre-date this change
    and CI only checks the baseline; see residual risks. The equivalent
    PowerShell command should be run on CI or a Windows host:
    `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath <path>`.
- `bash scripts/spec-index.sh`: prints a 25-row table covering every
  `SPEC-*.md`; verified by hand and by TC-472.
- Live-provider smoke tests: not applicable (no provider code touched).

## Residual risks

- The meta-SPEC (`SPEC-spec-index.md`) and `SPEC-image-attachment.md` do not
  conform to the `FEATURE-SPEC.md` heading scheme the checker supports;
  conforming them (template §4/§5 sections, numeric IDs for image attachment)
  is a separate maintenance task. TC-470/471/472 accept their current shape.
- The reconstructed `SPEC-input-redraw.md` is a best-effort restoration; its
  requirement wording is traceable to TEST-PLAN §1, dogfood #15/#17/#19, and
  existing tests, but the original text is unrecoverable from git.
- `TC-472` requires `bash`; CI and Linux/WSL provide it. The generator is a
  developer/CI utility, not part of the shipped Windows launcher path.

## Rollback

Revert the documentation changes, `src` test comments,
`scripts/spec-index.sh`, and `tests/spec_index.rs`. No persistence schema,
provider protocol, or UI behavior changed; no data migration is involved.
