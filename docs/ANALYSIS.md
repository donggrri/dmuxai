# DmuxAI 분석: Windows+WSL 앱, 권한/편집 차단, `.agents/skills`

**범위:** `main` 워크트리 코드 읽기 전용 분석 (본 문서·그래프·agent-graph-engineering 설치 파일만 변경).  
**작성:** Cloud Agent 분석 턴 (2026-10-16).  

### Phase 1 (permissions + mission) 상태 — 2026-10-16

| 항목 | 상태 |
| --- | --- |
| `MISSION.md` + composer-2.5-only 정책 | **완료** (PR) |
| Cursor Windows Implement opt-in (`dmuxai.toml` / env) | **완료** (PR) |
| Implement 턴 실패 기록 (권한/무변경) | **부분 완료** (adapter 스트림 검증) |
| 이미지 직접 첨부 / auto-routing / WSL 런처 | **보류** (외부 그래프 오케스트레이터 디스패치 대기) |

**관련 그래프:** [`docs/dmuxai-graph.yaml`](dmuxai-graph.yaml)

## agent-graph-engineering 설치 상태

| 단계 | 결과 |
| --- | --- |
| `git clone https://github.com/donggrri/cursor-agent-graph-engineering` | **실패** — `Repository not found` (비공개 저장소, 현 환경 토큰으로 접근 불가) |
| `gh api repos/donggrri/cursor-agent-graph-engineering` | **404** |
| 대체 조치 | 분석 후 공식 `scripts/install.sh`로 교체 설치 완료 (donggrri/cursor-agent-graph-engineering main). |

설치된 파일:

- `.agents/skills/agent-graph-engineering/SKILL.md`, `templates/graph.yaml`, `templates/edge-contract.md`
- `.cursor/agents/graph-*.md` (composer-2.5 only per repository model policy)
- `.cursor/rules/agent-graph-engineering.mdc`

공식 `install.sh` 실행 후 위 파일을 upstream과 diff하여 정합성을 맞출 것을 권장합니다.

---

## 1. 아키텍처 맵

### 1.1 목표 vs 현재 `main`

소유자 비전은 **tmux + git worktree 멀티에이전트 멀티플렉서**이나, 현재 `main`의 구현은 **단일 Ratatui TUI**에서 Codex/Cursor/AGY 헤드리스 CLI를 오케스트레이션합니다. tmux·worktree·pane 스킬 런처는 **아직 코드에 없음** (연구 문서 `docs/conversation-continuity-research.md`에 worktree 모델만 논의).

### 1.2 진입점

| 구성요소 | 경로 | 역할 |
| --- | --- | --- |
| 바이너리 진입 | `src/main.rs` | `current_dir()` → workspace, SQLite DB 경로, `App::run` |
| 라이브러리 | `src/lib.rs` | `conversation`, `chat_selection`, `clipboard` |
| TUI/라우팅 | `src/app.rs` | 키보드, 슬래시 명령, 에이전트/모드/모델 피커, shutdown |
| 영속 대화 | `src/conversation.rs` | SQLite WAL, runtime session, provider binding, context projection |
| 프로세스 경계 | `src/process/mod.rs` | CLI 경로 해석, Windows Cursor `node.exe` 직접 실행, `CREATE_NO_WINDOW` |

### 1.3 에이전트 기동 흐름

```text
사용자 Enter
  → App (turn 생성, busy_agents)
  → AgentAdapter::send (mpsc try_send, capacity 8)
  → adapter runtime_loop (Codex 장기 프로세스 / Cursor·AGY 턴당 프로세스)
  → headless_command + current_dir(workspace_root)
  → stdin에 prompt (+ 모드별 prefix)
  → NDJSON/JSONL → AgentEvent 정규화
  → SQLite + 채팅 UI (25ms coalesce)
```

