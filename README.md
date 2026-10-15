# Antler

Windows · WSL · SSH(원격 Ubuntu)의 git 저장소를 한 화면에서 살펴보는 가벼운 Git GUI.
사슴뿔(antler)처럼 갈라져 뻗는 브랜치 구조에서 이름을 따왔다. 읽기(조회) 전용이다.

> 현재 단계: **M3** — 커밋 상세·변경 파일 목록·diff 뷰어(unified/split, 지연 로딩, syntax highlight)

## 실행

```bash
pnpm install          # Node 22+, pnpm 10+, git 2.36+ (worktree list -z)
pnpm dev              # 개발 모드(HMR)
pnpm build            # out/ 에 빌드
pnpm test             # 파서·executor·커서 단위 테스트(vitest)
pnpm typecheck        # main/preload/renderer 타입 검사
```

> pnpm 이 없으면 `corepack enable pnpm`(관리자 권한 필요) 또는 `npm i -g pnpm`.
> Electron 44+ 는 설치 시점이 아니라 **첫 실행 때** 바이너리를 내려받는다(`pnpm dev` 첫 실행이 조금 느릴 수 있다).

### 개발 도구 스크립트

| 스크립트 | 용도 |
|---|---|
| `node scripts/make-fixtures.mjs` | 파서 테스트용 git 출력 fixture 재생성(한글 경로·공백·rename·merge·바이너리) |
| `node scripts/make-big-repo.mjs <dir> [n]` | 성능 검증용 대형 저장소(기본 10만 커밋, feature 브랜치·merge 포함) 생성 |
| `node scripts/drive.mjs <repo...> [--theme light] [--search q] [--out dir]` | 빌드된 앱을 Playwright 로 띄워 콜드 스타트·사이드바·찾아가기·필터·검색·스크롤 프레임·메모리 측정 + 스크린샷 |
| `node scripts/bench-log.mjs <repo>` | git log 옵션 조합별 소요 시간 |
| `node scripts/check-contrast.mjs` | 디자인 토큰 WCAG 대비 검사(미달 시 exit 1) |

`drive.mjs` 는 임시 userData(`ANTLER_USER_DATA`)를 쓰므로 실사용 설정을 건드리지 않는다.

## 구조

```
src/
  shared/      types.ts(데이터 타입), ipc.ts(IPC 계약 — 채널·인자·결과 타입)
  main/
    executors/ GitExecutor 인터페이스, LocalExecutor, processRunner, ProcessQueue(동시성 제한+dedupe)
    git/       commands.ts(refs·search·logArgs), logCursor.ts, parsers/(log, refs, worktree, stash, nulRecords)
    services/  ConfigStore(JSON 설정), RepoService(저장소 단위 진입점)
    ipc/       handlers.ts(IpcEnvelope 로 감싸 등록)
  preload/     window.antler — 허용 목록 채널만 노출
  renderer/src/
    components/  Toolbar, Sidebar, CommitList, DetailPanel, RepoSwitcher, Splitter …
    stores/      appStore(설정·활성 저장소·테마·refs), logStore(커밋 페이지·필터·찾아가기), searchStore
    graph/       lanes.ts(레인 계산, 순수 함수), graphEngine.ts(worker 결과 보관)
    workers/     laneWorker.ts
    styles/      tokens.css(디자인 토큰 3층), base.css
    i18n/        ko, en
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
- **커밋 상세·diff**: 커밋을 고르면(↑↓ 연타 대비 60ms 디바운스) 메타데이터와 첫 부모 대비 변경 파일(+/− 통계)을
  불러온다. 파일을 누르면 중앙이 diff 로 바뀐다(Esc 닫기, Alt+↑/↓ 이전/다음 파일). diff 는 3,000줄까지만 담아
  보내고 나머지는 "전체 불러오기"로 받는다. 상세·diff 는 커밋 해시 기준 불변 LRU 캐시(main)와 상세 캐시(renderer)에
  담는다. 모든 git 호출은 `GIT_LITERAL_PATHSPECS=1` 이라 renderer 가 보낸 경로가 pathspec 매직으로 해석되지 않고,
  `--no-textconv`·`--no-ext-diff` 로 저장소 설정이 외부 프로그램을 실행하지 못하게 한다.
- **사이드바 동작(GitKraken 식)**: 브랜치·태그 클릭 = 그래프에서 그 커밋으로 이동(필요하면 더 불러옴),
  더블클릭/깔때기 버튼 = 그 ref 만 보기(툴바 필터 칩으로 해제). 워크트리·스태시 클릭 = 그 HEAD 기준 보기.
- **보안**: contextIsolation · sandbox · nodeIntegration off, preload 는 허용 목록 채널만 노출,
  프로덕션 빌드에 CSP 메타 태그 주입, 외부 링크는 기본 브라우저로, 권한 요청은 모두 거부.
- **디자인 토큰**: palette(원시) → semantic(테마가 교체하는 층) → component(semantic 만 참조) 3층.
  컴포넌트 CSS 는 색을 하드코딩하지 않는다. 테마는 `<html data-theme>` 의 토큰 값 교체로만 바뀐다.

## 의존성과 추가 이유

| 패키지 | 이유 |
|---|---|
| react, react-dom | renderer UI |
| zustand | 보일러플레이트 없는 경량 상태관리(≈1KB) |
| @tanstack/react-virtual | 10만 행 가상 스크롤 |
| electron, electron-vite, vite, @vitejs/plugin-react | 앱 런타임과 main/preload/renderer 통합 빌드 |
| typescript, @types/* | strict 타입 검사 |
| vitest | 파서·executor 단위 테스트 |
| playwright-core (dev) | 빌드된 앱을 실제로 띄워 측정·스크린샷(`scripts/drive.mjs`) — 브라우저 다운로드 없는 core 판 |
| highlight.js | diff syntax highlight — core 와 필요한 언어만 diff 를 열 때 지연 로드(언어별 청크) |
| pretendard (dev) | UI 폰트 Pretendard Variable(OFL 1.1). woff2 하나만 renderer 자산으로 번들 |
| d2coding (dev) | 코드 폰트 D2Coding Regular/Bold 전체 한글 글리프(OFL 1.1). woff2 만 번들 |

## 마일스톤

- [x] **M1** 스캐폴딩 + 디자인 토큰/테마 + LocalExecutor + 커밋 목록
- [x] **M2** 그래프 렌더링(레인 팔레트) + 브랜치/태그/워크트리 사이드바
- [x] **M3** 커밋 상세, 변경 파일 리스트, diff 뷰어
- [ ] M4 WslExecutor, SshExecutor + 환경 관리 UI
- [ ] M5 GitHub PR/이슈 연동
- [ ] M6 아이콘/브랜딩, 성능 튜닝, 패키징
