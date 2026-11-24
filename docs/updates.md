# 앱 업데이트

[← README](../README.md)

패키지된 앱은 시작 10초 뒤와 이후 6시간마다 GitHub Releases 에서 새 버전을 확인한다. 하단 상태바의 "업데이트 확인" 으로 직접 확인할 수도 있다. 새 버전이 있으면 화면 오른쪽 아래에 스낵바가 뜨고, 자동으로 내려받거나 설치하지 않는다 — [업데이트] 를 눌러야 시작한다. [나중에] 는 앱을 다시 시작할 때까지, [이 버전 건너뛰기] 는 그 버전을 계속 숨기고(설정 `skippedUpdateVersion`) 더 새 버전이 나오면 다시 알린다. 프리릴리스(`-rc.1` 등)는 현재 버전이 프리릴리스일 때만 제안한다.
`update:*` 7채널을 Rust(`src-tauri/src/update/`)가 처리한다(서비스를 만들지 못하면 `update:*` 는 오류로 답한다). 구조·시험은 [tauri.md](tauri.md#pr-4--update-7채널을-rust-로앱에-연결한다--업데이트-공개-키-대기).

| 설치 형태 | 방식 |
|---|---|
| Windows NSIS 설치본 | 앱 안에서 설치한다(받기 → 검증 → [다시 시작하여 설치] — NSIS passive 모드, 설치 뒤 앱이 다시 뜬다) |
| macOS 앱(dmg 로 설치한 `.app`) | 앱 안에서 설치한다(받기 → 검증 → [다시 시작하여 설치] — 앱 번들 `Antler-<버전>-<arm64\|x64>.app.tar.gz` 를 플러그인이 실행 중인 `.app` 자리에 바꿔 넣고, 종료 정리 뒤 LaunchServices 로 새 판을 띄운다). 매니페스트 키는 `darwin-aarch64-app`·`darwin-x86_64-app` 하나뿐이다 |
| Linux deb | **수동 경로** — 같은 검증을 한 뒤 앱 전용 폴더에 저장하고 기본 앱으로 연다(소프트웨어 센터). 설치는 사용자가 한다 |
| macOS ≤ 2.1.0 | 그 판은 앱 번들 방식을 몰라 dmg 수동 경로(`darwin-<arch>-dmg`)로 받는다 — 그래서 릴리스는 dmg 항목도 계속 싣는다. 앱 번들 방식을 아는 판으로 한 번 옮긴 뒤부터 앱 안에서 바뀐다 |
| Windows portable, Linux AppImage | 없다(v2.0.0 부터 만들지 않는다) |

- **켜는 조건은 번들러가 실행 파일에 심은 표식뿐이다.** Windows 는 NSIS, Linux 는 deb(x86_64), macOS 는 `bundle_type()` 이 항상 앱(`App`)을 돌려주므로 **실행 파일이 `*.app/Contents/MacOS/` 안**일 때만 켠다(arm64·x86_64). `--no-bundle` 개발·측정 빌드와 AppImage·rpm·msi 는 업데이트하지 않는다(수동 확인은 "최신"). 환경 변수는 믿지 않는다.
- **macOS 앱 안 설치**: 바꿔 넣을 `.app` 은 실행 파일 위치(`*.app/Contents/MacOS/`)로 정한다. 앱 폴더에 쓸 권한이 없으면(관리자 소유 `/Applications`) macOS 가 관리자 암호를 묻는다. 교체에 실패하고 이전 앱도 되돌리지 못하면 오류로 알린다(그때는 dmg 로 다시 설치). 앱이 직접 받은 바이트로 쓴 파일이라 quarantine 표식이 붙지 않아, 미서명·미공증 앱이어도 다시 시작할 때 Gatekeeper 확인이 뜨지 않을 것으로 본다(실기기 확인 필요).
- **수동 경로(dmg·deb)**: 매니페스트·내려받기·무결성(minisign 서명 + 서명된 버전 = 매니페스트 버전)은 Windows 와 같고, **설치만 다르다** — 플러그인의 설치(`Update::install`)를 부르지 않는다. 검증을 통과한 바이트를 앱 전용 폴더(`<userData>/updates`)에 자산 이름 그대로 저장하고 기본 앱으로 연다. [다시 열기]는 디스크의 파일을 다시 읽어 **다시 검증한 뒤** 연다. 매니페스트 키는 `darwin-aarch64-dmg`·`darwin-x86_64-dmg`·`linux-x86_64-deb` 하나뿐이고(일반 키로 내려가지 않는다) URL 의 파일 이름이 `Antler-<버전>-<arm64|x64>.dmg`·`antler_<버전>_amd64.deb` 와 정확히 같아야 한다. 같은 이름의 파일이 이미 있으면 **덮어쓰지 않는다**(내용이 같으면 그대로 쓰고, 다르면 "같은 이름의 파일이 이미 있어 덮어쓰지 않았습니다 …" 오류 — 업데이트 폴더에서 그 파일을 지운 뒤 다시 시도한다). macOS 앱은 미서명·미공증이라 Gatekeeper 경고가 뜰 수 있다([build.md](build.md)).
- **무결성은 minisign 서명이다.** 앱은 매니페스트(`latest.json`)와 설치본을 자기 HTTP 클라이언트로 받는다(`github.com`·`objects.githubusercontent.com`·`release-assets.githubusercontent.com` 의 https·기본 포트·사용자 정보 없음, 리다이렉트도 매번 확인·5번까지, 매니페스트 1MB·설치본 300MB 상한, 읽기가 30초 멈추면 실패). 매니페스트의 설치본 주소는 이 저장소의 **그 버전 태그** 릴리스 자산이어야 한다. 받은 바이트(**디스크에 쓰기 전**)에 서명을 검증하고 — 서명에 기록된 버전이 매니페스트 버전과 달라도 거부한다(옛 서명 설치본에 새 버전 번호를 붙이는 공격 차단) — 설치 직전에 한 번 더 검증한다. 토큰은 쓰지 않는다. 업데이트 요청은 프록시를 따르지 않는다(프록시가 필수인 사내망에서는 확인이 실패한다).
- 설치 직전에 git 프로세스를 정리한다. "건너뛴 버전" 설정은 업데이트 호스트만 바꾼다(renderer 의 `config:setUi` 로는 불가, `update:dismiss` 는 지금 알리는 버전만 받는다).
- **Windows 설치본·업데이트에는 코드 서명(Authenticode)이 없다.** 업데이트의 진위는 minisign 서명이 맡는다. 처음 브라우저로 내려받아 실행하는 설치본에는 SmartScreen 경고가 뜰 수 있다. CI 는 `permissions: contents: read`(릴리스 발행 잡만 쓰기)와 SHA 로 고정한 액션을 쓴다.
- 개발·테스트: `ANTLER_UPDATE_FAKE=available|downloading|downloaded|error` 는 **개발·cdp 빌드에서만** 읽는다(배포 바이너리에는 이 이름이 없다). `node scripts/drive.mjs . --tauri <cdp 빌드 exe> --update-fake <상태>` 로 스크린샷을 남긴다. 업데이트 피드를 바꾸는 환경 변수는 없다(시험 훅은 `test-hooks` 기능의 루프백 http 뿐이다).

## 업데이트 공개 키 (사용자가 서명 키를 만든 뒤)

공개 키는 `src-tauri/src/update/host.rs` 의 `UPDATE_PUBKEY` 에 들어 있다(2026-11-19, 키 ID `E8CC4BB412D344F1`). 개인 키·비밀번호는 Environment `release` 의 시크릿에만 있고, 원본은 키를 만든 PC 의 `~/.tauri/` 에 있으니 **오프라인으로 백업**한다(잃으면 설치된 앱에 더 이상 업데이트를 보낼 수 없다).
상수가 비어 있으면(`None`) 앱은 새 버전을 *확인*은 하지만 [업데이트]를 누르면 **"업데이트 서명 키가 설정되지 않았습니다. 이 빌드는 업데이트를 설치할 수 없습니다"** 오류가 되고, 설치본을 내려받지도 설치를 시작하지도 않는다(fail-closed).
**그래서 공개 키를 넣기 전에는 릴리스를 발행할 수 없다** — 릴리스 워크플로(`release.yml`)의 preflight 가 `UPDATE_PUBKEY` 가 `None`(또는 형식이 틀리거나 시험용 키)이면 빌드·태그 전에 실패한다(`scripts/check-release-preflight.mjs`). 키 없는 빌드가 발행되면 그 앱을 받은 사용자는 앱 안에서 영영 업데이트할 수 없기 때문이다.

키를 넣는 방법:
1. `pnpm tauri signer generate -w <오프라인 경로>`(비밀번호 있는 키 권장 — 계획 U1)로 키를 만든다. 개인 키와 비밀번호는 GitHub Environment `release` 의 `TAURI_SIGNING_PRIVATE_KEY`·`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 로만 넣는다([build.md](build.md#설치본-빌드--tauri-packageyml)).
2. 만들어진 `<키>.pub` 의 **내용(base64 한 줄)** 을 `UPDATE_PUBKEY` 에 넣는다: `pub const UPDATE_PUBKEY: Option<&str> = Some("dW50cnVzdGVk…");`. 이 한 줄이 **믿는 키의 유일한 원천**이다 — `tauri.conf.json` 에 `plugins.updater` 를 넣지 않는다(시험이 거부한다. 플러그인 설정은 `lib.rs` 가 이 상수로 만든다).
3. `cargo test --features test-hooks --test update_security`·`pnpm exec vitest run tests/releaseScripts.test.ts tests/releaseFeatureGuard.test.ts` 를 돌린다(키 형식을 본다).
4. 개인 키가 그 공개 키의 쌍인지는 사람이 확인한다: 리허설에서 rc.1 → rc.2 자기 업데이트가 끝까지 되는지 본다([tauri.md](tauri.md) "PR 7"). 어긋나면 모든 업데이트가 "업데이트 서명이 올바르지 않습니다"로 거부된다.

**공개 키를 넣은 뒤에는 바꿀 수 없다**: 키는 모든 배포 바이너리에 박힌다. 잃으면 기존 설치본은 앱 안에서 영영 업데이트되지 않는다(교체하려면 옛 키로 서명한 "새 공개 키 판"을 먼저 내야 한다).

## Electron 판(v1.x)에서 올라오기

v2.0.0 은 Electron 판을 대체한다([release-notes/v2.0.0.md](release-notes/v2.0.0.md)). 이관을 위해 릴리스에 Electron 판용 `latest.yml` 을 함께 올리고(저장소 변수 `ELECTRON_MIGRATION` 이 꺼져 있지 않을 때), Tauri 설치기의 NSIS 훅(`src-tauri/nsis/hooks.nsh`)이 사용자별로 설치된 Electron 판을 제거한다.

- **Windows NSIS(사용자별 설치)**: Electron 판의 업데이트 확인이 `latest.yml`(sha512 검증)로 Tauri 설치본을 받아 실행하면 설치 훅이 Electron 판을 지운다(앱 데이터 `%APPDATA%\Antler` 는 남긴다). 모든 사용자용(Program Files) 설치는 건드리지 않는다 — 안내 창은 있지만 `/SD IDOK` 라 **조용한 설치(Electron 판의 자동 업데이트)에서는 뜨지 않으므로** 사용자는 노트의 안내로 직접 제거해야 한다. 이 경로의 무결성은 Electron 이 읽는 `latest.yml` 의 sha512 다 — Electron 쪽 코드라 바꿀 수 없다.
- **그 밖의 설치 형태**(Electron 판의 마지막 소스 `9e4f804:src/main/update/{manual,logic}.ts` 를 읽은 결과 — 실제 동작은 리허설에서 확인): portable 은 `Antler-Portable-<버전>.exe` 를, AppImage 는 `latest-linux.yml` 을 찾지 못해 v2.0.0 을 알리지 못한다(자동 확인은 조용히 넘기고 수동 확인만 오류를 보인다). macOS dmg·Linux deb 는 이전 판의 수동 경로가 같은 이름(`Antler-<버전>-<arm64|x64>.dmg`·`antler_<버전>_amd64.deb`)의 자산을 집고, `latest-mac.yml`·`latest-linux.yml` 이 없으므로 GitHub API 의 sha256 digest 로 검증한다(digest 도 없으면 받지 않는다). 즉 dmg·deb 사용자는 알림을 받을 *수 있다* — 아직 확인하지 못했다.
- **중단 장치**: 이관이 깨진 것이 발행 뒤에 드러나면 저장소 변수 `ELECTRON_MIGRATION=off` 로 이후 릴리스의 `latest.yml` 생성을 끄고 **현재 릴리스의 `latest.yml` 자산을 지운다**(둘 다 사람의 외부 쓰기). Electron 의 업데이트 확인이 조용히 멈추고 Tauri 사용자의 `latest.json` 은 영향받지 않는다.
- **되돌리기**: Electron 1.12.x NSIS 설치본을 이전 릴리스에서 받아 다시 설치하면 `%APPDATA%\Antler` 그대로 돈다(Tauri 판은 그 파일을 쓰지 않는다).
