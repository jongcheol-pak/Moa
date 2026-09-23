//! 원격 연결의 도메인 타입 — 프로토콜·사이트 설정·경로·항목·오류 (FR-27~FR-31).
//!
//! 이 모듈은 `serde` 외에 아무것도 참조하지 않는다. 프로토콜 구현(`remote::ftp`·`remote::sftp`)과
//! 화면(`ui`)이 이쪽을 참조하며 역방향은 없다 (AGENTS: 의존 단방향).
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 원격 프로토콜 (FR-30).
///
/// `Ftps`를 `Ftp`와 별개 항목으로 두는 이유: 디자인의 사이드바 배지·사이트 관리자 프로토콜
/// 드롭다운이 셋을 나란히 보이므로(README §1·§9) 화면이 곧 이 열거형이다. `Encryption`은
/// 그중 FTP 계열의 TLS 협상 방식을 더 좁히는 값이라 역할이 겹치지 않는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Protocol {
    Ftp,
    Ftps,
    Sftp,
}

impl Protocol {
    /// 탭 배지·사이드바에 그대로 쓰이는 표기 (README §4 배지 라벨)
    pub fn label(self) -> &'static str {
        match self {
            Protocol::Ftp => "ftp",
            Protocol::Ftps => "ftps",
            Protocol::Sftp => "sftp",
        }
    }

    /// 주소창 URL 스킴 (FR-34)
    pub fn scheme(self) -> &'static str {
        self.label()
    }

    /// 사용자가 포트를 지정하지 않았을 때의 기본값
    pub fn default_port(self) -> u16 {
        match self {
            // FTPS(명시적 TLS)는 평문 FTP와 같은 21번에서 AUTH TLS로 승격한다
            Protocol::Ftp | Protocol::Ftps => 21,
            Protocol::Sftp => 22,
        }
    }

    /// SSH 계열인가 — 암호화 설정·호스트 키 확인이 이 값으로 갈린다
    pub fn is_ssh(self) -> bool {
        matches!(self, Protocol::Sftp)
    }
}

/// FTP 계열의 TLS 협상 방식 (사이트 관리자 `암호화(E):` — README §9).
/// `Protocol::Sftp`에는 적용되지 않는다 (SSH가 전송 계층을 이미 암호화한다).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Encryption {
    /// 평문 — 서버가 TLS를 지원해도 쓰지 않는다
    Plain,
    /// 가능하면 명시적 TLS로 승격하고, 서버가 거부하면 평문으로 진행 (디자인 기본값)
    #[default]
    ExplicitIfAvailable,
    /// 명시적 TLS 필수 — 승격에 실패하면 연결을 포기한다
    ExplicitRequired,
    /// 묵시적 TLS — 연결 직후부터 TLS.
    ///
    /// 관례상 990번을 쓰지만 **기본 포트를 여기서 바꾸지 않는다** — `default_port`는
    /// 프로토콜만 보고 정하며(FR-27: FTP/FTPS 21, SFTP 22), 묵시적 TLS를 고른 사용자가
    /// 사이트 관리자의 `포트(P):`에 직접 적는다. 암호화 항목이 포트를 몰래 덮어쓰면
    /// 사용자가 적어 둔 값이 사라진다
    Implicit,
}

/// 로그온 유형 (사이트 관리자 `로그온 유형(L):`).
///
/// **에이전트(Pageant·OpenSSH) 인증은 계속 범위 밖이라** 세 가지다 (PRD Out of Scope).
/// 직렬화 표현이 이름 문자열이라 값을 더해도 기존 `settings.json`은 그대로 읽힌다
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LogonType {
    /// 익명 — 사용자 `anonymous`, 비밀번호는 비운다
    Anonymous,
    /// 일반 — 사용자·비밀번호를 직접 준다
    #[default]
    Normal,
    /// 키 파일 — 사용자와 **개인 키 파일**로 인증한다 (FR-66).
    ///
    /// **SFTP에서만 고를 수 있다**(D7) — FTP 계열에는 없는 개념이라 사이트 관리자가
    /// 그 선택지를 세우지 않는다. 세션 파일을 손으로 고쳐 FTP에 이 값이 들어와도
    /// 연결이 깨지지 않게, FTP 쪽은 비밀번호 인증과 같게 다룬다
    KeyFile,
}

/// FTP 데이터 연결 방식 (사이트 관리자 `전송 모드(T):` — FR-45)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TransferMode {
    /// 기본 — 수동형으로 시도하고 실패하면 능동형 (FileZilla의 `기본(E)`)
    #[default]
    Default,
    /// 능동형 — 서버가 클라이언트로 데이터 연결을 건다 (PORT)
    Active,
    /// 수동형 — 클라이언트가 서버로 데이터 연결을 건다 (PASV)
    Passive,
}

/// 원격 파일명 인코딩 (사이트 관리자 `문자셋` 탭 — FR-46).
///
/// 실제 인코딩·디코딩 함수는 `remote::charset`이 갖는다 — 이 타입은 `SiteRecord`가
/// 담아야 해서 도메인 타입 쪽에 둔다.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Charset {
    /// `UTF-8(U)` — 기본값
    #[default]
    Utf8,
    /// `문자셋 직접 설정(C)` + `인코딩(E):`에 적은 이름
    Named(String),
}

