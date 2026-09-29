//! egui 다크 팔레트 — 탐색기 고정 다크 스타일 (FR-21).
//!
//! 색 값은 egui 이식 이전 Win32 판과 **같은 화면색**을 내도록 그대로 옮겨 온 것이다.
//! 타입만 다르다: Win32는 `COLORREF`(0x00BBGGRR 바이트 순서)라 egui에서 그대로 쓸 수 없어
//! `Color32`로 재정의한다. 전환 UI는 없다(PRD Out of Scope).
use eframe::egui;

/// 창 배경·스플리터 틈
pub const WINDOW_BG: egui::Color32 = egui::Color32::from_rgb(0x1B, 0x1B, 0x1B);
/// 목록·트리·입력 컨트롤 배경
pub const SURFACE_BG: egui::Color32 = egui::Color32::from_rgb(0x1E, 0x1E, 0x1E);
/// 기본 글자색
pub const TEXT: egui::Color32 = egui::Color32::from_rgb(0xE8, 0xE8, 0xE8);
/// 목록 헤더 배경
pub const HEADER_BG: egui::Color32 = egui::Color32::from_rgb(0x25, 0x25, 0x25);
/// 목록 헤더 글자
pub const HEADER_TEXT: egui::Color32 = egui::Color32::from_rgb(0xC8, 0xC8, 0xC8);
/// 트리 연결선
pub const TREE_LINE: egui::Color32 = egui::Color32::from_rgb(0x45, 0x45, 0x45);
/// 버튼·컨트롤 기본 배경
pub const CONTROL_BG: egui::Color32 = egui::Color32::from_rgb(0x2A, 0x2A, 0x2A);
/// 버튼 hover 배경
pub const CONTROL_HOT: egui::Color32 = egui::Color32::from_rgb(0x38, 0x38, 0x38);
/// 버튼 눌림·선택 배경
pub const CONTROL_ACTIVE: egui::Color32 = egui::Color32::from_rgb(0x45, 0x45, 0x45);
/// 비활성 글자색
pub const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(0x6A, 0x6A, 0x6A);
/// 타이틀바 닫기 버튼 hover 배경 — Windows 11 캡션 닫기 버튼과 같은 빨강 (FR-22)
pub const CLOSE_HOT: egui::Color32 = egui::Color32::from_rgb(0xC4, 0x2B, 0x1C);
/// 끊긴 네트워크 드라이브의 연결 끊김 배지 — 트리 아이콘 오른쪽 아래에 겹치는 원 (FR-9).
///
/// `CLOSE_HOT`과 값이 같지만 뜻이 다른 자리라 이름을 따로 둔다(팔레트가 `OK_DOT`·`OK_TEXT`
/// 처럼 용도별 상수를 두는 관례와 같다) — `ERROR`(#FF6B6B)는 글자용이라 이 크기의 원에
/// 칠하면 연해서 탐색기 배지와 다르게 읽힌다
pub const OFFLINE_BADGE: egui::Color32 = egui::Color32::from_rgb(0xC4, 0x2B, 0x1C);
/// 분할 패널 경계선 — 패널마다 두르는 기본 테두리 (FR-1·FR-2)
pub const PANE_BORDER: egui::Color32 = egui::Color32::from_rgb(0x33, 0x33, 0x33);
/// 활성 패널 경계선 — 지금 입력을 받는 패널만 한 단계 밝게 두른다.
/// 버튼 상태색(`CONTROL_ACTIVE`)을 빌려 쓰지 않는 이유: 경계선과 버튼은 쓰임이 달라
/// 한쪽을 조정하면 다른 쪽이 함께 바뀐다
pub const PANE_BORDER_ACTIVE: egui::Color32 = egui::Color32::from_rgb(0x5A, 0x5A, 0x5A);
/// 탭의 폴더 아이콘 노랑 — 무채색이 아닌 두 상수 중 하나다(다른 하나는 위 `CLOSE_HOT`).
/// 탐색기 탭은 폴더를 가리키므로 색으로 종류가 드러나는 편이 낫고, 무채색 단계로는 표현할 수 없다
pub const FOLDER_ICON: egui::Color32 = egui::Color32::from_rgb(0xE8, 0xB3, 0x4D);

