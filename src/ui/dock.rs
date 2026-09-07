//! 창 아래에 붙는 도크 — 전송 큐와 서버 로그가 **같은 자리를 번갈아 쓴다** (FR-36·FR-40·D19).
//!
//! 원본 `FileExplorer-FTP.dc.html:262-271`(큐)·`:299-307`(로그). 두 화면의 셸(268px 높이·
//! 28px 탭 스트립·우측 아이콘 버튼)이 같아서 여기 한 벌만 둔다 — 각자 그리면 탭 줄이
//! 화면마다 조금씩 어긋난다.
//!
//! **본문은 여기서 그리지 않는다** — 큐 표는 `ui::queue_panel`, 로그는 `ui::log_panel`(T20)이
//! 그린다. 이 모듈은 자리(높이·탭·닫기)만 정한다.
use crate::remote::connection::TransferId;
use crate::remote::queue::{QueueFilter, TransferQueue};
use crate::remote::types::SiteId;
use crate::ui::theme;
use crate::ui::widgets;
use eframe::egui;
use std::collections::HashSet;

// ── 시각 토큰 (원본 `:262-270`) ──
/// 도크 높이 — 원본이 고정으로 정한 값
pub const DOCK_HEIGHT: f32 = 268.0;
/// 도크가 열려도 패널 그리드에 남겨 두는 최소 높이 (plan Edge Case: 창이 낮을 때)
const GRID_MIN_HEIGHT: f32 = 120.0;
/// 탭 스트립 — 호출부가 본문 자리를 계산할 때 함께 쓴다
pub const STRIP_HEIGHT: f32 = 28.0;
const STRIP_PAD_RIGHT: f32 = 6.0;
const TAB_PAD_X: f32 = 14.0;
const TAB_FONT_PX: f32 = 13.0;
/// 우측 아이콘 버튼
const ICON_SIZE: f32 = 26.0;
const ICON_FONT_PX: f32 = 13.0;
/// 닫기(`▼`)만 한 단계 크다 (`:270`)
const CLOSE_FONT_PX: f32 = 14.0;

// ── 아이콘 (인벤토리 #33~#34) — 탭 문구 #29~#32는 카탈로그로 옮겼다 ──
/// 큐 패널 우측 버튼 (인벤토리 #33) · 로그 패널 우측 버튼 (인벤토리 #34).
///
/// **아이콘 글꼴(phosphor)에서 가져온다** — 원본 HTML의 글리프(`⏸`·`✕`·`▼`·`⧉`)를 그대로 쓰면
/// 이 앱의 글꼴(맑은 고딕 + phosphor)에 그 부호점이 없어 **두부(`?`)로 보인다**
/// (2026-08-05 화면 확인 — 같은 함정을 메뉴 화살표에서도 겪었다)
const ICON_PAUSE: &str = egui_phosphor::regular::PAUSE;
/// 멈춘 뒤에는 **다시 시작**을 뜻하는 모양으로 바뀐다 — 멈춘 상태에서 일시 정지 표시를
/// 그대로 두면 아이콘도 툴팁도 지금 상태와 어긋난다
const ICON_PLAY: &str = egui_phosphor::regular::PLAY;
const ICON_CLEAR: &str = egui_phosphor::regular::BROOM;
const ICON_COLLAPSE: &str = egui_phosphor::regular::CARET_DOWN;
const ICON_COPY: &str = egui_phosphor::regular::COPY;

/// 도크가 지금 보이는 화면 — **둘은 같은 자리를 배타로 쓴다** (D19)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockPanel {
    Queue,
    Log,
}

