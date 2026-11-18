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
| 4 | GitHub·Jira(reqwest), 토큰은 OS 키체인(`keyring`) | 완료(코드) — PR 1(순수 로직·골든)·2(응답 매퍼·골든)·3(HTTP 코어·클라이언트)·4(토큰 저장소)·5(Jira 서비스)·6(Jira 연결)·7(GitHub 서비스)·8(gh CLI)·9(GitHub 연결: `gh:*` 를 Rust 가 처리, `repoq`·`ghToken`, 한도 시각)까지 완료. 실제 GitHub·Jira 계정·키체인·gh 로 확인하는 것은 사람 확인이 남음(아래 "GitHub·Jira(Phase 4)" 의 각 PR "사람이 확인해야 할 것"). 계획·상태는 아래 "GitHub·Jira(Phase 4)" |
| 5 | SSH — Electron 판과 같은 구조(자체 설정 파서 + 프로토콜 라이브러리)로 Rust 에 옮긴다. 라이브러리(`russh`) 도입은 결정됨 | 완료(코드) — PR 1(설정·known_hosts·원격 명령 인용의 순수 로직·골든, `russh` 없음)·PR 2a(`russh` 스파이크 go·연결·`run`·시험)·PR 2b(`GitExecutor::Ssh`·`stream` async·펌프·16MB 선읽기 상한)·PR 3(라우팅 — `sshHost` 미러·`SshPool`·`ANTLER_RUST_SSH`)·PR 4(ssh fetch — 소유 종류를 계산 값으로·SSH 전송 오류는 network·ssh 시나리오 골든) 완료. 남은 것은 실제 sshd 로 하는 사람 확인(아래 "남은 일")이다. 계획·결정은 아래 "SSH(Phase 5)" |
| 6 | 업데이트(서명 키 필수인 `tauri-plugin-updater` + dmg·deb 수동 경로), 패키징·CI, Electron 사용자 이관 릴리스 | 6a 진행 중 — PR 1(순수 로직·골든)·PR 2(코드 탐색기 4채널)·PR 3a(공유 상태)·PR 3b(`refs`·`refsDetails`)·PR 4(`search`·`activity`)·PR 5(`repoExtras`·`lfsPaths`)·PR 7a(감시 상태 기계)·PR 8(`gh:repo`·repoq 의 git 조회) 완료(아래 "나머지 git 채널(Phase 6a)") |

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
renderer → antler_invoke → Router::route ─ Done(envelope) ──────────────→ renderer      (Rust 경로: 지금은 git:commit·git:log·git:compare·git:fileDiff·git:fileHistory·git:blame·git:fetch·git:resolveRev·git:tree·git:blob·git:onRemote — ssh 는 `ANTLER_RUST_SSH` 가 켜졌을 때)
                                         └ Sidecar → Node 사이드카 → envelope → renderer (그 밖의 모든 것)
```

- **Rust 경로 조건**(전부): 채널이 `RUST_CHANNELS` 에 있다 · 인자의 저장소 id 가 미러에 있다 · 그 kind 가 local 또는 wsl 이다(github 소스는 사이드카. **ssh 는 `ANTLER_RUST_SSH` 가 켜져 있으면 `git:fetch` 를 포함한 11개 채널 모두 Rust 가 처리**한다 — 아래 "SSH PR 3"·"SSH PR 4") ·
  환경변수 `ANTLER_RUST_GIT` 이 `0` 이 아니다. 미러에 없는 id 는 사이드카로 가서 오류 문구를 사이드카가 만든다.
- **되돌리기 스위치**: `ANTLER_RUST_GIT=0` 으로 실행하면 모든 채널이 사이드카로 간다(Electron 판과 같은 경로). 확인용으로 `ANTLER_ROUTE_LOG=1` 이면
  `RUST_CHANNELS` 채널이 어디로 갔는지 stderr 에 `[route] git:commit -> rust|sidecar` 로 남긴다(기본은 조용하다. drive 는 `ANTLER_DRIVE_DEBUG=1` 로 stderr 를 본다).
- **미러**(`src-tauri/src/mirror.rs`, 프로토콜 `{t:"repos", gen, repos:[{id, kind, path, name, distro?, sshHost?}]}`): 설정을 쓰는 쪽은 사이드카의 `ConfigStore` 뿐이다.
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
  git 은 **읽기 태스크가 첫 요청 전에**(상태 락 밖; `GitExecutor::stream` 이 async 이고 Local·WSL 은 그 안에서 `spawn_blocking`) 띄운다 — 서비스가 락을 쥔 채 동기 CreateProcess 를 부르지 않게. PID 는 태스크가 meta 에 적는다.
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
- **LFS·lenient·캐시 비우기**: 이 네 채널의 TS 코드(`commands.ts`·`fileInsight.ts`)에는 LFS 처리가 없어 그대로 옮겼다(LFS 판정은 `git:blob`·`git:lfsPaths` 쪽이다 — `git:blob` 은 Phase 6a PR 2, `git:lfsPaths` 는 PR 5 에서 옮겼다).
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

Tauri 판의 **local·WSL 저장소 fetch** 를 Rust 가 맡는다(github 는 사이드카 그대로. ssh 는 PR 4 부터 Rust 가 ssh 를 풀 때 함께 Rust 가 맡는다 — 아래 "SSH PR 4"; PR D 시점에는 ssh fetch 가 사이드카였다). 켜고 끄는 스위치는 라우터와 같은 `ANTLER_RUST_GIT`(`0` 이면 전부 사이드카)이다.

- **라우팅 채널**: `RUST_CHANNELS` 에 `git:fetch` 가 늘었다(`git:commit`·`git:log`·`git:compare`·`git:fileDiff`·`git:fileHistory`·`git:blame`·`git:fetch`). `git:fetch(repoId, manual)` 의 `manual` 은 **정확히 `true` 일 때만** 참이다
  (`handlers.ts` 의 `manual === true` 와 같다 — 문자열·숫자·배열·빈 자리는 자동 fetch). 미러에 있는 local·wsl 이면 Rust, 미러에 없거나 ssh·github 면 사이드카다(**소스 종류로 정한다** — `RepoMirror::resolve_fetch`).
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

- **경계**: Rust 가 `gh:*`(`gh:repo` 제외)·`jira:*` 전부와 토큰 영속(Windows 자격 증명 관리자), API 호출·로그인을 맡는다. 사이드카에는 `gh:repo`·`repos:addGithub`·GitHub 원격 전용 저장소 git 이 남고(**`gh:repo` 는 Phase 6a PR 8 부터 Rust 가 git 을 맡는 저장소 것만 Rust 가 답하고** 사이드카에는 github·미러에 없는 저장소 것이 남는다),
  GitHub 토큰의 메모리 사본만 Rust → Node(`{t:"ghToken"}`)로 간다(Jira 토큰은 넘기지 않는다). Rust → Node 질의 `{t:"repoq",id,op}` 로 `ghRepo`·`ghRemotes` 등을 받는다(PR 8 부터 그 저장소 정보도 Rust 가 맡는 저장소는 Rust 가 직접 읽는다).
  스위치는 `ANTLER_RUST_API`(0 이면 끔), `ANTLER_RUST_GIT=0` 이면 함께 꺼진다. 마이그레이션은 없다(다시 로그인) — Phase 6 전에 사용자 확인이 필요하다.
- **HTTP**: reqwest 0.13(`default-features=false`, `native-tls`), 클라이언트 생성은 한 곳(`net/http.rs`)에 모으고 `Client::new` 등은 clippy 로 막는다. `https_only`·`no_proxy`·15초·UA `Antler`,
  Jira·Device Flow 는 리다이렉트를 따르지 않고 GitHub API 는 origin 이 정확히 `https://api.github.com` 인 5홉까지만. 본문 상한은 Jira 5MB·사진 256KB·GitHub 32MB.
  `reqwest::Error` 의 Display 는 쓰지 않는다. 비밀은 `SecretString`(Display·Serialize 없음, Debug `***`)이고 오류 문구는 마지막에 `redact_secrets` 를 거친다.
- **PR 순서**: 1 순수 로직·골든(완료) → 2 응답 매퍼(api·checks·launchpad·accessibleRepos·adf, 완료) → 3 HTTP 코어·두 클라이언트(루프백 목 서버, 완료) → 4 SecretStore(키링·메모리, 완료) → 5 Jira 서비스(완료) →
  6 Jira 연결(채널·`openExternal`, 완료) → 7 GitHub 서비스(완료) → 8 gh CLI(완료) → 9 GitHub 연결·키체인·`ghToken`/`repoq`(완료).

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
  이 PR 은 트레이트와 시험용 가짜(`tests/net.rs` 의 `FakeRepos`)만 만들었고 실제 구현은 PR 9 의 `repoq` 질의다(`repo_ids` 는 PR 9 가 `repoIds` op 를 더해 풀었다).
  gh CLI 로 토큰 가져오기는 PR 8(`gh_cli.rs`): 서비스는 `ExternalTokenProvider`(`available`·`token`)만 알고 `import_cli_token`·`auth` 의 `cliAvailable` 이 그것을 쓴다.
- **세대(epoch)**: 로그인·로그아웃·토큰 폐기가 `new_epoch` 를 부르면 그 세대의 `CancellationToken` 의 자식으로 돌던 조회가 모두 `ABORTED` 로 끝난다. 조회는 시작할 때 세대 id 를 잡고, 저장소를 묻는 동안 세대가 바뀌었으면(취소됐으면) 요청을 시작하지 않는다.
  저장된 토큰의 `/user` 검증(`validate`)은 `tokio::spawn` + `Shared` 로 돌고(기다리는 호출이 모두 떠나도 끝난다), 세대 교체와 같은 잠금 안에서 잡은 세대가 취소됐는지 보고 시작하며, 끝났을 때 세대가 바뀌었으면 결과를 쓰지 않는다.
  토큰·세대 상태는 `std::sync::Mutex` 하나(`inner`)와 클라이언트가 읽는 토큰 칸(`token`, 잠금 순서 inner → token)이고 `.await` 너머로 들고 가지 않는다.
- **직렬화**: 로그인(`sign_in`)·로그아웃·토큰 폐기는 tokio `Mutex`(`change`) 하나로 묶는다(저장·세대 교체·상태 교체가 섞이지 않는다). 동시에 두 번 로그인해도 키체인과 메모리의 토큰이 같다. 로그인 검증은 내부 id 로 `requests` 에 등록돼 **로그아웃이 취소한다**(검증 중 로그아웃하면 그 로그인은 `ABORTED` 이고 되살아나지 않는다).
- **쌓지 않고 그 자리에서 변환**: 파일·커밋·댓글·접근 가능 저장소는 쪽마다 변환해 쌓고(원본 응답은 쪽이 끝나면 버려진다), Launchpad 는 검색 결과를 읽는 대로 항목으로 줄여 쌓은 뒤 remote 읽기가 끝나면 등록된 저장소의 것만 남긴다. 상한은 TS 와 같다
  (리뷰 3쪽·파일 30쪽·댓글 10쪽·커밋 250개·접근 가능 저장소 20쪽·Launchpad 섹션당 5쪽·보강 동시 6개·체크 일괄 50개·빈 쪽 추가 5쪽).
- **아바타**: 조회는 `tokio::spawn` + `Shared`(같은 작성자의 동시 요청은 한 번만, 호출자가 떠나도 세마포어 슬롯을 물고 멈추지 않는다), 동시 3개, 한도 여유 10 미만이면 멈춘다. 파일 캐시의 키는 이메일의 **sha256**(TS 는 sha1)이고 `2초` 뒤 몰아 쓴다(`flush`·`dispose` 가 바로 쓴다).
- **시험**: `tests/net.rs` 의 `live::github_service`(루프백 목 서버·메모리 보관소, `start_paused` 없음). "응답이 오기 전/후"는 시간이 아니라 `Gate`·`wait_for` 로 만들고(로그아웃과 겹치는 보강·로그인·저장소 조회·접근 가능 저장소 취소), 시계(`Clock`)·대기(`Sleeper`)는 가짜를 넣는다.
  서비스·조회 future(채널 메서드 전부)가 `Send` 인지 컴파일로 증명하는 시험이 있다(PR 9 에서 tauri command 에 올릴 때 터지지 않게). 대응표에 없는 추가 시험: 로그인 검증 중 로그아웃, 동시 로그인의 키체인·메모리 일치, 저장소를 묻는 동안 세대 교체,
  낡은 세대의 저장된 토큰 검증이 거부돼도 새 로그인 유지, 서버가 되돌려 준 토큰의 가림, `import_cli_token`(가짜 `ExternalTokenProvider` — 성공·출처 없음·형식 오류·거부 안내), Device Flow 3xx 의 대상 연결 0. 대응표 `N/A: PR 7`(95개)을 **모두** Rust 테스트에 연결했다(남은 `N/A: PR n` 은 PR 8 의 18개뿐이었고 PR 8 이 모두 연결했다).

**TS 와 다른 점(PR 7)**
- **한도 해제 시각 표시(PR 9 에서 해결)**: 이 PR 의 운영 클라이언트는 UTC `HH:MM(UTC)` 로 표시했다(TS 는 ko-KR 로컬 시각). 주입 자리(`GitHubClient::with_format_time`)만 있었고, PR 9 가 `github/local_time.rs` 로 로컬 시각(`오전/오후 hh:mm`)을 넣었다.
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

### PR 8 — gh CLI 토큰 가져오기(`src-tauri/src/github/gh_cli.rs`; 앱에는 연결하지 않는다 — 연결은 PR 9)

`GhCli` 가 `ExternalTokenProvider` 의 실제 구현이다(`available`·`token`). `GitHubService::new` 의 `cli` 자리에 `Some(Arc::new(GhCli::new()))` 를 넣으면 `import_cli_token`·`auth` 의 `cliAvailable` 이 실제 gh 를 쓴다(PR 9 가 넣는다).
gh 는 `gh auth token --hostname github.com`(가져오기)과 `gh --version`(`available`, 결과를 `OnceCell` 로 캐시해 동시 호출도 한 번만 실행)만 띄운다.

- **환경**: `env_clear` 뒤 PR 1 의 `gh_env` 허용 목록만 넘기고 `GH_PROMPT_DISABLED=1`·`NO_COLOR=1` 을 붙인다. 토큰·프록시 변수(`GH_TOKEN`·`GITHUB_TOKEN`·`HTTPS_PROXY` 등)는 가지 않는다.
- **실행 파일 탐색(`find_gh`)**: 이름(`gh`)만 넘기면 Windows 의 프로세스 생성(`CreateProcess` 계열)이 현재 작업 디렉터리의 `gh.exe` 를 집을 수 있는 함정이 있어, **걸러진 환경의 PATH 를 직접 풀어 절대 경로로** 띄운다.
  빈 항목·절대 경로가 아닌 항목(`.`·`bin`·`C:foo`·`\foo`)·**현재 디렉터리와 같은 절대 경로 항목**은 건너뛰고 `gh.exe` 만 찾는다(셸 없이는 `.cmd`·`.bat` 을 못 띄운다 — PATHEXT 를 따르지 않는다). 못 찾으면 프로세스를 만들지 않고 "설치되어 있지 않습니다".
- **Job Object**: 프로세스마다 `KILL_ON_JOB_CLOSE` Job 을 만들어 `CREATE_SUSPENDED` → 할당 → 재개 순서로 띄운다(git 실행기의 `job.rs` 를 `pub(crate)` 로 열어 재사용). 끝났든 제한 시간으로 버려졌든 Job 이 닫히면 손자까지 죽는다 —
  git 실행기와 달리 루트가 스스로 끝나도 남은 자손을 두지 않는다. Job 을 만들거나 넣지 못하면 **실행하지 않고** 실패한다(taskkill 폴백 없음).
- **제한**: stdout 64KB(읽는 중에 적용 — 넘는 청크를 받는 순간 중단하고 죽인다, 정확히 64KB 는 받는다), 띄우기·읽기·종료 대기 전체에 10초. stderr 는 `null` 이라 보관하지 않는다.
- **토큰 다루기**: 읽는 버퍼(`Vec<u8>`)와 읽기 청크는 `Zeroizing`, `from_utf8` → `js_trim` → `SecretString::new` 로만 옮긴다(`from_utf8_lossy` 사본 없음). tokio 파이프 내부 버퍼는 덮을 수 없다.
- **오류 문구**는 고정 문장이다(stdout·stderr·경로·토큰이 실리지 않는다): 없음 `MSG_MISSING`, 10초 `MSG_TIMEOUT`, 그 밖(비 0 종료·빈 출력·UTF-8 아님) `MSG_NOT_LOGGED_IN`, 64KB 초과 `MSG_TOO_LARGE`.
- **시험 훅**(`test-hooks`): `with_env_for_test`(환경 원본 주입 — `gh_env` 필터는 그대로 거친다)·`with_limits_for_test`(제한 시간·stdout 상한). 운영 빌드에는 없다.
- **시험(`tests/net.rs` 의 `fake_gh`·`gh_cli_run`)**: 테스트 바이너리를 `gh.exe` 로 하드링크(안 되면 복사)해 두면 그 이름으로 시작된 프로세스는 ctor 에서 가짜 gh 로 동작한다(동작은 exe 옆 `gh.mode` 파일 — `GhCli` 는 환경을 비우므로 환경 변수로는 못 넘긴다.
  가짜가 받은 환경·인자·pid 도 exe 옆 파일에 남긴다). 정상 토큰(CRLF·앞 공백 trim)·빈 출력·64KB 경계·무한 출력(읽는 중 중단 + 프로세스 사망)·시간 초과(300ms 제한)·호출 future 를 버리면(제한 시간 만료와 같은 경로 — 가짜와 손자가 뜬 것을 확인한 뒤 버린다) 루트와 손자가 모두 죽음·비 0 종료(stdout·stderr 에 토큰 모양 문자열 — 메시지에 없음)·환경 허용 목록·인자 고정·`available` 한 번 실행·공백이 든 경로,
  그리고 **현재 작업 디렉터리의 가짜 `gh.exe` 가 실행되지 않음**: cwd 는 프로세스 전역이라 시험 바이너리를 자식 프로세스로 다시 띄워(cwd = 가짜가 든 폴더, 시험 하나만) PATH 가 `.`·빈 항목·절대 경로 cwd 일 때 "없음"이 되는지 보고, 표시 파일이 없음을 확인한다. 대조군으로 PATH 의 절대 경로에 있으면 표시가 생긴다.
  서비스 시험: 실제 `GhCli` 를 물린 `import_cli_token`(gh 없음 → `cliAvailable=false`·설치 안내·저장 없음, 가짜 gh 의 토큰으로 `gh-cli` 로그인). 대응표 `N/A: PR 8`(18개)을 **모두** Rust 테스트에 연결했다 —
  gh 실행 6개, `importCliToken` 6개(PR 7 의 가짜 포트 시험 + 위 실제 구현체 시험), `AvatarResolver` 차단 사유 5개(새 시험: needs-login·공개 저장소 404·rate-limited 의 `resetAt`·session-cap(40번 조회)·로그인 시 초기화), 비로그인 단기 캐시 1개(PR 3 의 시험). 이제 `N/A: PR n` 은 없다.

**TS 와 다른 점(PR 8)**
- **PATH 를 직접 풀어 절대 경로로 띄운다**(TS 는 `execFile('gh')` 에 이름만 넘긴다). 이 환경의 Node 로 cwd 에 `zzfake.exe` 를 두고 이름만으로 `execFile` 해 봤을 때는 cwd 에서 찾지 않고 `ENOENT` 였다(`cwd` 옵션·부모 cwd 둘 다) —
  즉 TS 와 동작이 갈리는 곳은 없고 Rust 쪽의 추가 방어다(Node 버전·Windows 에 따라 달라질 수 있어 가정하지 않았다). PATH 의 상대 경로·빈 항목·**현재 디렉터리와 같은 절대 경로 항목**도 건너뛴다
  (PATH 에 절대 경로로 든 저장소 폴더의 `gh.exe` 는 현재 디렉터리가 아니면 실행된다 — 사용자가 PATH 에 넣은 것이다). `.cmd`·`.bat` 은 찾지 않는다(TS 도 셸 없이는 못 띄운다).
- **64KB 초과**는 별도 문구(`MSG_TOO_LARGE`)다. TS 는 maxBuffer 초과 오류(`ERR_CHILD_PROCESS_STDIO_MAXBUFFER`, `killed` 없음 — 확인함)가 "로그인되어 있지 않습니다" 로 떨어진다. `available` 은 TS 처럼 실패면 모두 `false` 다.
- 종료 코드가 0 이어도 stdout 이 UTF-8 이 아니면 "로그인되어 있지 않습니다"(TS 는 깨진 문자열이 형식 검증에서 걸린다).
- stderr 는 읽지 않고 버린다(TS 도 메시지에 싣지 않지만 받아 둔다). 오류 코드(`GH_CLI_MISSING` 등)는 포트가 문구만 돌려주므로 서비스에 전달되지 않는다(TS 도 서비스가 `message` 만 쓴다).
- Job 을 못 만들면 실행하지 않는다(git 실행기는 taskkill 로 물러난다). 환경은 호출마다 이 프로세스의 것을 읽는다(TS `env()` 와 같다). UTF-8 이 아닌 환경 변수는 건너뛴다.

**남은 위험(PR 8)**
- 실제 gh·실제 로그인은 쓰지 않았다(실행 금지) — 연결 PR(9)에서 사람이 확인한다: 설치된 gh 로 "gh 에서 가져오기", gh 가 없거나 로그아웃된 상태의 안내 문구, `GH_CONFIG_DIR` 를 쓰는 환경, Windows 에서 gh 를 `gh.exe` 가 아닌 shim(scoop 의 `.cmd` 만 있는 경우 등)으로만 설치한 환경은 "설치되어 있지 않습니다" 가 된다.
- 손자까지 죽는지의 시험은 제한 시간으로 끊지 않고 가짜가 뜬 것을 확인한 뒤 호출을 버린다(처음에는 2.5초·5초 제한으로 썼다가 부하 큰 러너에서 가짜가 pid 를 적기 전에 끊겨 간헐 실패해 바꿨다). pid 가 안 생기면 최대 30초 기다린 뒤 실패한다.
- 아바타 needs-login 시험의 "묻는 횟수"는 TS 가 `≤7` 이지만 Rust 는 목 서버 응답 타이밍에 따라 8 이 관측돼 `1..=9`(전부 묻는 11번보다 적음)로 완화했다. 이후 새 작성자에 더 묻지 않음은 정확히(요청 수 불변) 단언한다.
- unix 에서는 손자까지 죽이지 않는다(Windows 전용 보장 — macOS·Linux 는 Phase 6 에서 프로세스 그룹 필요).
- 가짜 gh 는 시험마다 새 경로의 하드링크라 백신의 첫 실행 검사로 느릴 수 있다. `net` 시험 전체는 ~1분이다.

### PR 9 — GitHub 연결·`ghToken`/`repoq`·한도 시각(Phase 4 마지막)

Tauri 판에서 `gh:*` 23개 채널(`gh:repo` 제외)을 Rust 가 처리한다(`API_CHANNELS`). 이 PR 부터 앱이 GitHub 토큰을 Windows 자격 증명 관리자(Jira 와 같은 `Vault`)에 두고 읽는다.
스위치는 PR 6 의 `ANTLER_RUST_API`(0 이면 끔)·`ANTLER_RUST_GIT=0` 과 같다: 꺼져 있으면 서비스도 키체인도 없고 사이드카가 지금처럼 `jira:*`·`gh:*` 를 전부 처리한다.

- **`gh:repo` 는 이 PR 에서 사이드카에 남았다**(`API_SIDECAR_CHANNELS`, 사이드카의 `SIDECAR_KEPT_CHANNELS` — 두 목록은 `tests/api_route.rs` 가 대조한다). 저장소의 remote 를 읽는 일이라 git 실행기·60초 캐시·원격 전용 저장소 레코드를 쥔 `RepoService.ghRepo` 의 몫이었고,
  Rust 서비스는 같은 정보를 `repoq` 질의로 받았다(`gh:repo` 를 Rust 로 옮기면 git 호출·캐시를 두 곳에서 하게 된다). **Phase 6a PR 8 이 Rust 가 git 을 맡는 저장소의 `gh:repo`·`repoq` 를 Rust 로 옮겼다**(아래 "PR 8"). 두 목록은 그대로이고 github kind·미러에 없는 id 는 여전히 사이드카가 처리한다.
- **한 env, 두 서비스**: 사이드카의 `ANTLER_RUST_API=1` 하나가 `jira:*`·`gh:*` 를 함께 넘기므로 `lib.rs` 는 Jira·GitHub 서비스가 **둘 다** 만들어졌을 때만 둘 다 라우터에 주입한다. 하나라도 실패하면 둘 다 주입하지 않고 사이드카가 둘 다 처리한다(소스 검사가 이 분기를 고정한다).
  Vault(보관소)는 하나이고 생성은 스위치가 켜진 분기 안 한 곳이다.
- **소유 판정은 하나**: `Router::api_active()`(스위치 && Jira·GitHub **둘 다** 주입)가 사이드카 env(`lib.rs` 의 `claim_api()`)와 채널 라우팅을 함께 정한다. 한쪽만 있으면 둘 다 사이드카가 처리한다. `claim_api()` 로 env=1 을 정한 뒤 서비스가 없는 채널은 사이드카로 넘기지 않고 `ERROR` 봉투다(방어 — 지금 경로로는 닿지 않는다).
- **`api_host::gh_envelope`**: 서비스의 `GhError{code,status,message}` 를 봉투로 옮긴다(코드 순서 ABORTED → `GH_*` → ERROR 는 서비스가 정해서 준다). 취소는 jira 처럼 바깥에서 future 를 버리지 않는다 — 로그인·아바타(취소돼도 부분 결과를 돌려준다)가 도중에 끊기면 상태가 어정쩡해지므로
  서비스가 requestId 로 직접 취소한다(`req:cancel` → `GitHubService::cancel`, 라우터가 Jira 와 함께 부른다). requestId 자리는 서비스가 정한다(`devicePoll`·`launchpad`·`accessibleRepos` 0번, `avatars` 1번).
  `gh:openExternal` 은 서비스의 `external_url`(`https://github.com` 만) 뒤에 Jira 와 **같은 열기 함수**(`open_href` → 앱이 주입한 `open_https`)를 쓴다.
- **보안 수정(PR 6 과 같다)**: `gh:*` 도 인자 모양과 무관하게 먼저 가로챈다 — 배열이 아닌 인자(`gh:loginPat` 의 토큰 포함 가능)는 서비스 검증이 거부하고 사이드카 stdin 으로 가지 않는다(`tests/api_route.rs` 의 회귀 테스트).
- **서비스 조립**: `GitHubService::new(vault, RoutedRepos(SidecarRepos — PR 8 전에는 `SidecarRepos` 직접), GhCli, ServiceOptions{avatar_cache_file: <userData>/avatar-cache.json})`. 아바타 캐시 파일 이름은 Electron 판과 같지만 키가 sha256 이라 옛 파일의 항목은 버려진다(PR 7). 앱 종료(`RunEvent::Exit`)는 `Router::shutdown` 이 서비스 `dispose` 를 불러 2초 지연 쓰기를 비운다.

**`repoq` — Rust → Node 질의(`src-tauri/src/repoq.rs`, `src/sidecar/repoQuery.ts`)**

`RepoQuery` 트레이트의 실제 구현 `SidecarRepos` 가 전송(`QueryTransport` — 앱은 `sidecar.rs` 의 `SidecarQuery`, 시험은 가짜)으로 stdio JSON-lines 를 쓴다:
`{t:"repoq",id,op,p}` → 사이드카가 `{t:"res",id,env}`(기존 `res` 재사용, 같은 `next_id`·`pending`)로 답한다. `id` 가 없으면 응답 없는 알림(`invalidateGhRemotes`)이다. 응답 전에 future 를 버리면(제한 시간·취소) `QueryGuard` 가 대기표를 지우고 `{t:"repoqCancel",id}` 를 보내 사이드카가 그 질의의 `AbortSignal` 을 끊는다.

