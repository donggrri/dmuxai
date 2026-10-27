# PLAN: headless AGY plan-only runs must not report success

Status: complete  
Spec: [../specs/SPEC-plan-gate.md](../specs/SPEC-plan-gate.md) (FR-2121)  
Issue: GitHub #164

1. Add a headless edit policy to `App` (`headless_require_edits`,
   `headless_plan_only_approval`, `headless_plan_only_reply`) and set it in
   `run_headless` for a direct headless run on an explicit agent with the
   write-capable agent policy: `--approve-plan` requires file edits, without it
   a plan-only reply needs approval. Clear the flags when the run ends.
   (FR-2121)
2. Make `graph_node_expects_file_edits` return true while
   `headless_require_edits` is set, so the quiet 1-node run sends
   `require_file_edits = true` and AGY applies its existing stricter-edit
   retry. The FR-937 quiet behaviour stays the default. (FR-2121)
3. Let the AGY adapter report a plan-only completion: `run_turn` emits the
   `PLAN_ONLY_APPROVAL_NOTICE` marker notice (new) when an Agent-policy turn
   completes with no observed writes and a plan-like reply and no edits were
   required. (FR-2121)
4. Map the marker in the app: the notice is swallowed in the transcript, and a
   headless run without `--approve-plan` reports exit 7
   (`APPROVAL_REQUIRED`) with the plan-only note while keeping the plan text in
   the output. (FR-2121)
5. Add the tests (`tc_2121`, `tc_2122`, `tc_2123`, `tc_2124`), update
   [../specs/SPEC-plan-gate.md](../specs/SPEC-plan-gate.md) (FR-2121 and
   TC-2121..TC-2124) and [../API.md](../API.md), and run the gates.
   (FR-2121)

## Risks and mitigations

- The plan-only detection needs the adapter's view of the turn (observed writes
  and the raw result text). It stays in the AGY adapter, where that data
  exists, and only a marker notice crosses the adapter boundary; other runners
  are untouched.
- The marker notice is a non-terminal notice, so an interactive run keeps its
  current behaviour: the notice is dropped instead of becoming a transcript
  line, and the turn is not failed.
- Requiring file edits only applies to a headless direct run with
  `--approve-plan` and the write-capable policy. Read-only (`ask`) turns and
  the TUI/dashboard direct runs keep `require_file_edits = false`.
- The headless flags are App state: they are assigned at the start of every
  headless run and cleared when it ends, so a later run cannot inherit them.

## Rollback

Code-only change with no persistence or provider protocol migration: the flags,
the marker notice, and the exit-7 mapping can be reverted independently. The
AGY retry logic itself is unchanged, so reverting restores the previous
behaviour (a plan-only reply exits 0).
