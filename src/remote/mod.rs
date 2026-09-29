//! 원격 연결 계층 — FTP·FTPS·SFTP (FR-27~FR-46).
//!
//! `fs`(로컬 Win32 파일시스템 전담)와 나란한 최상위 모듈이다. 원격을 `fs`에 섞지 않는 이유는
//! 그쪽이 셸·Win32 핸들 전제로 짜여 있어 책임이 흐려지기 때문이다 (plan D5).
//!
//! 의존은 단방향이다 — `ui`가 이쪽을 참조하고, 이 모듈은 `ui`(egui)를 모른다 (AGENTS).
//! 그래서 워커가 화면을 깨워야 할 때는 egui를 직접 부르지 않고 **깨우기 콜백을 주입받는다**
//! (plan D6 — 썸네일 워커가 겪은 "화면이 50ms마다 스스로 확인" 우회를 되풀이하지 않기 위함).
//!
//! **모든 원격 I/O는 연결별 워커 스레드에서 일어난다** — UI 스레드는 채널을 폴링만 한다
//! (NFR-10). async 런타임은 쓰지 않는다: `suppaftp`·`ssh2` 둘 다 동기 API라
//! `std::thread` + 채널로 충분하다 (AGENTS 동시성 규약).
pub mod charset;
pub mod connection;
pub mod envelope;
pub mod ftp;
pub mod hostkey;
pub mod log;
pub mod manager;
pub mod queue;
pub mod secret;
pub mod sftp;
pub mod site_export;
pub mod sites;
pub mod testing;
pub mod transfer;
pub mod tree_cache;
pub mod types;
pub mod url;
/// 임시 FTP 명령 기록 — `MOA_FTP_TRACE=1`일 때만 동작한다 (원인 규명 후 걷어낸다)
pub mod wire_trace;

use std::io::{Read, Write};

use crate::remote::types::Progress;

/// 전송 버퍼 크기 — 파일을 통째로 메모리에 올리지 않기 위한 고정 값 (D12·NFR-12).
///
/// 64KB는 TCP 창을 채울 만큼 크면서, 동시 4건 전송에서도 버퍼 몫이 256KB에 그쳐
/// NFR-12 임계(유휴 대비 +50MB)에 여유 있게 머문다.
pub(crate) const TRANSFER_BUFFER: usize = 64 * 1024;

/// `pump`가 멈춘 이유
pub(crate) enum Pumped {
    Done(u64),
    Cancelled,
    Failed { transferred: u64, detail: String },
}

/// 64KB씩 옮기며 그때까지의 누적 바이트를 보고한다 (NFR-12).
///
/// 파일 크기와 무관하게 상주 메모리가 버퍼 한 장에 머무는 것이 이 함수의 존재 이유다.
/// 보고가 `false`를 돌려주면 그 자리에서 멈춘다 — 취소를 위한 별도 채널을 두지 않는다.
///
/// 프로토콜 모듈(`ftp`·`sftp`) 양쪽이 같은 것을 쓰므로 여기 둔다 — 사본이 둘이면 한쪽만
/// 고쳐졌을 때 NFR-12가 조용히 깨진다.
pub(crate) fn pump(
    src: &mut dyn Read,
    dest: &mut dyn Write,
    progress: &mut dyn Progress,
) -> Pumped {
    let mut buffer = vec![0u8; TRANSFER_BUFFER];
    let mut total = 0u64;
    loop {
        let read = match src.read(&mut buffer) {
            Ok(0) => return Pumped::Done(total),
            Ok(n) => n,
            // 신호로 끊긴 읽기는 실패가 아니다
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Pumped::Failed {
                    transferred: total,
                    detail: e.to_string(),
                };
            }
        };
        if let Err(e) = dest.write_all(&buffer[..read]) {
            return Pumped::Failed {
                transferred: total,
                detail: e.to_string(),
            };
        }
        total += read as u64;
        if !progress.report(total) {
            return Pumped::Cancelled;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    /// 보고 횟수와 마지막 값을 세는 진행 통지 — 취소 지점을 지정할 수 있다
    struct Counter {
        calls: usize,
        last: u64,
        cancel_after: Option<usize>,
    }

    impl Counter {
        fn new() -> Counter {
            Counter {
                calls: 0,
                last: 0,
                cancel_after: None,
            }
        }
    }

    impl Progress for Counter {
        fn report(&mut self, transferred: u64) -> bool {
            self.calls += 1;
            self.last = transferred;
            self.cancel_after.is_none_or(|limit| self.calls < limit)
        }
    }

    #[test]
    fn 전송은_64kb마다_진행을_보고한다() {
        // 200KB 입력 → 64KB 버퍼로 4번에 나뉜다 (인메모리 스트림이라 서버가 필요 없다)
        let source = vec![7u8; 200 * 1024];
        let mut src = Cursor::new(source.clone());
        let mut dest: Vec<u8> = Vec::new();
        let mut progress = Counter::new();

        let outcome = pump(&mut src, &mut dest, &mut progress);
        assert!(matches!(outcome, Pumped::Done(total) if total == source.len() as u64));
        assert_eq!(dest, source, "옮긴 내용이 원본과 같아야 한다");
        assert!(
            progress.calls >= 4,
            "64KB마다 보고해야 한다 (실제 {}회)",
            progress.calls
        );
        assert_eq!(progress.last, source.len() as u64);
    }

    #[test]
    fn 진행_보고가_거짓이면_전송이_그_자리에서_멈춘다() {
        let mut src = Cursor::new(vec![1u8; 200 * 1024]);
        let mut dest: Vec<u8> = Vec::new();
        let mut progress = Counter::new();
        progress.cancel_after = Some(2);

        let outcome = pump(&mut src, &mut dest, &mut progress);
        assert!(matches!(outcome, Pumped::Cancelled));
        assert_eq!(progress.calls, 2);
        assert_eq!(dest.len(), 2 * TRANSFER_BUFFER, "취소 후로는 옮기지 않는다");
    }

    #[test]
    fn 전송_중_실패는_그때까지_옮긴_바이트를_담는다() {
        /// 한 번 쓰고 나면 끊기는 대상 — 전송 중 연결 끊김을 흉내 낸다
        struct Flaky {
            written: usize,
        }
        impl Write for Flaky {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                if self.written > 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::ConnectionReset,
                        "connection reset",
                    ));
                }
                self.written += buf.len();
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let mut src = Cursor::new(vec![9u8; 200 * 1024]);
        let mut dest = Flaky { written: 0 };
        let mut progress = Counter::new();

        match pump(&mut src, &mut dest, &mut progress) {
            Pumped::Failed {
                transferred,
                detail,
            } => {
                // 이어받기 시작점이 되므로 0이면 안 된다
                assert_eq!(transferred, TRANSFER_BUFFER as u64);
                assert!(detail.contains("connection reset"), "원문: {detail}");
            }
            _ => panic!("전송 실패를 기대했다"),
        }
    }
}