// ── 원격 연결 화면의 색 (디자인 README `### Colors` 1:1) ──
// 여기 값들은 **디자인 문서가 정본**이다 — 화면마다 비슷한 색을 새로 고르면 같은 상태가
// 자리마다 다른 색으로 보인다.
//
// 표의 색을 **한 번에 다 옮겨 둔다** — 지금 쓰이지 않는 것(강조 파랑·기본 버튼·웰 배경 등)은
// 사이트 관리자·전송 큐·서버 로그 화면(T12~T21)이 쓸 자리다. 화면마다 그때그때 상수를 더하면
// 같은 색이 파일마다 다른 이름으로 생겨 정본이 갈린다.

/// 강조 파랑 — 선택된 탭 밑줄·진행 막대·라디오 선택
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x4A, 0x9E, 0xFF);

/// 연결됨(성공) 계열 — 점 / 글자 / 채움 / 테두리
pub const OK_DOT: egui::Color32 = egui::Color32::from_rgb(0x4A, 0xDE, 0x80);
pub const OK_TEXT: egui::Color32 = egui::Color32::from_rgb(0x7F, 0xD6, 0xA2);
pub const OK_FILL: egui::Color32 = egui::Color32::from_rgb(0x16, 0x24, 0x1C);
pub const OK_BORDER: egui::Color32 = egui::Color32::from_rgb(0x2F, 0x6B, 0x4F);

/// 끝난 전송의 진행 막대 채움 — 같은 성공 계열이지만 배지가 아니라 전송 큐가 쓴다.
///
/// `OK_BORDER`와 값이 같지만 합치지 않는다 — 한쪽은 상태 배지의 테두리이고 이쪽은 막대의
/// 채움이라, 합치면 한쪽 색을 조정할 때 다른 쪽이 함께 끌려간다.
pub const OK_BAR: egui::Color32 = egui::Color32::from_rgb(0x2F, 0x6B, 0x4F);

/// 연결 중(경고) 계열 — 점·글자가 같은 색이다
pub const WARN: egui::Color32 = egui::Color32::from_rgb(0xE8, 0xB3, 0x4D);
pub const WARN_FILL: egui::Color32 = egui::Color32::from_rgb(0x24, 0x1F, 0x14);
pub const WARN_BORDER: egui::Color32 = egui::Color32::from_rgb(0x6B, 0x56, 0x2F);

/// 오류 계열 — 어두운 배경 위 글자는 `ERROR_TEXT`가 따로 있다
pub const ERROR: egui::Color32 = egui::Color32::from_rgb(0xFF, 0x6B, 0x6B);
pub const ERROR_TEXT: egui::Color32 = egui::Color32::from_rgb(0xFF, 0x8A, 0x8A);
pub const ERROR_FILL: egui::Color32 = egui::Color32::from_rgb(0x2A, 0x1A, 0x1A);
pub const ERROR_BORDER: egui::Color32 = egui::Color32::from_rgb(0x4A, 0x26, 0x26);

/// 입력·목록 웰 배경 — 사이트 관리자의 입력칸과 목록이 이 색 위에 앉는다
pub const WELL_BG: egui::Color32 = egui::Color32::from_rgb(0x15, 0x15, 0x15);

/// 팝업 메뉴 프레임의 모서리 반경 — 컨텍스트 메뉴·드롭다운이 모두 이 값을 쓴다.
///
/// **정본을 여기 둔 이유**: 종전에는 메뉴마다 `Frame::menu`에 `.corner_radius(0)`을
/// 덧붙이거나 붙이지 않아, 같은 우클릭 메뉴인데 원격 목록은 각지고 설정 메뉴는 둥글었다
/// (2026-08-19 사용자 보고). `apply_dark`가 이 값을 egui 스타일에 세우면
/// `Frame::menu`가 그것을 읽으므로 각 메뉴는 아무것도 적지 않아도 같은 모양이 된다.
/// 대화 팝업(`ui::dialog`)의 12px과는 별개다 — 그쪽은 버튼 줄을 낀 모달이라 부품이 다르다.
///
/// **이것은 프레임(팝업 껍데기)의 모서리이고, 그 안의 한 줄은 아래 `MENU_ITEM_*`이 정한다**
/// — 부품이 둘이라 값도 둘이다(껍데기 6px / 항목 4px)
pub const MENU_CORNER_RADIUS: u8 = 6;

/// 팝업 메뉴 프레임의 테두리 두께.
///
/// 세 메뉴(원격 목록·트리 즐겨찾기·컨텍스트 메뉴)가 `Frame::menu`의 테두리를 이 값으로
/// 덮어쓰고, **화면 밖 보정(`ui::menu::menu_frame_pad`)도 같은 값을 읽는다** — 그리는 값과
/// 재는 값이 갈리면 메뉴가 화면 끝에서 잘린다
pub const MENU_FRAME_STROKE: f32 = 1.0;

