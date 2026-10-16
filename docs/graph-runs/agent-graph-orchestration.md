# Graph execution report: agent-graph orchestration (default)

**날짜:** 2026-10-16  
**브랜치:** `cursor/agent-graph-orchestration-c906`  
**베이스:** `cursor/image-attachment-c906`

## 요약

- 사용자 프롬프트마다 `plan_request`가 agent-graph-engineering 형태의 그래프를 생성하거나 1–2단계 작업은 단일 에이전트로 단축합니다.
- 오케스트레이터 모델: `grok-4.7-medium`. 역할별 모델: research/explore/test → `claude-haiku-5-5-high`, implement/draft → `composer-2.5`, plan/route/review → `grok-4.7-medium`. `fast` 포함 모델 ID 거부.
- UI 상태줄에 Graph 할당 표시, `/graph`, `/graph continue`, `/graph override <node> <runner> <model>` 지원.
- merge/DB/auth 민감 단계는 게이트에서 일시 정지.
- 병렬 노드 기본 2, 재시도 기본 2 (`dmuxai.toml` `[graph]`로 조정).
- `.dmuxai/graphs/<id>.json`에 실행 상태 저장.
- `.agents/skills/*/SKILL.md` 탐색 후 노드 프롬프트에 skill 프리픽스 주입.

## 디스패치

| 단계 | 담당 | 모델 | 비고 |
| --- | --- | --- | --- |
| 그래프 계획/정책 | in-repo `src/graph/*` | — | 결정론적 플래너 (CLI 오케스트레이터 턴은 후속) |
| TUI 통합 | `app.rs` | — | 제출·게이트·오버라이드 |
| 스킬 탐색 | `src/skills.rs` | — | `agent-graph-engineering` 등 |

## 검증

```bash
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
```