/// 사이트 식별자.
///
/// **이름으로 잡지 않는 이유**: 사이트 이름은 `이름 바꾸기(R)`로 바뀌는데, 그때 연결·전송 큐·
/// 원격 탭이 들고 있던 참조가 통째로 끊긴다 (워크스페이스가 `WorkspaceId`를 쓰는 것과 같은 이유).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SiteId(pub u32);

/// 사이트 관리자에 등록된 사이트 한 벌 (FR-27).
///
/// **비밀번호는 평문으로 담지 않는다** — `password_sealed`는 DPAPI로 봉인된 바이트이며
/// 연결 직전에만 `remote::secret::unseal`로 풀어 쓴다 (FR-28).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SiteRecord {
    pub id: SiteId,
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    /// FTP 계열에만 뜻이 있다 (`Protocol::Sftp`면 무시된다)
    pub encryption: Encryption,
    pub logon: LogonType,
    pub user: String,
    /// DPAPI로 봉인된 비밀번호. 비어 있으면 "저장된 비밀번호 없음"이다 (FR-28).
    /// **평문이 들어오면 안 된다** — 채우는 곳은 `remote::secret::seal`뿐이다
    #[serde(default)]
    pub password_sealed: Vec<u8>,
    /// 개인 키 파일 경로 (FR-66) — `LogonType::KeyFile`일 때만 뜻이 있다.
    ///
    /// **경로를 손대지 않고 그대로 든다** — 환경변수·상대 경로를 우리가 펼치면
    /// 사용자가 고른 것과 다른 파일을 열 수 있다
    #[serde(default)]
    pub key_path: Option<PathBuf>,
    /// DPAPI로 봉인된 **키 암호** (FR-66·FR-28). 비어 있으면 암호 없는 키다.
    /// `password_sealed`와 같은 규약 — 채우는 곳은 `remote::secret::seal`뿐이다
    #[serde(default)]
    pub key_passphrase_sealed: Vec<u8>,
    pub transfer_mode: TransferMode,
    /// `동시 연결 수 제한(L)`이 켜졌을 때의 상한 `M` (1~10). `None`이면 제한 없음이며
    /// 기본 정책(탐색 1 + 전송 2)을 쓴다 (FR-45·D4)
    pub connection_limit: Option<u8>,
    pub charset: Charset,
}

/// `최대 동시 연결 수(M)`가 가질 수 있는 범위 (사이트 관리자 스피너 — FR-45)
pub const CONNECTION_LIMIT_RANGE: std::ops::RangeInclusive<u8> = 1..=10;

impl SiteRecord {
    /// 새 사이트의 기본값 — 사이트 관리자가 빈 초안으로 열릴 때 쓴다
    pub fn new(id: SiteId, name: String) -> SiteRecord {
        let protocol = Protocol::Ftp;
        SiteRecord {
            id,
            name,
            protocol,
            host: String::new(),
            port: protocol.default_port(),
            encryption: Encryption::default(),
            logon: LogonType::default(),
            user: String::new(),
            password_sealed: Vec::new(),
            key_path: None,
            key_passphrase_sealed: Vec::new(),
            transfer_mode: TransferMode::default(),
            connection_limit: None,
            charset: Charset::default(),
        }
    }

    /// 로그인에 쓸 사용자 이름 — 익명이면 관례상 `anonymous`다
    pub fn effective_user(&self) -> &str {
        match self.logon {
            LogonType::Anonymous => "anonymous",
            // 키 파일도 사용자 이름은 직접 준다 — 갈리는 것은 무엇으로 증명하느냐뿐이다
            LogonType::Normal | LogonType::KeyFile => &self.user,
        }
    }

