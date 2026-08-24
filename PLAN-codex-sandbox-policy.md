# PLAN: Codex sandbox-policy object compatibility

Status: complete
Spec: [SPEC-codex-sandbox-policy.md](SPEC-codex-sandbox-policy.md)

1. Make the request-shape regression test expect the tagged object and confirm
   it fails against the current string payload. (INT-040, TC-046)
2. Change only Codex `turn/start` request construction to emit the documented
   discriminator object. (INT-040, NFR-041)
3. Update the baseline and review artifacts, then run narrow and full Rust
   gates. No persistent data or provider configuration requires rollback.
