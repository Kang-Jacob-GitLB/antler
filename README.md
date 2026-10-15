<img src="resources/icons/128.png" width="64" alt="Antler">

# Antler

Windows · WSL · SSH(원격 Ubuntu)의 git 저장소를 한 창에서 살펴보는 가벼운 크로스플랫폼 Git GUI(Electron).
읽기(조회) 전용이다. 이름은 사슴뿔(antler)처럼 갈라져 뻗는 git 브랜치(레인) 구조에서 따왔다.

## 주요 기능

- **커밋 그래프**: Canvas 로 그리는 레인 그래프, 10만 커밋 가상 스크롤
- **사이드바**: 로컬·원격 브랜치, 태그, 워크트리, 스태시, GitHub PR·이슈
- **ref 필터**: 특정 ref 만 보기(툴바 칩으로 해제)
- **커밋 검색**: `Ctrl/Cmd+F`
- **저장소 전환**: `Ctrl/Cmd+P`
- **커밋 상세·diff**: 변경 파일 통계, unified/split 보기, syntax highlight(지연 로드), 큰 diff 는 나눠서 불러오기
- **WSL·SSH 환경**: 저장소 추가 대화상자, 환경별 그룹
- **자동 새로고침**: 외부에서 커밋·브랜치가 바뀌면 활성 저장소를 다시 읽는다
- **GitHub PR·이슈**: PAT 또는 Device Flow 로그인
- **테마**: 다크 · 라이트 · 시스템
- **언어**: 한국어 · English

## 단축키

| 키 | 동작 |
|---|---|
| `Ctrl/Cmd+P` | 저장소 전환 |
| `Ctrl/Cmd+F` | 커밋 검색 |
| `Enter` / `Shift+Enter` | 다음 / 이전 검색 일치 |
| `↑` `↓` `PageUp` `PageDown` `Home` `End` | 커밋 이동 |
| `Esc` | diff·GitHub 화면 닫기(상세 → 목록 → 그래프) |
| `Alt+↑` / `Alt+↓` | 이전 / 다음 파일(diff) |

## 설치·실행

요구 사항: Node 22+, pnpm 10+, git 2.36+(`worktree list -z`) — git 은 **각 환경(로컬·WSL·SSH 원격) 안에** 있어야 한다.

```bash
pnpm install          # 의존성 설치
pnpm dev              # 개발 모드(HMR)
pnpm build            # out/ 에 빌드
pnpm test             # 파서·executor·커서 등 단위 테스트(vitest)
pnpm typecheck        # main/preload/renderer 타입 검사
```

> pnpm 이 없으면 `corepack enable pnpm`(관리자 권한 필요) 또는 `npm i -g pnpm`.
> Electron 44 는 설치 시점이 아니라 **첫 실행 때** 바이너리를 내려받는다(`pnpm dev` 첫 실행이 조금 느릴 수 있다).

## 패키징

```bash
pnpm build && pnpm dist   # 현재 OS 용 설치 파일
pnpm dist:win             # Windows
pnpm dist:mac             # macOS
pnpm dist:linux           # Linux
pnpm dist:dir             # 설치 파일 없이 풀린 폴더만(빠른 확인용)
```

설정은 `electron-builder.yml`, 결과물은 `dist/` 에 생긴다.

| OS | 타깃 |
|---|---|
| Windows | NSIS 설치 프로그램 + portable (x64) |
| macOS | dmg(x64, arm64) — ad-hoc 서명(`identity: '-'`, hardened runtime 끔). 개발자 ID 서명·공증은 배포 계정 설정 후 추가 |
| Linux | AppImage + deb |

### 릴리스(CI)

`package.json` 의 `version` 과 같은 semver 태그(`v0.1.0`, 프리릴리스는 `v0.2.0-rc.1`)를 annotated 태그로 push 하면 `.github/workflows/release.yml` 이 Windows·macOS·Ubuntu 러너에서 설치본을 빌드해 GitHub Release 에 첨부한다. 릴리스 노트는 태그 메시지에서 가져온다. PR 은 `build.yml`(타입 검사·테스트·빌드·대비 검사)을 통과해야 main 에 squash 머지된다.