    /// 연결 대상 `호스트:포트` — 프로토콜 구현이 그대로 쓴다
    pub fn address(&self) -> String {
        // IPv6 리터럴은 대괄호로 감싸야 포트와 구분된다
        if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// 원격 경로 — **POSIX `/` 구분자 전용** (D9).
///
/// `PathBuf`를 쓰지 않는 이유: Windows의 `PathBuf`는 `/`를 `\`로 정규화하고 `\`를 구분자로
/// 해석하는데, 원격 파일명에는 `\`가 그대로 들어갈 수 있어 경로가 손상된다.
/// 로컬 쪽의 `\\?\` 접두 함정(`fs::enumerate::to_extended_pattern`)과 같은 계열의 문제다.
///
/// 생성 시 **중복 슬래시를 접고 후행 슬래시를 떼되**, 백슬래시는 건드리지 않는다.
/// 앞의 `/` 유무(절대/상대)는 보존한다.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RemotePath(String);

impl RemotePath {
    /// 정규화해 만든다. 빈 문자열은 루트(`/`)로 본다
    pub fn new(raw: &str) -> RemotePath {
        let absolute = raw.starts_with('/');
        let mut out = String::with_capacity(raw.len() + 1);
        if absolute {
            out.push('/');
        }
        let mut first = true;
        for segment in raw.split('/').filter(|s| !s.is_empty()) {
            if !first {
                out.push('/');
            }
            out.push_str(segment);
            first = false;
        }
        // 세그먼트가 하나도 없으면(빈 문자열·`/`·`///`) 루트다
        if first && !absolute {
            out.push('/');
        }
        RemotePath(out)
    }

    /// 루트 경로 `/`
    pub fn root() -> RemotePath {
        RemotePath("/".to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_root(&self) -> bool {
        self.0 == "/"
    }

    /// 마지막 세그먼트 — 루트면 `None`
    pub fn file_name(&self) -> Option<&str> {
        if self.is_root() {
            return None;
        }
        self.0.rsplit('/').next().filter(|s| !s.is_empty())
    }

    /// 한 단계 위 경로.
    ///
    /// 루트는 `None`이고, **상대 경로의 단일 세그먼트도 `None`**이다 — 그 위가 무엇인지
    /// 서버의 현재 위치를 알아야 정해지므로 여기서 추측하지 않는다
    pub fn parent(&self) -> Option<RemotePath> {
        if self.is_root() {
            return None;
        }
        match self.0.rfind('/') {
            // `/a` → 루트
            Some(0) => Some(RemotePath::root()),
            Some(cut) => Some(RemotePath(self.0[..cut].to_owned())),
            // 상대 경로의 단일 세그먼트 (`pub`)
            None => None,
        }
    }

    /// 하위 경로를 잇는다. `name`에 `/`가 섞여 있어도 정규화된다
    pub fn join(&self, name: &str) -> RemotePath {
        if self.is_root() {
            RemotePath::new(&format!("/{name}"))
        } else {
            RemotePath::new(&format!("{}/{}", self.0, name))
        }
    }

    /// 세그먼트 목록 — 주소창 표시·트리 확장에 쓴다
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/').filter(|s| !s.is_empty())
    }
}

impl std::fmt::Display for RemotePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Default for RemotePath {
    fn default() -> RemotePath {
        RemotePath::root()
    }
}

/// 원격 디렉터리의 항목 하나 (FR-31).
///
/// 로컬의 `fs::enumerate::FileEntry`를 재사용하지 않는 이유(D10): 그쪽 `name`은 Win32 정렬
/// API와 공용하려고 널 종단 UTF-16을 담고 `modified`는 FILETIME 원시값이다 — 원격에는 둘 다
/// 맞지 않는다. 대신 **정렬 규칙은 T7의 `ListRow` 트레이트로 공유**해 화면 동작이 갈리지 않게 한다.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_symlink: bool,
    /// 심볼릭 링크가 가리키는 대상 — 이름 뒤에 `→ 대상`으로 붙는다 (README §3)
    pub link_target: Option<String>,
    pub size: u64,
    /// Unix epoch 초(UTC). 서버가 시각을 주지 않으면 `None`
    pub modified: Option<i64>,
    /// POSIX 권한 비트(`0o755` 등). 서버가 주지 않으면 `None` — 권한 열이 빈칸이 된다
    pub mode: Option<u32>,
    /// 소유자 표시값. 이름을 못 얻는 서버에서는 uid를 문자열로 담는다
    pub owner: Option<String>,
}

impl RemoteEntry {
    /// 소문자 확장자 (`""` = 없음/폴더) — 형식 아이콘·종류 열 조회에 쓴다 (D11).
    /// 로컬 `FileEntry::extension`과 같은 규칙이다(앞이 빈 `.gitignore`는 확장자 없음)
    pub fn extension(&self) -> String {
        if self.is_dir {
            return String::new();
        }
        match self.name.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
            _ => String::new(),
        }
    }

    /// `rwxr-xr-x` 형태의 권한 열 표시값 — 비트가 없으면 `None`
    pub fn permissions_string(&self) -> Option<String> {
        let mode = self.mode?;
        let mut out = String::with_capacity(9);
        // 소유자·그룹·기타 순으로 세 묶음, 각 묶음이 rwx
        for shift in [6, 3, 0] {
            let bits = (mode >> shift) & 0b111;
            out.push(if bits & 0b100 != 0 { 'r' } else { '-' });
            out.push(if bits & 0b010 != 0 { 'w' } else { '-' });
            out.push(if bits & 0b001 != 0 { 'x' } else { '-' });
        }
        Some(out)
    }
}

/// 원격 작업 실패 사유 (FR-30).
///
/// **어느 갈래든 서버가 준 원문을 잃지 않는다** — 실패 화면이 그 문구를 그대로 보이고
/// (README §5), 서버마다 다른 사유를 우리가 임의로 요약하면 사용자가 원인을 짚을 수 없다.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteError {
    /// 연결 자체가 안 됨 (DNS·거부·타임아웃·TLS 협상 실패)
    Connect { detail: String },
    /// 인증 실패 (FTP 530 등)
    Auth { detail: String },
    /// 호스트 키 문제 — 미등록이거나 지문이 바뀌었다 (D15, T3에서 채운다)
    HostKey { detail: String },
    /// 경로 없음
    NotFound { path: String, detail: String },
    /// 권한 거부 (FTP 550 등)
    PermissionDenied { path: String, detail: String },
    /// 전송 중 실패. `transferred`는 그때까지 옮긴 바이트로, **이어받기의 시작점**이 된다
    Transfer { detail: String, transferred: u64 },
    /// 서버가 그 기능을 지원하지 않음 (SITE CHMOD 미지원 등 — D22)
    Unsupported { operation: String, detail: String },
    /// 그 밖의 프로토콜 오류
    Protocol { detail: String },
    /// 사용자가 취소함 — 실패로 세지 않는다
    Cancelled,
    /// 여러 항목을 지우다 일부를 지우지 못함 — 폴더 재귀 삭제의 부분 실패.
    /// `detail`은 첫 실패의 문장이고 나머지 사유는 항목마다 서버 로그에 남는다
    Incomplete { failed: usize, detail: String },
}