| op | 인자(`p`) | 응답 `data` | 사이드카의 처리 |
|---|---|---|---|
| `ghRepo` | `repoId` | `{owner,repo,remote}` \| null | `RepoService.ghRepo` |
| `ghRemotes` | `repoId` | 위의 배열 | `RepoService.ghRemotes(id, signal)` |
| `pushedShas` | `repoId`·`email`·`limit` | sha 문자열 배열 | `RepoService.pushedShasByAuthor(…, signal)` |
| `invalidateGhRemotes` | 없음 | (알림) | `RepoService.invalidateGhRemotes()` |
| `githubClientId` | 없음 | 문자열 \| null | `config.get().ui.githubClientId` 를 trim(TS `clientId()` 와 같다) |
| `repoIds` | 없음 | 문자열 배열 | `config.get().repos` 의 ID — **PR 7 의 숙제**(`RepoQuery::repo_ids`)를 이 op 로 풀었다 |

**Phase 6a PR 8 이후**: `ghRepo`·`ghRemotes`·`pushedShas` 는 Rust 가 git 을 맡는 저장소(local·WSL·SSH)에 대해서는 사이드카에 오지 않는다(`RoutedRepos` 가 라우터로 직접 답한다). 이 표의 처리는 github 원격 전용 저장소·미러에 없는 id·`ANTLER_RUST_GIT=0` 만 받는다.

- **응답은 신뢰하지 않는다**: 응답 전체 1MB·`ghRemotes` 100개·`repoIds` 5000개·문자열 200자(클라이언트 ID 256자)·오류 문구 300자 상한, 모양이 틀리면 거부(`ghRepo`·`ghRemotes`)하거나 없는 것으로 줄인다(`repoIds`·`githubClientId`).
  sha 는 소문자 hex 40/64 만 받고 물은 개수까지만 쓴다. owner/repo 의 최종 검증은 서비스의 `Slug::parse`(PR 7)다. 질의마다 제한 시간 30초(멈춘 시계 시험). 알 수 없는 op 는 Rust 가 보낼 수 없고(`Op` 닫힌 목록) 사이드카도 허용 목록(`REPOQ_OPS`)을 다시 검사한다 — 목록이 둘 다 같음은 테스트가 대조한다.
  사이드카는 `ANTLER_RUST_API=1` 일 때만 `repoq` 를 받는다. 매개변수는 `p` 안에만 있어 `t`·`id`·`op` 를 덮을 수 없다. 동시 질의 64개 상한·같은 id 거부.

**`ghToken` — Rust → Node 토큰 메모리 사본(`repoq.rs` 의 `gh_token_line`, `src/sidecar/ghToken.ts`)**

사이드카에 남는 일(원격 전용 저장소의 git fetch 인증, `RepoService` 의 오류 문구 토큰 가림)이 GitHub 토큰을 필요로 한다. Rust 가 **`ANTLER_RUST_API=1` 일 때만** `{t:"ghToken",token}`(없으면 null)을 stdin 으로 보낸다:
앱이 사이드카를 띄운 **직후 한 번**(저장된 로그인 복원분)과 **토큰이 바뀔 때마다**(`GitHubService::set_on_auth_changed` — 로그인·로그아웃·토큰 폐기). "토큰 읽기 → 쓰기"는 `Sidecar` 의 전용 뮤텍스 하나 안에서 해서 로그인 직후 로그아웃이 겹쳐도 낡은 토큰이 나중에 써지지 않는다.

- **새지 않게**: 줄은 `json!`·`to_string` 을 거치지 않고 `Zeroizing<String>` 에 직접 만든다(손으로 이스케이프). 4096바이트를 넘거나 비면 `null`. env·인자·로그에 싣지 않는다(`tests/api_route.rs` 가 사이드카 env 가 이름 셋뿐임을 소스로 강제). 사이드카는 `GhTokenMirror`(private 필드, `toJSON`·`util.inspect` 가 가린다)에만 두고,
  모양(`[\x21-\x7e]+`, 4096자)이 틀린 값은 버리고 **이전 사본도 비우며**(낡은 토큰을 남기지 않는다. Rust 도 Device Flow access_token 을 PAT 와 같은 `parse_pat_str` 로 검사해 통과한 것만 로그인한다) 로그에는 고정 문구만 적는다. `rustApi` 가 아니면 이 메시지를 무시한다. 사이드카의 `GitHubService` 는 `rustApi` 일 때 `avatarCacheFile` 없이 돈다(로그인하지 않는 서비스다).
- 사이드카의 `githubToken`(`RepoService`)은 `rustApi ? 사본 : gh.gitToken()` 이다. 사본이 바뀌면 TS 의 `onAuthChanged` 와 같이 `fetcher.resumeGithubAuth()` 를 부른다(`IPC_EVENTS` 에 인증 이벤트는 없다 — 아래 "TS 와 다른 점").
- `ANTLER_RUST_API=0`(또는 서비스를 못 만들었을 때)이면 이 메시지도 `repoq` 도 오가지 않고 사이드카가 GitHub 를 지금처럼 전부 처리한다.

**한도 해제 시각(PR 7 의 미해결 해결)**: `github/local_time.rs` 가 TS `toLocaleTimeString('ko-KR', {hour:'2-digit', minute:'2-digit'})` 와 같은 `오전 09:05`·`오후 03:05`(12시간제, 0시는 오전 12·정오는 오후 12)를 만든다.
로컬 변환은 새 크레이트 없이 이미 쓰는 `windows` 0.62 크레이트의 `Win32_System_Time` 기능(`SystemTimeToTzSpecificLocalTime`, 시간대 정보 `None` = 현재 시간대)을 켜서 쓴다(`Cargo.lock` 불변 확인). 서머타임은 OS 가 그 날짜에 맞춰 적용한다.
epoch → 달력 날짜는 정수 연산(Hinnant 의 `civil_from_days`)이다. Windows 가 아니거나 연도가 `SYSTEMTIME` 범위(1601~30827) 밖이거나 변환이 실패하면 UTC `HH:MM(UTC)` 로 물러난다(macOS·Linux 는 Phase 6 에서 `localtime_r`). JS `Date` 범위를 넘는 값은 `Invalid Date`.
renderer 쪽 포맷은 쓰지 않았다: 문구는 main(여기서는 Rust)이 `GH_RATE_LIMIT` 오류 메시지에 이미 박아 보내는 것이라 renderer 가 epoch 를 다시 포맷할 경로가 없다. 서비스 생성자(`GitHubService::new`)가 `with_format_time` 으로 넣는다.
시험은 시간대마다 값이 다르므로 값 대신 성질(문구 모양, UTC 와 15분 단위로 어긋남)과 순수 함수(문구 표·날짜 변환 표)를 본다.

**채널 골든(`tests/fixtures/net/github.scenarios.json` 71단계)**: TS(`tests/goldenGithubChannels.test.ts` — `src/main/ipc/github.ts` 핸들러 + `GitHubService`, `fetchImpl` 로 api.github.com·github.com 을 같은 루프백 서버로)와
Rust(`src-tauri/tests/channels/github.rs` — `Router::route` → `api_host`, `*_for_test` 훅)가 같은 시나리오·같은 목 서버 규칙으로 돌려 `tests/golden/channels/github.json` 과 단계별 봉투(code)·**서버가 받은 요청 목록**이 같음을 본다.
시나리오: 비로그인 조회·검증 실패·Device Flow(대기→느리게→승인)·PAT·gh 가져오기(성공·실패·거부)·재시작 복원·PR/이슈/댓글/필터/체크/워크플로/아바타(404 → push 된 커밋)/Launchpad/접근 가능 저장소·한도 초과·401 재검증(폐기/유지)·폴링 취소·로그아웃.
23개 채널(`gh:repo` 제외)을 모두 다룬다. 시계에서 나오는 `fetchedAt` 만 `<time>` 으로 가린다. 목 서버·경로표·요청 정규화는 jira 골든과 `channels/mock.rs` 를 나눠 쓴다(404 기본 본문만 다르다).
Device Flow 의 간격은 TS·Rust 모두 기다리지 않는 대기를 주입한다. 취소 시나리오는 목 서버가 폴링 요청을 받은 것을 확인한 뒤(시간이 아니라 도착을 기다린다) `req:cancel` 을 보내고, 응답 지연(3초)은 취소 전에 답이 오지 않게 하는 용도뿐이다.

**시험**: `repoq.rs` 단위 13개(응답 검증·상한·시간 제한(멈춘 시계)·취소·op 목록 대조·메시지 줄·토큰 줄), `local_time.rs` 4개(비 Windows 는 UTC 폴백 1개 더), `tests/api_route.rs`(기존 jira 계열을 gh 포함으로 고치고 gh 전용 5개 추가: `gh:repo` 사이드카·계열별 주입·봉투·배열 아닌 인자·스위치 꺼짐·소스 검사),
vitest `tests/sidecarGhToken.test.ts`(4)·`tests/sidecarRepoQuery.test.ts`(9)·`tests/sidecarApiOwns.test.ts`(4)·`tests/goldenGithubChannels.test.ts`(3).

**TS 와 다른 점(PR 9)**
- 로그인 상태가 바뀌어도 renderer 로 가는 이벤트는 없다 — TS 도 `onAuthChanged` 는 main 안의 `fetcher.resumeGithubAuth()` 일 뿐이고 `IPC_EVENTS` 에 인증 이벤트가 없다. Rust 판은 그 호출을 `ghToken` 메시지로 사이드카에 옮긴다. `src/shared/ipc.ts` 는 이 PR 에서 바뀌지 않았다.
- `gh:*` 의 취소는 TS 와 같이 서비스가 requestId 로 한다(jira 처럼 레지스트리·`select!` 가 아니다). `req:cancel` 이 등록보다 먼저 오면 TS 처럼 놓친다(jira 는 tombstone 이 기억한다).
- 아바타 캐시 키가 sha256 이라 Electron 판 `avatar-cache.json` 의 항목은 버려진다(PR 7). Electron 판 토큰은 옮기지 않는다 — 다시 로그인한다(Phase 6 전에 사용자 확인).
- 한도 시각: `Win32_System_Time` 기능 추가(위). 비 Windows 는 UTC 표기.

**사람이 확인해야 할 것(PR 9 — 실제 GitHub·키체인·gh 는 쓰지 않았다)**
- 실제 GitHub 계정으로: PAT 로그인, Device Flow(설정에 OAuth Client ID)·승인 후 로그인, `gh auth login` 된 환경의 "gh 에서 가져오기", 로그아웃. PR·이슈·체크·Launchpad·접근 가능 저장소·아바타가 Electron 판과 같게 보이는지.
- 재시작 뒤 로그인 복원(`gh:auth` 가 저장된 토큰을 검증), 자격 증명 관리자에 GitHub 항목 하나가 생기고 로그아웃하면 지워지는지.
- 원격 전용 저장소(GitHub 탭으로 추가)의 fetch 가 로그인 상태로 되고, 로그아웃·재로그인 때 멈춤이 풀리는지(`ghToken` 사본).
- `ANTLER_RUST_API=0` 으로 띄우면 GitHub·Jira 가 전부 사이드카로 처리돼 지금처럼 동작(토큰은 메모리뿐)하는지. 한도 초과 오류 문구의 해제 시각이 로컬 시간(오전/오후)인지.

**남은 위험(PR 9)**
- 앱 전체(WebView2 + 사이드카) 기동에서 `repoq`·`ghToken` 이 실제 파이프로 오가는지는 자동 시험하지 않았다(`Sidecar` 는 `AppHandle` 이 필요하다) — 메시지 줄·검증·사이드카 처리기는 각각 시험했다. 위 사람 확인 항목(Launchpad·원격 전용 저장소 fetch)이 이 경로를 지난다.
- `gh_remotes` 를 취소해도 사이드카의 git 호출이 즉시 끝나는지는 `RepoService` 의 신호 처리에 달렸다(서비스는 대기를 즉시 버린다).
- `ghToken` 은 사이드카 stdin 파이프로 가는 평문이다(같은 사용자의 프로세스 사이). 사이드카 메모리의 사본은 JS 문자열이라 덮어쓸 수 없다.
- 골든의 취소 시나리오는 목 서버의 지연 응답 스레드가 3초 동안 남는다(테스트 프로세스가 끝나면 사라진다).

## SSH(Phase 5, `src-tauri/src/git/exec/ssh/`)

Electron 판의 SSH 는 시스템 `ssh.exe` 가 아니라 **JS `ssh2` + 자체 `~/.ssh/config` 파서**다(`src/main/executors/ssh/{connection,config,knownHosts,quote}.ts`). 호스트 키는 known_hosts 로만 검증하고(TOFU 없음 —
모르는 호스트는 거부하고 ssh CLI 로 수락하라고 안내), ProxyCommand·해당 Match 는 거부하며, 셸 확인(`echo 'an''tler'`)·연결당 채널 6·유휴 5분·keepalive 15초×3·접속 15초·ProxyJump 홉마다 호스트 키 검증·agent(`\\.\pipe\openssh-ssh-agent` → pageant → `SSH_AUTH_SOCK`,
IdentitiesOnly 필터)를 갖는다. Rust 판도 같은 구조로 간다 — 시스템 `ssh.exe` 는 ControlMaster 가 없고 ProxyCommand 를 거부할 수 없으며 SFTP 가 따로라 탈락했다.

**결정(사용자)**: 프로토콜 라이브러리로 `russh` 를 들인다(`default-features=false, features=["ring","rsa"]`, 새 크레이트 수는 PR 2 절). PR 1 은 이 결정과 무관했다(소켓·`russh` 가 없다).
저장소 추가 흐름(`env:*`·`repos:add` 의 SSH·SFTP 폴더 탐색, `remoteDir.ts`)은 사이드카에 남긴다(Phase 6b). 세션 정지(채널 윈도우가 읽는 쪽에 묶여 읽지 않는 채널이 연결 전체를 멈출 수 있음)는 스트림마다 펌프를 두고
16MB 선읽기를 넘으면 프로세스를 죽이고 `--skip` 으로 다시 시작하는 방식을 PR 2 에서 시험한다 — Electron 판과의 동작 차이는 받아들이기로 했다.

**PR 순서**: 1 순수 로직·골든(**완료**) → 2 `SshExecutor`(`russh`, **2a·2b 완료**; 첫 단계 go/no-go 스파이크: ring 빌드·세션 정지·agent. `GitExecutor::stream` async 전환, 루프백 `russh::server` 하네스, clippy 로 password/keyboard-interactive/TOFU 쓰기/포트 포워딩 금지) →
3 라우팅(`sshHost` 미러·`Target::Ssh`·`SshPool`·`ANTLER_RUST_SSH`, fetch 소유 판정을 kind 상수로 — 이중 fetch 방지, **완료**) → 4 ssh fetch 를 Rust 로(`rust_fetch_kinds` 계산 값·`fetchOwns.ts`·SSH 전송 오류는 network 로 분류·fetch.scenarios ssh 시나리오, **완료**) → 5 측정·사람 확인(사람 몫).
이 단계 뒤 Phase 6 은 6a(나머지 git 채널·RepoWatcher) · 6b(설정·저장소 관리·env·external·github 원격 전용) · 6c(업데이터·패키징·Electron 제거)로 나눈다. 사이드카가 응답하는 채널은 계산하면 40개다(`IPC_CHANNELS` 83개 − `RUST_CHANNELS` 7개 − `API_CHANNELS` 36개, 이 PR 에서 세어 확인했다. 이 문서가 적던 37개는 Rust 가 가로채기만 하고 응답은 사이드카가 하는 `repos:watch`·`req:cancel` 과 `gh:repo` 를 뺀 값과 같다 — 원래 기준은 확인하지 못했다). 6a 뒤에 남는 것은 아래 "나머지 git 채널(Phase 6a)".

### PR 1 — 설정·known_hosts·원격 명령 인용(앱에 연결하지 않는다)

| 파일 | 내용 |
|---|---|
| `ssh/config.rs` | `~/.ssh/config` 파서·`resolve_host`·`list_hosts`·`parse_host_spec`(Include 깊이 16·`%` 토큰·`$VAR`·`ProxyCommand`/해당 `Match` 를 `unsupported` 로 표시). 파일은 `ConfigFs`(운영 `RealFs`·시험 `MemFs`), 환경변수·로컬 사용자 이름은 `Ctx` 로 주입 |
| `ssh/known_hosts.rs` | `check_host_key`·`verify_host_key(_text)`·`known_host_key_algorithms`·지문·검증 문구. 해시 항목(HMAC-SHA1)·`@revoked` 거부·`@cert-authority` 무시. **known_hosts 를 쓰는 함수가 없다** |
| `ssh/quote.rs` | `sh_quote`·`build_remote_command`. 환경은 `merged_env(Option<&FetchEnv>)` 의 닫힌 타입만 받는다(자유 env 를 넣을 수 없다 — 자유 env 를 받는 안쪽 구현은 모듈 밖으로 나가지 않는다) |
| `ssh/win32_path.rs` | Node `path.win32` 의 `normalize`·`join`·`isAbsolute`·`dirname`·`basename`(Node 22.17.1 의 `lib/path.js`) |
| `ssh/mod.rs` | JS 정규식 `globToRegExp` 의 의미를 옮긴 `glob_match`(UTF-16 단위·줄 끝 문자 제외·ASCII 대소문자 무시) |

**골든**: `tests/fixtures/sshCases.ts`(입력 표 — 설정 사례 31개(질의 111)·호스트 스펙 29개·known_hosts 사례 14개(질의 49)·인용 28개·원격 명령 24개) → `tests/goldenSsh.test.ts` 가 TS 구현으로 `tests/golden/ssh/{config,knownHosts,quote}.json` 을 만들고
(`UPDATE_GOLDEN=1 npx vitest run tests/goldenSsh.test.ts`), 원격 명령 원문 바이트는 `tests/goldenExec.test.ts` 가 `tests/golden/exec/env.json` 의 `ssh` 절로 만든다(`GIT_ENV` 단독·fetch 환경 둘 × 작은따옴표·백슬래시·`$`·백틱·개행·한글·`-c alias…`·NUL).
`src-tauri/tests/ssh_golden.rs` 가 전부 읽어 비교한다(처리기 없는 골든이 있으면 실패). 입력 `TRICKY` 는 기존 `tests/sshQuote.test.ts` 와 공유한다.
- **플랫폼과 무관한 골든**: 골든 생성기는 `node:path` 를 win32 판으로, `node:fs` 를 가상 파일 시스템(`C:\golden\…`, 대소문자 무시)으로 바꿔 돌린다 — 임시 경로·플랫폼(CI 의 vitest 는 ubuntu)에 기대지 않고 Rust 의 `MemFs`·`win32_path` 와 같은 규칙으로 맞춘다. 홈(`C:\golden\home`)·로컬 사용자·환경변수는 사례 입력이다.
- **대응표**(`tests/golden/ssh/test-map.json`): 대상 테스트 5개(`sshConfig`·`sshKnownHosts`·`sshQuote`·`sshExecutor`·`sshRemoteDir`)의 모든 `it` 59개가 있어야 하고(vitest 가 강제) 값은 `<파일>.rs::<테스트 이름>`(그 이름의 `fn` 이 있어야 한다)·`N/A: PR n`·`N/A: 사유` 중 하나다.
  `sshExecutor` 는 PR 2(a·b)에서 전부 `ssh.rs::…` 로 옮겼고(`N/A: PR n` 0개), `sshRemoteDir` 는 `N/A: 저장소 추가 흐름(사이드카 유지)`. 골든 사례 이름은 해당 `it` 제목과 같고, 한 골든 시험이 여러 `it` 을 덮는다.
- **시험**: 라이브러리 단위 21개(`win32_path`·`glob`·줄 모양·토큰·`$VAR`·Include 깊이·실제 파일 시스템 Include·base64·키 종류·인용 모양·env 이름 거부), `ssh_golden.rs` 11개, vitest `goldenSsh`(2)·`goldenExec`(`ssh` 절 포함).

**TS 와 다른 점**
- **대소문자 무시는 ASCII 만이다 — 이름 쪽을 ASCII 로 막아 닫았다.** TS 의 `globToRegExp` 는 `i` 플래그(유니코드가 아닌 정규식)라 `toUpperCase` 로 정규화해 `é` 와 `É` 를 같게 본다.
  Rust 는 Node 의 대문자 표가 없어 ASCII 만 접는다(`ſ`·켈빈 기호가 ASCII 와 다른 것은 TS 도 같다). JS 비유니코드 `i` 의 Canonicalize 는 비 ASCII 를 ASCII 로 접지 않으므로 **비교되는 이름이 ASCII 면 어떤 패턴이든 Rust 결과가 TS 와 정확히 같다.**
  그래서 관문을 이름 쪽에 둔다(`to_lowercase` 로 바꾸지 않는다 — 켈빈 기호 등으로 새 차이가 생긴다): `resolve_host` 는 별칭·HostName·HostKeyAlias 에 비 ASCII 가 있으면 `unsupported`("비 ASCII 호스트 이름(punycode 로 적으세요)"),
  `check_host_key` 는 비 ASCII 호스트를 판정하지 않고 `HostKeyCheck::Unsupported`(검증 거부, `known_host_key_algorithms` 는 빈 목록)를 낸다. 골든(`rust` 필드·`rustStatus`)이 이 차이를 고정하고 서로 **달라야** 통과한다.
  비 ASCII 대문자 `@revoked`·`Host`/`Match host` 패턴이 호스트 이름과 맞지 않아 거부·ProxyCommand 판정이 새던 경로는 이 관문으로 닫혔다.
- **Rust 가 TS 보다 엄격한 곳**(모두 `unsupported`): ① 최종 포트와 모든 ProxyJump 홉 포트가 1..=65535 가 아니면("포트가 1~65535 범위 밖"; TS 는 `Number()` 라 어떤 값이든 받고 `ssh2` 가 실패한다. 설정 파일의 `Port` 는 TS 처럼 범위 밖이면 무시한다).
  ② 이 호스트에 적용되는 `RevokedHostKeys`("RevokedHostKeys" — 관리자 폐기 목록을 읽지 않는데 조용히 무시하면 폐기된 키를 수락한다. TS 는 무시한다).
- **지원하지 않는 한계(TS 와 같다, 조용히 무시한다)**: `GlobalKnownHostsFile`(시스템 known_hosts 를 읽지 않는다)·`KnownHostsCommand`·`CanonicalizeHostname`. 이 설정에 기대는 환경에서는 호스트를 "알 수 없음"으로 거부하거나 다른 이름으로 접속할 수 있으니 사용자 안내에 적는다.
- `~/.ssh/config` 의 경로는 **항상 win32 규칙**으로 정규화한다(`win32_path`). macOS·Linux 지원(Phase 6)에서 posix 규칙이 필요하면 호출부에서 고른다. Node 판이 올라 `path.win32` 가 바뀌면 골든이 실패하니 `lib/path.js` 와 다시 대조한다(예약 이름 `CON` 등의 처리는 `isWindowsReservedName(path, -1)` 의 끝 글자 하나 잘림까지 옮겼다).
- 포트는 `u32` 로 포화시킨다(골든 모양 유지). 범위 검사는 위 "엄격한 곳" ① 이 `resolve_host` 한 곳에서 한다.
- 로컬 사용자 이름은 Windows 에서 `GetUserNameW`(`windows` 크레이트의 `Win32_System_WindowsProgramming` 기능만 켰다 — 새 크레이트 아님), 실패하면 `USERNAME`·`USER`·`user`. 환경변수는 `process_env`(Windows 는 대소문자 무시, Node 와 같다).
- 정규식 크레이트를 쓰지 않았다. 줄 형식 `^([A-Za-z0-9]+)(?:\s*=\s*|\s+)(.*)$` 는 손으로 풀었다(JS `\s`·`trim` 은 U+FEFF 를 공백으로 보고 U+0085 는 아니다, `.` 는 줄 끝 문자를 맞추지 않아 값 안에 있으면 그 줄을 버린다). `.sort()` 는 UTF-16 단위 순서다.
- 해시된 known_hosts 의 salt·hash 는 Node `Buffer.from(x, 'base64')` 처럼 느슨하게 푼다(URL-safe 문자·공백·누락된 `=`). 키 종류는 Node `toString('ascii')` 처럼 바이트의 최상위 비트를 지운다.
- `build_remote_command` 는 환경으로 `Option<&FetchEnv>` 만 받는다(TS 는 `Record<string,string>`). 키 이름 검사는 닫힌 타입이라 일어나지 않지만 안쪽 구현에 남겨 시험한다.

**추가한 크레이트**: `hmac` 0.12.1·`sha1` 0.10.7(해시된 known_hosts 의 HMAC-SHA1). 이미 lock 에 있는 RustCrypto 0.10 계열(`digest` 0.10.7·`sha2` 0.10.9)과 같은 판이라 중복 판이 없다(`cargo tree -d` 로 확인).
전이로 `subtle` 2.6.1 이 하나 더 들어왔다(`digest` 의 `mac` 기능). 그 밖의 새 크레이트는 없다.

**PR 2 에서 반드시 옮길 방어**(TS `connection.ts` 가 하던 것 — 빠뜨리면 PR 1 의 순수 로직이 막은 것이 연결에서 새 나간다)
- **원격 셸 확인 `SHELL_PROBE`**(`connection.ts:70-76,490-510`): 접속 직후 `echo 'an''tler'` 로 로그인 셸이 POSIX 인용을 해석하는지 확인한다. cmd.exe 로그인 셸에서는 `sh_quote` 의 인용이 무효라 주입 방어선이 사라진다.
- **홉마다 `unsupported` 거부**(`connection.ts:285`): 대상뿐 아니라 ProxyJump 의 모든 홉을 `resolve_host` 로 다시 해석해 `unsupported` 가 있으면 거부한다.
- **HostKeyAlias 가 있으면 포트 22 고정 조회**(`connection.ts:412-413`): known_hosts 조회 이름을 `[alias]:port` 가 아니라 alias·22 로 한다(TS 와 같다).
- **비 ASCII 관문·포트 관문이 연결 경로에서도 걸리는지 시험**: 별칭·HostName·HostKeyAlias 비 ASCII, 대상·홉 포트 0·65536, `RevokedHostKeys` 가 `unsupported` 로 연결 전에 거부되는지, `check_host_key` 의 `Unsupported` 가 호스트 키 콜백을 거부로 이끄는지.

**남은 위험(PR 1)**
- 연결 쪽이 `ResolvedHost::unsupported`(비 ASCII·포트 범위·RevokedHostKeys 포함)를 거부해야 한다(TS 는 `connection.ts` 가 점프 호스트까지 모든 노드에서 한다). PR 1 에는 연결이 없어 이 거부는 시험되지 않는다 — PR 2 의 첫 시험으로 넣는다.
- `MemFs` 가 feature 없이 운영 lib 에 들어 있다(통합 시험이 별도 크레이트라 `cfg(test)` 로 숨길 수 없다). 메모리 파일만 다루고 운영 경로에서 쓰이지 않는다.
- 골든 생성기의 `node:path`·`node:fs` 모킹이 ubuntu CI(vitest)에서도 같은 골든을 내는지는 로컬(Windows)에서만 돌렸다.
- `RealFs` 의 Include(실제 파일)는 Windows 임시 디렉터리로 한 시험뿐이다(심볼릭 링크·권한 오류·비 UTF-8 이름은 시험하지 않았다).

### PR 2 — SSH 실행기(`russh`; 앱에 연결하지 않는다) — 2a: 스파이크 + 연결 + `run`

