# 보안 설계

[← README](../README.md)

읽기 전용 앱이지만 공격자가 내용을 정할 수 있는 저장소를 연다는 전제로 설계했다.

- 창: contextIsolation · sandbox · nodeIntegration off, preload 는 허용 목록 채널만 노출, 권한 요청은 모두 거부,
  프로덕션 빌드에 CSP 메타 태그 주입, 외부 링크는 기본 브라우저로. 외부 HTML 은 sanitize 로 정화한다.
- IPC: main 이 모든 입력(경로·해시·ref 등)을 검증한다.
- 악성 저장소 방어: 모든 git 호출에 `GIT_NO_LAZY_FETCH=1`, `GIT_ALLOW_PROTOCOL=none`, `-c core.fsmonitor=false`, `-c core.protectNTFS=true` 를 줘
  저장소 설정이 네트워크 접근이나 외부 명령 실행을 일으키지 못하게 한다.
- fetch: 사용자 저장소에 하는 유일한 쓰기이자 유일한 git 네트워크 접근이다 — GitHub 원격 전용 저장소는 아래 별도 절을 본다(remote 마다 `git fetch --prune --no-prune-tags --no-recurse-submodules --no-write-fetch-head --refmap= -- <이름> +refs/heads/*:refs/remotes/<이름>/*`).
  **로컬 ref 는 쓰지 않는다**: 설정의 `remote.<n>.fetch` 를 쓰는 `fetch --all` 은 목적지를 설정이 정해 `refs/heads`·`refs/stash`·`refs/replace`·`HEAD` 를 덮어쓰고
  `--prune` 이 로컬 브랜치를, `prunetags` 가 태그를 지울 수 있었다(실측). 그래서 목적지를 `refs/remotes/<이름>/` 로 고정한 refspec 을 명령줄에 주고, 설정으로 하는
  "기회적 원격 추적 갱신"은 `--refmap=`(빈 값)으로 끈다(이게 없으면 명령줄 refspec 을 줘도 설정이 고른 목적지에 쓴다 — git 2.49 실측). 태그는 새 것만 따라오고 지우거나 덮지 않는다.
  `GIT_NO_REPLACE_OBJECTS=1` 을 모든 git 호출에 줘서 `refs/replace` 로 위조한 이력을 보여 주지 않는다.
  활성 저장소를 N분마다(설정 `ui.autoFetchMinutes`, 기본 5, 새로고침 옆 ▾ 메뉴에서 끔/1/5/15/30분) 자동으로, 새로고침 버튼을 누를 때도 가져온다.
  fetch 에만 `GIT_ALLOW_PROTOCOL=https:http:ssh:git`(ext·fd·원격 도우미 제외; `file` 은 수동 새로고침에서 git ≥ 2.45.1 일 때만, CVE-2024-32004),
  `GIT_TERMINAL_PROMPT=0`, 자격 증명 창 금지, `gc.auto=0`·`maintenance.auto=false`, NTLM/Negotiate 통합 인증 끄기
  (`http.emptyAuth=false`·`http.allowNTLMAuth=false`·`http.delegation=none`)를 준다. 저장소 훅은 `core.hooksPath` 를 빈 디렉터리로 돌려 막는다 —
  Windows 로컬 git 은 `/dev/null` 이 `<드라이브>:\dev\null` 로 풀려 공격자가 훅을 심을 수 있어서 앱이 소유한 `userData/no-hooks`(비어 있는지 매번 확인)를 쓴다.
  ssh 는 항상 `GIT_SSH_COMMAND` 를 명시해(저장소의 `core.sshCommand` 는 적용되지 않는다) 사용자의 시스템·전역 `core.sshCommand`,
  없으면 `GIT_SSH_COMMAND`/`GIT_SSH` 환경변수, 그것도 없으면 `BatchMode=yes`·`ConnectTimeout=15` 로 실행한다. 60초 제한, 저장소당 하나만, 환경당 전용 슬롯 하나.
  **악성 설정 차단(허용 목록)**: fetch 전에(그리고 실행 직전에 한 번 더) `git config --list --show-scope --includes -z` 로 local/worktree 설정(include 한 파일 포함)을 검사한다.
  git 과 Git Credential Manager 같은 도구가 명령을 실행하거나 연결을 열거나 파일을 쓰는 설정을 계속 추가해 차단 목록은 번번이 새 키를 놓쳤으므로,
  **읽기만 하는 데이터인 키만 통과시키는 허용 목록**을 쓴다(`src/main/git/fetch.ts`의 `ALLOWED_KEYS`: 저장소 형식·플래그 `core.*`, `remote.<n>.url|fetch|…`,
  `branch.<n>.remote|merge|…`, `user.*`, `pull/push/fetch.prune` 류, `extensions.*`, `submodule.<n>.url|active`, `include.path`, git-lfs 기본값 그대로인 `filter.lfs.*`).
  그 밖의 키(`http.*`·`credential.*`·`protocol.*`·`url.*`·`trace2.*`·`fetch.bundleURI`·`core.sshCommand` 등 전부)는 "알 수 없는 설정"으로 자동 fetch 를 막고
  "fetch 차단됨"으로 표시한다. 새로고침을 누르면 해당 키·값을 이스케이프해서(제어·서식·방향·결합·닮은꼴 따옴표·공백처럼 보이는 문자를 \uXXXX 로 노출, 긴 값은 앞…뒤+길이) 보여 주는
  확인 창이 뜨고, 항목이 12개를 넘으면 허용 버튼 없이 차단만 알린다. 허용해도 이번 실행 동안 같은 설정에만 적용된다.
  허용 목록 외에도 fetch 명령에는 `-c fetch.bundleURI= -c transfer.bundleURI=false -c http.followRedirects=initial` 등을 덮어써 이중으로 막는다.
  (renderer 가 `manual=true` 로 호출하면 확인 창이 뜰 수 있다 — 침해된 renderer 가 수동 fetch 를 요청할 수는 있지만, 알 수 없는 설정의 허용은 항상 main 이 띄우는 확인 창을 사용자가 눌러야 한다.)
  모든 remote 의 실제 URL(`git remote get-url --all`, 옛 `.git/remotes`·`.git/branches`, submodule URL 포함)이 `ext::`·`fd::`·`<도우미>::`·`-` 로 시작하거나
  UNC·네트워크 경로(`//host/…`, `\\host\share`, `\\?\UNC\…`, 호스트가 있는 `file://host/…`)면 확인 없이 거부한다(Windows 가 NTLM 을 보내므로 모든 플랫폼에서).
  userinfo 가 든 http(s) remote URL(`http://user@host/`)은 NTLM/Negotiate 가 새므로 자동 fetch 를 막고 수동 확인을 받는다(비밀번호는 가려 표시).
  **자격 증명 도우미**: 자동 fetch 는 `-c credential.helper=` 로 도우미를 끄고, 사용자가 같은 remote(이름+실제 URL)를 수동 새로고침으로 한 번 성공시킨 뒤에만
  도우미를 쓴다(승인은 main 이 설정 파일의 `fetchApprovals` 에 저장하며 renderer 는 바꿀 수 없다, URL 이 바뀌면 사라진다). 그래서 비공개 https 저장소는
  "새로고침을 한 번 누르면 이후 자동으로 가져옵니다".
  인증 실패·허용 안 된 전송은 수동 새로고침 전까지 자동 fetch 를 멈추고, 네트워크 실패는 지수 백오프(최대 30분)로 다시 시도한다.
  git 프로세스의 stderr 는 4MB 까지만 담는다.
  **알려진 한계(fetch 가드로 막을 수 없는 것)**:
  (1) 저장소의 `include.path`·`core.worktree` 가 UNC 경로를 가리키면 설정 목록을 읽는 것을 포함해 **어떤 git 명령이든** 그 경로를 건드린다 —
  공격자가 설정을 정할 수 있는 저장소를 여는 일반적인 한계다(Antler 는 네트워크 공유 저장소 경로 자체는 거부한다).
  (2) 사용자가 "알 수 없는 설정" 확인 창에서 허용하면, 저장소의 `http.<url>.*` 같은 키가 우리가 `-c` 로 준 기본값을 덮을 수 있다 — 허용은 사용자의 결정이다.
  (3) renderer 가 주장하는 `manual` 값은 main 이 검증할 수 없다. 침해된 renderer 가 수동 fetch 를 요청해도 알 수 없는 설정은 main 의 확인 창에서 사용자가 눌러야 한다.
  **Tauri 판**: local·WSL 저장소의 fetch 가드(허용 목록·URL 판정·상태 기계)는 Rust(`src-tauri/src/git/fetch/`)에 있고, 사이드카의 FetchService 는 그 저장소를 맡지 않는다(이중 fetch 없음).
  확인 창은 webview 가 아니라 Rust 네이티브 대화상자(`dialogs::message_box`)이며 renderer 에는 그것을 띄우거나 누를 경로가 없다. 승인은 Rust 가 사이드카에 부탁해(`fetchApprove`) 설정 파일에 남기고, renderer 의 값은 그 경로로 가지 않는다 — 자세한 것은 docs/tauri.md "fetch 라우팅".