검증 상태:

- 확인함(이 Windows 머신): 풀린 빌드(`dist:dir`) 실행, NSIS x64 설치 파일 빌드(`Antler-Setup-0.1.0-x64.exe`, 약 110MB)
- 확인 못 함: macOS·Linux 빌드(해당 OS 필요), 코드 서명

아이콘은 `pnpm icons` 로 `resources/icon.svg`(48px 이상)와 `resources/icon-small.svg`(16–32px)에서
`resources/icon.png`·`icon.ico`·`icon.icns` 와 `resources/icons/*.png` 를 다시 만든다(`@resvg/resvg-js` + `png2icons`).

## GitHub 로그인

툴바의 GitHub 버튼에서 로그인한다. 로그인 없이도 공개 저장소는 읽을 수 있다(비인증 시간당 60회 제한).

- **PAT**: 읽기 전용 fine-grained 토큰을 권장한다(Contents·Pull requests·Issues 읽기). classic 은 `repo` 범위.
- **Device Flow**: GitHub OAuth App 을 만들어 **Enable Device Flow** 를 켜고, 그 client ID 를 GitHub 버튼 → **고급**에 입력한다.
- **토큰 보관**: Electron `safeStorage` 로 암호화해 userData 의 `github-token.bin` 에 저장하며 renderer 로는 넘기지 않는다.
  키링이 없는 Linux(`basic_text`)에서는 디스크에 쓰지 않고 메모리에만 둔다.
- GitHub Enterprise 는 지원하지 않는다.

## 원격 환경

**WSL** (Windows 전용)
- 배포판 목록은 `wsl -l -q` 로 읽는다.
- 경로는 `/home/…` 또는 `~/…`. `\\wsl.localhost\<배포판>\…` 경로는 WSL 저장소로 자동 변환한다.

**SSH**
- `~/.ssh/config` 의 `Host`·`HostName`·`User`·`Port`·`IdentityFile`·`ProxyJump`·`Include`·`UserKnownHostsFile`·`HostKeyAlias` 를 따른다.
- 인증은 키 파일 또는 ssh-agent 만. 비밀번호는 저장하지도 묻지도 않는다.
- 호스트 키가 이미 `known_hosts` 에 있어야 한다 — 처음이면 ssh CLI 로 한 번 접속해 둔다.
- 원격 로그인 셸은 POSIX 계열(sh/bash/zsh/fish)이어야 한다.
- `ProxyCommand` 와 호스트에 걸리는 `Match` 블록은 무시하지 않고 **거부**한다(잘못된 곳에 접속하는 것을 막기 위해).

## 성능

이 Windows 11 머신에서 `scripts/drive.mjs` 로 측정(콜드 스타트는 앱 프로세스 시작 기준).

| 항목 | 측정 | 목표 |
|---|---|---|
| 콜드 스타트 | 1.24s(이 저장소) / 1.80s(10만 커밋, loose ref 2,500개) | 2s |
| 유휴 메모리 | private 203–214MB | 300MB |
| 스크롤(10만 커밋) | p95 13.4ms | 60fps |

하드웨어 가속은 기본으로 끈다. Canvas2D·텍스트 위주 앱이라 GPU 프로세스 비용이 더 크다
(부하 후 private 506→280MB, p95 16.8→13.4ms). `ANTLER_GPU=1` 로 다시 켠다.

## 구조