**진행**: 2a(스파이크·연결·`run`·시험·clippy)와 2b(`GitExecutor::Ssh`·`stream`·펌프·`log_service`/`file_channels` 갱신·stream `it` 5개 이식)까지 완료. 앱 연결(라우팅)은 PR 3.
2a 에서는 `SshExecutor` 가 `GitExecutor` 와 별개 타입이었다(`run` 만) — 동기 `stream` 에 가짜 변형을 두지 않으려고 2b 에서 `stream` 을 async 로 바꾸며 enum 에 함께 넣었다.

#### 1단계 go/no-go 스파이크 — **go**
- **(a) 빌드**: `russh` **0.65.2**(2026-11-16, crates.io 최신 안정판, MSRV 1.89), `default-features = false, features = ["ring", "rsa"]`(`aws-lc-rs`·`flate2`(압축)는 끈다). Windows(MSVC, `-j 2`)에서 **NASM 없이** 빌드된다(`ring` 0.17.14).
  `cargo build -j 2 -p russh` 57초(따뜻한 target — 110개 컴파일 단위), 전체 라이브러리 `cargo check` 3분 22초, 시험 포함 첫 빌드 4분 12초. 경고 없음.
  **크레이트 수**: Windows 빌드 그래프에서 **새 `이름@판` 89개**(`cargo tree -e normal,build` 를 이전 `Cargo.lock` 과 대조), `Cargo.lock` 은 526 → 632(+106; `nix`·`windows-sys 0.52`·`windows_*` 등 다른 타깃용 항목 포함). `russh` 하위 트리 전체는 약 200개(기존 포함).
  사용자가 허용한 `russh` 와 그 의존성 외에 **새로 넣은 크레이트는 없다** — `rand = "0.10"`(dev, 시험 서버 키 생성)은 russh 가 이미 끌어오는 판이다. 기존 `tokio` 에 `net` 기능만 켰다(russh 가 이미 켠다).
  **중복 판**: RustCrypto 가 0.10 계열(PR 1 의 `sha2`·`sha1`·`hmac`·`digest`)과 0.11 계열(russh) 둘 다 들어왔다(`digest`·`sha1`·`sha2`·`hmac`·`crypto-common`·`block-buffer`·`cpufeatures`·`getrandom`·`rand_core`). PR 1 절의 "중복 판이 없다" 는 더 이상 사실이 아니다. 동작 문제는 없고 빌드 시간·바이너리 크기만 는다.
  **CVE**: CVE-2025-54804(채널 윈도우 조정 오버플로)는 GitHub Advisory DB 에서 `< 0.54.1` 이 취약, 0.54.1 에서 고쳐졌다(판 번호 순서가 아니라 권고 DB 로 확인). 그 뒤 나온 권고들(키 교환 중 호스트 키 검증 전 콜백 CVE-2026-128087 등)의 **가장 높은 패치 판이 0.65.1** 이라 0.65.2 는 모두 지났다. `cargo audit` 은 돌리지 않았다.
- **(b) 세션 정지(R1) — 재현됐고 펌프로 풀린다.** 소스(`client/encrypted.rs`): 데이터 패킷을 받으면 윈도우 조정은 **받은 바이트로** 하고(`adjust_window_size`), 채널 큐(`channel_buffer_size` = **메시지 100개**)에 `chan.send(..).await` 로 넣는다 — 소비자가 읽지 않아 큐가 차면 **세션 루프 전체**가 거기서 멈춘다.
  실측(`tests/ssh.rs::세션_정지_스파이크_…`, 날 russh 클라이언트와 프로세스 안 서버): 채널 A 가 1KB 패킷으로 4MB 를 쏟는데 클라이언트가 읽지 않으면 서버는 약 309KB 에서 멈추고, **그 뒤에 연 채널 B(10바이트)는 4초 안에 끝나지 않는다.** 같은 상황에서 A 를 태스크가 즉시 비우면 B 가 끝나고 A 는 4MB 를 다 받는다.
  (A 를 연 직후 곧바로 B 를 열면 B 가 먼저 끝나 버려 정지가 보이지 않는다 — 시험은 서버의 전송이 연속 5번 같고 전체보다 작은 것을 확인한 **뒤에** B 를 연다.) 패킷이 작을수록 정지가 빨리 온다(윈도우 2MB 보다 큐 100개가 먼저 찬다). 따라서 **읽는 쪽이 항상 비워야 한다** —
  `run` 은 읽기 루프 자체가 펌프라 정지가 없다(`세션_정지_없음_…` 가 4MB·512바이트 패킷 8192개 옆에서 짧은 호출 5개가 끝나는지 본다). `stream`(2b)은 스트림마다 펌프 태스크를 두고 상한(16MB)을 넘으면 KILL 한다.
  (이 시험의 "읽지 않으면 멈춘다" 단언이 깨지면 russh 가 고친 것이다 — 펌프 설계를 다시 본다.)
- **(c) agent**: 코드 경로가 있다 — Windows OpenSSH 파이프 `AgentClient::connect_named_pipe(\\.\pipe\openssh-ssh-agent)`·pageant `AgentClient::connect_pageant()`(`pageant` 0.2.5, russh 의 필수 의존성). 유닉스 `connect_uds`·`connect_env`(`SSH_AUTH_SOCK`)는 **`cfg(unix)` 전용**이라 Windows 에서는 파이프 → pageant 다(TS 와 같다).
  `connect_named_pipe` 는 `PIPE_BUSY` 에서 **무한 재시도**하므로 3초 제한 안에서 부른다. 서명은 `authenticate_publickey_with(.., &mut AgentClient)`(`Signer` 구현), `AgentClient::dynamic()` 로 종류를 하나로 합친다. **실제 agent 에는 접속하지 않았다**(API·소스 확인만) — 시험은 항상 `AgentSetting::Disabled`.

#### 구현(`src-tauri/src/git/exec/ssh/`)
| 파일 | 내용 |
|---|---|
| `connection.rs` | 연결 수명(지연 연결·단일 비행·끊김 감지 후 재연결·유휴 5분 종료·keepalive 15초×3·접속 15초), ProxyJump(direct-tcpip, 체인 전체 보관), 채널 세마포어 6, 원격 셸 확인, 홉마다 `unsupported` 거부·known_hosts 판정, exec 태스크 |
| `auth.rs` | agent(파이프 → pageant, IdentitiesOnly 필터) → 키 파일(암호화된 키는 사유만), 비밀번호·KI 없음 |
| `executor.rs` | `SshExecutor::run` — 원격 명령은 `build_remote_command`(PR 1) 결과만 보낸다 |
| `tests/ssh.rs`·`tests/ssh/{server,common,spike}.rs` | 프로세스 안 `russh::server`(127.0.0.1, 전용 런타임) 하네스와 시험 47개(1개 `#[ignore]`), `connection.rs` 단위 시험 3개(호스트 키 콜백) |

- **채널은 태스크가 소유한다.** Rust 는 future 를 버리는 것이 취소이고 `Drop` 은 동기라 원격에 KILL/close 를 보낼 수 없다. exec 채널을 연 뒤에는 스폰된 태스크가 채널과 슬롯(세마포어 허가)을 쥐고, 호출자가 취소하거나 future 를 버리면(`drop_guard`) 태스크가 **KILL → close → 서버가 닫을 때까지 비움(3초 상한)** 을 하고 그 뒤에 슬롯을 돌려준다.
  호출자는 기다리지 않고 바로 `Aborted` 로 돌아온다. `Eof` 와 종료 코드를 둘 다 받거나 `Close`/`None` 이 올 때까지 읽는다(종료 코드 없이 끝나면 "SSH 연결이 끊겨…", 신호 종료는 -1).
- **원격 셸 확인**: 접속 직후 `echo 'an''tler'` 를 보내 `antler` 가 아니면 거부한다. 서버가 받은 첫 명령이 항상 이것이고, 거부 시 그 밖의 명령이 없음을 시험이 확인한다.
- **clippy `disallowed-methods`**(`clippy.toml`): `russh::client::connect`·`connect_stream`·`Handle::channel_open_direct_tcpip`(connection.rs 의 문장 하나씩만 `allow`), `authenticate_password`·`authenticate_keyboard_interactive_{start,respond}`·`authenticate_gssapi_with_mic`,
  `russh::keys::known_hosts::{learn_known_hosts[_path], check_known_hosts[_path]}`(TOFU 쓰기·홈의 파일 읽기), `tcpip_forward`·`streamlocal_forward`(+cancel)·`channel_open_direct_streamlocal`·`channel_open_x11`·`Channel::{agent_forward, request_x11}`(+`ChannelWriteHalf`).
  임시 파일로 8개가 실제로 걸리는 것을 확인하고 지웠다. clippy 를 `allow` 로 우회하는 경우를 위해 `tests/ssh.rs::russh_의_연결_인증_포워딩_api_는_허용된_파일에서만_쓴다` 가 소스에서 같은 이름을 센다.
- **시험**(`cargo test --test ssh`, 약 25초): TS `sshExecutor.test.ts` 24개 `it` 중 19개를 이식(나머지 5개는 stream — 2b 에서 이식했다)하고 추가했다 — 세션 정지 스파이크, `run` 큰 출력 옆 작은 호출, 취소 토큰·future 버림 뒤 원격 자식 0·슬롯 반환, 슬롯 수보다 많은 반복, 출력 상한 뒤 정리, 종료 코드 없는 끊김,
  dispose 가 대기 호출을 깨움, 유휴 종료 뒤 재연결, 접속 제한 시간(배너 없는 서버), 2홉 ProxyJump(+둘째 홉 키 오류), 점프 홉의 `unsupported`, 비 ASCII 별칭·HostName·HostKeyAlias·포트 0/65536/홉 포트 70000·`ProxyCommand`·`RevokedHostKeys` 의 연결 전 거부, `@revoked`, `HostKeyAlias`(포트 22)·`UserKnownHostsFile`, 알고리즘을 등록 종류로 제한, 암호화된 키 사유, 원격 명령 바이트·환경 == `GIT_ENV`. **서버 쪽에서 연결이 실제로 닫히는지**(하네스의 열린 연결 수)를 유휴 종료·dispose·인증 실패·호스트 키 거부·셸 거부·점프 체인 실패 뒤에 확인한다 — 버려진 `Handle` 이 세션을 끝낸다는 것이 이것으로 확인됐다.
  `HostKeyHandler::check_server_key` 는 단위 시험이 직접 부른다(비 ASCII 이름의 `Unsupported` → 거부, `@revoked`, 모르는 호스트, 다른 키).
  시간 의존은 `wait_for`(조건 + 상한)이고 주입 가능한 `Timeouts`(접속·keepalive·유휴·셸 확인)를 쓴다 — 실제 소켓에 `start_paused` 를 쓰지 않았다. 무거운 시험은 `heavy()` 로 직렬화. 서버는 전용 런타임이라 시험 런타임이 끝나도 죽지 않는다.
  실제 sshd 시험은 `#[ignore]` + `ANTLER_SSH_TEST_HOST`·`ANTLER_SSH_TEST_REPO`(사용자의 `~/.ssh`·agent 를 **읽는** 유일한 시험).

**TS 와 다른 점**
- **취소가 즉시 돌아온다**: TS 는 `killChannel` 뒤 `close` 를 기다리지 않고 슬롯을 돌려줬다. Rust 는 호출자가 즉시 `Aborted` 를 받지만 슬롯은 **서버가 채널을 닫은 뒤**(최대 3초)에 돌아온다(서버 `MaxSessions` 를 넘지 않게).
- **연결 시도는 호출자별이다**: 처음 호출자가 취소하면 그 연결 시도도 버려진다(TS 는 진행 중인 연결 promise 를 다른 대기자와 공유해 계속 진행).
- **종료 확정**: `Eof` 와 `exit-status` 를 둘 다 받으면 `Close` 를 기다리지 않고 끝낸다(TS 는 `close`). 서버가 `Close` 를 늦게 보내도 멈추지 않게 하려는 것이다.
- **키 교환·암호**: russh 기본 목록(압축 없음 — `flate2` 를 껐다, `dsa` 없음). `ssh2` 가 받던 오래된 알고리즘만 되는 서버는 "SSH 연결에 실패했습니다" 로 보일 수 있다. 호스트 인증서(`*-cert-v01`)는 광고하지 않고 서버가 내밀면 거부한다.
- **오류 문구**: 연결 거부·이름 못 찾음·시간 초과·호스트 키 거부·인증 실패 문구는 TS 와 같고, 그 밖의 핸드셰이크 실패는 russh 의 오류 문장이 들어간다.
- **keepalive**: `keepalive_max = 3` 은 응답 없는 keepalive 가 3번을 **넘으면** 끊는다(ssh2 `keepaliveCountMax` 와 ±1 차이 가능). 시험하지 않았다(주입 가능한 간격만 있다).
- **IdentitiesOnly 필터**: TS 는 agent 를 감싸 키 목록을 걸렀고 Rust 는 agent 에서 목록을 받아 같은 규칙으로 걸러 시도한다. 공개키는 `<IdentityFile>.pub` → `IdentityFile` 자체가 `.pub` 인 경우 그 파일 → 개인키 파일의 공개 부분(암호화된 OpenSSH 키도 공개 부분은 평문) 순으로 읽는다.
- **서버가 여는 채널은 전부 거부한다**(session·direct-tcpip·direct-streamlocal·forwarded-tcpip·forwarded-streamlocal·auth-agent·x11 → `AdministrativelyProhibited`). russh 의 기본 핸들러는 accept 해서 채널 맵에 쌓이므로 7개 콜백을 모두 구현했다(ssh2 는 전부 거부). 루프백 서버가 인증 뒤 7종을 열어 거부 응답을 받는 시험이 있다. 클라이언트 쪽 채널 맵 크기는 공개 API 가 없어 직접 보지 못하고, 서버가 받은 거부 응답으로 갈음한다.
- **연결 실패 공유**: 연결 잠금을 얻은 직후 최근 5초 안의 실패가 있으면 다시 맺지 않고 그 오류로 돌려준다(인증을 거부하는 서버에 대기 호출 수만큼 재접속·재인증하지 않는다). 설정을 고친 뒤 5초 안에 다시 호출하면 이전 실패 문구가 한 번 더 나올 수 있다.
- **점프 호스트의 ProxyJump(중첩)는 거부한다**(`<별칭>: 점프 호스트의 ProxyJump(중첩)는 지원하지 않습니다`). TS 는 조용히 무시했지만 그러면 의도하지 않은 경로로 접속하므로 더 엄격하게 막았다. 대상의 `ProxyJump` 에 홉을 나열해야 한다.
- **시작 실패 정리**: 채널 열기와 exec 요청을 두 단계로 나눠, exec 단계가 실패·시간 초과하면 KILL → close → 닫힘 확인 뒤에 슬롯을 돌려준다. exec 거부(`Failure`)도 닫힘을 확인한 뒤 슬롯을 돌려준다(하네스의 거부 모드로 시험). exec 요청 전송 자체의 실패·시간 초과는 로컬 큐가 막혀야 일어나 루프백 시험으로 재현하지 못했다.
- **재키 시 호스트 키 재검사 없음**: 호스트 키는 접속 때 한 번만 known_hosts 로 판정한다. 재키(rekey)에서 서버가 다른 호스트 키를 내밀어도 다시 검사하지 않는다 — russh 가 재키마다 `check_server_key` 를 부르는지·부르면 같은 판정이 되는지 확인하지 않았고, OpenSSH 와 다른 점이다.
- **협상 목록**: `kex`·`cipher`·`mac` 에 `none` 이 없음을 단위 시험이 고정한다(russh 를 올릴 때 깨지면 목록을 명시로 고정). clippy 금지 목록에 `Handle`·`client::Session` 의 포워딩·전역 요청과 `Channel`·`ChannelWriteHalf` 의 `exec`(프로브·`open_exec`(`run`·`stream` 공용)만 allow)·`request_shell`·`set_env` 를 더했다.
- **Windows 에서 `SSH_AUTH_SOCK`**: TS 처럼 보지 않는다(`IdentityAgent` 에 파이프 경로를 줄 때만). 유닉스 소켓 경로는 Windows 에서 "지원하지 않는 agent 종류" 로 안내한다. `cfg(unix)` 분기는 이 환경에서 컴파일하지 못했다.

**사람이 확인해야 할 것**(2a)
- 실제 sshd(OpenSSH, 기본 셸이 cmd 인 Windows OpenSSH 서버 거부, bastion ProxyJump)에 접속해 `rev-parse`·큰 `log` 가 되는지(`ANTLER_SSH_TEST_HOST=<별칭> ANTLER_SSH_TEST_REPO=<경로> cargo test --test ssh -- --ignored`).
- Windows OpenSSH agent 파이프·pageant 에 실제로 붙어 서명·IdentitiesOnly 필터가 되는지(이 환경에서는 연결해 보지 않았다), 암호화된 키 안내 문구, RSA 키(`rsa-sha2-*` 협상) 접속.
- 해시된 known_hosts(`|1|…`)·`@cert-authority` 가 든 실제 파일, `UserKnownHostsFile` 여러 개.

**남은 위험(PR 2)**
- `ssh-key =0.7.0-rc.12` 가 russh 에 의해 **프리릴리스로 고정**돼 있다 — russh 를 올릴 때 같이 움직이며 따로 올릴 수 없다.
- `rsa` 0.10(RSA 키 서명)에 타이밍 부채널 권고(RUSTSEC-2023-0071 계열)가 해당하는지 확인하지 않았다. 클라이언트 서명에만 쓰이고 원격 공격자가 타이밍을 잴 수 있는 경로는 좁지만 `cargo audit` 으로 확인한다.
- 2a 의 `run` 은 읽기 루프가 펌프라 정지가 없고, 2b 의 `stream` 도 펌프라 소비자가 느려도 정지하지 않는다(아래 2b 시험이 읽지 않는 스트림 옆의 `run` 으로 확인).
- 실제 원격(지연·패킷 손실)에서의 keepalive·재연결·유휴 종료 경계는 루프백 시험으로만 봤다. **keepalive 는 시험하지 않았고**, `cfg(unix)` 분기·실제 agent·`cargo audit` 도 확인하지 못했다.
- CI(`tauri.yml` `timeout-minutes: 20`)는 새 크레이트 89개가 더해진 **콜드 빌드**(이 환경에서 시험 포함 첫 빌드 4분 12초, `-j 2`)를 감당하는지 첫 PR 실행에서 값을 확인한다.

### PR 2b — `GitExecutor::Ssh`·`stream`·펌프(앱에 연결하지 않는다)

| 파일 | 내용 |
|---|---|
| `exec/mod.rs` | `GitExecutor::Ssh`(`kind`="ssh", `cache_key`=`ssh:<host_spec>`), `run` 이 Ssh 로 디스패치, **`stream` 이 async**. Local·WSL 은 실행기 안에서 `spawn_blocking`(CreateProcess 가 느려 런타임 스레드를 막지 않게 — 예전 `log_service` 의 `spawn_blocking` 을 옮겼다), SSH 는 원격 명령이 시작될 때까지 기다린다 |
| `exec/process.rs` | `GitStream` 이 **enum**(`Process(Box<ProcessStream>)`·`Ssh(SshStream)`). `AsyncRead`·`finish`·`pid`·`is_running`·Drop 의미는 그대로(Local·WSL 은 Job Object 종료). `Local/WslExecutor::stream` 은 여전히 동기 |
| `exec/ssh/stream.rs` | `SshStream`(소비자 쪽 `AsyncRead`), `ReadAheadExceeded`·`is_read_ahead_exceeded`, `STREAM_AHEAD_LIMIT`(16MB) |
| `exec/ssh/connection.rs` | `Connection::stream`, 펌프 태스크 `pump`, 채널 열기+exec 를 `open_exec` 한 곳으로 묶음(`run`·`stream` 공용 — clippy allow 는 늘지 않았다), 시험 훅 `set_stream_ahead_limit_for_test`(`test-hooks`) |
| `git/log_service.rs`·`git/file_channels.rs` | `stream(..).await` 로 호출. `LogCursor` 가 선읽기 상한 초과를 처리(아래) |

- **펌프(세션 정지 R1 방지)**: `stream` 은 채널·슬롯을 소유한 태스크를 띄워 **소비자와 무관하게 즉시 비운다**. 소비자는 무제한 큐(`mpsc`)에서 가져가고, 큐에 쌓인(아직 꺼내지 않은) 바이트가 16MB 를 넘으면 펌프가 `Overflow` 를 큐 끝에 넣고 원격에 KILL → close 를 보낸 뒤 **남은 출력은 버린다**(닫힘 확인 뒤 슬롯 반환).
  소비자는 이미 쌓인 데이터를 모두 읽은 **뒤에** `io::Error`(원인 `ReadAheadExceeded`, `is_read_ahead_exceeded(&e)` 로 판별)를 받는다. 취소·연결 오류와 구별되고, 같은 오류가 반복되며, `finish()` 는 `Aborted` 다(정상 종료로 보이지 않는다).
- **Drop·취소**: 스트림을 버리면(`DropGuard`) 펌프가 KILL → close → 닫힘 확인(3초 상한) 뒤 슬롯을 돌려준다. `RunOpts.cancel` 은 펌프도 보므로(읽는 중이 아니어도) 즉시 KILL 한다. 읽기는 `GitStream` 과 같이 취소를 먼저 보고 오류로 깨어난다. 시작(`stream` 호출) 중에 취소·future 버림도 같다 — **채널 열기 대기도 취소를 본다**(`open_exec` 가 `stop`·호출자 토큰을 받아, 응답이 느린 서버가 슬롯을 최대 15초 쥐지 못하게 한다. 열기 future 는 버리지 않고 정리 태스크가 넘겨받아 뒤늦게 열린 채널을 KILL·close 한다. 그 사이 서버 `MaxSessions` 를 잠깐 넘을 수 있다). `run`(`Job::run`)도 같은 함수를 쓴다.
- **`LogCursor` 의 처리**: 상한 초과는 **오류가 아니다**. 읽기 태스크가 그 커서를 `overflowed`(재사용 불가, 실패 아님)로 표시하고 지금까지 읽은 만큼만 낸다(`has_more` 참). **커밋을 하나도 받지 못하면(진척 0) 빈 페이지가 아니라 오류**(`SSH 선읽기 상한으로 첫 커밋을 받지 못했습니다`)로 끝난다 — 빈 페이지는 renderer 가 백오프 없이 되풀이한다. 앵커(마지막으로 낸 커밋의 위치·해시)는 남기므로(`anchorable`) 다음 페이지는 기존 STALE 경로 그대로
  새 커서가 `--skip=<위치-1>` + 앵커 확인(`verify_then_read`)으로 이어 읽는다. 확인 커밋을 받기 전에 끊기면 위 오류로 끝내되 기존 앵커는 유지한다(불일치로 오판해 STALE 을 내지 않는다). STALE 알림(`logStale`)도 가지 않는다.
  `file_channels::file_diff` 는 상한 초과를 오류(`ExecError::Other`)로 돌려준다.

**TS 와 다른 점(2b)**
- **원격은 backpressure 로 멈추지 않는다.** TS(`ssh2`)는 읽는 쪽이 느리면 윈도우가 차서 원격이 멈췄고, 소비자가 읽지 않으면 같은 연결의 다른 채널도 멈출 수 있었다(R1). Rust 는 펌프가 16MB 까지 먼저 읽고, **넘으면 원격을 죽이고 읽기를 상한 초과로 끝낸다**(사용자 결정 D3).
  Local·WSL 은 그대로 파이프 backpressure(상한 없음)다. 소비가 느려 상한을 넘는 일은 `git log` 커서를 오래 쉬게 둔 경우(수 MB 의 출력이 이미 쌓인 상태)에 생기며, 로그는 위 앵커 경로로 투명하게 이어진다.
- 상한은 **쌓인 바이트** 기준이라 느린 소비자가 천천히라도 계속 읽으면 넘지 않는다. 한 번 넘으면 그 스트림은 못 쓴다(재개 없음). **메시지당 고정 비용 64바이트**(`EVENT_COST`)를 페이로드에 더해 과금하고(펌프가 더하고 소비자가 같은 값을 뺀다), **빈 데이터 프레임은 버린다** — 빈·1바이트 프레임을 무제한 보내 큐 노드만 쌓는 서버를 막는다.
- 시험 훅 `set_stream_ahead_limit_for_test`(`test-hooks`)만 상한을 바꾼다 — 운영 경로에 설정 수단이 없다.

**시험**(`cargo test --features test-hooks --test ssh`, 약 50초) — `ssh.rs` 에 새 시험 16개(상한을 줄이는 3개는 `test-hooks` 가 있을 때만 컴파일된다)
- TS `stream` `it` 5개 이식(`test-map.json` 의 `N/A: PR 2` 를 전부 전환): 큰 출력·느린 소비자(첫 청크만 읽고 같은 연결의 `run` 이 끝남)·비정상 종료의 git 오류·중간 취소(`Aborted`·원격 자식 0·슬롯 반환·연결 재사용)·소비자 이탈·슬롯 수보다 많은 반복.
- 추가: 읽지 않는 스트림(512B 패킷 8192개) 옆에서 같은 연결의 `run` 5개가 끝남(세션 정지 없음), `hold` 스트림 drop·읽는 중 취소 뒤 서버 자식 0·슬롯 반환, 이미 취소된 토큰(연결도 안 맺음), 서버의 exec 거부, **상한 초과**(상한을 256KB 로 줄임 — 신호·원격 KILL·연결 계속 사용·`flooded < 4MB`),
  상한 안에서 끝난 출력은 초과가 아님, `LogCursor`·`LogService` 의 SSH 대상 시나리오(페이지 이어 붙임 = 전체 log, 확인 읽기 불일치, 취소 뒤 커서·원격 자식·슬롯 정리, **상한(32KB)으로 끊긴 커서가 오류 없이 앵커로 다시 열려 중복·누락 없이 이어짐**).
  16MB 를 실제로 보내지 않는다. 시간 의존은 `wait_for`·`timeout` 뿐이다.
- 보안 리뷰 반영(2b): 작은(1바이트) 프레임이 메시지 비용으로 상한에 걸림·소비 뒤 큐 비용이 0 으로 복귀, 빈 프레임 무시(russh 서버 API 가 빈 프레임을 못 보내 `admit` 단위 시험), 열기 응답을 미룬 서버에서 `stream`·`run` 취소 시 슬롯이 먼저 돌아오고 늦게 열린 채널이 닫힘(하네스 `hold_opens`), 진척 0 이면 오류·앵커 유지(`log_ssh_상한으로_…`)·페이지마다 진척 단언.

**남은 위험(2b)**
- `file_diff`(전체 diff 보기 등)는 SSH 에서 상한 초과를 이어 읽지 못하고 **오류**로 끝낸다. 소비자(파서)가 네트워크보다 16MB 이상 느릴 때만 생기며 실제 sshd·큰 diff 로 확인하지 않았다. **Local 은 되는 일이 SSH 에서는 안 되는 기능 차이다**(Local 은 파이프 backpressure 라 상한이 없다).
- 펌프는 데이터마다 `Vec` 를 한 번 복사한다(`CryptoVec` → `Vec<u8>`). 메시지당 고정 비용을 상한에 더해 작은 패킷도 메시지 수가 상한(16MB / 65)으로 묶인다.
- `stream` 을 쓰는 호출부는 `log_service`·`file_diff` 둘뿐이다. 라우팅(PR 3)에서 `file_diff` 의 상한 초과 처리를 다시 봤다 — 아래 "SSH PR 3" 의 "`file_diff` 상한 초과".
- 실제 sshd 지연·패킷 손실 아래의 상한 동작은 루프백으로만 봤다.