/// 실패 화면이 사유 뒤에 덧붙일 안내를 고르는 기준 (FR-32).
///
/// 서버가 준 원문만으로는 무엇을 고쳐야 하는지 알기 어려워 한 줄을 덧붙이는데, **종류를
/// 가리지 않고 같은 문장을 붙이면 엉뚱한 원인을 지목한다** — 비밀번호가 틀린 사람에게
/// 암호화 설정을 의심하게 만들면 맞는 설정을 계속 바꿔 보게 된다.
///
/// `RemoteError`를 그대로 화면에 넘기지 않는 이유: 화면이 알아야 하는 것은 "어느 갈래인가"
/// 하나뿐이라, 오류 원문까지 들고 다니면 탭 상태가 원격 계층의 값을 통째로 품게 된다
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FailureKind {
    /// 연결 자체가 안 됨 — 주소·포트·암호화 설정을 의심할 자리다
    Connect,
    /// 서버가 로그인을 받지 않음
    Auth,
    /// 서버 지문 문제
    HostKey,
    /// **서 있던 연결이 도중에 끊겼다** — 유휴 타임아웃·네트워크 끊김 (사용자 보고 2026-09-16).
    ///
    /// `Connect`와 갈라 두는 이유: 그쪽 안내("암호화 설정이 다를 수도 있습니다")는 방금까지
    /// 잘 쓰던 설정을 의심하게 만든다. 여기서 알려야 하는 것은 「끊겼으니 다시 붙어라」다
    LinkLost,
    /// 그 밖 — 덧붙일 안내가 없다. 짐작으로 원인을 대느니 사유만 보인다
    #[default]
    Other,
}

impl RemoteError {
    /// 이 실패가 어느 갈래인가 — 실패 화면의 안내가 이것으로 갈린다
    pub fn failure_kind(&self) -> FailureKind {
        match self {
            RemoteError::Connect { .. } => FailureKind::Connect,
            RemoteError::Auth { .. } => FailureKind::Auth,
            RemoteError::HostKey { .. } => FailureKind::HostKey,
            RemoteError::NotFound { .. }
            | RemoteError::PermissionDenied { .. }
            | RemoteError::Transfer { .. }
            | RemoteError::Unsupported { .. }
            | RemoteError::Protocol { .. }
            | RemoteError::Cancelled
            | RemoteError::Incomplete { .. } => FailureKind::Other,
        }
    }

    /// 실패 화면·상태 줄에 그대로 보일 서버 원문. 취소는 원문이 없다
    pub fn detail(&self) -> &str {
        match self {
            RemoteError::Connect { detail }
            | RemoteError::Auth { detail }
            | RemoteError::HostKey { detail }
            | RemoteError::NotFound { detail, .. }
            | RemoteError::PermissionDenied { detail, .. }
            | RemoteError::Transfer { detail, .. }
            | RemoteError::Unsupported { detail, .. }
            | RemoteError::Protocol { detail }
            | RemoteError::Incomplete { detail, .. } => detail,
            RemoteError::Cancelled => "",
        }
    }

    /// 이어받기 시작점 — 전송 실패가 아니면 0
    pub fn transferred(&self) -> u64 {
        match self {
            RemoteError::Transfer { transferred, .. } => *transferred,
            _ => 0,
        }
    }
}

/// 오류 문구 앞에 붙는 작업 이름 — **언어 중립**이다.
///
/// 문자열 조각을 그대로 넘기면 그 자리에서 언어가 정해져 버린다. 무엇을 하다 실패했는지만
/// 이 열거형이 나르고, 사람이 읽을 말은 문구를 만드는 순간 카탈로그가 정한다 (D2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteOp {
    SessionSetup,
    SshHandshake,
    SftpStart,
    Home,
    Move,
    List,
    Mkdir,
    Remove,
    Rmdir,
    Rename,
    Chmod,
    Open,
    Resume,
    Create,
    Close,
    KeepAlive,
    Quit,
    Connect,
    ConnectImplicit,
    TlsUpgrade,
    Login,
    /// FTP 프로토콜 명령어(`LIST`·`CWD`…) — **번역 대상이 아니다**.
    /// 서버에 그대로 보낸 말이라 사용자가 서버 관리자에게 전할 때도 원문이 쓸모 있다
    Raw(&'static str),
}

