# 앱 업데이트

[← README](../README.md)

패키지된 앱은 시작 10초 뒤와 이후 6시간마다 GitHub Releases 에서 새 버전을 확인한다. 하단 상태바의 "업데이트 확인" 으로 직접 확인할 수도 있다. 새 버전이 있으면 화면 오른쪽 아래에 스낵바가 뜨고, 자동으로 내려받거나 설치하지 않는다 — [업데이트] 를 눌러야 시작한다. [나중에] 는 앱을 다시 시작할 때까지, [이 버전 건너뛰기] 는 그 버전을 계속 숨기고(설정 `skippedUpdateVersion`) 더 새 버전이 나오면 다시 알린다. 프리릴리스(`-rc.1` 등)는 현재 버전이 프리릴리스일 때만 제안한다.

| 설치 형태 | 방식 |
|---|---|
| Windows NSIS 설치본, Linux AppImage | electron-updater(GitHub 공급자, `latest.yml`/`latest-linux.yml` 의 sha512 검증). 내려받은 뒤 [다시 시작하여 설치] → `quitAndInstall` |
| macOS (dmg) | 앱이 ad-hoc 서명이라 Squirrel.Mac 의 자동 교체가 안정적이지 않다. 그래서 릴리스에서 `process.arch` 에 맞는 dmg 를 내려받아 해시를 검증한 뒤 앱 전용 폴더(userData/updates)에 저장하고 열어 준다 — 열린 창에서 Antler 를 Applications 로 끌어 넣으면 된다 |
| Windows portable, Linux deb | portable exe 또는 deb 를 같은 방식으로 검증해 내려받는다. deb 는 소프트웨어 센터로 열고, portable 은 파일 위치를 보여 준다 |

보안: 내려받기는 `github.com` / `objects.githubusercontent.com` 의 https 릴리스 자산만(첫 URL 은 이 저장소의 `releases/download/` 경로로 정규화해 확인하고, 리다이렉트도 매번 허용 목록 확인). 검증은 `latest*.yml` 의 sha512 → 없으면 GitHub API 의 sha256 digest 이며, **둘 다 없으면 크기만으로 통과시키지 않고 내려받지도 열지도 않는다**(portable exe 는 yml 에 없어 API digest 에 의존). 받은 파일은 `userData/updates`(0700)에 `.part` 로 받아 검증 뒤 이름을 바꾸고, 다시 열기 전에 해시를 한 번 더 확인한다. 검증에 실패한 파일은 지우고 열지 않는다. "건너뛴 버전" 설정은 main 만 바꾼다(renderer 의 `config:setUi` 로는 불가, `update:dismiss` 는 지금 알리는 버전만 받는다). 토큰은 쓰지 않는다(공개 저장소 · 비인증 한도). Linux 설치 형태는 환경 변수만 믿지 않고 `resources/package-type`(deb) 또는 `APPDIR` 안에서 실행 중인지(AppImage)로 판별하며, 그 밖은 업데이트하지 않는다.

**Windows 업데이트는 코드 서명이 없다**(`publisherName` 없음 — 앱이 미서명이라 electron-updater 가 Authenticode 검증을 건너뛴다). 무결성은 https 로 받은 릴리스 메타데이터(`latest.yml`)의 sha512 에만 의존한다. CI 는 `permissions: contents: read`(release 잡만 쓰기)와 SHA 로 고정한 액션을 쓴다.

주의: 업데이트 기능이 없는 0.1.0 은 스스로 업데이트할 수 없다. 이 기능이 들어간 **다음 릴리스를 한 번 수동 설치**해야 그 이후 릴리스부터 앱 안에서 업데이트된다.

개발·테스트: `ANTLER_UPDATE_FAKE=available|downloading|downloaded|error`(+ `ANTLER_UPDATE_FAKE_MODE=manual-dmg|manual-file`)로 스낵바 상태를 띄운다(미패키지 실행 또는 `ANTLER_UPDATE_DEV=1` 일 때만). `node scripts/drive.mjs . --update-fake <상태> --out out/shots-update` 로 스크린샷을 남긴다. `ANTLER_UPDATE_DEV=1` + `ANTLER_UPDATE_FEED=<url>` 이면 미패키지 실행에서 generic 피드로 auto 경로를 시험할 수 있다.

## Tauri 판 (Phase 6c PR 4)

