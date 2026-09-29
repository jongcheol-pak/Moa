//! 진입점 — COM 초기화, 세션 로드, egui 창 실행.
//!
//! UI는 `ui` 모듈(eframe/glow)이 전부 담당한다. 모듈 정의는 lib 타깃(lib.rs)에 있고
//! 여기서는 사용만 한다(tests/ 공유).

#![windows_subsystem = "windows"]

use eframe::egui;
use moa::app;
use moa::app::settings::load_session;
use moa::ui::app::{ExplorerApp, init_com, uninit_com};
use moa::ui::app_icon;
use moa::ui::window_start;

fn main() -> eframe::Result {
    // **가장 먼저 판정한다** (FR-51) — 두 번째 프로세스라면 COM 초기화·세션 읽기가
    // 전부 헛일이고, 세션 파일을 읽는 동안 첫 프로세스가 그것을 쓰고 있을 수도 있다
    let instance = moa::app::single_instance::acquire();
    if !instance.is_first() {
        // 이미 떠 있는 앱을 앞으로 불러내고 조용히 물러난다 —
        // 창이 둘이 되면 두 프로세스가 같은 설정 파일에 서로 덮어쓴다
        moa::app::single_instance::wake_existing();
        return Ok(());
    }

    // 자동 실행으로 시작했으면 창 없이 트레이로만 올라온다 (FR-49, D9) —
    // 부팅할 때마다 창이 튀어나오는 것이 자동 실행을 끄게 만드는 가장 흔한 이유다
    let start_hidden = moa::app::autostart::started_by_autostart();

    // 임시 FTP 명령 기록 — `MOA_FTP_TRACE=1`일 때만 켜진다. 첫 연결보다 먼저 걸어야 한다
    moa::remote::wire_trace::install();

    let com = init_com();
    // 셸 팝업 메뉴를 다크로 만드는 프로세스 전역 정책 — 창을 만들기 전에 켜야 적용된다.
    // 제목 표시줄은 앱이 직접 그리므로(FR-22) 이 정책의 대상이 아니지만,
    // 우클릭 셸 컨텍스트 메뉴(FR-8)는 여전히 이 정책으로 다크가 된다
    app::theme::enable_dark_mode();
    // 창을 만들기 전에 세션을 읽는다 — 지난번 크기·위치로 떠야 한다 (FR-11).
    // 화면 밖으로 저장된 위치는 창이 뜬 뒤 모니터 크기를 알고 나서 앱이 보정한다
    let session = load_session();
    // 저장된 언어를 **창을 만들기 전에** 적용한다 (FR-53) — 첫 프레임이 그려질 때
    // 이미 정해져 있어야 화면이 한국어로 한 번 그려졌다가 영어로 바뀌는 것이 보이지 않는다.
    // `moa::i18n`을 부르기만 한다 — `mod i18n;`을 여기 두면 같은 파일이 두 모듈로
    // 컴파일돼 전역 현재 언어가 둘이 된다
    moa::i18n::set_language(
        session
            .as_ref()
            .map(|session| session.settings.language)
            .unwrap_or_default(),
    );
    // 창 장식을 끄고 제목 표시줄을 앱이 그린다 (FR-22) — 그 줄에 사이드바 토글·설정 버튼을 두기 위함.
    // 대가로 창 그림자·둥근 모서리를 잃는다(eframe에는 winit의 무장식 그림자 확장에 닿을 길이 없다)
    let mut viewport = egui::ViewportBuilder::default()
        // 제목은 위에서 언어를 적용한 뒤에 읽는다 — 한국어면 `모아`, 영어면 `MOA`다 (FR-53)
        .with_title(moa::i18n::app_name())
        .with_decorations(false);
    // 작업 표시줄·Alt+Tab에 뜨는 창 아이콘. 실행 파일 자체의 아이콘은 `build.rs`가
    // 같은 그림을 리소스로 담아 처리한다 — 창 아이콘은 OS가 리소스에서 자동으로 가져가지 않는다
    if let Some(icon) = app_icon::icon_data() {
        viewport = viewport.with_icon(icon);
    }
    match session.as_ref().map(|s| &s.window) {
        Some(window) => {
            // **최대화는 여기서 걸지 않는다** — `with_maximized(true)`는 창을 만든 직후
            // `ShowWindow(SW_MAXIMIZE)`로 이어져 아직 아무것도 그리지 않은 창을 드러내고,
            // 그 흰 사각형이 번쩍인다(`ui::window_start` 참조). 최대화는 첫 프레임을 그린 뒤
            // `ui::app`이 건다. 대신 처음부터 그 자리의 작업 영역만 하게 띄워 크기가 튀지 않게 한다
            let rect = window_start::start_rect(window, window_start::work_area_for(window));
            viewport = viewport
                .with_position([rect.x as f32, rect.y as f32])
                .with_inner_size([rect.w as f32, rect.h as f32]);
        }
        None => viewport = viewport.with_inner_size([1100.0, 700.0]),
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let result = eframe::run_native(
        "moa",
        options,
        Box::new(move |cc| Ok(Box::new(ExplorerApp::new(cc, com, session, start_hidden)))),
    );
    // 안전성: init_com과 같은 스레드에서 1회 호출한다 (owned일 때만 실제 해제)
    unsafe { uninit_com(com) };
    result
}
