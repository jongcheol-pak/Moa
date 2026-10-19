# 워크스페이스 만들기·이름

사용자는 사이드바에서 워크스페이스를 더하고 이름을 정한다. 새 워크스페이스는 자기만의 패널·탭을 가진 탐색기 화면이 되고, 타이틀바 가운데에 활성 워크스페이스 이름이 뜬다. 바뀐 워크스페이스 목록은 곧바로 `settings.json` 에 저장된다.

## Sub-features

- `ws-add` — 사이드바 `워크스페이스` 줄의 `+` 로 새 워크스페이스를 만들고 이름 입력란을 연다.
- `ws-name` — 이름 입력란에서 Enter 로 이름을 확정한다(기본값 `워크스페이스 N`).
- `ws-switch` — 항목을 클릭해 워크스페이스를 바꾼다.
- `ws-persist` — 목록이 `settings.json` 에 저장된다.

## How to get to it (user POV)

- 사이드바 `워크스페이스`(`Workspaces`) 제목 줄 오른쪽 `+`.
- 사이드바 워크스페이스 항목 클릭(전환), `F2`·우클릭(이름 바꾸기).

## Driving it with karina-cli

Preconditions:

- `doctor.ps1` 이 `DOCTOR: OK`. 사이드바가 펼쳐져 있다(접혀 있으면 타이틀바 왼쪽 사이드바 토글을 클릭).

- **추가.** 사이드바 `워크스페이스` 제목 줄 오른쪽 끝 `+` 를 `click --restore-window` 로 클릭한다. 1초 뒤 `shot.ps1 -Name <n>-ws-add -NoRaise`. 첫 항목 아래에 `워크스페이스 2` 가 선택된 이름 입력란이 생기고, 타이틀바 가운데가 `워크스페이스 2`, 오른쪽 패널이 사용자 홈을 연다.
- **이름 확정.** `press-key --key Return`. 1.5초 뒤 캡처하면 입력란이 닫히고 항목 이름이 `워크스페이스 2` 다. 다른 이름을 시험하려면 Enter 전에 `hotkey --key CmdOrCtrl+A` 와 `type-text --text "<이름>"`.
- **저장.** 실행 사본 옆 `settings.json` 에서 `"name": "워크스페이스 2"` 를 찾는다: `Select-String -Path (Join-Path (Split-Path $run.exe) settings.json) -Pattern '"name": "워크스페이스'`. 두 줄(1·2)이 나온다.
- **전환.** 사이드바 첫 항목(`워크스페이스 1`)을 클릭한다. 타이틀바 가운데와 오른쪽 패널이 그 워크스페이스로 바뀐다.

## Gotchas

- 새 워크스페이스는 사용자 홈 폴더를 연다 — 그 목록이 캡처에 찍힌다. 증거를 남에게 보일 것이면 이름 확정 뒤 `navigate.md` 로 `work\` 로 옮기고 찍는다.
- 자동 저장은 바뀐 뒤 잠깐 조용해야 쓴다 — 확정 직후 바로 읽지 말고 1~2초 기다린다. `stop.ps1` 이 남기는 `evidence\settings.final.json` 으로도 확인할 수 있다.
- 삭제는 확인 대화를 거친다 — 이 지도는 아직 삭제 경로를 다루지 않는다.
- 2026-10-19 증명: `ws-add`·`ws-name`·`ws-persist` 통과. `ws-switch` 는 아직 조작하지 않았다.
