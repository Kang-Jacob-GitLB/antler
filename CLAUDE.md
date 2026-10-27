# CLAUDE.md

Antler 는 Windows · WSL · SSH 의 git 저장소를 한 창에서 보는 **읽기 전용** Git GUI(Electron + React + TypeScript)다.
사용자 소개는 [README.md](README.md), 세부 문서는 [docs/](docs/) 에 있다.

| 문서 | 내용 |
|---|---|
| [docs/architecture.md](docs/architecture.md) | 디렉터리 구조, 핵심 설계, 성능, 의존성 |
| [docs/security.md](docs/security.md) | 창·IPC·악성 저장소 방어, fetch 가드, GitHub 원격 전용 저장소 예외 |
| [docs/build.md](docs/build.md) | 설치·실행·패키징·릴리스(CI), 개발 도구 스크립트 |
| [docs/connections.md](docs/connections.md) | GitHub·Jira 로그인, WSL·SSH |
| [docs/updates.md](docs/updates.md) | 앱 안 업데이트 동작과 검증 |

## 명령

```bash
pnpm install
pnpm dev                        # 개발 모드(HMR)
pnpm typecheck                  # main/preload + renderer 타입 검사
pnpm test                       # vitest
pnpm build                      # out/
node scripts/check-contrast.mjs # 디자인 토큰 WCAG 대비(미달 시 exit 1)
```

PR 필수 체크(`build.yml`)는 위 typecheck · test · build · check-contrast 다. 커밋 전에 넷 다 돌린다.
화면 확인은 `pnpm build` 후 `node scripts/drive.mjs <저장소>` (임시 userData 를 써서 실사용 설정을 건드리지 않는다).

## 지켜야 할 경계

- **저장소에는 쓰지 않는다.** commit·push·merge 같은 쓰기 기능은 넣지 않는다. 예외는 원격 추적 ref 를 갱신하는 `git fetch` 와
  GitHub 원격 전용 저장소의 앱 캐시뿐이다. Jira 는 조회와 이슈 상태 전환만 한다(워크로그 쓰기는 사용자 결정 없이 넣지 않는다).
- **fetch 보안 모델을 약화하지 않는다.** 원격마다 목적지를 `refs/remotes/<이름>/` 로 고정한 refspec + `--refmap=`,
  저장소 로컬 설정은 **허용 목록**(`src/main/git/fetch.ts` 의 `ALLOWED_KEYS`)만 통과시킨다. 차단 목록 방식으로 되돌리지 않는다
  (리뷰마다 우회돼 폐기한 방식이다). 자세한 이유는 docs/security.md.
- 저장소 내용(코드·커밋)을 외부 서비스로 보내는 기능은 넣지 않는다(예: AI 커밋 설명).
- 토큰은 main 프로세스에만 두고 renderer 로 넘기지 않는다.
- 저장소 내용을 읽는 git 호출은 textconv·필터가 돌지 않는 형태만 쓴다(파일 내용은 `cat-file blob`, `git show` 금지 — `src/main/git/codeBrowse.ts`).

## 코드 규약

- **새 IPC 채널**은 `src/shared/ipc.ts` 의 `IpcMap` 과 `IPC_CHANNELS` 허용 목록 **둘 다**에 넣는다. 빠뜨리면 preload 가 막는다.
- main 은 renderer 가 보낸 모든 입력(경로·해시·ref 등)을 검증한다. git 호출은 `GitExecutor` 를 거친다.
- 명령·단축키는 `src/renderer/src/lib/commands.ts` 한 곳에서 정의한다(팔레트·단축키 목록·전역 단축키가 모두 여기서 나온다).
- 문구는 `src/renderer/src/i18n/` 의 ko · en 둘 다 추가한다.
- 클립보드 복사는 `lib/clipboard.ts` 의 `copyText` 를 쓴다. 앱이 권한 요청을 모두 거부해 `navigator.clipboard` 는 조용히 실패한다.
- 스타일: 컴포넌트 CSS 에 색을 하드코딩하지 않고 semantic 토큰(`styles/tokens.css`)만 쓴다. 대화상자 입력칸은 공통 `.field__input` 을 쓴다.
  디자인은 평면(Fork/GitKraken식)이다. UI 동작이 애매하면 GitKraken 을 기준으로 한다.
- 테스트는 `tests/setup.ts`·`tests/globalSetup.ts` 가 `GIT_CONFIG_GLOBAL` 등을 격리한다. git 을 실행하는 테스트도 이 격리를 우회하지 않는다.
- 줄바꿈이 파일마다 다르다(대부분 LF, 일부 CRLF). 셸·스크립트로 파일을 통째로 다시 쓰지 말고, 커밋 전에 `git diff --stat` 으로
  의도치 않은 전체 변경이 없는지 확인한다.

## 브랜치·PR·릴리스

- main 은 PR 필수이며 **squash 머지**만 쓴다.
- squash 커밋 제목 = PR 제목이고 야간 릴리스가 이것으로 버전을 정하므로 **PR 제목은 Conventional Commits** 로 쓴다
  (`feat:` minor, `fix:`·`perf:`·`style:` patch, `!`/`BREAKING CHANGE` major, `docs:`·`chore:`·`ci:`·`test:`·`refactor:` 만이면 릴리스 안 함).
- `package.json` 버전은 `0.0.0` 고정이다. 릴리스 때 버전을 올리는 커밋을 만들지 않는다(CI 가 태그에서 넣는다).