| Agent | 실행체 | Implement 시 권한 관련 플래그 |
| --- | --- | --- |
| **Codex** | `codex app-server --stdio` | `approvalPolicy: "never"`, `sandboxPolicy.type`: `readOnly` / `workspaceWrite` (`src/agent/codex.rs` ~397–433) |
| **Cursor** | `cursor-agent` (Win: `%LOCALAPPDATA%\cursor-agent\versions\*\node.exe index.js`) | Ask/Plan: `--mode ask|plan`; Implement: `--force --sandbox enabled` + `--trust --workspace <cwd>` (`src/agent/cursor.rs` ~291–318) |
| **AGY** | `agy` | Ask/Plan: `--mode plan --sandbox` + 프롬프트에 수정 금지; Implement: `--mode accept-edits --sandbox` (`src/agent/agy.rs` ~282–312) |

기본 모드는 전 에이전트 **Ask** (`ExecutionMode::default`, `src/agent/mod.rs`).

### 1.4 설정·훅·빌드

- **환경 오버라이드:** `DMUXAI_CODEX_BIN`, `DMUXAI_CURSOR_BIN`, `DMUXAI_AGY_BIN` (`README.md`, `src/process/mod.rs`)
- **DB:** Windows `%LOCALAPPDATA%\DmuxAI\conversations.sqlite3`; Linux `XDG_STATE_HOME` 또는 `~/.local/state/dmuxai/` (`src/main.rs` ~33–62)
- **Workspace 키:** `normalize_workspace` → `canonicalize` (`src/conversation.rs` ~1729–1737)
- **설치:** `install.ps1` / `install.cmd` — 릴리스 `dmuxai.exe`만 (에이전트 CLI는 별도)
- **CI:** `.github/workflows/verify.yml` — spec check + `fmt` / `test` / `clippy` on ubuntu-latest, plus clippy for the `x86_64-pc-windows-gnu` target (since v0.2.2; Windows release builds stay on windows-latest in `release.yml`)
- **릴리스:** `.github/workflows/release.yml` — tag `v*` 시 Windows zip
- **훅:** Cursor/에이전트 훅 통합 없음; DmuxAI는 자체 슬래시 명령만 (`/agent`, `/mode`, …)

### 1.5 테스트

- 단위/통합: `src/agent/*.rs`, `src/app.rs` 내 대량 `#[cfg(test)]`
- 통합: `tests/conversation.rs` (clippy 시 unused import 이슈 관측)
- 라이브 스모크: `real_*` 테스트 `#[ignore]` (인증된 CLI 필요)

---

## 2. 편집/권한 차단 — 원인 후보와 수정 방향

우선순위는 **Windows에서 “코드가 안 바뀐다”** 체감과 정합되게 정렬했습니다.

### 2.1 [주원인] Windows 네이티브에서 Cursor Implement 전면 거부

| 항목 | 내용 |
| --- | --- |
| **증상** | Cursor + Implement 선택 시 프로세스 없이 즉시 오류 |
| **근거** | `validate_cursor_mode`: `cfg!(windows)` && Implement → `UnsupportedMode` (`src/agent/cursor.rs` **381–388**, **106–114**, **257**) |
| **UI** | `validate_mode` 선행 (`src/app.rs` **990**, **1296**); 메시지 (`src/agent/mod.rs` **210–217**) |
| **스펙** | `docs/specs/SPEC-cursor-windows-implement-guard.md`, baseline `INT-002`, `NFR-060` |
| **이유** | Cursor CLI의 `--sandbox enabled`가 Windows에서 미지원; DmuxAI는 **비샌드박스 폴백 금지** (`README.md` **191–196**) |
| **수정 옵션** | (A) **WSL/Linux에서 dmuxai 실행** — Cursor Implement 사용 가능. (B) Windows에서는 **Codex/AGY Implement** 사용. (C) Cursor가 Win 샌드박스를 제공할 때까지 가드 유지 (별도 스펙). |

이 동작은 “권한 버그”가 아니라 **의도된 안전 가드**이나, 사용자는 Implement에서만 편집을 기대하므로 **편집 실패의 1순위 설명**입니다.

### 2.2 [설계] Ask/Plan 모드는 의도적으로 쓰기 불가

