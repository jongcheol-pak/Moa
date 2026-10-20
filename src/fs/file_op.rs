//! 로컬 파일·폴더 복사·이름 바꾸기·삭제 (FR-60·FR-61·FR-64) — **Windows 셸에 맡긴다**.
//!
//! 직접 재귀 복사를 만들지 않고 `IFileOperation`을 부르는 이유가 이 모듈의 전부다:
//! 진행률 대화·같은 이름 충돌 대화·되돌리기(`Ctrl+Z`)·권한 승격·긴 경로·심볼릭 링크를
//! 전부 OS가 처리한다. PRD의 「자체 파일 작업 UI는 셸에 위임」 원칙과 같은 방향이며,
//! 그래서 로컬끼리의 복사에는 FR-55의 자체 확인 대화를 띄우지 않는다(셸이 자기 대화로 묻는다).
//!
//! **일은 워커 스레드에서 한다** — `PerformOperations`는 걸어 둔 작업이 끝날 때까지
//! 돌아오지 않아, UI 스레드에서 부르면 대용량 복사·수백 항목 삭제 내내 앱이 굳는다
//! (AGENTS: UI 스레드 블로킹 I/O 금지). 그 골격은 `spawn_shell_op` 한 곳에 있다.
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::Win32::UI::Shell::{
    FILEOPERATION_FLAGS, FOF_ALLOWUNDO, FOF_NOCONFIRMMKDIR, FileOperation, IFileOperation,
    IFileOperationProgressSink, IFileOperationProgressSink_Impl, IShellItem,
    SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
};
use windows::core::{HRESULT, HSTRING, PCWSTR, Ref, implement};

use crate::fs::undo::{Stamp, stamp_of};

/// 셸 작업 하나가 실제로 해낸 것 — 원래 자리와 그 뒤의 자리 (FR-74).
///
/// `to`는 **셸이 알려 준 실제 자리**다 — 같은 이름 충돌 대화에서 「둘 다 유지」를 고르면
/// `(2)`가 붙어 우리가 건 이름과 달라진다. `preexisting`은 작업 직전에 그 자리에 이미
/// 무언가 있었는가(덮어쓰기·폴더 병합)이며, 그런 항목은 되돌리면 원래 있던 것까지 잃는다
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub from: PathBuf,
    pub to: PathBuf,
    /// 작업 직후 `to`의 도장 — 워커가 잰다(UI 스레드에서 파일시스템을 묻지 않는다)
    pub stamp: Option<Stamp>,
    pub preexisting: bool,
}

/// 복사·이동 한 번의 결과 — 화면이 알릴 것과 되돌리기가 기록할 것을 담는다.
///
/// 몇 개가 실제로 갔는지는 사용자가 충돌 대화에서 무엇을 골랐는지에 달려 셸만 안다 —
/// 그것은 `applied`가 셸의 진행 통지로 받아 온다
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyOutcome {
    /// 걸었던 항목 수 — 알림 문구가 규모를 적는 데 쓴다
    pub requested: usize,
    /// 복사였나 이동이었나 — 알림 문구와 되돌리기 방식이 갈린다
    pub how: Transfer,
    /// 사용자가 셸 대화에서 그만뒀는가. **오류가 아니다**
    pub cancelled: bool,
    /// 셸이 준 실패 사유 — 없으면 탈 없이 끝났다
    pub error: Option<String>,
    /// 실제로 옮겨지거나 생긴 것 — 도중에 그만뒀거나 실패했어도 된 것은 실린다
    pub applied: Vec<Applied>,
}

/// 워커를 깨워 화면을 다시 그리게 하는 손잡이 — `remote::connection::Wake`와 같은 모양이다
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// HWND를 워커 스레드로 넘기기 위한 래퍼.
///
/// 안전성: 핸들 값 자체는 스레드 간 옮겨도 무해하고, **다른 스레드가 만든 창을
/// `SetOwnerWindow`에 주는 것은 `IFileOperation`을 워커에서 돌릴 때의 표준 방식**이다 —
/// 진행률 대화는 이 워커 스레드에서 뜨고 소유자만 UI 스레드의 창을 가리킨다. 입력 큐를
/// 붙이지(`AttachThreadInput`) 않고, UI 스레드도 이 워커를 기다리지 않아(결과는 채널로
/// 온다) 서로 막을 자리가 없다. 창이 이미 파괴됐으면 셸이 소유자 없이 띄운다.
///
/// **`fs::enumerate`의 같은 이름 래퍼와는 근거가 다르다** — 그쪽은 `PostMessageW`가
/// 어느 스레드에서나 안전하다는 그 API의 성질이 근거다
struct HwndSend(isize);
// 안전성: 위 주석 참조
unsafe impl Send for HwndSend {}

/// `sources`를 `dest` 폴더로 복사한다 — **곧바로 돌아오고** 결과는 `done`으로 온다 (FR-60).
///
/// 이동이 아니라 언제나 복사다(FR-60 — 보조키로 가르지 않는다). 같은 이름이 이미 있으면
/// 셸이 자기 대화로 묻는다.
///
/// `sources`가 비면 워커를 띄우지 않고 `false`를 돌려준다 — 부르는 쪽이 헛되이 결과를
/// 기다리지 않게 한다
pub fn copy_into(
    dest: PathBuf,
    sources: Vec<PathBuf>,
    owner: HWND,
    done: Sender<CopyOutcome>,
    wake: Wake,
) -> bool {
    start_transfer(dest, sources, Transfer::Copy, owner, done, wake)
}

/// `sources`를 `dest` 폴더로 **옮긴다** — 곧바로 돌아오고 결과는 `done`으로 온다 (FR-64).
///
/// 잘라낸 것을 붙여넣는 길이다. `copy_into`와 갈라 두는 이유는 뜻이 다르기 때문이며
/// (저쪽은 드래그 복사 FR-60, 이쪽은 클립보드 FR-64), 실제로 갈리는 것은 셸에 거는 명령
/// 하나(`CopyItem` ↔ `MoveItem`)다 — 그것을 `perform`이 인자로 받는다.
///
/// `sources`가 비면 워커를 띄우지 않고 `false`를 돌려준다(`copy_into`와 같은 계약)
pub fn move_into(
    dest: PathBuf,
    sources: Vec<PathBuf>,
    owner: HWND,
    done: Sender<CopyOutcome>,
    wake: Wake,
) -> bool {
    start_transfer(dest, sources, Transfer::Move, owner, done, wake)
}

