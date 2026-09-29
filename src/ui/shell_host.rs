//! Win32 셸 컨텍스트 메뉴 연동 (FR-8)
//!
//! 기존 `fs::shell_menu`를 그대로 재사용한다. 필요한 것은 두 가지뿐이다:
//! ① eframe(winit) 창의 **HWND** ② 그 창의 **창 프로시저에 끼어들 지점**.
//!
//! ②가 필요한 이유: 셸 메뉴의 "보내기" 같은 서브메뉴는 `IContextMenu2/3`가
//! `WM_INITMENUPOPUP` 등을 받아야 채워지는데, winit은 그 메시지를 우리 코드로 넘겨주지 않는다.
//! 그래서 창을 서브클래싱해 `forward_menu_msg`로 전달한다(서브클래스가 없으면 서브메뉴가 빈다).
use crate::fs::shell_menu::{forward_menu_msg, show_context_menu};
use crate::fs::shell_menu_worker::ShellMenuWorker;
use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_V};
use windows::Win32::UI::Shell::{
    DefSubclassProc, SHELLEXECUTEINFOW, SetWindowSubclass, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, SW_SHOWNORMAL, WM_APPCOMMAND, WM_KEYDOWN,
};
use windows::core::{HSTRING, PCWSTR, w};

/// 서브클래스 식별자 — 이 바이너리가 붙인 것임을 구분하는 임의의 상수
const SUBCLASS_ID: usize = 1;

/// 셸 메뉴를 띄울 창 핸들 보유자
pub struct ShellHost {
    hwnd: HWND,
}

impl ShellHost {
    /// eframe 창에서 HWND를 얻고 서브클래스를 설치한다.
    /// HWND를 얻지 못하면(다른 백엔드·headless) `None` — 호출부는 셸 메뉴를 비활성한다
    pub fn new(cc: &eframe::CreationContext<'_>) -> Option<ShellHost> {
        let handle = cc.window_handle().ok()?;
        let RawWindowHandle::Win32(win32) = handle.as_raw() else {
            return None;
        };
        let hwnd = HWND(win32.hwnd.get() as *mut core::ffi::c_void);
        // 창 프로시저가 화면을 깨울 통로 — `WM_APPCOMMAND`는 winit을 거치지 않아
        // 이것이 없으면 표시만 남고 다음 프레임이 오지 않는다
        let _ = REPAINT.set(cc.egui_ctx.clone());
        // 안전성: 방금 얻은 유효한 창 핸들에 표준 서브클래스 등록.
        // 창이 파괴되면 서브클래스도 함께 사라지므로 별도 해제는 필요 없다
        unsafe {
            let _ = SetWindowSubclass(hwnd, Some(shell_menu_proc), SUBCLASS_ID, 0);
        }
        Some(ShellHost { hwnd })
    }

    /// 이 앱 창의 핸들 — 창 자체를 다루는 Win32 설정(DWM 속성 등)에 쓴다.
    /// HWND를 잡아 두는 곳이 여기뿐이라, 창 핸들이 필요한 쪽은 이 값을 빌려 간다
    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// **종전 Windows 표준 메뉴**를 화면 좌표에 띄운다 — `기본 메뉴`가 여는 그것이다.
    ///
    /// `items`가 비면 폴더 배경 메뉴("새로 만들기" 포함)가 나온다.
    /// 메뉴가 닫힐 때까지 이 호출은 반환하지 않는다(TrackPopupMenuEx 모달 루프)
    pub fn popup(&self, folder: &Path, items: &[PathBuf], screen_x: i32, screen_y: i32) {
        show_context_menu(self.hwnd, folder, items, screen_x, screen_y);
    }

    /// 셸 메뉴 워커를 띄운다 (FR-8) — 셸 대화가 이 창에 붙도록 핸들 값을 넘긴다.
    ///
    /// 우리가 그리는 메뉴의 셸 조회(열기·하위 메뉴 채움·실행)는 전부 그 워커가 한다 —
    /// UI 스레드에서 돌리면 우클릭마다 0.3~1.8초 창이 멈춘다(`fs::shell_menu_worker`)
    pub fn spawn_menu_worker(&self) -> Option<ShellMenuWorker> {
        ShellMenuWorker::spawn(self.hwnd.0 as isize)
    }

