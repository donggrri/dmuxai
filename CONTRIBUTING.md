# Contributing to DmuxAI

Thank you for helping improve DmuxAI. This project is a **Rust + Ratatui** orchestrator for
headless coding-agent CLIs with durable conversations, a local web dashboard, and graph-first
delivery. Product direction and model policy live in [MISSION.md](MISSION.md); operator setup
and architecture notes are in [README.md](README.md).

## Before you start

1. Read [docs/WORKFLOW.md](docs/WORKFLOW.md) and the shipped contract in
   [docs/specs/DMUXAI-BASELINE.md](docs/specs/DMUXAI-BASELINE.md).
2. For behavior changes, follow the spec-first flow documented in [AGENTS.md](AGENTS.md)
   (also summarized below).
3. Search existing issues and pull requests so work is not duplicated.

## Spec-first delivery

Every **behavior change** uses this sequence:

```text
Spec -> Plan -> Execute -> Review -> Test
```

Create these artifacts under `docs/` with an ASCII kebab-case `<slug>`:

| Artifact | Path |
| --- | --- |
| Requirements | `docs/specs/SPEC-<slug>.md` (start from [docs/templates/FEATURE-SPEC.md](docs/templates/FEATURE-SPEC.md)) |
| Implementation plan | `docs/plans/PLAN-<slug>.md` |
| Verifiable tasks | `docs/tasks/TASKS-<slug>.md` |
| Compliance review | `docs/reviews/REVIEW-<slug>.md` |

Documentation-only edits may use a short plan and review in the pull request, but links must
render and traceability must stay clear.

Do **not** start implementation until acceptance criteria in the spec are unambiguous. If a
requirement is unclear, record it as an open question instead of inventing behavior.

### Spec quality

- Give observable requirements stable IDs: `FR-*`, `BR-*`, `NFR-*`, `UX-*`, or `INT-*`.
- Use Given / When / Then scenarios with actor, preconditions, trigger, expected result, and
  persistence or error behavior.
- Separate shipped behavior from proposed behavior; list **non-goals** explicitly.
- Map every requirement to one or more test IDs (or document a justified manual check).

### Agent and provider boundaries

Preserve DmuxAI’s separation of concerns: UI, conversation persistence, adapter protocol, and
process launching stay independent. Keep external provider smoke tests `#[ignore]`; deterministic
parsing and state tests must not require credentials or a live provider.

## Local build and test gates

Run the same Rust gates as CI before opening a pull request (from the repository root):

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
```

Windows cross-target lint (matches the Verify workflow on Linux):

```bash
rustup target add x86_64-pc-windows-gnu
sudo apt-get update && sudo apt-get install -y gcc-mingw-w64-x86-64   # Debian/Ubuntu
cargo clippy --locked --all-targets --all-features --target x86_64-pc-windows-gnu -- -D warnings
```

When you change `docs/specs/SPEC-*.md` or the baseline spec, validate traceability:

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/check-spec.ps1 -SpecPath <path-to-spec>
```

Optional helper: `scripts/spec-index.sh` lists spec files.

Real-provider smoke tests stay ignored in the default suite; run them only with installed,
authenticated CLIs when your change requires live verification (see [README.md](README.md)
**Verify** section).

## Branches, commits, and pull requests

- **Do not commit directly to `main`.** Branch from the latest `main`, then open a pull request.
- Use a short, descriptive branch name (for example `feat/web-todo-board` or
  `fix/cursor-segment-dup`).
- Keep pull requests **small and focused** on one spec slug or one logical fix.
- Link the relevant `docs/specs/SPEC-<slug>.md` and any GitHub issue in the pull request
  description.
- **CI must pass** on the pull request before merge. The Verify workflow runs spec
  traceability, Rust quality gates (for non–docs-only changes), and secret scanning.
- Maintainers use **squash merge** to land changes on `main`.

Fill out the pull request template and, for product changes that use the graph workflow,
include an honest graph execution report as described in [MISSION.md](MISSION.md).

## Releases

Releases follow [docs/RELEASE-PROCESS.md](docs/RELEASE-PROCESS.md), including the
[manual TC template](docs/RELEASE-PROCESS.md#manual-tc-template) for test cases without automation.

## Security

Report vulnerabilities through GitHub **private vulnerability reporting** (repository
**Security** tab → **Report a vulnerability**). See [SECURITY.md](SECURITY.md).

## Code of conduct

Be respectful in issues and reviews. Disagree on technical merits; keep discussion actionable
and tied to specs and tests.
