# 보안 설계

[← README](../README.md)

읽기 전용 앱이지만 공격자가 내용을 정할 수 있는 저장소를 연다는 전제로 설계했다.

- 창: contextIsolation · sandbox · nodeIntegration off, preload 는 허용 목록 채널만 노출, 권한 요청은 모두 거부,
  프로덕션 빌드에 CSP 메타 태그 주입, 외부 링크는 기본 브라우저로. 외부 HTML 은 sanitize 로 정화한다.
- IPC: main 이 모든 입력(경로·해시·ref 등)을 검증한다.
- 악성 저장소 방어: 모든 git 호출에 `GIT_NO_LAZY_FETCH=1`, `GIT_ALLOW_PROTOCOL=none`, `-c core.fsmonitor=false` 를 줘
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
- 코드 탐색기: 저장소 문서(README 등)의 외부 링크는 사용자가 누를 때만 `code:openLink` 로 연다 — https 만, 사용자 정보가 든 URL 은 거부, 마우스를 올리면 실제 주소를 보여 준다.
- 경로·diff: `GIT_LITERAL_PATHSPECS=1` 이라 renderer 가 보낸 경로가 pathspec 매직으로 해석되지 않고,
  `--no-textconv`·`--no-ext-diff` 로 저장소 설정이 외부 프로그램을 실행하지 못하게 한다.

## GitHub 원격 전용 저장소의 예외

저장소 추가 대화상자의 GitHub 탭으로 추가한 저장소는 작업 폴더 없이 앱 캐시
(`<userData>/remote-repos/<owner>/<repo>.git`, bare + `blob:none` partial)에 이력만 받는다(`src/main/git/githubCache.ts`).
위 규칙과 의도적으로 다른 점은 셋이며, 모두 이 캐시 저장소에만 적용된다.

1. **토큰이 GitHub 서비스 밖으로 나가는 유일한 경로** — 이 저장소의 git 에만 `GIT_CONFIG_COUNT` 환경 변수로
   `http.https://github.com/.extraheader` 를 넘긴다. 명령줄 인자나 디스크에는 쓰지 않는다.
2. lazy fetch(`GIT_NO_LAZY_FETCH=0`)와 `GIT_ALLOW_PROTOCOL=https` 를 허용한다 — 필요한 blob 을 그때그때 받아야 해서다.
3. 로컬 설정은 일반 허용 목록(`isAllowedConfig`)이 아니라 기대값과 정확히 일치하는지(`checkGithubConfig`)로 검사한다
   (`core.bare=true` 는 여기서만 허용). 사용자 전역·시스템 git 설정은 읽지 않고 프록시·인증서 키만 골라 옮긴다.
