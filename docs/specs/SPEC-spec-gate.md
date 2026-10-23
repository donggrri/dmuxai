# SPEC-spec-gate

Status: Implemented
Created: 2026-10-24

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-1800 | Must | `check-spec.py --all` is invoked from the crate root | Every `SPEC-*.md` in the specs dir is checked, allowlisted failures are tolerated, and exit code is 0 only when all non-allowlisted specs pass | Non-allowlisted failures, stale allowlist entries, or unknown entries make the gate exit 1 | TC-1800, TC-1801 |
| FR-1801 | Must | An allowlist entry names a spec that now passes, or a name with no matching file | The entry is reported as `STALE allowlist entry: remove SPEC-x.md` or as an unknown entry, and the run fails so the allowlist can only shrink | A correctly allowlisted failing spec keeps the run green | TC-1801 |
| FR-1802 | Must | `check-spec.py <file>` is invoked with a single spec path | The original single-file output and exit codes (0 pass, 1 fail, 2 usage) are preserved | Missing file or malformed spec exits 1; bad invocation exits 2 | TC-1802 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-1800 | FR-1800 | integration | Repository with allowlist covering current failures | Run `python3 scripts/check-spec.py --all` | Exit 0 and passing summary | `tc_1800_check_spec_all_passes_with_allowlist` |
| TC-1801 | FR-1800, FR-1801 | integration | Temp specs dir with one passing and one failing spec plus a bad allowlist | Run `--all` with that allowlist, then with a correct one | First run exits 1 and mentions STALE and the unknown entry; second exits 0 | `tc_1801_check_spec_all_flags_stale_and_unknown_allowlist` |
| TC-1802 | FR-1802 | integration | Single spec paths | Run `check-spec.py` on a failing and a passing spec | Exit codes 1 and 0 respectively; covered by `tc_1801_check_spec_all_flags_stale_and_unknown_allowlist` | `tc_1801_check_spec_all_flags_stale_and_unknown_allowlist` |

## 6. Planned additions

FR-1803..1809 reserved for TC↔test linkage and scripts/gate.sh (added in the same PR)
