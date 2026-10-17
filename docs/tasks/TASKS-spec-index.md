# TASKS: Generated SPEC index and cross-SPEC ID uniqueness

Status: complete
Spec: [SPEC-spec-index.md](../specs/SPEC-spec-index.md)

- [x] T1 — Inventory first-cell ID definitions across `docs/specs/SPEC-*.md`
  and confirm the collision groups (TC-060..063, FR-390, TC-390/391).
  Requirements: FR-471. Verify: `rg '^\| (FR|NFR|UX|INT|TC)-[0-9]+' docs/specs/SPEC-*.md`
  inventory plus `cargo test --test spec_index spec_ids_are_unique_across_specs`.
- [x] T2 — Restore the empty `SPEC-input-redraw.md` from traceability records
  and add the missing `Status:` line to `SPEC-image-attachment.md`.
  Requirements: FR-470. Verify: `cargo test --test spec_index every_spec_has_status_line`.
- [x] T3 — Renumber the guard 060-block to `INT-480/461, NFR-480, TC-480..463`
  and the tui-stabilize tail to `FR-490, TC-490/441` in the SPEC, TEST-PLAN §1,
  plan/tasks/review artifacts, and `src` test comments.
  Requirements: FR-471. Verify: `cargo test --test spec_index spec_ids_are_unique_across_specs`.
- [x] T4 — Add `scripts/spec-index.sh` printing the index table.
  Requirements: FR-472. Verify: `bash scripts/spec-index.sh` and
  `cargo test --test spec_index spec_index_script_lists_every_spec`.
- [x] T5 — Rewrite TEST-PLAN §0.1 without per-spec rows, pointing to the
  generator and the rule; refresh the §1.1 ID-overlap bullet; downgrade
  dogfood #14 to resolved.
  Requirements: FR-473. Verify: manual review (TC-473) of `docs/TEST-PLAN.md`
  §0.1 and `docs/dogfood/README.md` #14.
- [x] T6 — Run the repository gates and record evidence in the review doc.
  Requirements: all. Verify: spec checks for each changed SPEC file,
  `cargo fmt --check`, `cargo test --all-targets`,
  `cargo clippy --all-targets --all-features -- -D warnings`.
