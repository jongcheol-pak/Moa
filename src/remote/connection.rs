//! 연결 하나 = 워커 스레드 하나 (NFR-10·NFR-11).
//!
//! UI 스레드는 **명령을 채널로 보내고 이벤트를 채널로 받을 뿐** 원격 I/O를 직접 하지 않는다.
//! 그래서 서버가 느리거나 아예 응답하지 않아도 창이 멈추지 않고, 한 연결이 막혀도 다른 연결과
//! 로컬 탐색은 그대로 돈다 — 연결마다 스레드·채널·상태가 따로이기 때문이다.
//!
//! `remote`는 `ui`(egui)를 모르므로 화면을 직접 깨우지 못한다. 대신 **깨우기 콜백**(`Wake`)을
//! 주입받아 이벤트를 보낸 뒤 부른다 (D6 — 썸네일 워커가 겪은 "화면이 50ms마다 스스로 확인"을
//! 되풀이하지 않기 위함).
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::remote::log::{LogBuffer, LogKind, mask_secrets};
use crate::remote::types::{
    FailureKind, Progress, RemoteEntry, RemoteError, RemotePath, RemoteResult, RemoteSession,
    SiteId, SiteRecord,
};

/// 화면을 다시 그리게 하는 콜백 (D6). `remote`가 egui를 모른 채 화면을 깨우는 유일한 통로다
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// 연결 식별자 — 같은 사이트에 두 번 연결하면 서로 다른 값을 받는다
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConnectionId(pub u32);

/// 전송 하나의 식별자 — 큐(T17)가 발급한다
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransferId(pub u64);

/// 취소 대상이 없다 — 아무 전송도 멈추라고 이르지 않은 상태
const CANCEL_NONE: u64 = u64::MAX;

/// 어느 전송이든 멈춘다 — 연결을 접을 때만 쓴다(`Connection`의 `Drop`)
const CANCEL_ALL: u64 = u64::MAX - 1;

/// 진행률 이벤트를 보내는 최소 간격.
///
/// 64KB마다(= 초당 수백 번) 이벤트를 보내면 채널과 화면이 그 갱신에 잠식된다 —
/// 사람이 알아볼 수 있는 간격으로 묶어 보낸다. 전송 자체는 그대로 64KB 단위다
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// `Drop`에서 워커가 끝나기를 기다리는 시간.
///
/// 워커가 서버 응답을 기다리는 중일 수 있어 무한정 기다리지 않는다 — 이만큼 기다려 보고
/// 안 끝나면 스레드를 떼어 놓는다(프로세스 종료가 정리한다)
const STOP_GRACE: Duration = Duration::from_millis(200);

/// 폴더를 재귀로 훑을 때의 깊이 상한 (plan T22 Edge Case — 순환 심볼릭 링크).
///
/// 40이면 실제 디렉터리 구조에는 넉넉하고, 링크가 자기를 가리켜도 그 안에서 멈춘다
const TREE_MAX_DEPTH: usize = 40;

/// 연결이 다시 시도할 규칙.
///
/// 서버가 "지금은 안 된다"(FTP 421 등)고 답하거나 네트워크가 잠깐 끊긴 것은 다시 걸면 되는
/// 실패다. 반대로 **인증 실패·호스트 키 거부는 다시 걸어도 같은 답**이라 재시도하지 않는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub base: Duration,
    pub max: Duration,
    /// 재시도 횟수 상한 — 이만큼 실패하면 포기한다
    pub attempts: u32,
}

impl RetryPolicy {
    /// 1초에서 시작해 두 배씩, 30초를 넘지 않게, 5회까지
    pub const DEFAULT: RetryPolicy = RetryPolicy {
        base: Duration::from_secs(1),
        max: Duration::from_secs(30),
        attempts: 5,
    };

    /// `attempt`(0부터)번째 재시도 전에 기다릴 시간. 상한을 넘으면 `None`(포기)
    pub fn delay(&self, attempt: u32) -> Option<Duration> {
        if attempt >= self.attempts {
            return None;
        }
        // 2의 거듭제곱이 커져도 넘치지 않게 상한에서 자른다
        let factor = 1u32.checked_shl(attempt).unwrap_or(u32::MAX);
        Some(self.base.saturating_mul(factor).min(self.max))
    }
}

/// 전송 방향
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDirection {
    Upload,
    Download,
}

/// 전송 한 건의 지시
#[derive(Debug, Clone, PartialEq)]
pub struct TransferRequest {
    pub id: TransferId,
    pub direction: TransferDirection,
    pub remote: RemotePath,
    pub local: PathBuf,
    /// 이어받기 시작점 — 0이면 처음부터.
    ///
    /// **받기는 이 값을 쓰지 않는다** — 받다 만 파일의 크기를 재는 것은 파일시스템 호출이라
    /// 화면 쪽에서 하면 프레임이 멈춘다(AGENTS: UI 스레드 블로킹 I/O 금지). 그래서 **워커가**
    /// 아래 `remote_size`와 실제 파일 크기로 직접 정한다. 보내기는 호출부가 정한 값을 쓴다
    pub offset: u64,
    /// 서버가 가진 파일 크기 — 받기에서 이어받기 지점을 정하는 데 쓴다(0이면 모른다)
    pub remote_size: u64,
}

/// 목록 조회를 청한 곳 — **요청이 자기 출처를 들고 다닌다**.
///
/// 종전에는 `u64` 세대 번호 하나에 **번호 공간을 잘라** 용도를 구분했다(패널은 1부터,
/// 트리는 `1<<40`, 같은 이름 확인은 `2<<40`). 그 방식은 종류가 늘 때마다 기준값을 하나씩
/// 더 만들어야 했고, 등록·발송 양쪽에서 손으로 기준값을 더하다 한쪽만 고쳐지면 답이 와도
/// 찾지 못했다. 출처를 타입으로 실으면 그 왕복 자체가 사라진다.
///
/// **`Panel`이 `PanelId`가 아니라 `u32` 둘을 담는 이유**: `remote` 계층은 `app`·`ui`를
/// 참조하지 않는다(단방향 의존). 워커에게 이 값은 **되돌려줄 꼬리표**일 뿐이라 뜻을 알
/// 필요가 없고, `PanelId`↔`u32` 변환은 그 뜻을 아는 `ui` 쪽이 한다.
///
/// **패널을 가리키는 데 `panel` 하나로는 모자란다** — `PanelId`는 워크스페이스마다
/// `0`부터 다시 매겨져 전역 유일하지 않은데 응답 라우팅은 모든 워크스페이스를 훑는다.
/// 셋(`workspace`·`panel`·`seq`)이 모두 맞아야 그 요청이다
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListSource {
    /// 패널 목록 — 두 `u32`는 `ui` 계층의 워크스페이스·패널 식별자다
    Panel {
        workspace: u32,
        panel: u32,
        seq: u64,
    },
    /// 원격 폴더 트리의 지연 확장
    Tree { seq: u64 },
    /// 같은 이름 확인 (FR-55) — `id`가 그 확인 번호다
    Conflict { id: u64 },
}

/// 워커에게 보내는 명령
#[derive(Debug, Clone, PartialEq)]
pub enum ConnCommand {
    Connect,
    /// 목록 조회. `source`는 **어느 요청의 답인지**를 가리며, 그 안의 일련번호가
    /// 늦게 도착한 이전 요청의 결과를 버리는 데 쓰인다.
    ///
    /// `quiet`면 **성공 기록을 서버 로그에 남기지 않는다** (FR-67 자동 재조회).
    /// 30초마다 도는 재조회가 기록을 남기면 조회마다 두 줄(`시작`·`완료`)이 쌓여
    /// **보이는 원격 탭 하나당 시간당 240줄**이 되고, 사용자가 실제로 한 조작의 기록이
    /// 그만큼 밀려난다. 실패는 `quiet`와 무관하게 남긴다 — 자동 재조회가 조용히 죽는 것을
    /// 알 수 없게 되기 때문이다
    List {
        source: ListSource,
        path: RemotePath,
        quiet: bool,
    },
    /// 폴더 하나를 **재귀로 훑어** 그 아래 파일을 모두 찾는다 (FR-38 — 폴더 드래그).
    ///
    /// 화면이 한 겹씩 요청해 가며 훑지 않는 이유: 그러면 목록 응답 라우팅과 뒤섞이고,
    /// 한 폴더를 훑는 동안 프레임마다 상태를 이어 붙여야 한다. 워커는 어차피 블로킹이라
    /// 여기서 한 번에 끝내는 편이 단순하다
    ListTree {
        generation: u64,
        root: RemotePath,
    },
    Cwd(RemotePath),
    Mkdir(RemotePath),
    Remove(RemotePath),
    Rmdir(RemotePath),
    Rename {
        from: RemotePath,
        to: RemotePath,
    },
    Chmod {
        path: RemotePath,
        mode: u32,
    },
    Transfer(TransferRequest),
    Disconnect,
    /// 워커를 끝낸다
    Stop,
}

/// 파일 작업 종류 — 결과 이벤트가 무엇에 대한 답인지 알리는 데 쓴다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Cwd,
    Mkdir,
    Remove,
    Rmdir,
    Rename,
    Chmod,
    Disconnect,
}

