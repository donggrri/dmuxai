# Edge Contract Template

An edge contract documents exactly what context flows between two nodes.
Use this when designing or reviewing a graph — before spawning any Task subagents.

## Format

```
Edge: <source_id> → <target_id>

## Payload
- Type: structured | freeform
- Content: <what data is passed>
- Trim Rule: keep_only | pass_through | summarize
  - keep_only: pass only the fields the target needs; drop everything else
  - pass_through: forward the full output unchanged
  - summarize: compress to a summary before passing (for large context)

## On Failure
- Retry: <n> (number of retry attempts, 0 = no retry)
- Fallback: skip | default_value | human_escalate
  - skip: proceed to next node without this output
  - default_value: substitute a predefined default
  - human_escalate: stop and ask the user to intervene

## Notes
<Any special handling, assumptions, or dependencies>
```

## Example

```
Edge: A (Researcher) → B (Drafter)

## Payload
- Type: structured
- Content: A.output.summary (string), A.output.sources (list)
- Trim Rule: keep_only — drop raw research notes, keep only the summary

## On Failure
- Retry: 1
- Fallback: human_escalate — without research, drafting is not safe to proceed

## Notes
Summary must be ≤ 500 words. If sources list is empty, treat as failure.
```

## Why This Matters

Undefined edges are the #1 source of multi-agent bugs:

- Node B expects structured data but receives free text → parse error
- No trim rule → context grows unbounded across the pipeline
- No fallback → one node failure halts the entire graph
- Implicit assumptions → nodes disagree on data format

Write the edge contract BEFORE implementing the agents. The contract is the spec.