/// 복사·이동의 공통 진입 — 워커를 띄우고 결과를 `CopyOutcome`으로 조립한다
fn start_transfer(
    dest: PathBuf,
    sources: Vec<PathBuf>,
    how: Transfer,
    owner: HWND,
    done: Sender<CopyOutcome>,
    wake: Wake,
) -> bool {
    if sources.is_empty() {
        return false;
    }
    let owner = HwndSend(owner.0 as isize);
    let requested = sources.len();
    spawn_shell_op(done, wake, move || {
        let ran = perform(&dest, &sources, &owner, how, TRANSFER_FLAGS);
        CopyOutcome {
            requested,
            how,
            cancelled: ran.cancelled,
            error: ran.error,
            applied: ran.applied,
        }
    })
}

/// 복사·이동에 거는 셸 플래그.
///
/// `FOF_ALLOWUNDO`는 셸 쪽 되돌리기 기록에도 남게 한다. `FOF_NOCONFIRMMKDIR`는 대상 폴더를
/// 만들 때만 묻지 않는 것이라, 같은 이름 충돌 확인은 그대로 뜬다(FR-60이 셸에 맡긴 그 대화다)
const TRANSFER_FLAGS: FILEOPERATION_FLAGS =
    FILEOPERATION_FLAGS(FOF_ALLOWUNDO.0 | FOF_NOCONFIRMMKDIR.0);

/// 셸에 걸 전송 방식 — 복사인가 이동인가.
///
/// 불리언 대신 두는 이유: 호출부에서 `perform(dest, sources, owner, true)`가 무엇의
/// 참인지 읽히지 않는다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    Copy,
    Move,
}

/// 셸 작업 한 번을 돌린 결과 — 걸기 전 실패도, 도중 실패도 여기 담긴다
struct Performed {
    cancelled: bool,
    error: Option<String>,
    applied: Vec<Applied>,
}

impl Performed {
    fn failed(error: String) -> Performed {
        Performed {
            cancelled: false,
            error: Some(error),
            applied: Vec::new(),
        }
    }
}

/// 실제 COM 호출.
///
/// **읽지 못하는 원본은 그것만 건너뛴다** — 여러 개를 끌어다 놓았는데 그 사이 하나가
/// 사라졌다고 나머지를 버릴 이유가 없다. 하나도 걸지 못했으면 실패로 본다.
///
/// **여기서 화면 문구를 만들지 않는다** — 이 층은 `ui`를 모르고(AGENTS 계층 규약), 셸이
/// 준 사유는 그대로 옮길 값이다
fn perform(
    dest: &Path,
    sources: &[PathBuf],
    owner: &HwndSend,
    how: Transfer,
    flags: FILEOPERATION_FLAGS,
) -> Performed {
    // 작업 직전에 대상 자리가 차 있었는가 — 덮어쓰기·병합을 가른다 (plan D1)
    let occupied: Vec<PathBuf> = sources
        .iter()
        .filter_map(|source| source.file_name().map(|name| dest.join(name)))
        .filter(|target| std::fs::symlink_metadata(target).is_ok())
        .collect();
    // 안전성: 아래 호출은 모두 COM이 STA로 초기화된 이 스레드에서만 돌고, 얻은 인터페이스는
    // 이 함수 안에서만 살다 `Drop`으로 해제된다. 경로 문자열은 호출이 끝날 때까지 지역 소유다
    let ran = unsafe {
        run_operation(flags, owner, sources, |op| {
            let folder: IShellItem = shell_item(dest)?;
            let mut queued = 0usize;
            for source in sources {
                let Ok(item) = shell_item(source) else {
                    // 그 사이 사라진 원본 — 나머지는 그대로 보낸다
                    continue;
                };
                // 사유를 만들지 않는다 — 이 실패는 그 항목만 건너뛰는 것이라 아무도 읽지 않는다.
                // 하나도 걸지 못한 경우의 사유는 아래에서 따로 만든다
                let queued_one = match how {
                    Transfer::Copy => op.CopyItem(&item, &folder, PCWSTR::null(), None),
                    Transfer::Move => op.MoveItem(&item, &folder, PCWSTR::null(), None),
                };
                if queued_one.is_ok() {
                    queued += 1;
                }
            }
            if queued == 0 {
                return Err(crate::i18n::copy_no_source().to_owned());
            }
            Ok(())
        })
    };
    finish(ran, |to| occupied.iter().any(|seen| same_path(seen, to)))
}

/// `PerformOperations`까지 간 작업 한 번 — 셸이 알려 준 (원래, 새) 자리를 함께 든다
struct Ran {
    cancelled: bool,
    error: Option<String>,
    pairs: Vec<(PathBuf, PathBuf)>,
}

/// 셸 작업 하나를 진행 통지와 함께 돌린다 — 항목을 거는 것은 `queue`가 한다.
///
/// 걸기 전 실패면 `Err`다. **도중에 실패해도 된 것은 버리지 않는다** — 되돌리기가 그것을
/// 기록해야 한다
///
/// 안전성: COM이 STA로 초기화된 스레드에서만 부른다
unsafe fn run_operation(
    flags: FILEOPERATION_FLAGS,
    owner: &HwndSend,
    sources: &[PathBuf],
    queue: impl FnOnce(&IFileOperation) -> Result<(), String>,
) -> Result<Ran, String> {
    // 안전성: 위 주석 참조
    unsafe {
        let op = new_operation(flags, owner)?;
        let done = Arc::new(Mutex::new(Vec::new()));
        let sink: IFileOperationProgressSink = ResultSink {
            sources: sources.to_vec(),
            done: done.clone(),
        }
        .into();
        // 통지를 걸지 못해도 작업 자체는 한다 — 그때는 되돌리기 기록만 비어 남는다
        let cookie = op.Advise(&sink).ok();
        queue(&op)?;
        let error = op.PerformOperations().err().map(|err| err.message());
        let cancelled = aborted(&op);
        if let Some(cookie) = cookie {
            let _ = op.Unadvise(cookie);
        }
        let pairs = done.lock().map(|pairs| pairs.clone()).unwrap_or_default();
        Ok(Ran {
            cancelled,
            error,
            pairs,
        })
    }
}

/// 작업 결과를 `Performed`로 접는다 — 도장은 여기(워커)서 잰다
fn finish(ran: Result<Ran, String>, was_occupied: impl Fn(&Path) -> bool) -> Performed {
    match ran {
        Err(error) => Performed::failed(error),
        Ok(Ran {
            cancelled,
            error,
            pairs,
        }) => Performed {
            cancelled,
            error,
            applied: pairs
                .into_iter()
                .map(|(from, to)| Applied {
                    stamp: stamp_of(&to),
                    preexisting: was_occupied(&to),
                    from,
                    to,
                })
                .collect(),
        },
    }
}