/// 도크의 화면 상태 — 어떤 화면을, 어떤 필터·사이트로 보고 있는가.
///
/// 앱이 들고 프레임마다 넘긴다. 큐·로그 모듈이 각자 들면 탭을 옮길 때 두 상태가 어긋난다
#[derive(Debug, Clone, PartialEq)]
pub struct DockState {
    /// `None`이면 접혀 있다 (상태 표시줄의 캐럿이 다시 편다 — T21)
    pub panel: Option<DockPanel>,
    /// 큐 화면의 성공·실패 필터 (인벤토리 #31·#32)
    pub filter: QueueFilter,
    /// 연결별 탭에서 고른 사이트 — `None`이면 `전체` (인벤토리 #35).
    ///
    /// **로그 화면도 이 값을 쓴다** — 그 사이트의 연결 로그를 보인다. 두 화면이 같은 줄을
    /// 쓰므로 고른 것도 하나다(디자인 `:272-276` — 탭 줄은 도크에 하나뿐)
    pub site: Option<SiteId>,
    /// 큐 표의 열 폭 — **탭마다 한 벌씩** 든다 (FR-11 세션 저장 · FR-36).
    ///
    /// 도크는 앱에 하나뿐이라 여기가 제자리다. 탭별로 나누는 이유는 `QueueColumns` doc 참조
    pub columns: crate::ui::queue_panel::QueueColumns,
    /// 큐 표에서 지금 고른 전송들 (FR-36).
    ///
    /// **번호로 든다** — 목록은 프레임마다 다시 걸러지므로 자리 번호로 들면 거르개가 바뀔 때
    /// 엉뚱한 행을 가리킨다. `TransferId`에 `Ord`가 없어 집합은 `HashSet`이다.
    /// **세션에 담지 않는다** — `site`와 같은 이유다(`to_session` 참조)
    pub queue_selection: HashSet<TransferId>,
    /// `Shift` 범위 선택의 기준점 — 파일 목록의 `anchor`와 같은 규칙이다
    pub queue_anchor: Option<TransferId>,
}

/// 세션 파일에 적히는 키 — 열거형 이름이 바뀌어도 저장 형식은 그대로여야 한다
/// 필터 탭 묶음과 로그 탭 사이의 구분선이 차지하는 폭
const TAB_DIVIDER_GAP: f32 = 13.0;
/// 그 선이 스트립 위아래에서 물러서는 거리 — 끝까지 그으면 탭 경계처럼 보인다
const TAB_DIVIDER_INSET: f32 = 7.0;

const FILTER_ALL: &str = "all";
const FILTER_DONE: &str = "done";
const FILTER_ERROR: &str = "error";

impl Default for DockState {
    fn default() -> DockState {
        DockState {
            panel: None,
            filter: QueueFilter::All,
            site: None,
            columns: crate::ui::queue_panel::QueueColumns::default(),
            queue_selection: HashSet::new(),
            queue_anchor: None,
        }
    }
}

impl DockState {
    /// 세션에 담을 형태로 (FR-44) — **담지 않는 것이 둘이다**.
    ///
    /// **사이트 고르기**: 연결이 없는 채로 시작하므로 되살려도 가리킬 곳이 없다.
    ///
    /// **열려 있었는가**: 앱은 언제나 도크가 닫힌 채로 시작한다(2026-08-21 사용자 요청) —
    /// 전송이 도는 중이 아닌데 재시작마다 화면 아래가 268px 먹힌 채 뜨는 것을 막는다.
    /// 트레이로 숨겼다 되부르는 것은 같은 실행이라 보고 있던 그대로다(세션을 다시 읽지 않는다).
    /// 필터·열 폭은 종전대로 담는다 — 다시 열었을 때 보던 조건이 남아야 한다
    pub fn to_session(&self) -> crate::app::settings::DockSession {
        crate::app::settings::DockSession {
            filter: match self.filter {
                QueueFilter::All => FILTER_ALL,
                QueueFilter::Done => FILTER_DONE,
                QueueFilter::Error => FILTER_ERROR,
            }
            .to_owned(),
            columns: self.columns.to_saved(QueueFilter::All),
            columns_done: self.columns.to_saved(QueueFilter::Done),
            columns_error: self.columns.to_saved(QueueFilter::Error),
            column_order: self.columns.order_saved(QueueFilter::All),
            column_order_done: self.columns.order_saved(QueueFilter::Done),
            column_order_error: self.columns.order_saved(QueueFilter::Error),
            column_hidden: self.columns.hidden_saved(QueueFilter::All),
            column_hidden_done: self.columns.hidden_saved(QueueFilter::Done),
            column_hidden_error: self.columns.hidden_saved(QueueFilter::Error),
        }
    }

