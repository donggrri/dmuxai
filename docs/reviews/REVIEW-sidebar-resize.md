# REVIEW: sidebar-resize

Spec: [SPEC-sidebar-resize](../specs/SPEC-sidebar-resize.md) (FR-970..974, UX-970..971, TC-970..972)
Implementer: Grok Build (`grok -m grok-4.7 -p`, headless), four scoped passes (implementation, JS/CSS review fixes, test fixes, follow-ups).
Reviewer: **claude-sonnet-5-5-high via cursor-agent** (`cursor-agent -p --model claude-sonnet-5-5-high --mode ask`, read-only), two rounds. The opencode free models were tried first: in round 1 deepseek-v4.1-flash-free and glm-5.3-flash-free both timed out after 600s with no output; in round 2 deepseek timed out (420s) and glm returned "OpenCode's free models aren't available from your current network". Per Dongjin's 00:34 KST instruction the review fell back to sonnet-5.5-high.

## Round 1 — CHANGES REQUESTED (no blockers)

MAJOR
1. A click on a handle without moving wrote the current displayed width to storage and `preferred`, losing a wider preference. Fixed: a click (≤3px) restores the start width and does not persist; only the double-click timer is armed.
2. `pointercancel` used the event's `clientX` and persisted. Fixed: `pointercancel` and `lostpointercapture` revert to the start width and never persist; a window-level fallback is used if pointer capture fails.
3. TC-970 checked strings that already existed in `origin/main`. Fixed: checks are anchored to the new rule blocks and code forms (`MIN_WIDTH = 180`, `Math.floor(vw * 0.5)`, `SHIFT_STEP = 64`, `addEventListener('pointercancel'`, `document.documentElement.clientWidth`, …).
4. PLAN/TASKS/REVIEW docs missing. Fixed: PLAN-sidebar-resize, TASKS-sidebar-resize, this file. The version bump is in the release PR (#69).

MINOR (all fixed): viewport width uses `documentElement.clientWidth` and the right width is measured from the sidebar's right edge (scrollbar-safe); `relayout()` runs after every drag end; programmatic width changes use `body.sidebar-resize-instant` so body padding does not lag; handles moved fully outside the inner edge so they do not cover the list scrollbar; tests use literal values and invariants instead of re-implementing the clamp, poll with `wait_for_function` instead of fixed sleeps, and cover the right-handle clamp, right End key, click-no-persist and cancel-revert. NITs fixed: no-op `overflow: visible` removed, static `aria-valuemax` no longer pinned, pointer offset kept on drag start, `forced-colors` and `hover: hover` rules for the accent line.

## Round 2 — APPROVE

Items 1–3, 5, 6, 9–11 FIXED; 4, 7, 8 PARTIAL. New MINORs, fixed after the round:
- NEW-1: the window fallback path did not listen for `pointercancel`. Fixed, and a stale drag is aborted on the next `pointerdown`.
- NEW-2: relayout after the other sidebar opens/closes did not use the instant class. Fixed: instant is applied whenever a displayed width actually changes.
- NIT: comments, `requestAnimationFrame` fallback (50ms `setTimeout`), spec UX-971 / TC rows updated; constant asserts that tripped clippy removed.

Residual risks (accepted):
- NEW-3: the code assumes Chromium's order (`pointerup` before `lostpointercapture`); the e2e only runs in Chromium. In another order a drag could revert on release.
- The 8px handle sits over the chat side of the border and takes clicks in that strip.
- On release, `relayout()` recomputes from both preferences, so the other open sidebar can grow back toward its preferred width (consistent with "preference kept").
- The `prepare()` SKIP path passes when Chrome/Playwright is missing (same convention as the other web e2e tests).

## Verification (2026-10-19 KST, rebased on a5de430)

- `cargo fmt --all -- --check`: clean
- `cargo clippy --all-targets -- -D warnings` and `--target x86_64-pc-windows-gnu`: clean
- `cargo test`: 608 passed, 0 failed
- `cargo test --test web_sidebar_resize --test web_prompt_center --test web_session_sidebar --test web_todo_sidebar --test web_docs_preview --test web_graph_view --test web_workspace_dir --test web_running -- --ignored`: 15 passed (TC-971, TC-972 included; also run 3× in a row by the implementer without flakes). TC-851 centered prompt still passes.
- The `--ignored` lib tests that need real codex/cursor/agy CLIs fail in this box as on main (unrelated).
- Light-theme screenshots: `v022-sidebar-resize-default.png` (320/320), `-wide.png` (≈440/460, hover accent on the right handle), `-narrow.png` (180/180 clamp, focus ring on the left handle).
