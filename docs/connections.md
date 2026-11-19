# 연결 가이드 — GitHub · Jira · WSL · SSH

[← README](../README.md)

## GitHub 로그인

툴바의 GitHub 버튼에서 로그인한다. 로그인 없이도 공개 저장소는 읽을 수 있다(비인증 시간당 60회 제한).

- **PAT**: 읽기 전용 fine-grained 토큰을 권장한다(Contents·Pull requests·Issues 읽기). classic 은 `repo` 범위.
- **Device Flow**: GitHub OAuth App 을 만들어 **Enable Device Flow** 를 켜고, 그 client ID 를 GitHub 버튼 → **고급**에 입력한다.
- **GitHub CLI 로그인 가져오기**: 터미널에서 `gh auth login` 해 둔 계정을 그대로 쓴다.
- **토큰 보관**: OS 키체인(Windows 자격 증명 관리자·macOS 로그인 키체인·Linux Secret Service — GNOME Keyring·KWallet)의 `com.antler.app` 에 비밀과 메타를 한 덩어리로 두고,
  키링을 열 수 없거나(Linux 에 D-Bus·Secret Service 없음) 쓰지 못하면 디스크에 쓰지 않고 메모리에만 둔다(앱을 끄면 다시 로그인). 토큰은 Rust 프로세스만 쥐고 renderer 로 보내지 않는다([docs/tauri.md](tauri.md) PR 4·Phase 6b PR 9·Phase 6c PR 5).
  **Electron 판(v1.x)에서 올라온 경우 토큰은 가져오지 않는다**(Electron `safeStorage` 로 암호화된 파일은 Tauri 판이 풀 수 없다) — GitHub·Jira 에 다시 로그인한다([v2.0.0 노트](release-notes/v2.0.0.md)).
- GitHub Enterprise 는 지원하지 않는다.

## Jira 연결

툴바의 Jira 버튼 → **연결**(또는 명령 팔레트의 "Jira 연결 설정").

- **Jira Cloud**: 사이트 주소(https) + Atlassian 계정 이메일 + API 토큰(id.atlassian.com → 보안 → API 토큰).
- **Data Center / Server**: 사이트 주소 + Personal Access Token(Jira 프로필 → 개인용 액세스 토큰).
- **프로젝트 키(선택)**: 쉼표로 구분한다. 비우면 `ABC-123` 형태를 모두 이슈 키로 보고 커밋 메시지·PR 의 키를 이슈로 잇는다.
- 토큰은 앱의 Rust 프로세스에만 보관하며(renderer 로 넘기지 않는다) 화면·로그에 나오지 않는다. 조회와 **이슈 상태 전환**에만 쓴다(워크로그 입력·수정은 하지 않는다).
- Cloud OAuth(3LO)는 지원하지 않는다.

## 원격 환경

**WSL** (Windows 전용)
- 배포판 목록은 `wsl -l -q` 로 읽는다.
- 경로는 `/home/…` 또는 `~/…`. `\\wsl.localhost\<배포판>\…` 경로는 WSL 저장소로 자동 변환한다.

**SSH**
- `~/.ssh/config` 의 `Host`·`HostName`·`User`·`Port`·`IdentityFile`·`ProxyJump`·`Include`·`UserKnownHostsFile`·`HostKeyAlias` 를 따른다.
- 인증은 키 파일 또는 ssh-agent 만. 비밀번호는 저장하지도 묻지도 않는다.
- 호스트 키가 이미 `known_hosts` 에 있어야 한다 — 처음이면 ssh CLI 로 한 번 접속해 둔다.
- 원격 로그인 셸은 POSIX 계열(sh/bash/zsh/fish)이어야 한다.
- `ProxyCommand` 와 호스트에 걸리는 `Match` 블록은 무시하지 않고 **거부**한다(잘못된 곳에 접속하는 것을 막기 위해).
