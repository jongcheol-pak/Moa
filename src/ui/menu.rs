//! 패널 메뉴와 단축키 (FR-12·FR-26).
//!
//! **상단 메뉴 바는 두지 않는다.** 종전의 보기·이동·탭·워크스페이스 네 메뉴는 항목이 모두
//! 다른 진입점(주소창 버튼·탭 스트립·사이드바 `+`·컨텍스트 메뉴)에 있었고, 유일하게 겹치지
//! 않던 '패널 닫기'는 이 패널 메뉴로 옮겼다.
//!
//! 이 모듈은 상태를 바꾸지 않는다 — 무엇을 하라는 **명령만 값으로 돌려주고**,
//! 실행은 `ui::app`이 한다(패널·워크스페이스 소유자가 거기이기 때문).
use crate::app::layout::{SplitDir, SplitPlace};
use crate::remote::types::SiteId;
use crate::ui::list_details::{ALL_COLUMNS, ColumnFlags, ColumnKind};
use crate::ui::queue_panel::QueueColumnKind;
use crate::ui::shell_host::AppNav;
use crate::ui::theme;
use crate::ui::view_mode::ViewMode;
use eframe::egui;

// ── 열 메뉴 시각 토큰 (원본 `FileExplorer-FTP.dc.html:337-342`) ──
/// 메뉴 폭
const COLUMN_MENU_WIDTH: f32 = 186.0;
/// 캡션 글자 크기
const COLUMN_MENU_CAPTION_PX: f32 = 12.0;
/// 체크 글리프가 차지하는 폭 — 켜짐/꺼짐이 섞여도 라벨이 흔들리지 않게 자리를 고정한다
const COLUMN_CHECK_WIDTH: f32 = 12.0;

/// 분할 방향 — 새 패널이 놓일 자리를 사용자 관점으로 나타낸다.
///
/// 트리는 축(`SplitDir`)과 앞뒤(`SplitPlace`)만 알므로 여기서 한 번 변환한다 (plan D1)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitTo {
    Right,
    Left,
    Up,
    Down,
}

impl SplitTo {
    /// 레이아웃 트리가 쓰는 (축, 배치)로 바꾼다
    pub fn to_layout(self) -> (SplitDir, SplitPlace) {
        match self {
            SplitTo::Right => (SplitDir::Horizontal, SplitPlace::After),
            SplitTo::Left => (SplitDir::Horizontal, SplitPlace::Before),
            SplitTo::Up => (SplitDir::Vertical, SplitPlace::Before),
            SplitTo::Down => (SplitDir::Vertical, SplitPlace::After),
        }
    }
}

/// 메뉴·단축키가 요청하는 동작.
///
/// 워크스페이스 생성·이름 변경·삭제는 여기 없다 — 메뉴 바를 없애면서 생성처가 사라졌고,
/// 그 기능들은 사이드바의 `+`·컨텍스트 메뉴가 `SidebarAction`으로 직접 처리한다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    NewTab,
    CloseTab,
    Back,
    Forward,
    Up,
    Refresh,
    Split(SplitTo),
    ClosePanel,
    /// 표시 중인 폴더에 빈 텍스트 문서를 만든다 (FR-25)
    NewFile,
    /// 표시 중인 폴더에 새 폴더를 만든다 (FR-25)
    NewFolder,
    /// 파일 목록 보기 모드를 바꾼다 (FR-23)
    SetViewMode(ViewMode),
    ToggleSidebar,
    /// 앱 설정 대화를 연다 (FR-47) — 타이틀바 설정 메뉴의 `설정` 항목이 유일한 진입점이다.
    ///
    /// 이름에 `App`을 붙인 것은 이 코드베이스에 `RemoteAction::OpenSettings`·
    /// `FailedAction::OpenSettings`가 이미 있고 **둘 다 사이트 관리자**를 뜻하기 때문이다 (D11)
    OpenAppSettings,
    /// 오픈소스 라이선스 대화를 연다 (FR-57) — 타이틀바 설정 메뉴의 그 항목이 유일한 진입점이다
    OpenLicenses,
    /// 정보 대화를 연다 (FR-58) — 타이틀바 설정 메뉴의 `정보` 항목이 유일한 진입점이다
    OpenAbout,
    /// 최신 판이 있는지 지금 확인한다 (FR-62) — 설정 메뉴의 `업데이트` 항목.
    ///
    /// 앱이 뜰 때 한 번은 저절로 확인하므로, 이 명령은 **손으로 다시 묻는** 길이다.
    /// 그래서 결과를 알림으로 보인다(저절로 도는 확인은 조용하다)
    CheckUpdate,
    /// 받아 둔 새 판을 설치한다 (FR-62) — 타이틀바 업데이트 배지를 누르면 나온다.
    ///
    /// 아직 받지 않았으면 내려받기부터 시작한다
    StartUpdate,
    /// 릴리즈 노트 페이지를 기본 브라우저로 연다 (FR-63) — 설정 메뉴의 그 항목이
    /// 유일한 진입점이다
    OpenReleaseNotes,
    /// 이 사이트를 **그 패널의 새 원격 탭**으로 열고 연결한다 (FR-33·FR-34·FR-38).
    ///
    /// 탭 스트립에서 여는 둘(`연결 사이트를 새 탭으로` 드롭다운·스트립에 끌어다 놓기)이
    /// 이 한 명령으로 착지한다 — 여는 방법마다 다른 경로를 두면 둘이 조금씩 다르게 동작한다.
    /// 나누지 않는 것이 이 명령의 뜻이다: 사이드바·사이트 관리자에서 여는 길은 활성 패널을
    /// 좌우로 나눠 열며(FR-35) 이 명령을 거치지 않는다.
    /// `SiteId`가 `Copy`라 이 열거형의 `Copy`도 유지된다
    OpenSiteTab(SiteId),
    /// 고른 것 중 첫 항목의 이름을 목록에서 바로 고친다 (FR-12·FR-64) — `F2`.
    ///
    /// 여러 개를 골라 두어도 하나만 고친다(새 이름은 하나뿐이다 — 탐색기와 같다)
    Rename,
    /// 고른 것을 지운다 (FR-12·FR-64) — `Delete`는 휴지통, `Shift+Delete`는 곧바로.
    ///
    /// 확인 대화는 셸이 자기 정책대로 띄운다(우리가 묻지 않는다)
    Delete {
        permanent: bool,
    },
    /// 고른 것을 클립보드에 담는다 (FR-12·FR-64) — `Ctrl+C`
    ClipboardCopy,
    /// 고른 것을 잘라내기로 담는다 (FR-12·FR-64) — `Ctrl+X`.
    ///
    /// 담는 순간 원본이 사라지지는 않는다 — 붙여넣을 때 옮겨진다(탐색기와 같다)
    ClipboardCut,
    /// 클립보드에 담긴 것을 지금 폴더에 붙여넣는다 (FR-12·FR-64) — `Ctrl+V`.
    ///
    /// 복사로 담겼으면 복사하고 잘라내기로 담겼으면 옮긴다
    ClipboardPaste,
}

/// 패널 메뉴 항목의 활성/비활성을 가르는 현재 상태
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelMenuState {
    /// 패널이 2개 이상인가 — 마지막 하나는 닫을 수 없다 (FR-2)
    pub can_close_panel: bool,
    /// 지금 이 패널이 쓰는 보기 모드 — 하위 메뉴에서 점으로 표시한다 (FR-23)
    pub view_mode: ViewMode,
}

