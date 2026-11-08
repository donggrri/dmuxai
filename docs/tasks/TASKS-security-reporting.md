# TASKS: security-reporting

Spec: [SPEC-security-reporting.md](../specs/SPEC-security-reporting.md)

- [x] T1 — `SECURITY.md` reporting policy (FR-2516, FR-2517) → `cargo test tc_2516_security_md_documents_reporting_channels`
- [x] T2 — `docs/RELEASE-PROCESS.md` release check (FR-2518) → `cargo test tc_2517_release_process_checks_private_vulnerability_reporting`
- [x] T3 — SPEC + gates (FR-2516–FR-2518) → `python3 scripts/check-spec.py --all` and `python3 scripts/check-tc-links.py`
