#!/bin/sh
# Fake headless agent for dmuxai custom-agent tests.
# usage: fake-agent.sh <behavior> [args...]; prompt is the last arg (or stdin with "stdin").

# Markers from src/graph/judge.rs (JUDGE_PROMPT) and src/graph/plan_doc.rs (PLAN_PROMPT).
JUDGE_MARKER='DmuxAI triage judge'
PLAN_MARKER='DmuxAI planner'

behavior="$1"
shift
for last; do :; done

pick_runner() {
  # First "- runner: models" line under "Allowed runners and models:" in a plan prompt.
  printf '%s\n' "$1" | awk '/^- / { line = $0; sub(/^- /, "", line); sub(/:.*/, "", line); print line; exit }'
}

emit_plan_json() {
  runner=$(pick_runner "$1")
  if [ -z "$runner" ]; then
    runner=fake
  fi
  # Valid 2-task / 2-step plan (schema in src/graph/plan_doc.rs).
  printf '%s\n' \
    "{\"tasks\":[{\"id\":\"task-1\",\"title\":\"Research\",\"done_check\":\"context gathered\",\"areas\":[],\"size\":\"small\",\"depends_on\":[]},{\"id\":\"task-2\",\"title\":\"Implement\",\"done_check\":\"change applied\",\"areas\":[],\"size\":\"small\",\"depends_on\":[\"task-1\"]}],\"steps\":[{\"id\":\"step-1\",\"role\":\"research\",\"job\":\"Gather context for the request\",\"runner\":\"$runner\",\"depends_on\":[],\"task\":\"task-1\"},{\"id\":\"step-2\",\"role\":\"implement\",\"job\":\"Apply the requested change\",\"runner\":\"$runner\",\"depends_on\":[\"step-1\"],\"task\":\"task-2\"}]}"
}

emit_judge_verdict() {
  route=complex
  reason=multi-step
  case "$behavior" in
    judge-simple) route=simple; reason=one-step ;;
  esac
  printf '%s\n' "{\"route\":\"$route\",\"reason\":\"$reason\"}"
}

dispatch_judge_or_plan() {
  prompt="$1"
  case "$prompt" in
    *"$JUDGE_MARKER"*)
      emit_judge_verdict
      ;;
    *"$PLAN_MARKER"*)
      emit_plan_json "$prompt"
      ;;
    *)
      printf 'done\n'
      ;;
  esac
}

case "$behavior" in
  jsonl)  printf '{"type":"step_start","sessionID":"ses_1"}\n{"type":"text","sessionID":"ses_1","part":{"text":"echo:%s"}}\n' "$last" ;;
  text)   echo "hello $last" ;;
  stdin)  read -r line; echo "stdin:$line" ;;
  empty)  : ;;
  denied) echo "Error: permission denied for bash" >&2; echo "partial" ;;
  fail)   echo "boom" >&2; exit 3 ;;
  error)  printf '{"type":"error","error":{"message":"model not found"}}\n' ;;
  env)    echo "cfg=$FAKE_CFG content=$(cat "$FAKE_CFG")" ;;
  sleep)  sleep 30; echo late ;;
  judge|plan|judge-simple) dispatch_judge_or_plan "$last" ;;
  pidsleep)
    printf '%s\n' "$$" > "$FAKE_AGENT_PIDFILE"
    sleep 60 &
    printf '%s\n' "$!" >> "$FAKE_AGENT_PIDFILE"
    printf '{"type":"text","part":{"text":"working"}}\n'
    wait
    ;;
esac