/// 두 경로가 같은 자리인가 — ASCII 대소문자를 접어 견준다(Windows 파일시스템 규칙)
fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

/// 셸의 진행 통지를 받아 **우리가 건 최상위 항목**이 실제로 간 자리를 모은다 (plan D2).
///
/// 폴더를 옮기면 그 안 항목의 통지도 올 수 있어 `sources`로 거른다. 실패한 항목(`hr` 실패)과
/// 새 자리를 알려 주지 않은 통지는 버린다
#[implement(IFileOperationProgressSink)]
struct ResultSink {
    sources: Vec<PathBuf>,
    done: Arc<Mutex<Vec<(PathBuf, PathBuf)>>>,
}

impl ResultSink_Impl {
    fn record(&self, item: Ref<IShellItem>, hr: HRESULT, created: Ref<IShellItem>) {
        if hr.is_err() {
            return;
        }
        let (Some(from), Some(to)) = (
            item.as_ref().and_then(item_path),
            created.as_ref().and_then(item_path),
        ) else {
            return;
        };
        if !self.sources.iter().any(|source| same_path(source, &from)) {
            return;
        }
        if let Ok(mut done) = self.done.lock() {
            done.push((from, to));
        }
    }
}

impl IFileOperationProgressSink_Impl for ResultSink_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _hr: HRESULT) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostRenameItem(
        &self,
        _: u32,
        item: Ref<IShellItem>,
        _: &PCWSTR,
        hr: HRESULT,
        created: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.record(item, hr, created);
        Ok(())
    }
    fn PreMoveItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostMoveItem(
        &self,
        _: u32,
        item: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        hr: HRESULT,
        created: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.record(item, hr, created);
        Ok(())
    }
    fn PreCopyItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostCopyItem(
        &self,
        _: u32,
        item: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        hr: HRESULT,
        created: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        self.record(item, hr, created);
        Ok(())
    }
    fn PreDeleteItem(&self, _: u32, _: Ref<IShellItem>) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostDeleteItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostNewItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: &PCWSTR,
        _: u32,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, _: u32, _: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

/// 셸 항목의 파일시스템 경로 — 가상 항목(경로가 없는 것)이면 `None`
fn item_path(item: &IShellItem) -> Option<PathBuf> {
    // 안전성: 유효한 셸 항목에만 부르고, 셸이 잡아 준 문자열은 읽은 뒤 곧바로 돌려준다
    unsafe {
        let raw = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = raw.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(raw.as_ptr().cast()));
        path
    }
}

/// 경로 하나를 셸 항목으로 바꾼다.
///
/// 안전성: COM이 초기화된 스레드에서만 부른다. `HSTRING`은 호출이 끝날 때까지 지역 소유다
fn shell_item(path: &Path) -> Result<IShellItem, String> {
    let wide = HSTRING::from(path.as_os_str());
    // 안전성: 위 주석 참조 — 유효한 널 종단 문자열 하나를 넘긴다
    unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).map_err(|err| err.message()) }
}

/// 이름 바꾸기·삭제 한 번의 결과 (FR-64).
///
/// 이름에 `File`을 붙인 것은 `ui::app::remote::OpOutcome`이 이미 있고 **뜻이 전혀 다르기**
/// 때문이다(그쪽은 원격 명령 뒤에 목록을 다시 읽을지 정하는 값이다).
///
/// **`CopyOutcome`과 필드가 같지만 합치지 않는다** — 바뀌는 이유가 다르다. 저쪽은 드래그
/// 복사(FR-60·FR-61)가 쥐고 있고 이쪽은 메뉴·단축키(FR-64)가 쥔다. 하나로 묶으면 한쪽
/// 요구가 바뀔 때마다 다른 쪽 호출부까지 흔들린다
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOpOutcome {
    /// 걸었던 항목 수 — 알림 문구가 규모를 적는 데 쓴다
    pub requested: usize,
    /// 사용자가 셸 대화에서 그만뒀는가. **오류가 아니다**
    pub cancelled: bool,
    /// 셸이 준 실패 사유 — 없으면 탈 없이 끝났다
    pub error: Option<String>,
    /// 실제로 이름이 바뀐 것 — 삭제는 늘 비어 있다(되돌리기 대상이 아니다)
    pub applied: Vec<Applied>,
}

/// 파일 이름에 쓸 수 없는 글자 — 셸에 걸기 전에 우리가 먼저 거른다.
///
/// 걸어 보고 실패를 받는 편이 짧지만, 그러면 셸이 자기 말로 알리는 오류 대화가 떠서
/// **어느 글자가 문제인지** 사용자가 알기 어렵다
const INVALID_NAME_CHARS: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

/// 바꿀 이름을 셸에 걸 수 있는가 — 걸 수 없으면 화면에 보일 사유를 돌려준다.
///
/// **예약된 장치 이름(`CON`·`NUL` 등)과 끝에 붙은 점·공백은 여기서 보지 않는다** —
/// 그것들은 글자 자체가 금지된 것이 아니라 자리에 따라 갈려, 판정을 흉내 내면 실제 규칙과
/// 어긋난 채 굳는다. 그런 이름은 셸이 자기 대화로 알린다
pub fn invalid_name_reason(new_name: &str) -> Option<String> {
    new_name
        .contains(INVALID_NAME_CHARS)
        .then(|| crate::i18n::rename_invalid_chars().to_owned())
}

/// 삭제에 줄 셸 플래그 (FR-64).
///
/// **휴지통으로 보내는 것이 기본**이고 `FOF_ALLOWUNDO`가 그것을 만든다 — 그 플래그를 빼면
/// 곧바로 지워진다. 확인 대화를 띄울지는 우리가 정하지 않는다(셸이 자기 정책을 따른다)
fn delete_flags(permanent: bool) -> FILEOPERATION_FLAGS {
    if permanent {
        FILEOPERATION_FLAGS(0)
    } else {
        FOF_ALLOWUNDO
    }
}

/// `path`의 이름을 `new_name`으로 바꾼다 — **곧바로 돌아오고** 결과는 `done`으로 온다 (FR-64).
///
/// 돌려주는 값은 "결과를 기다려야 하는가"다. **빈 이름이거나 지금 이름과 같으면 `false`** —
/// 아무 일도 하지 않고 결과도 보내지 않는다(사용자가 편집을 열었다 그대로 확정한 경우라
/// 알릴 것이 없다). 쓸 수 없는 글자가 있으면 워커를 띄우지 않고 **사유만** 보낸 뒤 `true`다
pub fn rename_item(
    path: PathBuf,
    new_name: String,
    owner: HWND,
    done: Sender<FileOpOutcome>,
    wake: Wake,
) -> bool {
    rename_many(vec![(path, new_name)], owner, done, wake)
}

