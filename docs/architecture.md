# 구조와 설계

[← README](../README.md)

## 구조

```
src/
  shared/      types(데이터 타입), ipc(IPC 계약 — 채널·인자·결과 타입, 허용 목록 IPC_CHANNELS),
               gitflow·lfs·launchpad·avatar·checkTtl·logScope·prNumber(main·renderer 공용 순수 로직)
  main/
    index.ts, titleBar.ts, windowState.ts
    executors/ GitExecutor 인터페이스, LocalExecutor, WslExecutor, SshExecutor, wslPaths,
               ssh/(config·connection·knownHosts·quote), processRunner, ProcessQueue(동시성 제한+dedupe)
    git/       commands(refs·search·logArgs), logCursor, parsers/(log, refs, worktree, stash, commit, changes, diff, nulRecords),
               fetch(설정 허용 목록 검사), wip(작업 트리 상태), activity(미니맵), fileInsight(파일 히스토리·blame·LFS),
               repoExtras(LFS·Gitflow·서브모듈), defaultBranch, githubCache(GitHub 원격 전용 저장소),
               codeBrowse(코드 탐색기: ls-tree·cat-file blob·rev 해석·push 여부)
    github/    api, client(ETag 캐시), deviceFlow, tokenStore, ghCli(CLI 로그인 가져오기), remoteUrl, service, validate,
               accessibleRepos, avatars, checks, launchpad, devFake(개발용 가짜 응답)
    jira/      api, client, service, validate, adf(Atlassian 문서 형식 변환)
    external/  opener, plan — 파일 관리자·에디터·터미널로 열기
    update/    logic(semver·자산 선택·해시 검증·정책), manual(GitHub Releases 검증 다운로드), UpdateService(상태 기계),
               autoBackend(electron-updater), releaseNotes
    services/  ConfigStore(JSON 설정), RepoService(저장소 단위 진입점), RepoWatcher(외부 변경 감시), FetchService, repoScan, lru
    ipc/       handlers(IpcEnvelope 로 감싸 등록), github, jira, external, confirmText
  preload/     window.antler — 허용 목록 채널만 노출
  renderer/src/
    components/   Toolbar, Sidebar, CommitList, GraphCanvas, DetailPanel(+두 커밋 비교), DiffView, FileView(히스토리·Blame),
                  CodeView·MarkdownDoc(코드 탐색기·GitHub 식 마크다운), CommandPalette, ShortcutsDialog, CommitMinimap, WipGraph·WipPanel, RepoSwitcher, SearchBar, AddRepoDialog,
                  SettingsDialog, ExternalMenu, LfsBadge·FlowBadge, UpdateSnackbar …
    components/github/     GitHubView, GhAuthButton, GhBits, 체크 목록
    components/jira/       JiraWorkspace, JiraView(목록·칸반), JiraConnectDialog, JiraDevPanel, worklog/(캘린더·리포트)
    components/launchpad/  LaunchpadView
    stores/       appStore(설정·활성 저장소·테마·refs), logStore(커밋 페이지·필터·찾아가기), detailStore(커밋 상세·diff),
                  compareStore, fileViewStore, githubStore, launchpadStore, jiraStore·jiraWorklogStore, wipStore, fetchStore,
                  paletteStore, searchStore …
    lib/          commands(명령 레지스트리 — 팔레트·단축키 목록·전역 단축키의 단일 출처), fuzzy·commandFilter, shortcuts,
                  sanitize, markdown, readme(문서 렌더·slug·상대 경로), lineLink(줄 링크), clipboard, format, laneColor, api …
    graph/        lanes.ts(레인 계산, 순수 함수), graphEngine.ts(worker 결과 보관)
    diff/         patch.ts(파싱), diffRows.ts(unified/split 행), highlight.ts(지연 로드)
    workers/      laneWorker.ts
    styles/       tokens.css(디자인 토큰 3층), base.css, fonts.css
    i18n/         ko, en
resources/       앱 아이콘(icon.svg·icon-small.svg 원본, 생성된 png/ico/icns, icons/*.png), licenses/
electron-builder.yml  패키징 설정
scripts/         개발 도구 스크립트(build.md 참고)
tests/           vitest + fixtures/
docs/            이 문서들과 README 스크린샷(images/)
```

## 핵심 설계

- **git 은 항상 저장소가 있는 환경 안에서 실행한다.** `GitExecutor.run/stream` 이 환경 차이를 감추고,
  모든 호출에 `GIT_OPTIONAL_LOCKS=0`, `-c core.quotepath=false`, `-z` + `--format` 을 쓴다.
