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
help=0

for arg in "$@"; do
    case "$arg" in
        --isolated) isolated=1 ;;
        --no-isolated) isolated=0 ;;
        --browser) browser=1 ;;
        --quick) quick=1 ;;
        --keep-going) keep_going=1 ;;
        --print-env) print_env=1 ;;
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
  --isolated        Run every step with all DMUXAI_* variables unset (default).
  --no-isolated     Do not unset DMUXAI_* variables.
  --browser         After the normal steps, run ignored browser tests.
  --quick           Skip clippy and run only the spec_index test.
  --keep-going      Run all steps and summarize failures at the end.
  -h, --help        Show this help and exit.

Hidden/debug:
  --print-env       Print the env var names passed to steps and exit.
USAGE
    exit 0
fi

if (( print_env )); then
    if (( isolated )); then
        compgen -e | grep -v '^DMUXAI_' | sort -u
    else
        compgen -e | sort -u
    fi
    exit 0
fi

# Build env(1) arguments that unset every DMUXAI_* variable.
env_unset_args=()
for var in "${dmuxai_vars[@]}"; do
    env_unset_args+=(-u "$var")
done

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
        env "${env_unset_args[@]}" "$@"
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
        browser_label="cargo test --all-features -- --ignored web_"
        if ! run_step "$browser_label" cargo test --all-features -- --ignored web_; then
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
