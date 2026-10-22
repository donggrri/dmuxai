# Mutation Testing 도입 계획

## 목적

DmuxAI의 Mutation Testing은 단순한 코드 커버리지 수치가 아니라, 테스트가 실제 동작의 변화를 감지하는지 검증하는 데 사용한다.

기본 도구는 Rust의 `cargo-mutants`를 사용한다. 모든 소스 파일을 처음부터 일괄 검사하지 않고, 결정론적인 핵심 로직부터 적용 범위를 넓힌다.

## 적용 원칙

- 테스트는 가능한 한 모듈의 공개 인터페이스를 통해 동작을 검증한다.
- private 함수의 구현 형태보다 상태 전이, 반환값, 오류, 저장 결과 같은 관찰 가능한 동작을 검증한다.
- 실제 Agent CLI, 네트워크, 인증 상태에 의존하는 smoke test는 일반 mutation 실행에 포함하지 않는다.
- equivalent mutant나 가치가 낮은 mutant는 무조건 허용하지 않는다. 제외가 필요하면 대상과 이유를 좁게 기록한다.
- 새로 변경된 코드에서 살아남은 mutant는 병합 전에 테스트 추가 또는 명시적인 제외 결정을 요구한다.

## 대상 계층

| 영역 | 적용 단계 | 검증할 동작 |
| --- | --- | --- |
| `src/conversation.rs` | 1차 | SQLite 상태 전이, 트랜잭션 원자성, summary, provider binding |
| `src/chat_selection.rs` | 1차 | Unicode grapheme, cell 범위, 역방향 선택, 경계값 |
| Agent 프로토콜 변환 | 2차 | 요청 생성, JSON/NDJSON 파싱, provider event 정규화 |
| App 상태 전이 | 2차 | 입력 이벤트에 따른 상태 변화와 effect 생성 |
| 프로세스 실행 | 기본 제외 | 실행 파일 탐색과 child lifecycle은 contract/smoke test로 검증 |
| 터미널 및 클립보드 | 기본 제외 | OS adapter는 작은 contract test와 수동 smoke test로 검증 |
| 실제 Agent 연동 | 제외 유지 | 인증된 CLI가 필요한 `ignored` smoke test로 별도 실행 |

## 목표 테스트 구조

```text
.cargo/
  mutants.toml              # 빠르고 결정론적인 기본 대상
  mutants-full.toml         # 정기 전체 검사 설정

tests/
  MUTATION_TESTING.md       # 이 계획과 운영 정책
  conversation.rs           # 대화 엔진 인터페이스 테스트
  app_state.rs              # 상태 모듈 분리 후 추가
  agent_protocol.rs         # 프로토콜 공통 contract
  support/
    mod.rs                  # DB harness와 fixture loader
  fixtures/
    codex/*.json
    cursor/*.jsonl
    agy/*.jsonl

.github/workflows/
  mutation.yml              # PR 변경분 검사와 정기 전체 검사
```

실제 파일은 필요한 시점에만 추가한다. 구조를 미리 채우기 위한 빈 파일이나 가상의 seam은 만들지 않는다.

## 선행 구조 개선

`conversation`은 `src/lib.rs`가 노출하는 실제 crate 모듈을 테스트 인터페이스로 사용해야 한다. `tests/conversation.rs`에서 `#[path = "../src/conversation.rs"]`로 구현 파일을 직접 포함하는 방식은 제거하고 다음 형태로 전환한다.

```rust
use dmuxai::conversation::{
    AgentKey, BeginTurn, BeginTurnOutcome, ConversationEngine, ConversationSelection,
};
```

Agent adapter는 다음 두 책임을 가능한 한 분리한다.

1. 프로세스 시작, 취소, 채널 관리처럼 I/O를 수행하는 runtime adapter
2. provider 요청과 응답을 변환하는 결정론적인 protocol module

