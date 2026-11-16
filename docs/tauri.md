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
| 2 | git 핵심(실행기·파서·LogCursor 스트리밍·상세/diff·RepoWatcher)을 Rust 로 | 파서 12개(log·refs·stash·worktree·nulRecords·diff·wipStatus·changes·commit·blame·fileHistory·submodule) 이식·골든 일치(건너뜀 0), 실행기(Local·WSL·ProcessQueue·wslPaths) 이식(아래 "실행기"), 라우터·미러·취소 레지스트리와 첫 채널 `git:commit`(아래 "라우터") — 나머지 채널·LogCursor 스트리밍·RepoWatcher 는 남음 |
| 3 | fetch 가드 — `ALLOWED_KEYS` 허용 목록과 refspec 을 글자 그대로 이식, 기존 fetch 테스트로 패리티 확인, 보안 리뷰 | |
| 4 | GitHub·Jira(reqwest), 토큰은 OS 키체인(`keyring`) | |
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
cargo test --manifest-path src-tauri/Cargo.toml --lib --test golden --test exec --test channels   # CI 와 같은 전체(라우터·채널 패리티 포함)
UPDATE_GOLDEN=1 npx vitest run tests/goldenChannels.test.ts   # 채널 골든(tests/golden/channels, exec/channels.json) 다시 쓰기
ANTLER_RUST_GIT=0 <exe>                               # 라우터를 끄고 모든 채널을 사이드카로(되돌리기 스위치)
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

- `common.rs` 의 `GIT_GLOBAL_ARGS`·`GIT_ENV` 가 유일한 정의이고 `RunOpts` 에는 env 필드가 없다(호출자가 방어 환경변수를 덮어쓸 수 없다. TS 의 `RunOpts.env`·`envFor`·WSL `extraEnv` 는 GitHub 원격 전용 저장소용이라 옮기지 않았다).
- `run`(stdout 64MB 상한, stderr 는 앞 4MB 만 담고 파이프는 끝까지 비움) / `stream`(`GitStream`: `AsyncRead`. **읽을 때만** 파이프에서 가져가므로 소비자가 안 읽으면 git 이 멈춘다.
  EOF 뒤 `finish()` 로 종료 코드를 확인하고, 버리면 트리를 죽인다). 취소는 `CancellationToken`.
- **앱에 연결하기 전 필수(보안 리뷰)**: 실행기는 호출자 `args` 를 전역 옵션 바로 뒤에 붙인다 — renderer 값이 그 자리에 오면
  `-c alias.x=!cmd`·`--output=<경로>` 로 명령 실행·파일 쓰기가 된다. 라우터는 (1) 서브커맨드·옵션을 상수로 정하고 renderer 값(ref·해시·경로)은
  검증 뒤 정해진 위치·`--` 뒤에만 넣고, (2) 저장소 경로·배포판은 renderer 가 아니라 사이드카가 보낸 미러(main 이 저장한 값)에서만 가져오며
  `src/main/ipc/handlers.ts` 의 `checkPath`(UNC·NUL·길이)·배포판 목록 대조를 이식하고, (3) LogCursor 를 버릴 때 `GitStream` 을 drop 한다
  (스트림의 취소 토큰은 다음 읽기 때만 확인된다 — 토큰만 취소하고 구조체를 남기면 git 이 파이프에서 멈춘 채 남는다).
  (1)·(2)는 라우터 PR 에서 채웠고(아래 "라우터"), (3)은 LogCursor 를 옮기는 PR 의 몫이다.
  Rust CI(`tauri.yml`)를 main 룰셋의 필수 체크(`Rust · 골든 · clippy`)로 올려야 clippy 의 `disallowed-methods`·주입 거부 테스트가 실패한 PR 이
  머지되지 않는다. 워크플로는 필수 지정에 맞게 항상 결과를 보고하도록 바꿨다(관련 없는 PR 은 건너뛰고 성공). **룰셋 변경은 저장소 설정이라 사용자 확인 후 한다.**
- Windows: `CREATE_NO_WINDOW`, 취소·Drop 때 `taskkill /pid N /T /F` 를 **한 번만**, **별도 스레드에서** 띄우고 기다린다. 이미 끝나 수거된 자식에는 보내지 않는다.
  루트 프로세스 핸들 복제본을 taskkill 이 끝날 때까지 쥐고 있어 그 사이 PID 가 재사용되지 않고, taskkill 이 실패하면 그 핸들로 루트를 `TerminateProcess` 한다.
  처음 이식은 런타임 스레드에서 taskkill 을 2~3번(명시적 kill + Drop) 띄워, 병렬 테스트 부하에서 취소→반환이 8~22초까지 늘었다(현재 1ms 미만).
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
renderer → antler_invoke → Router::route ─ Done(envelope) ──────────────→ renderer      (Rust 경로: 지금은 git:commit 하나)
                                         └ Sidecar → Node 사이드카 → envelope → renderer (그 밖의 모든 것)
```

- **Rust 경로 조건**(전부): 채널이 `RUST_CHANNELS` 에 있다 · 인자의 저장소 id 가 미러에 있다 · 그 kind 가 local 또는 wsl 이다(ssh·github 소스는 사이드카) ·
  환경변수 `ANTLER_RUST_GIT` 이 `0` 이 아니다. 미러에 없는 id 는 사이드카로 가서 오류 문구를 사이드카가 만든다.
- **되돌리기 스위치**: `ANTLER_RUST_GIT=0` 으로 실행하면 모든 채널이 사이드카로 간다(Electron 판과 같은 경로). 확인용으로 `ANTLER_ROUTE_LOG=1` 이면
  `RUST_CHANNELS` 채널이 어디로 갔는지 stderr 에 `[route] git:commit -> rust|sidecar` 로 남긴다(기본은 조용하다. drive 는 `ANTLER_DRIVE_DEBUG=1` 로 stderr 를 본다).
- **미러**(`src-tauri/src/mirror.rs`, 프로토콜 `{t:"repos", gen, repos:[{id, kind, path, distro?}]}`): 설정을 쓰는 쪽은 사이드카의 `ConfigStore` 뿐이다.
  `ConfigStore.onChange`(기본 아무 일도 안 함 — Electron 판은 쓰지 않는다)가 `update` 안에서 동기로 불려 사이드카가 미러를 보낸다 — 시작 때 ready 전에 한 번,
  저장소 목록이 바뀔 때마다(창 위치·UI 설정처럼 저장소와 무관한 쓰기는 같은 내용이라 보내지 않는다). 같은 stdout 에서 순서대로 읽으므로 저장소를 바꾼 호출의 응답보다 미러가 먼저 반영된다.
  `gen` 이 이미 받은 것 이하이면 버린다. 토큰 등 비밀은 담지 않는다. **Rust 는 renderer 가 보낸 경로·kind·배포판을 쓰지 않는다** — 저장소 id 와 검증된 해시만 받는다.
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
- 골든 갱신: `UPDATE_GOLDEN=1 npx vitest run tests/goldenChannels.test.ts`. 채널을 `RUST_CHANNELS` 에 추가할 때는 시나리오·골든도 함께 넣는다.
- Rust 테스트의 git 격리는 `tests/channels.rs` 의 `isolate`(ctor)가 한다(`exec.rs` 와 같다). 단위 테스트(`--lib`)는 git 을 띄우지 않는다.

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
