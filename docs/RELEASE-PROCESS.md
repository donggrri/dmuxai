# Release process

This is the release process for dmuxai. Every release follows these steps in order. A release
ships only after the owner approves it (step 7).

## Steps

0. **Create the release-candidate branch.** Branch from the latest `main` and name it
   `release-candidate-vX.Y.Z` (for example `release-candidate-v0.2.3`).
   ```sh
   git fetch origin
   git switch -c release-candidate-vX.Y.Z origin/main
   git push -u origin release-candidate-vX.Y.Z
   ```
1. **Audit specs against the implementation.** For every `docs/specs/SPEC-*.md` and
   [docs/specs/DMUXAI-BASELINE.md](specs/DMUXAI-BASELINE.md), list requirements that the code
   does not implement, implements differently, or only partly implements. Record each gap as an
   issue or fix PR before testing.
2. **Test from the specs.** First confirm that each test case (TC) has a written how-to-test
   procedure and an expected result (use the [manual TC template](#manual-tc-template)). Fill in
   missing TC docs before running them. Then run every TC and record pass/fail with evidence.
3. **Fix failures and retest.** Never commit directly to `main`. Fix through a PR to `main`, then
   bring the fix into the release-candidate branch (cherry-pick or merge `main`). Rerun the failed
   TCs and any related TCs.
4. **Improve the process.** If anything was missing to run this process (docs, TC procedures,
   scripts, agent skills), add or improve it in the same release cycle.
5. **Run dmuxai for real.** Anything testable in dmuxai (TUI, CLI, dashboard, runners) is verified
   by running the dmuxai build from the release-candidate branch, not only by unit tests.
6. **UI interactions with the Grok Bot agent.** UI checks that need clicks or other input (dashboard
   buttons, dialogs, window switching) are done together with the Grok Bot desktop/browser agent.
   Capture screenshots as evidence.
7. **Release gate.** The owner reviews the screenshots and the TC-ID pass/fail table and gives an
   explicit OK. Only then merge the release PR (`release-candidate-vX.Y.Z` -> `main`) and push
   tag `vX.Y.Z`.

## Release report

Attach this to the release PR:

| TC ID | Spec | Result (pass/fail/blocked) | Evidence |
|-------|------|----------------------------|----------|
| TC-... | SPEC-... | pass | screenshot / log link |

Also list spec gaps found in step 1, fix PRs from step 3, and any TCs that were not run (with the reason).

## Manual TC template

Copy this for each manual test case. Keep it in the spec's test section or in `docs/TEST-PLAN.md`.

```markdown
### TC-<SLUG>-<NN>: <short title>

- **Spec requirement:** <REQ ID / section>
- **Preconditions:** <build, runner/agent + model, config, login, working directory>
- **Steps:**
  1. ...
  2. ...
- **Expected result:** <observable outcome; exact text/state where possible>
- **Evidence:** <screenshot path, log excerpt, session id>
- **Result:** pass | fail | blocked (<date>, <tester>)
```

## 요약 (Korean summary)

0) `main`에서 `release-candidate-vX.Y.Z` 브랜치를 만든다.
1) SPEC과 구현이 다르거나 빠진 부분을 검사한다.
2) SPEC 기반으로 테스트한다. 먼저 TC마다 테스트 방법과 기대 결과 문서가 있는지 확인한다.
3) 실패하면 PR로 `main`에 수정하고 RC 브랜치에도 반영한 뒤 다시 테스트한다.
4) 과정에 부족한 점이 있으면 문서나 스킬을 보완한다.
5) dmuxai로 할 수 있는 테스트는 dmuxai를 직접 실행해서 확인한다.
6) 버튼 클릭 등 UI 테스트는 Grok Bot 에이전트와 함께 한다.
7) 오너가 스크린샷과 TC별 통과/실패 표를 보고 OK하면 릴리즈 PR을 머지하고 태그를 단다.
