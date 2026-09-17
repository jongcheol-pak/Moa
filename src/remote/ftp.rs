//! FTP·FTPS 세션 — `RemoteSession`의 FTP 계열 구현 (FR-30·FR-31·FR-37·FR-39).
//!
//! 이 모듈은 **연결 워커 스레드 안에서만** 살아 있다 — UI 스레드는 이 타입을 직접 만지지 않는다
//! (NFR-10). `suppaftp`의 동기 API를 그대로 쓰므로 async 런타임이 필요 없다.
//!
//! **평문과 TLS를 한 타입(`NativeTlsFtpStream`)으로 다룬다.** 계획은 평문 스트림과 TLS 스트림을
//! 열거형으로 감싸는 안이었는데, `suppaftp`에서 TLS 가능한 스트림 타입은 **연결 직후에는 평문**이고
//! `AUTH TLS` 승격에 성공해야 암호화로 바뀐다 — 즉 한 타입이 두 상태를 이미 담는다. 열거형을 덧대면
//! 15개 남짓한 메서드마다 같은 갈래를 다시 적을 뿐 동작이 달라지지 않아 두지 않았다.
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::UNIX_EPOCH;

use suppaftp::list::{File as ListFile, ListParser, PosixPexQuery};
use suppaftp::native_tls::TlsConnector;
use suppaftp::types::{FileType as TransferType, Response};
use suppaftp::{FtpError, Mode, NativeTlsConnector, NativeTlsFtpStream, Status};

use crate::remote::charset;
use crate::remote::types::{
    Charset, Encryption, LogonType, Progress, Protocol, RemoteEntry, RemoteError, RemoteOp,
    RemotePath, RemoteResult, RemoteSession, SiteRecord, TransferMode,
};
use crate::remote::{Pumped, pump};

/// FTP·FTPS 연결 하나.
///
/// `connect` → `login` 순으로 쓰고, 그 뒤 목록·전송·파일 작업을 부른다.
pub struct FtpSession {
    /// 연결 전과 `quit` 후에는 `None`이다
    stream: Option<NativeTlsFtpStream>,
    /// 서버가 MLSD를 지원하지 않는 것이 한 번 확인되면 켜져, 이후 목록은 곧장 LIST로 간다
    /// (매 조회마다 헛된 왕복을 만들지 않기 위함)
    mlsd_unsupported: bool,
    /// `TransferMode::Default`에서 아직 수동형만 시도해 본 상태인가 —
    /// 데이터 연결이 실패하면 한 번 능동형으로 넘어간다
    active_fallback_pending: bool,
    /// 이 연결이 실제로 TLS 위에 섰는가 — `ExplicitIfAvailable`이 거부당해 평문으로
    /// 되연결하면 거짓이다 (F-7 리뷰 B1)
    secure: bool,
    /// 이 사이트의 파일명 문자셋 (FR-46) — 목록 원문을 이 인코딩으로 읽는다.
    ///
    /// **서버가 UTF-8을 받아들이면(`OPTS UTF8 ON` 성공) 무시된다** — 그때는 서버가 이미
    /// UTF-8로 말하므로 다시 해석할 것이 없다
    charset: Charset,
    /// 서버가 `OPTS UTF8 ON`을 받아들였는가
    utf8_mode: bool,
    /// 전송이 중간에 끊겨 **제어 채널이 어긋났을 수 있는가** — 참이면 워커가 연결을 다시 세운다.
    ///
    /// 중단 시 서버가 주는 응답 수가 구현마다 갈리는데(전송 종료 하나 / `426`+`226` 둘 /
    /// `226`+`226 ABOR successful` 둘) 라이브러리는 첫 응답이 `426`일 때만 둘째를 읽는다.
    /// 그래서 남은 응답이 생기고, 그것을 비울 길이 없어 연결을 버리는 쪽을 택했다
    dirty: bool,
}

impl FtpSession {
    pub fn new() -> FtpSession {
        FtpSession {
            stream: None,
            mlsd_unsupported: false,
            active_fallback_pending: false,
            secure: false,
            charset: Charset::default(),
            utf8_mode: false,
            dirty: false,
        }
    }

    fn stream(&mut self) -> RemoteResult<&mut NativeTlsFtpStream> {
        self.stream.as_mut().ok_or_else(|| RemoteError::Protocol {
            detail: crate::i18n::remote_not_connected_err().to_owned(),
        })
    }

    /// 전송이 온전히 끝나지 못했으면 이 연결을 버릴 것으로 표시한다.
    ///
    /// **끝나지 못한 모든 길이 대상이다** — 사용자 취소·전송 실패는 물론 마무리(`finalize`)
    /// 실패까지. 어느 쪽이든 서버가 몇 개의 응답을 남겼는지 알 수 없고, 그것을 확인할 방법이
    /// 우리에게 없다. 성공한 전송은 응답까지 정확히 읽었으므로 그대로 쓴다
    fn mark_dirty_if_interrupted(&mut self, outcome: &RemoteResult<u64>) {
        if outcome.is_err() {
            self.dirty = true;
        }
    }

    /// 받기의 본체 — **실패를 `?`로 그대로 올린다.**
    ///
    /// 표시(`mark_dirty_if_interrupted`)를 여기서 하지 않고 부르는 쪽에 두는 이유가 이
    /// 분리의 전부다: 한 함수 안에 두면 `RETR`·`REST`·`stream()` 실패가 `?`로 빠져나가
    /// **그 표시를 건너뛴다**. 실제로 그랬고, 그래서 끊긴 연결이 다시 서지 못한 채
    /// 남아 그 뒤의 모든 전송이 같은 자리에서 실패했다 (사용자 보고 2026-09-17)
    fn download_inner(
        &mut self,
        path: &RemotePath,
        dest: &mut dyn Write,
        offset: u64,
        progress: &mut dyn Progress,
    ) -> RemoteResult<u64> {
        let name = path.as_str();
        let stream = self.stream()?;
        if offset > 0 {
            stream
                .resume_transfer(offset as usize)
                .map_err(|e| classify(e, RemoteOp::Raw("REST"), Some(name)))?;
        }
        let mut data = stream
            .retr_as_stream(name)
            .map_err(|e| classify(e, RemoteOp::Raw("RETR"), Some(name)))?;

        match pump(&mut data, dest, progress) {
            Pumped::Done(total) => stream
                .finalize_retr_stream(data)
                .map(|()| total)
                .map_err(|e| classify(e, RemoteOp::Raw("RETR"), Some(name))),
            Pumped::Cancelled => {
                // `ABOR`는 그대로 보낸다 — 서버에게 중단을 알리는 뜻이 있고, 실패해도
                // 부르는 쪽에서 연결을 버리므로 그 성패가 결과를 바꾸지 않는다
                let _ = stream.abort(data);
                Err(RemoteError::Cancelled)
            }
            Pumped::Failed {
                transferred,
                detail,
            } => {
                let _ = stream.abort(data);
                Err(RemoteError::Transfer {
                    detail,
                    transferred,
                })
            }
        }
    }