/// 여러 항목의 이름을 한 번에 바꾼다 — 곧바로 돌아오고 결과는 `done`으로 온다 (FR-73).
///
/// `rename_item`과 같은 계약이다: 빈 이름·지금과 같은 이름은 빼고, 남는 것이 없으면 `false`.
/// 쓸 수 없는 글자가 **하나라도** 있으면 아무것도 걸지 않고 사유만 보낸다 — 일부만 바뀐
/// 묶음은 사용자가 예상한 결과가 아니다.
///
/// 서로의 지금 이름을 노리는 항목(`a`↔`b` 맞바꿈)은 임시 이름을 거쳐 두 번에 바꾼다 (plan D12)
pub fn rename_many(
    pairs: Vec<(PathBuf, String)>,
    owner: HWND,
    done: Sender<FileOpOutcome>,
    wake: Wake,
) -> bool {
    let pairs: Vec<(PathBuf, String)> = pairs
        .into_iter()
        .filter(|(path, name)| {
            !name.is_empty()
                && path
                    .file_name()
                    .is_none_or(|current| current != std::ffi::OsStr::new(name))
        })
        .collect();
    if pairs.is_empty() {
        return false;
    }
    let requested = pairs.len();
    if let Some(reason) = pairs.iter().find_map(|(_, name)| invalid_name_reason(name)) {
        let outcome = FileOpOutcome {
            requested,
            cancelled: false,
            error: Some(reason),
            applied: Vec::new(),
        };
        if done.send(outcome).is_ok() {
            wake();
        }
        return true;
    }
    let owner = HwndSend(owner.0 as isize);
    spawn_shell_op(done, wake, move || {
        let ran = perform_rename_many(&pairs, &owner, FOF_ALLOWUNDO);
        FileOpOutcome {
            requested,
            cancelled: ran.cancelled,
            error: ran.error,
            applied: ran.applied,
        }
    })
}

/// `paths`를 지운다 — **곧바로 돌아오고** 결과는 `done`으로 온다 (FR-64).
///
/// `permanent`가 거짓이면 휴지통으로 보내고, 참이면 곧바로 지운다. 확인 대화는 셸이 띄운다.
/// `paths`가 비면 워커를 띄우지 않고 `false`를 돌려준다 — 부르는 쪽이 헛되이 결과를
/// 기다리지 않게 한다(`copy_into`와 같은 계약)
pub fn delete_items(
    paths: Vec<PathBuf>,
    permanent: bool,
    owner: HWND,
    done: Sender<FileOpOutcome>,
    wake: Wake,
) -> bool {
    if paths.is_empty() {
        return false;
    }
    let owner = HwndSend(owner.0 as isize);
    let requested = paths.len();
    spawn_shell_op(done, wake, move || {
        outcome_of(requested, perform_delete(&paths, permanent, &owner))
    })
}

/// 셸 작업 하나를 COM이 잡힌 워커에서 돌리고 결과를 채널로 보낸다.
///
/// 세 진입점(`copy_into`·`rename_item`·`delete_items`)이 **똑같은 껍데기**를 쓰기 때문에
/// 한 곳으로 모았다 — COM 초기화, **성공했을 때만** 짝지어 해제(실패한 초기화를 해제하면
/// 참조 수가 어긋난다), 결과 송신과 다시 그리기 요청이 그것이다. 안에서 무엇을 하는지는
/// `work`가 정하며 이 함수는 모른다.
///
/// 돌려주는 값은 언제나 `true`(=결과를 기다려야 한다)다 — 워커를 띄우지 않는 조건은
/// 부르는 쪽이 먼저 걸러 낸다
fn spawn_shell_op<T, F>(done: Sender<T>, wake: Wake, work: F) -> bool
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    std::thread::spawn(move || {
        // 안전성: 이 스레드에서 초기화하고 성공했을 때만 같은 스레드에서 1회 해제한다
        // (`fs::thumbnail`·`fs::drives`의 워커와 같은 방식)
        let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        let outcome = work();
        if initialized {
            // 안전성: 위에서 성공한 초기화와 짝지은 1회 호출
            unsafe {
                CoUninitialize();
            }
        }
        // 받을 쪽이 사라졌으면(앱 종료) 조용히 끝난다
        if done.send(outcome).is_ok() {
            wake();
        }
    });
    true
}

/// 셸 호출 결과를 화면이 읽는 값으로 바꾼다 — 성공은 `그만뒀는가`, 실패는 사유다
fn outcome_of(requested: usize, result: Result<bool, String>) -> FileOpOutcome {
    match result {
        Ok(cancelled) => FileOpOutcome {
            requested,
            cancelled,
            error: None,
            applied: Vec::new(),
        },
        Err(error) => FileOpOutcome {
            requested,
            cancelled: false,
            error: Some(error),
            applied: Vec::new(),
        },
    }
}

