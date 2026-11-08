# REVIEW: security-reporting

Spec: [SPEC-security-reporting.md](../specs/SPEC-security-reporting.md)
Date: 2026-11-08

## Spec compliance

| ID | Met | Notes |
| --- | --- | --- |
| FR-2516 | yes | `SECURITY.md` + `tc_2516_security_md_documents_reporting_channels` |
| FR-2517 | yes | Fallback and no mailto/email in reporting section (TC-2516) |
| FR-2518 | yes | `RELEASE-PROCESS.md` step 0 + `tc_2517_release_process_checks_private_vulnerability_reporting` |

## Regression risk

Low: documentation and release-process checklist only.

## Test evidence

```text
cargo test tc_2516_security_md_documents_reporting_channels tc_2517_release_process_checks_private_vulnerability_reporting
python3 scripts/check-spec.py --all
python3 scripts/check-tc-links.py
cargo fmt
```

## Validation

- Original spec, plan, and implementation artifacts were written by Composer
  (cursor-agent, composer-2.5) on 2026-11-07.
- This review was re-validated on 2026-11-08 by GLM free
  (`opencode/glm-5.3-flash-free` via dmuxai): requirement IDs renamed to the reserved
  FR-2516–FR-2518 / TC-2516–TC-2517 blocks (prior FR-2500–FR-2502 / TC-2500–TC-2501
  collided with earlier-merging coordination branches), the gates above were rerun,
  and app/user-facing docs were checked against the non-goals.

## Residual risk

Maintainers must still enable private vulnerability reporting in GitHub settings; the release step is manual.

## Rollback

Revert doc and test files; no migrations or binaries affected.