- 코드 탐색기: 저장소 문서(README 등)의 외부 링크는 사용자가 누를 때만 `code:openLink` 로 연다 — https 만, 사용자 정보가 든 URL 은 거부, 마우스를 올리면 실제 주소를 보여 준다.
- 경로·diff: `GIT_LITERAL_PATHSPECS=1` 이라 renderer 가 보낸 경로가 pathspec 매직으로 해석되지 않고,
  `--no-textconv`·`--no-ext-diff` 로 저장소 설정이 외부 프로그램을 실행하지 못하게 한다.
- 추적 안 되는 파일 diff(`git:wipDiff`, area `untracked`): `--no-index` 는 파일을 직접 열기 때문에 **허용 목록**만 받는다. main(RepoService)이 저장소별 마지막 `wipStatus` 가
  git 출력(`-uall`)으로 보고한 경로를 보관하고, 정확히 일치하는 경로만 연다(wipStatus 전이거나 목록에 없으면 거부, 저장소 제거 시 정리).
  `.git` 을 문자열로 비교하는 검사만으로는 Windows NTFS 8.3 짧은 이름(`GIT~1/config`)으로 `.git/config` 를 읽을 수 있어서다. 접두사 허용은 두지 않는다.
  `--no-index` 가 종료 코드 1 이면서 출력이 없으면 "차이 있음"이 아니라 파일을 못 연 오류로 올린다.