    /// 올리기의 본체 — `download_inner`와 같은 이유로 갈라 두었다
    fn upload_inner(
        &mut self,
        path: &RemotePath,
        src: &mut dyn Read,
        offset: u64,
        progress: &mut dyn Progress,
    ) -> RemoteResult<u64> {
        let name = path.as_str();
        let stream = self.stream()?;
        // 이어 올리기는 REST+STOR이 아니라 APPE로 한다 — REST를 업로드에 받아 주는 서버가
        // 들쭉날쭉하고, 이어받기 지점은 호출부가 원격 파일 크기로 이미 정해 두기 때문이다
        let mut data = if offset > 0 {
            stream
                .append_with_stream(name)
                .map_err(|e| classify(e, RemoteOp::Raw("APPE"), Some(name)))?
        } else {
            stream
                .put_with_stream(name)
                .map_err(|e| classify(e, RemoteOp::Raw("STOR"), Some(name)))?
        };

        match pump(src, &mut data, progress) {
            Pumped::Done(total) => stream
                .finalize_put_stream(data)
                .map(|()| total)
                .map_err(|e| classify(e, RemoteOp::Raw("STOR"), Some(name))),
            Pumped::Cancelled => {
                let _ = stream.abort(data);
                Err(RemoteError::Cancelled)
            }
            Pumped::Failed {
                transferred,
                detail,
            } => {
                let _ = stream.abort(data);
                Err(RemoteError::Transfer {
                    detail,
                    transferred,
                })
            }
        }
    }

    /// 사이트 설정의 전송 모드를 데이터 연결 방식으로 옮긴다 (FR-45)
    fn apply_transfer_mode(&mut self, mode: TransferMode) {
        let (data_mode, fallback) = match mode {
            // `기본`은 수동형으로 먼저 시도한다 — 방화벽·NAT 뒤에서 대부분 이쪽이 통한다
            TransferMode::Default => (Mode::Passive, true),
            TransferMode::Active => (Mode::Active, false),
            TransferMode::Passive => (Mode::Passive, false),
        };
        self.active_fallback_pending = fallback;
        if let Some(stream) = self.stream.as_mut() {
            stream.set_mode(data_mode);
        }
    }

    /// LIST 원문 줄을 받아 온다.
    ///
    /// `기본` 전송 모드에서 수동형 데이터 연결이 실패하면 **한 번만** 능동형으로 바꿔 다시 시도한다.
    /// 이 판정을 목록 조회에만 두는 이유: 연결 후 첫 데이터 명령은 언제나 목록이므로 전송이
    /// 시작될 때는 방식이 이미 정해져 있다.
    fn list_lines(&mut self, path: &str) -> RemoteResult<Vec<String>> {
        match self.raw_list(path) {
            Ok(lines) => Ok(lines),
            Err(err) if self.active_fallback_pending && is_data_connection_failure(&err) => {
                self.active_fallback_pending = false;
                self.stream()?.set_mode(Mode::Active);
                self.raw_list(path)
                    .map_err(|e| classify(e, RemoteOp::Raw("LIST"), Some(path)))
            }
            Err(err) => Err(classify(err, RemoteOp::Raw("LIST"), Some(path))),
        }
    }

    /// `LIST`를 **원문 바이트로** 읽어 사이트 문자셋으로 옮긴다 (FR-46).
    ///
    /// `suppaftp`의 `list()`를 쓰지 않는 이유: 그 함수가 줄을 `String::from_utf8_lossy`로
    /// 이미 디코딩해(`sync_ftp.rs`의 `get_lines_from_stream`) **CP949 서버의 한글 이름이
    /// 치환 문자로 뭉개진 뒤에** 우리 손에 온다. 데이터 연결을 직접 받아 바이트를 지키면
    /// 그 자리에서 올바로 옮길 수 있다
    fn raw_list(&mut self, path: &str) -> Result<Vec<String>, FtpError> {
        let charset = self.charset.clone();
        let utf8_mode = self.utf8_mode;
        // 연결이 없으면 데이터 명령을 낼 수 없다 — 위의 `list_lines`가 사유를 붙여 올린다
        let stream = self.stream().map_err(|_| FtpError::BadResponse)?;
        let (_, data) = stream.custom_data_command(
            format!("LIST {path}"),
            &[Status::AboutToSend, Status::AlreadyOpen],
        )?;
        let mut raw = Vec::new();
        let mut data = data;
        let read = data.read_to_end(&mut raw);
        // 데이터 연결은 읽기 성공 여부와 무관하게 닫는다 — 닫지 않으면 다음 명령이 막힌다
        stream.close_data_connection(data)?;
        read.map_err(FtpError::ConnectionError)?;
        Ok(decode_lines(&raw, &charset, utf8_mode))
    }
}

impl Default for FtpSession {
    fn default() -> FtpSession {
        FtpSession::new()
    }
}

