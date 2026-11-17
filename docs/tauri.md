# Tauri 2 전환

[← README](../README.md)

Electron 판을 Tauri 2 로 옮기는 작업의 계획과 현재 상태다. 배포되는 앱은 아직 Electron 판뿐이다.

## 결정 사항

- **동기는 백엔드를 Rust 로 옮기는 것 자체**다(크기·메모리가 아니다). 성능은 Electron 판보다 나빠지지 않았는지만 본다.
- **점진 이관**: `src/main`(Node)을 영역별로 Rust 로 옮긴다. 이관 중에는 아직 안 옮긴 채널을 개발 전용 Node 사이드카가 받고,
  **배포는 Node 가 완전히 빠진 뒤에만** 한다.
- Windows **portable 배포는 없앤다**(Tauri 번들러에 portable 타깃이 없다). NSIS 설치본만 낸다.
- **창 컨트롤은 앱이 직접 그린다.** Tauri 에는 Electron `titleBarOverlay` 에 해당하는 기능이 없다.
  Win11 스냅 레이아웃은 최대화 버튼에 620ms 머물면 Win+Z 를 보내 띄운다(`src-tauri/src/snap.rs`, 창에 포커스가 있을 때만).
  처음에는 `tauri-plugin-decorum` 으로 살리기로 했으나, 1.1.1 은 Tauri 2.12 에서 창 버튼이 주입되지 않아(페이지 로드 뒤에 `DOMContentLoaded` 리스너를 단다)
  같은 방식을 직접 구현했다. **플라이아웃이 실제로 뜨는지는 아직 사람이 확인하지 않았다**(입력 합성을 하지 않아 자동 확인 불가).

## 단계

| 단계 | 내용 | 상태 |
|---|---|---|
| 0 | renderer 를 그대로 Tauri 에 올리고(사이드카 중계) 창 컨트롤·측정 경로 확인 | 진행 중 — 화면 동작 확인, 성능 비교 남음 |
| 1 | 측정 자동화(`drive.mjs --tauri`), 창 상태·테마·폴더 선택 등 Electron 전용 기능 대체, 골든 테스트(TS↔Rust 같은 fixture 결과 비교) | 골든 장치 완료(아래 "골든 테스트") |
| 2 | git 핵심(실행기·파서·LogCursor 스트리밍·상세/diff·RepoWatcher)을 Rust 로 | 파서 12개(log·refs·stash·worktree·nulRecords·diff·wipStatus·changes·commit·blame·fileHistory·submodule) 이식·골든 일치(건너뜀 0), 실행기(Local·WSL·ProcessQueue·wslPaths) 이식(아래 "실행기"), 라우터·미러·취소 레지스트리와 첫 채널 `git:commit`(아래 "라우터"), `git:log` 와 LogCursor 스트리밍(아래 "git:log"), `git:compare`·`git:fileDiff`·`git:fileHistory`·`git:blame`(아래 "파일 단위 채널") — 나머지 채널·RepoWatcher 는 남음 |
| 3 | fetch 가드 — `ALLOWED_KEYS` 허용 목록과 refspec 을 글자 그대로 이식, 기존 fetch 테스트로 패리티 확인, 보안 리뷰 | 완료(코드) — 판정 로직 이식·골든 일치(아래 "fetch 가드 판정 로직"), git 실행(`run.rs`)·통합 테스트(아래 "fetch 실행"), 상태 기계(아래 "fetch 상태 기계"), 라우팅·소유권(아래 "fetch 라우팅 — PR D"). 확인 창이 실제로 뜨고 버튼이 맞는지는 사람 확인이 남음 |
| 4 | GitHub·Jira(reqwest), 토큰은 OS 키체인(`keyring`) | 진행 중 — PR 1(순수 로직·골든)·PR 2(응답 매퍼·골든)·PR 3(HTTP 코어·Jira/GitHub 클라이언트)·PR 4(토큰 저장소)·PR 5(Jira 서비스)·PR 6(Jira 연결: `jira:*` 를 Rust 가 처리, `ANTLER_RUST_API`)·PR 7(GitHub 서비스, 연결 안 함) 완료, GitHub 는 아직 앱에 연결 안 됨(gh CLI 8, 연결 9). 계획·상태는 아래 "GitHub·Jira(Phase 4)" |
| 5 | SSH(`russh` 또는 시스템 ssh) — Windows agent·ProxyJump 지원을 먼저 확인 | |
| 6 | 업데이트(서명 키 필수인 `tauri-plugin-updater` + dmg·deb 수동 경로), 패키징·CI, Electron 사용자 이관 릴리스 | |

이관 PR 은 `refactor:`·`chore:` 제목으로 야간 릴리스를 피한다. Electron 을 걷어 내는 마지막 PR 만 `feat!:` 다.

## 구조(Phase 0)

```
renderer(src/renderer/src, 그대로)
  └ window.antler ← src/renderer/tauri/bridge.ts   (preload 대신. 허용 목록 검사·envelope 해제·창 버튼·툴바 드래그)
       └ invoke('antler_invoke') → src-tauri(Rust) → stdio JSON-lines → src/sidecar(Node, 개발 전용)
                                                                         └ src/main 의 registerIpc 를 그대로 재사용(electron 은 shim)
```