- 작업 트리 diff 의 정션·symlink 우회: Git for Windows 의 `status -uall` 은 `mklink /J j .git` 정션을 따라 들어가 `j/config` 를 untracked 로 보고하고 `--no-index` 가 `.git/config` 를 출력한다
  (저장소 밖을 가리키는 정션도 같다). 그래서 **로컬 실행기**(`exec.kind === 'local'`)는 untracked·unstaged diff 를 열기 직전에(`src/main/git/linkGuard.ts`)
  저장소 루트부터 구성 요소마다 `lstat` 해 symlink·정션(Node 는 정션도 `isSymbolicLink` 로 보고 — 시험으로 확인)이면 거부하고(untracked 는 파일 자체가 링크여도 거부, unstaged 는 추적 중인 링크 파일만 허용),
  `realpath.native`(8.3 이름이 펴진다)가 저장소 루트 실제 경로 안이며 `.git` 구성 요소가 없는지 확인한다. 허용 목록을 만들 때(wipStatus)는 파일마다 lstat 하지 않는다(파일 수천 개 비용) — diff 시점 검사가 방어선이다.
  unstaged 는 추가로 `~숫자` 로 끝나는 구성 요소(8.3 모양)를 거부한다(조작된 index 의 `GIT~1/config`; 정상 파일 `foo~1` 도 막히는 오탐을 허용). NTFS 는 끝의 `.`·공백을 떼므로 `GIT~1.`·`GIT~1 ` 도 같이 거부한다(`hasShortNameSegment`).
