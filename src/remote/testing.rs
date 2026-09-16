//! 인메모리 가짜 서버 — **외부 FTP/SFTP 서버 없이** 워커·연결 관리자·전송을 실측한다 (D25).
//!
//! NFR-10(워커 격리)·NFR-11(다중 연결 무간섭)·NFR-12(스트리밍)가 사는 곳은 서버가 아니라
//! **우리 쪽 워커·버퍼 코드**다. 그래서 자격증명도 네트워크도 없이 이 가짜 세션만으로
//! 그 셋을 전부 재현할 수 있다.
//!
//! **전송 바이트를 보관하지 않는다 (D25-a).** 업로드는 길이만 세는 싱크로 받고, 다운로드는
//! 요청한 크기만큼 패턴을 만들어 내주는 생성기로 준다 — 가짜 세션이 바이트를 쌓으면
//! 1GB×4건 측정에서 4GB가 잡혀, 재려던 것(우리 스트리밍)이 아니라 측정 장치를 재게 된다.
//!
//! 이 모듈은 `#[cfg(test)]`가 아니다 — `tests/`의 통합 테스트가 라이브러리를 일반 빌드로
//! 링크하기 때문이다(T26의 동시성 회귀 테스트가 이것을 쓴다).
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::remote::pump;
use crate::remote::types::{
    Progress, RemoteEntry, RemoteError, RemotePath, RemoteResult, RemoteSession, SiteRecord,
};

/// 가짜 서버 한 대. 여러 `FakeSession`이 이것을 함께 본다 —
/// 테스트가 바깥에서 지연·실패·응답 없음을 켜고 끌 수 있다.
#[derive(Default)]
pub struct FakeServer {
    /// 경로 → 그 폴더의 항목들
    entries: Mutex<HashMap<String, Vec<RemoteEntry>>>,
    /// 명령마다 재우는 시간 — 느린 서버를 흉내 낸다
    delay_ms: AtomicU64,
    /// 전송 덩이(`TRANSFER_BUFFER`)마다 재우는 시간 — 오래 걸리는 전송을 흉내 낸다.
    /// 위 `delay_ms`는 명령을 시작할 때 한 번뿐이라, **전송 도중**에 끼어드는 취소를
    /// 시험하려면 옮기는 동안 실제로 시간이 흘러야 한다
    transfer_delay_ms: AtomicU64,
    /// 켜져 있는 동안 명령이 **돌아오지 않는다** — "응답 없는 서버"(NFR-11 실측용)
    hang: AtomicBool,
    /// 남은 연결 실패 횟수 — 이 수만큼 `connect`가 실패한 뒤 성공한다(재시도 확인용)
    connect_failures: AtomicU32,
    /// `download`가 내줄 바이트 수
    download_bytes: AtomicU64,
    /// 지금까지 받은 업로드 바이트 — **내용은 버리고 길이만 센다**
    uploaded_bytes: AtomicU64,
    /// 살아 있는 세션 수 — 워커가 회수됐는지 보는 데 쓴다
    live_sessions: AtomicUsize,
    /// 처리한 명령 이름을 순서대로 — 직렬 처리 확인용
    calls: Mutex<Vec<String>>,
    /// 켜져 있으면 `chmod`가 "지원하지 않는다"로 답한다 — SITE CHMOD를 모르는 FTP 서버(D22)
    chmod_unsupported: AtomicBool,
    /// 켜져 있으면 TLS 승격을 거부한다 — 연결은 서지만 **평문이다** (F-7 리뷰 B1)
    refuse_tls: AtomicBool,
    /// 켜져 있으면 **이미 선 연결이 죽은 것처럼** 군다 — 유휴 타임아웃·네트워크 끊김.
    /// `noop`을 포함한 모든 명령이 실패하므로 끊김 판정의 프로브도 함께 실패한다
    link_down: AtomicBool,
    /// 켜져 있으면 **목록만** 실패하고 `noop`은 성공한다 — FTP의 수동형 데이터 연결이
    /// 방화벽에 막힌 상태다. 제어 채널은 멀쩡하므로 끊김으로 보면 안 된다
    data_failure: AtomicBool,
}

impl FakeServer {
    pub fn new() -> Arc<FakeServer> {
        Arc::new(FakeServer::default())
    }

