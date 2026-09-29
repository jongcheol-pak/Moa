//! Windows 11 탐색기 모양의 컨텍스트 메뉴 (FR-8 개정) — **우리가 그린다**.
//!
//! Win11의 모던 메뉴는 탐색기 프로세스 안에서만 사는 표면이라 다른 앱이 띄울 길이 없다.
//! 그래서 **모양만 같게 그리고 항목은 셸에서 읽어 온다**(`fs::shell_menu`) — 그 PC에 깔린
//! 확장(압축·이름 바꾸기 도구 등)이 그대로 목록에 들어온다.
//!
//! 구성은 위에서부터 **아이콘 줄**(잘라내기·복사·이름 바꾸기·삭제) → 표준 항목 줄들 →
//! **`앱 확장`** → **`기본 메뉴`**다. 마지막 줄이 종전 Windows 표준 메뉴를 그대로 연다 —
//! 글자 없이 스스로 그리는 확장은 이쪽에서만 제대로 보이므로 그 길을 남겨 둔다.
//!
//! **실행하지 않는다** — 고른 것을 값으로 돌려주고 무엇을 할지는 `ui::app`이 정한다
//! (`ui::remote_menu`와 같은 규칙).
use eframe::egui;

use crate::fs::shell_menu::{ShellMenuItem, SubmenuHandle};
use crate::ui::icon_tex::bgra_to_color_image;
use crate::ui::theme;
use crate::ui::widgets;
use crate::ui::widgets::MenuRowIcon;

/// 메뉴 **최소** 폭 — 우측 단축키 열이 있어 원격 메뉴(180)보다 넓다.
///
/// 실제 폭은 `menu_width`가 정한다 — 아이콘 줄의 이름이 더 넓으면 그만큼 벌어진다
pub const MENU_MIN_WIDTH: f32 = 260.0;

/// 아이콘 줄의 높이 — 일반 줄보다 조금 높다(기준 이미지가 그렇다)
/// 아이콘 줄 라벨 글자 크기 — 항목 줄보다 작게 둔다(아이콘이 주인공이고 라벨은 이름표다)
const ACTION_LABEL_PX: f32 = 12.0;
/// 아이콘과 그 아래 라벨 사이
const ACTION_LABEL_GAP: f32 = 3.0;
/// 라벨이 칸 좌우 끝에 닿지 않게 두는 여백 — 옆 칸 이름과 붙어 보이지 않게 한다
const ACTION_LABEL_PAD_X: f32 = 3.0;
/// 아이콘 줄 높이 — **아이콘 아래에 이름이 붙는다**(2026-08-22 사용자 요청, 기준 이미지와 같다).
/// 종전에는 아이콘만 두고 이름을 툴팁으로만 보였는데, 무엇인지 알려면 하나씩 올려 봐야 했다
const ACTION_ROW_HEIGHT: f32 = ACTION_ICON_PX + ACTION_LABEL_GAP + ACTION_LABEL_PX + 12.0;

/// 아이콘 줄에서 아이콘 글자 크기
const ACTION_ICON_PX: f32 = 16.0;

/// **앱이 모은 하위 메뉴**의 종류 (FR-8).
///
/// 셸이 준 하위 메뉴(`SubmenuHandle`)와 다르다 — 그쪽은 실재 `HMENU`를 가리키고 이쪽은
/// 우리가 항목을 골라 담은 묶음이다. 그래서 둘을 한 값으로 합치지 않는다.
///
/// 셋이던 것을 하나로 모은 것은 **네 번째(`업로드`)가 생겼기 때문**이다 — 종류가 늘 때마다
/// `ShellMenuPick`·`ShellMenuRow`·`OpenSubmenu`·`ExpandTarget` 네 자리를 각각 고쳐야 했고,
/// 그 비용이 「어느 묶음이 열리는지 값으로 감추지 않는다」는 이득을 넘겼다(공통화 문턱 3회를
/// 이미 넘겼다 — 2026-08-26 Deferred 항목의 재검토 조건).
///
/// **라벨과 글리프는 여기 두고 자리(차례)는 두지 않는다** — 셋의 차례 처리가 균일하지 않다:
/// `Compress`·`Extract`는 `arrange`의 표준 벡터에 차례 6으로 들어가지만 `Extensions`는
/// 그 벡터 밖에서 구분선과 함께 맨 뒤에 붙는다. 자리는 지금처럼 `arrange`가 정한다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VirtualSubmenu {
    /// 설치된 셸 확장을 한 겹으로 묶은 것
    Extensions,
    /// `다음으로 압축` (2026-08-26)
    Compress,
    /// `압축 풀기` — `Compress`와 **같은 차례(6)를 번갈아 쓴다**(압축 파일을 골랐으면 이쪽) (2026-08-26)
    Extract,
    /// `업로드` — 같은 워크스페이스에서 **연결된 원격 탭**들을 모은 것 (2026-08-28).
    ///
    /// 앞의 셋과 달리 재료가 셸이 아니라 **앱의 탭 목록**에서 온다. 연결된 탭이 하나도
    /// 없으면 줄은 서되 흐리고 펼쳐지지 않는다(`ShellMenuRow::Virtual`의 `enabled`)
    Upload,
}

impl VirtualSubmenu {
    /// 그 줄에 적을 문구
    fn label(self) -> &'static str {
        match self {
            VirtualSubmenu::Extensions => crate::i18n::menu_app_extensions(),
            VirtualSubmenu::Compress => crate::i18n::menu_compress_to(),
            VirtualSubmenu::Extract => crate::i18n::menu_extract_to(),
            VirtualSubmenu::Upload => crate::i18n::menu_upload(),
        }
    }

    /// 그 줄에 그릴 아이콘 (phosphor — 프로젝트 아이콘 규약)
    fn glyph(self) -> &'static str {
        match self {
            VirtualSubmenu::Extensions => egui_phosphor::regular::PUZZLE_PIECE,
            VirtualSubmenu::Compress => egui_phosphor::regular::FILE_ZIP,
            VirtualSubmenu::Extract => egui_phosphor::regular::FILE_ARCHIVE,
            VirtualSubmenu::Upload => egui_phosphor::regular::UPLOAD_SIMPLE,
        }
    }
}

/// 사용자가 이 메뉴에서 고른 것 — 실행은 `ui::app`이 한다
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellMenuPick {
    /// 위쪽 아이콘 줄에서 골랐다
    Action(MenuAction),
    /// 셸이 준 항목을 골랐다 — 그 명령 번호를 `ShellMenu::invoke`에 넘긴다
    Command(u32),
    /// 셸이 준 하위 메뉴를 펼친다
    Expand(SubmenuHandle),
    /// **앱이 모은 하위 메뉴**를 펼친다 — 셸 손잡이(`Expand`)가 아니라 우리가 세운 묶음이다.
    ///
    /// 어느 묶음인지는 값이 그대로 들고 있어(`ExpandVirtual(Extensions)`) `match`가 여전히
    /// 종류별로 갈린다 — 감추는 것이 아니라 네 자리(`Pick`·`Row`·`OpenSubmenu`·`ExpandTarget`)에
    /// 흩어져 있던 같은 갈래를 하나로 모은 것이다
    ExpandVirtual(VirtualSubmenu),
    /// 업로드 하위 메뉴에서 **어느 원격 탭으로 보낼지** 골랐다 (2026-08-28).
    ///
    /// 값은 `ui::app`이 모은 목록에서의 자리 번호다 — 그 목록은 메뉴를 여는 순간 굳으므로
    /// 번호로 되짚어도 어긋나지 않는다(셸 명령 번호와 같은 성질이다)
    UploadTo(usize),
    /// **Windows 기본 압축**을 고랐다 (`fs::zip_shell`).
    ///
    /// 셸 명령 번호가 없다 — 그 항목은 메뉴로 오지 않아 `SendTo`에서 직접 얻는다(D13-3)
    CompressZip,
    /// 앱이 스스로 세운 줄을 골랐다
    App(AppMenuItem),
    /// **하위 메뉴가 없는 줄에 마우스가 얹혔다** — 펼쳐 둔 하위 메뉴를 접는다.
    ///
    /// 마우스를 올리면 펼치는 규칙(2026-08-26)의 짝이다. 접지 않으면 마우스가 다른 줄로
    /// 가도 하위 팝업이 남아 **어느 줄의 것인지 알 수 없게 된다**(탐색기·패널 메뉴 둘 다 접는다)
    CollapseSubmenu,
    /// 종전 Windows 표준 메뉴를 연다
    ShowMore,
}

