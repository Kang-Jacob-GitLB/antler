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