    /// 저장된 것에서 되살린다 — 모르는 키는 기본값(전체)이다.
    ///
    /// **`panel`은 언제나 `None`이다** — 열려 있었는지를 담지 않으므로 되살릴 것도 없다
    /// (위 `to_session` 참조). 옛 설정 파일에 남은 `"panel"` 키는 serde가 무시한다
    pub fn from_session(saved: &crate::app::settings::DockSession) -> DockState {
        DockState {
            panel: None,
            filter: match saved.filter.as_str() {
                FILTER_DONE => QueueFilter::Done,
                FILTER_ERROR => QueueFilter::Error,
                _ => QueueFilter::All,
            },
            site: None,
            columns: {
                // 폭을 먼저 세우고 차례·숨김을 얹는다 — 셋이 서로를 건드리지 않으므로
                // 순서 자체에 뜻은 없으나, 폭이 정본이라 그것을 먼저 둔다
                let mut columns = crate::ui::queue_panel::QueueColumns::from_saved(
                    &saved.columns,
                    &saved.columns_done,
                    &saved.columns_error,
                );
                for (filter, order, hidden) in [
                    (QueueFilter::All, &saved.column_order, &saved.column_hidden),
                    (
                        QueueFilter::Done,
                        &saved.column_order_done,
                        &saved.column_hidden_done,
                    ),
                    (
                        QueueFilter::Error,
                        &saved.column_order_error,
                        &saved.column_hidden_error,
                    ),
                ] {
                    columns.apply_saved_order(filter, order);
                    columns.apply_saved_hidden(filter, hidden);
                }
                columns
            },
            // 고른 행은 담지 않는다 — 큐 항목 자체가 완료·취소분을 버리고 되살아나므로
            // 번호를 되살려 봐야 가리킬 곳이 없다 (`site`와 같은 이유)
            queue_selection: HashSet::new(),
            queue_anchor: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.panel.is_some()
    }

    /// 상태 표시줄의 캐럿이 누르는 토글 — 같은 화면을 다시 누르면 접힌다 (README §8)
    pub fn toggle(&mut self, panel: DockPanel) {
        self.panel = if self.panel == Some(panel) {
            None
        } else {
            Some(panel)
        };
    }
}

/// 탭 스트립에서 사용자가 고른 것
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockAction {
    /// `⏸` — 전송을 멈추거나 다시 시작한다
    TogglePause,
    /// `✕` — 끝난 항목을 치운다
    ClearDone,
    /// `⧉` — 로그를 복사한다 (T20이 처리한다)
    CopyLog,
}

/// 도크가 실제로 차지한 높이 — 창이 낮으면 줄어든다 (plan Edge Case).
///
/// 패널 그리드에 최소 높이를 남기는 이유: 도크가 화면을 통째로 먹으면 파일을 고를 수 없어
/// 전송을 새로 걸 수도 없다
pub fn dock_height(available: f32) -> f32 {
    let room = available - GRID_MIN_HEIGHT;
    DOCK_HEIGHT.min(room.max(0.0))
}

/// 큐·로그가 함께 보는 값 — 탭 라벨의 건수와 연결별 탭이 여기서 나온다
pub struct DockView<'a> {
    pub queue: &'a TransferQueue,
    /// **연결이 실패한 상태로 남아 있는** 사이트들 — 연결별 탭의 점이 빨강이 되는 조건이다.
    ///
    /// "연결이 있는가"가 아니라 "실패했는가"를 받는 이유: 원본은 `phase === "error"`일 때만
    /// 빨강이고 그 밖(연결 중·연결됨·연결 없음)은 초록이다(`:728`). 연결 객체의 유무로
    /// 가르면 **실패한 사이트가 초록**으로, 정상 종료한 사이트가 빨강으로 뒤집힌다
    pub failed: &'a [SiteId],
    /// 지금 **연결이 열려 있는** 사이트들 — 큐가 비어 있어도 연결별 탭에 서야 한다.
    ///
    /// 원본은 큐에 든 항목에서 이름을 모으지만(`:722`), 그러면 **연결만 하고 아직 아무것도
    /// 옮기지 않은 서버가 탭에 없다** — 사용자가 서버를 고를 수 없다(2026-08-05 보고)
    pub connected: &'a [SiteId],
}