    /// 폴더 하나의 목록을 심는다
    pub fn set_entries(&self, path: &str, entries: Vec<RemoteEntry>) {
        if let Ok(mut map) = self.entries.lock() {
            map.insert(path.to_owned(), entries);
        }
    }

    /// 모든 명령을 이만큼 지연시킨다
    pub fn set_delay(&self, delay: Duration) {
        self.delay_ms
            .store(delay.as_millis() as u64, Ordering::SeqCst);
    }

    /// 전송 덩이마다 이만큼 지연시킨다 — 전송이 도중에 머무는 동안을 만든다
    pub fn set_transfer_delay(&self, delay: Duration) {
        self.transfer_delay_ms
            .store(delay.as_millis() as u64, Ordering::SeqCst);
    }

    /// 응답 없는 서버로 만들거나 되돌린다
    pub fn set_hang(&self, hang: bool) {
        self.hang.store(hang, Ordering::SeqCst);
    }

    /// TLS 승격을 거부하는 서버로 만들거나 되돌린다 — 연결은 서되 암호화되지 않는다
    pub fn set_refuse_tls(&self, refuse: bool) {
        self.refuse_tls.store(refuse, Ordering::SeqCst);
    }

    /// `chmod`를 지원하지 않는 서버로 만들거나 되돌린다 (D22)
    pub fn set_chmod_unsupported(&self, unsupported: bool) {
        self.chmod_unsupported.store(unsupported, Ordering::SeqCst);
    }

    /// 선 연결을 죽이거나 되살린다 — **되살리는 것은 시험이 명시한다**.
    /// `connect`가 이 값을 지우지 않는 이유: 서버가 살아난 것을 앱이 짐작하면 안 된다.
    /// 죽은 동안에도 `connect` 자체는 성공한다 — 여기서 재는 것은 **선 연결이 도중에
    /// 죽는 길**이고, 연결이 서지 않는 길은 `fail_connects`가 따로 맡는다
    pub fn set_link_down(&self, down: bool) {
        self.link_down.store(down, Ordering::SeqCst);
    }

    /// 데이터 연결만 막는다 — 목록은 실패하고 제어 채널(`noop`)은 그대로 답한다
    pub fn set_data_failure(&self, failing: bool) {
        self.data_failure.store(failing, Ordering::SeqCst);
    }

    /// 다음 `count`번의 연결을 실패시킨다
    pub fn fail_connects(&self, count: u32) {
        self.connect_failures.store(count, Ordering::SeqCst);
    }

    /// `download`가 내줄 크기를 정한다
    pub fn set_download_size(&self, bytes: u64) {
        self.download_bytes.store(bytes, Ordering::SeqCst);
    }

    pub fn uploaded_bytes(&self) -> u64 {
        self.uploaded_bytes.load(Ordering::SeqCst)
    }

    pub fn live_sessions(&self) -> usize {
        self.live_sessions.load(Ordering::SeqCst)
    }