// ── 메뉴 한 줄의 시각 토큰 (2026-08-20 사용자 요청) ──
//
// 종전에는 같은 뜻의 값이 파일마다 다시 정의돼 있어 메뉴마다 모습이 갈렸다 — 우클릭 메뉴는
// hover가 각지고(0px), 설정·사이드바 메뉴는 문구가 하이라이트 끝에 붙고(좌우 2px), 행 높이도
// 18·26·28로 흩어져 설정 메뉴만 작아 보였다. 값의 정본을 여기 하나로 모으고, 각 메뉴는
// `menu_style`을 거치거나(egui 버튼 경로) `widgets::menu_row`를 쓴다(직접 그리는 경로).
//
// 글자 크기는 토큰으로 두지 않는다 — 이미 전 메뉴가 egui 기본 13px로 같고, 여기서 따로
// 정하면 메뉴만 앱의 다른 화면과 다른 크기를 갖게 된다(`ui::dialog`가 버튼 글꼴을 정하지
// 않는 것과 같은 이유)

/// 메뉴 한 줄의 높이 — `menu_style`이 `interact_size.y`로 세우고, 직접 그리는 쪽은 이 값으로 자리를 잡는다
pub const MENU_ITEM_HEIGHT: f32 = 28.0;
/// 메뉴 한 줄의 좌우 여백 — 글자가 hover 하이라이트 끝에 붙지 않게 띄운다
pub const MENU_ITEM_PAD_X: f32 = 12.0;
/// 메뉴 한 줄의 hover 하이라이트 모서리 — 프레임(위 6px)보다 덜 둥글다
pub const MENU_ITEM_CORNER_RADIUS: u8 = 4;

/// 행 hover / 메뉴 hover
pub const ROW_HOT: egui::Color32 = egui::Color32::from_rgb(0x2E, 0x2E, 0x2E);
pub const MENU_HOT: egui::Color32 = egui::Color32::from_rgb(0x38, 0x38, 0x38);
/// **되돌릴 수 없는 메뉴 한 줄**의 hover — 디자인 원본의 삭제 줄이 이 색이다(`:359`).
/// 값은 `CLOSE_HOT`과 같지만 쓰임이 달라 이름을 따로 둔다(그 상수의 주석과 같은 규칙)
pub const MENU_HOT_DANGER: egui::Color32 = egui::Color32::from_rgb(0xC4, 0x2B, 0x1C);

/// 테두리 — 옅은 것 / 컨트롤
pub const BORDER_SUBTLE: egui::Color32 = egui::Color32::from_rgb(0x2C, 0x2C, 0x2C);
pub const BORDER_CONTROL: egui::Color32 = egui::Color32::from_rgb(0x3A, 0x3A, 0x3A);

/// 사이드바 카드·사이트 행의 배경과 hover.
///
/// 워크스페이스 카드와 연결 섹션의 사이트 행이 **같은 hover 색**(`#282828`)을 쓴다 —
/// 사이드바 안에 색 정본이 둘로 갈리지 않게 여기에 둔다 (D20)
pub const CARD_BG: egui::Color32 = egui::Color32::from_rgb(0x23, 0x23, 0x23);
pub const CARD_HOT: egui::Color32 = egui::Color32::from_rgb(0x28, 0x28, 0x28);

/// 선택된 탭의 글자 — 탭 배경 차이가 옅어 기본 글자색(#E8E8E8)으로는 비선택(#9A9A9A)과
/// 잘 갈리지 않는다(2026-08-18 보고). 도크 탭과 전송 큐의 사이트 탭이 함께 쓴다
pub const TEXT_SELECTED: egui::Color32 = egui::Color32::WHITE;

/// 보조 글자 — 밝은 순서대로. `HEADER_TEXT`(#C8C8C8)·`TEXT_DIM`(#6A6A6A)이 그 사이를 메운다
pub const TEXT_BUTTON: egui::Color32 = egui::Color32::from_rgb(0xD8, 0xD8, 0xD8);
pub const TEXT_LOG: egui::Color32 = egui::Color32::from_rgb(0xB4, 0xB4, 0xB4);
pub const TEXT_MUTED: egui::Color32 = egui::Color32::from_rgb(0x9A, 0x9A, 0x9A);
pub const TEXT_FAINT: egui::Color32 = egui::Color32::from_rgb(0x8A, 0x8A, 0x8A);

