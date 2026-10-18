# REVIEW: graph-swimlane spec

Date: 2026-10-18 (KST). Reviewer: opencode-haiku (read-only), three rounds. Spec: SPEC-graph-swimlane (#61).

- Round 1 — REQUEST_CHANGES, all fixed:
  - the same-lane arrow route was undefined;
  - ticks for runs over 10h were not covered;
  - the "now" clock source and the dependency edge cases were undefined.
  - Fixes: a server-corrected clock with clamping, ticks up to 1d, and the same-lane route through sub-row gaps.
- Round 2 — REQUEST_CHANGES, fixed:
  - the stacking threshold said "overlap" in one place and "touch" in another; it is now 24px everywhere;
  - a running dependency now ends at max(now, start plus estimate);
  - the `blocked` case now has a test.
- Round 3 — APPROVE.
