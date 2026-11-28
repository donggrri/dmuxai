# TASKS: dashboard feature coverage

Spec: `docs/specs/SPEC-dashboard-feature-coverage.md`

- [x] T1 (FR-3750..FR-3753): rename browser tests; `cargo test --test web_chat_copy --test web_chat_single_send --test web_model_menu --test web_followup_queue -- --ignored`.
- [x] T2 (FR-3762): shared human-header guard + asset headers; `cargo test --lib -- web::server web::assets`.
- [x] T3 (FR-3756, FR-3758, FR-3761): HTTP tests; `cargo test --lib -- tc_3756 tc_3758 tc_3761`.
- [x] T4 (FR-3754, FR-3755, FR-3756, FR-3757, FR-3758, FR-3759, FR-3760, FR-3763): browser tests; `cargo test --test 'web_*' -- --ignored`.
- [x] T5 (NFR-3750): meta test; `cargo test --test web_dashboard_coverage_meta`.
- [x] T6: CHANGELOG and `docs/API.md` note for FR-3762.