```
src/
  shared/      types.ts(데이터 타입), ipc.ts(IPC 계약 — 채널·인자·결과 타입)
  main/
    executors/ GitExecutor 인터페이스, LocalExecutor, WslExecutor, SshExecutor, wslPaths,
               ssh/(config·connection·knownHosts·quote), processRunner, ProcessQueue(동시성 제한+dedupe)
    git/       commands.ts(refs·search·logArgs), logCursor.ts, parsers/(log, refs, worktree, stash, commit, changes, diff, nulRecords)
    github/    api, client(ETag 캐시), deviceFlow, tokenStore, remoteUrl, service, validate
    services/  ConfigStore(JSON 설정), RepoService(저장소 단위 진입점), RepoWatcher(외부 변경 감시), lru
    ipc/       handlers.ts(IpcEnvelope 로 감싸 등록), github.ts
  preload/     window.antler — 허용 목록 채널만 노출
  renderer/src/
    components/  Toolbar, Sidebar, CommitList, GraphCanvas, DetailPanel, DiffView, RepoSwitcher, SearchBar, AddRepoDialog, Splitter …
    components/github/  GitHubView, GhAuthButton, GhBits
    stores/      appStore(설정·활성 저장소·테마·refs), logStore(커밋 페이지·필터·찾아가기),
                 detailStore(커밋 상세·diff), githubStore, searchStore
    graph/       lanes.ts(레인 계산, 순수 함수), graphEngine.ts(worker 결과 보관)
    diff/        patch.ts(파싱), diffRows.ts(unified/split 행), highlight.ts(지연 로드)
    workers/     laneWorker.ts
    lib/         sanitize.ts, laneColor.ts, format.ts, api.ts
    styles/      tokens.css(디자인 토큰 3층), base.css, fonts.css
    i18n/        ko, en
resources/       앱 아이콘(icon.svg·icon-small.svg 원본, 생성된 png/ico/icns, icons/*.png)
electron-builder.yml  패키징 설정
scripts/         개발 도구 스크립트
tests/           vitest + fixtures/
```

### 핵심 설계

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
- **보안**
  - 창: contextIsolation · sandbox · nodeIntegration off, preload 는 허용 목록 채널만 노출, 권한 요청은 모두 거부,
    프로덕션 빌드에 CSP 메타 태그 주입, 외부 링크는 기본 브라우저로. 외부 HTML 은 sanitize 로 정화한다.
  - IPC: main 이 모든 입력(경로·해시·ref 등)을 검증한다.
  - 악성 저장소 방어: 모든 git 호출에 `GIT_NO_LAZY_FETCH=1`, `GIT_ALLOW_PROTOCOL=none`, `-c core.fsmonitor=false` 를 줘
    저장소 설정이 네트워크 접근이나 외부 명령 실행을 일으키지 못하게 한다.
  - 경로·diff: `GIT_LITERAL_PATHSPECS=1` 이라 renderer 가 보낸 경로가 pathspec 매직으로 해석되지 않고,
    `--no-textconv`·`--no-ext-diff` 로 저장소 설정이 외부 프로그램을 실행하지 못하게 한다.
- **디자인 토큰**: palette(원시) → semantic(테마가 교체하는 층) → component(semantic 만 참조) 3층.
  컴포넌트 CSS 는 색을 하드코딩하지 않는다. 테마는 `<html data-theme>` 의 토큰 값 교체로만 바뀐다.

## 디자인

- 사슴뿔과 숲에서 가져온 색 토큰을 palette → semantic → component 3층으로 둔다(`tokens.css`).
- 스타일은 평면(Fork/GitKraken식)이다.
- 폰트: UI 는 Pretendard Variable(앱에 번들, OFL 1.1). 코드는 GitHub 과 같은 시스템 고정폭 스택(`ui-monospace, SFMono-Regular, SF Mono, Menlo, Consolas, Liberation Mono`).

## 개발 도구 스크립트

| 스크립트 | 용도 |
|---|---|
| `node scripts/make-fixtures.mjs` | 파서 테스트용 git 출력 fixture 재생성(한글 경로·공백·rename·merge·바이너리) |
| `node scripts/make-big-repo.mjs <dir> [n]` | 성능 검증용 대형 저장소(기본 10만 커밋, feature 브랜치·merge 포함) 생성 |
| `node scripts/drive.mjs <repo...> [옵션]` | 빌드된 앱을 Playwright 로 띄워 콜드 스타트·사이드바·찾아가기·필터·검색·스크롤 프레임·메모리 측정 + 스크린샷 |
| `node scripts/bench-log.mjs <repo>` | git log 옵션 조합별 소요 시간 |
| `node scripts/check-contrast.mjs` | 디자인 토큰 WCAG 대비 검사(미달 시 exit 1) |
| `pnpm icons` (`scripts/gen-icons.mjs`) | `resources/` 아이콘 재생성 |
| `pnpm notices` (`scripts/gen-notices.mjs`) | renderer 번들 라이브러리 라이선스 고지를 `resources/licenses/THIRD_PARTY_NOTICES.txt` 로 재생성(의존성 갱신 후) |

