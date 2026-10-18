//! 마우스 입력이 몰리는 동안에도 화면이 제때 그려지게 한다 — 고주사율 마우스 대응.
//!
//! winit(Windows)은 다시 그리기를 `WM_PAINT`로 받는데, Windows는 큐에 입력 메시지가 남아 있으면
//! `WM_PAINT`를 내주지 않는다(우선순위가 가장 낮다). 1000Hz 이상 마우스를 움직이는 동안에는
//! `WM_INPUT`·`WM_MOUSEMOVE`가 끊이지 않아 egui가 청한 그리기가 수십~수백 ms 밀리고, 목록의
//! hover 표시가 커서가 지나간 뒤에야 따라온다(2026-10-18 실측 — 1000Hz 이동 주입에서 프레임 간격이
//! 최대 80~375ms로 늘었고, 프레임당 CPU는 3ms 그대로였다).
//!
//! 두 가지를 한다.
//! - **raw 마우스 입력 등록을 걷는다.** winit은 창이 포커스를 가진 동안 마우스 보고마다
//!   `WM_INPUT`을 받는데(1000Hz면 초당 1000개), 이것이 큐를 독차지하면 `WM_MOUSEMOVE`까지
//!   프레임당 하나로 굶어 egui가 아는 커서 위치도 낡는다. 그 값은 egui의 `drag_motion`만 쓰고
//!   이 앱은 쓰지 않는다. 키보드 raw 입력은 그대로 둔다
//! - **그리기를 강제한다.** `WM_MOUSEMOVE`만으로도 큐는 찬다(raw 입력만 걷었을 때 실측 최대
//!   150ms). 메시지 훅에서 `WM_MOUSEMOVE`를 볼 때 마지막 그리기로부터 [`MAX_PAINT_GAP`]이
//!   지났으면, 그 이동을 먼저 넘긴 뒤 그 자리에서 동기 그리기(`RDW_UPDATENOW`)를 건다.
//!   입력이 몰리지 않을 때는 그리기가 이 간격 안에 오므로 아무것도 하지 않는다
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Gdi::{RDW_INVALIDATE, RDW_UPDATENOW, RedrawWindow};
use windows::Win32::UI::Input::{RAWINPUTDEVICE, RIDEV_REMOVE, RegisterRawInputDevices};
use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, WM_MOUSEMOVE, WM_PAINT};
use winit::platform::windows::EventLoopBuilderExtWindows;

/// 마우스가 움직이는 동안 그리기 시작 사이에 허용하는 가장 긴 간격 — 60Hz 한 프레임쯤이다.
///
/// 평소에는 이동 하나마다 그리기가 1ms 안쪽에 따라와 이 간격에 닿지 않는다. 닿는 것은 그리기가
/// 입력에 밀렸을 때와 멈췄던 마우스를 처음 움직였을 때뿐이다(2026-10-18 실측 — 25ms로 두면
/// 입력이 몰리는 동안 프레임 간격 중앙값이 23~32ms였다)
pub const MAX_PAINT_GAP: Duration = Duration::from_millis(16);

/// eframe 이벤트 루프에 메시지 훅을 건다 — 창을 만들기 전에 불러야 한다
pub fn install(options: &mut eframe::NativeOptions) {
    options.event_loop_builder = Some(Box::new(|builder| {
        let mut state = PaintState::default();
        builder.with_msg_hook(move |raw| {
            // 안전성: winit은 `PeekMessageW`로 채운 유효한 `MSG`의 주소를 넘기고,
            // 그 값은 이 호출 동안 살아 있다
            let msg = unsafe { &*(raw as *const MSG) };
            state.on_message(msg)
        });
    }));
}

