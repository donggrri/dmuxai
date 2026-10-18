# dmuxai roadmap

Release history is in [CHANGELOG.md](../CHANGELOG.md). Each item links its SPEC (the contract) and its GitHub issue (status). GitHub milestones: [v0.2.1](https://github.com/donggrri/dmuxai/milestone/1) (released), [v0.2.2](https://github.com/donggrri/dmuxai/milestone/2).

## v0.2.4 (planned, after v0.2.3)

Source: Dongjin, 2026-10-18 23:3x KST: "모든 TUI 기능이 대시보드에서 구현이 된다면 TUI는 사용하지 않고 제거하는 방향으로 간다" and "그 기능은 다음 릴리즈에". Not part of v0.2.2 or v0.2.3.

| # | Item | Kind | SPEC (IDs) | Issue | Status |
| --- | --- | --- | --- | --- | --- |
| 1 | Dashboard full parity and TUI retirement: headless interactive `dmuxai serve`, every TUI-only feature on the dashboard with logic in the shared Rust core, then remove the TUI | feature | [SPEC-dashboard-full-parity](specs/SPEC-dashboard-full-parity.md) (FR-1000..) | — | spec draft; gap list awaiting Dongjin's review |

The grok runner ([#66](https://github.com/donggrri/dmuxai/pull/66), merged for v0.2.2) is already on the dashboard under `--web`; it must also work under the new interactive `serve`. The plan gate ([#67](https://github.com/donggrri/dmuxai/pull/67)) and the rest of v0.2.3 must be reachable from the dashboard before the TUI is removed. #70 (TUI picker width, merged (f1b36ae)) is TUI-only and #71 (resizable sidebars) is dashboard-only; neither adds a parity gap.

## v0.2.2 (in progress)

Source: Dongjin, 2026-10-18 09:42 KST.

| # | Item | Kind | SPEC (IDs) | Issue | Status |
| --- | --- | --- | --- | --- | --- |
| 1 | Prompt box stays centered with sidebars open | UX | [SPEC-centered-prompt](specs/SPEC-centered-prompt.md) (FR-850..852, UX-850) | [#54](https://github.com/donggrri/dmuxai/issues/54) | planned |
| 2 | Left sidebar of sessions/conversations (ChatGPT-style), TUI picker parity | UX | [SPEC-session-sidebar](specs/SPEC-session-sidebar.md) (FR-860..864, UX-860) | [#55](https://github.com/donggrri/dmuxai/issues/55) | planned |
| 3 | Set the project working directory (CLI, config, dashboard) | feature | [SPEC-workspace-dir](specs/SPEC-workspace-dir.md) (FR-870..875, NFR-870) | [#56](https://github.com/donggrri/dmuxai/issues/56) | planned |
| 4 | Graph view: horizontal swimlane timeline (replaces the subway map) | UX | [SPEC-graph-swimlane](specs/SPEC-graph-swimlane.md) (FR-880..885, UX-880..881) | [#61](https://github.com/donggrri/dmuxai/issues/61) | merged ([#65](https://github.com/donggrri/dmuxai/pull/65)) |

Order: 1 → 2 → 3 → 4 (1 and 2 share the layout; 2 and 3 both touch the header). No release in this round.

## v0.2.1 (released 2026-10-18)

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
- TUI workspace switcher (v0.2.2 switches from the dashboard and with `-C`).
- Windows compatibility (postponed in v0.2.1; v0.2.2 workspace switch answers 501 on Windows): job objects for Cursor warm processes (SPEC-cursor-latency FR-807), and re-check v0.2.1 features on Windows.
