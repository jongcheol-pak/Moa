//! 탭 스트립 — 패널별 독립 탭 (FR-3)과 분할 버튼 (FR-1 진입점).
//!
//! 상태는 `panel::tabs::TabsModel`(순수 모델)이 갖고, 이 파일은 그리기와 입력만 한다.
//!
//! 탭 하나는 **한 덩어리 위젯**이다 — 제목과 닫기 버튼을 각각 `Button`으로 두면
//! 저마다 프레임·여백을 그려 두 개의 사각형으로 보인다(Windows 11 탐색기는 한 탭 안에
//! 아이콘·제목·닫기가 들어 있다). 그래서 영역만 잡고 내용은 직접 그린다.
use crate::panel::tabs::{TabId, TabPhase, TabSource, TabsModel};
use crate::remote::connection::TransferDirection;
use crate::remote::types::Protocol;
use crate::remote::types::SiteId;
use crate::ui::menu::{self, Command, PanelMenuState};
use crate::ui::remote_states::{self, RemoteView};
use crate::ui::theme;
use crate::ui::widgets;
use eframe::egui;

// ── 시각 토큰 (plan `## 시각 요소 분해` 1:1, 96DPI 기준 고정 px) ──
/// 탭·버튼의 높이 — 스트립 한 줄의 기준
const STRIP_HEIGHT: f32 = 28.0;
/// 탭 왼쪽 여백
const TAB_PAD_LEFT: f32 = 6.0;
/// 폴더 아이콘 구역 폭
const TAB_ICON_WIDTH: f32 = 16.0;
/// 아이콘과 제목 사이 간격
const TAB_ICON_GAP: f32 = 4.0;
/// 닫기 버튼 구역 폭
const TAB_CLOSE_WIDTH: f32 = 20.0;
/// 제목과 닫기 구역 사이 간격 (사용자 요청) — 붙으면 ✕가 글자에 얹힌 것처럼 보인다
const TAB_LABEL_CLOSE_GAP: f32 = 8.0;
/// 탭 오른쪽 여백
const TAB_PAD_RIGHT: f32 = 4.0;
/// 원격 배지와 이웃(아이콘·이름) 사이 간격 — 원본 `FileExplorer-FTP.dc.html:99`·`:101`의 `margin-left:6px`
const BADGE_MARGIN: f32 = 6.0;
/// 연결되지 않은 원격 탭의 아이콘 불투명도 (README §4 — 연결됨은 1)
const DIM_ICON_ALPHA: f32 = 0.45;
/// 폴더 아이콘 글꼴 크기
const TAB_ICON_PX: f32 = 14.0;
/// 닫기 아이콘 글꼴 크기
const TAB_CLOSE_PX: f32 = 12.0;
/// 새 탭 버튼 폭
const NEW_TAB_WIDTH: f32 = 24.0;
/// 새 탭 `+` 글리프 크기 — 기본 아이콘 크기보다 한 단계 작다 (사용자 요청).
/// 버튼 자리(폭)는 그대로 둔다: 좁히면 누르기 어려워지고 옆 드롭다운과 간격도 무너진다
const NEW_TAB_ICON_PX: f32 = 12.0;
/// 탭 사이 구분선이 차지하는 높이 비율 — 위아래를 띄워 선이 스트립을 가르지 않게 한다
const SEPARATOR_RATIO: f32 = 0.6;

/// 고른 탭의 위쪽 모서리 반경 (사용자 결정) — 아래 두 모서리는 각지게 둔다
const TAB_CORNER_RADIUS: u8 = 6;

// ── 새 탭 메뉴 (FR-33) ──
/// 메뉴 폭 — 사이트 이름과 프로토콜 라벨이 한 줄에 들어가는 크기
const NEW_TAB_MENU_WIDTH: f32 = 250.0;
/// 사이트 줄 글자 크기 — 줄 높이·좌우 여백·hover 모서리는 `theme::MENU_ITEM_*`이 정한다
const SITE_ROW_FONT_PX: f32 = 13.0;
/// 줄 안 상태 점 지름 — 사이드바(7px)보다 한 단계 작다
const SITE_ROW_DOT: f32 = 6.0;
/// 프로토콜 글자 크기
const SITE_PROTO_FONT_PX: f32 = 12.0;
/// 줄 안 요소 사이 간격
const SITE_ROW_GAP: f32 = 8.0;

/// 분할 버튼 아이콘(사각형) 한 변 — 글리프가 아니라 직접 그린다 (split-4way plan D8)
const SPLIT_ICON_SIZE: f32 = 12.0;

/// 분할 버튼 아이콘 선 두께
const SPLIT_ICON_STROKE: f32 = 1.0;

