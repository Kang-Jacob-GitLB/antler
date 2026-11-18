# 보안 설계

[← README](../README.md)

읽기 전용 앱이지만 공격자가 내용을 정할 수 있는 저장소를 연다는 전제로 설계했다.

- 창: contextIsolation · sandbox · nodeIntegration off, preload 는 허용 목록 채널만 노출, 권한 요청은 모두 거부,
  프로덕션 빌드에 CSP 메타 태그 주입, 외부 링크는 기본 브라우저로. 외부 HTML 은 sanitize 로 정화한다.
- IPC: main 이 모든 입력(경로·해시·ref 등)을 검증한다.
- 악성 저장소 방어: 모든 git 호출에 `GIT_NO_LAZY_FETCH=1`, `GIT_ALLOW_PROTOCOL=none`, `-c core.fsmonitor=false`, `-c core.protectNTFS=true`, `-c diff.submodule=short`, `-c submodule.recurse=false` 를 줘
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
  체크아웃 직후 수정 없는 LFS 파일은 인덱스 stat 캐시 덕분에 수정됨으로 보이지 않는다(`tests/lfsFilterExemption.test.ts` 로 확인). 단 파일과 인덱스가 같은 타임스탬프 단위에 쓰인 직후(racy-git)는 git 이 내용을 다시 해시해 수정됨으로 보일 수 있다. repoExtras 의 LFS 판정(`lfs`)은 속성 파일 텍스트만 읽어 바뀌지 않는다.
  (fetch 가드의 `filter.lfs.*` 기본값 허용 목록은 별개다 — fetch 는 작업 트리를 읽지 않아 clean 을 돌지 않는다.)
  `git config -z --get-regexp '^filter\.'` 출력을 **바이트로** 받아 키를 UTF-8 로 엄격히 검증하고(접미사 판정도 JS 에서 — git 정규식의 로캘 의존 제거),
  **무력화할 수 없는 저장소는 작업 트리를 읽지 않고 거부한다**: 이름이 비었음(`[filter ""]` + `.git/info/attributes` 의 `* filter=` 가 빈 이름 드라이버의 clean 을 실행한다 — 실측.
  `-c filter..clean=` 로 덮을 수는 있지만 정상 저장소에 빈 이름은 없어 거부로 단순화했다. TS·Rust 가 같다), 이름에 `=` 가 있음, 이름이 UTF-8 이 아님(`\xff` 등 — 문자열로 바꾸면 U+FFFD 가 돼 `-c` 가 실제 드라이버를 덮지 못하고,
  Windows 는 argv 가 UTF-16 이라 원래 바이트로 `-c` 를 만들 수도 없다. 재현: 악성 `.git/config` + `.gitattributes` 로 status 가 저장소가 심은 명령을 실행했다), 드라이버가 256개를 넘거나 `-c` 총 길이가 24000자를 넘음(Windows 명령줄 한계 32767자).
  WSL·SSH 실행기는 Node 가 그 파일 시스템을 볼 수 없어 이 검사를 하지 않는다. Linux git 의 status 는 디렉터리 symlink 를 따라 들어가지 않고 링크 자체를 한 항목으로 보고하므로 `j/config` 가 허용 목록에 들어오지 않는다고 본다 — **미확인**(실측하지 않았다).
  확인과 git 의 읽기 사이 경쟁(TOCTOU)은 막지 못한다. **알려진 한계**: 정션이 순환(`mklink /J a .`)하면 status 의 재귀가 길어지는 로컬 DoS 가 가능하다 — 이 가드의 범위 밖이다.
  **`git diff --no-index` 도 필터를 돈다**(실측, git 2.49): "저장소 속성을 읽지 않는다"는 전제는 틀렸다. `-C` 저장소의 설정에 `filter.<이름>.clean|process` 가 있고 `.gitattributes`·`.git/info/attributes` 규칙이 읽는 파일에 맞으면
  그 명령이 실행된다. 그래서 `--no-index` 를 쓰는 모든 호출에 같은 가드를 건다 — untracked diff(`wipFileDiff`, 모든 area 공통)와 LFS 판정용 파일 읽기(`repoExtras.ts` `fileUsesLfs`: 작업 트리 `.gitattributes`·`info/attributes`, 저장소를 열 때마다 돈다.
  가드 조회는 저장소당 한 번). 무력화할 수 없으면 wip 는 오류로 거부하고, repoExtras 는 그 파일을 읽지 않고 기존 실패 처리(`lfs: false`)와 같게 둔다. Tauri 판의 `repoExtras`(Rust `lfs.rs` `read_worktree_attributes`)도 `wip::filter_overrides`·`FilterGuard` 로 같은 가드를 건다(가드 없이는 `--no-index` argv 를 만들 수 없는 시그니처 — `docs/tauri.md` PR 5). Rust 의 `wipStatus`·`wipDiff`(`git/wip.rs` — status·staged·unstaged·untracked 모두)도 같은 가드를 전역 옵션 자리에 건다: argv 빌더가 `&FilterGuard` 를 필수 인자로 받아 가드 없이는 만들 수 없고, 소스 검사 시험이 `-c` 인자를 만드는 제품 코드를 `wip.rs`·상수만 쓰는 `exec/common.rs`·`fetch/guard.rs` 로 한정한다(`docs/tauri.md` PR 6). untracked 허용 목록은 `Router::untracked_allow`(저장소가 미러에서 빠지거나 대상이 바뀌면 지운다)가 보관하고, 링크 확인(`link_guard`)은 `.`·`..`·Windows 의 `:` 구성 요소를 TS 보다 엄격히 거부한다.
  **WIP staged·unstaged diff 는 plumbing 이다**(`wip.ts` `wipFileDiff`: `diff-index -p --cached <HEAD|빈 트리>`·`diff-files -p`). porcelain `git diff` 는 실측으로 두 가지가 샜다.
  (1) stat 만 바뀐 파일이 있으면 index 를 갱신해 **쓰고** `.git/hooks/post-index-change` 를 **실행**한다(`GIT_OPTIONAL_LOCKS=0` 이어도). plumbing 은 refresh 를 하지 않는다.
  (2) 상위 저장소 `diff.submodule=diff` + 인덱스에는 파일이고 작업 트리에는 중첩 저장소인 경로(status 가 `1 .T` 로 보고)면 서브모듈 안에서 자식 git diff 를 `--no-ext-diff` 없이 띄워 그 저장소의 `diff.external` 이 실행된다.
  plumbing 은 `diff.submodule`·`diff.external` 같은 ui 설정을 읽지 않는다. 그래도 `--no-ext-diff --no-textconv --ignore-submodules=all --submodule=short` 를 명시하고 filter 가드 `-c` 도 그대로 건다.
  출력 차이: 헤더·hunk 형식은 같다. stat 만 바뀐 파일(내용 동일)은 refresh 를 안 해 hunk 없이 나오므로 UI 에서 빈 diff 다. 커밋이 없는 저장소는 `HEAD` 대신 빈 트리(SHA-1·SHA-256 중 저장소가 아는 쪽)와 비교한다.
  untracked(`diff --no-index`)는 index 를 읽지 않아 porcelain 그대로다. Rust 의 wip diff argv(`git/wip.rs`)도 같은 plumbing 인자·같은 staged 기준 결정을 쓴다(`docs/tauri.md` PR 6).
  전역 인자에도 `-c diff.submodule=short`·`-c submodule.recurse=false` 를 더했다(`GIT_GLOBAL_ARGS`, Rust `exec/common.rs`) — 어떤 호출이 porcelain diff 를 부르더라도 서브모듈 재귀가 켜지지 않는 방어 심층이다.
  작업 트리를 읽지 않는 호출(`blame`·`cat-file blob`·`ls-files`·`stash list`·`check-attr`·`ls-tree`)은 필터를 돌리지 않아 가드 대상이 아니다.