impl RemoteOp {
    /// 지금 언어로 쓴 작업 이름
    pub fn label(self) -> &'static str {
        use crate::i18n;
        match self {
            RemoteOp::SessionSetup => i18n::op_session_setup(),
            RemoteOp::SshHandshake => i18n::op_ssh_handshake(),
            RemoteOp::SftpStart => i18n::op_sftp_start(),
            RemoteOp::Home => i18n::op_home(),
            RemoteOp::Move => i18n::op_move(),
            RemoteOp::List => i18n::op_list(),
            RemoteOp::Mkdir => i18n::op_mkdir(),
            RemoteOp::Remove => i18n::op_remove(),
            RemoteOp::Rmdir => i18n::op_rmdir(),
            RemoteOp::Rename => i18n::op_rename_op(),
            RemoteOp::Chmod => i18n::op_chmod(),
            RemoteOp::Open => i18n::op_open(),
            RemoteOp::Resume => i18n::op_resume(),
            RemoteOp::Create => i18n::op_create(),
            RemoteOp::Close => i18n::op_close(),
            RemoteOp::KeepAlive => i18n::op_keepalive(),
            RemoteOp::Quit => i18n::op_quit(),
            RemoteOp::Connect => i18n::op_connect(),
            RemoteOp::ConnectImplicit => i18n::op_connect_implicit(),
            RemoteOp::TlsUpgrade => i18n::op_tls_upgrade(),
            RemoteOp::Login => i18n::op_login(),
            RemoteOp::Raw(name) => name,
        }
    }
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use crate::i18n::dynamic as t;
        // 문장은 언어별로 통째로 만든다 — 조사(`을(를)`)가 영어에 없어 틀 자체가 갈린다 (D2)
        match self {
            RemoteError::Connect { detail } => f.write_str(&t::err_connect(detail)),
            RemoteError::Auth { detail } => f.write_str(&t::err_login(detail)),
            RemoteError::HostKey { detail } => f.write_str(&t::err_host_key(detail)),
            RemoteError::NotFound { path, detail } => f.write_str(&t::err_not_found(path, detail)),
            RemoteError::PermissionDenied { path, detail } => {
                f.write_str(&t::err_permission(path, detail))
            }
            RemoteError::Transfer {
                detail,
                transferred,
            } => f.write_str(&t::err_interrupted(*transferred, detail)),
            RemoteError::Unsupported { operation, detail } => {
                f.write_str(&t::err_unsupported(operation, detail))
            }
            RemoteError::Protocol { detail } => f.write_str(&t::err_protocol(detail)),
            RemoteError::Cancelled => f.write_str(crate::i18n::remote_cancelled()),
            RemoteError::Incomplete { failed, detail } => {
                f.write_str(&t::err_incomplete(*failed, detail))
            }
        }
    }
}

/// 원격 작업 결과
pub type RemoteResult<T> = Result<T, RemoteError>;

/// 전송 진행 통지 — 워커가 64KB마다 부른다 (D12·NFR-12).
///
/// **`false`를 돌려주면 전송을 그 자리에서 멈춘다** — 취소를 별도 채널로 폴링하지 않고
/// 이 반환값 하나로 처리해, 프로토콜 구현이 취소 방식을 몰라도 되게 한다
pub trait Progress {
    /// `transferred`는 이번 전송에서 옮긴 누적 바이트다
    fn report(&mut self, transferred: u64) -> bool;
}

/// 진행을 보고받지 않고 취소도 하지 않는 통지 — 목록 조회처럼 진행률이 없는 곳에서 쓴다
pub struct NoProgress;

impl Progress for NoProgress {
    fn report(&mut self, _transferred: u64) -> bool {
        true
    }
}

/// 프로토콜 한 벌이 제공해야 하는 동작 (FR-30·FR-31·FR-37·FR-39).
///
/// 구현은 `remote::ftp::FtpSession`(FTP·FTPS)과 `remote::sftp::SftpSession`(SFTP) 둘뿐이며,
/// **연결 워커 스레드 안에서만** 쓰인다 — 그래서 `Send`만 요구하고 `Sync`는 요구하지 않는다.
///
/// 프로토콜별 설정 차이(FTP의 능동/수동, SFTP의 호스트 키)는 이 트레이트에 올리지 않는다 —
/// 각 구현이 `SiteRecord`에서 필요한 것만 읽는다 (T1 Design 비추상화 선언).
pub trait RemoteSession: Send {
    /// 소켓을 열고 프로토콜 협상까지 한다 (TLS 승격·SSH 핸드셰이크 포함)
    fn connect(&mut self, site: &SiteRecord) -> RemoteResult<()>;

    /// 지금 이 연결이 **실제로** 암호화돼 있는가.
    ///
    /// 설정값(`Encryption`)이 아니라 **협상 결과**다 — `ExplicitIfAvailable`은 서버가 거부하면
    /// 평문으로 되연결하므로, 설정만 보고 `· TLS`를 적으면 평문 연결을 암호화됐다고 알리게 된다
    /// (F-7 리뷰 B1). 연결 전이면 거짓이다
    fn is_secure(&self) -> bool {
        false
    }