/// 셸이 주지 않아 **앱이 스스로 세우는 줄** (FR-8 재개정).
///
/// 셸의 비슷한 항목을 쓰지 않는 이유가 **셋 다 다르다** — `즐겨찾기에 추가`는 셸 것이
/// **탐색기 홈**을 가리켜 이 앱의 즐겨찾기(FR-56)와 다른 곳이고, `새 탭에서 열기`는 셸이
/// 아예 주지 않으며, `붙여넣기`는 셸이 **클립보드가 비면 그 줄 자체를 주지 않아** 흐린
/// 상태를 만들 수 없다(2026-08-26 사용자 선택: 항상 보이고 비었으면 흐리게)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMenuItem {
    /// 이 폴더를 앱 즐겨찾기에 담는다 (FR-56)
    AddFavorite,
    /// 이 폴더를 앱의 새 탭에서 연다 (FR-3)
    OpenInNewTab,
    /// 클립보드에 담긴 것을 이 폴더에 붙여넣는다 (FR-12·FR-64 — 2026-08-26).
    ///
    /// **빈 곳 우클릭에서만 선다** — 탐색기도 파일을 골랐을 때는 이 줄을 보이지 않는다.
    /// 그 게이트는 [`arrange`](crate::ui::app)가 진다
    Paste,
}

impl AppMenuItem {
    /// 상위 목록에서의 차례 — 셸 표준 항목과 같은 축이다(plan 「기준 항목 차례」)
    pub fn order(self) -> u8 {
        match self {
            // **배경 메뉴에서 맨 위다** — 그 표의 차례는 `붙여넣기`(1)·`새로 만들기`(3)·
            // `속성`(4)이고 1이 비어 있었다
            AppMenuItem::Paste => 1,
            AppMenuItem::OpenInNewTab => 2,
            AppMenuItem::AddFavorite => 7,
        }
    }

    /// 오른쪽 끝에 적을 단축키 — 없으면 빈 글이다.
    ///
    /// 셸 줄은 자기 것을 들고 오지만(`ShellMenuItem::shortcut`) 앱 줄에는 그 필드가 없어
    /// 종전에는 그 칸이 언제나 비어 있었다 (2026-08-26)
    pub fn shortcut(self) -> &'static str {
        match self {
            AppMenuItem::Paste => "Ctrl+V",
            AppMenuItem::OpenInNewTab | AppMenuItem::AddFavorite => "",
        }
    }

    /// 화면에 보일 문구
    pub fn label(self) -> &'static str {
        match self {
            AppMenuItem::OpenInNewTab => crate::i18n::menu_open_new_tab(),
            AppMenuItem::AddFavorite => crate::i18n::menu_add_favorite(),
            AppMenuItem::Paste => crate::i18n::menu_paste(),
        }
    }

    /// 그 줄에 그릴 아이콘 (phosphor — 프로젝트 아이콘 규약)
    fn glyph(self) -> &'static str {
        match self {
            AppMenuItem::OpenInNewTab => egui_phosphor::regular::ARROW_SQUARE_OUT,
            AppMenuItem::AddFavorite => egui_phosphor::regular::STAR,
            AppMenuItem::Paste => egui_phosphor::regular::CLIPBOARD,
        }
    }
}

/// 메뉴에 그릴 줄 하나 (FR-8 재개정).
///
/// **평면 `ShellMenuItem` 목록을 대신한다** — 앱이 세우는 줄과 `앱 확장` 머리는 그 타입으로
/// 표현할 수 없다(`id: u32`가 셸 명령 번호 자리라 앱 줄에 줄 값이 없고, `SubmenuHandle`은
/// 실재 `HMENU`를 가리킨다).
///
/// 이름에 `Shell` 접두를 두는 것은 `ui::remote_menu::MenuRow`와 겹치지 않게 하려는 것이다
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellMenuRow {
    /// 셸이 준 줄 — `origin`은 **셸이 준 원래 목록에서의 자리**다.
    ///
    /// **이 값이 없으면 아이콘이 어긋난다**: `MenuIcons`는 그 원래 목록과 1:1로 정렬된
    /// 캐시라(`MenuIcons::row`), 표준 차례로 재정렬하고 앱 줄을 끼운 뒤의 자리로 찾으면
    /// 모든 줄에 엉뚱한 그림이 붙는다
    Shell {
        item: ShellMenuItem,
        origin: usize,
        /// 셸이 비트맵을 주지 않았을 때 그 자리에 그릴 아이콘 — 표준 자리 줄에만 있다.
        ///
        /// **`앱 확장`으로 간 줄은 `None`이다**(2026-08-26 D2) — 서로 다른 확장에 같은
        /// 글리프를 붙이면 아이콘이 구분에 쓸모없어진다
        glyph: Option<&'static str>,
    },
    /// 앱이 스스로 세운 줄 — 눌리는가를 함께 든다(대응 기능이 없으면 비활성)
    App { item: AppMenuItem, enabled: bool },
    /// 구분선
    Separator,
    /// **앱이 모은 하위 메뉴**의 머리 — 펼치면 그 묶음이 오른쪽에 뜬다.
    ///
    /// `enabled`를 드는 것은 **눌러도 펼칠 것이 없는 묶음**이 있기 때문이다(연결된 원격 탭이
    /// 하나도 없을 때의 `업로드` — 흐린 채로 서고 펼쳐지지 않는다). 지금 셋은 언제나 참이다
    Virtual { kind: VirtualSubmenu, enabled: bool },
    /// **셸 항목이 오기 전의 자리 표시** — 흐린 한 줄이며 아무것도 고르지 않는다 (2026-09-29).
    ///
    /// 셸 메뉴는 워커가 읽어 오므로(`fs::shell_menu_worker`) 메뉴는 이 줄을 단 뼈대로 먼저 뜨고,
    /// 셸 항목이 도착하면 이 줄이 그것들로 바뀐다
    Loading,
}

/// 셸 하위 메뉴를 채우는 동안 그 팝업에 세우는 한 줄 — 흐리고 아무것도 실행하지 않는다.
///
/// 하위 팝업은 `ShellMenuItem` 목록을 그리므로(`show_submenu`) 부모 목록의
/// [`ShellMenuRow::Loading`]을 쓸 수 없다. **비활성이라 누름이 걸러지고**(`MenuRowHit`),
/// `id`는 0이라 어느 셸 명령도 가리키지 않는다
pub fn loading_submenu_item() -> ShellMenuItem {
    ShellMenuItem {
        id: 0,
        label: crate::i18n::shell_menu_loading().to_owned(),
        shortcut: String::new(),
        icon: None,
        enabled: false,
        checked: false,
        separator: false,
        submenu: None,
    }
}

/// 아이콘 줄의 네 가지 (FR-8·FR-64).
///
/// **넷 다 앱이 자체 기능으로 수행한다** — 셸 verb로 넘기는 칸이 없다. 셸의 `rename` verb는
/// 탐색기 자신의 목록 뷰가 처리하는 것이라 다른 호스트에서는 동작하지 않고, 잘라내기·복사도
/// verb로 부르면 셸이 자기 클립보드 상태를 쥐어 우리 화면의 잘라내기 표시와 어긋난다.
///
/// **다섯째 칸은 두지 않는다** — 종전에 `Windows.Share` verb로 가던 칸이며 사용자 요청으로
/// 뺐다(2026-08-22). 셸이 주는 그 항목 둘(`Windows.Share`·`Windows.ModernShare`)도
/// `ui::app::shell_menu`의 숨김 목록에서 함께 걷어낸다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Cut,
    Copy,
    Rename,
    Delete,
}

impl MenuAction {
    /// 왼쪽부터의 차례 — 기준 이미지의 순서다
    pub const ALL: [MenuAction; 4] = [
        MenuAction::Cut,
        MenuAction::Copy,
        MenuAction::Rename,
        MenuAction::Delete,
    ];

    /// 그 자리에 그릴 아이콘 (phosphor — 프로젝트 아이콘 규약)
    fn glyph(self) -> &'static str {
        match self {
            MenuAction::Cut => egui_phosphor::regular::SCISSORS,
            MenuAction::Copy => egui_phosphor::regular::COPY,
            MenuAction::Rename => egui_phosphor::regular::PENCIL,
            MenuAction::Delete => egui_phosphor::regular::TRASH,
        }
    }

    /// 마우스를 얹으면 뜨는 이름 — 라벨이 없는 줄이라 이것이 유일한 설명이다
    fn tooltip(self) -> &'static str {
        match self {
            MenuAction::Cut => crate::i18n::menu_cut(),
            MenuAction::Copy => crate::i18n::menu_copy(),
            MenuAction::Rename => crate::i18n::rename(),
            MenuAction::Delete => crate::i18n::delete(),
        }
    }
}

/// 이번에 그릴 메뉴의 상태 — 그리기 전에 정해지는 것들
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuState {
    /// 고른 항목 수 — 0이면 폴더 배경 메뉴라 아이콘 줄이 통째로 비활성이다
    pub selected: usize,
    /// 목록이 이름 편집을 받을 수 있는가 — **받을 곳이 없으면 그 칸이 비활성이다**.
    ///
    /// 이 값을 두는 이유는 「대응 기능이 없는 칸은 비활성」이라는 이 메뉴의 규칙을 그 칸에도
    /// 그대로 적용하기 위해서다 — 눌러도 아무 일이 없는 것은 비활성보다 나쁘다
    pub can_rename: bool,
}

