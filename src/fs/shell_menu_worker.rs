//! 셸 컨텍스트 메뉴 전용 STA 워커 (FR-8).
//!
//! 셸 메뉴를 여는 일(`QueryContextMenu` — 설치된 확장 DLL을 그 자리에서 싣는다)은 이 PC
//! 실측으로 **매번 0.27~0.3초, 프로세스 첫 회 1.8초**, 하위 메뉴를 채우는 일(`WM_INITMENUPOPUP`)은
//! 0.1~0.4초다(2026-09-29). UI 스레드에서 돌리면 그만큼 창이 멈춘다. 그래서 **이 스레드 하나가
//! `IContextMenu`와 그 `HMENU`를 쥐고** 열기·펼치기·실행을 도맡고, 화면은 요청을 보내고 응답을
//! 채널로 거둔다.
//!
//! **이 스레드가 쥐는 이유**: `IContextMenu`는 만든 아파트(STA)에 묶여 다른 스레드에서 부를 수
//! 없다. 그리고 STA는 메시지를 펌프해야 확장이 보낸 창 메시지·COM 호출이 처리된다.
//!
//! **깨우기는 자동 리셋 이벤트다** — `PostThreadMessageW`는 큐가 생기기 전 게시가 실패하고,
//! 이 스레드가 `InvokeCommand`가 띄운 모달 대화 안에 있으면 스레드 메시지가 버려진다. 이벤트는
//! 세워진 채 남아 신호를 잃지 않는다.
//!
//! **첫 열기가 확장을 싣는다**(첫 회 최대 1.8초 — 그동안 메뉴는 뼈대로 떠 있다). 앱 시작 때
//! 미리 싣지 않는 것은 우클릭하지 않는 세션이 +42MB·CPU 1.8초를 치르지 않게 하려는 것이다
//! (2026-09-29 자원 검토 뒤 사용자 선택).
//!
//! 이 모듈은 UI를 모른다 — 결과는 채널로만 내보낸다(`fs` 계층 규칙).
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
    PeekMessageW, QS_ALLINPUT, TranslateMessage,
};

use crate::fs::shell_menu::{ShellMenu, ShellMenuItem, SubmenuHandle};

/// 워커에게 보내는 요청.
///
/// `ticket`은 부르는 쪽이 메뉴 한 판마다 새로 매기는 번호다 — 워커는 지금 쥔 메뉴의 번호와
/// 다른 펼치기·실행을 무시하고, 부르는 쪽은 지금 열린 메뉴의 번호와 다른 응답을 버린다
pub enum Request {
    Open {
        ticket: u64,
        folder: PathBuf,
        items: Vec<PathBuf>,
    },
    Expand {
        ticket: u64,
        handle: SubmenuHandle,
    },
    Invoke {
        ticket: u64,
        id: u32,
    },
    Close {
        ticket: u64,
    },
    Stop,
}

/// 워커가 돌려주는 응답
#[derive(Debug)]
pub enum Response {
    /// 메뉴를 열었다 — `verbs`는 `items`와 1:1이다(구분선·verb 없는 줄은 `None`).
    ///
    /// verb를 여기서 함께 읽는 것은 **그것도 `IContextMenu`를 불러야 해서**다 — 화면이
    /// 나중에 묻게 두면 그 질문이 다시 이 스레드까지 와야 한다
    Opened {
        ticket: u64,
        items: Vec<ShellMenuItem>,
        verbs: Vec<Option<String>>,
    },
    OpenFailed {
        ticket: u64,
    },
    Expanded {
        ticket: u64,
        handle: SubmenuHandle,
        items: Vec<ShellMenuItem>,
    },
}

/// 깨우기 이벤트 — 두 스레드가 함께 쥐고, 마지막 쪽이 놓을 때 닫는다.
///
/// `HANDLE`은 `Send`가 아니라 값(`isize`)으로 든다. 닫는 자리가 `Drop` 하나라, 한쪽이 먼저
/// 닫아 다른 쪽이 재사용된 핸들을 세우는 일이 없다
struct WakeEvent(isize);

impl WakeEvent {
    fn new() -> Option<WakeEvent> {
        // 안전성: 이름 없는 자동 리셋 이벤트를 만든다 — 닫는 것은 `Drop` 하나다
        let handle = unsafe { CreateEventW(None, false, false, None) }.ok()?;
        Some(WakeEvent(handle.0 as isize))
    }

