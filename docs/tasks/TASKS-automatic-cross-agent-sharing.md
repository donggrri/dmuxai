# TASKS: Automatic cross-agent conversation sharing

Status: complete  
Spec: `../specs/SPEC-automatic-cross-agent-sharing.md`
Plan: `../plans/PLAN-automatic-cross-agent-sharing.md`

- [x] T-projection — Add source-agent labels and all-completed-turn projection;
  protects FR-025, FR-026, FR-028, NFR-005; verify with `cargo test --test conversation`.
- [x] T-binding — Ensure common-head changes prevent incorrect native resume;
  protects FR-027; verify with the mixed-agent binding test in
  `cargo test --test conversation`.
- [x] T-ui — Render shared context and preserve optional `/share`; protects
  UX-015 and FR-025; verify with targeted `cargo test automatic_cross_agent`.
- [x] T-docs — Update README and shipped baseline wording; protects UX-015 and
  traceability; verify with `scripts/check-spec.ps1`.
- [x] T-gates — Run formatting, all-target tests, strict clippy, and diff check;
  protects all requirements; record exact results in the review artifact.
