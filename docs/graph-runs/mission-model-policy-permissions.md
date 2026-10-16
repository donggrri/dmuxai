# Graph execution report — mission / model policy / permissions

**PR 주제:** `mission-model-policy-permissions`  
**일시:** 2026-10-16 (UTC)  
**오케스트레이터:** Dongjin 측 외부 어시스턴트 (본 PR은 Cloud Agent가 in-flight 작업만 마무리)

## 사용한 그래프 (개념)

| Node ID | 역할 | 모델 | 실제 수행자 |
| --- | --- | --- | --- |
| `spec-mission` | 방향 문서화 | composer-2.5 | Cloud Agent (self) |
| `policy-models` | composer-2.5 전역 정책 + 가드 테스트 | composer-2.5 | Cloud Agent (self) |
| `implement-permissions` | Cursor Windows opt-in Implement, implement 턴 실패 기록 | composer-2.5 | Cloud Agent (self) |
| `review-test` | fmt / test / clippy | composer-2.5 | Cloud Agent (self) |

**서브에이전트 위임:** 없음. Task/`graph-*` 서브에이전트를 호출하지 않았고, 모든 노드를 현재 Cloud Agent 세션에서 직접 처리했다.

## 엣지 페이로드 (요약)

- `spec-mission` → `policy-models`: `MISSION.md` 초안, primary CLI = cursor-agent/agy, composer-2.5 only.
- `policy-models` → `implement-permissions`: `.cursor/agents/*.md`, `docs/dmuxai-graph.yaml`, `src/model_policy.rs`, `tests/model_policy_repo.rs`.
- `implement-permissions` → `review-test`: `settings.rs`, `workspace.rs`, `implement_outcome.rs`, adapter/stream 변경 diff.

**재시도 / 폴백:** 컴파일 오류 시 이미지 첨부 WIP는 오케스트레이션 지시에 따라 **롤백**하고 본 PR 범위에서 제외함.

## 타이밍 (대략)

| 단계 | 소요 |
| --- | --- |
| permission / workspace 분석·구현 | ~40분 (누적 세션) |
| MISSION + model policy | ~15분 |
| 이미지 첨부 시도 후 범위 축소 | ~10분 |
| 테스트·PR 정리 | ~15분 |

## 결과

- **완료:** `MISSION.md`, composer-2.5-only 정책(코드·그래프·에이전트 정의·repo 테스트), Cursor Windows `windows_unsandboxed` opt-in, implement 턴 권한/무변경 실패 처리(부분), workspace canonicalize/WSL 경고, 기본 에이전트 순서(Cursor 우선).
- **미완료 (의도적):** 이미지 직접 첨부, auto-routing UI 통합, 스크린샷, 스킬 실행, Windows WSL 런처.
- **블로커:** 없음(로컬에 cursor-agent/agy CLI 없어 라이브 스모크는 기존 `#[ignore]` 유지).