/// 고정 다크 팔레트를 egui 컨텍스트에 적용한다.
/// egui 기본 다크를 토대로, 위 상수로 현행 앱과 같은 색을 덮어쓴다.
pub fn apply_dark(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = WINDOW_BG;
    visuals.window_fill = SURFACE_BG;
    visuals.extreme_bg_color = SURFACE_BG;
    visuals.faint_bg_color = HEADER_BG;
    visuals.override_text_color = Some(TEXT);

    // 위젯 상태별 배경 — 버튼·입력창이 현행 오너드로우와 같은 명도 단계를 갖게 한다
    visuals.widgets.noninteractive.bg_fill = SURFACE_BG;
    visuals.widgets.inactive.bg_fill = CONTROL_BG;
    visuals.widgets.hovered.bg_fill = CONTROL_HOT;
    visuals.widgets.active.bg_fill = CONTROL_ACTIVE;
    visuals.selection.bg_fill = CONTROL_ACTIVE;

    // 팝업 메뉴 모서리 — `Frame::menu`가 여기서 읽어 간다 (`MENU_CORNER_RADIUS` 참조)
    visuals.menu_corner_radius = egui::CornerRadius::same(MENU_CORNER_RADIUS);

    ctx.set_visuals(visuals);

    // egui의 디버그 경고 하나를 끈다 — 디버그 빌드에만 있는 옵션이라 cfg로 가른다.
    //
    // `warn_if_rect_changes_id`는 "같은 자리를 지난 프레임과 다른 위젯이 차지하면 id가
    // 불안정한 것"이라는 휴리스틱이고, 걸리면 그 위젯에 **2px 빨간 테두리**를 두른다.
    // 이 앱은 하단 도크를 여닫을 때마다 패널이 통째로 위아래로 밀리므로 자리를 물려받는
    // 위젯이 수십 개씩 생긴다 — 화면 대부분이 한 프레임 빨갛게 칠해졌다(사용자 보고).
    // 우리 위젯 id는 패널 id·탭 인덱스로 고정돼 있어 이 경고가 겨냥하는 불안정이 아니다
    #[cfg(debug_assertions)]
    ctx.all_styles_mut(|style| style.debug.warn_if_rect_changes_id = false);
}