impl MenuState {
    /// 그 자리가 눌릴 수 있는가.
    ///
    /// 고른 것이 없으면 넷 다 뜻이 없고, `이름 바꾸기`는 **하나일 때만** 열린다
    /// (새 이름은 하나뿐이라 여럿에 줄 수 없다 — 원격 메뉴의 같은 규칙)
    pub fn enabled(&self, action: MenuAction) -> bool {
        if self.selected == 0 {
            return false;
        }
        match action {
            MenuAction::Rename => self.selected == 1 && self.can_rename,
            _ => true,
        }
    }
}

/// 메뉴 한 판을 그린다 — 고른 것을 값으로 돌려준다 (FR-8).
///
/// `rows`는 셸 줄·앱 줄·구분선·`앱 확장` 머리가 섞인 [`ShellMenuRow`] 목록이고, `icons`는
/// **셸이 준 원래 목록**과 1:1로 정렬된 아이콘 캐시다.
///
/// **둘의 차례는 같지 않다** — `ui::app::shell_menu::arrange`가 표준 차례로 재정렬하고 앱
/// 줄을 끼우기 때문이다. 아이콘은 반드시 각 [`ShellMenuRow::Shell`]의 `origin`으로 찾는다
/// (`icons.row(줄 번호)`로 찾으면 모든 줄에 엉뚱한 그림이 붙는다)
pub fn show(
    ui: &mut egui::Ui,
    state: MenuState,
    rows: &[ShellMenuRow],
    icons: &MenuIcons,
    max_height: f32,
) -> Option<(ShellMenuPick, f32)> {
    // 팝업을 여는 자리에서 공통 항목 스타일을 세운다 (AGENTS 「팝업 메뉴 한 줄」).
    // 하위 메뉴는 부모 스타일을 잇지 않는 별도 `Area`라 거기서도 따로 부른다
    theme::menu_style(ui);
    ui.set_width(menu_width(ui.ctx()));

    // 고른 값과 **그 줄의 위쪽 y** — 하위 팝업을 그 높이에 띄우는 데 쓴다 (2026-08-26).
    // 아이콘 줄과 `기본 메뉴` 줄에는 하위 메뉴가 없어 그 값이 쓰이지 않지만, 반환 모양을
    // 하나로 두는 편이 호출부에서 갈래를 나누지 않아 짧다
    let mut picked: Option<(ShellMenuPick, f32)> = None;
    if let Some(action) = action_row(ui, state) {
        picked = Some((ShellMenuPick::Action(action), ui.min_rect().top()));
    }
    ui.separator();

    // **항목이 화면 높이를 넘으면 그 부분만 세로로 굴린다** — 아이콘 줄과 마지막 줄은
    // 제자리에 남는다. 확장이 많이 깔린 PC에서는 목록이 화면보다 길어진다
    // 아이콘 줄·구분선 둘·마지막 줄이 쓰는 만큼을 빼고 남는 자리가 목록 몫이다.
    // 아무리 좁아도 한 줄은 보인다 — 0이 되면 무엇이 있는지조차 알 수 없다
    let 고정_높이 = ACTION_ROW_HEIGHT + SEPARATOR_HEIGHT * 2.0 + theme::MENU_ITEM_HEIGHT;
    let 목록_높이 = (max_height - 고정_높이).max(theme::MENU_ITEM_HEIGHT);
    egui::ScrollArea::vertical()
        .max_height(목록_높이)
        .show(ui, |ui| {
            // 스크롤 영역은 자식 `Ui`라 부모 스타일을 잇지만, **이어받는지에 기대지 않고**
            // 다시 세운다 — 값은 같고 부작용이 없어 비용이 없다(AGENTS가 재호출을 못 박는
            // 것은 별도 `Area`인 하위 메뉴이며, 여기는 그보다 넓게 잡은 것이다)
            theme::menu_style(ui);
            ui.set_width(menu_width(ui.ctx()));
            for row in rows {
                if let Some(pick) = draw_row(ui, row, icons) {
                    picked = Some(pick);
                }
            }
        });

    ui.separator();
    // 이 줄에도 하위 메뉴가 없으므로 마우스가 얹히면 펼쳐 둔 것을 접는다 — 목록의 다른
    // 줄로 간 것과 같다. **판정은 `draw_row`와 같은 `pick_for`를 쓴다** — 여기만 따로
    // 쓰면 누름·얹힘 순서가 두 곳에서 갈릴 수 있고 그 갈림을 잡을 시험이 없다.
    //
    // 이 줄과 목록 줄에 마우스가 동시에 있을 수 없어 순서가 결과를 바꾸지 않지만,
    // 먼저 그려진 목록 쪽을 우선해 둔다
    let hit = widgets::menu_row_rich(
        ui,
        MenuRowIcon::Glyph(egui_phosphor::regular::ARROW_SQUARE_OUT),
        crate::i18n::menu_default(),
        "",
        false,
        true,
    );
    picked = picked.or(pick_for(hit, None, ShellMenuPick::ShowMore).map(|pick| (pick, hit.top)));
    picked
}

/// 줄 하나를 그린다 — 종류마다 아이콘·화살표·고른 값이 다르다 (FR-8 재개정).
///
/// **하위 메뉴가 있는 줄은 마우스가 얹히기만 해도 펼친다**(2026-08-26 사용자 요청) — 판정은
/// [`pick_for`]가 한다.
///
/// **고른 값과 함께 그 줄의 위쪽 y를 돌려준다** — 하위 팝업을 그 높이에 띄우기 위해서다
/// (2026-08-26). 그리는 자리만이 그 값을 알고, `Response.rect`가 이미 갖고 있다
fn draw_row(
    ui: &mut egui::Ui,
    row: &ShellMenuRow,
    icons: &MenuIcons,
) -> Option<(ShellMenuPick, f32)> {
    match row {
        ShellMenuRow::Separator => {
            ui.separator();
            None
        }
        // 아이콘은 **셸이 준 원래 자리**로 찾는다 — 재정렬 뒤의 자리로 찾으면 어긋난다
        ShellMenuRow::Shell {
            item,
            origin,
            glyph,
        } => {
            let hit = widgets::menu_row_rich(
                ui,
                icons.row(*origin, *glyph),
                &item.label,
                &item.shortcut,
                item.submenu.is_some(),
                item.enabled,
            );
            // **흐린 줄은 얹혀도 펼치지 않는다** — 누를 수 없는 것이 마우스만 스쳐도 열리면
            // 앞뒤가 맞지 않고, 그 하위 팝업의 줄들은 각자 활성이라 실행까지 이어진다.
            // 누름은 `MenuRowHit`이 이미 걸러 두지만 얹힘은 비활성 줄에서도 참이다
            let expand = if item.enabled {
                item.submenu.map(ShellMenuPick::Expand)
            } else {
                None
            };
            pick_for(hit, expand, ShellMenuPick::Command(item.id)).map(|pick| (pick, hit.top))
        }
        ShellMenuRow::App { item, enabled } => {
            let hit = widgets::menu_row_rich(
                ui,
                MenuRowIcon::Glyph(item.glyph()),
                item.label(),
                item.shortcut(),
                false,
                *enabled,
            );
            pick_for(hit, None, ShellMenuPick::App(*item)).map(|pick| (pick, hit.top))
        }
        ShellMenuRow::Virtual { kind, enabled } => {
            let hit = widgets::menu_row_rich(
                ui,
                MenuRowIcon::Glyph(kind.glyph()),
                kind.label(),
                "",
                true,
                *enabled,
            );
            // 비활성이면 펼치지 않는다 — 셸 줄이 이미 그렇게 한다(위 `Shell` 갈래)
            let expand = enabled.then_some(ShellMenuPick::ExpandVirtual(*kind));
            pick_for(
                hit,
                expand,
                // 하위 메뉴 머리는 눌러도 「펼치기」다 — 실행할 명령이 따로 없다
                ShellMenuPick::ExpandVirtual(*kind),
            )
            .map(|pick| (pick, hit.top))
        }
        ShellMenuRow::Loading => {
            // **얹혀도 아무것도 내지 않는다** — 누를 것도 펼칠 것도 없는 자리 표시다
            widgets::menu_row_rich(
                ui,
                MenuRowIcon::Blank,
                crate::i18n::shell_menu_loading(),
                "",
                false,
                false,
            );
            None
        }
    }
}