impl RemoteSession for FtpSession {
    fn connect(&mut self, site: &SiteRecord) -> RemoteResult<()> {
        if site.protocol.is_ssh() {
            return Err(RemoteError::Protocol {
                detail: crate::i18n::remote_sftp_site_on_ftp().to_owned(),
            });
        }
        let addr = site.address();
        // TLS 인증서의 이름 대조에 쓰는 호스트 — IPv6 리터럴의 대괄호는 이름이 아니다
        let domain = site.host.trim_start_matches('[').trim_end_matches(']');

        // 협상 결과를 여기서 정한다 — 아래 분기 중 **실제로 TLS가 선 길**만 참이다
        self.secure = false;
        let stream = match effective_encryption(site) {
            Encryption::Plain => NativeTlsFtpStream::connect(&addr)
                .map_err(|e| classify(e, RemoteOp::Connect, None))?,
            Encryption::Implicit => {
                let stream =
                    NativeTlsFtpStream::connect_secure_implicit(&addr, tls_connector()?, domain)
                        .map_err(|e| classify(e, RemoteOp::ConnectImplicit, None))?;
                self.secure = true;
                stream
            }
            explicit => {
                let plain = NativeTlsFtpStream::connect(&addr)
                    .map_err(|e| classify(e, RemoteOp::Connect, None))?;
                match plain.into_secure(tls_connector()?, domain) {
                    Ok(secure) => {
                        self.secure = true;
                        secure
                    }
                    Err(_) if explicit == Encryption::ExplicitIfAvailable => {
                        // 서버가 AUTH TLS를 거부했다. 승격이 스트림을 소비했으므로 평문으로 다시
                        // 연결한다 — 이때 이 연결은 **암호화되지 않았다**(`secure`는 거짓 그대로)
                        NativeTlsFtpStream::connect(&addr)
                            .map_err(|e| classify(e, RemoteOp::Connect, None))?
                    }
                    Err(err) => return Err(classify(err, RemoteOp::TlsUpgrade, None)),
                }
            }
        };

        // 제어 연결의 상대 주소를 알아 두면 수동형 데이터 주소가 사설로 왔을 때 갈아끼울 수 있다.
        // 주소를 얻지 못하면 서버가 알려 준 주소를 그대로 쓴다(라이브러리 기본 동작)
        let control_ip = stream.get_ref().peer_addr().ok().map(|addr| addr.ip());
        let stream = match control_ip {
            Some(control) => stream.passive_stream_builder(move |advertised| {
                TcpStream::connect(passive_target(advertised, control))
                    .map_err(FtpError::ConnectionError)
            }),
            None => stream,
        };

        self.stream = Some(stream);
        // 새로 선 연결은 어긋난 것이 없다
        self.dirty = false;
        self.mlsd_unsupported = false;
        self.charset = site.charset.clone();
        // 서버가 UTF-8을 받아들이면 그 뒤로는 이름이 UTF-8로 온다 — 사이트 문자셋보다 이것이
        // 우선이다(RFC 2640). 받아들이지 않는 서버에서는 조용히 지나가고 문자셋이 일을 한다
        self.utf8_mode = self
            .stream()
            .map(|stream| stream.opts("UTF8", Some("ON")).is_ok())
            .unwrap_or(false);
        self.apply_transfer_mode(site.transfer_mode);
        Ok(())
    }

    fn login(&mut self, site: &SiteRecord, password: &str) -> RemoteResult<()> {
        // 익명 로그인은 비밀번호 자리에 연락처를 적는 것이 관례다 — 빈 값을 거부하는 서버가 있다
        let password = match (site.logon, password.is_empty()) {
            (LogonType::Anonymous, true) => "anonymous@",
            _ => password,
        };
        let stream = self.stream()?;
        stream
            .login(site.effective_user(), password)
            .map_err(|e| classify(e, RemoteOp::Login, None))?;
        // 이 앱은 모든 파일을 있는 그대로 옮긴다 — ASCII 모드는 줄바꿈을 바꿔 파일을 손상시킨다
        stream
            .transfer_type(TransferType::Binary)
            .map_err(|e| classify(e, RemoteOp::Raw("TYPE"), None))
    }

    fn pwd(&mut self) -> RemoteResult<RemotePath> {
        let raw = self
            .stream()?
            .pwd()
            .map_err(|e| classify(e, RemoteOp::Raw("PWD"), None))?;
        Ok(RemotePath::new(&raw))
    }

    fn list(&mut self, path: &RemotePath) -> RemoteResult<Vec<RemoteEntry>> {
        if !self.mlsd_unsupported {
            match self.stream()?.mlsd(Some(path.as_str())) {
                Ok(lines) => return Ok(entries_from_mlsd(&lines)),
                Err(err) => {
                    // 서버가 명령 자체를 모르면 다음부터는 묻지 않는다. 그 밖의 실패(경로 없음·
                    // 데이터 연결 실패)도 LIST로 한 번 더 시도해 본다 — 실패하면 LIST의 사유가 남는다
                    if is_unsupported(&err) {
                        self.mlsd_unsupported = true;
                    }
                }
            }
        }
        let lines = self.list_lines(path.as_str())?;
        Ok(entries_from_list(&lines))
    }

    fn cwd(&mut self, path: &RemotePath) -> RemoteResult<()> {
        self.stream()?
            .cwd(path.as_str())
            .map_err(|e| classify(e, RemoteOp::Raw("CWD"), Some(path.as_str())))
    }

    fn mkdir(&mut self, path: &RemotePath) -> RemoteResult<()> {
        self.stream()?
            .mkdir(path.as_str())
            .map_err(|e| classify(e, RemoteOp::Raw("MKD"), Some(path.as_str())))
    }

    fn remove(&mut self, path: &RemotePath) -> RemoteResult<()> {
        self.stream()?
            .rm(path.as_str())
            .map_err(|e| classify(e, RemoteOp::Raw("DELE"), Some(path.as_str())))
    }

    fn rmdir(&mut self, path: &RemotePath) -> RemoteResult<()> {
        self.stream()?
            .rmdir(path.as_str())
            .map_err(|e| classify(e, RemoteOp::Raw("RMD"), Some(path.as_str())))
    }

    fn rename(&mut self, from: &RemotePath, to: &RemotePath) -> RemoteResult<()> {
        self.stream()?
            .rename(from.as_str(), to.as_str())
            .map_err(|e| classify(e, RemoteOp::Raw("RNFR/RNTO"), Some(from.as_str())))
    }

    fn chmod(&mut self, path: &RemotePath, mode: u32) -> RemoteResult<()> {
        // SITE CHMOD는 표준이 아니라 서버 재량이다 — 지원 여부는 눌러 봐야 안다 (D22)
        let command = format!("SITE CHMOD {:03o} {}", mode & 0o7777, path.as_str());
        self.stream()?
            .custom_command(command, &[Status::CommandOk, Status::RequestedFileActionOk])
            .map(|_| ())
            .map_err(|e| classify(e, RemoteOp::Raw("SITE CHMOD"), Some(path.as_str())))
    }

    /// **이 함수에 `?`를 두지 않는다** — 어느 실패든 아래 한 줄을 지나야 한다.
    ///
    /// 본체를 `download_inner`로 갈라 둔 것이 그 보장이다. 종전에는 `RETR`·`REST`·
    /// `stream()` 실패가 `?`로 빠져나가 표시를 건너뛰었고, 그래서 죽은 제어 채널이
    /// 그대로 남아 재시도가 같은 자리에서 계속 실패했다 (사용자 보고 2026-09-17)
    fn download(
        &mut self,
        path: &RemotePath,
        dest: &mut dyn Write,
        offset: u64,
        progress: &mut dyn Progress,
    ) -> RemoteResult<u64> {
        let outcome = self.download_inner(path, dest, offset, progress);
        self.mark_dirty_if_interrupted(&outcome);
        outcome
    }

