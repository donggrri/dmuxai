# PLAN: runner auth error kind (#183)

Status: complete  
Spec: [SPEC-runner-auth-error-kind.md](../specs/SPEC-runner-auth-error-kind.md)

1. Add `AgentErrorKind::Auth` with constructor and stable `auth` string (FR-2280, TC-1687).
2. Map built-in grok probe failures that equal `grok_login_message()` through `builtin_probe_agent_error` (FR-2281, TC-2280, TC-2281).
3. Treat `auth` as a non-trigger in `classify_failure` and skip catalog-hint reclassification for `auth` errors (FR-2282, TC-2282, TC-2283).
4. Update SPEC-runner-fallback FR-2104/2105 rows and traceability for TC-2282/TC-2283.

## Risks and rollback

- **Mid-turn auth still falls back.** Only the explicit `auth` kind is excluded; provider stderr mentioning 401 keeps FR-2104 behaviour.
- **Rollback.** Remove `Auth` variant and restore `AgentError::model_unavailable` in the grok worker paths; drop `auth` from `classify_failure` non-triggers.