impl PanelMenuState {
    /// 화면에 있는 패널 수로 활성 조건을 정한다.
    ///
    /// 패널은 서로를 모르므로 이 판정은 트리를 아는 쪽(`ui::splitter`)이 내려준다 (plan D15).
    /// 그 계산을 여기 두는 이유는 "마지막 하나는 닫을 수 없다"는 규칙(FR-2)이 갈리지 않게
    /// 한 곳에 모으기 위해서다
    pub fn for_panes(pane_count: usize, view_mode: ViewMode) -> PanelMenuState {
        PanelMenuState {
            can_close_panel: pane_count > 1,
            view_mode,
        }
    }
}

/// 패널 메뉴를 그리고 고른 항목을 돌려준다 (FR-26).
///
/// 항목 순서·구분선 위치는 plan `## 시각 요소 분해`의 인벤토리 표 13행 그대로다.
/// 진입점이 이 메뉴 하나뿐이므로, 여기서 빠진 기능은 마우스로 닿을 수 없게 된다
pub fn panel_menu_items(ui: &mut egui::Ui, state: PanelMenuState, out: &mut Option<Command>) {
    // 마우스를 올리기만 해도 펼쳐진다 — `SubMenuButton`이 hover로 여는 팝업이다 (사용자 요청 8번).
    // 화살표를 egui 기본값(`⏵` U+23F5) 대신 아이콘 글꼴에서 가져오는 이유: 이 앱은 egui 내장
    // 글꼴을 끄고 맑은 고딕만 쓰는데 맑은 고딕에 U+23F5가 없어 두부(`?`)로 보였다
    egui::containers::menu::SubMenuButton::from_button(
        egui::Button::new(crate::i18n::menu_view()).right_text(egui_phosphor::regular::CARET_RIGHT),
    )
    .ui(ui, |ui| view_items(ui, state.view_mode, out));
    ui.separator();
    split_items(ui, out);
    ui.separator();
    item(
        ui,
        crate::i18n::menu_refresh(),
        "F5",
        true,
        Command::Refresh,
        out,
    );
    ui.separator();
    item(
        ui,
        crate::i18n::menu_new_file(),
        "",
        true,
        Command::NewFile,
        out,
    );
    item(
        ui,
        crate::i18n::menu_new_folder(),
        "",
        true,
        Command::NewFolder,
        out,
    );
    ui.separator();
    item(
        ui,
        crate::i18n::close(),
        "Ctrl+Shift+W",
        state.can_close_panel,
        Command::ClosePanel,
        out,
    );
}

/// 열 메뉴 — 목록 머리글 우클릭으로 열린다 (인벤토리 #22~28).
///
/// 앞 넷은 **체크된 채 비활성**이다: 눌러도 아무 일이 없고 글자가 흐리다(원본의 `cursor:default`).
/// 지우는 대신 남겨 두는 이유는 원본이 그렇기 때문이며, 어떤 열이 있는지 한눈에 보인다.
///
/// 뒤집을 열을 값으로 돌려주고 상태는 호출부가 바꾼다 — 이 모듈은 상태를 갖지 않는다
pub fn column_menu_items(ui: &mut egui::Ui, flags: ColumnFlags, out: &mut Option<ColumnKind>) {
    // 스타일은 여기서 세우지 않는다 — 팝업을 여는 쪽(`list_details`)이 `theme::menu_style`을
    // 부른다는 한 가지 규칙을 따른다. 두 곳에서 세우면 어느 값이 실제로 먹는지 흐려진다
    ui.set_width(COLUMN_MENU_WIDTH);
    ui.label(
        egui::RichText::new(crate::i18n::menu_columns())
            .size(COLUMN_MENU_CAPTION_PX)
            .color(theme::TEXT_MUTED),
    );
    for kind in ALL_COLUMNS {
        // 행 높이는 적지 않는다 — `menu_style`이 세운 공통 값(`theme::MENU_ITEM_HEIGHT`)을 따른다
        let button = egui::Button::new(column_menu_label(ui, kind, flags.shows(kind)));
        // 고정 열도 그리기는 한다 — 클릭만 무시한다(원본과 같은 동작).
        // 커서도 손가락이 아니라 기본 화살표로 둔다(원본의 `cursor:default`) —
        // 누를 수 있는 것처럼 보이면 눌러 보고 아무 일이 없어 고장으로 읽힌다
        let response = ui.add(button);
        if kind.is_fixed() {
            response.on_hover_cursor(egui::CursorIcon::Default);
            continue;
        }
        if response.clicked() {
            *out = Some(kind);
            ui.close();
        }
    }
}

/// 큐 표의 열 메뉴 (FR-36) — 그 탭에 있는 열만 보이고 고정 열은 눌러도 바뀌지 않는다.
///
/// **위 `column_menu_items`와 합치지 않는 이유**: 그쪽은 `ColumnFlags`(권한·소유자 두 bool)를
/// 받는 파일 목록 전용 형태이고, 큐는 종류가 아홉이며 **탭마다 후보가 다르다**. 재료가 달라
/// 합치면 분기 인자만 늘고, 반복은 아직 2회째라 공통화 문턱(3회)에도 미달한다 (plan D6)
pub fn queue_column_menu_items(
    ui: &mut egui::Ui,
    kinds: &[QueueColumnKind],
    hidden: &[QueueColumnKind],
    out: &mut Option<QueueColumnKind>,
) {
    ui.set_width(COLUMN_MENU_WIDTH);
    ui.label(
        egui::RichText::new(crate::i18n::menu_columns())
            .size(COLUMN_MENU_CAPTION_PX)
            .color(theme::TEXT_MUTED),
    );
    for &kind in kinds {
        let shown = !hidden.contains(&kind);
        let response = ui.add(egui::Button::new(queue_column_menu_label(ui, kind, shown)));
        // 고정 열도 그리기는 한다 — 클릭만 무시한다(파일 목록 열 메뉴와 같은 규칙)
        if kind.is_fixed() {
            response.on_hover_cursor(egui::CursorIcon::Default);
            continue;
        }
        if response.clicked() {
            *out = Some(kind);
            ui.close();
        }
    }
}

/// 큐 열 메뉴 한 줄의 글자 — 위 `column_menu_label`과 같은 모양이되 종류가 다르다
fn queue_column_menu_label(
    ui: &egui::Ui,
    kind: QueueColumnKind,
    checked: bool,
) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    job.append(
        if checked {
            egui_phosphor::regular::CHECK
        } else {
            " "
        },
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color: theme::OK_TEXT,
            ..Default::default()
        },
    );
    job.append(
        kind.header(),
        COLUMN_CHECK_WIDTH,
        egui::TextFormat {
            font_id: font,
            color: if kind.is_fixed() {
                theme::TEXT_DIM
            } else {
                theme::TEXT
            },
            ..Default::default()
        },
    );
    job
}

/// 열 메뉴 한 줄의 글자 — 체크 글리프만 초록이고 라벨은 켤 수 있는지에 따라 색이 다르다
fn column_menu_label(ui: &egui::Ui, kind: ColumnKind, checked: bool) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    job.append(
        if checked {
            egui_phosphor::regular::CHECK
        } else {
            " "
        },
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color: theme::OK_TEXT,
            ..Default::default()
        },
    );
    job.append(
        kind.label(),
        COLUMN_CHECK_WIDTH,
        egui::TextFormat {
            font_id: font,
            // 끌 수 없는 열은 흐리게 — 눌러도 바뀌지 않는다는 것을 색으로 알린다
            color: if kind.is_fixed() {
                theme::TEXT_DIM
            } else {
                theme::TEXT
            },
            ..Default::default()
        },
    );
    job
}

