//! SFTP 세션 — `RemoteSession`의 SSH 계열 구현 (FR-30·FR-31·FR-37·FR-39).
//!
//! FTP와 달리 전송 계층을 SSH가 이미 암호화하므로 암호화 설정이 없고, 대신 **서버가 맞는지**를
//! 우리가 판정해야 한다 — 그 판정은 `remote::hostkey`가 하고 사용자에게 묻는 화면은 `ui`(T10)가
//! 만든다. 이 모듈은 물어볼 통로(`HostKeyPrompt`)만 열어 둔다.
//!
//! 인증은 **비밀번호와 개인 키 파일** 두 가지다 (FR-66) — 갈래가 둘뿐이라 인증 방식을 트레이트로
//! 열지 않고 `login` 안의 `match` 하나로 가른다. 에이전트(Pageant·OpenSSH) 인증은 계속 범위 밖이다
//! (PRD Out of Scope).
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::TcpStream;
use std::panic::AssertUnwindSafe;
use std::path::Path;

use ssh2::{Error as Ssh2Error, ErrorCode, FileStat, HashType, OpenFlags, OpenType, Session, Sftp};

use crate::remote::hostkey::{HostKeyCheck, HostKeyDecision, KnownHosts, fingerprint_sha256};
use crate::remote::types::{
    LogonType, Progress, RemoteEntry, RemoteError, RemoteOp, RemotePath, RemoteResult,
    RemoteSession, SiteRecord,
};
use crate::remote::{Pumped, pump};

/// SFTP 프로토콜의 상태 코드 (draft-ietf-secsh-filexfer). 라이브러리가 숫자로만 넘겨 준다
const FX_NO_SUCH_FILE: i32 = 2;
const FX_PERMISSION_DENIED: i32 = 3;
const FX_OP_UNSUPPORTED: i32 = 8;

/// 새 파일·새 폴더의 기본 권한. FileZilla·OpenSSH가 쓰는 값과 같다
const NEW_FILE_MODE: i32 = 0o644;
const NEW_DIR_MODE: i32 = 0o755;

/// 지문 확인을 사용자에게 묻는 통로 — 대화 화면은 T10이 만든다.
///
/// **이 통로가 없으면 미등록·변경 지문은 거절된다** — 자동으로 수락하는 경로를 두지 않는다 (D15).
pub type HostKeyPrompt = Box<dyn FnMut(&HostKeyCheck) -> HostKeyDecision + Send>;

/// SFTP 연결 하나
pub struct SftpSession {
    /// 연결 전과 `quit` 후에는 `None`이다
    session: Option<Session>,
    /// 로그인 뒤에 열린다 — SFTP 하위 시스템은 인증이 끝나야 시작할 수 있다
    sftp: Option<Sftp>,
    /// 알려진 서버 지문 표. **처음 필요할 때 워커 스레드에서 읽는다** — 세션을 만드는 곳은
    /// 화면(UI 스레드)이라, 거기서 읽으면 파일 I/O가 UI 스레드에 놓인다(AGENTS 계층 규약)
    known_hosts: Option<KnownHosts>,
    prompt: Option<HostKeyPrompt>,
}

impl SftpSession {
    /// 지문 표는 이 세션이 **연결할 때**(워커 스레드) 파일에서 읽는다.
    ///
    /// 만들 때 읽지 않는 이유는 둘이다 — ① 세션 조립은 UI 스레드에서 일어난다,
    /// ② 앱이 사본을 들고 있으면 방금 수락한 서버를 다음 연결에서 또 묻게 된다
    pub fn new(prompt: Option<HostKeyPrompt>) -> SftpSession {
        SftpSession {
            session: None,
            sftp: None,
            known_hosts: None,
            prompt,
        }
    }

    /// 지문 표를 직접 주입해 만든다 — **테스트 전용 통로**다.
    /// 파일에서 읽는 경로를 타면 테스트가 사용자 폴더의 실제 표를 건드린다
    #[cfg(test)]
    fn with_known_hosts(known_hosts: KnownHosts, prompt: Option<HostKeyPrompt>) -> SftpSession {
        SftpSession {
            session: None,
            sftp: None,
            known_hosts: Some(known_hosts),
            prompt,
        }
    }

    fn session(&mut self) -> RemoteResult<&mut Session> {
        self.session.as_mut().ok_or_else(|| RemoteError::Protocol {
            detail: crate::i18n::remote_not_connected_err().to_owned(),
        })
    }

    fn sftp(&self) -> RemoteResult<&Sftp> {
        self.sftp.as_ref().ok_or_else(|| RemoteError::Protocol {
            detail: crate::i18n::remote_not_logged_in().to_owned(),
        })
    }

    /// 서버 지문을 대조하고, 처음 보거나 바뀌었으면 사용자에게 묻는다.
    ///
    /// 이 함수는 `connect` 안에서만 불리므로 **워커 스레드에서 실행된다** — 지문 표를 여기서
    /// 처음 읽는 것이 곧 "파일 I/O를 UI 스레드에 두지 않는다"이다
    fn verify_host_key(&mut self, host: &str, port: u16, fingerprint: &str) -> RemoteResult<()> {
        let known = self.known_hosts.get_or_insert_with(KnownHosts::load);
        let check = known.check(host, port, fingerprint);
        let accepted = resolve_host_key(known, host, port, &check, self.prompt.as_deref_mut())?;
        if accepted {
            // 수락한 지문만 파일에 남긴다 (판정과 저장을 나눠 두어 판정이 단위 테스트 대상이 된다)
            known.save();
        }
        Ok(())
    }
}

