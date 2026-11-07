# REVIEW: security-reporting

Spec: [SPEC-security-reporting.md](../specs/SPEC-security-reporting.md)
Date: 2026-11-07

## Spec compliance

| ID | Met | Notes |
| --- | --- | --- |
| FR-2500 | yes | `SECURITY.md` + `tc_2500_security_md_documents_reporting_channels` |
| FR-2501 | yes | Fallback and no mailto/email in reporting section (TC-2500) |
| FR-2502 | yes | `RELEASE-PROCESS.md` step 0 + `tc_2501_release_process_checks_private_vulnerability_reporting` |

## Regression risk

Low: documentation and release-process checklist only.

## Test evidence

```text
cargo test tc_2500_security_md_documents_reporting_channels tc_2501_release_process_checks_private_vulnerability_reporting
python3 scripts/check-spec.py --all
python3 scripts/check-tc-links.py
cargo fmt
```

## Residual risk

Maintainers must still enable private vulnerability reporting in GitHub settings; the release step is manual.

## Rollback

Revert doc and test files; no migrations or binaries affected.