    /// 이 세션을 계속 써도 되는가 — 참이면 **연결을 다시 세워야 한다**.
    ///
    /// FTP는 명령과 응답이 제어 채널 하나를 차례로 오가는데, 전송을 중간에 끊으면 서버가
    /// 주는 응답 수가 구현마다 갈려 **읽지 않은 응답이 남을 수 있다**. 그러면 그 뒤의 모든
    /// 명령이 한 칸씩 밀린 응답을 읽어 연결 하나가 통째로 못 쓰게 된다. 우리가 그 남은 것을
    /// 비울 방법이 없으므로(라이브러리가 응답 읽기를 노출하지 않는다) 그 연결을 버린다.
    ///
    /// **기본은 거짓이다** — SFTP처럼 전송이 채널 단위라 이런 어긋남이 없는 프로토콜은
    /// 이것을 구현하지 않는다. 그래야 워커에 프로토콜별 분기가 생기지 않는다
    fn needs_reset(&self) -> bool {
        false
    }

    /// 인증한다. 둘째 인자는 **연결 직전에 봉인을 푼 인증 비밀**(비밀번호 또는 키 암호)이며
    /// 어디에도 보관하지 않는다. 어느 쪽인지는 `site.logon`이 말해 준다 (FR-66)
    fn login(&mut self, site: &SiteRecord, password: &str) -> RemoteResult<()>;

    /// 로그인 직후의 현재 위치 — 서버가 정하는 홈 디렉터리다
    fn pwd(&mut self) -> RemoteResult<RemotePath>;

    fn list(&mut self, path: &RemotePath) -> RemoteResult<Vec<RemoteEntry>>;

    fn cwd(&mut self, path: &RemotePath) -> RemoteResult<()>;

    fn mkdir(&mut self, path: &RemotePath) -> RemoteResult<()>;

    /// 파일 삭제
    fn remove(&mut self, path: &RemotePath) -> RemoteResult<()>;

    /// 빈 디렉터리 삭제 (재귀 삭제는 호출부가 항목을 훑어 조립한다 — T23)
    fn rmdir(&mut self, path: &RemotePath) -> RemoteResult<()>;

    fn rename(&mut self, from: &RemotePath, to: &RemotePath) -> RemoteResult<()>;

    /// POSIX 권한 변경. 지원하지 않는 서버는 `RemoteError::Unsupported`를 돌려준다 (D22)
    fn chmod(&mut self, path: &RemotePath, mode: u32) -> RemoteResult<()>;

    /// 원격 → 로컬. `offset`이 0보다 크면 그 지점부터 이어받는다.
    /// 반환값은 **이번 호출에서 옮긴 바이트**다 (이어받기 시작점은 포함하지 않는다)
    fn download(
        &mut self,
        path: &RemotePath,
        dest: &mut dyn std::io::Write,
        offset: u64,
        progress: &mut dyn Progress,
    ) -> RemoteResult<u64>;

    /// 로컬 → 원격. `offset`이 0보다 크면 그 지점에 이어 붙인다
    fn upload(
        &mut self,
        path: &RemotePath,
        src: &mut dyn std::io::Read,
        offset: u64,
        progress: &mut dyn Progress,
    ) -> RemoteResult<u64>;

    /// 연결 유지 확인 (유휴 타임아웃 방지)
    fn noop(&mut self) -> RemoteResult<()>;

