# DmuxAI

One terminal. Multiple AI coding agents.

This repository currently implements the Phase 1 local prototype. It is a
Ratatui application with Codex, Cursor, and AGY selection, prompt editing,
message history, deterministic fake replies, and panic/error-resistant terminal
cleanup. It does **not** start real agent processes yet.

## Run

```powershell
cargo run
```

Controls:

- `Up` / `Down`: select an agent
- Type or paste: edit the prompt
- `Backspace`: delete the previous character
- `Enter`: submit
- `Esc`: exit
- `/error`: simulate an adapter failure and verify visible error handling

Verify the non-interactive parts with:

```powershell
cargo fmt --check
cargo test
cargo clippy --all-targets --all-features -- -D warnings
```

## Code map and Rust-to-C++ guide

- `src/main.rs` owns terminal setup and cleanup. `TerminalSession` is an RAII
  guard, like a small C++ class whose constructor acquires a resource and whose
  destructor releases it. Rust calls `Drop::drop` on normal and error returns.
- `src/app.rs` owns UI state and the event loop. `&self` resembles a `const`
  member function; `&mut self` is exclusive mutable access enforced by the
  compiler.
- `src/agent/mod.rs` is the provider boundary. `AgentAdapter` is similar to a
  C++ abstract base class. `Box<dyn AgentAdapter>` is type-erased dynamic
  dispatch, roughly analogous to owning a `std::unique_ptr<AgentAdapter>`.
- `AgentEvent` and `Role` are tagged unions. `match` is an exhaustive visitor:
  adding a variant forces relevant consumers to handle it.
- `Vec<Message>` and `String` own their contents. `mem::take(&mut input)` moves
  the current input into history and replaces it with an empty string, avoiding
  an unnecessary copy.
- Adapter output is provider-neutral. The fake adapter returns `AgentEvent`s;
  the UI never manufactures a fake response or renders provider-specific data.

The next architectural step is Phase 3: introduce Tokio, a harmless child
process adapter, and a bounded channel. Real Codex protocol work should follow
only after that asynchronous process boundary is proven.
