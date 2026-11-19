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

### Tauri 판 패키징(발행 없음 — Phase 6c PR 2)

`.github/workflows/tauri-package.yml` 이 Tauri 판 설치본을 빌드해 **워크플로 아티팩트로만** 올린다. 릴리스·태그를 만들지 않고 쓰기 권한도 없으며, `release.yml`·야간 릴리스와 이어져 있지 않다(야간 릴리스는 계속 Electron 판을 낸다 — 전환은 마지막 `feat!:` PR 의 몫, [tauri.md](tauri.md#pr-2--tauri-패키징-워크플로발행-없음)).
산출물은 **배포 후보다**: Phase 6c PR 5 에서 Node 사이드카를 없애 Tauri 판은 Node 런타임에 기대지 않는다(발행은 리허설·마지막 `feat!:` PR 이후).

실행: Actions 탭 → `tauri-package` → Run workflow(선택 입력 `version`, 비우면 `0.0.0-ci.<실행 번호>`). 워크플로 파일이 기본 브랜치에 있어야 실행되므로 PR 에서는 돌지 않는다.

| 갈래 | 러너 | 산출물(아티팩트 `tauri-installers-<갈래>`) |
|---|---|---|
| windows | `windows-latest` | `Antler-Setup-<버전>-x64.exe`(NSIS, 사용자별 설치) · 서명 키가 있으면 `.exe.sig`·`latest.json` · `latest.yml` (서명·매니페스트는 `sign` 잡) |
| mac-arm64 · mac-x64 | `macos-latest`(arm64 러너에서 둘 다 빌드) | `Antler-<버전>-arm64.dmg` · `Antler-<버전>-x64.dmg` (ad-hoc 서명 `-`) |
| linux | `ubuntu-22.04` | `antler_<버전>_amd64.deb` |

이름은 Electron 판의 규칙을 그대로 따른다(앱 안 수동 업데이트가 자산을 고르는 규칙, `pickAsset`).

**업데이트 서명** — 빌드는 키 없이 한다. `windows` 잡이 시크릿 없이 NSIS 를 빌드해 서명 전 설치본을 중간 아티팩트(`tauri-win-unsigned`, 1일 보관)로 올리고, 이어서 `sign` 잡(Environment `release`)이 그것과 macOS·Linux 빌드 잡(`unix`)의 dmg·deb 를 받아 `tauri signer sign --app-version <버전>` 으로 각각 `.sig` 를 만들고 매니페스트를 만든다(`unix` 의 일부 갈래가 실패해도 있는 것만 서명하고 없는 것은 경고한다). 키(`TAURI_SIGNING_PRIVATE_KEY`·`_PASSWORD`)는 서명 단계의 env 에만 있다 — 빌드 중에 도는 npm 빌드 코드·모든 crate 의 `build.rs`·proc-macro 가 키를 읽을 수 없게 하려는 분리다. `sign` 은 새 러너에서 `pnpm install --frozen-lockfile --ignore-scripts` 로 Tauri CLI 만 설치해 쓰며(Rust·빌드 없음), 남는 신뢰는 lockfile 이 고정한 `@tauri-apps/cli` 의 코드뿐이다(`cargo install tauri-cli` 로 바꾸면 더 줄지만 컴파일이 길다). 시크릿이 없으면 경고(`::warning::`)를 내고 서명을 건너뛴다 — `latest.json` 만 빠진다. macOS dmg·Linux deb 의 앱 안 업데이트는 **수동 경로**(Phase 6c PR 4b — 앱이 검증한 파일을 받아 열어 준다)라 설치본 옆의 `.sig` 가 그 무결성이고, 서명이 없는 자산은 `latest.json` 에 들어가지 않는다(그 플랫폼은 업데이트 알림을 받지 못한다).

**필수 사전 설정(저장소 Settings > Environments > `release`)** — 시크릿을 넣기 전에 먼저 한다.
- Deployment branches and tags: **Selected branches and tags → `main` 만**. 태그를 허용해야 한다면 tag ruleset 으로 `v*` 태그 생성을 제한한다.
- Required reviewers: 소유자 1명.
- 이유: `workflow_dispatch` 는 다른 브랜치의 워크플로 파일로도 실행할 수 있고, Environment 의 시크릿은 그 파일로 전달된다. 파일 안의 `if:` 가드는 공격자가 고친 파일에서는 무효이므로 방어가 되지 않는다 — 방어는 Environment 쪽 설정뿐이다.
키는 사용자가 만들어 시크릿으로 넣고, **공개 키는 아직 저장소에 없다**(사용자 공개 키 대기). 공개 키는 `src-tauri/src/update/host.rs` 의 `UPDATE_PUBKEY` 한 곳에 넣는다(Phase 6c PR 4 가 업데이터를 앱에 연결했고 `tauri.conf.json` 에는 `plugins` 를 두지 않는다 — [updates.md](updates.md#tauri-판-phase-6c-pr-4)). 상수가 `None` 인 동안 앱은 업데이트를 확인만 하고 내려받기·설치는 거부한다.

**매니페스트** — `scripts/make-update-manifests.mjs` 가 만든다(순수 함수 + `tests/updateManifests.test.ts`).
- `latest.json`: Tauri 업데이터 형식. `url` 은 `https://github.com/Kang-Jacob-GitLB/antler/releases/download/v<버전>/<파일>`, 플랫폼 키는 Windows 가 `windows-x86_64-nsis` 와 `windows-x86_64`(같은 항목), 수동 경로가 `darwin-aarch64-dmg`·`darwin-x86_64-dmg`·`linux-x86_64-deb`(각각 `Antler-<버전>-arm64.dmg`·`Antler-<버전>-x64.dmg`·`antler_<버전>_amd64.deb` — 앱이 **그 키 하나만, 그 이름과 정확히 같을 때만** 받는다)다. 앱의 `check_manifest_for`(`update/logic.rs`)가 읽는 형식이고, 실제 검사가 표본 `tests/fixtures/update/latest.json`·`latest-all.json` 을 받는지 `src-tauri/tests/update.rs` 가 확인한다. 서명(`.sig`)이 있는 설치본만 들어간다.
- `latest.yml`: Electron 판(electron-updater)이 Tauri 설치본을 받아 가게 하는 이관용. `url`·`path` 는 **파일 이름만**이다(절대 URL 이면 electron-updater 가 경로를 깨뜨린다). 서명과 무관하게 만든다.
- **비상 정지**: 저장소 변수 `ELECTRON_MIGRATION` 이 `off`·`false`·`0`·`no`(대소문자 무시)면 `latest.yml` 을 만들지 않는다(기본 on). 이미 발행된 릴리스에서는 사람이 `latest.yml` 자산도 지워야 한다.
- 스크립트가 파일을 쓰기 전에 버전·파일 이름·해시·크기를 실물과 대조하고, 어긋나면 워크플로가 실패한다.

**배포 빌드 가드** — 빌드 뒤 `scripts/check-release-features.mjs` 가 바이너리에 `cdp`·`test-hooks` feature 의 표식 문자열이 있는지 본다(있으면 실패). 워크플로는 `--features` 를 쓰지 않는다. "없음" 검사라 표식이 사라지면 조용히 통과하는 한계가 있다 — cdp 표식은 실제 cdp 빌드 바이너리에서 잡히는 것을 확인했고, test-hooks 표식이 릴리스 프로파일에서 살아남는지는 확인하지 못했다.

**macOS — 미서명·미공증(Gatekeeper)** — 개발자 ID 인증서가 없어 ad-hoc 서명만 하고 공증하지 않는다. 내려받은 dmg 의 앱을 처음 열면 "확인되지 않은 개발자" 또는 "손상되었기 때문에 열 수 없음" 경고가 뜰 수 있다. 릴리스 노트 초안 메모(문구는 실기기에서 확인 필요):
> 이 앱은 Apple 개발자 인증서로 서명·공증되지 않았습니다. 처음 열 때 막히면 **시스템 설정 → 개인정보 보호 및 보안**에서 "그래도 열기"를 누르거나, 터미널에서 `xattr -dr com.apple.quarantine /Applications/Antler.app` 을 실행하세요.

**Linux — deb 의존** — Tauri 번들러가 `libwebkit2gtk-4.1-0`(WebKitGTK 4.1)·`libgtk-3-0` 를 `Depends` 로 기록하고, git 은 `Recommends` 로 둔다(`tauri.conf.json`). 따라서 WebKitGTK 4.1 이 있는 배포판(Ubuntu 22.04+ · Debian 12+)이 필요하다. 빌드 러너는 `libwebkit2gtk-4.1-dev` 등을 설치하며, 실제 `Depends` 는 실행 요약(deb 메타데이터)에서 확인한다.

**머지 뒤 사람 확인** — (1) Actions 에서 `tauri-package` 를 한 번 실행해 세 갈래가 끝나는지, 아티팩트 이름·크기를 본다(**macOS·Linux 는 Tauri 판을 한 번도 컴파일한 적이 없어 첫 실행에서 빌드가 실패할 수 있다** — 실패하면 갈래별 로그를 본다). (2) 위 "필수 사전 설정"(Environment `release` 의 브랜치 제한·승인자)을 한 뒤 서명 키를 시크릿에 등록하고 다시 실행해 `.sig`·`latest.json` 이 생기고 시크릿이 `sign` 잡에 실제로 전달되는지 본다(승인 대기가 뜨는지 포함). 이관 훅(NSIS)의 실제 설치·제거는 리허설 R 에서 확인한다.

아이콘은 `pnpm icons` 로 `resources/icon.svg`(48px 이상)와 `resources/icon-small.svg`(16–32px)에서
`resources/icon.png`·`icon.ico`·`icon.icns` 와 `resources/icons/*.png` 를 다시 만든다(`@resvg/resvg-js` + `png2icons`).

## 개발 도구 스크립트

| 스크립트 | 용도 |
|---|---|
| `node scripts/make-fixtures.mjs` | 파서 테스트용 git 출력 fixture 재생성(한글 경로·공백·rename·merge·바이너리) |
| `node scripts/make-edge-fixtures.mjs` | 골든 테스트용 경계 입력(`tests/fixtures/edge`) 재생성. 골든 갱신은 `UPDATE_GOLDEN=1 npx vitest run tests/golden.test.ts`([tauri.md](tauri.md)) |
| `node scripts/make-big-repo.mjs <dir> [n]` | 성능 검증용 대형 저장소(기본 10만 커밋, feature 브랜치·merge 포함) 생성 |
| `node scripts/drive.mjs <repo...> [옵션]` | 빌드된 앱을 Playwright 로 띄워 콜드 스타트·사이드바·찾아가기·필터·검색·스크롤 프레임·메모리 측정 + 스크린샷 |
| `node scripts/bench-log.mjs <repo>` | git log 옵션 조합별 소요 시간 |
| `node scripts/check-contrast.mjs` | 디자인 토큰 WCAG 대비 검사(미달 시 exit 1) |
| `pnpm icons` (`scripts/gen-icons.mjs`) | `resources/` 아이콘 재생성 |
| `pnpm notices` (`scripts/gen-notices.mjs`) | renderer 번들 라이브러리 라이선스 고지를 `resources/licenses/THIRD_PARTY_NOTICES.txt` 로 재생성(의존성 갱신 후) |
| `pnpm tauri:notices` (`scripts/gen-tauri-notices.mjs [--target <triple>]`) | Tauri 판 고지 `src-tauri/gen/licenses/THIRD_PARTY_NOTICES.txt`(커밋 안 함) 생성: renderer npm 라이브러리 + `cargo metadata --locked --filter-platform` 정상 의존 트리의 Rust 크레이트(이름·버전·license·라이선스 파일 본문, 같은 본문은 한 번만). `bundle.resources` 가 폰트 라이선스와 함께 설치본의 `licenses/` 에 싣는다. `tauri build` 의 beforeBuildCommand 와 `tauri-package.yml` 빌드 앞 단계가 돌린다. 앱 안에서 고지를 여는 메뉴는 Electron·Tauri 판 모두 없다(설치 폴더의 `licenses/`). 파일 없이 `cargo check` 하면 `build.rs` 가 자리 표시 파일을 둔다 |

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
| `--tauri <exe>` | Electron 대신 Tauri 판(`pnpm tauri:build:cdp` 산출물 `src-tauri/target/release/antler.exe`)을 같은 시나리오로 측정. `--gpu` 로 하드웨어 가속을 켠다(기본 끔). 끝에 OS 프로세스 트리 메모리(`process tree`)를 두 판 같은 방식으로 출력 — [docs/tauri.md](tauri.md#phase-0-측정) |
| `--inject-css <css>` | 실험용 CSS 주입 |
| `ANTLER_DRIVE_DEBUG=1` (환경 변수) | main 프로세스 출력 표시 |

`drive.mjs` 는 임시 userData(`ANTLER_USER_DATA`)를 쓰므로 실사용 설정을 건드리지 않는다.
