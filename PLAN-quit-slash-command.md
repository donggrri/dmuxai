# PLAN: Slash-command quit

Status: complete  
Spec: [SPEC-quit-slash-command.md](SPEC-quit-slash-command.md)

1. Add `/quit` to the slash-command catalog and route exact command execution
   to the existing application exit state. (UX-011, TC-019, TC-020)
2. Make `Esc` dismiss-only in the main input path while preserving picker and
   slash-preview dismissal. (UX-012, TC-021)
3. Add deterministic tests for exact, preview, boundary, and non-exiting Esc
   behavior; update controls and baseline traceability. (UX-011, UX-012,
   TC-019–TC-022)
4. Run the spec check, narrow tests, and repository Rust gates, then record
   evidence and residual risks in the review artifact.