- **LogCursor**: `git log` 는 시작할 때마다 모든 ref 를 읽는다. loose ref 2,500개짜리 저장소에서 이 고정 비용이
  약 450ms 라, 페이지마다 `--skip` 으로 새 프로세스를 띄우면 스크롤을 못 따라간다. 그래서 프로세스 하나를 열어 두고
  스트림 backpressure 로 멈춰 둔 채 다음 페이지를 이어 읽는다. 순서를 벗어난 요청만 `--skip` 으로 다시 시작한다.
- **레인 그래프**: `--date-order`(자식이 항상 부모보다 먼저) 결과를 Web Worker 가 위에서 아래로 한 번 훑으며
  행마다 FULL(통과)·TOP(모여듦)·BOTTOM(갈라짐) 선분과 노드 레인을 계산한다. renderer 는 보이는 행만 Canvas 에
  색별 `Path2D` 로 묶어 그린다. 색: main/master 는 `--lane-0`(앰버) 고정, 브랜치 tip 은 이름 해시,
  merge 의 두 번째 부모는 merge 제목(`Merge branch 'x'`)에서 이름을 읽어 같은 브랜치가 세션이 바뀌어도 같은 색이 된다.
- **환경별 실행(M4)**: git 은 항상 저장소가 있는 환경 안에서 돈다.
  - WSL: `wsl.exe -d <배포판> --cd ~ --exec env <GIT_ENV> git -C <경로> …` — `--exec` 라 리눅스 셸이 인자를 해석하지 않는다.
    `\\wsl.localhost\<배포판>\…` 경로를 고르면 WSL 저장소로 바꿔 등록한다(9P 공유를 거치지 않아 빠르고 정확하다).
  - SSH: `ssh2` 로 호스트당 연결 하나를 유지하고 exec 채널을 연다(Windows OpenSSH 는 ControlMaster 미지원).
    `~/.ssh/config`(Host·HostName·User·Port·IdentityFile·ProxyJump·Include)를 따르고, 인증은 ssh-agent·키 파일만 —
    비밀번호는 저장하지도 묻지도 않는다. 호스트 키는 `known_hosts` 로 검증하며 모르는 키는 자동 등록하지 않는다.
  - 경로 `~/x` 는 셸 없이 실행되므로 두 실행기 모두 홈 디렉터리에서 실행해 상대 경로로 처리한다.
- **GitHub 연동(M5)**: remote URL(origin 우선)에서 owner/repo 를 추론하고, API 는 환경(로컬·WSL·SSH)과 무관하게
  main 이 직접 호출한다. 모든 GET 은 ETag 조건부 요청이라 다시 볼 때 304(호출 한도 차감 없음)로 끝난다.
  로그인은 PAT 또는 Device Flow(설정에 OAuth App client ID 필요). 토큰은 Electron `safeStorage` 로 암호화해
  저장하고 renderer 로는 절대 넘기지 않는다. PR·이슈 본문은 GitHub 가 렌더링한 HTML 을 앱에서 한 번 더 허용 목록으로
  정화하고, 링크는 github.com 만 기본 브라우저로 연다(이미지는 CSP 로 *.githubusercontent.com 만).
- **외부 변경 자동 반영**: 활성 저장소만 감시한다(`RepoWatcher`). 로컬은 `.git`(워크트리면 공통 디렉터리까지) 재귀 감시 → 디바운스,
  WSL 4초·SSH 30초 폴링(창이 뒤에 있으면 4배 느리게, 앞으로 오면 즉시 확인). HEAD 와 모든 ref 의 해시 요약이
  바뀐 경우에만 사이드바·그래프를 다시 읽고, 보던 커밋 선택은 유지한다.
- **커밋 상세·diff**: 커밋을 고르면(↑↓ 연타 대비 60ms 디바운스) 메타데이터와 첫 부모 대비 변경 파일(+/− 통계)을
  불러온다. 파일을 누르면 중앙이 diff 로 바뀐다(Esc 닫기, Alt+↑/↓ 이전/다음 파일). diff 는 3,000줄까지만 담아
  보내고 나머지는 "전체 불러오기"로 받는다. 상세·diff 는 커밋 해시 기준 불변 LRU 캐시(main)와 상세 캐시(renderer)에 담는다.
- **사이드바 동작(GitKraken 식)**: 브랜치·태그 클릭 = 그래프에서 그 커밋으로 이동(필요하면 더 불러옴),
  더블클릭/깔때기 버튼 = 그 ref 만 보기(툴바 필터 칩으로 해제). 워크트리·스태시 클릭 = 그 HEAD 기준 보기.
- **디자인 토큰**: palette(원시) → semantic(테마가 교체하는 층) → component(semantic 만 참조) 3층.
  컴포넌트 CSS 는 색을 하드코딩하지 않는다. 테마는 `<html data-theme>` 의 토큰 값 교체로만 바뀐다.

