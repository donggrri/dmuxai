#!/bin/sh
# Fake headless agent for dmuxai custom-agent tests.
# usage: fake-agent.sh <behavior> [args...]; prompt is the last arg (or stdin with "stdin").
behavior="$1"; shift
for last; do :; done
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
esac
