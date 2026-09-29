//! 임시 FTP 명령 기록 — 어느 연결이 어떤 명령을 보내고 서버가 무엇을 답했는지 실측하기 위한
//! 것이며 **기본은 꺼져 있다** (FTP 폴더 삭제 550 조사, 2026-09-28).
//!
//! `suppaftp`는 제어 채널로 오가는 줄을 전부 `log::trace!`로 내보낸다(`CC OUT:`/`CC IN:`).
//! 이 모듈은 그것을 받아 적는 로거다. `MOA_FTP_TRACE=1`로 켜면 **실행 파일 옆**
//! `ftp-trace.log`에 `시각 · 스레드 · 줄`을 덧붙인다. 스레드가 곧 연결 하나라 탐색 연결과
//! 전송 연결이 갈린다. 라이브러리가 응답의 첫 줄을 두 번 찍으므로 같은 `CC IN` 줄이 연달아
//! 두 번 보이는 것은 정상이다.
//!
//! **자격증명·주소를 적지 않는다** — 라이브러리가 `PASS`를 평문으로 찍고, `USER`·`PASV`
//! 응답·`PORT`에는 계정과 IP가 실린다. 그 인자는 가린 뒤 적는다(`mask`). 원격 경로·파일
//! 이름은 남는다 — 그것이 조사 대상이다.
//!
//! 원인을 가른 뒤에는 이 모듈째 걷어낸다.
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;

struct WireTrace {
    start: Instant,
    /// 여러 연결 스레드가 한 파일에 덧붙인다 — 줄이 섞이지 않게 한 번에 하나씩 쓴다
    file: Mutex<Option<std::fs::File>>,
}

impl ::log::Log for WireTrace {
    fn enabled(&self, metadata: &::log::Metadata) -> bool {
        metadata.target().starts_with("suppaftp")
    }

    fn log(&self, record: &::log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let Some(line) = mask(&record.args().to_string()) else {
            return;
        };
        let since = self.start.elapsed().as_secs_f32();
        let thread = std::thread::current().id();
        let Ok(mut guard) = self.file.lock() else {
            return;
        };
        if let Some(file) = guard.as_mut() {
            let _ = writeln!(file, "[{since:9.3}] {thread:?} {line}");
        }
    }

    fn flush(&self) {}
}

/// `MOA_FTP_TRACE=1`이면 로거를 건다 — 꺼져 있으면 아무것도 하지 않는다.
///
/// 파일을 열지 못하면 조용히 지나간다. 기록이 실행을 막아서는 안 된다
pub fn install() {
    let on =
        std::env::var_os("MOA_FTP_TRACE").is_some_and(|value| value != "0" && !value.is_empty());
    if !on {
        return;
    }
    let file =
        log_path().and_then(|path| OpenOptions::new().create(true).append(true).open(path).ok());
    let logger = Box::new(WireTrace {
        start: Instant::now(),
        file: Mutex::new(file),
    });
    if ::log::set_boxed_logger(logger).is_ok() {
        ::log::set_max_level(::log::LevelFilter::Trace);
    }
}

/// 기록 파일 자리 — 실행 파일 옆(`settings.json`과 같은 규칙)
fn log_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("ftp-trace.log"))
}

/// 적을 줄만 골라 자격증명·주소를 가린다 — 제어 채널 줄(`CC OUT:`/`CC IN:`)이 아니면 버린다.
///
/// 버리는 것에는 `Signin in with user '…'`·`Connecting to server {주소}`·`Passive address`
/// 같은 라이브러리의 설명 줄이 있다 — 계정·IP를 싣고, 조사에 필요한 것은 오간 명령뿐이다
fn mask(message: &str) -> Option<String> {
    if let Some(command) = message.strip_prefix("CC OUT: ") {
        let verb = command.split(' ').next().unwrap_or_default();
        let hidden = matches!(
            verb.to_ascii_uppercase().as_str(),
            "USER" | "PASS" | "ACCT" | "PORT" | "EPRT"
        );
        return Some(if hidden && verb.len() < command.len() {
            format!("CC OUT: {verb} ***")
        } else {
            message.to_owned()
        });
    }
    if let Some(bytes) = message.strip_prefix("CC IN: ") {
        let reply = decode_bytes(bytes)?;
        let reply = reply.trim_end();
        let code = reply
            .get(..3)
            .filter(|c| c.bytes().all(|b| b.is_ascii_digit()));
        // 227(PASV)·229(EPSV)는 데이터 주소, 220(인사말)·230(로그인)은 호스트·계정 이름을 싣는다.
        // 코드로 시작하지 않는 줄은 여러 줄 응답의 중간 줄이라 무엇이 실렸는지 몰라 가린다
        return Some(match code {
            Some(c) if !["227", "229", "220", "230"].contains(&c) => format!("CC IN: {reply}"),
            Some(c) => format!("CC IN: {c} ***"),
            None => "CC IN: ***".to_owned(),
        });
    }
    None
}

/// 라이브러리는 응답 줄을 바이트 배열의 `Debug`(`[53, 53, 48, …]`)로 찍는다 — 글자로 되돌린다
fn decode_bytes(debug: &str) -> Option<String> {
    let inner = debug.strip_prefix('[')?.strip_suffix(']')?;
    let bytes = inner
        .split(',')
        .map(|n| n.trim().parse::<u8>().ok())
        .collect::<Option<Vec<u8>>>()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::mask;

    #[test]
    fn 비밀번호와_계정은_가린다() {
        assert_eq!(
            mask("CC OUT: PASS 비밀").as_deref(),
            Some("CC OUT: PASS ***")
        );
        assert_eq!(
            mask("CC OUT: USER 누구").as_deref(),
            Some("CC OUT: USER ***")
        );
        assert_eq!(
            mask("CC OUT: PORT 1,2,3,4,5,6").as_deref(),
            Some("CC OUT: PORT ***")
        );
    }

    /// 라이브러리가 찍는 모양 그대로 — `trace!("CC IN: {:?}", line)`의 `line`은 `Vec<u8>`이다
    fn cc_in(reply: &str) -> String {
        format!("CC IN: {:?}", reply.as_bytes().to_vec())
    }

    #[test]
    fn 주소와_이름을_싣는_응답은_가린다() {
        assert_eq!(
            mask(&cc_in(
                "227 Entering Passive Mode (1,2,3,4,5,6).
"
            ))
            .as_deref(),
            Some("CC IN: 227 ***")
        );
        assert_eq!(
            mask(&cc_in(
                "230 User logged in.
"
            ))
            .as_deref(),
            Some("CC IN: 230 ***")
        );
        assert_eq!(
            mask(&cc_in(
                " 이름이 실린 중간 줄
"
            ))
            .as_deref(),
            Some("CC IN: ***")
        );
    }

    #[test]
    fn 조사할_명령과_응답은_그대로_둔다() {
        assert_eq!(
            mask("CC OUT: DELE /a/b.txt").as_deref(),
            Some("CC OUT: DELE /a/b.txt")
        );
        assert_eq!(
            mask(&cc_in(
                "550 Access is denied.
"
            ))
            .as_deref(),
            Some("CC IN: 550 Access is denied.")
        );
    }

    #[test]
    fn 제어_채널_줄이_아니면_버린다() {
        assert_eq!(mask("Signin in with user 'x'"), None);
        assert_eq!(mask("Connecting to server host:21"), None);
    }
}
