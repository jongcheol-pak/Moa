; MOA 설치 스크립트 (NSIS 3) — `cargo run --example gen_installer`가 makensis에 넘긴다.
;
; **사용자 단위 설치**다: `%LOCALAPPDATA%\Programs\MOA`에 놓아 UAC를 띄우지 않는다.
; 이 앱은 설정을 실행 파일 옆에, 자동 실행을 HKCU에 쓰므로 권한 모델이 사용자 단위와 맞는다.
;
; **설치 폴더는 고정이고 대화 상자는 띄우지 않는다**(2026-08-21 사용자 요청) — 폴더 선택
; 페이지는 종전처럼 보이되 **경로 입력란과 「찾아보기」를 잠가** 바꿀 수 없게 한다.
;
; **모드가 둘이다** — 사람이 직접 실행하는 보통 설치와, 앱이 자동 업데이트로 부르는
; `/UPDATE` 모드(FR-62). 업데이트 모드에서는 언어 선택 대화와 앞의 세 페이지(환영·
; 라이선스·폴더)를 건너뛰고 **설치 진행 화면만** 보인 뒤, 마침 페이지 없이 닫히며 앱을
; 다시 띄운다. 스크립트를 둘로 나누지 않은 이유는 설치 내용이 같기 때문이다 —
; 나누면 한쪽만 고치는 사고가 난다.
;
; **경로는 모두 이 파일이 있는 폴더 기준**이다 — `gen_installer`가 makensis의 작업 디렉터리를
; `installer\`로 지정하고 부르므로, makensis가 스크립트 폴더로 옮기든 아니든 같은 자리를 가리킨다.

; 산출물을 유니코드 exe로 만든다
Unicode true

; **이 파일은 BOM 없는 UTF-8이다**(레포 규약 — AGENTS). makensis는 BOM이 없으면
; 시스템 코드페이지로 읽어 아래 한글 문구가 깨지므로, 빌더(`examples/gen_installer.rs`)가
; `/INPUTCHARSET UTF8`을 주며 부른다. 손으로 돌릴 때도 그 인자가 필요하다

; 버전은 `gen_installer`가 `/DVERSION`으로 넘긴다. 손으로 makensis를 돌려도 빌드가
; 성립하도록 기본값을 둔다 — 그때는 산출물 이름이 `MOA-Setup-0.0.0-dev.exe`가 된다
!ifndef VERSION
  !define VERSION "0.0.0-dev"
!endif

!define APP_NAME "MOA"
!define PUBLISHER "jongcheol-pak"
!define EXE_NAME "moa.exe"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\MOA"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"

OutFile "..\target\installer\MOA-Setup-${VERSION}.exe"
InstallDir "$LOCALAPPDATA\Programs\MOA"
; 관리자 권한을 청하지 않는다 — 사용자 폴더에만 쓴다
RequestExecutionLevel user
SetCompressor /SOLID lzma

!include "MUI2.nsh"
; `${GetParameters}`·`${GetOptions}` — 명령행에서 `/UPDATE`를 가려내는 데 쓴다
!include "FileFunc.nsh"

; 업데이트 모드인가 (`/UPDATE`로 불렸는가). 1이면 안내 페이지를 건너뛴다
Var UpdateMode

!define MUI_ICON "..\docs\AppIcon.ico"
!define MUI_UNICON "..\docs\AppIcon.ico"
!define MUI_ABORTWARNING

; 언어 선택 대화의 문구 — MUI 기본값은 영문 **고정 define**이라 그대로 두면 한국어 Windows에서도
; 영문으로 뜬다. `$(...)` 참조를 넣으면 컴파일 때 언어 문자열로 박혀, 그 대화가 뜨는 시점의
; `$LANGUAGE`(= 시스템 표시 언어)를 따른다. **콤보에서 고른 언어로 그 대화를 다시 그리는 것은
; 할 수 없다** — LangDLL 플러그인은 고를 때마다 자기 문구를 다시 칠하지 않는다
!define MUI_LANGDLL_WINDOWTITLE "$(LANGDLL_TITLE)"
!define MUI_LANGDLL_INFO "$(LANGDLL_INFO)"

; 안내 페이지를 건너뛰는 콜백 — 업데이트 모드에서 `Abort`하면 그 페이지가 표시되지 않는다.
; **`MUI_PAGE_CUSTOMFUNCTION_PRE`는 페이지 매크로가 쓰고 나서 `!undef` 하므로**
; (`Contrib\Modern UI 2\Pages.nsh`의 `MUI_PAGE_FUNCTION_CUSTOM`) 페이지마다 다시 정의한다
Function SkipWhenUpdating
  StrCmp $UpdateMode "1" 0 +2
    Abort
FunctionEnd

; 설치 페이지 — 라이선스는 레포의 MIT 원문을 그대로 보인다
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipWhenUpdating
!define MUI_WELCOMEPAGE_TEXT "$(WELCOME_TEXT)"
!insertmacro MUI_PAGE_WELCOME
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipWhenUpdating
!insertmacro MUI_PAGE_LICENSE "..\LICENSE"

; 설치 위치를 보여 주는 페이지 — 고를 수는 없다. 페이지를 통째로 빼면 어디에 깔리는지
; 알 길이 없어져, 종전 화면을 그대로 두고 입력란과 「찾아보기」만 잠근다
!define MUI_PAGE_HEADER_TEXT "$(DIR_PAGE_HEADER)"
!define MUI_PAGE_HEADER_SUBTEXT "$(DIR_PAGE_SUBHEADER)"
!define MUI_DIRECTORYPAGE_TEXT_TOP "$(DIR_PAGE_TOP)"
!define MUI_PAGE_CUSTOMFUNCTION_SHOW LockDirectoryPage
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipWhenUpdating
!insertmacro MUI_PAGE_DIRECTORY

; 그 잠금. **MUI2가 SHOW 콜백 직전에 컨트롤 핸들을 이 변수들에 담아 두므로**
; 대화 상자 컨트롤 ID(1019·1001)를 이 스크립트가 알 필요가 없다 — 그래서
; 페이지 매크로 **뒤에** 둔다(그 변수들이 거기서 선언된다)
Function LockDirectoryPage
  ; 경로 입력란은 **읽기 전용**으로 둔다 — 비활성화하면 경로 글자까지 흐려져 읽기 어렵다.
  ; 「찾아보기」는 눌러도 소용이 없으니 그쪽은 비활성화한다
  SendMessage $mui.DirectoryPage.Directory ${EM_SETREADONLY} 1 0
  EnableWindow $mui.DirectoryPage.BrowseButton 0
  ; 포커스를 「설치」 버튼으로 옮긴다 — 그러지 않으면 이 페이지에 들어설 때 읽기 전용
  ; 입력란이 포커스를 받아 경로가 통째로 선택된 것처럼 파랗게 보인다
  GetDlgItem $0 $HWNDPARENT 1
  SendMessage $HWNDPARENT ${WM_NEXTDLGCTL} $0 1
FunctionEnd

!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_RUN "$INSTDIR\${EXE_NAME}"
Function CreateDesktopShortcut
  CreateShortcut "$DESKTOP\$(SHORTCUT_NAME).lnk" "$INSTDIR\${EXE_NAME}"
FunctionEnd

; 바탕화면 바로가기는 마침 페이지의 체크박스로 고른다(`SHOWREADME` 자리를 빌린다).
; **`MUI_PAGE_FINISH`보다 먼저 정의해야 한다** — NSIS 전처리기는 위에서 아래로 훑고,
; 그 매크로가 전개되는 줄에서 `!ifdef`로 이 정의들을 찾기 때문이다(뒤에 두면 체크박스가 통째로 빠진다).
; 문구는 `LangString`으로 둔다 — `..._TEXT_KOREAN` 같은 언어별 접미사는 MUI2가 읽지 않는다
!define MUI_FINISHPAGE_SHOWREADME ""
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED
!define MUI_FINISHPAGE_SHOWREADME_TEXT "$(DESKTOP_SHORTCUT_TEXT)"
!define MUI_FINISHPAGE_SHOWREADME_FUNCTION CreateDesktopShortcut
; 업데이트 모드는 이 페이지도 건너뛴다 — 대신 `.onInstSuccess`가 앱을 다시 띄운다
!define MUI_PAGE_CUSTOMFUNCTION_PRE SkipWhenUpdating
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

; 한국어가 먼저다 — 첫 언어가 기본 선택이 된다
!insertmacro MUI_LANGUAGE "Korean"
!insertmacro MUI_LANGUAGE "English"
; 솔리드 압축이라 언어 대화 리소스를 앞쪽에 예약한다 — 예약하지 않으면 대화가 뜨기 전에
; 아카이브 전체를 풀어야 해 시작이 그만큼 늦다
!insertmacro MUI_RESERVEFILE_LANGDLL

; 창 제목에 쓰는 앱 이름도 설치 언어를 따른다 — 한국어면 「모아 0.1.0 설치」가 된다
; (2026-08-21 사용자 요청). **`Name`에는 `/LANG=` 스위치가 없다** — 그렇게 적으면
; 「/LANG=1033」이 통째로 제품명이 되므로, 언어 문자열을 만들어 그 참조를 넘긴다.
; `Name`이라야 하는 이유: 이 명령이 `^Name`과 `^NameDA`를 함께 채우고,
; 환영 화면 머리글(「… 설치를 시작합니다」)이 뒤엣것을 읽는다.
; 레지스트리에 쓰는 `DisplayName`은 `${APP_NAME}` 그대로 둔다 — 그것은 데이터가 걸린 이름이다
LangString APP_TITLE ${LANG_KOREAN} "모아 ${VERSION}"
LangString APP_TITLE ${LANG_ENGLISH} "MOA ${VERSION}"
Name "$(APP_TITLE)"

; 바로가기 이름 — 설치 언어를 따른다 (2026-08-21 사용자 요청)
LangString SHORTCUT_NAME ${LANG_KOREAN} "모아"
LangString SHORTCUT_NAME ${LANG_ENGLISH} "MOA"
LangString DESKTOP_SHORTCUT_TEXT ${LANG_KOREAN} "바탕화면에 바로가기 만들기"
LangString DESKTOP_SHORTCUT_TEXT ${LANG_ENGLISH} "Create a desktop shortcut"

; 언어 선택 대화 문구
LangString LANGDLL_TITLE ${LANG_KOREAN} "설치 언어"
LangString LANGDLL_TITLE ${LANG_ENGLISH} "Installer Language"
LangString LANGDLL_INFO ${LANG_KOREAN} "설치에 사용할 언어를 선택해 주세요."
LangString LANGDLL_INFO ${LANG_ENGLISH} "Please select a language."

; 환영 화면 본문 — 팝업으로 알리던 「제거하면 설정까지 사라진다」(D11)가 여기로 왔다.
; 설치 위치와 실행 중 자동 종료는 그 다음 폴더 페이지에서 알린다
LangString WELCOME_TEXT ${LANG_KOREAN} "모아를 이 컴퓨터에 설치합니다.$\r$\n$\r$\n제거하면 사이트 목록·저장한 비밀번호·서버 지문이 함께 지워집니다."
LangString WELCOME_TEXT ${LANG_ENGLISH} "MOA will be installed on this computer.$\r$\n$\r$\nUninstalling also deletes your site list, saved passwords, and server fingerprints."

; 폴더 페이지 문구 — 기본 문구는 「다른 폴더에 설치하고 싶으시면 찾아보기를 누르라」고
; 안내하는데 그 버튼을 잠갔으니 그대로 두면 틀린 말이 된다. 머리글에서도 「선택」을 뺀다
LangString DIR_PAGE_HEADER ${LANG_KOREAN} "설치 위치"
LangString DIR_PAGE_HEADER ${LANG_ENGLISH} "Install Location"
LangString DIR_PAGE_SUBHEADER ${LANG_KOREAN} "모아를 설치할 폴더입니다."
LangString DIR_PAGE_SUBHEADER ${LANG_ENGLISH} "The folder where MOA will be installed."
LangString DIR_PAGE_TOP ${LANG_KOREAN} "모아를 다음 폴더에 설치합니다. 설치 폴더는 바꿀 수 없습니다.$\r$\n$\r$\n설치를 시작하면 실행 중인 모아가 자동으로 종료됩니다."
LangString DIR_PAGE_TOP ${LANG_ENGLISH} "MOA will be installed in the following folder. The install folder cannot be changed.$\r$\n$\r$\nIf MOA is running, it will be closed automatically when the installation starts."

; 설치 진행 기록에 남기는 줄 — 무엇 때문에 앱이 닫혔는지 알 수 있게 한다
LangString CLOSING_APP ${LANG_KOREAN} "실행 중인 모아를 닫습니다..."
LangString CLOSING_APP ${LANG_ENGLISH} "Closing MOA if it is running..."

Function .onInit
  ; 앱이 자동 업데이트로 부를 때 `/UPDATE`를 넘긴다 (`app::update::install`).
  ; `GetOptions`는 대소문자를 가리지 않아 `/update`로 적어도 걸린다
  StrCpy $UpdateMode "0"
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/UPDATE" $R1
  IfErrors +2 0
    StrCpy $UpdateMode "1"

  ; 업데이트 모드에서는 언어를 묻지 않는다 — 사용자가 볼 것은 진행 막대뿐이라 고를 것이
  ; 없고, 앱이 부른 설치에 대화가 뜨면 「자동으로 진행」이 아니게 된다. 그때 화면 문구는
  ; 시스템 표시 언어를 따른다(`$LANGUAGE` 기본값)
  StrCmp $UpdateMode "1" 0 +2
    Return
  !insertmacro MUI_LANGDLL_DISPLAY
FunctionEnd

; 설치가 끝난 뒤 — 업데이트 모드는 마침 페이지가 없으므로 우리가 앱을 다시 띄운다.
; 보통 설치는 마침 페이지의 실행 체크박스(`MUI_FINISHPAGE_RUN`)가 그 일을 한다
Function .onInstSuccess
  StrCmp $UpdateMode "1" 0 +2
    Exec "$INSTDIR\${EXE_NAME}"
FunctionEnd

Section "Install"
  ; 업데이트 모드는 마침 페이지를 건너뛰므로 진행 화면이 스스로 닫혀야 한다.
  ; 보통 설치에서는 종전대로 「다음」을 눌러 마침 페이지로 간다
  StrCmp $UpdateMode "1" 0 +2
    SetAutoClose true

  ; 실행 중이면 파일을 덮어쓸 수 없다 — 「먼저 닫아 주세요」라고 청하는 대신 우리가 닫는다
  ; (2026-08-21 사용자 요청). `nsExec`는 NSIS에 딸린 플러그인이라 따로 받을 것이 없고
  ; 콘솔 창도 띄우지 않는다. 먼저 정상 종료를 청해 앱이 설정을 저장할 틈을 준다
  DetailPrint "$(CLOSING_APP)"
  nsExec::Exec '"$SYSDIR\taskkill.exe" /IM "${EXE_NAME}"'
  Pop $0
  Sleep 2000
  ; 트레이에 상주 중이면 창 닫기만으로는 끝나지 않아 강제 종료로 한 번 더 확인한다.
  ; 이미 끝났으면 taskkill이 「해당 프로세스 없음」으로 물러날 뿐이라 해가 없다
  nsExec::Exec '"$SYSDIR\taskkill.exe" /F /IM "${EXE_NAME}"'
  Pop $0
  ; 프로세스가 사라진 뒤에도 파일 잠금이 풀리기까지 잠깐 걸린다
  Sleep 500

  SetOutPath "$INSTDIR"
  File "..\target\release\moa.exe"
  File "..\LICENSE"
  File "..\THIRD-PARTY-NOTICES.md"

  CreateDirectory "$SMPROGRAMS\$(SHORTCUT_NAME)"
  CreateShortcut "$SMPROGRAMS\$(SHORTCUT_NAME)\$(SHORTCUT_NAME).lnk" "$INSTDIR\${EXE_NAME}"

  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; 「앱 및 기능」 목록에 뜨는 항목 — 사용자 단위 설치라 HKCU에 쓴다
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\${EXE_NAME}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
SectionEnd

; `un.onInit`을 두지 않는다 — `MUI_UNGETLANGUAGE`가 거기서 언어 선택 대화를 띄우는데,
; 제거에는 고를 것이 없다(2026-08-21 사용자 요청). 그것을 빼면 제거 화면은 Windows
; 표시 언어를 따르고, 바로가기는 어차피 두 이름을 모두 지우므로 언어가 갈려도 남지 않는다

Section "Uninstall"
  ; 바로가기는 **두 이름을 모두** 지운다 — 언인스톨러는 자기 언어를 새로 정하므로
  ; 설치 때 쓴 이름과 갈릴 수 있다. `Delete`는 없는 파일에 관대해 두 번 지워도 해가 없다
  Delete "$DESKTOP\모아.lnk"
  Delete "$DESKTOP\MOA.lnk"
  Delete "$SMPROGRAMS\모아\모아.lnk"
  Delete "$SMPROGRAMS\MOA\MOA.lnk"
  RMDir "$SMPROGRAMS\모아"
  RMDir "$SMPROGRAMS\MOA"

  ; 우리가 놓은 파일만 이름으로 지운다 — 런타임 생성물 둘을 함께 적는 이유는
  ; 설정이 이 폴더 안에 있기 때문이다(T1). 묻지 않고 지우는 것이 사용자 결정이다(D11)
  Delete "$INSTDIR\${EXE_NAME}"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\THIRD-PARTY-NOTICES.md"
  Delete "$INSTDIR\settings.json"
  Delete "$INSTDIR\known_hosts.json"
  ; 저장 도중 끊기면 남는 임시 파일 — 둘은 다 쓴 뒤에만 바꿔 끼워 저장된다(fs::atomic_write)
  Delete "$INSTDIR\settings.json.tmp"
  Delete "$INSTDIR\known_hosts.json.tmp"
  Delete "$INSTDIR\uninstall.exe"
  ; 자동 업데이트가 받아 둔 것 (FR-62) — 앱이 뜰 때마다 치우지만 받는 도중에 제거하면
  ; 남아 있을 수 있다. **이 폴더가 남으면 아래 비재귀 RMDir이 실패해 설치 폴더가 통째로
  ; 남는다** — 그래서 여기만 재귀로 지운다(우리가 만든 폴더이고 든 것도 우리가 받은 것뿐)
  RMDir /r "$INSTDIR\update"

  ; 재귀로 지우지 않는다 — 사용자가 넣어 둔 파일이 있으면 그대로 남긴다
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "${UNINST_KEY}"

  ; 자동 실행은 **이 설치본을 가리킬 때만** 지운다 — 개발 빌드로 켜 둔 값이 있으면
  ; 그것까지 지우게 되기 때문이다. 앱이 쓰는 값 형식은 `"<exe 경로>" --tray`다
  ReadRegStr $0 HKCU "${RUN_KEY}" "MOA"
  StrCmp $0 '"$INSTDIR\${EXE_NAME}" --tray' 0 +2
    DeleteRegValue HKCU "${RUN_KEY}" "MOA"
SectionEnd