impl RemoteSession for SftpSession {
    fn connect(&mut self, site: &SiteRecord) -> RemoteResult<()> {
        if !site.protocol.is_ssh() {
            return Err(RemoteError::Protocol {
                detail: crate::i18n::remote_ftp_site_on_sftp().to_owned(),
            });
        }
        let tcp = TcpStream::connect(site.address()).map_err(|e| RemoteError::Connect {
            detail: e.to_string(),
        })?;

        let mut session = Session::new().map_err(|e| classify(e, RemoteOp::SessionSetup, None))?;
        // 이 세션은 워커 스레드가 독점하므로 동기(블로킹) 호출로 쓴다 (NFR-10)
        session.set_blocking(true);
        session.set_tcp_stream(tcp);
        session
            .handshake()
            .map_err(|e| classify(e, RemoteOp::SshHandshake, None))?;

        let fingerprint = session
            .host_key_hash(HashType::Sha256)
            .map(fingerprint_sha256)
            .ok_or_else(|| RemoteError::HostKey {
                detail: crate::i18n::remote_no_fingerprint().to_owned(),
            })?;
        self.verify_host_key(&site.host, site.port, &fingerprint)?;

        self.session = Some(session);
        Ok(())
    }

    fn login(&mut self, site: &SiteRecord, secret: &str) -> RemoteResult<()> {
        let user = site.effective_user().to_owned();
        // 무엇으로 증명할지는 사이트가 이미 정해 뒀다 — 여기서는 그 결정을 읽기만 한다
        let kind = auth_kind(site)?;
        let session = self.session()?;
        let attempt = match &kind {
            AuthKind::Password => session.userauth_password(&user, secret),
            AuthKind::KeyFile(path) => {
                // 공개키 파일은 주지 않는다 — libssh2가 개인 키에서 유도한다.
                // 암호가 비어 있으면 `None` — 암호 없는 키가 흔하다
                let passphrase = (!secret.is_empty()).then_some(secret);
                session.userauth_pubkey_file(&user, None, path, passphrase)
            }
        };
        if let Err(err) = attempt {
            return Err(auth_error(session, &user, &kind, err));
        }
        if !session.authenticated() {
            return Err(RemoteError::Auth {
                detail: crate::i18n::remote_login_rejected().to_owned(),
            });
        }
        let sftp = session
            .sftp()
            .map_err(|e| classify(e, RemoteOp::SftpStart, None))?;
        self.sftp = Some(sftp);
        Ok(())
    }

    fn pwd(&mut self) -> RemoteResult<RemotePath> {
        let sftp = self.sftp()?;
        // SFTP에는 작업 디렉터리가 없다 — `.`의 실제 경로가 서버가 정한 홈이다.
        // `realpath`도 아래 `list`와 같은 경로 변환을 거치므로 같은 방어막이 필요하다
        guard_path_panic(crate::i18n::remote_subject_home(), || {
            let home = sftp
                .realpath(Path::new("."))
                .map_err(|e| classify(e, RemoteOp::Home, None))?;
            Ok(RemotePath::new(&home.to_string_lossy()))
        })
    }

    fn list(&mut self, path: &RemotePath) -> RemoteResult<Vec<RemoteEntry>> {
        let sftp = self.sftp()?;
        guard_path_panic(crate::i18n::remote_subject_names(), || {
            read_directory(sftp, path)
        })
    }

    fn cwd(&mut self, path: &RemotePath) -> RemoteResult<()> {
        // SFTP는 작업 디렉터리를 옮기는 명령이 없다 — 갈 수 있는 곳인지만 확인해 준다
        let stat = self
            .sftp()?
            .stat(Path::new(path.as_str()))
            .map_err(|e| classify(e, RemoteOp::Move, Some(path.as_str())))?;
        if stat.is_dir() {
            Ok(())
        } else {
            Err(RemoteError::NotFound {
                path: path.as_str().to_owned(),
                detail: crate::i18n::remote_not_a_folder().to_owned(),
            })
        }
    }

    fn mkdir(&mut self, path: &RemotePath) -> RemoteResult<()> {
        self.sftp()?
            .mkdir(Path::new(path.as_str()), NEW_DIR_MODE)
            .map_err(|e| classify(e, RemoteOp::Mkdir, Some(path.as_str())))
    }

    fn remove(&mut self, path: &RemotePath) -> RemoteResult<()> {
        self.sftp()?
            .unlink(Path::new(path.as_str()))
            .map_err(|e| classify(e, RemoteOp::Remove, Some(path.as_str())))
    }

    fn rmdir(&mut self, path: &RemotePath) -> RemoteResult<()> {
        self.sftp()?
            .rmdir(Path::new(path.as_str()))
            .map_err(|e| classify(e, RemoteOp::Rmdir, Some(path.as_str())))
    }