/// 탭 스트립을 그린다. 본문은 호출부가 남은 자리에 그린다.
///
/// 돌려주는 값은 **사용자가 누른 것**이며 상태 변경(탭 전환·접기)은 여기서 `state`에 바로 쓴다 —
/// 탭 전환까지 값으로 돌려주면 호출부가 그것을 다시 `state`에 옮겨 적어야 해 실수가 는다
pub fn show_strip(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    state: &mut DockState,
    view: &DockView<'_>,
) -> Option<DockAction> {
    ui.painter().rect_filled(rect, 0.0, theme::WINDOW_BG);
    let showing_log = state.panel == Some(DockPanel::Log);

    let counts = (
        view.queue.count(QueueFilter::All),
        view.queue.count(QueueFilter::Done),
        view.queue.count(QueueFilter::Error),
    );
    // 큐 탭 셋은 **같은 화면의 필터**이고 로그 탭만 다른 화면이다.
    // 원본은 로그를 필터 사이에 끼워 두었지만(`:1018-1022`), 성격이 다른 것이 가운데 서면
    // 넷이 같은 종류로 읽힌다 — 필터를 모으고 로그는 구분선 뒤로 보낸다 (2026-08-16 검토)
    let tabs = [
        (
            format!("{} ({})", crate::i18n::dock_queue(), counts.0),
            Some(QueueFilter::All),
        ),
        (
            format!("{} ({})", crate::i18n::dock_success(), counts.1),
            Some(QueueFilter::Done),
        ),
        (
            format!("{} ({})", crate::i18n::dock_failed(), counts.2),
            Some(QueueFilter::Error),
        ),
        (crate::i18n::dock_log().to_owned(), None),
    ];

    let mut left = rect.left();
    for (index, (label, filter)) in tabs.into_iter().enumerate() {
        // 필터 묶음과 로그 사이에 선을 하나 긋는다 — 종류가 다르다는 것을 자리로도 보인다
        if index > 0 && filter.is_none() {
            ui.painter().vline(
                left + TAB_DIVIDER_GAP / 2.0,
                (rect.top() + TAB_DIVIDER_INSET)..=(rect.bottom() - TAB_DIVIDER_INSET),
                egui::Stroke::new(1.0, theme::BORDER_CONTROL),
            );
            left += TAB_DIVIDER_GAP;
        }
        // 색을 여기서 굽지 않는다 — 갤리에 든 색은 아래 `galley`에 넘기는 색을 덮어써
        // 선택·hover 색이 화면에 나오지 않는다(`list_common`의 같은 함정 주석 참고).
        // 폭이 정해져야 탭 자리가 나오고 그래야 hover를 알 수 있어 색은 나중에 정해진다
        let text = ui.painter().layout_no_wrap(
            label,
            egui::FontId::proportional(TAB_FONT_PX),
            egui::Color32::PLACEHOLDER,
        );
        let width = text.size().x + TAB_PAD_X * 2.0;
        let tab = egui::Rect::from_min_size(
            egui::pos2(left, rect.top()),
            egui::vec2(width, STRIP_HEIGHT),
        );
        left += width;
        let active = match filter {
            Some(filter) => !showing_log && state.filter == filter,
            None => showing_log,
        };
        // id를 **라벨이 아니라 그 배후의 값**으로 잡는다 — 라벨에는 건수가 들어 있어
        // 누르는 사이에 전송이 끝나면 id가 바뀌어 클릭이 씹힌다 (T19 quality 리뷰 M2)
        let key = filter.map(|filter| filter as u8);
        let response = ui.interact(tab, ui.id().with(("dock_tab", key)), egui::Sense::click());
        if active {
            ui.painter().rect_filled(tab, 0.0, theme::SURFACE_BG);
        }
        // 선택된 탭만 흰색이다 — 성공·실패도 예외를 두지 않는다. 상태색(#7FD6A2·#FF8A8A)은
        // 선택 표시로 쓰기엔 옅어 어느 탭을 보고 있는지가 먼저 읽히지 않았다 (2026-08-18 보고)
        let color = if active {
            theme::TEXT_SELECTED
        } else if response.hovered() {
            theme::TEXT
        } else {
            theme::TEXT_MUTED
        };
        ui.painter().galley(
            egui::pos2(tab.left() + TAB_PAD_X, tab.center().y - text.size().y / 2.0),
            text,
            color,
        );
        if response.clicked() {
            match filter {
                Some(filter) => {
                    state.panel = Some(DockPanel::Queue);
                    // **바뀔 때만** 고른 것을 비운다 — 이 대입은 이미 활성인 탭을 다시 눌러도
                    // 실행되므로, 견주지 않으면 같은 탭 재클릭만으로 선택이 사라진다
                    if state.filter != filter {
                        state.filter = filter;
                        state.queue_selection.clear();
                        state.queue_anchor = None;
                    }
                }
                None => state.panel = Some(DockPanel::Log),
            }
        }
    }

    // 우측 아이콘 — 화면에 따라 구성이 다르다 (인벤토리 #33·#34).
    // 넷 다 아이콘뿐이라 **툴팁이 유일한 설명**이다
    let (pause_icon, pause_hint) = if view.queue.is_paused() {
        (ICON_PLAY, crate::i18n::dock_resume())
    } else {
        (ICON_PAUSE, crate::i18n::dock_pause())
    };
    let log_icons = [
        (
            ICON_COPY,
            ICON_FONT_PX,
            Some(DockAction::CopyLog),
            crate::i18n::dock_copy_log(),
        ),
        (
            ICON_COLLAPSE,
            CLOSE_FONT_PX,
            None,
            crate::i18n::dock_collapse(),
        ),
    ];
    let queue_icons = [
        (
            pause_icon,
            ICON_FONT_PX,
            Some(DockAction::TogglePause),
            pause_hint,
        ),
        (
            ICON_CLEAR,
            ICON_FONT_PX,
            Some(DockAction::ClearDone),
            crate::i18n::dock_clear_done(),
        ),
        (
            ICON_COLLAPSE,
            CLOSE_FONT_PX,
            None,
            crate::i18n::dock_collapse(),
        ),
    ];
    let icons: &[(&str, f32, Option<DockAction>, &str)] = if showing_log {
        &log_icons
    } else {
        &queue_icons
    };
    let mut action = None;
    let mut right = rect.right() - STRIP_PAD_RIGHT;
    // 오른쪽 끝부터 거꾸로 놓는다 — 원본의 순서(⏸ ✕ ▼)가 유지된다
    for (glyph, font_px, kind, hint) in icons.iter().rev() {
        let icon = egui::Rect::from_min_size(
            egui::pos2(right - ICON_SIZE, rect.center().y - ICON_SIZE / 2.0),
            egui::vec2(ICON_SIZE, ICON_SIZE),
        );
        right -= ICON_SIZE;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(icon));
        let clicked = widgets::icon_button_styled(
            &mut child,
            glyph,
            icon.size(),
            theme::MENU_HOT,
            theme::TEXT_MUTED,
            *font_px,
        )
        .on_hover_text(*hint)
        .clicked();
        if clicked {
            match kind {
                Some(kind) => action = Some(*kind),
                // `▼`는 접기다 — 값으로 돌려주지 않고 여기서 닫는다
                None => state.panel = None,
            }
        }
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 열_배치가_세션을_왕복한다() {
        // G3 — 탭마다 따로 왕복해야 한다. 한 벌만 담으면 탭을 옮길 때 배치가 흔들린다
        use crate::ui::queue_panel::QueueColumnKind;
        let mut state = DockState::default();
        state
            .columns
            .toggle(QueueFilter::All, QueueColumnKind::Server);
        state.columns.reorder(QueueFilter::All, 0, 2);
        state
            .columns
            .toggle(QueueFilter::Done, QueueColumnKind::Size);

        let back = DockState::from_session(&state.to_session());
        for filter in [QueueFilter::All, QueueFilter::Done, QueueFilter::Error] {
            assert_eq!(
                back.columns.visible(filter),
                state.columns.visible(filter),
                "{filter:?} 탭의 배치가 왕복하지 않는다"
            );
            assert_eq!(
                back.columns.to_saved(filter),
                state.columns.to_saved(filter),
                "{filter:?} 탭의 폭이 왕복하지 않는다"
            );
        }
    }

    #[test]
    fn 열_배치가_없는_옛_세션은_기본_배치로_선다() {
        // G4의 짝 — 키가 비어 있어도 그 탭의 기본 차례가 서고 숨긴 열은 없다
        let 기본 = DockState::default();
        let mut 옛것 = 기본.to_session();
        옛것.column_order = Vec::new();
        옛것.column_order_done = Vec::new();
        옛것.column_order_error = Vec::new();
        옛것.column_hidden = Vec::new();
        옛것.column_hidden_done = Vec::new();
        옛것.column_hidden_error = Vec::new();

        let back = DockState::from_session(&옛것);
        for filter in [QueueFilter::All, QueueFilter::Done, QueueFilter::Error] {
            assert_eq!(
                back.columns.visible(filter),
                기본.columns.visible(filter),
                "{filter:?} 탭이 기본 배치로 서지 않았다"
            );
        }
    }

    #[test]
    fn 도크_치수는_원본과_같다() {
        // Acceptance ① — 268px·탭 스트립 28px
        assert_eq!(DOCK_HEIGHT, 268.0);
        assert_eq!(STRIP_HEIGHT, 28.0);
        assert_eq!(ICON_SIZE, 26.0);
        assert_eq!(TAB_PAD_X, 14.0);
    }

    #[test]
    fn 탭_문구는_인벤토리_원문_그대로다() {
        // 인벤토리 #29~#32 — 건수는 호출부가 붙인다
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        assert_eq!(crate::i18n::dock_queue(), "전송 큐");
        assert_eq!(crate::i18n::dock_log(), "서버 로그");
        assert_eq!(crate::i18n::dock_success(), "성공");
        assert_eq!(crate::i18n::dock_failed(), "실패");
        // 아이콘은 **아이콘 글꼴의 것**이어야 한다 — 원본 글리프를 그대로 쓰면 두부가 된다
        for icon in [ICON_PAUSE, ICON_CLEAR, ICON_COLLAPSE, ICON_COPY] {
            let code = icon.chars().next().expect("한 글자") as u32;
            assert!(
                (0xE000..=0xF8FF).contains(&code),
                "아이콘 글꼴의 사용자 영역 밖이다: {icon:?} (U+{code:04X})"
            );
        }
    }

    #[test]
    fn 창이_낮으면_도크가_줄어든다() {
        // plan Edge Case — 도크가 화면을 통째로 먹으면 파일을 고를 수 없다
        assert_eq!(dock_height(1000.0), DOCK_HEIGHT);
        assert_eq!(dock_height(300.0), 180.0, "그리드 몫 120px를 남긴다");
        assert_eq!(
            dock_height(100.0),
            0.0,
            "남길 자리가 없으면 도크가 사라진다"
        );
    }

    #[test]
    fn 도크는_아래쪽_패널로_자리를_떼야_그리드가_남는다() {
        // T19 quality 리뷰 B1 — 사각형을 직접 잡아 `allocate_rect`로 떼면 위→아래 배치에서
        // 커서가 **화면 바닥 너머**로 밀려 뒤에 오는 그리드가 높이 0이 된다.
        // 앱이 기대는 것은 egui의 아래쪽 패널이 남은 자리를 줄여 준다는 성질이라, 그것을 고정한다
        let ctx = egui::Context::default();
        let mut before = 0.0;
        let mut after = 0.0;
        let height = 100.0;
        let _ = ctx.run_ui(Default::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                before = ui.available_rect_before_wrap().height();
                egui::Panel::bottom(egui::Id::new("도크 자리 시험"))
                    .resizable(false)
                    .default_size(height)
                    .size_range(egui::Rangef::new(height, height))
                    .frame(egui::Frame::NONE)
                    .show(ui, |ui| {
                        ui.label("도크");
                    });
                after = ui.available_rect_before_wrap().height();
            });
        });
        assert!(before > 0.0, "시험 자체가 성립하지 않았다");
        assert!(
            after > 0.0,
            "도크를 뗀 뒤 그리드 자리가 사라졌다 — 파일 목록이 통째로 보이지 않게 된다"
        );
        assert!(
            (before - after - height).abs() < 1.0,
            "뗀 높이가 도크 높이와 다르다: {before} → {after}"
        );
    }

    #[test]
    fn 같은_화면을_다시_누르면_접힌다() {
        // README §8 — 상태 표시줄 캐럿의 토글
        let mut state = DockState::default();
        assert!(!state.is_open());
        state.toggle(DockPanel::Queue);
        assert_eq!(state.panel, Some(DockPanel::Queue));
        state.toggle(DockPanel::Log);
        assert_eq!(state.panel, Some(DockPanel::Log), "다른 화면이면 갈아탄다");
        state.toggle(DockPanel::Log);
        assert!(!state.is_open());
    }

    #[test]
    fn 도크_스트립은_큐와_로그_탭을_보인다() {
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        // 연결별 탭은 **스트립이 아니라 그 아래 줄**에 선다 (디자인 `:272` — 도크에 줄은 하나다).
        // 여기서는 위 줄이 네 탭을 그대로 내는지만 본다
        let queue = TransferQueue::new();
        let view = DockView {
            queue: &queue,
            failed: &[],
            connected: &[SiteId(1)],
        };
        let ctx = egui::Context::default();
        let mut state = DockState {
            panel: Some(DockPanel::Log),
            ..DockState::default()
        };
        let 글자 = draw_strip(&ctx, &mut state, &view);
        for 탭 in [
            crate::i18n::dock_queue(),
            crate::i18n::dock_log(),
            crate::i18n::dock_success(),
            crate::i18n::dock_failed(),
        ] {
            assert!(
                글자.iter().any(|text| text.starts_with(탭)),
                "`{탭}` 탭이 없다: {글자:?}"
            );
        }
    }

    #[test]
    fn 필터_탭을_바꾸면_선택이_비고_같은_탭은_남는다() {
        // Acceptance ④(필터 탭) — 보이는 목록이 통째로 갈리므로 고른 것을 남기지 않는다.
        // **같은 탭 재클릭은 비우지 않는다** — 이 대입은 이미 활성인 탭에도 실행되므로
        // 견주지 않으면 다시 누르는 것만으로 선택이 사라진다
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        let queue = TransferQueue::new();
        let view = DockView {
            queue: &queue,
            failed: &[],
            connected: &[SiteId(1)],
        };
        let 고른것: HashSet<TransferId> = [TransferId(1), TransferId(2)].into_iter().collect();

        // `전체` 탭에서 `성공` 탭으로 — 비운다
        let mut state = DockState {
            panel: Some(DockPanel::Queue),
            filter: QueueFilter::All,
            queue_selection: 고른것.clone(),
            queue_anchor: Some(TransferId(1)),
            ..DockState::default()
        };
        let 성공_탭 = 탭_가운데(&state, &view, crate::i18n::dock_success());
        click_strip(&mut state, &view, 성공_탭);
        assert_eq!(
            state.filter,
            QueueFilter::Done,
            "필터 탭 클릭이 먹지 않았다"
        );
        assert!(
            state.queue_selection.is_empty(),
            "필터 탭을 바꿨는데 고른 것이 남았다"
        );
        assert_eq!(state.queue_anchor, None);

        // 이미 활성인 `성공` 탭을 다시 누른다 — 그대로 남는다
        let mut state = DockState {
            panel: Some(DockPanel::Queue),
            filter: QueueFilter::Done,
            queue_selection: 고른것.clone(),
            queue_anchor: Some(TransferId(1)),
            ..DockState::default()
        };
        click_strip(&mut state, &view, 성공_탭);
        assert_eq!(state.filter, QueueFilter::Done);
        assert_eq!(
            state.queue_selection, 고른것,
            "같은 탭을 다시 눌렀는데 고른 것이 사라졌다"
        );
        assert_eq!(state.queue_anchor, Some(TransferId(1)));
    }

    /// 그 문구로 시작하는 탭 글자의 한가운데 — 탭 폭이 건수·글꼴에 따라 달라 자리를 잰다.
    ///
    /// 상태를 고치지 않는다(사본에 그린다) — 자리를 재는 것뿐이라 클릭이 없다
    fn 탭_가운데(state: &DockState, view: &DockView<'_>, 문구: &str) -> egui::Pos2 {
        let state = &mut state.clone();
        let ctx = egui::Context::default();
        let output = ctx.run_ui(Default::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let rect =
                    egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(900.0, STRIP_HEIGHT));
                show_strip(ui, rect, state, view);
            });
        });
        for clipped in &output.shapes {
            if let egui::Shape::Text(text) = &clipped.shape
                && text.galley.text().starts_with(문구)
            {
                return egui::pos2(
                    text.pos.x + text.galley.size().x / 2.0,
                    text.pos.y + text.galley.size().y / 2.0,
                );
            }
        }
        panic!("`{문구}` 탭을 찾지 못했다");
    }

    /// 스트립의 그 자리를 누른다 — 자리 잡기·누름·뗌 세 프레임
    fn click_strip(state: &mut DockState, view: &DockView<'_>, at: egui::Pos2) {
        let ctx = egui::Context::default();
        let mut time = 0.0;
        let frames = [
            vec![egui::Event::PointerMoved(at)],
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        ];
        for events in [Vec::new()].into_iter().chain(frames) {
            time += 0.1;
            let input = egui::RawInput {
                time: Some(time),
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let rect = egui::Rect::from_min_size(
                        ui.max_rect().min,
                        egui::vec2(900.0, STRIP_HEIGHT),
                    );
                    show_strip(ui, rect, state, view);
                });
            });
        }
    }

    /// 스트립을 한 프레임 그리고 글자를 모은다
    fn draw_strip(ctx: &egui::Context, state: &mut DockState, view: &DockView<'_>) -> Vec<String> {
        let mut texts = Vec::new();
        let output = ctx.run_ui(Default::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let rect =
                    egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(900.0, STRIP_HEIGHT));
                show_strip(ui, rect, state, view);
            });
        });
        for clipped in &output.shapes {
            if let egui::Shape::Text(text) = &clipped.shape {
                texts.push(text.galley.text().to_owned());
            }
        }
        texts
    }
}