    /// `download`와 같은 규칙 — **`?`를 두지 않는다**
    fn upload(
        &mut self,
        path: &RemotePath,
        src: &mut dyn Read,
        offset: u64,
        progress: &mut dyn Progress,
    ) -> RemoteResult<u64> {
        let outcome = self.upload_inner(path, src, offset, progress);
        self.mark_dirty_if_interrupted(&outcome);
        outcome
    }

    fn noop(&mut self) -> RemoteResult<()> {
        self.stream()?
            .noop()
            .map_err(|e| classify(e, RemoteOp::Raw("NOOP"), None))
    }

    fn needs_reset(&self) -> bool {
        self.dirty
    }

    fn is_secure(&self) -> bool {
        // 연결이 끊긴 뒤에는 무엇도 암호화돼 있지 않다
        self.stream.is_some() && self.secure
    }

    fn quit(&mut self) -> RemoteResult<()> {
        let Some(mut stream) = self.stream.take() else {
            return Ok(());
        };
        stream
            .quit()
            .map_err(|e| classify(e, RemoteOp::Raw("QUIT"), None))
    }
}

/// 실제로 적용할 암호화 방식.
///
/// 사용자가 프로토콜로 `FTPS`를 골랐다면 화면에서 고른 것은 "암호화된 FTP"다 — 암호화 항목이
/// `평문`으로 남아 있어도 평문으로 내려가지 않고 명시적 TLS를 요구한다. `FTP`를 골랐을 때는
/// 암호화 항목의 값을 그대로 따른다.
fn effective_encryption(site: &SiteRecord) -> Encryption {
    match (site.protocol, site.encryption) {
        (Protocol::Ftps, Encryption::Plain) => Encryption::ExplicitRequired,
        (_, encryption) => encryption,
    }
}

/// 수동형 데이터 연결을 실제로 걸 주소.
///
/// NAT 뒤의 서버는 PASV 응답에 자기 **사설** 주소를 적어 보내는 일이 흔하다 — 그대로 접속하면
/// 우리 쪽 사설망의 엉뚱한 기계로 가거나 아무 데도 닿지 못한다. 제어 연결이 공인 주소로 서
/// 있는데 데이터 주소만 사설이면, **포트만 살리고 호스트는 제어 연결 쪽을 쓴다.**
///
/// 반대로 제어 연결 자체가 사설(사내망 서버)이면 사설 데이터 주소가 정상이므로 손대지 않는다.
fn passive_target(advertised: SocketAddr, control: IpAddr) -> SocketAddr {
    if is_site_local(advertised.ip()) && !is_site_local(control) {
        SocketAddr::new(control, advertised.port())
    } else {
        advertised
    }
}

/// 인터넷 밖으로 나갈 수 없는 주소인가
fn is_site_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(),
        // 고유 로컬(fc00::/7)과 링크 로컬(fe80::/10)이 IPv4의 사설 대역에 해당한다
        IpAddr::V6(v6) => {
            let head = v6.segments()[0];
            v6.is_loopback() || (head & 0xfe00) == 0xfc00 || (head & 0xffc0) == 0xfe80
        }
    }
}

fn tls_connector() -> RemoteResult<NativeTlsConnector> {
    TlsConnector::new()
        .map(NativeTlsConnector::from)
        .map_err(|e| RemoteError::Connect {
            detail: crate::i18n::dynamic::tls_setup_failed(&e.to_string()),
        })
}

/// LIST 응답 줄들을 항목으로 옮긴다. **해석하지 못한 줄은 그 줄만 버리고 나머지는 살린다** —
/// 방언 하나 때문에 목록 전체가 빈 화면이 되면 사용자가 할 수 있는 일이 없다.
fn entries_from_list(lines: &[String]) -> Vec<RemoteEntry> {
    lines
        .iter()
        .filter_map(|line| parse_list_line(line))
        .filter(|entry| entry.name != ".")
        .collect()
}

/// MLSD 응답 줄들을 항목으로 옮긴다
fn entries_from_mlsd(lines: &[String]) -> Vec<RemoteEntry> {
    lines
        .iter()
        .filter_map(|line| parse_mlsd_line(line))
        .collect()
}

/// LIST 한 줄 → 항목. POSIX(`ls -l`) 방언을 먼저 보고, 아니면 DOS(IIS) 방언으로 본다.
///
/// **MLSD 파서로는 넘기지 않는다** — 그쪽은 `=`가 없는 토큰을 조용히 건너뛰어 아무 문자열이나
/// 받아들이므로, 깨진 줄이 이름만 있는 가짜 항목으로 살아남는다.
fn parse_list_line(line: &str) -> Option<RemoteEntry> {
    if let Ok(file) = ListParser::parse_posix(line) {
        let mut entry = entry_from_file(&file);
        entry.mode = Some(mode_bits(&file));
        entry.owner = posix_owner(line).or_else(|| file.uid().map(|uid| uid.to_string()));
        return Some(entry);
    }
    // DOS 방언에는 권한·소유자가 아예 없다 — 권한 열·소유자 열이 빈칸이 된다
    let file = ListParser::parse_dos(line).ok()?;
    Some(entry_from_file(&file))
}

/// MLSD 한 줄 → 항목.
///
/// 권한·소유자·시각은 **그 사실(fact)이 실제로 온 경우에만** 채운다. 파서는 사실이 없으면
/// `0o777`·1970년을 기본값으로 채워 넣는데, 그것을 그대로 믿으면 화면이 서버가 하지 않은 말을 한다.
fn parse_mlsd_line(line: &str) -> Option<RemoteEntry> {
    // 사실 목록과 이름은 마지막 `;`로 갈린다 (`type=dir;modify=...; pub`)
    let (facts, _) = line.rsplit_once(';')?;

    let mut kind = None;
    let mut mode = None;
    let mut owner_name = None;
    let mut uid = None;
    let mut has_modify = false;
    let mut fact_count = 0usize;
    for token in facts.split(';') {
        let Some((key, value)) = token.split_once('=') else {
            continue;
        };
        fact_count += 1;
        match key.trim().to_ascii_lowercase().as_str() {
            "type" => kind = Some(value.to_ascii_lowercase()),
            "unix.mode" => mode = u32::from_str_radix(value, 8).ok(),
            "unix.ownername" | "unix.owner" => owner_name = Some(value.to_owned()),
            "unix.uid" => uid = Some(value.to_owned()),
            "modify" => has_modify = true,
            _ => {}
        }
    }
    if fact_count == 0 {
        return None;
    }

    let file = ListParser::parse_mlsd(line).ok()?;
    let mut entry = entry_from_file(&file);
    match kind.as_deref() {
        // 현재 디렉터리 자신 — 목록의 항목이 아니다
        Some("cdir") => return None,
        // 상위 디렉터리. 이름 자리에 서버 경로가 오므로 목록에서 쓰는 표기로 바꾼다
        Some("pdir") => entry.name = "..".to_owned(),
        _ => {}
    }
    if entry.name.is_empty() || entry.name == "." {
        return None;
    }
    entry.mode = mode;
    entry.owner = owner_name.or(uid);
    if !has_modify {
        entry.modified = None;
    }
    Some(entry)
}