/// 앱 창의 마지막 그리기 시작 시각
#[derive(Default)]
struct PaintState {
    /// 마우스 이동을 받은 창 — winit의 내부 메시지 창도 `WM_PAINT`를 받으므로 그것과 가른다
    window: Option<isize>,
    last_paint: Option<Instant>,
    /// raw 마우스 입력 등록을 걷었는가 — winit이 이벤트 루프를 만들며 등록하므로 그 뒤에 걷는다
    raw_mouse_removed: bool,
}

impl PaintState {
    /// 메시지 하나를 본다 — 직접 처리했으면 `true`(winit은 그 메시지를 다시 넘기지 않는다)
    fn on_message(&mut self, msg: &MSG) -> bool {
        if !self.raw_mouse_removed {
            self.raw_mouse_removed = true;
            remove_raw_mouse_input();
        }
        let hwnd = msg.hwnd.0 as isize;
        match msg.message {
            WM_PAINT if self.window == Some(hwnd) => {
                self.last_paint = Some(Instant::now());
                false
            }
            WM_MOUSEMOVE => {
                self.window = Some(hwnd);
                if !should_force_paint(self.last_paint, Instant::now()) {
                    return false;
                }
                // 시작 시각으로 잰다 — 끝난 시각으로 재면 그리기(수직 동기 대기 포함)에 든
                // 시간만큼 주기가 늘어난다
                self.last_paint = Some(Instant::now());
                dispatch_then_paint(msg);
                true
            }
            _ => false,
        }
    }
}

/// 지금 그리기를 강제해야 하는가 — 아직 한 번도 그리지 않았으면 그런다
fn should_force_paint(last_paint: Option<Instant>, now: Instant) -> bool {
    last_paint.is_none_or(|at| now.saturating_duration_since(at) >= MAX_PAINT_GAP)
}

/// 마우스 이동을 넘겨 egui가 새 위치를 알게 한 뒤, 그 위치로 곧바로 그리게 한다.
///
/// `RDW_UPDATENOW`는 `WM_PAINT`를 큐를 거치지 않고 바로 보내므로 남은 입력에 밀리지 않는다.
/// winit은 그것을 평소처럼 `RedrawRequested`로 넘기고 eframe이 한 프레임을 그린다
fn dispatch_then_paint(msg: &MSG) {
    // 안전성: winit이 방금 꺼낸 유효한 메시지를 같은 스레드에서 한 번 넘기고(훅이 `true`를
    // 돌려 winit은 다시 넘기지 않는다), 같은 메시지가 가리키는 살아 있는 창을 다시 그린다
    unsafe {
        DispatchMessageW(msg);
        let _ = RedrawWindow(Some(msg.hwnd), None, None, RDW_INVALIDATE | RDW_UPDATENOW);
    }
}

/// 이 프로세스의 raw 마우스 입력(HID 사용 페이지 1·사용 2) 등록을 걷는다.
///
/// 실패하면 그대로 둔다 — `WM_INPUT`이 계속 오는 것뿐이고, 그리기 강제는 따로 돈다
fn remove_raw_mouse_input() {
    let device = [RAWINPUTDEVICE {
        usUsagePage: 1,
        usUsage: 2,
        dwFlags: RIDEV_REMOVE,
        // `RIDEV_REMOVE`에서는 대상 창을 비워야 한다
        hwndTarget: Default::default(),
    }];
    // 안전성: 스택의 배열 하나와 그 원소 크기를 넘긴다 — 호출 동안만 읽힌다
    unsafe {
        let _ = RegisterRawInputDevices(&device, std::mem::size_of::<RAWINPUTDEVICE>() as u32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 그린_적이_없으면_강제한다() {
        assert!(should_force_paint(None, Instant::now()));
    }

    #[test]
    fn 간격_안이면_기다린다() {
        let at = Instant::now();
        assert!(!should_force_paint(Some(at), at + MAX_PAINT_GAP / 2));
    }

    #[test]
    fn 간격을_넘으면_강제한다() {
        let at = Instant::now();
        assert!(should_force_paint(Some(at), at + MAX_PAINT_GAP));
    }
}