**남은 위험(3)**
- **호스트당 연결이 둘이 된다.** 사이드카가 `git:refs`·fetch·감시 등을 위해 `ssh2` 연결을 계속 쥐고, Rust 가 russh 연결을 하나 더 맺는다. 인증도 두 번이라 터치가 필요한 키·agent 확인이 필요한 키는 두 번 묻는다. 호스트의 접속 수 제한(`MaxSessions`·`MaxStartups`)에도 영향이 있다. Phase 6a 에서 나머지 git 채널·감시가 넘어오면 줄어든다. 사이드카의 `gh:repo`·repoq 의 git 조회(`remotes`·`pushedShas`)는 **6a PR 8 이 Rust 로 옮겼다**(아래 "PR 8"). 사라지는 것은 6a 가 끝났을 때다 — 이 PR 8 만으로는 아니다:
  `wipStatus`·`wipDiff`(PR 6)·RepoWatcher(PR 7b)와 저장소 **추가 흐름**의 사이드카 git(`repos:add`·`scan`·`env:test`·`env:listDir` — 6b)가 남아 있는 동안은 Rust 가 맡은 저장소에도 사이드카가 연결을 연다.
- **정리 경합.** 미러 변경 직전에 해석을 마친 요청이 정리(`sync_ssh_hosts`) 뒤에 `runtime()` 으로 들어오면 지운 호스트가 풀에 다시 생긴다. 다음 미러 변경이나 유휴 5분(연결 해제) 뒤에 정리되며 기능 영향은 없다.
- `file_diff` 상한 초과(위)·실제 sshd 에서의 라우팅은 확인하지 않았다(루프백 서버만).
- 검증 중 `cargo test` 가 한 번 `fetch_service` 의 `git clone file://…`(시험 준비용 git)에서 15분 넘게 멈춘 적이 있다 — 1회 관측, 원인은 확인하지 못했고 그 바이너리를 단독으로 다시 돌리면 통과했다(PR 3 의 코드는 그 바이너리에서 `MirrorRepo` 리터럴 한 줄만 바뀌었다).

### PR 3 — 라우팅(`sshHost` 미러·`SshPool`·`ANTLER_RUST_SSH`)

ssh 저장소의 `git:commit`·`git:log`·`git:compare`·`git:fileDiff`·`git:fileHistory`·`git:blame` 6개를 Rust 가 처리한다. **`git:fetch` 는 이 PR 에서는 사이드카였다**(PR 4 에서 바꿨다 — 아래). 새 IPC 채널은 없다.

| 파일 | 내용 |
|---|---|
| `src/sidecar/{protocol,mirror}.ts` | 미러 항목에 `sshHost`(ssh 소스일 때만) 추가. `tests/sidecarMirror.test.ts` 가 고정 |
| `mirror.rs` | `MirrorRepo.ssh_host`·`Target::Ssh{host,path}`(`source_key` = `ssh:<host>`)·`check_ssh_host`·`resolve_fetch`·`ssh_hosts`·ssh 스위치(`set_ssh`) |
| `git/exec/ssh/pool.rs` | `SshPool`: 호스트마다 `SshExecutor` 하나(= 연결 하나). `get`·`retain`(쓰지 않는 호스트를 dispose 하고 풀에서 뺀다)·`dispose_all`. 만드는 방법은 주입(운영 `for_home`, 시험은 임시 홈) |
| `router.rs` | `SSH_LIMIT`(4, `exec/queue.rs`) 큐를 호스트마다 하나·`Router::{ssh_enabled, ssh_active, set_ssh_pool, ssh_pool}`·`rust_ssh_enabled`·`on_mirror_changed` 가 `sync_ssh_hosts`·`shutdown` 이 `dispose_all` |
| `lib.rs`·`sidecar.rs` | 풀 주입(홈은 `USERPROFILE`/`HOME` → Tauri `home_dir`) → fetch 서비스 → `rust_ssh = router.ssh_active()` → 사이드카 env `ANTLER_RUST_SSH` |

- **스위치**: `ANTLER_RUST_SSH` — 기본 켬, `0` 이면 끈다. `ANTLER_RUST_GIT=0` 이면 함께 꺼진다(`api` 와 같은 관례). 판정은 `Router::ssh_active()`(스위치 + 풀 주입) **하나**이고 ① 미러가 ssh 를 `Resolved::Rust` 로 풀지(`RepoMirror::set_ssh` — 풀을 주입할 때 켠다)
  ② 사이드카 env 가 `1` 인지가 같은 값이다. Rust 가 env 를 직접 정한다(켜면 `cmd.env("ANTLER_RUST_SSH","1")`, 아니면 `env_remove` — 부모 환경이 뒤집지 못한다). 꺼져 있으면 풀도 만들지 않아 `~/.ssh` 를 읽지 않고 ssh 는 전부 사이드카다.
  **사이드카(TS)는 이 env 를 읽지 않는다**(PR 4 에서도 fetch 소유는 `ANTLER_RUST_FETCH_KINDS` 만 읽는다 — ssh 포함 여부를 Rust 가 같은 `ssh_active` 로 정해 넘긴다). 해석을 미러 한 곳에서만 갈라 로그 커서·fetch 서비스·라우터가 같은 답을 본다.
- **`check_ssh_host`**(`handlers.ts` 의 `checkEnvInput` 규칙): 앞뒤 공백을 지운 값이 비지 않음·255 UTF-16 단위 이하·공백(JS `\s`)·NUL 없음·`-` 로 시작하지 않음. **검사한 값(공백을 지운 값)이 풀 키·큐 키·실행기 호스트이다** — 다른 값을 쓰면 ` -x` 가 검사를 빠져나간다.
  검사에 걸리면 연결하지 않고 `ERROR "잘못된 SSH 호스트입니다"`(사이드카로 넘기지 않는다 — 로컬·WSL 과 같다). 호스트가 `~/.ssh/config` 에 있는지는 연결할 때 실행기가 본다(확인 창은 저장소를 **추가할 때만** 뜬다 — 사이드카 몫이고 이 PR 은 그대로다).
- **연결 정리**: `on_mirror_changed` 는 `logs.sync_mirror()` 뒤에 "지금 미러에서 Rust 로 풀리는 ssh 호스트 집합"에 없는 호스트를 **풀과 `sources["ssh:<host>"]` 를 함께** 비운다(`sources` 잠금을 쥔 채 풀을 만진다 — `runtime` 과 같은 순서).
  `SshExecutor::dispose` 는 영구 거부라 풀에 남기면 같은 호스트를 다시 추가했을 때 죽은 실행기가 나온다(시험: 다시 추가한 호스트가 새 연결로 동작). 쓰는 호스트는 연결을 그대로 쓴다. `shutdown` 도 `dispose_all`.
  앱 종료(`RunEvent::Exit`)는 런타임 밖 스레드라 `disconnect` 해제 메시지를 보내지 못하고 연결만 버린다(프로세스가 끝나며 소켓이 닫힌다) — `in_runtime_of` 로 감싸지 않았다.
- **이중 fetch 방지**: fetch 소유 판정을 `Resolved::Rust(_)`(해석 결과)가 아니라 **소스 종류**로 바꿨다 — `RepoMirror::resolve_fetch` 가 원시 `kind` 를 `RUST_FETCH_KINDS`(`local,wsl`)와 먼저 대조하고(검증보다 먼저 — 호스트가 잘못된 ssh 의 `git:fetch` 도 오류가 아니라 사이드카),
  fetch 서비스(`resolve`·`is_rust_repo` — 소유·활성·`sync_mirror`)와 라우터(`git_fetch`·`repos:watch` 의 자동 fetch 대상)가 이 함수만 쓴다(소스 검사가 `mirror.resolve(` 호출이 없음을 강제). 사이드카 env `ANTLER_RUST_FETCH_KINDS` 는 PR 3 시점에는 계속 `local,wsl` 이었다.
  **PR 4 에서 상수가 `rust_fetch_kinds(ssh_active)` 계산 값으로 바뀌어** ssh 를 포함하고 `fetchOwns.ts` 도 같이 바뀌었다(아래 "PR 4"). fetch 서비스의 `make_runner` 는 `Target::Ssh` 에 같은 풀의 실행기를 준다.
- **`file_diff` 상한 초과**: ssh 의 `file_diff` 는 원격 출력이 소비자보다 16MB 이상 앞서면(소비자가 네트워크보다 느릴 때만) 이어 읽지 못하고 오류로 끝난다. **PR 3 에서는 그대로 두기로 했다** — 로컬 파서가 네트워크보다 느린 경우는 드물고
  재시도·`ANTLER_RUST_SSH=0`(사이드카의 backpressure 경로)으로 비껴갈 수 있으며, 소비자 쪽 대기(원격을 멈추는 방법)는 russh 윈도우 정지(R1)를 되살린다. 실제 sshd·큰 diff 로는 확인하지 않았다(아래 "남은 위험").
- **큐·캐시**: ssh 큐는 호스트마다 하나, 동시 4(`SSH_LIMIT` — 연결당 채널 6 안). 커밋·compare·diff 캐시는 저장소 id 키라 local·wsl 과 같은 LRU 를 나눠 쓴다(`runtime` 만 소스별이다).