    /// 처리한 명령 이름들 (순서 보존)
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }

    fn record(&self, name: &str) {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(name.to_owned());
        }
    }

    /// 전송 덩이 하나만큼 시간을 흘려 보낸다 — `pump`가 한 덩이를 옮길 때마다 불린다
    fn pace_transfer(&self) {
        let delay = self.transfer_delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
    }

    /// 지연·응답 없음을 흉내 낸다. 응답 없음이 풀릴 때까지 여기서 머문다
    fn tick(&self) {
        let delay = self.delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
        while self.hang.load(Ordering::SeqCst) {
            // 완전히 멈춰 세우면 테스트가 끝나지 못한다 — 짧게 자며 풀리기를 기다린다
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// 가짜 서버에 붙은 세션 하나 — `RemoteSession`을 그대로 구현한다
pub struct FakeSession {
    server: Arc<FakeServer>,
    connected: bool,
}

impl FakeSession {
    pub fn new(server: Arc<FakeServer>) -> FakeSession {
        server.live_sessions.fetch_add(1, Ordering::SeqCst);
        FakeSession {
            server,
            connected: false,
        }
    }

    fn ensure_connected(&self) -> RemoteResult<()> {
        // 선 연결이 도중에 죽은 경우 — 실제 서버에서는 소켓 오류로 나타난다
        if self.server.link_down.load(Ordering::SeqCst) {
            return Err(RemoteError::Protocol {
                detail: "연결이 끊어졌습니다".to_owned(),
            });
        }
        if self.connected {
            Ok(())
        } else {
            Err(RemoteError::Protocol {
                detail: "연결되어 있지 않습니다".to_owned(),
            })
        }
    }
}

impl Drop for FakeSession {
    fn drop(&mut self) {
        self.server.live_sessions.fetch_sub(1, Ordering::SeqCst);
    }
}

impl RemoteSession for FakeSession {
    fn connect(&mut self, _site: &SiteRecord) -> RemoteResult<()> {
        self.server.record("connect");
        self.server.tick();
        // 남은 실패 횟수가 있으면 그만큼 거절한다 — 재시도 경로를 태우기 위함
        if self
            .server
            .connect_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return Err(RemoteError::Connect {
                detail: "421 Too many connections".to_owned(),
            });
        }
        self.connected = true;
        Ok(())
    }

    fn is_secure(&self) -> bool {
        // 연결이 서 있고 서버가 TLS를 거부하지 않았을 때만 암호화된 것으로 본다
        self.connected && !self.server.refuse_tls.load(Ordering::SeqCst)
    }

    fn login(&mut self, _site: &SiteRecord, _password: &str) -> RemoteResult<()> {
        self.server.record("login");
        self.server.tick();
        self.ensure_connected()
    }

    fn pwd(&mut self) -> RemoteResult<RemotePath> {
        self.server.record("pwd");
        self.server.tick();
        self.ensure_connected()?;
        Ok(RemotePath::root())
    }

    fn list(&mut self, path: &RemotePath) -> RemoteResult<Vec<RemoteEntry>> {
        self.server.record("list");
        self.server.tick();
        self.ensure_connected()?;
        // 데이터 연결만 막힌 서버 — 실제 FTP에서도 이 실패는 `Connect` 갈래로 온다
        if self.server.data_failure.load(Ordering::SeqCst) {
            return Err(RemoteError::Connect {
                detail: "데이터 연결을 세우지 못했습니다".to_owned(),
            });
        }
        let map = self
            .server
            .entries
            .lock()
            .map_err(|_| RemoteError::Protocol {
                detail: "가짜 서버 상태가 오염됐습니다".to_owned(),
            })?;
        map.get(path.as_str())
            .cloned()
            .ok_or_else(|| RemoteError::NotFound {
                path: path.as_str().to_owned(),
                detail: "없는 폴더".to_owned(),
            })
    }

    fn cwd(&mut self, _path: &RemotePath) -> RemoteResult<()> {
        self.server.record("cwd");
        self.server.tick();
        self.ensure_connected()
    }

    fn mkdir(&mut self, _path: &RemotePath) -> RemoteResult<()> {
        self.server.record("mkdir");
        self.server.tick();
        self.ensure_connected()
    }

    fn remove(&mut self, _path: &RemotePath) -> RemoteResult<()> {
        self.server.record("remove");
        self.server.tick();
        self.ensure_connected()
    }

    fn rmdir(&mut self, _path: &RemotePath) -> RemoteResult<()> {
        self.server.record("rmdir");
        self.server.tick();
        self.ensure_connected()
    }

    fn rename(&mut self, _from: &RemotePath, _to: &RemotePath) -> RemoteResult<()> {
        self.server.record("rename");
        self.server.tick();
        self.ensure_connected()
    }

    fn chmod(&mut self, _path: &RemotePath, _mode: u32) -> RemoteResult<()> {
        self.server.record("chmod");
        self.server.tick();
        self.ensure_connected()?;
        if self.server.chmod_unsupported.load(Ordering::SeqCst) {
            return Err(RemoteError::Unsupported {
                operation: "SITE CHMOD".to_owned(),
                detail: "500 Unknown command".to_owned(),
            });
        }
        Ok(())
    }

    fn download(
        &mut self,
        _path: &RemotePath,
        dest: &mut dyn Write,
        offset: u64,
        progress: &mut dyn Progress,
    ) -> RemoteResult<u64> {
        self.server.record("download");
        self.server.tick();
        self.ensure_connected()?;
        let total = self.server.download_bytes.load(Ordering::SeqCst);
        let remaining = total.saturating_sub(offset);
        // 바이트를 쌓아 두지 않고 그 자리에서 만들어 낸다 (D25-a)
        let mut source = PatternReader {
            position: offset,
            remaining,
        };
        finish(pump(&mut source, dest, progress))
    }

    fn upload(
        &mut self,
        _path: &RemotePath,
        src: &mut dyn Read,
        _offset: u64,
        progress: &mut dyn Progress,
    ) -> RemoteResult<u64> {
        self.server.record("upload");
        self.server.tick();
        self.ensure_connected()?;
        // 받은 바이트는 버리고 길이만 센다 (D25-a)
        let mut sink = CountingSink {
            server: Arc::clone(&self.server),
        };
        finish(pump(src, &mut sink, progress))
    }

    fn noop(&mut self) -> RemoteResult<()> {
        self.server.record("noop");
        self.ensure_connected()
    }

    fn quit(&mut self) -> RemoteResult<()> {
        self.server.record("quit");
        self.connected = false;
        // 죽은 연결에는 인사도 닿지 않는다 — 실제 FTP에서 흔한 자리다.
        // 접는 길이 이 실패를 「끊김」으로 오해하지 않는지를 시험이 여기서 잰다
        if self.server.link_down.load(Ordering::SeqCst) {
            return Err(RemoteError::Protocol {
                detail: "연결이 끊어졌습니다".to_owned(),
            });
        }
        Ok(())
    }
}

/// `pump` 결과를 전송 메서드의 반환값으로 옮긴다 — 실제 구현(`ftp`·`sftp`)과 같은 규칙이다
fn finish(outcome: crate::remote::Pumped) -> RemoteResult<u64> {
    match outcome {
        crate::remote::Pumped::Done(total) => Ok(total),
        crate::remote::Pumped::Cancelled => Err(RemoteError::Cancelled),
        crate::remote::Pumped::Failed {
            transferred,
            detail,
        } => Err(RemoteError::Transfer {
            detail,
            transferred,
        }),
    }
}

/// 요청한 크기만큼 바이트를 만들어 내주는 읽기 원본 — 메모리에 미리 쌓지 않는다.
///
/// **자리마다 다른 값**을 낸다(`pattern_byte`) — 같은 바이트로 채우면 이어받기가 어긋나
/// 몇 바이트 겹치거나 빠져도 결과가 같아 보여, "이어받은 파일이 원본과 같다"를 검증할 수 없다
struct PatternReader {
    /// 파일 안에서 지금 읽고 있는 자리 — 이어받기는 이 값이 오프셋에서 시작한다
    position: u64,
    remaining: u64,
}

impl Read for PatternReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 {
            return Ok(0);
        }
        let take = buf.len().min(self.remaining as usize);
        for (index, slot) in buf[..take].iter_mut().enumerate() {
            *slot = pattern_byte(self.position + index as u64);
        }
        self.position += take as u64;
        self.remaining -= take as u64;
        Ok(take)
    }
}