    fn handle(&self) -> HANDLE {
        HANDLE(self.0 as *mut core::ffi::c_void)
    }

    fn set(&self) {
        // 안전성: `Drop` 전까지 살아 있는 이벤트 핸들이다
        unsafe {
            let _ = SetEvent(self.handle());
        }
    }
}

impl Drop for WakeEvent {
    fn drop(&mut self) {
        // 안전성: `new`가 만든 핸들을 한 번만 닫는다(마지막 `Arc`가 놓일 때)
        unsafe {
            let _ = CloseHandle(self.handle());
        }
    }
}

/// 워커 손잡이 — 요청을 보내고 응답을 거둔다. 버리면 워커가 끝난다.
///
/// **`Drop`은 스레드를 기다리지 않는다** — 셸 확장이 그 스레드를 붙잡고 있으면(모달 대화 등)
/// 기다리는 쪽인 UI가 함께 멎는다. 끝내라는 신호만 보내고 떠난다
pub struct ShellMenuWorker {
    tx: Sender<Request>,
    rx: Receiver<Response>,
    wake: Arc<WakeEvent>,
    #[cfg(test)]
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ShellMenuWorker {
    /// 워커를 띄운다 — `owner`는 셸 대화가 붙을 창의 핸들 값이다(0이면 소유 창 없음).
    ///
    /// 이벤트·스레드를 만들지 못하면 `None`이며 부르는 쪽은 셸 메뉴를 쓰지 못한다
    pub fn spawn(owner: isize) -> Option<ShellMenuWorker> {
        let wake = Arc::new(WakeEvent::new()?);
        let (request_tx, request_rx) = channel::<Request>();
        let (response_tx, response_rx) = channel::<Response>();
        let worker_wake = Arc::clone(&wake);
        let thread = std::thread::Builder::new()
            .name("shell-menu".into())
            .spawn(move || run(owner, &request_rx, &response_tx, &worker_wake))
            .ok()?;
        #[cfg(not(test))]
        drop(thread);
        Some(ShellMenuWorker {
            tx: request_tx,
            rx: response_rx,
            wake,
            #[cfg(test)]
            thread: Some(thread),
        })
    }

    /// 메뉴를 연다 — `items`가 비면 `folder`의 배경 메뉴다
    pub fn open(&self, ticket: u64, folder: PathBuf, items: Vec<PathBuf>) {
        self.send(Request::Open {
            ticket,
            folder,
            items,
        });
    }

    /// 그 메뉴의 하위 메뉴를 채워 달라고 한다
    pub fn expand(&self, ticket: u64, handle: SubmenuHandle) {
        self.send(Request::Expand { ticket, handle });
    }

    /// 그 메뉴의 항목을 실행한다 — 실행 뒤 워커는 그 메뉴를 놓는다
    pub fn invoke(&self, ticket: u64, id: u32) {
        self.send(Request::Invoke { ticket, id });
    }

    /// 그 메뉴를 놓는다 — 메모리를 일찍 돌려주는 것일 뿐, 보내지 않아도 다음 열기가 놓는다
    pub fn close(&self, ticket: u64) {
        self.send(Request::Close { ticket });
    }

    /// 도착한 응답 하나 — 없으면 `None`
    pub fn try_recv(&self) -> Option<Response> {
        self.rx.try_recv().ok()
    }

    /// 보내고 깨운다. 워커가 이미 끝났으면(앱 종료 중) 전송 실패는 무해하다
    fn send(&self, request: Request) {
        let _ = self.tx.send(request);
        self.wake.set();
    }

    #[cfg(test)]
    fn take_thread(&mut self) -> Option<std::thread::JoinHandle<()>> {
        self.thread.take()
    }
}

impl Drop for ShellMenuWorker {
    fn drop(&mut self) {
        self.send(Request::Stop);
    }
}

/// 한 번에 비운 요청들에서 **마지막 열기만** 남긴다 — 나머지 순서는 그대로다.
///
/// 빠르게 거듭 우클릭하면 0.3~1.8초짜리 열기가 줄을 선다. 앞의 것은 어차피 곧 버려질 메뉴라
/// 열 까닭이 없다. 옛 번호의 펼치기·실행은 남겨도 워커가 번호로 걸러 무시한다
fn coalesce(batch: Vec<Request>) -> Vec<Request> {
    let last_open = batch
        .iter()
        .rposition(|request| matches!(request, Request::Open { .. }));
    batch
        .into_iter()
        .enumerate()
        .filter(|(index, request)| {
            !matches!(request, Request::Open { .. }) || Some(*index) == last_open
        })
        .map(|(_, request)| request)
        .collect()
}

/// 워커 스레드 본체 — 요청을 기다리며 메시지를 펌프한다.
///
/// 한 바퀴는 ① 쌓인 창 메시지를 처리하고 ② 쌓인 요청을 한 번에 비워 처리한 뒤 ③ 이벤트나
/// 새 메시지가 올 때까지 잔다. ①이 ③보다 앞인 것은 `MWMO_INPUTAVAILABLE` 없이 자면 이미
/// 큐에 있던 메시지로는 깨지 않기 때문이다
fn run(owner: isize, requests: &Receiver<Request>, responses: &Sender<Response>, wake: &WakeEvent) {
    // 안전성: 이 스레드에서 초기화하고 **성공했을 때만** 같은 스레드에서 해제한다 — 실패한
    // 초기화를 짝지어 해제하면 COM 참조 수가 어긋난다
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    let owner = HWND(owner as *mut core::ffi::c_void);
    let mut current: Option<(u64, ShellMenu)> = None;
    'outer: loop {
        pump_messages();
        let mut batch = Vec::new();
        loop {
            match requests.try_recv() {
                Ok(request) => batch.push(request),
                Err(TryRecvError::Empty) => break,
                // 손잡이가 사라졌다 — 끝낸다
                Err(TryRecvError::Disconnected) => break 'outer,
            }
        }
        for request in coalesce(batch) {
            if !handle(request, owner, &mut current, responses) {
                break 'outer;
            }
        }
        wait_for_work(wake);
    }
    // 메뉴는 COM을 해제하기 전에 놓아야 한다
    drop(current);
    if initialized {
        // 안전성: 위에서 성공한 초기화와 같은 스레드에서 1회 호출
        unsafe { CoUninitialize() };
    }
}

