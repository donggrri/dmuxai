#!/usr/bin/env bash
set -euo pipefail

# Linux only.
if [[ "$(uname -s)" != "Linux" ]]; then
    echo "gate.sh: Linux only (uname reported: $(uname -s))" >&2
    exit 2
fi

# Collect DMUXAI_* variable names from the current environment.
dmuxai_vars=()
while IFS= read -r var; do
    [[ -n "$var" ]] && dmuxai_vars+=("$var")
done < <(compgen -e | grep '^DMUXAI_' || true)

isolated=1
browser=0
quick=0
keep_going=0
print_env=0
print_browser_cmd=0
help=0

for arg in "$@"; do
    case "$arg" in
        --isolated) isolated=1 ;;
        --no-isolated) isolated=0 ;;
        --browser) browser=1 ;;
        --quick) quick=1 ;;
        --keep-going) keep_going=1 ;;
        --print-env) print_env=1 ;;
        --print-browser-cmd) print_browser_cmd=1 ;;
        -h|--help) help=1 ;;
        *)
            echo "gate.sh: unknown option: $arg" >&2
            exit 2
            ;;
    esac
done

if (( help )); then
    cat <<'USAGE'
Usage: scripts/gate.sh [options]

Run the local DmuxAI quality gate.

Options:
  --isolated        Run every step with DMUXAI_* variables unset and a fresh
                    empty XDG_CONFIG_HOME temp dir (default).
  --no-isolated     Leave the environment untouched.
  --browser         After the normal steps, run ignored browser tests.
  --quick           Skip clippy and run only the spec_index test.
  --keep-going      Run all steps and summarize failures at the end.
  -h, --help        Show this help and exit.

Hidden/debug:
  --print-env          Print the env var names passed to steps and exit.
  --print-browser-cmd  Print the browser test cargo command and exit.
USAGE
    exit 0
fi

# Build the browser test command from every tests/web_*.rs integration target.
browser_test_args=()
for test_file in tests/web_*.rs; do
    [[ -e "$test_file" ]] || continue
    stem="${test_file##*/}"
    stem="${stem%.rs}"
    browser_test_args+=(--test "$stem")
done