/// 파서가 준 공통 부분만 옮긴다 — 권한·소유자는 방언마다 달라 호출부가 채운다
fn entry_from_file(file: &ListFile) -> RemoteEntry {
    RemoteEntry {
        name: file.name().to_owned(),
        is_dir: file.is_directory(),
        is_symlink: file.is_symlink(),
        link_target: file
            .symlink()
            .map(|target| target.to_string_lossy().into_owned())
            .filter(|target| !target.is_empty()),
        size: file.size() as u64,
        modified: file
            .modified()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|since| since.as_secs() as i64),
        mode: None,
        owner: None,
    }
}

/// 권한 비트 — 파서는 소유자·그룹·기타의 rwx 여부만 내주므로 여기서 9비트로 되돌린다
fn mode_bits(file: &ListFile) -> u32 {
    let mut mode = 0;
    for (who, shift) in [
        (PosixPexQuery::Owner, 6),
        (PosixPexQuery::Group, 3),
        (PosixPexQuery::Others, 0),
    ] {
        if file.can_read(who) {
            mode |= 0o4 << shift;
        }
        if file.can_write(who) {
            mode |= 0o2 << shift;
        }
        if file.can_execute(who) {
            mode |= 0o1 << shift;
        }
    }
    mode
}

/// POSIX LIST 줄의 소유자 칸 (`-rw-r--r-- 1 user group ...`의 `user`).
///
/// 파서는 이 칸을 숫자로 읽어 보고 실패하면 버리는데, 대부분의 서버는 **이름**을 준다 —
/// 그대로 두면 소유자 열이 늘 빈칸이 된다 (FR-31).
fn posix_owner(line: &str) -> Option<String> {
    let mut tokens = line.split_whitespace();
    // 첫 칸은 종류 1글자 + 권한 9글자
    let permissions = tokens.next()?;
    if permissions.len() != 10 {
        return None;
    }
    let _link_count = tokens.next()?;
    let owner = tokens.next()?;
    Some(owner.to_owned())
}

/// 서버가 명령 자체를 모른다고 답했는가 (MLSD 미지원 판정)
fn is_unsupported(err: &FtpError) -> bool {
    matches!(err, FtpError::UnexpectedResponse(response) if matches!(response.status.code(), 500 | 502))
}

/// 데이터 연결을 열지 못한 실패인가 (수동형 → 능동형 전환 판정)
fn is_data_connection_failure(err: &FtpError) -> bool {
    matches!(err, FtpError::ConnectionError(_))
}

/// 라이브러리 오류를 도메인 오류로 옮긴다. **서버 원문은 어느 갈래로 가든 그대로 남는다** —
/// 실패 화면이 그 문구를 보여야 사용자가 원인을 짚는다 (README §5).
/// 목록 원문을 줄 단위로 쪼개 사이트 문자셋으로 옮긴다 (FR-46).
///
/// 서버가 UTF-8 모드를 받아들였으면 문자셋 설정과 무관하게 UTF-8로 읽는다 —
/// 그때 서버가 보내는 것이 UTF-8이기 때문이다
fn decode_lines(raw: &[u8], charset: &Charset, utf8_mode: bool) -> Vec<String> {
    let charset = if utf8_mode { &Charset::Utf8 } else { charset };
    raw.split(|byte| *byte == b'\n')
        .map(|line| {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            charset::decode_name(line, charset)
        })
        .filter(|line| !line.trim().is_empty())
        .collect()
}

fn classify(err: FtpError, operation: RemoteOp, path: Option<&str>) -> RemoteError {
    match err {
        FtpError::ConnectionError(e) => RemoteError::Connect {
            detail: e.to_string(),
        },
        FtpError::SecureError(detail) => RemoteError::Connect { detail },
        FtpError::InvalidAddress(e) => RemoteError::Connect {
            detail: e.to_string(),
        },
        FtpError::BadResponse => RemoteError::Protocol {
            detail: crate::i18n::dynamic::reply_unreadable(operation.label()),
        },
        FtpError::DataConnectionAlreadyOpen => RemoteError::Protocol {
            detail: crate::i18n::dynamic::data_connection_open(operation.label()),
        },
        FtpError::UnexpectedResponse(response) => classify_response(response, operation, path),
    }
}

fn classify_response(response: Response, operation: RemoteOp, path: Option<&str>) -> RemoteError {
    let code = response.status.code();
    let detail = response.to_string();
    let path_of = || path.unwrap_or_default().to_owned();
    match code {
        // 421 — 서비스 불가·동시 접속 초과. 연결을 다시 세워야 한다
        421 => RemoteError::Connect { detail },
        530 | 532 => RemoteError::Auth { detail },
        500 | 502 | 504 => RemoteError::Unsupported {
            operation: operation.label().to_owned(),
            detail,
        },
        // 550은 "없음"과 "권한 없음"에 함께 쓰인다 — 서버 문구로만 갈린다
        550 => {
            if mentions_permission(&detail) {
                RemoteError::PermissionDenied {
                    path: path_of(),
                    detail,
                }
            } else {
                RemoteError::NotFound {
                    path: path_of(),
                    detail,
                }
            }
        }
        553 => RemoteError::PermissionDenied {
            path: path_of(),
            detail,
        },
        _ => RemoteError::Protocol { detail },
    }
}