- `src/sidecar/electronShim.ts`: 확인 창(`dialog.showMessageBox`)·폴더/파일 선택(`showOpenDialog`)은 Rust 에 `host` 요청(`messageBox`·`pickPath`)으로 부탁하고
  `src-tauri/src/dialogs.rs` 가 `tauri-plugin-dialog` 의 Rust API 로 **네이티브 대화상자**(부모 = 메인 창, 모달)를 띄운다. webview 안 HTML 이 아니라서 침해된
  renderer 가 스스로 누를 수 없고, 플러그인의 JS command 는 capability 에 주지 않았다. 확인 창은 **어떤 실패(창 없음·잘못된 응답·범위 밖 값·닫기·Esc)에서도 cancelId** 로
  답한다. 확인 창 규칙:
  - 버튼은 1~3개(문구가 서로 달라야 한다 — 눌린 버튼을 문구로 구분한다). Esc·창 닫기·취소 버튼은 모두 거부다.
  - **Windows**: tauri-plugin-dialog 를 거치지 않고 rfd 를 직접 쓴다. rfd 는 누른 사용자 지정 버튼(`Custom(문구)`)과 Esc·닫기(`Cancel`)를
    구분해 돌려주므로 **취소 버튼을 맨 앞(= 기본 버튼, Enter)** 에 둔다 — Electron 판의 `defaultId`(취소)와 같다.
    fetch 확인은 `[취소][이번만 fetch][이 저장소에서 항상 허용]`, SSH 확인은 `[취소][연결]`.
  - **그 밖의 OS**(플러그인 경로): 플러그인 2.8 은 Esc·닫기로 생긴 `Cancel` 을 "마지막(cancel 자리) 버튼을 누른 것"으로 바꿔 돌려주므로,
    그 자리에 다른 버튼이 있으면 Esc 가 "연결"·"항상 허용"으로 뒤집힌다(#84 의 첫 구현이 그렇게 배치했다가 리뷰에서 잡았다).
    그래서 취소를 **마지막**에 두고, 대가로 Enter 가 첫 버튼(허용 쪽)을 누른다. macOS·Linux 빌드는 아직 컴파일조차 확인하지 않았다.
  - 버튼 배치 규칙은 `src-tauri/src/dialogs.rs` 의 단위 테스트(`cargo test --lib`)로 고정했다.
    응답 인덱스는 원래 `buttons` 기준으로 되돌려 준다.
  - **체크박스는 지원하지 않는다**(`checkboxChecked` 는 항상 `false`). 지금 호출부는 쓰지 않으며, 앞으로 체크가 권한을 넓히는 용도로 쓰이면 false 가 안전한 쪽이다.
  - `message` 와 `detail` 은 빈 줄로 이어 한 본문으로 보인다.
- `host` 요청의 `arg` 는 JSON 값, 응답 `hostres` 는 `ok` 와 함께 `value` 를 돌려줄 수 있다(`src/sidecar/protocol.ts`).
- 창 위치·크기·최대화는 `tauri-plugin-window-state` 가 기억한다(`StateFlags::SIZE | POSITION | MAXIMIZED`, visible·decorations·fullscreen 제외).
  파일은 `app_config_dir` 의 `.window-state.json`(`%APPDATA%\com.antler.app`)이고, 종료 때 저장한다. **`ANTLER_USER_DATA` 가 있는 실행(측정·자동화)에서는 등록하지 않는다.**
  최소 960×600·기본 1440×900 은 창 생성 코드가 그대로 정한다.
- 채널 허용 목록은 bridge 와 사이드카 두 곳에서 검사한다(`__TAURI__` 로 bridge 를 건너뛸 수 있으므로 사이드카가 최종 관문).
- 창 수준 가드: 앱 밖 주소로 이동·새 창·권한 요청을 모두 거부한다(`src-tauri/src/lib.rs`). capability 는 실제로 쓰는 창·이벤트 권한만 준다
  (`core:default` 는 tray·menu·path·devtools 까지 열어 preload 허용 목록보다 넓다).
- userData 는 Tauri 의 `app_data_dir`(`%APPDATA%\com.antler.app`)이다. Electron 판(`%APPDATA%\Antler`)과 공유하지 않는다.
  토큰은 메모리에만 둔다(Electron `safeStorage` 로 암호화된 파일은 풀 수 없다).

## 명령

```bash
pnpm tauri:build        # 사이드카·renderer 빌드 후 src-tauri/target/release/antler.exe (설치본 없음)
pnpm tauri:build:cdp    # 위 + ANTLER_CDP_PORT 로 원격 디버깅 포트를 열 수 있는 빌드(측정·자동화용, 배포 금지)
pnpm tauri:dev          # vite dev 서버(5174) + 디버그 빌드 — 아직 실행해 확인하지 않았다

UPDATE_GOLDEN=1 npx vitest run tests/golden.test.ts   # TS 파서 출력으로 골든(tests/golden/parsers) 다시 쓰기
npx vitest run tests/golden.test.ts                   # TS 파서가 골든과 같은지
cargo test --manifest-path src-tauri/Cargo.toml --lib --test golden -- --nocapture   # Rust 파서가 같은 골든과 같은지(건너뛴 수 출력)
cargo test --manifest-path src-tauri/Cargo.toml --features test-hooks --lib --test golden --test exec --test channels --test log --test fetch_guard --test fetch --test fetch_service --test fetch_route --test api_route --test net   # CI 와 같은 전체(라우터·채널 패리티·로그 커서 포함)
UPDATE_GOLDEN=1 npx vitest run tests/goldenChannels.test.ts   # 채널 골든(tests/golden/channels, exec/channels.json) 다시 쓰기
ANTLER_RUST_GIT=0 <exe>                               # 라우터를 끄고 모든 채널을 사이드카로(되돌리기 스위치)
ANTLER_RUST_API=0 <exe>                               # jira:* 만 사이드카로(서비스·키체인 없음). ANTLER_RUST_GIT=0 이어도 함께 꺼진다
UPDATE_GOLDEN=1 npx vitest run tests/goldenJiraChannels.test.ts   # jira:* 채널 골든(tests/golden/channels/jira.json) 다시 쓰기
```

요구 사항: Rust(stable, MSVC), WebView2 런타임, PATH 의 node(또는 `ANTLER_NODE`). 환경 변수 `ANTLER_USER_DATA` 로 userData 를,
`ANTLER_GPU=1` 로 하드웨어 가속을, `ANTLER_MEASURE=1` 로 첫 커밋 행 시각 기록(`window.__ANTLER_FIRST_ROW__`)을 켠다.

## 골든 테스트(Phase 1)

TS 파서가 기준이다. `tests/fixtures`(git 원시 출력, `make-fixtures.mjs`)와 `tests/fixtures/edge`(손으로 짠 경계 입력, `make-edge-fixtures.mjs` —
빈 입력·마지막 NUL 없음·잘못된 레코드·숫자 아닌 날짜·깨진 UTF-8·정렬 경계)에 TS 파서를 돌린 결과를 `tests/golden/parsers/<파서>__<fixture>.json`
(키 정렬·2칸·LF)로 저장하고, `manifest.json`(`[{parser, fixture, args?, golden}]`)이 목록이다.
`tests/golden.test.ts` 가 TS 출력 = 골든을 검사하고(git 을 실행하지 않는다), `src-tauri/tests/golden.rs` 가 같은 manifest 로 Rust 구현의 출력 JSON 을 골든과 비교한다
(불일치는 JSON 경로로 보이고, 아직 안 옮긴 파서 항목은 건너뛰며 수를 출력한다 — 지금은 0). 파서를 하나 옮길 때는 TS 입력 케이스를 manifest 에 먼저 넣고 골든을 만든 뒤
Rust 를 맞춘다. CI 는 `tauri.yml`(windows-latest). 모든 PR 에서 돌고 항상 결과를 보고한다 — 관련 파일이 바뀌지 않았으면
첫 단계에서 무거운 검사를 건너뛰고 성공으로 끝나므로 필수 체크로 지정해도 관련 없는 PR 이 대기하지 않는다.

2차 이식(diff·wipStatus·changes·commit·blame·fileHistory·submodule)의 fixture 는 전부 `make-edge-fixtures.mjs` 가 만든다(확장자 `.diff`·`.dat`·`.txt` — `.bin` 은 nulRecords 골든이
전부 돌리므로 피했다). `tests/fixtures/** -text`(`.gitattributes`)로 autocrlf 가 CRLF 를 담은 fixture 를 바꾸지 못하게 했다. 파서별 골든 수:
diff 30(청크 1·3·7·4096 바이트 분할, 줄·바이트·세기 한계, 3,000줄 기본 상한 — 큰 diff 는 `summary` 인자로 hunk 줄 목록 대신 줄 수·처음·마지막 줄만 고정) ·
wipStatus 9 · nameStatus 9 · numstat 5 · changes 6 · commit 10 · blame 4 · fileHistory 4 · gitlinks 3 · gitmodules 3 · probe 8.
TS 입력이 여럿이거나 상태가 있는 파서는 manifest `args` 로 맞춘다(`chunk`·`maxLines`·`numstat`·`fallback` 등). `parseCommitMeta` 가 던지는 입력은 골든에 `{"error": 메시지}` 로 남는다.

이식 때 정한 모양(2차):

- `DiffParser`·`WipStatusParser` 는 TS 처럼 `push`/`finish` 스트리밍 구조 그대로다(줄·바이트·세기 한계 상수 3000 / 200,000 / 64MB 와 "상한 직후 끝나면 잘린 것이 아님" 판정 포함).
- TS 가 `Map` 으로 돌려주는 `parseGitmodules` 는 Rust 에서 삽입 순서를 지킨 `Vec<ModuleEntry>`(항목에 `path` 포함)이고, 같은 경로가 둘이면 뒤가 덮어쓰되 자리는 처음 그대로다.
- `parseGitlinks`·`parseGitmodules` 는 TS 가 `Buffer | string` 을 받지만 Rust 는 바이트만 받는다. `submoduleStatus`·`FileStatus` 는 골든 없이 단위 테스트로 본다.
- **알려진 차이(보고만, 고치지 않음)**: TS 는 줄·문자열을 UTF-16 코드 단위로 자른다. diff 내용 줄의 접두 한 글자, `fileHistory` 의 상태 한 글자, wip `? ` 접두 두 글자가
  비-BMP 문자(이모지 등)일 때만 Rust(코드 포인트 단위)와 다르다. git 출력에서 그 자리는 ASCII 접두라 실제로는 일어나지 않아 fixture 에 넣지 않았다.
  2^53 이상 정수(예: hunk 시작 줄 1e20)는 TS·Rust 모두 부동소수점이라 값은 같지만, 골든 비교기는 값(`f64`)으로 비교한다.
- 로케일·플랫폼 의존으로 빼 둔 것은 없다 — 2차 파서 7개는 정렬·`Intl`·날짜 파싱을 쓰지 않는다(`Number()`·`trim()` 은 `js_number`·`js_trim` 이 흉내 낸다).

이식 때 TS 와 맞춘 것:

- 디코드는 전부 UTF-8(깨진 바이트는 U+FFFD — Node `toString('utf8')` = Rust `from_utf8_lossy`). `-z` 레코드는 바이트 단위로 자르고, 마지막 NUL 이 없는 조각은 비어 있지 않을 때만 레코드다.
- `Number(문자열)` 은 `js_number` 가 흉내 낸다(빈 문자열 0, 앞뒤 공백 허용, `0x`, 문자열이 숫자가 아니면 NaN → JSON null).
- TS 에서 키가 없는 optional 필드(`upstream`·`ahead`·`behind`·`upstreamGone`·`date`)는 `skip_serializing_if`, `null` 인 필드(`Worktree.head`·`branch`)는 `Option` 그대로.
- **알려진 차이**: 브랜치·태그·리모트 정렬은 TS 가 `localeCompare(undefined, {numeric: true})`(ICU, **실행 환경 로케일에 따라 다르다** — ko-KR 에서는 한글이 라틴 문자보다 앞)이고,
  Rust `natural_compare` 는 숫자 덩어리 수치 비교 + 대소문자 무시(같으면 소문자 먼저) + ASCII 기호는 CLDR root 순서 + ASCII 밖은 코드 포인트 순인 근사다.
  그래서 골든 fixture 에는 서로 다른 문자 체계(한글↔라틴)가 첫 차이가 되는 정렬 쌍을 넣지 않았다(로케일이 다르면 골든이 달라진다). 실제 정렬을 Rust 로 완전히 옮기려면 `icu_collator` 가 필요하다 — Phase 2 의 결정 사항.

## 실행기(Phase 2, `src-tauri/src/git/exec/`)

`src/main/executors` 의 Local·WSL 실행기·`ProcessQueue`·`wslPaths` 를 옮겼다(SSH 는 Phase 5). 앱에서는 라우터(아래 "라우터")가 등록된 채널에서만 부른다.

- `common.rs` 의 `GIT_GLOBAL_ARGS`·`GIT_ENV` 가 유일한 정의이고 `RunOpts` 에는 **일반 env 필드가 없다**(호출자가 방어 환경변수를 덮어쓸 수 없다. TS 의 `RunOpts.env`·`envFor`·WSL `extraEnv` 는 GitHub 원격 전용 저장소용이라 옮기지 않았다).
  예외는 fetch 용 닫힌 타입 `FetchEnv { allow_file, ssh_command }` 하나뿐이다(`RunOpts.fetch_env`, **`run` 에서만**, `stream` 은 무시). 키는 `fetch_env()` 의 상수 여섯 개이고 자유 문자열은 `GIT_SSH_COMMAND` 의 값뿐이다
  (아래 "fetch 실행").
- `run`(stdout 64MB 상한, stderr 는 앞 4MB 만 담고 파이프는 끝까지 비움) / `stream`(`GitStream`: `AsyncRead`. **읽을 때만** 파이프에서 가져가므로 소비자가 안 읽으면 git 이 멈춘다.
  EOF 뒤 `finish()` 로 종료 코드를 확인하고, 버리면 트리를 죽인다). 취소는 `CancellationToken`.
- **앱에 연결하기 전 필수(보안 리뷰)**: 실행기는 호출자 `args` 를 전역 옵션 바로 뒤에 붙인다 — renderer 값이 그 자리에 오면
  `-c alias.x=!cmd`·`--output=<경로>` 로 명령 실행·파일 쓰기가 된다. 라우터는 (1) 서브커맨드·옵션을 상수로 정하고 renderer 값(ref·해시·경로)은
  검증 뒤 정해진 위치·`--` 뒤에만 넣고, (2) 저장소 경로·배포판은 renderer 가 아니라 사이드카가 보낸 미러(main 이 저장한 값)에서만 가져오며
  `src/main/ipc/handlers.ts` 의 `checkPath`(UNC·NUL·길이)·배포판 목록 대조를 이식하고, (3) LogCursor 를 버릴 때 `GitStream` 을 drop 한다
  (스트림의 취소 토큰은 다음 읽기 때만 확인된다 — 토큰만 취소하고 구조체를 남기면 git 이 파이프에서 멈춘 채 남는다).
  (1)·(2)는 라우터 PR 에서 채웠고(아래 "라우터"), (3)은 `git:log` PR 에서 채웠다(아래 "git:log").
  Rust CI(`tauri.yml`)는 main 룰셋의 필수 체크(`Rust · 골든 · clippy`)다(2026-11-16 지정) — clippy 의 `disallowed-methods`·주입 거부 테스트가
  실패한 PR 은 머지되지 않는다. 워크플로는 모든 PR 에서 항상 결과를 보고한다(관련 없는 PR 은 건너뛰고 성공).
- Windows: `CREATE_NO_WINDOW`. **트리 종료는 Job Object**(`exec/job.rs`)다. git 을 띄울 때마다 Job 을 만들어 `KILL_ON_JOB_CLOSE` 를 걸고, `CREATE_SUSPENDED` 로 띄운 뒤
  Job 에 넣고 `NtResumeProcess` 로 재개한다(래퍼 `cmd\git.exe` 가 손자를 띄우기 전에 Job 안에 있어야 손자도 Job 에 든다. 주 스레드 핸들을 std 가 닫아 프로세스 단위로 재개한다 —
  Toolhelp 스레드 스냅샷으로 재개하는 방법은 호출당 60ms~1.5초가 걸려 버렸다). 취소·Drop 은 동기 `TerminateJobObject` 이고 프로세스를 만들지 않아 **호출이 70µs 안팎**에 돌아온다(실측).
  Job 핸들은 `ChildGuard` 가 쥐므로 앱이 비정상 종료해도 커널이 트리를 죽인다. 루트가 스스로 끝난 뒤의 Drop 은 `KILL_ON_JOB_CLOSE` 를 풀고 닫는다(남은 자손을 건드리지 않는다 — fetch 의 분리된 `gc --auto` 등).
  재개가 실패하면 자식을 죽이고 `SpawnFailed`. 남는 창: `CreateProcess` 직후 Job 에 넣기 전(마이크로초)에 앱이 죽으면 정지된 git 이 남는다.
  **taskkill 폴백**은 Job 생성·할당이 실패할 때만 쓴다(`taskkill /pid N /T /F` 를 한 번만, 이미 끝나 수거된 자식에는 보내지 않는다). **별도 스레드**에서 띄우고 `JoinHandle` 을 모아 두며,
  앱 종료 때 `shutdown_kills`(lib.rs `RunEvent::Exit`, 라우터 종료 뒤 2초 상한)가 기다린다. 그 뒤 들어온 폴백은 프로세스를 새로 만들지 않고 루트만 `TerminateProcess` 한다.
  루트 핸들 복제본을 taskkill 이 끝날 때까지 쥐어 그 사이 PID 가 재사용되지 않는다. `tests/exec.rs` 의 수동 테스트가 폴백도 검증한다(`set_jobs_disabled`).
  이력: 처음 이식은 런타임 스레드에서 taskkill 을 2~3번 띄워 병렬 테스트 부하에서 취소→반환이 8~22초까지 늘었고, 스레드로 옮긴 뒤에도 앱이 끝나는 순간 `ExitProcess` 가 CreateProcess 도중의
  스레드를 죽여 **일시 정지된 taskkill.exe·conhost.exe 가 재부팅까지 고아로 남았다**(상속된 stdout 핸들을 쥐어 `cargo test | tail` 이 끝나지 않았다). Job Object 로 바꾼 이유다.
  std 는 Windows 자식을 `bInheritHandles=TRUE` 로 띄워 stdio 를 null 로 줘도 부모의 상속 가능 핸들을 물려주지만, git 은 파이프 stdio 고 taskkill 은 폴백에서만 뜨므로 std 핸들의 상속 플래그는 건드리지 않았다.
- 오류는 `ExecError`(`Git`·`Aborted`·`SpawnFailed`) → envelope code `GIT`·`ABORTED`·`ERROR`. WSL 은 wsl.exe 자신의 실패(종료 코드 -1, stderr 빔)를 stdout 의 WSL 메시지로 바꾼다.
- `ProcessQueue`: local 6 / wsl 4 동시성(저장소마다 하나), 같은 키 요청은 `Shared` 로 합치고(키가 있는 요청에는 취소 신호를 걸지 않는다), 기다리는 쪽이 모두 사라지면 프로세스도 정리된다.
- **git 을 띄우는 `Command::new` 는 `process.rs` 한 곳뿐이다.** `src-tauri/clippy.toml` 의 `disallowed-methods` 가 `std::process::Command::new`·`tokio::process::Command::new` 를 막고,
  예외(`sidecar.rs` 의 Node 사이드카, `process.rs` 의 git·taskkill, 테스트 헬퍼)는 `#[allow(clippy::disallowed_methods)]` 와 이유 주석을 단다.
- 골든 `tests/golden/exec/env.json`(`UPDATE_GOLDEN=1 npx vitest run tests/goldenExec.test.ts`): TS 의 전역 인자·`GIT_ENV`(순서 포함)·로컬 spawn 인자와 환경·`wsl.exe` 인자와 `WSL_UTF8`·`wslArgv` 샘플. `src-tauri/tests/exec.rs` 가 Rust 상수와 비교한다.
- 테스트 격리: `src-tauri/tests/exec.rs` 의 `isolate`(ctor — 어떤 테스트보다 먼저 한 번)가 `GIT_CONFIG_GLOBAL`(커밋된 `src-tauri/tests/gitconfig`)·
  `GIT_CONFIG_NOSYSTEM`·`GCM_INTERACTIVE` 를 테스트 프로세스 환경에 넣는다. `.cargo/config.toml` 의 `[env]` 는 쓰지 않는다 —
  `cargo run`·`tauri dev` 에도 걸려 개발 중인 앱의 git 이 사용자 전역 설정(WSL 경로의 safe.directory, 자격 증명 도우미)을 못 보게 된다.
  git 을 띄우는 Rust 테스트를 새 파일에 만들면 같은 격리를 넣어야 한다(`격리` 테스트가 본보기).
- 실행: `cargo test --manifest-path src-tauri/Cargo.toml --lib --test golden --test exec`. WSL 통합 케이스는 배포판(과 그 안의 git)이 없으면 건너뛴다(`[wsl] 건너뜀`, `--nocapture`).
  수동(트리 kill): `ANTLER_BIG_REPO=<큰 저장소> cargo test --manifest-path src-tauri/Cargo.toml --test exec -- --ignored --nocapture --test-threads=1` — powershell 로 프로세스 트리를 읽는다.

## 라우터(Phase 2, `src-tauri/src/router.rs`)

`antler_invoke` 는 이제 라우터를 거친다. 채널이 Rust 에서 처리할 수 있는 것이면 Rust 가 envelope 를 만들고, 아니면 사이드카에 인자를 **그대로** 넘긴다.

```
renderer → antler_invoke → Router::route ─ Done(envelope) ──────────────→ renderer      (Rust 경로: 지금은 git:commit·git:log·git:compare·git:fileDiff·git:fileHistory·git:blame)
                                         └ Sidecar → Node 사이드카 → envelope → renderer (그 밖의 모든 것)
```

- **Rust 경로 조건**(전부): 채널이 `RUST_CHANNELS` 에 있다 · 인자의 저장소 id 가 미러에 있다 · 그 kind 가 local 또는 wsl 이다(ssh·github 소스는 사이드카) ·
  환경변수 `ANTLER_RUST_GIT` 이 `0` 이 아니다. 미러에 없는 id 는 사이드카로 가서 오류 문구를 사이드카가 만든다.
- **되돌리기 스위치**: `ANTLER_RUST_GIT=0` 으로 실행하면 모든 채널이 사이드카로 간다(Electron 판과 같은 경로). 확인용으로 `ANTLER_ROUTE_LOG=1` 이면
  `RUST_CHANNELS` 채널이 어디로 갔는지 stderr 에 `[route] git:commit -> rust|sidecar` 로 남긴다(기본은 조용하다. drive 는 `ANTLER_DRIVE_DEBUG=1` 로 stderr 를 본다).
- **미러**(`src-tauri/src/mirror.rs`, 프로토콜 `{t:"repos", gen, repos:[{id, kind, path, distro?}]}`): 설정을 쓰는 쪽은 사이드카의 `ConfigStore` 뿐이다.
  `ConfigStore.onChange`(기본 아무 일도 안 함 — Electron 판은 쓰지 않는다)가 `update` 안에서 동기로 불려 사이드카가 미러를 보낸다 — 시작 때 ready 전에 한 번,
  저장소 목록이 바뀔 때마다(창 위치·UI 설정처럼 저장소와 무관한 쓰기는 같은 내용이라 보내지 않는다). 같은 stdout 에서 순서대로 읽으므로 저장소를 바꾼 호출의 응답보다 미러가 먼저 반영된다.
  `gen` 이 이미 받은 것 이하이면 버린다. 토큰 등 비밀은 담지 않는다. **Rust 는 renderer 가 보낸 경로·kind·배포판을 쓰지 않는다** — 저장소 id 와, 채널별로 검증을 통과한 값(해시·ref 범위·파일 경로·숫자)만 받는다.
  저장소 경로와 파일 경로는 다르다: 저장소 경로는 미러에서만, 파일 경로(`git:fileDiff` 등의 `path`)는 renderer 값이라 아래 "파일 단위 채널"의 규칙으로 검증한다.
- **미러 값 검사**: `check_path` 는 `handlers.ts` 의 `checkPath` 와 같다(NUL·빈 값·4096 UTF-16 단위 초과 거부, 로컬·WSL 의 `\\host\share` 거부, `\\wsl.localhost\…` 허용, UNC 는 trim 한 값으로 본다).
  WSL 배포판은 TS 가 **추가할 때만** 설치 목록(`wsl.exe -l -q`)과 대조하고 쓸 때는 다시 확인하지 않는다 — Rust 는 목록을 읽지 않고(`Command::new` 가 `process.rs` 에만 있어야 한다)
  형식만 본다(비었음·255 초과·제어 문자·`-` 로 시작 거부). 검사에 걸린 미러 값은 사이드카로 넘기지 않고 `ERROR` envelope 로 돌려준다(TS 는 이 값으로 git 을 돌렸을 것이다).
- **인자 구성**: `git:commit` 의 renderer 인자는 `repoId, hash, requestId?` 다. 해시는 `HexHash`(16진수 4~64자, TS `assertHex` 와 같음 — 정규식 대신 손으로 검사해 JS `$`·`\s` 차이를 피했다)만 통과하고,
  `git/commit.rs` 의 `meta_args`·`diff_tree_args` 가 서브커맨드·옵션 상수 + `HexHash` 를 정해진 자리에만 넣는다(TS 와 같게 `--end-of-options` 없이 뒤에 `--`).
  `HexHash` 가 아닌 문자열은 argv 를 만들 방법이 없다. `-c alias…`·`--output=…`·`-x`·개행 붙은 해시 등의 거부는 단위·통합 테스트가 증명한다.
  git 이 돌려준 `%H`·부모 해시도 `HexHash` 로 다시 검사한 뒤 diff-tree 범위에 쓴다.
- **채널 처리**(TS `RepoService.commit` 과 같은 순서): 해시 기준 불변 LRU(300개·24MB, 키 `repoId\0소문자 해시`, 크기 추정은 UTF-16 길이) → 같은 키 요청 합치기(`ProcessQueue` 의 `Shared`) →
  큐 슬롯 한 개 안에서 `log -1`, 이어 `diff-tree --name-status` 와 `--numstat` 동시. 큐는 **소스별**(TS 와 같게 local 하나, wsl 은 배포판마다 하나; 6/4 동시성)이고 큐의 결과 타입이 JSON 값이라 이후 채널이 같은 큐를 나눠 쓴다.
- **envelope**: `{ok:true,data}` / `{ok:false,error,code}`. code 는 `handlers.ts` 의 판정 순서(ABORTED → STALE → GitHub·Jira → GIT → ERROR)와 같게 `ExecError` 에서 정한다
  (Aborted→ABORTED, Git→GIT, 그 밖(검증 실패·파서 거부·spawn 실패)→ERROR). **오류 문구는 code 만 패리티**다 — 문구는 Rust 가 비슷하게 만들지만 같다고 보장하지 않는다.
- **취소**: `requestId → CancellationToken`(`CancelRegistry`). 핸들러는 첫 await 전에 등록한다. `req:cancel` 은 Rust 레지스트리를 취소한 뒤 **항상** 사이드카에도 전달한다.
  cancel 과 요청은 별개 invoke 라 도착 순서가 보장되지 않으므로, 등록 전에 온 cancel 은 tombstone(10초·256개)에 남겨 두었다가 등록 때 이미 취소된 토큰을 준다.
  합쳐진 요청에서는 한 호출자의 취소가 다른 호출자의 프로세스를 죽이지 않는다 — 호출자 토큰은 실행기가 아니라 대기(`select!`)에만 걸리고, 기다리는 쪽이 모두 사라지면 큐가 프로세스를 정리한다.
- `UNDEFINED_ARG` 표식은 Rust 경로에서만 디코드한다(`decode_args` — 사이드카 `decodeArgs` 와 같은 규칙). 사이드카로 보낼 때는 원본 인자를 그대로 넘긴다.

### 채널 골든

- `tests/golden/exec/channels.json`: TS `IPC_CHANNELS` 덤프. `src-tauri/tests/channels.rs` 가 `RUST_CHANNELS ⊆ IPC_CHANNELS` 를 강제한다(채널 이름을 오타 내면 Rust 테스트가 실패한다).
- `tests/fixtures/repos/*.fi`(`node scripts/make-repo-fixtures.mjs` 로 만든 `git fast-import` 스트림 — 날짜·작성자·내용이 고정이라 해시가 결정적이다. 루트·병합·가지·빈 커밋·이름 변경·바이너리·한글 경로·5001개 파일 포함)와
  `commit.scenarios.json`(시나리오: 해시 풀이, 대문자·약식 해시, 없는 해시, 잘못된 해시 여러 가지)을 TS(`tests/goldenChannels.test.ts` → `RepoService.commit`)와 Rust(`channels.rs` → `Router::route`)가 임시 디렉터리에 import 해 돈다.
  TS 결과가 `tests/golden/channels/commit.json`(오류는 code 만, 파일이 20개를 넘으면 개수·처음·마지막만)이고 Rust 가 같은지 본다.
- `git:log`: `log.scenarios.json` 은 **다단계 스크립트**다(`steps`: `{log: query}` 와 `{git: [인자…]}`). 시나리오마다 `<repo>.fi` 를 새 디렉터리에 import 해 페이지 이어 붙이기·solo·hide·빈 저장소·orphan HEAD·잘못된 scope·잘못된 query,
  커서가 닫힌 뒤 앵커로 이어 읽기, `update-ref -d` 뒤 STALE 같은 흐름을 TS(`RepoService.log`)와 Rust(`Router::route`)가 똑같이 돈다(커서 닫기는 테스트 훅 없이 다른 범위의 요청으로 한다 — 두 구현 모두 다른 키의 커서를 닫는다).
  결과는 커밋을 `해시12 부모7,… 제목 종류:이름,…` 한 줄로 줄여 `tests/golden/channels/log.json` 에 담고(`full` 단계는 전체 필드), 오류는 code 와 ERROR 의 문구(`invalid …`)만 비교한다.
  query 안의 `@rev:<식>`(해시로)·`{"$gen": [접두, n]}`·`{"$str": [접두, 문자, n]}` 은 두 쪽이 같은 규칙으로 푼다. `pagesEqualFull` 은 페이지를 이어 붙인 결과가 전체 한 번과 같음을 TS·Rust 가 각자 확인한다.
  `logArgs.cases.json` → `logArgs.json` 은 `logArgs` 의 argv(범위별)와 오류 문구를 고정한다. fixture 는 `branchy.fi`(병합·가지 여럿·가벼운/주석 태그·리모트 추적 ref·어디에도 합쳐지지 않은 최신 가지)와 `empty.fi`(빈 스트림)가 새로 들어갔다.
- 골든 갱신: `UPDATE_GOLDEN=1 npx vitest run tests/goldenChannels.test.ts`. 채널을 `RUST_CHANNELS` 에 추가할 때는 시나리오·골든도 함께 넣는다.
- Rust 테스트의 git 격리는 `tests/channels.rs` 의 `isolate`(ctor)가 한다(`exec.rs` 와 같다). 단위 테스트(`--lib`)는 git 을 띄우지 않는다.

## git:log(Phase 2, `src-tauri/src/git/log_service.rs`·`scope.rs`)

TS 의 `LogCursor`(`src/main/git/logCursor.ts`)와 `RepoService.log()` 의 커서 맵·앵커·STALE·유휴 정리를 옮겼다. 응답은 invoke 의 envelope 이다(Channel 이 아니다).

- **커서**: 커서마다 읽기 태스크 하나가 `GitStream` 을 소유하고, 요청은 mpsc 로 받아 보낸 순서대로 처리한다. 요청을 넣는 일(= 시작 위치 예약)이 동기라서
  "예약 순서 = 읽기 순서"가 보장된다(TS 의 `busy` 사슬과 같은 효과). 버퍼가 count 개보다 많아지면 읽기를 멈추고, 읽지 않는 동안에는 파이프 backpressure 로 git 이 멈춘다.
  `usable`·`ended`·`reading`·`position`·`next_position`·`last_hash` 와 "끝에서 덜 받으면 예약 위치를 되돌린다"까지 TS 와 같다.
  커밋 없는 저장소는 EOF 뒤 `finish()` 의 stderr(`does not have any commits`·`bad default revision`·`bad revision 'HEAD'`·`ambiguous argument 'HEAD'`, 대소문자 무시)로 빈 목록이다.
- **서비스**(`LogService`): 키 `repoId\0범위 키`. 요청마다 ① 다른 키의 커서를 닫고(앵커를 남긴다) ② skip 0 이면 앵커를 지우고 ③ 예약된 다음 위치가 맞고 살아 있고 끝나지 않은 커서만 이어 쓰며
  ④ 아니면 새로 만든다. 닫힌 커서의 앵커(위치·마지막 해시)가 `skip` 과 맞으면 한 커밋 앞에서 시작해 그 커밋이 같은지 확인한 뒤 이어 읽고, 다르면 `STALE`
  (code STALE, 문구 `commit history changed while paging`)다. 이 앞부분(커서 선택·요청 넣기)은 await 없이 한 번의 락 안에서 끝난다 — TS 가 첫 await 전에 하는 일이다.
  확인 읽기와 본 읽기는 한 요청으로 묶어 그 사이에 다른 읽기가 끼지 못하게 했다(TS 는 둘을 따로 예약한다).
- **유휴 정리**: 5초 안 쓰인 커서는 닫고(2.5초마다 훑는다) 읽는 중인 커서는 건드리지 않는다. 훑는 태스크는 첫 커서를 만들 때 띄우고(`setup` 은 tokio 런타임 밖이다) 커서가 없어지면 스스로 끝난다.
  앱 종료(`RunEvent::Exit`)·저장소 제거 때는 `Router::shutdown`·`on_mirror_changed` 가 닫는다. 종료 때 닫지 못한 git 은 앱이 어떻게 끝나든 Job 핸들이 닫히며(`KILL_ON_JOB_CLOSE`) 커널이 트리를 죽인다.
  git 은 **읽기 태스크가 첫 요청 전에**(상태 락 밖, `spawn_blocking`) 띄운다 — 서비스가 락을 쥔 채 동기 CreateProcess 를 부르지 않게. PID 는 태스크가 meta 에 적는다.
  한 레코드(NUL 사이)가 16MB 를 넘으면 스트림을 오류(`ERROR`)로 끝내 커서를 닫는다(`NulRecordSplitter::push_checked` — `run` 결과를 파싱하는 `split_nul` 에는 상한이 없다).
  한 페이지를 채우는 동안 읽은 바이트도 64MB(`DEFAULT_MAX_BUFFER`)를 넘으면 오류로 끝낸다 — 큰 메시지의 커밋 1000개(압축돼 저장소는 작다)가
  페이지 하나에 수 GB 를 쌓지 못하게. 분리기는 새 청크만 훑는다(긴 미완성 조각을 매번 다시 훑으면 시간이 길이의 제곱으로 는다).
  위치 계산(`skip` 이 u64 끝까지 올 수 있다)은 포화 산술이라 `{"skip":1e300,"count":1}` 도 패닉 없이 git 오류로 끝난다.
- **스트림 drop(앞의 "앱에 연결하기 전 필수" (3))**: 커서를 닫는 모든 길 — `dispose`(유휴·다른 저장소·요청 취소·STALE·미러 변경·종료), 서비스에서 빠질 때의 `Drop`, 읽기 오류 —
  이 `GitStream` 을 버린다. `dispose` 는 토큰을 취소하고 읽기 태스크를 `abort` 해 태스크가 쥔 스트림을 버리게 한다(토큰만 취소하면 스트림은 다음 읽기 때만 깬다).
  `tests/log.rs` 가 4000 커밋 저장소로 git 이 파이프에서 멈춘 상태를 만들고, 닫은 뒤 프로세스 트리가 모두 사라지는지(powershell 스냅샷) 본다.
  요청 취소(`req:cancel`)는 읽는 도중이면 커서를 버린다(앵커도 남기지 않는다) — TS 와 같다.
- **`repos:watch`**: 라우터가 사이드카로 넘기기 전에 활성 저장소가 아닌 저장소의 커서를 모두 닫는다(`null`·모양이 틀린 인자면 전부). 응답은 여전히 사이드카의 몫이다.
  미러에서 사라졌거나 경로·환경이 바뀐 저장소의 커서·앵커는 미러 적용 직후(사이드카 읽기 스레드) 지운다.
- **사이드카 알림**: Rust 가 `git:log` 를 처리하면 사이드카의 `RepoService` 는 로그 경로를 거치지 않으므로, Rust 가 `{t:"note", op, repoId}` 를 보낸다.
  `logFirstPage`(skip 0 요청이 끝났을 때 — 성공·실패·취소 모두, 검증에서 거부된 요청은 제외. TS 의 `openFirstPage` 와 같다)와
  `logStale`(STALE 일 때 — `invalidateSignature`). 사이드카는 `src/sidecar/note.ts` 가 허용 목록의 op 와 문자열 repoId 만 받아 `RepoService.noteLog` 를 부른다
  (Electron 판 경로는 새 메서드만 추가되고 호출은 바뀌지 않았다). `t` 는 Rust 가 정하며 renderer 의 값은 `req` 의 `args` 안에만 들어간다.
  라우터는 `Sidecar`·`AppHandle` 에 기대지 않으므로 알림은 주입한 싱크(`Router::set_note_sink`)로 나가고, 앱(`lib.rs`)이 사이드카에 쓰는 클로저를 넣는다.
- **검증(인자 주입 경계)**: `LogQuery` 는 객체여야 하고 `count`·`skip` 은 유한한 숫자여야 하며(그 밖은 `invalid query`; TS `RepoService.log` 도 같은 PR 에서 같은 규칙을 갖췄다 —
  renderer 는 늘 숫자를 보내므로 Electron 동작은 그대로다) count 는 1~5000, skip 은 0 이상 정수로 맞춘다. 범위는 `scope.rs` 가 TS `isValidScopeRef`·`isRemoteAll`·`scopeArgs` 를 옮긴 것이다:
  항목은 `refs/(heads|tags|remotes)/…`(공백·제어 문자·`~^:?*[\` 없음, `..`·`@{`·`//`·끝 `/`·`.lock`·`/.` 거부)·소문자 40/64자 해시·`refs/remotes/<이름>/*` 만 통과하고,
  한 목록 200개·1024 UTF-16 단위 상한, 해시는 hide 에 못 쓴다. **정규식 크레이트를 쓰지 않고** JS `\s`(U+FEFF 포함·U+0085 제외 — Rust `is_whitespace` 와 반대인 두 문자)를 직접 나열했다.
  argv 에서 renderer 값은 정해진 자리에만 들어간다: solo 의 rev 는 `--end-of-options` 뒤, 리모트 전체·exclude 는 `--glob=`·`--exclude=` 접두가 붙은 한 인자, 숫자는 `--skip=N`.
  서브커맨드·옵션은 상수다. `-c…`·`--output=…`·개행·BOM 이 든 값은 git 을 띄우기 전에 `invalid solo|hide|scope|query`(ERROR)로 거부된다(`tests/channels.rs`).
- **TS 와 같음을 보는 곳**: 채널 골든(아래), `tests/logCursor.test.ts` 의 케이스를 옮긴 `tests/log.rs`, `src/shared/logScope.ts` 의 JS 규칙을 못 박은 `tests/logScope.test.ts`.
- 앱 확인(`drive.mjs`, `ANTLER_ROUTE_LOG=1` 로 `git:log -> rust` 확인, 10만 커밋 저장소 1회씩이라 **추세만** 본다): Solo `커밋 1,000개+`·diff `f32.txt` 3줄·검색 `1 / 2250` 이 Rust 경로·`ANTLER_RUST_GIT=0`·Electron 에서 모두 같다.
  콜드 스타트(첫 커밋 행) 7.6초(Rust) / 13.0초(`ANTLER_RUST_GIT=0`) / 16.3초(Electron), 스크롤 p95 26.6 / 26.6 / 26.7ms(스크롤 중 읽은 행 7,000+ / 1,000+ / 6,000+) — 부하가 있는 머신에서 1회씩이라 콜드 스타트 차이는 결론을 내지 않는다.
- 알려진 차이: 확인 읽기+본 읽기를 한 요청으로 묶는다(위). 앵커는 TS 처럼 키마다 하나씩이지만 Rust 쪽은 **활성 저장소의 것만 남기고 32개를 넘으면 오래된 것부터 버린다**(TS 는 상한이 없다).

## 파일 단위 채널(Phase 2, `src-tauri/src/git/file_channels.rs`)

`git:compare`·`git:fileDiff`·`git:fileHistory`·`git:blame` 를 옮겼다(TS: `commands.ts` 의 `compareCommits`·`fileDiff`, `fileInsight.ts`, `RepoService` 의 같은 이름 메서드).
라우팅·미러·취소·envelope 은 `git:commit` 과 같다. 여기서 처음으로 **renderer 의 자유 문자열(파일 경로)** 이 git argv 에 들어간다.

| 채널 | 인자 | git | 캐시 · 합치기 | 취소 |
|---|---|---|---|---|
| `git:compare` | `repoId, from, to, requestId?` | `diff-tree -r -z -M --no-commit-id --no-ext-diff --name-status\|--numstat <from> <to> --` 둘을 동시에(커밋 상세와 같은 `changed_files`·파서·5000개 상한) | 키 `compare\0repoId\0from소문자\0to소문자`, 300개·24MB, 같은 키 합치기 | 기다림만 그만둔다(합쳐진 호출자 보호) |
| `git:fileDiff` | `repoId, {hash, path, oldPath?, parent?, full?}, requestId?` | `rev-list --parents -n1 <hash> --`(parent 생략 때만) → `diff-tree -p -M --no-color --no-ext-diff --no-textconv --no-commit-id -U3 <부모\|--root> <hash> -- [oldPath] path` 를 **스트림**으로 읽어 `DiffParser` 에 먹인다 | 키 `repoId\0hash\0parent\0oldPath\0path\0full\|part`(소문자·빈 값은 `''`), 500개·32MB, part/full 이 서로 다른 항목, 합치기 | 기다림만 그만둔다 |
| `git:fileHistory` | `repoId, {path, rev?, skip, count}, requestId?` | `rev-parse --verify --quiet HEAD`(rev 생략 때) → `log --follow --no-color --no-ext-diff -z --name-status -M --format=… --max-count=skip+count+1 --end-of-options <rev> -- path` | 없음(HEAD 가 움직인다) | 프로세스에 바로 닿는다(`queue.run(None, Some(token))`) |
| `git:blame` | `repoId, {path, rev?}, requestId?` | `rev-parse HEAD`(rev 생략 때) → `cat-file -s <rev>:<path>` → `blame --no-textconv --porcelain <rev> -- path`(128MB 버퍼, `blame.ignoreRevsFile` 이 깨졌으면 `--no-ignore-revs-file` 로 한 번 더) | 없음 | 프로세스에 바로 닿는다 |

- **잘림·전체 불러오기**: diff 는 기본 8MB·3,000줄에서 줄 객체 만들기를 멈추고 이후 줄 수만 센다(`truncated`, 20만 줄·64MB 를 넘으면 `totalIsLowerBound`). `full` 이면 64MB·**20만 줄**까지 읽는다.
  - **알려진 차이**: TS 의 `full` 은 줄 수 상한이 없다. Rust 는 줄 객체를 `serde_json::Value`(줄당 수백 바이트)로 만들므로 한 줄 1바이트짜리 64MB 파일이면
    수십 GB 가 되어 앱이 죽는다(보안 리뷰) — 그래서 20만 줄에서 자르고(`truncated`) 줄 수는 끝까지 센다. 캐시에 들어가지 않은 큰 결과는 응답할 때 복사하지 않는다.
  - **blame 목록 파일**: `blame.ignoreRevsFile` 이 OID 가 아닌 줄을 담은 파일을 가리키면 git 은 `invalid object name: <그 줄>` 로 파일 내용을 stderr 에 낸다.
    악성 저장소가 이 설정을 사용자 파일(자격 증명 등)로 가리키면 그 첫 줄이 오류 문구로 renderer 에 갔다(TS 도 같다 — 별도 수정). 이런 실패도 `--no-ignore-revs-file`
    로 다시 돌리고, 그래도 실패하면 원문 대신 `git blame failed` 를 돌려준다(테스트 `blame_ignore_revs_file_의_내용이_응답으로_새지_않는다`).
  상한을 넘기고 세기도 끝났으면 스트림을 버려 git 을 끊고, EOF 까지 읽었으면 `finish()` 로 종료 코드를 확인한다(없는 해시는 `GIT`). `full` 은 JS 참 거짓으로 읽는다(`1`·`"no"` 는 참, `0`·`""`·`null` 은 거짓).
  blame 은 512KiB 가 넘는 파일이면 blame 하지 않고 `tooLarge`, 그 시점에 경로가 없거나 파일이 아니면(`cat-file -s` 실패) `missing`, 출력에 NUL 이 있으면 `binary`, HEAD 를 풀 수 없으면 `no commits yet`(ERROR).
  fileHistory 는 `skip` 이 10만 이상이면 git 을 부르지 않고 `{entries:[], hasMore:false}`, 커밋 없는 저장소도 같다(`rev` 키 없음).
- **LFS·lenient·캐시 비우기**: 이 네 채널의 TS 코드(`commands.ts`·`fileInsight.ts`)에는 LFS 처리가 없어 그대로 옮겼다(LFS 판정은 `git:blob`·`git:lfsPaths` 쪽이고 둘 다 사이드카에 남는다).
  TS 의 `lenient`(blob 을 못 받아도 목록만 돌려주기)는 GitHub 원격 전용 저장소용이라 옮기지 않았다 — 그 저장소는 사이드카로 간다.
  TS 는 `RepoService.dispose` 에서만 compare·diff 캐시를 비우고(해시 기준 불변) Rust 도 `git:commit` 캐시와 마찬가지로 앱 수명 동안 둔다.
- **검증(보안 경계)** — renderer 값이 argv 에 들어가는 길은 둘뿐이고 둘 다 검증된 newtype 이다:
  - 해시(`hash`·`parent`·`rev`·`from`·`to`): `HexHash`(16진수 4~64자). 틀리면 `invalid commit hash|parent hash|from hash|to hash`(ERROR). `null`·숫자도 거부한다(키가 아예 없어야 생략이다).
  - 파일 경로(`path`·`oldPath`): `FilePath` — **문자열이고 비어 있지 않고 NUL 이 없다**. TS(`assertPath`·`fileDiff`)의 규칙 그대로이며 **길이·절대 경로·`..` 는 TS 에도 없어** 거르지 않는다
    (`../a.txt`·`/a.txt` 는 diff·history 에서 git 이 `GIT` 오류로 거부하고 blame 은 `missing` 이다 — 골든이 TS 와 같음을 고정한다). 틀리면 `invalid path`.
  - git 이 돌려준 해시(`rev-parse HEAD`·`rev-list --parents`)도 `HexHash` 로 다시 검사한 뒤 argv 에 넣는다.
  - 요청 객체가 아니면(`null`·문자열·숫자) ERROR 다 — 사이드카로 넘기지 않는다.
  - `fileHistory` 의 `skip`·`count` 는 오류 없이 맞춘다(TS `Number.isInteger`): skip 은 양의 정수만 쓰고 아니면 0, count 는 정수면 1~500, 아니면 100.
- **argv 위치**: 서브커맨드·옵션은 상수다. 파일 경로는 **항상 `--` 뒤**(fileHistory 의 rev 는 `--end-of-options` 뒤)이고, 유일한 예외는 blame 의 크기 조회 `cat-file -s <16진수>:<경로>` 로 경로가 해시 접두가 붙은 **한 인자** 안에 있어 옵션이 될 수 없다.
  `GIT_LITERAL_PATHSPECS=1`(실행기 `GIT_ENV`)이라 `:(glob)`·`:(top)`·`:!`·`*`·`[ab]`·`?` 는 리터럴이다. `--no-textconv`(diff·blame)·`--no-ext-diff`(diff·log) 는 TS 가 붙이는 곳에 그대로 붙었다
  (`log --follow` 에 `--no-textconv` 가 없는 것도 TS 와 같다 — name-status 만 읽어 내용 변환이 돌지 않는다).
  `-c alias…`·`--output=…`·`--`·`-` 로 시작하는 경로가 옵션 자리에 오지 않음은 `file_channels.rs` 의 argv 단위 테스트와 `tests/channels.rs` 가 증명한다
  (적대적 해시·경로·요청 모양은 git 을 띄우기 전에 거부되고, 실제 저장소에서는 옵션 모양 경로가 리터럴 경로로만 쓰이며 마커 파일이 생기지 않는다).
- **골든**: `tests/fixtures/repos/files.fi`(이름 변경 이력·삭제·바이너리·한글/공백 경로·병합·작성자 둘·4000줄 파일(diff 3,000줄 초과)·512KiB 넘는 파일·glob 문자 이름 `[ab].txt`·옵션 모양 이름 `--output=pwned.txt`)와
  `files.scenarios.json`(채널별 시나리오 — 루트·병합(첫 부모)·잘림과 full·없는 경로·잘못된 해시·주입 입력 등 compare 17·fileDiff 51·fileHistory 50·blame 35)을 TS(`goldenChannels.test.ts`)와 Rust(`channels.rs`)가 돈다.
  결과는 `tests/golden/channels/{compare,fileDiff,fileHistory,blame}.json` — 오류는 code 만(시나리오에 `msg:true` 면 검증 문구도), diff 는 hunk 줄이 40개를 넘으면 개수·처음·마지막만 남긴다.
  `$upper`·`$cat` 식이 시나리오 값에 더해졌다. Windows 의 git 은 `:`·`*` 가 든 경로를 `fast-import` 에서 거부(`invalid path`)하므로 fixture 에는 `[ab].txt` 를 쓰고, 매직 경로(`:(glob)*.txt` 등)는 요청 쪽에서만 보낸다.
  20만 줄 한계를 넘어 git 을 끊는 경로(`totalIsLowerBound`)는 골든 fixture 가 너무 커져 `channels.rs` 가 테스트 시점에 210,000줄 파일을 만들어 따로 본다.
- **WSL**: 이 채널들은 자유 형식 문자열(공백·따옴표·백슬래시·한글)을 처음으로 `wsl.exe` argv 에 싣는다. `tests/exec.rs` 의 `wsl_통합` 이 그런 경로가 한 인자로 그대로 도착하는지 본다(배포판이 없으면 건너뜀).
- 앱 확인(`drive.mjs`, `ANTLER_ROUTE_LOG=1 ANTLER_DRIVE_DEBUG=1` 로 네 채널이 `-> rust` 임을 확인, `ANTLER_RUST_GIT=0` 이면 `-> sidecar(…)`): 저장소 하나를 고정 클론해
  Rust 경로·`ANTLER_RUST_GIT=0`·Electron 에서 같은 시나리오를 돌렸다. `drive.mjs` 에 파일 히스토리·blame(diff 머리의 버튼)과 두 커밋 비교(Shift+클릭) 단계가 새로 들어갔다.
  세 모드의 출력이 같다 — diff `log_service.rs` 67줄, 파일 히스토리 `mod.rs` 4개 항목(같은 제목들), blame 8행(같은 해시·작성자), 비교 13개 파일 `+599−60`(`715edde → 29bf645`).
  (상대 시간 문구만 실행 시각에 따라 다르다.) 1회씩이라 시간(ms)은 비교하지 않았다.

## fetch 가드 판정 로직(Phase 3, `src-tauri/src/git/fetch/`)

fetch 는 이 앱이 저장소에 하는 유일한 쓰기라 가드(허용 목록·URL 판정·확인 창 표시·오류 분류)를 **1:1 로** 옮긴다 — 더 엄격하게도, 느슨하게도 바꾸지 않는다(바꾸려면 TS 에 먼저 고친다).
판정 로직(이 절)은 git 을 실행하지 않는다. 실행(`git config`·`remote`·`fetch`)은 바로 아래 "fetch 실행"이다(둘 다 앱에서는 아래 라우팅이 부른다). 상태 기계(진행·큐·백오프·승인 저장)는 아래 "fetch 상태 기계"(`service.rs`, PR C)이고, 라우팅(앱 연결)은 아래 "fetch 라우팅 — PR D"다.

- **파일**: `guard.rs`(`fetch.ts` 의 순수 함수), `confirm_text.rs`(`confirmText.ts`), `pattern.rs`(허용 목록 매처), `special_table.rs`·`lowercase_table.rs`(생성 파일).
- **허용 목록은 regex 크레이트 없이** TS 정규식 소스 문자열(`ALLOWED_KEY_SOURCES`)을 그대로 두고 `pattern.rs` 의 작은 매처가 해석한다.
  아는 문법은 `^…$`, `\.`, `.+`, 리터럴, `(a|b)`, `(…)?` 뿐이고(`.` 는 `\n`·`\r`·U+2028·U+2029 를 맞추지 않는다), 그 밖의 토큰이 소스에 들어오면
  TS(`tests/goldenFetch.test.ts` 의 문법 검사)와 Rust(컴파일 단위 테스트) 양쪽이 실패한다 — 그 정규식은 새로 옮긴다.
- **골든** `tests/golden/fetch/*.json`(TS 가 기준: `UPDATE_GOLDEN=1 npx vitest run tests/goldenFetch.test.ts`, 이어서 `node scripts/gen-fetch-special.mjs`):
  `lists`(소스·값 표·손으로 옮긴 정규식 소스), `allowed`(접두×가운데×접미 그리드·값 근접 오답·refspec), `urls`, `config`, `inspect`(가짜 실행기로 돌린 `inspectConfig`·지문),
  `confirm`(표시·`SPECIAL` 범위), `misc`(오류 분류·백오프·버전·`-c` 인자·환경·stderr 판정), `fuzz`(TS 가 결정적 LCG 로 만든 **입력 문자열 자체**와 `isAllowedConfig`·`fetchRefspecOk`·`unsafeRemoteUrl`·키 소문자화 결과 — Rust 는 생성 규칙을 따라 하지 않고 적힌 문자열을 읽는다).
  `src-tauri/tests/fetch_guard.rs` 가 전부 읽어 비교하고, 처리기 없는 골든이 있으면 실패한다.
  입력 표는 `tests/fixtures/fetchGuardCases.ts` 를 `tests/fetch.test.ts` 와 골든 생성기가 함께 쓴다.
- **안전 방향**: 판정 그리드 전체에서 "Rust 허용 ⇒ TS 허용"을 따로 센다.
- **JS 와 다른 곳**(각각 `위험N_…` 테스트가 있다): `trim`·`\s` 는 U+FEFF 를 공백으로 보고 U+0085 는 아니다 / `$` 는 맨 끝만 / `i` 플래그는 ASCII 만 접는다(`ſ`·`K` 는 `s`·`k` 가 아니다) /
  길이·자르기는 UTF-16 단위(이모지 경계에서 짝이 쪼개지면 TS 는 `\ud83d` 로 드러내고, Rust 안쪽은 `&[u16]` 로 같게 처리한다) / `toLowerCase` 로 길이가 바뀌는 문자(`İ`)가 있는 refspec 의 어긋남까지 그대로 / NT 네임스페이스(`\??\`)는 URL 어디에 있어도 거부.
- **Unicode 판은 Node 에 고정한다**: 소문자화(`js_to_lowercase` — Node 변환표 + 최종 시그마용 Cased·Case_Ignorable 범위)와 `SPECIAL` 은 모두 Node 에서 뽑은 생성 표(`lowercase_table.rs`·`special_table.rs`)만 쓰고, Rust std 의 `to_lowercase` 는 쓰지 않는다
  (std 는 Node 가 모르는 대소문자 쌍 48개 — U+A7CE→A7CF 등 — 을 바꿔 `remote."꟎".fetch` 가 TS 는 "알 수 없는 설정", Rust 는 허용으로 갈렸다). 표는 `lists.json` 의 `runtime.unicode` 판에 묶인다.
  그래서 CI(`build.yml`)의 Node 를 골든을 만든 판(**22.17.1 = Unicode 16.0**)으로 고정한다 — `"22"` 로 두었을 때 러너의 최신 22.x(Unicode 17.0)가
  골든과 달라 실패했다. 골든을 다시 만들 때는 같은 판의 Node 로 만들고(`UPDATE_GOLDEN=1 npx vitest run tests/goldenFetch.test.ts` 후 `node scripts/gen-fetch-special.mjs`),
  `build.yml` 의 판도 함께 올린다. 다른 판의 Node 로 로컬 테스트를 돌리면 이 골든 비교만 실패한다.
- **매처 메모리**: `pattern.rs` 는 입력을 `Vec<char>` 로 풀지 않고 바이트 오프셋 비트셋으로 판정한다(악성 설정의 수천만 자 키에서도 입력 크기의 1/8 짜리 집합 몇 개뿐).
- **뒤 단계가 따를 API 계약**(`guard.rs`): 범위별 설정 읽기는 `ScopeWalk::start()` → `scope()` 실행 → `feed(…)` 가 다음 `WalkStep::Read` 나 `Done` 을 돌려주는 값 이동 방식이라 순서를 어길 수 없고,
  `http.followRedirects` 는 `FollowRedirects` 열거형, `core.hooksPath` 는 `hooks_path_for` 가 만드는 `HooksPath`(빈 값·상대 경로 불가, 로컬은 존재하는 빈 디렉터리 — 플랫폼을 주입하는 `hooks_path_for_test` 는 `doc(hidden)` 테스트 전용이다)다.
  TS 보다 엄격한 유일한 지점이다(TS 는 빈 값·상대 경로를 거부하지 않는다). 훅 경로는 앱이 정하는 값이라 정상 경로에는 영향이 없고,
  "Rust 가 TS 보다 넓게 허용하지 않는다"는 원칙에 맞다. 그래서 `fetchConfigArgs` 골든 중 `hooksPath: ""` 9건은 값 비교 대신 빈 값 거부를 따로 확인한다.
- 알려진 차이: TS `ALLOWED_VALUES[key]` 는 일반 객체 조회라 키가 `constructor`·`__proto__` 면 `TypeError` 를 던진다(git 키는 점이 있어야 해 실제로는 오지 않는다). Rust 는 허용 목록에 없는 키로 본다.

### fetch 실행(`run.rs`)

`inspectConfig`(`config --list --show-scope --includes -z` → `--show-scope` 를 모르면 `ScopeWalk` 로 범위별 읽기 → `remote` / `remote get-url --all -- <n>`)와 `runFetch` 의 이식이다.
git 은 `GitRunner` 트레이트로만 부른다(실제는 `GitExecutor`, 테스트는 옛 git 을 흉내 내는 래퍼를 끼운다 — 프로세스를 띄우는 길은 늘지 않는다).
local·WSL 저장소만 다룬다. ssh·github fetch 와 `filter`·`FollowRedirects::False`·임의 env 는 이 API 에 없다(사이드카 몫).

- **env 는 닫힌 타입으로만**: local 은 `GIT_ENV` 위에 덮어쓰고, WSL 은 `env K=V…` 순서를 JS `{...GIT_ENV, ...fetchEnv}` 와 같게(이미 있는 키는 제자리에서 값 교체, 새 키는 fetch 환경 순서로 뒤에) 만든다 —
  `tests/golden/exec/env.json` 의 `fetch` 가 TS `runFetch` 가 실제로 spawn 에 준 argv·환경(local)과 wsl.exe argv 를 담고 `tests/exec.rs` 가 비교한다.
- **훅 경로는 run.rs 가 실행기 종류로 만든다**(호출자가 `HooksPath` 를 넘길 수 없다): local 은 앱 소유 빈 디렉터리(드라이브 문자 경로, Windows 에서 없으면 거부), WSL 은 `/dev/null` — Windows 경로가 WSL 로 갈 수 없다.
  경로는 `canonicalize` 하지 않는다(Windows 에서 `\\?\` 가 붙는다). `hooks_path_for` 는 `//` 로 시작하거나 `/??/` 가 든 경로(UNC·NT 네임스페이스)를 거부한다(TS 보다 엄격한 두 번째 지점. 앱이 정하는 값이라 정상 경로에는 영향이 없다). Windows 에서는 **드라이브 문자 경로(`X:/`)만** 인정한다 — `/tmp/no-hooks` 같은 루트 상대 경로는 git.exe 가 현재 드라이브 기준으로 풀어 저장소 드라이브의 경로가 되므로 거부한다(세 번째 지점. 그 밖의 플랫폼은 `/…` 만 인정).
- **인자 순서**: 모든 `-c` 는 `fetch` 앞, `--refmap=`(빈 값) 필수, 같은 `-c` 키 중복 없음, remote 이름은 `--` 뒤와 refspec 안에만. 옛 git 이 `--no-write-fetch-head` 를 모르면 그 옵션 없이 한 번 더 한다.
- **`GIT_ASKPASS=''` 는 실제 git 에 빈 값으로 도착한다**(local·WSL 실측: 환경에 `GIT_ASKPASS=` 가 보이고, 저장소 설정의 `core.askpass` 스크립트가 실행되지 않는다 — fetch 환경이 없는 대조군에서는 실행된다).
  git 은 `GIT_ASKPASS` 가 비어 있으면 `core.askpass` 로 물러서지 않는다. 확인은 `tests/fetch.rs` 의 `위험9_…`(alias 셸이 본 자식 환경과 효과 둘 다 — git.exe 자신의 `getenv` 를 직접 읽은 것은 아니다).
- **ssh 명령**: 사용자(시스템·전역) `core.sshCommand` > (**local 만**) 환경 `GIT_SSH_COMMAND` > `GIT_SSH` > BatchMode 기본. 환경은 `std::env::var` 로 이름을 정해 읽는다(Windows 는 대소문자 무시 — `vars()` 로 맵을 채우면 `Git_Ssh_Command` 를 놓친다).
  Windows 에서 부모가 `Git_Pager` 처럼 다른 철자로 갖고 있으면 자식 환경의 이름이 그 철자로 남지만 값은 실행기 것이다(git.exe 는 대소문자를 구분하지 않아 문제없고, MSYS 셸 자식만 이름을 철자대로 본다).
- git 버전은 실행기별(`cache_key`: local 은 실행 파일, WSL 은 배포판)로 한 번만 읽고 실패도 기억한다(`GitVersions`).
- **확인 창 인자**: TS `fetchConfirmOptions(name, items)`(순수 함수, `confirmText.ts`; `handlers.ts` 는 이것을 부르기만 한다 — 동작 불변은 `goldenFetch.test.ts` 가 분리 전 인라인 코드의 사본과 비교해 증명)와
  Rust `fetch_confirm_plan` 이 같은 인자를 만든다. 사이드카 shim 이 만드는 `MessageBoxArg`(본문 = message + 빈 줄 + detail, `defaultId` 는 가지 않는다)는 shim 을 실제로 불러 얻은 `confirmBox.json` 골든으로 고정한다.
- **테스트**: `src-tauri/tests/fetch.rs`(실제 git, 127.0.0.1 가짜 서버만). TS `tests/fetch.test.ts` 의 모든 `it` 이 `tests/golden/fetch/test-map.json` 에 있어야 하고(vitest 가 강제),
  각 항목은 Rust 테스트 이름이거나 `N/A: 사유`다. `fetcher.fetch(...)` 를 거치는 시나리오는 이 파일에서는 검사·실행 부분만 옮겼고, 상태 기계(진행 합치기·확인→재검사 TOCTOU·승인 저장·백오프·authPaused)는 `fetch_service.rs`(PR C)가 옮겼다 — 지금 표는 71개 전부 이식이고 N/A 가 없다.
  전역 git 설정 파일(커밋된 `tests/gitconfig`)은 고치지 않는다 — 전역 값이 필요한 시나리오는 가짜 실행기의 `--global` 응답으로 준다. WSL 의 git 은 격리를 받지 않으므로 `/tmp` 저장소로만 돌린다.

### fetch 상태 기계(`service.rs`, PR C)

`FetchService.ts` 의 이식이다(GitHub 원격 전용 갈래는 사이드카에 남는다). 앱에 연결하는 라우팅은 아래 "fetch 라우팅 — PR D"다. 외부 의존은 `FetchHost` 트레이트(`emit`·`confirm`·`approve`·`note`·`repo_name`)로 주입하고,
git 은 `GitRunner` 로 주입한다(운영은 `GitExecutor`, 주기·백오프·60초 검증은 프로세스를 띄우지 않는 가짜).

- **진행·큐**: 저장소당 진행 중인 요청은 하나(`fetch()` 가 같은 `FetchFuture`(`Shared`)를 돌려준다 — 두 번째 호출의 `manual` 은 TS 처럼 무시된다). 본체는 spawn 한 태스크라 기다리는 쪽이 사라져도 끝까지 가고,
  패닉하면 `fetching=false` 로 되돌려 fetch 하지 않은 채 끝난다. 환경(`source_key`)마다 전용 큐 슬롯이 하나(`ProcessQueue::new(1)`, TS `fetchQueue`)다.
- **60초 제한은 요청 시점부터**다 — 큐에서 기다린 시간과 확인 창이 떠 있던 시간을 포함한다(TS 와 같다. 확인 창은 취소되지 않고, 닫힌 뒤 다음 git 호출이 취소를 보고 "시간 초과(60초)"로 끝난다).
- **흐름**: 검사 → remote URL 거부(확인 없음) → 위험 설정이면 수동 fetch 에서만 확인 → **실행 직전 재검사**(TOCTOU: 확인 중에 위험 항목이 늘었거나 remote URL 이 거부 대상이 되면 멈춘다) → fetch.
  ref 시그니처(`for-each-ref`·`symbolic-ref`·`rev-parse HEAD` 의 원문 — TS 의 sha1 요약과 같은 동등 비교)가 달라졌으면 **note(`fetchChanged`)를 먼저, 그다음 `repo:changed`** 를 낸다.
- **실패 분류**: 인증·허용 안 된 전송은 `authPaused`(수동 새로고침 전까지 자동 중단), 네트워크·시간 초과는 지수 백오프(주기×2ⁿ, 30분 상한), 수동 fetch 로 승인되지 않은 http(s) remote 의 자동 인증 실패는 `needsManual`.
  오류 문구는 300 UTF-16 단위로 자른다(서로게이트가 쪼개지면 U+FFFD — `slice_utf16_lossy`).
- **자동 fetch**: 활성 저장소만. 선택 3초 뒤 → 주기마다, 창 포커스 때 마지막 시도가 주기보다 오래됐으면 즉시. 시계는 `tokio::time::Instant` 라 `start_paused` 테스트가 움직인다.
  **첫 `fetchPrefs` 를 받기 전에는 주기를 몰라 예약하지 않는다**(기본값 5분을 정하는 곳은 사이드카 한 곳이다).
- **확인 창 응답 해석은 타입에 묶여 있다**(`confirm_text.rs`): `fetch_confirm_plan` 이 `ConfirmPlan::Blocked`(항목이 12개를 넘어 "닫기"만 있는 차단 창)·`Ask`(이번만·항상·취소)를 만들고,
  `decide(응답)` 이 `Decision{Once, Always, Deny}` 를 낸다 — 차단 창은 어떤 응답이든 `Deny`, 확인 창은 0 → `Once`·1 → `Always`·그 밖(취소·실패 `None`)은 `Deny`(TS `handlers.ts` 와 같다).
  `FetchHost::confirm` 은 눌린 버튼 번호만 돌려주고 허용 여부는 서비스가 `decide` 로만 정한다. 저장소 이름(창 제목)은 `FetchHost::repo_name` 으로 받는다(출처는 미러의 `name` — 아래 "fetch 라우팅 — PR D").

#### 승인 저장과 프로토콜

승인(`fetchApprovals` 수동 fetch 로 승인된 remote 지문, `trustedConfigs` "항상 허용"한 위험 설정 지문)은 **main(사이드카의 `ConfigStore`)만 쓴다.** Rust 는 쓰지 않고 부탁만 한다.

| 방향 | 메시지 | 내용 |
|---|---|---|
| Node → Rust | `{t:"fetchPrefs", gen, autoFetchMinutes, approvals:{repoId:지문}, trusted:{repoId:지문}}` | 별도 `FetchPrefsSender`(`src/sidecar/fetchPrefs.ts`, gen 은 미러와 따로 1 부터). `config.onChange` 에서 `mirror.push` **다음에**, 시작 때 ready 전에. 같은 내용이면 보내지 않는다. 주기는 기본값(5) 적용 후이며 0 이면 끈다. 설정에 있는 저장소의 승인만 담는다. 기존 `MirrorSender`·`on_mirror_changed` 는 그대로다 |
| Rust → Node | `{t:"fetchApprove", kind:"approval"\|"trusted", repoId, fingerprint}` | 사이드카(`src/sidecar/fetchApprove.ts`)가 kind 허용 목록(`includes`)·`config.repo(repoId)` 존재·지문 모양(소문자 hex 64자)을 검사한 뒤 `setFetchApproval`/`setTrustedConfig`. 해제(null)는 받지 않는다 |
| Rust → Node | `{t:"note", op:"fetchChanged", repoId}` | `RepoService.noteLog` → 기억한 시그니처를 버린다. 항상 `repo:changed` 보다 먼저 |

- **승인 경로는 `fetchApprove` 하나다. renderer 의 `req`(채널)로는 절대 받지 않는다**(채널 허용 목록에 없고, 메시지의 `t` 는 Rust 가 정한다). 보내는 곳은 서비스의 두 지점뿐이다 —
  수동 fetch **성공 뒤**(`approval`: 그 remote 의 이름+URL 지문)와 확인 창에서 **"이 저장소에서 항상 허용"** 을 받은 뒤(`trusted`: 위험 항목 지문). `tests/fetch_service.rs` 와 `tests/sidecarFetchPrefs.test.ts` 가
  소스를 훑어 이를 강제한다(`host.approve(` 한 곳·`send_approval(` 호출 둘·`"fetchApprove"` 리터럴 한 곳·승인을 쓰는 TS 코드는 ConfigStore·Electron FetchService·fetchApprove 핸들러뿐). `config:setUi` 로 승인을 위조할 수 없다는 기존 TS 테스트(`fetch.test.ts`)도 그대로다.
- **판정은 항상 "지문이 같은가"**다(항목이 있는가가 아니다): 자동 fetch 의 자격 증명 도우미는 `fetchPrefs.approvals[repo] == 지금 remotes 지문`일 때만 켜지고, 위험 설정은 `trusted[repo] == 지금 위험 항목 지문`이거나 이번 실행에서 허용한 지문일 때만 통과한다.
- **메모리 오버레이**: 방금 부탁한 값은 사이드카가 `fetchPrefs` 로 되돌려 주기 전까지 Rust 메모리에 쥔다(그 사이의 자동 fetch 가 승인을 놓치지 않게). 같은 값 또는 다른 값이 돌아오면 정리하고, 값이 없는 스냅샷(처리 전)은 남겨 두며(안전한 쪽),
  저장소가 사라지면(`forget`·미러에서 빠짐) 지운다. gen 이 이미 받은 것 이하인 `fetchPrefs` 는 버린다.
- **소유권**: 사이드카 `FetchService` 에 `owns(repoId)` 술어가 생겼다(기본 전부 소유 — Electron 판 동작 불변). 소유하지 않은 저장소는 `fetch` 가 ERROR(거부)이고 `setActive` 는 null 로 취급한다(이중 fetch 방지).
  술어를 거는 배선(Rust 가 맡는 local·WSL 을 뺀다)은 아래 "fetch 라우팅 — PR D"다.

#### PR B 보안 리뷰의 잠재 결함 4건(이 PR 에서 막음)

1. **플랫폼 주입**: `run_fetch_with`·`fetch_config_for`·`fetch_env_for`·`hooks_path_with` 가 플랫폼 값을 호출자에게 받던 것을, 운영 경로는 항상 `cfg!(windows)` 인 진입점(`run_fetch`·`fetch_config_for`·`fetch_env_for`·`hooks_path_for`)으로 줄이고
   주입 버전은 `#[doc(hidden)]` + `*_for_test` 로 바꿨다(통합 테스트가 별도 크레이트라 `#[cfg(test)]` 는 쓸 수 없다). 운영 코드(`service.rs`·`sidecar.rs` 등)에 그 이름이 없음을 `fetch_service.rs::운영_코드는_플랫폼을_주입하지_않는다` 가 소스로 강제한다.
2. **확인 창 응답**: 위 `ConfirmPlan`/`Decision`. 골든(`confirmBox.json`)은 그대로 통과한다.
3. **WSL askpass 실측**(`tests/fetch.rs`)에 `-c credential.helper=` 를 넣어 사용자의 실제 WSL 자격 증명 도우미가 불리지 않게 했다.
4. **훅 경로**: Windows 는 드라이브 문자 경로만 인정(위 "fetch 실행"). `tests/fetch.rs` 의 `/tmp/no-hooks` Windows 통과 단언을 거부 단언으로 고쳤고, 골든 `env.json` 의 POSIX 값(`/tmp/no-hooks`)은 값의 모양으로 플랫폼을 골라 통과시킨다.

#### 테스트

`src-tauri/tests/fetch_service.rs`: 가짜 git + 멈춘 시계(`start_paused`, dev-dependency tokio `test-util`)로 주기·첫 지연·백오프·60초(확인 창·큐 대기 포함)·환경별 큐·포커스·활성 전환·취소·패닉·오버레이·응답 해석을,
실제 git(127.0.0.1 401 서버, 명령줄 범위 `GIT_CONFIG_COUNT` 로 심은 자격 증명 도우미)으로 TS 의 PR C 몫 시나리오 7개를 옮겼다. 멈춘 시계는 런타임이 한가하면 다음 타이머로 건너뛰므로 프로세스를 띄우는 테스트와 섞지 않는다.
`test-map.json`: TS `fetch.test.ts` 의 71개 `it` 이 모두 Rust 테스트에 대응한다(N/A 없음).

### fetch 라우팅 — PR D

Tauri 판의 **local·WSL 저장소 fetch** 를 Rust 가 맡는다(ssh·github 는 사이드카 그대로). 켜고 끄는 스위치는 라우터와 같은 `ANTLER_RUST_GIT`(`0` 이면 전부 사이드카)이다.

- **라우팅 채널**: `RUST_CHANNELS` 에 `git:fetch` 가 늘었다(`git:commit`·`git:log`·`git:compare`·`git:fileDiff`·`git:fileHistory`·`git:blame`·`git:fetch`). `git:fetch(repoId, manual)` 의 `manual` 은 **정확히 `true` 일 때만** 참이다
  (`handlers.ts` 의 `manual === true` 와 같다 — 문자열·숫자·배열·빈 자리는 자동 fetch). 미러에 있는 local·wsl 이면 Rust, 미러에 없거나 ssh·github 면 사이드카다.
  미러 검사에 걸린 local·wsl 저장소(UNC 경로 등)와 서비스가 없을 때는 **사이드카로 보내지 않고 ERROR** 를 낸다 — 사이드카는 그 환경을 맡지 않아 보내면 이중 경로가 된다.
- **자동 fetch 배선**(라우터 → `FetchControl` 포트): `repos:watch` 가로채기에서 `set_active`(미러에 있고 Rust 가 맡는 저장소만, 아니면 비운다), 창 포커스(`lib.rs` 의 `Focused`) → `on_focus`,
  `fetchPrefs`(`FetchPrefsSink`) → `apply_prefs`(주기 변경은 `config:setUi` → 사이드카 설정 변경 → `fetchPrefs` 로 와 재예약된다), 미러가 바뀔 때 `sync_mirror`(사라졌거나 Rust 가 맡지 않게 된 저장소는 `forget`),
  앱 종료 때 `dispose`(진행 중인 fetch 취소).
- **런타임**: 예약·fetch 시작은 tokio 태스크를 만든다. 사이드카 읽기 스레드(`fetchPrefs`·미러 변경)와 창 이벤트 스레드(포커스)는 런타임 밖이라 `router::in_runtime_of(앱 런타임 핸들, …)` 로 들어가서 부른다
  (`tests/fetch_route.rs` 가 활성 저장소가 있을 때 런타임 밖 호출은 패닉하고 들어가서 부르면 안전함을 보인다).
- **호스트**(`src-tauri/src/fetch_host.rs`): 이벤트는 `antler:evt` 로 `fetch:status`·`repo:changed`(사이드카 이벤트와 같은 `{name,payload}`), 확인 창은 `dialogs::message_box`(Rust 네이티브, `spawn_blocking`)로 띄우고
  실패·닫기는 `None` = 거부(`decide`), 승인은 `Sidecar::approve`, 알림은 `Sidecar::note`(`fetchChanged` 가 `repo:changed` 보다 먼저). 저장소 이름(확인 창 제목)은 **미러의 `name`**(비밀 아님 —
  `{id,kind,path,name,distro?}`; 이름만 바뀌어도 미러가 다시 간다)이고 없으면 id 다.
- **소유권(이중 fetch 방지)**: 라우터가 켜져 있을 때만 사이드카를 env `ANTLER_RUST_FETCH_KINDS=local,wsl` 로 띄운다(`router::sidecar_fetch_kinds`; 꺼져 있으면 부모 환경의 값도 지워서 띄운다). 사이드카(`src/sidecar/fetchOwns.ts`)는
  이 값으로 FetchService 의 `owns` 를 정해 local·wsl 저장소를 **fetch 도 자동 예약도 하지 않는다**(알 수 없는 토큰은 무시 — 아무도 안 맡는 일이 없게). `ANTLER_RUST_GIT=0` 이면 env 도 Rust 서비스도 없어 사이드카가 전부 맡는다(되돌리기).
  Electron 판은 `owns` 기본(전부)이라 동작이 그대로다. 증명: `tests/sidecarFetchOwns.test.ts`(사이드카가 소유하지 않은 저장소는 fetch·setActive·onFocus 에서 아무것도 하지 않는다)와
  `tests/fetch_route.rs`(Rust 라우터는 local·wsl 의 `git:fetch` 를 절대 사이드카로 보내지 않고, 스위치가 꺼지면 서비스를 주입하지도 건드리지도 않는다).
  WSL 은 Windows 에서만 Rust 가 맡는다(미러가 그 밖의 OS 에서는 사이드카로 돌린다) — Tauri 판은 Windows 전용이라 다른 OS 의 WSL 저장소 fetch 는 어느 쪽도 하지 않는다.
- **PR C 보안 리뷰 후속**: (1) `FetchPrefsSink` 는 `Sidecar::spawn` **보다 먼저** manage 한다(첫 `fetchPrefs` 는 ready 전에 한 번만 온다) — `fetch_prefs_받는_곳은_사이드카를_띄우기_전에_등록한다` 가 `lib.rs` 의 순서를 강제하고,
  서비스가 아직 없을 때 도착한 prefs 는 라우터가 마지막 것을 보관했다가 주입 때 적용한다. (2) `Sidecar::approve` 는 `pub(crate)`(+`#[allow(dead_code)]` 제거)이고, 운영 코드의 `.approve(` 는 서비스의 `host.approve(` 한 곳과
  호스트 구현의 `sidecar.approve(` 한 곳뿐임을 `사이드카에_승인을_부탁하는_호출은_호스트_구현_한_곳뿐이다` 가 소스로 강제한다.
- **채널 골든**: `tests/fixtures/repos/fetch.scenarios.json` 을 TS `FetchService`(`tests/goldenChannels.test.ts`)와 Rust 라우터(`src-tauri/tests/channels.rs`)가 같은 순서로 돌려 단계별 상태·확인 창 횟수·변경 알림·승인 저장을
  `tests/golden/channels/fetch.json` 과 비교한다(수동 성공·변경 없음, 자동 file 전송 거부, `manual` 엄격성, remote 없음, 위험 설정의 자동 차단·취소·창 실패·이번만·항상 허용·바뀌면 재질문·TOCTOU,
  UNC·`ext::` 거부와 확인 중 변경, 인증 실패(자동은 `needsManual`, 수동은 일시 정지)). 시각은 유무만, 오류 문구는 유무와 file 안내 여부만 비교한다.
  갱신: `UPDATE_GOLDEN=1 npx vitest run tests/goldenChannels.test.ts -t "channels/fetch.json"`.
- **되돌리기**: `ANTLER_RUST_GIT=0` 으로 실행하면 사이드카가 fetch 를 전부 맡는다(env·Rust 서비스 없음). 확인용 로그는 `ANTLER_ROUTE_LOG=1`(`[route] git:fetch -> rust`).

**사람 확인 필요**: 확인 창(`[이번만][항상][취소]`)이 실제로 뜨고 버튼이 맞게 동작하는지(자동화는 입력을 합성하지 않는다), WSL 저장소의 실제 fetch.

## GitHub·Jira(Phase 4, `src-tauri/src/{net,github,jira}`)

### 계획 요약

- **경계**: Rust 가 `gh:*`(`gh:repo` 제외)·`jira:*` 전부와 토큰 영속(Windows 자격 증명 관리자), API 호출·로그인을 맡는다. 사이드카에는 `gh:repo`·`repos:addGithub`·GitHub 원격 전용 저장소 git 이 남고,
  GitHub 토큰의 메모리 사본만 Rust → Node(`{t:"ghToken"}`)로 간다(Jira 토큰은 넘기지 않는다). Rust → Node 질의 `{t:"repoq",id,op}` 로 `ghRepo`·`ghRemotes` 등을 받는다.
  스위치는 `ANTLER_RUST_API`(0 이면 끔), `ANTLER_RUST_GIT=0` 이면 함께 꺼진다. 마이그레이션은 없다(다시 로그인) — Phase 6 전에 사용자 확인이 필요하다.
- **HTTP**: reqwest 0.13(`default-features=false`, `native-tls`), 클라이언트 생성은 한 곳(`net/http.rs`)에 모으고 `Client::new` 등은 clippy 로 막는다. `https_only`·`no_proxy`·15초·UA `Antler`,
  Jira·Device Flow 는 리다이렉트를 따르지 않고 GitHub API 는 origin 이 정확히 `https://api.github.com` 인 5홉까지만. 본문 상한은 Jira 5MB·사진 256KB·GitHub 32MB.
  `reqwest::Error` 의 Display 는 쓰지 않는다. 비밀은 `SecretString`(Display·Serialize 없음, Debug `***`)이고 오류 문구는 마지막에 `redact_secrets` 를 거친다.
- **PR 순서**: 1 순수 로직·골든(완료) → 2 응답 매퍼(api·checks·launchpad·accessibleRepos·adf, 완료) → 3 HTTP 코어·두 클라이언트(루프백 목 서버, 완료) → 4 SecretStore(키링·메모리, 완료) → 5 Jira 서비스(완료) →
  6 Jira 연결(채널·`openExternal`, 완료) → 7 GitHub 서비스(완료) → 8 gh CLI → 9 GitHub 연결·키체인·`ghToken`/`repoq`.

### PR 1 — 순수 로직과 골든(앱에 연결하지 않는다)

`lib.rs` 에 `pub mod net / github / jira` 가 선언돼 있을 뿐 앱 경로에서 부르는 곳이 없다(통합 테스트가 별도 크레이트라 `pub`). 네트워크·reqwest·키체인은 없다.

| 모듈 | 원본(TS) | 내용 |
|---|---|---|
| `net/json.rs` | — | `JSON.parse` 같은 관대한 해석(짝 없는 서로게이트·삽입 순서·중복 키), JS `Number(문자열)`·숫자 문자열화, IPC 인자(`serde_json::Value`) 헬퍼 |
| `net/url_rules.rs` | `jira/client.ts` | `inSite`, `PUBLIC_AVATAR_HOST_RE`(정규식 크레이트 없이 손 판정), 프로필 사진 홉 판정, 요청 URL 조립(`searchParams.set` 의 공백 → `+` 포함), IDN 안전 범위 |
| `net/http_error.rs` | `jira/client.ts`·`github/client.ts` | `parseRetryAfter`, `hasNextLink`, `parseRate`, `bodyMessage` 둘, `httpError` 둘(분류 + 문구) |
| `net/redact.rs` | `git/githubCache.ts` | `usableToken`·`redactToken`, Rust 전용 `redact_secrets`, `SecretString` |
| `github/validate.rs`·`jira/validate.rs` | `validate.ts` | IPC 인자 검증 전부(문구·검사 순서까지) |
| `github/device_rules.rs` | `deviceFlow.ts` | 시작·폴링 응답 해석 분기(`PollState`: 간격·백오프·`slow_down`·만료) |
| `github/gh_env.rs` | `ghCli.ts` | `ghEnv` 허용 목록 필터 |
| `jira/mod.rs` | `jira/client.ts` | `JiraKind`, `authHeader` |

공백은 `git::scope::is_js_space`·`git::fetch::js_trim`, UTF-16 길이는 `git::commit::utf16_len`, 소문자화는 `git::fetch::js_to_lowercase`(Node 표)를 재사용한다. 의존성은 `url = "2"`·`base64 = "0.23"`(둘 다 이미 Cargo.lock 에 있던 판).

**골든**: `tests/fixtures/netCases.ts`(입력 표) → `tests/goldenNet.test.ts` 가 TS 로 `tests/golden/net/{validate,urls,http,device,env}.json` 을 만든다(`UPDATE_GOLDEN=1 npx vitest run tests/goldenNet.test.ts`),
`src-tauri/tests/net.rs` 가 전부 읽어 비교한다(처리기 없는 골든이 있으면 실패). 시간대에 따라 달라지는 값(시간대 표기 없는 날짜·ko-KR 시각)은 골든에 값을 넣지 않는다(`"tz"`, `{TIME}`) —
`TZ=UTC`·`America/New_York`·`Asia/Kolkata` 에서 모두 통과를 확인했다. TS 쪽은 `src/main` 동작을 바꾸지 않았다(`PUBLIC_AVATAR_HOST_RE`·`inSite`·`bodyMessage`·`httpError`·`parseRate`·`usableToken` 에 `export` 만 더했다).
요청 URL·사진 홉·Device Flow 는 실제 `JiraClient`/`GitHubClient`/`DeviceFlow` 를 가짜 `fetch`·`sleep`·`now` 로 돌려 얻은 결과다. 손으로 옮긴 정규식은 소스 조각을 골든(`handWritten`)과 `net::HAND_WRITTEN` 에 적어 TS 가 바뀌면 둘 다 알아챈다.

**안전 방향**: URL 판정 격자(약 1,500개 주소 × 사이트 주소·외부 링크·사이트 안·Jira 링크·직렬화, 총 12,000여 판정)와 IDN 전수 대조에서 "Rust 허용 ⇒ TS 허용" 위반이 0 이다(위반이 있으면 실패).

**TS 와 다르게 동작하는 곳**(각각 `차이_…` 테스트나 알려진 목록 단언이 고정한다. 대부분 Rust 가 더 엄격한 쪽이고, **같은 입력을 다른 값으로 받는 곳**은 따로 표시했다):

- **IDN 호스트(Unicode 판) — 설계 결정**: Node 22 는 Unicode 16, `url` 크레이트의 idna 는 더 새 판이라 새로 배정된 문자(U+A7CE 등)·UTS 46 이 갈리는 문자(U+04C0, U+0558 등)를 Rust 만 받는 호스트가 생겼다(Rust 허용 ⇒ TS 허용 위반).
  그래서 `parse_url` 은 호스트 부분의 입력 문자(탭·개행을 지운 뒤)와 `xn--` 레이블을 해독한 문자가 모두 ASCII 이거나 `IDNA_SAFE_RANGES`(오래된 블록 17개)에 있고 `IDNA_SAFE_EXCLUDED`(전수 대조가 찾은 9자)에 없을 때만 받는다.
  호스트의 퍼센트 이스케이프 비 ASCII 바이트(`%D3%80`)·`blob:`·`ẞ` 가 든 IDN 은 거부한다(해독한 문자를 입력 쪽에서 검사할 수 없거나 origin 이 안쪽 주소의 것이어서). `resolve_location`(리다이렉트 `Location`)도 호스트가 바뀌면 같은 검사를 거친다.
  `xn--` 레이블 해독은 자체 구현(`punycode_decode`, RFC 3492)이다. 범위 안 모든 코드 포인트(약 4만 자)를 골든 `idna` 가 TS 와 같은 호스트로 푸는지 대조하고, TS 만 받는 문자(분수·`ŉ`·`ẞ`·반각 기호 20자)와
  격자의 TS 만 받는 주소 17개(잘못된 punycode `xn--github-`, `ẞ`(TS `ss`·Rust `xn--zca` 로 다른 서버), `№`·`①`·`ﬁ`·ZWSP·BOM, `blob:`, 호스트의 `%C3%A9` 등)는 목록으로 못 박았다. 탭·개행·퍼센트 이스케이프로 검사를 비켜 가려는 입력도 격자에 있다.
  idna 크레이트를 올려 새로 어긋나는 문자가 생기면 이 전수 대조가 실패한다 — 제외 목록에 더하거나 범위를 줄인다. 한계: 전수 대조는 `a{문자}b.example.com` 한 가지 문맥뿐이라 bidi(RTL) 레이블 같은 문맥 규칙의 조합은 대조하지 않았다.
  대안(상위 결정): 비 ASCII 호스트를 전부 거부하거나, idna 판을 Node 와 맞춰 고정한다.
- **`parseRetryAfter`**: 날짜는 IMF-fixdate·RFC 850·asctime 만 받는다(TS `Date.parse` 는 ISO 8601·`Mon, 31 Apr`·초 60 같은 것도 받는다 — Rust 는 `null`). 받는 것은 TS 와 같은 값이다(두 자리 연도: 0–49 → 2000년대, 50–99 → 1900년대, V8 과 같다).
  **다른 값**: asctime 은 시간대 표기가 없어 TS 는 로컬 시간, Rust 는 UTC(HTTP 규격)로 푼다 — 골든 값 비교에서 빼고 Rust 단독으로 단언한다. 숫자(초)는 `Number()` 와 같다(`0x10`·` 5 `·`1e3`·`""` 포함).
- **JSON(`net/json.rs`)**: **다른 값**: 짝 없는 서로게이트는 U+FFFD 로 받는다(TS 는 그대로 둔다. 서버가 보낸 `device_code`·`access_token`·메시지에 있으면 Rust 는 다른 문자열이 된다. 응답 문구를 UTF-16 200 단위로 자르다 쌍이 쪼개져도 U+FFFD).
  중첩 깊이 상한 64(배열 65겹까지 — 넘으면 오류, TS 는 스택이 다할 때까지 받는다. 재귀 한 겹이 스택을 1.4KB 안팎 써서 512 는 약 700KB 였다. 256KB 스택 스레드에서도 넘치지 않는지 테스트가 본다). `serde_json` 은 거부하는 짝 없는 서로게이트 이스케이프와 삽입 순서(정수형 키 먼저)를 위해 쓰지 않는다.
- **Device Flow**: 필드 타입이 틀린 응답(문자열이 아닌 `access_token`·`device_code`, 문자열 `interval`)을 TS 는 truthiness·형 변환으로 받지만 Rust 는 없는 것으로 본다. 본문이 `null` 이면 TS 는 `TypeError` 를 던지고 Rust 는 `DeviceError::TypeError` 다.
  **다른 값**: Rust 는 `interval` 을 1..=60초, `expires_in` 을 0..=3600초로 묶고 `slow_down` 의 새 간격은 유한하고 현재보다 큰 값만 받아 60초를 넘기지 않는다(`+5초`도 60초 상한). TS 는 `1e999`(Infinity)·아주 큰 값을 그대로 써 사실상 영원히 기다린다. 정상 응답(5초·900초)은 같다.
- **`inSite`**: 사이트 주소가 URL 이 아니면 TS 는 던지고 Rust 는 `false`. GitHub `httpError` 의 시각 문자열은 호출자가 넣는다(TS 는 ko-KR 로컬 시각).
- Jira `httpError` 는 코드 외에 문구까지 옮겼다(골든이 글자 단위로 비교한다). 토큰이 든 값(`authHeader` 결과·PAT·Jira 토큰)은 `SecretString` 으로 돌려준다.

**대응표**(`tests/golden/net/test-map.json`): 대상 테스트 15개(`avatars`·`ghAccessibleRepos`·`ghApi`·`ghChecks`·`ghClient`·`ghCliImport`·`ghDevice`·`ghService`·`jiraAdf`·`jiraAvatar`·`jiraClient`·`jiraHardening`·`jiraService`·`launchpadQuery`·`launchpadService`)의
모든 `it`(271개)이 `net.rs::<테스트>`, `N/A: PR n`(뒤 PR 이 채운다) 또는 `N/A: 사유` 다. **기준**: TS `it` 이 순수 함수(검증·`ghEnv`·`authHeader`·`parseRetryAfter`·매퍼)를 **직접** 부를 때만 `net.rs::` 이고,
client·service·DeviceFlow 를 가짜 서버로 구동하는 `it` 은 Rust 가 순수 판정만 옮겼어도 `N/A: PR n`(그 계층을 옮기는 PR 이 실제 테스트로 바꾼다)이다. PR 2 이후 `net.rs::` 는 46개(테스트 10개),
`N/A: PR n` 은 211개(3→41, 4→5, 5→52, 7→95, 8→18)였고 **PR 3 이 3→41 을 모두 채워 지금은 `net.rs::` 87개(테스트 39개), `N/A: PR n` 170개(4→5, 5→52, 7→95, 8→18)다**, `N/A: 사유` 는 14개(ConfigStore·renderer 의 `sanitizeHtml`·`extractIssueKeys`·`remoteUrl` 은 Rust 로 옮기지 않고, getter 예외 대체 경로는 JSON 값으로 도달할 수 없다).
`net.rs` 의 나머지 테스트(요청 URL·사진 홉·Device Flow·오류 분류 등)는 표에 묶이지 않은 추가 패리티 검사다.
PR 2 가 `N/A: PR 2`(72개)를 채웠다 — 매퍼를 직접 부르는 `it` 21개는 `net.rs::`, 가짜 `GitHubClient`/Jira 서버로 쪽 읽기·보강·캐시를 도는 `it` 은 그 계층의 PR(GitHub 서비스 7, Jira 서비스 5)로 옮겼다.
vitest 가 누락·낡은 항목·존재하지 않는 Rust 테스트를 막는다(`N/A: PR [2-9]` 형식은 따로 허용). 계획에는 "15파일 ~268 it" 만 있고 목록이 없어, `src/main/{github,jira}` 를 import 하는 테스트 중 GitHub 원격 URL 해석(`ghRemote`)·`launchpadMerge`(병합 로직)를 뺀 15개로 골랐다(it 271개).
파일별 PR 번호도 추정이다. 목록이 다르면 `TARGET_TESTS`(`tests/goldenNet.test.ts`)와 표를 고친다.

### PR 2 — 응답 매퍼와 골든(앱에 연결하지 않는다)

API 응답 JSON → 공유 타입(IPC 반환 값)으로 바꾸는 **순수 함수**만 옮겼다. 입력은 `net/json.rs` 의 `Json`(응답 원문을 자체 해석기로 읽은 값), 출력도 `Json` 이라 TS 와 같은 키 순서로 직렬화된다(`js_value::stringify`).
요청·쪽 읽기·재시도·캐시·`Date.now()` 는 뒤 PR(서비스 계층)의 몫이다. `src/main` 은 바꾸지 않았다(골든 구동기 `tests/goldenMaps.ts` 가 가짜 client 로 실제 TS 함수를 돌린다).

| 모듈 | 원본(TS) | 내용 |
|---|---|---|
| `net/js_date.rs` | — | `Date.parse`(ISO 8601 만)·`toISOString`·`toEpoch`. 일(day)이 달력을 넘으면 V8 처럼 다음 달로 밀리고, 확장 연도 `±YYYYYY`·`+0900` 꼴 오프셋을 받는다 |
| `net/js_value.rs` | — | `MapError`(TS 라면 `TypeError`), `??`·truthiness·`Number()`·`encodeURIComponent` 헬퍼, `JSON.stringify` 꼴 직렬화 |
| `github/api_map.rs` | `api.ts` | `mapPull`·`mapSearchPull`·`mapIssue`·`mapUser`, 리뷰·체크 집계, `getPull` 상세 조립, 파일·커밋·댓글·필터·이슈 목록 변환, 목록 보강 |
| `github/checks_map.rs` | `checks.ts` | `rollupState`·`rollupQuery`·일괄 조회 응답 해석, 체크 상세·워크플로 실행·잡 변환 |
| `github/launchpad_map.rs` | `launchpad.ts` | 섹션 검색어, `slugFromRepositoryUrl`(엄격한 `decodeURIComponent`), 검색 한 쪽 해석, 항목 변환·병합(중복 제거·최신순) |
| `github/accessible_repos.rs` | `accessibleRepos.ts` | `mapAccessibleRepo`·쪽 변환·`pushedAt` 정렬 |
| `jira/adf.rs` | `adf.ts` | ADF → HTML(깊이 50·출력 1,000,000 UTF-16 단위 상한) |
| `jira/api_map.rs` | `api.ts` | `avatarOf`·`mapSummary`·`getIssue`·전환·댓글(Cloud/Data Center)·개발 정보(dev-status)·검색 결과·`getMyId`·사용자 검색·`pickDateFields`·워크로그 조립 |

**골든**: `tests/fixtures/mapCases.ts`(응답 원문 표본 — `ghApi`·`ghChecks`·`ghAccessibleRepos`·`jiraAdf`·`jiraHardening`·`jiraService` 의 픽스처와 경계 사례) → `tests/goldenMaps.ts` 가 TS 로 `tests/golden/net/{ghmap,jiramap}.json` 을 만든다
(`UPDATE_GOLDEN=1 npx vitest run tests/goldenNet.test.ts`). `src-tauri/tests/net.rs` 가 `Json`(키 순서 보존)으로 읽어 **순서까지 같은지** 비교한다 — `github_응답_매퍼가_ts_와_같다`(318건)·`jira_응답_매퍼가_ts_와_같다`(260건),
날짜는 `Date.parse` 격자(172개)·`toISOString` 대조. TS 가 던진 사례(`{"err":true}`)는 Rust 도 `MapError` 여야 한다. 시간대에 따라 달라지는 날짜는 `"tz"` 로 적고 Rust 는 받지 않아야 한다(`TZ=UTC`·`America/New_York`·`Asia/Kolkata` 모두 통과 확인).
퍼즈성 사례(타입이 틀린 필드·null·깊이 64 중첩·10만 개 배열·제어 문자·모든 키에 같은 값)는 `매퍼는_이상한_입력에도_패닉하지_않는다` 가 모든 매퍼에 먹인다.
**HTML 안전**: `adf_출력은_허용_태그와_http_링크뿐이다` 가 모든 문자열 자리에 공격 문자열을 넣어도 태그가 고정 목록 안이고 `href` 가 http(s) 뿐이며 속성에 이스케이프하지 않은 문자가 없음을 본다.
TS 의 허용 태그·속성을 넓히지 않았다(링크 주소는 PR 1 의 `parse_url` 로 해석해 오히려 더 엄격하다).

**TS 와 다르게 동작하는 곳**(대부분 Rust 가 더 엄격하다. 각각 목록 단언이나 `차이_…` 테스트가 고정한다):

- **`Date.parse`**: ISO 8601 만 받는다(`YYYY`·`YYYY-MM`·`YYYY-MM-DD`·`THH:MM[:SS[.fff]]` + `Z`/`±HH:MM`/`±HHMM`, 확장 연도). TS 가 받는 그 밖의 꼴은 Rust 가 `None` → 날짜 필드 0·`''`·"범위 밖"이다:
  시간대 없는 시각(TS 는 로컬 시간이라 머신마다 값이 다르다)·공백 구분(`2026-01-01 00:00:00Z`)·소문자 `t`/`z`·RFC 2822(`Mon, 01 Jan 2026 …`)·`2026-1-1`. **다른 값**: 문자열이 아닌 입력(숫자 `12345`)을 TS 는 문자열로 바꿔 해석하지만(연도 12345) Rust 는 0 이다.
  영향: GitHub 의 `updated_at`·`created_at`(항상 `Z`)·Jira 의 `started`(`+0900` 꼴)는 같고, dev-status 의 시각 문자열이 시간대 없는 꼴이면 Rust 는 `''`·정렬 0 이다.
- **ADF 출력 상한의 잘린 꼬리**: 1,000,000 UTF-16 단위로 자른 끝에 미완성 태그(`<strong`)·엔티티(`&am`)가 남으면 Rust 는 그 자리부터 버린다(TS 는 그대로 둔다). Rust 출력은 TS 출력의 접두사다.
- **배열 자리에 문자열**: TS 는 `for…of`·스프레드가 문자열을 글자 단위로 돌아 항목을 만든다(`check_runs: "abc"`, 검색 `items: "x"`, 커밋 목록 `"x"`). Rust 는 `MapError` 다. `.map` 을 부르는 자리(`labels: "x"`)는 TS 도 던진다.
- **타입이 틀린 스칼라**(`title: 5`)는 TS 처럼 그대로 흘린다. `.toUpperCase()` 를 부르는 자리(리뷰 `state`)에 문자열이 아닌 값이 오면 TS·Rust 모두 오류다.
- **짝 없는 서로게이트**는 U+FFFD 다(PR 1 과 같다). ADF 출력을 UTF-16 1,000,000 단위에서 자를 때 쌍이 쪼개져도 U+FFFD 다(TS 는 반쪽을 그대로 둔다 — 골든은 `toWellFormed` 한 꼬리를 비교한다).
- **ADF**: JSON 깊이 상한(64)이 있어 텍스트로 읽은 ADF 는 약 30겹까지다. 깊이 50 상한은 코드로 만든 값으로 시험한다. 예외 시 평문 한 문단으로 대체하는 TS 경로는 옮기지 않았다(JSON 값만으로는 던질 수 없다).
  `safeHref` 는 `new URL` 대신 `parse_url`(IDN·`blob:` 를 더 엄격하게 거른다)을 쓴다. `Number(attrs.x)` 는 `null`·불리언·문자열·배열(`[2]`)·객체까지 JS 와 같게 바꾼다.
- **`getPull`/`listPulls` 조립**: 체크 변환이 던지면 TS 는 `soft()` 로 삼켜 `checks` 키를 빼고, 리뷰 목록이 배열이 아니면 `listPulls` 는 `review` 키를 뺀다. 조립은 서비스 계층이 하므로 매퍼는 `Result` 를 그대로 돌려주고, 골든 구동(`run_gh_case`)이 같은 규칙을 적용한다.
  **PR 7 에서 라이브러리로 옮겼다**: 규칙은 `github::queries::{soft_checks, reviews_page}` 이고 서비스(`list_pulls`·`get_pull`)와 `run_gh_case` 가 같은 함수를 부른다.
- **리뷰 판정의 사용자 키**는 `login` 값 그대로다(숫자 5 와 문자열 `"5"` 는 다른 사용자). `toUpperCase` 는 Rust 표준 라이브러리의 유니코드 표다(Node 와 새 문자만 다를 수 있다).
- **오류 문구**: TS 가 던지는 `Error`(`잘못된 페이지 표지입니다`·`Jira 계정 정보를 읽을 수 없습니다`)는 `MapError` 하나로 합쳤고 골든도 "던졌다"만 비교한다(PR 1 의 `httpError` 와 달리 문구를 비교하지 않는다) — 사용자에게 보이는 문구는 서비스 계층(Jira 5)이 정한다.
- **그대로 흘리는 객체 값**: TS 는 `title`·`number` 같은 필드가 객체여도 그대로 돌려주고 Rust 도 같다. 단 JS 객체의 정수형 키 선행 정렬은 Rust 의 `Json::Obj`(삽입 순서)가 출력 시점에 하지 않는다(필드가 객체일 때만 보이는 차이).
  워크로그가 2만 건을 넘으면 TS 는 이슈 병렬 수집의 도착 순서에 따라 자르는 지점이 달라질 수 있고, Rust 는 이슈 순서대로 앞에서 자른다(`truncated` 는 같다).
- 퍼즈·성능: Jira 매퍼도 모두 이상한 입력 시험에 들어 있고(`fuzz_jira`), 표시 20만 개·PR/커밋/리뷰어/워크로그 각 10만 개 입력이 디버그 빌드에서 수 초 안에 끝나는지 `큰_입력은_제곱_시간이_아니다` 가 본다(ADF 표시 감싸기·dev-status 중복 제거·리뷰 판정은 제곱 시간이 되기 쉬워 색인·누적으로 구현했다).
- Jira `search` 의 Data Center 쪽 표지는 `Number(cursor)` 를 `js_number` 로 해석한다(`0x10`·`1e1`·` 5 ` 도 TS 와 같고, 정수·0..=200 이 아니면 오류).

### PR 3 — HTTP 코어와 Jira·GitHub 클라이언트(앱에 연결하지 않는다)

| 모듈 | 원본(TS) | 내용 |
|---|---|---|
| `net/http.rs` | — | reqwest 클라이언트를 만드는 **유일한 곳**(`#[allow(clippy::disallowed_methods)]` 한 군데). `https_only`·`no_proxy`·요청당 15초(본문 포함)·UA `Antler`·`native-tls`. 리다이렉트는 `Redirects::None` 또는 `SameOrigin`(origin 이 정확히 같고 5홉 이하). `read_capped`(Content-Length 선검사 + chunk 누계), `or_abort`(취소), `classify`(`is_timeout`→`is_connect`→`is_body`), `NetError`(`Api`·`InvalidPath`·`Aborted`, `sealed` 가 `redact_secrets` 를 건다) |
| `jira/client.rs` | `jira/client.ts` | GET·POST·`image`. 3xx 는 `JIRA_REDIRECT`, 본문 5MB(엔드포인트별 `max_bytes`)·사진 256KB, GET 429 + Retry-After ≤ 3초면 한 번 재시도(재시도 경로의 본문도 상한), POST 는 재시도 없이 `X-Atlassian-Token: no-check`. 사진은 PR 1 의 `ImageWalk` 가 홉을 정한다 |
| `github/client.rs` | `github/client.ts` | `get`(ETag LRU 500개·32MB, 키 `sha256(토큰)[..16]\0URL\0Accept`, 비로그인 3분 캐시, core 한도 추적)·`graphql`. 본문 32MB 상한. 리다이렉트는 origin 이 정확히 `https://api.github.com` 인 5홉까지 |

**clippy**: `clippy.toml` 의 `disallowed-methods` 에 `reqwest::Client::new`·`Client::builder`·`ClientBuilder::new`·`reqwest::get`·`ClientBuilder::danger_accept_invalid_certs`·`danger_accept_invalid_hostnames` 를 넣었다
(허용 없이 돌리면 `Client::builder` 에서 경고가 나는 것을 확인했다). 소스 검사 테스트(`운영_코드에는_테스트_전용_주입_호출과_클라이언트_생성이_없다`)가 `net/http.rs` 밖의 `Client`·`ClientBuilder` 이름과
`new_for_test`·`insecure_loopback_for_test` 호출을 막는다(`SiteOptions::allow_insecure_loopback` 필드는 비공개다).
TLS 가드레일은 reqwest 0.13 의 실제 이름(`tls_danger_accept_invalid_certs`·`tls_danger_accept_invalid_hostnames`)과 `Client::default`·`ClientBuilder::default` 도 막고, `Client::builder()` 의 예외 `allow` 는
`net/http.rs` 의 `new_builder()` 한 줄에만 둔다. 소스 검사는 `danger_accept`·`tls_danger`·`.proxy(`·`.resolve(도메인, 주소)` 를 `net/http.rs` 도 예외 없이 검사한다.
자체 서명 인증서 TLS 목 서버(`tests/fixtures/selfsigned-loopback.pfx` — 테스트 전용 공개 키, `native-tls` dev-dependency) 음성 테스트가 운영 클라이언트의 핸드셰이크 실패(`JIRA_NETWORK`·`Failure::Connect`)와
서버가 `Authorization` 을 받지 못함을 확인한다(검증을 끈 대조 클라이언트로 목 서버가 정상임도 확인).

**`test-hooks` feature**: `Profile::Test`·`Rewrite`·net/github/jira 의 `*_for_test`·`SiteOptions::insecure_loopback_for_test` 는 Cargo feature `test-hooks` 뒤(`#[cfg(feature = "test-hooks")]`)에만 있어
운영 빌드에는 존재하지 않는다. `tests/net.rs` 는 `required-features = ["test-hooks"]` 라 feature 없이는 빌드되지 않고(건너뛴다), 소스 검사 `테스트_훅_정의는_모두_test_hooks_feature_뒤에_있다` 가 정의마다 cfg 속성을 강제한다.
다른 테스트 바이너리는 이 훅을 쓰지 않는다(`git/fetch` 의 `*_for_test` 는 운영 코드가 부르는 순수 함수라 대상이 아니다). CI 는 `cargo test --features test-hooks …` 와
`cargo clippy --all-targets -- -D warnings`(feature 없음 — 운영 빌드 모양) + `cargo clippy --all-targets --features test-hooks -- -D warnings` 둘을 돈다. 로컬: `cargo test --features test-hooks --lib --test net`.

**테스트**(`src-tauri/tests/net.rs` 의 `mod live`, 외부 접속 없음, `start_paused` 안 씀): 127.0.0.1 `TcpListener` 목 서버(요청 기록·지연·chunked 무한 본문·도중 끊기·헤더만 보내고 멈춤·3xx),
**프록시 덫**(`ctor` 가 테스트 시작 전에 `HTTP(S)_PROXY`·`ALL_PROXY` 를 덫 리스너로 돌리고 소켓 테스트마다 끝에서 덫 연결 0 을 확인, 덫이 살아 있는지 대조 테스트 포함), TLS 음성 둘(운영 클라이언트가 http 루프백에 연결 0,
https 로 평문 리스너에 붙으면 핸드셰이크 실패·Authorization 미유출), 리다이렉트(Jira 302·같은 호스트 301 은 대상 연결 0, GitHub 같은 origin 은 따라가고 5홉 넘으면 멈춤·다른 포트·호스트·스킴은 요청 0),
상한 초과(Jira 5MB chunked·Content-Length 선검사·`max_bytes`, 사진 256KB, GitHub 32MB), 모든 오류 봉투(서버가 `Authorization` 과 base64 를 본문 메시지로 되돌려 줘도)에 토큰·base64 형태 없음.
공개 사진 호스트(gravatar·wp.com) 테스트는 테스트 전용 주소 치환(`Rewrite`)으로 판정은 원래 주소로 하고 연결만 목 서버로 보낸다. TS 테스트 41개를 Rust 로 옮겼다(대응표 `N/A: PR 3` → `net.rs::…`).

**TS 와 다르게 동작하는 곳**:

- **TLS 신뢰 저장소**: Electron(Node/Chromium)은 번들된 CA 를 쓰지만 Rust 는 `native-tls` 라 Windows 인증서 저장소(SChannel)를 쓴다 — 사내 CA 가 저장소에 있으면 Rust 만 연결된다(반대로 번들에만 있는 CA 는 Rust 가 못 받는다).
- **GitHub 본문 상한 32MB**: TS 는 상한이 없다. 넘으면 `GH_HTTP`("GitHub 응답이 너무 큽니다").
- **GitHub 리다이렉트**: TS(`fetch` 기본)는 전부 따라가지만 Rust 는 origin 이 정확히 `https://api.github.com` 인 주소로 5번까지만 따라간다. 그 밖은 3xx 가 그대로 돌아와 `GH_HTTP`("GitHub API 오류(301)")다.
- **오류 문구**: 연결 실패 문구에 원인 문자열을 붙이지 않는다(`reqwest::Error` 의 Display 는 요청 주소를 담을 수 있다). 분류(`시간 초과`·`연결 실패`·`읽는 중 끊김`)별 고정 문구다. 사용자에게 보이는 문구는 TS 와 같다(원인 괄호 부분만 없다).
- **취소**: `AbortSignal` 대신 `CancellationToken`, `AbortError` 대신 `NetError::Aborted`. 취소하면 요청 future 를 버려 연결이 닫힌다.
- **GitHub 한도 해제 시각**: ko-KR 로컬 시각은 시간대 크레이트 없이는 만들 수 없어 `GitHubClient::with_format_time` 주입이다(기본은 UTC `HH:MM(UTC)`). 서비스 계층(PR 7)이 실제 표시 함수를 넣는다.
- **자격 증명·토큰은 호출마다 받는다**: Jira 는 `JiraCredential` 인자, GitHub 는 `TokenProvider`(+`TokenChoice`). TS 의 `credProvider` 대응은 서비스 계층(PR 5·7)이 맡는다. 비로그인 3분 캐시 시간·재시도 대기 단위는 생성자에서만 바꿀 수 있다(`new_for_test`).
- **Jira 사진 요청 오류**: 사진은 연결·시간 초과·상한 초과·잘못된 응답 모두 `None`(TS 와 같다), 취소만 오류다.

**알려진 한계**: 공개 사진 호스트의 실제 TLS 연결(gravatar·wp.com)은 시험하지 못했다(외부 접속 금지 — 주소 치환으로 홉 규칙만 확인). 서버 본문 메시지는 비밀을 **가린 뒤** 200 단위로 자른다(`github_body_message`·`jira_body_message`·`*_http_error` 가 `secrets` 를 받는다. TS 는 자른 뒤 가리는 순서였으나 Rust 는 경계에 걸린 토큰이 남지 않게 바꿨다 — 골든 비교는 `secrets` 가 비면 TS 와 같다).
`sealed` 는 마지막 방어로 남아 있다. 소스 검사는 `git/fetch` 의 기존 `*_for_test` 함수(운영 코드가 부르는 순수 함수)를 대상으로 삼지 않는다.

### PR 4 — 토큰 보관(`src-tauri/src/secret`, 앱에 연결하지 않는다)

| 파일 | 내용 |
|---|---|
| `secret/mod.rs` | `SecretStore` 트레이트(`get`·`set`·`delete` 는 바이트, `get_secret_string`·`set_secret_string` 은 `SecretString`), `Vault`(GitHub·Jira 저장·복원), 저장소 선택 `open_store`·`open_default_store`, `service_name` |
| `secret/keyring.rs` | `KeyringStore`(Windows 자격 증명 관리자, `cfg(windows)`). **키링 항목을 만드는 유일한 곳**. keyring 오류는 값이 들 수 있어(`BadEncoding` 의 바이트) 싣지 않고 `SecretError` 로만 바꾼다 |
| `secret/memory.rs` | `MemoryStore`. Windows 가 아니거나 `ANTLER_USER_DATA` 가 있으면 이것이며 `storage_unavailable` 이다 |

- **저장 형식**: 비밀과 메타데이터를 **한 덩어리 UTF-8 JSON** 으로 둔다(메타 파일 없음). GitHub `{method, token}`(계정 `github-token`), Jira `{kind, siteUrl, email, token, displayName}`(계정 `jira-token`).
  서비스 이름은 운영 `com.antler.app`, 디버그 빌드 `com.antler.app.dev` 다. 값은 `set_secret` 으로 **바이트 그대로** 두므로 UTF-16 으로 바꾸지 않는다.
- **읽기**: JSON 실패·검증 실패(GitHub: 방식 `pat|device|gh-cli`·`usable_token`, Jira: `parse_connect_input` 재검증)면 그 항목을 지우고 `None`. 저장소 접근 실패는 "없음"으로 보되 지우지 않는다.
- **쓰기**: 실패하거나 저장소가 남지 않는 종류면 메모리에만 두고 `storage_unavailable`(tokenStore.ts 와 같다). 다음 쓰기가 성공하면 풀린다.
- **크기 예산**: 한 덩어리 2560바이트(`MAX_BLOB`). Jira `displayName` 을 200자로 먼저 자르고 그래도 넘으면 `SecretError::TooLarge`(연결하지 않는다 — 기존 값은 그대로). 메모리 저장소도 같은 한도다.
- **실측**(`keyring.rs` 의 `#[ignore]` 시험, `cargo test --lib -- --ignored secret::keyring`, 고유 서비스 이름 `com.antler.test.<pid>-<시각>` 을 쓰고 끝나면 지운다): blob 2560바이트 저장 성공·2561바이트 실패(실패해도 기존 값 유지),
  `set_secret` 은 바이트를 **그대로** 저장(UTF-8 "가나" 6바이트 → 6바이트), `set_password` 는 UTF-16LE 로 바꿔 한도가 1280자다.
- **보존 방식(Windows 자격 증명 지속성)**: `persistence=Local` 로 만든다(`windows-native-keyring-store 1.1` 의 항목 수정자, 대소문자 무시 `Session|Local|Enterprise` — 크레이트 소스 `lib.rs`·`store.rs` 로 확인).
  크레이트 기본값은 Enterprise 이며 Electron 판은 로밍 경로였다. 토큰은 이 PC 에서만 쓰면 되고 도메인 로밍으로 다른 PC 에 복제되지 않는 편이 안전해 Local 을 골랐다.
  수정자는 값을 쓸 때만 적용되고 읽기에는 영향이 없다. 실제 자격 증명 관리자에서의 동작(이미 Enterprise 인 항목을 덮어쓸 때 바뀌는지 포함)은 수동 시험으로 확인하지 않았다.
- **비밀 다루기**: `SecretString` 은 `Debug` `***`·`Display`/`Serialize` 없음을 유지하고 버릴 때 0 으로 덮는다(`Drop`). 저장 JSON 버퍼와 읽은 바이트는 `Zeroizing`, 해석한 `Value` 의 문자열은 `scrub` 이 덮는다
  (serde 가 이스케이프를 풀어 만든 중간 복사까지는 보장하지 못한다 — "가능하면").
- **소스 검사**(`tests/net.rs` 의 `mod secret_store`): `keyring_core`·`windows_native_keyring_store` 는 `secret/` 밖에서 쓰지 않고(Cargo.toml 에서도 windows 대상 의존성뿐), `secret/` 에는 `println`·`eprintln`·`dbg`·`log` 가 없다
  (파일 끝의 `#[cfg(test)]` 모듈은 제외 — 수동 시험이 `eprintln!` 으로 실측값을 낸다). 테스트는 메모리 구현만 쓴다. 대응표 `N/A: PR 4` 5개 중 4개를 Rust 테스트에 연결했고, 하나(Linux `basic_text`)는 해당 분기가 없어 N/A 사유로 바꿨다.
- 의존성: `keyring-core 1`·`windows-native-keyring-store 1.1`(windows 대상), `zeroize 1`(전이 의존이던 판). `windows-sys 0.61.2` 를 기존 트리와 공유해 중복 판이 없다(`cargo tree -d`).
- **연결**: Jira 는 PR 6(`ANTLER_RUST_API`·키체인 읽기), GitHub 는 PR 9 이다. Electron 판 토큰은 옮기지 않는다(다시 로그인).

### PR 5 — Jira 서비스(`src-tauri/src/jira/service.rs`; 앱에 연결한 것은 PR 6)

`JiraService`(`Clone`, 내부는 `Arc`)가 TS `JiraService` 의 상태와 `api.ts` 의 호출부를 옮긴 것이다. `jira:*` 채널마다 메서드가 하나씩 있다:
`auth`·`connect`·`disconnect`·`issue`·`transitions`·`transition`·`comments`·`dev_info`·`search`·`worklogs`·`user_search`·`avatar`·`external_url`(`jira:openExternal` 의 주소 검증만 — 여는 일은 PR 6)·`cancel`(`req:cancel`)·`dispose`.
인자는 IPC 값(`serde_json::Value`)을 그대로 받아 `validate` 로 검증하고(검증 실패는 요청 없이 `ERROR`), 결과는 키 순서를 지키는 `Json` 이다. 상태(`JiraAuthState`)만 `serde` 로 직렬화하며 토큰 필드가 없다(테스트가 키 목록과 직렬화 문자열을 단언한다).

- **세대(epoch)**: `connect`·`disconnect` 가 `CancellationToken` 을 취소하고 새로 만든다. 모든 요청은 세대 토큰의 **자식 토큰**으로 돌므로 진행 중인 요청이 `ABORTED` 로 끝나고, 내 id·날짜 필드·사진 캐시가 비워진다.
  느린 작업(날짜 필드·사진)이 끝났을 때 세대가 바뀌었으면 결과를 캐시에 넣지 않는다. `requestId` 는 자식 토큰을 취소하며, 같은 id 의 재등록을 번호로 구분해 지운다.
- **날짜 필드**: 공유 `/field` 작업은 `tokio::spawn` 으로 돌고(대기자가 모두 떠나도 끝나서 캐시된다) `Shared` 로 동시 호출이 나눠 쓴다. 이슈 조회의 대기 상한은 `Sleeper`(운영 4초), 쿨다운(5분)은 `Clock` 이다.
  응답이 너무 크면(`JIRA_TOO_BIG`) 이 연결 동안 빈 결과를 캐시하고, 그 밖의 실패는 캐시하지 않고 쿨다운을 건다.
- **프로필 사진**: 캐시 300개(최근 사용 순)·성공 24시간·실패 10분(`Clock`), 동시 4장(`Semaphore`), 같은 주소의 동시 요청은 한 번만 받는다.
- **`jira:transition`**: `requestId` 를 받지 않아 취소되지 않는다(연결이 바뀔 때만 중단). POST 는 클라이언트가 재시도하지 않는다(429 도 한 번).
- **오류 envelope**: `JiraError{code, status, message}` — 취소 `ABORTED`, API 오류 `JIRA_*`, 그 밖(검증·저장·해석 실패)은 `ERROR`(`handlers.ts` 와 같다). 문구는 `data()` 경계에서 마지막으로 `redact_secrets` 를 거친다.
- **이전 리뷰가 넘긴 항목**: ① `disconnect` 는 키체인 삭제가 실패해도 연결은 끊고, 그 오류를 `Err` 로 돌려준다(문구: "연결은 끊었지만 안전 저장소에서 토큰을 지우지 못했습니다: …"). ② `storage_unavailable` 은 Jira 계정 것(`Vault::jira_storage_unavailable`)만 본다.
  ③ 서버 본문 메시지는 PR 3 에서 이미 가린 뒤 자른다. ④ `search` 의 잘못된 표지는 `잘못된 페이지 표지입니다`, `getMyId` 가 id 를 못 읽으면 `Jira 계정 정보를 읽을 수 없습니다`(둘 다 TS 와 같은 문구, 코드 `ERROR`).
- **`Vault::load_jira_with(SiteOptions)`** 를 더했다. 서비스가 복원할 때 연결 때와 같은 사이트 옵션으로 재검증해야 루프백 http 로 만든 시험 연결이 복원된다(`load_jira` 는 운영 옵션 그대로).
- **소스 검사**: 서비스는 `*_for_test` 를 부르지 않는다 — 시험이 만든 `JiraClient`·`SiteOptions` 를 `JiraService::with_parts` 로 받는다. 시험 전용 접근자 `date_fields_retry_at` 는 `test-hooks` 뒤에 있다.
- **시험**: `tests/net.rs` 의 `live::service`(루프백 목 서버·메모리 보관소, `start_paused` 없음). 시계를 주입하는 시험(쿨다운, 실패 사진 캐시 만료)은 소켓 없이(닫힌 포트) 또는 가짜 시계로 만든다. 대응표 `N/A: PR 5` 52개를 모두 Rust 테스트에 연결했다.

**TS 와 다른 점(PR 5)**
- `connect` 는 저장이 실패하면(너무 긴 값) 세대를 바꾸기 전에 오류를 낸다(기존 연결 유지). TS 는 세대를 먼저 바꾼다.
- 워크로그는 이슈마다 받는 즉시 `worklog_result` 로 거르고 변환한다. 20,000건 상한은 이슈 순서로 마지막에 자르며(TS 는 도착 순서로 세다가 넘으면 버린다) 건너뛴 이슈의 개수를 되돌리는 보정이 필요 없다.
- 전체 실패가 확정되면 TS 는 워커에 취소 신호를 보내고 Rust 는 진행 중인 요청을 버린다(연결이 닫힌다). 새 요청은 시작하지 않는 점은 같다.
- `RangeError`(dev-status 의 범위 밖 시각·워크로그 날짜)는 문구 `Invalid time value`·코드 `ERROR` 로 낸다.
- 날짜 필드 공유 작업은 호출자가 없어져도 계속 돈다(TS 의 Promise 와 같다). 서비스를 `dispose` 하거나 연결이 바뀌면 취소된다.

### PR 6 — Jira 연결(`src-tauri/src/api_host.rs`, 라우터·`lib.rs`·사이드카)

Tauri 판에서 `jira:*` 13개 채널을 Rust 가 처리한다(`API_CHANNELS`). 이 PR 부터 앱이 Jira 토큰을 Windows 자격 증명 관리자에 두고 읽는다(GitHub 는 PR 9 까지 사이드카·메모리).

- **스위치 `ANTLER_RUST_API`**: `0` 이면 끈다. `ANTLER_RUST_GIT=0` 이어도 함께 꺼진다(`router::rust_api_enabled`). 끄면 앱은 **Jira 서비스도 보관소(키체인)도 만들지 않고**(키체인을 읽지 않는다)
  `jira:*` 는 사이드카가 지금처럼 처리한다. 켜면 `setup` 에서 `secret::open_store`(`ANTLER_USER_DATA` 가 있으면 메모리) → `Vault` → `JiraService::new`(저장된 연결을 여기서 복원) 순으로 만들어 라우터에 주입한다.
  서비스를 만들지 못하면(HTTP 클라이언트 초기화 실패) 로그만 남기고 사이드카가 처리한다. 확인용 `ANTLER_ROUTE_LOG=1` 이면 `[route] jira:auth -> rust`(꺼져 있으면 `-> sidecar(ANTLER_RUST_API=0)`).
- **라우팅**: `API_CHANNELS` 는 `RUST_CHANNELS`·미러와 무관하다 — 스위치가 켜져 있고 서비스가 있으면 항상 Rust 다(저장소 id 가 없다). `req:cancel` 은 Rust 의 `CancelRegistry`(등록보다 먼저 온 cancel 은 tombstone)와 서비스의 요청표를 취소하고 **계속 사이드카에도 전달**한다.
- **사이드카**: 서비스가 실제로 만들어졌을 때만 `ANTLER_RUST_API=1` 로 띄운다(아니면 부모 환경의 값도 지운다 — `ANTLER_RUST_FETCH_KINDS` 와 같은 방식). 사이드카(`src/sidecar/apiOwns.ts`)는 이 값이 있으면 `jira:` 채널을 처리하지 않고 오류로 돌려준다
  (라우터가 가로채므로 닿지 않아야 하는 방어). Jira 토큰은 사이드카로 넘기지 않는다.
- **봉투**(`api_host::jira_envelope`): `handlers.ts` 의 `handle()` 과 같은 순서(`ABORTED` → `JIRA_*` → `ERROR`), void 채널(`jira:transition`·`jira:openExternal`)은 `data` 가 없다(사이드카의 JSON 직렬화와 같다). 인자 검증은 서비스(PR 5, TS `validate.ts` 와 같은 규칙·같은 code).
  requestId 는 `typeof === 'string'` 만 쓴다(빈 문자열은 없는 것). 취소는 서비스 호출 future 를 버려 요청을 끊는다. `jira:transition` 은 requestId 를 받지 않는다.
- **`jira:openExternal`**: 서비스의 `external_url`(연결한 사이트의 주소만)을 통과한 주소만 `lib.rs` 의 `open_https`(https 가 아니면 거부)로 `tauri_plugin_opener::open_url` 한다. 소스 검사가 호출 순서를 강제한다.
- **`Send` 증명(rustc)**: 서비스 future 가 tauri command(`antler_invoke`)의 `Send` 검사를 통과하려면 `jira/service.rs` 의 워크로그 스트림 클로저가 `&usize` 가 아니라 값을 받아야 했다
  (`todo.iter().copied().map(|idx| …)` — 안 그러면 "implementation of `FnOnce` is not general enough" 로 시작해 다른 메서드까지 "Send is not general enough" 로 번진다). 동작 변화는 없다.
- **시험**: `tests/channels.rs` 의 `jira_채널_시나리오가_ts_골든과_같다`(`test-hooks`) — 127.0.0.1 목 서버 + 메모리 보관소로 `tests/fixtures/net/jira.scenarios.json`(`{routes, steps}`, 연결·조회·전환·검색·워크로그·사진·열기·취소·재시작 복원·오류 401·403·404·429·네트워크)을
  Rust 라우터로 돌려 TS(`tests/goldenJiraChannels.test.ts`: 실제 `ipc/jira.ts` 핸들러 + `JiraService`)가 만든 `tests/golden/channels/jira.json` 과 비교한다(봉투는 오류면 code 만, 요청은 method·path·정렬한 query·Authorization 종류·본문).
  `tests/api_route.rs`: 스위치별 라우팅, 꺼졌을 때 서비스·키체인 없음(소스 검사), `API_CHANNELS` = `IPC_CHANNELS` 의 `jira:*`, 사이드카 env, 운영 코드에 `_for_test` 없음. 골든 갱신: `UPDATE_GOLDEN=1 npx vitest run tests/goldenJiraChannels.test.ts`.

**TS 와 다른 점(PR 6)**
- 연결하지 않은 서비스에 requestId 를 붙여 부르기 전에 같은 id 의 `req:cancel` 이 먼저 와 있으면 Rust 는 `ABORTED`, TS 는 `JIRA_AUTH` 다(Rust 는 연결 확인보다 취소 등록을 먼저 한다 — 어차피 취소된 요청이다).
- 연결·해제의 직렬화·저장 실패 시 기존 연결 유지는 PR 5 의 차이 그대로다. 오류 문구는 비교하지 않는다(code 만).

**사람 확인 필요(PR 6)** — 자동 시험은 루프백 목 서버뿐이다
- 실제 Jira Cloud(이메일 + API 토큰)와 Data Center(PAT)에 연결해 이슈·댓글·검색·워크로그·상태 전환·프로필 사진·"Jira 에서 열기"가 Electron 판과 같은지.
- 연결한 뒤 앱을 완전히 끄고 다시 켜면 연결이 복원되는지(키체인 읽기). 설정 → 연결 해제 뒤에는 복원되지 않는지.
- Windows **자격 증명 관리자**(제어판 → 자격 증명 관리자 → Windows 자격 증명)에 `jira-token` 항목 **하나**가 생기고(디버그 빌드는 `com.antler.app.dev`, 릴리스는 `com.antler.app`), 연결 해제 시 사라지는지. Electron 판의 토큰은 옮기지 않는다 — Tauri 판에서는 다시 로그인한다.
- `ANTLER_RUST_API=0` 으로 실행하면 키체인을 읽지 않고 사이드카가 이전처럼 동작하는지.

### PR 7 — GitHub 서비스(`src-tauri/src/github/service.rs` 외; 앱에는 연결하지 않는다 — 연결은 PR 9)

`GitHubService`(`Clone`, 내부는 `Arc`)가 TS `GitHubService` 의 상태와 `api.ts`·`checks.ts`·`avatars.ts`·`launchpad.ts`·`accessibleRepos.ts`·`deviceFlow.ts` 의 호출부를 옮긴 것이다. `gh:*` 채널마다 메서드가 하나씩 있다
(`gh:repo` 는 사이드카에 남는다): `auth`·`login_pat`·`import_cli_token`·`device_start`·`device_poll`·`logout`·`pulls`·`pulls_for_key`·`pull`·`pull_files`·`pull_commits`·`issues`·`issue`·`comments`·`filters`·`avatars`·`commit_checks`·`commit_check_runs`·`runs`·`run_jobs`·`launchpad`·`accessible_repos`·`external_url`(`gh:openExternal` 의 주소 검증만)·`cancel`(`req:cancel`)·`dispose`,
그리고 사이드카로 넘길 `git_token`(원격 전용 저장소 fetch 용 — PR 9 의 `{t:"ghToken"}`)과 `set_on_auth_changed`(`onAuthChanged`). 인자는 IPC 값(`serde_json::Value`)을 받아 `validate` 로 검증하고(검증 실패는 요청 없이 `ERROR`), 결과는 키 순서를 지키는 `Json` 이다.
상태(`GhAuthState`)와 Device Flow 코드(`GhDeviceCode`)만 `serde` 로 직렬화하며 토큰·`device_code` 필드가 없다(테스트가 키 목록과 직렬화 문자열을 단언한다. 숫자는 정수로 나간다).

| 모듈 | 내용 |
|---|---|
| `github/service.rs` | 상태·세대·인증(PAT·gh CLI 가져오기·Device Flow)·한도 상태·데이터 호출 공통 처리(`data`)·요청 취소 |
| `github/device_flow.rs` | Device Flow 요청(`Redirects::None`, 본문 1MB 상한, `Sleeper`·`Clock` 주입). 분기는 PR 1 의 `device_rules` |
| `github/queries.rs` | `api.ts` 호출부(PR·이슈·파일·커밋·댓글·필터·접근 가능 저장소). 보강 규칙 `soft_checks`·`reviews_page` 는 여기 있고 골든 구동기 `run_gh_case` 도 같은 함수를 쓴다 |
| `github/checks.rs` | `CheckStateCache`(5000개, 상태별 TTL)·GraphQL 일괄 요약·커밋 상세·워크플로 실행·잡 |
| `github/launchpad.rs` | Launchpad 검색 3섹션 + 저장소 remote 읽기(동시 8개·저장소마다 제한 시간) |
| `github/avatars.rs` | noreply·avatar_url 규칙(`src/shared/avatar.ts`)과 `AvatarResolver`(파일 캐시) |
| `github/ports.rs` | 포트(트레이트): `RepoQuery`(사이드카에 묻는 것), `ExternalTokenProvider`(gh CLI 자리) |
| `github/error.rs` | `GhError{code,status,message}` — 코드 순서는 `handlers.ts`(ABORTED → `GH_*` → ERROR) |

- **포트**: 저장소 → owner/repo(`gh_repo`), remote 목록(`gh_remotes`), push 된 sha(`pushed_shas_by_author`), `githubClientId`, 등록된 저장소 ID 목록(`repo_ids` — TS 는 `config.repos`), remote 캐시 무효화(`invalidate_gh_remotes`)는 사이드카가 쥔다.
  이 PR 은 트레이트와 시험용 가짜(`tests/net.rs` 의 `FakeRepos`)만 만들었고 실제 구현은 PR 9 의 `repoq` 질의다(**`repo_ids` 는 PR 9 의 op 목록(`ghRepo`·`ghRemotes`·`pushedShas`·`invalidateGhRemotes`·`githubClientId`)에 없다 — PR 9 가 op 를 더하거나 Rust 가 설정을 읽어야 한다**).
  gh CLI 로 토큰 가져오기는 PR 8: 서비스는 `ExternalTokenProvider`(`available`·`token`)만 알고 `import_cli_token`·`auth` 의 `cliAvailable` 이 그것을 쓴다.
- **세대(epoch)**: 로그인·로그아웃·토큰 폐기가 `new_epoch` 를 부르면 그 세대의 `CancellationToken` 의 자식으로 돌던 조회가 모두 `ABORTED` 로 끝난다. 조회는 시작할 때 세대 id 를 잡고, 저장소를 묻는 동안 세대가 바뀌었으면(취소됐으면) 요청을 시작하지 않는다.
  저장된 토큰의 `/user` 검증(`validate`)은 `tokio::spawn` + `Shared` 로 돌고(기다리는 호출이 모두 떠나도 끝난다), 세대 교체와 같은 잠금 안에서 잡은 세대가 취소됐는지 보고 시작하며, 끝났을 때 세대가 바뀌었으면 결과를 쓰지 않는다.
  토큰·세대 상태는 `std::sync::Mutex` 하나(`inner`)와 클라이언트가 읽는 토큰 칸(`token`, 잠금 순서 inner → token)이고 `.await` 너머로 들고 가지 않는다.
- **직렬화**: 로그인(`sign_in`)·로그아웃·토큰 폐기는 tokio `Mutex`(`change`) 하나로 묶는다(저장·세대 교체·상태 교체가 섞이지 않는다). 동시에 두 번 로그인해도 키체인과 메모리의 토큰이 같다. 로그인 검증은 내부 id 로 `requests` 에 등록돼 **로그아웃이 취소한다**(검증 중 로그아웃하면 그 로그인은 `ABORTED` 이고 되살아나지 않는다).
- **쌓지 않고 그 자리에서 변환**: 파일·커밋·댓글·접근 가능 저장소는 쪽마다 변환해 쌓고(원본 응답은 쪽이 끝나면 버려진다), Launchpad 는 검색 결과를 읽는 대로 항목으로 줄여 쌓은 뒤 remote 읽기가 끝나면 등록된 저장소의 것만 남긴다. 상한은 TS 와 같다
  (리뷰 3쪽·파일 30쪽·댓글 10쪽·커밋 250개·접근 가능 저장소 20쪽·Launchpad 섹션당 5쪽·보강 동시 6개·체크 일괄 50개·빈 쪽 추가 5쪽).
- **아바타**: 조회는 `tokio::spawn` + `Shared`(같은 작성자의 동시 요청은 한 번만, 호출자가 떠나도 세마포어 슬롯을 물고 멈추지 않는다), 동시 3개, 한도 여유 10 미만이면 멈춘다. 파일 캐시의 키는 이메일의 **sha256**(TS 는 sha1)이고 `2초` 뒤 몰아 쓴다(`flush`·`dispose` 가 바로 쓴다).
- **시험**: `tests/net.rs` 의 `live::github_service`(루프백 목 서버·메모리 보관소, `start_paused` 없음). "응답이 오기 전/후"는 시간이 아니라 `Gate`·`wait_for` 로 만들고(로그아웃과 겹치는 보강·로그인·저장소 조회·접근 가능 저장소 취소), 시계(`Clock`)·대기(`Sleeper`)는 가짜를 넣는다.
  서비스·조회 future(채널 메서드 전부)가 `Send` 인지 컴파일로 증명하는 시험이 있다(PR 9 에서 tauri command 에 올릴 때 터지지 않게). 대응표에 없는 추가 시험: 로그인 검증 중 로그아웃, 동시 로그인의 키체인·메모리 일치, 저장소를 묻는 동안 세대 교체,
  낡은 세대의 저장된 토큰 검증이 거부돼도 새 로그인 유지, 서버가 되돌려 준 토큰의 가림, `import_cli_token`(가짜 `ExternalTokenProvider` — 성공·출처 없음·형식 오류·거부 안내), Device Flow 3xx 의 대상 연결 0. 대응표 `N/A: PR 7`(95개)을 **모두** Rust 테스트에 연결했다(남은 `N/A: PR n` 은 PR 8 의 18개뿐).

**TS 와 다른 점(PR 7)**
- **한도 해제 시각 표시(미해결)**: 운영 클라이언트는 UTC `HH:MM(UTC)` 로 표시한다. TS 는 ko-KR 로컬 시각이다. 새 크레이트 없이 로컬 시각을 만들 수 없어(Windows API 를 쓰려면 `windows` 크레이트의 기능을 더해야 한다) 주입 자리만 있다 —
  `GitHubService::with_parts` 의 `make_client` 에서 `GitHubClient::with_format_time` 으로 넣는다. PR 9 가 정해야 한다.
- 로그인은 키체인 저장이 실패하면(너무 긴 값 등) 세대를 바꾸기 전에 오류를 낸다(TS 는 세대를 먼저 바꾼다). 로그아웃은 키체인 삭제가 실패해도 연결을 끊고 그 오류를 돌려준다(`async`, "로그아웃했지만 안전 저장소에서 토큰을 지우지 못했습니다: …").
  저장된 토큰이 거부돼 버려질 때의 키체인 삭제 실패는 올릴 곳이 없어 삼킨다.
- 로그인 검증이 로그아웃으로 취소된다(TS 는 취소되지 않아 로그아웃 뒤에 로그인이 되살아날 수 있다). Device Flow 시작도 세대 토큰의 자식이라 로그아웃이 취소한다.
- **로그아웃 세대**: 로그인 계열(`login`·`device_poll`·`import_cli_token`)은 첫 await(`client_id`·`cli.token()`) 전에 `logout_gen` 과 `logout_token` 의 자식 취소 토큰을 잡고, `sign_in` 이 change 잠금 안에서 세대가 바뀌었으면 `ABORTED` 다.
  같은 `requestId` 로 다른 요청이 맵 항목을 덮어써도 로그아웃이 `logout_token` 을 취소하므로 폴링이 멈춘다. 체크 캐시는 세대를 두어, 읽는 동안 `clear` 가 있었으면 그 결과를 넣지 않는다.
- **`Slug` 재검증**: 사이드카(`RepoQuery`)가 준 owner/repo 는 `Slug::parse`(TS `remoteUrl.ts` 의 `OWNER_RE`·`REPO_RE`·`.`/`..` 거부, 정규식 없이)만 거치고 필드는 비공개다. 실패하면 `NOT_GITHUB` 오류(아바타는 `github:false`)이고 요청은 나가지 않는다.
  토큰의 평문 복사본(`github_secrets`·`device_code`·Device Flow 요청·응답 본문)은 `Zeroizing` 으로 감싸 버릴 때 덮는다(reqwest 가 쥐는 본문 사본은 제외).
- Device Flow 의 3xx 는 따라가지 않고 해석할 수 없는 응답 → 일시 오류다(TS `fetch` 는 따라간다). 응답 본문은 1MB 까지만 읽는다. 오류 문구는 `device_code`·토큰으로 가려진다.
- 응답 해석 실패(TS 라면 `TypeError`)는 고정 문구("GitHub 응답 형식이 올바르지 않습니다")의 `ERROR` 다. 상태의 `user.login`·`avatarUrl` 은 문자열로 바꿔 담는다(문자열이 아닌 값이 와도 TS 는 그대로 흘리지만 이 필드는 항상 문자열이다).
- 아바타 파일 캐시 키가 sha256 이라 Electron 판의 `avatar-cache.json` 을 읽지 못한다(키 길이가 다르면 버린다). `parse_noreply`·`LOGIN_RE` 는 정규식 없이 손으로 옮겼고 TS 가 바뀌어도 알아채는 장치(`HAND_WRITTEN` 골든)는 **붙이지 않았다** —
  `src/shared/avatar.ts` 의 정규식을 노출하는 변경이 필요해 이 PR 에서는 손대지 않았다.
- `avatars` 는 취소돼도 던지지 않고 그때까지 알아낸 것을 돌려준다(TS 와 같다). 조회 작업은 시작한 호출의 취소 신호(세대의 자식)로 돌아, 취소되면 새 요청을 시작하지 않는다.
- Launchpad 의 remote 읽기 제한 시간은 `ServiceOptions::remote_timeout`(운영 3초)이다. `fresh` 인자는 TS 핸들러처럼 **`requestId` 검증보다 먼저** remote 캐시를 비운다.
- `login_pat`·`import_cli_token` 의 토큰은 `SecretString` 이고 gh CLI 의 토큰은 `Value` 로 한 번 더 복사하지 않는다(`parse_pat_str`).

**남은 위험**
- 실제 GitHub·gh CLI·키체인·`repoq` 는 쓰지 않았다(외부 접속 금지) — 연결 PR(9)에서 사람이 확인한다. 공개 사진 호스트(`avatars.githubusercontent.com`)는 주소 규칙만 시험했다(이 PR 은 이미지를 받지 않고 주소만 돌려준다).
- 아바타의 동시성 상한(3)과 목록 보강의 동시성 상한(6×3)은 목 서버가 센 최대 동시 연결 수로 확인했다. Launchpad 의 remote 읽기 동시 8개는 시험하지 않았다(상수만 TS 와 같다). 느린 CI 에서 `Gate`·`wait_for`(최대 10초)가 모자랄 수 있다.
- `GitHubClient` 의 `rate`·ETag 캐시는 토큰을 버릴 때만 비우고 로그인할 때는 비우지 않는다(TS 와 같다 — 캐시 키에 토큰 해시가 들어 있다).

## Phase 0 에서 확인한 것

- renderer 를 고치지 않고 WebView2 에서 그대로 동작한다(그래프·사이드바·커밋 상세가 그려지고, 로드 뒤 CDP 로 붙어 본 콘솔 오류는 0).
- 사이드카 번들에 `electron` 참조 0, `src/main` 무수정.
- CDP 로 확인(OS 입력 합성 없이 DOM `click()`): 창 버튼 최대화·복원·최소화, 밖에서 커밋하면 `repo:changed` 가 도착해 목록이 갱신됨,
  `location.href` 로 앱 밖 이동 시도는 같은 문서에 머무르고 `window.open` 은 null.

## Phase 0 측정

`node scripts/drive.mjs <저장소> --tauri src-tauri/target/release/antler.exe` 가 Electron 판과 **같은 시나리오**로 Tauri 판을 잰다(호스트 차이는
`scripts/lib/electronHost.mjs`·`tauriHost.mjs` 뒤에 숨겼다). 이 기록은 2026-11-13, Electron 은 `pnpm build` 산출물을 개발 모드로 실행한
것이고(패키지본이 아니다), Tauri 는 `tauri:build:cdp` 릴리스 빌드(사이드카는 Node 개발용)다. GPU 끔, 판·저장소 조합마다 3회씩
(전체 시나리오 3회 + `--idle-only 30` 3회, 번갈아 실행). 저장소: `small` = 이 워크트리(85커밋), `big` = `make-big-repo.mjs` 10만 커밋.

| 항목 | 저장소 | Electron 중앙값 (범위) | Tauri 중앙값 (범위) |
|---|---|---|---|
| 콜드 스타트 — spawn → 첫 커밋 행 감지(ms, n=6) | small | 15218 (13140–18869) | 11547 (3894–20903) |
| 〃 | big | 8215 (3776–11816) | 9227 (3883–15971) |
| 스크롤 p95(ms, n=3) | small | 13.4 (13.4–13.4) | 13.4 (13.4–13.4) |
| 〃 | big | 26.7 (26.6–26.7) | 13.5 (13.4–26.7) |
| 유휴 메모리 30초, 프로세스 트리 합 WorkingSet / Private (MB, n=3) | small | 449 / 232 | A 전체 538 / 269 · B node 제외 475 / 189 |
| 〃 | big | 415 / 209 | A 전체 507 / 246 · B node 제외 447 / 168 |

범위: 유휴 메모리는 Electron 446–457 / 229–234(small), 413–415 / 208–211(big), Tauri A 535–539 / 265–273(small), 507–511 / 246–247(big),
B 469–477 / 185–195(small), 446–448 / 167–168(big).

- **콜드 스타트는 결론을 낼 수 없다.** 같은 판·같은 저장소에서도 3.9초~21초로 흔들린다(측정 중 다른 프로세스 부하·디스크 캐시 영향으로 보이며 원인은 확인하지 않았다).
  Electron small 이 big 보다 느린 것도 순서·부하 탓이지 저장소 차이가 아니다. 정의는 두 판 모두 "프로세스 spawn → 드라이버가 첫 `.commit-subject` 를 본 시각"이고,
  Playwright 붙는 시간이 들어간다. 출력 괄호의 `app uptime` 은 Electron 은 `process.uptime`(감지 시점), Tauri 는 bridge 가 기록한 첫 행 DOM 등장 시각을 spawn 기준으로 환산한 값이라
  **두 값은 정의가 달라 서로 비교하지 않는다**(비교 표에는 앞의 감지 값을 쓴다). 안정적인 비교를 하려면 부하 없는 상태에서 더 많이 반복해야 한다.
- **스크롤 p95 는 화면 주사율(13.3ms 프레임)에 걸려 변별력이 없다.** 85커밋은 항상 한 프레임이고, 10만 커밋에서도 Tauri 는 3회 중 2회가 한 프레임, Electron 은 3회 모두 두 프레임(26.7ms)이었다.
  3회로 "Tauri 가 낫다"고 말할 수는 없고, 적어도 나빠지지 않았다는 정도만 본다.
- **유휴 메모리는 Tauri 가 더 크다.** Node 사이드카(약 60MB WS)를 빼도(B) Electron 과 비슷하거나 약간 큰 수준(small +26MB WS, big +32MB WS 이나 Private 은 오히려 작음: 189 vs 232, 168 vs 209).
  Tauri 의 WebView2 프로세스는 앱 exe 의 자손으로 잡혔다(트리 안). WorkingSet 은 공유 페이지가 중복 합산되어 실제보다 크게 나오므로 Private 쪽이 더 믿을 만하다.
  Electron 의 `app` 항목 14MB 안에는 playwright 가 돌려주는 `cmd.exe` 래퍼 1개가 `other` 로 따로 잡힌다. Tauri 의 `other`(12MB, 1개)는 정체를 확인하지 않았다.
- 같은 줄에 출력되는 기존 `memory:`(Electron `getAppMetrics`)는 트리 합과 약간 다르다(small 474/244 vs 트리 449/234 등). 판 사이 비교는 `process tree` 줄만 쓴다.

측정 조건과 한계:

- 머신: Windows 11 Pro. 측정 시작 시 여유 메모리 약 4.3GB, 실행 중 3.6~5.2GB(Win32_OperatingSystem 기준). **vmmemWSL 이 약 3GB 를 쓰며 계속 떠 있었고**,
  브라우저·여러 Claude 세션·작업 관리자 등 다른 앱도 함께 돌았다(CPU 사용률 20%대). 측정 전용 머신이 아니고, 수치를 보정하지 않았다.
- 시나리오의 WSL 연결 테스트가 전체 시나리오 3회마다 실행된다(기존 동작).
- 창이 전면으로 나오는지·다른 창이 가려지는지는 제어하지 않았다(Electron 도 같다).
- Tauri 판에서는 창 최대화 버튼의 Win+Z 가 실제 입력으로 나가므로 `tauriHost` 가 측정 중 `.tb-controls` 에 `pointer-events: none` 을 주고, `page.mouse` 좌표가 그 상자와 겹치면 throw 한다.
  (`page.click(selector)` 는 위 CSS 로만 막는다.) 프로세스는 PID 로만 종료한다.
- 위 표를 잰 뒤 Tauri 판에서만 다르게 나오던 화면(Solo 후 `0개+`, diff 줄 0, 검색 `invalid scope`)을 고쳤다. 원인은 둘이었다.
  (1) JSON 은 인자 배열의 `undefined` 를 `null` 로 바꾸는데 main 의 검증은 둘을 구분한다(`git:search` 의 scope 가 null 이면 `invalid scope`) —
  bridge 가 빈 자리를 표식으로 보내고 사이드카가 되돌린다(`UNDEFINED_ARG`, `src/sidecar/protocol.ts`).
  (2) `drive.mjs` 가 Solo·diff·검색 결과를 고정 시간만 기다리고 읽어, 응답이 조금 늦으면 "불러오는 중"·"검색 중"을 읽었다 — 끝날 때까지 기다리게 했다(Electron 측정도 더 정확해진다).
  고친 뒤 같은 저장소(small)에서 두 판의 시나리오 결과가 모두 같다(Solo 84개, diff 64줄, 검색, 스크롤 p95 13.4ms). 미니맵 툴팁 `커밋 0개` 는 Electron 판도 같아 Tauri 문제가 아니다.
  위 표의 스크롤 p95·메모리는 이 수정과 무관한 단계에서 잰 값이다.

## 남은 일·알려진 제약

- 직접 확인 필요: 스냅 레이아웃 플라이아웃 위치·지연, 툴바 드래그·더블클릭 최대화(입력 합성을 하지 않아 자동 확인 못 함).
- Electron 대비 성능은 위 "Phase 0 측정" — 콜드 스타트는 부하 없는 환경에서 다시 재야 한다.
- 새 창으로 여는 링크는 github.com 만 기본 브라우저로 보낸다(Electron 판 `setWindowOpenHandler` 와 같은 규칙).
- 미지원: `theme:system` 이벤트(renderer 가 `matchMedia` 로 시스템 테마를 직접 따라 필요 없다), `openPath`·`showItemInFolder`, 확인 창의 체크박스, Windows 밖에서의 기본 버튼 지정(위 참고).
- 직접 확인 필요: 확인 창(fetch·SSH)과 폴더·에디터 선택 대화상자가 실제로 뜨고 버튼 응답이 맞는지, 창 위치·크기·최대화가 재시작 뒤 복원되는지(자동 확인은 대화상자를 띄우지 않고 요청·응답 형식만 봤다).
- 배포 전 필수: 사이드카 경로가 빌드 머신 절대 경로로 박혀 있다(`src-tauri/src/sidecar.rs`) — Node 를 걷어 내면 사라지지만,
  그 전에 배포해야 한다면 resource 경로로 바꾼다. `cdp` 기능은 배포 빌드에서 켜지지 않게 CI 에서 막는다.
