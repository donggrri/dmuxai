# SPEC-spec-gate

Status: Implemented
Created: 2026-10-24

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-1800 | Must | `check-spec.py --all` is invoked from the crate root | Every `SPEC-*.md` in the specs dir is checked, allowlisted failures are tolerated, and exit code is 0 only when all non-allowlisted specs pass | Non-allowlisted failures, stale allowlist entries, or unknown entries make the gate exit 1 | TC-1800, TC-1801 |
| FR-1801 | Must | An allowlist entry names a spec that now passes, or a name with no matching file | The entry is reported as `STALE allowlist entry: remove SPEC-x.md` or as an unknown entry, and the run fails so the allowlist can only shrink | A correctly allowlisted failing spec keeps the run green | TC-1801 |
| FR-1802 | Must | `check-spec.py <file>` is invoked with a single spec path | The original single-file output and exit codes (0 pass, 1 fail, 2 usage) are preserved | Missing file or malformed spec exits 1; bad invocation exits 2 | TC-1802 |
| FR-1803 | Must | `check-tc-links.py` is invoked with test files and specs | Every defined TC is linked (strong `fn tc_<digits>_` or comment `TC-<digits>`) or allowlisted; tests referencing undefined TCs are rejected; exit 0 only when no orphans or undefined tests exist | Unlinked/unallowlisted TCs and tests referencing undefined TCs cause exit 1 | TC-1803, TC-1804 |
| FR-1804 | Must | Allowlist entries in `scripts/tc-link-allowlist.txt` | Each entry specifies `TC-<digits> <reason>` with a non-empty reason; linked entries are rejected as `STALE`; undefined TC entries or missing reasons cause exit 1 | Malformed, stale, or undefined allowlist entries fail the check so the list only shrinks | TC-1803, TC-1804 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-1800 | FR-1800 | integration | Repository with allowlist covering current failures | Run `python3 scripts/check-spec.py --all` | Exit 0 and passing summary | `tc_1800_check_spec_all_passes_with_allowlist` |
| TC-1801 | FR-1800, FR-1801 | integration | Temp specs dir with one passing and one failing spec plus a bad allowlist | Run `--all` with that allowlist, then with a correct one | First run exits 1 and mentions STALE and the unknown entry; second exits 0 | `tc_1801_check_spec_all_flags_stale_and_unknown_allowlist` |
| TC-1802 | FR-1802 | integration | Single spec paths | Run `check-spec.py` on a failing and a passing spec | Exit codes 1 and 0 respectively; covered by `tc_1801_check_spec_all_flags_stale_and_unknown_allowlist` | `tc_1801_check_spec_all_flags_stale_and_unknown_allowlist` |
| TC-1803 | FR-1803, FR-1804 | integration | Repository with allowlist covering unlinked TCs | Run `python3 scripts/check-tc-links.py` | Exit 0 and passing summary | `tc_1803_tc_links_have_tests_or_allowlist` |
| TC-1804 | FR-1803, FR-1804 | integration | Temp root with defined TCs, valid/undefined tests, stale/undefined allowlist | Run `check-tc-links.py` on dirty fixture then clean fixture | Dirty run exits 1 naming TC-2 (orphan), TC-9 (undefined test), TC-1 (stale) and TC-3 (undefined allowlist); clean run exits 0 | `tc_1804_tc_link_check_reports_orphans_both_ways` |

## 6. Planned additions

FR-1805..1809 reserved for scripts/gate.sh (added in the same PR)
