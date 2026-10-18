#!/bin/sh
# Mock grok for dmuxai tests. It does not use the network or read auth files.
#
# models: logged-in catalog unless GROK_FAKE_AUTH=0 (exit 0, no logged-in line)
# or GROK_FAKE_AUTH=fail (exit 1, "not logged in" on stderr) or
# GROK_FAKE_AUTH=fast (logged in, only a fast id).
# Prompt mode (-p) prints canned streaming-json unless GROK_FAKE_MODE is
# fail, empty, or sleep. GROK_FAKE_ARGV, when set, receives each invocation's
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