/// 줄 하나의 누름·얹힘을 고른 값으로 바꾼다 (FR-8 재개정).
///
/// - `expand`: 이 줄에 하위 메뉴가 있으면 그것을 펼치는 값. 없으면 `None`
/// - `activate`: 눌렀을 때 실행할 값
///
/// **누름이 얹힘보다 앞선다.** 누른 프레임에는 얹힘도 함께 참이라(같은 `Response`),
/// 얹힘을 먼저 보면 하위 메뉴 **없는** 줄을 눌렀을 때 실행 대신 접기가 나가 **메뉴의 모든
/// 실행이 죽는다** — 빌드는 되고 조용히 통과하는 종류다.
///
/// **하위 메뉴가 없는 줄에 마우스가 얹히면 접기를 낸다** — 펼침의 짝이며, 그러지 않으면
/// 마우스가 다른 줄로 가도 하위 팝업이 남는다
fn pick_for(
    hit: widgets::MenuRowHit,
    expand: Option<ShellMenuPick>,
    activate: ShellMenuPick,
) -> Option<ShellMenuPick> {
    if hit.clicked {
        // 하위 메뉴가 있으면 눌러도 펼치는 것이 맞다 — 그 줄에는 실행할 명령이 없다
        return Some(expand.unwrap_or(activate));
    }
    if !hit.hovered {
        return None;
    }
    Some(expand.unwrap_or(ShellMenuPick::CollapseSubmenu))
}

/// 셸이 준 아이콘을 egui 텍스처로 올려 두는 자리 (FR-8).
///
/// **메뉴가 닫히면 함께 버린다** — 그 메뉴 한 판에서만 쓰는 그림이라 앱 수명 내내 들고 있을
/// 이유가 없다(목록 아이콘을 캐시하는 `ui::icon_tex`와 그 점이 다르다. 그쪽은 같은 확장자가
/// 수천 줄이라 캐시가 곧 성능이다).
///
/// 차례는 `items`와 같다 — 줄 번호로 찾는다
pub struct MenuIcons {
    textures: Vec<Option<egui::TextureHandle>>,
}

impl MenuIcons {
    /// 줄들의 아이콘을 텍스처로 올린다.
    ///
    /// 셸이 아이콘을 주지 않았거나 올리지 못한 자리는 `None`이고, 그 줄은 아이콘 없이
    /// 그려진다(열은 그대로 자리를 지킨다)
    pub fn build(ctx: &egui::Context, items: &[ShellMenuItem]) -> MenuIcons {
        let textures = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let (width, height, bgra) = item.icon.clone()?;
                if width <= 0 || height <= 0 {
                    return None;
                }
                let image = bgra_to_color_image(width as usize, height as usize, bgra);
                // 이름에 줄 번호를 넣어 같은 판의 아이콘끼리 덮어쓰지 않게 한다
                Some(ctx.load_texture(
                    format!("shell_menu_icon_{index}"),
                    image,
                    egui::TextureOptions::LINEAR,
                ))
            })
            .collect();
        MenuIcons { textures }
    }

    /// 그 줄에 그릴 아이콘 — **셸이 준 그림이 우선이고 `glyph`는 그것이 없을 때의 대체다**.
    ///
    /// 셸 비트맵을 앱 글리프로 덮지 않는 이유는 그쪽이 **그 항목의 진짜 아이콘**이기 때문이다
    /// (2026-08-26 D1). 판정을 그리는 자리가 아니라 여기 두는 것은 `draw_row`가 고른 값만
    /// 돌려주어 **어느 아이콘을 그렸는지 시험에서 관측할 수 없기** 때문이다
    fn row(&self, index: usize, glyph: Option<&'static str>) -> MenuRowIcon<'_> {
        match self.textures.get(index).and_then(|slot| slot.as_ref()) {
            Some(texture) => MenuRowIcon::Texture(texture),
            None => match glyph {
                Some(glyph) => MenuRowIcon::Glyph(glyph),
                // 아이콘이 없어도 **열은 남긴다** — 이 메뉴에는 아이콘 있는 줄이 섞여 있다
                None => MenuRowIcon::Blank,
            },
        }
    }

    /// 그림이 하나도 없는 빈 캐시 — 시험에서 하위 메뉴 상태를 세울 때 쓴다.
    ///
    /// **`build`는 `egui::Context`를 요구한다** — 아이콘과 무관한 판정(어느 하위 메뉴가
    /// 펼쳐졌는가)을 시험하는 데 그것까지 세우는 것은 과하다
    #[cfg(test)]
    pub fn for_test() -> MenuIcons {
        MenuIcons {
            textures: Vec::new(),
        }
    }

    /// 고른 자리들만 뽑아 새 캐시를 만든다 — `앱 확장` 하위 메뉴가 쓴다 (FR-8 재개정).
    ///
    /// 그 하위 메뉴의 줄들은 부모 목록에서 골라낸 것이라 **자리가 0부터 다시 매겨진다**.
    /// 부모 캐시를 그대로 넘기면 그 새 자리로 찾게 되어 아이콘이 어긋난다.
    ///
    /// 텍스처 핸들은 참조 계수라 복제가 그림을 다시 올리지 않는다
    pub fn subset(&self, origins: &[usize]) -> MenuIcons {
        MenuIcons {
            textures: origins
                .iter()
                .map(|&origin| self.textures.get(origin).cloned().flatten())
                .collect(),
        }
    }
}

/// 메뉴를 팝업으로 띄운다 — 고른 것과 그 팝업이 차지한 자리를 함께 돌려준다 (FR-8).
///
/// **프레임을 여는 것도 이 모듈이 한다** — 그래야 프레임 모양(모서리·채움·테두리)과 그 안의
/// 줄 모양이 한 곳에서 정해진다. 부르는 쪽(`ui::app`)은 자리와 상태만 준다
pub fn show_popup(
    ctx: &egui::Context,
    id: egui::Id,
    at: egui::Pos2,
    state: MenuState,
    rows: &[ShellMenuRow],
    icons: &MenuIcons,
    max_height: f32,
) -> (Option<(ShellMenuPick, f32)>, egui::Rect) {
    let mut picked = None;
    let response = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .show(ctx, |ui| {
            // 모서리는 적지 않는다 — `Frame::menu`가 테마의 공통 값을 읽는다
            // (`theme::MENU_CORNER_RADIUS`)
            egui::Frame::menu(ui.style())
                .fill(theme::SURFACE_BG)
                .stroke(egui::Stroke::new(
                    theme::MENU_FRAME_STROKE,
                    theme::PANE_BORDER,
                ))
                .show(ui, |ui| {
                    picked = show(ui, state, rows, icons, max_height);
                });
        })
        .response;
    (picked, response.rect)
}

/// 하위 메뉴를 팝업으로 띄운다 — 부모 오른쪽에 붙는다 (FR-8)
pub fn show_submenu_popup(
    ctx: &egui::Context,
    id: egui::Id,
    at: egui::Pos2,
    zip_row: bool,
    items: &[ShellMenuItem],
    icons: &MenuIcons,
) -> (Option<ShellMenuPick>, egui::Rect) {
    let mut picked = None;
    let response = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style())
                .fill(theme::SURFACE_BG)
                .stroke(egui::Stroke::new(
                    theme::MENU_FRAME_STROKE,
                    theme::PANE_BORDER,
                ))
                .show(ui, |ui| {
                    picked = show_submenu(ui, zip_row, items, icons);
                });
        })
        .response;
    (picked, response.rect)
}

/// 이 메뉴가 쓸 폭 — **아이콘 줄의 이름을 실제로 재서 정한다** (FR-8).
///
/// 칸이 균등하게 나뉘므로 한 칸이 `폭 / 칸 수`이고, 그 안에 가장 긴 이름이 줄지 않고
/// 들어가야 한다. **상수로 어림하지 않는 이유**: 이름은 언어에 따라 길이가 달라지고
/// (`이름 바꾸기` ↔ `Rename`) 글꼴·배율에 따라서도 달라진다. 어림값을 두 번 고쳤는데
/// 두 번 다 잘렸다(2026-08-22 사용자 보고 — 260px, 340px).
///
/// 최소 폭 아래로는 내려가지 않는다 — 이름이 짧은 언어에서 메뉴가 옹색해지지 않게 한다
pub fn menu_width(ctx: &egui::Context) -> f32 {
    let font = egui::FontId::proportional(ACTION_LABEL_PX);
    let 가장_긴 = MenuAction::ALL
        .iter()
        .map(|action| {
            ctx.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(action.tooltip().to_owned(), font.clone(), theme::TEXT)
                    .size()
                    .x
            })
        })
        .fold(0.0_f32, f32::max);
    width_for(가장_긴)
}

/// 가장 긴 이름의 폭에서 메뉴 폭을 낸다 — 재는 것과 셈하는 것을 갈라 시험할 수 있게 한다.
///
/// 글꼴에 기대지 않는 순수 셈이라 이 함수만으로 규칙을 고정할 수 있다 — `menu_width`는
/// 글꼴이 있어야 돌고, 시험 환경의 기본 글꼴에는 한글이 없어 실제 폭이 나오지 않는다
fn width_for(가장_긴_이름: f32) -> f32 {
    let 필요한_칸 = 가장_긴_이름 + ACTION_LABEL_PAD_X * 2.0;
    (필요한_칸 * MenuAction::ALL.len() as f32)
        .max(MENU_MIN_WIDTH)
        .ceil()
}