/// 연결의 현재 단계 — 화면 표시(T10)가 이것을 투영한다
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnPhase {
    /// 아직 연결을 걸지 않았다
    Idle,
    Connecting,
    Ready,
    Failed {
        detail: String,
        /// 실패 화면이 덧붙일 안내를 고르는 기준 (FR-32) — 원문과 함께 나른다.
        /// 문자열만 넘기면 화면이 사유를 다시 뜯어 갈래를 짐작해야 한다
        kind: FailureKind,
    },
    Closed,
}

/// 워커가 돌려주는 소식
#[derive(Debug, Clone, PartialEq)]
pub enum ConnEvent {
    Phase(ConnPhase),
    Listed {
        source: ListSource,
        path: RemotePath,
        entries: Vec<RemoteEntry>,
    },
    /// 서버와 주고받은 줄 — `Connection`이 자기 로그 버퍼에 쌓고 화면(T20)이 읽는다.
    ///
    /// **이 변형은 `log_event`로만 만든다** — 거기서 비밀번호를 가리기 때문이다(D14).
    /// 직접 만들면 평문이 채널을 타고 나간다(열거형 필드에는 가시성을 걸 수 없어 규약으로 지킨다)
    Log {
        kind: LogKind,
        text: String,
    },
    OpDone {
        op: OpKind,
        result: Result<(), RemoteError>,
    },
    /// 이 연결이 실제로 암호화됐는가 — 연결 직후 한 번 온다 (F-7 리뷰 B1).
    ///
    /// 설정값이 아니라 협상 결과다: `ExplicitIfAvailable`이 거부당해 평문으로 되연결하면 거짓이다
    Secure(bool),
    /// `List`가 실패했다 — **어느 요청이 실패했는지** 알린다 (T24).
    ///
    /// 실패를 로그에만 남기면, 답을 기다리던 쪽(원격 트리)이 영영 `읽는 중…`에 머문다
    ListFailed {
        source: ListSource,
        detail: String,
    },
    /// `ListTree`의 답 — 찾은 **파일**들의 전체 경로와 크기다(폴더는 담지 않는다).
    ///
    /// 훑는 중 실패한 가지는 조용히 건너뛴다(권한 없는 폴더가 흔하다) — 그 사실은 서버 로그에 남는다
    TreeListed {
        generation: u64,
        root: RemotePath,
        files: Vec<(RemotePath, u64)>,
    },
    TransferProgress {
        id: TransferId,
        transferred: u64,
    },
    TransferDone {
        id: TransferId,
        result: Result<u64, RemoteError>,
    },
}

/// 연결 하나 — 워커 스레드의 손잡이다.
///
/// 이 값이 사라지면 워커도 끝난다(`Drop`).
pub struct Connection {
    pub id: ConnectionId,
    pub site: SiteId,
    tx: Sender<ConnCommand>,
    rx: Receiver<ConnEvent>,
    handle: Option<JoinHandle<()>>,
    /// 멈추라고 이른 전송의 번호 — 워커의 진행 통지가 매 64KB마다 본다.
    ///
    /// **참·거짓이 아니라 번호를 담는 이유**: 명령 채널 밖의 신호라 그 전송이 시작되기 전에
    /// 먼저 닿을 수 있다. 참·거짓이면 그 신호가 누구를 겨냥한 것인지 알 수 없어, 막 시작한
    /// 전송이 그것을 지워 취소가 사라지거나 반대로 엉뚱한 다음 전송이 그것을 집어 곧바로
    /// 멈춘다. 번호를 담으면 그 전송만 자기 것으로 알아본다
    cancel: Arc<AtomicU64>,
    /// 이 연결을 접는다는 신호. **명령 채널을 보지 않는 구간**(재시도 백오프 대기)에서도
    /// 워커가 이것을 살펴 곧바로 빠져나온다
    shutdown: Arc<AtomicBool>,
    /// 워커가 끝나면 켜진다 — `Drop`이 이것으로 회수를 확인한다
    finished: Arc<AtomicBool>,
    phase: ConnPhase,
    /// 워커가 올린 **협상 결과** — 이 연결이 실제로 암호화됐는가 (F-7 리뷰 B1)
    secure: bool,
    /// 이 연결의 서버 로그 (FR-40). 워커가 올린 줄을 `poll`이 여기 쌓고 화면은 읽기만 한다
    log: LogBuffer,
}

impl Connection {
    /// 워커를 띄운다. `session`은 프로토콜 구현(`FtpSession`·`SftpSession`)이거나
    /// 테스트의 가짜 세션이며, **이 스레드로 소유가 넘어간다**
    pub fn spawn(
        id: ConnectionId,
        site: SiteRecord,
        password: String,
        session: Box<dyn RemoteSession>,
        wake: Wake,
        retry: RetryPolicy,
    ) -> Connection {
        let site_id = site.id;
        let (command_tx, command_rx) = channel::<ConnCommand>();
        let (event_tx, event_rx) = channel::<ConnEvent>();
        let cancel = Arc::new(AtomicU64::new(CANCEL_NONE));
        let finished = Arc::new(AtomicBool::new(false));
        let shutdown = Arc::new(AtomicBool::new(false));

        let worker_cancel = Arc::clone(&cancel);
        let worker_shutdown = Arc::clone(&shutdown);
        let worker_finished = Arc::clone(&finished);
        let handle = std::thread::spawn(move || {
            worker(Worker {
                site,
                password,
                session,
                rx: command_rx,
                tx: event_tx,
                wake,
                cancel: worker_cancel,
                shutdown: worker_shutdown,
                retry,
            });
            worker_finished.store(true, Ordering::SeqCst);
        });

        Connection {
            id,
            site: site_id,
            tx: command_tx,
            rx: event_rx,
            handle: Some(handle),
            cancel,
            shutdown,
            finished,
            phase: ConnPhase::Idle,
            secure: false,
            log: LogBuffer::new(),
        }
    }

    /// 명령을 보낸다. 워커가 이미 죽었으면 `false` — **조용히 무시하는 것이 맞다**
    /// (연결이 끊긴 뒤 남은 화면 조작이 오류 대화를 띄우면 사용자만 성가시다)
    pub fn send(&self, command: ConnCommand) -> bool {
        self.tx.send(command).is_ok()
    }

    /// 그 전송을 멈추라고 알린다. 워커가 다음 64KB 경계에서 멈춘다.
    ///
    /// **아직 시작되지 않은 전송에 걸어도 된다** — 워커가 그 번호를 꺼내 옮기기 시작하면
    /// 첫 경계에서 곧바로 멈춘다. 이미 끝난 전송에 걸어도 다음 전송은 번호가 달라 무사하다
    pub fn cancel_transfer(&self, transfer: TransferId) {
        self.cancel.store(transfer.0, Ordering::SeqCst);
    }

    /// 도착한 이벤트를 모두 가져온다. 단계 변화는 여기서 반영된다
    pub fn poll(&mut self) -> Vec<ConnEvent> {
        let mut events = Vec::new();
        // 비었거나 워커가 사라지면 `try_recv`가 실패하고 그 자리에서 끝난다
        while let Ok(event) = self.rx.try_recv() {
            match &event {
                ConnEvent::Phase(phase) => {
                    // 끊기거나 실패한 연결은 더 이상 암호화된 것이 아니다 — 참으로 남으면
                    // 새 호출부가 생겼을 때 조용히 거짓 표시가 된다 (F-7 2라운드 m4)
                    if !matches!(phase, ConnPhase::Ready | ConnPhase::Connecting) {
                        self.secure = false;
                    }
                    self.phase = phase.clone();
                }
                ConnEvent::Secure(secure) => self.secure = *secure,
                ConnEvent::Log { kind, text } => self.log.push(*kind, text.clone()),
                _ => {}
            }
            events.push(event);
        }
        events
    }

    pub fn phase(&self) -> &ConnPhase {
        &self.phase
    }

    /// 이 연결이 **실제로** 암호화돼 있는가 (F-7 리뷰 B1).
    ///
    /// 설정값이 아니라 워커가 올린 협상 결과다 — `ExplicitIfAvailable`이 거부당해 평문으로
    /// 되연결한 연결은 거짓이라, 상태 표시줄이 `· TLS`를 붙이지 않는다
    pub fn is_secure(&self) -> bool {
        self.secure
    }

    /// 이 연결의 서버 로그 — 화면은 읽기만 한다 (FR-40)
    pub fn log(&self) -> &LogBuffer {
        &self.log
    }

    /// 앱이 알아낸 사실을 이 연결의 로그에 남긴다 — 파일 작업 실패 사유 등 (FR-39).
    ///
    /// 워커가 올린 줄과 같은 버퍼에 쌓인다. 실패를 화면 한 곳(상태 줄)에만 띄우면
    /// 잠깐 뒤 사라져 무엇이 왜 안 됐는지 되짚을 수 없다
    pub fn push_log(&mut self, kind: LogKind, text: String) {
        self.log.push(kind, text);
    }

    /// 워커가 끝났는가 — 회수 확인용
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        // 전송 중이거나 재시도를 기다리는 중이라면 먼저 깨워 놓아야 `Stop`을 볼 수 있다 —
        // 이 연결은 사라지는 중이라 **무엇을 옮기고 있든** 멈춘다
        self.cancel.store(CANCEL_ALL, Ordering::SeqCst);
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = self.tx.send(ConnCommand::Stop);