위는 Electron 판이다. Tauri 판은 `update:*` 7채널을 Rust(`src-tauri/src/update/`)가 처리한다(`ANTLER_RUST_GIT=0` 이면 사이드카가 예전처럼 처리한다). 화면·동작(스낵바·[나중에]·[이 버전 건너뛰기]·6시간 주기)은 같고 다음이 다르다. 구조·시험은 [tauri.md](tauri.md#pr-4--update-7채널을-rust-로앱에-연결한다--업데이트-공개-키-대기).

- **Windows NSIS 설치본은 앱 안에서 설치하고, macOS dmg·Linux deb 는 수동 경로(PR 4b)다.** 켜는 조건은 번들러가 실행 파일에 심은 표식뿐이다 — Windows 는 NSIS, Linux 는 deb(x86_64), macOS 는 `bundle_type()` 이 항상 앱(`App`)을 돌려주므로 **실행 파일이 `*.app/Contents/MacOS/` 안**일 때만 켠다(arm64·x86_64). `--no-bundle` 개발·측정 빌드와 AppImage·rpm·msi·portable 은 확인하지 않는다(수동 확인은 "최신"). portable 은 폐지됐고 CI 는 AppImage 를 만들지 않는다.
- **수동 경로(dmg·deb)**: 매니페스트·내려받기·무결성(minisign 서명 + 서명된 버전 = 매니페스트 버전)은 Windows 와 같고, **설치만 다르다** — 플러그인의 설치(`Update::install`)를 부르지 않는다. 검증을 통과한 바이트를 앱 전용 폴더(`<userData>/updates`, TS 와 같은 위치)에 자산 이름 그대로 저장하고 기본 앱으로 연다(dmg → 마운트, deb → 소프트웨어 센터). [다시 열기]는 디스크의 파일을 다시 읽어 **다시 검증한 뒤** 연다. 매니페스트 키는 `darwin-aarch64-dmg`·`darwin-x86_64-dmg`·`linux-x86_64-deb` 하나뿐이고(일반 키로 내려가지 않는다) URL 의 파일 이름이 `Antler-<버전>-<arm64|x64>.dmg`·`antler_<버전>_amd64.deb` 와 정확히 같아야 한다. 같은 이름의 파일이 이미 있으면 **덮어쓰지 않는다**(내용이 같으면 그대로 쓰고, 다르면 "같은 이름의 파일이 이미 있어 덮어쓰지 않았습니다 …" 오류 — 업데이트 폴더에서 그 파일을 지운 뒤 다시 시도한다). macOS 앱은 미서명·미공증이라 Gatekeeper 경고가 뜰 수 있다([build.md](build.md)).
- **무결성은 minisign 서명이다.** 앱은 매니페스트(`latest.json`)와 설치본을 자기 HTTP 클라이언트로 받는다(`github.com`·`objects.githubusercontent.com`·`release-assets.githubusercontent.com` 의 https·기본 포트·사용자 정보 없음, 리다이렉트도 매번 확인·5번까지, 매니페스트 1MB·설치본 300MB 상한, 읽기가 30초 멈추면 실패). 매니페스트의 설치본 주소는 이 저장소의 **그 버전 태그** 릴리스 자산이어야 한다. 받은 바이트(**디스크에 쓰기 전**)에 서명을 검증하고 — 서명에 기록된 버전이 매니페스트 버전과 달라도 거부한다(옛 서명 설치본에 새 버전 번호를 붙이는 공격 차단) — 설치 직전에 한 번 더 검증한다. 토큰은 쓰지 않는다. 업데이트 요청은 프록시를 따르지 않는다.
- 설치는 NSIS passive 모드(진행 창만)로 하고 설치 뒤 앱이 다시 뜬다. 그 직전에 git 프로세스를 정리한다.
- 개발·테스트: `ANTLER_UPDATE_FAKE=available|downloading|downloaded|error` 는 **개발·cdp 빌드에서만** 읽는다(배포 바이너리에는 이 이름이 없다). `ANTLER_UPDATE_DEV`·`ANTLER_UPDATE_FEED`·`ANTLER_UPDATE_FAKE_MODE` 는 없다(가짜 상태의 모드는 이 빌드의 실제 모드다).

### 업데이트 공개 키 (사용자가 서명 키를 만든 뒤)

공개 키는 `src-tauri/src/update/host.rs` 의 `UPDATE_PUBKEY` 에 들어 있다(2026-11-19, 키 ID `E8CC4BB412D344F1`). 개인 키·비밀번호는 Environment `release` 의 시크릿에만 있고, 원본은 키를 만든 PC 의 `~/.tauri/` 에 있으니 **오프라인으로 백업**한다(잃으면 설치된 앱에 더 이상 업데이트를 보낼 수 없다).
상수가 비어 있으면(`None`) 앱은 새 버전을 *확인*은 하지만 [업데이트]를 누르면 **"업데이트 서명 키가 설정되지 않았습니다. 이 빌드는 업데이트를 설치할 수 없습니다"** 오류가 되고, 설치본을 내려받지도 설치를 시작하지도 않는다(fail-closed).

키를 넣는 방법:
1. `pnpm tauri signer generate -w <오프라인 경로>`(비밀번호 있는 키 권장 — 계획 U1)로 키를 만든다. 개인 키와 비밀번호는 GitHub Environment `release` 의 `TAURI_SIGNING_PRIVATE_KEY`·`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 로만 넣는다([build.md](build.md#tauri-판-패키징발행-없음--phase-6c-pr-2)).
2. 만들어진 `<키>.pub` 의 **내용(base64 한 줄)** 을 `UPDATE_PUBKEY` 에 넣는다: `pub const UPDATE_PUBKEY: Option<&str> = Some("dW50cnVzdGVk…");`. 이 한 줄이 **믿는 키의 유일한 원천**이다 — `tauri.conf.json` 에 `plugins.updater` 를 넣지 않는다(시험이 거부한다. 플러그인 설정은 `lib.rs` 가 이 상수로 만든다).
3. `cargo test --features test-hooks --test update_security`·`pnpm exec vitest run tests/releaseFeatureGuard.test.ts` 를 돌린다(키 형식을 본다).
4. 개인 키가 그 공개 키의 쌍인지는 사람이 확인한다: `tauri-package.yml`(워크플로 `tauri-package`)로 만든 설치본·`.sig` 를 앱이 받아 통과하는지(리허설). 어긋나면 모든 업데이트가 "업데이트 서명이 올바르지 않습니다"로 거부된다.

**공개 키를 넣은 뒤에는 바꿀 수 없다**: 키는 모든 배포 바이너리에 박힌다. 잃으면 기존 설치본은 앱 안에서 영영 업데이트되지 않는다(교체하려면 옛 키로 서명한 "새 공개 키 판"을 먼저 내야 한다).
