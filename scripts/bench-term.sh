#!/usr/bin/env bash
set -euo pipefail

# Loose terminal-session latency benchmark (SPEC-terminal-sessions NFR-2691/2692).
# Builds the release binary when it is missing, then prints p50/p95 for:
#   - status with no session
#   - term send + term read on a warm shell session
#   - term exec vs bash -c for `true` and `sleep 1`
# The numbers are indicative; the gate test keeps the loose bounds.

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
target_dir="${CARGO_TARGET_DIR:-$root/target}"
bin="$target_dir/release/dmuxai"

needs_build=0
if [[ ! -x "$bin" ]]; then
    needs_build=1
elif [[ -n "$(find "$root/src" "$root/Cargo.toml" -newer "$bin" -print -quit)" ]]; then
    needs_build=1
fi
if (( needs_build )); then
    echo "bench-term: building $bin" >&2
    (cd "$root" && cargo build --release --bin dmuxai)
fi

tmp="$(mktemp -d)"
cleanup() {
    if [[ -n "${tmp:-}" && -d "$tmp" ]]; then
        "$bin" -C "$tmp/ws" term close --all --json >/dev/null 2>&1 || true
        rm -rf "$tmp"
    fi
}
trap cleanup EXIT

ws="$tmp/ws"
mkdir -p "$ws" "$tmp/data" "$tmp/state" "$tmp/runtime" "$tmp/home"
export HOME="$tmp/home"
export XDG_DATA_HOME="$tmp/data"
export XDG_STATE_HOME="$tmp/state"
export XDG_RUNTIME_DIR="$tmp/runtime"
export DMUXAI_TERM_IDLE_SECS=5
export DMUXAI_TRUST_WORKSPACE_CONFIG=1
export SHELL=/bin/sh

now_ms() {
    date +%s%N | awk '{ printf "%d", $1 / 1000000 }'
}

percentiles() {
    local label="$1"
    shift
    local -a values=("$@")
    local count="${#values[@]}"
    [[ "$count" -gt 0 ]] || { echo "$label: no samples" >&2; return; }
    mapfile -t sorted < <(printf '%s\n' "${values[@]}" | sort -n)
    local p50_idx=$(( (count * 50 + 99) / 100 - 1 ))
    local p95_idx=$(( (count * 95 + 99) / 100 - 1 ))
    printf '%-28s p50=%6sms  p95=%6sms  (n=%s)\n' \
        "$label" "${sorted[$p50_idx]}" "${sorted[$p95_idx]}" "$count"
}

measure() {
    local start end
    start="$(now_ms)"
    "$@" >/dev/null 2>&1
    end="$(now_ms)"
    echo $((end - start))
}

run_cmd() {
    "$bin" -C "$ws" "$@"
}

status_values=()
for _ in $(seq 1 20); do
    status_values+=("$(measure run_cmd status --json)")
done
percentiles "status (no session)" "${status_values[@]}"

run_cmd term open bench --json >/dev/null
run_cmd term send bench "echo warmup" --json >/dev/null
run_cmd term read bench --tail 1 --json >/dev/null

send_values=()
for i in $(seq 1 20); do
    start="$(now_ms)"
    run_cmd term send bench "echo bench-$i" --json >/dev/null
    run_cmd term read bench --tail 1 --json >/dev/null
    end="$(now_ms)"
    send_values+=("$((end - start))")
done
percentiles "send+read" "${send_values[@]}"

true_exec_values=()
true_bash_values=()
for _ in $(seq 1 10); do
    true_exec_values+=("$(measure run_cmd term exec bench true --timeout 5s --json)")
    true_bash_values+=("$(measure bash -c true)")
done
percentiles "exec true" "${true_exec_values[@]}"
percentiles "bash -c true" "${true_bash_values[@]}"

sleep_exec_values=()
sleep_bash_values=()
for _ in $(seq 1 3); do
    sleep_exec_values+=("$(measure run_cmd term exec bench "sleep 1" --timeout 10s --json)")
    sleep_bash_values+=("$(measure bash -c "sleep 1")")
done
percentiles "exec sleep 1" "${sleep_exec_values[@]}"
percentiles "bash -c 'sleep 1'" "${sleep_bash_values[@]}"

run_cmd term close bench --json >/dev/null
