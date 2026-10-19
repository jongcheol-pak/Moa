# MOA 검증 지도

이 폴더는 MOA 의 사용자 기능을 검증하는 유지 대상 정본이다. 앱을 조작하기 전에 이 색인을 읽고, 맞는 기능 파일을 조작법으로 쓴다.

## 기본 전제

- `../helpers/launch.ps1 -RepoRoot <레포 루트>` 로 띄운 실행 사본만 조작한다. 다른 MOA 가 떠 있으면 띄우지 않는다(중복 실행 방지 뮤텍스).
- 빈 `settings.json` 에서 시작한다 — 창 1100x700, 워크스페이스 `워크스페이스 1` 하나, 탭 하나가 사용자 홈 폴더를 연다. 시드 데이터는 없고, 파일 기능은 실행 폴더의 빈 `work\`(`run.json` 의 `workDir`)에서 한다.
- `../helpers/doctor.ps1 -RunDir <실행 폴더>` 가 `DOCTOR: OK` 여야 조작한다.

## 조작 규칙

- 접근성 트리가 없어 모든 조작은 창 기준 좌표다. 좌표는 매번 직전 `shot.ps1` 캡처에서 읽고, 기능 파일에는 자리를 찾는 기준만 적는다.
- 조작은 `<karina-cli> computer click|hotkey|type-text|press-key --app pid:<PID> --window-id <창 id> --no-screenshot --json` 이다(`../SKILL.md` Drive).
- 키 입력 전에 대상 영역(목록·입력란)을 한 번 클릭해 포커스를 옮긴다.
- 정리 때 증거 파일은 지우지 않는다(`../helpers/stop.ps1`).

## 증명과 건너뜀 보고

- 조작 전후 캡처(`shot.ps1`)와 부수 효과(파일시스템·`settings.json`)를 함께 남긴다.
- 증거마다 기능 ID 와 쓴 진입 경로를 적는다.
- 닿지 못한 경로는 시도한 명령과 채우지 못한 전제와 함께 보고한다. 건너뛴 경로를 다른 경로로 검증했다고 보고하지 않는다.

## 기능 파일 규칙

H1 제목과 사용자에게 보이는 동작 한 문단 뒤에 H2 넷을 이 순서로 쓴다: `Sub-features` · `How to get to it (user POV)` · `Driving it with karina-cli` · `Gotchas`. 구현 세부는 넣지 않는다.

## 기능

| 기능 | 파일 | 코드 위치(변경 검증에서 고를 때) |
|---|---|---|
| 새 폴더·새 파일 만들기 | [create-item.md](./create-item.md) | `src/ui/panel*`·`src/ui/app.rs`(패널 메뉴·단축키) · `src/fs/create.rs`(이름 번호 규칙) · `src/fs/file_op.rs` |
| 주소창 탐색 | [navigate.md](./navigate.md) | `src/ui/address_bar.rs` · `src/panel/`(히스토리) · `src/fs/`(열거) · `src/ui/file_list.rs`·`list_*.rs` |
| 워크스페이스 만들기·이름 | [workspace.md](./workspace.md) | `src/ui/sidebar.rs` · `src/ui/titlebar.rs` · `src/app/workspace.rs` · `src/app/settings.rs`(세션 저장) |
| 설정 메뉴·설정 화면 | [settings-menu.md](./settings-menu.md) | `src/ui/titlebar.rs` · `src/ui/menu.rs` · `src/ui/settings_dialog.rs` · `src/ui/about_dialog.rs` · `src/ui/license_dialog.rs` · `src/i18n/` |