/// 팝업 안의 한 줄에 메뉴 항목 토큰을 세운다 — **그 `Ui`와 자식에만** 적용된다.
///
/// 팝업을 여는 쪽이 클로저 첫 줄에서 부른다. 하위 메뉴(`SubMenuButton`이 여는 것)는 부모의
/// 스타일을 잇지 않는 별도 `Area`라 **거기서도 따로 불러야 한다**.
///
/// **전역(`apply_dark`)에 세우지 않는 이유**는 둘이다 — ① 앱 전체 버튼이 함께 커진다
/// ② egui가 팝업마다 자기 메뉴 스타일을 새로 입히므로(`Popup::menu` → `containers::menu::menu_style`)
/// 전역 값은 메뉴 안에서 덮여 **효과가 없다**. 그 위에 이 함수가 앱 값을 다시 덮는 순서다.
pub fn menu_style(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    // 세로 여백은 0이다 — 행 높이는 아래 `interact_size.y`가 잡으므로, 여기에 값을 주면
    // 글자 높이에 더해져 28px를 넘는다
    style.spacing.button_padding = egui::vec2(MENU_ITEM_PAD_X, 0.0);
    style.spacing.interact_size.y = MENU_ITEM_HEIGHT;

    let corner = egui::CornerRadius::same(MENU_ITEM_CORNER_RADIUS);
    let widgets = &mut style.visuals.widgets;
    for state in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        state.corner_radius = corner;
        // 눌리거나 얹혔을 때 항목이 커지지 않는다 — 커지면 좌우 여백이 그만큼 흔들린다.
        // egui 기본값도 0이지만 값에 기대지 않고 명시한다
        state.expansion = 0.0;
    }

    // 채움은 **얹힌·눌린·열린 세 상태만** 칠한다.
    //
    // `inactive`는 egui가 투명으로 세워 둔 것을 그대로 둔다 — 여기에 색을 주면 평상시에도
    // 모든 줄에 배경이 생겨 메뉴가 버튼 목록처럼 보인다.
    // `open`을 빠뜨리면 하위 메뉴가 열린 동안 그 부모 줄만 egui 기본색으로 남는다
    for state in [&mut widgets.hovered, &mut widgets.active, &mut widgets.open] {
        // egui는 버튼 프레임 채움으로 `weak_bg_fill`을 읽고 다른 위젯은 `bg_fill`을 읽는다
        state.weak_bg_fill = MENU_HOT;
        state.bg_fill = MENU_HOT;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `src/ui` 아래의 `.rs`를 하위 폴더까지 모아 온다.
    ///
    /// **비재귀로 훑으면 `ui/app/`·`ui/panel/` 같은 하위가 통째로 빠진다** — 이 시험이
    /// 먼저 그 함정을 피했고, 모달·아이콘 규약 시험도 2026-08-20에 같은 방식으로 맞췄다
    fn ui_sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("ui 디렉터리") {
            let path = entry.expect("항목").path();
            if path.is_dir() {
                ui_sources(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// `Frame::menu(...)`로 시작하는 체인 안에서 `.corner_radius(`를 부르는가.
    ///
    /// 같은 파일에 메뉴가 아닌 프레임(대화 셸 등)이 함께 있을 수 있어 파일 전체를
    /// 훑지 않고 **그 호출에서 이어지는 구간만** 본다. 체인은 `.show(`에서 끝난다
    fn menu_frame_sets_corner(source: &str) -> bool {
        let mut rest = source;
        while let Some(at) = rest.find("Frame::menu(") {
            let tail = &rest[at..];
            let end = tail.find(".show(").unwrap_or(tail.len());
            if tail[..end].contains(".corner_radius(") {
                return true;
            }
            rest = &tail[end.min(tail.len())..];
            // 진행이 없으면(찾은 자리가 끝) 무한히 돌지 않도록 끊는다
            if rest.is_empty() {
                break;
            }
        }
        false
    }

    #[test]
    fn 규약_검사는_값을_가리지_않는다() {
        // 검사기 자신을 시험한다 — `0`만 잡던 종전 방식이 놓치던 두 형태를 든다
        assert!(menu_frame_sets_corner(
            "egui::Frame::menu(s).fill(a).corner_radius(0).show(ui, |ui| {})"
        ));
        assert!(menu_frame_sets_corner(
            "egui::Frame::menu(s).corner_radius(2).show(ui, |ui| {})"
        ));
        assert!(menu_frame_sets_corner(
            "egui::Frame::menu(s).corner_radius(egui::CornerRadius::ZERO).show(ui, |ui| {})"
        ));
        // 메뉴 프레임이 모서리를 적지 않으면 통과한다
        assert!(!menu_frame_sets_corner(
            "egui::Frame::menu(s).fill(a).stroke(b).show(ui, |ui| {})"
        ));
        // 메뉴가 아닌 프레임의 모서리는 대상이 아니다 (대화 셸 등)
        assert!(!menu_frame_sets_corner(
            "egui::Frame::new().corner_radius(12).show(ui, |ui| {})"
        ));
    }

    #[test]
    fn 팝업_메뉴는_모서리를_따로_적지_않는다() {
        // 규약: 메뉴 모서리의 정본은 `theme::MENU_CORNER_RADIUS` 하나이고, 각 메뉴는
        // `Frame::menu`가 읽어 가게 둔다. 문서로만 두면 다음 작업자가 다시 `.corner_radius(0)`을
        // 붙여도 아무것도 걸리지 않아, 같은 우클릭 메뉴인데 어떤 것은 각지고 어떤 것은
        // 둥근 상태로 되돌아간다 (2026-08-19 사용자 보고가 그 상태였다)
        let ui_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        let mut sources = Vec::new();
        ui_sources(&ui_dir, &mut sources);
        assert!(!sources.is_empty(), "ui 소스를 하나도 읽지 못했다");

        // 이 파일은 규약을 설명하느라 그 문자열을 주석에 담는다. **경로 전체로 견준다** —
        // 이름만 보면 하위 폴더에 같은 이름이 생겼을 때 그 파일까지 조용히 빠진다
        let self_path = ui_dir.join("theme.rs");
        let mut 발견 = Vec::new();
        for path in sources {
            if path == self_path {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("소스를 읽지 못했다");
            // **값을 가리지 않고 잡는다** — `.corner_radius(0)`만 찾으면 `.corner_radius(2)`나
            // `.corner_radius(CornerRadius::ZERO)`로 적은 곳이 규약을 어긴 채 통과한다.
            // 메뉴 프레임을 만드는 자리에서는 모서리를 **아예 적지 않는 것**이 규약이다
            if menu_frame_sets_corner(&source) {
                발견.push(
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        assert!(
            발견.is_empty(),
            "메뉴 모서리를 따로 적은 곳(테마의 공통 값을 쓰도록 지운다): {발견:?}"
        );
    }

    /// 검사 대상이 되는 부분만 남긴 소스 — 주석 줄과 **시험 모듈**을 걷어낸다.
    ///
    /// 주석에 팝업 구문 이름(`Popup::menu`·`SubMenuButton` 등)을 적은 파일이 있으면, 이것이
    /// 없으면 그 파일의 팝업 수가 부풀어 거짓 실패한다.
    ///
    /// **시험 모듈을 빼는 이유**: 시험이 부르는 `menu_style`이 호출 수에 얹히면 그만큼
    /// 여유가 생겨, 그 파일에 팝업을 하나 더 만들며 규약을 빠뜨려도 검사가 통과한다
    /// (`titlebar.rs`가 실제로 생산 1 + 시험 1이었다)
    fn code_only(source: &str) -> String {
        let 본문 = source.split("#[cfg(test)]").next().unwrap_or(source);
        본문
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// 이 파일이 여는 팝업 목록의 수.
    ///
    /// `Frame::menu(`는 팝업을 여는 것이 아니라 **그 껍데기 모양을 적는 것**이라, 같은 파일에
    /// 팝업을 여는 구문이 있으면 그 팝업의 프레임을 지정한 것으로 보고 따로 세지 않는다
    /// (`widgets.rs`의 드롭다운이 그 형태다). **컨텍스트 메뉴도 함께 본다** — `Popup::menu(`만
    /// 보면 우클릭 메뉴에 프레임을 지정한 파일이 이중으로 세어져 거짓 실패한다
    fn menu_openers(source: &str) -> usize {
        let code = code_only(source);
        let popup = code.matches("Popup::menu(").count();
        let context =
            code.matches("Popup::context_menu(").count() + code.matches(".context_menu(").count();
        // 하위 메뉴는 부모 스타일을 잇지 않는 별도 `Area`라 자체 호출이 필요하다
        // egui의 `ui.menu_button(`도 함께 본다 — 지금 이 레포에 쓰는 곳은 없지만, 규약이
        // 지키려는 것은 **앞으로 만들 메뉴**라 진입 API가 늘 때마다 사각이 생기면 뜻이 없다.
        // 점을 붙여 좁히는 이유: 그냥 `menu_button(`으로 찾으면 이 레포의 함수 이름
        // (`show_menu_button`)까지 잡혀 거짓 실패한다(실측)
        let submenu = code.matches("SubMenuButton").count() + code.matches(".menu_button(").count();
        let frame = if popup + context > 0 {
            0
        } else {
            code.matches("Frame::menu(").count()
        };
        popup + context + submenu + frame
    }

    /// 이 파일이 공통 경로를 거친 횟수 — `theme::menu_style` 또는 `widgets::menu_row`.
    ///
    /// **자격 형태를 먼저 세고 그 자리를 지운 뒤 무자격을 센다** — `theme::menu_style(`는
    /// `menu_style(`를 부분 문자열로 품고 있어, 따로 세면 한 호출이 둘로 세어져 검사가
    /// 통과하지만 아무것도 보증하지 못한다
    fn style_calls(source: &str) -> usize {
        let code = code_only(source);
        // egui 자신의 메뉴 스타일은 앱 토큰을 세우지 않는다 — 인정하지 않는다
        let code = code.replace("egui::containers::menu::menu_style(", "");
        let qualified =
            code.matches("theme::menu_style(").count() + code.matches("widgets::menu_row(").count();
        let rest = code
            .replace("theme::menu_style(", "")
            .replace("widgets::menu_row(", "")
            // 정의는 호출이 아니다
            .replace("fn menu_style(", "")
            .replace("fn menu_row(", "");
        let bare = rest.matches("menu_style(").count() + rest.matches("menu_row(").count();
        qualified + bare
    }

    #[test]
    fn 항목_규약_검사는_네_가지_오차를_피한다() {
        // 검사기 자신을 시험한다 — 아래 넷 중 하나만 어긋나도 검사가 통과하면서
        // 아무것도 보증하지 못하는 상태가 된다
        // ⓐ 자격/무자격 이중계수: `theme::menu_style(` 하나는 **1**이어야 한다
        assert_eq!(style_calls("theme::menu_style(ui);"), 1);
        assert_eq!(style_calls("crate::ui::theme::menu_style(ui);"), 1);
        // ⓑ 정의는 호출이 아니다
        assert_eq!(
            style_calls("pub(crate) fn menu_row(ui: &mut Ui) -> bool {}"),
            0
        );
        // ⓒ 주석 줄은 세지 않는다
        assert_eq!(
            menu_openers("// `SubMenuButton`이 hover로 여는 팝업이다"),
            0
        );
        // ⓓ 같은 자리의 팝업 + `Frame::menu(`는 하나로 센다 — 여는 구문이 `Popup::menu(`든
        // 컨텍스트 메뉴든 마찬가지다(앞엣것만 보면 우클릭 메뉴 쪽이 이중으로 세어진다)
        assert_eq!(
            menu_openers("egui::Popup::menu(&r).frame(egui::Frame::menu(s)).show(|ui| {})"),
            1
        );
        assert_eq!(
            menu_openers("r.context_menu(|ui| { egui::Frame::menu(s).show(ui, |ui| {}) });"),
            1
        );

        // 통과해야 할 형태 — 팝업 둘에 호출 둘
        let 성한_파일 = "egui::Popup::menu(&a).show(|ui| { theme::menu_style(ui); });
                         egui::Popup::menu(&b).show(|ui| { theme::menu_style(ui); });";
        assert_eq!(menu_openers(성한_파일), 2);
        assert_eq!(style_calls(성한_파일), 2);
        // 실패해야 할 형태 — 팝업 둘에 호출 하나(한 곳만 고친 상태)
        let 빠뜨린_파일 = "egui::Popup::menu(&a).show(|ui| { theme::menu_style(ui); });
                           egui::Popup::menu(&b).show(|ui| { 항목(ui); });";
        assert!(menu_openers(빠뜨린_파일) > style_calls(빠뜨린_파일));
        // 실패해야 할 형태 — 하위 메뉴만 있고 호출이 없다
        let 하위메뉴만 = "SubMenuButton::from_button(b).ui(ui, |ui| items(ui));";
        assert!(menu_openers(하위메뉴만) > style_calls(하위메뉴만));
        // 시험 모듈의 호출은 세지 않는다 — 세면 그만큼 여유가 생겨 규약이 헐거워진다
        let 시험만_부르는_파일 = "egui::Popup::menu(&a).show(|ui| { 항목(ui); });
\n                                   #[cfg(test)]
\n                                   mod tests { fn t() { theme::menu_style(ui); } }";
        assert_eq!(style_calls(시험만_부르는_파일), 0);
        // 함수 이름은 진입 API가 아니다 — `menu_button(`으로 넓게 찾으면 여기 걸린다
        assert_eq!(menu_openers("fn show_menu_button(ui: &mut Ui) {}"), 0);
        assert_eq!(menu_openers("ui.menu_button(\"파일\", |ui| {});"), 1);
        assert!(menu_openers(시험만_부르는_파일) > style_calls(시험만_부르는_파일));
    }

    #[test]
    fn 팝업_메뉴는_항목_스타일을_거친다() {
        // 규약: 팝업 목록을 여는 자리는 모두 `theme::menu_style`을 거치거나
        // `widgets::menu_row`로 그린다. **개수를 견주는 이유**는 한 파일에 팝업이 여럿인 곳
        // (`sidebar.rs` 셋·`tabs.rs` 둘·`site_manager.rs` 둘)에서 하나만 고쳐도 「있는지」만
        // 보는 검사는 통과하기 때문이다
        let ui_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        let mut sources = Vec::new();
        ui_sources(&ui_dir, &mut sources);
        assert!(!sources.is_empty(), "ui 소스를 하나도 읽지 못했다");

        // 예외는 **경로 전체로** 견준다 — 이름만 보면 하위 폴더의 동명 파일이 조용히 빠진다
        let 예외 = [
            // 이 파일은 규약을 설명하느라 그 문자열을 담고, 시험이 egui 쪽 `menu_style`을 부른다
            ui_dir.join("theme.rs"),
            // 프레임만 열고 한 줄은 `remote_menu`가 그린다 — 그 모듈이 `widgets::menu_row`를 거친다
            ui_dir.join("panel.rs"),
        ];
        let mut 발견 = Vec::new();
        for path in sources {
            if 예외.contains(&path) {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("소스를 읽지 못했다");
            let (열림, 호출) = (menu_openers(&source), style_calls(&source));
            if 열림 > 호출 {
                발견.push(format!(
                    "{}(팝업 {열림} · 호출 {호출})",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
        }
        assert!(
            발견.is_empty(),
            "팝업을 열면서 공통 항목 스타일을 거치지 않은 곳: {발견:?}"
        );
    }

    #[test]
    fn 메뉴_항목_토큰은_정해진_값이다() {
        // 값이 바뀌면 화면의 모든 메뉴가 함께 바뀐다 — 정본이 여기 하나임을 이 시험이 못 박는다
        assert_eq!(MENU_ITEM_HEIGHT, 28.0);
        assert_eq!(MENU_ITEM_PAD_X, 12.0);
        assert_eq!(MENU_ITEM_CORNER_RADIUS, 4);
    }

    #[test]
    fn 메뉴_스타일은_egui_기본_위에_앱_값을_세운다() {
        // **실제 팝업과 같은 순서를 흉내 낸다** — `Popup::menu`가 egui의 메뉴 스타일을 먼저
        // 입히고(좌우 여백 2px·평상시 투명) 그 위에 우리 헬퍼가 온다. 이 순서를 빼고 재면
        // egui가 덮는 값을 우리가 되돌려 세우는지 알 수 없다
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            egui::containers::menu::menu_style(ui.style_mut());
            assert_eq!(
                ui.style().visuals.widgets.inactive.weak_bg_fill,
                egui::Color32::TRANSPARENT,
                "egui 메뉴 스타일이 평상시 줄을 투명으로 둔다는 전제가 깨졌다"
            );

            menu_style(ui);

            let style = ui.style();
            assert_eq!(style.spacing.button_padding.x, MENU_ITEM_PAD_X);
            // 세로 여백이 0이어야 행 높이를 `interact_size.y`가 단독으로 정한다
            assert_eq!(style.spacing.button_padding.y, 0.0);
            assert_eq!(style.spacing.interact_size.y, MENU_ITEM_HEIGHT);

            let corner = egui::CornerRadius::same(MENU_ITEM_CORNER_RADIUS);
            let widgets = &style.visuals.widgets;
            for (이름, state) in [
                ("noninteractive", &widgets.noninteractive),
                ("inactive", &widgets.inactive),
                ("hovered", &widgets.hovered),
                ("active", &widgets.active),
                ("open", &widgets.open),
            ] {
                assert_eq!(state.corner_radius, corner, "{이름} 상태의 모서리");
                assert_eq!(state.expansion, 0.0, "{이름} 상태의 확대");
            }

            // 채움은 세 상태만 — `open`이 빠지면 하위 메뉴가 열린 동안 부모 줄만 색이 다르다
            for (이름, state) in [
                ("hovered", &widgets.hovered),
                ("active", &widgets.active),
                ("open", &widgets.open),
            ] {
                assert_eq!(state.weak_bg_fill, MENU_HOT, "{이름} 상태의 채움");
                assert_eq!(state.bg_fill, MENU_HOT, "{이름} 상태의 배경");
            }

            // 평상시 줄은 투명을 지킨다 — 칠하면 메뉴가 버튼 목록처럼 보인다
            assert_eq!(
                widgets.inactive.weak_bg_fill,
                egui::Color32::TRANSPARENT,
                "평상시 줄에 배경이 생겼다"
            );
        });
    }

    #[test]
    fn 메뉴_스타일은_바깥_ui를_오염시키지_않는다() {
        // 전역이 아니라 국소로 덮는다는 것이 이 헬퍼의 전제다 — 새 나가면 앱 전체 버튼이 커진다
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            let 원래_여백 = ui.style().spacing.button_padding;
            let 원래_높이 = ui.style().spacing.interact_size.y;
            ui.scope(|ui| {
                menu_style(ui);
                assert_eq!(ui.style().spacing.button_padding.x, MENU_ITEM_PAD_X);
            });
            assert_eq!(
                ui.style().spacing.button_padding,
                원래_여백,
                "바깥 Ui의 여백이 함께 바뀌었다"
            );
            assert_eq!(
                ui.style().spacing.interact_size.y,
                원래_높이,
                "바깥 Ui의 최소 높이가 함께 바뀌었다"
            );
        });
    }

    #[test]
    fn 메뉴_모서리는_대화_모서리와_다른_부품이다() {
        // 둘을 한 값으로 묶으면 한쪽을 바꿀 때 다른 쪽이 조용히 따라간다 —
        // 메뉴는 버튼 줄이 없는 얇은 팝업이라 대화(12px)보다 덜 둥글다
        assert_ne!(MENU_CORNER_RADIUS, crate::ui::dialog::CORNER_RADIUS);
    }
}
