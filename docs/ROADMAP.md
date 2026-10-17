# dmuxai roadmap

Release history is in [CHANGELOG.md](../CHANGELOG.md). Each item links its SPEC (the contract) and its GitHub issue (status). GitHub milestone: [v0.2.1](https://github.com/donggrri/dmuxai/milestone/1).

## v0.2.1 (in progress)

Source: Dongjin's dogfooding of the v0.2.0 release binary on Linux (`dmuxai --web`, Cursor and opencode runners), 2026-10-17 23:38–23:51 KST.

| # | Item | Kind | SPEC (IDs) | Issue | Status |
| --- | --- | --- | --- | --- | --- |
| 1 | Cursor assistant text shown twice | bug | [SPEC-cursor-stream-dedupe](specs/SPEC-cursor-stream-dedupe.md) (FR-800..801) | [#40](https://github.com/donggrri/dmuxai/issues/40) | merged #39 (049dc68) |
| 2 | Opencode missing on a fresh install: built-in preset, `dmuxai init`, first-run docs | bug | [SPEC-first-run](specs/SPEC-first-run.md) (FR-810..814; FR-815 README first run) | [#45](https://github.com/donggrri/dmuxai/issues/45) | merged #48 (e394ab7) |
| 3 | README: release tarball layout and install steps | docs | [SPEC-first-run](specs/SPEC-first-run.md) (FR-816) | [#46](https://github.com/donggrri/dmuxai/issues/46) | merged #48 (e394ab7) |
| 4 | Running state: spinner and elapsed time in TUI and dashboard | UX | [SPEC-running-indicators](specs/SPEC-running-indicators.md) (FR-820..824, UX-820..821, NFR-820) | [#42](https://github.com/donggrri/dmuxai/issues/42) | merged #49 (f6165fe) |
| 5 | Documents: collapsible folder tree in a right sidebar | UX | [SPEC-docs-tree](specs/SPEC-docs-tree.md) (FR-830..833, UX-830) | [#43](https://github.com/donggrri/dmuxai/issues/43) | merged #50 (9c258eb) |
| 6 | To-dos: header button and sidebar panel | UX | [SPEC-todo-sidebar](specs/SPEC-todo-sidebar.md) (FR-840..842, UX-840) | [#44](https://github.com/donggrri/dmuxai/issues/44) | merged #52 (0c25cac) |
| 7 | Cursor latency: measured; pre-warm the next cursor-agent | perf | [SPEC-cursor-latency](specs/SPEC-cursor-latency.md) (FR-805..807, NFR-805) | [#41](https://github.com/donggrri/dmuxai/issues/41) | merged #51 (bf31a2e) |
| 8 | Release v0.2.1: CHANGELOG, version bump, tag, verify the Linux and Windows assets | release | — | — | release PR; tag `v0.2.1` on its merge commit |

Order: 1 → 2+3 → 4 → 5 → 6 (4–6 share the web assets, so they go in sequence). 7 runs in parallel with 4–6. The release waits for all of 1–7.

## Later (not scheduled)

- Persistent cursor-agent session server, if cursor-agent ever ships one (would remove the cold start that pre-warm only hides).
- Tree view in the TUI file picker (the dashboard tree comes first).
- Windows compatibility (postponed in v0.2.1): job objects for Cursor warm processes (SPEC-cursor-latency FR-807), and re-check v0.2.1 features on Windows.