/// 여러 이름 바꾸기 — 필요하면 임시 이름을 거쳐 두 번에 한다 (plan D12).
///
/// 1단계는 **묶음 안 다른 항목의 지금 자리를 노리는 것**만 임시 이름으로 비켜 둔다. 2단계는
/// 노리지 않는 것을 먼저 걸고 임시 이름인 것을 뒤에 건다 — 셸은 건 차례대로 하므로 그때는
/// 노리던 자리가 모두 비어 있다. 어느 단계든 실패·취소되면 **임시 이름으로 남은 것을 원래
/// 이름으로 되돌리고**(최선 시도), 되돌리지 못한 것은 오류 사유에 적는다.
///
/// `preexisting`은 새 자리가 묶음 밖의 것으로 차 있었는가다 — 묶음 항목이 비워 줄 자리와
/// 대소문자만 바꾸는 자기 자리는 해당하지 않는다
fn perform_rename_many(
    pairs: &[(PathBuf, String)],
    owner: &HwndSend,
    flags: FILEOPERATION_FLAGS,
) -> Performed {
    let current: Vec<&Path> = pairs.iter().map(|(path, _)| path.as_path()).collect();
    let targets: Vec<PathBuf> = pairs
        .iter()
        .map(|(path, name)| path.with_file_name(name))
        .collect();
    let in_batch = |target: &Path| current.iter().any(|path| same_path(path, target));
    let occupied: Vec<PathBuf> = targets
        .iter()
        .filter(|target| !in_batch(target) && std::fs::symlink_metadata(target).is_ok())
        .cloned()
        .collect();
    let blocked: Vec<bool> = targets
        .iter()
        .enumerate()
        .map(|(index, target)| {
            current
                .iter()
                .enumerate()
                .any(|(other, path)| other != index && same_path(path, target))
        })
        .collect();

    // 1단계 — 노리는 자리를 가진 것만 임시 이름으로
    let parked: Vec<(PathBuf, String)> = pairs
        .iter()
        .zip(&blocked)
        .enumerate()
        .filter(|(_, (_, blocked))| **blocked)
        .map(|(index, ((path, _), _))| (path.clone(), temp_name(path, index)))
        .collect();
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    if !parked.is_empty() {
        match rename_batch(&parked, owner, flags) {
            Err(error) => return Performed::failed(error),
            Ok(ran) => {
                moved = ran.pairs;
                if ran.cancelled || ran.error.is_some() {
                    let left = restore_parked(&moved, &[], owner, flags);
                    return Performed {
                        cancelled: ran.cancelled,
                        error: with_left(ran.error, &left),
                        applied: Vec::new(),
                    };
                }
            }
        }
    }

    // 2단계 — 노리지 않는 것 먼저, 임시 이름인 것 뒤에
    let parked_at = |original: &Path| {
        moved
            .iter()
            .find(|(from, _)| same_path(from, original))
            .map(|(_, temp)| temp.clone())
    };
    let mut second: Vec<(PathBuf, String)> = pairs
        .iter()
        .zip(&blocked)
        .filter(|(_, blocked)| !**blocked)
        .map(|(pair, _)| pair.clone())
        .collect();
    second.extend(
        pairs
            .iter()
            .zip(&blocked)
            .filter(|(_, blocked)| **blocked)
            .filter_map(|((path, name), _)| parked_at(path).map(|temp| (temp, name.clone()))),
    );
    let ran = rename_batch(&second, owner, flags);
    let (cancelled, error, finals) = match ran {
        Err(error) => (false, Some(error), Vec::new()),
        Ok(ran) => (ran.cancelled, ran.error, ran.pairs),
    };
    let left = restore_parked(&moved, &finals, owner, flags);
    // 임시 이름에서 간 것은 원래 자리를 되찾아 싣는다 — 되돌리기는 원래 이름으로 돌린다
    let original_of = |from: &Path| {
        moved
            .iter()
            .find(|(_, temp)| same_path(temp, from))
            .map(|(original, _)| original.clone())
            .unwrap_or_else(|| from.to_path_buf())
    };
    Performed {
        cancelled,
        error: with_left(error, &left),
        applied: finals
            .into_iter()
            .map(|(from, to)| Applied {
                from: original_of(&from),
                stamp: stamp_of(&to),
                preexisting: occupied.iter().any(|seen| same_path(seen, &to)),
                to,
            })
            .collect(),
    }
}

/// 이름 바꾸기 한 묶음을 작업 하나로 건다 — 셸 항목으로 못 만든 것은 그것만 건너뛴다
fn rename_batch(
    list: &[(PathBuf, String)],
    owner: &HwndSend,
    flags: FILEOPERATION_FLAGS,
) -> Result<Ran, String> {
    let sources: Vec<PathBuf> = list.iter().map(|(path, _)| path.clone()).collect();
    let names: Vec<HSTRING> = list.iter().map(|(_, name)| HSTRING::from(name)).collect();
    // 안전성: COM이 STA로 초기화된 워커 스레드에서만 부른다. 이름 문자열은 `names`가
    // 호출이 끝날 때까지 쥔다
    unsafe {
        run_operation(flags, owner, &sources, |op| {
            let mut queued = 0usize;
            for ((path, _), name) in list.iter().zip(&names) {
                let Ok(item) = shell_item(path) else {
                    continue;
                };
                if op.RenameItem(&item, PCWSTR(name.as_ptr()), None).is_ok() {
                    queued += 1;
                }
            }
            if queued == 0 {
                return Err(crate::i18n::rename_no_source().to_owned());
            }
            Ok(())
        })
    }
}

/// 임시 이름으로 비켜 둔 것 중 최종 이름까지 가지 못한 것을 원래 이름으로 되돌린다.
///
/// 돌려주는 값은 **되돌리지 못한 임시 자리**다 — 사용자가 찾아 고칠 수 있게 사유에 적는다
fn restore_parked(
    moved: &[(PathBuf, PathBuf)],
    finals: &[(PathBuf, PathBuf)],
    owner: &HwndSend,
    flags: FILEOPERATION_FLAGS,
) -> Vec<PathBuf> {
    let stuck: Vec<(PathBuf, String)> = moved
        .iter()
        .filter(|(_, temp)| !finals.iter().any(|(from, _)| same_path(from, temp)))
        .filter_map(|(original, temp)| {
            original
                .file_name()
                .map(|name| (temp.clone(), name.to_string_lossy().into_owned()))
        })
        .collect();
    if stuck.is_empty() {
        return Vec::new();
    }
    let back = rename_batch(&stuck, owner, flags)
        .map(|ran| ran.pairs)
        .unwrap_or_default();
    stuck
        .into_iter()
        .map(|(temp, _)| temp)
        .filter(|temp| !back.iter().any(|(from, _)| same_path(from, temp)))
        .collect()
}

/// 임시 이름 — 같은 폴더 안, 원래 이름을 끝에 남겨 남았을 때 무엇이었는지 보이게 한다
fn temp_name(path: &Path, index: usize) -> String {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("~moa-{}-{index}-{name}", std::process::id())
}

/// 실패 사유에 되돌리지 못한 임시 자리를 덧붙인다
fn with_left(error: Option<String>, left: &[PathBuf]) -> Option<String> {
    if left.is_empty() {
        return error;
    }
    let names: Vec<String> = left.iter().map(|path| path.display().to_string()).collect();
    let note = crate::i18n::dynamic::rename_temp_left(&names.join(", "));
    Some(match error {
        Some(error) => format!("{error} — {note}"),
        None => note,
    })
}