- 작업 트리 status·diff 의 filter 드라이버 무력화(`src/main/git/wip.ts` `filterOverrides`): 모든 드라이버를 이번 실행에서만 `-c filter.<이름>.clean=`(smudge·process 도, `required=false`)로 비운다. **git-lfs 도 예외가 아니다**(값 비교로 기본 명령을 통과시키던 예외를 없앴다).
  git-lfs 의 clean 은 기본 명령 그대로여도 저장소 설정의 `lfs.extension.<이름>.clean` 명령을 실행하고 `.git/lfs/objects` 에 객체를 **쓴다**(`lfs.storage` 로 쓰기 위치도 바꿀 수 있다) —
  저장소를 열기만 해도(repoExtras) 임의 명령 실행과 쓰기가 가능해 읽기 전용 경계를 깬다(재현: git-lfs 3.6.1, `.gitattributes filter=lfs` + `lfs.extension.x.clean` 표지 명령). 그래서 git-lfs 프로세스는 아예 뜨지 않는다.
  **화면 영향**: clean 이 돌지 않으므로 stat 이 바뀐(내용은 같은) LFS 추적 파일은 포인터가 아닌 원본과 비교돼 "수정됨"으로 보일 수 있고, 그 diff 는 포인터가 아니라 실제 내용(바이너리면 바이너리)으로 나온다.
  체크아웃 직후 수정 없는 LFS 파일은 인덱스 stat 캐시 덕분에 수정됨으로 보이지 않는다(`tests/lfsFilterExemption.test.ts` 로 확인). repoExtras 의 LFS 판정(`lfs`)은 속성 파일 텍스트만 읽어 바뀌지 않는다.
  (fetch 가드의 `filter.lfs.*` 기본값 허용 목록은 별개다 — fetch 는 작업 트리를 읽지 않아 clean 을 돌지 않는다.)
  `git config -z --get-regexp '^filter\.'` 출력을 **바이트로** 받아 키를 UTF-8 로 엄격히 검증하고(접미사 판정도 JS 에서 — git 정규식의 로캘 의존 제거),
  **무력화할 수 없는 저장소는 작업 트리를 읽지 않고 거부한다**: 이름에 `=` 가 있음, 이름이 UTF-8 이 아님(`\xff` 등 — 문자열로 바꾸면 U+FFFD 가 돼 `-c` 가 실제 드라이버를 덮지 못하고,
  Windows 는 argv 가 UTF-16 이라 원래 바이트로 `-c` 를 만들 수도 없다. 재현: 악성 `.git/config` + `.gitattributes` 로 status 가 저장소가 심은 명령을 실행했다), 드라이버가 256개를 넘거나 `-c` 총 길이가 24000자를 넘음(Windows 명령줄 한계 32767자).
  WSL·SSH 실행기는 Node 가 그 파일 시스템을 볼 수 없어 이 검사를 하지 않는다. Linux git 의 status 는 디렉터리 symlink 를 따라 들어가지 않고 링크 자체를 한 항목으로 보고하므로 `j/config` 가 허용 목록에 들어오지 않는다고 본다 — **미확인**(실측하지 않았다).
  확인과 git 의 읽기 사이 경쟁(TOCTOU)은 막지 못한다. **알려진 한계**: 정션이 순환(`mklink /J a .`)하면 status 의 재귀가 길어지는 로컬 DoS 가 가능하다 — 이 가드의 범위 밖이다.
  **`git diff --no-index` 도 필터를 돈다**(실측, git 2.49): "저장소 속성을 읽지 않는다"는 전제는 틀렸다. `-C` 저장소의 설정에 `filter.<이름>.clean|process` 가 있고 `.gitattributes`·`.git/info/attributes` 규칙이 읽는 파일에 맞으면
  그 명령이 실행된다. 그래서 `--no-index` 를 쓰는 모든 호출에 같은 가드를 건다 — untracked diff(`wipFileDiff`, 모든 area 공통)와 LFS 판정용 파일 읽기(`repoExtras.ts` `fileUsesLfs`: 작업 트리 `.gitattributes`·`info/attributes`, 저장소를 열 때마다 돈다.
  가드 조회는 저장소당 한 번). 무력화할 수 없으면 wip 는 오류로 거부하고, repoExtras 는 그 파일을 읽지 않고 기존 실패 처리(`lfs: false`)와 같게 둔다.
  작업 트리를 읽지 않는 호출(`blame`·`cat-file blob`·`ls-files`·`stash list`·`check-attr`·`ls-tree`)은 필터를 돌리지 않아 가드 대상이 아니다.
- 서브모듈 상태 조회: 인덱스의 gitlink 경로가 절대 경로·드라이브 문자로 시작하거나 `..`·`.`·빈 구성 요소를 가지면 `git -C` 를 돌리지 않고 `unknown` 으로 둔다(저장소 밖 HEAD 를 읽지 않게). 로컬 실행기는 추가로 `realpath.native(저장소/경로)` 가 저장소 루트 안(`.git` 아님)인지 확인해 정션·symlink 로 밖을 가리키면 `unknown` 이다(없는 경로는 git 이 미초기화로 판정).

**Tauri 판의 Jira·GitHub**: `jira:*`·`gh:*`(`gh:repo` 제외)는 Rust(`src-tauri/src/api_host.rs` → `jira/service.rs`·`github/service.rs`)가 처리하고 토큰은 Windows 자격 증명 관리자에만 둔다(`ANTLER_USER_DATA` 로 도는 자동화 실행은 메모리).
Jira 토큰은 Rust 프로세스 밖(renderer·사이드카)으로 나가지 않는다 — 사이드카는 `ANTLER_RUST_API=1` 이면 `jira:`·`gh:`(`gh:repo` 제외)를 처리하지 않는다.
GitHub 토큰은 renderer 로 가지 않고, 사이드카에 남는 원격 전용 저장소의 git fetch 인증용 **메모리 사본**만 사이드카 stdin(`ghToken`, `ANTLER_RUST_API=1` 일 때만)으로 간다 — env·인자·로그에는 싣지 않는다. HTTP 는 `https` 만(루프백 http 는 테스트 빌드에서만), 프록시 환경 변수를 따르지 않고
리다이렉트를 따라가지 않으며, `jira:openExternal` 은 연결한 사이트의 https 주소만, `gh:openExternal` 은 `https://github.com` 의 주소만 연다. 자세한 것은 docs/tauri.md "PR 6"·"PR 9".