    fn rename(&mut self, from: &RemotePath, to: &RemotePath) -> RemoteResult<()> {
        // 덮어쓰기 플래그를 주지 않는다 — 이미 있는 이름이면 서버가 거절하고, 덮어쓸지는 사용자가 정한다
        self.sftp()?
            .rename(Path::new(from.as_str()), Path::new(to.as_str()), None)
            .map_err(|e| classify(e, RemoteOp::Rename, Some(from.as_str())))
    }

    fn chmod(&mut self, path: &RemotePath, mode: u32) -> RemoteResult<()> {
        let stat = FileStat {
            size: None,
            uid: None,
            gid: None,
            // 권한 비트만 보낸다 — 나머지가 `None`이면 서버는 그 항목을 건드리지 않는다
            perm: Some(mode & 0o7777),
            atime: None,
            mtime: None,
        };
        self.sftp()?
            .setstat(Path::new(path.as_str()), stat)
            .map_err(|e| classify(e, RemoteOp::Chmod, Some(path.as_str())))
    }

    fn download(
        &mut self,
        path: &RemotePath,
        dest: &mut dyn Write,
        offset: u64,
        progress: &mut dyn Progress,
    ) -> RemoteResult<u64> {
        let name = path.as_str();
        let mut file = self
            .sftp()?
            .open(Path::new(name))
            .map_err(|e| classify(e, RemoteOp::Open, Some(name)))?;
        if offset > 0 {
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| RemoteError::Transfer {
                    detail: e.to_string(),
                    transferred: 0,
                })?;
        }
        match pump(&mut file, dest, progress) {
            Pumped::Done(total) => Ok(total),
            Pumped::Cancelled => Err(RemoteError::Cancelled),
            Pumped::Failed {
                transferred,
                detail,
            } => Err(RemoteError::Transfer {
                detail,
                transferred,
            }),
        }
    }

    fn upload(
        &mut self,
        path: &RemotePath,
        src: &mut dyn Read,
        offset: u64,
        progress: &mut dyn Progress,
    ) -> RemoteResult<u64> {
        let name = path.as_str();
        let sftp = self.sftp()?;
        let mut file = if offset > 0 {
            // 이어 올리기 — 자르지 않고 열어 그 지점부터 쓴다
            sftp.open_mode(
                Path::new(name),
                OpenFlags::WRITE | OpenFlags::CREATE,
                NEW_FILE_MODE,
                OpenType::File,
            )
            .map_err(|e| classify(e, RemoteOp::Resume, Some(name)))?
        } else {
            sftp.create(Path::new(name))
                .map_err(|e| classify(e, RemoteOp::Create, Some(name)))?
        };
        if offset > 0 {
            file.seek(SeekFrom::Start(offset))
                .map_err(|e| RemoteError::Transfer {
                    detail: e.to_string(),
                    transferred: 0,
                })?;
        }

        let outcome = pump(src, &mut file, progress);
        let total = match outcome {
            Pumped::Done(total) => total,
            Pumped::Cancelled => return Err(RemoteError::Cancelled),
            Pumped::Failed {
                transferred,
                detail,
            } => {
                return Err(RemoteError::Transfer {
                    detail,
                    transferred,
                });
            }
        };
        // 닫을 때 서버가 실패를 알려 주는 일이 있다 — 여기서 버리면 "성공했는데 파일이 없는" 일이 생긴다
        file.close()
            .map_err(|e| classify(e, RemoteOp::Close, Some(name)))?;
        Ok(total)
    }

    /// **`keepalive_send`를 쓰지 않는다** (완료 리뷰 2026-09-16).
    ///
    /// libssh2는 `keepalive_interval`이 0이면 **소켓을 건드리지 않고 0을 돌려준다**
    /// (`libssh2/src/keepalive.c`의 첫 가지). 이 앱은 `set_keepalive`를 부르지 않으므로
    /// 그 간격이 0이고, 그래서 그 호출은 **연결이 죽어도 언제나 성공한다** — 연결 생사를
    /// 확인하는 쪽(`remote::connection::note_if_lost`)이 그것을 믿으면 SFTP에서는 끊김이
    /// 영영 잡히지 않는다.
    ///
    /// 대신 `realpath(".")`로 **실제 왕복을 만든다** — `pwd`가 쓰는 것과 같은 요청이라
    /// 서버 지원을 새로 전제하지 않고, 죽은 소켓에서는 오류가 난다
    fn noop(&mut self) -> RemoteResult<()> {
        let sftp = self.sftp()?;
        guard_path_panic(crate::i18n::remote_subject_home(), || {
            sftp.realpath(Path::new("."))
                .map_err(|e| classify(e, RemoteOp::KeepAlive, None))?;
            Ok(())
        })
    }

    fn is_secure(&self) -> bool {
        // SSH는 전송 계층이 곧 암호화다 — 연결이 서 있으면 참이다
        self.session.is_some()
    }

    fn quit(&mut self) -> RemoteResult<()> {
        self.sftp = None;
        let Some(session) = self.session.take() else {
            return Ok(());
        };
        session
            .disconnect(None, "bye", None)
            .map_err(|e| classify(e, RemoteOp::Quit, None))
    }
}