browser_cmd=(cargo test --all-features)
if (( ${#browser_test_args[@]} > 0 )); then
    browser_cmd+=("${browser_test_args[@]}")
fi
browser_cmd+=(-- --ignored)
browser_label="${browser_cmd[*]}"

if (( print_browser_cmd )); then
    if (( ${#browser_test_args[@]} == 0 )); then
        echo "gate.sh: no tests/web_*.rs targets found" >&2
    else
        echo "$browser_label"
    fi
    exit 0
fi

# In isolated mode, point XDG_CONFIG_HOME at a fresh empty directory so tests
# never read the developer's DmuxAI config. HOME is intentionally left alone.
isolated_xdg=""
browser_log=""
cleanup() {
    if [[ -n "$isolated_xdg" && -d "$isolated_xdg" ]]; then
        rm -rf "$isolated_xdg"
    fi
    if [[ -n "$browser_log" && -f "$browser_log" ]]; then
        rm -f "$browser_log"
    fi
}
trap cleanup EXIT

if (( isolated )); then
    isolated_xdg=$(mktemp -d)
    echo "isolated: XDG_CONFIG_HOME=$isolated_xdg"
fi

if (( print_env )); then
    if (( isolated )); then
        compgen -e | grep -v '^DMUXAI_' | grep -v '^XDG_CONFIG_HOME$' | sort -u
        echo "XDG_CONFIG_HOME=$isolated_xdg"
    else
        while IFS= read -r var; do
            if [[ "$var" == "XDG_CONFIG_HOME" ]]; then
                echo "XDG_CONFIG_HOME=${XDG_CONFIG_HOME}"
            else
                echo "$var"
            fi
        done < <(compgen -e | sort -u)
    fi
    exit 0
fi

# Build env(1) arguments that unset every DMUXAI_* variable and, in isolated
# mode, set XDG_CONFIG_HOME to the gate-owned temp dir.
env_args=()
for var in "${dmuxai_vars[@]}"; do
    env_args+=(-u "$var")
done
if (( isolated )); then
    env_args+=(XDG_CONFIG_HOME="$isolated_xdg")
fi

if (( isolated )) && (( ${#dmuxai_vars[@]} > 0 )); then
    echo "isolated: unsetting DMUXAI_* vars:"
    for var in "${dmuxai_vars[@]}"; do
        echo "  $var"
    done
fi

run_step() {
    local label="$1"
    shift
    local start end elapsed rc
    echo "==> $label"
    start=$(date +%s%N)
    set +e
    if (( isolated )); then
        env "${env_args[@]}" "$@"
    else
        "$@"
    fi
    rc=$?
    set -e
    end=$(date +%s%N)
    elapsed=$(awk "BEGIN { printf \"%.3f\", ($end - $start) / 1000000000 }")
    echo "    (${elapsed}s)"
    return "$rc"
}

sum_result_field() {
    local field="$1"
    awk -v field="$field" '
        /test result:/ {
            for (i = 1; i <= NF; i++) {
                if ($i == field ";") {
                    print $(i - 1)
                    break
                }
            }
        }
    ' "$browser_log" | awk '{ total += $1 } END { print total + 0 }'
}

run_browser_step() {
    local start end elapsed rc passed_total failed_total
    browser_log=$(mktemp)
    echo "==> $browser_label"
    start=$(date +%s%N)
    set +e
    if (( isolated )); then
        env "${env_args[@]}" "${browser_cmd[@]}" 2>&1 | tee "$browser_log"
    else
        "${browser_cmd[@]}" 2>&1 | tee "$browser_log"
    fi
    rc=${PIPESTATUS[0]}
    set -e
    end=$(date +%s%N)
    elapsed=$(awk "BEGIN { printf \"%.3f\", ($end - $start) / 1000000000 }")
    echo "    (${elapsed}s)"
    passed_total=$(sum_result_field passed)
    failed_total=$(sum_result_field failed)
    rm -f "$browser_log"
    browser_log=""
    if (( rc == 0 && passed_total == 0 && failed_total == 0 )); then
        echo "GATE: browser step ran zero tests (no matching tests/web_*.rs targets?)" >&2
        return 1
    fi
    return "$rc"
}

failed_steps=()
first_failed=""

record_failure() {
    local label="$1"
    failed_steps+=("$label")
    if [[ -z "$first_failed" ]]; then
        first_failed="$label"
    fi
}

# Determine step list.
if (( quick )); then
    test_label="cargo test --all-features --test spec_index"
    test_cmd=(cargo test --all-features --test spec_index)
else
    test_label="cargo test --all-features"
    test_cmd=(cargo test --all-features)
fi

# Step 1
if ! run_step "cargo fmt --all -- --check" cargo fmt --all -- --check; then
    record_failure "cargo fmt --all -- --check"
    if (( ! keep_going )); then
        echo "GATE: FAIL at cargo fmt --all -- --check"
        exit 1
    fi
fi

# Step 2 (skip under --quick)
if (( ! quick )); then
    if ! run_step "cargo clippy --all-targets --all-features -- -D warnings" \
        cargo clippy --all-targets --all-features -- -D warnings; then
        record_failure "cargo clippy --all-targets --all-features -- -D warnings"
        if (( ! keep_going )); then
            echo "GATE: FAIL at cargo clippy --all-targets --all-features -- -D warnings"
            exit 1
        fi
    fi
fi

# Step 3
if ! run_step "$test_label" "${test_cmd[@]}"; then
    record_failure "$test_label"
    if (( ! keep_going )); then
        echo "GATE: FAIL at $test_label"
        exit 1
    fi
fi

# Step 4
if ! run_step "python3 scripts/check-spec.py --all" python3 scripts/check-spec.py --all; then
    record_failure "python3 scripts/check-spec.py --all"
    if (( ! keep_going )); then
        echo "GATE: FAIL at python3 scripts/check-spec.py --all"
        exit 1
    fi
fi

# Step 5
if ! run_step "python3 scripts/check-tc-links.py" python3 scripts/check-tc-links.py; then
    record_failure "python3 scripts/check-tc-links.py"
    if (( ! keep_going )); then
        echo "GATE: FAIL at python3 scripts/check-tc-links.py"
        exit 1
    fi
fi

# Optional browser tests.
if (( browser )); then
    if command -v google-chrome >/dev/null 2>&1 || command -v chromium >/dev/null 2>&1 || command -v chromium-browser >/dev/null 2>&1; then
        if (( ${#browser_test_args[@]} == 0 )); then
            browser_label="browser tests: no tests/web_*.rs targets"
            echo "==> $browser_label" >&2
            record_failure "$browser_label"
            if (( ! keep_going )); then
                echo "GATE: FAIL at $browser_label"
                exit 1
            fi
        elif ! run_browser_step; then
            record_failure "$browser_label"
            if (( ! keep_going )); then
                echo "GATE: FAIL at $browser_label"
                exit 1
            fi
        fi
    else
        echo "==> browser tests skipped: no google-chrome, chromium, or chromium-browser on PATH"
    fi
fi

if (( ${#failed_steps[@]} == 0 )); then
    echo "GATE: PASS"
    exit 0
fi

echo "GATE: failures:"
for s in "${failed_steps[@]}"; do
    echo "  - $s"
done
echo "GATE: FAIL at $first_failed"
exit 1
