; Antler Tauri 판 NSIS 설치 훅(tauri.conf.json 의 bundle.windows.nsis.installerHooks).
; Tauri 템플릿이 파일·바로가기·제거 항목을 만든 **뒤**(NSIS_HOOK_POSTINSTALL)에 두 가지를 한다:
;   1) 사용자별로 설치된 Electron 판 Antler 를 조용히 제거한다 — 이관(Electron 의 electron-updater 가 이 설치본을 받아 실행한다).
;      %APPDATA%\Antler(설정·저장소 목록)는 건드리지 않는다: Tauri 판 첫 실행의 가져오기 원천이고 Electron 으로 되돌아갈 길이다.
;      모든 사용자용(Program Files) 설치본은 권한이 달라 지우지 않고 안내만 한다.
;   2) Electron 의 업데이터가 `--force-run` 으로 부른 경우 설치 뒤 앱을 다시 띄운다(템플릿은 `/R` 일 때만 띄운다).
; 설치 순서가 중요하다: Electron 제거기는 `Antler.exe` 를 이름으로 종료시키므로 반드시 앱을 띄우기 **전에** 돌린다.
; 근거·전제: docs/tauri.md 의 "Phase 6c PR 2". 이 파일은 정적 검토와 makensis 컴파일만 했고 실제 설치·제거는 리허설(R)에서 확인한다.

!include LogicLib.nsh

; Electron 판(electron-builder)의 레지스트리 키. GUID = UUID v5("com.antler.app", electron-builder 의 네임스페이스) — tests/updateManifests.test.ts 가 같은 값인지 대조한다.
; electron-builder 는 사용자별 설치본을 HKCU 에, 모든 사용자용을 HKLM 에 쓴다.
!define ANTLER_ELECTRON_GUID "a0282e38-7d2f-5de0-a2ab-1ae175fbe58f"
!define ANTLER_ELECTRON_UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${ANTLER_ELECTRON_GUID}"
!define ANTLER_ELECTRON_INSTALL_KEY "Software\${ANTLER_ELECTRON_GUID}"