/// 전송 대상 탭과 지금 할 수 있는 일 — 앱이 정해 패널·탭 스트립으로 내려보낸다 (FR-54).
///
/// 화면에 보이는 것과 실제로 가는 곳이 **같은 값에서 나와야** 하므로, 아이콘을 그리는 쪽과
/// 메뉴를 여닫는 쪽과 전송을 만드는 쪽이 모두 이 한 값을 본다.
///
/// 값을 정하는 곳은 `ui::app`인데 **정의는 여기 둔다** — 이 파일이 그리기에 쓰므로 `ui::app`에
/// 두면 `ui::tabs` → `ui::app`이라는 상향 의존이 생긴다. 아래로 흐르는 값은 아래에 둔다
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TransferTargets {
    /// 받기 아이콘을 그릴 로컬 탭
    pub download: Option<TabId>,
    /// 올리기 아이콘을 그릴 원격 탭
    pub upload: Option<TabId>,
    /// 받기가 지금 뜻이 있는가 — 받을 곳이 정해져 있다
    pub can_download: bool,
    /// 올리기가 지금 뜻이 있는가 — 올릴 곳이 연결돼 있고 올릴 것도 골라져 있다
    pub can_upload: bool,
}

/// 탭 스트립이 상위(패널)에 돌려주는 조작.
/// 탭 조작과 메뉴 명령은 서로 독립이라 한 프레임에 함께 나올 수 있다
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct TabStripOutcome {
    pub tab: Option<TabAction>,
    /// 드롭다운에서 고르거나 사이드바에서 끌어다 놓은 사이트 — 새 원격 탭으로 연다 (FR-33·FR-38)
    pub open_site: Option<SiteId>,
    /// 패널 메뉴에서 고른 명령 — 대상은 **이 스트립이 속한 패널**이다 (plan D16)
    pub command: Option<Command>,
}

/// 탭 하나에 대한 조작 — 스트립 전체 결과는 `TabStripOutcome`이 담는다
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TabAction {
    Switch(usize),
    Close(usize),
    New,
}

/// 탭 하나를 구역별로 나눈 결과 — 그리기와 히트 판정이 같은 좌표를 쓰게 한다
#[derive(Clone, Copy, PartialEq, Debug)]
struct TabParts {
    icon: egui::Rect,
    /// 원격 탭의 연결 배지 자리 — 로컬 탭이면 `None`. 아이콘과 이름 **사이**다 (원본 `:99`)
    badge: Option<egui::Rect>,
    label: egui::Rect,
    close: egui::Rect,
}

/// 탭 하나에서 이번 프레임에 일어난 일
#[derive(Clone, Copy, PartialEq, Debug)]
struct TabHit {
    switch: bool,
    close: bool,
    /// 그려진 탭 영역 — 구분선을 어디에 그릴지 정하는 데 쓴다
    rect: egui::Rect,
}

/// 탭 스트립을 그리고 이번 프레임의 조작을 반환한다.
///
/// 탭이 많으면 왼쪽 영역이 가로로 스크롤되고, **메뉴 버튼은 오른쪽 끝에 고정**된다 —
/// 버튼까지 스크롤되면 탭이 늘어날수록 메뉴에 닿기 어려워진다 (split-4way plan D6)
pub fn show_tab_strip(
    ui: &mut egui::Ui,
    model: &TabsModel,
    remote: RemoteView<'_>,
    menu_state: PanelMenuState,
    targets: TransferTargets,
) -> TabStripOutcome {
    let mut outcome = TabStripOutcome::default();
    // 두 클로저가 `outcome`을 동시에 빌리지 않도록 필드를 나눠 넘긴다
    let TabStripOutcome {
        tab,
        open_site,
        command,
    } = &mut outcome;
    egui::Sides::new().shrink_left().height(STRIP_HEIGHT).show(
        ui,
        |ui| show_tabs(ui, model, remote, targets, tab, open_site),
        |ui| show_menu_button(ui, &menu_state, command),
    );
    // 사이드바에서 끌어온 사이트를 이 스트립에 놓으면 새 탭으로 연다 (FR-38·인벤토리 #15).
    // 스트립 전체가 드롭 대상이라 탭 사이 어디에 놓아도 받는다
    if let Some(site) = dropped_site(ui) {
        outcome.open_site = Some(site);
    }
    outcome
}

/// 이번 프레임에 이 스트립 위로 놓인 사이트 — 없으면 `None`.
///
/// egui의 드래그·드롭 페이로드를 쓴다: 사이드바가 `SiteId`를 실어 보내고 여기서 받는다.
/// 좌표를 직접 비교하지 않는 이유는, 스트립이 스크롤 안에 있어 화면 좌표가 흔들리기 때문이다
fn dropped_site(ui: &egui::Ui) -> Option<SiteId> {
    let rect = ui.min_rect();
    let pointer = ui.ctx().pointer_interact_pos()?;
    if !rect.contains(pointer) {
        return None;
    }
    // 놓는 순간에만 받는다 — 끌고 지나가는 동안에는 아무 일도 일어나지 않는다
    if !ui.input(|i| i.pointer.any_released()) {
        return None;
    }
    egui::DragAndDrop::take_payload::<SiteId>(ui.ctx()).map(|site| *site)
}