/// 가짜 서버가 자리 `offset`에 두는 바이트.
///
/// 251은 256과 서로 소인 소수라 값이 바이트 경계(64KB·4KB)와 같은 주기로 반복되지 않는다 —
/// 한 블록만큼 어긋난 이어받기가 그대로 드러난다
pub fn pattern_byte(offset: u64) -> u8 {
    (offset % 251) as u8
}

/// 받은 바이트를 세기만 하는 쓰기 대상
struct CountingSink {
    server: Arc<FakeServer>,
}

impl Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.server.pace_transfer();
        self.server
            .uploaded_bytes
            .fetch_add(buf.len() as u64, Ordering::SeqCst);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 테스트에서 쓰는 항목 하나 — 이름과 폴더 여부만 정한다
pub fn fake_entry(name: &str, is_dir: bool) -> RemoteEntry {
    RemoteEntry {
        name: name.to_owned(),
        is_dir,
        is_symlink: false,
        link_target: None,
        size: 0,
        modified: None,
        mode: None,
        owner: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::types::{SiteId, SiteRecord};

    struct Silent;
    impl Progress for Silent {
        fn report(&mut self, _transferred: u64) -> bool {
            true
        }
    }

    fn site() -> SiteRecord {
        SiteRecord::new(SiteId(1), "가짜".to_owned())
    }

    #[test]
    fn 세션은_연결_전에는_명령을_받지_않는다() {
        let server = FakeServer::new();
        let mut session = FakeSession::new(Arc::clone(&server));
        assert!(session.list(&RemotePath::root()).is_err());
        session.connect(&site()).expect("연결");
        server.set_entries("/", vec![fake_entry("a.txt", false)]);
        assert_eq!(session.list(&RemotePath::root()).expect("목록").len(), 1);
    }

    #[test]
    fn 연결이_죽으면_목록도_연결_확인도_실패한다() {
        // T1 Acceptance — 끊김 판정의 프로브(`noop`)까지 함께 실패해야
        // 워커가 「제어 채널이 죽었다」로 가를 수 있다
        let server = FakeServer::new();
        let mut session = FakeSession::new(Arc::clone(&server));
        session.connect(&site()).expect("연결");
        server.set_entries("/", vec![fake_entry("a.txt", false)]);

        server.set_link_down(true);
        assert!(session.list(&RemotePath::root()).is_err(), "목록이 살았다");
        assert!(session.noop().is_err(), "연결 확인이 살았다");

        server.set_link_down(false);
        assert!(session.noop().is_ok(), "되살린 뒤에도 죽어 있다");
    }

    #[test]
    fn 데이터_연결만_막히면_목록만_실패한다() {
        // T1 Acceptance — FTP 수동형 데이터 연결이 방화벽에 막힌 상태다.
        // 제어 채널은 멀쩡하므로 끊김으로 보면 안 된다
        let server = FakeServer::new();
        let mut session = FakeSession::new(Arc::clone(&server));
        session.connect(&site()).expect("연결");
        server.set_entries("/", vec![fake_entry("a.txt", false)]);

        server.set_data_failure(true);
        assert!(session.list(&RemotePath::root()).is_err(), "목록이 살았다");
        assert!(session.noop().is_ok(), "제어 채널까지 죽었다");
    }

    #[test]
    fn 정한_횟수만큼_연결이_실패한_뒤_성공한다() {
        let server = FakeServer::new();
        server.fail_connects(2);
        let mut session = FakeSession::new(Arc::clone(&server));
        assert!(session.connect(&site()).is_err());
        assert!(session.connect(&site()).is_err());
        session.connect(&site()).expect("세 번째는 성공해야 한다");
    }

    #[test]
    fn 전송은_바이트를_쌓지_않고_길이만_센다() {
        // D25-a — 가짜 세션이 바이트를 보관하면 NFR-12 측정이 측정 장치를 재게 된다
        let server = FakeServer::new();
        server.set_download_size(300 * 1024);
        let mut session = FakeSession::new(Arc::clone(&server));
        session.connect(&site()).expect("연결");

        let mut sink = std::io::sink();
        let moved = session
            .download(&RemotePath::root(), &mut sink, 0, &mut Silent)
            .expect("받기");
        assert_eq!(moved, 300 * 1024);

        let mut source = PatternReader {
            position: 0,
            remaining: 128 * 1024,
        };
        let sent = session
            .upload(&RemotePath::root(), &mut source, 0, &mut Silent)
            .expect("보내기");
        assert_eq!(sent, 128 * 1024);
        assert_eq!(server.uploaded_bytes(), 128 * 1024);
    }

    #[test]
    fn 이어받기_지점만큼_남은_바이트가_준다() {
        let server = FakeServer::new();
        server.set_download_size(100);
        let mut session = FakeSession::new(Arc::clone(&server));
        session.connect(&site()).expect("연결");
        let mut sink = std::io::sink();
        let moved = session
            .download(&RemotePath::root(), &mut sink, 40, &mut Silent)
            .expect("이어받기");
        assert_eq!(moved, 60);
    }

    #[test]
    fn 세션이_사라지면_살아있는_수가_준다() {
        let server = FakeServer::new();
        assert_eq!(server.live_sessions(), 0);
        {
            let _session = FakeSession::new(Arc::clone(&server));
            assert_eq!(server.live_sessions(), 1);
        }
        assert_eq!(server.live_sessions(), 0);
    }
}
