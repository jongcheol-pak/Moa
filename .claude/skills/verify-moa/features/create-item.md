# 새 폴더·새 파일 만들기

사용자는 패널이 보고 있는 로컬 폴더에 새 폴더나 새 파일을 만든다. 이름은 `새 폴더`(`New folder`)이고, 같은 이름이 있으면 `새 폴더 (2)`, `새 폴더 (3)`… 로 번호가 붙어 기존 항목을 덮어쓰지 않는다. 만든 항목은 곧바로 목록에 나타나고 상태 줄의 `폴더 N 파일 N` 이 늘어난다.

## Sub-features

- `create-folder-menu` — 패널 메뉴의 `새 폴더` 로 폴더를 만든다.
- `create-folder-key` — `Ctrl+Shift+N` 으로 폴더를 만든다.
- `create-file-menu` — 패널 메뉴의 `새 파일` 로 파일을 만든다.
- `create-dedupe` — 같은 이름이 있으면 `(2)` 번호를 붙인다.

## How to get to it (user POV)

- 탭 줄 맨 오른쪽 끝 패널 메뉴 버튼 → `새 폴더`(`New folder`).
- 목록에 포커스가 있을 때 `Ctrl+Shift+N`.
- 탭 줄 맨 오른쪽 끝 패널 메뉴 버튼 → `새 파일`(`New file`).

## Driving it with karina-cli

Preconditions:

- `doctor.ps1` 이 `DOCTOR: OK`.
- 활성 패널이 `run.workDir` 을 보고 있다 — `navigate.md` 의 「경로 입력」으로 옮긴다. 목록이 `이 폴더는 비어 있습니다` 이고 상태 줄이 `폴더 0 파일 0`.

- **패널 메뉴 열기.** 탭 줄과 같은 높이, 패널 오른쪽 끝의 사각 분할 아이콘 버튼을 클릭한다. `shot.ps1 -Name <n>-create-menu -NoRaise` 로 찍는다. 버튼 아래로 `오른쪽 분할`·`왼쪽 분할`·`위쪽 분할`·`아래쪽 분할`·`새로 고침 F5`·`새 파일`·`새 폴더`·`닫기`(마지막 패널이면 흐림) 메뉴가 보인다.
- **새 폴더(메뉴).** 메뉴의 `새 폴더` 줄을 클릭한다. 1초 뒤 `shot.ps1 -Name <n>-create-folder-menu -NoRaise`. 목록에 `새 폴더` 줄(종류 `파일 폴더`, 수정한 날짜 지금)이 생기고 상태 줄이 `폴더 1 파일 0`. `Get-ChildItem $run.workDir` 에 `새 폴더` 디렉터리가 있다.
- **새 폴더(단축키)·번호.** 목록의 빈 자리를 클릭한 뒤 `hotkey --key CmdOrCtrl+Shift+N`. 1초 뒤 캡처하면 `새 폴더 (2)` 가 생기고 상태 줄이 `폴더 2 파일 0`. `Get-ChildItem` 에 두 폴더가 있다.
- **새 파일(메뉴).** 패널 메뉴 → `새 파일` 을 클릭한다. 목록과 `Get-ChildItem` 에 파일이 하나 생기고 상태 줄의 파일 수가 1 늘어난다.
- **증명.** 각 단계의 캡처와 `Get-ChildItem $run.workDir | Select-Object Name,Mode` 출력을 `evidence\<n>-<기능 ID>.fs.txt` 로 남긴다.

## Gotchas

- 단축키는 포커스가 주소창·거르기 입력란에 있으면 글자 입력으로 먹힐 수 있다 — 목록 빈 자리를 먼저 클릭한다.
- 패널 메뉴는 `shot.ps1` 기본 동작(창 최소화·복원)에 닫힌다 — 메뉴가 열린 상태는 `-NoRaise` 로 찍는다.
- 만들기는 워커 스레드에서 돌아 목록 반영이 한 박자 늦다 — 1초 기다린 뒤 찍는다.
- 실패 사유는 목록 위에 뜬다. 읽기 전용 폴더에서 시험하려면 `work\` 아래에 따로 만든다(사용자 폴더를 쓰지 않는다).
- 2026-10-19 증명: `create-folder-menu`·`create-folder-key`·`create-dedupe` 통과. `create-file-menu` 는 아직 조작하지 않았다.