/// 탭 목록과 새 탭 버튼 — 폭이 모자라면 가로로 스크롤된다.
///
/// 스크롤 막대는 그리지 않는다 — egui의 떠 있는 막대는 **내용 위에** 겹쳐 그려지는데,
/// 스트립은 높이가 `STRIP_HEIGHT` 한 줄뿐이라 막대가 탭 제목을 가로질러 덮는다.
/// 대신 세로 휠을 가로 스크롤로 넘겨(`always_scroll_the_only_direction`) 막대 없이도
/// 휠만으로 탭을 넘길 수 있게 한다 (브라우저·탐색기 탭과 같은 조작)
fn show_tabs(
    ui: &mut egui::Ui,
    model: &TabsModel,
    remote: RemoteView<'_>,
    targets: TransferTargets,
    action: &mut Option<TabAction>,
    open_site: &mut Option<SiteId>,
) {
    // `ScrollArea`가 이 값을 부모 `Ui`에서 읽으므로 만들기 전에 세운다 (이 스트립 안에만 적용)
    ui.style_mut().always_scroll_the_only_direction = true;
    egui::ScrollArea::horizontal()
        .auto_shrink([false, true])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            // 탭끼리 붙어야 한 줄로 이어져 보인다 — 사이를 벌리면 다시 낱개 버튼처럼 보인다
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.horizontal(|ui| {
                let tabs = model.tabs();
                let active_index = model.active_index();
                for (index, tab) in tabs.iter().enumerate() {
                    let source = &tab.source;
                    let active = index == active_index;
                    // 원격 탭의 이름·프로토콜은 사이트 설정에서 그때그때 읽는다 —
                    // 탭이 사본을 들면 `이름 바꾸기(R)` 뒤에 탭만 옛 이름으로 남는다 (T7 결정)
                    let site = source.site().and_then(|id| remote.sites.get(id));
                    let title = source.title(site.map(|record| record.name.as_str()));
                    // 사이트가 지워진 원격 탭에는 배지를 그리지 않는다 — 프로토콜을 모르는 채
                    // 아무 값이나 보이면 화면이 서버에 대해 없는 말을 하게 된다
                    // (제목의 `알 수 없는 사이트`가 이미 그 상태를 알린다)
                    let badge = match source {
                        TabSource::Remote { phase, .. } => {
                            site.map(|record| (phase, record.protocol))
                        }
                        TabSource::Local(_) | TabSource::Zip { .. } => None,
                    };
                    let hit = show_tab(ui, index, &title, badge, active, tab_icon(tab.id, targets));
                    if hit.close {
                        *action = Some(TabAction::Close(index));
                    } else if hit.switch {
                        *action = Some(TabAction::Switch(index));
                    }
                    // 구분선은 **양옆이 모두 비활성일 때만** 그린다 —
                    // 활성 탭은 배경 자체가 경계 역할을 해서 선까지 그으면 지저분해진다.
                    // 마지막 탭 뒤에도 같은 규칙으로 하나 둔다 — 탭 줄과 `+`를 가르는 선이다
                    // (사용자 요청). 활성 탭 옆이면 여기서도 긋지 않는다
                    let next = index + 1;
                    let before_plus = next == tabs.len();
                    if !active && (before_plus || next != active_index) {
                        draw_separator(ui.painter(), hit.rect);
                    }
                }
                show_new_tab_menu(ui, remote, action, open_site);
            });
        });
}

/// `+` 버튼과 그 메뉴 — `새 탭`, 그리고 등록된 사이트를 새 탭으로 여는 줄들 (FR-33).
///
/// 종전에는 `+`가 곧바로 새 탭을 만들고 그 옆의 `▾`가 사이트 목록을 열었다. 버튼 두 개가
/// 나란히 서면 무엇이 무엇인지 눌러 봐야 알 수 있어, 진입점을 이 메뉴 하나로 합쳤다
/// (2026-08-19 사용자 요청). 여는 동작 자체는 그대로다 — 사이트 줄은 종전 `▾`와 같이
/// `open_site`로 나가 그 패널의 새 원격 탭이 된다.
///
/// **등록된 사이트가 없으면 `새 탭` 한 줄만 남는다** — 구분선과 빈 목록을 그리면
/// 무언가 빠진 것처럼 보인다
fn show_new_tab_menu(
    ui: &mut egui::Ui,
    remote: RemoteView<'_>,
    action: &mut Option<TabAction>,
    open_site: &mut Option<SiteId>,
) {
    let response = widgets::icon_button_styled(
        ui,
        egui_phosphor::regular::PLUS,
        egui::vec2(NEW_TAB_WIDTH, STRIP_HEIGHT),
        theme::CONTROL_HOT,
        theme::TEXT,
        NEW_TAB_ICON_PX,
    )
    // 안내는 `새 탭`이 아니다 — 이 버튼은 탭을 만들지 않고 메뉴를 연다.
    // 메뉴 첫 항목과 같은 글자를 쓰면 누르는 즉시 탭이 생기는 것처럼 읽힌다
    .on_hover_text(crate::i18n::tabs_new_menu());
    egui::Popup::menu(&response).show(|ui| {
        theme::menu_style(ui);
        new_tab_menu_items(ui, remote, action, open_site);
    });
}