/// 지문 판정에 따라 진행 여부를 정한다. **수락으로 표를 바꿨으면 `true`**를 돌려준다.
///
/// 파일 저장을 여기서 하지 않는 이유: 이 판정이 D15의 핵심이라 단위 테스트로 고정해야 하는데,
/// 저장까지 하면 테스트가 사용자 폴더를 건드린다.
fn resolve_host_key<'prompt>(
    known: &mut KnownHosts,
    host: &str,
    port: u16,
    check: &HostKeyCheck,
    // 수명을 열어 둔다 — 세션이 든 통로는 `'static`이지만 테스트는 지역 클로저를 넘긴다
    prompt: Option<&mut (dyn FnMut(&HostKeyCheck) -> HostKeyDecision + Send + 'prompt)>,
) -> RemoteResult<bool> {
    let (fingerprint, rejected_detail) = match check {
        HostKeyCheck::Match => return Ok(false),
        HostKeyCheck::Unknown { fingerprint } => (
            fingerprint.clone(),
            crate::i18n::remote_unknown_server().to_owned(),
        ),
        HostKeyCheck::Changed { old, new } => (
            new.clone(),
            crate::i18n::dynamic::hostkey_changed_reason(old.as_str(), new.as_str()),
        ),
    };

    let Some(prompt) = prompt else {
        // 물어볼 수단이 없으면 거절한다 — 조용히 수락하는 경로는 만들지 않는다 (D15)
        return Err(RemoteError::HostKey {
            detail: crate::i18n::dynamic::hostkey_unverifiable(&rejected_detail),
        });
    };
    match prompt(check) {
        HostKeyDecision::Accept => {
            known.accept(host, port, &fingerprint);
            Ok(true)
        }
        HostKeyDecision::Reject => Err(RemoteError::HostKey {
            detail: rejected_detail,
        }),
    }
}

/// 서버가 준 이름을 경로로 옮기는 라이브러리 호출을 감싼다.
///
/// `ssh2`는 Windows에서 경로 바이트를 `str::from_utf8(..).unwrap()`으로 옮기므로 **UTF-8이 아닌
/// 이름을 만나면 패닉한다** — 목록(`readdir`)·링크 대상(`readlink`)·홈 경로(`realpath`)가 모두
/// 그 변환을 지난다. 워커 스레드가 통째로 죽으면 그 연결이 사라지므로 오류로 바꿔 돌린다.
/// 서버 문자셋 지원 자체는 T16(D23) 몫이다.
fn guard_path_panic<T>(subject: &str, call: impl FnOnce() -> RemoteResult<T>) -> RemoteResult<T> {
    std::panic::catch_unwind(AssertUnwindSafe(call)).unwrap_or_else(|_| {
        Err(RemoteError::Protocol {
            detail: crate::i18n::dynamic::name_decode_failed(subject),
        })
    })
}

/// 목록 조회 본체 — `list`가 패닉 방어막을 두르고 부른다
fn read_directory(sftp: &Sftp, path: &RemotePath) -> RemoteResult<Vec<RemoteEntry>> {
    let items = sftp
        .readdir(Path::new(path.as_str()))
        .map_err(|e| classify(e, RemoteOp::List, Some(path.as_str())))?;
    // 라이브러리는 항목 경로를 `dirname`과 이어 붙여 주는데, Windows에서는 그 이음매가 `\`가 된다.
    // 우리에게 필요한 것은 마지막 이름뿐이므로 거기만 떼어 쓴다 (D9 — 원격 경로는 우리가 조립한다)
    let named: Vec<(String, FileStat)> = items
        .into_iter()
        .filter_map(|(joined, stat)| {
            joined
                .file_name()
                .map(|name| (name.to_string_lossy().into_owned(), stat))
        })
        .collect();

    let mut entries = entries_from_readdir(path, named);
    for entry in &mut entries {
        if !entry.is_symlink {
            continue;
        }
        // 링크가 가리키는 곳은 따로 물어봐야 안다. 못 얻으면 대상 없이 링크로만 보인다
        let target = sftp.readlink(Path::new(path.join(&entry.name).as_str()));
        entry.link_target = target.ok().map(|p| p.to_string_lossy().into_owned());
    }
    Ok(entries)
}

/// `readdir` 결과를 목록 항목으로 옮긴다.
///
/// 라이브러리는 `.`과 `..`를 **둘 다** 걸러 내므로 상위 이동 항목은 여기서 되돌려 놓는다 —
/// 로컬·FTP 목록과 같은 규칙이어야 화면이 프로토콜마다 달라지지 않는다.
fn entries_from_readdir(path: &RemotePath, items: Vec<(String, FileStat)>) -> Vec<RemoteEntry> {
    let mut entries = Vec::with_capacity(items.len() + 1);
    if !path.is_root() {
        entries.push(parent_entry());
    }
    for (name, stat) in items {
        // 라이브러리가 이미 걸러 내지만, 걸러 주지 않는 서버·판올림에 대비해 여기서도 막는다
        if name.is_empty() || name == "." || name == ".." {
            continue;
        }
        entries.push(entry_from_stat(name, &stat));
    }
    entries
}

/// 상위 폴더로 올라가는 항목
fn parent_entry() -> RemoteEntry {
    RemoteEntry {
        name: "..".to_owned(),
        is_dir: true,
        is_symlink: false,
        link_target: None,
        size: 0,
        modified: None,
        mode: None,
        owner: None,
    }
}

