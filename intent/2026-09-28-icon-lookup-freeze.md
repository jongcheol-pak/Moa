# Intent: 시작 시 폴더 목록 아이콘 조회 프리징 해소
Author: 사용자. Status: approved.

## Problem
사용자 요청(2026-09-28): 「앱이 실행되면 폴더 목록을 가져오면서 ui 프리징이 심하게 발생하는데 확인해서 수정」. 파일 목록이 보이는 exe·lnk·ico 행마다 셸 아이콘을 UI 스레드에서 실제 경로로 조회해, 처음 보는 exe 46개 폴더로 시작하면 한 프레임이 774ms 멈추고 이어 새 아이콘 텍스처 변환으로 35~100ms 프레임이 이어진다(같은 날 계측 실측).

## Proposed outcome
처음 보는 exe가 많은 폴더를 열어도 창이 멈추지 않는다 — 같은 시나리오에서 느린 프레임이 50ms를 넘지 않는다. exe·lnk·ico 행은 잠깐 확장자 기본 아이콘으로 보였다가 곧 파일 고유 아이콘으로 바뀐다.

## Affected users and systems
로컬 폴더를 여는 모든 MOA 사용자. 범위는 파일 목록의 아이콘 조회와 텍스처 변환이다 — 트리 하위 폴더·즐겨찾기 아이콘은 사용자 선택으로 제외(2026-09-28). 셸 아이콘 캐시(`fs::icons`), 목록 렌더(`ui::list_details`·`ui::list_grid`·`ui::drag_preview`), 아이콘 텍스처(`ui::icon_tex`), 앱 프레임 루프(`ui::app`), README.

## Constraints
UI 스레드에서 블로킹 셸 조회를 하지 않는다. `fs`는 `ui`를 모른다. 목록의 행별 아이콘 캐시가 임시 아이콘을 영구로 굳히지 않는다. 셸 잠금 규약(`shell_guard`)을 지킨다.

## Open questions
없음