/// 새 탭 메뉴의 내용 — 팝업 껍데기와 나눠 두어 시험이 이 함수를 직접 그릴 수 있게 한다
/// (`Popup::menu`는 눌러야 열리는 두 단계라 시험에서 좌표를 흉내 내기 어렵다 —
/// 설정 대화가 같은 이유로 같은 형태를 쓴다)
fn new_tab_menu_items(
    ui: &mut egui::Ui,
    remote: RemoteView<'_>,
    action: &mut Option<TabAction>,
    open_site: &mut Option<SiteId>,
) {
    ui.set_width(NEW_TAB_MENU_WIDTH);
    if ui.button(crate::i18n::tabs_new()).clicked() {
        *action = Some(TabAction::New);
        ui.close();
    }
    let mut first = true;
    for record in remote.sites.visible() {
        // 구분선은 사이트가 하나라도 있을 때 **한 번만** 긋는다
        if first {
            ui.separator();
            first = false;
        }
        if show_site_row(
            ui,
            &record.name,
            record.protocol.label(),
            remote.is_connected(record.id),
        ) {
            *open_site = Some(record.id);
            ui.close();
        }
    }
}

/// 메뉴의 사이트 한 줄 — 상태 점 · 이름 · 프로토콜. 눌렸으면 `true`.
///
/// `ui.button`을 쓰지 않는 이유: 한 줄에 점·이름·프로토콜 셋이 서로 다른 정렬로 앉아야
/// 하는데, 버튼 라벨 하나로는 그 배치를 만들 수 없다
fn show_site_row(ui: &mut egui::Ui, name: &str, protocol: &str, connected: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), theme::MENU_ITEM_HEIGHT),
        egui::Sense::click(),
    );
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, theme::MENU_ITEM_CORNER_RADIUS, theme::MENU_HOT);
    }
    let painter = ui.painter();
    let dot_center = egui::pos2(
        rect.left() + theme::MENU_ITEM_PAD_X + SITE_ROW_DOT / 2.0,
        rect.center().y,
    );
    painter.circle_filled(
        dot_center,
        SITE_ROW_DOT / 2.0,
        if connected {
            theme::OK_DOT
        } else {
            theme::TEXT_DIM
        },
    );

    // 프로토콜을 먼저 오른쪽에 붙여 이름이 길어도 밀려나지 않게 한다
    let proto = painter.layout_no_wrap(
        protocol.to_owned(),
        egui::FontId::proportional(SITE_PROTO_FONT_PX),
        theme::TEXT_MUTED,
    );
    let proto_left = rect.right() - theme::MENU_ITEM_PAD_X - proto.size().x;
    painter.galley(
        egui::pos2(proto_left, rect.center().y - proto.size().y / 2.0),
        proto,
        theme::TEXT_MUTED,
    );

    let name_left = dot_center.x + SITE_ROW_DOT / 2.0 + SITE_ROW_GAP;
    let name = painter.layout(
        name.to_owned(),
        egui::FontId::proportional(SITE_ROW_FONT_PX),
        theme::TEXT,
        (proto_left - SITE_ROW_GAP - name_left).max(0.0),
    );
    painter.galley(
        egui::pos2(name_left, rect.center().y - name.size().y / 2.0),
        name,
        theme::TEXT,
    );
    response.clicked()
}

