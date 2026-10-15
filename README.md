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

## 업데이트

패키지된 앱은 시작 10초 뒤와 이후 6시간마다 GitHub Releases 에서 새 버전을 확인한다. 툴바의 (i) 메뉴 → "업데이트 확인" 으로 직접 확인할 수도 있다. 새 버전이 있으면 화면 아래에 스낵바가 뜨고, 자동으로 내려받거나 설치하지 않는다 — [업데이트] 를 눌러야 시작한다. [나중에] 는 앱을 다시 시작할 때까지, [이 버전 건너뛰기] 는 그 버전을 계속 숨기고(설정 `skippedUpdateVersion`) 더 새 버전이 나오면 다시 알린다. 프리릴리스(`-rc.1` 등)는 현재 버전이 프리릴리스일 때만 제안한다.

| 설치 형태 | 방식 |
|---|---|
| Windows NSIS 설치본, Linux AppImage | electron-updater(GitHub 공급자, `latest.yml`/`latest-linux.yml` 의 sha512 검증). 내려받은 뒤 [다시 시작하여 설치] → `quitAndInstall` |
| macOS (dmg) | 앱이 ad-hoc 서명이라 Squirrel.Mac 의 자동 교체가 안정적이지 않다. 그래서 릴리스에서 `process.arch` 에 맞는 dmg 를 내려받아 해시를 검증한 뒤 앱 전용 폴더(userData/updates)에 저장하고 열어 준다 — 열린 창에서 Antler 를 Applications 로 끌어 넣으면 된다 |
| Windows portable, Linux deb | portable exe 또는 deb 를 같은 방식으로 검증해 내려받는다. deb 는 소프트웨어 센터로 열고, portable 은 파일 위치를 보여 준다 |

보안: 내려받기는 `github.com` / `objects.githubusercontent.com` 의 https 릴리스 자산만(첫 URL 은 이 저장소의 `releases/download/` 경로로 정규화해 확인하고, 리다이렉트도 매번 허용 목록 확인). 검증은 `latest*.yml` 의 sha512 → 없으면 GitHub API 의 sha256 digest 이며, **둘 다 없으면 크기만으로 통과시키지 않고 내려받지도 열지도 않는다**(portable exe 는 yml 에 없어 API digest 에 의존). 받은 파일은 `userData/updates`(0700)에 `.part` 로 받아 검증 뒤 이름을 바꾸고, 다시 열기 전에 해시를 한 번 더 확인한다. 검증에 실패한 파일은 지우고 열지 않는다. "건너뛴 버전" 설정은 main 만 바꾼다(renderer 의 `config:setUi` 로는 불가, `update:dismiss` 는 지금 알리는 버전만 받는다). 토큰은 쓰지 않는다(공개 저장소 · 비인증 한도). Linux 설치 형태는 환경 변수만 믿지 않고 `resources/package-type`(deb) 또는 `APPDIR` 안에서 실행 중인지(AppImage)로 판별하며, 그 밖은 업데이트하지 않는다.

**Windows 업데이트는 코드 서명이 없다**(`publisherName` 없음 — 앱이 미서명이라 electron-updater 가 Authenticode 검증을 건너뛴다). 무결성은 https 로 받은 릴리스 메타데이터(`latest.yml`)의 sha512 에만 의존한다. CI 는 `permissions: contents: read`(release 잡만 쓰기)와 SHA 로 고정한 액션을 쓴다.

주의: 업데이트 기능이 없는 0.1.0 은 스스로 업데이트할 수 없다. 이 기능이 들어간 **다음 릴리스를 한 번 수동 설치**해야 그 이후 릴리스부터 앱 안에서 업데이트된다.

