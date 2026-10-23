#!/usr/bin/env python3
"""TC to test linkage check."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROW_TC = re.compile(r"^\|\s*(TC-([0-9]+))\s*\|", re.MULTILINE)
FN_TC = re.compile(r"\bfn\s+tc_([0-9]+(?:_[0-9]+)*)_([a-zA-Z0-9_]*)")
TEXT_TC = re.compile(r"\bTC-([0-9]+)\b")


def tc_sort_key(tc_id: str) -> tuple[int, str]:
    m = re.search(r"\d+", tc_id)
    return (int(m.group(0)) if m else 0, tc_id)


def collect_defined_tcs(specs_dir: Path) -> dict[str, dict]:
    defined_by_raw: dict[str, dict] = {}
    if not specs_dir.is_dir():
        return defined_by_raw

    for spec_path in sorted(specs_dir.glob("SPEC-*.md")):
        if not spec_path.is_file():
            continue
        content = spec_path.read_text(encoding="utf-8")
        for line in content.splitlines():
            m = ROW_TC.match(line)
            if m:
                raw_tc = m.group(1)
                defined_by_raw[raw_tc] = {
                    "raw": raw_tc,
                    "spec": str(spec_path),
                    "row": line,
                }
    return defined_by_raw


def derive_reason(row: str) -> str:
    if re.search(r"\bmanual\b", row, re.IGNORECASE):
        return "manual"
    if re.search(r"\b(ignored|browser|e2e)\b", row, re.IGNORECASE):
        return "e2e/browser (ignored)"
    if re.search(r"\bplanned\b", row, re.IGNORECASE):
        return "planned — no test yet"
    return "legacy: no test reference yet"


def collect_evidence(
    root: Path, defined_tcs: dict[str, dict]
) -> tuple[
    dict[str, list[tuple[str, int, str]]],
    dict[str, list[tuple[str, int]]],
    list[tuple[str, int, str, str]],
]:
    strong_links: dict[str, list[tuple[str, int, str]]] = {}
    comment_links: dict[str, list[tuple[str, int]]] = {}
    undefined_tests: list[tuple[str, int, str, str]] = []

    rs_files: list[Path] = []
    for sub in ["src", "tests"]:
        d = root / sub
        if d.is_dir():
            rs_files.extend(sorted(p for p in d.rglob("*.rs") if p.is_file()))

    for rs_path in rs_files:
        try:
            rel_path = str(rs_path.relative_to(root))
        except ValueError:
            rel_path = str(rs_path)
        content = rs_path.read_text(encoding="utf-8")

        for m in FN_TC.finditer(content):
            lineno = content.count("\n", 0, m.start()) + 1
            fn_name = f"tc_{m.group(1)}_{m.group(2)}"
            for part in m.group(1).split("_"):
                tc_id = f"TC-{part}"
                if tc_id in defined_tcs:
                    strong_links.setdefault(tc_id, []).append((rel_path, lineno, fn_name))
                else:
                    undefined_tests.append((rel_path, lineno, fn_name, tc_id))

        for m in TEXT_TC.finditer(content):
            lineno = content.count("\n", 0, m.start()) + 1
            tc_id = f"TC-{m.group(1)}"
            if tc_id in defined_tcs:
                comment_links.setdefault(tc_id, []).append((rel_path, lineno))

    return strong_links, comment_links, undefined_tests


def parse_allowlist(
    allowlist_path: Path,
    defined_tcs: dict[str, dict],
) -> tuple[dict[str, str], list[tuple[str, str]], list[tuple[str, str]]]:
    allowlist_entries: dict[str, str] = {}
    bad_allowlist: list[tuple[str, str]] = []
    undefined_allowlist: list[tuple[str, str]] = []

    if not allowlist_path.is_file():
        return allowlist_entries, bad_allowlist, undefined_allowlist

    for lineno, raw in enumerate(allowlist_path.read_text(encoding="utf-8").splitlines(), start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(None, 1)
        raw_tc = parts[0]
        reason = parts[1].strip() if len(parts) > 1 else ""

        tc_match = re.match(r"^TC-[0-9]+$", raw_tc)
        if not tc_match:
            bad_allowlist.append((raw_tc, f"line {lineno}: invalid TC id format"))
            continue

        if not reason:
            bad_allowlist.append((raw_tc, f"line {lineno}: missing required reason"))
            continue

        if raw_tc not in defined_tcs:
            undefined_allowlist.append((raw_tc, reason))
            continue

        allowlist_entries[raw_tc] = reason

    return allowlist_entries, bad_allowlist, undefined_allowlist


def main() -> int:
    default_root = Path(__file__).resolve().parent.parent

    parser = argparse.ArgumentParser(
        description="Verify TC linkage between specs and Rust tests."
    )
    parser.add_argument(
        "--root",
        default=str(default_root),
        help="Repository root directory (default: parent of scripts/)",
    )
    parser.add_argument(
        "--allowlist",
        default=None,
        help="Path to allowlist file (default: scripts/tc-link-allowlist.txt)",
    )
    parser.add_argument(
        "--strict",
        action="store_true",
        help="Report TCs linked only by comment",
    )
    parser.add_argument(
        "--strict-fail",
        action="store_true",
        help="Exit 1 if comment-only links exist",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit JSON summary",
    )
    parser.add_argument(
        "--dump-allowlist",
        action="store_true",
        help="Generate allowlist from currently orphaned TCs",
    )

    args = parser.parse_args()

    root = Path(args.root).resolve()
    if not root.is_dir():
        print(f"Error: root directory {root} does not exist", file=sys.stderr)
        return 2

    if args.allowlist is not None:
        allowlist_path = Path(args.allowlist)
        if not allowlist_path.is_absolute() and not allowlist_path.is_file():
            if (root / allowlist_path).is_file():
                allowlist_path = root / allowlist_path
            else:
                allowlist_path = allowlist_path.resolve()
    else:
        allowlist_path = root / "scripts" / "tc-link-allowlist.txt"

    specs_dir = root / "docs" / "specs"
    defined_tcs = collect_defined_tcs(specs_dir)
    strong_links, comment_links, undefined_tests = collect_evidence(root, defined_tcs)

    if args.dump_allowlist:
        print("# TC link allowlist. This list must only shrink.")
        print("# Format: TC-<digits> <reason>")
        orphans_to_dump = []
        for raw_tc, info in sorted(defined_tcs.items(), key=lambda item: tc_sort_key(item[0])):
            if raw_tc not in strong_links and raw_tc not in comment_links:
                reason = derive_reason(info["row"])
                orphans_to_dump.append((raw_tc, reason))
        for raw_tc, reason in orphans_to_dump:
            print(f"{raw_tc} {reason}")
        return 0

    allowlist_entries, bad_allowlist, undefined_allowlist = parse_allowlist(
        allowlist_path, defined_tcs
    )

    strong_set: set[str] = set()
    comment_only_set: set[str] = set()
    allowlisted_set: set[str] = set()
    stale_allowlist: list[tuple[str, str]] = []
    orphans: list[tuple[str, str]] = []

    for raw_tc, info in sorted(defined_tcs.items(), key=lambda item: tc_sort_key(item[0])):
        is_strong = raw_tc in strong_links
        is_comment = raw_tc in comment_links
        is_linked = is_strong or is_comment

        if is_strong:
            strong_set.add(raw_tc)
        elif is_comment:
            comment_only_set.add(raw_tc)

        if raw_tc in allowlist_entries:
            reason = allowlist_entries[raw_tc]
            if is_linked:
                stale_allowlist.append((raw_tc, reason))
            else:
                allowlisted_set.add(raw_tc)
        else:
            if not is_linked:
                orphans.append((raw_tc, info["spec"]))

    d = len(defined_tcs)
    s = len(strong_set)
    c = len(comment_only_set)
    a = len(allowlisted_set)
    o = len(orphans)
    u = len(undefined_tests)
    x = len(stale_allowlist)

    has_errors = bool(
        orphans or undefined_tests or stale_allowlist or undefined_allowlist or bad_allowlist
    )
    if args.strict_fail and c > 0:
        has_errors = True

    if args.json:
        payload = {
            "defined": d,
            "strong": s,
            "comment_only": c,
            "allowlisted": a,
            "orphan": o,
            "undefined_test": u,
            "stale": x,
            "orphans": [{"tc": tc, "spec": spec} for tc, spec in orphans],
            "undefined_tests": [
                {"file": f, "line": line, "fn": fn, "tc": tc}
                for f, line, fn, tc in undefined_tests
            ],
            "stale_entries": [{"tc": tc, "reason": r} for tc, r in stale_allowlist],
            "undefined_allowlist": [{"tc": tc, "reason": r} for tc, r in undefined_allowlist],
            "bad_allowlist": [{"tc": tc, "error": err} for tc, err in bad_allowlist],
        }
        print(json.dumps(payload, indent=2))
        return 1 if has_errors else 0

    for raw_tc, spec in orphans:
        try:
            rel_spec = str(Path(spec).relative_to(root))
        except ValueError:
            rel_spec = spec
        print(f"TC LINK CHECK: orphan {raw_tc} ({rel_spec})")

    for rel_path, lineno, fn_name, tc_id in undefined_tests:
        print(f"TC LINK CHECK: test {rel_path}:{lineno} {fn_name} references undefined {tc_id}")

    for raw_tc, reason in stale_allowlist:
        print(f"TC LINK CHECK: STALE allowlist entry: {raw_tc} ({reason}) is now linked")

    for raw_tc, reason in undefined_allowlist:
        print(f"TC LINK CHECK: allowlist references undefined {raw_tc}")

    for raw_tc, err in bad_allowlist:
        print(f"TC LINK CHECK: allowlist entry {raw_tc}: {err}")

    if args.strict:
        for raw_tc in sorted(comment_only_set, key=tc_sort_key):
            ref = comment_links[raw_tc][0]
            print(f"TC LINK CHECK: comment-only link: {raw_tc} ({ref[0]}:{ref[1]})")

    print(
        f"TC LINKS: {d} defined, {s} strong, {c} comment-only, {a} allowlisted, "
        f"{o} orphan, {u} undefined-test, {x} stale"
    )

    return 1 if has_errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