/// 탭 하나를 그린다 — 폴더 아이콘·제목·닫기 ×가 한 영역 안에 들어간다.
///
/// 내부 요소는 위젯이 아니라 painter로 그린다. `icon_button` 같은 위젯을 쓰면
/// 자기 자리를 새로 잡아 탭 밖에 배치되고, 그 자리에서 탭 클릭도 삼켜진다
fn show_tab(
    ui: &mut egui::Ui,
    index: usize,
    title: &str,
    badge: Option<(&TabPhase, Protocol)>,
    active: bool,
    icon: (&'static str, egui::Color32),
) -> TabHit {
    let text = elide(title);
    let font = egui::TextStyle::Button.resolve(ui.style());
    let text_width = ui
        .painter()
        .layout_no_wrap(text.clone(), font.clone(), theme::TEXT)
        .size()
        .x;
    // 배지는 제 폭을 먼저 떼어 간다 — 탭이 좁아지면 이름이 먼저 줄어든다 (plan Edge Case)
    let badge_width = badge.map_or(0.0, |(phase, protocol)| {
        remote_states::badge_width(ui, phase, protocol)
    });
    // 제목이 0폭이어도 아이콘·배지·닫기 구역은 남는다 (tab_parts의 전제)
    let width = min_width(badge_width) + text_width;
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, STRIP_HEIGHT), egui::Sense::click());
    let parts = tab_parts(rect, badge_width);

    // 활성 탭만 배경을 채운다 — 비활성은 스트립 배경 그대로 둔다.
    // **위쪽 두 모서리만** 둥글다: 아래는 같은 색 주소창 줄과 이어져야 해서 각져 있어야 한다
    // (둥글리면 그 자리에 배경색 틈이 생겨 탭이 줄에서 떠 보인다 — Windows 11 탐색기와 같은 모양)
    if active {
        let corner = egui::CornerRadius {
            nw: TAB_CORNER_RADIUS,
            ne: TAB_CORNER_RADIUS,
            sw: 0,
            se: 0,
        };
        ui.painter().rect_filled(rect, corner, theme::CONTROL_BG);
    }
    let (glyph, base_color) = icon;
    let icon_color = icon_color(base_color, badge);
    let painter = ui.painter();
    painter.text(
        parts.icon.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(TAB_ICON_PX),
        icon_color,
    );
    painter.text(
        parts.label.left_center(),
        egui::Align2::LEFT_CENTER,
        &text,
        font,
        theme::TEXT,
    );
    if let (Some(rect), Some((phase, protocol))) = (parts.badge, badge) {
        remote_states::show_badge(ui, rect, phase, protocol);
    }

    // 닫기 구역은 탭 위에 얹힌 별도 위젯이다 — 나중에 등록해야 탭보다 클릭이 우선한다
    let close = ui.interact(
        parts.close,
        ui.id().with(("tab_close", index)),
        egui::Sense::click(),
    );
    if close.hovered() {
        widgets::hover_backdrop(ui.painter(), parts.close, theme::CONTROL_HOT);
    }
    ui.painter().text(
        parts.close.center(),
        egui::Align2::CENTER_CENTER,
        egui_phosphor::regular::X,
        egui::FontId::proportional(TAB_CLOSE_PX),
        theme::TEXT,
    );
    let close_clicked = close.clicked();
    close.on_hover_text(crate::i18n::tabs_close());

    // 닫기 자리를 누른 클릭은 전환으로 세지 않는다. 위 `interact`가 이미 가로채지만,
    // 히트 우선순위가 아니라 좌표로도 한 번 더 막는다 — 이 규칙이 깨지면
    // ×를 눌러도 탭만 바뀌는 회귀가 조용히 들어온다
    let clicked_on_close = response
        .interact_pointer_pos()
        .is_some_and(|pos| is_close_hit(&parts, pos));
    let switch = response.clicked() && !clicked_on_close;
    // 가운데 버튼 클릭으로도 닫는다 (브라우저 관례)
    let middle_close = response.middle_clicked();
    // 마우스를 올리면 줄임표 없는 탭 이름 전체가 보인다 (경로는 주소창이 보여 준다)
    response.on_hover_text(title);

    TabHit {
        switch,
        close: close_clicked || middle_close,
        rect,
    }
}

/// 아이콘을 얼마나 또렷하게 그릴지.
///
/// 연결된 원격 탭만 또렷하다 — 로컬 탭은 늘 또렷하다 (README §4). **전송 대상 아이콘도 같은
/// 규칙을 따른다**: 끊긴 서버는 올릴 수 있는 곳이 아니므로 그 화살표도 흐려야 한다
fn icon_color(base: egui::Color32, badge: Option<(&TabPhase, Protocol)>) -> egui::Color32 {
    match badge {
        None | Some((TabPhase::Ok, _)) => base,
        Some(_) => base.gamma_multiply(DIM_ICON_ALPHA),
    }
}

/// 이 탭의 아이콘 자리에 무엇을 그릴지 (FR-54).
///
/// 전송 대상 탭은 폴더 대신 **방향 화살표**를 단다 — 큐 화면이 이미 쓰는 것과 같은 글리프·색이라
/// (`widgets::direction_mark`) "받는 곳"·"올리는 곳"의 뜻이 화면마다 갈리지 않는다.
/// 색이 폴더의 노랑에서 벗어나므로 여러 탭 중 대상이 한눈에 띈다
fn tab_icon(id: TabId, targets: TransferTargets) -> (&'static str, egui::Color32) {
    if targets.download == Some(id) {
        widgets::direction_mark(TransferDirection::Download)
    } else if targets.upload == Some(id) {
        widgets::direction_mark(TransferDirection::Upload)
    } else {
        (egui_phosphor::regular::FOLDER, theme::FOLDER_ICON)
    }
}

/// 제목이 0폭일 때의 탭 폭 — 아이콘·(배지)·닫기 구역만 남은 상태다.
///
/// `badge_width`가 0이면 로컬 탭이라 배지 자리를 잡지 않는다
fn min_width(badge_width: f32) -> f32 {
    let after_icon = if badge_width > 0.0 {
        BADGE_MARGIN * 2.0 + badge_width
    } else {
        TAB_ICON_GAP
    };
    TAB_PAD_LEFT
        + TAB_ICON_WIDTH
        + after_icon
        + TAB_LABEL_CLOSE_GAP
        + TAB_CLOSE_WIDTH
        + TAB_PAD_RIGHT
}

