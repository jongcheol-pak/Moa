# Intent: 원격 폴더 재귀 삭제
Author: 사용자. Status: approved.

## Problem
사용자 보고(2026-09-23): 「ftp 탭의 목록에서 폴더를 다운로드/삭제 시 동작하지 않음. 파일 다운로드/삭제는 잘 됨」. 다운로드는 같은 날 `b658352`로 고쳤다. 삭제는 폴더에 `RMD` 한 번만 보내는 비재귀 설계라, 비어 있지 않은 폴더는 서버가 `550 Directory not empty`로 거절해 아무 일도 일어나지 않는 것처럼 보인다.

## Proposed outcome
원격 목록에서 폴더를 삭제하면 안의 파일·하위 폴더까지 지워지고 목록에서 사라진다. 일부가 지워지지 않으면 나머지는 계속 지우고, 몇 개를 못 지웠는지 상태 줄에, 항목별 사유를 서버 로그에 남긴다. 확인 대화는 한 번이며, 고른 것에 폴더가 있으면 안의 내용까지 지워진다는 경고가 붙는다.

## Affected users and systems
원격(FTP·FTPS·SFTP) 탭 사용자. 연결 워커(`remote::connection`), 원격 삭제 명령 조립·결과 처리(`ui::app::remote`), 삭제 확인 대화(`ui::remote_menu`), 오류 종류(`remote::types`), 가짜 서버(`remote::testing`), i18n, README.

## Constraints
심볼릭 링크는 따라 들어가지 않고 링크만 지운다. 훑기는 깊이 상한과 종료 신호를 지킨다. 삭제는 워커 스레드에서 돈다(UI 스레드 블로킹 금지). 연결이 도중에 끊기면 그 자리에서 멈추고 끊김 판정을 거친다.

## Open questions
없음