        let deadline = Instant::now() + STOP_GRACE;
        while !self.finished.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        // 제때 끝난 워커만 거둔다. 서버 응답을 기다리며 붙잡힌 스레드까지 기다리면
        // 창을 닫는 순간 앱이 멈춘 것처럼 보인다
        if self.finished.load(Ordering::SeqCst)
            && let Some(handle) = self.handle.take()
        {
            let _ = handle.join();
        }
    }
}

/// 워커가 들고 도는 것들
/// 목록을 조회해 결과를 올린다 — 살아 있으면 `true`(받는 쪽이 사라졌으면 `false`).
///
/// `loud`면 시작·완료를 서버 로그에 남긴다. **실패는 `loud`와 무관하게 언제나 남긴다** —
/// 자동 재조회(FR-67)가 조용히 죽는 것을 사용자가 알 길이 없어지기 때문이다 (D9)
fn run_list(worker: &mut Worker, source: ListSource, path: RemotePath, loud: bool) -> bool {
    if loud
        && !worker.log(
            LogKind::Status,
            crate::i18n::dynamic::log_list_start(path.as_str()),
        )
    {
        return false;
    }
    match worker.session.list(&path) {
        Ok(entries) => {
            if loud
                && !worker.log(
                    LogKind::Status,
                    crate::i18n::dynamic::log_list_done(path.as_str()),
                )
            {
                return false;
            }
            worker.emit(ConnEvent::Listed {
                source,
                path,
                entries,
            })
        }
        Err(err) => {
            let detail = err.to_string();
            worker.log(LogKind::Error, detail.clone())
                && worker.emit(ConnEvent::ListFailed { source, detail })
        }
    }
}

struct Worker {
    site: SiteRecord,
    password: String,
    session: Box<dyn RemoteSession>,
    rx: Receiver<ConnCommand>,
    tx: Sender<ConnEvent>,
    wake: Wake,
    cancel: Arc<AtomicU64>,
    shutdown: Arc<AtomicBool>,
    retry: RetryPolicy,
}

impl Worker {
    /// 이벤트를 보내고 화면을 깨운다. 수신부가 사라졌으면 `false` — 워커가 스스로 끝낼 신호다
    fn emit(&self, event: ConnEvent) -> bool {
        if self.tx.send(event).is_err() {
            return false;
        }
        (self.wake)();
        true
    }

    fn log(&self, kind: LogKind, text: String) -> bool {
        self.emit(log_event(kind, text))
    }
}

/// 워커 본체 — 명령을 하나씩 순서대로 처리한다.
///
/// **한 번에 하나만 처리한다**는 것이 이 구조의 핵심이다: 탐색과 전송을 같은 연결에 태우면
/// 서로 끼어들지 않고 차례로 처리되며(D4의 겸용 상태), 다른 연결은 자기 워커에서 따로 돈다.
fn worker(mut worker: Worker) {
    // 명령 송신부가 사라지면 `recv`가 실패한다 — 이 연결은 더 쓰이지 않으므로 끝낸다
    while let Ok(command) = worker.rx.recv() {
        match command {
            ConnCommand::Stop => break,
            ConnCommand::Connect => {
                if !connect_with_retry(&mut worker) {
                    break;
                }
            }
            ConnCommand::List {
                source,
                path,
                quiet,
            } => {
                if !run_list(&mut worker, source, path, !quiet) {
                    break;
                }
            }
            ConnCommand::Cwd(path) => {
                let result = worker.session.cwd(&path);
                if !finish_op(&worker, OpKind::Cwd, result) {
                    break;
                }
            }
            ConnCommand::Mkdir(path) => {
                let result = worker.session.mkdir(&path);
                if !finish_op(&worker, OpKind::Mkdir, result) {
                    break;
                }
            }
            ConnCommand::Remove(path) => {
                let result = worker.session.remove(&path);
                if !finish_op(&worker, OpKind::Remove, result) {
                    break;
                }
            }
            ConnCommand::Rmdir(path) => {
                let result = worker.session.rmdir(&path);
                if !finish_op(&worker, OpKind::Rmdir, result) {
                    break;
                }
            }
            ConnCommand::Rename { from, to } => {
                let result = worker.session.rename(&from, &to);
                if !finish_op(&worker, OpKind::Rename, result) {
                    break;
                }
            }
            ConnCommand::Chmod { path, mode } => {
                let result = worker.session.chmod(&path, mode);
                if !finish_op(&worker, OpKind::Chmod, result) {
                    break;
                }
            }
            ConnCommand::ListTree { generation, root } => {
                let files = list_tree(&mut worker, &root);
                if !worker.emit(ConnEvent::TreeListed {
                    generation,
                    root,
                    files,
                }) {
                    break;
                }
            }
            ConnCommand::Transfer(request) => {
                if !run_transfer(&mut worker, request) {
                    break;
                }
                // 전송이 중간에 끊긴 뒤에는 이 연결을 그대로 쓸 수 없을 수 있다 —
                // 세션이 그렇다고 하면 버리고 다시 세운다
                if worker.session.needs_reset() && !reset_session(&mut worker) {
                    break;
                }
            }
            ConnCommand::Disconnect => {
                let result = worker.session.quit();
                let alive = worker.emit(ConnEvent::Phase(ConnPhase::Closed))
                    && finish_op(&worker, OpKind::Disconnect, result);
                if !alive {
                    break;
                }
            }
        }
    }
    // 끝내기 전에 인사만 하고 결과는 보지 않는다 — 소켓은 어차피 닫힌다
    let _ = worker.session.quit();
}

/// 어긋났을 수 있는 연결을 버리고 다시 세운다 (`RemoteSession::needs_reset`).
///
/// **인사만 하고 결과는 보지 않는다** — 이 소켓은 어차피 버리는 것이고, 그 위의 응답이
/// 몇 개 남았는지 알 수 없어 `quit`의 답도 믿을 것이 못 된다. 이어서 `connect_with_retry`가
/// 새 연결을 세우므로 재시도 정책·단계 로그·`Phase` 통지가 그대로 따라온다.
///
/// 돌아오는 값은 **워커를 계속 돌릴지**다(수신부가 사라졌으면 `false`).
fn reset_session(worker: &mut Worker) -> bool {
    let _ = worker.session.quit();
    connect_with_retry(worker)
}

/// 연결·로그인을 시도하고, 다시 걸어 볼 만한 실패면 지수 백오프로 되풀이한다.
///
/// 돌아오는 값은 **워커를 계속 돌릴지**다(수신부가 사라졌으면 `false`).
fn connect_with_retry(worker: &mut Worker) -> bool {
    let mut attempt = 0;
    loop {
        if !worker.emit(ConnEvent::Phase(ConnPhase::Connecting)) {
            return false;
        }
        // 진행을 단계마다 남긴다 — 로그 화면이 "지금 어디까지 갔는지"를 보여야
        // 실패했을 때 어느 단계에서 막혔는지 알 수 있다 (사용자 요청 2026-08-05)
        if !worker.log(
            LogKind::Status,
            crate::i18n::dynamic::log_connecting(&worker.site.address()),
        ) {
            return false;
        }
        let outcome = worker.session.connect(&worker.site).and_then(|()| {
            worker.log(
                LogKind::Status,
                crate::i18n::remote_log_connected().to_owned(),
            );
            // 암호화 여부는 연결이 선 직후에 정해진다 — 평문으로 떨어졌으면 그 사실을 알린다
            worker.log(
                LogKind::Status,
                if worker.session.is_secure() {
                    crate::i18n::remote_log_tls().to_owned()
                } else {
                    crate::i18n::remote_log_plain().to_owned()
                },
            );
            worker.log(LogKind::Status, crate::i18n::remote_log_login().to_owned());
            worker.session.login(&worker.site, &worker.password)
        });

        let Err(err) = outcome else {
            // 협상 결과를 먼저 올린다 — 화면이 `Ready`를 보고 상태 줄을 그릴 때
            // 암호화 여부가 이미 도착해 있어야 한 프레임도 거짓으로 적히지 않는다
            let secure = worker.session.is_secure();
            return worker.log(
                LogKind::Status,
                crate::i18n::remote_log_login_done().to_owned(),
            ) && worker.emit(ConnEvent::Secure(secure))
                && worker.emit(ConnEvent::Phase(ConnPhase::Ready));
        };

        // 인증·호스트 키 실패는 다시 걸어도 같은 답이다
        let Some(delay) = (is_retryable(&err))
            .then(|| worker.retry.delay(attempt))
            .flatten()
        else {
            return worker.emit(ConnEvent::Phase(ConnPhase::Failed {
                detail: err.to_string(),
                kind: err.failure_kind(),
            }));
        };

        if !worker.log(
            LogKind::Status,
            crate::i18n::dynamic::log_retry(delay.as_secs(), &err.to_string()),
        ) {
            return false;
        }
        // 기다리는 동안은 명령 채널을 보지 않는다 — 그 사이 연결이 닫히면 곧바로 접는다
        if !sleep_until_shutdown(&worker.shutdown, delay) {
            return false;
        }
        attempt += 1;
    }
}