/// 위쪽 아이콘 줄 — 칸을 균등하게 나눈다
fn action_row(ui: &mut egui::Ui, state: MenuState) -> Option<MenuAction> {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ACTION_ROW_HEIGHT),
        egui::Sense::hover(),
    );
    let width = rect.width() / MenuAction::ALL.len() as f32;
    let mut picked = None;
    for (index, action) in MenuAction::ALL.iter().enumerate() {
        let 칸 = egui::Rect::from_min_size(
            egui::pos2(rect.left() + width * index as f32, rect.top()),
            egui::vec2(width, rect.height()),
        );
        let enabled = state.enabled(*action);
        let response = ui.interact(
            칸,
            // 위젯 열쇠는 **화면 언어를 따르지 않는다** — 언어를 바꾸면 상태가 끊긴다
            // (AGENTS 「화면 문구」의 그 예외)
            ui.id().with(("shell_menu_action", index)),
            if enabled {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            },
        );
        if enabled && response.hovered() {
            // 삭제만 파괴색으로 얹는다 — 되돌리기 어려운 것이 눈에 띄어야 한다
            let 채움 = if *action == MenuAction::Delete {
                theme::MENU_HOT_DANGER
            } else {
                theme::MENU_HOT
            };
            ui.painter()
                .rect_filled(칸.shrink(2.0), theme::MENU_ITEM_CORNER_RADIUS, 채움);
        }
        let 색 = if enabled {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        };
        // 아이콘은 위, 이름은 그 아래 — 둘을 한 덩어리로 칸 가운데에 세운다
        let 덩어리_높이 = ACTION_ICON_PX + ACTION_LABEL_GAP + ACTION_LABEL_PX;
        let 위 = 칸.center().y - 덩어리_높이 / 2.0;
        ui.painter().text(
            egui::pos2(칸.center().x, 위 + ACTION_ICON_PX / 2.0),
            egui::Align2::CENTER_CENTER,
            action.glyph(),
            egui::FontId::proportional(ACTION_ICON_PX),
            색,
        );
        // 칸보다 긴 이름은 끝을 줄인다 — 넘치면 옆 칸 이름과 붙어 읽을 수 없다.
        // `painter.layout`은 폭을 **줄바꿈 폭**으로 보므로 한 줄로 못 박고 자른다
        let mut job = egui::text::LayoutJob::simple(
            action.tooltip().to_owned(),
            egui::FontId::proportional(ACTION_LABEL_PX),
            색,
            칸.width() - ACTION_LABEL_PAD_X * 2.0,
        );
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.halign = egui::Align::Center;
        let galley = ui.painter().layout_job(job);
        ui.painter().galley(
            egui::pos2(칸.center().x, 위 + ACTION_ICON_PX + ACTION_LABEL_GAP),
            galley,
            색,
        );
        // 이름이 줄에 보이지만 툴팁도 남긴다 — 칸이 좁아 이름이 잘리는 언어가 있다
        let response = response.on_hover_text(action.tooltip());
        if enabled && response.clicked() {
            picked = Some(*action);
        }
    }
    picked
}

/// 이 메뉴가 차지할 크기 — 화면 밖으로 나가지 않게 미리 재는 데 쓴다.
///
/// **프레임 여백까지 더한 값이다** — 화면 밖 보정은 실제로 차지하는 자리를 알아야 한다.
/// 그 여백은 `ui::menu::menu_frame_pad`가 잰다(원격 메뉴·트리 메뉴와 같은 값).
///
/// **줄 사이 간격을 스타일에서 읽는 이유**: egui는 위젯을 하나 놓을 때마다 `item_spacing.y`를
/// 더하는데, 그 값을 상수로 적어 두면 스타일이 바뀔 때 조용히 어긋난다. 실제로 그것을 빼고
/// 세었더니 줄마다 3px씩 모자랐다.
///
/// `Ui`가 아닌 `Context`를 받는다 — 메뉴를 **열기 전에** 자리를 정해야 하는 `ui::app`이
/// 유일한 호출처이고, 그 자리엔 아직 `Ui`가 없다
pub fn menu_size_at(ctx: &egui::Context, rows: &[ShellMenuRow]) -> egui::Vec2 {
    let (항목, 구분선) = count_rows(rows);
    let style = ctx.style_of(ctx.theme());
    let inner = egui::vec2(
        menu_width(ctx),
        menu_height(항목, 구분선, style.spacing.item_spacing.y),
    );
    inner + crate::ui::menu::menu_frame_pad(&style)
}

/// 하위 메뉴가 차지할 크기 — 아이콘 줄도 마지막 줄도 없는 **줄 목록뿐**이다 (FR-8 재개정).
///
/// [`menu_size_at`]과 갈라 둔 이유는 **재는 대상이 다르기** 때문이다 — 그쪽은 부모 메뉴의
/// [`ShellMenuRow`] 목록을 받고 아이콘 줄·구분선·마지막 줄을 더하는데, 하위 메뉴에는 그
/// 셋이 없고 받는 것도 셸 항목 목록이다
pub fn submenu_size_at(ctx: &egui::Context, zip_row: bool, items: &[ShellMenuItem]) -> egui::Vec2 {
    let 구분선 = items.iter().filter(|item| item.separator).count();
    // **`Zip 파일` 줄을 함께 센다** — 그리는 쪽(`show_submenu`)이 세우는 줄이라 `items`에
    // 없다. 빠뜨리면 높이가 한 줄 모자라 화면 가장자리 보정이 어긋난다
    let 항목 = items.len() - 구분선 + usize::from(zip_row);
    let style = ctx.style_of(ctx.theme());
    let gap = style.spacing.item_spacing.y;
    let inner = egui::vec2(
        menu_width(ctx),
        theme::MENU_ITEM_HEIGHT * 항목 as f32
            + SEPARATOR_HEIGHT * 구분선 as f32
            + gap * (items.len() + usize::from(zip_row)).saturating_sub(1) as f32,
    );
    inner + crate::ui::menu::menu_frame_pad(&style)
}

/// 하위 메뉴 한 판을 그린다 — 아이콘 줄도 `기본 메뉴`도 없는 **줄 목록뿐**이다.
///
/// 그 둘은 부모 메뉴의 것이라 하위에 다시 두면 같은 것이 두 번 보인다.
///
/// `zip_row`가 참이면 맨 앞에 **Windows 기본 압축** 줄을 세운다 — `다음으로 압축` 하위에서만
/// 참이다. 그 항목은 셸 메뉴로 오지 않아 `items`에 담을 수 없고(D13-3), 여기서 세우는 것이
/// 하위 메뉴 그리기를 두 벌로 나누지 않는 가장 짧은 길이다
pub fn show_submenu(
    ui: &mut egui::Ui,
    zip_row: bool,
    items: &[ShellMenuItem],
    icons: &MenuIcons,
) -> Option<ShellMenuPick> {
    // 하위 메뉴는 부모 스타일을 잇지 않는 **별도 `Area`**라 여기서 다시 세운다
    // (AGENTS 「팝업 메뉴 한 줄」)
    theme::menu_style(ui);
    ui.set_width(menu_width(ui.ctx()));
    let mut picked = None;
    if zip_row
        && widgets::menu_row_rich(
            ui,
            MenuRowIcon::Glyph(egui_phosphor::regular::FILE_ZIP),
            crate::i18n::menu_compress_zip(),
            "",
            false,
            true,
        )
        .clicked
    {
        picked = Some(ShellMenuPick::CompressZip);
    }
    for (index, item) in items.iter().enumerate() {
        if item.separator {
            ui.separator();
            continue;
        }
        // **얹힘은 보지 않는다** — 이 판은 이미 펼쳐진 하위 메뉴라 여기서 접기를 내면
        // 자기 자신을 닫는다. 두 단계까지만 펴므로 여기서 더 펼칠 것도 없다
        if widgets::menu_row_rich(
            ui,
            // 하위 메뉴 줄에는 앱 글리프를 주지 않는다 — 이 판은 `앱 확장`과 셸 하위
            // 메뉴라 D2의 대상 밖이다
            icons.row(index, None),
            &item.label,
            &item.shortcut,
            // **두 단계까지만 편다** — 셸 메뉴가 그보다 깊은 경우는 드물고, 더 펼치면
            // 어느 것이 열려 있는지 화면에서 읽기 어렵다
            false,
            item.enabled,
        )
        .clicked
        {
            picked = Some(ShellMenuPick::Command(item.id));
        }
    }
    picked
}

