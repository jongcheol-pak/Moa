---
name: verify-moa
description: MOA(egui/glow 기반 Windows 파일 탐색기 데스크톱 앱)를 레포에서 빌드해 임시 폴더의 격리 사본으로 띄우고, Karina CLI(karina-computer-use)로 화면 좌표를 눌러 사용자처럼 조작하고, 화면 캡처·파일시스템·settings.json 으로 증거를 남긴다. 코드 변경을 실제 앱에서 증명할 때(pjc:implement 의 화면 확인, pr-verifier 의 merge 전 검증) 쓴다.
---

# verify-moa

MOA 를 실제로 띄워 사용자 경로로 조작하고 증거를 남기는 절차다. 헬퍼는 모두 `helpers/` 에 있고 `pwsh -NoProfile -File <헬퍼> ...` 로 부른다. 아래에서 `$H` 는 이 스킬 폴더의 `helpers` 절대 경로, `$R` 는 `launch.ps1` 이 마지막 줄 `RUN_DIR=` 로 알려 준 실행 폴더다.

## 이 앱의 성질 — 조작 전에 알아야 할 것

- **표면**: Windows 데스크톱 GUI 하나다(콘솔 창 없음). CLI·HTTP 표면은 없다.
- **접근성 트리가 없다** — eframe 을 `default-features = false, features = ["glow"]` 로 써서 accesskit 이 빠져 있다. Karina 트리에는 OS 창 틀(제목 표시줄·최소화·복원·닫기)만 잡힌다. **모든 조작은 창 기준 좌표 클릭·키 입력**이고, 좌표는 매번 직전 캡처에서 읽는다.
- **Karina 스크린샷은 쓰지 않는다** — `get-app-state` 의 캡처(PrintWindow)는 MOA 의 OpenGL 창을 빈 회색으로 잡는다(`--restore-window` 를 줘도 같다, 2026-10-19 실측). 캡처는 `shot.ps1` 이 창을 앞으로 올린 뒤 화면 영역을 직접 복사한다.
- **한 로그온 세션에 MOA 는 하나만 뜬다** — `Local\MOA-single-instance` 뮤텍스. 다른 MOA(설치본 `%LOCALAPPDATA%\Programs\MOA\moa.exe` 포함, 트레이에 숨어 있어도)가 떠 있으면 새로 띄운 것은 그 창을 깨우고 스스로 끝난다. **두 인스턴스를 나란히 띄울 수 없다.** `launch.ps1` 은 다른 MOA 가 보이면 띄우지 않고 실패한다 — 사용자에게 그 MOA 를 종료해 달라고 하고, 남의 프로세스를 내리지 않는다.
- **데이터 격리** — 설정·세션은 실행 파일 옆 `settings.json`, SSH 지문은 `known_hosts.json` 이다. `launch.ps1` 이 빌드 산출물을 실행 폴더 `app\` 로 복사해 띄우므로 매 실행이 빈 설정에서 시작하고 개발 빌드·설치본 설정을 건드리지 않는다. 사본 폴더에 언인스톨러가 없어 자동 업데이트 확인도 돌지 않는다.
- **화면 언어**는 빈 설정에서 `시스템 기본`이라 Windows UI 언어를 따른다(한국어 PC 면 한국어). 이 문서와 `features/` 의 라벨은 한국어 기준이고 영어 라벨을 괄호에 함께 적는다.

## Launch

```powershell
pwsh -NoProfile -File "$H\launch.ps1" -RepoRoot <레포 루트> [-Release] [-RunId <이름>]
```

- `-RepoRoot` 로 받은 레포에서 `cargo build`(`-Release` 면 `--release`)를 돌리고 `target\<profile>\moa.exe` 를 `%TEMP%\verify-moa\<RunId>\app\moa.exe` 로 복사해 띄운다. BASE worktree 대조는 그 worktree 경로를 `-RepoRoot` 로, 다른 `-RunId` 로 따로 띄운다(동시에는 못 띄운다 — 하나를 `stop.ps1` 로 내린 뒤 띄운다).
- **준비 판정**: 그 PID 의 제목 있는 주 창(`모아`/`MOA`)이 30초 안에 나타나야 한다. 나타나면 `PID=… WINDOW=… SIZE=…` 와 `RUN_DIR=…` 를 찍는다. 빈 설정이라 창은 1100x700 이고 첫 워크스페이스 `워크스페이스 1` 이 사용자 홈 폴더를 연 상태다.
- 실패 메시지가 「다른 MOA 가 실행 중」이면 사용자에게 종료를 부탁하고 멈춘다.
- **종료**는 Cleanup 의 `stop.ps1` 이다.

## Doctor

```powershell
pwsh -NoProfile -File "$H\doctor.ps1" -RunDir $R
```

읽기 전용이다. Karina CLI 와 `skills get karina-computer-use` exit 0 · PID 생존과 경로가 실행 사본인지 · 다른 MOA 부재 · 사본 해시가 launch 때 및 레포 빌드 산출물과 같은지 · 주 창 보임 · Karina 가 창 상태를 읽는지 · 창 id 가 launch 때와 같은지를 줄마다 `OK`/`FAIL` 로 찍는다. `DOCTOR: OK` 가 아니면 조작하지 않고 그 출력을 보고한다. 조작이 이상하게 먹힐 때 가장 먼저 돌린다.

## Drive

조작은 Karina CLI 의 `computer` 명령이다. CLI 고르는 순서·명령 형식의 정본은 `<karina-cli> skills get karina-computer-use` 출력이다. 이 앱에서 쓰는 형태:

```powershell
$run = Get-Content "$R\run.json" -Raw | ConvertFrom-Json
$k   = $env:KARINA_CLI_COMMAND    # 없으면 helpers\common.ps1 의 Get-KarinaCli 순서
$a   = @('--app', "pid:$($run.pid)", '--window-id', "$($run.windowId)", '--no-screenshot', '--json')
& $k computer click --x <창 x> --y <창 y> --restore-window @a   # 첫 클릭은 창을 앞으로 올린다
& $k computer hotkey --key CmdOrCtrl+A @a
& $k computer type-text --text "<문자열>" @a
& $k computer press-key --key Return @a
```

- **좌표는 창 기준 물리 픽셀**이다. `shot.ps1` 의 캡처가 창 왼쪽 위를 (0,0) 으로 같은 크기로 잡으므로, 캡처 그림의 픽셀 좌표를 그대로 넘긴다. 이 대응은 디스플레이 배율 100% 에서만 실측했다(2026-10-19) — 배율이 다른 모니터면 첫 클릭 뒤 캡처로 클릭이 의도한 자리에 먹혔는지 확인하고, 어긋나면 그 사실을 보고한다.
- 좌표값을 기억해 쓰지 않는다. **매 조작 전 캡처에서 아래 기준으로 자리를 찾는다**:
  - 타이틀바(맨 위 줄): 왼쪽 앱 아이콘 옆 사이드바 토글, 가운데 활성 워크스페이스 이름, 오른쪽 톱니(설정 메뉴)·최소화·최대화·닫기
  - 사이드바(왼쪽 세로 띠): `워크스페이스` 제목 줄 오른쪽 `+`, 그 아래 워크스페이스 항목(이름·경로 2줄), 그 아래 `연결` 제목 줄의 새로 고침·`+`
  - 패널(오른쪽): 탭 줄(탭 · `+` · 맨 오른쪽 끝 패널 메뉴 버튼) → 주소 줄(뒤로·앞으로·위로 화살표, 경로 입력란, 오른쪽 `이름으로 거르기` 입력란) → 상태 줄(왼쪽 폴더 트리·보기 버튼, 오른쪽 `폴더 N 파일 N`) → 목록 머리글(`이름`·`크기`·`종류`·`수정한 날짜`) → 목록
  - 맨 아래: `전송 큐` 줄
- `click`·`type-text`·`press-key` 의 `action.verification` 은 늘 `synthetic_input`(미확인)이다. **입력이 먹혔는지는 다음 캡처와 부수 효과로만 판정한다.**
- 입력란 글자 입력은 `type-text` 를 쓴다. `paste-text` 는 사용자 클립보드를 거치므로 쓰지 않는다.
- 팝업 메뉴는 `Escape` 로 닫는다.
- 기능별 조작법은 `features/README.md` 색인에서 고른다.

## Evidence

```powershell
pwsh -NoProfile -File "$H\shot.ps1" -RunDir $R -Name <증거 이름> [-NoRaise]
```

- 증거는 **레포 밖** `%TEMP%\verify-moa\<RunId>\evidence\` 에 둔다. `shot.ps1` 은 `<이름>.png`(창 캡처, 제목 표시줄 포함)와 `<이름>.json`(창 위치·크기·제목)을 남긴다. 같은 이름의 증거가 이미 있으면 덮어쓰지 않고 실패한다.
- `shot.ps1` 은 기본으로 창을 최소화·복원해 앞으로 올린 뒤(화면이 한 번 깜박인다) 전경 창이 MOA 인지 확인하고 찍는다. 팝업 메뉴처럼 앞으로 올리면 닫히는 상태를 찍을 때는 `-NoRaise` 를 준다 — 그래도 전경이 MOA 가 아니면 찍지 않고 실패한다.
- 증거 기준:
  - 조작 전 캡처와 조작 후 캡처를 함께 남긴다. 이름은 `<순번>-<기능 ID>-<단계>` 로 짓는다(예: `12-create-menu-after`).
  - 부수 효과는 앱 밖에서 다시 읽는다 — 파일 작업은 `Get-ChildItem $run.workDir` 결과를 `evidence\<이름>.fs.txt` 로, 세션·설정 변경은 실행 사본 옆 `settings.json` 을 읽는다(`stop.ps1` 이 `evidence\settings.final.json` 으로 복사해 둔다).
  - 내부 상태를 바꾸는 우회로(설정 파일 직접 편집 후 재시작 등)는 조작으로 치지 않는다. 사용자 경로로 조작한다.
  - 사용자 홈 등 실제 사용자 폴더 목록이 증거 그림에 찍히지 않게, 파일 기능은 먼저 `run.workDir` 로 이동한 뒤 조작한다.

## Cleanup

```powershell
pwsh -NoProfile -File "$H\stop.ps1" -RunDir $R
```

- `run.json` 의 PID 가 실행 사본 경로일 때만 내린다(창 닫기 → 5초 뒤 남아 있으면 강제 종료). 경로가 다르면 PID 가 재사용된 남의 프로세스라 건드리지 않는다.
- `settings.json` 을 `evidence\settings.final.json` 으로 복사한 뒤 `app\`·`work\` 를 지운다. **`evidence\` 와 `run.json` 은 남긴다.**
- launch 가 준비 판정에서 실패해도 `run.json` 은 이미 있으므로 같은 명령으로 내린다.

## 변경 검증

코드 변경 하나를 증명할 때 이 순서로 한다.

1. **입력이 있으면 그 시나리오를 그대로 돈다** — 계획의 `**화면**` 줄이나 PR 본문의 `화면 시나리오:` 줄이 오면 Launch → Doctor → 그 시나리오의 조작을 Drive 로 하고, 시나리오가 이름 댄 스크린샷을 그 이름으로 `shot.ps1 -Name` 에 남기고, 시나리오의 통과 조건으로 판정한다 → Cleanup. BASE 에서도 돌리라는 입력이면 현재 인스턴스를 `stop.ps1` 로 내린 뒤 BASE worktree 경로를 `-RepoRoot` 로, 다른 `-RunId` 로 Launch 해 같은 조작을 반복한다.
2. **입력이 없으면 지도로 고른다** — 바뀐 파일이 닿는 기능을 `features/README.md` 색인의 「코드 위치」로 고르고, 고른 기능 파일의 `How to get to it (user POV)` 에 적힌 **모든 진입 경로**를 조작한다. 편한 경로 하나만 조작한 증명은 불완전하다.
3. **어느 기능에도 닿지 않으면** 그 사실과 근거(바뀐 파일·그 파일이 화면에 닿지 않는 이유)를 적고 앱 조작을 건너뛴다.
4. 보고에는 기능 ID · 쓴 진입 경로 · 증거 파일 경로 · 판정을 적는다. 닿지 못한 경로는 시도한 명령과 채우지 못한 전제와 함께 적고, 다른 경로로 검증했다고 적지 않는다.