/// 탭 영역을 아이콘·(배지)·제목·닫기 구역으로 나눈다.
///
/// **폭이 `min_width(badge_width)` 이상임을 전제한다**(호출부가 보장) — 여기서 다시 늘리면
/// 반환 구역이 입력 영역을 벗어나 "닫기 구역은 탭 안에 있다"는 계약이 깨진다
fn tab_parts(rect: egui::Rect, badge_width: f32) -> TabParts {
    let icon = egui::Rect::from_min_size(
        egui::pos2(rect.left() + TAB_PAD_LEFT, rect.top()),
        egui::vec2(TAB_ICON_WIDTH, rect.height()),
    );
    let close = egui::Rect::from_min_size(
        egui::pos2(rect.right() - TAB_PAD_RIGHT - TAB_CLOSE_WIDTH, rect.top()),
        egui::vec2(TAB_CLOSE_WIDTH, rect.height()),
    );
    // 배지는 아이콘과 제목 사이에 앉는다 (원본 `:99` — 제목 오른쪽이 아니다)
    let (badge, label_left) = if badge_width > 0.0 {
        let badge = egui::Rect::from_min_size(
            egui::pos2(icon.right() + BADGE_MARGIN, rect.top()),
            egui::vec2(badge_width, rect.height()),
        );
        (Some(badge), badge.right() + BADGE_MARGIN)
    } else {
        (None, icon.right() + TAB_ICON_GAP)
    };
    // 제목은 닫기 구역에 닿기 전에 끝난다 — 붙어 있으면 ✕가 글자에 얹힌 것처럼 보인다 (사용자 요청)
    let label_right = (close.left() - TAB_LABEL_CLOSE_GAP).max(label_left);
    let label = egui::Rect::from_min_max(
        egui::pos2(label_left, rect.top()),
        egui::pos2(label_right, rect.bottom()),
    );
    TabParts {
        icon,
        badge,
        label,
        close,
    }
}

/// 포인터가 닫기 구역 안인가 — 닫기가 탭 전환보다 우선한다는 규칙의 정본
fn is_close_hit(parts: &TabParts, pos: egui::Pos2) -> bool {
    parts.close.contains(pos)
}

/// 비활성 탭 사이 구분선 — 탭 오른쪽 경계에 세로로 짧게 긋는다
fn draw_separator(painter: &egui::Painter, tab: egui::Rect) {
    let half = tab.height() * SEPARATOR_RATIO / 2.0;
    let center = tab.center().y;
    painter.vline(
        tab.right(),
        center - half..=center + half,
        egui::Stroke::new(1.0, theme::TREE_LINE),
    );
}

/// 패널 메뉴 버튼 — 누르면 보기·분할·새로 고침·새 파일/폴더·닫기 메뉴가 뜬다 (FR-26).
///
/// `MenuButton`을 쓰지 않는 이유: 그것은 `Button` 위젯을 요구해 프레임을 함께 그린다.
/// 타이틀바 설정 버튼과 같이 아이콘 버튼에 팝업만 붙인다.
/// **아이콘은 분할 도형 그대로 둔다** — 사용자가 이미 이 자리를 메뉴 진입점으로 쓰고 있어
/// 모양이 바뀌면 찾지 못한다 (plan D10)
fn show_menu_button(ui: &mut egui::Ui, state: &PanelMenuState, command: &mut Option<Command>) {
    let response = widgets::icon_button(
        ui,
        "",
        egui::vec2(STRIP_HEIGHT, STRIP_HEIGHT),
        theme::CONTROL_HOT,
    )
    .on_hover_text(crate::i18n::tabs_menu());
    draw_split_icon(ui.painter(), response.rect);
    egui::Popup::menu(&response).show(|ui| {
        theme::menu_style(ui);
        menu::panel_menu_items(ui, state, command);
    });
}

/// 메뉴 버튼 아이콘 — 사각형 테두리와 세로 중앙선(분할을 뜻하던 도형을 그대로 쓴다).
/// 글리프(`◫`)를 쓰지 않는 이유: 폰트에 그 문자가 없으면 두부(□)로 보이는데,
/// 폰트 지원 여부는 화면을 띄우기 전에는 알 수 없다 (split-4way plan D8)
fn draw_split_icon(painter: &egui::Painter, rect: egui::Rect) {
    let icon = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(SPLIT_ICON_SIZE));
    let stroke = egui::Stroke::new(SPLIT_ICON_STROKE, theme::TEXT);
    painter.rect_stroke(icon, 0.0, stroke, egui::StrokeKind::Inside);
    painter.vline(icon.center().x, icon.y_range(), stroke);
}