/// 업로드 하위 메뉴를 그린다 — 연결된 원격 탭들 (2026-08-28).
///
/// **셸 항목을 그리는 `show_submenu`와 나눠 둔다** — 앞의 셋(`앱 확장`·압축·해제)은 재료가
/// `ShellMenuItem`이지만 이것은 앱이 모은 글자 목록이다. 억지로 한 함수에 넣으려면 `id`가
/// 셸 명령 번호인 척해야 하고, 그러면 고른 것이 `InvokeCommand`로 새어 나갈 길이 생긴다.
///
/// 줄 자체(`ShellMenuRow::Virtual`)는 넷이 함께 쓴다 — 갈리는 것은 **열린 내용**뿐이다
pub fn show_upload_submenu(ui: &mut egui::Ui, labels: &[String]) -> Option<ShellMenuPick> {
    // 하위 메뉴는 부모 스타일을 잇지 않는 **별도 `Area`**라 여기서 다시 세운다
    // (AGENTS 「팝업 메뉴 한 줄」)
    theme::menu_style(ui);
    ui.set_width(menu_width(ui.ctx()));
    let mut picked = None;
    for (index, label) in labels.iter().enumerate() {
        if widgets::menu_row_rich(
            ui,
            // 원격 탭에는 셸 아이콘이 없다 — 어느 줄이든 같은 서버 글리프를 쓴다
            MenuRowIcon::Glyph(egui_phosphor::regular::HARD_DRIVES),
            label,
            "",
            false,
            true,
        )
        .clicked
        {
            picked = Some(ShellMenuPick::UploadTo(index));
        }
    }
    picked
}

/// 업로드 하위 메뉴가 차지할 자리 — 화면 밖으로 나가지 않게 위치를 잡는 데 쓴다
pub fn upload_submenu_size(ctx: &egui::Context, count: usize) -> egui::Vec2 {
    let style = ctx.style_of(ctx.theme());
    let gap = style.spacing.item_spacing.y;
    let inner = egui::vec2(
        menu_width(ctx),
        theme::MENU_ITEM_HEIGHT * count as f32 + gap * count.saturating_sub(1) as f32,
    );
    inner + crate::ui::menu::menu_frame_pad(&style)
}

/// 업로드 하위 메뉴를 팝업으로 띄운다 — `show_submenu_popup`의 짝
pub fn show_upload_submenu_popup(
    ctx: &egui::Context,
    id: egui::Id,
    at: egui::Pos2,
    labels: &[String],
) -> (Option<ShellMenuPick>, egui::Rect) {
    let mut picked = None;
    let response = egui::Area::new(id)
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .show(ctx, |ui| {
            egui::Frame::menu(ui.style())
                .fill(theme::SURFACE_BG)
                .stroke(egui::Stroke::new(
                    theme::MENU_FRAME_STROKE,
                    theme::PANE_BORDER,
                ))
                .show(ui, |ui| {
                    picked = show_upload_submenu(ui, labels);
                });
        })
        .response;
    (picked, response.rect)
}

/// 그릴 줄을 `(항목 수, 구분선 수)`로 센다 — 구분선만 높이가 다르다
fn count_rows(rows: &[ShellMenuRow]) -> (usize, usize) {
    let separators = rows
        .iter()
        .filter(|row| matches!(row, ShellMenuRow::Separator))
        .count();
    (rows.len() - separators, separators)
}

/// 메뉴 높이 — 위젯 하나하나와 그 사이 간격을 더한다.
///
/// 놓이는 위젯은 **아이콘 줄 1 + 구분선 1 + 항목 `rows` + 셸 구분선 `separators` +
/// 구분선 1 + `기본 메뉴` 1**이고, 간격은 그 사이사이(개수 − 1)에 들어간다.
/// Win32 호출이 없어 이 계산만 따로 시험할 수 있게 떼어 두었다
fn menu_height(rows: usize, separators: usize, gap: f32) -> f32 {
    let widgets = rows + separators + 4;
    ACTION_ROW_HEIGHT
        + SEPARATOR_HEIGHT * 2.0
        + theme::MENU_ITEM_HEIGHT * (rows + 1) as f32
        + SEPARATOR_HEIGHT * separators as f32
        + gap * widgets.saturating_sub(1) as f32
}