fn mentions_permission(detail: &str) -> bool {
    let lowered = detail.to_ascii_lowercase();
    ["permission", "denied", "access", "forbidden"]
        .iter()
        .any(|word| lowered.contains(word))
        // **화면 언어를 따르지 않는다** — 여기서 보는 것은 서버가 보낸 응답 원문이라,
        // 앱을 영어로 두었다고 서버 문구가 영어가 되지는 않는다. 한국어로 답하는 서버를
        // 위해 그 낱말을 그대로 둔다
        || detail.contains("권한")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::types::SiteId;

    /// 끝나지 못한 전송은 이 연결을 버릴 것으로 표시한다 — 취소든 실패든 마찬가지다.
    ///
    /// **네 갈래(받기 취소·받기 실패·올리기 취소·올리기 실패)가 모두 이 한 자리를 지난다** —
    /// 아래 두 시험이 `download`·`upload` 본문이 실제로 그것을 거치는지 소스에서 확인하므로,
    /// 여기서 판정 규칙을 재고 거기서 경로를 잰다(실서버 없이 네 갈래를 다 태울 수는 없다)
    #[test]
    fn 끝나지_못한_전송은_연결을_버릴_것으로_표시한다() {
        let mut session = FtpSession::new();
        assert!(!session.needs_reset(), "새 세션은 멀쩡하다");

        session.mark_dirty_if_interrupted(&Err(RemoteError::Cancelled));
        assert!(session.needs_reset(), "취소한 전송 뒤에는 다시 세워야 한다");
    }

    #[test]
    fn 온전히_끝난_전송은_연결을_그대로_쓴다() {
        let mut session = FtpSession::new();
        session.mark_dirty_if_interrupted(&Ok(1024));
        assert!(
            !session.needs_reset(),
            "성공한 전송에까지 재수립을 걸면 가장 흔한 길에 왕복이 붙는다"
        );
    }

    /// `download`가 끊긴 전송을 표시하는 자리를 **건너뛸 수 없는가**.
    ///
    /// 소스를 훑어 재는 이유는 네 갈래(받기·올리기 × 취소·실패)를 실제로 태우려면 살아
    /// 있는 FTP 서버가 필요하기 때문이다 — 이 레포의 규약 강제 시험과 같은 방식이다.
    ///
    /// **`?`가 0개인지도 함께 잰다** — 표시 호출이 있는지만 보면 그 앞의 `?`가 그것을
    /// 건너뛰는 것을 놓친다. 실제로 놓쳤고(`RETR`·`REST`·`stream()` 셋), 그래서 죽은
    /// 연결이 다시 서지 못한 채 재시도가 계속 실패했다 (사용자 보고 2026-09-17)
    #[test]
    fn 받기는_끊긴_전송을_표시하는_자리를_지난다() {
        let body = 함수_본문("fn download(");
        assert!(
            body.contains("mark_dirty_if_interrupted"),
            "받기의 취소·실패가 연결을 버릴 것으로 표시하지 않는다"
        );
        let code = 주석을_뺀_줄(&body);
        assert!(
            !code.contains('?'),
            "받기에 조기 반환이 있다 — 그 갈래는 표시를 건너뛴다: {code}"
        );
    }

    #[test]
    fn 올리기는_끊긴_전송을_표시하는_자리를_지난다() {
        let body = 함수_본문("fn upload(");
        assert!(
            body.contains("mark_dirty_if_interrupted"),
            "올리기의 취소·실패가 연결을 버릴 것으로 표시하지 않는다"
        );
        let code = 주석을_뺀_줄(&body);
        assert!(
            !code.contains('?'),
            "올리기에 조기 반환이 있다 — 그 갈래는 표시를 건너뛴다: {code}"
        );
    }

    /// 주석 줄을 걷어낸다 — **재는 대상은 코드이지 서술이 아니다**.
    ///
    /// `함수_본문`은 다음 함수 머리 앞까지 자르므로 **그 함수의 doc 주석이 딸려 온다**.
    /// 그 주석이 `?`를 서술하면(이 자리에서는 그럴 이유가 충분하다) 걸러 내지 않는 판정이
    /// 거짓 실패를 낸다
    fn 주석을_뺀_줄(body: &str) -> String {
        body.lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 이 파일에서 그 함수의 본문만 잘라 낸다 — 다음 `    fn ` 앞까지
    fn 함수_본문(머리: &str) -> String {
        let source = include_str!("ftp.rs");
        let start = source.find(머리).expect("함수를 찾지 못했다");
        let rest = &source[start + 머리.len()..];
        // 다음 함수 머리 앞까지가 이 함수의 본문이다
        let end = rest
            .find(
                "
    fn ",
            )
            .unwrap_or(rest.len());
        rest[..end].to_owned()
    }

    fn lines(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn posix_목록_줄이_항목으로_옮겨진다() {
        let entry = parse_list_line("-rw-r--r-- 1 deploy staff 1234 Nov 5 13:46 example.txt")
            .expect("POSIX 줄을 해석하지 못했다");
        assert_eq!(entry.name, "example.txt");
        assert!(!entry.is_dir);
        assert_eq!(entry.size, 1234);
        assert_eq!(entry.permissions_string().as_deref(), Some("rw-r--r--"));
        assert_eq!(entry.owner.as_deref(), Some("deploy"));
        assert!(entry.modified.is_some());
    }

    #[test]
    fn posix_폴더와_큰_파일과_한글_이름을_옮긴다() {
        let dir = parse_list_line("drwxr-xr-x 4 root root 4096 Jan 10 2024 배포").expect("폴더");
        assert!(dir.is_dir);
        assert_eq!(dir.name, "배포");
        assert_eq!(dir.permissions_string().as_deref(), Some("rwxr-xr-x"));

        let big = parse_list_line("-rw-rw---- 1 1000 1000 8589934592 Feb 2 2025 이미지 백업.iso")
            .expect("큰 파일");
        // 4GB를 넘는 크기가 잘리지 않는다
        assert_eq!(big.size, 8_589_934_592);
        assert_eq!(big.name, "이미지 백업.iso");
        // 서버가 이름 대신 uid를 주면 그 숫자를 그대로 보인다
        assert_eq!(big.owner.as_deref(), Some("1000"));
    }

    #[test]
    fn posix_심볼릭_링크는_대상을_따로_담는다() {
        let link = parse_list_line("lrwxrwxrwx 1 root root 7 Mar 3 09:12 current -> releases/42")
            .expect("링크");
        assert!(link.is_symlink);
        assert_eq!(link.name, "current");
        // 이름 뒤에 `→ 대상`으로 붙일 원천이다 (FR-31)
        assert_eq!(link.link_target.as_deref(), Some("releases/42"));
    }

    #[test]
    fn dos_방언은_권한과_소유자를_비운다() {
        let entry = parse_list_line("04-08-14  03:09PM       403 readme.txt").expect("DOS 줄");
        assert_eq!(entry.name, "readme.txt");
        assert_eq!(entry.size, 403);
        // IIS 목록에는 POSIX 권한이 없다 — 기본값 777을 지어내지 않는다
        assert_eq!(entry.mode, None);
        assert_eq!(entry.owner, None);

        let dir = parse_list_line("10-19-20  03:19PM       <DIR> pub").expect("DOS 폴더");
        assert!(dir.is_dir);
    }

    #[test]
    fn 해석하지_못한_줄은_그_줄만_버린다() {
        let raw = lines(&[
            "-rw-r--r-- 1 user group 10 Nov 5 13:46 a.txt",
            "총 사용량 20",
            "!!! 깨진 줄 !!!",
            "drwxr-xr-x 2 user group 4096 Nov 5 13:46 sub",
            "",
        ]);
        let entries = entries_from_list(&raw);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["a.txt", "sub"], "나머지 목록이 살아남아야 한다");
    }

    #[test]
    fn 현재_디렉터리_항목은_목록에서_빠진다() {
        let raw = lines(&[
            "drwxr-xr-x 2 user group 4096 Nov 5 13:46 .",
            "drwxr-xr-x 2 user group 4096 Nov 5 13:46 ..",
            "-rw-r--r-- 1 user group 10 Nov 5 13:46 a.txt",
        ]);
        let names: Vec<String> = entries_from_list(&raw)
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, vec!["..", "a.txt"]);
    }

    #[test]
    fn mlsd_줄은_사실이_있을_때만_권한과_시각을_담는다() {
        let with_facts = parse_mlsd_line(
            "type=file;size=8192;modify=20181105163248;UNIX.mode=0640;UNIX.uid=1000; omar.txt",
        )
        .expect("MLSD 줄");
        assert_eq!(with_facts.name, "omar.txt");
        assert_eq!(with_facts.size, 8192);
        assert_eq!(
            with_facts.permissions_string().as_deref(),
            Some("rw-r-----")
        );
        assert_eq!(with_facts.owner.as_deref(), Some("1000"));
        assert!(with_facts.modified.is_some());

        let bare = parse_mlsd_line("type=file;size=10; bare.txt").expect("사실이 적은 줄");
        // 서버가 주지 않은 값을 파서 기본값(0o777·1970년)으로 채우지 않는다
        assert_eq!(bare.mode, None);
        assert_eq!(bare.owner, None);
        assert_eq!(bare.modified, None);
    }

    #[test]
    fn mlsd의_소유자_이름이_uid보다_앞선다() {
        let entry = parse_mlsd_line("type=file;size=1;UNIX.uid=1000;UNIX.ownername=deploy; app.js")
            .expect("줄");
        assert_eq!(entry.owner.as_deref(), Some("deploy"));
    }

    #[test]
    fn mlsd의_현재_디렉터리는_빠지고_상위는_점_두_개가_된다() {
        let raw = lines(&[
            "type=cdir;modify=20201019151930; /pub",
            "type=pdir;modify=20201019151930; /",
            "type=dir;modify=20201019151930; docs",
            "사실이 하나도 없는 줄",
        ]);
        let names: Vec<String> = entries_from_mlsd(&raw)
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, vec!["..", "docs"]);
    }

    fn site(protocol: Protocol, encryption: Encryption) -> SiteRecord {
        let mut record = SiteRecord::new(SiteId(1), "테스트".to_owned());
        record.protocol = protocol;
        record.encryption = encryption;
        record
    }

    #[test]
    fn ftps_사이트는_평문으로_내려가지_않는다() {
        // 프로토콜로 FTPS를 고른 것 자체가 "암호화해서 연결하라"는 뜻이다
        assert_eq!(
            effective_encryption(&site(Protocol::Ftps, Encryption::Plain)),
            Encryption::ExplicitRequired
        );
        // FTP는 사용자가 고른 값을 그대로 따른다
        assert_eq!(
            effective_encryption(&site(Protocol::Ftp, Encryption::Plain)),
            Encryption::Plain
        );
        assert_eq!(
            effective_encryption(&site(Protocol::Ftp, Encryption::ExplicitIfAvailable)),
            Encryption::ExplicitIfAvailable
        );
    }

    #[test]
    fn 사설_주소로_온_수동형_응답은_제어_연결_호스트로_바뀐다() {
        let control: IpAddr = "203.0.113.7".parse().expect("공인 주소");
        // NAT 뒤 서버가 자기 사설 주소를 적어 보냈다 — 포트만 살리고 호스트를 갈아끼운다
        let advertised: SocketAddr = "192.168.0.5:50123".parse().expect("사설 주소");
        assert_eq!(
            passive_target(advertised, control),
            "203.0.113.7:50123".parse::<SocketAddr>().expect("결과")
        );

        // 서버가 제대로 공인 주소를 줬으면 손대지 않는다
        let public: SocketAddr = "198.51.100.9:50124".parse().expect("공인 주소");
        assert_eq!(passive_target(public, control), public);
    }

    #[test]
    fn 사내망_서버의_사설_응답은_그대로_쓴다() {
        // 제어 연결부터 사설이면 사설 데이터 주소가 정상이다
        let control: IpAddr = "10.0.0.2".parse().expect("사설 주소");
        let advertised: SocketAddr = "10.0.0.2:50125".parse().expect("사설 주소");
        assert_eq!(passive_target(advertised, control), advertised);

        let loopback: IpAddr = "127.0.0.1".parse().expect("루프백");
        let local: SocketAddr = "127.0.0.1:50126".parse().expect("루프백");
        assert_eq!(passive_target(local, loopback), local);
    }

    #[test]
    fn ipv6의_고유_로컬과_링크_로컬도_사설로_본다() {
        // 비트 마스크를 한 칸만 잘못 잡아도 공인 주소를 사설로 오판한다 — 경계를 고정해 둔다
        let site_local = ["fc00::1", "fdff::1", "fe80::1", "febf::1", "::1"];
        for raw in site_local {
            let ip: IpAddr = raw.parse().expect("주소");
            assert!(is_site_local(ip), "{raw}은(는) 사설이어야 한다");
        }
        let public = ["2001:db8::1", "fec0::1", "fb00::1", "2400::1"];
        for raw in public {
            let ip: IpAddr = raw.parse().expect("주소");
            assert!(!is_site_local(ip), "{raw}은(는) 사설이 아니어야 한다");
        }
    }

    fn response_error(code: u32, body: &str) -> FtpError {
        FtpError::UnexpectedResponse(Response::new(Status::from(code), body.as_bytes().to_vec()))
    }

    #[test]
    fn 응답_코드가_오류_갈래를_가른다() {
        let auth = classify(
            response_error(530, "530 Login incorrect"),
            RemoteOp::Login,
            None,
        );
        assert!(matches!(auth, RemoteError::Auth { .. }));
        assert!(auth.detail().contains("530 Login incorrect"));

        let denied = classify(
            response_error(550, "550 Permission denied"),
            RemoteOp::Raw("DELE"),
            Some("/var/www/sw.js"),
        );
        assert!(
            matches!(&denied, RemoteError::PermissionDenied { path, .. } if path == "/var/www/sw.js")
        );

        let missing = classify(
            response_error(550, "550 No such file or directory"),
            RemoteOp::Raw("CWD"),
            Some("/none"),
        );
        assert!(matches!(&missing, RemoteError::NotFound { path, .. } if path == "/none"));

        let unsupported = classify(
            response_error(502, "502 Command not implemented"),
            RemoteOp::Raw("MLSD"),
            None,
        );
        assert!(
            matches!(&unsupported, RemoteError::Unsupported { operation, .. } if operation == "MLSD")
        );

        // 서버가 다시 연결하라고 답한 경우
        assert!(matches!(
            classify(
                response_error(421, "421 Too many connections"),
                RemoteOp::Connect,
                None
            ),
            RemoteError::Connect { .. }
        ));
    }

    #[test]
    fn mlsd_미지원_판정은_명령을_모른다는_응답에만_반응한다() {
        assert!(is_unsupported(&response_error(500, "500 Unknown command")));
        assert!(is_unsupported(&response_error(502, "502 Not implemented")));
        assert!(!is_unsupported(&response_error(550, "550 Not found")));
    }

    #[test]
    fn 연결_전에는_명령이_조용히_실패하지_않는다() {
        let mut session = FtpSession::new();
        let err = session.pwd().expect_err("연결 전 PWD가 성공했다");
        assert!(matches!(err, RemoteError::Protocol { .. }));
        // 연결한 적이 없으면 종료는 할 일이 없다
        assert!(session.quit().is_ok());
    }

    /// 실제 서버 왕복 — `FE_TEST_FTP_URL`(`ftp://사용자:비밀번호@호스트:포트/경로`)이 있을 때만 돈다.
    /// 자격증명은 레포에 넣지 않으며 실패 메시지에도 담지 않는다 (D25·보안 규칙).
    #[test]
    fn 실서버_왕복은_환경변수가_있을_때만_돈다() {
        let Ok(url) = std::env::var("FE_TEST_FTP_URL") else {
            println!("건너뜀 — FE_TEST_FTP_URL이 설정되지 않았습니다 (실서버 테스트는 선택 사항)");
            return;
        };
        let Some((record, path)) = parse_test_url(&url) else {
            panic!("FE_TEST_FTP_URL 형식이 ftp://사용자:비밀번호@호스트:포트/경로가 아닙니다");
        };
        let password = test_password(&url);

        let mut session = FtpSession::new();
        session.connect(&record).expect("연결");
        session.login(&record, &password).expect("로그인");
        let home = session.pwd().expect("PWD");
        println!("서버 홈: {home}");
        let entries = session.list(&path).expect("목록");
        println!("항목 {}개", entries.len());
        session.quit().expect("종료");
    }

    /// 테스트용 URL 해석 — 본 구현의 URL 파서는 T13(`remote::url`)이 만든다
    fn parse_test_url(url: &str) -> Option<(SiteRecord, RemotePath)> {
        let rest = url
            .strip_prefix("ftps://")
            .map(|r| (r, Protocol::Ftps))
            .or_else(|| url.strip_prefix("ftp://").map(|r| (r, Protocol::Ftp)));
        let (rest, protocol) = rest?;
        let (credentials, rest) = rest.split_once('@')?;
        let (user, _) = credentials.split_once(':')?;
        let (authority, path) = match rest.split_once('/') {
            Some((authority, path)) => (authority, format!("/{path}")),
            None => (rest, "/".to_owned()),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse().ok()?),
            None => (authority, protocol.default_port()),
        };

        let mut record = SiteRecord::new(SiteId(1), "실서버".to_owned());
        record.protocol = protocol;
        record.host = host.to_owned();
        record.port = port;
        record.user = user.to_owned();
        Some((record, RemotePath::new(&path)))
    }

    fn test_password(url: &str) -> String {
        url.split_once("://")
            .and_then(|(_, rest)| rest.split_once('@'))
            .and_then(|(credentials, _)| credentials.split_once(':'))
            .map(|(_, password)| password.to_owned())
            .unwrap_or_default()
    }

    #[test]
    fn 목록_원문을_사이트_문자셋으로_옮긴다() {
        // 사용자 보고(2026-08-05): CP949 서버의 한글 폴더 이름이 `?? ???`로 깨졌다.
        // `suppaftp::list()`가 줄을 UTF-8로 단정해 뭉갠 뒤 넘겨주기 때문이라, 원문 바이트를
        // 직접 받아 이 함수가 옮긴다
        let mut raw = b"drwxr-xr-x 2 ftp ftp 4096 Aug  5 21:00 ".to_vec();
        // `한글 폴더`의 CP949 바이트
        raw.extend_from_slice(&[0xC7, 0xD1, 0xB1, 0xDB, 0x20, 0xC6, 0xFA, 0xB4, 0xF5]);
        raw.extend_from_slice(
            b"
",
        );

        let cp949 = Charset::Named("CP949".to_owned());
        let lines = decode_lines(&raw, &cp949, false);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].ends_with("한글 폴더"),
            "CP949 이름이 옮겨지지 않았다: {}",
            lines[0]
        );
        // 서버가 UTF-8 모드를 받아들였으면 설정과 무관하게 UTF-8로 읽는다
        let utf8_raw = "drwxr-xr-x 2 ftp ftp 4096 Aug  5 21:00 한글 폴더
"
        .as_bytes();
        let lines = decode_lines(utf8_raw, &cp949, true);
        assert!(lines[0].ends_with("한글 폴더"), "{}", lines[0]);
        // 빈 줄은 담지 않는다
        assert!(
            decode_lines(
                b"

",
                &Charset::Utf8,
                false
            )
            .is_empty()
        );
    }
}
