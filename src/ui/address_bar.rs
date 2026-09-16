//! 주소 스트립 — [←][→][↑] 탐색 버튼 + 경로 입력 (FR-6).
//!
//! 입력 정규화는 `panel::address_bar`의 두 함수를 그대로 쓴다(따옴표·상대 경로 처리).
//! **로컬 탭과 원격 탭이 같은 부품을 쓴다**(FR-31) — 갈리는 것은 무엇을 보이고 무엇을
//! 기준으로 입력을 푸느냐뿐이며, 그 갈림은 `AddressTarget` 하나가 든다.
use crate::panel::address_bar::{normalize_input, normalize_remote_input};
use crate::remote::types::RemotePath;
use crate::remote::url::{RemoteUrl, parse_remote_url};
use crate::ui::theme;
use crate::ui::widgets;
use eframe::egui;
use std::borrow::Cow;
use std::path::{Path, PathBuf};

// ── 시각 토큰 (plan `## 시각 요소 분해` 1:1, 96DPI 기준 고정 px) ──
/// 탐색 버튼 한 변
const NAV_BUTTON_SIZE: f32 = 24.0;
/// 탐색 버튼 아이콘 글꼴 — 타이틀바 버튼과 같은 크기다.
/// 활성·비활성이 이 한 값을 함께 쓴다(상태에 따라 아이콘 크기가 변하면 안 된다)
const NAV_ICON_PX: f32 = 16.0;
/// 이름 필터 입력란의 폭 (FR-65·D8) — 줄이 하나 늘지 않게 주소창과 같은 줄에 둔다
const FILTER_WIDTH: f32 = 160.0;
/// 주소창이 지키는 최소 폭 — 패널이 좁아지면 **필터란이 먼저 줄어든다**.
/// 긴 경로가 이미 잘려 보이는 자리라 주소창까지 함께 줄이면 어느 폴더인지 읽을 수 없다
const ADDRESS_MIN_WIDTH: f32 = 120.0;