| Agent | 모드 | 편집 |
| --- | --- | --- |
| Codex | Ask/Plan | `sandboxPolicy.type: readOnly` (`codex.rs` **428–432**) |
| Cursor | Ask/Plan | `--mode ask` / `plan` only (`cursor.rs` **299–305**) |
| AGY | Ask/Plan | `plan --sandbox` + 프롬프트에 “Do not modify files” (`agy.rs` **284–288**, **307–308**) |

**수정:** UX — Windows에서 Implement 불가 시 상태줄/모드 피커에 **대체 에이전트 안내** (이미 부분적으로 README·시스템 메시지에 존재).

### 2.3 [Codex] 승인 대화 없음 (`approvalPolicy: never`)

Implement에서도 사용자 승인 프롬프트는 없고, 샌드박스가 `workspaceWrite`로 제한 (`codex.rs` **400–401**, **427–432**).  
**실패 패턴:** 샌드박스/정책 거부는 CLI/계정 쪽 오류로 채팅에 표시 — DmuxAI가 `danger-full-access`로 풀지 않음 (baseline `INV-005`).

**수정:** 운영 문서화 + 필요 시 **명시적 opt-in** 스펙 (non-goal 위반 주의).

### 2.4 [AGY] plan+sandbox에서 “플랜만” 반환

`is_plan_review_response` 시 Notice로 파일 변경 없음 안내 (`agy.rs` **430–434**, **453–460**). Implement가 아닌데 편집을 기대하면 “막힌 것”처럼 보일 수 있음.

### 2.5 [WSL/Windows 경로·소유권] (앱화 시 핵심)

현재 dmuxai는 **실행 OS의 `current_dir`를 canonical workspace**로 저장 (`conversation.rs` **1729–1737**).  
Windows에서 `dmuxai` 실행 시 `C:\...` 키; WSL에서 `/home/...` 키 — **동일 프로젝트가 다른 workspace로 분리**.

| 시나리오 | 위험 |
| --- | --- |
| 에이전트는 WSL, 파일은 `/mnt/c/...` | drvfs **chmod/소유권**, 느린 I/O, Git worktree 메타데이터 꼬임 |
| Windows Cursor CLI가 WSL 경로를 workspace로 | CLI가 경로를 거부하거나 샌드박스 밖으로 인식 |
| Linux 에이전트가 Windows checkout 편집 | 샌드박스 workspace 루트 불일치 → 쓰기 거부 |

**수정:** WSL 앱은 **단일 Linux canonical root** (`\\wsl$\...` 역변환 금지, 항상 `wslpath`/`/home/...` 클론) + `DMUXAI_WORKSPACE` 명시; worktree는 Linux 쪽에서만 생성.

### 2.6 [프로세스] Windows Cursor 직접 기동

`cursor_command()`가 셸 없이 `node.exe` + `index.js` (`process/mod.rs` **56–77**). 인용/권한 문제보다 **취소·신뢰** 측면 이점. `--trust`는 workspace 신뢰 (`cursor.rs` **315**)이나 **Implement 샌드박스 대체 아님**.

### 2.7 [미구현] tmux pane / skill 하위 프로세스

스킬·멀티 pane 런처 없음 → “스킬로 편집” 요구는 **Cursor rules/skills를 CLI가 자동 로드하는지**에 달림; DmuxAI는 `.agents/skills`를 **탐색·주입하지 않음**.

---

## 3. Windows+WSL 앱 갭 및 옵션 비교

### 3.1 갭 요약

| 영역 | 현재 | 앱 목표 |
| --- | --- | --- |
| UI | Ratatui 전체화면 TUI | Windows에서 “앱” 실행 |
| 멀티플렉싱 | 단일 TUI, 에이전트 전환 | tmux + worktree pane (비전) |
| Cursor Implement | Win 네이티브 불가 | WSL 내 Linux dmuxai로 해결 가능 |
| Skills | 없음 | `.agents/skills/*/SKILL.md` |
| 인증 | 사용자가 CLI별 `login` | WSL 내 동일 Linux 홈에 토큰 |

### 3.2 옵션

