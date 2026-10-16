# Graph execution report: image attachments & UI screenshots

**날짜:** 2026-10-16  
**브랜치:** `cursor/image-attachment-c906`  
**베이스:** `cursor/phase1-implement-permissions-c906` (권한 PR #2)

## 요약

- 프롬프트 UI에 `/image <path>`·경로 붙여넣기·프롬프트 내 이미지 경로 토큰으로 `png/jpg/webp`(10MiB) 첨부.
- Cursor·AGY는 CLI `--image`; Codex는 턴 텍스트에 `@path` 참조.

> 후속 정리(2026-10-17): Cursor·AGY는 `--image` 플래그가 없어 `@path` 토큰으로
> 전달하도록 변경되었고, `docs/screenshots/` 및 스크린샷 export 테스트는 삭제되었다.

## 디스패치 (이번 작업)

| 단계 | 담당 | 모델 | 결과 |
| --- | --- | --- | --- |
| 첨부 저장소·러너 전달 | in-repo | — | `src/attachments.rs`, adapter `--image` |
| TUI·제출 연동 | in-repo | — | `app.rs` chips·`/image`·paste |
| 스크린샷·문서 | in-repo | — | `ui_png.rs`, README, screenshots |

외부 graph subagent 디스패치는 사용하지 않음 (단일 PR 범위).

## 검증

```bash
cargo fmt --check
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
```
<!-- 스크린샷 export 테스트는 삭제됨 (docs/screenshots 정리) -->