/// 주소 스트립이 지금 가리키는 곳 — 탭의 소스를 그리기에 필요한 만큼만 빌려 온다.
///
/// `TabSource`를 그대로 받지 않는 이유: 주소 스트립에 필요한 것은 경로 하나뿐인데
/// 그쪽은 연결·단계·사이트까지 들고 있어, 시험에서 그 전부를 세워야 한다
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AddressTarget<'a> {
    Local(&'a Path),
    Remote(&'a RemotePath),
}

/// 주소창이 상위(패널)에 돌려주는 탐색 요청
#[derive(Clone, PartialEq, Debug)]
pub enum NavAction {
    Back,
    Forward,
    Up,
    Goto(PathBuf),
    /// 원격 주소를 입력했다 (FR-34) — 새 원격 탭으로 연다.
    ///
    /// 로컬 경로와 **같은 입력칸**에서 갈린다: `://`가 있고 아는 스킴이면 여기로,
    /// 아니면 위 `Goto`로 간다(`C:\ftp` 같은 폴더 이름이 원격으로 오해받지 않게 한다)
    GotoRemote(RemoteUrl),
    /// **보고 있는 그 서버 안에서** 다른 경로로 옮긴다 — 원격 탭에서만 나온다.
    ///
    /// 위 `GotoRemote`와 갈리는 지점: 그쪽은 주소에 호스트가 있어 **새 탭**을 열고,
    /// 이쪽은 호스트가 없어 지금 연결을 그대로 쓴다(`/var/www`·`sub/dir`)
    GotoRemotePath(RemotePath),
}

impl<'a> AddressTarget<'a> {
    /// 주소칸에 보일 글자 — 원격은 **서버 안 경로만** 싣고 호스트·프로토콜은 싣지 않는다.
    ///
    /// 어느 서버인지는 탭 제목·배지가 이미 말하고, 여기에 호스트를 실으면 내부 서버 주소가
    /// 화면에 상시로 뜬다(FR-39의 `경로로 복사`가 호스트를 빼는 것과 같은 이유다).
    ///
    /// `Cow`인 이유는 로컬 쪽이다 — 비UTF-8 경로는 빌려 줄 `&str`이 없어 손실 변환한 사본을
    /// 내야 한다. `&str`로 좁히면 그런 경로에서 주소칸이 빈 채로 남는다
    pub fn display_text(self) -> Cow<'a, str> {
        match self {
            AddressTarget::Local(path) => path.to_string_lossy(),
            AddressTarget::Remote(path) => Cow::Borrowed(path.as_str()),
        }
    }

    /// 위로 갈 자리가 있는가 — `↑` 버튼의 활성 판정이다.
    ///
    /// 원격은 루트(`/`)에서만 죽는다. 로컬의 빈 경로도 `parent()`가 `None`이라 함께 죽는데,
    /// **원격 탭이 그 빈 경로를 받던 것이 이 버튼이 눌리지 않던 원인이었다**
    pub fn can_go_up(self) -> bool {
        match self {
            AddressTarget::Local(path) => path.parent().is_some(),
            AddressTarget::Remote(path) => path.parent().is_some(),
        }
    }

    /// 주소칸에 친 글자를 탐색 요청으로 푼다 — 확정할 것이 없으면 `None`.
    ///
    /// **원격 주소를 먼저 본다**: 로컬 정규화는 `sftp://`를 상대 경로로 오해하고,
    /// 원격 정규화는 그것을 서버 안 폴더 이름으로 오해한다. 그래서 `://`가 붙은 것은
    /// 어느 탭에서 쳤든 **새 원격 탭**으로 가고(FR-34), 그 밖은 지금 탭의 종류로 갈린다
    pub fn resolve_input(self, input: &str) -> Option<NavAction> {
        if let Some(url) = parse_remote_url(input) {
            return Some(NavAction::GotoRemote(url));
        }
        match self {
            AddressTarget::Local(current) => normalize_input(current, input).map(NavAction::Goto),
            AddressTarget::Remote(current) => {
                normalize_remote_input(current, input).map(NavAction::GotoRemotePath)
            }
        }
    }
}

/// 주소 스트립이 한 프레임에 돌려주는 것 — 탐색 요청과 바뀐 이름 필터.
///
/// 둘을 한 벌로 돌려주는 이유: 같은 줄에서 함께 그려지므로 호출부가 한 번만 받아 처리한다
pub struct AddressBarOutcome {
    pub nav: Option<NavAction>,
    /// 사용자가 필터란을 고쳤으면 그 새 값 — 고치지 않았으면 `None`이다 (FR-65)
    pub filter: Option<String>,
}

/// 주소 스트립 상태 — 편집 중인 문자열만 보유한다(경로 정본은 패널이 갖는다)
pub struct AddressBar {
    /// 입력 버퍼. 편집 중이 아니면 현재 경로로 계속 덮어쓴다
    buffer: String,
    /// 사용자가 입력란을 건드린 뒤인가 — 편집 중에는 폴더가 바뀌어도 버퍼를 덮지 않는다
    editing: bool,
}

impl Default for AddressBar {
    fn default() -> AddressBar {
        AddressBar::new()
    }
}

impl AddressBar {
    pub fn new() -> AddressBar {
        AddressBar {
            buffer: String::new(),
            editing: false,
        }
    }

