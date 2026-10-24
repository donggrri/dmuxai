# SPEC-input-command

Status: Implemented

Owner: dmuxai
Created: 2026-10-24
Baseline: `18f2913`
Related: refactor scan finding #10, PR-8d

## 1. Decision summary

Input command classification and dispatch handling in `src/app.rs` is refactored into a dedicated input command module (`src/input_command.rs`) as part of PR-8d (finding #10: "one input classifier for TUI, dashboard, and headless entry points, and headless stops borrowing the TUI input buffer").
Previously, prompt submission logic was fragmented across entry points:
- `submit_prompt` in `src/app.rs` checked for escaped leading slashes (`starts_with("//")`), distinguished paths from commands (`first_token_is_path`), parsed command names, and had each match arm re-derive its arguments via `strip_prefix("/<cmd>").unwrap().trim()`.
- `headless_submit_prompt` duplicated the slash command detection check on `req.prompt.trim_start()` and borrowed `self.input` as an execution seam (`self.input = req.prompt.clone(); self.submit_prompt();`), overwriting any active user draft in the TUI buffer.
- `submit_dashboard_prompt` and `preprocess_dashboard_prompt_text` performed partial slash command detection (`starts_with('/') && !starts_with("//")`) without file path disambiguation.

PR-8d introduces:
1. `InputKind` enum (`Escaped`, `Slash { cmd, rest }`, `Plain`), `classify(prompt)`, `is_slash_command(text)`, and moved `first_token_is_path(prompt)` in an App-free `src/input_command.rs` module.
2. Factored `App::submit_precheck(&mut self, text: &str) -> bool` and `App::submit_text(&mut self, prompt: String)` consolidating prompt preprocessing, plan gating, and slash command dispatch using pre-trimmed `rest` arguments.
3. Decoupled `headless_submit_prompt` using `is_slash_command(&req.prompt)` and `self.submit_text(req.prompt.clone())` so headless runs never clobber `self.input`.
4. Documentation of dashboard prompt preprocessing differences as a follow-up towards a unified structured intent type.

## 2. Scope

### In scope

- Bin module `src/input_command.rs` declaring `InputKind`, `classify`, `is_slash_command`, and `first_token_is_path`.
- Re-export of `first_token_is_path` in `src/app.rs` to maintain compatibility with existing callers.
- `App::submit_precheck` handling workspace switch and empty text preflight checks.
- `App::submit_text` containing prompt trimming, slash preview reset, plan gate refusal, image attachment, and classified command dispatch.
- Headless execution via `headless_submit_prompt` avoiding mutation or borrowing of `self.input`.
- Unit tests `tc_2060_classify_escaped_slash_plain`, `tc_2061_is_slash_command_trim_start`, and `tc_2062_headless_slash_does_not_touch_tui_input`.

### Out of scope / non-goals

- Altering dashboard prompt preprocessing rules (`submit_dashboard_prompt` / `preprocess_dashboard_prompt_text`), which currently does not disambiguate paths; left as follow-up for a structured intent representation across all three entry points.
- Moving permission, plan gate, or session-level dispatch policies outside `src/app.rs`.
- Changing slash command syntax, argument semantics, or existing command handling behavior.

### Assumptions and open questions

| ID | Assumption or question | Owner | Needed by |
| --- | --- | --- | --- |
| OQ-2060 | Dashboard prompt handling continues using legacy slash check until unified structured intent model is introduced in follow-up. | dmuxai | PR-8d implementation |

## 3. Terms and state model

| Term | Meaning | Not to be confused with |
| --- | --- | --- |
| InputKind | Classified category of prompt text: Escaped, Slash command with cmd and rest, or Plain text. | Intent or action execution model. |
| Slash command | An input line whose first token starts with `/`, is not an escape `//`, and is not an existing file or directory path. | File path argument or escaped literal slash prompt. |
| Headless submission | Non-interactive execution of a prompt request without altering the interactive TUI state buffer. | Interactive TUI prompt submission via Enter. |

## 4. Requirements

| ID | Priority | Preconditions / trigger | Required observable result | Failure or boundary behavior | Evidence |
| --- | --- | --- | --- | --- | --- |
| FR-2060 | Must | Given a prompt string passed to `classify(prompt)` | Returns `InputKind::Escaped(&prompt[1..])` if starting with `"//"`; returns `InputKind::Slash { cmd, rest }` with command token and trimmed `rest` if starting with `'/'` and `!first_token_is_path`; otherwise returns `InputKind::Plain` | File paths with internal slashes or existing disk paths classify as `Plain`; empty strings classify as `Plain` | TC-2060 |
| FR-2061 | Must | Given a text string passed to `is_slash_command(text)` | Trims leading whitespace and returns `true` iff trimmed text starts with `'/'`, does not start with `"//"`, and `!first_token_is_path` | Returns `false` for empty strings, escaped slashes, file paths, and non-slash text | TC-2061 |
| FR-2062 | Must | Given an `App` with an existing TUI input buffer draft and an incoming headless request with a slash command | `headless_submit_prompt` dispatches the slash command via `submit_text` without modifying or clearing `self.input` | If workspace switch is in progress or prompt is empty, precheck returns false and leaves input buffer untouched | TC-2062 |

## 5. Acceptance scenarios and test design

| Test ID | Requirement IDs | Type | Setup / Given | Action / When | Oracle / Then | Automated location or command |
| --- | --- | --- | --- | --- | --- | --- |
| TC-2060 | FR-2060 | unit | Various prompt strings (escaped, slash commands with arguments, paths, plain text) | Call `classify` on each input | Matches expected `InputKind` variant (`//x` -> `Escaped("/x")`, `/agent codex` -> `Slash { cmd: "/agent", rest: "codex" }`, `/plan   do x` -> `Slash { cmd: "/plan", rest: "do x" }`, `/tmp/file.png` -> `Plain`, `hello` -> `Plain`) | `src/input_command.rs` `tc_2060_classify_escaped_slash_plain` |
| TC-2061 | FR-2061 | unit | Inputs with leading whitespace, escapes, paths, and commands | Call `is_slash_command` on each input | Returns `true` for valid commands regardless of leading whitespace; returns `false` for escaped slashes, paths, and plain text | `src/input_command.rs` `tc_2061_is_slash_command_trim_start` |
| TC-2062 | FR-2062 | unit | Test `App` with pre-filled `app.input = "draft text in tui"` | Call `headless_submit_prompt` with a slash command request (`/agent cursor`) | Slash command executes successfully and `app.input` remains equal to `"draft text in tui"` | `src/app.rs` `tc_2062_headless_slash_does_not_touch_tui_input` |

## 6. Data, integration, and safety contracts

| Area | Contract / change | Compatibility and rollback | Evidence |
| --- | --- | --- | --- |
| Classification | `InputKind` classifies `Escaped`, `Slash`, `Plain` without mutating input text | Fully backward compatible with legacy TUI `submit_prompt` parsing logic | TC-2060, TC-2061 |
| Headless Seam | Headless slash submission dispatches directly via `submit_text` without borrowing `app.input` | Eliminates race and draft loss when headless commands run concurrently with interactive drafting | TC-2062 |
| Dashboard Parity | Dashboard slash detection preserved as-is pending unified structured intent | Documented follow-up; existing dashboard tests pass unchanged | Existing test suite |

## 7. Implementation plan

| Order | Change | Files / modules | Requirement IDs | Verification |
| --- | --- | --- | --- | --- |
| 1 | Create `src/input_command.rs` module with `InputKind`, `classify`, `is_slash_command`, `first_token_is_path`, and unit tests | `src/input_command.rs`, `src/main.rs` | FR-2060, FR-2061 | `tc_2060`, `tc_2061` |
| 2 | Refactor `submit_prompt` into `submit_precheck` and `submit_text` in `src/app.rs` using `InputKind` | `src/app.rs` | FR-2060, FR-2062 | `cargo test --bin dmuxai` |
| 3 | Update `headless_submit_prompt` to use `is_slash_command` and `submit_text` without touching `self.input` | `src/app.rs` | FR-2062 | `tc_2062` |
| 4 | Create specification document | `docs/specs/SPEC-input-command.md` | FR-2060, FR-2061, FR-2062 | `python3 scripts/check-spec.py --all`, `python3 scripts/check-tc-links.py` |

## 8. Review and test exit criteria

- [x] All Must requirements (FR-2060..FR-2062) map to passing test evidence (TC-2060..TC-2062).
- [x] The diff has been reviewed against scope and non-goals.
- [x] Required Rust checks pass: format, test, and clippy with `-D warnings`.
- [x] `scripts/check-spec.py` passes and `tests/spec_index.rs` verifies spec indexing.