/// 보기 모드 8종 (FR-23) — 지금 쓰는 모드 왼쪽에 점을 찍는다.
///
/// 문구·순서는 plan `### 참조 정합 인벤토리 — '보기' 하위 메뉴` 8행 그대로다.
/// 모드를 나타내는 아이콘은 넣지 않는다 — phosphor에 대응 글리프가 없어 두부가 될 위험이
/// 있고(사이드바 `◧` 사례), 점만으로도 지금 모드가 드러난다
fn view_items(ui: &mut egui::Ui, current: ViewMode, out: &mut Option<Command>) {
    // 하위 메뉴는 부모 팝업의 스타일을 잇지 않는 **별도 `Area`**라 여기서 다시 세운다
    theme::menu_style(ui);
    for mode in ViewMode::ALL {
        let mark = if mode == current {
            egui_phosphor::regular::DOT_OUTLINE
        } else {
            " "
        };
        let button = egui::Button::new(format!("{mark} {}", mode.label()));
        if ui.add(button).clicked() {
            *out = Some(Command::SetViewMode(mode));
            ui.close();
        }
    }
}

/// 네 방향 분할 항목 (FR-1) — 패널 메뉴 안에 놓인다
fn split_items(ui: &mut egui::Ui, out: &mut Option<Command>) {
    for (label, shortcut, to) in [
        (
            crate::i18n::menu_split_right(),
            "Ctrl+Alt+→",
            SplitTo::Right,
        ),
        (crate::i18n::menu_split_left(), "Ctrl+Alt+←", SplitTo::Left),
        (crate::i18n::menu_split_up(), "Ctrl+Alt+↑", SplitTo::Up),
        (crate::i18n::menu_split_down(), "Ctrl+Alt+↓", SplitTo::Down),
    ] {
        item(ui, label, shortcut, true, Command::Split(to), out);
    }
}

/// 메뉴 항목 하나 — 오른쪽에 단축키를 함께 보인다(`shortcut`이 비면 표기하지 않는다)
fn item(
    ui: &mut egui::Ui,
    label: &str,
    shortcut: &str,
    enabled: bool,
    command: Command,
    out: &mut Option<Command>,
) {
    let button = egui::Button::new(label).right_text(shortcut);
    if ui.add_enabled(enabled, button).clicked() {
        *out = Some(command);
        ui.close();
    }
}

/// 무수식 키(`F2`·`Delete`)를 받는 영역 (FR-12).
///
/// **마지막으로 조작한 쪽이 갖는다** — 탐색기와 같은 방식이다. 지금 갈라야 하는 것은 두
/// 영역뿐이라 일반 포커스 체계(위젯별 포커스 링·Tab 순회)를 만들지 않는다.
///
/// `ui::app`이 이 값을 들고 `ui::menu`·`ui::sidebar`에 내려 준다 — 타입을 `ui::app`이 아니라
/// 여기 두는 이유는 사이드바가 `ui::app`을 참조하게 되면 의존이 거꾸로 서기 때문이다
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyOwner {
    /// 파일 목록 — 앱이 막 떴을 때의 기본값이다(아무것도 누르지 않은 상태)
    #[default]
    FileList,
    /// 워크스페이스 사이드바
    Sidebar,
}

/// 이번 프레임에 눌린 자리로 키 소유를 옮긴다 (FR-12).
///
/// **누른 곳이 없으면 그대로 둔다**(`pressed`가 `None`) — 마우스가 지나가기만 해도 대상이
/// 바뀌면 `F2`가 어디로 갈지 예측할 수 없다(그래서 hover가 아니라 클릭으로 가른다).
///
/// 누른 영역을 `Option` 하나로 받는 이유: 부르는 쪽은 한 프레임에 **사이드바를 그린 뒤**와
/// **패널을 그린 뒤** 두 번 부르는데, 각 시점에 아는 것은 자기 영역 하나뿐이다. 두 개의
/// 불리언으로 받으면 한쪽은 늘 거짓이라, 있지도 않은 "동시에 눌렸을 때의 우선순위"를
/// 시그니처가 약속하게 된다(품질 리뷰 S1)
pub fn next_key_owner(current: KeyOwner, pressed: Option<KeyOwner>) -> KeyOwner {
    pressed.unwrap_or(current)
}

/// 이 명령이 **파일 목록의 고른 항목**을 대상으로 하는가 (FR-12).
///
/// 그런 명령은 파일 목록이 키를 가질 때만 듣는다 — 사이드바를 누른 뒤 `Delete`를 눌렀는데
/// 파일이 지워지면 안 된다. 나머지(탐색·분할·탭·보기)는 어느 쪽을 눌렀든 활성 패널에 간다.
///
/// **망라 `match`로 적는다** — 새 명령을 더할 때 컴파일러가 이 판정을 묻게 한다
fn targets_file_list(command: Command) -> bool {
    match command {
        Command::NewTab
        | Command::CloseTab
        | Command::Back
        | Command::Forward
        | Command::Up
        | Command::Refresh
        | Command::Split(_)
        | Command::ClosePanel
        | Command::NewFile
        | Command::NewFolder
        | Command::SetViewMode(_)
        | Command::ToggleSidebar
        | Command::OpenAppSettings
        | Command::OpenLicenses
        | Command::OpenAbout
        | Command::CheckUpdate
        | Command::StartUpdate
        | Command::OpenReleaseNotes
        | Command::OpenSiteTab(_) => false,
        // 고른 항목이 대상이다 — 파일 목록이 키를 쥔 때만 나간다
        Command::Rename
        | Command::Delete { .. }
        | Command::ClipboardCopy
        | Command::ClipboardCut
        | Command::ClipboardPaste => true,
    }
}

/// 이번 프레임에 눌린 단축키를 명령으로 바꾼다.
///
/// 무수식 키(`F2`·`Delete`·`F5`)도 여기서 받는다 — 이름 편집 중에는 아래 `egui_wants_keyboard_input`이
/// 먼저 걸러 텍스트 입력을 가로채지 않는다.
///
/// **어느 영역이 키를 갖는지는 `owner`가 정한다** (FR-12). 사이드바가 키를 **전역으로** 보던
/// 종전 구조에서는 파일 목록에서 누른 `F2`까지 워크스페이스 이름 편집을 열었다
/// (`docs/plans/deferred.md` 2026-07-28). 이제 마지막으로 누른 영역이 키를 가지며,
/// 파일 목록을 대상으로 하는 명령은 그쪽이 키를 쥔 때만 나간다
pub fn poll_shortcuts(ctx: &egui::Context, owner: KeyOwner) -> Option<Command> {
    // 포커스를 가진 위젯이 있으면 단축키를 보지 않는다 — 주소창·이름 편집이 키를 먼저 가져간다.
    // 이 검사는 텍스트 입력뿐 아니라 포커스를 받은 위젯 전부를 덮는다(가로채기를 막는 쪽으로 넉넉하게)
    if ctx.egui_wants_keyboard_input() {
        return None;
    }
    ctx.input_mut(|input| {
        shortcut_table()
            .into_iter()
            .find_map(|(modifiers, key, command)| {
                // 파일 목록이 키를 갖지 않으면 그 대상 명령은 **소비하지도 않는다** —
                // 소비해 버리면 그 키를 기다리던 다른 곳(사이드바)이 받지 못한다
                if targets_file_list(command) && owner != KeyOwner::FileList {
                    return None;
                }
                let shortcut = egui::KeyboardShortcut::new(modifiers, key);
                input.consume_shortcut(&shortcut).then_some(command)
            })
    })
}