    /// 스트립을 그리고 이번 프레임의 탐색 요청·필터 변경을 반환한다.
    ///
    /// `filter`는 지금 걸려 있는 이름 필터다 — **정본은 목록이 갖고** 여기서는 그것을 그대로
    /// 보이기만 한다. 자체 버퍼를 두지 않는 이유: 폴더를 옮겨 목록이 필터를 비웠는데(D2)
    /// 입력란만 옛 글자를 들고 있으면 화면과 목록이 어긋난다
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        target: AddressTarget<'_>,
        can_back: bool,
        can_forward: bool,
        filter: &str,
    ) -> AddressBarOutcome {
        let mut action = None;
        let mut filter_changed = None;
        if !self.editing {
            let shown = target.display_text();
            if self.buffer != shown {
                self.buffer = shown.into_owned();
            }
        }
        // 이 줄의 바탕은 **활성 탭과 같은 색**이다 — 고른 탭이 아래 내용과 이어져 보여야 한다
        // (Windows 11 탐색기). 자리를 먼저 잡아 두고 줄 높이가 정해진 뒤에 채운다:
        // 내용보다 나중에 그리면 버튼·입력칸을 덮고, 먼저 그리려면 높이를 알 수 없다
        let background = ui.painter().add(egui::Shape::Noop);
        let row = ui
            .horizontal(|ui| {
                if nav_button(
                    ui,
                    egui_phosphor::regular::ARROW_LEFT,
                    can_back,
                    crate::i18n::address_back(),
                ) {
                    action = Some(NavAction::Back);
                }
                if nav_button(
                    ui,
                    egui_phosphor::regular::ARROW_RIGHT,
                    can_forward,
                    crate::i18n::address_forward(),
                ) {
                    action = Some(NavAction::Forward);
                }
                if nav_button(
                    ui,
                    egui_phosphor::regular::ARROW_UP,
                    target.can_go_up(),
                    crate::i18n::address_up(),
                ) {
                    action = Some(NavAction::Up);
                }

                // 남은 폭을 주소창과 필터란이 나눠 갖는다 — 주소창이 최소 폭을 먼저 챙기므로
                // 패널이 좁아지면 필터란이 먼저 줄어든다 (D8)
                let gap = ui.spacing().item_spacing.x;
                let (address_width, filter_width) = split_width(ui.available_width(), gap);

                let edit = egui::TextEdit::singleline(&mut self.buffer)
                    .desired_width(address_width)
                    .text_color(theme::TEXT);
                let resp = ui.add(edit);
                if resp.changed() {
                    self.editing = true;
                }
                if resp.lost_focus() {
                    // 포커스를 잃으면 편집을 접고 현재 경로로 되돌린다(엔터로 확정하지 않은 입력은 버린다)
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        action = target.resolve_input(&self.buffer);
                    }
                    self.editing = false;
                }
                if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.editing = false;
                    resp.surrender_focus();
                }

                // 이름 필터 (FR-65) — 목록이 준 값을 그대로 싣고, 고쳐지면 새 값을 돌려준다.
                // `widgets::text_field`가 아니라 인라인 `TextEdit`을 쓰는 이유: 그쪽은 폼 행
                // 배치(라벨 96px + 우물 배경)를 전제해 이 줄의 버튼·주소창과 벌이 맞지 않는다
                let mut text = filter.to_owned();
                let filter_edit = egui::TextEdit::singleline(&mut text)
                    .desired_width(filter_width)
                    .hint_text(crate::i18n::address_filter_hint())
                    .text_color(theme::TEXT);
                if ui.add(filter_edit).changed() {
                    filter_changed = Some(text);
                }
            })
            .response
            .rect;
        // 바탕은 **패널 폭 전체**를 채운다 — 내용 폭에만 칠하면 오른쪽 끝이 잘려 탭과 이어지지 않는다
        let strip = egui::Rect::from_min_max(
            egui::pos2(ui.max_rect().left(), row.top()),
            egui::pos2(ui.max_rect().right(), row.bottom()),
        );
        ui.painter().set(
            background,
            egui::Shape::rect_filled(strip, 0.0, theme::CONTROL_BG),
        );
        AddressBarOutcome {
            nav: action,
            filter: filter_changed,
        }
    }
}

