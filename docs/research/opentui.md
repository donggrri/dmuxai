# Research: OpenTUI vs. DmuxAI's Rust + Ratatui stack

- Status: research documentation only. No code changed.
- Date: 2026-10-17
- Subject: [OpenTUI](https://opentui.com/) (the TypeScript/Zig TUI framework behind
  [opencode](https://github.com/anomalyco/opencode)) evaluated against DmuxAI's current
  [Rust + Ratatui](../..) stack as the UI layer.
- Facts below were verified against live sources on the date above; anything that is
  an estimate rather than a published fact is marked as such.

## Summary recommendation

**Keep Ratatui for now. Do not migrate the DmuxAI UI to OpenTUI.**

The concrete, worthwhile takeaway is smaller than a framework swap: DmuxAI already
stores image attachments (README: [Image attachments](../../README.md#image-attachments))
but renders only `[Image #N]` chips. The [ratatui-image](https://crates.io/crates/ratatui-image)
crate can render attached PNG/JPEG/WebP previews in the chat pane on terminals that
support Kitty graphics or Sixel, with a half-block fallback everywhere else. That is a
days-scale change inside the existing render loop, not a rewrite.

OpenTUI is impressive — a production-hardened Zig core with a C ABI, excellent
image support, and real-world use under opencode — but it is a different package: a
TypeScript (Bun/Node) frontend framework, still 0.x with weekly breaking churn, whose
native library has no official Rust bindings. Adopting it means either rewriting the
whole client in TypeScript or splitting DmuxAI into a Rust core daemon plus a TS
frontend over IPC. Both conflict with today's product contract: the Windows installer
must require no Rust, Cargo, or Git and installs `dmuxai.exe` only
([README requirements](../../README.md#requirements)), and MISSION.md lists
"Replacing Ratatui with another UI framework" as an explicit
[non-goal](../../MISSION.md#non-goals-for-now).

## What OpenTUI is

- A terminal UI library with a **native core written in Zig** ("with a focus on
  correctness, stability, and high performance"), TypeScript bindings over FFI, and
  an exposed **C ABI "usable from any language"**. It renders a component tree laid
  out with Flexbox and updates only changed cells
  ([npm @opentui/core](https://www.npmjs.com/package/@opentui/core),
  [docs introduction](https://opentui.com/docs/getting-started/)).
  GitHub reports ~13.6k stars, 776 forks, 1,246 commits
  ([github](https://github.com/sst/opentui) — the repo formerly under the `sst` org
  now resolves to [anomalyco/opentui](https://github.com/anomalyco/opentui), which is
  documented under [opentui.com](https://opentui.com)).
- **Packages**: `@opentui/core` (renderer + TS API), `@opentui/react` and
  `@opentui/solid` (framework renderers), plus `@opentui/keymap` (key routing),
  `@opentui/qrcode`, `@opentui/three` (WebGPU, Bun-only), and `@opentui/ssh`
  (serve a TUI over SSH) ([README/packages](https://github.com/sst/opentui#packages)).
- **Components** relevant to a chat client: Text, Box, Input, Textarea, Select,
  TabSelect, ScrollBox, Markdown, Code (Tree-sitter highlighting), Diff,
  Image, Embedded terminal (host a child PTY inside the UI)
  ([component index](https://opentui.com/docs/components)).
- **Latest release**: GitHub Releases show **v0.6.2 (2026-10-12)** as latest, with a
  roughly weekly cadence of small native fixes from a small maintainer group
  ([releases](https://github.com/sst/opentui/releases)); the documentation site
  tracks ahead at 0.6.6
  ([docs changelog](https://opentui.com/docs/reference/terminal-capabilities#changes)).
  npm mirror snapshots in the same week variously report 0.4.x–0.6.x as "latest",
  confirming rapid, uneven version churn across registries
  ([npm](https://www.npmjs.com/package/@opentui/core)). Version is 0.x; no 1.0.
- **License**: MIT ([repo](https://github.com/sst/opentui#license)).
- **Language bindings today**: TypeScript first (Bun 1.3.14+ or Node 26.4.0+ with
  `--experimental-ffi` and ESM); React 19.2+ and Solid 1.9.12 renderers; eight
  cross-compiled native artifacts (macOS x64/arm64, Linux x64/arm64 glibc+musl,
  Windows x64/arm64) ([runtime and platform support](https://opentui.com/docs/getting-started/runtime-support)).
  There is **no official Rust binding**; the C ABI is documented for bindings authors
  but shipping crates/tooling are not provided
  ([npm readme](https://www.npmjs.com/package/@opentui/core),
  [native overview](https://opentui.com/docs/native/overview)).
- **Maturity signals**: powers **opencode in production** ("for millions of users",
  ~213k stars on the opencode repo), plus terminal.shop announced
  ([npm readme](https://www.npmjs.com/package/@opentui/core)). Test matrix: Bun/Node
  core tests on macOS arm64, Linux x64, Windows x64; Node single-executable
  acceptance runs on **Linux x64 only**
  ([runtime support](https://opentui.com/docs/getting-started/runtime-support),
  [standalone executables](https://opentui.com/docs/reference/standalone-executables)).

## Fit with DmuxAI

DmuxAI's current architecture
([README architecture](../../README.md#architecture)) is a single Rust binary:

```text
TUI (ratatui/crossterm) -> ConversationEngine -> SQLite WAL
  -> adapter actors (codex/cursor/agy, headless JSONL/NDJSON processes)
  -> provider-independent AgentEvent -> coalesced ratatui render (25 ms)
```

What an OpenTUI adoption would and would not touch:

| Concern | Status under OpenTUI |
| --- | --- |
| Adapter protocol layer (`src/agent/*`, headless processes, cancel/resume) | Untouched — provider-neutral by design (README, [MISSION.md](../../MISSION.md)). |
| SQLite persistence (`src/conversation.rs`) | Untouched (stays in Rust). |
| Graph orchestration, `.dmuxai/` state, custom runners (`[[agents.custom]]`) | Untouched (config + core behavior, not UI). |
| UI: event loop, pickers, slash previews, autoscroll, text selection/clipboard, 25 ms coalescing ([`src/app.rs`](../../src/app.rs)) | Fully rewritten — this is the actual migration cost. |
| Image paste pipeline (arboard, `wl-paste`/`xclip`/WSL `powershell.exe` fallbacks in [`src/clipboard.rs`](../../src/clipboard.rs)) | Clipboard ingestion stays in Rust; a TS frontend that owns the keyboard would have to forward Ctrl+V bytes over IPC so the core can still read pixels. OpenTUI's own clipboard support is OSC 52–oriented ([terminal capabilities](https://opentui.com/docs/reference/terminal-capabilities)), which does not replace pixel-image extraction from the Windows clipboard. |
| `[Image #N]` rendering | This is where OpenTUI genuinely shines — see the image section below. |

Points in OpenTUI's favor for DmuxAI specifically:

- **The opencode precedent is exactly the hybrid shape**: a TS application on top of
  OpenTUI, which is the "Rust core + TS frontend over IPC" option rather than a
  whole-program rewrite.
- **Embedded terminal component** could in principle host an interactive child CLI
  (e.g. MISSION.md item 5's interactive cursor-agent pass-through on Linux/WSL)
  inside the DmuxAI UI; Ratatui offers no such primitive and DmuxAI would otherwise
  need to pair a vte/ANSI-screen parser by hand.
- Image, scrollback, and input quality (Kitty keyboard protocol, bracketed paste,
  pixel-size-aware images) are ahead of what Ratatui offers out of the box.

Points against:

- **No Rust bindings.** The TypeScript boundary is mandatory unless DmuxAI writes and
  maintains its own C-ABI FFI layer against a 0.x Zig core — undefined high-risk work.
- **Runtime dependency change.** Every user installation would need Bun or Node at
  runtime (Node additionally needs `--experimental-ffi`
  [runtime support](https://opentui.com/docs/getting-started/runtime-support)), versus
  today's single static Rust binary with no runtime beyond the provider CLIs.
- **Windows distribution regression** — see the next section.

## Windows and WSL support

- **OpenTUI Windows status**: supported and cross-compiled (`@opentui/core-win32-x64`
  and `-win32-arm64`), with Bun/Node core tests on Windows x64 in CI
  ([runtime support](https://opentui.com/docs/getting-started/runtime-support)).
  However, tracked Windows issues historically include: closing the terminal/tty on
  exit (not occurring under WSL), resize and mouse breakage (later fixed), Three.js
  demo breakage, flicker in Windows Terminal/VS Code, and 16–20 fps limits in some
  Windows terminal paths
  ([issue #152, closed](https://github.com/anomalyco/opentui/issues/152)).
  WSL is explicitly detected and supported (`WSL`-related environment keys are part of
  terminal detection
  [terminal capabilities](https://opentui.com/docs/reference/terminal-capabilities)).
- **Node-based Windows deployment is upstream-untested for executables**: the Node SEA
  acceptance lane runs on Linux x64 only
  ([standalone executables](https://opentui.com/docs/reference/standalone-executables)).
  A Bun-compiled Windows frontend is the more beaten path, but adds a large embedded
  runtime to the payload (size unaudited in this review).
- **DmuxAI today ships a native Windows exe** with an installer that "does not
  require Rust, Cargo, or Git" and installs `dmuxai.exe` only, with provider CLIs as
  independent installs ([README](../../README.md#requirements)); children get
  `CREATE_NO_WINDOW` and Cursor.is-Implement is correctly gated there
  ([safety/platform behavior](../../README.md#safety-and-platform-behavior)).
  A TS-frontend DmuxAI would need to either embed a JS runtime in that installer or
  require Bun/Node — both against the spirit of the current install contract unless
  MISSION.md's "Windows ships as a launcher that runs DmuxAI inside WSL" direction
  becomes the official position ([MISSION.md item 6](../../MISSION.md#product-direction)).
- **WSL**: both stacks are fine. Ratatui/crossterm is already used inside WSL today,
  and OpenTUI treats WSL as a first-class detection target. Under the WSL-launcher
  platform direction, OpenTUI's Windows-native gaps matter less.

Net: for the current **native-Windows exe** product, OpenTUI is a regression risk;
under a WSL-first product direction, it stops being the blocker.

## Image rendering: OpenTUI vs. the ratatui-image crate

DmuxAI's image feature today is attachment management only (paths copied into
`.dmuxai/attachments/`, chips in the prompt bar; README:
[Image attachments](../../README.md#image-attachments)). Neither stack is currently
used to preview images inline.

| Dimension | OpenTUI `[Image](https://opentui.com/docs/components/image)` | [ratatui-image](https://crates.io/crates/ratatui-image) |
| --- | --- | --- |
| Protocols | Kitty (incl. `raw`/`zlib`/file transport, tmux passthrough), Sixel (pixel-geometry aware), Unicode quadrant blocks fallback; auto-resolution policy per terminal | Kitty (stateful, tmux-aware), Sixel (`icy_sixel`), iTerm2, half-blocks; protocol picked up-front by its capability Picker |
| Detection | Runtime capability queries with a live `TerminalCapabilities` snapshot (`kitty_graphics`, `sixel`, `image_protocol`) | Picker probes (ioctl font-size; Kitty fonts) at startup |
| Fidelity | Quadrant blocks (better than half-block fallback), alpha handling per protocol, fit/cover/fill sizing | Half-blocks (`▀` fg/bg) fallback; optional chafa feature |
| Integration | Owns the whole terminal; renderable-tree model (React/Solid friendly) | A widget inside an existing Ratatui render loop — incremental adoption |
| Maturity | Powers opencode production; recent fixes to iTerm2 Kitty placement (0.6.4) | Stable at 11.1.0, 12.0.0-rc.2 pre-release ([crates.io](https://crates.io/crates/ratatui-image)); actively maintained |
| Risk for DmuxAI | Requires adopting OpenTUI entirely | Low: drop-in widget, same Rust runtime; verify Ratatui 0.30 compatibility (`dmuxai` pins `ratatui 0.30.2`, [Cargo.toml](../../Cargo.toml)) |

Advantage OpenTUI for full-bleed image UX (scrollback images, overlapping images,
transports), advantage ratatui-image for a bounded "preview in chat" feature. For
DmuxAI's actual requirements — show the image a user just pasted before sending —
the widget-grade option is sufficient: parity between image and chat,
confirmation of what was attached, and graceful blocks fallback in SSH/mux.

## Migration cost (rewrite vs. Rust core + TS frontend over IPC)

Three realistic options, sizes are estimates (unmarked points above are sourced):
| Option | Description | Estimated size | Durable risk |
| --- | --- | --- | --- |
| A. Direct Rust use of OpenTUI | Bind against the Zig C ABI from Rust | Very large and undefined: no official bindings exist, no idiomatic React/Solid layer, integrate input + capability detection by hand | High; upstream churn (0.x, weekly breaking API changes such as the removals in v0.6.0) |
| B. Full TS rewrite | Rewrite the client (event loop, pickers, slash system, autoscroll, selection/paste, theme) in TypeScript on OpenTUI; move core (SQLite, adapters, graph, attachments) to a Rust daemon; talk JSONL over stdio/pipe | 4–8 weeks to reach current-feature parity (estimate), plus a permanent two-language build/install matrix | Distribution on Windows (embedded runtime), IPC protocol becomes product surface that needs versioning and tests |
| C. Incremental (ratatui-image) | Add inline image preview to the existing Ratatui UI | Days, not weeks (estimate) | Low; contained behind a feature spec per the existing delivery workflow |

Session shape worth noting for Option B: DmuxAI's process model (one persistent Codex
app-server, per-turn headless Cursor/AGY processes, bounded mpsc queues, `kill_on_drop`
cancellation) would become invisible to the frontend behind an event protocol that
resembles [`AgentEvent`](../../src/agent/mod.rs) today. That is a clean seam but the
IPC protocol design, reconnection behavior, and multi-window ownership semantics in
SQLite (repo README: multiple live windows) must be explicitly specified — this is
where the schedule risk concentrates for B.

## Risks

- **Framework-level**: OpenTUI is 0.x with a fast, breaking release cadence (e.g.
  minimum Bun bumped in 0.6.0, API removals in 0.6.0) ([releases](https://github.com/sst/opentui/releases)).
  Committing now means joining upgrade churn.
- **Platform-level**: Windows-native installer regressions (runtime dependency or
  payload growth; fps/flicker issues open in Windows terminal paths
  ([#152](https://github.com/anomalyco/opentui/issues/152))); Node SEA not validated on
  Windows ([standalone executables](https://opentui.com/docs/reference/standalone-executables)).
- **Team/scope-level**: two-language codebase permanently splits review, test, and
  release processes; graph orchestration and provider adapters stay in Rust regardless.
- **Product/mission-level**: replacing Ratatui is an explicit non-goal
  ([MISSION.md](../../MISSION.md#non-goals-for-now)); migrating on aesthetics grounds
  would need an explicit mission change first.
- **Risk C mitigations**: ratatui-image keeps the terminal-probe, upload, and protocol
  behavior inside one mature crate; the fallback path (half-blocks) cannot regress any
  existing text-only behavior.

## Recommendation and concrete next steps

1. **Do not replace Ratatui now.** Keep MISSION.md's non-goal standing. The value
   OpenTUI offers that DmuxAI actually needs today (inline image preview) is
   obtainable via ratatui-image without any architectural change.
2. **Adopt image preview incrementally (Option C).** Write a short spec per
   [docs/WORKFLOW.md](../WORKFLOW.md) (e.g. `docs/specs/SPEC-image-preview.md`),
   gated on `/image`/Ctrl+V attachment chips, adding preview rendering with
   ratatui-image's capability Picker; fall back to chips-only in terminals without
   graphics support. Estimated days.
3. **Track OpenTUI as a watch item.** Revisit when: (a) a 1.x release exists, (b) the
   Windows flicker/exit issues in #152-class paths are demonstrably resolved on
   Windows Terminal + VS Code, or (c) first-class Rust bindings ship. Until then,
   re-review this document at each opencode 3 minor OpenTUI feature of interest.
4. **If the product direction changes** to a "DmuxAI served/daemon" shape (e.g. a
   user-facing desire to run DmuxAI from SSH with a rich UI, MISSION.md's
   Windows-in-WSL launcher made official), re-evaluate **Option B** with a
   time-boxed prototype: keep this repo's Rust core intact and stand a minimal
   OpenTUI/React frontend against the existing `AgentEvent` stream over stdio JSONL
   before committing to a full migration.
5. Record any mission change and Phase-gate artifacts (spec → plan → tasks → review)
   as required by the delivery workflow before any code in step 2 or 4 execution.

## Sources

- OpenTUI repo (formerly `sst/opentui`, now `anomalyco/opentui`):
  https://github.com/sst/opentui · https://github.com/anomalyco/opentui
- OpenTUI docs site: https://opentui.com
- Runtime and platform support: https://opentui.com/docs/getting-started/runtime-support
- Standalone executables (SEA constraints): https://opentui.com/docs/reference/standalone-executables
- Terminal capabilities (image protocols, WSL/Windows Terminal detection):
  https://opentui.com/docs/reference/terminal-capabilities
- Image component: https://opentui.com/docs/components/image
- GitHub releases (v0.6.2, 2026-10-12): https://github.com/anomalyco/opentui/releases
- Windows support issue #152: https://github.com/anomalyco/opentui/issues/152
- npm @opentui/core: https://www.npmjs.com/package/@opentui/core
- opencode (production user): https://github.com/anomalyco/opencode
- ratatui-image crate: https://crates.io/crates/ratatui-image · docs: https://docs.rs/ratatui-image
- ratatui-image repo: https://github.com/ratatui/ratatui-image
- DmuxAI in-repo context: [README.md](../../README.md), [MISSION.md](../../MISSION.md),
  [docs/specs/DMUXAI-BASELINE.md](../specs/DMUXAI-BASELINE.md), [Cargo.toml](../../Cargo.toml)