**Tauri 판의 SSH**(`src-tauri/src/git/exec/ssh/`, `russh`): Electron 판(`ssh2` + 자체 설정 파서)과 같은 규칙을 Rust 로 옮겼다. 호스트 키는 known_hosts 로만 검증한다(TOFU 없음 — 모르는 호스트·키가 바뀐 호스트·폐기된 키는 거부하고
ssh CLI 로 직접 수락하라고 안내한다. ProxyJump 는 홉마다 검증). `ProxyCommand`·`ProxyCommand` 가 걸리는 `Match` 는 설정을 읽는 단계에서 거부한다. 연결 뒤 원격 셸이 POSIX 인지 확인하는 명령(`echo 'an''tler'`)이 통과해야 사용자 값이 든 명령을 보낸다(Windows `cmd` 서버는 거부).
인증은 ssh-agent 와 암호 없는 키 파일뿐이다(password·keyboard-interactive·known_hosts 쓰기·포트 포워딩 API 는 소스 검사가 허용된 파일 밖의 사용을 막는다). **서버가 먼저 여는 채널은 종류를 가리지 않고 모두 거부한다**(agent·X11·forwarded-tcpip·direct 등 7종).
원격으로 가는 명령은 `build_remote_command`(모든 인자를 작은따옴표로 인용, 환경은 `GIT_ENV` 와 닫힌 `FetchEnv` 뿐) 하나로만 만든다 — 임의 명령을 보낼 길이 없다.
**ssh 저장소의 fetch**: 위 fetch 규칙(refspec 고정·`--refmap=`·허용 목록·훅 차단·원격 계정의 사용자 `core.sshCommand` 또는 `BatchMode` 기본)이 원격에서도 그대로이고, 훅 경로는 `/dev/null` 이다. 이 PC 의 `GIT_SSH_COMMAND`·`GIT_SSH` 는 원격으로 가지 않는다.
SSH 전송 오류(연결·호스트 키·인증·셸 확인 실패)는 문구가 아니라 오류 종류로 network(백오프)로 분류한다 — 호스트 키 불일치가 "인증 일시 정지"로 오분류되지 않는다. 원격 git 이 낸 인증 오류는 문구로 auth 다.
호스트가 미러에서 빠지면 연결과 fetch 서비스의 실행기를 함께 닫는다. 연결 확인 창은 저장소를 **추가할 때만** 뜨고(사이드카 몫), fetch 중에는 원격 설정의 위험 항목에 대한 확인 창(local·WSL 과 같은 것)만 뜬다. 자세한 것은 docs/tauri.md "SSH(Phase 5)".

## GitHub 원격 전용 저장소의 예외

저장소 추가 대화상자의 GitHub 탭으로 추가한 저장소는 작업 폴더 없이 앱 캐시
(`<userData>/remote-repos/<owner>/<repo>.git`, bare + `blob:none` partial)에 이력만 받는다(`src/main/git/githubCache.ts`).
위 규칙과 의도적으로 다른 점은 셋이며, 모두 이 캐시 저장소에만 적용된다.

1. **토큰이 GitHub 서비스 밖으로 나가는 유일한 경로** — 이 저장소의 git 에만 `GIT_CONFIG_COUNT` 환경 변수로
   `http.https://github.com/.extraheader` 를 넘긴다. 명령줄 인자나 디스크에는 쓰지 않는다.
2. lazy fetch(`GIT_NO_LAZY_FETCH=0`)와 `GIT_ALLOW_PROTOCOL=https` 를 허용한다 — 필요한 blob 을 그때그때 받아야 해서다.
3. 로컬 설정은 일반 허용 목록(`isAllowedConfig`)이 아니라 기대값과 정확히 일치하는지(`checkGithubConfig`)로 검사한다
   (`core.bare=true` 는 여기서만 허용). 사용자 전역·시스템 git 설정은 읽지 않고 프록시·인증서 키만 골라 옮긴다.