/// 단축키 → 명령 대응표.
///
/// **`Ctrl+C`·`Ctrl+X`·`Ctrl+V`는 이 표에 없다** — egui가 그 세 조합을 키 이벤트로 주지
/// 않기 때문이며, 빠진 것이 아니라 다른 길로 받는다(`poll_clipboard_keys`).
/// 수식 키가 많은 조합을 앞에 두어 `Ctrl+Shift+\`가 `Ctrl+\`로 오인되지 않게 한다.
///
/// **이 표가 유일한 대응이며 사용자가 바꿀 길을 두지 않는다** — 키 매핑 설정 화면도,
/// 그것을 담을 설정 항목도 만들지 않았다(요청에 없다 — plan T10 Design ④)
fn shortcut_table() -> [(egui::Modifiers, egui::Key, Command); 18] {
    let ctrl_shift = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
    let ctrl_alt = egui::Modifiers::CTRL | egui::Modifiers::ALT;
    [
        // 기존 두 단축키는 뜻을 그대로 잇는다 — 좌우 분할이 곧 오른쪽, 상하 분할이 곧 아래쪽이었다 (D4)
        (
            ctrl_shift,
            egui::Key::Backslash,
            Command::Split(SplitTo::Down),
        ),
        (ctrl_shift, egui::Key::W, Command::ClosePanel),
        // 탐색기와 같은 새 폴더 단축키 (FR-12) — `NewFolder`는 패널 메뉴에도 있다
        (ctrl_shift, egui::Key::N, Command::NewFolder),
        // **`Shift+Delete`가 `Delete`보다 앞이다** — 수식 키가 많은 조합을 먼저 본다는
        // 이 표의 규칙 그대로다. 뒤집히면 영구 삭제가 휴지통 이동으로 읽힌다
        (
            egui::Modifiers::SHIFT,
            egui::Key::Delete,
            Command::Delete { permanent: true },
        ),
        (
            egui::Modifiers::CTRL,
            egui::Key::Backslash,
            Command::Split(SplitTo::Right),
        ),
        (
            ctrl_alt,
            egui::Key::ArrowRight,
            Command::Split(SplitTo::Right),
        ),
        (
            ctrl_alt,
            egui::Key::ArrowLeft,
            Command::Split(SplitTo::Left),
        ),
        (ctrl_alt, egui::Key::ArrowUp, Command::Split(SplitTo::Up)),
        (
            ctrl_alt,
            egui::Key::ArrowDown,
            Command::Split(SplitTo::Down),
        ),
        (egui::Modifiers::CTRL, egui::Key::T, Command::NewTab),
        (egui::Modifiers::CTRL, egui::Key::W, Command::CloseTab),
        (egui::Modifiers::CTRL, egui::Key::B, Command::ToggleSidebar),
        (egui::Modifiers::ALT, egui::Key::ArrowLeft, Command::Back),
        (
            egui::Modifiers::ALT,
            egui::Key::ArrowRight,
            Command::Forward,
        ),
        (egui::Modifiers::ALT, egui::Key::ArrowUp, Command::Up),
        (egui::Modifiers::NONE, egui::Key::F5, Command::Refresh),
        (egui::Modifiers::NONE, egui::Key::F2, Command::Rename),
        (
            egui::Modifiers::NONE,
            egui::Key::Delete,
            Command::Delete { permanent: false },
        ),
    ]
}

/// 클립보드 키(`Ctrl+C`·`Ctrl+X`·`Ctrl+V`)를 명령으로 바꾼다 (FR-12).
///
/// **왜 `shortcut_table`이 아닌가**: `egui-winit`이 이 세 조합을 가로채 `Event::Cut`·
/// `Event::Copy`·`Event::Paste`로 바꾸고 **키 이벤트를 만들지 않는다**(`lib.rs`의
/// `is_cut_command`·`is_copy_command`·`is_paste_command` 뒤의 `return`). 그래서 표에 적어
/// 두어도 영영 발동하지 않는다 — 2026-08-22 사용자 보고("단축키 복사·잘라내기 동작하지 않음").
///
/// **붙여넣기만 창에서 직접 받는다**(`paste_pressed`): egui는 `Event::Paste`를 **글자가
/// 담겨 있을 때만** 만드는데, 탐색기가 파일을 복사한 클립보드에는 글자가 없어 아무 이벤트도
/// 오지 않는다. 그 하나는 창 서브클래스가 `WM_KEYDOWN`으로 본 값을 받아 온다
/// (`ui::shell_host::take_paste_pressed`).
///
/// 키 소유 판정은 `poll_shortcuts`와 같다 — 셋 다 고른 항목이 대상이라 파일 목록이 키를
/// 쥔 때만 나간다
pub fn poll_clipboard_keys(
    ctx: &egui::Context,
    owner: KeyOwner,
    paste_pressed: bool,
) -> Option<Command> {
    if owner != KeyOwner::FileList || ctx.egui_wants_keyboard_input() {
        return None;
    }
    let 이벤트 = ctx.input(|input| {
        input.events.iter().find_map(|event| match event {
            egui::Event::Cut => Some(Command::ClipboardCut),
            egui::Event::Copy => Some(Command::ClipboardCopy),
            egui::Event::Paste(_) => Some(Command::ClipboardPaste),
            _ => None,
        })
    });
    이벤트.or(paste_pressed.then_some(Command::ClipboardPaste))
}

/// 마우스 옆의 뒤로·앞으로 버튼을 명령으로 바꾼다.
///
/// **왜 `shortcut_table`이 아닌가**: 그 표는 키 조합만 담는다. 이 둘은 키가 아니라 포인터
/// 입력이며, `WM_XBUTTONDOWN`의 XBUTTON1·XBUTTON2가 winit의 `MouseButton::Back`·`Forward`를
/// 거쳐 `PointerButton::Extra1`·`Extra2`로 들어온다 — `consume_shortcut` 경로에 실리지 않는다.
///
/// **길이 둘이다**: 옆 버튼을 `WM_XBUTTONDOWN`으로 보내는 마우스는 egui까지 포인터 입력으로
/// 오지만, **`WM_APPCOMMAND`(브라우저 뒤로·앞으로)로 보내는 마우스**는 egui에 아무것도 남기지
/// 않아 창이 직접 받아 온다(`app_nav` — `ui::shell_host::take_app_nav`). 2026-09-03에 이 PC의
/// 마우스가 뒤엣것임이 실측돼 두 길을 함께 둔다. **둘이 겹쳐 두 번 나가지 않는다** — winit이
/// `WM_XBUTTONDOWN`을 처리하고 끝내므로 그 마우스에서는 `WM_APPCOMMAND`가 생기지 않는다.
///
/// **키 소유(`KeyOwner`)도 포커스도 보지 않는다**: 뒤로·앞으로는 고른 항목이 아니라 보고 있는
/// 위치에 하는 일이라 어느 영역이 키를 쥐었는지와 무관하고, 주소창에 글자를 넣던 중에 눌러도
/// 탐색기와 같이 그대로 듣는다. 대상은 활성 패널이며 `Alt+←`·`Alt+→`와 같은 길로 나간다
pub fn poll_mouse_nav(ctx: &egui::Context, app_nav: Option<AppNav>) -> Option<Command> {
    let 포인터 = ctx.input(|input| {
        if input.pointer.button_pressed(egui::PointerButton::Extra1) {
            Some(Command::Back)
        } else if input.pointer.button_pressed(egui::PointerButton::Extra2) {
            Some(Command::Forward)
        } else {
            None
        }
    });
    포인터.or(app_nav.map(|nav| match nav {
        AppNav::Back => Command::Back,
        AppNav::Forward => Command::Forward,
    }))
}