App도 mutation 결과가 지나치게 느리거나 불안정하면, 입력을 받아 상태와 effect를 반환하는 순수 상태 모듈과 effect를 실행하는 runtime을 분리한다. Mutation Testing만을 위해 production interface를 넓히지는 않는다.

## 1차 설정안

기본 `.cargo/mutants.toml`은 처음에는 다음 범위로 제한한다.

```toml
test_tool = "cargo"

examine_globs = [
    "src/conversation.rs",
    "src/chat_selection.rs",
]

timeout_multiplier = 5.0
minimum_test_timeout = 20.0
```

baseline 결과를 확인하기 전에는 광범위한 `exclude_re`를 추가하지 않는다. 반복적으로 hang을 유발하거나 관찰 가능한 동작이 없는 함수만 정확한 이름으로 제외한다.

## 대화 엔진 우선 시나리오

다음 시나리오를 첫 mutation 기준으로 삼는다.

- summary가 허용된 최대 글자 수와 같으면 저장된다.
- summary가 최대 글자 수보다 한 글자 크면 `SummaryTooLarge`로 실패한다.
- summary 크기는 UTF-8 byte 수가 아니라 Unicode character 수를 기준으로 계산한다.
- 활성 대화가 없으면 `prepare_summary`가 실패한다.
- 선택한 agent가 busy이면 `prepare_summary`가 실패한다.
- 완료된 turn이 없으면 summary job이 생성되지 않는다.
- 최신 checkpoint가 canonical head를 이미 포함하면 중복 summary job이 생성되지 않는다.
- summary commit 성공 후 해당 agent의 provider binding만 stale 상태가 된다.
- stale source hash로 commit하면 checkpoint와 binding이 모두 변경되지 않는다.
- oversized summary로 commit하면 checkpoint와 binding이 모두 변경되지 않는다.
- 여러 output을 기록하다 하나가 잘못되면 전체 batch가 rollback된다.
- conversation과 agent별 checkpoint 및 binding이 서로 격리된다.
- migration 실패 시 schema가 부분 적용되지 않는다.

각 경계 조건은 성공 케이스와 실패 케이스를 쌍으로 둔다. `>`를 `>=`로 바꾸거나 빈 컬렉션 처리, 상태 비교, transaction commit 위치를 변경하는 mutant가 살아남지 않아야 한다.

## Agent 프로토콜 시나리오

2차 단계에서는 실제 CLI를 실행하지 않고 저장된 fixture를 사용한다.

- 정상적인 시작, text delta, 완료 이벤트
- provider session/conversation/thread 식별자 추출
- 오류와 취소 이벤트 정규화
- 알 수 없는 필드가 추가된 forward-compatible 응답
- 필수 필드 누락 및 잘못된 타입
- 여러 delta의 순서 보존
- 실행 모드와 선택 모델에 따른 요청 인자 차이

fixture 테스트는 결과 이벤트 전체와 parser의 최종 상태를 함께 검증한다. 문자열 일부 포함 여부만 검사하여 잘못된 상태 전이가 살아남지 않도록 한다.

## 실행 단계

### 로컬 빠른 검사

```bash
cargo test --all-targets
cargo mutants
```

### 특정 파일 조사

```bash
cargo mutants --file src/conversation.rs
```

### PR 변경분 검사

CI에서는 base branch와의 diff를 만들고 변경된 코드의 mutant만 실행한다.

```bash
git diff "origin/${BASE_REF}...HEAD" > git.diff
cargo mutants --in-diff git.diff --in-place
```

### 정기 전체 검사

main branch 또는 정기 schedule에서 전체 설정으로 실행하고 `mutants.out`을 artifact로 보관한다. 실제 provider smoke test는 일반 `cargo test`처럼 계속 ignored 상태로 둔다.

### 릴리즈 필수 검사

모든 릴리즈 후보는 tag 생성, 배포 artifact 생성 또는 배포 작업보다 먼저 전체 Mutation Testing을 통과해야 한다. 정기 실행이나 이전 commit의 성공 결과로 대체할 수 없으며, 실제 릴리즈 대상 commit에서 다시 실행한다.