보안 설계(창·IPC·악성 저장소 방어·fetch 가드)는 [security.md](security.md) 에 따로 정리했다.

## 디자인

- 사슴뿔과 숲에서 가져온 색 토큰을 palette → semantic → component 3층으로 둔다(`tokens.css`).
- 스타일은 평면(Fork/GitKraken식)이다.
- 폰트: UI 는 Pretendard Variable(앱에 번들, OFL 1.1). 코드는 GitHub 과 같은 시스템 고정폭 스택(`ui-monospace, SFMono-Regular, SF Mono, Menlo, Consolas, Liberation Mono`). 한글만 나눔고딕코딩(400·700, OFL 1.1, 앱에 번들, unicode-range 로 한글에만 적용)으로 그린다.

## 성능

이 Windows 11 머신에서 `scripts/drive.mjs` 로 측정(콜드 스타트는 앱 프로세스 시작 기준).

| 항목 | 측정 | 목표 |
|---|---|---|
| 콜드 스타트 | 1.24s(이 저장소) / 1.80s(10만 커밋, loose ref 2,500개) | 2s |
| 유휴 메모리 | private 203–214MB | 300MB |
| 스크롤(10만 커밋) | p95 13.4ms | 60fps |

하드웨어 가속은 기본으로 끈다. Canvas2D·텍스트 위주 앱이라 GPU 프로세스 비용이 더 크다
(부하 후 private 506→280MB, p95 16.8→13.4ms). `ANTLER_GPU=1` 로 다시 켠다.

## 의존성과 추가 이유

런타임 의존성(`dependencies`)은 **ssh2, electron-updater 둘**이다. renderer 라이브러리는 번들에 들어가므로 devDependencies 에 둔다.

| 패키지 | 이유 |
|---|---|
| electron-updater (runtime) | 앱 안 업데이트(Windows NSIS·Linux AppImage). main 프로세스에서 쓰이므로 번들되지 않는 runtime 의존성 |
| ssh2 (runtime) | SSH 연결 유지·exec 채널 — Windows OpenSSH 는 ControlMaster 미지원 |
| react, react-dom (dev) | renderer UI — 번들에 포함 |
| zustand (dev) | 보일러플레이트 없는 경량 상태관리(≈1KB) |
| @tanstack/react-virtual (dev) | 10만 행 가상 스크롤 |
| highlight.js (dev) | diff syntax highlight — core 와 필요한 언어만 diff 를 열 때 지연 로드(언어별 청크) |
| electron, electron-vite, vite, @vitejs/plugin-react | 앱 런타임과 main/preload/renderer 통합 빌드 |
| electron-builder | 설치 파일 패키징(`pnpm dist*`) |
| typescript, @types/* | strict 타입 검사 |
| vitest | 파서·executor 단위 테스트 |
| happy-dom | renderer 의 HTML 정화(sanitize) 테스트에 쓰는 가벼운 DOM 구현 — 테스트 전용 |
| playwright-core | 빌드된 앱을 실제로 띄워 측정·스크린샷(`scripts/drive.mjs`) — 브라우저 다운로드 없는 core 판 |
| @resvg/resvg-js, png2icons | 아이콘 생성(`pnpm icons`): SVG → PNG, PNG → ico/icns |
| marked | 커밋 메시지 본문을 마크다운으로 렌더(GFM). 결과 HTML 은 `lib/sanitize.ts` 허용 목록으로 정화 |
| pretendard | UI 폰트 Pretendard Variable(OFL 1.1). woff2 하나만 renderer 자산으로 번들 |
| @fontsource/nanum-gothic-coding | 코드 속 한글용 나눔고딕코딩(OFL 1.1). korean 서브셋 400·700 woff2 두 개(약 1 MB) — devDependency |

## 마일스톤

- [x] **M1** 스캐폴딩 + 디자인 토큰/테마 + LocalExecutor + 커밋 목록
- [x] **M2** 그래프 렌더링(레인 팔레트) + 브랜치/태그/워크트리 사이드바
- [x] **M3** 커밋 상세, 변경 파일 리스트, diff 뷰어
- [x] **M4** WslExecutor, SshExecutor + 환경 관리 UI
- [x] **M5** GitHub PR/이슈 연동
- [x] **M6** 아이콘/브랜딩, 성능 튜닝, 패키징
- [x] 이후(v1.x): 명령 팔레트, 파일 히스토리·Blame, 두 커밋 비교, WIP 행, 커밋 미니맵, LFS·Gitflow 표시, Actions,
      Launchpad, Jira 작업 공간·워크로그, GitHub 원격 전용 저장소, 외부 도구로 열기, 앱 안 업데이트
