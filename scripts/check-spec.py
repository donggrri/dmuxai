#!/usr/bin/env python3
"""Baseline/feature spec traceability gate."""

from __future__ import annotations

import re
import sys
from pathlib import Path


REQUIREMENTS_HEADING = re.compile(
    r"(?ms)^## (?:2\. Functional requirements|4\. Requirements)\s*\r?\n(.*?)(?=^## |\Z)"
)
SCENARIOS_HEADING = re.compile(
    r"(?ms)^## (?:4\. Acceptance scenarios and current evidence|5\. Acceptance scenarios and test design)\s*\r?\n(.*?)(?=^## |\Z)"
)
REQUIREMENT_ROW = re.compile(
    r"(?m)^\|\s*((?:FR|BR|UX|INT|NFR)-\d+)\s*\|\s*([^|]+)\|"
)
TEST_ROW = re.compile(r"(?m)^\|\s*(TC-\d+)\s*\|")


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: check-spec.py <path-to-spec.md>", file=sys.stderr)
        return 2

    spec_path = Path(sys.argv[1])
    if not spec_path.is_file():
        print(f"SPEC CHECK: missing file {spec_path}", file=sys.stderr)
        return 1

    content = spec_path.read_text(encoding="utf-8")
    problems: list[str] = []

    req_match = REQUIREMENTS_HEADING.search(content)
    scen_match = SCENARIOS_HEADING.search(content)
    if not req_match:
        problems.append("Missing a Requirements section with a supported heading.")
    if not scen_match:
        problems.append("Missing an Acceptance scenarios section with a supported heading.")

    if not problems:
        requirements = req_match.group(1)
        scenarios = scen_match.group(1)
        requirement_rows = list(REQUIREMENT_ROW.finditer(requirements))
        if not requirement_rows:
            problems.append("No requirement rows with stable IDs were found.")

        defined_ids = [m.group(1) for m in requirement_rows]
        for req_id in sorted({i for i in defined_ids if defined_ids.count(i) > 1}):
            problems.append(f"Requirement ID {req_id} is defined more than once.")

        test_rows = list(TEST_ROW.finditer(scenarios))
        if not test_rows:
            problems.append("No acceptance scenario rows with TC-* IDs were found.")

        test_ids = [m.group(1) for m in test_rows]
        for test_id in sorted({i for i in test_ids if test_ids.count(i) > 1}):
            problems.append(f"Test ID {test_id} is defined more than once.")

        for row in requirement_rows:
            req_id = row.group(1)
            priority = row.group(2).strip()
            if priority == "Must" and req_id not in scenarios:
                problems.append(f"Must requirement {req_id} has no acceptance-scenario mapping.")

    if problems:
        for problem in problems:
            print(f"SPEC CHECK: {problem}", file=sys.stderr)
        return 1

    print(f"SPEC CHECK: PASS ({spec_path})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