**TS 와 다른 점(PR 3)**
- `check_path` 의 NT 네임스페이스(`\??\`) 거부가 **ssh 경로에도** 걸린다— `handlers.ts` 의 `checkPath` 도 모든 kind 에 같은 검사를 하므로 규칙은 같지만, TS 는 저장소를 **추가할 때만** 보고 Rust 는 **쓸 때마다** 미러 값을 다시 본다(설정 파일을 손으로 고친 값도 걸린다). 원격 POSIX 경로에 `/??/` 가 들어가는 일은 현실에 없다.
- `check_ssh_host` 는 제어 문자 전체를 거부한다(TS 는 NUL 과 `\s`). 호스트가 없는(`sshHost` 없음) ssh 항목도 거부한다(TS 는 `source.sshHost!`).
- 홈은 환경변수(Windows `USERPROFILE`·그 밖 `HOME`)가 있으면 그것, 없으면 Tauri `home_dir` 이다 — libuv `uv_os_homedir`(Node `os.homedir()`)가 환경변수를 먼저 보는 것과 같은 순서로 맞췄다(Node 쪽 동작은 코드로 대조하지 않았고 기억에 의한다). 홈을 알 수 없으면 풀을 주입하지 않아 ssh 는 사이드카다.

**시험**(`cargo test --features test-hooks`)
- `tests/channels.rs`: commit·log·파일 단위 4채널 시나리오를 **루프백 SSH 서버 뒤에서 한 번 더** 돌려 같은 TS 골든과 비교한다(`git_commit_시나리오가_ssh_에서도_…`·`git_log_…`·`파일_단위_채널_시나리오가_ssh_에서도_…` 3개 — 헬퍼로 뽑아 local 과 본문 공유, 서버·풀은 시험마다 하나, 끝에 `connections()==1`). 파일 단위 SSH 시험은 약 1분 걸린다(원격 git 프로세스 수 × 로컬 프로세스 비용).
- `tests/ssh_route.rs`(새 파일, 9개): 잘못된 호스트(공백·탭·`-` 시작·공백 뒤 `-`·NUL·제어 문자·256자·빈 값·없음)가 6채널 모두 연결 없이 `ERROR`·호스트 앞뒤 공백 제거·스위치 끔(`ANTLER_RUST_SSH=0`·`ANTLER_RUST_GIT=0`·풀 없음)은 사이드카·연결 하나 공유와 쓰지 않는 호스트 정리·**재추가**·호스트가 바뀐 저장소·종료 시 전부 닫힘·`SSH_LIMIT=4`·
  소스 검사(`lib.rs` 순서: 스위치 확인 < 풀 주입 < fetch 서비스 < `ssh_active` < `manage(router)` < `Sidecar::spawn`, 사이드카 env 를 직접 정함).
- `tests/fetch_route.rs`(새 4개): ssh 가 Rust 로 풀려도 `git:fetch`(수동·자동·호스트가 잘못된 것 포함)는 사이드카, 서비스에 닿지 않음·`repos:watch` 는 `active None`·진짜 `FetchService` 가 ssh 를 소유하지 않음("unknown repo", 실행기 0개)·소스 검사(`resolve_fetch` 만).
- 단위(lib +7): `mirror.rs`(호스트 검사·스위치·`resolve_fetch`·메시지의 `sshHost`) 3개 추가, `router.rs`(스위치 술어·풀이 있어야 Rust) 2개, `pool.rs` 2개. vitest `sidecarMirror` 1개 추가.

### PR 4 — ssh fetch 를 Rust 로(Phase 5 마지막 코드 PR)

ssh 저장소의 `git:fetch`(수동·자동·포커스)를 Rust 의 `FetchService` 가 맡는다. 새 IPC 채널·새 크레이트는 없다. 켜고 끄는 것은 PR 3 의 `ANTLER_RUST_SSH`(`ssh_active`) **하나**다.

| 파일 | 내용 |
|---|---|
| `router.rs` | `RUST_FETCH_KINDS` 상수 → `rust_fetch_kinds(ssh_active)` 계산 값(`local,wsl` + ssh 활성이면 `,ssh`). `sidecar_fetch_kinds(router_enabled, ssh_active)` |
| `mirror.rs` | `resolve_fetch` 가 자기 ssh 플래그(= `ssh_active`)로 같은 함수를 부른다 — 소유 판정은 **종류 대조**다(검증보다 먼저) |
| `lib.rs`·`sidecar.rs` | `rust_ssh = router.ssh_active()` → `fetch_kinds = sidecar_fetch_kinds(enabled, rust_ssh)` → 사이드카 env `ANTLER_RUST_FETCH_KINDS` 와 `ANTLER_RUST_SSH`. fetch 서비스의 `make_runner` 가 `Target::Ssh` 에 **라우터와 같은 `SshPool`** 의 실행기를 준다 |
| `src/sidecar/fetchOwns.ts` | 허용 토큰에 `ssh` 추가(같은 PR — 안 그러면 `local,wsl,ssh` 를 받은 사이드카가 ssh 를 계속 fetch 해 이중 fetch 다) |
| `git/exec/error.rs`·`ssh/connection.rs`·`ssh/stream.rs` | `ExecError::Ssh`(Antler 와 호스트 사이의 전송 오류) 추가. `connection.rs`·`stream.rs` 의 오류는 모두 이 종류다. envelope code 는 `ERROR`(TS 의 일반 `Error`) |
| `git/fetch/service.rs`·`run.rs` | 아래 "분류"·"정리". `GitRunner::is_closed`(기본 거짓, `GitExecutor::Ssh` 만 `dispose` 여부) |

- **술어가 하나다(이중 fetch·미소유 방지)**: 사이드카 env 와 미러의 `resolve_fetch` 가 같은 `rust_fetch_kinds` 를 부르고, 둘의 입력(`ssh_active`)이 같은 값이다. 스위치를 끄면(`ANTLER_RUST_SSH=0`·`ANTLER_RUST_GIT=0`·홈을 모름) env 에서 `ssh` 가 빠지고
  미러가 ssh 를 풀지 않아 사이드카가 fetch 한다. ssh 가 Rust 로 풀리는데 호스트가 잘못된 저장소(`-oProxyCommand=x` 등)의 `git:fetch` 는 local·wsl 의 검사 실패와 같이 **사이드카로 보내지 않고 `ERROR "잘못된 SSH 호스트입니다"`** 다(사이드카는 그 저장소를 맡지 않는다).
  `repos:watch` 의 자동 fetch 대상에도 ssh 가 들어간다.
- **ssh 의 fetch 호출**(코드는 local·wsl 과 같고 실행기 종류만 다르다): 훅 차단은 `/dev/null`(앱 소유 빈 디렉터리는 local 만), ssh 명령은 **원격 저장소의 사용자(시스템·전역) `core.sshCommand`**, 없으면 `ssh -o BatchMode=yes -o ConnectTimeout=15`
  (`GIT_SSH_COMMAND`·`GIT_SSH` 환경변수는 local 만 읽는다 — 이 PC 의 환경변수가 원격으로 가지 않는다). 환경은 `FetchEnv`(닫힌 타입)가 `build_remote_command` 를 거쳐 원격의 `env K='V' … git -C <경로> -c … fetch …` 로만 간다(`SshExecutor::run`). refspec 고정·`--refmap=`·`ALLOWED_KEYS` 허용 목록은 원격에서도 그대로다.
  원격 git 이 쓰는 `~/.ssh` 는 원격 계정의 것이다(이 PC 의 `~/.ssh` 가 아니다).
- **연결 공유**: fetch 서비스와 라우터가 같은 `SshPool` 의 실행기를 쓴다 — 호스트당 연결 하나·채널 6개 한도 공유(TS `fetchContext` 가 `runtimes` 의 같은 실행기를 주는 것과 같다). fetch 큐는 환경마다 하나(동시 1)다. 골든 시험이 시나리오마다 `connections() <= 1` 을 본다.
- **분류**: SSH 전송 오류(`ExecError::Ssh` — 연결 거부·시간 초과·호스트 키 불일치·인증 수단 없음/거부·셸 확인 실패·연결 끊김·exec 거부)는 **오류 종류로** `network`(지수 백오프)다. 문구 정규식에 기대지 않는다 —
  TS 는 한국어 문구라 정규식(`permission denied`·`host key verification failed`·`publickey`)에 걸리지 않아 우연히 network 가 됐지만, Rust 문구에 그 영어 단어가 섞여도(russh 오류 문구) 인증 중단이 되지 않게 했다.
  원격 **git** 이 낸 오류(`ExecError::Git`: 원격 git 이 자기 remote 에 인증하지 못한 `Permission denied (publickey)`·http 401 등)는 TS 와 같이 문구로 분류한다(auth) — 골든 시나리오가 이 둘을 가른다.
  `ExecError::Aborted` 와 60초 시간 초과는 기존대로 network 다.
- **정리(리뷰 지적)**: ① 호스트가 미러에서 빠지면 라우터의 `sync_ssh_hosts` 가 풀의 연결을 닫고(`dispose`), fetch 서비스의 `sync_mirror` 가 `sources` 에서 `ssh:<호스트>` 환경(실행기·큐·버전 캐시)을 함께 뺀다 — 저장소 상태가 없어도 호스트 단위로 거른다.
  ② 서비스는 `sources` 에서 닫힌 실행기(`GitRunner::is_closed`)를 만나면 버리고 `make_runner`(= `pool.get`)로 새로 받는다 — 풀의 정리와 서비스의 정리 사이에 같은 호스트가 다시 추가돼도(삭제→재추가가 두 번의 `on_mirror_changed` 사이에 끼는 순서) 죽은 실행기를 쥐지 않는다.
  ③ **낡은 대상 재등록 경합**: `resolve` 와 `sources` 잠금 사이에 미러가 바뀌어 호스트가 빠졌으면 실행기를 만들지 않고 `"저장소 목록이 바뀌었습니다. 다시 시도하세요"`(라우터 `runtime` 과 같은 문구·같은 잠금 순서 `sources` → 풀). 지운 호스트로 `pool.get` 해 풀에 낡은 항목이 다시 생기지 않는다.
  이 경합 자체는 시험으로 재현하지 못했다(결정적으로 끼워 넣을 이음새가 없다) — ①②의 결과(삭제 뒤 재추가하면 새 연결로 fetch 성공, 닫힌 실행기 재생성)는 시험한다.
- **골든**: `fetch.scenarios.json` 에 `target:"ssh"` 시나리오 5개(수동 성공·승인, 위험 설정의 자동 차단과 수동 확인, 원격 git 의 http 401(auth), SSH 연결 거부(network), 호스트 키 불일치(network)). TS 는 ssh2 하네스(`tests/sshHarness.ts`)와 `SshExecutor` 를
  임시 홈으로, Rust 는 루프백 `russh::server`(`tests/ssh/server.rs`)와 `SshExecutor` 로 같은 골든(`tests/golden/channels/fetch.json`)을 낸다. 원격 저장소의 remote 는 경로 URL(수동 fetch 는 file 전송 허용)과 기존 127.0.0.1 가짜 http 서버(401)다.
  TS 쪽은 `RepoService` 의 실행기 공장이 실제 `~/.ssh` 를 읽으므로 fetch 가 쓰는 `fetchContext`·`signature` 두 메서드만 임시 홈의 `SshExecutor` 로 바꾼 서브클래스를 쓴다(운영 코드는 건드리지 않았다).
  Rust 하네스는 원격 명령의 `env K='V'` 에서 `GIT_SSH_COMMAND`·`GIT_ALLOW_PROTOCOL`(file 허용 여부)을 읽어 닫힌 `FetchEnv` 로만 자식 git 에 준다(자유 env 는 적용하지 않는다). `tests/golden/exec/env.json` 의 `fetch` 에 ssh 표본 2개(원격 셸이 받는 명령 원문)를 더했고 `exec.rs` 가 같은 `build_remote_command` 결과인지 본다.
  재생성: `UPDATE_GOLDEN=1 npx vitest run tests/goldenChannels.test.ts -t "git:fetch"`, `UPDATE_GOLDEN=1 npx vitest run tests/goldenExec.test.ts`.

**TS 와 다른 점(PR 4)**
- SSH 전송 오류의 network 분류가 **종류 기반**이다(TS 는 문구가 한국어라 정규식을 비껴가 network 가 되는 것에 기댄다). 결과(code·authPaused=false)는 같다. 오류 문구는 Rust 가 PR 2 의 문구다(골든은 문구를 비교하지 않는다).
- 삭제된 호스트·닫힌 실행기 처리는 Rust 의 미러(사이드카가 보낸 목록)와 풀 구조에서 나온 것이다. TS 는 `releaseUnused` 가 설정에서 쓰지 않는 소스의 런타임을 놓는다.
- 서버발 채널·ProxyCommand 거부 등 SSH 계층의 규칙은 PR 1~3 그대로다(docs/security.md "Tauri 판의 SSH").

**시험**(`cargo test --features test-hooks`)
- `channels.rs`: `git_fetch_ssh_시나리오가_ts_골든과_같다`(ssh 시나리오 5개). 기존 local 시험은 target 이 없는 시나리오만 돌고, 두 시험 모두 고른 시나리오가 0개면 실패한다(조용히 건너뛰지 않는다). 서버 하네스 `ssh/server.rs` 에 fetch 환경 반영을 더했다.
- `fetch_route.rs`(ssh 절 교체, 6개): ssh 활성이면 `git:fetch` 가 서비스로(호스트가 잘못되면 `ERROR`, 사이드카 env 에 ssh 포함)·`ANTLER_RUST_SSH=0`/`ANTLER_RUST_GIT=0`/풀 없음은 사이드카·env 에 ssh 없음·`repos:watch` 가 ssh 를 활성으로·진짜 서비스가 ssh 를 소유하고 닫힌 실행기를 새로 받으며 전송 오류가 network·
  스위치가 꺼지면 서비스가 소유하지 않음·호스트가 빠졌다 재추가되면 서비스도 새 실행기를 받음(루프백 거부 포트)·소스 검사(`rust_fetch_kinds` 한 곳, `RUST_FETCH_KINDS` 상수 없음).
- `ssh_route.rs`(+1): 루프백 서버 너머 저장소의 수동 fetch 성공, 라우터와 연결 하나 공유, 호스트 삭제 → 연결 닫힘 → 재추가 → 새 연결(`connections()==2`)로 fetch 성공.
- `fetch_service.rs`(+1): 전송 오류는 문구가 auth 단어여도 network(백오프 10분→20분), 같은 문구의 `Git` 오류는 auth.
- 단위: `mirror.rs`(`resolve_fetch` 가 스위치에 따라 ssh 를 포함)·`error.rs`. vitest `sidecarFetchOwns`(+1: `local,wsl,ssh` 면 ssh 도 맡지 않는다).

**사람 확인 필요(SSH PR 4)**: 실제 sshd 의 저장소로 ① 수동 fetch 가 새 커밋을 받고 그래프가 갱신되는지, ② 원격 저장소에 위험 설정(`core.sshCommand` 등)을 넣고 새로고침하면 확인 창이 뜨고 [이번만][항상][취소]가 맞게 동작하는지(자동 fetch 는 묻지 않고 차단),
③ 자동 fetch 가 **한 번만** 도는지(`ANTLER_ROUTE_LOG=1` 로 `[route] git:fetch -> rust`, 원격의 `ps`/`last` 로 호스트당 연결 하나·fetch 가 중복되지 않는지), ④ 네트워크를 끊거나 호스트 키를 바꿨을 때 오류가 표시되고 백오프(수동 새로고침 전까지 멈추지 않고 간격이 길어짐)이며 "인증 일시 정지"가 아닌지,
⑤ `ANTLER_RUST_SSH=0` 으로 실행하면 같은 저장소가 사이드카로 fetch 되는지, ⑥ ssh 저장소 삭제 뒤 같은 호스트를 다시 추가하고 fetch 가 성공하는지.

## 나머지 git 채널(Phase 6a, `src-tauri/src/git/`)

Rust 로 옮기는 것: 사이드카가 처리하던 git 채널 12개(`git:refs`·`refsDetails`·`search`·`activity`·`resolveRev`·`tree`·`blob`·`onRemote`·`repoExtras`·`lfsPaths`·`wipStatus`·`wipDiff`)와 RepoWatcher.
GitHub 원격 전용(github kind) 저장소는 미러가 계속 사이드카로 보낸다. 되돌리기 스위치는 늘리지 않는다 — `ANTLER_RUST_GIT=0`(전부 사이드카)·`ANTLER_RUST_SSH=0`(ssh 만)이 새 채널과 감시에도 그대로 적용된다.

**결정(사용자 승인)**: ① refs 정렬은 `natural_compare` 근사를 유지한다(ICU 를 들이지 않는다 — 한글↔라틴 경계만 TS 와 다르다, 위 "골든 테스트"). ② `notify` 크레이트는 PR 7b 에서 도입한다(이 PR 에는 새 크레이트가 없다).
③ #113 에서 TS 에 들어간 방어(untracked 허용 목록·정션/심볼릭 링크 거부·unstaged `~숫자` 거부·서브모듈 경로 검증과 realpath)는 Rust 에 **동등 이상으로** 옮긴다.
④ PR 8(`gh:repo`·repoq 의 git 조회를 Rust 가 처리)은 6a 에 **포함**한다(2026-11-17) — 하지 않으면 SSH 호스트당 사이드카 `ssh2` 연결이 6a 뒤에도 남는다.

| PR | 내용 | 앱 연결 | 상태 |
|---|---|---|---|
| 1 | 순수 로직·골든: 코드 탐색기 트리 파서·정렬·입력 검증, LFS·gitflow 파서, 활동 집계, 기본 브랜치, filter 가드, WIP 경로 검증·링크 확인. TS 는 순수 함수 export 만 | 없음 | **완료** |
| 2 | 코드 탐색기 4채널(`resolveRev`·`tree`·`blob`·`onRemote`) | 있음 | **완료** |
| 3a / 3b | 3a: 공유 상태(첫 페이지 문·ref 시그니처 저장소, `signature.rs` 추출, note 팬아웃). 3b: `refs`·`refsDetails`(서브모듈 확인 포함) | 3a 없음 / 3b 있음 | 3a·3b **완료** |
| 4 | `search`·`activity` | 있음 | **완료** |
| 5 | `repoExtras`·`lfsPaths` | 있음 | **완료(코드)** — 작업 트리 `--no-index` 읽기는 #121 의 `FilterGuard` 로 필터를 무력화한다(아래 "필터 실행 차단") |
| 6 | `wipStatus`·`wipDiff`(filter 드라이버 무력화 — 보안 리뷰 집중) | 있음 | |
| 7a / 7b | 7a: 감시 상태 기계·가짜 `DirWatch`·멈춘 시계 시험. 7b: notify 어댑터·라우팅·포커스 배선·사이드카 감시 `owns`·activity 의 시그니처 재사용 | 7a 없음 / 7b 있음 | 7a **완료** |
| 8 | `gh:repo`·repoq 의 git 조회 | 있음 | **완료(코드)** — 저장소 추가 흐름의 사이드카 git 은 6b 몫이다 |

모든 PR 의 제목은 `refactor:` 다(야간 릴리스 회피). 마지막 PR 에서 이식 대응표의 `N/A: PR n` 이 0개여야 한다.

### PR 1 — 순수 로직과 골든(앱에 연결하지 않는다)

Electron 동작은 그대로다(TS 쪽은 `export` 추가와 순수 함수 추출뿐이고 기존 vitest 가 증명한다).

| 파일 | 내용 |
|---|---|
| `git/code_browse.rs` | `CommitId`(40·64자 16진수, **대소문자 무시** — `scope::is_hash`(소문자만)·`HexHash`(4~64자)와 다르다)·`normalize_dir`·`normalize_file_path`·`is_resolvable_ref`·`resolved_commit_id`·`is_missing_object_error`·`image_mime`·`parse_tree`·`compare_names` |
| `git/lfs.rs` | `parse_lfs_pointer`·`parse_check_attr_lfs`·`added_lines`·`gitattributes_uses_lfs`·`parse_gitflow_config`(`GitflowConfig`) |
| `git/activity.rs` | `bucketize`(`now` 를 인자로)·`TimeStreamParser`(`Uint32Array` 처럼 2^32 로 감긴다)·`empty_activity` |
| `git/refs_service.rs` | `pick_default_branch`·`is_safe_submodule_path`·`submodule_base`·서브모듈 상한 상수 |
| `git/wip.rs` | `FilterGuard`(닫힌 타입 — filter 드라이버 무력화 `-c` 인자를 만드는 유일한 곳; 키는 바이트로 받아 UTF-8 엄격 검사, 드라이버 256개·`-c` 24000자 상한)·`filter_driver_names`·`assert_rel_path`·`has_short_name_segment`·`WipArea`·`validate_wip_diff_request`·`is_untracked_dir_entry` |
| `git/link_guard.rs` | `assert_no_link_escape`(구성 요소마다 lstat — 정션·symlink 거부)·`assert_inside_root`(`canonicalize` 로 8.3 이름을 편 실제 경로가 저장소 안이고 `.git` 이 아닌지)·순수 비교 `check_real_path` |

**TS 쪽 변경(동작 불변)**: `codeBrowse.ts` 의 `imageMime`·`assertCommitId`·`compareNames`·`parseTree` 를 export 하고 `isResolvableRef` 를 추출(`resolveRev` 가 부른다), `wip.ts` 의 `assertRelPath` export 와 `filterOverrideArgs(stdout)`(#114 의 `parseFilterDrivers` 결과를 `-c` 로 펴고 총 길이 상한을 보는 순수 함수 — 골든이 부른다)를
추출(`filterOverrides` 가 부른다 — 명령 실행·종료 코드 처리는 그대로). `parseFilterDrivers`·`hasShortNameSegment`(끝의 `.`·공백을 벗긴 뒤 판정)는 #114 의 것 그대로다. `tests/fixtures/itTitles.ts` 가 `it.skipIf(...)(…)`·`it.runIf` 제목도 읽는다(#113 이 `wip.test.ts` 에 `skipIf` 를 넣었다).

**골든**(`tests/golden/parsers/manifest.json` 에 27개 추가, 총 150개 — `UPDATE_GOLDEN=1 npx vitest run tests/golden.test.ts`): 입력은 `scripts/lib/edgeFixtures6a.mjs`(`make-edge-fixtures.mjs` 가 부른다)가 `tests/fixtures/edge/` 에 만든다.
`.bin` 은 nulRecords 골든이 전부 돌리므로 피했다(`.dat`·`.txt`·`.json`). JSON 입력 목록은 케이스 390개이고 골든은 `{in, out}`(던지면 `{in, error}`)로 짝을 남긴다.

| 골든 | 입력·경계 |
|---|---|
| `tree`(3) | ls-tree 출력: 대소문자·디렉터리 우선·`İ`/`ẞ`(소문자화로 길이가 달라진다)·비 BMP 와 U+FF5E(UTF-16 순서)·시그마·잘린 레코드·깨진 UTF-8·`dir` 접두 |
| `commitId`·`normalizeDir`(2)·`resolveRef`·`imageMime` | 전체 해시 대소문자·길이 경계·`\n`·전각·`ſ`/비문자열, 경로 정규화 `..`·`.`·빈 구성 요소·NUL, ref 모양(`/*`·`HEAD`·hex·`--`), 확장자 대소문자 |
| `lfsPointer`·`checkAttrLfs`·`addedLines`·`gitattributes` | CRLF·확장 키·중복 키·15자리 크기·1024(UTF-16 단위) 경계(한글·이모지)·BOM·U+0085/U+FEFF/NBSP 경계 |
| `gitflowConfig` | 제어 문자·U+2028·U+0085·100(UTF-16 단위) 경계·키 대소문자·같은 키 덮어쓰기 |
| `timeStream`(7)·`bucketize` | 청크 1·3·5·7 분할, 2^32 감김·2^53 반올림·309자리 이상(Infinity), `now` 고정 인자·구간 단위(1h·6h·1d·1w·30d·1y) 경계·미래 허용 폭 정확히 2일 |
| `defaultBranch` | origin/HEAD 우선·JS trim 경계(BOM·U+0085·NBSP)·같은 이름이 두 리모트에 |
| `filterOverrides` | git-lfs 정확 일치·뒤에 명령을 붙인 값·`=` 든 이름은 오류·`.` 든 이름·U+0085/U+FEFF/NBSP·값에 개행·순서·접미사가 어긋난 키 |
| `relPath`·`shortName`·`submodulePath` | `..`·`.git`·`.GIT`·드라이브 문자·백슬래시·`GIT~1`·`.git.`·4096(UTF-16 단위) 경계(한글·이모지)·비문자열 |

**대응표**(`tests/golden/channels/test-map.json`): 대상 TS 테스트 15개(`codeBrowse`·`activity`·`activityRepo`·`lfsGitflow`·`refs`·`refsDetails`·`search`·`wip`·`wipFilter`·`wipFilterNonUtf8`·`hostileRepo`·`submodule`·`defaultBranch`·`repoWatcher`, `repoService` 는 첫 페이지 문·저장소 추가·`ConfigStore` 만 — `RepoService.log` 는 Phase 2 몫)의 모든 `it` 199개가 있어야 한다.
vitest(`tests/golden6a.test.ts`)와 Rust(`golden.rs::phase6a_대응표가_가리키는_rust_테스트가_있다`)가 같은 규칙으로 강제한다. 값은 `<파일>.rs::<fn>`(그 fn 이 있다) · `N/A: PR n[a|b]`(뒤 PR 몫) · `N/A: <사유>`(renderer 전용·6b·수동 측정)다.
이번 PR 에서 `<파일>.rs::<fn>` 은 54개, `N/A: PR n` 은 90개, 사유 있는 N/A 는 43개다. `<파일>.rs::<fn>` 으로 옮긴 것은 TS 테스트와 같은 입력을 쓰는 Rust 단위 테스트이거나(activity·lfs·defaultBranch·isSafeSubmodulePath),
Phase 2 가 이미 옮긴 파서의 골든(`parseRefs`·`parseWorktrees`·`parseStashes`·`parseGitlinks`·`parseGitmodules`·`parseProbe` — TS 가 인라인 입력을 쓰는 곳은 Phase 2 의 edge fixture 가 대신한다)이거나, `exec.rs`(`hostileRepo` 5개)다.
실제 git·`RepoService` 가 있어야 하는 `it` 은 그 채널이 오는 PR 의 `N/A: PR n` 이다.

**#113 방어의 옮김(D4)** — 파일 시스템을 보지 않는 것은 이번 PR 에서, 채널에 묶인 것은 그 채널 PR 에서 옮긴다:

| 방어(TS) | Rust | 상태 |
|---|---|---|
| `assertRelPath`(NUL·4096·`/`·`\`·드라이브·`..`·`.git`) | `wip::assert_rel_path` | 이번 PR(골든) |
| untracked 허용 목록(정확히 일치, 목록 없으면 거부) | `wip::validate_wip_diff_request` 가 `Option<&HashSet<String>>` 을 받는다 | 판정은 이번 PR, **목록의 보관(저장소별 마지막 `wipStatus` 가 보고한 경로, 저장소 제거 시 정리)은 PR 6** |
| unstaged `~숫자` 구성 요소 거부 | `wip::has_short_name_segment`(골든) | 이번 PR |
| `isSafeSubmodulePath`(절대·드라이브·빈/`.`/`..`) | `refs_service::is_safe_submodule_path`(골든) | 이번 PR, 호출은 PR 3b(서브모듈 확인 — 완료) |
| 정션·symlink 거부(`assertNoLinkEscape`)·`assertInsideRoot`(realpath, 8.3 이름, `.git`) | `link_guard::assert_no_link_escape`·`assert_inside_root` | 이번 PR 에 구현·시험(실제 정션·`GIT~1`). **호출은 로컬 저장소 한정 — untracked·unstaged 는 PR 6, 서브모듈 경로는 PR 3b** |
| FilterGuard(`filter.<d>.*` 덮어쓰기, 이름의 `=` 거부) | `wip::FilterGuard` | 이번 PR(골든), `-c` 인자 소스 검사 시험은 PR 6 |

- `link_guard` 는 TS 가 쓰는 두 사실을 실제로 확인하고 지었다(`link_guard.rs` 시험, 이 PC 의 NTFS): Rust `symlink_metadata` 가 정션(`mklink /J`)을 symlink 로 보고하고, `canonicalize` 가 8.3 `GIT~1` 을 긴 이름 `.git` 으로 편다(이 볼륨이 짧은 이름을 만들지 않으면 그 시험은 건너뛴다 — TS 의 `skipIf` 와 같다).
- TS 보다 엄격한 곳: `assert_no_link_escape` 는 `.`·`..` 구성 요소를 어휘적으로 걷어 내지 않고 거부한다(TS 는 `path.join` 이 걷어 낸다), Windows 에서 `:` 가 든 구성 요소(드라이브 상대·대체 데이터 스트림)도 거부한다.
  `.` 를 건너뛰면 `j/.`(`j` 가 정션)에서 `j` 가 마지막 구성 요소가 되어 `allow_leaf_link`(unstaged)로 통과하므로 거부로 닫았다. `assert_rel_path` 는 `..` 만 거르고 `.`·`:` 는 통과시키므로 이 두 곳은 닿을 수 있는 입력이다(정상 경로는 git 이 `./`·`a:b` 를 보고하지 않는다).
- WSL·SSH 저장소에서는 호출하지 않는다(TS 와 같다 — Rust 가 그 파일 시스템을 보지 못한다. Linux git 이 디렉터리 symlink 를 따라 들어가지 않는다는 가정은 여전히 실측하지 않았다, `docs/security.md`). 검사와 열기 사이의 경쟁(TOCTOU)은 막지 못한다(TS 와 같다).
- 문자열 검사(`assert_rel_path`)는 `GIT~1/config`·`.git./config`·`.git /config` 를 **통과시킨다**(골든이 이를 고정한다). 막는 것은 허용 목록(untracked)·`~숫자` 거부(unstaged)·실제 경로 확인(둘 다, 로컬)이다 — 이 셋을 모두 PR 6 이 갖춰야 한다.

**TS 와 다른 점**
- `image_mime` 은 객체 프로토타입 이름(`a.constructor`·`a.toString`·`a.__proto__`)을 확장자로 쳐 주지 않는다. **TS 는 `IMAGE_MIME[ext]` 객체 조회라 이런 확장자가 함수·객체로 잡혀 이미지로 취급되고 `data:function Object()…` 가 만들어진다**(발견만 하고 고치지 않았다 — TS 쪽 수정은 따로).
  이 입력은 골든에 넣지 않았다. 이 PR 의 범위 밖이라 TS 는 그대로다.
- `bucketize` 는 `times: &[T]`(`T: Into<f64>`)와 `now` 를 받고 `n` 은 슬라이스 길이다. 계산은 TS 와 같은 f64 연산이고 정수 필드는 `JsNumber` 로 같은 JSON 모양이다.
- `LfsPointer.size` 는 `u64`(TS `Number`, 15자리 상한이라 정밀도 손실 없음). `parse_lfs_pointer` 의 입력은 `&str` 이다(`Buffer.toString` 이 아닌 호출자가 `from_utf8_lossy`).
- 오류는 `Result<_, &'static str>`(TS 의 `Error('invalid …')` 문구 그대로)이다. 비문자열 입력은 라우터 경계(PR 2~)에서 같은 문구로 거절한다 — 골든은 `5`·`null` 케이스를 harness 가 같은 문구로 바꿔 비교하므로 이 경계 자체는 시험하지 않는다.
- 새 크레이트는 없다. 이미 lock 에 있는 판만 쓴다.

**발견했지만 이 PR 에서 손대지 않은 것**
- `parsers/mod.rs` 의 `js_trim`(`char::is_whitespace` + BOM)은 U+0085 를 지운다 — JS `trim` 은 지우지 않는다(`fetch::js_trim`·`scope::is_js_space` 는 맞다). 새 코드는 모두 후자를 쓴다. 기존 파서가 이 차이에 걸리는지는 확인하지 않았다.
- `filterOverrides`: `config -z` 출력은 첫 개행에서 키와 값을 가르므로, 서브섹션 이름에 개행이 든 드라이버는 키가 잘려 드라이버로 보이지 않는다(덮어쓰지 않는다). git 설정 문법이 서브섹션의 개행을 허용하지 않는다고 알고 있어 실제 영향은 없을 것으로 보지만 실측하지 않았다.
  골든이 이 동작을 고정한다(TS 와 같다).

**시험**: `cargo test --features test-hooks --lib --test golden`(CI 목록 그대로 — 새 바이너리 없음). lib 241개 중 새 것 62개(`activity` 9·`code_browse` 10·`lfs` 18·`link_guard` 6·`refs_service` 8·`wip` 10·`parsers/refs` 1), golden 2개(`rust_parsers_match_ts_golden` 149개 비교·건너뜀 0, 대응표 검사).
vitest: `golden`(151)·`golden6a`(1)와 `codeBrowse`·`wip`·`wipFilter`·`lfsGitflow`·`activity`·`activityRepo`·`defaultBranch`·`submodule`·`hostileRepo`·`goldenChannels`·`goldenExec`(13개 파일 301개 통과).

**남은 위험**
- 골든이 TS 와 같다는 것은 입력 표(390 케이스·manifest 26개)의 범위만큼만 보장한다. 정렬(`compare_names`)은 `js_to_lowercase` 의 변환표와 UTF-16 비교에 기대므로 새 유니코드 경계가 발견되면 `tree-edge.dat` 에 더한다.
- `link_guard` 는 로컬 NTFS 에서만 실측했다(ReFS·네트워크 드라이브·서브스트 드라이브의 `canonicalize` 는 시험하지 않았다). 실패는 모두 거부 쪽(`invalid path`)으로 기운다.
- 대응표의 `<파일>.rs::<fn>` 은 fn 이 있는지만 본다(내용이 TS 와 같은 입력인지는 이 문서의 기준으로 사람이 본다).
- 링크 확인의 마지막 구성 요소 면제(`allow_leaf_link`)는 디렉터리 정션 자체(`j`, `j/`)에도 적용된다(TS 와 같다). 조작된 index 로 그 경로의 unstaged diff 가 링크 안을 읽는지는 PR 6 이 확인한다.
- 이 PR 에서 돌린 시험은 `--lib`·`golden` 외에 소스를 훑는 `fetch_guard`·`fetch_service`·`fetch_route`·`api_route`·`net`·`ssh_golden`·`ssh_route` 다. `exec`·`fetch`·`channels`·`ssh`·`log` 는 돌리지 않았다(WSL 배포판이 있으면 wsl.exe 를 부를 수 있고, 이 PR 은 그 코드를 건드리지 않았다).

### PR 2 — 코드 탐색기 4채널(앱에 연결한다)

`git:resolveRev`·`git:tree`·`git:blob`·`git:onRemote` 를 Rust 가 처리한다(local·WSL·SSH 세 실행기 — 라우팅·미러·취소·envelope 는 `git:fileDiff` 등과 같다). TS: `RepoService.ts` 의 `resolveRev`·`codeTree`·`codeBlob`·`codeOnRemote` + `codeBrowse.ts`.
소유 판정은 새로 만든 것이 없다 — 한 술어 `RepoMirror::resolve`(미러에 있고 local·wsl·(켜진) ssh)와 스위치 `ANTLER_RUST_GIT`·`ANTLER_RUST_SSH` 뿐이고, 사이드카는 이 채널에 소유 env 가 없다(미러가 사이드카로 푸는 저장소 — 없는 id·github·풀 없는 ssh — 와 스위치가 꺼진 라우터만 사이드카로 간다. 단위 시험 `코드_탐색기_채널의_소유는_다른_git_채널과_같은_한_술어다`).

| 채널 | 인자 | git(상수 + 검증된 값) | 캐시 | 취소 |
|---|---|---|---|---|
| `git:resolveRev` | `repoId, ref` | `rev-parse --verify --end-of-options <ref>^{commit}` | 없음(ref 는 움직인다) | 없음 |
| `git:tree` | `repoId, {rev, dir}, requestId?` | (dir 이 있으면 `cat-file -t <rev>:<dir>` 로 트리인지) → `ls-tree -z --end-of-options <rev>[:<dir>]` — **`-l` 은 쓰지 않는다**(크기 조회가 lazy fetch 를 부른다) | 키 `repoId\0rev 소문자\0정규화 dir`, 200개·8MB(항목 비용 200×(항목+1)), **missing 은 캐시하지 않는다** | 프로세스에 바로 닿는다(`queue.run(None, Some(token))`) |
| `git:blob` | `repoId, {rev, path}, requestId?` | `cat-file -t <rev>:<path>` → `cat-file -s` → (상한 이하일 때만) `cat-file blob` | 키 `repoId\0rev 소문자\0정규화 경로`, 64개·32MB(비용 text×2 + dataUrl + 1024, 길이는 UTF-16 단위), missing 제외 | 프로세스에 바로 닿는다 |
| `git:onRemote` | `repoId, rev` | `for-each-ref --count=1 --contains <rev 소문자> --format=%(refname) refs/remotes/` | **true 만** 기억(`repoId:rev 소문자`, 2000개가 되면 비운다) | 없음 |

- **파일 내용은 `cat-file blob` 으로만 읽는다**(`git show`·diff 계열·`--textconv`·`--filters` 없음 — textconv·필터·속성이 돌지 않는다). 소스 검사 시험이 `code_browse.rs` 에 그 인자가 없음을 강제하고,
  `악성_저장소의_textconv_와_filter_는_코드_탐색기가_실행하지_않는다`(`codeBrowse.test.ts` 의 같은 시험)가 저장소에 심은 `diff.*.textconv`·`filter.*.smudge|clean|process` 가 표식 파일을 만들지 않음을 본다.
- **검증(보안 경계)**: renderer 값이 argv 에 들어가는 길은 셋이고 모두 검증된 값이다 — ①`ref`: `HEAD`·16진수 4~64자·전체 ref 이름(`is_valid_scope_ref`, 리모트 전체 패턴 `/*` 제외), `--end-of-options` 뒤 ②`rev`: `CommitId`(40·64자 16진수, **대소문자 무시**, `-` 로 시작할 수 없다)
  ③`dir`·`path`: `normalize_dir`(NUL·`.`·`..`·빈 구성 요소 거부). ②③은 `<rev>:<경로>` **한 인자**라 첫 글자가 16진수여서 옵션이 될 수 없다. 검증 실패는 git 도 취소 레지스트리도 건드리지 않는다.
  검증 순서·문구는 TS 와 같다: `invalid ref`(resolveRev) · `invalid commit hash` → `invalid path`(rev 먼저, **`dir` 키가 없으면 최상위가 아니라 `invalid path`**) · onRemote 의 rev 는 `invalid commit hash`. 문자열·숫자·배열·`{}` 요청은 `invalid commit hash`(TS 도 `req.rev` 가 undefined 라 같다),
  요청이 `null`·인자 없음이면 TS 는 TypeError(code ERROR)라 Rust 는 `invalid request`(code ERROR)로 거절한다 — 문구만 다르다.
- **blob 판정**(TS 와 같은 순서): 종류가 `blob` 이 아니거나(디렉터리·서브모듈·없는 경로·로컬에 없는 객체) 크기를 못 읽으면 `missing`(size 0) → 크기가 상한(텍스트 2MB·이미지 10MB, 이미지는 확장자 대소문자 무관)을 넘으면 **내용을 읽지 않고** `tooLarge` →
  `cat-file blob`(최대 상한 + 64KB) → 이미지이고 크기 ≤ 1024 면 LFS 포인터 먼저(`lfs`) → 이미지(`image`, `data:<mime>;base64,…`) → 앞 8000바이트에 NUL 이 있으면 `binary` → LFS 포인터 → `text`. 텍스트는 깨진 바이트를 U+FFFD 로(`invalidUtf8: true` 는 엄격 디코드가 실패할 때만 키를 넣는다) 선두 BOM 을 한 번 벗긴다.
- **응답의 `rev` 는 보낸 표기 그대로**(대문자 유지)이고 소문자화는 캐시 키뿐이다. 그래서 캐시에 걸린 대문자 요청은 먼저 캐시한 요청의 표기를 돌려준다(TS 도 같다 — 골든이 이를 고정한다).
- **SSH(16MB 선읽기)**: 이 네 채널은 모두 `run`(stdout 을 전부 모으는 호출)이고 `stream` 이 아니다. 선읽기 상한(16MB)은 `stream` 에만 걸리므로 **해당 없다** — 읽는 양은 `run` 의 `max_buffer` 가 정한다(트리·ref·크기 조회는 기본 64MB, blob 은 상한 + 64KB = 최대 10MB + 64KB).
  `ls-tree` 출력이 64MB 를 넘는 디렉터리는 `GIT` 오류(`output exceeded …`, TS 와 같다). 이미지 10MB 는 base64 로 13.3MB 문자열이 되어 envelope 에 실린다(TS·사이드카 경로도 같다).
  호스트당 연결 하나를 나눠 쓴다(골든이 끝에 `connections()==1` 을 확인한다). 6a 계획 §6 의 선읽기 표는 stream 채널(search·activity·wip)만 해당한다.
- **WSL**: 새 종류의 자유 문자열(경로·ref)은 `<rev>:<경로>`·`<ref>^{commit}` 한 인자로 `wsl.exe` argv 에 실린다. 이 PC 에는 WSL 이 없어 **실행해 보지 않았다** — argv 단위 시험(`argv_는_ts_와_바이트까지_같다`)과 기존 `tests/exec.rs` 의 `wsl_통합`(한 인자 도착)이 덮는다.

**골든**: `tests/fixtures/repos/code.fi`(새 fixture — `scripts/make-repo-fixtures.mjs` 가 만든다. 텍스트·BOM·깨진 UTF-8·NUL(8000바이트 밖 포함)·이미지(대문자 `PNG`·`JPG`·svg)·LFS 포인터(`model.bin`·`logo.png`)·심볼릭 링크(120000)·서브모듈(160000)·`-x`·`--output=x.txt`·한글·비 BMP 이름·`dir.png/`·중첩 디렉터리·가벼운/주석 태그·가지 둘)와
`code.scenarios.json`(resolveRev 37·tree 50·blob 55·onRemote 22)을 TS(`goldenChannels.test.ts` → `RepoService`)와 Rust(`channels.rs` → `Router::route`)가 **한 서비스 인스턴스로 순서대로** 돌아 캐시 적중까지 본다. 결과는 `tests/golden/channels/{resolveRev,tree,blob,onRemote}.json`.
`branchy.fi`(원격 추적 ref)·`many.fi`(`f/` 에 5001개 → 5000개 + `truncated`)·`empty.fi` 도 쓴다. 시나리오 값에 `{"$short": [값, n]}` 이 더해졌다. 2MB·10MB 경계(`big.txt` 2MB+1 → `tooLarge`, `limit.txt` 정확히 2MB → text, `huge.png` 10MB+1 → `tooLarge`, `limit.png` 정확히 10MB → image)는
`setup` 으로 **시험 시점에** 고아 커밋을 만든다(`hash-object`·`mktree`·`commit-tree`, 작성자·시각 고정이라 해시가 결정적 — TS·Rust 가 같은 명령). 응답이 크면 text·dataUrl 은 200 글자를 넘을 때 `{len, head, tail}` 로, tree 항목은 20개를 넘으면 `{count, first, last}` 로 줄여 비교한다.
Rust 는 local 과 SSH(127.0.0.1 하네스) 둘 다 같은 골든과 같다. 악성 입력 시나리오: 옵션 주입(`--output`·`-x`·`-` 로 시작하는 40자 해시·`--end-of-options`), `..`·`.`·빈 구성 요소·NUL, 39·41·63·65자 해시·개행·앞 공백, 비문자열(숫자·null·배열·객체).

**그 밖의 시험**(`channels.rs`): `적대적_코드_탐색기_입력은_git_을_띄우지도_파일을_쓰지도_않는다`(저장소 경로가 없는 곳 — git 을 띄웠다면 GIT 오류가 난다. 거부된 요청은 `rust_handled` 에 세지 않는다),
`코드_탐색기_채널은_미러에_없거나_스위치가_꺼지거나_github_면_사이드카`, `등록_전에_온_cancel_은_코드_탐색기_요청을_git_없이_취소한다`, `진행_중_코드_탐색기_요청을_취소해도_멈추지_않는다`(10MB 이미지 포함),
`트리와_파일_내용은_캐시에서_나온다`(저장소를 지워도), `없던_커밋이_들어오면_missing_이_아니게_된다`(missing 비캐시), `원격_여부는_true_만_기억한다`(false → 원격 ref 생성 → true). `ssh_route.rs` 의 "잘못된 호스트면 연결 없이 ERROR"·"스위치가 꺼지면 사이드카" 표에 네 채널이 더해졌다.
`code_browse.rs` 단위 시험은 요청 검증 순서·캐시 키·argv·디코드·캐시 비용을 본다.

**대응표**(`test-map.json`): `codeBrowse` 24개 `it` 의 `N/A: PR 2` 를 Rust 대응(`channels.rs::…`)으로 바꿨다. `golden.rs` 의 `source_of`·`golden6a.test.ts` 의 `RUST_SOURCES` 에 `channels.rs` 를 더했다.
이 PR 뒤 `<파일>.rs::<fn>` 87개, `N/A: PR n` 69개, 사유 있는 N/A 43개(합 199, PR 2 직전 실측은 63·93·43 — 위 PR 1 절의 54·90·43 은 그 PR 시점의 숫자이고 이후 바뀌었다).

**TS 와 다른 점**
- 요청이 `null`·인자 없음일 때 문구가 `invalid request` 다(TS 는 TypeError 문구). code 는 같은 ERROR 이고 골든은 이 입력의 문구를 비교하지 않는다.
- `image_mime` 은 객체 프로토타입 이름(`a.constructor` 등)을 확장자로 치지 않는다(PR 1 의 알려진 차이 그대로 — TS 쪽 수정은 따로).
- 같은 키의 동시 요청을 합치지 않는다 — TS 도 tree·blob 은 합치지 않는다(commit·compare·diff 와 달리 `cached_shared` 를 쓰지 않고 조회 → 큐 → 조건부 저장이다).
- `git:onRemote` 의 GitHub 원격 전용 분기(항상 true)는 옮기지 않았다 — github 소스는 미러가 사이드카로 보낸다.

**사람 확인(자동화 불가·실환경)**
- `pnpm tauri:build:cdp` 뒤 `ANTLER_ROUTE_LOG=1 ANTLER_DRIVE_DEBUG=1 node scripts/drive.mjs <저장소> --tauri …` 로 코드 탭을 열어 네 채널이 `-> rust` 인지 본다. `ANTLER_RUST_GIT=0`(사이드카)·Electron 과 같은 화면인지 비교한다: 트리 정렬(대소문자·한글), 이미지·LFS·큰 파일(`tooLarge`)·바이너리·BOM·깨진 UTF-8 표시, README 이미지(`git:blob` 을 쓴다), 줄 링크의 "push 안 된 커밋" 경고(`git:onRemote`).
- WSL 저장소에서 같은 코드 탭 동작(이 PC 에서는 실행할 수 없었다), 실제 sshd 의 SSH 저장소에서 큰 이미지(10MB)·5000개 넘는 디렉터리가 호스트당 연결 하나로 열리는지(원격 `ss`/`who`).
- 큰 저장소의 트리·파일 첫 응답 시간(Electron·사이드카 대비). 측정은 하지 않았다.

**남은 위험**
- 골든이 TS 와 같다는 것은 시나리오(164개)의 범위만큼만 보장한다. `ls-tree` 의 "객체 없음" 판정은 git 의 stderr 문구(`not a valid object name`·`bad object`·…)에 기대므로 다른 로캘·git 판에서 문구가 다르면 `missing` 이 `GIT` 오류로 보일 수 있다(TS 도 같은 정규식이다).
- 10MB 이미지의 base64(13.3MB)가 Tauri invoke 의 JSON 응답으로 가는 비용을 측정하지 않았다(계획 Q3 — 사이드카 경로도 같은 크기라 회귀는 아니다).
- `RUST_CHANNELS` 가 11개가 되면서 사이드카는 이 채널을 local·wsl·ssh 저장소에서 받지 않는다. 사이드카의 `gh:repo` 등이 같은 저장소의 git 을 계속 돌리는 문제(SSH 이중 연결)는 PR 8 결정에 달려 있다.

### PR 3a — 공유 상태(앱 노출 변화 없음, 내부만)

TS `RepoService` 가 쥔 상태 둘을 Rust 에 둔다. 소비자(`refsDetails`·`activity`·`wipStatus`·감시)는 아직 없어 **보이는 동작은 그대로다** — 알림이 Rust 상태에도 먼저 반영될 뿐이다.

| 파일 | 내용 |
|---|---|
| `git/signature.rs`(새) | `refs_signature`(fetch 서비스에서 꺼냈다 — 비공개 → `pub`, argv 3개·`join!`·`for-each-ref` 종료 코드만 보는 조건은 바이트까지 그대로)·`refs_signature_digest`(같은 조회의 SHA-256 요약 `Signature`). fetch 서비스는 호출만 바뀌었다 |
| `git/repo_state.rs`(새) | `RepoState`: 첫 페이지 문(`arm`·`after_first_page`·`open`)·시그니처 저장소(`known_signature`·`invalidate_signature`·`compute_signature`·`shared_signature`)·`note(NoteOp)`·`dispose` |
| `router.rs` | `Router` 가 `RepoState` 를 쥔다(`repo_state()`). 알림 길 `note_sink()`(팬아웃: **상태에 먼저, 그다음 앱이 단 싱크(사이드카)로**)를 `Router::new` 에서 로그 서비스에 달고(`with_log_timing` 뒤에도), `repos:watch` 가로채기에서 `arm`, `shutdown` 에서 `dispose` |
| `fetch_host.rs` | `AppFetchHost::note` 가 `Router::note_sink()` 를 거친다(사이드카에 직접 보내지 않는다). `FetchHost` 트레이트는 그대로라 가짜 호스트를 쓰는 시험은 바뀌지 않았다 |

**TS 와 같은 의미(`RepoService.ts:586-687`)**
- **문**: `arm` 은 항상 시그니처를 **먼저** 무효화하고, 문이 이미 있으면 유예를 다시 걸지 않는다(1.5초). 첫 로그 페이지(`logFirstPage`) 또는 유예가 지나면 열린다. 문이 없는 저장소는 기다리지 않는다.
- **시그니처**: 계산은 시작할 때(`compute_signature` 를 **부르는 순간**) 순번을 받고, 성공했을 때만 `seq > applied` 이면 기억한다. 무효화(`logStale`·`fetchChanged`·재 `arm`)는 기억·진행 중 계산을 버리고 순번을 올려 그 전에 시작한 계산이 다시 기록하지 못하게 한다.
  토큰을 건 계산(감시 확인·fetch 전후)은 합치지 않고 따로 하며, 토큰 없는 호출(`shared_signature`)은 진행 중인 계산에 합류한다(제한 20초). `dispose`(앱 종료)는 기다리는 문을 모두 열고 기억을 비운다(TS `dispose`).
- **알림**: `logFirstPage` 는 문을 열고 `logStale`·`fetchChanged` 는 시그니처를 버린다. `fetchChanged` 는 `repo:changed` 보다 먼저다(순서는 그대로 — 서비스가 `note` 를 먼저 부른다). 모르는 저장소를 무시하는 것(TS `noteLog` 의 `config.repo`)은 **미러가 Rust 로 푸는 저장소만 반영**하는 것으로 옮겼다 — github·풀 없는 ssh·없는 id 는 상태를 만지지 않고 사이드카에만 간다.
- `repos:watch`: TS 의 `if (repoId && config.repo(repoId)) armFirstPage` 와 같다. 스위치(`ANTLER_RUST_GIT=0`)가 꺼져 있으면 하지 않는다. 응답은 사이드카가 하고 renderer 는 기다리지 않는다.
- 저장소가 제거돼도 TS 는 문·기억을 지우지 않는다(`dispose` 에서만) — 그대로 따랐다(저장소 수에 비례한 작은 표).

**TS 와 다른 점**
- **타이머가 없다**: TS 는 `setTimeout` 으로 문을 연다. Rust 는 문에 **마감 시각**을 두고 대기자가 마감까지만 기다리며(`timeout_at`), 마감이 지난 문은 다음에 만지는 순간 치운다. 관찰되는 동작은 같고, 재활성화한 새 문을 낡은 타이머가 일찍 열 수 없다(시험으로 고정).
- **기억은 요약이다**: 저장하는 값은 원문이 아니라 SHA-256 이다(ref 수에 비례해 커지지 않는다). TS 는 SHA-1 요약이다 — 같은지만 비교하므로 의미는 같다. **원시 바이트를 해시한다**(TS 도 Buffer 를 해시한다). fetch 서비스의 비교 값(원문 문자열)은 그대로다.
- 공유 계산은 spawn 한 태스크이고 제한 시간이 지나면 계산에 취소를 보낸다(TS 는 `AbortSignal.timeout`). 대기자가 모두 떠나도 끝까지 간다.
- 패닉한 계산은 `Failed` 로 돌려주고 진행 중 표에서 빠진다(TS 에는 해당 없음).

**소비 금지**: `known_signature` 는 **감시(PR 7b)가 갱신하기 전까지 소비하지 않는다** — 감시와 `activity` 가 다른 프로세스에 있으면 기억값이 낡는다(계획 §2). PR 4 의 `activity` 는 기억값을 쓰지 않고 `shared_signature` 로 매번 얻는다.
이 PR 은 사이드카가 보내는 `repo:changed`(감시 이벤트)는 **연결하지 않았다** — 감시는 PR 7 의 몫이고, 지금 Rust 상태를 갱신할 감시자가 없다.

**대응표**(`test-map.json`): `repoService` 의 첫 페이지 문 `it` 2개(문 열림·`logFirstPage`)만 `repo_state.rs::…` 로 연결했다(`golden.rs` 의 `source_of`·`golden6a.test.ts` 의 `RUST_SOURCES` 에 `repo_state.rs` 를 더했다).
TS 가 `activity` 결과로 관찰하던 4개(`logStale`·`fetchChanged`·시그니처 재사용·`StaleLogError`)는 `N/A: PR 4` 로 되돌렸다 — 이 PR 의 상태 수준 시험(기억이 비었는가·합류했는가·낡은 계산이 덮지 못하는가)은 `activity` 결과를 보증하지 않으므로, **PR 4 가 `activity` 수준 시험으로 다시 매핑해야 한다**(PR 4 에서 했다 — 아래).
이 PR 뒤 `<파일>.rs::<fn>` 89개, `N/A: PR n` 67개, 사유 있는 N/A 43개(합 199).

**시험**: `repo_state.rs` 단위 17개(문: 활성화·유예·재 arm 이 유예를 안 미룸·낡은 유예가 새 문을 못 엶·저장소별·dispose / 시그니처: 실패 비기록·늦게 시작한 계산 우선·무효화 전 계산 비기록·순번 시점·note·합류·합류 끊김·기다림 취소·제한 시간·패닉·동시 16개 저장소, 멈춘 시계(`start_paused`)와 손으로 푸는 future),
`signature.rs` 1개, `router.rs` 6개(`repos:watch` arm 대상·재 arm 이 기억을 버림·**팬아웃 순서**(싱크 안에서 상태가 이미 반영됨)·싱크 없이도 반영/나중에 단 싱크도 받음·`with_log_timing` 뒤에도 로그 알림이 상태를 엶·종료가 문을 풂).
fetch 서비스는 `fetch_service`·`fetch_route`·`fetch` 시험으로 동작 불변을 확인했다.

**남은 위험**
- `repos:watch` 와 `git:log`(첫 페이지)의 도착 순서가 뒤집히면 문이 열린 뒤 다시 닫혀 유예(1.5초)만큼 기다린다 — 사이드카와 같은 한계다.
- `RepoState` 의 표는 저장소 제거 시 줄지 않는다(TS 와 같다).
- 공유 계산이 아직 쓰이지 않아 실제 git 과 돌려 본 적은 없다(`refs_signature_digest` 는 PR 3b 이후 소비자가 부른다).

### PR 3b — `git:refs`·`git:refsDetails`(앱에 연결한다)

`git:refs(repoId)`·`git:refsDetails(repoId)` 를 Rust 가 처리한다(local·WSL·SSH 세 실행기 — 라우팅·미러·envelope 는 코드 탐색기 채널과 같다). TS: `RepoService.refs`·`refsDetails` + `commands.ts` 의 `refs`·`refsDetails`·`submodules`·`head`.
소유 판정은 새로 만든 것이 없다 — 한 술어 `RepoMirror::resolve` 와 스위치 `ANTLER_RUST_GIT`·`ANTLER_RUST_SSH` 뿐이다(github 소스는 미러가 사이드카로 보낸다 — TS 의 "원격 전용 저장소는 서브모듈을 읽지 않고 작업 트리를 비운다" 분기는 옮기지 않았다).
인자는 저장소 id 뿐이라 검증할 renderer 값이 없다(저장소 id 가 문자열이 아니면 사이드카가 TS 와 같은 오류를 만든다).

| 파일 | 내용 |
|---|---|
| `git/refs_service.rs` | `refs_snapshot(lite)`·`refs_details`·`submodules`·`head_info` — 서브모듈 경로 방어(아래)가 여기 있다 |
| `git/parsers/refs.rs` | `refs_args`·`refs_args_lite`·`REFS_TRACK_ARGS`(TS `REFS_ARGS`·`REFS_ARGS_LITE`·`REFS_TRACK_ARGS` 와 바이트까지 같은 argv — 단위 시험 `argv_는_ts_와_바이트까지_같다`) |
| `git/parsers/submodule.rs` | `SubmoduleStatus::Uninitialized` 추가(상태 판정이 아니라 확인이 정한다) |
| `router.rs` | `RUST_CHANNELS` 에 두 채널, `git_refs_channel` |

| 채널 | git(상수만) | 큐 키 | 첫 페이지 대기 |
|---|---|---|---|
| `git:refs` | 동시 7개: `for-each-ref`(lite — ahead/behind 필드를 비운 `REFS_ARGS_LITE`)·`remote`·`worktree list --porcelain -z`·`stash list -z`·`symbolic-ref --quiet --short HEAD`·`rev-parse --verify --quiet HEAD`·`symbolic-ref --quiet refs/remotes/origin/HEAD`. 서브모듈은 비운다(lite) | `refs:<repoId>` | 없다(TS 도 첫 화면용이라 기다리지 않는다) |
| `git:refsDetails` | 동시 2개: `REFS_TRACK_ARGS`·서브모듈 확인 | `refsDetails:<repoId>` | **있다** — `RepoState::after_first_page` 를 큐에 들어가기 **전에** 기다린다(문이 없으면 바로, `repos:watch` 가 닫고 `logFirstPage` 알림·1.5초 유예가 연다) |

둘 다 캐시가 없고 취소 신호도 없다(TS 도 같다). 같은 키의 동시 요청은 큐가 합친다. 오류는 `for-each-ref` 가 0 이 아닌 코드로 끝났을 때만 GIT 이고 나머지 조회는 실패해도 빈 값이다(`okOrEmpty`).
서브모듈 조회가 던져도(실행 실패) 스냅샷을 막지 않고 빈 목록이다.

**서브모듈 확인(보안)** — 인덱스의 gitlink 경로는 **저장소가 정하는 값**이고 `git -C <저장소>/<경로>` 에 들어간다. TS(#113)와 같게 둘을 모두 통과할 때만 git 을 돌리고 아니면 `unknown` 이다:
1. `is_safe_submodule_path`: 절대 경로·드라이브 문자·빈/`.`/`..` 구성 요소(구분자 `/`·`\`) 거부 — 문자열 검사.
2. **로컬 실행기만** `link_guard::assert_inside_root(base, base/경로)`: `canonicalize`(Windows 에서 8.3 이름이 펴진다) 결과가 저장소 실제 경로 안이고 `.git` 구성 요소가 없어야 한다 — 정션·symlink 로 밖·`.git` 안을 가리키는 경로를 거른다. 없는 경로는 통과한다(git 이 스스로 실패하고 `uninitialized` 다). WSL·SSH 는 Rust 가 그 파일 시스템을 볼 수 없어 하지 않는다(TS 도 같다).
   `link_guard` 에는 git 의 cwd·`-C` 와 같은 **비 verbatim 문자열**(`base`·`base/경로`, `base` 는 끝 `/`·`\` 를 지운 저장소 경로)을 넘긴다 — `canonicalize` 는 `link_guard` 안에서만 한다(`\\?\` 접두 경로를 넘기지 않는다). 파일 시스템 호출이라 `spawn_blocking` 이다.
- 확인 호출은 `rev-parse --absolute-git-dir HEAD` 하나이고 **같은 실행기**를 거친다 — `GIT_GLOBAL_ARGS`·`GIT_ENV`(fsmonitor·filter·hook·sshCommand 등을 끄는 하드닝)가 서브모듈에도 그대로 걸린다. `git submodule` 은 쓰지 않는다(하위 저장소 설정까지 건드린다 — 소스 검사 시험이 강제한다).
- 상한: 목록 200개(`SUBMODULE_MAX`), 앞 50개만 확인(`SUBMODULE_PROBE_MAX`, 나머지 `unknown`), 동시 8개. 상위 git-dir 을 못 읽으면 확인하지 않고 전부 `unknown`.
- 상태: git-dir 이 상위와 같으면(비어 있는 서브모듈 디렉터리에서 git 이 상위로 올라간다) `uninitialized`, 디렉터리 자체가 없으면(`cannot change to`·`No such file`·`not a directory`) `uninitialized`, 아니면 HEAD 와 기록 해시를 비교해 `clean`·`modified`(HEAD 를 못 읽으면 `unknown`).

**골든**: `tests/fixtures/repos/refs.scenarios.json`(11개 시나리오 — basic·branchy·empty·code, origin/HEAD·업스트림·gone·`/` 든 리모트 이름, detached·커밋 없는 HEAD, 스태시 둘, 작업 트리(잠금·detached·한글 경로), 서브모듈(clean·modified·uninitialized 둘·한글·공백 경로·`.gitmodules` 이름), 정렬 알려진 차이)를
TS(`goldenChannels.test.ts` → `RepoService.refs·refsDetails`)와 Rust(`channels.rs` → `Router::route`)가 시나리오마다 새 저장소에서 단계(`git`·`mkdir`·`refs`·`refsDetails`)를 순서대로 돌아 `tests/golden/channels/{refs,refsDetails}.json` 과 비교한다. local 과 SSH(127.0.0.1 하네스, 끝에 `connections()==1`)에서 같다.
git 단계는 작성자·시각을 고정해(`REFS_STEP_ENV`) 커밋·스태시 해시와 시각이 결정적이고, 작업 트리 경로는 시험 루트를 `<ROOT>` 로 바꿔 비교한다. 인자의 `@rev:<경로>::<식>` 은 하위 저장소의 rev-parse 다.

**정렬(결정 D1)**: 브랜치·태그·리모트 정렬은 `natural_compare` 근사를 유지했다. 사이드바가 처음 사용자에게 보이는 지점이라 **TS 와 다른 경우를 골든에 기록했다** — `알려진 차이` 시나리오(`refs.scenarios.json` 의 `knownDiff`): ICU 는 `é` 를 `e` 와 `f` 사이에 놓지만(`eagle, éclair, feature, fzzz, main, zebra`)
근사는 코드 포인트 순이라 `z` 뒤다(`eagle, feature, fzzz, main, zebra, éclair`). Rust 하네스는 이 시나리오에서 local 의 순서가 선언한 `rustLocal` 이고 **TS 골든의 순서와 달라야** 하며(같아지면 ICU 와 같게 옮긴 것이니 `knownDiff` 를 지우라고 실패한다) 이름순으로 다시 놓은 나머지가 같음을 본다.
같은 계열의 차이: 라틴이 아닌 문자 체계끼리(한자 ↔ 한글: ICU 는 한글이 한자보다 앞, 근사는 한자가 앞)·악센트·전각 문자, 그리고 **실행 환경 로케일**(TS 는 `localeCompare(undefined)` 라 ko-KR 에서는 한글이 라틴 문자보다 앞이다 — 근사는 라틴이 앞이다). 로케일 의존이라 후자는 골든에 넣지 않았다. 완전한 해결은 `icu_collator` 크레이트(결정 사항)다.

**그 밖의 시험**: `channels.rs` — `ref_채널은_미러에_없거나_스위치가_꺼지거나_github_면_사이드카`, `refs_details_는_첫_페이지_문이_열릴_때까지_기다리고_refs_는_기다리지_않는다`(`repos:watch` 가 문을 닫고 `NoteOp::FirstPage` 가 연다),
`non_lite_스냅샷은_서브모듈을_담고_lite_는_비운다`, 서브모듈 방어 4개(`악성_서브모듈의_설정과_훅은_상태_조회가_실행하지_않는다`(대조군: 하드닝 없이 `status` 를 돌리면 fsmonitor 가 실제로 실행됨을 확인), `저장소_밖을_가리키는_서브모듈_경로는_git_을_돌리지_않고_unknown_이다`(인덱스 파일(v2)을 직접 써서 `../outside` 등을 넣는다),
`정션인_서브모듈_경로는_git_을_돌리지_않고_unknown_이다`(Windows 만 — `mklink /J`, 정션을 먼저 `rmdir` 한다), `서브모듈은_200개까지_담고_앞_50개만_확인한다`). 방어를 끄면(문자열 검사·실제 경로 검사를 각각 끄는 변형) 해당 시험이 실패함을 확인했다.
`router.rs` 단위 `ref_채널의_소유는_다른_git_채널과_같은_한_술어다`, `ssh_route.rs` 의 "잘못된 호스트면 연결 없이 ERROR"·"스위치가 꺼지면 사이드카" 표에 두 채널이 더해졌다. `refs_service.rs` 단위: argv 가 TS 와 바이트까지 같음, 소스에 `git submodule` 이 없음.

**대응표**(`test-map.json`): `N/A: PR 3b` 10개를 Rust 대응(`channels.rs::…`)으로 바꿨다 — `search`(refs 2개)·`submodule`(실제 저장소 5·악성 1·경로 검증 2). 이 PR 뒤 `<파일>.rs::<fn>` 99개, `N/A: PR n` 57개, 사유 있는 N/A 43개(합 199).

**TS 와 다른 점**
- 정렬: 위 D1. 
- 같은 PR 의 여러 조회 중 둘 이상이 실패하면 TS 는 가장 먼저 거절된 것의 오류를 내고(`Promise.all`) Rust 는 모두 끝난 뒤 `for-each-ref` → `remote` → `worktree` → `stash` → HEAD → origin/HEAD 순으로 처음 실패한 것을 낸다. 한 조회만 실패하는 보통의 경우는 같다.
- GitHub 원격 전용 분기는 옮기지 않았다(미러가 사이드카로 보낸다).
- 서브모듈 실제 경로 검사는 `spawn_blocking` 이다(TS 는 `await`).

**사람 확인(자동화 불가·실환경)**
- `pnpm tauri:build:cdp` 뒤 `ANTLER_ROUTE_LOG=1 ANTLER_DRIVE_DEBUG=1 node scripts/drive.mjs <저장소> --tauri …` 로 두 채널이 `-> rust` 인지 보고, `ANTLER_RUST_GIT=0`(사이드카)·Electron 과 사이드바를 비교한다: 브랜치·태그·리모트 정렬(**대소문자·숫자·한글·악센트 이름**이 있는 저장소), 기본 브랜치 줄기, 스태시·작업 트리, 서브모듈 상태 표시(서브모듈이 있는 실제 저장소 — 초기화·미초기화·HEAD 가 다른 것), ahead/behind(업스트림이 있는 저장소)가 첫 로그 페이지 뒤 채워지는지.
- 서브모듈 수가 많은 저장소(50개 초과)·느린 디스크(네트워크 드라이브)에서 `refsDetails` 지연. 정션이 든 저장소의 실제 동작은 자동 시험(Windows)이 덮는다.
- WSL 저장소의 사이드바(이 PC 에는 WSL 이 없어 **실행해 보지 않았다** — 서브모듈 실제 경로 검사는 하지 않는 길이고 argv 는 상수뿐이다), 실제 sshd 의 SSH 저장소에서 사이드바가 호스트당 연결 하나로 열리는지.

**남은 위험**
- 정렬 근사 차이가 사이드바에 보인다(위). 사용자 보고가 있으면 `icu_collator` 로 옮긴다.
- 서브모듈 실제 경로 검사와 git 의 읽기 사이의 경쟁(TOCTOU)은 막지 못한다(`link_guard` 의 알려진 한계, TS 도 같다).
- `refsDetails` 의 첫 페이지 대기는 `repos:watch` 와 `git:log` 의 도착 순서에 기댄다(PR 3a 의 남은 위험과 같다).
- `git:refs` 의 7개 동시 조회가 SSH 에서는 연결당 채널 6개 세마포어에 걸려 일곱 번째가 기다린다(TS 도 같다).

### PR 4 — `git:search`·`git:activity`(앱에 연결한다)

`git:search(repoId, query, scope?, requestId?)`·`git:activity(repoId, scope?, requestId?)` 를 Rust 가 처리한다(local·WSL·SSH 세 실행기 — 라우팅·미러·취소·envelope 는 앞 채널들과 같다). TS: `RepoService.search`·`activity` + `commands.ts` 의 `search`·`reachable`·`mergeInGraphOrder` + `activity.ts` 의 `commitActivity`.
소유 판정은 새로 만든 것이 없다 — 한 술어 `RepoMirror::resolve` 와 스위치 `ANTLER_RUST_GIT`·`ANTLER_RUST_SSH` 뿐이다(github 소스는 미러가 사이드카로 보낸다. 사이드카에는 이 채널의 소유 env 가 없다).

| 파일 | 내용 |
|---|---|
| `git/search.rs`(새) | `SearchRequest`(검증)·argv 빌더 5개·`search`(세 갈래 + 도달 가능성 + 그래프 순서 병합) |
| `git/activity.rs` | `ActivityRequest`(범위 검증·키)·`commit_activity`(스트림 집계)·`ActivityCache`(8개, 삽입 순서) — 순수 로직(`bucketize`·`TimeStreamParser`)은 PR 1 |
| `git/signature.rs`·`git/repo_state.rs` | 시그니처 계산 실패를 TS 처럼 GIT 으로(아래), `SigError::Git`·`From<SigError> for ExecError` |
| `router.rs` | `RUST_CHANNELS` 에 두 채널, `git_history_channel`·`history_job`·`activity_job`·`queued_signature`, 합치기 큐 `joins` |

| 채널 | git(상수 + 검증된 값) | 큐 | 취소 |
|---|---|---|---|
| `git:search` | 동시 3갈래: `log --format=%H -z --date-order -i -F --max-count=10001 --grep=<q> <범위> --`·같은 모양의 `--author=<q>`·(해시 갈래) `rev-parse --verify --quiet <q>^{commit}`. 이어서 `rev-list -n1 <해시> --not <범위>`(해시 갈래가 풀렸을 때)·`log --format=%H -z --date-order <범위> --`(스트림 — 두 갈래가 모두 비어 있지 않을 때만) | **큐 슬롯 하나 안에서** 전부(합치지 않는다) | 모든 git 호출에 토큰이 걸린다(`queue.run(None, Some(token))` + `RunOpts::with_cancel`) |
| `git:activity` | `log --format=%at <범위> --`(스트림). 그 앞에 시그니처(`for-each-ref`·`symbolic-ref`·`rev-parse` 3개) | 합치기는 슬롯 밖, 시그니처·스트림이 각각 슬롯 하나 | 합친 요청은 기다리는 쪽이 모두 떠나야 git 이 죽는다(각 호출자의 토큰은 그 호출자의 기다림만 끊는다) |

**search 검증(보안 — 질의는 자유 문자열이라 git 인자에 들어간다)** — 순서는 TS 와 같다: 저장소(미러) → 범위(`parse_scope` + `scope_args`: `invalid scope`·`invalid solo`·`invalid hide`) → 질의 → 취소 등록. 검증에 걸린 요청은 git 도 취소 레지스트리도 건드리지 않고 `rust_handled` 에도 세지 않는다.
- 질의는 **옵션 접두가 붙은 한 인자**(`--grep=<q>`·`--author=<q>`)로만 들어가 `-` 로 시작해도 옵션이 될 수 없고, `-F`(고정 문자열)·`-i` 가 상수다. 해시 갈래는 질의(앞뒤 JS 공백을 지운 값)가 16진수 4~**40**자(대소문자 무시 — `HexHash` 의 4~64 와 다르다)일 때만 열리고, git 이 돌려준 해시도 `HexHash` 로 다시 검사한 뒤에야 `rev-list` 의 인자가 된다(아니면 쓰지 않는다).
- 문자열이 아닌 질의는 `invalid query`(code ERROR — TS 는 `.trim` TypeError 라 문구만 다르다). **NUL 이 든 질의는 git 을 띄우기 전에 `invalid query` 로 거부한다**(TS 는 Node 의 spawn 인자 검사가 막는다 — Rust 는 WSL argv·SSH 인용으로 가기 전에 막아야 한다). 공백뿐인 질의는 git 없이 빈 결과다. U+0085 는 JS `trim` 처럼 지우지 않는다(`fetch::js_trim`).
- **질의 길이 상한은 없다** — TS 의 `search` 에도 없고(`commands.ts`·`handlers.ts`·renderer `searchStore.ts`·`SearchBar.tsx` 를 읽어 확인했다) 새로 만들지 않았다. TS 에 **있는** 상한은 범위(scope)의 것이고 `scope.rs` 를 그대로 쓰므로 같다: 항목 길이 1024(UTF-16 단위)·목록 항목 200개·합 200개. 아주 긴 질의는 운영체제 명령줄 한도를 넘어 local 은 spawn 오류(ERROR)이고 ssh 는 원격 셸이 갈라서, 공유 골든에는 넣지 않았다.
- `GIT_LITERAL_PATHSPECS`·`GIT_GLOBAL_ARGS`·`GIT_ENV`(fsmonitor·lazy fetch·replace 끔)는 실행기가 모든 git 에 건다. 이 채널은 pathspec 을 받지 않는다(`--` 뒤에 아무것도 없다).

**결과 크기·스트리밍(TS 와 맞춘 것)** — 각 갈래는 `--max-count=10001` 이고 합집합이 1만 개를 넘으면 앞 1만 개 + `truncated: true` 다. 갈래의 stdout 은 `run` 이라 기본 `max_buffer`(64MB)가 상한이다(1만 해시 ≈ 0.4MB).
그래프 순서 병합은 `stream` 이고 **합집합을 다 찾는 순간 스트림을 버려 git 을 끊는다**(TS 가 `break` 로 제너레이터를 닫는 것과 같다). 레코드 하나의 상한은 로그 커서와 같은 16MB(`MAX_STREAM_RECORD`)다.
**SSH 16MB 선읽기**: 이 채널의 소비는 해시 집합 삽입뿐이라 지연으로 상한을 넘는 일은 사실상 없다. 넘으면(원격은 이미 죽었다) `SSH 선읽기 상한(16MB)을 넘어 검색을 중단했습니다`(code ERROR)로 끝나고 다시 검색하면 된다 — TS(ssh2)는 backpressure 라 오류가 없다(2b 의 알려진 차이, 사용자 결정 D3). Local·WSL 은 파이프 backpressure 라 상한이 없다.

**activity 의 순서와 시그니처(핵심 결정)** — TS 와 같은 순서다: 같은 키(`activity\0<repoId>\0<범위 키>`)의 진행 중 요청에 합류 → 첫 페이지 문(`RepoState::after_first_page`) → 시그니처 → 캐시(`(시그니처, 결과)` 8개, **삽입 순서** — 적중해도 순서가 안 바뀌고 같은 키를 다시 넣으면 맨 뒤) → 큐 슬롯 안에서 스트림. 빈 저장소는 빈 결과다.
- **기억한 시그니처(`known_signature`)를 소비하지 않는다.** TS 는 감시가 갱신하는 `knownSigs` 를 재사용하지만(`RepoService.ts:823`), 이 PR 에는 Rust 쪽 감시가 없어 기억값을 갱신할 주체가 없다 — 소비하면 낡은 값이 캐시 적중을 만든다. 그래서 매번 `shared_signature` 로 새로 계산한다(진행 중인 계산에 합류, 제한 20초).
  `shared_signature` 가 기록하는 것(`finish`)은 그대로라 기억값은 계속 갱신되고, 읽는 곳이 없을 뿐이다. 7b 가 감시로 기억값을 갱신하기 시작하면 이 한 줄(`activity_job` 의 `shared_signature` 호출)을 `known_signature` 우선으로 바꾼다. 대가: activity 호출마다 git 3번이 는다(캐시 적중이어도).
- **PR 3a 계약**: 공유 계산은 spawn 된 태스크라 future 를 버려도 멈추지 않으므로, 넘긴 계산(`queued_signature`)은 받은 토큰을 **큐 슬롯 대기**(`queue.run(None, Some(token))`)와 **git 실행**(`refs_signature_digest(.., &token)`) 모두에 건다. 제한 시간의 취소가 닿는 길은 이 토큰뿐이다. 큐의 결과 타입이 JSON 이라 시그니처는 셀로 꺼낸다(큐 밖에서 git 을 돌리지 않는다).
- **합치기는 슬롯과 분리한다.** 합치기를 소스의 큐 키(`queue.run(Some(key))`)로 하면 합친 작업 안에서 시그니처가 같은 큐의 슬롯을 또 요구한다 — 한도(local 6·wsl 4·ssh 4)만큼 겹치면 서로 슬롯을 쥔 채 기다려 20초 제한까지 막힌다.
  그래서 합치기 전용 큐(`Router::joins`, 슬롯 2^20 개라 막지 않는다 — 합치기·"기다리는 쪽이 모두 떠나면 버려진다"는 `ProcessQueue` 의 키 요청 구현을 그대로 쓴다)에서 하고, 실제 git 은 소스의 큐에서 슬롯 하나씩만 쥔다(`활동_요청은_합쳐지고_큐_한도보다_많아도_서로_막지_않는다` — 키가 다른 요청 16개를 local·ssh 로 돌린다).
- 오류: 시그니처 계산 실패는 TS 처럼 **GIT**(`for-each-ref` 가 0 이 아닌 코드로 끝남 — `signature.rs` 가 PR 3a 에서는 `Other`(ERROR)였다. fetch 는 `.ok()` 라 종류를 보지 않아 영향이 없다). 제한 시간·취소는 ABORTED. SSH 선읽기 초과는 ERROR(`활동 집계를 중단했습니다`) — 미니맵만 비고 다시 열면 된다.

**골든**: `tests/fixtures/repos/search.fi`(새 fixture — 작성자 셋(홍길동·Alice·Bob)·`--force` 메시지·정규식 문자가 든 메시지·한글·side 가지·`refs/stash` 만 가리키는 닿지 않는 커밋, 시각이 모두 달라 `--date-order` 가 결정적이다)와
`search.scenarios.json`(시나리오 15개 — 검색 123단계·활동 35단계)를 TS(`goldenChannels.test.ts` → `RepoService.search·activity`)와 Rust(`channels.rs` → `Router::route`)가 시나리오마다 새 저장소에서 단계(`git`·`search`·`activity`)를 순서대로 돌아 `tests/golden/channels/{search,activity}.json` 과 비교한다(인자도 같은지 본다). local 과 SSH(127.0.0.1 하네스, 끝에 `connections()==1`)에서 같다.
악성 입력: 옵션 모양(`--output=x`·`-1`·`--end-of-options`·`--`·`-c alias.x=!echo`·`--author=alice`), 정규식 문자(`x.*`·`a|b`·`(z)`·`[w]`·`.*`), 셸·인용 문자(`$(…)`·`` `…` ``·`;`·`'`·`"`·개행·이모지·NBSP·U+0085), NUL·비문자열·누락 질의, 잘못된 범위(`null`·문자열·숫자·`-x`·해시 hide·201개 항목·`includeHead` 비불리언·`..` 든 ref)·`requestId` 자리 이동(범위 생략과 `null` 은 bridge 표식(`UNDEFINED_ARG`)으로 구분한다).
스크립트 `make-repo-fixtures.mjs` 의 `Stream.commit` 에 작성자 옵션 `by`(기본 `WHO`)를 더했다 — 기존 `.fi` 는 바이트까지 그대로다(`git diff --stat` 로 확인). 기존 `files.fi` 가 넘기는 `who: ALICE` 는 예전부터 무시되는 값이라 건드리지 않았다.
TS 하네스는 `git` 단계 뒤에 `svc.noteLog('logStale', id)` 를 부른다 — 앱에서는 감시가 ref 변경을 알아 기억한 시그니처를 새로 계산하는데 하네스에는 감시가 없어서, 같은 효과로 버린다. Rust 는 이 호출 없이 같은 골든을 낸다(위 "소비하지 않는다").
결과에 경로가 들어가지 않아 refs 시험의 시험 루트 스크럽(`root_long`)은 필요 없다 — 다만 러너 TEMP 가 8.3 짧은 이름이어도 같음을 이 PC 에서 짧은 이름 TMP/TEMP 로 한 번 돌려 확인했다(아래 "검증").

**그 밖의 시험**(`channels.rs` 13개 + `search.rs`·`activity.rs`·`router.rs` 단위): `적대적_검색_활동_입력은_git_을_띄우지_않는다`(경로 없는 저장소 — 검증 순서·문구·`rust_handled` 0), `검증을_통과한_요청이_git_실패로_끝나면_git_오류다`(경로 없는 저장소의 검색·활동이 GIT — 시그니처 계산 실패 포함),
`활동은_첫_페이지_문이_열릴_때까지_기다리고_검색은_기다리지_않는다`(`repos:watch` → 대기 중 → `NoteOp::FirstPage` → 완료), `셸_메타문자_질의는_명령을_실행하지_않는다`(local·ssh — 표식 파일. 골든의 양성 단계 `alice's`·`로그인 버그`·`x.y (z) [w]` 가 인용이 깨지지 않았음을 본다),
`검색_결과는_1만개에서_자르고_truncated_를_알린다`(커밋 10,010개·두 갈래가 모두 1만 1개라 병합 스트림을 끝까지 훑는다·활동 total 10,010), `검색_활동_채널은_미러에_없거나_스위치가_꺼지거나_github_면_사이드카`, `등록_전에_온_cancel_은_검색과_활동을_git_없이_취소한다`, `진행_중_검색_활동을_취소해도_멈추지_않는다`,
`활동_요청은_합쳐지고_큐_한도보다_많아도_서로_막지_않는다`(같은 키 셋 중 하나만 취소 + 키가 다른 16개, local·ssh). `ssh_route.rs` 의 "잘못된 호스트면 연결 없이 ERROR"·"스위치가 꺼지면 사이드카" 표에 두 채널이 더해졌다.
단위: 인자가 TS 와 바이트까지 같음(`search.rs`·`activity.rs`), 옵션 모양 질의가 접두가 붙은 한 인자로만 들어감, 해시 갈래 문턱(4·40·41·대소문자·공백), 검증 순서·문구, 빈 질의가 git 을 안 띄움, 캐시(시그니처 일치·삽입 순서·8개), `router.rs` 의 소유 술어 시험.

**대응표**(`test-map.json`): `N/A: PR 4` 16개를 전부 Rust 대응으로 바꿨다 — `activityRepo` 2개·`search` 10개(검색 질의 `it` 는 `channels.rs::검색_활동_시나리오가_ts_골든과_같다`, 빈 질의는 `search.rs::빈_질의는_git_을_띄우지_않고_빈_결과를_낸다`),
그리고 PR 3a 가 `N/A: PR 4` 로 돌려놓은 `repoService` 4개를 **activity 수준 시험**(라우터 경유·실제 git 저장소)으로 매핑했다:

| TS `it` | Rust | 단정 |
|---|---|---|
| `noteLog(logStale)` | `활동은_logstale_알림_뒤에도_바뀐_저장소를_새로_센다` | 활동 → 커밋 → `NoteOp::Stale` → 활동이 +1, 기억이 비었다, 모르는 저장소는 무시 |
| `noteLog(fetchChanged)` | `활동은_fetch_변경_알림_뒤에도_…` | 같은 모양(`NoteOp::FetchChanged`) |
| 시그니처 재사용 | `활동은_기억한_시그니처를_재사용하지_않고_알림_없이도_매번_새로_계산한다` | **TS 와 반대로 "재사용하지 않음"을 단정한다** — 첫 활동 뒤 기억이 있어도 알림 없이 커밋하면 활동이 +1(기억값을 읽는 구현이면 캐시 적중으로 낡은 값이 나온다), 기억이 갱신되고, 바뀐 게 없으면 캐시 적중, ref 삭제도 반영 |
| `StaleLogError` | `로그가_stale_을_알린_뒤의_활동은_바뀐_이력을_센다` | `git:log` 가 STALE 을 내는 시나리오(`branchy`: 커서를 물리고 `topic` 삭제) 뒤 활동 total = `rev-list --count`(줄었다) |

주의: 앞의 두 알림 시험은 PR 4 에서는 **알림이 없어도 통과한다**(기억값을 안 읽으므로). 알림이 활동 결과를 깨지 않음을 보증할 뿐이고, 알림이 필요해지는 것은 7b 가 재사용을 켜는 때다(그때 알림 없이는 낡은 값이 나오므로 이 시험이 의미를 갖는다 — 7b 에서 "알림 없이는 낡다"를 단정하는 시험을 더한다).
`golden.rs` 의 `source_of`·`golden6a.test.ts` 의 `RUST_SOURCES` 에 `search.rs` 를 더했다. 이 PR 뒤 `<파일>.rs::<fn>` 115개, `N/A: PR n` 41개, 사유 있는 N/A 43개(합 199).

**TS 와 다른 점**
- 비문자열·누락 질의의 문구가 `invalid query` 다(TS 는 `query.trim is not a function` TypeError — code 는 같은 ERROR, 골든은 이 입력의 문구를 비교하지 않는다). NUL 질의는 Rust 가 git 전에 거부하고 TS 는 spawn 검사가 거부한다(둘 다 ERROR).
- 세 갈래 중 하나가 실패하면 TS 는 가장 먼저 거절된 것의 오류를 내고(`Promise.all`) Rust 는 `try_join!` 로 같다 — 나머지 갈래는 TS 에서는 계속 돌지만 Rust 는 future 를 버려 git 이 정리된다(결과에 영향 없음).
- SSH 의 선읽기 초과가 검색 병합·활동 집계에서 ERROR 다(위). TS(ssh2)는 backpressure 라 없다.
- activity 는 호출마다 시그니처를 새로 계산한다(위) — TS 는 감시가 갱신한 기억값을 재사용한다. 7b 에서 맞춘다.
- 시그니처 계산 실패의 code 를 GIT 으로 맞췄다(`signature.rs`, 위).
- 같은 키 활동 요청의 합치기는 `futures::Shared`(키 큐)이고 TS 는 `AbortController` + 구독자 수다 — 관찰되는 동작(모두 떠나면 git 종료, 한쪽 취소가 다른 쪽을 죽이지 않음)은 같다.

**사람 확인(자동화 불가·실환경)**
- `pnpm tauri:build:cdp` 뒤 `ANTLER_ROUTE_LOG=1 ANTLER_DRIVE_DEBUG=1 node scripts/drive.mjs <저장소> --tauri …` 로 검색창(메시지·작성자·해시·`--` 로 시작하는 질의·한글)과 커밋 미니맵이 `-> rust` 인지 보고, `ANTLER_RUST_GIT=0`(사이드카)·Electron 과 결과·순서가 같은지 비교한다. Jira 개발 패널의 이슈 키 검색(`git:search`)도 같다.
- 10만 커밋 저장소(`scripts/make-big-repo.mjs`)에서 미니맵 첫 표시 시간(시그니처 3 git + 스트림)이 Electron·사이드카와 비슷한지, 검색 중 새 질의로 이전 검색이 바로 취소되는지(작업 관리자에서 git 자식 프로세스가 남지 않는지).
- 실제 sshd 의 SSH 저장소(큰 저장소)에서 검색·미니맵이 호스트당 연결 하나로 열리는지(원격 `ss`/`who`)와 16MB 선읽기 초과 시 오류가 나지 않는지. 이 PC 에는 WSL 이 없어 WSL 저장소는 **실행해 보지 않았다**(질의는 `--grep=<q>` 한 인자로 `wsl.exe` argv 에 실린다 — 인자 단위 시험과 `tests/exec.rs` 의 `wsl_통합` 이 덮는다).

**남은 위험**
- activity 가 호출마다 git 3번(시그니처)을 더 돈다 — 느린 SSH·큰 저장소에서 미니맵 지연이 TS 보다 클 수 있다(7b 까지). 캐시 적중이어도 시그니처는 매번 계산한다.
- `-F -i --grep=` 이 git 판마다 같다고 가정한다(다중 행 질의 `a\nb` 는 줄마다 OR 패턴이 된다 — 골든이 이 동작을 고정한다. 두 판이 같은 git 을 쓰므로 일치하지만 git 판이 바뀌면 골든 갱신이 필요할 수 있다).
- **SSH 선읽기 초과 분기(검색 병합·활동 집계)는 실행된 적이 없다** — 소비가 빨라 시험에서 상한(16MB, 시험 훅으로 줄일 수 있다)에 닿게 만들지 못했다. 코드는 `log_service` 의 같은 분기를 따랐을 뿐이다. 취소 시험은 2ms 뒤 취소라 스트림 도중에 끊겼는지는 알 수 없다(어느 쪽이든 ABORTED 이거나 정상 끝이어야 하고 슬롯·합치기 표가 풀려 다음 요청이 받아지는 것만 본다).
- 합치기 큐(`joins`)의 키 표는 버려진 요청의 항목을 다음 요청이 넣을 때 치운다(`ProcessQueue` 의 기존 동작) — 저장소·범위 조합 수에 비례한 작은 표다.
- 해시 갈래의 `rev-parse` 가 모호한 짧은 해시(여러 객체에 일치)에서 실패하면 조용히 갈래가 비는 것은 TS 와 같다.

### PR 5 — `git:repoExtras`·`git:lfsPaths`(앱에 연결한다)

`git:repoExtras(repoId)`·`git:lfsPaths(repoId, {rev?, paths}, requestId?)` 를 Rust 가 처리한다(local·WSL·SSH 세 실행기 — 라우팅·미러·취소·envelope 는 앞 채널들과 같다). TS: `RepoService.repoExtras`·`lfsPaths` + `src/main/git/repoExtras.ts`.
소유 판정은 새로 만든 것이 없다 — 한 술어 `RepoMirror::resolve` 와 스위치 `ANTLER_RUST_GIT`·`ANTLER_RUST_SSH` 뿐이다(github 소스는 미러가 사이드카로 보낸다).

| 파일 | 내용 |
|---|---|
| `git/lfs.rs` | PR 1 의 파서 아래에 git 호출을 더했다: `repo_extras`·`lfs_paths`·`LfsPathsRequest`(검증)·argv 빌더 6개·`RepoExtras`(응답). `code_browse::field` 가 `pub(crate)` 로 바뀌었다(요청 객체의 필드 읽기 재사용) |
| `router.rs` | `RUST_CHANNELS` 에 두 채널, `git_extras_channel`·`extras_job`·`ExtrasJob` |

| 채널 | git(상수 + 검증된 값) | 큐 | 취소 |
|---|---|---|---|
| `git:repoExtras` | 동시: `ls-tree -r -z --name-only --full-tree HEAD`(16MB) → `.gitattributes` 마다 `cat-file blob HEAD:<n>`(최대 20개, 각 256KB, 이름은 git 출력) · `config -z --get-regexp ^filter\.`(저장소당 한 번 — 필터 가드) → `<가드 -c…> diff --no-index --no-ext-diff --no-textconv --no-color -- /dev/null .gitattributes`(512KB) · `rev-parse --path-format=absolute --git-path info/attributes` → 그 경로에 같은 `diff --no-index`(같은 가드) · `config --local --null --get-regexp ^gitflow\.`(256KB). **어느 하나가 실패해도 `lfs:false`·`gitflow:null` 로 삼킨다**(TS `.catch`). 종료 코드 1 이 `--no-index` 의 정상이다 | 슬롯 하나 안에서 전부(합치지 않는다·캐시 없다) | 없다 |
| `git:lfsPaths` | `check-attr [--source=<rev>] -z filter -- <경로…>` — rev 는 `HexHash`(대소문자 보존), 경로는 전부 `--` 뒤, UTF-16 6000 단위로 묶음. `--source` 를 모르는 옛 git(`unknown option`·`unrecognized option`·`usage: git check-attr`, ASCII 대소문자 무시)이면 그 묶음과 이후 묶음을 `--source` 없이 다시 한다. 그 밖의 실패는 GIT(`git check-attr failed (<코드>)` 또는 stderr) | 슬롯 하나 | requestId 로 프로세스에 바로 닿는다(`queue.run(None, Some(token))`) |

**검증(보안)**: `lfsPaths` 의 renderer 값은 둘이다. ①`rev`: 있으면(`null` 도 있는 것) `HexHash`(16진수 4~64자 — `-` 로 시작할 수 없다) `invalid commit hash` ②`paths`: 배열이어야 하고(`invalid paths`) **앞 1000개로 자른 뒤** 각 항목이 비어 있지 않은 NUL 없는 문자열이어야 한다(`invalid path`) —
1001번째부터는 거부가 아니라 버려지고 검사도 받지 않는다(TS 와 같다). 순서도 TS 와 같다(rev → paths → 항목). 검증에 걸리면 git 도 취소 레지스트리도 건드리지 않고 `rust_handled` 에도 세지 않는다. 요청이 `null`·인자 없음이면 TS 는 TypeError 라 Rust 는 `invalid request`(code ERROR)다 — 문구만 다르다.
`repoExtras` 는 인자가 저장소 id 뿐이다. git 인자에 들어가는 저장소발 값은 `HEAD:<ls-tree 가 준 이름>`(한 인자, 접두가 있어 옵션이 될 수 없다)과 `rev-parse` 가 준 절대 경로(`--` 뒤)뿐이다.

**필터 실행 차단(#121) — `git diff --no-index` 는 저장소의 `filter.*.clean|process` 를 실행하므로 `FilterGuard` 로 무력화한다**
- `--no-ext-diff`·`--no-textconv` 는 외부 diff·textconv 만 끈다. **`--no-index` 도 `-C` 저장소의 설정·`.gitattributes`·`info/attributes` 를 읽어** 파일에 맞는 규칙이 있으면 `filter.<이름>.clean`·`.process` 를 실행한다
  (git 2.49.0 windows 실측. 최소 재현: `R/.gitattributes` 에 `.gitattributes filter=evil`, `git -C R config filter.evil.clean "sh -c 'echo x > MARK; cat' --"` 뒤 `git -C R diff --no-index --no-ext-diff --no-textconv --no-color -- /dev/null .gitattributes` 가 `MARK` 를 만든다.
  `info/attributes` 규칙·`process` 만 있는 경우·일반 파일도 같다). 닿는 곳은 `repoExtras` 의 작업 트리 `.gitattributes`·`info/attributes` 읽기(저장소를 열 때마다)다. `lfsPaths`(`check-attr`)·`ls-tree`·`cat-file blob`·`config` 는 필터를 돌리지 않는다.
- Rust 의 처리(TS `repoExtras.ts` `usesLfs`·`fileUsesLfs` 와 같다): `wip::filter_overrides` 가 저장소당 한 번 `git config -z --get-regexp ^filter\.`(종료 코드 1 = 빈 가드) 를 조회해 `FilterGuard::from_config_output` 로 가드를 만들고, 두 읽기(작업 트리 `.gitattributes`·`rev-parse` 가 준 `info/attributes`)가 같은 가드를 쓴다.
  `-c filter.<이름>.clean= … smudge= … process= … required=false` 는 `diff` 앞(전역 옵션 자리)에 붙는다. 조회는 다른 읽기(`ls-tree`·`rev-parse`)와 동시에 시작하고(`futures` `Shared`), 두 `--no-index` 는 조회가 끝난 뒤에 돈다.
  **무력화할 수 없으면**(`UnsafeFilterName`: 이름에 `=`·잘못된 UTF-8·개수/길이 상한 초과, 또는 `git config` 가 0·1 이 아닌 코드/실행 실패) 그 파일은 읽지 않는다 — 두 읽기 모두 건너뛰어 `lfs` 는 커밋된 `.gitattributes`(HEAD 트리) 만으로 판정한다(저장소에 커밋된 규칙이 있으면 true 일 수 있다. TS 와 같다).
- **가드 없이 `--no-index` 를 돌릴 수 없다**: `no_index_args(&FilterGuard, path)`·`read_worktree_attributes(…, &FilterGuard, …)` 가 가드를 필수 인자로 받고, 소스 검사 시험(`lfs_argv_는_상수_뒤_검증된_값뿐이고_작업_트리_읽기는_한_곳이다`)이 `"--no-index"` 리터럴이 `no_index_args` 한 곳뿐이고 호출이 `read_worktree_attributes` 안에서 `no_index_args(guard, …)` 로만 일어남을 강제한다.
- **`wipDiff` 의 untracked 도 같은 `--no-index` 다 — TS 는 #121 에서 가드를 걸었고, Rust 는 PR 6 이 `filter_overrides` 로 같게 건다**(PR 6 이 이 사실을 알고 시작해야 한다. `filter_overrides` 는 이 PR 이 `wip.rs` 에 두었다).

**골든**: `tests/fixtures/repos/extras.scenarios.json`(시나리오 12개 — 기존 `basic`·`empty` 저장소에 단계를 더한다)를 TS(`goldenChannels.test.ts` → `RepoService.repoExtras·lfsPaths`)와 Rust(`channels.rs` → `Router::route`)가 시나리오마다 새 저장소에서 순서대로 돌아 `tests/golden/channels/{repoExtras,lfsPaths}.json` 과 비교한다(local·SSH 127.0.0.1 하네스, 끝에 `connections()==1`).
새 단계 `{write: [경로, 내용]}`(작업 트리·`.git/info/attributes` 파일 쓰기)를 두 하네스에 더했다(PR 6 이 이어 쓴다). 경우: 규칙 없음·커밋 없는 저장소·커밋된 루트/하위 `.gitattributes` 와 작업 트리의 차이·`info/attributes` 만·토큰 경계(주석·`filter=lfsx`·`myfilter=lfs`·탭·CRLF)·
속성 파일 크기 한계(커밋본 256KB 이상은 안 읽고, 작업 트리 읽기는 출력 512KB 에서 끊긴다)·gitflow(안전한 값만·제어 문자 탭·100/101자·`$(…)`·쓸 만한 키 없음 → null)·요청 검증 24개(비객체·`null`·잘못된 rev·잘못된 paths 항목·rev 가 paths 보다 먼저)·특수 경로(`-weird.psd`·`--help.psd`·`--output=x.psd`·`:(top)…`·`:!…`·공백·한글·이모지·개행·`*`·`dir/../a`·저장소 밖 `../outside.psd` → GIT)·
묶음(1500개 중 앞 1000개, 긴 경로라 여러 묶음). 결과 배열이 20개를 넘으면 `{count, first, last}` 로 줄이고 골든의 `args` 도 같은 규칙이다(`summarizeLfsArgs`·`summarize_lfs_args`).
골든 시나리오에는 **저장소가 심은 필터를 둔 것이 없다**(결과가 아니라 "표지 명령이 실행되지 않았는가"가 관심사라 `channels.rs` 의 `no_index_*` 시험이 실제 저장소로 맡는다). TS 의 "전역 설정의 gitflow.* 는 무시한다"는 전역 설정을 바꾸지 않는 원칙(위 "골든 테스트")상 argv 시험(`--local` 고정)으로 옮겼다.

**그 밖의 시험**: `channels.rs` — `적대적_lfs_요청은_git_을_띄우지_않는다`(검증 순서·문구·`rust_handled` 0·1001번째는 검사하지 않음), `repo_extras_는_git_실패를_삼키고_lfs_paths_는_git_오류를_낸다`, `lfs_채널은_미러에_없거나_스위치가_꺼지거나_github_면_사이드카`,
`등록_전에_온_cancel_은_lfs_paths_를_git_없이_취소한다`, `진행_중_lfs_paths_를_취소해도_멈추지_않는다`, `악성_저장소의_필터는_lfs_paths_가_실행하지_않는다`(`*` 규칙 + `filter.evil.*`·`diff.evil.textconv` — `check-attr` 길만 본다),
`lfs_argv_는_상수_뒤_검증된_값뿐이고_작업_트리_읽기는_한_곳이다`(소스 검사). `lfs.rs` 단위 6개: argv 가 TS 와 바이트까지 같음·요청 검증 순서·앞 1000개만 검증·묶음 경계(합 정확히 6000·한글/이모지 단위)·옛 git 판정. `router.rs` 단위 `lfs_채널의_소유는_다른_git_채널과_같은_한_술어다`, `ssh_route.rs` 의 두 표에 두 채널이 더해졌다.
"사이드카로 가는 다른 채널" 예시(`api_route.rs`·`channels.rs`·`router.rs` 단위)는 `git:lfsPaths` 에서 `config:get` 으로 바꿨다(PR 6·7 에서 옮겨질 git 채널 대신 6b 까지 남는 비 git 채널).

**대응표**(`test-map.json`): `N/A: PR 5` 9개를 전부 Rust 대응으로 바꿨다 — `lfsGitflow` 의 실제 git `it` 8개는 `channels.rs::lfs_gitflow_시나리오가_ts_골든과_같다`(LFS 사용·gitflow·하위·info·커밋 없음·묶음),
필터 미실행은 `악성_저장소의_필터는_lfs_paths_가_실행하지_않는다`(`check-attr` 길)·`no_index_읽기는_…`(작업 트리 읽기), rev·NUL 검증은 `적대적_lfs_요청은_git_을_띄우지_않는다`, 전역 gitflow 무시는 `lfs.rs::argv_는_ts_와_바이트까지_같다`(argv `--local`). 이 PR 뒤 `N/A: PR n` 은 PR 6(wip 계열)·7b(repoWatcher)·8 분만 남는다.
`tests/noIndexFilter.test.ts`(#121)도 대응 대상에 넣었다(`golden6a.test.ts` `TARGET_TESTS`): `repoExtras` 쪽 4개 `it` 는 `channels.rs::no_index_읽기는_…`(local)·`…ssh_에서도_…`(루프백 SSH)로, `wipFileDiff(untracked)` 2개는 `N/A: PR 6`.
`no_index_*` 시험: 실제 저장소에 표지 명령(`sh -c "echo x > <임시>/MARK-…; cat"`)을 심은 `filter.evil.clean` 을 두고 **대조군**(가드 없는 `git diff --no-index` 가 표지를 만든다)·작업 트리 `.gitattributes`·`info/attributes`·이름에 `=` 인 드라이버(`lfs:false`)를 `git:repoExtras`(라우터 경유)로 불러 표지가 없고 lfs 판정이 TS 와 같음을 본다. 가드를 빼면(`no_index_args(&FilterGuard::none(), …)`) 두 시험이 실패함을 확인했다.

**TS 와 다른 점**
- 요청이 `null`·인자 없음일 때 문구가 `invalid request` 다(TS 는 TypeError 문구). code 는 같은 ERROR 이고 골든은 이 입력의 문구를 비교하지 않는다.
- 검증이 큐 슬롯 **앞**이다(TS 는 슬롯 안에서 던진다). 결과·code 는 같고, 거부된 요청이 슬롯을 기다리지 않는다.
- `repoExtras` 는 `Aborted` 까지 삼킨다(TS `.catch` 와 같다 — 이 채널에는 취소 신호가 없어 닿지 않는다).
- 옛 git 판정(`OLD_GIT_RE`)은 ASCII 대소문자만 무시한다(패턴이 ASCII 라 JS `/i` 와 결과가 같다).

**사람 확인(자동화 불가·실환경)**
- `pnpm tauri:build:cdp` 뒤 `ANTLER_ROUTE_LOG=1 ANTLER_DRIVE_DEBUG=1 node scripts/drive.mjs <저장소> --tauri …` 로 두 채널이 `-> rust` 인지 보고(저장소를 열 때 `git:repoExtras`, 변경 파일 목록·WIP 패널·상세 패널을 열 때 `git:lfsPaths`),
  `ANTLER_RUST_GIT=0`(사이드카)·Electron 과 같은지 비교한다: LFS 배지(상태 표시줄)·gitflow 배지·LFS 로 추적되는 파일 표시(`.gitattributes` 가 하위 폴더·`info/attributes` 에 있는 저장소 포함), git-lfs 가 설치된 저장소(`filter.lfs.*` 가 있는 일반 저장소)에서 같은 표시.
- 필터 차단: 최소 재현의 저장소(`.gitattributes filter=evil` + `filter.evil.clean` 이 표식 파일을 만드는 명령)를 앱으로 열어도 표식이 생기지 않는지(Rust·사이드카). 자동 시험이 이미 보지만 앱 경로(`ANTLER_ROUTE_LOG` 로 `-> rust`)에서 한 번 더 본다.
- WSL 저장소·실제 sshd 의 SSH 저장소: 같은 배지·표시, 호스트당 연결 하나(원격 `ss`/`who`). 이 PC 에는 WSL 이 없어 **WSL 은 실행해 보지 않았다**(경로·rev 는 `wsl.exe` argv 에 한 인자로 실린다 — argv 단위 시험과 기존 `tests/exec.rs` 의 `wsl_통합` 이 덮는다).

**남은 위험**
- `repoExtras` 는 저장소를 열 때마다 `git config` 조회가 하나 더 돈다(TS 도 같다). 조회가 실패하는 저장소는 작업 트리 속성 파일을 읽지 않아 커밋되지 않은 `.gitattributes` 만으로 LFS 를 쓰는 저장소는 `lfs:false` 로 보인다(안전 쪽 실패, TS 와 같다).
- `wipDiff` untracked 의 가드는 PR 6 의 몫이다(`filter_overrides` 는 준비돼 있다).
- `lfsPaths` 의 `--source` 폴백은 실제 옛 git(2.40 미만)으로 돌려 본 적이 없다 — 판정 함수만 단위 시험했다(TS 도 같다).
- 골든이 TS 와 같다는 것은 시나리오(`repoExtras` 19단계·`lfsPaths` 41단계)의 범위만큼만 보장한다. 이름에 NUL 아닌 비 UTF-8 바이트가 든 `.gitattributes` 경로는 `U+FFFD` 로 바뀐 이름으로 `cat-file` 이 실패해 그 파일만 건너뛴다(TS 와 같다 — 시험하지 않았다).

### PR 7a — 감시 상태 기계(앱에 연결하지 않는다)

`src-tauri/src/git/watcher.rs`(새, 새 크레이트 없음). TS `RepoWatcher.ts` 의 상태 전이를 순수 로직으로 옮겼고, **사이드카 감시는 그대로**다(Rust 감시자는 아무도 만들지 않는다).
- **주입점**: `DirWatch`(이벤트원 — `has_head`·`watch(dir, sink)`; 7b 가 notify 로 구현)와 `WatchHost`(저장소 종류·첫 페이지 문·시그니처·git 디렉터리·`on_change`; 7b 가 `RepoState`·`GitRunner` 로 구현). 시계는 `tokio::time` 이라 시험이 `start_paused` 로 멈춘다(fetch 서비스와 같다 — 별도 시계 트레이트는 두지 않았다).
- **상태 기계**: 저장소마다 태스크 하나가 기준 시그니처(첫 페이지 문 뒤)→ 디렉터리 감시 → `select!`(이벤트 깨움·디바운스 마감·포커스 확인·안전망 폴링·진행 중 확인)을 돈다. `Timing::kick` 이 TS `kick` 의 1:1 이다(디바운스 400ms·확인 사이 2초·2초 넘게 기다린 확인은 더 미루지 않음).
  확인은 한 번에 하나(진행 중이면 건너뜀)·20초 제한(넘으면 git 에 취소)·기준을 못 잡았으면 첫 확인은 기억만·바뀌었을 때만 `on_change`. 폴링은 로컬·github 60초, WSL 4초, SSH 30초이고 포커스를 잃은 채 **거는** 타이머만 4배다.
- **이벤트 필터**: 경로(감시 디렉터리 기준 상대, `\`→`/`)가 `objects/` 로 시작하면 버리고, 경로를 모르거나 rescan 표시면 확인한다. UNC(`^[\\/]{2}`)·`HEAD` 없는 디렉터리는 감시하지 않는다(악성 `.git` 파일의 DoS 방어).
- **스레드 경계**: 이벤트 콜백은 `AtomicBool`+`Notify` 로 합칠 뿐 쌓지 않는다(10만 이벤트 → 확인 1회). 저장소를 바꾸면 태스크를 취소해 이전 확인의 결과가 버려지고 git 에도 취소가 간다. `Signature::for_test`(`cfg(test)`)만 `signature.rs` 에 더했다.

**시험**(`watcher.rs` 단위 28개, 가짜 `DirWatch`·`WatchHost`·멈춘 시계): 상수·종류별 주기, 이벤트 필터 표, UNC 판정, 디바운스(399ms 전엔 없고 401ms 에 확인)·알림 여부(시그니처가 같으면 확인만 하고 알리지 않음)·최소 간격(2400ms)·끊이지 않는 이벤트(2200ms 에 확인)·10만 이벤트,
WSL(4초→포커스 잃으면 16초)·SSH(30초)·로컬 60초 폴링, 포커스 얻으면 즉시 확인·진행 중이면 건너뜀(디바운스 확인은 다시 부르지 않음·폴링 시각이 오면 건너뛰고 재무장)·기준 중 포커스 무시·첫 페이지 문 전에는 폴링도 걸지 않음, 기준 실패 뒤 첫 확인은 기억만, 확인 실패 뒤 재시도, 20초 제한과 취소, 저장소 전환 중 늦게 끝난 확인 무시·핸들 닫힘, 멈춤·dispose, 정지·교체가 돌아오기 전에 핸들을 닫음·`unwatch_if`·소유자 drop 으로 정리, UNC·HEAD 없음 거부, git 디렉터리 조회 실패·종류 모름.
`test-map.json` 의 `repoWatcher` 항목은 실제 git·파일 변화를 쓰는 `it` 하나뿐이라 **그대로 `N/A: PR 7b`** 다(그 `it` 의 상태 전이 부분은 위 시험이 덮고, 실제 git·notify 는 7b 가 옮긴다).

**TS 와 다른 점**
- `watch` 가 동기다(기준 계산은 태스크). 이벤트는 합쳐서 처리한다 — 같은 순간의 이벤트는 TS 에서도 같은 결과다. 이벤트 사이 간격이 있으면 깨어나는 즉시 처리하므로 TS 와 같다.
- 기준을 잡은 뒤 디렉터리를 여는 동안(`git rev-parse` 한 번) 받은 포커스는 감시 루프가 시작된 뒤에 확인한다(TS 는 바로).
- 알려진 TS 동작을 그대로 옮겼다: **확인이 진행 중일 때 온 이벤트의 확인은 건너뛰고 다시 부르지 않는다**(그 사이 바뀐 ref 는 다음 이벤트·폴링에서 잡는다).
- `last_check`(최소 간격의 기준)는 저장소마다 새로 시작한다(TS 는 감시자 필드라 저장소를 바꿔도 남는다). `kind` 는 저장소를 감시하기 시작할 때 한 번만 읽는다(TS 는 폴링을 걸 때마다 조회).
- 정지(`watch(None)`·저장소 교체·`unwatch_if`·`Drop`)는 돌아오기 전에 디렉터리 감시 핸들을 **동기로** 닫는다(핸들은 `Ctl` 이 쥔다 — TS `stop()` 의 `w.close()`, 호출처가 곧바로 캐시 폴더를 지워도 Windows 에서 실패하지 않게). `RepoWatcher` 는 복제되지 않는 소유자이고 `dispose` 없이 버려도 멈춘다. 멀티스레드에서는 정지 직전에 끝난 확인의 `on_change` 가 드물게 정지 뒤 한 번 늦게 올 수 있다(renderer 가 activeRepoId 로 거른다).

**7b 로 넘긴 것**: notify 어댑터(`DirWatch` 구현·`has_head` 의 실제 파일 검사), `WatchHost` 구현(`RepoState` 의 `after_first_page`·`shared_signature`/`compute_signature`, 확인마다 `known_signature` 갱신), 라우터·`lib.rs` 포커스 배선(`router::in_runtime_of`), 사이드카 `owns`(소유 전환은 한 PR), `repo:changed` 이벤트 싱크, activity 의 시그니처 재사용 켜기, 실제 git·SSH 루프백 시험, 종료 뒤 notify 스레드 0.
7b 가 지킬 계약: `WatchHandle` 의 drop·`DirWatch::has_head`·`WatchHost::on_change` 는 tokio 워커(또는 정지를 부른 스레드)에서 불리니 **막지 않는다**(네트워크 드라이브 stat 등 오래 걸리는 일을 하지 않는다). 저장소를 제거할 때는 `watch(None)` 이 아니라 `unwatch_if(repo_id)` 를 쓴다(다른 저장소로 바뀐 감시를 끊지 않게 한 잠금 안에서 비교한다).

**남은 위험**: 확인 건너뜀(위)이 TS 와 같아 변화를 한 주기 늦게 알 수 있다. 상태 기계 시험은 가짜 이벤트원이라 notify 의 실제 이벤트 모양(rename·rescan 표시)은 7b 에서 처음 확인된다.

### PR 8 — `gh:repo`·repoq 의 git 조회(앱에 연결한다)

Rust 가 git 을 맡는 저장소(local·WSL·SSH)의 `gh:repo` 채널과 GitHub 서비스가 사이드카에 묻던 저장소 정보(`repoq` 의 `ghRepo`·`ghRemotes`·`pushedShas`)를 Rust 가 처리한다. 그 저장소에 사이드카가 `git config --get-regexp ^remote…`·`git log --remotes` 를 돌리지 않는다.
TS: `RepoService.ghRepo`·`ghRemotes`·`pushedShasByAuthor`·`invalidateGhRemotes` + `commands.ts` 의 `remotes` + `github/remoteUrl.ts`. github 원격 전용 저장소(kind github)는 사이드카 그대로다(레코드가 어느 GitHub 저장소인지 안다 — git 을 부르지 않는다).
토큰은 건드리지 않았다(이 PR 의 git 은 remote URL·로컬 로그만 읽고, 서비스의 토큰 경로는 그대로 Rust 안이다).

| 파일 | 내용 |
|---|---|
| `github/remote_url.rs` | `parseGithubRemote`·`pickGithubRemote(s)` 이식. URL 은 `url` 크레이트(WHATWG), scp 형식은 `^(?:[^@/\s]+@)?([^:/\s]+):(.+)$` 를 손으로 옮겼고 owner/repo 규칙은 `Slug::parse` 한 곳이다 |
| `git/remotes.rs` | `remotes`(`config --null --get-regexp ^remote\..*\.url$`)·`parse_remote_config`·`pushed_shas`(`log --remotes --max-count=<n> --format=%H -F -i --author=<이메일>`)·`parse_pushed_shas`·요청 검사 |
| `router.rs` | `RUST_CHANNELS` 에 `gh:repo`, `gh_repo_channel`, 서비스 길의 질의 `gh_repo`·`gh_remotes`·`pushed_shas_by_author`, 캐시 둘, `invalidate_gh_remotes`, `repos:update`·`repos:remove` 엿보기 |
| `routed_repos.rs`(신규) | `RepoQuery` 의 합성 구현 `RoutedRepos` — Rust 가 맡는 저장소는 라우터가, 나머지는 `SidecarRepos`(repoq)가 답한다. `lib.rs` 가 GitHub 서비스에 이것을 넣는다 |

**소유 판정은 새로 만든 것이 없다 — 한 술어 `RepoMirror::resolve` + 스위치 `ANTLER_RUST_GIT`**: 채널(`gh:repo`)과 서비스 길(`RoutedRepos`)이 같은 `Router::rust_target` 를 쓴다.

| 결과 | `gh:repo` | 서비스의 `gh_repo`·`gh_remotes`·`pushed_shas_by_author` |
|---|---|---|
| `Resolved::Rust`(local·wsl·켜진 ssh) | Rust | Rust |
| `Resolved::Sidecar`(github·미러에 없는 id·풀 없는 ssh)·`ANTLER_RUST_GIT=0` | 사이드카 | `SidecarRepos` 위임 |
| `Resolved::Invalid`(미러가 경로·호스트·배포판을 거부) | ERROR 봉투 | **오류**(위임하지 않는다 — 위임하면 미러가 막은 경로에 사이드카가 git 을 돌린다) |

`github_client_id`·`repo_ids`(설정 읽기)는 항상 사이드카다. `gh:repo` 는 `ANTLER_RUST_API` 와 무관하다(git 채널이다) — `ANTLER_RUST_API=0` 이면 GitHub 서비스가 사이드카에 있어 사이드카의 `RepoService` 가 remote 를 git 으로 읽는다.

**TS 와 같은 순서·캐시** (`RepoService.ts` 745-797)
- `gh:repo`: 60초 캐시(**없음도** 담는다) → 첫 로그 페이지 문(`RepoState::after_first_page` — 큐에 들어가기 전에) → 큐 슬롯 안에서 `config`(키 `remotes:<id>`) → `pick_github_remote`.
- `ghRemotes`(Launchpad): 10분 캐시, 문은 없다(TS 도 없다), 같은 큐 키 `remotes:<id>` 라 진행 중인 `gh:repo` 의 git 과 합쳐진다. 취소는 기다림만 끊는다(합쳐진 다른 호출자의 git 은 죽이지 않는다).
- `pushedShas`: 큐 키 없음, 취소가 슬롯 대기와 프로세스에 모두 닿는다. git 이 실패하면(`code != 0`) 빈 목록이다. 이메일은 `--author=<…>` 한 인자(`-F -i`)로만 들어간다.
- `remotes`: 종료 코드 1 은 빈 목록, 그 밖의 실패는 GIT 오류(`stderr` 를 다듬은 문장, 비면 `git config failed (<코드>)`). stdout 은 손실 UTF-8 로 읽는다(TS `toString('utf8')`).
- 무효화: `repos:update`·`repos:remove` 는 사이드카가 응답하므로 라우터가 인자의 저장소 id 를 엿보아 **목록 캐시(10분)만** 비운다(TS 핸들러의 `invalidateGhRemotes(repoId)` 와 같다 — `gh:repo` 의 60초 추론은 TS 도 그대로다). `gh:launchpad` 의 `fresh` 는 서비스가 `invalidate_gh_remotes` 로 전부 비우고 사이드카에도 알린다.
  미러가 바뀌면 사라졌거나 Rust 가 맡지 않게 된 저장소의 캐시 항목을 치운다(표가 자라지 않게). 앱 종료 때 둘 다 비운다.
- 실행기·큐·SSH 연결은 라우터의 `runtime()` 하나다 — 채널 핸들러도 서비스 길도 같은 큐(local 6·wsl·ssh 4)와 같은 `SshPool` 연결을 쓴다. 서비스는 라우터보다 먼저 만들어져서 `RoutedRepos` 가 라우터를 **호출 때 찾는다**(`RouterSource` — 앱은 `AppHandle` 의 관리 상태, 시험은 `Arc<Router>`).

**보안 경계**
- git 인자는 서브커맨드·옵션 상수 + 검증된 값뿐이다. 이메일은 renderer(아바타 작성자)에서 오는 값이라 `check_pushed_request` 가 사이드카 `repoq` 검사(길이 1~320·개수 1~100)에 **NUL·개행 거부**를 더해 git 을 띄우기 전에 거른다(원격 셸 줄·프로세스 인자에 들어갈 수 없는 값). 옵션처럼 보이는 이메일(`--output=x`)도 `--author=<…>` 한 인자라 옵션이 될 수 없다(골든 시나리오가 `pwned.txt` 가 생기지 않음을 본다).
- 서비스 길에는 사이드카 질의와 같은 제한 시간(30초, `QUERY_TIMEOUT`)을 건다(멈춘 SSH 호스트가 `gh:*` 를 붙잡지 않게). 시간이 지나면 future 를 버려 대기 중인 git 이 정리된다. 채널(`gh:repo`)에는 TS 처럼 제한이 없다.
- URL 해석: 앞뒤 JS 공백(`\s` 와 같은 집합 — `is_js_space`)을 자르고 NUL·공백이 있으면 거부, 스킴은 `http`·`https`·`ssh`·`git` 만, 호스트는 `github.com`·`www.github.com`(대소문자 무시), 포트는 그 스킴의 기본값만, 경로는 정확히 owner/repo 둘(끝 `/`·`.git` 한 번만). 서비스가 받은 값을 `Slug::parse` 로 다시 검증하는 것(PR 9)은 그대로다 — 이 PR 의 값도 같은 `Slug::parse` 를 통과한 것만 나온다.

**골든·시험**
- 순수 로직 골든(`tests/golden/parsers/{githubRemote,pickRemote,remoteConfig,pushedShas}__edge_*`): TS 가 만든 케이스 목록(`tests/fixtures/edge/{github-remote,pick-remote,remote-config,pushed-shas}-cases.json`)을 `tests/golden.rs` 가 같은 결과로 재현한다. URL 케이스 137개는 WHATWG 두 구현(Node·`url` 크레이트)이 갈릴 만한 입력을 모았다(포트 `:022`·`:443`·빈 포트, 호스트 끝 점·퍼센트 인코딩 호스트·대소문자, `\`, `?`·`#`, `..`·`%2e%2e` 경로, userinfo 의 `@`, NBSP·BOM·`U+0085`·NUL, 이모지·IPv6, owner 39/40자·repo 100/101자). 모두 일치했다.
  TS 쪽은 `commands.ts` 에 순수 함수 `parseRemoteConfig`·`parsePushedShas` 를 export 만 했다(동작은 그대로).
- 채널 골든(`tests/golden/channels/{ghRepo,ghRemotes,pushedShas}.json`, 시나리오 `tests/fixtures/repos/remotes.scenarios.json` 10개): TS `RepoService` 가 만들고 `channels.rs` 의 `remote_조회_시나리오가_ts_골든과_같다`(local)·`…ssh_에서도…`(127.0.0.1 루프백 SSH, 끝에 `connections()==1`)가 `gh:repo`(라우터)·`RoutedRepos`(remote 목록·push 된 커밋)로 같은 결과를 낸다. **어느 단계도 가짜 사이드카에 질의가 가지 않음**을 시나리오마다 본다.
  캐시(60초 추론이 바뀐 remote 를 못 봄·무효화는 목록만), 우선순위·중복(대소문자)·포트가 다른 ssh·자체 호스트·`github.com.evil.com`, 점이 든 이름·같은 remote 의 여러 url, 이메일 고정 문자열·대소문자·`.*`·`>`·`--output=…`, 원격 추적 ref 없음, 커밋 없는 저장소를 본다. 골든에 시험 경로가 들지 않는다(결과는 owner/repo/해시뿐이라 TEMP 8.3 경로 문제가 없다).
- 그 밖: `channels.rs` — `remote_질의는_rust_가_맡는_저장소만_답하고_나머지는_사이드카에_위임한다`(위임표·거부된 저장소는 위임 없음·스위치 꺼짐·설정 읽기·무효화 통지), `gh_repo_채널은_미러에_없거나_스위치가_꺼지거나_github_면_사이드카`, `저장소를_바꾸거나_지우면_remote_목록_캐시만_비워진다`,
  `gh_repo_는_첫_페이지_문이_열릴_때까지_기다린다`, `push_된_커밋_조회는_적대적_입력을_git_없이_거절한다`, `remote_질의는_취소와_제한_시간을_따른다`. `lib` 단위 4개(scp 분해·우선순위·argv 바이트·요청 검사). `api_route.rs` 의 `gh:repo` 시험은 새 동작으로 바꿨다(미러에 없으면 사이드카·맡은 저장소는 Rust 가 GIT 오류로 증명), `ssh_route.rs` 의 두 표에 `gh:repo` 를 더했다(잘못된 호스트 → ERROR 이고 연결 0, 스위치 꺼짐 → 사이드카).
- `test-map.json`: `ghRemote`(9)·`pushedShas`(2)의 모든 `it` 를 Rust 대응으로 적었다(`golden6a.test.ts` `TARGET_TESTS` 에 두 파일 추가). 이 PR 뒤 `N/A: PR n` 은 PR 6(wip 계열)·7b(repoWatcher) 분만 남는다.

**TS 와 다른 점**
- 서비스 길(`RoutedRepos`)에 30초 제한이 있다(TS `RepoService` 의 `ghRemotes`·`pushedShasByAuthor` 에는 없고 사이드카 경로의 `SidecarRepos` 에 있었다 — Rust 로 옮기며 그 제한을 잃지 않게 했다).
- `pushedShas` 의 이메일에 NUL·개행이 있으면 오류다(사이드카의 `repoq` 검사는 길이만 봤다). 정상적인 이메일에는 없는 값이다.
- 캐시 표를 미러 변경 때 치운다(TS 는 `dispose` 에서만 비운다).
- Rust 경로에는 `SidecarRepos` 의 응답 모양 상한(remote 100개·이름 200자)이 걸리지 않는다 — 읽는 쪽이 Rust 자신이라 신뢰하지 않을 응답이 없고, 값은 `Slug::parse` 를 통과한 owner/repo 뿐이다(remote 개수는 git 설정이 정한다).
- `U+0085`(NEL)는 JS 공백이 아니다 — 새 코드는 `is_js_space` 로 JS 와 같게 잘라, 기존 `parsers::js_trim`(Rust 의 `is_whitespace` + BOM)과 이 한 글자가 다르다. 기존 helper 는 건드리지 않았다.

**사람 확인(자동화 불가·실환경)**
- `pnpm tauri:build:cdp` 뒤 `ANTLER_ROUTE_LOG=1 ANTLER_DRIVE_DEBUG=1 node scripts/drive.mjs <저장소> --tauri …` 로 `gh:repo` 가 `-> rust`(local·WSL·SSH 저장소)인지, github 원격 전용 저장소는 `-> sidecar` 인지, 이슈·PR·Actions 탭과 커밋 아바타가 `ANTLER_RUST_GIT=0`·Electron 과 같게 나오는지(GitHub 실접속이라 이 PR 에서 하지 않았다).
- **실제 sshd 의 SSH 저장소로 앱을 켠 뒤 사이드카 `ssh2` 연결 수**(원격 `ss`/`who`) — 위 "남은 위험(3)"의 이중 연결은 PR 6·7b 와 6b 가 끝나야 0 이 된다. 저장소를 **추가한 직후는 제외**한다(추가 흐름은 사이드카 실행기를 쓰고 저장된 소스의 런타임은 놓지 않는다 — `RepoService.ts` 443·497·530·551·579).
- Launchpad 의 새로고침(`fresh`)이 저장소 remote 를 다시 읽는지, remote 를 바꾼 직후 `repos:update` 뒤 목록이 갱신되는지. WSL 저장소는 이 PC 에 WSL 이 없어 **실행해 보지 않았다**(git 인자는 상수 + 이메일 한 인자라 `wsl.exe` argv 단위 시험과 `tests/exec.rs` 의 `wsl_통합` 이 덮는다).

**남은 위험**
- `ANTLER_RUST_API=0`(또는 서비스를 못 만든 경우)이면 사이드카의 GitHubService 가 `RepoService` 로 remote 를 git 으로 읽는다 — Rust 가 맡는 SSH 저장소에도 사이드카 연결이 열린다(되돌리기 경로의 비용이다).
- 앱 시작 직후 미러가 채워지기 전의 질의는 사이드카가 답한다(미러는 사이드카가 ready 전에 보내 거의 닿지 않는 틈이다).
- 이 PR 만으로 이중 연결이 사라지지는 않는다: `wipStatus`·`wipDiff`(PR 6)·RepoWatcher(PR 7b)·저장소 추가 흐름(6b)이 남아 있다.
- URL 해석은 두 WHATWG 구현의 일치에 기댄다. 골든 137개가 같았지만 `url` 크레이트가 올라 규칙이 바뀌면 골든이 알려 준다(`UPDATE_GOLDEN` 으로 덮지 않고 원인을 본다).

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
- 직접 확인 필요(SSH PR 3): 실제 sshd 에 있는 저장소를 열어 그래프·로그(더 보기)·커밋 상세·파일 diff·파일 이력·blame 이 TS 판과 같이 나오는지, 큰 저장소의 `file_diff` 가 상한 오류 없이 열리는지,
  `ANTLER_RUST_SSH=0` 으로 실행하면 같은 저장소가 사이드카(ssh2)로 열리는지(`ANTLER_ROUTE_LOG=1` 로 `[route] git:log -> rust|sidecar` 확인), ssh 저장소를 삭제한 뒤 연결이 닫히는지(원격 `who`·`ss`), ssh 저장소의 fetch 가 한 번만 일어나는지(PR 4 부터 Rust 만 — 위 "사람 확인 필요(SSH PR 4)").
- 직접 확인 필요: 확인 창(fetch·SSH)과 폴더·에디터 선택 대화상자가 실제로 뜨고 버튼 응답이 맞는지, 창 위치·크기·최대화가 재시작 뒤 복원되는지(자동 확인은 대화상자를 띄우지 않고 요청·응답 형식만 봤다).
- 배포 전 필수: 사이드카 경로가 빌드 머신 절대 경로로 박혀 있다(`src-tauri/src/sidecar.rs`) — Node 를 걷어 내면 사라지지만,
  그 전에 배포해야 한다면 resource 경로로 바꾼다. `cdp` 기능은 배포 빌드에서 켜지지 않게 CI 에서 막는다.