    /// egui가 주는 클라이언트 좌표(물리 픽셀)를 셸 메뉴가 요구하는 화면 좌표로 바꾼다
    pub fn to_screen(&self, client_x: i32, client_y: i32) -> (i32, i32) {
        let mut point = POINT {
            x: client_x,
            y: client_y,
        };
        // 안전성: 유효한 창 핸들 + 스택에 있는 POINT 하나를 변환한다
        unsafe {
            let _ = ClientToScreen(self.hwnd, &mut point);
        }
        (point.x, point.y)
    }

    /// 지금 마우스 커서가 이 창의 어디에 있는가 — **논리 pt**로 돌려준다 (FR-61).
    ///
    /// **egui의 포인터를 쓸 수 없어 Win32로 직접 잰다** — OS 드래그가 도는 동안에는
    /// `WM_MOUSEMOVE`가 오지 않아 egui의 `hover_pos`가 드래그 시작 전 값에 굳어 있고,
    /// winit이 주는 파일 드롭 이벤트에는 좌표가 실려 있지 않다.
    ///
    /// 창 밖이거나 커서를 읽지 못하면 `None`이다 — 부르는 쪽이 아무 일도 하지 않는다
    pub fn cursor_client_pos(&self, pixels_per_point: f32) -> Option<egui::Pos2> {
        let mut point = POINT { x: 0, y: 0 };
        // 안전성: 스택에 있는 POINT 하나를 채우고, 유효한 창 핸들로 좌표계를 옮긴다.
        // 둘 다 실패하면 커서 자리를 모르는 것이라 아무 일도 하지 않는다
        unsafe {
            GetCursorPos(&mut point).ok()?;
            if !ScreenToClient(self.hwnd, &mut point).as_bool() {
                return None;
            }
        }
        Some(client_px_to_pt((point.x, point.y), pixels_per_point))
    }
}

/// 클라이언트 **물리 픽셀**을 egui의 **논리 pt**로 옮긴다 (FR-61).
///
/// `to_screen`의 반대 방향이다 — 그쪽은 논리 pt에 배율을 곱해 물리 픽셀을 만들고
/// (`ui::app`의 셸 메뉴 호출부), 이쪽은 물리 픽셀을 배율로 나눈다. **이 환산이 빠지면
/// 배율 125%·150% 화면에서 놓은 패널과 실제로 받는 패널이 어긋난다.**
///
/// Win32 호출과 떼어 둔 이유는 이것만 시험할 수 있게 하기 위함이다 — 커서를 읽는 쪽은
/// 실제 창이 있어야 돈다
pub fn client_px_to_pt(px: (i32, i32), pixels_per_point: f32) -> egui::Pos2 {
    // 배율이 0이면 나눌 수 없다 — 그런 값은 오지 않지만 오면 1배로 본다
    let scale = if pixels_per_point > 0.0 {
        pixels_per_point
    } else {
        1.0
    };
    egui::pos2(px.0 as f32 / scale, px.1 as f32 / scale)
}

/// 파일을 연결 프로그램으로 연다 (FR-7).
/// 실패해도 셸이 자기 UI로 알리므로 여기서는 따로 알리지 않는다
pub fn execute(path: &Path) {
    let file = HSTRING::from(path.to_string_lossy().as_ref());
    let mut sei = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        lpVerb: w!("open"),
        lpFile: PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // 안전성: sei와 file은 호출이 끝날 때까지 살아 있는 지역 소유다
    unsafe {
        let _ = ShellExecuteExW(&mut sei);
    }
}