fn entry_from_stat(name: String, stat: &FileStat) -> RemoteEntry {
    let file_type = stat.file_type();
    RemoteEntry {
        name,
        is_dir: file_type.is_dir(),
        is_symlink: file_type.is_symlink(),
        // 대상은 `readlink`로 따로 물어 채운다
        link_target: None,
        size: stat.size.unwrap_or(0),
        modified: stat.mtime.map(|seconds| seconds as i64),
        // 위쪽 비트는 파일 종류라 권한 열에 보일 것이 아니다. 서버가 주지 않으면 빈칸이 된다
        mode: stat.perm.map(|perm| perm & 0o7777),
        owner: stat.uid.map(|uid| uid.to_string()),
    }
}

/// 이 사이트가 무엇으로 인증하는가 (FR-66).
///
/// `login`에서 떼어 둔 이유는 하나다 — **가짜 서버가 인증을 흉내 내지 않아**(`remote::testing`)
/// 실제 인증 경로는 기계로 잴 수 없고, 갈림 자체는 순수 판정이라 그것만이라도 시험으로 못박는다
#[derive(Debug, PartialEq)]
enum AuthKind<'a> {
    Password,
    KeyFile(&'a Path),
}

fn auth_kind(site: &SiteRecord) -> RemoteResult<AuthKind<'_>> {
    match site.logon {
        LogonType::KeyFile => match site.key_path.as_deref() {
            Some(path) => Ok(AuthKind::KeyFile(path)),
            // 키 파일로 접속하겠다고 해 놓고 경로가 없으면 물어볼 것이 없다 —
            // 서버를 두드려 실패 사유를 받는 것보다 이 사실을 그대로 알리는 편이 낫다
            None => Err(RemoteError::Auth {
                detail: crate::i18n::remote_key_path_missing().to_owned(),
            }),
        },
        LogonType::Anonymous | LogonType::Normal => Ok(AuthKind::Password),
    }
}

/// 로그인 실패를 사용자가 다음 행동을 정할 수 있는 문구로 바꾼다.
///
/// 비밀번호 인증이 꺼진 서버(키 인증만 받는 서버)에서 "비밀번호가 틀렸습니다"만 보이면
/// 사용자가 비밀번호만 계속 고쳐 보게 된다.
///
/// **그 안내는 비밀번호로 시도했을 때만 붙인다** — 키로 시도해 실패한 사람에게
/// 「이 서버는 비밀번호 인증을 받지 않는다」고 알리면 엉뚱한 곳을 고치게 된다
fn auth_error(session: &Session, user: &str, kind: &AuthKind<'_>, err: Ssh2Error) -> RemoteError {
    if !matches!(kind, AuthKind::Password) {
        return RemoteError::Auth {
            detail: err.to_string(),
        };
    }
    let methods = session.auth_methods(user).ok().map(|list| list.to_owned());
    let detail = match methods {
        Some(list) if !list.split(',').any(|method| method.trim() == "password") => {
            crate::i18n::dynamic::auth_no_password(&err.to_string(), &list)
        }
        _ => err.to_string(),
    };
    RemoteError::Auth { detail }
}

