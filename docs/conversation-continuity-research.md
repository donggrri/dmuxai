# 코딩 에이전트의 대화 연속성 조사와 DmuxAI 적용안

조사 기준일은 2026-08-23이다. 제품 동작은 공식 문서와 공식 소스 코드만 확인했다. 빠르게 변하는 OpenCode, Gemini CLI, Aider의 소스 링크는 조사 당시 커밋에 고정했다.

## 결론

DmuxAI에는 단순한 “에이전트별 세션 ID 저장 + 전환 시 요약 전달”보다 **OpenCode형 애플리케이션 소유 대화 엔진**이 더 잘 맞는다.

OpenCode는 공급자의 네이티브 대화가 기억을 소유하게 두지 않는다. 자체 SQLite에 provider-neutral `session -> message -> part` 원장을 저장하고, 매 요청마다 현재 세션에서 모델에 보여 줄 기록을 구성한다. 따라서 같은 세션 안에서 agent/model/provider를 바꿔도 OpenCode 기록을 현재 공급자 형식으로 다시 재생할 수 있다. 이는 [사용자 메시지마다 agent와 provider/model을 기록하는 스키마](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/schema/src/v1/session.ts#L327-L355), [다른 모델로 재생할 때 provider-specific metadata를 제거하는 변환](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/opencode/src/session/message-v2.ts#L244-L377), [세션 기록을 읽어 모델 입력으로 만드는 실행 루프](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/opencode/src/session/prompt.ts#L1081-L1167)에서 도출되는 결론이다.

DmuxAI도 다음 두 상태를 분리해야 한다.

- **내구성 있는 전체 원장**: 사용자 입력, 모든 에이전트 응답, tool call/result, 파일·diff, 오류·취소, 비용·토큰, fork와 compaction 기록
- **이번 모델에 보낼 유효 컨텍스트**: 최신 summary, 보존할 최근 turn, 그리고 Manual/Autopilot 정책에 따라 명시적으로 공유하거나 자동 배정된 다른 에이전트의 결과

공급자의 Codex thread, Cursor session, AGY conversation은 재개 최적화를 위한 binding이지 DmuxAI 대화의 진실의 원천이 아니다.

## 제품별 비교

| 제품 | 영속 저장과 범위 | 재개 | 분기 | 긴 대화 처리 | DmuxAI가 배울 점 |
|---|---|---|---|---|---|
| OpenCode | XDG data 아래 SQLite, project/workspace/session/message/part | project-scoped session ID 또는 최신 root session | 자체 기록을 새 ID로 복제하고 참조 ID를 remap | 전체 원장은 유지하고 summary + recent tail projection을 모델에 제공 | 가장 적합한 기준 모델: provider-neutral 원장과 모델용 projection 분리 |
| Claude Code | 프로젝트별 JSONL transcript | latest, ID, 이름, PR로 같은 session에 append | 기록을 복사한 새 session ID | `/compact`가 현재 컨텍스트를 summary로 교체 | 이름·picker·worktree 범위, 동일 session 동시 사용 방지 |
| Gemini CLI | project-root hash별 append-only JSONL | latest, index, UUID, searchable picker | named checkpoint/rewind로 branch point 복원 | 검증된 state snapshot + recent tail, 큰 tool output 선제 절단 | crash-safe event log, retention, summary 검증, chat/code rewind 분리 |
| Aider | repo-local Markdown history file | 선택적으로 history file 복원 | first-class session fork 없음 | 오래된 head를 요약하고 최근 tail 유지 | 읽기 쉬운 export는 유용하지만 canonical 원장으로는 부족 |

## OpenCode 심층 분석

OpenCode 기준 소스는 공식 저장소 `dev`의 [`3a31c4e`](https://github.com/anomalyco/opencode/commit/3a31c4ea801915c0b050df4b3842997ea62b6e93)이다.

### 1. 현재 저장 백엔드

OpenCode의 현재 기본 저장소는 SQLite다. DB는 XDG data의 `opencode.db`에 놓이며 `OPENCODE_DB`로 바꿀 수 있다. 연결은 WAL, `synchronous=NORMAL`, 5초 busy timeout, foreign key를 사용한다. [database.ts](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/core/src/database/database.ts#L22-L54)와 [global.ts](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/core/src/global.ts#L10-L29)에 구현되어 있고, 플랫폼별 실제 데이터 위치는 [공식 troubleshooting 문서](https://opencode.ai/docs/troubleshooting/#storage)에 정리되어 있다.

즉, 과거 OpenCode의 JSON 파일 저장 방식을 그대로 참고하면 현재 구현과 어긋난다. DmuxAI 설계의 기준은 현재 SQLite 스키마와 session service여야 한다.

### 2. `session -> message -> part` 모델

`session`은 `project_id`, 선택적인 `workspace_id`와 `parent_id`, 실제 `directory`, worktree 상대 `path`, title, agent/model, summary, 비용·토큰, revert, permission, metadata, 생성·수정·압축·보관 시각을 가진다. `message`는 session FK와 JSON data를, `part`는 message FK·session ID와 JSON data를 가진다. session 또는 message 삭제 시 자식이 cascade되고 조회용 인덱스가 별도로 있다. [OpenCode SQL 스키마](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/core/src/session/sql.ts#L22-L98)

메시지 본문을 한 문자열로 저장하지 않는 점이 중요하다. 모든 part는 `id`, `sessionID`, `messageID`를 공유하며 다음 tagged type을 지원한다.

- `text`, `reasoning`, `file`, `tool`
- `agent`, `subtask`, `retry`
- `step-start`, `step-finish`
- `snapshot`, `patch`, `compaction`

정의는 [part 공통 필드와 주요 part](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/schema/src/v1/session.ts#L81-L202) 및 [전체 part union](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/schema/src/v1/session.ts#L357-L383)에 있다. 특히 tool part는 `pending/running/completed/error` 상태, input/output, attachment, metadata와 시간을 보존한다. [tool part 스키마](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/schema/src/v1/session.ts#L259-L325)

각 user message가 선택한 `agent`와 `providerID/modelID/variant`를 기록하고, assistant message가 부모 user message ID, 실제 agent/model/provider, cwd/root, 비용·토큰을 기록한다. [user message](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/schema/src/v1/session.ts#L327-L355), [assistant message](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/schema/src/v1/session.ts#L453-L500)

현재 SQL에는 이 V1 `message`/`part`와 함께 순번 기반 `session_message`, 입력 queue, context epoch 같은 V2용 테이블도 공존한다. [V2 session tables](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/core/src/session/sql.ts#L119-L176) 다만 여기서 분석한 실제 prompt 경로는 여전히 `MessageV2`라는 이름의 V1 `message`/`part` reader를 사용한다. OpenCode가 전환 중인 두 스키마를 DmuxAI가 그대로 복제하기보다, 필요한 의미만 한 세대의 스키마로 설계하는 편이 안전하다.

### 3. project와 worktree 귀속

OpenCode project는 ID, main worktree와 sandbox/worktree 목록을 저장하고, `project_directory`는 `(project_id, directory)`와 `main/root/git_worktree` 유형을 저장한다. [project SQL 스키마](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/core/src/project/sql.ts#L6-L34)

Git repository의 project ID는 normalized remote, repository cache ID, root commit을 순서대로 이용해 안정적으로 찾는다. non-Git 디렉터리는 global project로 처리한다. [project resolution](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/core/src/project.ts#L65-L120) 세션 생성 시 현재 project ID뿐 아니라 정확한 cwd와 worktree 대비 상대 경로도 저장한다. [session 생성과 조회 범위](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/opencode/src/session/session.ts#L499-L553)

그 결과 같은 저장소의 여러 worktree는 project identity를 공유할 수 있지만, 각 세션이 어느 checkout에서 실행되는지는 잃지 않는다. 대화 분기와 파일 분기를 별개의 축으로 다룰 수 있는 구조다.

### 4. resume와 agent/model 전환

CLI는 `--continue/-c`, `--session/-s`, 그리고 이 둘과 함께 쓰는 `--fork`를 제공한다. [`run` CLI 옵션](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/opencode/src/cli/cmd/run.ts#L147-L160) `--session`은 지정 ID를 고르고, `--continue`는 현재 project의 최신 root session을 고른다. 없으면 새 session을 만든다. [session 선택 로직](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/opencode/src/cli/cmd/run.ts#L456-L523)

프롬프트는 선택된 `sessionID`와 함께 server의 message endpoint로 전송되고, server는 DB의 활성 기록을 현재 모델용 messages로 변환해 호출한다. [프롬프트 전송](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/opencode/src/cli/cmd/run.ts#L863-L870), [기록 재구성과 실행](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/opencode/src/session/prompt.ts#L1252-L1284) 공식 server API도 session list/get/create/fork/children/summarize와 message read/send를 분리해 노출한다. [OpenCode server API](https://opencode.ai/docs/server/#sessions)

따라서 OpenCode의 “resume”은 공급자의 opaque thread에 다시 접속하는 동작이 아니라, OpenCode가 소유한 session 기록에서 모델 컨텍스트를 재구성하는 동작이다. 이 때문에 [공식 agents 문서](https://opencode.ai/docs/agents/#usage)가 한 session 안의 primary agent 전환을 지원할 수 있다.

### 5. fork

Fork는 원본 session을 공급자에게 fork해 달라고 요청하지 않는다. 새 session을 만든 뒤 지정 message 직전까지, 또는 지정이 없으면 전체 message/part를 새 ID로 복제한다. assistant의 `parentID`와 compaction의 `tail_start_id`도 새 ID에 맞게 remap한다. [fork 구현](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/opencode/src/session/session.ts#L691-L732)

이 방식이면 공급자가 native fork를 제공하지 않아도 동일한 의미를 구현할 수 있다. 파일 변경까지 격리하려면 새 session과 별개로 새 Git worktree를 연결해야 한다.

### 6. compaction

OpenCode는 원장을 summary로 덮어쓰지 않는다. 최근 turn을 위한 토큰 예산을 잡고, 오래된 head만 summary 대상으로 고른다. [tail 선택](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/opencode/src/session/compaction.ts#L203-L269) 완료된 과거 summary를 다음 rolling summary에 포함하고, 결과를 같은 session의 `summary:true` assistant message로 저장하며 compaction part에 `tail_start_id`를 기록한다. [compaction 처리](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/opencode/src/session/compaction.ts#L319-L466)

모델에 보낼 때만 최신 compaction marker + summary + recent tail + 이후 messages의 순서로 projection을 만든다. DB의 원문은 남아 있다. [model-visible compaction projection](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/opencode/src/session/message-v2.ts#L521-L581)

tool output도 별도로 다룬다. 최신 약 40k token을 보호한 뒤 오래된 tool output 중 20k token 이상을 줄일 수 있을 때 `compacted` 표시를 하고 모델 변환 시 placeholder로 바꾼다. 원래 JSON output 자체는 지우지 않는다. [tool output pruning](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/opencode/src/session/compaction.ts#L271-L317), [placeholder 변환](https://github.com/anomalyco/opencode/blob/3a31c4ea801915c0b050df4b3842997ea62b6e93/packages/opencode/src/session/message-v2.ts#L290-L323)

## 다른 에이전트의 방식

### Claude Code: 프로젝트별 JSONL transcript와 native fork

Claude Code session은 프로젝트 디렉터리에 묶인 독립 대화다. 같은 session을 resume하면 같은 ID와 transcript에 append하고, fork하면 원본 기록을 복사한 새 ID가 생긴다. `--continue`, `--resume <id|name>`, `/resume`, `/branch`, `--fork-session`을 제공한다. [Claude Code session 문서](https://code.claude.com/docs/en/sessions), [Agent SDK session 문서](https://code.claude.com/docs/en/agent-sdk/sessions)

transcript는 `~/.claude/projects/<project>/<session-id>.jsonl`에 message, tool use, metadata를 줄 단위 JSON으로 저장한다. 기본 보존 기간은 30일이다. picker는 현재 worktree에서 시작해 repository의 모든 worktree나 모든 project로 범위를 넓힐 수 있고 fork 계보도 묶어 보여 준다. [저장·picker·보존 정책](https://code.claude.com/docs/en/sessions)

중요한 운영 규칙도 있다. 같은 session을 두 terminal에서 fork 없이 resume하면 한 transcript에 message가 interleave될 수 있다. 대화 fork는 filesystem을 fork하지 않으므로 병렬 변경은 worktree 또는 checkpoint로 격리해야 한다. [branch 경고](https://code.claude.com/docs/en/sessions#branch-a-session), [checkpoint와 fork의 차이](https://code.claude.com/docs/en/checkpointing)

`/compact [instructions]`는 현재 컨텍스트를 summary로 교체한다. project instructions와 memory 같은 일부 항목은 다시 주입되지만, 대화 중에만 말한 세부는 summary 품질에 따라 손실될 수 있다. [context window와 compaction](https://code.claude.com/docs/en/context-window)

### Gemini CLI: project hash별 append-only JSONL과 checkpoint

Gemini CLI는 대화를 자동 저장하며 project root의 hash로 범위를 나눈다. 사용자·모델 메시지뿐 아니라 tool input/output, token usage, thought/reasoning summary도 저장한다. 위치는 `~/.gemini/tmp/<project_hash>/chats/`다. `--resume`은 최신 session, index 또는 UUID를 받을 수 있고 `/resume` picker는 검색·미리보기를 제공한다. 기본 retention은 30일이며 age/count를 설정할 수 있다. [공식 session management](https://github.com/google-gemini/gemini-cli/blob/5411f113cafae26161b4969b0237b8e1e024e2c2/docs/cli/session-management.md#automatic-saving)

현재 session 파일은 append-only JSONL이다. `ConversationRecord`에는 `sessionId`, `projectHash`, timestamps, messages, summary, memory scratchpad, 추가 directory, main/subagent 종류가 있고, message에는 type/content와 tool calls, thoughts, tokens, model을 담을 수 있다. [`ConversationRecord`와 `MessageRecord`](https://github.com/google-gemini/gemini-cli/blob/5411f113cafae26161b4969b0237b8e1e024e2c2/packages/core/src/services/chatRecordingTypes.ts#L40-L133) 저장 서비스는 metadata와 message를 append하고 `$set`, `$rewindTo` control record를 replay해 현재 상태를 재구성한다. [JSONL append/replay 구현](https://github.com/google-gemini/gemini-cli/blob/5411f113cafae26161b4969b0237b8e1e024e2c2/packages/core/src/services/chatRecordingService.ts#L150-L258), [append와 rewind](https://github.com/google-gemini/gemini-cli/blob/5411f113cafae26161b4969b0237b8e1e024e2c2/packages/core/src/services/chatRecordingService.ts#L512-L564)

Gemini CLI의 분기는 dedicated provider fork보다 checkpoint 중심이다. `/resume save <tag>`로 branch point를 저장하고 `/resume resume <tag>`로 그 시점의 대화를 복원해 다른 경로를 시도한다. `/rewind`는 대화만, 코드만, 둘 다 되돌리는 선택을 제공하고 병렬 작업에는 Git worktree를 권한다. [checkpoint와 worktree](https://github.com/google-gemini/gemini-cli/blob/5411f113cafae26161b4969b0237b8e1e024e2c2/docs/cli/session-management.md#manual-chat-checkpoints), [rewind 명령](https://github.com/google-gemini/gemini-cli/blob/5411f113cafae26161b4969b0237b8e1e024e2c2/docs/reference/commands.md#rewind)

압축은 오래된 head를 `<state_snapshot>`으로 요약하고 recent tail을 유지한다. 이전 snapshot의 유효한 사실을 병합하도록 지시하며, 두 번째 검증 호출로 file path, tool result, user constraint 누락을 점검한다. 큰 tool response는 먼저 별도 파일과 placeholder로 줄이고, summary가 원문보다 커지면 적용하지 않는다. [Gemini compression service](https://github.com/google-gemini/gemini-cli/blob/5411f113cafae26161b4969b0237b8e1e024e2c2/packages/core/src/context/chatCompressionService.ts#L248-L474)

### Aider: repo-local Markdown 복원과 최근 tail 보존 요약

Aider는 명시적인 session catalog보다 history file을 연속성의 단위로 사용한다. 기본 `.aider.chat.history.md`에 사람이 읽는 대화를 append하고, `.aider.input.history`에는 prompt 입력 기록을 둔다. `--restore-chat-history`는 기본 false이며 활성화하면 Markdown을 다시 user/assistant messages로 parse해 `done_messages`에 넣고 즉시 요약 필요 여부를 확인한다. [Aider 설정](https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/website/docs/config/aider_conf.md), [restore 구현](https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/coders/base_coder.py#L500-L525), [append 구현](https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/io.py#L1117-L1136)

이 복원은 완전한 실행 원장의 replay가 아니다. Markdown parser는 `####`를 user, 일반 본문을 assistant, blockquote를 tool로 해석하지만 기본 `include_tool=False`라 restore에서 tool-role 기록을 제외한다. [Markdown history parser](https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/utils.py#L148-L187) 따라서 DmuxAI는 Aider의 Markdown을 export 형식으로는 참고할 수 있지만 canonical storage로 채택해서는 안 된다.

대화가 soft token limit를 넘으면 Aider는 weak model 우선으로 오래된 head를 요약하고 최근 tail을 유지한다. 합친 결과도 크면 재귀적으로 다시 요약한다. [ChatSummary 구현](https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/history.py#L17-L121) 공식 명령과 옵션에는 ID 기반 session fork/picker가 없으므로, 별도 가지를 만들려면 다른 history file과 Git branch/worktree를 상위 도구가 관리해야 한다. [Aider 명령 목록](https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/website/docs/usage/commands.md)

## DmuxAI의 현재 구조와 간극

현재 DmuxAI UI의 대화 모델은 `Role`과 문자열 `content`뿐이고 `App.messages`에만 보관된다. agent, provider session, turn, tool event와 영속 ID가 없다. [현재 UI message](../src/app.rs#L39-L55), [인메모리 messages](../src/app.rs#L84-L91)

adapter 계약도 `send(String)`이고, 출력에는 agent와 event만 있다. conversation/turn/provider session binding을 UI나 저장소로 올릴 통로가 없다. [현재 adapter 계약](../src/agent/mod.rs#L53-L115)

공급자별 연속성은 worker 프로세스 메모리에만 있다.

- Codex는 시작할 때 항상 `thread/start`를 호출하고 받은 `thread_id`를 client 필드에만 둔다. [Codex adapter](../src/agent/codex.rs#L81-L181)
- Cursor는 local `session_id`가 있으면 `--resume`에 넘기지만 앱 재시작 시 복구되지 않는다. [Cursor adapter](../src/agent/cursor.rs#L84-L130), [resume 호출](../src/agent/cursor.rs#L202-L225)
- AGY도 같은 방식으로 local `conversation_id`를 `--conversation`에 넘긴다. [AGY adapter](../src/agent/agy.rs#L85-L131), [conversation 호출](../src/agent/agy.rs#L203-L224)

화면에 모든 agent의 text가 섞여 보이는 것과 실제 모델 컨텍스트가 공유되는 것은 별개다. 현재 구조에서는 다른 agent가 만든 turn을 provider-neutral 형식으로 재생할 방법이 없다.

## DmuxAI 권장 설계: OpenCode형 canonical session engine

### 핵심 원칙

1. **DmuxAI session이 대화를 소유한다.** 모든 agent는 같은 DmuxAI session의 turn producer다.
2. **provider binding은 부속 정보다.** native resume가 되면 사용하되, binding이 사라져도 DmuxAI 원장으로 새 provider session을 구성할 수 있어야 한다.
3. **원장과 projection을 분리한다.** compaction은 원문 삭제가 아니라 모델에 보낼 view 생성이다.
4. **대화 branch와 worktree를 분리한다.** 둘을 함께 만들 수는 있지만 동일 개념으로 취급하지 않는다.
5. **사람용 export와 기계용 저장을 분리한다.** Markdown은 export, SQLite의 구조화된 row가 canonical record다.
6. **Manual 공유와 Autopilot 공유를 분리한다.** 수동 대화에서는 사용자가 `/share` 같은 명시적 동작으로 컨텍스트 전달을 확인하고, 자동 실행에서는 같은 `run_id`의 계획·작업·리뷰 결과만 Orchestrator가 전달한다.
7. **slug는 별칭이고 UUID가 정체성이다.** 사람이 읽는 `run_slug`나 session title은 변경할 수 있지만, 내부 연결과 원장의 기본키는 안정적인 UUID를 사용한다.

### 최소 데이터 모델

```text
Project
  id, canonical_root, repository_identity

Workspace
  id, project_id, worktree_path, git_branch, head_commit

Session
  id, project_id, workspace_id, parent_session_id, forked_at_seq,
  title, created_at, updated_at, archived_at

Run (optional; Autopilot에서 사용)
  id, session_id, slug, goal, status, created_at, completed_at

Task (optional; Run에 속함)
  id, run_id, parent_task_id, owner_agent, dependencies_json, status

Turn
  id, session_id, run_id?, task_id?, seq, requested_agent, model, status,
  started_at, completed_at

Message
  id, turn_id, role, provider, parent_message_id, created_at

Part
  id, message_id, seq, kind, state, data_json
  # text | reasoning | tool_call | tool_result | file | diff |
  # usage | notice | error | summary | compaction

ProviderBinding
  session_id, agent, provider_session_id, last_consumed_seq,
  status, updated_at

ContextShare
  id, session_id, source_turn_id, target_agent, mode,
  included_through_seq, rendered_hash, created_at

RunEvent
  id, run_id, sequence, event_type, actor_agent, payload_json, created_at

ContextCheckpoint
  session_id, covers_through_seq, tail_start_seq, source_hash,
  prompt_version, summary_json, created_at
```

`Part.state`는 최소 `pending/running/completed/error/cancelled`를 지원해야 한다. streaming 중 앱이 종료되어도 미완료 part를 복구할 수 있고, tool output과 일반 text를 구분해 선택적으로 압축할 수 있다.

### adapter 계약 변경

현재 문자열 중심 API를 session/turn 중심으로 바꾼다.

```rust
pub struct AgentRequest {
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub prompt: String,
    pub provider_session_id: Option<String>,
    pub context: ContextProjection,
}

pub enum AgentEvent {
    ProviderSessionBound { id: String },
    PartStarted { part_id: PartId, kind: PartKind },
    PartDelta { part_id: PartId, delta: String },
    PartCompleted { part_id: PartId, data: serde_json::Value },
    Usage(Usage),
    Completed,
    Cancelled,
    Error(String),
}
```

모든 event envelope에 `session_id`, `turn_id`, `agent`를 넣는다. provider가 새로운 thread/session/conversation ID를 알려 주는 즉시 `ProviderSessionBound`를 먼저 commit한다. 완료 시점까지 기다리면 프로세스 crash로 binding을 잃을 수 있다.

### Manual 공유와 Autopilot 컨텍스트 동기화

각 `ProviderBinding.last_consumed_seq`로 해당 native session이 이미 본 DmuxAI 원장의 위치를 기록한다.

- 같은 agent를 연속 사용하면 native resume 후 현재 요청만 전달한다.
- Manual 모드에서 agent를 바꿨다는 이유만으로 다른 agent의 turn을 자동 주입하지 않는다. 자동 주입은 무엇이 공유됐는지 불명확하고, native session의 기존 문맥과 중복될 수 있기 때문이다.
- Manual 모드의 `/share` 또는 `F3`는 사용자가 선택한 최근 완료 turn 하나를 대상 agent의 다음 요청에 synthetic briefing으로 첨부한다. 초기 제한은 4~8KB가 적당하며 출처 agent, turn, 시각, 잘림 여부를 화면과 `ContextShare`에 남긴다.
- `/relay cursor codex`는 Cursor의 완료 결과를 Codex의 새 turn으로 명시적으로 전달하고, `/broadcast`는 같은 사용자 요청을 각 agent의 독립 turn으로 보낸다. 둘 다 자동 루프를 만들지 않는다.
- Autopilot 모드에서는 Orchestrator가 같은 `run_id`의 goal, acceptance criteria, 선행 task 결과, 미해결 blocker와 review 결과만 자동 전달한다. 일반 Chat 전체를 붙이지 않고 `run_id/task_id/turn_id`로 관련 결과를 좁힌다.
- 처음 쓰는 agent나 native resume가 실패한 agent는 DmuxAI projection으로 새 native session을 만들고 binding을 기록하거나 교체한다.

이 구분은 두 사용자 경험을 모두 보존한다. Manual 사용자는 언제 어떤 답변이 공유되는지 통제할 수 있고, Autopilot은 계획→배정→구현→리뷰 파이프라인에 필요한 결과를 자동으로 이어 간다. synthetic briefing은 canonical user message와 구분되는 part type/metadata로 남겨 재생과 디버깅 때 식별 가능해야 한다.

현재 추가된 `/agent` picker는 이 정책의 자연스러운 진입점이다. agent를 고른 뒤 `현재 세션 계속`, `새 세션`, `선택한 turn과 함께 새 세션`을 고르게 하고, 별도의 `/share` 명령은 전송 전에 briefing preview를 보여 주는 편이 좋다.

### fork와 worktree

OpenCode처럼 DmuxAI 자체 fork를 우선 구현한다.

1. branch point의 `seq`를 선택한다.
2. 새 session에 `parent_session_id`, `forked_at_seq`를 기록한다.
3. 구현은 초기에는 copy-on-write 조회로 하고, 단순성이 더 중요하면 message/part를 새 ID로 복제한다.
4. provider가 native fork를 지원하면 새 branch의 binding으로 연결한다.
5. 지원하지 않으면 branch point projection으로 새 provider session을 시작한다.
6. 파일 변경 격리가 필요하면 별도의 Workspace/worktree를 만들고 새 session에 연결한다.

동일 provider session에는 한 번에 한 turn만 append하도록 mutex 또는 DB lease를 둔다. Claude Code가 문서화한 transcript interleave 문제를 DmuxAI 계층에서 차단하기 위해서다.

### compaction

OpenCode와 Gemini의 장점을 합친다.

- full transcript는 삭제하거나 summary로 덮어쓰지 않는다.
- summary는 `covers_through_seq`, `tail_start_seq`, `source_hash`, `prompt_version`과 함께 별도 저장한다.
- summary 본문은 `goal`, `constraints`, `decisions`, `facts`, `files`, `failed_attempts`, `open_questions`의 구조화 JSON으로 저장하고 모델용 text로 렌더링한다.
- 최근 turn은 token budget 안에서 원문 보존한다.
- 오래된 대형 tool output은 먼저 placeholder로 바꾸되 원본 part는 유지한다.
- rolling summary는 이전 summary의 아직 유효한 사실을 병합하고, 별도 검증 pass로 file path·결정·사용자 제약의 누락을 확인한다.
- 다른 agent에 전달할 때 reasoning 원문이나 provider metadata는 제거하고 text/tool result/diff/결정만 provider-neutral 형태로 변환한다.

### 저장 방식

DmuxAI에는 OpenCode처럼 SQLite를 canonical store로 권장한다. TUI가 목록·검색·branch tree·agent별 상태를 자주 조회하고 streaming part를 갱신하므로 정규화된 테이블과 transaction이 잘 맞는다. WAL과 foreign key를 켜고, schema migration을 앱 시작 시 적용한다.

Gemini/Claude의 JSONL 장점도 가져올 수 있다. 원한다면 provider raw event를 append-only JSONL diagnostic log로 별도 저장하되, 앱 동작과 검색은 SQLite의 normalized row를 사용한다. raw log는 canonical UI 모델이 아니라 복구·디버깅 자료다.

## 구현 순서

### 1단계: 재시작 후 같은 agent 대화 복구

- SQLite migration과 `Session/Turn/Message/Part/ProviderBinding` repository 추가
- `AgentOutput` envelope에 session/turn ID 추가
- `ProviderSessionBound` event 추가
- Codex `thread/resume`, Cursor `--resume`, AGY `--conversation`을 저장된 binding으로 시작
- streaming delta를 transaction 단위로 part에 반영하고 미완료 turn 복구

이 단계가 가장 작은 end-to-end slice다. 완료 조건은 앱 종료 후 재실행해 Codex, Cursor, AGY 각각의 기존 대화를 선택하고 이어 갈 수 있는 것이다.

### 2단계: Manual 명시 공유

- `last_consumed_seq`와 provider-neutral `ContextProjectionBuilder` 추가
- `/share` 또는 `F3`로 최근 완료 turn 하나를 preview 후 synthetic briefing으로 전달
- `ContextShare`에 source/target/range/hash를 기록하고 Chat에 공유 표시
- text 외 tool result, file, diff, 오류도 normalized part로 수집
- “이 agent가 어디까지 알고 있는지”를 UI에 표시

완료 조건은 공유하지 않은 사실은 다른 agent가 모르고, `/share`한 사실만 대상 agent가 알며, Chat에서 전달 범위를 다시 확인할 수 있는 것이다.

### 3단계: Autopilot Run 공유

- `run_id/task_id/turn_id`와 append-only `RunEvent` 추가
- 계획·작업 결과·리뷰 결과를 구조화하고 다음 turn의 관련 컨텍스트로 자동 투영
- 최대 turn/재시도, 동일 결론 반복 감지, 구현자와 reviewer 분리
- 초기에는 한 번에 하나의 쓰기 agent만 허용하고 분석·리뷰만 병렬화
- 같은 파일을 병렬 수정해야 할 때는 write lease 또는 task별 Git worktree 사용

완료 조건은 하나의 목표가 계획→작업 배정→구현 결과 전달→독립 리뷰까지 사용자 중계 없이 진행되고, 모든 이벤트가 같은 `run_id`로 재생되는 것이다.

### 4단계: session UX와 branch

- session create/list/search/rename/archive/delete picker
- title, first prompt, message count, agent badges, last activity, worktree 표시
- turn에서 fork, branch tree, optional worktree 생성
- provider session 동시 append 방지

### 5단계: compaction과 retention

- token estimator와 summary + recent tail projection
- tool output pruning placeholder
- summary source hash/검증/재생성
- retention 설정과 Markdown/JSON export

## OpenCode를 새 agent로 직접 붙이는 경우

OpenCode 자체도 DmuxAI의 provider adapter로 추가할 수 있다. CLI subprocess의 텍스트만 읽는 방식보다 `opencode serve`를 실행하고 공식 Session/Message API를 쓰는 편이 안정적이다. API가 session create/list/get/fork/summarize 및 message send/read를 이미 제공하기 때문이다. [OpenCode server API](https://opencode.ai/docs/server/#sessions)

다만 이 경우에도 OpenCode session을 DmuxAI session으로 사용해서는 안 된다. OpenCode session ID는 `ProviderBinding(agent=OpenCode)`에 저장하고, DmuxAI의 canonical transcript와 branch/worktree 모델을 위에 유지해야 Codex·Cursor·AGY와 동일한 전환 규칙을 적용할 수 있다.

## 최종 권고

DmuxAI가 따라야 할 핵심은 “모든 공급자 세션을 잘 보관한다”가 아니라 **OpenCode처럼 공급자보다 위에서 대화를 소유한다**는 점이다.

우선순위는 다음과 같다.

1. provider-neutral structured transcript
2. project/worktree-scoped session과 persistent provider binding
3. Manual의 명시적 `/share`와 감사 가능한 `ContextShare`
4. Autopilot의 `run_id/task_id/turn_id` 기반 자동 전달과 `RunEvent`
5. 원장과 model-visible context projection의 분리
6. application-level fork와 concurrency guard
7. summary + recent tail compaction 및 retention

이 구조가 완성되면 provider-native resume는 성능과 정확도를 높이는 가속 경로가 되고, 공급자 세션이 사라지거나 agent를 바꿔도 DmuxAI 대화 자체는 계속 유지된다.
