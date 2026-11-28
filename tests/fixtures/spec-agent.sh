#!/bin/sh
# Fake spec-first runner for the spec review gate tests.
# usage: spec-agent.sh <prompt>; logs each call to $SPEC_AGENT_LOG, writes the
# spec file on the first call and appends reviewer feedback on re-runs.
for last; do :; done
printf 'call\n' >> "$SPEC_AGENT_LOG"
spec="docs/specs/SPEC-x.md"
case "$last" in
  *"Reviewer feedback"*)
    printf '%s\n' "$last" | grep 'Reviewer feedback' | head -n 1 >> "$spec"
    ;;
  *)
    if [ ! -f "$spec" ]; then
      mkdir -p docs/specs
      printf '# Spec X\n' > "$spec"
    fi
    ;;
esac
printf '{"type":"text","part":{"text":"done"}}\n'