; 사용자별 Electron 설치본을 지운다. 레지스트리 값은 믿지 않고 검증한 뒤에만 실행한다:
;   - InstallLocation 이 비어 있지 않고, 드라이브 경로(`X:\`)이며(UNC·`\\?\`·`\\.\` 거부), 이 설치 폴더와 다르고 이 설치 폴더가 그 하위도 아니다
;   - DisplayName 이 `Antler` 또는 `Antler <버전>` 이다(같은 GUID 를 쓰는 다른 앱 방어). 설치기가 관리자 토큰이면 제거하지 않고 안내만 한다
;   - UninstallString 이 따옴표로 감싼 경로이고, 그 파일이 InstallLocation **바로 아래**의 `.exe`(하위 경로·`:` 없음)이며 실제로 있다
; 하나라도 어긋나면 아무것도 하지 않는다(Electron 설치본은 그대로 남는다). 실행 파일은 검증한 그 경로 하나뿐이고, 지우는 것은 그 파일과
; 비어 있는 InstallLocation 뿐이다(재귀 삭제 없음).
; 레지스터: $0 UninstallString, $1 InstallLocation, $2 제거기 경로, $3·$4·$5·$6 임시
Function AntlerRemoveElectron
  Push $0
  Push $1
  Push $2
  Push $3
  Push $4
  Push $5
  Push $6

  ReadRegStr $0 HKCU "${ANTLER_ELECTRON_UNINSTALL_KEY}" "UninstallString"
  ReadRegStr $1 HKCU "${ANTLER_ELECTRON_INSTALL_KEY}" "InstallLocation"
  ${If} $0 == ""
    Goto check_machine_wide
  ${EndIf}
  ${If} $1 == ""
    DetailPrint "Electron Antler: InstallLocation is empty - skipped"
    Goto done
  ${EndIf}

  ; 드라이브 경로만(`C:\...`): 두 번째 글자부터 두 글자가 `:\`
  StrCpy $3 $1 2 1
  ${If} $3 != ':\'
    DetailPrint "Electron Antler: InstallLocation is not a drive path - skipped"
    Goto done
  ${EndIf}
  ; 끝의 `\` 제거. 루트(`C:\`)나 너무 짧은 경로는 거부
  StrCpy $3 $1 1 -1
  ${If} $3 == '\'
    StrCpy $1 $1 -1
  ${EndIf}
  StrLen $3 $1
  ${If} $3 < 4
    DetailPrint "Electron Antler: InstallLocation is too short - skipped"
    Goto done
  ${EndIf}
  ; 새 설치 폴더가 InstallLocation 과 같거나 그 **하위**여도 건드리지 않는다(`$1\` 로 시작). NSIS 의 `==` 는 대소문자를 구분하지 않는다
  ${If} $1 == $INSTDIR
    DetailPrint "Electron Antler: InstallLocation equals the new install dir - skipped"
    Goto done
  ${EndIf}
  StrLen $3 $1
  IntOp $3 $3 + 1
  StrCpy $4 $INSTDIR $3
  ${If} $4 == '$1\'
    DetailPrint "Electron Antler: the new install dir is inside InstallLocation - skipped"
    Goto done
  ${EndIf}

  ; 같은 GUID 를 쓰는 다른 앱을 지우지 않도록 표시 이름을 대조한다. electron-builder 는 `<productName> <버전>`(예: `Antler 0.5.0`)을 쓴다
  ReadRegStr $3 HKCU "${ANTLER_ELECTRON_UNINSTALL_KEY}" "DisplayName"
  StrCpy $4 $3 7
  ${If} $3 != "Antler"
  ${AndIf} $4 != "Antler "
    DetailPrint "Electron Antler: DisplayName is not Antler - skipped"
    Goto done
  ${EndIf}

  ; UninstallString = `"<경로>" <인자>` — 첫 따옴표 안쪽만 취한다
  StrCpy $3 $0 1
  ${If} $3 != '"'
    DetailPrint "Electron Antler: UninstallString is not quoted - skipped"
    Goto done
  ${EndIf}
  StrCpy $4 1
  find_quote:
    StrCpy $3 $0 1 $4
    ${If} $3 == ""
      DetailPrint "Electron Antler: UninstallString has no closing quote - skipped"
      Goto done
    ${EndIf}
    ${If} $3 == '"'
      Goto got_path
    ${EndIf}
    IntOp $4 $4 + 1
    Goto find_quote
  got_path:
  IntOp $4 $4 - 1
  StrCpy $2 $0 $4 1

  ; 제거기는 InstallLocation 바로 아래의 파일이어야 한다
  StrLen $5 $1
  IntOp $5 $5 + 1
  StrCpy $3 $2 $5
  ${If} $3 != '$1\'
    DetailPrint "Electron Antler: uninstaller is not inside InstallLocation - skipped"
    Goto done
  ${EndIf}
  StrCpy $3 $2 "" $5
  StrCpy $4 0
  name_loop:
    StrCpy $6 $3 1 $4
    ${If} $6 == ""
      Goto name_done
    ${EndIf}
    ${If} $6 == '\'
    ${OrIf} $6 == '/'
    ${OrIf} $6 == ':'
      DetailPrint "Electron Antler: uninstaller path has a sub path - skipped"
      Goto done
    ${EndIf}
    IntOp $4 $4 + 1
    Goto name_loop
  name_done:
  ${If} $4 < 5
    Goto done
  ${EndIf}
  StrCpy $6 $3 "" -4
  ${If} $6 != '.exe'
    DetailPrint "Electron Antler: uninstaller is not an .exe - skipped"
    Goto done
  ${EndIf}
  ${IfNot} ${FileExists} $2
    DetailPrint "Electron Antler: uninstaller file is missing - skipped"
    Goto done
  ${EndIf}

  ; 설치기가 관리자 토큰으로 돌고 있으면(관리자 권한으로 실행) HKCU 는 이 사용자의 것이 아닐 수 있고, 관리자 권한으로 레지스트리 값이 가리키는
  ; 제거기를 실행하는 것은 권한 상승 경로가 된다 — 자동 제거를 하지 않고 안내만 한다
  UserInfo::GetAccountType
  Pop $6
  ${If} $6 == "Admin"
    DetailPrint "Electron Antler: installer is elevated - automatic removal skipped"
    ${If} $LANGUAGE == 1042
      MessageBox MB_OK|MB_ICONINFORMATION "이전 버전(Electron 판) Antler 가 설치되어 있습니다. 관리자 권한으로 실행된 설치기는 자동으로 제거하지 않습니다. 'Windows 설정 > 앱'에서 이전 Antler 를 제거해 주세요. 설정과 저장소 목록은 지워지지 않습니다." /SD IDOK
    ${Else}
      MessageBox MB_OK|MB_ICONINFORMATION "An older (Electron) edition of Antler is installed. An installer running with administrator rights does not remove it automatically. Please remove it in Windows Settings > Apps. Your settings and repository list are kept." /SD IDOK
    ${EndIf}
    Goto done
  ${EndIf}

  ; electron-builder 가 옛 판을 지울 때 쓰는 인자와 같다(/KEEP_APP_DATA 와 --updated 로 앱 데이터를 보존하고, --keep-shortcuts 로
  ; 방금 만든 바로가기를 지우지 않게 한다). `_?=` 는 제거기가 제자리에서 끝날 때까지 기다리게 하며 반드시 마지막 인자다
  ; 제거기는 **임시 폴더로 복사해서** 돌린다(electron-builder 의 installUtil.nsh 와 같은 방식). `--updated` 제거기는 설치 폴더를
  ; 통째로 옮긴(rename) 뒤 지우는데, 설치 폴더 안의 제거기를 제자리에서 돌리면 자기 자신이 잠겨 옮기기가 실패하고 종료 코드 2 로
  ; 멈춘다(v2.0.0-rc.3 이관 리허설에서 재현). `_?=` 는 그대로 두어 설치 폴더를 알려 주고 끝날 때까지 기다리게 한다
  DetailPrint "Removing the per-user Electron edition of Antler..."
  InitPluginsDir
  CopyFiles /SILENT /FILESONLY "$2" "$PLUGINSDIR\old-uninstaller.exe"
  ${IfNot} ${FileExists} "$PLUGINSDIR\old-uninstaller.exe"
    DetailPrint "Electron Antler: could not copy the uninstaller - skipped"
    Goto done
  ${EndIf}
  ExecWait '"$PLUGINSDIR\old-uninstaller.exe" /currentuser /S /KEEP_APP_DATA --updated --keep-shortcuts _?=$1' $6
  ${If} $6 == 0
    ; `_?=` 로 돌리면 제거기 자신이 남는다 — 검증한 그 파일만 지우고, 폴더는 비어 있을 때만 지운다(재귀 삭제 없음)
    Delete $2
    RMDir $1
  ${Else}
    DetailPrint "Electron Antler: uninstaller exit code $6"
    ${If} $LANGUAGE == 1042
      MessageBox MB_OK|MB_ICONEXCLAMATION "이전 버전(Electron 판) Antler 를 자동으로 제거하지 못했습니다. 'Windows 설정 > 앱'에서 직접 제거해 주세요. 설정과 저장소 목록은 지워지지 않습니다." /SD IDOK
    ${Else}
      MessageBox MB_OK|MB_ICONEXCLAMATION "The previous (Electron) edition of Antler could not be removed automatically. Please remove it in Windows Settings > Apps. Your settings and repository list are kept." /SD IDOK
    ${EndIf}
  ${EndIf}
  Goto done

  check_machine_wide:
  ; 모든 사용자용(HKLM) Electron 설치본: 권한이 달라 지우지 않는다. 32·64비트 뷰를 모두 본다. 끝나면 64비트 뷰로 돌려놓는다(x64 설치본)
  SetRegView 64
  ReadRegStr $3 HKLM "${ANTLER_ELECTRON_UNINSTALL_KEY}" "UninstallString"
  SetRegView 32
  ReadRegStr $4 HKLM "${ANTLER_ELECTRON_UNINSTALL_KEY}" "UninstallString"
  SetRegView 64
  ${If} $3 != ""
  ${OrIf} $4 != ""
    ${If} $LANGUAGE == 1042
      MessageBox MB_OK|MB_ICONINFORMATION "이전 버전(Electron 판) Antler 가 모든 사용자용으로 설치되어 있어 그대로 두었습니다. 관리자 권한으로 'Windows 설정 > 앱'에서 이전 Antler 를 제거해 주세요." /SD IDOK
    ${Else}
      MessageBox MB_OK|MB_ICONINFORMATION "An older all-users installation of Antler (Electron edition) was left in place. Please remove it in Windows Settings > Apps with administrator rights." /SD IDOK
    ${EndIf}
  ${EndIf}

  done:
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

; Electron 업데이터가 `--force-run` 으로 설치기를 띄웠고 템플릿이 앱을 띄우지 않을 때(`/R` 없음)만 직접 띄운다(RunAsUser: 설치기가 관리자 토큰이어도 권한을 낮춰 실행)
; Function 이 아니라 매크로다: hooks.nsh 는 템플릿이 nsis_tauri_utils 플러그인 경로를 더하기(!addplugindir) **전에** include 되고,
; Function 본문은 그 자리에서 컴파일돼 "Plugin not found" 로 makensis 가 멈춘다(v2.0.0-rc.1 빌드). 매크로는 NSIS_HOOK_POSTINSTALL 이 설치 Section 안에서 펼칠 때 컴파일된다
!macro AntlerForceRun
  Push $0
  ClearErrors
  ${GetOptions} $CMDLINE "--force-run" $0
  ${IfNot} ${Errors}
    ClearErrors
    ${GetOptions} $CMDLINE "/R" $0
    ${If} ${Errors}
      ; 템플릿의 `/R` 재실행과 같은 방식: 설치기가 관리자 토큰이어도 권한을 낮춘 사용자로 띄운다
      nsis_tauri_utils::RunAsUser "$INSTDIR\${MAINBINARYNAME}.exe" ""
    ${EndIf}
  ${EndIf}
  Pop $0
!macroend

!macro NSIS_HOOK_POSTINSTALL
  Call AntlerRemoveElectron
  !insertmacro AntlerForceRun
!macroend