/// 요청 하나를 처리한다 — 끝내야 하면 거짓.
///
/// 지금 쥔 메뉴와 번호가 다른 펼치기·실행·놓기는 무시한다(그 메뉴는 이미 버려졌다)
fn handle(
    request: Request,
    owner: HWND,
    current: &mut Option<(u64, ShellMenu)>,
    responses: &Sender<Response>,
) -> bool {
    let is_current = |current: &Option<(u64, ShellMenu)>, ticket: u64| {
        current.as_ref().is_some_and(|(held, _)| *held == ticket)
    };
    let response = match request {
        Request::Stop => return false,
        Request::Open {
            ticket,
            folder,
            items,
        } => {
            // 새 메뉴를 열기 전에 옛 것을 놓는다 — 한 번에 하나만 쥔다
            *current = None;
            match ShellMenu::open(owner, &folder, &items) {
                Some(menu) => {
                    let items = menu.model();
                    let verbs = items.iter().map(|item| menu.verb(item.id)).collect();
                    *current = Some((ticket, menu));
                    Response::Opened {
                        ticket,
                        items,
                        verbs,
                    }
                }
                None => Response::OpenFailed { ticket },
            }
        }
        Request::Expand { ticket, handle } => {
            let Some((_, menu)) = current.as_ref().filter(|_| is_current(current, ticket)) else {
                return true;
            };
            Response::Expanded {
                ticket,
                handle,
                items: menu.expand(handle),
            }
        }
        Request::Invoke { ticket, id } => {
            if is_current(current, ticket)
                && let Some((_, menu)) = current.take()
            {
                menu.invoke(id, owner);
            }
            return true;
        }
        Request::Close { ticket } => {
            if is_current(current, ticket) {
                *current = None;
            }
            return true;
        }
    };
    // 받는 쪽이 사라졌어도(앱 종료 중) 다음 바퀴에서 요청 채널 끊김으로 끝난다
    let _ = responses.send(response);
    true
}

