#!/usr/bin/env bash
# SPEC index generator (SPEC-spec-index.md FR-472 / TC-472).
#
# Prints the SPEC index as a markdown table with one row per
# docs/specs/SPEC-*.md: the SPEC link, its Status line (FR-470: within the
# first 10 lines of the file), and the ID ranges it defines as the first cell
# of a table row. Run from anywhere; run `bash scripts/spec-index.sh`.
#
# ID uniqueness across SPEC files is enforced by `tests/spec_index.rs`
# (FR-471), not here.

set -eu

specs_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)/docs/specs"

printf '| SPEC | Status | ID ranges |\n'
printf '|---|---|---|\n'

for spec in "$specs_dir"/SPEC-*.md; do
    name="$(basename -- "$spec")"

    status="$(awk '
        /^Status:/ { sub(/^Status:[[:space:]]*/, ""); sub(/[[:space:]]+$/, ""); print; exit }
        NR >= 10 { exit }
    ' "$spec")"
    if [ -z "$status" ]; then
        status="missing"
    fi

    ranges="$(grep -E -o '^\|[[:space:]]*(FR|NFR|UX|INT|TC)-[0-9]+[[:space:]]*\|' "$spec" \
        | awk '{ n = split($2, a, "-"); print a[1], a[2] + 0 }' | sort -u \
        | awk '
        {
            if (!($1 in lo)) { lo[$1] = $2; hi[$1] = $2 }
            if ($2 + 0 < lo[$1] + 0) { lo[$1] = $2 }
            if ($2 + 0 > hi[$1] + 0) { hi[$1] = $2 }
        }
        END {
            m = 0
            for (p in lo) { keys[++m] = p }
            for (i = 1; i <= m; i++) {
                for (j = i + 1; j <= m; j++) {
                    if (keys[j] < keys[i]) { t = keys[i]; keys[i] = keys[j]; keys[j] = t }
                }
            }
            out = ""
            for (i = 1; i <= m; i++) {
                p = keys[i]
                if (lo[p] == hi[p]) { r = p "-" lo[p] }
                else { r = p "-" lo[p] ".." hi[p] }
                out = out (out == "" ? "" : ", ") r
            }
            print out
        }')"

    printf '| [%s](docs/specs/%s) | %s | %s |\n' "$name" "$name" "$status" "${ranges:-—}"
done
