# Antler

Windows · WSL · SSH(원격 Ubuntu)의 git 저장소를 한 화면에서 살펴보는 가벼운 Git GUI.
사슴뿔(antler)처럼 갈라져 뻗는 브랜치 구조에서 이름을 따왔다. 읽기(조회) 전용이다.

> 현재 단계: **M1** — 스캐폴딩, 디자인 토큰/테마, LocalExecutor, 커밋 목록

## 실행

```bash
pnpm install          # Node 22+, pnpm 10+
pnpm dev              # 개발 모드(HMR)
pnpm build            # out/ 에 빌드
pnpm test             # 파서·executor·커서 단위 테스트(vitest)
pnpm typecheck        # main/preload/renderer 타입 검사
```

> pnpm 이 없으면 `corepack enable pnpm`(관리자 권한 필요) 또는 `npm i -g pnpm`.
> pnpm 10 은 설치 스크립트를 기본 차단하므로 `package.json` 의 `pnpm.onlyBuiltDependencies` 에 electron·esbuild 를 허용해 두었다.
> 그래도 `node_modules/electron/dist` 가 비어 있으면 `node node_modules/electron/install.js` 를 실행한다.

### 개발 도구 스크립트

| 스크립트 | 용도 |
|---|---|
| `node scripts/make-fixtures.mjs` | 파서 테스트용 git 출력 fixture 재생성(한글 경로·공백·rename·merge·바이너리) |
| `node scripts/make-big-repo.mjs <dir> [n]` | 성능 검증용 대형 저장소(기본 10만 커밋, feature 브랜치·merge 포함) 생성 |
| `node scripts/drive.mjs <repo...> [--theme light] [--out dir]` | 빌드된 앱을 Playwright 로 띄워 콜드 스타트·스크롤 프레임·메모리 측정 + 스크린샷 |
| `node scripts/bench-log.mjs <repo>` | git log 옵션 조합별 소요 시간 |
| `node scripts/check-contrast.mjs` | 디자인 토큰 WCAG 대비 검사(미달 시 exit 1) |

`drive.mjs` 는 임시 userData(`ANTLER_USER_DATA`)를 쓰므로 실사용 설정을 건드리지 않는다.

## 구조

```
src/
  shared/      types.ts(데이터 타입), ipc.ts(IPC 계약 — 채널·인자·결과 타입)
  main/
    executors/ GitExecutor 인터페이스, LocalExecutor, processRunner, ProcessQueue(동시성 제한+dedupe)
    git/       commands.ts, logCursor.ts, parsers/(log, nulRecords)
    services/  ConfigStore(JSON 설정), RepoService(저장소 단위 진입점)
    ipc/       handlers.ts(IpcEnvelope 로 감싸 등록)
  preload/     window.antler — 허용 목록 채널만 노출
  renderer/src/
    components/  Toolbar, Sidebar, CommitList, DetailPanel, RepoSwitcher, Splitter …
    stores/      appStore(설정·활성 저장소·테마), logStore(커밋 페이지)
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

## 마일스톤

- [x] **M1** 스캐폴딩 + 디자인 토큰/테마 + LocalExecutor + 커밋 목록
- [ ] M2 그래프 렌더링(레인 팔레트) + 브랜치/태그/워크트리 사이드바
- [ ] M3 커밋 상세, 변경 파일 리스트, diff 뷰어
- [ ] M4 WslExecutor, SshExecutor + 환경 관리 UI
- [ ] M5 GitHub PR/이슈 연동
- [ ] M6 아이콘/브랜딩, 성능 튜닝, 패키징