/// 쌓인 창 메시지를 모두 처리한다 — 셸 확장이 이 스레드에 만든 창이 그것을 받는다
fn pump_messages() {
    let mut msg = MSG::default();
    // 안전성: 스택의 `MSG` 하나를 채워 이 스레드의 큐를 비운다
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 요청 이벤트나 새 창 메시지가 올 때까지 잔다
fn wait_for_work(wake: &WakeEvent) {
    let handles = [wake.handle()];
    // 안전성: 살아 있는 이벤트 핸들 하나를 기다린다(`wake`가 이 호출 동안 살아 있다)
    unsafe {
        let _ =
            MsgWaitForMultipleObjectsEx(Some(&handles), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// 응답 하나를 시한까지 기다린다
    fn wait(worker: &ShellMenuWorker, limit: Duration) -> Option<Response> {
        let start = Instant::now();
        while start.elapsed() < limit {
            if let Some(response) = worker.try_recv() {
                return Some(response);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    /// 시험마다 다른 임시 폴더 — 병렬 시험이 서로의 파일을 지우지 않게
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("moa_menu_worker_{name}"));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn open_kind(request: &Request) -> Option<u64> {
        match request {
            Request::Open { ticket, .. } => Some(*ticket),
            _ => None,
        }
    }

    #[test]
    fn 쌓인_열기는_마지막_것만_남는다() {
        let open = |ticket| Request::Open {
            ticket,
            folder: PathBuf::new(),
            items: Vec::new(),
        };
        let kept = coalesce(vec![
            open(1),
            Request::Close { ticket: 1 },
            open(2),
            open(3),
        ]);
        let opens: Vec<u64> = kept.iter().filter_map(open_kind).collect();
        assert_eq!(opens, vec![3]);
        // 열기가 아닌 요청은 버리지 않는다
        assert!(
            kept.iter()
                .any(|r| matches!(r, Request::Close { ticket: 1 }))
        );
    }

    #[test]
    fn 띄운_직후_보낸_열기에도_셸_항목이_온다() {
        let dir = temp_dir("open");
        let file = dir.join("보기.txt");
        let _ = std::fs::write(&file, b"worker");
        let worker = ShellMenuWorker::spawn(0).expect("워커를 띄운다");
        worker.open(7, dir.clone(), vec![file]);
        match wait(&worker, Duration::from_secs(10)) {
            Some(Response::Opened {
                ticket,
                items,
                verbs,
            }) => {
                assert_eq!(ticket, 7);
                assert!(!items.is_empty());
                assert_eq!(verbs.len(), items.len());
            }
            other => panic!("셸 항목이 와야 한다: {other:?}"),
        }
    }

    #[test]
    fn 배경_메뉴의_하위_메뉴를_채운다() {
        let dir = temp_dir("expand");
        let worker = ShellMenuWorker::spawn(0).expect("워커를 띄운다");
        worker.open(1, dir, Vec::new());
        let Some(Response::Opened { items, .. }) = wait(&worker, Duration::from_secs(10)) else {
            panic!("배경 메뉴가 와야 한다");
        };
        let handles: Vec<SubmenuHandle> = items.iter().filter_map(|it| it.submenu).collect();
        assert!(
            !handles.is_empty(),
            "배경 메뉴에는 `새로 만들기` 하위 메뉴가 있다"
        );
        // **모두 펼친다** — 확장이 붙인 하위 메뉴가 먼저 와도 판정이 서게
        for handle in &handles {
            worker.expand(1, *handle);
        }
        let mut filled = false;
        for want in &handles {
            match wait(&worker, Duration::from_secs(10)) {
                Some(Response::Expanded {
                    ticket,
                    handle,
                    items,
                }) => {
                    assert_eq!(ticket, 1);
                    assert!(handles.contains(&handle), "{want:?} 가운데 하나여야 한다");
                    filled |= !items.is_empty();
                }
                other => panic!("펼친 결과가 와야 한다: {other:?}"),
            }
        }
        assert!(filled, "`새로 만들기`는 줄이 차 있다");
    }

    #[test]
    fn 표가_다른_펼침에는_답하지_않는다() {
        let dir = temp_dir("stale");
        let worker = ShellMenuWorker::spawn(0).expect("워커를 띄운다");
        worker.open(1, dir, Vec::new());
        let Some(Response::Opened { items, .. }) = wait(&worker, Duration::from_secs(10)) else {
            panic!("배경 메뉴가 와야 한다");
        };
        let Some(handle) = items.iter().find_map(|it| it.submenu) else {
            panic!("하위 메뉴가 있어야 한다");
        };
        worker.expand(2, handle);
        assert!(wait(&worker, Duration::from_secs(1)).is_none());
    }

    #[test]
    fn 손잡이를_버리면_스레드가_끝난다() {
        let mut worker = ShellMenuWorker::spawn(0).expect("워커를 띄운다");
        let thread = worker.take_thread().expect("스레드 손잡이");
        drop(worker);
        let start = Instant::now();
        while !thread.is_finished() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(thread.is_finished());
    }
}