/// `WM_APPCOMMAND`의 명령 번호를 `lParam`에서 꺼낼 때 걸러 낼 자리
/// (`GET_APPCOMMAND_LPARAM` — 상위 워드에서 장치 비트 `FAPPCOMMAND_MASK`를 뗀다)
const APPCOMMAND_MASK: u16 = 0x0FFF;
/// `APPCOMMAND_BROWSER_BACKWARD`
const APPCOMMAND_BROWSER_BACKWARD: u16 = 1;
/// `APPCOMMAND_BROWSER_FORWARD`
const APPCOMMAND_BROWSER_FORWARD: u16 = 2;

/// 마우스 옆 버튼이 가리키는 방향 — 창이 본 것을 다음 프레임이 명령으로 바꾼다
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppNav {
    Back,
    Forward,
}

/// 창 프로시저가 화면을 깨울 통로 (`ShellHost::new`가 채운다).
///
/// 트레이가 `OnceLock`에 통로를 두는 것과 같은 이유다 — 창은 하나뿐이라
/// 프로세스 수명과 같은 자리에 두면 해제 시점 문제가 아예 없다
static REPAINT: std::sync::OnceLock<egui::Context> = std::sync::OnceLock::new();

thread_local! {
    /// 마우스 옆 버튼(뒤로·앞으로)이 눌렸다 — 다음 프레임이 거둬 간다.
    ///
    /// **egui를 거칠 수 없어 창에서 직접 받는다**: 마우스에 따라 옆 버튼이
    /// `WM_XBUTTONDOWN`(→ egui `PointerButton::Extra1`·`Extra2`)이 아니라
    /// **`WM_APPCOMMAND`(브라우저 뒤로·앞으로)**로 온다. winit은 그 메시지를 처리하지
    /// 않고 egui `Key`에도 브라우저 키가 없어, 그런 마우스에서는 아무것도 도달하지
    /// 않는다(2026-09-03 실측 — 같은 누름에 `WM_KEYDOWN vk=166/167`도 함께 왔지만
    /// 그 키 역시 egui가 모른다). 탐색기가 옆 버튼을 받는 경로가 이것이다
    static NAV_PRESSED: std::cell::Cell<Option<AppNav>> = const { std::cell::Cell::new(None) };

    /// `Ctrl+V`가 눌렸다 — 다음 프레임이 거둬 간다 (FR-12).
    ///
    /// **egui를 거칠 수 없어 창에서 직접 받는다**: `egui-winit`은 `Ctrl+V`를 가로채
    /// `Event::Paste(글자)`로 바꾸는데, 그 글자를 **텍스트 클립보드에서 읽어 비어 있으면
    /// 아무 이벤트도 만들지 않는다**. 탐색기가 파일을 복사한 클립보드에는 글자가 없어
    /// 붙여넣기 키가 통째로 사라진다(2026-08-22 사용자 보고).
    ///
    /// UI 스레드 전용이라 `thread_local`로 충분하다 — 창 프로시저도 그 스레드에서 돈다
    static PASTE_PRESSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `Ctrl+V`가 눌렸는지 거두고 표시를 내린다 — 프레임마다 한 번 부른다
pub fn take_paste_pressed() -> bool {
    PASTE_PRESSED.replace(false)
}

/// 마우스 옆 버튼이 눌렸는지 거두고 표시를 내린다 — 프레임마다 한 번 부른다
pub fn take_app_nav() -> Option<AppNav> {
    NAV_PRESSED.replace(None)
}

/// `WM_APPCOMMAND`의 `lParam`에서 우리가 쓰는 두 명령을 가려낸다.
///
/// Win32 호출과 떼어 둔 이유는 이것만 시험할 수 있게 하기 위함이다
fn app_nav_from_lparam(lparam: isize) -> Option<AppNav> {
    match ((lparam >> 16) as u16) & APPCOMMAND_MASK {
        APPCOMMAND_BROWSER_BACKWARD => Some(AppNav::Back),
        APPCOMMAND_BROWSER_FORWARD => Some(AppNav::Forward),
        _ => None,
    }
}

/// 창 서브클래스 프로시저 — 메뉴 모달 중의 소유자 메시지를 셸에 전달한다.
/// 메뉴가 떠 있지 않으면 `forward_menu_msg`가 `None`을 주므로 원래 프로시저로 넘긴다
unsafe extern "system" fn shell_menu_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // 트레이 아이콘 조작이 먼저다 — 셸 메뉴와 메시지 번호가 겹치지 않으므로 순서는
    // 성능 문제일 뿐이지만, 트레이는 창이 숨은 동안에도 와야 하는 유일한 경로다
    if unsafe { crate::ui::tray::handle_callback(hwnd, msg, lparam) } {
        return LRESULT(0);
    }
    if let Some(result) = forward_menu_msg(msg, wparam, lparam) {
        return result;
    }
    // 마우스 옆 버튼이 `WM_APPCOMMAND`로 오는 경우 — **여기서 삼킨다**. 위의 `Ctrl+V`와
    // 달리 넘겨 봐야 winit이 모르는 메시지이고, 넘기면 셸이 기본 처리로 다른 창에
    // 되돌려 보낸다. 처리했음을 알리려면 TRUE를 돌려주어야 한다
    if msg == WM_APPCOMMAND
        && let Some(nav) = app_nav_from_lparam(lparam.0)
    {
        NAV_PRESSED.set(Some(nav));
        // 이 메시지는 winit을 거치지 않아 스스로 화면을 깨워야 한다
        if let Some(ctx) = REPAINT.get() {
            ctx.request_repaint();
        }
        return LRESULT(1);
    }
    // `Ctrl+V`만 여기서 본다 — **메시지를 삼키지 않고 표시만 남긴다**(아래에서 winit으로
    // 그대로 넘어간다). 다른 키는 egui가 정상으로 넘겨주므로 손대지 않는다
    if msg == WM_KEYDOWN && wparam.0 as u16 == VK_V.0 {
        // 안전성: `GetKeyState`는 이 스레드의 키보드 상태를 읽기만 한다
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        if ctrl {
            PASTE_PRESSED.set(true);
        }
    }
    // 안전성: 그 외 메시지는 원래(winit) 창 프로시저로 위임한다
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 브라우저_뒤로_앞으로만_가려낸다() {
        // 2026-09-03 실측 — 옆 버튼이 `WM_APPCOMMAND cmd=1`(뒤로)·`cmd=2`(앞으로)로 왔다.
        // 상위 워드에는 장치 비트(`FAPPCOMMAND_MOUSE` = 0x8000)가 함께 실려 있어
        // 그것을 떼지 않으면 어느 명령인지 알아볼 수 없다
        let 마우스가_보낸 = |cmd: u16| ((0x8000u32 | cmd as u32) << 16) as isize;
        assert_eq!(app_nav_from_lparam(마우스가_보낸(1)), Some(AppNav::Back));
        assert_eq!(app_nav_from_lparam(마우스가_보낸(2)), Some(AppNav::Forward));
        // 키보드가 보낸 것(장치 비트 0)도 같은 자리에서 읽힌다
        assert_eq!(app_nav_from_lparam(1 << 16), Some(AppNav::Back));
        // 볼륨·미디어 등 다른 명령은 우리 것이 아니다 — 삼키지 않고 넘어가야 한다
        assert_eq!(app_nav_from_lparam(마우스가_보낸(9)), None);
        assert_eq!(app_nav_from_lparam(0), None);
    }

    #[test]
    fn 물리_픽셀을_배율로_나눠_논리_pt로_옮긴다() {
        // Acceptance ⓓ (FR-61) — 이 환산이 빠지면 배율 125%·150%에서 대상이 어긋난다
        assert_eq!(client_px_to_pt((200, 100), 1.0), egui::pos2(200.0, 100.0));
        assert_eq!(client_px_to_pt((250, 125), 1.25), egui::pos2(200.0, 100.0));
        assert_eq!(client_px_to_pt((300, 150), 1.5), egui::pos2(200.0, 100.0));
    }

    #[test]
    fn 배율이_0이면_1배로_본다() {
        // 그런 값은 오지 않지만 오면 0으로 나눠 좌표가 무한대가 된다
        assert_eq!(client_px_to_pt((200, 100), 0.0), egui::pos2(200.0, 100.0));
    }
}