    /// 정상 종료 인사. 실패해도 소켓은 어차피 닫히므로 호출부가 무시해도 된다
    fn quit(&mut self) -> RemoteResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 키_필드를_모르는_옛_설정도_그대로_읽힌다() {
        // FR-66 — 두 필드에 `#[serde(default)]`가 붙어 세션 스키마를 올리지 않아도 된다.
        // 이 시험이 깨지면 사용자의 기존 `settings.json`이 통째로 안 읽힌다
        let old = r#"{
            "id": 1,
            "name": "옛 사이트",
            "protocol": "Ftp",
            "host": "old.test",
            "port": 21,
            "encryption": "Plain",
            "logon": "Normal",
            "user": "deploy",
            "password_sealed": [1, 2, 3],
            "transfer_mode": "Default",
            "connection_limit": null,
            "charset": "Utf8"
        }"#;
        let record: SiteRecord = serde_json::from_str(old).expect("옛 설정을 읽지 못했다");
        assert_eq!(record.key_path, None);
        assert!(record.key_passphrase_sealed.is_empty());
        // 기존 값도 그대로다
        assert_eq!(record.logon, LogonType::Normal);
        assert_eq!(record.password_sealed, vec![1, 2, 3]);
    }

    #[test]
    fn 키_파일_사이트는_저장하고_읽어도_값이_그대로다() {
        let mut site = SiteRecord::new(SiteId(3), "키 서버".to_owned());
        site.protocol = Protocol::Sftp;
        site.logon = LogonType::KeyFile;
        site.user = "deploy".to_owned();
        site.key_path = Some(PathBuf::from(r"C:\keys\id_ed25519"));
        site.key_passphrase_sealed = vec![9, 8, 7];

        let json = serde_json::to_string(&site).expect("직렬화");
        let back: SiteRecord = serde_json::from_str(&json).expect("역직렬화");
        assert_eq!(back, site);
        // 사용자 이름은 익명과 달리 직접 준 값을 쓴다
        assert_eq!(back.effective_user(), "deploy");
    }

    #[test]
    fn 경로는_중복_슬래시와_후행_슬래시를_정리한다() {
        assert_eq!(RemotePath::new("/a//b/").as_str(), "/a/b");
        assert_eq!(
            RemotePath::new("//var///www//html").as_str(),
            "/var/www/html"
        );
        assert_eq!(RemotePath::new("/pub/").as_str(), "/pub");
    }

    #[test]
    fn 빈_경로와_슬래시만_있는_경로는_루트다() {
        assert!(RemotePath::new("").is_root());
        assert!(RemotePath::new("/").is_root());
        assert!(RemotePath::new("///").is_root());
        assert_eq!(RemotePath::new("").as_str(), "/");
    }

    #[test]
    fn 상대_경로의_앞_슬래시_유무는_보존된다() {
        // 서버가 상대 경로를 돌려줄 수 있어 절대로 승격하지 않는다
        assert_eq!(RemotePath::new("pub/data").as_str(), "pub/data");
        assert_eq!(RemotePath::new("/pub/data").as_str(), "/pub/data");
    }

    #[test]
    fn 백슬래시는_구분자가_아니라_파일명_문자다() {
        // POSIX 서버의 파일명에는 `\`가 들어갈 수 있다 — 로컬 경로처럼 정규화하면 손상된다 (D9)
        let path = RemotePath::new(r"/data/a\b.txt");
        assert_eq!(path.as_str(), r"/data/a\b.txt");
        assert_eq!(path.file_name(), Some(r"a\b.txt"));
        assert_eq!(
            path.parent().map(|p| p.as_str().to_owned()),
            Some("/data".to_owned())
        );
    }

    #[test]
    fn 상위_경로는_한_단계씩_올라가고_루트에서_멈춘다() {
        let deep = RemotePath::new("/var/www/html");
        let up1 = deep.parent().expect("한 단계 위가 있어야 한다");
        assert_eq!(up1.as_str(), "/var/www");
        let up2 = up1.parent().expect("두 단계 위가 있어야 한다");
        assert_eq!(up2.as_str(), "/var");
        let up3 = up2.parent().expect("루트가 나와야 한다");
        assert!(up3.is_root());
        assert_eq!(up3.parent(), None, "루트 위로는 올라가지 않는다");
    }

    #[test]
    fn 상대_경로의_단일_세그먼트는_상위를_추측하지_않는다() {
        assert_eq!(RemotePath::new("pub").parent(), None);
    }

    #[test]
    fn 파일명은_마지막_세그먼트고_루트는_없다() {
        assert_eq!(
            RemotePath::new("/var/www/index.html").file_name(),
            Some("index.html")
        );
        assert_eq!(RemotePath::new("pub").file_name(), Some("pub"));
        assert_eq!(RemotePath::root().file_name(), None);
    }

    #[test]
    fn 잇기는_루트에서도_슬래시를_겹치지_않는다() {
        assert_eq!(RemotePath::root().join("var").as_str(), "/var");
        assert_eq!(RemotePath::new("/var").join("www").as_str(), "/var/www");
        // 이름에 구분자가 섞여도 정규화된다
        assert_eq!(
            RemotePath::new("/var").join("/www/html").as_str(),
            "/var/www/html"
        );
    }

    #[test]
    fn 이름에_개행이나_제어문자가_있어도_경로는_유지된다() {
        // 서버가 이상한 이름을 줘도 우리가 잘라내지 않는다 — 조작은 되어야 한다
        let path = RemotePath::root().join("이상한\t이름\u{7f}.txt");
        assert_eq!(path.file_name(), Some("이상한\t이름\u{7f}.txt"));
    }

    #[test]
    fn 세그먼트는_빈_칸을_건너뛴다() {
        let path = RemotePath::new("//var//www/");
        let parts: Vec<&str> = path.segments().collect();
        assert_eq!(parts, vec!["var", "www"]);
        assert_eq!(RemotePath::root().segments().count(), 0);
    }

    fn sample_site() -> SiteRecord {
        SiteRecord {
            id: SiteId(7),
            name: "배포 서버".into(),
            protocol: Protocol::Sftp,
            host: "example.test".into(),
            port: 2222,
            encryption: Encryption::ExplicitRequired,
            logon: LogonType::Normal,
            user: "deploy".into(),
            password_sealed: vec![1, 2, 3, 250],
            key_path: None,
            key_passphrase_sealed: Vec::new(),
            transfer_mode: TransferMode::Passive,
            connection_limit: Some(3),
            charset: Charset::Named("CP949".into()),
        }
    }

    #[test]
    fn 사이트_설정은_왕복해도_같다() {
        let site = sample_site();
        let json = serde_json::to_string(&site).expect("직렬화");
        let back: SiteRecord = serde_json::from_str(&json).expect("역직렬화");
        assert_eq!(back, site);
    }

    #[test]
    fn 봉인_비밀번호_필드가_없는_옛_기록도_읽힌다() {
        // 세션 스키마가 늘 때 필드 하나 때문에 통째로 폴백되면 안 된다 (T25와 같은 규칙)
        let site = sample_site();
        let json = serde_json::to_string(&site).expect("직렬화");
        let without = json.replace(",\"password_sealed\":[1,2,3,250]", "");
        assert!(
            !without.contains("password_sealed"),
            "테스트가 필드를 못 걷어냈다"
        );
        let back: SiteRecord = serde_json::from_str(&without).expect("옛 기록이 거부됐다");
        assert!(back.password_sealed.is_empty());
        assert_eq!(back.user, site.user);
    }

    #[test]
    fn 새_사이트는_프로토콜_기본_포트로_시작한다() {
        let site = SiteRecord::new(SiteId(1), "새 사이트".into());
        assert_eq!(site.protocol, Protocol::Ftp);
        assert_eq!(site.port, 21);
        assert_eq!(Protocol::Sftp.default_port(), 22);
        assert_eq!(Protocol::Ftps.default_port(), 21);
    }

    #[test]
    fn 익명_로그온은_사용자를_anonymous로_본다() {
        let mut site = SiteRecord::new(SiteId(1), "공개".into());
        site.user = "무시됨".into();
        site.logon = LogonType::Anonymous;
        assert_eq!(site.effective_user(), "anonymous");
        site.logon = LogonType::Normal;
        assert_eq!(site.effective_user(), "무시됨");
    }

    #[test]
    fn ipv6_호스트는_대괄호로_감싼다() {
        let mut site = SiteRecord::new(SiteId(1), "v6".into());
        site.host = "2001:db8::1".into();
        site.port = 21;
        assert_eq!(site.address(), "[2001:db8::1]:21");
        // 이미 감싼 값은 다시 감싸지 않는다
        site.host = "[2001:db8::1]".into();
        assert_eq!(site.address(), "[2001:db8::1]:21");
        site.host = "example.test".into();
        assert_eq!(site.address(), "example.test:21");
    }

    fn entry(name: &str, is_dir: bool) -> RemoteEntry {
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

    #[test]
    fn 확장자_추출은_로컬과_같은_규칙이다() {
        assert_eq!(entry("A.TXT", false).extension(), "txt");
        assert_eq!(entry("no_ext", false).extension(), "");
        assert_eq!(entry(".gitignore", false).extension(), "");
        assert_eq!(entry("dir.name", true).extension(), "");
    }

    #[test]
    fn 권한_문자열은_비트를_rwx로_옮긴다() {
        let mut e = entry("a", false);
        e.mode = Some(0o755);
        assert_eq!(e.permissions_string().as_deref(), Some("rwxr-xr-x"));
        e.mode = Some(0o640);
        assert_eq!(e.permissions_string().as_deref(), Some("rw-r-----"));
        e.mode = Some(0);
        assert_eq!(e.permissions_string().as_deref(), Some("---------"));
        // 서버가 권한을 안 주면 열이 빈칸이 된다
        e.mode = None;
        assert_eq!(e.permissions_string(), None);
    }

    #[test]
    fn 항목은_4gb를_넘는_크기도_담는다() {
        let mut e = entry("big.bin", false);
        e.size = 8 * 1024 * 1024 * 1024;
        assert_eq!(e.size, 8_589_934_592);
    }

    #[test]
    fn 오류는_서버_원문을_잃지_않는다() {
        // 실패 화면이 이 문구를 그대로 보인다 (README §5) — 요약하면 원인을 짚을 수 없다
        let raw = "530 Login incorrect";
        let err = RemoteError::Auth {
            detail: raw.to_owned(),
        };
        assert_eq!(err.detail(), raw);
        assert!(err.to_string().contains(raw), "Display가 원문을 잃었다");

        let err = RemoteError::PermissionDenied {
            path: "/var/www/sw.js".to_owned(),
            detail: "550 Permission denied".to_owned(),
        };
        assert!(err.to_string().contains("550 Permission denied"));
        assert!(err.to_string().contains("/var/www/sw.js"));
    }

    #[test]
    fn 전송_오류는_이어받기_시작점을_담는다() {
        let err = RemoteError::Transfer {
            detail: "connection reset".to_owned(),
            transferred: 1_048_576,
        };
        assert_eq!(err.transferred(), 1_048_576);
        assert!(err.to_string().contains("1048576"));
        // 다른 갈래는 0이다
        assert_eq!(RemoteError::Cancelled.transferred(), 0);
        assert_eq!(RemoteError::Cancelled.detail(), "");
    }

    #[test]
    fn 진행_통지의_기본_구현은_취소하지_않는다() {
        let mut sink = NoProgress;
        assert!(sink.report(0));
        assert!(sink.report(u64::MAX));
    }
}