/// 삭제 COM 호출 — 성공하면 `사용자가 그만뒀는가`를 돌려준다.
///
/// **그 사이 사라진 항목은 그것만 건너뛴다** — 여러 개를 골랐는데 하나가 없어졌다고
/// 나머지를 버릴 이유가 없다(`perform`의 같은 규칙). 하나도 걸지 못했으면 실패로 본다
fn perform_delete(paths: &[PathBuf], permanent: bool, owner: &HwndSend) -> Result<bool, String> {
    // 안전성: 위 `perform_rename`과 같다
    unsafe {
        let op = new_operation(delete_flags(permanent), owner)?;
        let mut queued = 0usize;
        for path in paths {
            let Ok(item) = shell_item(path) else {
                // 그 사이 사라진 항목 — 나머지는 그대로 지운다
                continue;
            };
            if op.DeleteItem(&item, None).is_ok() {
                queued += 1;
            }
        }
        if queued == 0 {
            return Err(crate::i18n::delete_no_source().to_owned());
        }
        op.PerformOperations().map_err(|err| err.message())?;
        Ok(aborted(&op))
    }
}

/// 플래그와 소유자 창을 세운 `IFileOperation` 하나.
///
/// 안전성: COM이 초기화된 스레드에서만 부른다. 창이 없으면(headless·파괴됨) 소유자를 주지
/// 않아 셸이 자기 대화를 소유자 없이 띄운다
unsafe fn new_operation(
    flags: FILEOPERATION_FLAGS,
    owner: &HwndSend,
) -> Result<IFileOperation, String> {
    // 안전성: 위 주석 참조
    unsafe {
        let op: IFileOperation =
            CoCreateInstance(&FileOperation, None, CLSCTX_ALL).map_err(|err| err.message())?;
        op.SetOperationFlags(flags).map_err(|err| err.message())?;
        if owner.0 != 0 {
            let _ = op.SetOwnerWindow(HWND(owner.0 as *mut core::ffi::c_void));
        }
        Ok(op)
    }
}