/// 로그 이벤트를 만든다 — **비밀번호는 여기서 가려진다** (D14).
///
/// 버퍼에 쌓을 때만 가리면, 이벤트를 버퍼 대신 직접 소비하는 쪽(화면·상위 계층)에는
/// 평문이 그대로 흘러간다. 채널에 실리기 전에 한 번 가려 그 경로를 막는다.
fn log_event(kind: LogKind, text: String) -> ConnEvent {
    ConnEvent::Log {
        kind,
        text: mask_secrets(&text),
    }
}

/// 종료 신호를 살피며 잔다. 신호가 오면 그 자리에서 `false`를 돌려준다.
///
/// 재시도 대기는 최대 30초라 한 번에 자 버리면, 그동안 앱을 닫아도 워커가 소켓을 쥔 채
/// 그만큼 남는다. 잘게 나눠 자며 살펴 종료가 `STOP_GRACE` 안에 끝나게 한다.
fn sleep_until_shutdown(shutdown: &AtomicBool, total: Duration) -> bool {
    /// 살피는 간격 — `Drop`의 대기(200ms)보다 짧아야 그 안에 빠져나온다
    const SLICE: Duration = Duration::from_millis(20);
    let deadline = Instant::now() + total;
    loop {
        if shutdown.load(Ordering::SeqCst) {
            return false;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        std::thread::sleep(left.min(SLICE));
    }
}

/// 다시 걸어 볼 만한 실패인가.
///
/// 연결 계열(FTP 421 "지금은 안 된다", 일시적 네트워크 오류)만 다시 시도한다.
/// 인증·호스트 키·경로·권한 실패는 상황이 바뀌지 않으면 같은 답이 돌아온다
fn is_retryable(err: &RemoteError) -> bool {
    matches!(err, RemoteError::Connect { .. })
}

fn finish_op(worker: &Worker, op: OpKind, result: RemoteResult<()>) -> bool {
    worker.emit(ConnEvent::OpDone { op, result })
}

/// 폴더 아래의 파일을 모두 찾는다 — 깊이 상한 `TREE_MAX_DEPTH`.
///
/// **끊지 않으면 순환 심볼릭 링크에서 영원히 돈다** (plan Edge Case). 실패한 가지는
/// 건너뛴다 — 권한 없는 폴더 하나 때문에 나머지를 통째로 버릴 이유가 없다
fn list_tree(worker: &mut Worker, root: &RemotePath) -> Vec<(RemotePath, u64)> {
    let mut found = Vec::new();
    let mut pending = vec![(root.clone(), 0usize)];
    while let Some((dir, depth)) = pending.pop() {
        // 큰 트리를 훑는 중에도 앱은 닫힐 수 있다 — 종료 신호를 살피지 않으면
        // 창을 닫아도 워커가 서버를 계속 훑는다 (T4가 백오프 대기에서 겪은 것과 같다)
        if worker.shutdown.load(Ordering::SeqCst) {
            return found;
        }
        if depth >= TREE_MAX_DEPTH {
            worker.log(
                LogKind::Status,
                crate::i18n::dynamic::log_too_deep(dir.as_str()),
            );
            continue;
        }
        let entries = match worker.session.list(&dir) {
            Ok(entries) => entries,
            Err(err) => {
                worker.log(
                    LogKind::Error,
                    crate::i18n::dynamic::log_read_failed(dir.as_str(), &err.to_string()),
                );
                continue;
            }
        };
        for entry in entries {
            // 상위 이동은 따라가지 않는다 — 따라가면 트리를 벗어나 위로 올라간다
            if entry.name == ".." || entry.name == "." {
                continue;
            }
            let path = dir.join(&entry.name);
            if entry.is_dir {
                pending.push((path, depth + 1));
            } else {
                found.push((path, entry.size));
            }
        }
    }
    found
}

/// 전송 한 건 — 로컬 파일을 열고 세션에 스트림을 넘긴다 (NFR-12).
///
/// 진행률은 `PROGRESS_INTERVAL`마다 묶어 보내고, 취소 신호는 64KB 경계마다 본다
fn run_transfer(worker: &mut Worker, request: TransferRequest) -> bool {
    let id = request.id;
    // **들어오면서 신호를 지우지 않는다** — 이 전송을 멈추라는 말이 시작보다 먼저 닿아 있을
    // 수 있고, 지우면 그 취소가 사라진다
    let result = transfer(worker, &request);
    let alive = worker.emit(ConnEvent::TransferDone { id, result });
    // 이 전송을 겨냥한 신호만 거둔다. 연결을 접는 중(`CANCEL_ALL`)이거나 벌써 다음 전송을
    // 겨냥한 신호가 들어와 있으면 그대로 둔다
    let _ = worker
        .cancel
        .compare_exchange(id.0, CANCEL_NONE, Ordering::SeqCst, Ordering::SeqCst);
    alive
}

fn transfer(worker: &mut Worker, request: &TransferRequest) -> RemoteResult<u64> {
    // 받기의 이어받기 지점은 **여기서** 정한다 — 받다 만 파일의 크기를 재는 것은
    // 파일시스템 호출이고, 그것을 화면 쪽에서 하면 프레임이 멈춘다 (AGENTS)
    let offset = match request.direction {
        TransferDirection::Download => {
            let done = std::fs::metadata(&request.local)
                .map(|meta| meta.len())
                .unwrap_or(0);
            crate::remote::transfer::resume_offset(done, request.remote_size)
        }
        TransferDirection::Upload => request.offset,
    };
    let mut progress = ThrottledProgress {
        tx: worker.tx.clone(),
        wake: Arc::clone(&worker.wake),
        id: request.id,
        cancel: Arc::clone(&worker.cancel),
        last_sent: Instant::now(),
        // 이어받는 중이면 화면이 보는 값은 **파일 전체 기준**이어야 한다 —
        // 이번에 옮긴 바이트만 올리면 진행률이 뒤로 튄다
        base: offset,
    };

    match request.direction {
        TransferDirection::Download => {
            let mut file = open_for_download(&request.local, offset)?;
            worker
                .session
                .download(&request.remote, &mut file, offset, &mut progress)
                .map(|moved| moved + offset)
        }
        TransferDirection::Upload => {
            let mut file = File::open(&request.local).map_err(local_error)?;
            if offset > 0 {
                file.seek(SeekFrom::Start(offset)).map_err(local_error)?;
            }
            worker
                .session
                .upload(&request.remote, &mut file, offset, &mut progress)
        }
    }
}

/// 받을 파일을 연다. 이어받기면 있던 파일을 자르지 않고 그 지점부터 이어 쓴다
fn open_for_download(local: &PathBuf, offset: u64) -> RemoteResult<File> {
    if offset == 0 {
        return File::create(local).map_err(local_error);
    }
    let mut file = OpenOptions::new()
        .write(true)
        .open(local)
        .map_err(local_error)?;
    file.seek(SeekFrom::Start(offset)).map_err(local_error)?;
    Ok(file)
}

/// 로컬 파일 쪽 실패도 전송 실패로 알린다 — 사용자에게는 같은 한 가지 일이다
fn local_error(err: std::io::Error) -> RemoteError {
    RemoteError::Transfer {
        detail: err.to_string(),
        transferred: 0,
    }
}

/// 진행률을 묶어 보내고 취소 신호를 전하는 통지
struct ThrottledProgress {
    tx: Sender<ConnEvent>,
    wake: Wake,
    id: TransferId,
    cancel: Arc<AtomicU64>,
    last_sent: Instant,
    /// 이미 받아 둔 만큼 — 보고 값에 더해 **파일 전체 기준**으로 올린다
    base: u64,
}

impl Progress for ThrottledProgress {
    fn report(&mut self, transferred: u64) -> bool {
        if self.last_sent.elapsed() >= PROGRESS_INTERVAL {
            self.last_sent = Instant::now();
            if self
                .tx
                .send(ConnEvent::TransferProgress {
                    id: self.id,
                    transferred: self.base + transferred,
                })
                .is_ok()
            {
                (self.wake)();
            }
        }
        // 이 전송을 겨냥한 신호이거나 연결이 접히는 중일 때만 멈춘다
        let target = self.cancel.load(Ordering::SeqCst);
        target != self.id.0 && target != CANCEL_ALL
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::testing::{FakeServer, FakeSession, fake_entry};
    use crate::remote::types::SiteId;

    fn silent_wake() -> Wake {
        Arc::new(|| {})
    }

    fn fast_retry() -> RetryPolicy {
        // 테스트가 실제로 초 단위를 기다리지 않도록 눈금만 줄인다 — 규칙은 같다
        RetryPolicy {
            base: Duration::from_millis(1),
            max: Duration::from_millis(4),
            attempts: 5,
        }
    }

    fn site() -> SiteRecord {
        SiteRecord::new(SiteId(9), "가짜".to_owned())
    }

    fn spawn(server: &Arc<FakeServer>, retry: RetryPolicy) -> Connection {
        Connection::spawn(
            ConnectionId(1),
            site(),
            "비밀".to_owned(),
            Box::new(FakeSession::new(Arc::clone(server))),
            silent_wake(),
            retry,
        )
    }

    /// 전송이 중간에 끊긴 연결은 **버리고 다시 세운다** (`RemoteSession::needs_reset`).
    ///
    /// 다시 세우지 않으면 어긋난 세션이 그대로 남아 그 뒤의 모든 명령이 실패한다 — 사용자에게는
    /// 「취소하고 다시 전송하면 연결 오류」로 보인다. 실제 FTP 서버에서 그 일이 났고,
    /// 여기서는 `DirtyingSession`이 그 상태를 흉내 낸다
    #[test]
    fn 전송이_끊긴_연결은_다시_세워진다() {
        let server = FakeServer::new();
        server.set_entries("/", vec![fake_entry("있다.txt", false)]);
        server.set_download_size(64 * 1024 * 16);
        let mut connection = Connection::spawn(
            ConnectionId(1),
            site(),
            "비밀".to_owned(),
            Box::new(DirtyingSession::new(&server)),
            silent_wake(),
            fast_retry(),
        );
        connection.send(ConnCommand::Connect);
        wait_for(&mut connection, Duration::from_secs(2), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Phase(ConnPhase::Ready)))
        });

        // **기계 속도에 기대지 않는다** — 서버를 응답 없는 상태로 두어 전송을 들어간 자리에
        // 세워 놓고, 취소 신호를 준 뒤에 풀어 준다
        let local = temp_path("reset_after_cancel");
        server.set_hang(true);
        connection.send(ConnCommand::Transfer(TransferRequest {
            id: TransferId(1),
            direction: TransferDirection::Download,
            remote: RemotePath::new("/big.bin"),
            local: local.clone(),
            offset: 0,
            remote_size: 0,
        }));
        wait_until(Duration::from_secs(2), || {
            server.calls().iter().any(|name| name == "download")
        });
        connection.cancel_transfer(TransferId(1));
        server.set_hang(false);

        // 취소로 끝난 뒤 재수립이 돌아 `Ready`가 **다시** 온다.
        // 첫 `Ready`는 위 연결 대기가 이미 거둬 갔으므로 여기서는 한 번 더 오는지를 본다
        let events = wait_for(&mut connection, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Phase(ConnPhase::Ready)))
        });
        assert!(
            events.iter().any(|event| matches!(
                event,
                ConnEvent::TransferDone {
                    result: Err(RemoteError::Cancelled),
                    ..
                }
            )),
            "취소로 끝나야 한다: {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Phase(ConnPhase::Ready))),
            "연결을 다시 세우지 않았다: {events:?}"
        );

        // 다시 세운 연결로 보낸 명령이 통해야 한다 — 이것이 사용자가 겪던 그 자리다
        connection.send(ConnCommand::List {
            source: ListSource::Tree { seq: 7 },
            path: RemotePath::root(),
            quiet: false,
        });
        let after = wait_for(&mut connection, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Listed { .. }))
        });
        assert!(
            after
                .iter()
                .any(|event| matches!(event, ConnEvent::Listed { source, .. } if *source == ListSource::Tree { seq: 7 })),
            "다시 세운 연결로 목록을 읽지 못했다: {after:?}"
        );

        drop(connection);
        let _ = std::fs::remove_file(local);
    }

    /// 전송이 중간에 끊기면 **스스로를 못 쓰는 것으로 표시하는** 세션.
    ///
    /// FTP의 실제 결함을 흉내 낸다 — 중단 뒤 제어 채널에 읽지 않은 응답이 남아 그 뒤 명령이
    /// 전부 어긋나는 상태다. 여기서는 그것을 「오염된 동안 모든 명령이 실패한다」로 단순화하고,
    /// 다시 연결하면 풀린다. `FakeSession`을 그대로 두고 감싸는 이유는 그쪽이 프로토콜 무관한
    /// 가짜 서버라 이 결함을 알 이유가 없기 때문이다
    struct DirtyingSession {
        inner: FakeSession,
        dirty: bool,
    }

    impl DirtyingSession {
        fn new(server: &Arc<FakeServer>) -> DirtyingSession {
            DirtyingSession {
                inner: FakeSession::new(Arc::clone(server)),
                dirty: false,
            }
        }

        /// 오염된 동안에는 어떤 명령도 답을 믿을 수 없다
        fn guard(&self) -> RemoteResult<()> {
            if self.dirty {
                return Err(RemoteError::Protocol {
                    detail: "제어 채널이 어긋났습니다".to_owned(),
                });
            }
            Ok(())
        }
    }

    impl RemoteSession for DirtyingSession {
        fn connect(&mut self, site: &SiteRecord) -> RemoteResult<()> {
            self.dirty = false;
            self.inner.connect(site)
        }

        fn is_secure(&self) -> bool {
            // 오염 여부와 협상 결과는 다른 축이다 — 감싼 세션의 답을 그대로 전한다
            self.inner.is_secure()
        }

        fn needs_reset(&self) -> bool {
            self.dirty
        }

        fn login(&mut self, site: &SiteRecord, password: &str) -> RemoteResult<()> {
            self.inner.login(site, password)
        }

        fn pwd(&mut self) -> RemoteResult<RemotePath> {
            self.guard()?;
            self.inner.pwd()
        }

        fn list(&mut self, path: &RemotePath) -> RemoteResult<Vec<RemoteEntry>> {
            self.guard()?;
            self.inner.list(path)
        }

        fn cwd(&mut self, path: &RemotePath) -> RemoteResult<()> {
            self.guard()?;
            self.inner.cwd(path)
        }

        fn mkdir(&mut self, path: &RemotePath) -> RemoteResult<()> {
            self.guard()?;
            self.inner.mkdir(path)
        }

        fn remove(&mut self, path: &RemotePath) -> RemoteResult<()> {
            self.guard()?;
            self.inner.remove(path)
        }

        fn rmdir(&mut self, path: &RemotePath) -> RemoteResult<()> {
            self.guard()?;
            self.inner.rmdir(path)
        }

        fn rename(&mut self, from: &RemotePath, to: &RemotePath) -> RemoteResult<()> {
            self.guard()?;
            self.inner.rename(from, to)
        }

        fn chmod(&mut self, path: &RemotePath, mode: u32) -> RemoteResult<()> {
            self.guard()?;
            self.inner.chmod(path, mode)
        }

        fn download(
            &mut self,
            path: &RemotePath,
            dest: &mut dyn std::io::Write,
            offset: u64,
            progress: &mut dyn Progress,
        ) -> RemoteResult<u64> {
            self.guard()?;
            let outcome = self.inner.download(path, dest, offset, progress);
            self.dirty = outcome.is_err();
            outcome
        }

        fn upload(
            &mut self,
            path: &RemotePath,
            src: &mut dyn std::io::Read,
            offset: u64,
            progress: &mut dyn Progress,
        ) -> RemoteResult<u64> {
            self.guard()?;
            let outcome = self.inner.upload(path, src, offset, progress);
            self.dirty = outcome.is_err();
            outcome
        }

        fn noop(&mut self) -> RemoteResult<()> {
            self.guard()?;
            self.inner.noop()
        }

        fn quit(&mut self) -> RemoteResult<()> {
            self.inner.quit()
        }
    }

    /// 테스트가 쓰는 임시 파일 — 프로세스 번호를 넣어 동시에 도는 다른 실행과 겹치지 않게 한다
    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("fe_t4_{label}_{}.bin", std::process::id()))
    }

    #[test]
    fn 폴더를_재귀로_훑어_파일만_올린다() {
        // T22 Acceptance ④의 원격 절반 — 화면이 한 겹씩 청하지 않고 워커가 한 번에 끝낸다.
        // 읽지 못하는 가지는 건너뛰고(권한 없는 폴더는 흔하다) 나머지는 그대로 돌려준다
        let server = FakeServer::new();
        server.set_entries(
            "/pub",
            vec![
                fake_entry("겉.txt", false),
                fake_entry("안쪽", true),
                fake_entry("못읽는곳", true),
            ],
        );
        server.set_entries(
            "/pub/안쪽",
            vec![fake_entry("속.bin", false), fake_entry("..", true)],
        );
        // `/pub/못읽는곳`은 등록하지 않는다 — 가짜 서버가 "없는 폴더"로 답해 그 가지가 실패한다
        let mut connection = spawn(&server, fast_retry());
        // 훑기도 서버와 말하는 일이라 연결이 먼저다
        connection.send(ConnCommand::Connect);
        connection.send(ConnCommand::ListTree {
            generation: 3,
            root: RemotePath::new("/pub"),
        });

        // 연결 단계 이벤트가 먼저 오므로 **훑기 결과가 나올 때까지** 모은다
        let mut events = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            events.extend(connection.poll());
            if events
                .iter()
                .any(|event| matches!(event, ConnEvent::TreeListed { .. }))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let listed = events.iter().find_map(|event| match event {
            ConnEvent::TreeListed {
                generation,
                root,
                files,
            } if *generation == 3 => Some((root.clone(), files.clone())),
            _ => None,
        });
        let (root, mut files) = listed.expect("훑기 결과가 오지 않았다");
        assert_eq!(root, RemotePath::new("/pub"));
        files.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        let paths: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["/pub/겉.txt", "/pub/안쪽/속.bin"],
            "폴더는 담지 않고 파일만, 실패한 가지는 건너뛴다"
        );
    }

    /// 조건이 참이 될 때까지 짧게 기다린다. 시간이 아니라 **관측된 상태**로 판정하기 위한 것이다
    fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        done()
    }

    /// **찾는 이벤트가 올 때까지** 모은다 — 개수로 기다리면 로그 한 줄이 늘 때마다 깨진다
    /// (진행 상태 줄을 더했을 때 실제로 셋이 깨졌다)
    fn wait_for(
        connection: &mut Connection,
        limit: Duration,
        mut found: impl FnMut(&[ConnEvent]) -> bool,
    ) -> Vec<ConnEvent> {
        let deadline = Instant::now() + limit;
        let mut events = Vec::new();
        loop {
            events.extend(connection.poll());
            if found(&events) || Instant::now() >= deadline {
                return events;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// 이벤트가 올 때까지 짧게 기다린다 — 워커가 다른 스레드라 즉시 오지 않는다
    fn wait_events(connection: &mut Connection, want: usize, limit: Duration) -> Vec<ConnEvent> {
        let deadline = Instant::now() + limit;
        let mut events = Vec::new();
        while events.len() < want && Instant::now() < deadline {
            events.extend(connection.poll());
            if events.len() < want {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        events
    }

    #[test]
    fn 로그_이벤트는_채널에_실리기_전에_가려진다() {
        // 버퍼에 쌓을 때만 가리면, 이벤트를 직접 소비하는 쪽에 평문이 흘러간다 (D14)
        let ConnEvent::Log { text, .. } =
            log_event(LogKind::Command, "PASS 진짜비밀번호".to_owned())
        else {
            panic!("로그 이벤트가 아니다");
        };
        assert_eq!(text, "PASS ******");

        let ConnEvent::Log { text, .. } = log_event(
            LogKind::Status,
            "sftp://deploy:p@ss@example.test:22 에 연결".to_owned(),
        ) else {
            panic!("로그 이벤트가 아니다");
        };
        assert!(!text.contains("p@ss"), "비밀번호가 남았다: {text}");
    }

    #[test]
    fn 백오프는_두_배씩_늘고_상한에서_멈추며_횟수를_넘기면_포기한다() {
        let policy = RetryPolicy::DEFAULT;
        assert_eq!(policy.delay(0), Some(Duration::from_secs(1)));
        assert_eq!(policy.delay(1), Some(Duration::from_secs(2)));
        assert_eq!(policy.delay(2), Some(Duration::from_secs(4)));
        assert_eq!(policy.delay(3), Some(Duration::from_secs(8)));
        assert_eq!(policy.delay(4), Some(Duration::from_secs(16)));
        // 5회를 넘기면 포기한다
        assert_eq!(policy.delay(5), None);

        // 상한을 넘는 지연은 30초에서 잘린다
        let long = RetryPolicy {
            attempts: 10,
            ..RetryPolicy::DEFAULT
        };
        assert_eq!(long.delay(5), Some(Duration::from_secs(30)));
        assert_eq!(long.delay(9), Some(Duration::from_secs(30)));
    }

    #[test]
    fn 워커는_명령을_처리하고_이벤트를_돌려준다() {
        let server = FakeServer::new();
        server.set_entries("/pub", vec![fake_entry("a.txt", false)]);
        let mut connection = spawn(&server, fast_retry());

        connection.send(ConnCommand::Connect);
        connection.send(ConnCommand::List {
            source: ListSource::Tree { seq: 1 },
            path: RemotePath::new("/pub"),
            quiet: false,
        });
        let events = wait_events(&mut connection, 3, Duration::from_secs(2));

        assert!(events.contains(&ConnEvent::Phase(ConnPhase::Connecting)));
        assert!(events.contains(&ConnEvent::Phase(ConnPhase::Ready)));
        let listed = events
            .iter()
            .find_map(|event| match event {
                ConnEvent::Listed {
                    source, entries, ..
                } => Some((*source, entries.len())),
                _ => None,
            })
            .expect("목록 이벤트가 없다");
        assert_eq!(listed, (ListSource::Tree { seq: 1 }, 1));
        assert_eq!(*connection.phase(), ConnPhase::Ready);
    }

    #[test]
    fn 연결이_사라지면_워커_스레드가_회수된다() {
        let server = FakeServer::new();
        {
            let mut connection = spawn(&server, fast_retry());
            connection.send(ConnCommand::Connect);
            wait_events(&mut connection, 2, Duration::from_secs(2));
            assert_eq!(server.live_sessions(), 1);
        }
        // `Drop`이 `Stop`을 보내고 짧게 기다린다 — 세션도 함께 사라진다
        let deadline = Instant::now() + Duration::from_secs(2);
        while server.live_sessions() > 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(server.live_sessions(), 0, "워커가 회수되지 않았다");
    }

    #[test]
    fn 늦게_도착한_이전_세대의_목록은_버려진다() {
        // 세대 번호는 워커가 실어 돌려주고, 화면 쪽이 지금 세대와 견줘 버린다
        let server = FakeServer::new();
        server.set_entries("/old", vec![fake_entry("old.txt", false)]);
        server.set_entries("/new", vec![fake_entry("new.txt", false)]);
        let mut connection = spawn(&server, fast_retry());
        connection.send(ConnCommand::Connect);
        connection.send(ConnCommand::List {
            source: ListSource::Tree { seq: 1 },
            path: RemotePath::new("/old"),
            quiet: false,
        });
        connection.send(ConnCommand::List {
            source: ListSource::Tree { seq: 2 },
            path: RemotePath::new("/new"),
            quiet: false,
        });
        let events = wait_events(&mut connection, 4, Duration::from_secs(2));

        let current = ListSource::Tree { seq: 2 };
        let kept: Vec<&RemotePath> = events
            .iter()
            .filter_map(|event| match event {
                ConnEvent::Listed { source, path, .. } if *source == current => Some(path),
                _ => None,
            })
            .collect();
        let dropped = events
            .iter()
            .filter(|event| matches!(event, ConnEvent::Listed { source, .. } if *source != current))
            .count();
        assert_eq!(kept.len(), 1, "지금 세대의 결과만 남아야 한다");
        assert_eq!(kept[0].as_str(), "/new");
        assert_eq!(
            dropped, 1,
            "이전 세대의 결과가 함께 돌아와야 한다(버릴 대상)"
        );
    }

    #[test]
    fn 한_연결이_막혀도_다른_연결은_계속_처리된다() {
        // NFR-11의 핵심 근거 — 연결마다 스레드가 따로라 서로를 막지 않는다
        let blocked_server = FakeServer::new();
        let live_server = FakeServer::new();
        live_server.set_entries("/", vec![fake_entry("살아있음", false)]);

        let mut blocked = spawn(&blocked_server, fast_retry());
        let mut live = Connection::spawn(
            ConnectionId(2),
            site(),
            "비밀".to_owned(),
            Box::new(FakeSession::new(Arc::clone(&live_server))),
            silent_wake(),
            fast_retry(),
        );
        live.send(ConnCommand::Connect);
        wait_events(&mut live, 2, Duration::from_secs(2));

        // 한쪽 서버를 응답 없는 상태로 만들고 명령을 밀어 넣는다
        blocked_server.set_hang(true);
        blocked.send(ConnCommand::Connect);
        blocked.send(ConnCommand::List {
            source: ListSource::Tree { seq: 1 },
            path: RemotePath::root(),
            quiet: false,
        });
        // 워커가 그 명령을 **실제로 집어 막혔다**를 확인한 뒤에 단언한다 — 고정 시간을 재우면
        // CPU가 붐빌 때 워커가 아직 시작도 못 한 채 단언이 돌아 「목록이 없다」가 우연히 참이 된다.
        // `FakeSession::connect`가 `record` 뒤에 `tick`에서 멈추므로(`remote::testing`), `"connect"`가
        // 보이면 그 스레드는 hang 루프 안이고 뒤이어 보낸 `List`는 큐에 남아 있다
        let deadline = Instant::now() + Duration::from_secs(2);
        while !blocked_server
            .calls()
            .iter()
            .any(|c| c.as_str() == "connect")
        {
            assert!(
                Instant::now() < deadline,
                "막힌 연결의 워커가 명령을 집지 못했다"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            !blocked
                .poll()
                .iter()
                .any(|event| matches!(event, ConnEvent::Listed { .. })),
            "막힌 연결이 목록을 내면 안 된다"
        );

        // 그 사이에도 다른 연결은 정상 처리된다
        live.send(ConnCommand::List {
            source: ListSource::Tree { seq: 1 },
            path: RemotePath::root(),
            quiet: false,
        });
        let events = wait_events(&mut live, 1, Duration::from_secs(2));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Listed { .. })),
            "막히지 않은 연결은 계속 답해야 한다"
        );

        // 테스트가 끝나도록 풀어 준다
        blocked_server.set_hang(false);
    }

    #[test]
    fn 다시_걸어_볼_만한_실패만_재시도한다() {
        assert!(is_retryable(&RemoteError::Connect {
            detail: "421 Too many connections".to_owned()
        }));
        // 비밀번호가 틀린 것은 다시 걸어도 같다
        assert!(!is_retryable(&RemoteError::Auth {
            detail: "530".to_owned()
        }));
        assert!(!is_retryable(&RemoteError::HostKey {
            detail: "지문 불일치".to_owned()
        }));
    }

    #[test]
    fn 거절당한_연결은_백오프로_다시_시도한다() {
        let server = FakeServer::new();
        server.fail_connects(2);
        let mut connection = spawn(&server, fast_retry());
        connection.send(ConnCommand::Connect);
        let events = wait_for(&mut connection, Duration::from_secs(3), |events| {
            events.contains(&ConnEvent::Phase(ConnPhase::Ready))
        });

        assert!(
            events.contains(&ConnEvent::Phase(ConnPhase::Ready)),
            "두 번 거절당한 뒤 연결되어야 한다: {events:?}"
        );
        let attempts = server
            .calls()
            .iter()
            .filter(|name| name.as_str() == "connect")
            .count();
        assert_eq!(attempts, 3);
    }

    #[test]
    fn 상한까지_실패하면_포기하고_실패_단계로_간다() {
        let server = FakeServer::new();
        server.fail_connects(99);
        let mut connection = spawn(&server, fast_retry());
        connection.send(ConnCommand::Connect);
        let events = wait_for(&mut connection, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Phase(ConnPhase::Failed { .. })))
        });

        assert!(
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Phase(ConnPhase::Failed { .. }))),
            "포기하고 실패 단계로 가야 한다: {events:?}"
        );
        // 처음 1회 + 재시도 5회
        let attempts = server
            .calls()
            .iter()
            .filter(|name| name.as_str() == "connect")
            .count();
        assert_eq!(attempts, 6);
    }

    #[test]
    fn 조용한_목록_조회는_성공_기록을_남기지_않는다() {
        // FR-67 acceptance ⓙ (D9) — 30초마다 도는 재조회가 `List`를 그대로 타면
        // 보이는 원격 탭 하나당 시간당 240줄이 쌓여 사용자 조작 기록이 밀려난다.
        // **같은 조건의 `List`와 견준다** — 조용한 쪽만 재면 로그가 원래 비어 있었을 수도 있다
        fn 상태_줄_수(server: &std::sync::Arc<FakeServer>, quiet: bool) -> usize {
            let mut connection = spawn(server, fast_retry());
            connection.send(ConnCommand::Connect);
            let command = if quiet {
                ConnCommand::List {
                    source: ListSource::Tree { seq: 1 },
                    path: RemotePath::root(),
                    quiet: true,
                }
            } else {
                ConnCommand::List {
                    source: ListSource::Tree { seq: 1 },
                    path: RemotePath::root(),
                    quiet: false,
                }
            };
            connection.send(command);
            let _ = wait_for(&mut connection, Duration::from_secs(3), |events| {
                events
                    .iter()
                    .any(|event| matches!(event, ConnEvent::Listed { .. }))
            });
            connection
                .log()
                .iter()
                .filter(|line| {
                    line.kind == LogKind::Status && line.text.contains(RemotePath::root().as_str())
                })
                .count()
        }

        let server = FakeServer::new();
        server.set_entries("/", vec![fake_entry("a.txt", false)]);
        let 소리내어 = 상태_줄_수(&server, false);
        let 조용히 = 상태_줄_수(&server, true);
        assert_eq!(소리내어, 2, "`quiet: false`는 시작·완료 두 줄을 남긴다");
        assert_eq!(조용히, 0, "`quiet: true`인데 성공 기록을 남겼다");
    }

    #[test]
    fn 탐색과_전송이_한_연결에서_차례로_처리된다() {
        // D4의 겸용 상태(M=1) — 한 워커가 둘을 직렬로 처리하고 어느 쪽도 잃지 않는다
        let server = FakeServer::new();
        server.set_entries("/", vec![fake_entry("a.txt", false)]);
        server.set_download_size(4 * 1024);
        let mut connection = spawn(&server, fast_retry());
        let local = temp_path("직렬_확인");

        connection.send(ConnCommand::Connect);
        connection.send(ConnCommand::List {
            source: ListSource::Tree { seq: 1 },
            path: RemotePath::root(),
            quiet: false,
        });
        connection.send(ConnCommand::Transfer(TransferRequest {
            id: TransferId(7),
            direction: TransferDirection::Download,
            remote: RemotePath::new("/a.txt"),
            local: local.clone(),
            offset: 0,
            remote_size: 0,
        }));
        connection.send(ConnCommand::List {
            source: ListSource::Tree { seq: 2 },
            path: RemotePath::root(),
            quiet: false,
        });
        let events = wait_for(&mut connection, Duration::from_secs(3), |events| {
            events
                .iter()
                .filter(|event| matches!(event, ConnEvent::Listed { .. }))
                .count()
                >= 2
                && events
                    .iter()
                    .any(|event| matches!(event, ConnEvent::TransferDone { .. }))
        });

        let listed = events
            .iter()
            .filter(|event| matches!(event, ConnEvent::Listed { .. }))
            .count();
        let done = events.iter().any(
            |event| matches!(event, ConnEvent::TransferDone { id, result } if *id == TransferId(7) && result.is_ok()),
        );
        assert_eq!(listed, 2, "탐색 두 건이 모두 살아 있어야 한다: {events:?}");
        assert!(done, "전송도 끝나야 한다: {events:?}");
        // 명령이 뒤섞이지 않고 보낸 순서대로 처리됐다
        let order: Vec<String> = server
            .calls()
            .into_iter()
            .filter(|name| name == "list" || name == "download")
            .collect();
        assert_eq!(order, vec!["list", "download", "list"]);

        drop(connection);
        let _ = std::fs::remove_file(local);
    }

    #[test]
    fn 취소하면_전송이_그_자리에서_멈춘다() {
        let server = FakeServer::new();
        server.set_download_size(8 * 1024 * 1024);
        let mut connection = spawn(&server, fast_retry());
        let local = temp_path("취소_확인");

        connection.send(ConnCommand::Connect);
        wait_events(&mut connection, 2, Duration::from_secs(2));

        // **기계 속도에 기대지 않는다** — 서버를 응답 없는 상태로 두어 전송을 들어간 자리에
        // 세워 놓고, 취소 신호를 준 뒤에 풀어 준다. 그래야 "얼마나 빨리 옮기느냐"와 무관하게
        // 취소가 반드시 전송 도중에 도착한다
        server.set_hang(true);
        connection.send(ConnCommand::Transfer(TransferRequest {
            id: TransferId(1),
            direction: TransferDirection::Download,
            remote: RemotePath::new("/big.bin"),
            local: local.clone(),
            offset: 0,
            remote_size: 0,
        }));
        wait_until(Duration::from_secs(2), || {
            server.calls().iter().any(|name| name == "download")
        });
        connection.cancel_transfer(TransferId(1));
        server.set_hang(false);

        let events = wait_events(&mut connection, 1, Duration::from_secs(3));
        let cancelled = events.iter().any(|event| {
            matches!(
                event,
                ConnEvent::TransferDone {
                    result: Err(RemoteError::Cancelled),
                    ..
                }
            )
        });
        assert!(cancelled, "취소로 끝나야 한다: {events:?}");

        drop(connection);
        let _ = std::fs::remove_file(local);
    }

    #[test]
    fn 재시도를_기다리는_중에_닫아도_곧바로_회수된다() {
        // 백오프 대기는 기본값이 최대 30초다 — 그 사이 앱을 닫았는데 워커가 소켓을 쥔 채
        // 그만큼 남으면, 사용자에게는 종료가 걸린 것으로 보인다
        let server = FakeServer::new();
        server.fail_connects(99);
        let slow_retry = RetryPolicy {
            base: Duration::from_secs(5),
            max: Duration::from_secs(5),
            attempts: 5,
        };
        let started = Instant::now();
        {
            let connection = spawn(&server, slow_retry);
            connection.send(ConnCommand::Connect);
            // 첫 시도가 실패해 대기에 들어갈 때까지 기다린다
            wait_until(Duration::from_secs(2), || {
                server.calls().iter().any(|name| name == "connect")
            });
        }
        wait_until(Duration::from_secs(3), || server.live_sessions() == 0);
        assert_eq!(server.live_sessions(), 0, "워커가 회수되지 않았다");
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "백오프 대기(5초)를 다 기다린 뒤에야 끝났다: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn 워커가_죽은_뒤의_명령은_조용히_무시된다() {
        let server = FakeServer::new();
        let mut connection = spawn(&server, fast_retry());
        connection.send(ConnCommand::Stop);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !connection.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(connection.is_finished(), "워커가 끝나야 한다");
        // 워커가 사라지면 수신부도 함께 사라져 `send`가 실패한다 —
        // 호출부는 이 `false`를 보고 조용히 넘어간다(오류 대화를 띄우지 않는다)
        assert!(!connection.send(ConnCommand::Disconnect));
        assert!(connection.poll().is_empty());
    }

    #[test]
    fn 파일_작업_명령이_세션에_닿고_답이_돌아온다() {
        // T23 Acceptance ② — 이름 바꾸기·새 폴더가 세션까지 전달되고 성공 응답이 온다.
        // 목록을 다시 읽는 것은 그 응답을 받은 화면 쪽 몫이다(`op_outcome`이 판정한다)
        let server = FakeServer::new();
        server.set_entries("/pub", vec![fake_entry("낡은.txt", false)]);
        let mut connection = spawn(&server, fast_retry());
        connection.send(ConnCommand::Connect);
        connection.send(ConnCommand::Rename {
            from: RemotePath::new("/pub/낡은.txt"),
            to: RemotePath::new("/pub/새.txt"),
        });
        connection.send(ConnCommand::Mkdir(RemotePath::new("/pub/새폴더")));

        let mut done = Vec::new();
        wait_until(Duration::from_secs(3), || {
            done.extend(
                connection
                    .poll()
                    .into_iter()
                    .filter_map(|event| match event {
                        ConnEvent::OpDone { op, result } => Some((op, result)),
                        _ => None,
                    }),
            );
            done.len() >= 2
        });
        assert_eq!(
            done,
            vec![(OpKind::Rename, Ok(())), (OpKind::Mkdir, Ok(()))],
            "명령 순서대로 성공 응답이 와야 한다"
        );
        let calls = server.calls();
        assert!(
            calls.contains(&"rename".to_owned()) && calls.contains(&"mkdir".to_owned()),
            "세션이 받은 명령: {calls:?}"
        );
    }

    #[test]
    fn 권한_바꾸기를_모르는_서버여도_연결은_이어진다() {
        // T23 Acceptance ④ — SITE CHMOD를 모르는 FTP 서버는 흔하다(D22). 사유만 돌아오고
        // 워커는 그대로 살아 다음 명령을 처리한다
        let server = FakeServer::new();
        server.set_entries("/pub", Vec::new());
        server.set_chmod_unsupported(true);
        let mut connection = spawn(&server, fast_retry());
        connection.send(ConnCommand::Connect);
        connection.send(ConnCommand::Chmod {
            path: RemotePath::new("/pub/설정.ini"),
            mode: 0o644,
        });
        connection.send(ConnCommand::Mkdir(RemotePath::new("/pub/다음")));

        let mut done = Vec::new();
        wait_until(Duration::from_secs(3), || {
            done.extend(
                connection
                    .poll()
                    .into_iter()
                    .filter_map(|event| match event {
                        ConnEvent::OpDone { op, result } => Some((op, result)),
                        _ => None,
                    }),
            );
            done.len() >= 2
        });
        let (op, result) = done
            .first()
            .cloned()
            .expect("권한 바꾸기의 답이 오지 않았다");
        assert_eq!(op, OpKind::Chmod);
        let err = result.expect_err("지원하지 않는데 성공으로 왔다");
        assert!(
            matches!(&err, RemoteError::Unsupported { operation, .. } if operation == "SITE CHMOD"),
            "{err}"
        );
        assert_eq!(
            done.get(1).map(|(op, result)| (*op, result.is_ok())),
            Some((OpKind::Mkdir, true)),
            "실패 뒤에도 워커가 다음 명령을 처리해야 한다"
        );
    }

    #[test]
    fn 조회가_실패하면_어느_요청인지_함께_알린다() {
        // T24 Edge Case — 실패를 로그에만 남기면 답을 기다리던 트리가 영영 `읽는 중…`에 머문다
        let server = FakeServer::new();
        // `/없는곳`은 심지 않는다 — 가짜 서버가 "없는 폴더"로 답한다
        let mut connection = spawn(&server, fast_retry());
        connection.send(ConnCommand::Connect);
        connection.send(ConnCommand::List {
            source: ListSource::Tree { seq: 77 },
            path: RemotePath::new("/없는곳"),
            quiet: false,
        });

        let mut failed = None;
        wait_until(Duration::from_secs(3), || {
            for event in connection.poll() {
                if let ConnEvent::ListFailed { source, detail } = event {
                    failed = Some((source, detail));
                }
            }
            failed.is_some()
        });
        let (source, detail) = failed.expect("실패 알림이 오지 않았다");
        assert_eq!(
            source,
            ListSource::Tree { seq: 77 },
            "어느 요청이 실패했는지 알 수 없다"
        );
        assert!(!detail.is_empty(), "사유가 비어 있다");
    }

    #[test]
    fn 티엘에스가_거부되면_암호화된_연결로_보지_않는다() {
        // F-7 리뷰 B1 — 설정이 `ExplicitIfAvailable`이어도 서버가 거부하면 평문으로 간다.
        // 그때 화면이 `· TLS`를 적으면 **거짓 보안 표시**가 된다
        let 거부하는_서버 = FakeServer::new();
        거부하는_서버.set_refuse_tls(true);
        let mut 평문 = spawn(&거부하는_서버, fast_retry());
        평문.send(ConnCommand::Connect);
        wait_until(Duration::from_secs(3), || {
            평문.poll();
            matches!(평문.phase(), ConnPhase::Ready)
        });
        assert!(!평문.is_secure(), "평문 연결을 암호화됐다고 보았다");

        // 받아 주는 서버에서는 그대로 참이다
        let 받아주는_서버 = FakeServer::new();
        let mut 암호화 = spawn(&받아주는_서버, fast_retry());
        암호화.send(ConnCommand::Connect);
        wait_until(Duration::from_secs(3), || {
            암호화.poll();
            matches!(암호화.phase(), ConnPhase::Ready)
        });
        assert!(암호화.is_secure(), "암호화된 연결인데 아니라고 보았다");
    }

    #[test]
    fn 연결_전과_끊긴_뒤에는_암호화로_보지_않는다() {
        // 연결이 서기 전에는 물론, **끊기거나 실패한 뒤에도** 암호화된 것으로 보면 안 된다
        // (F-7 2라운드 m4 — 참으로 남으면 새 호출부가 생겼을 때 조용히 거짓 표시가 된다)
        let server = FakeServer::new();
        let mut connection = spawn(&server, fast_retry());
        assert!(!connection.is_secure(), "연결 전인데 암호화로 보았다");

        connection.send(ConnCommand::Connect);
        wait_until(Duration::from_secs(3), || {
            connection.poll();
            matches!(connection.phase(), ConnPhase::Ready)
        });
        assert!(connection.is_secure(), "선 연결을 암호화가 아니라고 보았다");

        // 연결을 접으면 다시 거짓이 된다
        connection.send(ConnCommand::Disconnect);
        wait_until(Duration::from_secs(3), || {
            connection.poll();
            matches!(connection.phase(), ConnPhase::Closed)
        });
        assert!(!connection.is_secure(), "끊긴 연결을 암호화로 보았다");
    }

    #[test]
    fn 연결_진행이_상태_줄로_남는다() {
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        // 사용자 요청(2026-08-05): 로그 화면에 "어디까지 갔는지"가 보여야 한다 —
        // 연결 시도 · 연결 수립 · 암호화 여부 · 로그인 · 목록 조회와 그 결과
        let server = FakeServer::new();
        server.set_entries("/", Vec::new());
        let mut connection = spawn(&server, fast_retry());
        connection.send(ConnCommand::Connect);
        connection.send(ConnCommand::List {
            source: ListSource::Tree { seq: 1 },
            path: RemotePath::root(),
            quiet: false,
        });
        let events = wait_for(&mut connection, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Listed { .. }))
        });

        let lines: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                ConnEvent::Log {
                    kind: LogKind::Status,
                    text,
                } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            lines.iter().any(|line| line.ends_with("에 연결…")),
            "연결 시도가 남지 않았다: {lines:?}"
        );
        // 기대값은 **원문 리터럴**이다 — 카탈로그를 다시 부르면 그 값이 무엇으로
        // 바뀌어도 통과해, 문구를 지키라고 있는 시험이 아무것도 지키지 못한다
        assert!(
            lines.contains(&"연결 수립, 환영 메시지를 기다림…"),
            "{lines:?}"
        );
        assert!(lines.contains(&"TLS로 암호화된 연결입니다."), "{lines:?}");
        assert!(lines.contains(&"로그인…"), "{lines:?}");
        assert!(lines.contains(&"로그인 완료"), "{lines:?}");
        assert!(
            lines.iter().any(|line| line.contains("목록 조회…")),
            "조회 시작이 남지 않았다: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains("목록 조회 성공")),
            "조회 성공이 남지 않았다: {lines:?}"
        );
    }

    #[test]
    fn 암호화되지_않은_연결은_그_사실을_알린다() {
        // 이미지의 `보안되지 않은 서버입니다…` 줄 — 평문으로 떨어진 것을 사용자가 알아야 한다
        let server = FakeServer::new();
        server.set_refuse_tls(true);
        let mut connection = spawn(&server, fast_retry());
        connection.send(ConnCommand::Connect);
        let events = wait_for(&mut connection, Duration::from_secs(3), |events| {
            events.contains(&ConnEvent::Phase(ConnPhase::Ready))
        });
        let lines: Vec<&str> = events
            .iter()
            .filter_map(|event| match event {
                ConnEvent::Log { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            lines.contains(&"암호화되지 않은 연결입니다. 이 서버는 TLS를 지원하지 않습니다."),
            "{lines:?}"
        );
        assert!(!lines.contains(&"TLS로 암호화된 연결입니다."), "{lines:?}");
    }
}