```bash
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
cargo mutants --config .cargo/mutants-full.toml
```

릴리즈 pipeline은 위 검사를 선행 job으로 두고, 다음 조건을 모두 만족한 경우에만 package, tag, publish 또는 deploy job이 실행되도록 구성한다.

- formatting, 일반 테스트와 lint가 모두 성공한다.
- mutation baseline 실행이 성공한다.
- `Missed` mutant가 없다.
- 설명되거나 명시적으로 제외되지 않은 `Timeout`이 없다.
- mutation 결과인 `mutants.out`이 릴리즈 증적으로 보관된다.

검사를 임시로 건너뛰는 release 옵션은 제공하지 않는다. 긴급 릴리즈에서도 제외가 필요하면 해당 mutant의 범위, 이유, 대체 검증과 후속 조치를 repository에 먼저 기록한다.

## 결과 판정

`cargo-mutants` 결과는 다음과 같이 처리한다.

| 결과 | 의미 | 처리 |
| --- | --- | --- |
| Caught | 테스트가 변경을 발견함 | 통과 |
| Missed | 잘못된 동작이 테스트를 통과함 | 동작 테스트 추가 또는 근거 있는 제외 |
| Timeout | mutant가 종료 조건을 손상했을 가능성 | 종료 조건 테스트 추가 또는 좁은 제외 |
| Unviable | 변경된 코드가 컴파일되지 않음 | 점수 계산에서 제외하고 추세만 관찰 |

Mutation score를 기록할 경우 다음 공식을 사용한다.

```text
caught / (caught + missed + timeout)
```

초기 baseline 전에는 임의의 전역 점수 기준을 병합 조건으로 두지 않는다. 대신 PR에서 새로 변경된 코드의 `Missed`와 설명되지 않은 `Timeout`이 0개인지 우선 검사한다. 전체 점수 목표는 baseline을 분류한 뒤 결정한다.

## Survivor 처리 기록

살아남은 mutant는 다음 순서로 판단한다.

1. 요구되는 동작이지만 테스트가 없는가?
2. 테스트가 구현 세부사항만 보고 있어 실제 결과 변화를 놓치는가?
3. 해당 변경이 정말 equivalent mutant인가?
4. OS adapter나 무한 loop처럼 mutation 대상이 부적절한가?

제외가 필요하다면 설정 옆에 다음 내용을 남긴다.

- mutant 또는 함수의 정확한 범위
- 제외 이유
- 대신 수행되는 contract/smoke test
- 다시 검토해야 하는 조건

점수를 높이기 위해 private helper의 현재 구현을 그대로 복제하는 테스트는 작성하지 않는다.

## 도입 순서

1. `tests/conversation.rs`가 실제 crate의 `conversation` 인터페이스를 사용하도록 변경한다.
2. 현재 summary 기능의 경계값과 transaction 원자성 테스트를 완성한다.
3. `.cargo/mutants.toml`을 추가한다.
4. conversation과 chat selection baseline을 실행한다.
5. survivor를 테스트 공백, equivalent, 부적합 대상으로 분류한다.
6. Agent protocol과 runtime adapter를 분리하고 fixture 테스트를 추가한다.
7. App 상태 전이의 mutation 비용을 측정한 뒤 필요한 seam만 분리한다.
8. PR 변경분 검사와 정기 전체 검사 CI를 추가한다.

## 완료 조건

1차 도입은 다음 조건을 모두 만족하면 완료로 본다.

- 일반 테스트 suite가 안정적으로 통과한다.
- conversation과 chat selection의 mutation baseline이 기록되어 있다.
- 현재 summary 기능에서 설명되지 않은 survivor가 없다.
- 모든 제외 항목에 구체적인 이유와 대체 검증 방법이 있다.
- 로컬 실행법과 CI artifact 위치가 문서화되어 있다.
- release pipeline이 전체 Mutation Testing 성공에 의존하며, 실패한 검사를 우회해 배포할 수 없다.
