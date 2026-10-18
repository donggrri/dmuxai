#!/bin/sh
# Mock grok for dmuxai tests. It does not use the network or read auth files.
#
# models: logged-in catalog unless GROK_FAKE_AUTH=0 (exit 0, no logged-in line)
# or GROK_FAKE_AUTH=fail (exit 1, "not logged in" on stderr) or
# GROK_FAKE_AUTH=fast (logged in, only a fast id).
# Prompt mode (-p) prints canned streaming-json unless GROK_FAKE_MODE is
# fail, empty, sleep, linger, orphan, noend, or tools:
#   tools   prints a tool_call, its completed update, then text and end.
#   linger  prints text, a tool_call, more text and the end line, then leaves a
#           child holding stdout and idles (grok 1.0.62 after a stranded
#           auto-wake prompt). The child's pid goes to GROK_FAKE_PIDFILE.
#   orphan  prints text and the end line, leaves a child holding stdout, and
#           exits 0 at once (a background command outliving the runner).
#   noend   prints text and exits 0 without an end line.
# GROK_FAKE_ARGV, when set, receives each invocation's
# argv, one element per line. The prompt is never executed.

if [ -n "${GROK_FAKE_ARGV:-}" ]; then
  for arg in "$@"; do
    printf '%s\n' "$arg" >> "$GROK_FAKE_ARGV"
  done
fi

if [ "${1:-}" = "models" ]; then
  case "${GROK_FAKE_AUTH:-}" in
    fail)
      echo "not logged in" >&2
      exit 1
      ;;
    0)
      cat <<'EOF'
Available models:
  * grok-4.7 (default)
  - grok-4.6
  - grok-4.7-build-fast
EOF
      exit 0
      ;;
    fast)
      cat <<'EOF'
You are logged in with grok.com.
Available models:
  - grok-4.7-build-fast
EOF
      exit 0
      ;;
    *)
      cat <<'EOF'
You are logged in with grok.com.
Available models:
  * grok-4.7 (default)
  - grok-4.6
  - grok-4.7-build-fast
EOF
      exit 0
      ;;
  esac
fi

case "${GROK_FAKE_MODE:-}" in
  fail)
    echo "boom" >&2
    exit 3
    ;;
  empty)
    printf '%s\n' '{"type":"end","stopReason":"end_turn","sessionId":"11111111-1111-1111-1111-111111111111","cwd":"/tmp","num_turns":1}'
    exit 0
    ;;
  sleep)
    # Grandchild sleeper, still in this process group, pid written for tests.
    (
      sleep 120 &
      echo "$!" > "${GROK_FAKE_PIDFILE:-/dev/null}"
      wait
    ) &
    sleep 120
    exit 0
    ;;
  linger)
    cat <<'EOF'
{"type":"text","data":"Checking"}
{"type":"tool_call","toolCallId":"t1","title":"run_command","kind":"execute"}
{"type":"tool_call_update","toolCallId":"t1","status":"completed"}
{"type":"text","data":"Done."}
{"type":"end","stopReason":"end_turn","sessionId":"11111111-1111-1111-1111-111111111111","cwd":"/tmp","num_turns":1}
EOF
    sleep 120 &
    echo "$!" > "${GROK_FAKE_PIDFILE:-/dev/null}"
    sleep 120
    exit 0
    ;;
  orphan)
    cat <<'EOF'
{"type":"text","data":"OK"}
{"type":"end","stopReason":"end_turn","sessionId":"11111111-1111-1111-1111-111111111111","cwd":"/tmp","num_turns":1}
EOF
    sleep 120 &
    echo "$!" > "${GROK_FAKE_PIDFILE:-/dev/null}"
    exit 0
    ;;
  noend)
    printf '%s\n' '{"type":"text","data":"OK"}'
    exit 0
    ;;
  tools)
    cat <<'EOF'
{"type":"tool_call","toolCallId":"t1","title":"read_file","kind":"read","status":"pending","toolName":"read_file","rawInput":{"target_file":"src/app.rs"},"content":[],"locations":[]}
{"type":"tool_call_update","toolCallId":"t1","status":"in_progress","content":[],"rawOutput":null,"locations":[]}
{"type":"tool_call_update","toolCallId":"t1","status":"completed","content":[],"rawOutput":null,"locations":[]}
{"type":"text","data":"OK"}
{"type":"end","stopReason":"end_turn","sessionId":"11111111-1111-1111-1111-111111111111","cwd":"/tmp","num_turns":1}
EOF
    exit 0
    ;;
  *)
    cat <<'EOF'
{"type":"available_commands","data":[]}
{"type":"thought","data":"thinking"}
{"type":"text","data":"OK"}
{"type":"usage","usage":{"input":1}}
{"type":"end","stopReason":"end_turn","sessionId":"11111111-1111-1111-1111-111111111111","cwd":"/tmp","num_turns":1}
EOF
    exit 0
    ;;
esac