| 옵션 | 설명 | 장점 | 단점 |
| --- | --- | --- | --- |
| **A. Tauri/Electron + `wsl.exe -d <distro> -- dmuxai`** | Win UI는 런처·설정·로그; 실제 dmuxai/tmux은 WSL | Cursor Implement·Linux 샌드박스 사용; 기존 Rust 코드 재사용 | tmux GUI 부착 어려움 → **Windows Terminal** 또는 **wezterm** 연동 필요 |
| **B. Windows Terminal 프로필만** | `wsl -d Ubuntu --cd $(wslpath)` + `dmuxai` | 구현 최소 | “앱” 느낌 약함, 설치·업데이트 수동 |
| **C. WSL 내 웹 UI** | dmuxai가 HTTP+WS 서버 (신규) | Win 브라우저 접근 | 신규 대형 작업, 보안·세션 모델 필요 |
| **D. Win 네이티브 dmuxai 유지** | 현재 릴리스 | 설치 단순 | **Cursor Implement 영구 불가** (가드 유지 시) |

### 3.3 권장

**단기: A + Windows Terminal (또는 Wezterm) 임베디드 터미널**

1. Tauri 셸: distro 검출, `wsl.exe` 존재 확인, 프로젝트 경로를 **`wslpath -u`** 로 변환 후 WSL에서 `dmuxai`/`tmux` 실행.  
2. **에이전트 CLI·`cursor-agent login`·Codex 인증은 전부 WSL Linux 홈**에서 수행.  
3. 저장소는 가능하면 `~/projects/...` (ext4). `/mnt/c` 편집은 피하거나 “고급”으로만.  
4. **tmux**: WSL 패키지 `tmux`; 앱은 `wt.exe` / `cmd /c start wt` 로 세션 attach.  
5. **경로**: 런처가 `\\wsl$\Distro\...` ↔ `/home/...` 변환 단일 소유; dmuxai에 `DMUXAI_WORKSPACE` 전달 (신규 PR).

**중기:** Ratatui와 tmux 멀티플렉서를 통합할지, tmux는 “pane per agent”만 담당하고 dmuxai는 pane 내 프로세스로 둘지 **별도 스펙** 필요 (현 `main`과 비전 갭).

---

## 4. `.agents/skills` 지원 설계

### 4.1 디스커버리

```text
workspace_root/
  .agents/skills/<skill-id>/SKILL.md   # 필수
  .agents/skills/<skill-id>/...        # optional scripts, templates
```

- **스캔 시점:** `App::new` 또는 `/skills` 명령 시 (캐시 + mtime).
- **파싱:** YAML frontmatter `name`, `description` (Cursor skill 규약과 동일).
- **검증:** `SKILL.md` 없는 디렉터리는 스킵; id = 디렉터리명.

### 4.2 UI

- 슬래시 **`/skills`** 또는 `F4`: 스킬 목록 (이름, 설명, id).
- 선택 시: **새 turn** 프롬프트에 스킬 본문 일부를 주입하거나, “이 스킬을 따르라” 시스템 preamble 추가.
- **모델:** 피커에서 스킬별 default model override (graph 정책과 일치: implement → `composer-2.5`, 등).

### 4.3 pane/에이전트 태스크로 실행 (비전: tmux)

| 단계 | 동작 |
| --- | --- |
| 1 | 사용자가 skill + model + mode(Implement 등) 선택 |
| 2 | (tmux 도입 후) `git worktree add` + 새 pane |
| 3 | pane에서 `dmuxai` 또는 `cursor-agent --print ...` with skill context env `DMUXAI_SKILL_ID` |
| 4 | DmuxAI conversation에 `skill_id`, `worktree_path` 메타데이터 저장 (스키마 확장) |

**당장 `main`에서:** tmux 없이 **단일 TUI turn**에 skill preamble + 선택 모델로 adapter `send` — 최소 기능 MVP.

### 4.4 agent-graph-engineering 스킬

이 PR에 설치한 스킬은 분석·그래프 작성용. 제품 스킬 루트는 **`.agents/skills/`** 로 통일 (`.cursor/skills`와 중복 시 `.agents` 우선 정책 문서화).

---

## 5. 우선순위 개선 목록 및 PR 단위 계획

