# REVIEW: centered-prompt

Spec: [SPEC-centered-prompt](../specs/SPEC-centered-prompt.md) (#54)
Implementer: AGY (`dmuxai run --agent agy --mode implement`, about 10 min). Reviewer: opencode-haiku (read-only). The opencode free models were tried first as reviewers: deepseek returned an empty reply and glm timed out on the diff prompt.

## Round 1 — REQUEST_CHANGES

- The header jumped during the transition: `body` padding animated, but the `.top` counter-margins snapped. Fixed: `.top` gets `transition: margin 0.2s`, and it is included in the `prefers-reduced-motion` and `.reduced-motion` overrides.
- "The left dock is unreachable" (no JS sets `body.left-sidebar-open`): this is intended. The left sidebar ships in #55 (SPEC-session-sidebar), and TC-851 simulates it with a probe element. Noted in the SPEC design notes.
- No overlay regression at 900px and below; the dock variables only change above 900px.

## Verification

- TC-851 fails against the old CSS: the prompt was off-centre with the Files sidebar open. It passes with the new rules.

## Round 2 — APPROVE
