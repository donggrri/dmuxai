# PLAN: security-reporting (issue #188)

Status: complete
Spec: [SPEC-security-reporting.md](../specs/SPEC-security-reporting.md)

1. Rewrite `SECURITY.md` § Reporting a vulnerability (FR-2516, FR-2517, TC-2516).
2. Add release step 0 checklist bullet in `docs/RELEASE-PROCESS.md` (FR-2518, TC-2517).
3. Soften `CONTRIBUTING.md` security pointer to match the policy (FR-2516).
4. Add `tests/security_reporting.rs` documentation oracles (TC-2516, TC-2517).
5. Add SPEC, tasks, and review artifacts; run `check-spec.py` and `check-tc-links.py`.

## Risks and rollback

- **Risk:** Fallback public issues increase maintainer triage noise. **Mitigation:** issues must stay minimal and detail-free per policy.
- **Rollback:** Revert documentation and tests only; no runtime impact.
