# Intent: 끊긴 연결에서 전송 재시도가 계속 실패하는 문제
Author: 사용자. Status: approved.

## Problem

사용자 보고 — *"ftp에 파일 전시 ftp에 연결이 끊어져서 실패한 경우 목록에서 재시도를 하면 계속 실패 하는데 이유가 뭐지?"*
디버깅으로 원인 셋이 확정됐다: ① `FtpSession`이 전송 시작 단계의 실패를 재수립 대상으로 표시하지 않아 죽은 제어 채널을 그대로 쥔다 ② 전송 배정이 연결 상태를 보지 않아 끊긴 연결에 계속 밀어 넣는다 ③ 전송 실패가 끊김으로 판정되지 않아 탭이 계속 `연결됨`으로 보인다. 지금 하는 이유는 재시도가 회복으로 이어지는 길이 현재 코드에 없어 사용자가 앱을 다시 켜는 것 말고는 방법이 없기 때문이다.

## Proposed outcome

전송이 어느 단계에서 실패하든 그 연결이 다시 서거나 끊긴 것으로 표시되고, 끊긴 연결에는 전송이 배정되지 않아 항목이 `대기 중`으로 남았다가 재연결되면 저절로 이어진다.

## Affected users and systems

원격 사이트로 파일을 주고받는 MOA 사용자. 걸리는 코드는 `src/remote/ftp.rs`(재수립 표시) · `src/remote/connection.rs`(끊김 판정) · `src/remote/transfer.rs`(전송 배정)이며, SFTP는 `connection.rs`의 끊김 판정이 함께 덮는다.

## Constraints

어긋난(`needs_reset`) FTP 세션에는 `NOOP`을 보내지 않는다 — `suppaftp`가 제어 채널의 응답 읽기를 노출하지 않아 밀린 응답을 비울 수 없다. 기존 시험 1417건 통과와 `clippy -D warnings` 0을 유지한다.

## Open questions

없음