/// 팝업이 화면 밖으로 나가지 않게 시작점을 안으로 당긴다 (quality 리뷰 m1).
///
/// 화면보다 큰 팝업이면 왼쪽·위쪽 모서리를 우선한다 — 아래가 잘려도 첫 줄은 보인다.
///
/// **패널과 트리가 함께 쓴다** — 패널의 원격 목록 메뉴와 트리의 즐겨찾기 메뉴가 같은 보정을
/// 받아야 해서, 어느 한쪽에 두지 않고 명령만 값으로 돌려주는 이 모듈에 둔다 (plan D6)
pub(crate) fn clamp_menu_pos(screen: egui::Rect, at: egui::Pos2, size: egui::Vec2) -> egui::Pos2 {
    egui::pos2(
        at.x.min(screen.right() - size.x).max(screen.left()),
        at.y.min(screen.bottom() - size.y).max(screen.top()),
    )
}

/// 팝업 **프레임**이 안쪽 내용 밖에 더 차지하는 크기 — 화면 밖 보정에 더한다.
///
/// 안쪽 여백은 `Frame::menu`가 스타일에서 읽어 가는 그대로(`spacing.menu_margin`)를 읽는다.
///
/// **테두리 두께는 스타일이 아니라 `theme::MENU_FRAME_STROKE`다** — 세 메뉴가 `Frame::menu`의
/// 테두리를 각자 그 값으로 덮어쓰기 때문이다(`.stroke(...)`). 스타일 쪽을 읽으면 지금은
/// 우연히 같은 1px이라 맞지만, 한쪽만 바뀌는 날 조용히 어긋난다.
///
/// **그림자는 세지 않는다** — 그것은 프레임 **밖에** 번지는 그리기라 자리를 차지하지 않고,
/// 화면 끝에서 잘려도 메뉴 내용이 가려지지 않는다.
///
/// 종전에는 두 메뉴가 각자 `8.0`이라는 어림값을 적고 있었다(`remote_menu::FRAME_PAD`·
/// `tree::MENU_FRAME_PAD`). 값이 실제와 어긋나면 메뉴가 화면 끝에서 잘리거나 쓸데없이
/// 안으로 당겨진다
pub(crate) fn menu_frame_pad(style: &egui::Style) -> egui::Vec2 {
    let margin = style.spacing.menu_margin;
    let stroke = theme::MENU_FRAME_STROKE * 2.0;
    egui::vec2(
        (margin.left + margin.right) as f32 + stroke,
        (margin.top + margin.bottom) as f32 + stroke,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 큐 열 메뉴를 한 프레임 그리고, 그려진 줄의 글자를 차례대로 모은다
    fn 큐_열_메뉴_줄들(
        kinds: &[QueueColumnKind],
        hidden: &[QueueColumnKind],
    ) -> (Vec<String>, egui::Context) {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(Default::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let mut out = None;
                queue_column_menu_items(ui, kinds, hidden, &mut out);
            });
        });
        let mut 줄 = Vec::new();
        for clipped in &output.shapes {
            if let egui::Shape::Text(text) = &clipped.shape {
                줄.push(text.galley.text().to_owned());
            }
        }
        (줄, ctx)
    }

    #[test]
    fn 끈_열도_메뉴에_남아_다시_켤_수_있다() {
        // **2026-09-07 완료 리뷰가 잡은 결함** — 메뉴에 `visible`(보이는 열)을 넘기고 있어
        // 끈 열이 목록에서 사라졌고, 그 상태가 세션에 저장되므로 **재시작해도 되살릴 길이
        // 없었다**. 메뉴는 언제나 **그 탭의 전체 후보**를 받아야 한다
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        let kinds = crate::ui::queue_panel::columns_for(crate::remote::queue::QueueFilter::All);
        let (켜진_채, _ctx) = 큐_열_메뉴_줄들(kinds, &[]);
        assert!(
            켜진_채.iter().any(|줄| 줄.contains("서버")),
            "끄기 전에는 서버 줄이 있어야 한다 — 이 시험이 아무것도 보지 않는다"
        );

        let (끈_뒤, _ctx) = 큐_열_메뉴_줄들(kinds, &[QueueColumnKind::Server]);
        assert!(
            끈_뒤.iter().any(|줄| 줄.contains("서버")),
            "끈 열이 메뉴에서 사라져 다시 켤 수 없다"
        );
        assert_eq!(
            끈_뒤.len(),
            켜진_채.len(),
            "끄고 켜도 메뉴 줄 수는 그대로다"
        );
    }

    #[test]
    fn 큐_열_메뉴는_켜진_열에만_체크를_붙인다() {
        // 체크 글리프가 켜짐/꺼짐을 가르는 유일한 표시다 — 그것이 안 갈리면
        // 사용자가 지금 무엇이 켜져 있는지 알 수 없다
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        let kinds = crate::ui::queue_panel::columns_for(crate::remote::queue::QueueFilter::All);
        let 체크수 = |hidden: &[QueueColumnKind]| {
            let (줄, _ctx) = 큐_열_메뉴_줄들(kinds, hidden);
            줄.iter()
                .filter(|줄| 줄.contains(egui_phosphor::regular::CHECK))
                .count()
        };
        let 전부켜짐 = 체크수(&[]);
        assert_eq!(전부켜짐, kinds.len(), "켜진 열마다 체크가 있어야 한다");
        assert_eq!(
            체크수(&[QueueColumnKind::Server]),
            전부켜짐 - 1,
            "끈 열의 체크가 빠지지 않았다"
        );
    }

    #[test]
    fn 여백은_스타일에서_테두리는_앱_상수에서_읽는다() {
        // 안쪽 여백은 `Frame::menu`가 스타일에서 읽어 가는 값이고, 테두리는 세 메뉴가
        // 스타일을 덮어쓰며 쓰는 값이다 — 어느 한쪽이라도 어긋나면 메뉴가 화면 끝에서 잘린다
        let mut style = egui::Style::default();
        style.spacing.menu_margin = egui::Margin::same(7);
        // 스타일 쪽 테두리는 **읽지 않는다** — 일부러 다른 값을 넣어 그것을 확인한다
        style.visuals.window_stroke = egui::Stroke::new(9.0, egui::Color32::WHITE);
        let 기대 = 7.0 * 2.0 + theme::MENU_FRAME_STROKE * 2.0;
        let pad = menu_frame_pad(&style);
        assert_eq!(pad.x, 기대, "좌우 여백 + 앱이 그리는 테두리 양쪽");
        assert_eq!(pad.y, 기대, "위아래도 같은 규칙");
    }

    #[test]
    fn 좌우와_위아래_여백이_다르면_따로_센다() {
        // `Margin`은 네 변을 각각 가질 수 있다 — 한쪽만 보면 폭이나 높이가 어긋난다
        let mut style = egui::Style::default();
        style.spacing.menu_margin = egui::Margin {
            left: 2,
            right: 3,
            top: 10,
            bottom: 20,
        };
        let 테두리 = theme::MENU_FRAME_STROKE * 2.0;
        let pad = menu_frame_pad(&style);
        assert_eq!(pad.x, 5.0 + 테두리);
        assert_eq!(pad.y, 30.0 + 테두리);
    }

    #[test]
    fn 그림자는_자리로_세지_않는다() {
        // 그림자는 프레임 **밖에** 번지는 그리기라 자리를 차지하지 않는다 —
        // 세면 메뉴가 쓸데없이 화면 안쪽으로 당겨진다
        let mut style = egui::Style::default();
        style.spacing.menu_margin = egui::Margin::ZERO;
        style.visuals.popup_shadow = egui::epaint::Shadow {
            offset: [0, 0],
            blur: 40,
            spread: 40,
            color: egui::Color32::BLACK,
        };
        let 테두리만 = theme::MENU_FRAME_STROKE * 2.0;
        assert_eq!(menu_frame_pad(&style), egui::vec2(테두리만, 테두리만));
    }

    #[test]
    fn 네_방향은_축과_배치로_정확히_갈린다() {
        // 이 매핑이 틀리면 "왼쪽 분할"이 오른쪽에 패널을 만드는 식으로 조용히 어긋난다
        assert_eq!(
            SplitTo::Right.to_layout(),
            (SplitDir::Horizontal, SplitPlace::After)
        );
        assert_eq!(
            SplitTo::Left.to_layout(),
            (SplitDir::Horizontal, SplitPlace::Before)
        );
        assert_eq!(
            SplitTo::Up.to_layout(),
            (SplitDir::Vertical, SplitPlace::Before)
        );
        assert_eq!(
            SplitTo::Down.to_layout(),
            (SplitDir::Vertical, SplitPlace::After)
        );
    }

    #[test]
    fn 기존_분할_단축키는_뜻을_그대로_잇는다() {
        // Ctrl+\(좌우 분할) = 오른쪽, Ctrl+Shift+\(상하 분할) = 아래쪽 — 익힌 동작이 바뀌면 안 된다 (D4)
        let table = shortcut_table();
        let find = |modifiers, key| {
            table
                .iter()
                .find(|(m, k, _)| *m == modifiers && *k == key)
                .map(|(_, _, command)| *command)
        };
        assert_eq!(
            find(egui::Modifiers::CTRL, egui::Key::Backslash),
            Some(Command::Split(SplitTo::Right))
        );
        assert_eq!(
            find(
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                egui::Key::Backslash
            ),
            Some(Command::Split(SplitTo::Down))
        );
    }

    #[test]
    fn 네_방향_단축키가_모두_배정돼_있다() {
        let table = shortcut_table();
        let ctrl_alt = egui::Modifiers::CTRL | egui::Modifiers::ALT;
        let find = |key| {
            table
                .iter()
                .find(|(m, k, _)| *m == ctrl_alt && *k == key)
                .map(|(_, _, command)| *command)
        };
        assert_eq!(
            find(egui::Key::ArrowRight),
            Some(Command::Split(SplitTo::Right))
        );
        assert_eq!(
            find(egui::Key::ArrowLeft),
            Some(Command::Split(SplitTo::Left))
        );
        assert_eq!(find(egui::Key::ArrowUp), Some(Command::Split(SplitTo::Up)));
        assert_eq!(
            find(egui::Key::ArrowDown),
            Some(Command::Split(SplitTo::Down))
        );
    }

    #[test]
    fn 같은_키_조합이_두_명령에_겹치지_않는다() {
        // 겹치면 표에서 앞선 것만 동작하고 뒤엣것은 조용히 죽는다
        let table = shortcut_table();
        for (index, (modifiers, key, _)) in table.iter().enumerate() {
            for (other_modifiers, other_key, command) in &table[index + 1..] {
                assert!(
                    !(modifiers == other_modifiers && key == other_key),
                    "{command:?}의 단축키가 앞선 항목과 겹친다"
                );
            }
        }
    }

    #[test]
    fn 키_소유는_마지막으로_누른_영역이_갖는다() {
        use KeyOwner::{FileList, Sidebar};
        // 앱이 막 떴을 때는 파일 목록이다 — 아무것도 누르지 않았다
        assert_eq!(KeyOwner::default(), FileList);
        // 누른 자리로 옮겨 간다
        assert_eq!(next_key_owner(FileList, Some(Sidebar)), Sidebar);
        assert_eq!(next_key_owner(Sidebar, Some(FileList)), FileList);
        // 누르지 않은 프레임에는 그대로다 — 마우스가 지나가기만 해서는 바뀌지 않는다
        assert_eq!(next_key_owner(Sidebar, None), Sidebar);
        assert_eq!(next_key_owner(FileList, None), FileList);
    }

    #[test]
    fn 파일_대상_명령만_키_소유를_가린다() {
        // 고른 항목을 건드리는 명령만 파일 목록이 키를 쥔 때 나간다 (FR-12) —
        // 사이드바를 누른 뒤 `Delete`를 눌렀는데 파일이 지워지면 안 된다.
        // 나머지(탐색·분할·탭·보기)는 어느 쪽을 눌렀든 활성 패널로 간다
        let 파일_대상: Vec<Command> = shortcut_table()
            .into_iter()
            .map(|(_, _, command)| command)
            .filter(|command| targets_file_list(*command))
            .collect();
        assert_eq!(
            파일_대상,
            vec![
                Command::Delete { permanent: true },
                Command::Rename,
                Command::Delete { permanent: false },
            ]
        );
        // 클립보드 셋은 **표에 없다**(egui가 키 이벤트를 주지 않는다 — `poll_clipboard_keys`).
        // 그래도 파일 대상 분류는 같아야 한다 — 그 판정으로 키 소유를 가리기 때문이다
        for command in [
            Command::ClipboardCopy,
            Command::ClipboardCut,
            Command::ClipboardPaste,
        ] {
            assert!(targets_file_list(command), "{command:?}");
        }
        // 새 폴더는 **고른 것이 아니라 보고 있는 폴더**가 대상이라 여기 없다
        assert!(!targets_file_list(Command::NewFolder));
    }

    #[test]
    fn fr12_요청한_단축키_열하나가_모두_있다() {
        // 사용자가 적어 준 목록 그대로다 (FR-12 — 2026-08-22 요청)
        let table = shortcut_table();
        let 찾기 = |modifiers, key| {
            table
                .iter()
                .find(|(m, k, _)| *m == modifiers && *k == key)
                .map(|(_, _, command)| *command)
        };
        let ctrl = egui::Modifiers::CTRL;
        let ctrl_shift = egui::Modifiers::CTRL | egui::Modifiers::SHIFT;
        let none = egui::Modifiers::NONE;
        assert_eq!(찾기(ctrl_shift, egui::Key::N), Some(Command::NewFolder));
        assert_eq!(
            찾기(egui::Modifiers::ALT, egui::Key::ArrowLeft),
            Some(Command::Back)
        );
        assert_eq!(
            찾기(egui::Modifiers::ALT, egui::Key::ArrowRight),
            Some(Command::Forward)
        );
        assert_eq!(
            찾기(egui::Modifiers::ALT, egui::Key::ArrowUp),
            Some(Command::Up)
        );
        assert_eq!(찾기(none, egui::Key::F5), Some(Command::Refresh));
        assert_eq!(찾기(none, egui::Key::F2), Some(Command::Rename));
        assert_eq!(
            찾기(none, egui::Key::Delete),
            Some(Command::Delete { permanent: false })
        );
        assert_eq!(
            찾기(egui::Modifiers::SHIFT, egui::Key::Delete),
            Some(Command::Delete { permanent: true })
        );
        // **클립보드 셋은 이 표에 없다** — `egui-winit`이 `Ctrl+C/X/V`를 가로채
        // `Event::Cut`·`Copy`·`Paste`로 바꾸고 키 이벤트를 만들지 않기 때문이다.
        // 표에 적어 두면 영영 발동하지 않으므로 `poll_clipboard_keys`가 따로 받는다
        for key in [egui::Key::C, egui::Key::X, egui::Key::V] {
            assert_eq!(찾기(ctrl, key), None, "{key:?}는 표에 있으면 안 된다");
        }
    }

    #[test]
    fn 클립보드_키는_egui_이벤트로_받는다() {
        // 2026-08-22 사용자 보고 — "단축키 복사·잘라내기 동작하지 않음".
        // 원인은 `egui-winit`이 그 조합을 키 이벤트가 아니라 `Event::Cut`·`Copy`로
        // 바꾸는 것이었다(`lib.rs`의 `is_cut_command` 뒤 `return`)
        let 눌러_보기 = |event: egui::Event, owner: KeyOwner| {
            let ctx = egui::Context::default();
            let mut input = egui::RawInput::default();
            input.events.push(event);
            let mut got = None;
            let _ = ctx.run_ui(input, |ui| {
                got = poll_clipboard_keys(ui.ctx(), owner, false);
            });
            got
        };
        assert_eq!(
            눌러_보기(egui::Event::Copy, KeyOwner::FileList),
            Some(Command::ClipboardCopy)
        );
        assert_eq!(
            눌러_보기(egui::Event::Cut, KeyOwner::FileList),
            Some(Command::ClipboardCut)
        );
        assert_eq!(
            눌러_보기(egui::Event::Paste("글자".to_owned()), KeyOwner::FileList),
            Some(Command::ClipboardPaste)
        );
        // 사이드바가 키를 쥐고 있으면 나가지 않는다 — 셋 다 고른 항목이 대상이다
        assert_eq!(눌러_보기(egui::Event::Copy, KeyOwner::Sidebar), None);
    }

    #[test]
    fn 붙여넣기는_창이_본_것도_받는다() {
        // 탐색기가 파일을 복사한 클립보드에는 **글자가 없어** egui가 `Event::Paste`를
        // 만들지 않는다 — 그 하나는 창 서브클래스가 본 `Ctrl+V`를 받아 온다
        let ctx = egui::Context::default();
        let mut got = None;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            got = poll_clipboard_keys(ui.ctx(), KeyOwner::FileList, true);
        });
        assert_eq!(got, Some(Command::ClipboardPaste));

        // 눌리지 않았으면 아무 일도 없다
        let ctx = egui::Context::default();
        let mut got = None;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            got = poll_clipboard_keys(ui.ctx(), KeyOwner::FileList, false);
        });
        assert_eq!(got, None);
    }

    #[test]
    fn 마우스_옆_버튼이_뒤로_앞으로가_된다() {
        // `WM_XBUTTONDOWN`의 XBUTTON1·XBUTTON2가 winit·egui를 거쳐 `Extra1`·`Extra2`로 온다 —
        // 탐색기에서 익힌 대로 앞의 것이 뒤로, 뒤의 것이 앞으로다
        let 눌러_보기 = |button: Option<egui::PointerButton>| {
            let ctx = egui::Context::default();
            let mut input = egui::RawInput::default();
            if let Some(button) = button {
                input.events.push(egui::Event::PointerButton {
                    pos: egui::pos2(10.0, 10.0),
                    button,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                });
            }
            let mut got = None;
            let _ = ctx.run_ui(input, |ui| {
                got = poll_mouse_nav(ui.ctx(), None);
            });
            got
        };
        assert_eq!(
            눌러_보기(Some(egui::PointerButton::Extra1)),
            Some(Command::Back)
        );
        assert_eq!(
            눌러_보기(Some(egui::PointerButton::Extra2)),
            Some(Command::Forward)
        );
        // 다른 버튼·아무 것도 안 누른 프레임은 아무 명령도 내지 않는다 —
        // 왼쪽 클릭이 탐색을 일으키면 안 된다
        assert_eq!(눌러_보기(Some(egui::PointerButton::Primary)), None);
        assert_eq!(눌러_보기(Some(egui::PointerButton::Secondary)), None);
        assert_eq!(눌러_보기(None), None);
    }

    #[test]
    fn 창이_받은_옆_버튼도_같은_명령이_된다() {
        // 2026-09-03 실측 — 이 PC의 마우스는 옆 버튼을 `WM_XBUTTONDOWN`이 아니라
        // `WM_APPCOMMAND`(브라우저 뒤로·앞으로)로 보내 egui에는 아무것도 남지 않았다.
        // 그래서 창이 직접 받아 온 것도 같은 명령으로 이어져야 한다
        let 창이_본_것 = |nav: Option<AppNav>| {
            let ctx = egui::Context::default();
            let mut got = None;
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                got = poll_mouse_nav(ui.ctx(), nav);
            });
            got
        };
        assert_eq!(창이_본_것(Some(AppNav::Back)), Some(Command::Back));
        assert_eq!(창이_본_것(Some(AppNav::Forward)), Some(Command::Forward));
        assert_eq!(창이_본_것(None), None);
    }

    #[test]
    fn shift_delete가_delete보다_앞선다() {
        // 수식 키가 많은 조합을 먼저 본다는 이 표의 규칙 그대로다 — 뒤집히면
        // 영구 삭제가 휴지통 이동으로 읽힌다
        let table = shortcut_table();
        let 자리 = |modifiers, key| {
            table
                .iter()
                .position(|(m, k, _)| *m == modifiers && *k == key)
                .expect("표에 있어야 한다")
        };
        assert!(
            자리(egui::Modifiers::SHIFT, egui::Key::Delete)
                < 자리(egui::Modifiers::NONE, egui::Key::Delete)
        );
        // 새 폴더도 같은 규칙 — `Ctrl+Shift+N`이 있고 `Ctrl+N`은 두지 않았다
        assert!(
            !table
                .iter()
                .any(|(m, k, _)| *m == egui::Modifiers::CTRL && *k == egui::Key::N)
        );
    }

    #[test]
    fn fr12_기본_단축키가_모두_들어_있다() {
        // 네 방향 Ctrl+Alt 조합은 `네_방향_단축키가_모두_배정돼_있다`가 따로 덮는다
        let table = shortcut_table();
        let has = |modifiers, key| table.iter().any(|(m, k, _)| *m == modifiers && *k == key);
        assert!(has(egui::Modifiers::CTRL, egui::Key::T)); // 새 탭
        assert!(has(egui::Modifiers::CTRL, egui::Key::W)); // 탭 닫기
        assert!(has(egui::Modifiers::ALT, egui::Key::ArrowLeft)); // 뒤로
        assert!(has(egui::Modifiers::ALT, egui::Key::ArrowRight)); // 앞으로
        assert!(has(egui::Modifiers::NONE, egui::Key::F5)); // 새로 고침
        assert!(has(egui::Modifiers::CTRL, egui::Key::Backslash)); // 오른쪽 분할
        assert!(has(
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            egui::Key::Backslash
        )); // 아래쪽 분할
    }

    /// 주어진 그리기를 한 번 실행하고 **그려진 텍스트를 순서대로** 모은다.
    /// 구분선은 글자가 없어 잡히지 않으므로 항목 문구만 남는다
    fn drawn_labels(draw: impl FnMut(&mut egui::Ui)) -> Vec<String> {
        fn collect(shape: &egui::Shape, found: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => found.push(text.galley.text().to_owned()),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        collect(shape, found);
                    }
                }
                _ => {}
            }
        }
        let ctx = egui::Context::default();
        crate::ui::app::install_fonts(&ctx, None);
        let output = ctx.run_ui(Default::default(), draw);
        let mut labels = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut labels);
        }
        labels
    }

    /// 패널 메뉴 본문을 그려 라벨을 모은다
    fn menu_labels(state: PanelMenuState) -> Vec<String> {
        drawn_labels(|ui| {
            let mut command = None;
            panel_menu_items(ui, state, &mut command);
        })
    }

    /// '보기' 하위 메뉴를 그려 라벨을 모은다 — 호버로 열리는 팝업이라
    /// 패널 메뉴를 그리는 것만으로는 잡히지 않아 직접 부른다
    fn view_labels(current: ViewMode) -> Vec<String> {
        drawn_labels(|ui| {
            let mut command = None;
            view_items(ui, current, &mut command);
        })
    }

    #[test]
    fn 패널_메뉴는_요청한_순서와_문구를_그린다() {
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        // plan `## 시각 요소 분해`의 인벤토리 표 13행 중 글자가 있는 항목들.
        // 메뉴 바를 없앤 뒤 이 메뉴가 유일한 마우스 진입점이라, 항목이 빠지면 그 기능에
        // 마우스로 닿을 수 없게 된다
        let labels = menu_labels(PanelMenuState::for_panes(2, ViewMode::Details));
        let expected = [
            crate::i18n::menu_view(),
            crate::i18n::menu_split_right(),
            crate::i18n::menu_split_left(),
            crate::i18n::menu_split_up(),
            crate::i18n::menu_split_down(),
            crate::i18n::menu_refresh(),
            crate::i18n::menu_new_file(),
            crate::i18n::menu_new_folder(),
            crate::i18n::close(),
        ];
        let found: Vec<&String> = labels
            .iter()
            .filter(|label| expected.contains(&label.as_str()))
            .collect();
        assert_eq!(
            found,
            expected.iter().collect::<Vec<_>>(),
            "메뉴 항목의 문구나 순서가 인벤토리와 다르다: {labels:?}"
        );
    }

    #[test]
    fn 보기_하위_메뉴는_여덟_모드를_순서대로_그린다() {
        // plan `### 참조 정합 인벤토리 — '보기' 하위 메뉴` 8행 그대로여야 한다
        let labels = view_labels(ViewMode::Details);
        let expected: Vec<String> = ViewMode::ALL
            .iter()
            .map(|mode| mode.label().to_owned())
            .collect();
        let found: Vec<String> = labels
            .iter()
            .map(|label| {
                // 표시 점은 아이콘 글꼴의 것이다 (프로젝트 규약) — 문구만 남겨 견준다
                label
                    .trim_start_matches(egui_phosphor::regular::DOT_OUTLINE)
                    .trim_start()
                    .to_owned()
            })
            .filter(|label| expected.contains(label))
            .collect();
        assert_eq!(
            found, expected,
            "보기 항목의 문구나 순서가 다르다: {labels:?}"
        );
    }

    #[test]
    fn 지금_쓰는_모드에만_점이_붙는다() {
        // 점이 없거나 여러 개면 어느 모드로 보고 있는지 알 수 없다 (4번 이미지의 표시 방식)
        for current in [ViewMode::Details, ViewMode::Tiles, ViewMode::List] {
            let marked: Vec<String> = view_labels(current)
                .into_iter()
                .filter(|label| label.starts_with(egui_phosphor::regular::DOT_OUTLINE))
                .collect();
            assert_eq!(
                marked.len(),
                1,
                "{current:?}: 점이 하나가 아니다 — {marked:?}"
            );
            assert!(
                marked[0].contains(current.label()),
                "{current:?}: 점이 엉뚱한 항목에 붙었다 — {marked:?}"
            );
        }
    }

    #[test]
    fn 마지막_패널_하나는_닫을_수_없다() {
        // FR-2 — 이 조건이 뒤집히면 마지막 패널을 닫아 빈 화면이 된다
        let mode = ViewMode::Details;
        assert!(!PanelMenuState::for_panes(1, mode).can_close_panel);
        assert!(PanelMenuState::for_panes(2, mode).can_close_panel);
        assert!(PanelMenuState::for_panes(4, mode).can_close_panel);
        // 패널이 0개인 상태는 정상 흐름에 없지만, 그때도 닫기를 열어주면 안 된다
        assert!(!PanelMenuState::for_panes(0, mode).can_close_panel);
    }

    #[test]
    fn 메뉴_바가_없어도_단축키는_모두_살아_있다() {
        // 메뉴 바를 지우면서 잃은 것이 없어야 한다 — 이동·탭 명령은 이제 단축키와
        // 주소창·탭 스트립 버튼으로만 닿는다
        let table = shortcut_table();
        for command in [
            Command::NewTab,
            Command::CloseTab,
            Command::Back,
            Command::Forward,
            Command::Up,
            Command::Refresh,
            Command::ClosePanel,
            Command::ToggleSidebar,
        ] {
            assert!(
                table.iter().any(|(_, _, c)| *c == command),
                "{command:?}의 단축키가 사라졌다"
            );
        }
    }

    #[test]
    fn 무수식_키는_셋뿐이다() {
        // `F5`·`F2`·`Delete` 셋만 수식 키 없이 받는다 (FR-12). 글자 키를 여기 더하면
        // 이름 편집 중 텍스트 입력을 가로챈다 — `F2`·`Delete`는 글자가 아니고,
        // 편집 중에는 `egui_wants_keyboard_input`이 먼저 걸러 낸다
        let 무수식: Vec<egui::Key> = shortcut_table()
            .into_iter()
            .filter(|(modifiers, ..)| *modifiers == egui::Modifiers::NONE)
            .map(|(_, key, _)| key)
            .collect();
        assert_eq!(
            무수식,
            vec![egui::Key::F5, egui::Key::F2, egui::Key::Delete]
        );
    }

    #[test]
    fn 가장자리에서_연_메뉴는_화면_안으로_당겨진다() {
        // quality 리뷰 m1 — 셸 메뉴는 OS가 보정해 주지만(D21) 우리가 그리는 메뉴는 아니다
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 800.0));
        let size = egui::vec2(200.0, 240.0);
        // 안쪽에서 열면 그 자리 그대로다
        assert_eq!(
            clamp_menu_pos(screen, egui::pos2(100.0, 100.0), size),
            egui::pos2(100.0, 100.0)
        );
        // 오른쪽·아래 가장자리에서 열면 안으로 당긴다
        assert_eq!(
            clamp_menu_pos(screen, egui::pos2(1150.0, 780.0), size),
            egui::pos2(1000.0, 560.0)
        );
        // 화면보다 큰 메뉴는 왼쪽 위를 맞춘다 — 아래가 잘려도 첫 줄은 보인다
        let huge = egui::vec2(2000.0, 2000.0);
        assert_eq!(
            clamp_menu_pos(screen, egui::pos2(600.0, 400.0), huge),
            egui::pos2(0.0, 0.0)
        );
    }
}
