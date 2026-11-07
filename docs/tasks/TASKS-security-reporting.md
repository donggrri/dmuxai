# TASKS: security-reporting

Spec: [SPEC-security-reporting.md](../specs/SPEC-security-reporting.md)

- [x] T1 — `SECURITY.md` reporting policy (FR-2500, FR-2501) → `cargo test tc_2500_security_md_documents_reporting_channels`
- [x] T2 — `docs/RELEASE-PROCESS.md` release check (FR-2502) → `cargo test tc_2501_release_process_checks_private_vulnerability_reporting`
- [x] T3 — SPEC + gates (FR-2500–FR-2502) → `python3 scripts/check-spec.py --all` and `python3 scripts/check-tc-links.py`