/// 탭 제목 말줄임 — 긴 폴더명이 스트립을 다 차지하지 않게 한다
fn elide(title: &str) -> String {
    // 문자 수 기준 — 한글은 폭이 넓어 대략 절반으로 잡는다
    const MAX_CHARS: usize = 16;
    let count = title.chars().count();
    if count <= MAX_CHARS {
        return title.to_owned();
    }
    let kept: String = title.chars().take(MAX_CHARS - 1).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 탭 영역 하나 — 왼쪽 위 모서리를 원점에서 떼어 좌표 계산 실수를 드러낸다
    fn tab_rect(width: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(width, STRIP_HEIGHT))
    }

    #[test]
    fn 짧은_제목은_그대로_둔다() {
        assert_eq!(elide("문서"), "문서");
        assert_eq!(elide("Downloads"), "Downloads");
    }

    #[test]
    fn 긴_제목은_말줄임한다() {
        let long = "아주아주아주아주아주아주긴폴더이름입니다";
        let out = elide(long);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 16);
    }

    #[test]
    fn 경계_길이는_자르지_않는다() {
        let exact: String = "가".repeat(16);
        assert_eq!(elide(&exact), exact);
    }

    /// 원격 탭의 배지 폭 — 실제 글꼴 폭 대신 고정값으로 배치 계약만 본다
    const SAMPLE_BADGE: f32 = 46.0;

    #[test]
    fn 세_구역은_서로_겹치지_않는다() {
        for badge in [0.0, SAMPLE_BADGE] {
            for width in [min_width(badge), min_width(badge) + 60.0, 300.0] {
                let rect = tab_rect(width);
                let parts = tab_parts(rect, badge);
                assert!(
                    parts.icon.right() <= parts.label.left(),
                    "폭 {width}(배지 {badge}): 아이콘과 제목이 겹친다"
                );
                assert!(
                    parts.label.right() <= parts.close.left(),
                    "폭 {width}(배지 {badge}): 제목과 닫기가 겹친다"
                );
                if let Some(badge_rect) = parts.badge {
                    assert!(
                        parts.icon.right() <= badge_rect.left(),
                        "폭 {width}: 아이콘과 배지가 겹친다"
                    );
                    assert!(
                        badge_rect.right() <= parts.label.left(),
                        "폭 {width}: 배지와 제목이 겹친다"
                    );
                }
            }
        }
    }

    #[test]
    fn 배지는_아이콘과_제목_사이에_앉는다() {
        // 원본 `:99`의 배치다 — 제목 오른쪽에 두면 이름이 길 때 배지가 화면 밖으로 밀린다
        let parts = tab_parts(tab_rect(300.0), SAMPLE_BADGE);
        let badge = parts.badge.expect("원격 탭에는 배지 자리가 있다");
        assert_eq!(badge.width(), SAMPLE_BADGE);
        assert!(badge.left() < parts.label.left());
        // 로컬 탭에는 배지 자리를 잡지 않는다
        assert!(tab_parts(tab_rect(300.0), 0.0).badge.is_none());
    }

    #[test]
    fn 닫기_구역은_항상_탭_안에_있다() {
        // 탭이 좁아져 닫기가 밖으로 밀리면 탭을 닫을 수 없게 된다
        for badge in [0.0, SAMPLE_BADGE] {
            for width in [min_width(badge), min_width(badge) + 60.0, 300.0] {
                let rect = tab_rect(width);
                let parts = tab_parts(rect, badge);
                assert!(
                    rect.contains_rect(parts.close),
                    "폭 {width}(배지 {badge}): 닫기 구역이 탭을 벗어났다"
                );
            }
        }
    }

    #[test]
    fn 최소_폭에서도_아이콘과_배지와_닫기_크기가_유지된다() {
        // 좁아질 때 줄어드는 것은 제목뿐이다 — 아이콘·배지·닫기가 함께 찌그러지면 읽을 수 없다
        let parts = tab_parts(tab_rect(min_width(0.0)), 0.0);
        assert_eq!(parts.icon.width(), TAB_ICON_WIDTH);
        assert_eq!(parts.close.width(), TAB_CLOSE_WIDTH);
        assert_eq!(parts.label.width(), 0.0);

        let remote = tab_parts(tab_rect(min_width(SAMPLE_BADGE)), SAMPLE_BADGE);
        assert_eq!(remote.icon.width(), TAB_ICON_WIDTH);
        assert_eq!(remote.close.width(), TAB_CLOSE_WIDTH);
        assert_eq!(
            remote.badge.map(|b| b.width()),
            Some(SAMPLE_BADGE),
            "배지가 제 폭을 잃었다"
        );
        assert_eq!(remote.label.width(), 0.0);
    }

    #[test]
    fn 닫기_구역_안의_클릭만_닫기로_친다() {
        // 이 판정이 뒤집히면 ×를 눌러도 탭 전환만 되는 회귀가 들어온다
        let parts = tab_parts(tab_rect(200.0), 0.0);
        assert!(is_close_hit(&parts, parts.close.center()));
        assert!(!is_close_hit(&parts, parts.label.center()));
        assert!(!is_close_hit(&parts, parts.icon.center()));
    }

    /// 전송 대상 판정용 탭 신원 — 실제 탭 없이 값만 만든다
    fn ids() -> (TabId, TabId, TabId) {
        let a = crate::panel::tabs::TabState::new(std::path::PathBuf::from(r"C:\a"));
        let b = crate::panel::tabs::TabState::new(std::path::PathBuf::from(r"C:\b"));
        let c = crate::panel::tabs::TabState::new(std::path::PathBuf::from(r"C:\c"));
        (a.id, b.id, c.id)
    }

    #[test]
    fn 받기_대상_탭은_받기_화살표를_단다() {
        // 큐 화면과 같은 글리프·색이어야 "받는 곳"의 뜻이 화면마다 갈리지 않는다 (D9)
        let (down, up, plain) = ids();
        let targets = TransferTargets {
            download: Some(down),
            upload: Some(up),
            can_download: true,
            can_upload: true,
        };
        assert_eq!(
            tab_icon(down, targets),
            widgets::direction_mark(TransferDirection::Download)
        );
        assert_eq!(
            tab_icon(up, targets),
            widgets::direction_mark(TransferDirection::Upload)
        );
        // 대상이 아닌 탭은 폴더 그대로다
        assert_eq!(
            tab_icon(plain, targets),
            (egui_phosphor::regular::FOLDER, theme::FOLDER_ICON)
        );
    }

    #[test]
    fn 대상이_없으면_모든_탭이_폴더다() {
        let (a, _, _) = ids();
        assert_eq!(
            tab_icon(a, TransferTargets::default()),
            (egui_phosphor::regular::FOLDER, theme::FOLDER_ICON)
        );
    }

    #[test]
    fn 끊긴_원격_탭은_대상이어도_아이콘이_흐리다() {
        // 끊긴 서버는 올릴 수 있는 곳이 아니다 — 화살표만 또렷하면 눌러도 되는 것처럼 보인다
        let up = widgets::direction_mark(TransferDirection::Upload).1;
        let 또렷 = icon_color(up, Some((&TabPhase::Ok, Protocol::Ftp)));
        let 흐림 = icon_color(up, Some((&TabPhase::Connecting, Protocol::Ftp)));
        assert_eq!(또렷, up);
        assert_ne!(흐림, up, "끊긴 원격 탭의 대상 아이콘이 흐려지지 않았다");
        // 로컬 탭(배지 없음)은 늘 또렷하다
        assert_eq!(icon_color(up, None), up);
    }

    /// 새 탭 메뉴 내용을 그리고 그 안에 실린 글자를 모은다.
    ///
    /// 팝업을 눌러 여는 대신 내용 함수를 직접 그린다 — 무엇이 메뉴에 실렸는지는
    /// 그려진 글자로 판정하는 것이 가장 확실하다
    fn menu_labels(sites: &crate::remote::sites::SiteStore) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut labels = Vec::new();
        let output = ctx.run_ui(Default::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let tree = crate::remote::tree_cache::TreeCache::new();
                new_tab_menu_items(
                    ui,
                    RemoteView {
                        sites,
                        connected: &[],
                        tree: &tree,
                    },
                    &mut None,
                    &mut None,
                );
            });
        });
        for shape in output.shapes {
            collect_text(&shape.shape, &mut labels);
        }
        labels
    }

    /// 도형 트리를 훑어 글자만 모은다 — 메뉴 항목은 중첩 도형 안에 들어 있다
    fn collect_text(shape: &egui::epaint::Shape, out: &mut Vec<String>) {
        match shape {
            egui::epaint::Shape::Text(text) => out.push(text.galley.text().to_owned()),
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, out);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn 사이트가_있으면_메뉴가_그_사이트를_싣는다() {
        // 종전에는 `▾` 드롭다운이 하던 일이다 — `+` 메뉴로 옮겨도 목록은 그대로 나와야 한다
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        let mut sites = crate::remote::sites::SiteStore::new();
        let id = sites.add("배포 서버");
        // 호스트가 있어야 고르는 자리에 선다 (FR-29)
        if let Some(site) = sites.get_mut(id) {
            site.host = "example.test".to_owned();
        }
        let labels = menu_labels(&sites);
        assert!(
            labels.iter().any(|label| label == "새 탭"),
            "`새 탭`이 첫 항목으로 있어야 한다: {labels:?}"
        );
        assert!(
            labels.iter().any(|label| label == "배포 서버"),
            "등록한 사이트가 메뉴에 없다: {labels:?}"
        );
    }

    #[test]
    fn 사이트가_없으면_새_탭만_남는다() {
        // 빈 목록과 구분선을 그리면 무언가 빠진 것처럼 보인다
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        let labels = menu_labels(&crate::remote::sites::SiteStore::new());
        assert_eq!(labels, vec!["새 탭".to_owned()], "메뉴에 다른 것이 실렸다");

        // 숨긴 사이트만 남아도 같다 — 목록이 비는 것과 구분되지 않는다
        let mut hidden = crate::remote::sites::SiteStore::new();
        let id = hidden.add("배포 서버");
        // **호스트를 채워야 「숨김」만이 사라진 사유가 된다** (FR-29)
        if let Some(site) = hidden.get_mut(id) {
            site.host = "example.test".to_owned();
        }
        hidden.hide(id);
        assert_eq!(menu_labels(&hidden), vec!["새 탭".to_owned()]);
    }

    #[test]
    fn 두_방향_아이콘은_서로_다르다() {
        // 같은 글리프·색이면 받는 곳과 올리는 곳을 구분할 수 없다
        let down = widgets::direction_mark(TransferDirection::Download);
        let up = widgets::direction_mark(TransferDirection::Upload);
        assert_ne!(down.0, up.0);
        assert_ne!(down.1, up.1);
        // 폴더 아이콘과도 달라야 대상이 눈에 띈다
        assert_ne!(down.1, theme::FOLDER_ICON);
        assert_ne!(up.1, theme::FOLDER_ICON);
        // 아이콘 글꼴에서 온 글리프여야 두부로 그려지지 않는다 (AGENTS 규약)
        assert!(widgets::is_icon_font(down.0));
        assert!(widgets::is_icon_font(up.0));
    }
}