`drive.mjs` 옵션(저장소는 로컬 경로 또는 `wsl:<배포판>:<경로>` / `ssh:<호스트>:<경로>`):

| 옵션 | 동작 |
|---|---|
| `--out <dir>` | 스크린샷 디렉터리 |
| `--theme dark\|light`, `--locale ko\|en` | 테마·언어 지정 |
| `--search <q>` | 검색 시나리오의 검색어(기본 `Merge branch`) |
| `--github` | GitHub PR 목록·상세·파일 탭 시나리오(첫 저장소의 origin 이 공개 GitHub 저장소일 때, 로그인 없이) |
| `--external-commit` | diff 를 연 채 첫 저장소에 빈 커밋을 만들어 자동 반영 확인 — **임시 저장소에만** 쓸 것 |
| `--idle-only <N>` | 조작 없이 N초 쉰 뒤 메모리 측정(유휴 메모리) |
| `--electron-args "<...>"` | 실험용 Chromium·Electron 스위치 전달 |
| `--exe <경로>` | 패키지된 실행 파일(예: `dist/win-unpacked/Antler.exe`)을 띄움 |
| `--inject-css <css>` | 실험용 CSS 주입 |
| `ANTLER_DRIVE_DEBUG=1` (환경 변수) | main 프로세스 출력 표시 |

`drive.mjs` 는 임시 userData(`ANTLER_USER_DATA`)를 쓰므로 실사용 설정을 건드리지 않는다.

## 의존성과 추가 이유

런타임 의존성(`dependencies`)은 **ssh2 하나**다. renderer 라이브러리는 번들에 들어가므로 devDependencies 에 둔다.

| 패키지 | 이유 |
|---|---|
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
| pretendard | UI 폰트 Pretendard Variable(OFL 1.1). woff2 하나만 renderer 자산으로 번들 |

## 알려진 한계

- 읽기 전용이다(commit·push 등 쓰기 작업 없음).
- GitHub Enterprise 는 지원하지 않는다.
- 원격 URL 이 바뀌어도 저장소를 다시 선택해야 GitHub 연동에 반영된다.
- SSH 원격의 로그인 셸이 cmd/PowerShell 이면 지원하지 않는다.
- macOS·Linux 패키지는 이 저장소 작업 환경에서 검증하지 못했다.
- 개발자 ID 코드 서명이 없다(Windows 는 SmartScreen, macOS 는 ad-hoc 서명이라 Gatekeeper 경고가 뜬다).

## 마일스톤

- [x] **M1** 스캐폴딩 + 디자인 토큰/테마 + LocalExecutor + 커밋 목록
- [x] **M2** 그래프 렌더링(레인 팔레트) + 브랜치/태그/워크트리 사이드바
- [x] **M3** 커밋 상세, 변경 파일 리스트, diff 뷰어
- [x] **M4** WslExecutor, SshExecutor + 환경 관리 UI
- [x] **M5** GitHub PR/이슈 연동
- [x] **M6** 아이콘/브랜딩, 성능 튜닝, 패키징

## 라이선스

앱 자체의 라이선스는 아직 정하지 않았다(LICENSE 파일 없음). 번들 폰트는 SIL OFL 1.1 이다.

- Pretendard — OFL 1.1

라이선스 고지는 `resources/licenses/`(Pretendard 폰트, 번들 라이브러리 `THIRD_PARTY_NOTICES.txt`)에 있고, 패키지에는 `resources/licenses/` 로 동봉된다. ssh2 와 그 의존 패키지는 `app.asar` 의 node_modules 에 각자의 LICENSE 와 함께 실린다.