개발·테스트: `ANTLER_UPDATE_FAKE=available|downloading|downloaded|error`(+ `ANTLER_UPDATE_FAKE_MODE=manual-dmg|manual-file`)로 스낵바 상태를 띄운다(미패키지 실행 또는 `ANTLER_UPDATE_DEV=1` 일 때만). `node scripts/drive.mjs . --update-fake <상태> --out out/shots-update` 로 스크린샷을 남긴다. `ANTLER_UPDATE_DEV=1` + `ANTLER_UPDATE_FEED=<url>` 이면 미패키지 실행에서 generic 피드로 auto 경로를 시험할 수 있다.

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
    update/    logic(semver·자산 선택·해시 검증·정책), manual(GitHub Releases 검증 다운로드), UpdateService(상태 기계), autoBackend(electron-updater)
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
  - fetch: 저장소에 하는 유일한 쓰기이자 유일한 네트워크 접근이다(remote 마다 `git fetch --prune --no-prune-tags --no-recurse-submodules --no-write-fetch-head --refmap= -- <이름> +refs/heads/*:refs/remotes/<이름>/*`).
    **로컬 ref 는 쓰지 않는다**: 설정의 `remote.<n>.fetch` 를 쓰는 `fetch --all` 은 목적지를 설정이 정해 `refs/heads`·`refs/stash`·`refs/replace`·`HEAD` 를 덮어쓰고
    `--prune` 이 로컬 브랜치를, `prunetags` 가 태그를 지울 수 있었다(실측). 그래서 목적지를 `refs/remotes/<이름>/` 로 고정한 refspec 을 명령줄에 주고, 설정으로 하는
    "기회적 원격 추적 갱신"은 `--refmap=`(빈 값)으로 끈다(이게 없으면 명령줄 refspec 을 줘도 설정이 고른 목적지에 쓴다 — git 2.49 실측). 태그는 새 것만 따라오고 지우거나 덮지 않는다.
    `GIT_NO_REPLACE_OBJECTS=1` 을 모든 git 호출에 줘서 `refs/replace` 로 위조한 이력을 보여 주지 않는다.
    활성 저장소를 N분마다(설정 `ui.autoFetchMinutes`, 기본 5, 새로고침 옆 ▾ 메뉴에서 끔/1/5/15/30분) 자동으로, 새로고침 버튼을 누를 때도 가져온다.
    fetch 에만 `GIT_ALLOW_PROTOCOL=https:http:ssh:git`(ext·fd·원격 도우미 제외; `file` 은 수동 새로고침에서 git ≥ 2.45.1 일 때만, CVE-2024-32004),
    `GIT_TERMINAL_PROMPT=0`, 자격 증명 창 금지, `gc.auto=0`·`maintenance.auto=false`, NTLM/Negotiate 통합 인증 끄기
    (`http.emptyAuth=false`·`http.allowNTLMAuth=false`·`http.delegation=none`)를 준다. 저장소 훅은 `core.hooksPath` 를 빈 디렉터리로 돌려 막는다 —
    Windows 로컬 git 은 `/dev/null` 이 `<드라이브>:\dev\null` 로 풀려 공격자가 훅을 심을 수 있어서 앱이 소유한 `userData/no-hooks`(비어 있는지 매번 확인)를 쓴다.
    ssh 는 항상 `GIT_SSH_COMMAND` 를 명시해(저장소의 `core.sshCommand` 는 적용되지 않는다) 사용자의 시스템·전역 `core.sshCommand`,
    없으면 `GIT_SSH_COMMAND`/`GIT_SSH` 환경변수, 그것도 없으면 `BatchMode=yes`·`ConnectTimeout=15` 로 실행한다. 60초 제한, 저장소당 하나만, 환경당 전용 슬롯 하나.
    **악성 설정 차단(허용 목록)**: fetch 전에(그리고 실행 직전에 한 번 더) `git config --list --show-scope --includes -z` 로 local/worktree 설정(include 한 파일 포함)을 검사한다.
    git 과 Git Credential Manager 같은 도구가 명령을 실행하거나 연결을 열거나 파일을 쓰는 설정을 계속 추가해 차단 목록은 번번이 새 키를 놓쳤으므로,
    **읽기만 하는 데이터인 키만 통과시키는 허용 목록**을 쓴다(`src/main/git/fetch.ts`의 `ALLOWED_KEYS`: 저장소 형식·플래그 `core.*`, `remote.<n>.url|fetch|…`,
    `branch.<n>.remote|merge|…`, `user.*`, `pull/push/fetch.prune` 류, `extensions.*`, `submodule.<n>.url|active`, `include.path`, git-lfs 기본값 그대로인 `filter.lfs.*`).
    그 밖의 키(`http.*`·`credential.*`·`protocol.*`·`url.*`·`trace2.*`·`fetch.bundleURI`·`core.sshCommand` 등 전부)는 "알 수 없는 설정"으로 자동 fetch 를 막고
    "fetch 차단됨"으로 표시한다. 새로고침을 누르면 해당 키·값을 이스케이프해서(제어·서식·방향·결합·닮은꼴 따옴표·공백처럼 보이는 문자를 \uXXXX 로 노출, 긴 값은 앞…뒤+길이) 보여 주는
    확인 창이 뜨고, 항목이 12개를 넘으면 허용 버튼 없이 차단만 알린다. 허용해도 이번 실행 동안 같은 설정에만 적용된다.
    허용 목록 외에도 fetch 명령에는 `-c fetch.bundleURI= -c transfer.bundleURI=false -c http.followRedirects=initial` 등을 덮어써 이중으로 막는다.
    (renderer 가 `manual=true` 로 호출하면 확인 창이 뜰 수 있다 — 침해된 renderer 가 수동 fetch 를 요청할 수는 있지만, 알 수 없는 설정의 허용은 항상 main 이 띄우는 확인 창을 사용자가 눌러야 한다.)
    모든 remote 의 실제 URL(`git remote get-url --all`, 옛 `.git/remotes`·`.git/branches`, submodule URL 포함)이 `ext::`·`fd::`·`<도우미>::`·`-` 로 시작하거나
    UNC·네트워크 경로(`//host/…`, `\\host\share`, `\\?\UNC\…`, 호스트가 있는 `file://host/…`)면 확인 없이 거부한다(Windows 가 NTLM 을 보내므로 모든 플랫폼에서).
    userinfo 가 든 http(s) remote URL(`http://user@host/`)은 NTLM/Negotiate 가 새므로 자동 fetch 를 막고 수동 확인을 받는다(비밀번호는 가려 표시).
    **자격 증명 도우미**: 자동 fetch 는 `-c credential.helper=` 로 도우미를 끄고, 사용자가 같은 remote(이름+실제 URL)를 수동 새로고침으로 한 번 성공시킨 뒤에만
    도우미를 쓴다(승인은 main 이 설정 파일의 `fetchApprovals` 에 저장하며 renderer 는 바꿀 수 없다, URL 이 바뀌면 사라진다). 그래서 비공개 https 저장소는
    "새로고침을 한 번 누르면 이후 자동으로 가져옵니다".
    인증 실패·허용 안 된 전송은 수동 새로고침 전까지 자동 fetch 를 멈추고, 네트워크 실패는 지수 백오프(최대 30분)로 다시 시도한다.
    git 프로세스의 stderr 는 4MB 까지만 담는다.
    **알려진 한계(fetch 가드로 막을 수 없는 것)**:
    (1) 저장소의 `include.path`·`core.worktree` 가 UNC 경로를 가리키면 설정 목록을 읽는 것을 포함해 **어떤 git 명령이든** 그 경로를 건드린다 —
    공격자가 설정을 정할 수 있는 저장소를 여는 일반적인 한계다(Antler 는 네트워크 공유 저장소 경로 자체는 거부한다).
    (2) 사용자가 "알 수 없는 설정" 확인 창에서 허용하면, 저장소의 `http.<url>.*` 같은 키가 우리가 `-c` 로 준 기본값을 덮을 수 있다 — 허용은 사용자의 결정이다.
    (3) renderer 가 주장하는 `manual` 값은 main 이 검증할 수 없다. 침해된 renderer 가 수동 fetch 를 요청해도 알 수 없는 설정은 main 의 확인 창에서 사용자가 눌러야 한다.
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
| `--update-fake <상태>` | 업데이트 스낵바 상태(`available|downloading|downloaded|error`)를 띄워 `update-<상태>-<테마>.png` 만 남기고 종료(`--update-mode manual-dmg|manual-file`) |
| `--idle-only <N>` | 조작 없이 N초 쉰 뒤 메모리 측정(유휴 메모리) |
| `--electron-args "<...>"` | 실험용 Chromium·Electron 스위치 전달 |
| `--exe <경로>` | 패키지된 실행 파일(예: `dist/win-unpacked/Antler.exe`)을 띄움 |
| `--inject-css <css>` | 실험용 CSS 주입 |
| `ANTLER_DRIVE_DEBUG=1` (환경 변수) | main 프로세스 출력 표시 |

`drive.mjs` 는 임시 userData(`ANTLER_USER_DATA`)를 쓰므로 실사용 설정을 건드리지 않는다.

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

## 알려진 한계

- 읽기 전용이다(commit·push 등 쓰기 작업 없음). 예외는 원격 추적 ref·객체를 갱신하는 `git fetch` 하나뿐이다.
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
