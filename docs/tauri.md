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
| 1 | 측정 자동화(`drive.mjs --tauri`), 창 상태·테마·폴더 선택 등 Electron 전용 기능 대체, 골든 테스트(TS↔Rust 같은 fixture 결과 비교) | |
| 2 | git 핵심(실행기·파서·LogCursor 스트리밍·상세/diff·RepoWatcher)을 Rust 로 | |
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

- `src/sidecar/electronShim.ts`: 확인 창(`dialog.showMessageBox`)은 **항상 cancelId(거부)** 로 닫는다 — fetch 허용·SSH 호스트 연결을 renderer 가
  스스로 승인할 수 없다. 그래서 Phase 0 에서는 설정 밖 SSH 호스트·알 수 없는 설정의 fetch 가 막힌다. 실제 확인 창은 Rust 네이티브 대화상자로 만든다
  (webview 안 HTML 로 만들면 침해된 renderer 가 스스로 누를 수 있다).
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
```

요구 사항: Rust(stable, MSVC), WebView2 런타임, PATH 의 node(또는 `ANTLER_NODE`). 환경 변수 `ANTLER_USER_DATA` 로 userData 를,
`ANTLER_GPU=1` 로 하드웨어 가속을, `ANTLER_MEASURE=1` 로 첫 커밋 행 시각 기록(`window.__ANTLER_FIRST_ROW__`)을 켠다.

## Phase 0 에서 확인한 것

- renderer 를 고치지 않고 WebView2 에서 그대로 동작한다(그래프·사이드바·커밋 상세가 그려지고, 로드 뒤 CDP 로 붙어 본 콘솔 오류는 0).
- 사이드카 번들에 `electron` 참조 0, `src/main` 무수정.
- CDP 로 확인(OS 입력 합성 없이 DOM `click()`): 창 버튼 최대화·복원·최소화, 밖에서 커밋하면 `repo:changed` 가 도착해 목록이 갱신됨,
  `location.href` 로 앱 밖 이동 시도는 같은 문서에 머무르고 `window.open` 은 null.

## 남은 일·알려진 제약

- 직접 확인 필요: 스냅 레이아웃 플라이아웃 위치·지연, 툴바 드래그·더블클릭 최대화(입력 합성을 하지 않아 자동 확인 못 함).
- Electron 대비 성능(콜드 스타트·유휴 메모리·10만 커밋 스크롤 p95) — `drive.mjs` 의 Tauri 경로가 아직 없다.
- 새 창으로 여는 링크는 github.com 만 기본 브라우저로 보낸다(Electron 판 `setWindowOpenHandler` 와 같은 규칙).
- 미지원: 폴더 선택 대화상자, `theme:system` 이벤트, 창 위치·크기 기억, `openPath`·`showItemInFolder`.
- 배포 전 필수: 사이드카 경로가 빌드 머신 절대 경로로 박혀 있다(`src-tauri/src/sidecar.rs`) — Node 를 걷어 내면 사라지지만,
  그 전에 배포해야 한다면 resource 경로로 바꾼다. `cdp` 기능은 배포 빌드에서 켜지지 않게 CI 에서 막는다.