- **원격 UNC 를 가리키는 링크는 따라가기 전에 거부한다**(`src/main/git/linkGuard.ts` `isRemoteLinkTarget`·`linksStayLocal`): 작업 트리 안의 심볼릭 링크·정션이 `\\host\share\…`·`//host/…`·`\\?\UNC\…`·`\??\UNC\…`·
  WebDAV `\\host@SSL\DavWWWRoot\…`·장치 경로(`\\.\…`)·볼륨 GUID(`\\?\Volume{…}`)·`\??\GLOBALROOT\Device\Mup\…`·`\??\Device\…` 를 가리키면, `realpath`·`stat` 처럼 링크를 따라가는 호출이 나가는 순간 Windows 가 SMB/WebDAV 인증을 시도해
  NTLM 해시가 공격자 서버로 샌다 — 이후 "저장소 밖"이라 거부해도 이미 늦다. 그래서 **따라가기 전에** `linksStayLocal(path)` 가 경로를 루트부터 구성 요소 단위로 `lstat` 하며 링크마다 `readlink` 대상 문자열만 판정한다
  (대상에는 접속하지 않는다). 원격이면 거부, 로컬 대상은 그 경로도 같은 방식으로 이어서 확인하고, 링크 수가 합쳐 16 을 넘으면 거부한다. 판정은 **허용 목록**이다(차단 목록은 `GLOBALROOT` 같은 NT 장치 경로로 우회돼 폐기): 로컬은 드라이브 경로(`C:\`·`C:/`·`\\?\C:\`·`\??\C:\`)·상대 경로·단일 `/` 시작 경로·WSL 공유(`\\wsl.localhost\…`·`\\wsl$\…`)뿐이고,
  나머지(`\\…`·`//…`, 드라이브가 아닌 `\??\…`·`\\?\…`)는 전부 원격으로 본다. 중간 링크(`b -> a\x`, `a -> 원격`)도 구성 요소 단위로 a 에서 멈춘다.
  적용 지점: `assertInsideRoot`·`assertNoLinkEscape`(서브모듈 확인·WIP 열기 전), 저장소 스캔(`repoScan.ts` — 원격 대상 링크는 폴더·`.git` 어느 쪽이든 건너뛴다), 외부 열기(`opener.ts` `inspect`, 로컬만 — WSL 은 리눅스 쪽 링크라 Node 가 볼 수 없다).
  한계: 확인과 사용 사이 경쟁(TOCTOU)은 막지 못한다.
  **Rust 판은 공용 모듈 `src-tauri/src/fsguard.rs` 한 곳에 둔다**(외부 열기·폴더 선택·폴더 찾기·환경 채널이 모두 이것을 쓰고, 소스 검사 `tests/external.rs` 가 다른 곳의 중복 구현을 막는다).
  `is_remote_link_target` 은 **허용 목록**이다 — 로컬로 인정하는 것만 나열한다: 드라이브 문자 경로(`X:\`·`X:/`·`\\?\X:\`·`\??\X:\`)·상대 경로·WSL 공유(`\\wsl.localhost\…`·`\\wsl$\…`).
  그 밖의 `\\…`·`//…`·`\??\…`·`\\?\…`·`\\.\…`(`GLOBALROOT\Device\Mup\…`·`Device\…`·`Volume{…}` 포함)는 전부 원격이다 — 예전에 `\??\`·`\\?\` 뒤가 `UNC\`·`Volume{` 이 아니면 로컬로 보던
  차단 목록식 판정은 `\??\GLOBALROOT\Device\Mup\host\share` 로 SMB 접속을 열어 주었다. `links_stay_local` 은 경로의 **구성 요소마다** `symlink_metadata` 로 링크를 보고 대상만 판정한다
  (`b -> a\x`, `a -> \\evil\share` 처럼 중간 구성 요소의 링크도 잡는다. 사슬 16 초과 거부). 적용 지점: 외부 열기 `realpath`, 폴더 선택 시작 위치, 폴더 찾기(`repos:scan`)의 하위 항목,
  그리고 `repos:scan` 의 **루트**(local 종류, Windows) — `git -C <루트>` 와 `read_dir` 가 루트·중간 링크를 따라가기 전에 `check_local_root` 로 거른다("네트워크 공유 경로는 지원하지 않습니다(링크가 …)").
  TS 사례표 `tests/fixtures/repos/link-target.cases.json` 은 `fsguard` 단위 시험이 TS 와 함께 읽고, GLOBALROOT·Mup 같은 Rust 쪽 추가 사례는 `fsguard.rs` 시험에 따로 있다(TS 수정 PR 과 맞춘 뒤 사례표로 옮긴다).
- 서브모듈 상태 조회: 인덱스의 gitlink 경로가 절대 경로·드라이브 문자로 시작하거나 `..`·`.`·빈 구성 요소를 가지면 `git -C` 를 돌리지 않고 `unknown` 으로 둔다(저장소 밖 HEAD 를 읽지 않게). 로컬 실행기는 추가로 `realpath.native(저장소/경로)` 가 저장소 루트 안(`.git` 아님)인지 확인해 정션·symlink 로 밖을 가리키면 `unknown` 이다(없는 경로는 git 이 미초기화로 판정).

**Tauri 판의 Jira·GitHub**: `jira:*`·`gh:*`(`gh:repo` 제외)는 Rust(`src-tauri/src/api_host.rs` → `jira/service.rs`·`github/service.rs`)가 처리하고 토큰은 Windows 자격 증명 관리자에만 둔다(`ANTLER_USER_DATA` 로 도는 자동화 실행은 메모리).
Jira 토큰은 Rust 프로세스 밖(renderer·사이드카)으로 나가지 않는다 — 사이드카는 `ANTLER_RUST_API=1` 이면 `jira:`·`gh:`(`gh:repo` 제외)를 처리하지 않는다.
GitHub 토큰은 renderer 로도 **사이드카로도 가지 않는다**(Phase 6b PR 9 부터 사이드카로 보내던 메모리 사본 `ghToken` 줄이 없다 — 원격 전용 저장소의 fetch·조회 인증은 Rust 의 캐시 실행기가 환경 변수로만 쓴다). 하위 스위치 `ANTLER_RUST_API`·`ANTLER_RUST_SSH` 는 은퇴했고 `ANTLER_RUST_GIT=0` 만 6c 까지 산다. HTTP 는 `https` 만(루프백 http 는 테스트 빌드에서만), 프록시 환경 변수를 따르지 않고
리다이렉트를 따라가지 않으며, `jira:openExternal` 은 연결한 사이트의 https 주소만, `gh:openExternal` 은 `https://github.com` 의 주소만 연다. 자세한 것은 docs/tauri.md "PR 6"·"PR 9".

**Tauri 판의 SSH**(`src-tauri/src/git/exec/ssh/`, `russh`): Electron 판(`ssh2` + 자체 설정 파서)과 같은 규칙을 Rust 로 옮겼다. 호스트 키는 known_hosts 로만 검증한다(TOFU 없음 — 모르는 호스트·키가 바뀐 호스트·폐기된 키는 거부하고
ssh CLI 로 직접 수락하라고 안내한다. ProxyJump 는 홉마다 검증). `ProxyCommand`·`ProxyCommand` 가 걸리는 `Match` 는 설정을 읽는 단계에서 거부한다. 연결 뒤 원격 셸이 POSIX 인지 확인하는 명령(`echo 'an''tler'`)이 통과해야 사용자 값이 든 명령을 보낸다(Windows `cmd` 서버는 거부).
인증은 ssh-agent 와 암호 없는 키 파일뿐이다(password·keyboard-interactive·known_hosts 쓰기·포트 포워딩 API 는 소스 검사가 허용된 파일 밖의 사용을 막는다). **서버가 먼저 여는 채널은 종류를 가리지 않고 모두 거부한다**(agent·X11·forwarded-tcpip·direct 등 7종).
원격으로 가는 명령은 `build_remote_command`(모든 인자를 작은따옴표로 인용, 환경은 `GIT_ENV` 와 닫힌 `FetchEnv` 뿐) 하나로만 만든다 — 임의 명령을 보낼 길이 없다.
**SFTP 폴더 목록**(저장소 추가 창의 원격 폴더 탐색, `ssh/sftp.rs`): 채널의 `sftp` 하위 시스템으로 `realpath`·`opendir`·`readdir`·`stat`·`close` 만 보내 읽기만 하고, **셸 명령을 보내지 않는다**. 하위 시스템 요청은 `connection.rs` 의 한 곳에서만 하고(clippy `disallowed-methods` + 소스 검사),
쓰기 계열 SFTP 요청은 부르지 않는다. 서버가 주는 값은 믿지 않는다 — 패킷 256KiB·읽는 항목 20,000개·이름 1024바이트 상한, `/`·NUL 이 든 이름은 건너뛰고, `.`·`..` 만 되풀이하는 응답(연속 2회)과 빈 응답은 오류로 끝내고, 서버가 준 오류 문구는 제어·방향 서식 문자를 지우고 300자로 자르며, 이름순은 전순서라 악성 이름으로 정렬이 패닉하지 않고, `realpath` 결과는 절대 경로·NUL 없음·4096 이하여야 한다. 시간 제한(전체 30초)이 있고 읽지 않는 채널로 세션이 멈추지 않게 채널은 항상 비운다. 서버가 여는 채널 거부는 그대로다.
**ssh 저장소의 fetch**: 위 fetch 규칙(refspec 고정·`--refmap=`·허용 목록·훅 차단·원격 계정의 사용자 `core.sshCommand` 또는 `BatchMode` 기본)이 원격에서도 그대로이고, 훅 경로는 `/dev/null` 이다. 이 PC 의 `GIT_SSH_COMMAND`·`GIT_SSH` 는 원격으로 가지 않는다.
SSH 전송 오류(연결·호스트 키·인증·셸 확인 실패)는 문구가 아니라 오류 종류로 network(백오프)로 분류한다 — 호스트 키 불일치가 "인증 일시 정지"로 오분류되지 않는다. 원격 git 이 낸 인증 오류는 문구로 auth 다.
호스트가 미러에서 빠지면 연결과 fetch 서비스의 실행기를 함께 닫는다. 연결 확인 창은 저장소를 **추가하는 흐름에서만** 뜨고(`env:test`·`env:listDir`·`repos:scan` 은 Rust `env_host.rs` 가, `repos:add` 는 PR 9 전까지 사이드카가 묻는다 — 승인은 프로세스 메모리이고 취소가 기본 버튼이다. 모르는 호스트·바뀐 키는 이 흐름에서도 거부하고 지문과 `ssh <별칭>` 안내를 보이며 `known_hosts` 에 쓰지 않는다), fetch 중에는 원격 설정의 위험 항목에 대한 확인 창(local·WSL 과 같은 것)만 뜬다. 자세한 것은 docs/tauri.md "SSH(Phase 5)".

**저장소 감시**(Electron `RepoWatcher.ts`, Tauri `src-tauri/src/git/{watcher,fs_watch}.rs` — notify): 활성 저장소 하나의 `.git`(워크트리면 공통 디렉터리까지)만 재귀로 감시하고 변화는 **ref 시그니처가 바뀌었을 때만** 알린다(이벤트가 git 을 부르는 폭주가 되지 않게 400ms 디바운스·확인 사이 최소 2초·확인 제한 20초, 이벤트는 쌓지 않고 합친다).
`.git` 파일의 `gitdir`/`commondir` 은 저장소(공격자)가 정하므로 **UNC(네트워크)·NT 네임스페이스(`\??\UNC\…`) 경로이거나, 디렉터리·`HEAD` 가 링크이거나, `HEAD` 가 없는 디렉터리는 감시하지 않는다**(링크를 따라가지 않고 `lstat` 로 본다 — stat 만으로도 SMB 접속이 일어난다. notify 도 `follow_symlinks(false)`) — 홈 전체·네트워크 경로를 가리켜 재귀 감시를 이벤트 폭주(DoS)로 만들지 못하게 한다. `objects/` 이벤트는 무시한다.
WSL 은 4초, SSH 는 30초 폴링만 한다(`.git` 을 직접 보지 않는다). WSL 폴링은 **등록된 배포판이 있을 때만** 한다(레지스트리 확인 — WSL 을 지운 PC 의 `wsl.exe` 스텁이 설치 창을 띄우지 않게). Tauri 판에서 감시 소유는 fetch 와 같은 한 술어(소스 종류 대조 — Rust 가 맡는 종류는 Rust 만, 나머지는 사이드카만)라 이중 감시도 무감시 구간도 없다.

## GitHub 원격 전용 저장소의 예외

저장소 추가 대화상자의 GitHub 탭으로 추가한 저장소는 작업 폴더 없이 앱 캐시
(`<userData>/remote-repos/<owner>/<repo>.git`, bare + `blob:none` partial)에 이력만 받는다(`src/main/git/githubCache.ts`).
위 규칙과 의도적으로 다른 점은 셋이며, 모두 이 캐시 저장소에만 적용된다.

1. **토큰이 GitHub 서비스 밖으로 나가는 유일한 경로** — 이 저장소의 git 에만 `GIT_CONFIG_COUNT` 환경 변수로
   `http.https://github.com/.extraheader` 를 넘긴다. 명령줄 인자나 디스크에는 쓰지 않는다.
2. lazy fetch(`GIT_NO_LAZY_FETCH=0`)와 `GIT_ALLOW_PROTOCOL=https` 를 허용한다 — 필요한 blob 을 그때그때 받아야 해서다.
3. 로컬 설정은 일반 허용 목록(`isAllowedConfig`)이 아니라 기대값과 정확히 일치하는지(`checkGithubConfig`)로 검사한다
   (`core.bare=true` 는 여기서만 허용). 사용자 전역·시스템 git 설정은 읽지 않고 프록시·인증서 키만 골라 옮긴다.

토큰을 환경 변수로 싣는 이 git 은 **앱 프로세스의 환경도 거른다**(`githubEnv` 의 `SCRUB_PARENT_ENV_KEY` 표식 → `processRunner.childEnv`).
부모에 `GIT_SSL_NO_VERIFY`(값이 비어도 TLS 검증이 꺼져 중간자가 Authorization 을 받는다)나 `GIT_TRACE*`·`GIT_CURL_VERBOSE`(헤더·설정값이 파일·stderr 에 남는다)가 있거나,
`GIT_COMMON_DIR`·`GIT_WORK_TREE`·`GIT_INDEX_FILE`·`GIT_OBJECT_DIRECTORY`·`GIT_ALTERNATE_OBJECT_DIRECTORIES`·`GIT_NAMESPACE`·`GIT_CONFIG*`·`GIT_EXEC_PATH`·`GIT_TEMPLATE_DIR` 가 있어도
물려주지 않는다(이름은 대소문자 무관 — Windows 환경 변수 규칙). 프록시·`GIT_SSL_CAINFO` 등 나머지 환경은 그대로 상속한다.
일반 local·WSL·SSH git 은 토큰을 싣지 않으므로 이 거르기를 적용하지 않는다(사용자의 TLS·추적 설정은 사용자 선택이고, 자격 증명 도우미는 호스트가 정한다) — 표식을 `env` 에 넣는 실행만 거른다.
시험: `tests/githubEnvScrub.test.ts`.

**캐시의 로컬 config 가 TLS·프록시를 못 바꾸게 고정한다.** 정확 일치 검사(`checkGithubConfig`)는 fetch 앞에서만 돌므로 조회 경로의 lazy fetch 는 검사 없이 로컬 config 를 읽는다.
로컬 쓰기 권한을 가진 쪽이 캐시 config 에 `http.sslVerify=false`·`http.proxy=…` 를 쓰면 Authorization 이 그 프록시를 지나고 검증도 꺼진다.
command scope(`GIT_CONFIG_KEY_n`)는 로컬 config 보다 우선하므로 토큰 환경이 다음을 고정한다(TS `githubEnv`·Rust `github_env`, 값과 순서 동일 — 골든 `tests/golden/github/env.json`):
- `http.sslVerify=true` 와 `http.https://github.com/.sslVerify=true` — URL 범위 키가 일반 키를 이기므로 둘 다 둔다.
- `http.proxy` 와 `http.https://github.com/.proxy` = 사용자 전역·시스템 설정의 프록시, 없으면 **환경 변수**(`https_proxy`→`all_proxy`, 소문자 먼저)의 프록시, 그것도 없으면 빈 값(프록시 없음).
  빈 `http.proxy` 는 환경 변수 `HTTPS_PROXY` 까지 꺼 버린다(git 2.49 로 127.0.0.1 접속 실측 — 빈 값이면 직접 접속, 없으면 환경 변수 프록시로 접속).
  그래서 그냥 비우면 환경 변수 프록시를 쓰는 사용자의 fetch 가 깨진다. 환경 변수 프록시는 `readNetworkConfig`/`read_network_config` 가 netConfig 로 미리 옮겨 둔다.
- `http.sslCAInfo`·`sslCAPath` 는 사용자 설정이 있을 때만(netConfig) 실리고 환경의 `GIT_SSL_CAINFO` 는 그대로 남는다(사내 CA 유지). 사용자 설정이 없으면 고정하지 않는다 — 빈 값을 줄 수 없고(빈 경로는 인증서 로드 오류가 된다) 사용자 CA 를 깨뜨릴 수 있어서다.
시험: `tests/githubEnvPin.test.ts`, `src-tauri/tests/github_cache_pin.rs`(로컬 config 를 오염시킨 캐시에서 같은 환경으로 `git config --get`/`--get-urlmatch` 를 실행해 고정값이 이기는지, 고정하지 않은 git 에서는 로컬 값이 이기는 음성 대조 포함).
**남는 위험**: 로컬 config 의 더 구체적인 URL 범위 키(`http.https://github.com/<owner>/<repo>.git.sslVerify`)는 호스트 범위 고정값을 이긴다(git 의 URL 일치는 경로가 긴 쪽이 우선 — 실측 확인). 원격 URL 의 대소문자가 캐시 경로(소문자)와 달라 `githubEnv` 가 그 URL 을 알 수 없어 고정하지 않았다.
`http.sslCAInfo`·`http.curloptResolve`·`http.proxySSLVerify` 를 로컬 config 에 심는 경우도 같은 수준(네트워크 위치까지 필요)의 잔여 위험이다.

Tauri 판의 Rust 이식(`src-tauri/src/git/github_cache.rs`·`git/exec/github.rs`)은 같은 예외를 세 겹으로 가둔다. 앱에는 **조회 채널(PR 6)과 fetch·감시(PR 7)** 가 연결돼 있다(추가·삭제·고아 정리는 설정 소유 PR 9 까지 Node 사이드카).
fetch 는 일반 저장소의 허용 목록을 타지 않는 **별도 갈래**다(`git/fetch/service/github.rs`): 레코드의 owner/repo 와 캐시 경로가 맞지 않으면 일반 흐름으로 가지 않고 차단하고, 설정이 앱이 만든 것과 **정확히 일치**하지 않으면(`include.path` 등 다른 키·remote 둘) 확인 없이 차단하며,
fetch 자체는 닫힌 진입점(`--filter=blob:none`·`http.followRedirects=false`·remote `origin` 하나·고정 refspec·`--refmap=`)뿐이다. 사이드카 env(`ANTLER_RUST_FETCH_KINDS`)와 Rust 의 소유 판정은 같은 함수·같은 플래그 하나라 이중 fetch·무감시가 없다.
라우터는 GitHub 서비스(토큰의 출처)와 캐시 실행기가 주입됐고 스위치가 켜져 있을 때만 github 저장소를 푼다(`Router::set_github_cache`). 토큰은 Rust 안에서만 읽혀 실행기의 환경 변수로만 가고 어떤 응답·오류 봉투에도 실리지 않으며(시험 `tests/github_route.rs`),
`repos:remove` 는 캐시 폴더를 지우기 전에(PR 9 부터 폴더도 Rust 가 지운다 — 아래 "설정 소유") fetch(멈추고 끝나기를 기다린다 — 최대 5초)·감시·진행 중 조회·활동 시그니처 계산을 끊고 로그 커서를 닫은 뒤, 그 저장소의 git 프로세스가 끝나기를(실행기가 센다, 최대 5초) 기다린다.
보장은 "트리 종료를 요청했고 호출이 돌아왔다"까지이고 파일 핸들이 풀리는 짧은 틈은 `remove_github_cache` 의 재시도(10회×100ms)가 흡수한다. 정리를 시작한 id 에는 "삭제 중" 표식이 달려 새 조회가 폴더를 다시 쥐지 못한다(미러가 바뀌면 풀리고, 아니면 30초 뒤). 세 겹은 이렇다:
캐시 모양을 통과한 경로만 담는 `GithubCachePath` 타입이 환경·init·HEAD 쓰기의 입력이고, 환경(`github_env`)을 만드는 곳은 그 파일 한 곳(소스 검사로 강제)이며,
캐시 밖 경로의 git 은 토큰도 읽지 않고 기본 환경(전송 금지)으로 돈다. `http.followRedirects=false` 는 `-c` 로도 명시한다(`-c` 는 환경의 설정을 이긴다). 일반 저장소의 fetch 허용 목록은 바뀌지 않는다.

**부모 환경을 거른다**(Rust 판). 캐시 경로의 git 은 부모 프로세스의 `GIT_*` 변수를 **하나도 물려받지 않는다**(`GIT_SSL_CAINFO`·`GIT_SSL_CAPATH` 만 예외 — 사설 인증서용이고 검증을 끄지 않는다. `SSLKEYLOGFILE` 도 지운다). 이름은 대소문자를 구분하지 않는다.
토큰 헤더가 환경에 실리는 유일한 경로라 허용 목록 쪽으로 정했다: 부모의 `GIT_SSL_NO_VERIFY`(빈 값이어도 TLS 검증을 끈다)·`GIT_TRACE*`/`GIT_CURL_VERBOSE`(`GIT_TRACE_REDACT=0` 와 함께 Authorization 기록)·`GIT_EXEC_PATH`·`GIT_ALTERNATE_OBJECT_DIRECTORIES`·`GIT_NAMESPACE`·`GIT_COMMON_DIR`·`GIT_OBJECT_DIRECTORY`·`GIT_INDEX_FILE`·`GIT_WORK_TREE`·`GIT_CONFIG*` 는 어느 하나도 이 실행기에 필요하지 않고,
`GIT_COMMON_DIR` 를 물려받으면 `init_github_repo` 의 `git config --local` 이 사용자 저장소의 `.git/config` 를 덮어쓴다. 필요한 `GIT_*` 는 `github_env` 가 모두 직접 넣는다. 지우기는 덧씌우기보다 **먼저** 적용돼(`exec/process.rs` `launch`) 실행기가 넣은 `GIT_DIR`·`GIT_CONFIG_COUNT` 등은 살아남는다.
토큰은 `run` 의 stdout·stderr·오류 문구와 `stream` 의 `finish` 오류 문구(message·stderr)에서 가린다(`config --list` 가 환경의 `extraheader` 를 보여 주기 때문이다). `stream` 의 stdout 은 파일 내용(blob)이라 가리지 않는다 — 설정을 드러내는 명령은 `run` 으로만 읽는다.

## Tauri 판의 설정 소유(Phase 6b PR 9)

**한 실행 동안 설정 파일(`antler.config.json`)을 쓰는 쪽은 하나다.** `ANTLER_RUST_GIT≠0` 이면 Rust(`src-tauri/src/config/`·`config_host.rs`)가 유일한 작성자이고, 사이드카는 `ANTLER_RUST_CONFIG=1` 로 떠서 파일을 열지도 쓰지도 않는다(읽기 전용 빈 설정 — 변경자가 던진다)·미러와 fetch 설정을 보내지 않는다·설정 소유 채널(`config:*`·`repos:*`·`external:*`·`req:cancel`)을 받지 않는다·**고아 캐시를 정리하지 않는다**(빈 설정이면 모든 캐시가 고아로 보인다).
`ANTLER_RUST_GIT=0` 이면 Rust 는 파일을 열지 않고 사이드카가 쓴다(소유 술어는 시작 때 `router.enabled()` 하나 — 소스 검사가 `ConfigStore::open` 이 그 분기 안의 한 곳임을 강제한다). Rust 는 소유할 때 사이드카가 보낸 `repos`·`fetchPrefs` 줄을 버린다.
- 쓰기: 임시 파일 → `sync_all` → rename 의 원자적 쓰기, 디스크에 먼저 쓰고 성공했을 때만 메모리를 바꾼다, JSON 깊이 상한(`MAX_DEPTH`)을 넘는 값은 저장하지 않는다, 손상된 파일은 덮어쓰기 전에 `.corrupt-*.bak` 로 보존하고 `load_failed` 면 캐시를 정리하지 않는다.
- **renderer 입력은 모두 검증한다**(TS `handlers.ts` 와 같은 규칙 이상): 환경·경로·호스트(`check_env_input` — NT 네임스페이스·UNC(WSL 제외)·NUL·길이·`-` 시작 호스트·WSL 설치 목록 대조), 그룹 200자, `addMany` 500개·같은 환경, `addGithub` 200개·`validate_github_link`, `setUi` 검증기(`editorExePath`·`skippedUpdateVersion` 키 거부).
  **`config:setUi` 로는 승인(`fetchApprovals`·`trustedConfigs`)을 위조할 수 없다** — 패치는 `ui` 아래로만 들어간다. 승인을 쓰는 곳은 fetch 서비스의 두 지점(수동 fetch 성공 뒤·"항상 허용" 뒤)이 부르는 `FetchHost::approve` 구현(`fetch_host.rs`) 한 곳이고 소스 검사가 강제한다(저장소 존재·소문자 hex 64 를 다시 확인).
- 저장소를 추가할 때 git 이 돌려준 최상위 경로를 **다시 `check_path`**(UNC·NT 네임스페이스 거부)로 본다. 로컬 경로는 `git -C` 앞에 `fsguard::check_local_root` 로 링크가 원격으로 이어지는지 본다(NTLM 유출).
- 설정에 없는 SSH 호스트의 확인 창 승인 기억은 `EnvHost` 하나다(`repos:add`·`env:test`·`env:listDir`·`repos:scan` 이 같은 것을 쓴다). 거부는 기억하지 않는다. 호스트 키는 `known_hosts` 로만(TOFU 없음).
- 저장소 쓰기 예외(앱 캐시)의 **삭제**: 폴더를 지우는 입력은 `GithubCachePath` 뿐이고, 레코드는 지우기 전에 읽어 **링크의 owner/repo 와 캐시 경로가 맞을 때만**(`cache_path_matches_link`) 지운다 — 다른 저장소의 캐시를 가리키는 레코드가 그 폴더를 지우지 못한다. owner·저장소 폴더가 링크(정션 포함)면 따라가지도 지우지도 않는다.
  순서는 fetch 정지·대기 → 감시 정지 → 조회·로그 커서·git 프로세스 드레인 → 폴더 삭제(경로 잠금 안) → 레코드 삭제. 고아 정리·추가도 같은 경로 잠금을 쓰고, 고아 정리는 모양이 맞고 링크가 아닌 폴더만 지운다.
- 미러의 GitHub 링크·기본 브랜치는 `RepoMirror::apply_message`(`sanitize_github_link`·`is_safe_branch`)의 검증 경로로만 들어간다 — 설정에서 직접 만든 값도 같다.
- `external:open`: 동작 허용 목록·저장소 id → **설정에서만** 루트·환경을 읽는다(renderer 는 경로를 정하지 못한다)·GitHub 소스 거부·`path` 는 문자열만(`null` 거부). 프로그램을 띄우는 곳은 `external/launch.rs` 한 곳이고 `lib.rs` 가 주입한다 — `config_host.rs` 는 `Command`·실행기를 직접 만들지 않는다(소스 검사).
  에디터 실행 파일은 사용자가 고른 `.exe`/`.com` 만(`check_chosen_exe` — 절대 경로·UNC 거부·존재), `setUi` 로는 바꿀 수 없다.
- **Electron 설정 가져오기(D1)**: Tauri 설정이 없을 때 한 번만, Electron 파일은 읽기만(고치지도 백업도 만들지 않는다). **명령을 실행할 수 있는 저장소 설정의 "항상 허용"(`trustedConfigs`)은 가져오지 않는다**(이관 뒤 한 번 다시 묻는다). 창 위치·GitHub 캐시 폴더·토큰(D2)도 가져오지 않는다.
- **매핑된 네트워크 드라이브(`Z:\repo`)** 로 등록한 저장소는 Electron 판처럼 열린다 — 입력이 드라이브 문자 경로이고 대상의 canonical 이 그 드라이브 루트의 canonical 과 **같은 서버·공유 아래**일 때만(`restore_mapped_drive`). UNC 로 직접 등록한 경로·드라이브 안에서 다른 공유로 나가는 링크·`canonicalize` 앞의 링크 가드(원격 UNC 를 가리키는 링크는 따라가기 전에 거부)는 그대로 거부한다.
