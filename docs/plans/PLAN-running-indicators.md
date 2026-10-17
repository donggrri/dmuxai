# PLAN: running-indicators

Spec: [SPEC-running-indicators](../specs/SPEC-running-indicators.md)
Tasks: [TASKS-running-indicators](../tasks/TASKS-running-indicators.md)

## Order of work

1. `RunState` model and snapshot serialization: `RunState` per agent (`idle`, `running`, `done`, `failed`, `cancelled`), `duration: Option<Duration>` omitting elapsed on finish without a prior running entry, and `RunStateView` carried in `DashboardSnapshot` (FR-820, NFR-820).
2. TUI braille spinner, status bar integration, and end marks: braille frames `⠋⠙⠹...` at ~10 fps while running, static `●` mark under reduced motion, and `✓ done`, `✗ failed`, `■ cancelled` marks (FR-821, FR-822, UX-820, UX-821).
3. Web dashboard global status pill, agent chips, and graph station spinner: `#run-status` pill, `.chip-running` spinner with in-place elapsed update, and dedicated `.station-spinner` nested `<g>` element avoiding CSS transform translation collisions (FR-823, UX-821).
4. Browser tab title and favicon state indicators: tab title prefix `⏳` while running, `✓`/`✗` on completion, restored on window focus; small same-origin SVG favicons (idle, running, done, failed) swapped dynamically while respecting `default-src 'self'` CSP (FR-824).
5. State transitions & event log mapping: `status_json` in `src/agent_api.rs` maps `pending` and `stalled` runner states to `running`, preserving original `started_at_ms`.
6. Verification & documentation: asset assertions (TC-823), headless Chrome e2e test with non-empty screenshot assertion (TC-824), clippy, and full repository test suites.

## Risks

- SVG transform conflict: placing CSS rotation on translated `<g transform="translate(x,y)">` overrides SVG translation, causing nodes to jump to `(0,0)`. Mitigated by placing `.station-spinner` on a dedicated nested `<g>` with `transform-box: fill-box; transform-origin: center`.
- Animation reset on DOM replacement: rebuilding agent chips and pill every second restarts CSS keyframe animations. Mitigated by updating elapsed text in place and rebuilding only when agent/state sets change.
- TUI event loop CPU consumption: fast redraws for braille spinner must only run while an agent is running and reduced motion is disabled. Idle pause yields 5 ms (`EVENT_LOOP_PAUSE = 5ms`), with fast redraw capped at 100 ms.

## Rollback

Revert commit/changes on `feat/running-indicators`. `RunStateView` fields are optional with defaults, so no database schema migrations are affected.
