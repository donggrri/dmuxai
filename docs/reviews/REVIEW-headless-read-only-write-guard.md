# REVIEW: headless read-only workspace write guard

Status: complete
Specs: [SPEC-headless-read-only-write-guard.md](../specs/SPEC-headless-read-only-write-guard.md) (FR-2540, FR-2541)
Reviewed: 2026-11-08 (two passes; second pass closes the tail-edit gap)
Reviewer: opencode running GLM free model (dmuxai itself; same free model for both passes) — first pass over the staged WIP, second pass the 2026-11-08 whole-file content-detection follow-up

## Spec compliance

- FR-2540: `src/workspace.rs` builds a content-aware fingerprint: `git status
  --porcelain=v2 -z --untracked-files=all --no-renames -- . ':(exclude).dmuxai'`
  where every entry contributes its status letters, index modes, staged blob
  hash (`hI`), path, and a worktree content summary — the stored length plus a
  whole-file streaming hash of every byte, read in bounded 64 KiB chunks (no
  fixed prefix shortcut); stable markers for unreadable and non-regular
  paths; porcelain paths resolve against `git rev-parse --show-toplevel` since
  the repo root may sit above the workspace. `App::run_headless` captures the
  fingerprint just before the prompt is submitted and fails otherwise-successful
  read-only outcomes when it changes: `ok: false`, exit 1, `error_kind: policy`
  (`READ_ONLY_WORKSPACE_WRITE_ERROR`).
- FR-2541: the CLI's hidden `--read-only` maps to the existing `ask`
  (`TurnPolicy::ReadOnly`) path of FR-1824, so the same guard applies; the CLI
  integration test uses the hidden flag.
- 2026-11-08 tail-edit correctness follow-up: the earlier implementation
  summarized content as length + hash of the first 1 MiB, which misses a
  same-length edit past that prefix in an already-dirty/untracked file. That is
  part of #257's requested write detection (FR-2540), not an accepted residual,
  so the summary now hashes the entire file with bounded memory. TC-2543
  proves it: for an already-dirty tracked `big.txt` (> 1 MiB) and an
  already-untracked `UNTRACKED_BIG.txt` (> 1 MiB), changing exactly one byte at
  offset 1 MiB keeps the file length and the porcelain line identical
  (asserted via `git status --porcelain` in the test) yet the before/after
  fingerprints differ — the first-MiB-prefix design fails this test.

## Regression and safety

- Write-capable runs skip the check entirely (`TurnPolicy::Agent`).
- Non-git workspaces return `None` fingerprints and keep prior behaviour
  (`read_only_workspace_write_detected` returns `false`).
- The guard only replaces a would-be success outcome; agent errors and
  cancellation keep precedence (checked before the policy branch).
- Per-entry caps remain for cost bounding (4096 porcelain entries; 64 KiB read
  chunk; memory per file stays O(chunk), not O(file)); the whole ride is
  behind the existing 5 s bounded wait. Whole-file hashing costs more I/O
  than the old 1 MiB prefix; the residual effect is rate-limiting, not
  correctness of what is detected.
- Non-regular files (FIFOs/sockets) are no longer opened at all — a blocking
  `open(2)` could have hung the sampling thread; they get a stable
  `unreadable` marker. Covered by the dedicated no-hang test.
- No provider argv, prompt, or parsing change; no persistence schema change;
  git is invoked read-only with stdin/stderr silenced and never writes into the
  workspace.

## Test evidence (all actually executed 2026-11-08)

| Command | Result |
| --- | --- |
| `cargo test --lib tc_2540` | ok, 1 passed / 0 failed |
| `cargo test --lib tc_2543` | ok, 1 passed / 0 failed (new tail-edit regression) |
| `cargo test --lib fingerprint_skips_untracked_fifo` | ok, 1 passed / 0 failed |
| `cargo test --bin dmuxai tc_2542` | ok, 1 passed / 0 failed |
| `cargo test --test agent_api_cli tc_2541` | ok, 1 passed / 0 failed |
| `cargo fmt --check` | PASS |
| `bash scripts/check-spec.sh docs/specs/SPEC-headless-read-only-write-guard.md` | PASS |
| `python3 scripts/check-spec.py --all` | PASS: 87 specs, 87 pass, 0 fail |
| `python3 scripts/check-tc-links.py` | PASS after TC-2543: 1058 defined, 397 strong, 434 comment-only, 227 allowlisted, 0 orphan, 0 undefined-test, 0 stale |