/// 주소창과 필터란이 남은 폭 `rest`를 나눠 갖는 규칙 — `(주소창, 필터란)` (D8).
///
/// 주소창이 최소 폭을 **먼저** 챙기므로 패널이 좁아지면 필터란이 먼저 줄어든다.
/// 다만 **그 최소 폭도 남은 폭을 넘지 못한다** — `ui.horizontal()`은 줄을 바꾸지 않아,
/// 넘겨 주면 그만큼 패널 오른쪽으로 삐져나간다. 스플리터가 허용하는 최소 패널 폭
/// (`app::layout::MIN_PANE_SIZE`, 120px)에서는 탐색 버튼 셋만으로 이미 그 지점에 닿는다.
///
/// `ui` 없이 셀 수 있게 떼어 둔다 — 경계값을 시험으로 못박기 위해서다
fn split_width(rest: f32, gap: f32) -> (f32, f32) {
    let filter = (rest - ADDRESS_MIN_WIDTH - gap).clamp(0.0, FILTER_WIDTH);
    let address = (rest - filter - gap)
        .max(ADDRESS_MIN_WIDTH)
        .min((rest - gap).max(0.0));
    (address, filter)
}

/// 탐색 버튼 하나 — 프레임 없이 아이콘만 그리고, 눌렸으면 참을 돌려준다.
///
/// 갈 수 없는 방향은 흐린 글자색으로 그리고 hover 배경·툴팁·클릭을 모두 막는다.
/// `add_enabled`를 쓰지 않는 이유: 그것은 버튼 프레임까지 함께 그려 사각 배경이 남는다.
/// 대신 그 함수가 주던 비활성 표현(흐린 색·클릭 차단·툴팁 억제)을 여기서 직접 재현한다
fn nav_button(ui: &mut egui::Ui, icon: &str, enabled: bool, hint: &str) -> bool {
    let size = egui::vec2(NAV_BUTTON_SIZE, NAV_BUTTON_SIZE);
    if !enabled {
        widgets::icon_button_styled(
            ui,
            icon,
            size,
            egui::Color32::TRANSPARENT,
            theme::TEXT_DIM,
            NAV_ICON_PX,
        );
        return false;
    }
    widgets::icon_button_styled(ui, icon, size, theme::CONTROL_HOT, theme::TEXT, NAV_ICON_PX)
        .on_hover_text(hint)
        .clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// egui 기본 가로 간격 — 시험이 그 값에 매이지 않게 한 곳에 적는다
    const GAP: f32 = 8.0;

    #[test]
    fn 넓으면_필터란이_고정_폭을_갖는다() {
        // Acceptance ⓑ — 필터란 160px 고정, 주소창이 남은 폭 전부
        let (address, filter) = split_width(800.0, GAP);
        assert_eq!(filter, FILTER_WIDTH);
        assert_eq!(address, 800.0 - FILTER_WIDTH - GAP);
    }

    #[test]
    fn 좁아지면_필터란이_먼저_줄어든다() {
        // Acceptance ⓒ — 주소창은 최소 폭을 지키고 필터란이 그 몫을 내놓는다
        let (address, filter) = split_width(200.0, GAP);
        assert_eq!(address, ADDRESS_MIN_WIDTH);
        assert_eq!(filter, 200.0 - ADDRESS_MIN_WIDTH - GAP);
    }

    #[test]
    fn 두_칸의_합은_어떤_폭에서도_남은_폭을_넘지_않는다() {
        // `ui.horizontal()`은 줄을 바꾸지 않는다 — 넘기면 패널 밖으로 삐져나간다.
        // 스플리터 최소 패널 폭(120px)에서 탐색 버튼 셋을 뺀 자리가 이 구간에 든다
        for rest in [0.0, 8.0, 24.0, 120.0, 128.0, 129.0, 288.0, 800.0] {
            let (address, filter) = split_width(rest, GAP);
            assert!(
                address >= 0.0 && filter >= 0.0,
                "음수 폭이 나왔다 (rest {rest})"
            );
            // 두 칸 사이의 간격은 egui가 언제나 끼우므로 그 몫을 빼고 남는 것이 상한이다
            assert!(
                address + filter <= (rest - GAP).max(0.0) + f32::EPSILON,
                "두 칸의 합이 남은 폭을 넘었다 (rest {rest}, 주소창 {address}, 필터 {filter})"
            );
        }
    }

    fn r(s: &str) -> RemotePath {
        RemotePath::new(s)
    }

    #[test]
    fn 원격_탭의_주소칸은_서버_안_경로만_보인다() {
        // 사용자 보고의 증상 — 원격 탭에서 이 칸이 빈 채로 있었다.
        // 종전에는 `TabState::committed()`가 준 빈 경로를 그렸다
        let path = r("/backup/2026-09-16");
        let 보이는_것 = AddressTarget::Remote(&path).display_text();
        assert_eq!(보이는_것, "/backup/2026-09-16");
        // 호스트·프로토콜은 싣지 않는다 — 내부 서버 주소가 화면에 상시로 뜨지 않게 한다
        assert!(!보이는_것.contains("://"));

        // 로컬은 종전 그대로다
        assert_eq!(
            AddressTarget::Local(Path::new(r"C:\Users\me")).display_text(),
            r"C:\Users\me"
        );
    }

    #[test]
    fn 상위_버튼은_원격_루트에서만_죽는다() {
        // `↑`가 눌리지 않던 원인 — 원격 탭이 빈 경로를 받아 `parent()`가 언제나 `None`이었다
        let 루트 = r("/");
        assert!(
            !AddressTarget::Remote(&루트).can_go_up(),
            "루트에서는 위가 없다"
        );

        for 아래 in ["/DB", "/var/www"] {
            let path = r(아래);
            assert!(
                AddressTarget::Remote(&path).can_go_up(),
                "{아래}에서 위로 갈 수 있어야 한다"
            );
        }

        // 로컬 판정은 그대로 — 드라이브 뿌리는 죽고 그 아래는 산다
        assert!(!AddressTarget::Local(Path::new(r"C:\")).can_go_up());
        assert!(AddressTarget::Local(Path::new(r"C:\Users")).can_go_up());
    }

    #[test]
    fn 주소칸_입력은_세_갈래로_갈린다() {
        let 원격_현재 = r("/var");
        let 원격 = AddressTarget::Remote(&원격_현재);
        let 로컬 = AddressTarget::Local(Path::new(r"C:\base"));

        // ⓐ 호스트가 붙은 주소는 어느 탭에서 쳤든 새 원격 탭이다 (FR-34)
        assert!(matches!(
            원격.resolve_input("ftp://example.test/pub"),
            Some(NavAction::GotoRemote(_))
        ));
        assert!(matches!(
            로컬.resolve_input("sftp://example.test/pub"),
            Some(NavAction::GotoRemote(_))
        ));

        // ⓑ 원격 탭의 경로 입력은 같은 서버 안 이동이다
        assert_eq!(
            원격.resolve_input("/etc"),
            Some(NavAction::GotoRemotePath(r("/etc")))
        );
        assert_eq!(
            원격.resolve_input("www"),
            Some(NavAction::GotoRemotePath(r("/var/www")))
        );

        // ⓒ 로컬 탭의 같은 입력은 로컬 이동이다 — `/etc`가 원격 경로로 새지 않는다
        assert!(matches!(
            로컬.resolve_input("D:\\data"),
            Some(NavAction::Goto(_))
        ));
        assert!(matches!(
            로컬.resolve_input("sub"),
            Some(NavAction::Goto(_))
        ));

        // 빈 입력은 양쪽 다 아무것도 확정하지 않는다
        assert_eq!(원격.resolve_input("   "), None);
        assert_eq!(로컬.resolve_input("   "), None);
    }
}
