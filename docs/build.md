# 빌드·패키징·개발 도구

[← README](../README.md)

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

버전의 기준은 git 태그 하나다. 저장소의 `package.json` 은 `0.0.0` 으로 고정해 두고, 릴리스 CI 가 빌드 직전에 태그 버전을 넣는다(커밋하지 않음) — 그래서 릴리스마다 버전을 올리는 커밋·PR 이 필요 없다. 로컬 빌드는 가장 가까운 태그로 `<태그>-dev`(예: `0.2.0-dev`)를 버전으로 쓴다. semver 태그(`v0.1.0`, 프리릴리스는 `v0.2.0-rc.1`)를 annotated 태그로 push 하면 `.github/workflows/release.yml` 이 Windows·macOS·Ubuntu 러너에서 설치본을 빌드해 GitHub Release 에 첨부한다. 릴리스 노트는 태그 메시지에서 가져온다. PR 은 `build.yml`(타입 검사·테스트·빌드·대비 검사)을 통과해야 main 에 squash 머지된다.

**야간 자동 릴리스** — `.github/workflows/nightly-release.yml` 이 매일 00:00(KST) 마지막 정식 태그 이후 main 의 새 커밋을 본다. squash 머지라 커밋 제목이 곧 PR 제목이므로 **PR 제목을 Conventional Commits 로 쓴다**.

| PR 제목 | 다음 버전 |
|---|---|
| `feat: …` | minor (1.0.0 → 1.1.0) |
| `fix: …` · `perf: …` · `style: …` | patch (1.0.0 → 1.0.1) |
| `feat!: …` 또는 본문에 `BREAKING CHANGE:` | major |
| `ci:` · `docs:` · `chore:` · `test:` · `refactor:` 만 있음 | 릴리스하지 않음 |

릴리스할 변경이 있으면 같은 실행 안에서 `release.yml` 을 재사용 워크플로(`workflow_call`)로 불러 세 OS 빌드 → (모두 성공하면) 커밋을 종류별로 묶은 노트로 annotated 태그 → Release 발행까지 한다. 빌드가 실패하면 태그도 남지 않으며, 실패한 잡만 다시 실행하면 된다. Actions 탭에서 수동 실행할 수도 있다(올림 단계 지정, `dry_run` 으로 계산만). 로컬 점검: `node scripts/next-version.mjs [--since vX.Y.Z]`.

검증 상태:

- 확인함(이 Windows 머신): 풀린 빌드(`dist:dir`) 실행, NSIS x64 설치 파일 빌드(`Antler-Setup-0.1.0-x64.exe`, 약 110MB)
- 확인 못 함: macOS·Linux 빌드(해당 OS 필요), 코드 서명

아이콘은 `pnpm icons` 로 `resources/icon.svg`(48px 이상)와 `resources/icon-small.svg`(16–32px)에서
`resources/icon.png`·`icon.ico`·`icon.icns` 와 `resources/icons/*.png` 를 다시 만든다(`@resvg/resvg-js` + `png2icons`).

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
| `--wip` | 작업 트리(WIP) 행·변경 패널·diff 시나리오 — 첫 저장소가 더럽혀져 있어야 한다(**임시 저장소에만** 쓸 것) |
| `--update-fake <상태>` | 업데이트 스낵바 상태(`available|downloading|downloaded|error`)를 띄워 `update-<상태>-<테마>.png` 만 남기고 종료(`--update-mode manual-dmg|manual-file`) |
| `--idle-only <N>` | 조작 없이 N초 쉰 뒤 메모리 측정(유휴 메모리) |
| `--electron-args "<...>"` | 실험용 Chromium·Electron 스위치 전달 |
| `--exe <경로>` | 패키지된 실행 파일(예: `dist/win-unpacked/Antler.exe`)을 띄움 |
| `--inject-css <css>` | 실험용 CSS 주입 |
| `ANTLER_DRIVE_DEBUG=1` (환경 변수) | main 프로세스 출력 표시 |

`drive.mjs` 는 임시 userData(`ANTLER_USER_DATA`)를 쓰므로 실사용 설정을 건드리지 않는다.