Note: `tc_2542` lives in `src/app.rs`, which belongs to the `dmuxai` binary
target (`mod app` in `src/main.rs`); `cargo test --lib` filters it out, so the
review cites the `--bin dmuxai` invocation.

## Independent review of remaining fail-open / cancellation / non-UTF-8 limits

The following limitations were identified in an independent pass and are
reported honestly. They are **not** silently accepted as "full protection":

- **Timeout / missing git fail-open (pre-existing):** if `git` is missing or
  the 5 s wait expires, the fingerprint is `None` ⇒ no detection for that run.
  Whole-file hashing increases sample I/O, so very large worktrees make this
  fail-open more likely than the 1 MiB prefix did. Documented in the spec's
  platform row.
- **Cannot be cancelled:** the sampling runs in `tokio::spawn_blocking`, which
  cannot be interrupted. If the run is cancelled while sampling, the 5 s wait
  simply abandons the result and the background thread keeps reading files
  (read-only, individually bounded) until done; nothing is corrupted, but
  cancellation does not reclaim the in-flight sampling promptly.
- **Non-UTF-8 paths:** porcelain bytes are `from_utf8_lossy`-converted.
  Two distinct non-UTF-8 filenames can collapse to the same replacement-char
  string, and `repo_root.join(lossy)` may not resolve a real file ⇒ the entry
  hashes as an `unreadable` marker, so tail edits to an already-dirty
  non-UTF-8-named file are not content-aware there (staged `hI` still tracks
  staged content). Degraded, not fully protective.
- **4096-entry cap:** files beyond that cap contribute only a `truncated`
  marker; their contents are not hashed, so tail edits to them are undetected.
- **Sampling race (TOCTOU):** per-file reads happen after the status listing
  without synchronization; a write racing inside the sampling window may land
  before both samples or make a file unreadable, in which case detection
  relies on the stable `unreadable` marker and misses the content change.
- **Unreadable-marker fail-open:** a file readable in one sample but
  permission-denied in both "before" and "after" samples yields identical
  `unreadable` summaries — a content change that coincided with chmod to
  unreadable may be invisible beyond the porcelain line.
- **Submodules / raw-token entries:** entries other than `1`/`u`/`?` are kept
  as raw porcelain tokens with no content summary, and `git status` in the
  superproject does not recurse into submodule worktrees; writes inside
  submodules are not content-scanned.
- **Non-regular files:** FIFOs/sockets/devices are not opened (fix introduced
  here), and upstream `git status` does not list FIFOs at all, so writes
  conveyed by creating them are invisible to this guard.

## ID governance

- FR-2450/TC-2450/TC-2451 are already merged on main via #258
  (`SPEC-opencode-workspace-cwd.md`, `src/process/mod.rs`,
  `src/custom_agent.rs`, `tests/agent_api_cli.rs`); the WIP reusing them would
  have collided. All issue-257 identifiers were moved to the PR266 reservation
  FR-2540/FR-2541 and TC-2540/TC-2541/TC-2542, verified unused across all
  fetched refs (`git grep` over `main`, `origin`, `v0.3.0`) and the PR260
  reserve 2509–2512 (untouched).
- The parallel upstream branch `origin/fix/issue-257-read-only-write-detection`
  also reuses the colliding 2450 IDs and was not merged here; it was used only
  as independent prior art.

## Residual risks and intentionally untested paths

- The fail-open, cancellation, non-UTF-8, cap, race, and submodule limits above
  stand as reviewed; the guard is defense in depth on read-only runs, not a
  guarantee of full coverage. The former "1 MiB prefix" trade-off is gone: the
  content summary now covers whole files (TC-2543).
- Renames (`--no-renames`) surface deletes+adds as separate entries.
- Full `scripts/gate.sh` and live-provider smokes were intentionally not run
  per task scope; focused gates above are the evidence.

## Rollback

Revert the before/after fingerprint in `run_headless` and the
`workspace::git_worktree_fingerprint` helpers together with these artifacts.