/// 사용자가 셸 대화에서 그만뒀는가 — 물어보지 못하면 "그만두지 않았다"로 본다.
///
/// 안전성: `PerformOperations`가 끝난 뒤의 유효한 인터페이스에만 부른다
unsafe fn aborted(op: &IFileOperation) -> bool {
    // 안전성: 위 주석 참조
    unsafe {
        op.GetAnyOperationsAborted()
            .map(|aborted| aborted.as_bool())
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn 가짜_깨우기() -> Wake {
        Arc::new(|| {})
    }

    #[test]
    fn 지울_것이_없으면_워커를_띄우지_않는다() {
        // `copy_into`와 같은 계약 — 부르는 쪽이 오지 않을 결과를 기다리지 않게 한다
        let (tx, rx) = channel();
        let started = delete_items(
            Vec::new(),
            false,
            HWND(std::ptr::null_mut()),
            tx,
            가짜_깨우기(),
        );
        assert!(!started);
        assert!(rx.try_recv().is_err(), "결과도 오지 않는다");
    }

    #[test]
    fn 휴지통과_영구_삭제는_되돌리기_플래그로_갈린다() {
        // `FOF_ALLOWUNDO`가 곧 휴지통이다 — 이 판정이 뒤집히면 지운 파일을 되찾을 수 없다
        assert_eq!(delete_flags(false), FOF_ALLOWUNDO, "기본은 휴지통");
        assert_eq!(
            delete_flags(true),
            FILEOPERATION_FLAGS(0),
            "Shift+Delete는 되돌리기 없이 곧바로 지운다"
        );
    }

    #[test]
    fn 쓸_수_없는_글자가_있는_이름은_셸에_걸지_않는다() {
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        for 이름 in [
            r"a\b", "a/b", "a:b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b",
        ] {
            assert!(
                invalid_name_reason(이름).is_some(),
                "{이름} 은 거부돼야 한다"
            );
        }
        assert_eq!(
            invalid_name_reason("보고서 (최종).txt"),
            None,
            "괄호·공백·점은 쓸 수 있다"
        );
    }

    #[test]
    fn 예약된_장치_이름은_이_계층이_판정하지_않는다() {
        // 자리에 따라 갈리는 규칙이라 흉내 내면 실제와 어긋난 채 굳는다 — 셸이 알린다
        assert_eq!(invalid_name_reason("CON"), None);
        assert_eq!(invalid_name_reason("보고서."), None);
    }

    #[test]
    fn 빈_이름과_같은_이름은_아무_일도_하지_않는다() {
        // 편집을 열었다 그대로 확정한 경우다 — 알릴 것이 없으므로 결과도 보내지 않는다
        for 새_이름 in ["", "app.js"] {
            let (tx, rx) = channel();
            let started = rename_item(
                PathBuf::from(r"C:\work\app.js"),
                새_이름.to_owned(),
                HWND(std::ptr::null_mut()),
                tx,
                가짜_깨우기(),
            );
            assert!(!started, "{새_이름:?} 는 워커를 띄우지 않는다");
            assert!(rx.try_recv().is_err(), "결과도 오지 않는다");
        }
    }

    #[test]
    fn 대상이_없으면_워커를_띄우지_않는다() {
        // 고른 것 없이 `Delete`·`Ctrl+V`를 눌렀을 때다 (FR-12) — 워커도, 결과도 없어야
        // 부르는 쪽이 헛되이 기다리지 않는다
        let (tx, rx) = channel();
        assert!(!delete_items(
            Vec::new(),
            false,
            HWND(std::ptr::null_mut()),
            tx,
            가짜_깨우기()
        ));
        assert!(rx.try_recv().is_err());

        for 옮기기 in [false, true] {
            let (tx, rx) = channel();
            let 시작 = if 옮기기 {
                move_into(
                    PathBuf::from(r"C:\일"),
                    Vec::new(),
                    HWND(std::ptr::null_mut()),
                    tx,
                    가짜_깨우기(),
                )
            } else {
                copy_into(
                    PathBuf::from(r"C:\일"),
                    Vec::new(),
                    HWND(std::ptr::null_mut()),
                    tx,
                    가짜_깨우기(),
                )
            };
            assert!(!시작, "옮기기={옮기기} 에서 워커가 떴다");
            assert!(rx.try_recv().is_err());
        }
    }

    #[test]
    fn 거부된_이름은_워커_없이_사유만_알린다() {
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        let (tx, rx) = channel();
        let started = rename_item(
            PathBuf::from(r"C:\work\app.js"),
            "a/b.js".to_owned(),
            HWND(std::ptr::null_mut()),
            tx,
            가짜_깨우기(),
        );
        assert!(started, "사유가 갈 것이므로 기다려야 한다");
        let outcome = rx.try_recv().expect("사유가 곧바로 온다");
        assert_eq!(outcome.requested, 1);
        assert!(!outcome.cancelled);
        assert_eq!(
            outcome.error.as_deref(),
            Some(r#"이름에 \ / : * ? " < > | 는 쓸 수 없습니다"#)
        );
    }

    #[test]
    fn 원본이_없으면_워커를_띄우지_않는다() {
        // 부르는 쪽이 오지 않을 결과를 기다리지 않게 한다
        let (tx, rx) = channel();
        let started = copy_into(
            PathBuf::from(r"C:\down"),
            Vec::new(),
            HWND(std::ptr::null_mut()),
            tx,
            가짜_깨우기(),
        );
        assert!(!started);
        assert!(rx.try_recv().is_err(), "결과도 오지 않는다");
    }

    #[test]
    fn 대상이_원본과_같은_폴더여도_그대로_셸에_넘긴다() {
        // **이 계층은 걸러내지 않는다** — 무엇을 걸러낼지는 부르는 쪽이 정한다.
        // 앱 안 드롭은 `ui::list_common::local_copy_target`이 같은 폴더를 먼저 잘라내므로
        // (2026-08-21 — FR-60) 여기까지 오지 않고, 여기로 오는 같은 폴더 복사는
        // OS에서 끌어온 경로(FR-61 ⓐ)뿐이다. 그 경우 사본을 만들지 거부할지는 셸이 정한다
        let (tx, _rx) = channel();
        let started = copy_into(
            PathBuf::from(r"C:\work"),
            vec![PathBuf::from(r"C:\work\app.js")],
            HWND(std::ptr::null_mut()),
            tx,
            가짜_깨우기(),
        );
        assert!(started, "같은 폴더라는 이유로 걸러내지 않는다");
    }

    #[test]
    fn 결과는_걸었던_항목_수를_그대로_싣는다() {
        // 화면이 규모를 적는 데 쓴다 — 셸이 실제로 몇 개를 복사했는지는 셸만 안다
        let outcome = CopyOutcome {
            requested: 3,
            how: Transfer::Copy,
            cancelled: false,
            error: None,
            applied: Vec::new(),
        };
        assert_eq!(outcome.requested, 3);
        assert!(outcome.error.is_none());
    }

    #[test]
    fn 취소는_오류가_아니다() {
        // 사용자가 셸 대화에서 그만둔 것과 셸이 실패한 것은 화면에서 다르게 알린다
        let 취소 = CopyOutcome {
            requested: 1,
            how: Transfer::Copy,
            cancelled: true,
            error: None,
            applied: Vec::new(),
        };
        assert!(취소.cancelled && 취소.error.is_none());
    }

    /// 시험 전용 셸 플래그 — 대화를 하나도 띄우지 않는다(충돌은 덮어쓰기로 답한다)
    const 조용히: FILEOPERATION_FLAGS = FILEOPERATION_FLAGS(
        windows::Win32::UI::Shell::FOF_SILENT.0
            | windows::Win32::UI::Shell::FOF_NOCONFIRMATION.0
            | windows::Win32::UI::Shell::FOF_NOERRORUI.0
            | FOF_NOCONFIRMMKDIR.0,
    );

    fn 임시_폴더(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fe_fileop_test_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("임시 폴더");
        dir
    }

    /// COM 을 STA 로 잡은 새 스레드에서 돌린다 — 운영 워커(`spawn_shell_op`)와 같은 조건이다
    fn 셸_스레드에서<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::spawn(move || {
            // 안전성: 이 스레드에서 초기화하고 성공했을 때만 같은 스레드에서 1회 해제한다
            let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
            let out = work();
            if initialized {
                // 안전성: 위에서 성공한 초기화와 짝지은 1회 호출
                unsafe { CoUninitialize() };
            }
            out
        })
        .join()
        .expect("셸 스레드")
    }

    fn 없는_창() -> HwndSend {
        HwndSend(0)
    }

    #[test]
    fn 복사_결과에_생긴_경로가_실린다() {
        let 원본 = 임시_폴더("copy_src");
        let 대상 = 임시_폴더("copy_dst");
        std::fs::write(원본.join("a.txt"), b"abc").expect("파일");
        let (원본2, 대상2) = (원본.clone(), 대상.clone());
        let 결과 = 셸_스레드에서(move || {
            perform(
                &대상2,
                &[원본2.join("a.txt")],
                &없는_창(),
                Transfer::Copy,
                조용히,
            )
        });
        assert_eq!(결과.error, None);
        assert_eq!(결과.applied.len(), 1, "{:?}", 결과.applied);
        let 한건 = &결과.applied[0];
        assert!(same_path(&한건.from, &원본.join("a.txt")));
        assert!(same_path(&한건.to, &대상.join("a.txt")));
        assert!(!한건.preexisting);
        assert_eq!(한건.stamp.map(|stamp| stamp.len), Some(3), "도장이 실린다");
    }

    #[test]
    fn 둘_다_유지로_번호가_붙은_경로가_실린다() {
        let 폴더 = 임시_폴더("keep_both");
        let 원본 = 폴더.join("src");
        std::fs::create_dir_all(&원본).expect("폴더");
        std::fs::write(원본.join("a.txt"), b"new").expect("파일");
        std::fs::write(폴더.join("a.txt"), b"old").expect("있던 파일");
        let 폴더2 = 폴더.clone();
        let 결과 = 셸_스레드에서(move || {
            perform(
                &폴더2,
                &[폴더2.join(r"src\a.txt")],
                &없는_창(),
                Transfer::Copy,
                FILEOPERATION_FLAGS(조용히.0 | windows::Win32::UI::Shell::FOF_RENAMEONCOLLISION.0),
            )
        });
        assert_eq!(결과.applied.len(), 1, "{:?}", 결과.applied);
        let 한건 = &결과.applied[0];
        assert!(
            !same_path(&한건.to, &폴더.join("a.txt")),
            "번호가 붙은 새 자리여야 한다: {}",
            한건.to.display()
        );
        assert!(한건.to.exists());
        assert!(!한건.preexisting, "새 자리는 있던 것이 아니다");
        assert_eq!(std::fs::read(폴더.join("a.txt")).expect("읽기"), b"old");
    }

    #[test]
    fn 있던_자리에_복사하면_preexisting_이다() {
        let 폴더 = 임시_폴더("overwrite");
        std::fs::create_dir_all(폴더.join("src")).expect("폴더");
        std::fs::write(폴더.join(r"src\a.txt"), b"new").expect("파일");
        std::fs::write(폴더.join("a.txt"), b"old").expect("있던 파일");
        let 폴더2 = 폴더.clone();
        let 결과 = 셸_스레드에서(move || {
            perform(
                &폴더2,
                &[폴더2.join(r"src\a.txt")],
                &없는_창(),
                Transfer::Copy,
                조용히,
            )
        });
        assert_eq!(결과.applied.len(), 1, "{:?}", 결과.applied);
        assert!(
            결과.applied[0].preexisting,
            "덮어쓴 자리는 되돌리기에서 빠져야 한다"
        );
    }

    #[test]
    fn 이동과_이름_바꾸기도_새_경로를_싣는다() {
        let 폴더 = 임시_폴더("move_rename");
        std::fs::create_dir_all(폴더.join("dst")).expect("폴더");
        std::fs::write(폴더.join("m.txt"), b"m").expect("파일");
        std::fs::write(폴더.join("r.txt"), b"r").expect("파일");
        let 폴더2 = 폴더.clone();
        let (이동, 이름) = 셸_스레드에서(move || {
            let 이동 = perform(
                &폴더2.join("dst"),
                &[폴더2.join("m.txt")],
                &없는_창(),
                Transfer::Move,
                조용히,
            );
            let 이름 = perform_rename_many(
                &[(폴더2.join("r.txt"), "s.txt".to_owned())],
                &없는_창(),
                조용히,
            );
            (이동, 이름)
        });
        assert_eq!(이동.applied.len(), 1, "{:?}", 이동.applied);
        assert!(same_path(&이동.applied[0].to, &폴더.join(r"dst\m.txt")));
        assert_eq!(이름.applied.len(), 1, "{:?}", 이름.applied);
        assert!(same_path(&이름.applied[0].to, &폴더.join("s.txt")));
        assert!(!이름.applied[0].preexisting);
    }

    /// 폴더 안 이름들을 정렬해 돌려준다 — 결과 대조용
    fn 이름들(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .expect("읽기")
            .map(|entry| {
                entry
                    .expect("항목")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    fn 이름_바꾸기(pairs: Vec<(PathBuf, &str)>) -> Performed {
        let pairs: Vec<(PathBuf, String)> = pairs
            .into_iter()
            .map(|(path, name)| (path, name.to_owned()))
            .collect();
        셸_스레드에서(move || perform_rename_many(&pairs, &없는_창(), 조용히))
    }

    #[test]
    fn 여러_이름을_한_번에_바꾼다() {
        let 폴더 = 임시_폴더("rename_many");
        for 이름 in ["a.txt", "b.txt"] {
            std::fs::write(폴더.join(이름), 이름).expect("파일");
        }
        let 결과 = 이름_바꾸기(vec![
            (폴더.join("a.txt"), "x.txt"),
            (폴더.join("b.txt"), "y.txt"),
        ]);
        assert_eq!(결과.error, None);
        assert_eq!(이름들(&폴더), ["x.txt", "y.txt"]);
        assert_eq!(결과.applied.len(), 2);
    }

    #[test]
    fn 서로_맞바꾸는_이름도_된다() {
        let 폴더 = 임시_폴더("rename_swap");
        std::fs::write(폴더.join("a.txt"), b"A").expect("파일");
        std::fs::write(폴더.join("b.txt"), b"B").expect("파일");
        let 결과 = 이름_바꾸기(vec![
            (폴더.join("a.txt"), "b.txt"),
            (폴더.join("b.txt"), "a.txt"),
        ]);
        assert_eq!(결과.error, None);
        assert_eq!(이름들(&폴더), ["a.txt", "b.txt"], "임시 이름이 남지 않는다");
        assert_eq!(std::fs::read(폴더.join("a.txt")).expect("읽기"), b"B");
        assert_eq!(std::fs::read(폴더.join("b.txt")).expect("읽기"), b"A");
        // 기록은 원래 자리 → 최종 자리다 — 임시 이름이 끼지 않는다
        for 한건 in &결과.applied {
            assert!(!한건.from.to_string_lossy().contains("~moa-"), "{:?}", 한건);
            assert!(!한건.preexisting, "묶음이 비워 준 자리는 있던 것이 아니다");
        }
        assert_eq!(결과.applied.len(), 2);
    }

    #[test]
    fn 대소문자만_바꾸는_이름도_된다() {
        let 폴더 = 임시_폴더("rename_case");
        std::fs::write(폴더.join("a.txt"), b"A").expect("파일");
        let 결과 = 이름_바꾸기(vec![(폴더.join("a.txt"), "A.txt")]);
        assert_eq!(결과.error, None);
        assert_eq!(이름들(&폴더), ["A.txt"]);
        assert_eq!(결과.applied.len(), 1);
        assert!(!결과.applied[0].preexisting, "자기 자리는 있던 것이 아니다");
    }

    #[test]
    fn 잘못된_글자가_하나라도_있으면_사유만_보낸다() {
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        let (tx, rx) = channel();
        let started = rename_many(
            vec![
                (PathBuf::from(r"C:\work\a.txt"), "ok.txt".to_owned()),
                (PathBuf::from(r"C:\work\b.txt"), "a?b.txt".to_owned()),
            ],
            HWND(std::ptr::null_mut()),
            tx,
            가짜_깨우기(),
        );
        assert!(started);
        let outcome = rx.try_recv().expect("사유가 곧바로 온다");
        assert_eq!(outcome.requested, 2);
        assert!(outcome.error.is_some());
        assert!(outcome.applied.is_empty(), "하나도 걸지 않는다");
    }

    #[test]
    fn 둘째_단계가_실패하면_임시_이름을_되돌린다() {
        use std::os::windows::fs::OpenOptionsExt;
        // a→b, b→c 사슬 — a 는 b 의 지금 자리를 노려 임시 이름으로 비켜 간다. b 를 공유 없이
        // 열어 두면 2단계의 b→c 가 실패하고, 그 뒤의 임시→b 도 자리가 차 실패한다
        let 폴더 = 임시_폴더("rename_restore");
        std::fs::write(폴더.join("a.txt"), b"A").expect("파일");
        std::fs::write(폴더.join("b.txt"), b"B").expect("파일");
        let 잠금 = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(폴더.join("b.txt"))
            .expect("잠금");
        let 결과 = 이름_바꾸기(vec![
            (폴더.join("a.txt"), "b.txt"),
            (폴더.join("b.txt"), "c.txt"),
        ]);
        drop(잠금);
        assert_eq!(
            이름들(&폴더),
            ["a.txt", "b.txt"],
            "임시 이름이 원래대로 돌아온다"
        );
        assert_eq!(std::fs::read(폴더.join("a.txt")).expect("읽기"), b"A");
        assert!(결과.applied.is_empty(), "{:?}", 결과.applied);
        assert!(결과.error.is_some() || 결과.cancelled);
    }
}
