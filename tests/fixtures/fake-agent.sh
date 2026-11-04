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

# Escapes a text for a JSON string value (newlines become \n).
json_escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' | awk 'BEGIN { ORS = "" } { if (NR > 1) printf "\\n"; print }'
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
  argv)
    # Record the real runner argv (TC-2317 / FR-2317).
    if [ -n "$FAKE_AGENT_ARGV" ]; then
      printf '%s\n' "$@" > "$FAKE_AGENT_ARGV"
    fi
    printf '{"type":"text","sessionID":"ses_1","part":{"text":"argv logged"}}\n' ;;
  jsonl)  printf '{"type":"step_start","sessionID":"ses_1"}\n{"type":"text","sessionID":"ses_1","part":{"text":"echo:%s"}}\n' "$(json_escape "$last")" ;;
  jsonl_tool)
    printf '%s\n' '{"type":"step_start","sessionID":"ses_1"}'
    printf '%s\n' '{"type":"tool_use","sessionID":"ses_1","part":{"type":"tool","tool":"read","state":{"status":"completed","title":"src/app.rs","input":{"filePath":"src/app.rs"}}}}'
    sleep 0.2
    printf '{"type":"text","sessionID":"ses_1","part":{"text":"echo:%s"}}\n' "$(json_escape "$last")" ;;
  jsonl_file)
    # Replay a saved JSONL stream (for example the opencode compaction leak).
    if [ -n "$FAKE_AGENT_JSONL" ]; then
      cat "$FAKE_AGENT_JSONL"
    fi
    ;;
  text)   echo "hello $last" ;;
  stdin)  read -r line; echo "stdin:$line" ;;
  empty)  : ;;
  writefile)
    # Edit the workspace without printing a final message (FR-2125).
    if [ -n "$FAKE_AGENT_OUT" ]; then
      printf 'changed\n' > "$FAKE_AGENT_OUT"
    fi
    ;;
  denied) echo "Error: permission denied for bash" >&2; echo "partial" ;;
  fail)   echo "boom" >&2; exit 3 ;;
  error)  printf '{"type":"error","error":{"message":"model not found"}}\n' ;;
  rate-limit)
    # Partial output, then a provider error that triggers a runner fallback.
    printf '{"type":"text","sessionID":"ses_1","part":{"text":"partial from the failed runner\\n"}}\n'
    printf '{"type":"error","error":{"message":"429 rate limit exceeded"}}\n'
    ;;
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
  idle_sleep)
    if [ -n "$FAKE_AGENT_PIDFILE" ]; then
      printf '%s\n' "$$" > "$FAKE_AGENT_PIDFILE"
    fi
    sleep 30
    ;;
  trickle)
    for i in 1 2 3 4 5 6; do
      printf '{"type":"text","part":{"text":"line%s\\n"}}\n' "$i"
      sleep 0.4
    done
    ;;
  delayed)
    sleep 2
    printf '{"type":"text","part":{"text":"delayed text\\n"}}\n'
    ;;
  orphan_stdout)
    sleep 60 &
    if [ -n "$FAKE_AGENT_PIDFILE" ]; then
      printf '%s\n' "$!" > "$FAKE_AGENT_PIDFILE"
    fi
    printf '{"type":"text","part":{"text":"orphan ok\\n"}}\n'
    exit 0
    ;;
esac