### 5.1 우선순위 (P0 → P2)

| P | 항목 | 이유 |
| --- | --- | --- |
| P0 | WSL 런처 + Linux workspace canonical | Cursor Implement·샌드박스·경로 소유권 |
| P0 | Windows UX: Implement 불가 시 대체 경로 명시 | 2.1 사용자 혼란 제거 |
| P1 | `.agents/skills` 스캔 + `/skills` UI | 소유자 요구 핵심 |
| P1 | `DMUXAI_WORKSPACE` / WSL path helper | DB workspace 키 안정화 |
| P2 | tmux + worktree pane 오케스트레이션 | 비전과 `main` 갭 해소 |
| P2 | 스킬 → subgraph (graph.yaml) 실행 추적 | agent-graph-engineering 운영화 |
| P2 | CI `cargo fmt` trailing newline 수정 | Verify 녹색화 |

### 5.2 PR 크기별 페이즈 (노드 / 역할 / 모델)

| PR | 내용 | Graph node | 역할 | 모델 |
| --- | --- | --- | --- | --- |
| PR-1 | `docs/spec` + WSL 런처 스파이크 (Tauri `wsl.exe` 호출, 경로 변환) | `plan-wsl-app` | orchestrate | composer-2.5 |
| PR-2 | `DMUXAI_WORKSPACE`, `normalize_workspace` WSL 테스트 | `research-permissions` | research | composer-2.5 |
| PR-3 | Win UI: Cursor Implement 가드 메시지·대체 에이전트 CTA | `review-report` | review | composer-2.5 |
| PR-4 | `.agents/skills` 디스커버리 라이브러리 + 단위 테스트 | `plan-skills` | research | composer-2.5 |
| PR-5 | `/skills` TUI + skill preamble 주입 | `synthesize-report` | implement | composer-2.5 |
| PR-6 | 스킬 메타 SQLite + conversation 연결 | `implement` | implement | composer-2.5 |
| PR-7 | tmux 세션 매니저 (WSL only) POC | `plan-wsl-app` | orchestrate | composer-2.5 |
| PR-8 | worktree-per-skill pane + graph.yaml run ledger | `orchestrate` | orchestrate | composer-2.5 |
| PR-9 | CI fmt/clippy/test 정리 | `research-architecture` | test | composer-2.5 |

의존 순서: PR-1/2 → PR-3; PR-4 → PR-5 → PR-6; PR-7/8는 PR-1 이후; PR-9는 병행 가능.

---

## 6. 테스트·CI 관측 (이 환경)

| 명령 | 결과 |
| --- | --- |
| `cargo test --all-targets` (rustc **1.99.1**, Linux) | **86 passed, 1 failed**, 5 ignored — 실패: `app::tests::cancelled_and_interrupted_turn_notices_survive_reload` (conversation id 불일치, flakiness 의심) |
| `cargo fmt --check` | **실패** — `src/app.rs` 끝부분 trailing newline diff |
| `cargo clippy ... -D warnings` | **실패** — `tests/conversation.rs` unused imports |
| GitHub Actions `Verify` on `main` (최근 run `32732922263`, 2026-08-24) | **failure** — `cargo fmt --check` 동일 diff |

Spec job은 해당 run에서 Rust 실패 전 단계; 최근 연속 push에서 Verify 실패 패턴 동일.

---

## 7. 결론

- **편집이 “막힌” 가장 흔한 원인(Windows):** Cursor **Implement 자체가 플랫폼 가드로 차단**되며, DmuxAI는 의도적으로 비샌드박스 Cursor를 쓰지 않습니다.  
- **그 외:** Ask/Plan 모드, AGY plan+sandbox, Codex readOnly, WSL/Windows workspace 경로 불일치·`/mnt/c` 권한.  
- **앱 방향:** Win 셸 + **WSL 내부 dmuxai** + Linux workspace + (후속) tmux/skills.  
- **agent-graph-engineering:** 공식 `install.sh`로 설치됨 (클라우드 VM에서는 clone 권한이 없어 처음엔 수동 배치했음).