/// 구분선 한 줄이 차지하는 높이 — egui `Separator`의 기본 `spacing`이다
/// (`egui::widgets::Separator`가 `style().separator_style(..).spacing`을 읽고, 그 기본이 6.0).
/// **위젯 사이 간격은 여기 포함되지 않는다** — 그것은 `menu_height`가 따로 더한다
const SEPARATOR_HEIGHT: f32 = 6.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn 줄(label: &str) -> ShellMenuItem {
        ShellMenuItem {
            id: 1,
            label: label.to_owned(),
            shortcut: String::new(),
            icon: None,
            enabled: true,
            checked: false,
            separator: false,
            submenu: None,
        }
    }

    /// 셸이 준 줄 하나를 표시 모델로 감싼다 — `origin`은 원래 목록에서의 자리
    fn 셸줄(label: &str, origin: usize) -> ShellMenuRow {
        ShellMenuRow::Shell {
            item: 줄(label),
            origin,
            glyph: None,
        }
    }

    /// 줄 하나의 누름·얹힘을 만든다 — `pick_for` 시험용.
    ///
    /// `top`은 0이다 — `pick_for`는 그 값을 보지 않는다(하위 팝업 위치는 `draw_row` 위쪽에서
    /// 붙는다)
    fn 얹힘(hovered: bool, clicked: bool) -> widgets::MenuRowHit {
        widgets::MenuRowHit {
            clicked,
            hovered,
            top: 0.0,
        }
    }

    /// 줄 하나를 두 프레임 그리고 둘째 프레임의 고른 값을 돌려준다 — **y는 버린다**.
    ///
    /// 그 값을 보는 시험은 [`펼침은_그_줄의_높이를_함께_알린다`]가 따로 맡는다.
    ///
    /// **두 프레임이 필요하다** — egui는 직전 프레임에 등록된 사각형으로 hit-test하므로
    /// 포인터를 옮긴 그 프레임에는 아직 얹힘이 잡히지 않는다(`widgets`의 같은 시험 참조)
    fn 얹은_채_그린다(row: &ShellMenuRow) -> Option<ShellMenuPick> {
        얹은_채_그린다_상세(row).map(|(pick, _)| pick)
    }

    /// 위와 같되 **고른 값과 그 줄의 y를 함께** 돌려준다
    fn 얹은_채_그린다_상세(row: &ShellMenuRow) -> Option<(ShellMenuPick, f32)> {
        let ctx = egui::Context::default();
        let icons = MenuIcons::for_test();

        let mut 자리 = egui::Pos2::ZERO;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            자리 = ui.cursor().min;
            draw_row(ui, row, &icons);
        });

        let 가운데 = 자리 + egui::vec2(20.0, theme::MENU_ITEM_HEIGHT / 2.0);
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::PointerMoved(가운데));
        let mut 고른것 = None;
        let _ = ctx.run_ui(input, |ui| {
            고른것 = draw_row(ui, row, &icons);
        });
        고른것
    }

    #[test]
    fn 펼침은_그_줄의_높이를_함께_알린다() {
        // `draw_row`가 pick만 돌려주면 하위 팝업을 어디에 띄울지 그리는 쪽이 알 수 없다.
        // 두 줄을 서로 다른 자리에 그려 **각자 자기 y를 올리는지** 본다
        let ctx = egui::Context::default();
        let icons = MenuIcons::for_test();
        let mut 위 = None;
        let mut 아래 = None;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            위 = draw_row(
                ui,
                &ShellMenuRow::Virtual {
                    kind: VirtualSubmenu::Extensions,
                    enabled: true,
                },
                &icons,
            );
            아래 = draw_row(
                ui,
                &ShellMenuRow::Virtual {
                    kind: VirtualSubmenu::Compress,
                    enabled: true,
                },
                &icons,
            );
        });
        // 첫 프레임에는 얹힘이 잡히지 않아 pick이 없다 — 그 사실 자체를 확인해 둔다
        assert!(위.is_none() && 아래.is_none());

        // 둘째 프레임에 아래 줄로 포인터를 옮기면 **그 줄의 y**가 함께 온다
        let mut 자리 = egui::Pos2::ZERO;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            자리 = ui.cursor().min;
            draw_row(
                ui,
                &ShellMenuRow::Virtual {
                    kind: VirtualSubmenu::Extensions,
                    enabled: true,
                },
                &icons,
            );
            draw_row(
                ui,
                &ShellMenuRow::Virtual {
                    kind: VirtualSubmenu::Compress,
                    enabled: true,
                },
                &icons,
            );
        });
        let 아래_가운데 = 자리 + egui::vec2(20.0, theme::MENU_ITEM_HEIGHT * 1.5);
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::PointerMoved(아래_가운데));
        let mut 고른것 = None;
        let _ = ctx.run_ui(input, |ui| {
            draw_row(
                ui,
                &ShellMenuRow::Virtual {
                    kind: VirtualSubmenu::Extensions,
                    enabled: true,
                },
                &icons,
            );
            고른것 = draw_row(
                ui,
                &ShellMenuRow::Virtual {
                    kind: VirtualSubmenu::Compress,
                    enabled: true,
                },
                &icons,
            );
        });
        let (pick, top) = 고른것.expect("아래 줄에 마우스가 얹혔다");
        assert_eq!(pick, ShellMenuPick::ExpandVirtual(VirtualSubmenu::Compress));
        assert!(
            top >= 자리.y + theme::MENU_ITEM_HEIGHT,
            "둘째 줄의 y여야 한다 (자리 {}, 받은 값 {top})",
            자리.y
        );
    }

    #[test]
    fn 그리는_자리가_펼침을_실제로_내보낸다() {
        // **`pick_for` 시험만으로는 배선이 확인되지 않는다** — `draw_row`가 `expand` 자리에
        // `None`을 넘기면 하위 메뉴 줄에 마우스를 올렸을 때 펼침 대신 **접기**가 나가는데,
        // 순수 시험은 그 잘못을 그대로 통과시킨다. 그래서 그리는 자리를 직접 지난다
        assert_eq!(
            얹은_채_그린다(&ShellMenuRow::Virtual {
                kind: VirtualSubmenu::Extensions,
                enabled: true
            }),
            Some(ShellMenuPick::ExpandVirtual(VirtualSubmenu::Extensions)),
            "`앱 확장`에 마우스를 올렸는데 펼침이 나오지 않았다"
        );

        let mut item = 줄("보내기");
        item.submenu = Some(crate::fs::shell_menu::SubmenuHandle::for_test(7, 3));
        let 펼침 = 얹은_채_그린다(&ShellMenuRow::Shell {
            item,
            origin: 0,
            glyph: None,
        });
        assert!(
            matches!(펼침, Some(ShellMenuPick::Expand(_))),
            "하위 메뉴가 있는 셸 줄에 마우스를 올렸는데 펼침이 아니라 {펼침:?}가 나왔다"
        );
    }

    #[test]
    fn 불러오는_중_줄은_얹혀도_아무것도_고르지_않는다() {
        // 뼈대 메뉴의 자리 표시 — 얹힘으로 펼쳐 둔 하위 메뉴를 접지도, 실행을 내지도 않는다
        assert_eq!(얹은_채_그린다(&ShellMenuRow::Loading), None);
    }

    #[test]
    fn 불러오는_중_하위_줄은_눌러도_실행하지_않는다() {
        fn press(pos: egui::Pos2, pressed: bool) -> egui::Event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }
        }
        let ctx = egui::Context::default();
        let icons = MenuIcons::for_test();
        let items = [loading_submenu_item()];
        let 자리 = std::cell::Cell::new(egui::Pos2::ZERO);
        let frame = |time: f64, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut 고른것 = None;
            let _ = ctx.run_ui(input, |ui| {
                자리.set(ui.cursor().min);
                고른것 = show_submenu(ui, false, &items, &icons);
            });
            고른것
        };
        assert_eq!(frame(0.0, Vec::new()), None);
        let 가운데 = 자리.get() + egui::vec2(20.0, theme::MENU_ITEM_HEIGHT / 2.0);
        assert_eq!(frame(0.05, vec![press(가운데, true)]), None);
        assert_eq!(
            frame(0.10, vec![press(가운데, false)]),
            None,
            "자리 표시 줄이 셸 명령(id 0)을 냈다"
        );
    }

    #[test]
    fn 불러오는_중_하위_줄은_한_줄_높이로_센다() {
        // 폭은 글자를 재므로 글꼴이 선 프레임 안에서 부른다
        let ctx = egui::Context::default();
        let mut 크기 = None;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            크기 = Some((
                submenu_size_at(ui.ctx(), false, &[loading_submenu_item()]),
                submenu_size_at(ui.ctx(), false, &[줄("열기")]),
            ));
        });
        let (로딩, 한줄) = 크기.expect("한 프레임을 그렸다");
        assert_eq!(로딩, 한줄);
    }

    #[test]
    fn 흐린_줄은_얹혀도_펼치지_않는다() {
        // 누를 수 없는 줄이 마우스만 스쳐 열리면 그 하위 팝업의 줄들은 각자 활성이라
        // 실행까지 이어진다 — 얹힘은 비활성 줄에서도 참이므로 그리는 자리가 걸러야 한다
        let mut item = 줄("보내기");
        item.enabled = false;
        item.submenu = Some(crate::fs::shell_menu::SubmenuHandle::for_test(7, 3));
        assert_eq!(
            얹은_채_그린다(&ShellMenuRow::Shell {
                item,
                origin: 0,
                glyph: None,
            }),
            Some(ShellMenuPick::CollapseSubmenu),
            "흐린 줄이 펼침을 냈다 — 하위 메뉴가 열려 그 안의 항목이 실행될 수 있다"
        );
    }

    #[test]
    fn 하위_메뉴가_있는_줄은_얹히기만_해도_펼친다() {
        // 2026-08-26 사용자 요청 — 종전에는 눌러야 열렸다
        let 손잡이 = ShellMenuPick::Expand(SubmenuHandle::for_test(7, 2));
        assert_eq!(
            pick_for(
                얹힘(true, false),
                Some(손잡이.clone()),
                ShellMenuPick::Command(1)
            ),
            Some(손잡이.clone()),
            "얹히기만 해도 펼친다"
        );
        // 눌러도 같은 결과다 — 키보드·터치 경로를 막지 않는다
        assert_eq!(
            pick_for(
                얹힘(true, true),
                Some(손잡이.clone()),
                ShellMenuPick::Command(1)
            ),
            Some(손잡이)
        );
    }

    #[test]
    fn 하위_메뉴가_없는_줄에_얹히면_펼친_것을_접는다() {
        // 접지 않으면 마우스가 다른 줄로 가도 하위 팝업이 남아 어느 줄의 것인지 알 수 없다
        assert_eq!(
            pick_for(얹힘(true, false), None, ShellMenuPick::Command(9)),
            Some(ShellMenuPick::CollapseSubmenu)
        );
    }

    #[test]
    fn 누름이_얹힘보다_앞선다() {
        // **누른 프레임에는 얹힘도 함께 참이다**(같은 `Response`) — 얹힘을 먼저 보면
        // 하위 메뉴 없는 줄을 눌렀을 때 실행 대신 접기가 나가 **메뉴의 모든 실행이 죽는다**
        assert_eq!(
            pick_for(얹힘(true, true), None, ShellMenuPick::Command(9)),
            Some(ShellMenuPick::Command(9)),
            "누름이 이겨야 한다"
        );
        assert_eq!(
            pick_for(
                얹힘(true, true),
                None,
                ShellMenuPick::App(AppMenuItem::AddFavorite)
            ),
            Some(ShellMenuPick::App(AppMenuItem::AddFavorite))
        );
        // **`기본 메뉴` 줄도 같은 판정을 쓴다** — 그 줄만 따로 쓰면 순서가 두 곳에서
        // 갈릴 수 있다. 눌렀으면 표준 메뉴를 열고, 얹히기만 했으면 펼친 것을 접는다
        assert_eq!(
            pick_for(얹힘(true, true), None, ShellMenuPick::ShowMore),
            Some(ShellMenuPick::ShowMore)
        );
        assert_eq!(
            pick_for(얹힘(true, false), None, ShellMenuPick::ShowMore),
            Some(ShellMenuPick::CollapseSubmenu)
        );
    }

    #[test]
    fn 아무_일도_없으면_고른_것이_없다() {
        assert_eq!(
            pick_for(얹힘(false, false), None, ShellMenuPick::Command(1)),
            None
        );
        // 하위 메뉴가 있는 줄도 마우스가 없으면 펼치지 않는다
        assert_eq!(
            pick_for(
                얹힘(false, false),
                Some(ShellMenuPick::ExpandVirtual(VirtualSubmenu::Extensions)),
                ShellMenuPick::ExpandVirtual(VirtualSubmenu::Extensions)
            ),
            None
        );
    }

    #[test]
    fn 고른_것이_없으면_아이콘_줄이_통째로_비활성이다() {
        // 폴더 배경 메뉴다 — 잘라낼 것도 지울 것도 없다
        let state = MenuState {
            selected: 0,
            can_rename: true,
        };
        for action in MenuAction::ALL {
            assert!(!state.enabled(action), "{action:?} 는 열리면 안 된다");
        }
    }

    #[test]
    fn 아이콘_줄은_네_칸이다() {
        // 2026-08-22 사용자 요청으로 다섯째 칸을 뺐다 — 칸 수가 줄면 폭 셈(`width_for`)도
        // 따라 바뀌므로 그 수를 여기서 못 박는다
        assert_eq!(
            MenuAction::ALL,
            [
                MenuAction::Cut,
                MenuAction::Copy,
                MenuAction::Rename,
                MenuAction::Delete
            ]
        );
    }

    #[test]
    fn 이름_바꾸기는_하나를_골랐을_때만_열린다() {
        // 새 이름은 하나뿐이라 여럿에 줄 수 없다 (원격 메뉴의 같은 규칙)
        let 하나 = MenuState {
            selected: 1,
            can_rename: true,
        };
        let 여럿 = MenuState {
            selected: 3,
            can_rename: true,
        };
        assert!(하나.enabled(MenuAction::Rename));
        assert!(!여럿.enabled(MenuAction::Rename));
        assert!(여럿.enabled(MenuAction::Copy), "복사는 여럿이어도 된다");
    }

    #[test]
    fn 이름_편집을_받을_곳이_없으면_그_칸도_비활성이다() {
        // 「대응 기능이 없는 칸은 비활성」 — 눌러도 아무 일이 없는 것은 비활성보다 나쁘다
        let state = MenuState {
            selected: 1,
            can_rename: false,
        };
        assert!(!state.enabled(MenuAction::Rename));
        assert!(state.enabled(MenuAction::Delete), "삭제는 그대로 열린다");
    }

    #[test]
    fn 줄과_구분선을_따로_센다() {
        let rows = vec![셸줄("열기", 0), ShellMenuRow::Separator, 셸줄("복사", 2)];
        assert_eq!(count_rows(&rows), (2, 1));
        assert_eq!(count_rows(&[]), (0, 0));
    }

    #[test]
    fn 앱_줄과_확장_머리도_항목으로_센다() {
        // 높이 셈이 이 수를 쓰므로, 종류를 빠뜨리면 메뉴 아래가 화면 밖으로 나간다
        let rows = vec![
            셸줄("열기", 0),
            ShellMenuRow::App {
                item: AppMenuItem::AddFavorite,
                enabled: true,
            },
            ShellMenuRow::Separator,
            ShellMenuRow::Virtual {
                kind: VirtualSubmenu::Extensions,
                enabled: true,
            },
        ];
        assert_eq!(count_rows(&rows), (3, 1));
    }

    #[test]
    fn 앱_줄의_차례는_탐색기와_같다() {
        // 기준 화면에서 `새 탭에서 열기`는 `열기` 바로 아래(2), `즐겨찾기에 추가`는
        // `다음으로 압축`과 `속성` 사이(7)다
        assert_eq!(AppMenuItem::OpenInNewTab.order(), 2);
        assert_eq!(AppMenuItem::AddFavorite.order(), 7);
    }

    #[test]
    fn 셸이_준_그림이_앱_글리프보다_앞선다() {
        // **셸 비트맵이 그 항목의 진짜 아이콘이다**(D1) — 앱 글리프로 덮으면 탐색기와
        // 오히려 멀어진다. 그림이 없을 때만 글리프가 그 자리를 채운다
        let ctx = egui::Context::default();
        let items = vec![
            ShellMenuItem {
                icon: Some((1, 1, vec![10, 10, 10, 255])),
                ..줄("그림 있는 줄")
            },
            줄("그림 없는 줄"),
        ];
        let icons = MenuIcons::build(&ctx, &items);
        let 글리프 = Some(egui_phosphor::regular::FOLDER_OPEN);

        assert!(
            matches!(icons.row(0, 글리프), MenuRowIcon::Texture(_)),
            "셸이 준 그림이 있는데 앱 글리프가 이겼다"
        );
        assert!(
            matches!(icons.row(1, 글리프), MenuRowIcon::Glyph(_)),
            "그림이 없으면 글리프가 그 자리를 채워야 한다"
        );
        assert!(
            matches!(icons.row(1, None), MenuRowIcon::Blank),
            "둘 다 없으면 열만 남긴다"
        );
    }

    #[test]
    fn 확장_아이콘은_원래_자리로_찾는다() {
        // `앱 확장` 하위 메뉴의 줄은 부모 목록에서 골라낸 것이라 자리가 0부터 다시
        // 매겨진다 — 부분집합 캐시를 만들지 않으면 모든 줄에 엉뚱한 그림이 붙는다
        let ctx = egui::Context::default();
        let 그림 = |값: u8| Some((1, 1, vec![값, 값, 값, 255]));
        let items = vec![
            ShellMenuItem {
                icon: 그림(10),
                ..줄("첫째")
            },
            ShellMenuItem {
                icon: 그림(20),
                ..줄("둘째")
            },
            ShellMenuItem {
                icon: 그림(30),
                ..줄("셋째")
            },
        ];
        let icons = MenuIcons::build(&ctx, &items);
        // 셋째·첫째만 확장으로 밀렸다고 하면, 부분집합의 0·1번이 그 둘이어야 한다
        let 부분 = icons.subset(&[2, 0]);
        assert_eq!(부분.textures.len(), 2);
        assert!(부분.textures[0].is_some() && 부분.textures[1].is_some());
        // 원래 캐시는 그대로다 — 복제이지 이동이 아니다
        assert_eq!(icons.textures.len(), 3);
    }

    #[test]
    fn 높이는_아이콘_줄과_항목과_구분선과_그_사이_간격을_더한_값이다() {
        // 이 값이 틀리면 화면 가장자리 보정이 어긋나 메뉴가 잘린다
        let 기대 = ACTION_ROW_HEIGHT
            + SEPARATOR_HEIGHT * 2.0
            + theme::MENU_ITEM_HEIGHT * 3.0
            + SEPARATOR_HEIGHT
            // 놓이는 위젯 7개(아이콘 줄·구분선·항목 2·셸 구분선·구분선·추가 옵션) 사이 6칸
            + 3.0 * 6.0;
        assert_eq!(menu_height(2, 1, 3.0), 기대, "항목 2 + 추가 옵션 1 = 세 줄");
    }

    #[test]
    fn 위젯_사이_간격을_빠뜨리지_않는다() {
        // 간격을 0으로 두면 그만큼 낮게 재어 메뉴 아래가 화면 밖으로 나간다
        let 간격_없음 = menu_height(5, 2, 0.0);
        let 간격_있음 = menu_height(5, 2, 3.0);
        assert!(간격_있음 > 간격_없음);
        assert_eq!(간격_있음 - 간격_없음, 3.0 * 10.0, "위젯 11개 사이 10칸");
    }

    #[test]
    fn 항목이_없어도_아이콘_줄과_추가_옵션은_남는다() {
        // 셸이 아무것도 주지 못한 경우다 — 그래도 표준 메뉴로 가는 길은 있어야 한다
        let 기대 = ACTION_ROW_HEIGHT
            + SEPARATOR_HEIGHT * 2.0
            + theme::MENU_ITEM_HEIGHT
            // 위젯 4개(아이콘 줄·구분선·구분선·추가 옵션) 사이 3칸
            + 3.0 * 3.0;
        assert_eq!(menu_height(0, 0, 3.0), 기대);
    }

    #[test]
    fn 아이콘_줄_이름이_칸_안에_들어간다() {
        // 2026-08-22 사용자 보고 두 번 — 260px에서도 340px에서도 `이름 바꾸기`가 잘렸다.
        // 어림값을 고치는 대신 **실제 글자 폭을 재서** 폭을 정한다
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let 폭 = menu_width(ui.ctx());
            let 칸 = 폭 / MenuAction::ALL.len() as f32;
            for action in MenuAction::ALL {
                let 글자 = ui
                    .painter()
                    .layout_no_wrap(
                        action.tooltip().to_owned(),
                        egui::FontId::proportional(ACTION_LABEL_PX),
                        theme::TEXT,
                    )
                    .size()
                    .x;
                assert!(
                    글자 + ACTION_LABEL_PAD_X * 2.0 <= 칸,
                    "{:?}: 글자 {글자}px + 여백이 칸 {칸}px를 넘는다",
                    action
                );
            }
            // 이름이 짧아도 메뉴가 옹색해지지 않는다
            assert!(폭 >= MENU_MIN_WIDTH);
        });
    }

    #[test]
    fn 폭은_가장_긴_이름이_칸에_들어가게_정해진다() {
        // 위 시험은 **시험 환경의 기본 글꼴에 한글이 없어** 폭이 최소값으로 떨어진다 —
        // 규칙 자체는 이 순수 셈으로 고정한다 (2026-08-22 사용자 보고 두 번)
        let 칸 = |이름: f32| width_for(이름) / MenuAction::ALL.len() as f32;
        // 이름이 짧으면 최소 폭을 지킨다
        assert_eq!(width_for(10.0), MENU_MIN_WIDTH);
        // 이름이 길면 그만큼 벌어진다 — 이름 + 좌우 여백이 한 칸에 든다
        for 이름 in [40.0_f32, 64.0, 120.0] {
            assert!(
                이름 + ACTION_LABEL_PAD_X * 2.0 <= 칸(이름),
                "이름 {이름}px가 칸 {}px를 넘는다",
                칸(이름)
            );
        }
    }
}