/// 라이브러리 오류를 도메인 오류로 옮긴다. 서버가 준 사유를 그대로 담는다
fn classify(err: Ssh2Error, operation: RemoteOp, path: Option<&str>) -> RemoteError {
    let detail = format!("{}: {err}", operation.label());
    let path_of = || path.unwrap_or_default().to_owned();
    match err.code() {
        ErrorCode::SFTP(FX_NO_SUCH_FILE) => RemoteError::NotFound {
            path: path_of(),
            detail,
        },
        ErrorCode::SFTP(FX_PERMISSION_DENIED) => RemoteError::PermissionDenied {
            path: path_of(),
            detail,
        },
        ErrorCode::SFTP(FX_OP_UNSUPPORTED) => RemoteError::Unsupported {
            operation: operation.label().to_owned(),
            detail,
        },
        _ => RemoteError::Protocol { detail },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::types::{Protocol, SiteId};

    const FINGERPRINT_A: &str = "SHA256:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU";
    const FINGERPRINT_B: &str = "SHA256:LPJNul+wow4m6DsqxbninhsWHlwfp0JecwQzYpOLmCQ";

    /// 연결 확인이 **실제 왕복을 만드는가** (완료 리뷰 2026-09-16).
    ///
    /// 소스를 훑어 재는 이유는 살아 있는 SSH 서버가 있어야 그 왕복을 태울 수 있기
    /// 때문이다 — `ftp.rs`의 같은 방식 시험과 짝이다. **`keepalive_send`로 돌아가면
    /// 이 시험이 잡는다**: libssh2는 `keepalive_interval`이 0일 때 소켓을 건드리지 않고
    /// 성공을 돌려주므로(`libssh2/src/keepalive.c`), 죽은 연결에서도 참이 된다
    #[test]
    fn 연결_확인은_서버에_실제로_묻는다() {
        let source = include_str!("sftp.rs");
        let 머리 = "fn noop(&mut self)";
        let start = source.find(머리).expect("noop을 찾지 못했다");
        let rest = &source[start + 머리.len()..];
        let end = rest.find("\n    fn ").unwrap_or(rest.len());
        let body = &rest[..end];

        assert!(
            body.contains("realpath"),
            "연결 확인이 서버에 묻지 않는다: {body}"
        );
        assert!(
            !body.contains("keepalive_send"),
            "keepalive_send는 간격이 0이면 소켓을 건드리지 않는다: {body}"
        );
    }

    /// 인증 갈림만 재는 시험용 사이트 — 가짜 서버가 인증을 흉내 내지 않아
    /// 실제 `userauth_*` 호출은 기계로 잴 수 없다(그쪽은 HUMAN-VERIFY다)
    fn key_site(logon: LogonType, key_path: Option<&str>) -> SiteRecord {
        let mut site = SiteRecord::new(SiteId(1), "키 서버".to_owned());
        site.protocol = Protocol::Sftp;
        site.user = "deploy".to_owned();
        site.logon = logon;
        site.key_path = key_path.map(std::path::PathBuf::from);
        site
    }

    #[test]
    fn 인증_갈래는_로그온_유형과_키_경로가_정한다() {
        // FR-66 — 갈래는 셋이다: 비밀번호 / 키 파일 / 키 파일인데 경로가 없어 물어볼 것이 없음
        let 비밀번호 = key_site(LogonType::Normal, None);
        assert_eq!(
            auth_kind(&비밀번호).expect("비밀번호 갈래"),
            AuthKind::Password
        );

        let 익명 = key_site(LogonType::Anonymous, None);
        assert_eq!(
            auth_kind(&익명).expect("익명도 비밀번호 갈래"),
            AuthKind::Password
        );

        let 키 = key_site(LogonType::KeyFile, Some(r"C:\keys\id_ed25519"));
        assert_eq!(
            auth_kind(&키).expect("키 갈래"),
            AuthKind::KeyFile(Path::new(r"C:\keys\id_ed25519"))
        );

        // 경로가 없으면 서버를 두드리지 않고 그 사실을 그대로 알린다
        let 경로없음 = key_site(LogonType::KeyFile, None);
        assert!(
            matches!(auth_kind(&경로없음), Err(RemoteError::Auth { .. })),
            "키 경로가 없으면 인증 오류여야 한다"
        );
    }

    #[test]
    fn 키_경로는_손대지_않고_그대로_넘긴다() {
        // 환경변수·상대 경로를 우리가 펼치면 사용자가 고른 것과 다른 파일을 열 수 있다
        let 상대 = key_site(LogonType::KeyFile, Some(r"..\keys\id_rsa"));
        assert_eq!(
            auth_kind(&상대).expect("키 갈래"),
            AuthKind::KeyFile(Path::new(r"..\keys\id_rsa"))
        );
    }

    fn stat(perm: Option<u32>, size: u64, mtime: Option<u64>, uid: Option<u32>) -> FileStat {
        FileStat {
            size: Some(size),
            uid,
            gid: None,
            perm,
            atime: None,
            mtime,
        }
    }

    /// 파일 종류 비트 — POSIX `S_IFDIR`·`S_IFLNK`·`S_IFREG`
    const S_IFDIR: u32 = 0o040_000;
    const S_IFLNK: u32 = 0o120_000;
    const S_IFREG: u32 = 0o100_000;

    #[test]
    fn 목록은_현재_디렉터리를_빼고_상위는_되돌려_놓는다() {
        let path = RemotePath::new("/var/www");
        let items = vec![
            (".".to_owned(), stat(Some(S_IFDIR | 0o755), 0, None, None)),
            ("..".to_owned(), stat(Some(S_IFDIR | 0o755), 0, None, None)),
            (
                "index.html".to_owned(),
                stat(Some(S_IFREG | 0o644), 120, Some(1_700_000_000), Some(1000)),
            ),
        ];
        let names: Vec<String> = entries_from_readdir(&path, items)
            .into_iter()
            .map(|entry| entry.name)
            .collect();
        assert_eq!(names, vec!["..", "index.html"], "`..`는 한 번만 남는다");
    }

    #[test]
    fn 루트에서는_상위_항목이_없다() {
        let entries = entries_from_readdir(
            &RemotePath::root(),
            vec![("etc".to_owned(), stat(Some(S_IFDIR | 0o755), 0, None, None))],
        );
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["etc"]);
    }

    #[test]
    fn 항목의_권한과_소유자와_크기를_옮긴다() {
        let items = vec![(
            "app.tar.gz".to_owned(),
            stat(
                Some(S_IFREG | 0o640),
                8_589_934_592,
                Some(1_700_000_000),
                Some(1000),
            ),
        )];
        let entries = entries_from_readdir(&RemotePath::root(), items);
        let entry = &entries[0];
        assert!(!entry.is_dir);
        // 4GB를 넘는 크기가 잘리지 않는다
        assert_eq!(entry.size, 8_589_934_592);
        assert_eq!(entry.modified, Some(1_700_000_000));
        // 파일 종류 비트는 권한 열에 섞이지 않는다
        assert_eq!(entry.permissions_string().as_deref(), Some("rw-r-----"));
        assert_eq!(entry.owner.as_deref(), Some("1000"));
    }

    #[test]
    fn 권한을_주지_않는_서버에서는_권한_열이_빈다() {
        let items = vec![("secret".to_owned(), stat(None, 0, None, None))];
        let entries = entries_from_readdir(&RemotePath::root(), items);
        assert_eq!(entries[0].mode, None);
        assert_eq!(entries[0].permissions_string(), None);
        assert_eq!(entries[0].owner, None);
        // 종류를 모르면 파일로 본다
        assert!(!entries[0].is_dir);
    }

    #[test]
    fn 심볼릭_링크는_링크로_표시된다() {
        let items = vec![(
            "current".to_owned(),
            stat(Some(S_IFLNK | 0o777), 0, None, None),
        )];
        let entries = entries_from_readdir(&RemotePath::root(), items);
        assert!(entries[0].is_symlink);
        // 대상은 `readlink`로 따로 물어 채운다 — 목록 변환 단계에서는 비어 있다
        assert_eq!(entries[0].link_target, None);
    }

    /// 지정한 결정만 돌려주고 호출 횟수를 세는 확인 통로
    fn prompt_with(
        decision: HostKeyDecision,
        calls: &mut usize,
    ) -> impl FnMut(&HostKeyCheck) -> HostKeyDecision + Send {
        move |_| {
            *calls += 1;
            decision
        }
    }

    #[test]
    fn 이미_아는_서버는_묻지_않고_지나간다() {
        let mut known = KnownHosts::empty();
        known.accept("example.test", 22, FINGERPRINT_A);
        let mut calls = 0;
        let mut prompt = prompt_with(HostKeyDecision::Reject, &mut calls);
        let check = known.check("example.test", 22, FINGERPRINT_A);

        let changed = resolve_host_key(&mut known, "example.test", 22, &check, Some(&mut prompt))
            .expect("일치하는 서버는 통과해야 한다");
        assert!(!changed, "표를 바꿀 일이 없다");
        drop(prompt);
        assert_eq!(calls, 0, "일치하면 사용자에게 묻지 않는다");
    }

    #[test]
    fn 처음_보는_서버는_수락해야_지나간다() {
        let mut known = KnownHosts::empty();
        let check = known.check("example.test", 22, FINGERPRINT_A);
        let mut calls = 0;
        let mut prompt = prompt_with(HostKeyDecision::Accept, &mut calls);

        let changed = resolve_host_key(&mut known, "example.test", 22, &check, Some(&mut prompt))
            .expect("수락했으므로 통과");
        assert!(changed);
        drop(prompt);
        assert_eq!(calls, 1);
        // 수락한 지문이 표에 남아 다음부터는 묻지 않는다
        assert_eq!(
            known.check("example.test", 22, FINGERPRINT_A),
            HostKeyCheck::Match
        );
    }

    #[test]
    fn 거절하면_연결이_진행되지_않고_표도_그대로다() {
        let mut known = KnownHosts::empty();
        let check = known.check("example.test", 22, FINGERPRINT_A);
        let mut calls = 0;
        let mut prompt = prompt_with(HostKeyDecision::Reject, &mut calls);

        let err = resolve_host_key(&mut known, "example.test", 22, &check, Some(&mut prompt))
            .expect_err("거절했으면 실패해야 한다");
        assert!(matches!(err, RemoteError::HostKey { .. }));
        drop(prompt);
        assert_eq!(calls, 1);
        assert!(matches!(
            known.check("example.test", 22, FINGERPRINT_A),
            HostKeyCheck::Unknown { .. }
        ));
    }

    #[test]
    fn 지문이_바뀐_서버는_수락_없이는_진행하지_않는다() {
        // D15의 핵심 — 자동 수락 경로가 없다
        let mut known = KnownHosts::empty();
        known.accept("example.test", 22, FINGERPRINT_A);
        let check = known.check("example.test", 22, FINGERPRINT_B);
        assert!(matches!(check, HostKeyCheck::Changed { .. }));

        // ① 확인 통로가 없으면 거절된다
        let err = resolve_host_key(&mut known, "example.test", 22, &check, None)
            .expect_err("물어볼 수단이 없으면 진행하면 안 된다");
        assert!(matches!(err, RemoteError::HostKey { .. }));
        assert_eq!(
            known.check("example.test", 22, FINGERPRINT_A),
            HostKeyCheck::Match,
            "거절했는데 표가 바뀌면 안 된다"
        );

        // ② 사용자가 거절하면 그대로 실패한다
        let mut calls = 0;
        let mut prompt = prompt_with(HostKeyDecision::Reject, &mut calls);
        assert!(
            resolve_host_key(&mut known, "example.test", 22, &check, Some(&mut prompt)).is_err()
        );
        drop(prompt);

        // ③ 수락해야만 새 지문으로 바뀐다
        let mut calls = 0;
        let mut prompt = prompt_with(HostKeyDecision::Accept, &mut calls);
        assert!(
            resolve_host_key(&mut known, "example.test", 22, &check, Some(&mut prompt)).is_ok()
        );
        drop(prompt);
        assert_eq!(
            known.check("example.test", 22, FINGERPRINT_B),
            HostKeyCheck::Match
        );
    }

    #[test]
    fn 지문_변경_안내에는_두_지문이_모두_들어간다() {
        let mut known = KnownHosts::empty();
        known.accept("example.test", 22, FINGERPRINT_A);
        let check = known.check("example.test", 22, FINGERPRINT_B);
        let err = resolve_host_key(&mut known, "example.test", 22, &check, None).expect_err("거절");
        let detail = err.detail();
        assert!(detail.contains(FINGERPRINT_A), "이전 지문이 없다: {detail}");
        assert!(detail.contains(FINGERPRINT_B), "이번 지문이 없다: {detail}");
    }

    #[test]
    fn sftp_상태_코드가_오류_갈래를_가른다() {
        // 작업 이름이 문구에 섞이므로 언어를 고정한다
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        let missing = classify(
            Ssh2Error::new(ErrorCode::SFTP(FX_NO_SUCH_FILE), "no such file"),
            RemoteOp::List,
            Some("/none"),
        );
        assert!(matches!(&missing, RemoteError::NotFound { path, .. } if path == "/none"));

        let denied = classify(
            Ssh2Error::new(ErrorCode::SFTP(FX_PERMISSION_DENIED), "permission denied"),
            RemoteOp::Remove,
            Some("/etc/passwd"),
        );
        assert!(matches!(&denied, RemoteError::PermissionDenied { .. }));

        let unsupported = classify(
            Ssh2Error::new(ErrorCode::SFTP(FX_OP_UNSUPPORTED), "op unsupported"),
            RemoteOp::Chmod,
            Some("/a"),
        );
        assert!(
            matches!(&unsupported, RemoteError::Unsupported { operation, .. } if operation == "권한 변경")
        );

        // 그 밖의 실패는 프로토콜 오류로 모으되 원문을 잃지 않는다
        let other = classify(
            Ssh2Error::new(ErrorCode::Session(-18), "authentication failed"),
            RemoteOp::Login,
            None,
        );
        assert!(matches!(&other, RemoteError::Protocol { .. }));
        assert!(other.detail().contains("authentication failed"));
    }

    #[test]
    fn 연결_전에는_명령이_조용히_실패하지_않는다() {
        let mut session = SftpSession::with_known_hosts(KnownHosts::empty(), None);
        assert!(matches!(
            session.pwd().expect_err("로그인 전 PWD가 성공했다"),
            RemoteError::Protocol { .. }
        ));
        // 연결한 적이 없으면 종료는 할 일이 없다
        assert!(session.quit().is_ok());
    }

    #[test]
    fn ftp_사이트는_sftp_세션으로_연결하지_않는다() {
        let mut record = SiteRecord::new(SiteId(1), "테스트".to_owned());
        record.protocol = Protocol::Ftp;
        record.host = "example.test".to_owned();
        let mut session = SftpSession::with_known_hosts(KnownHosts::empty(), None);
        assert!(matches!(
            session
                .connect(&record)
                .expect_err("FTP 사이트를 받아들였다"),
            RemoteError::Protocol { .. }
        ));
    }

    /// 실제 서버 왕복 — `FE_TEST_SFTP_URL`(`sftp://사용자:비밀번호@호스트:포트/경로`)이 있을 때만 돈다.
    /// 자격증명은 레포에 넣지 않으며 실패 메시지에도 담지 않는다 (D25·보안 규칙).
    #[test]
    fn 실서버_왕복은_환경변수가_있을_때만_돈다() {
        let Ok(url) = std::env::var("FE_TEST_SFTP_URL") else {
            println!("건너뜀 — FE_TEST_SFTP_URL이 설정되지 않았습니다 (실서버 테스트는 선택 사항)");
            return;
        };
        let Some((record, path, password)) = parse_test_url(&url) else {
            panic!("FE_TEST_SFTP_URL 형식이 sftp://사용자:비밀번호@호스트:포트/경로가 아닙니다");
        };

        // 실서버 테스트는 지문을 자동 수락한다 — 확인 화면이 없는 환경이기 때문이다
        let prompt: HostKeyPrompt = Box::new(|_| HostKeyDecision::Accept);
        let mut session = SftpSession::with_known_hosts(KnownHosts::empty(), Some(prompt));
        session.connect(&record).expect("연결");
        session.login(&record, &password).expect("로그인");
        let home = session.pwd().expect("홈");
        println!("서버 홈: {home}");
        let entries = session.list(&path).expect("목록");
        println!("항목 {}개", entries.len());
        session.quit().expect("종료");
    }

    /// 테스트용 URL 해석 — 본 구현의 URL 파서는 T13(`remote::url`)이 만든다
    fn parse_test_url(url: &str) -> Option<(SiteRecord, RemotePath, String)> {
        let rest = url.strip_prefix("sftp://")?;
        let (credentials, rest) = rest.split_once('@')?;
        let (user, password) = credentials.split_once(':')?;
        let (authority, path) = match rest.split_once('/') {
            Some((authority, path)) => (authority, format!("/{path}")),
            None => (rest, "/".to_owned()),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse().ok()?),
            None => (authority, Protocol::Sftp.default_port()),
        };

        let mut record = SiteRecord::new(SiteId(1), "실서버".to_owned());
        record.protocol = Protocol::Sftp;
        record.host = host.to_owned();
        record.port = port;
        record.user = user.to_owned();
        Some((record, RemotePath::new(&path), password.to_owned()))
    }
}
