#!/usr/bin/env python3
"""Baseline/feature spec traceability gate."""

from __future__ import annotations

import argparse
import json
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


def check_file(path) -> list[str]:
    spec_path = Path(path)
    if not spec_path.is_file():
        return [f"missing file {spec_path}"]

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

    return problems


def parse_allowlist(path: Path) -> list[str]:
    entries: list[str] = []
    if not path.is_file():
        return entries
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if " # " in line:
            name = line.split(" # ", 1)[0].strip()
        else:
            name = line
        if name:
            entries.append(name)
    return entries


def run_all(specs_dir: Path, allowlist_path: Path, as_json: bool) -> int:
    spec_files = sorted(
        p for p in specs_dir.glob("SPEC-*.md") if p.is_file()
    )
    allow_entries = parse_allowlist(allowlist_path)
    allow_set = set(allow_entries)

    unknown_entries = sorted(
        name for name in allow_set if not (specs_dir / name).is_file()
    )

    results: dict[str, tuple[bool, list[str]]] = {}
    for spec in spec_files:
        problems = check_file(spec)
        results[spec.name] = (len(problems) == 0, problems)

    stale: list[str] = []
    for name in sorted(allow_set):
        if name in results and results[name][0]:
            stale.append(name)

    non_allowlisted_failures = {
        name: problems
        for name, (passed, problems) in results.items()
        if name not in allow_set and not passed
    }

    n = len(spec_files)
    p = sum(1 for passed, _ in results.values() if passed)
    f = len(non_allowlisted_failures)
    a = sum(1 for name in results if name in allow_set)
    s = len(stale)

    exit_ok = (
        not unknown_entries
        and not non_allowlisted_failures
        and not stale
    )

    if as_json:
        payload = {
            "specs": n,
            "pass": p,
            "fail": f,
            "allowlisted": a,
            "stale": s,
            "failing": sorted(non_allowlisted_failures),
            "stale_entries": stale,
            "unknown_entries": unknown_entries,
        }
        print(json.dumps(payload))
        return 0 if exit_ok else 1

    for name in unknown_entries:
        print(f"SPEC CHECK: unknown allowlist entry: {name}")

    for name in sorted(results):
        passed, problems = results[name]
        if name in allow_set:
            if passed:
                print(f"STALE allowlist entry: remove {name}")
            continue
        if not passed:
            print(f"SPEC CHECK: {name}: {'; '.join(problems)}")

    print(f"SPEC CHECK --all: {n} specs, {p} pass, {f} fail, {a} allowlisted, {s} stale")
    return 0 if exit_ok else 1


def main() -> int:
    parser = argparse.ArgumentParser(
        usage="check-spec.py <path-to-spec.md> | check-spec.py --all [--specs-dir DIR] [--allowlist FILE] [--json]"
    )
    parser.add_argument("path", nargs="?")
    parser.add_argument("--all", action="store_true")
    parser.add_argument("--specs-dir", default="docs/specs")
    parser.add_argument("--allowlist", default="scripts/spec-check-allowlist.txt")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args()

    if args.all:
        return run_all(Path(args.specs_dir), Path(args.allowlist), args.json)

    if args.path is None:
        print("usage: check-spec.py <path-to-spec.md>", file=sys.stderr)
        return 2

    problems = check_file(args.path)
    if problems:
        for problem in problems:
            print(f"SPEC CHECK: {problem}", file=sys.stderr)
        return 1
    print(f"SPEC CHECK: PASS ({args.path})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
