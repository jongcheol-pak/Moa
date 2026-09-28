//! 자세히 보기 — 열 머리글·열 폭 조절·가로 스크롤·행 렌더 (FR-4).
//!
//! `ui::file_list`가 목록 상태(항목·선택·정렬)를 소유하고, 이 모듈은 **자세히 보기의 그리기와
//! 열 조작만** 담당한다. 상태를 바꾸지 않고 이번 프레임의 조작을 `DetailsOutcome`으로 돌려준다 —
//! 그리기 루프가 목록을 빌린 채로는 선택·정렬을 고칠 수 없기 때문이다.
use crate::fs::icons::IconCache;
use crate::panel::file_list::{ListRow, SortKey, format_filetime, format_size};
use crate::ui::icon_tex::IconTextures;
use crate::ui::list_common::{
    FileListAction, RenameEdit, RenameEnd, cut_icon_tint, cut_text_color, dim_if_hidden,
    elided_galley_colored, rename_editor,
};
use crate::ui::theme;
use eframe::egui;
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// 행 높이 — 16px 시스템 아이콘이 들어갈 여유를 둔다
pub const ROW_HEIGHT: f32 = 20.0;
/// 헤더 높이
pub const HEADER_HEIGHT: f32 = 22.0;
/// 아이콘 크기·좌측 여백
const ICON_SIZE: f32 = 16.0;
const ICON_X: f32 = 4.0;
/// 이름 텍스트 시작 x (아이콘 뒤)
const NAME_X: f32 = 24.0;
/// 셀 사이 여백
const CELL_PAD: f32 = 6.0;

/// 이름 편집 입력칸이 행 위아래로 물러나는 여백 (FR-64) — 행 높이가 20px이라
/// 입력칸 테두리가 위아래 행에 닿지 않게 조금 줄인다
const EDIT_INSET_Y: f32 = 1.0;
/// 열 최소 폭 — 머리글이 잘려도 드래그 핸들은 잡히는 하한 (plan 시각 속성 표)
pub const MIN_COL_WIDTH: f32 = 40.0;
/// 열 경계 드래그 핸들 폭 — 경계 중심에서 좌우로 절반씩
const HANDLE_WIDTH: f32 = 6.0;

/// 자세히 보기의 열 종류 (FR-31).
///
/// 앞 넷은 로컬·원격 어디서나 보이고, **권한·소유자는 원격 패널에서만** 켤 수 있다 —
/// 로컬 파일의 권한은 ACL이고 소유자도 SID라, 같은 열에 담으면 두 체계가 한 칸에 섞인다
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColumnKind {
    Name,
    Size,
    Type,
    Modified,
    Permissions,
    Owner,
}

impl ColumnKind {
    /// 열 머리글과 열 메뉴에 쓰는 이름 (인벤토리 #23~28 원문 그대로)
    pub fn label(self) -> &'static str {
        match self {
            ColumnKind::Name => crate::i18n::column_name(),
            ColumnKind::Size => crate::i18n::column_size(),
            ColumnKind::Type => crate::i18n::column_type(),
            ColumnKind::Modified => crate::i18n::column_modified(),
            ColumnKind::Permissions => crate::i18n::column_permissions(),
            ColumnKind::Owner => crate::i18n::column_owner(),
        }
    }

    /// 폭 배열에서의 자리
    fn slot(self) -> usize {
        match self {
            ColumnKind::Name => 0,
            ColumnKind::Size => 1,
            ColumnKind::Type => 2,
            ColumnKind::Modified => 3,
            ColumnKind::Permissions => 4,
            ColumnKind::Owner => 5,
        }
    }

    /// 머리글을 눌러 정렬할 수 있는 열인가.
    ///
    /// **권한·소유자는 정렬 대상이 아니다** — 디자인이 이 두 열에 정렬을 주지 않았고,
    /// `SortKey`를 늘리면 로컬 목록의 정렬 저장값까지 함께 바뀐다(이번 범위 밖)
    fn sort_key(self) -> Option<SortKey> {
        match self {
            ColumnKind::Name => Some(SortKey::Name),
            ColumnKind::Size => Some(SortKey::Size),
            ColumnKind::Type => Some(SortKey::Type),
            ColumnKind::Modified => Some(SortKey::Modified),
            ColumnKind::Permissions | ColumnKind::Owner => None,
        }
    }

    /// 끌 수 없는 열인가 — 앞 넷은 열 메뉴에서 항상 체크된 채 비활성이다 (인벤토리 #23~26)
    pub fn is_fixed(self) -> bool {
        !matches!(self, ColumnKind::Permissions | ColumnKind::Owner)
    }

    /// 세션에 담는 키 (FR-4) — `SortKey::as_key`·`ViewMode::as_key`와 같은 방식이다.
    ///
    /// **숫자가 아니라 문자열인 이유**: 열이 늘거나 `ALL_COLUMNS`의 차례가 바뀌면 숫자는
    /// 값이 밀려 옛 파일이 엉뚱한 열을 가리킨다
    pub fn as_key(self) -> &'static str {
        match self {
            ColumnKind::Name => "name",
            ColumnKind::Size => "size",
            ColumnKind::Type => "type",
            ColumnKind::Modified => "modified",
            ColumnKind::Permissions => "permissions",
            ColumnKind::Owner => "owner",
        }
    }

    /// 저장 키를 되읽는다 — 모르는 키는 `None`이라 그 자리를 버린다
    fn from_key(key: &str) -> Option<ColumnKind> {
        ALL_COLUMNS.into_iter().find(|kind| kind.as_key() == key)
    }
}

/// 열 메뉴에 보이는 순서 (원본 `FileExplorer-FTP.dc.html:890-897`)
pub const ALL_COLUMNS: [ColumnKind; 6] = [
    ColumnKind::Name,
    ColumnKind::Size,
    ColumnKind::Type,
    ColumnKind::Modified,
    ColumnKind::Permissions,
    ColumnKind::Owner,
];

/// 켤 수 있는 열의 표시 여부 — 기본은 둘 다 꺼짐이다 (인벤토리 #27·#28)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ColumnFlags {
    pub permissions: bool,
    pub owner: bool,
}

impl ColumnFlags {
    /// 그 열이 켜져 있는가. 고정 열은 늘 켜져 있다
    pub fn shows(&self, kind: ColumnKind) -> bool {
        match kind {
            ColumnKind::Permissions => self.permissions,
            ColumnKind::Owner => self.owner,
            _ => true,
        }
    }

    /// 켤 수 있는 열을 뒤집는다. 고정 열에는 아무 일도 하지 않는다
    pub fn toggle(&mut self, kind: ColumnKind) {
        match kind {
            ColumnKind::Permissions => self.permissions = !self.permissions,
            ColumnKind::Owner => self.owner = !self.owner,
            _ => {}
        }
    }
}

/// 자세히 보기의 열 폭 — **여섯 열 몫을 늘 들고**, 그중 보이는 것만 그린다.
///
/// **모든 열이 고정 폭을 갖는다.** 종전에는 마지막 열이 "남는 폭 전부"였는데, 그러면 콘텐츠
/// 폭이 뷰포트 폭에 의존해 가로 스크롤과 순환한다(스크롤 폭을 정하려면 콘텐츠 폭이 필요한데
/// 콘텐츠 폭이 다시 뷰포트에 의존). 합이 뷰포트보다 좁을 때만 `effective`가 마지막 열을
/// 늘려 빈틈을 없앤다 (plan D2)
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Columns {
    widths: [f32; Columns::COUNT],
    /// 열이 서는 차례 (FR-4). **`widths`와 독립이다** — 폭은 `ColumnKind::slot()`의 고정
    /// 자리에 담기고 여기만 바뀌므로, 차례를 바꿔도 각 열의 폭이 따라 움직인다.
    ///
    /// **`Vec`이 아니라 고정 배열인 이유**(D11): 이 구조체가 `Copy`라 `to_saved(self)`가
    /// 값으로 받는데, `Vec`을 넣으면 그 파생이 깨져 호출부가 컴파일되지 않는다.
    /// 언제나 `ALL_COLUMNS` 여섯을 한 번씩 담는다(`apply_saved_order`가 그것을 지킨다)
    order: [ColumnKind; Columns::COUNT],
}

impl Default for Columns {
    fn default() -> Columns {
        Columns::new()
    }
}

impl Columns {
    pub const COUNT: usize = 6;
    /// 기본 폭 — 앞 넷은 종전 값을 그대로 잇는다.
    /// 권한은 `rwxr-xr-x` 아홉 글자가, 소유자는 흔한 계정 이름이 잘리지 않을 만큼 잡았다
    const DEFAULT: [f32; Columns::COUNT] = [320.0, 90.0, 150.0, 150.0, 100.0, 110.0];

    pub fn new() -> Columns {
        Columns {
            widths: Columns::DEFAULT,
            order: ALL_COLUMNS,
        }
    }

    /// 저장된 폭으로 되살린다 (FR-11 세션 복원).
    ///
    /// **앞에서부터 있는 만큼만 받는다** — 열이 넷이던 시절의 세션에는 권한·소유자 폭이 없고,
    /// 길이가 다르다고 전부 버리면 사용자가 맞춰 둔 이름·크기 폭까지 초기화된다 (plan Edge Case).
    /// 유한하지 않은 값이 섞이면 그 자리만 기본값으로 되돌린다 — 설정 파일이 손상돼도
    /// 목록이 못 그려지는 상태로 시작하지 않게 한다
    pub fn from_saved(saved: &[f32]) -> Columns {
        let mut widths = Columns::DEFAULT;
        for (slot, &value) in widths.iter_mut().zip(saved) {
            if value.is_finite() {
                *slot = value.max(MIN_COL_WIDTH);
            }
        }
        Columns {
            widths,
            order: ALL_COLUMNS,
        }
    }

    /// 세션에 저장할 폭 — 보이지 않는 열의 폭도 함께 남긴다(다시 켰을 때 그대로 돌아온다)
    pub fn to_saved(self) -> Vec<f32> {
        self.widths.to_vec()
    }

    /// 이 패널에 보일 열 — 로컬은 앞 넷뿐이고, 원격은 켜 둔 것이 뒤에 붙는다 (Acceptance ①).
    ///
    /// **`self`를 받는 이유**(D11 부수 결정): 사용자가 바꾼 차례(`order`)를 반영해야 하는데
    /// 연관 함수로는 그 값에 닿을 수 없다
    pub fn visible(&self, is_remote: bool, flags: ColumnFlags) -> Vec<ColumnKind> {
        self.order
            .into_iter()
            .filter(|kind| {
                if kind.is_fixed() {
                    true
                } else {
                    is_remote && flags.shows(*kind)
                }
            })
            .collect()
    }

    /// 조절된 그대로의 폭 — 뷰포트를 반영하지 않은 값
    pub fn widths_of(&self, visible: &[ColumnKind]) -> Vec<f32> {
        visible
            .iter()
            .map(|kind| self.widths[kind.slot()])
            .collect()
    }

    /// 보이는 열의 폭 합 — 가로 스크롤 콘텐츠 폭의 근거
    pub fn content_width(&self, visible: &[ColumnKind]) -> f32 {
        visible.iter().map(|kind| self.widths[kind.slot()]).sum()
    }

    /// 실제로 그릴 폭. 합이 뷰포트보다 좁으면 **마지막 열만 늘려** 오른쪽 빈틈을 없앤다.
    /// 늘리는 것은 표시뿐이며 저장되는 폭은 그대로다 — 창 크기를 바꿀 때마다
    /// 사용자가 정한 폭이 덮어써지면 안 된다
    pub fn effective(&self, visible: &[ColumnKind], viewport_width: f32) -> Vec<f32> {
        let mut widths = self.widths_of(visible);
        let slack = viewport_width - widths.iter().sum::<f32>();
        if slack > 0.0
            && let Some(last) = widths.last_mut()
        {
            *last += slack;
        }
        widths
    }

    /// 드래그로 그 열의 폭을 바꾼다. 최소 폭 아래로는 줄지 않는다
    pub fn apply_drag(&mut self, kind: ColumnKind, delta: f32) {
        let width = &mut self.widths[kind.slot()];
        *width = (*width + delta).max(MIN_COL_WIDTH);
    }

    /// 그 열이 `order` 배열의 몇째 자리인가 — 화면에 보이는 차례를 배열 자리로 옮길 때 쓴다.
    ///
    /// `order`는 언제나 여섯 열을 한 번씩 담으므로 못 찾는 경우가 없다(못 찾으면 맨 앞으로 본다)
    pub fn order_index(&self, kind: ColumnKind) -> usize {
        self.order
            .iter()
            .position(|candidate| *candidate == kind)
            .unwrap_or(0)
    }

    /// `from` 자리의 열을 `to` 자리로 옮긴다 — 사이의 열들은 한 칸씩 밀린다.
    ///
    /// 자리는 **`order` 배열의 자리**이지 화면에 보이는 차례가 아니다 — 꺼 둔 열도 그 배열에
    /// 있으므로 둘이 어긋난다(`order_index`가 그 변환을 한다).
    /// 범위 밖이거나 제자리면 아무 일도 하지 않는다
    pub fn reorder(&mut self, from: usize, to: usize) {
        if from >= Columns::COUNT || to >= Columns::COUNT || from == to {
            return;
        }
        let moved = self.order[from];
        if from < to {
            self.order.copy_within(from + 1..=to, from);
        } else {
            self.order.copy_within(to..from, to + 1);
        }
        self.order[to] = moved;
    }

    /// 세션에 저장할 차례 — 열 키 여섯 개다 (FR-4)
    pub fn order_saved(self) -> Vec<String> {
        self.order
            .iter()
            .map(|kind| kind.as_key().to_owned())
            .collect()
    }

    /// 저장된 차례로 되살린다.
    ///
    /// **모르는 키와 겹친 키는 버리고, 빠진 키는 기본 차례대로 뒤에 채운다** — 열이 늘어난
    /// 판에서 돌아온 파일이나 손으로 고친 파일에서도 여섯 열이 하나도 빠지지 않는다
    pub fn apply_saved_order(&mut self, saved: &[String]) {
        let mut order: Vec<ColumnKind> = Vec::with_capacity(Columns::COUNT);
        for key in saved {
            if let Some(kind) = ColumnKind::from_key(key)
                && !order.contains(&kind)
            {
                order.push(kind);
            }
        }
        for kind in ALL_COLUMNS {
            if !order.contains(&kind) {
                order.push(kind);
            }
        }
        for (slot, kind) in self.order.iter_mut().zip(order) {
            *slot = kind;
        }
    }
}

/// 그 x가 화면에 보이는 열 가운데 몇째 위인가 — 표 밖이면 `None`
fn slot_at_x(left: f32, offsets: &[f32], widths: &[f32], x: f32) -> Option<usize> {
    offsets
        .iter()
        .zip(widths)
        .position(|(offset, width)| x >= left + offset && x < left + offset + width)
}

/// 열 왼쪽 경계의 x 오프셋 (첫 열은 0). 헤더와 행이 같은 x를 쓰게 하는 계산의 정본
fn x_offsets(widths: &[f32]) -> Vec<f32> {
    let mut acc = 0.0;
    widths
        .iter()
        .map(|width| {
            let offset = acc;
            acc += width;
            offset
        })
        .collect()
}

/// 자세히 보기가 그리는 데 필요한 목록 상태 — 소유하지 않고 빌려 쓴다
pub struct DetailsInput<'a, R: ListRow> {
    pub dir: &'a Path,
    pub entries: &'a [R],
    pub type_names: &'a [String],
    /// 보이는 행에 도달했을 때 채우는 지연 캐시라 가변으로 받는다
    pub icon_indices: &'a mut Vec<Option<i32>>,
    pub selection: &'a BTreeSet<usize>,
    /// 이름을 고치는 중인 행 (FR-64) — 그 행의 이름 칸에 입력칸을 얹는다
    pub rename: Option<&'a mut RenameEdit>,
    /// 잘라내기로 담긴 경로들 (FR-64) — 그 행은 흐리게 그린다
    pub cut_marks: &'a HashSet<PathBuf>,
    pub sort_key: SortKey,
    pub ascending: bool,
    pub columns: &'a mut Columns,
    /// 원격 패널인가 — 권한·소유자 열과 그 메뉴 항목은 여기서만 보인다 (Acceptance ①)
    pub is_remote: bool,
    /// 켤 수 있는 열의 현재 상태
    pub column_flags: ColumnFlags,
    /// 항목이 로컬 파일인가. 원격이면 **전체 경로로 하는 일**(셸 아이콘 정밀 조회)을 하지 않는다 —
    /// 원격 이름을 로컬 경로에 이어 붙이면 있지도 않은 파일을 셸에 묻게 된다 (D11)
    pub local_paths: bool,
    /// 이름 뒤 확장자를 보일지 (FR-52) — 표시에만 쓴다(경로·정렬은 원래 이름)
    pub show_extensions: bool,
}

/// 이번 프레임에 일어난 조작 — 목록 상태 변경은 호출부가 한다
#[derive(Default)]
pub struct DetailsOutcome {
    pub action: FileListAction,
    /// 선택 요청 (행 인덱스, 수식 키)
    pub select_request: Option<(usize, egui::Modifiers)>,
    /// 머리글 클릭으로 요청된 정렬 기준
    pub sort_click: Option<SortKey>,
    /// 빈 영역 클릭 — 선택 해제
    pub clear_selection: bool,
    /// 열 메뉴에서 뒤집기로 고른 열 — 상태 변경은 호출부가 한다 (인벤토리 #27·#28)
    pub column_toggle: Option<ColumnKind>,
    /// 이 행에서 끌기가 시작됐다 (FR-38) — 무엇을 실을지는 호출부가 정한다
    pub drag_started: Option<usize>,
    /// 이름 편집이 이번 프레임에 끝난 방식 (FR-64) — 상태를 접는 것은 호출부가 한다
    pub rename_end: Option<RenameEnd>,
    /// 지금 끌고 있는 열 경계의 x — **이 모듈 안에서 소비한다**(호출부는 보지 않는다).
    ///
    /// `show_header`가 담고 행을 다 그린 뒤에 긋는다. 머리글이 행보다 먼저 그려지므로
    /// 거기서 바로 그으면 행 배경(얼룩·hover·선택)이 같은 레이어에서 선을 덮는다
    resize_guide_x: Option<f32>,
    /// 열을 끌어 옮기는 중 **놓일 자리**의 x — 위 `resize_guide_x`와 같은 이유로 이 모듈
    /// 안에서 소비한다(행을 다 그린 뒤에 그어야 행 배경에 덮이지 않는다)
    reorder_guide_x: Option<f32>,
}

/// 자세히 보기를 그린다.
///
/// 헤더와 행을 **하나의 `ScrollArea::both()` 안에** 넣는다 — 따로 두면 가로 스크롤 시
/// 머리글과 본문의 x가 어긋난다. 대신 세로 스크롤에서도 머리글이 함께 올라간다
/// (세로만 고정하려면 두 영역의 오프셋을 수동 동기화해야 해 이번 범위 밖 — plan Deferred)
pub fn show<R: ListRow>(
    ui: &mut egui::Ui,
    input: DetailsInput<'_, R>,
    icons: &mut IconCache,
    textures: &mut IconTextures,
) -> DetailsOutcome {
    let mut outcome = DetailsOutcome::default();
    let himl = icons.himl();
    let ctx = ui.ctx().clone();
    let row_count = input.entries.len();
    let DetailsInput {
        dir,
        show_extensions,
        entries,
        type_names,
        icon_indices,
        selection,
        mut rename,
        cut_marks,
        sort_key,
        ascending,
        columns,
        is_remote,
        column_flags,
        local_paths,
    } = input;
    // 보일 열은 패널 종류와 토글로 정해진다 — 로컬 패널에는 권한·소유자가 아예 없다
    let visible = columns.visible(is_remote, column_flags);

    let scroll = egui::ScrollArea::both().auto_shrink([false, false]);
    let output = scroll.show_viewport(ui, |ui, viewport| {
        // 뷰포트 폭은 **스크롤 영역 안에서** 잰다 — 바깥의 `available_width`는 세로 스크롤 막대
        // 폭을 빼지 않아, 그만큼 마지막 열이 넓어져 늘 가로 스크롤이 생긴다
        let widths = columns.effective(&visible, viewport.width());
        let offsets = x_offsets(&widths);
        let content_width = widths.iter().sum::<f32>();
        let content_height = HEADER_HEIGHT + row_count as f32 * ROW_HEIGHT;
        // 콘텐츠 전체를 한 번에 잡는다 — 스크롤 막대의 범위가 이 크기에서 나온다
        let (_, content) =
            ui.allocate_space(egui::vec2(content_width, content_height.max(HEADER_HEIGHT)));
        let (left, top) = (content.left(), content.top());

        show_header(
            ui,
            HeaderInput {
                columns,
                visible: &visible,
                widths: &widths,
                offsets: &offsets,
                left,
                top,
                sort_key,
                ascending,
                is_remote,
                column_flags,
            },
            &mut outcome,
        );

        // 보이는 행만 그린다(가상 스크롤) — 헤더가 차지한 높이를 뺀 뒤 행 높이로 나눈다
        let first = ((viewport.top() - HEADER_HEIGHT) / ROW_HEIGHT)
            .floor()
            .max(0.0) as usize;
        let last =
            (((viewport.bottom() - HEADER_HEIGHT) / ROW_HEIGHT).ceil() as usize + 1).min(row_count);
        // 이름 칸의 자리 — 편집 입력칸을 얹는 데 쓴다. 보이는 행과 화면 밖으로 밀린
        // 편집 행이 **같은 계산**을 써야 스크롤 중에 입력칸이 튀지 않는다
        let name_slot = visible.iter().position(|kind| *kind == ColumnKind::Name);
        let name_cell_rect = |index: usize| -> Option<egui::Rect> {
            let slot = name_slot?;
            let width = widths[slot] - NAME_X - CELL_PAD;
            if width <= 0.0 {
                return None;
            }
            let row_top = top + HEADER_HEIGHT + index as f32 * ROW_HEIGHT;
            Some(egui::Rect::from_min_size(
                egui::pos2(left + offsets[slot] + NAME_X, row_top + EDIT_INSET_Y),
                egui::vec2(width, ROW_HEIGHT - EDIT_INSET_Y * 2.0),
            ))
        };
        let font = egui::TextStyle::Body.resolve(ui.style());
        let mut editor_drawn = false;
        let stripe = ui.visuals().faint_bg_color;
        let hover_bg = ui.visuals().widgets.hovered.bg_fill;
        let sel_bg = ui.visuals().selection.bg_fill;
        for index in first..last {
            let row_top = top + HEADER_HEIGHT + index as f32 * ROW_HEIGHT;
            let rect = egui::Rect::from_min_size(
                egui::pos2(left, row_top),
                egui::vec2(content_width, ROW_HEIGHT),
            );
            let resp = ui.interact(
                rect,
                ui.id().with(("row", index)),
                egui::Sense::click_and_drag(),
            );
            if resp.drag_started() {
                outcome.drag_started = Some(index);
            }
            if resp.clicked() {
                outcome.select_request = Some((index, ui.input(|i| i.modifiers)));
            }
            // **세 번째 클릭까지 열기로 본다** — egui는 앞선 클릭에서 0.6초
            // (`max_double_click_delay`의 두 배) 안에 든 더블클릭을 트리플클릭으로 세고,
            // 그때는 `double_clicked()`가 서지 않는다(egui `input_state`의 클릭 수 계산).
            // 메뉴 항목을 고른 직후처럼 바로 앞에 클릭이 있으면 이어지는 더블클릭이 죽는다.
            // 파일 목록에 트리플클릭으로 하는 일은 따로 없어 둘을 같이 받아도 겹치지 않는다
            if resp.double_clicked() || resp.triple_clicked() {
                outcome.action = FileListAction::Open(index);
            }
            if resp.secondary_clicked()
                && let Some(pos) = resp.interact_pointer_pos()
            {
                // 선택되지 않은 행을 우클릭하면 그 행을 단독 선택한 뒤 메뉴를 연다
                if !selection.contains(&index) {
                    outcome.select_request = Some((index, egui::Modifiers::NONE));
                }
                outcome.action = FileListAction::Context {
                    index: Some(index),
                    pos,
                };
            }
            if !ui.is_rect_visible(rect) {
                continue;
            }

            let painter = ui.painter();
            if index % 2 == 1 {
                painter.rect_filled(rect, 0.0, stripe);
            }
            if selection.contains(&index) {
                painter.rect_filled(rect, 0.0, sel_bg);
            } else if resp.hovered() {
                painter.rect_filled(rect, 0.0, hover_bg);
            }

            let entry = &entries[index];
            let y = rect.center().y;
            // 숨김·시스템 항목은 아이콘과 글자를 함께 흐리게 그린다 (FR-13 — 탐색기와 같은 표시)
            let dimmed = entry.is_dimmed();
            // 잘라내기로 담긴 항목도 흐리게 (FR-64). **표시가 하나도 없으면 경로를 짓지
            // 않는다** — 그 문자열 만들기가 보이는 행마다 붙는데 대부분의 프레임에서 헛일이다
            let cut =
                local_paths && !cut_marks.is_empty() && cut_marks.contains(&dir.join(entry.name()));
            let text_color =
                cut_text_color(cut).unwrap_or_else(|| dim_if_hidden(theme::TEXT, dimmed));
            let editing = rename.as_ref().is_some_and(|edit| edit.index == index);
            // 보이는 행에 한해 아이콘 인덱스를 조회한다 — 규칙은 격자 보기와 같다
            let icon_index = crate::ui::list_grid::resolve_icon(
                dir,
                entry,
                index,
                icon_indices,
                icons,
                local_paths,
            );
            if let Some(tex) = textures.get(&ctx, himl, icon_index) {
                let icon_rect = egui::Rect::from_min_size(
                    egui::pos2(left + ICON_X, y - ICON_SIZE / 2.0),
                    egui::vec2(ICON_SIZE, ICON_SIZE),
                );
                // painter를 다시 얻는다 — textures가 ui를 빌리는 사이 앞의 painter가 무효화된다
                ui.painter().image(
                    tex.id(),
                    icon_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    cut_icon_tint(cut)
                        .unwrap_or_else(|| dim_if_hidden(egui::Color32::WHITE, dimmed)),
                );
            }

            let painter = ui.painter();
            for (slot, kind) in visible.iter().enumerate() {
                // 편집 중인 행의 이름은 그리지 않는다 — 그 자리에 입력칸이 놓인다
                if editing && *kind == ColumnKind::Name {
                    continue;
                }
                let text = cell_text(entry, &type_names[index], *kind, show_extensions);
                // 이름 열만 아이콘 자리를 비켜 시작한다
                let leading = if *kind == ColumnKind::Name {
                    NAME_X
                } else {
                    CELL_PAD
                };
                let x = left + offsets[slot] + leading;
                let width = widths[slot] - leading - CELL_PAD;
                if text.is_empty() || width <= 0.0 {
                    continue;
                }
                let galley = elided_galley_colored(painter, text, font.clone(), width, text_color);
                painter.galley(egui::pos2(x, y - galley.size().y / 2.0), galley, text_color);
            }

            // 이름 칸 자리에 입력칸을 얹는다 — 행 배경·아이콘을 다 그린 뒤라 위에 놓인다
            if editing
                && let Some(edit_rect) = name_cell_rect(index)
                && let Some(edit) = rename.as_mut()
            {
                editor_drawn = true;
                if let Some(end) = rename_editor(ui, edit_rect, edit) {
                    outcome.rename_end = Some(end);
                }
            }
        }

        // 편집 중인 행이 화면 밖으로 밀려도 입력칸을 계속 놓는다 — 위젯을 그리지 않으면
        // egui가 포커스를 거두고 그것이 취소로 처리돼, 스크롤 한 번에 고치던 이름이
        // 사라진다(탐색기는 유지한다). 스크롤 영역 밖이라 잘려 보이지 않는다
        if !editor_drawn
            && let Some(edit) = rename.as_mut()
            && edit.index < row_count
            && let Some(edit_rect) = name_cell_rect(edit.index)
            && let Some(end) = rename_editor(ui, edit_rect, edit)
        {
            outcome.rename_end = Some(end);
        }
        // 끄는 동안의 강조선 — 행 배경보다 **나중에** 그어야 끊기지 않는다
        if let Some(x) = outcome.resize_guide_x {
            ui.painter().vline(
                x,
                top..=content.bottom(),
                egui::Stroke::new(1.0, theme::ACCENT),
            );
        }
        // 열을 옮기는 중이면 놓일 자리를 같은 방식으로 보인다 — 표시가 없으면 어디에
        // 놓이는지 알 수 없다. 폭 조절선보다 굵게 그어 둘을 구분한다
        if let Some(x) = outcome.reorder_guide_x {
            ui.painter().vline(
                x,
                top..=content.bottom(),
                egui::Stroke::new(2.0, theme::ACCENT),
            );
        }
        content
    });

    // 목록 아래 빈 공간 우클릭 → 폴더 배경 메뉴.
    // 대상은 **콘텐츠가 끝난 지점부터** 뷰포트 바닥까지다 — 뷰포트 전체를 잡으면 이 위젯이
    // 행보다 나중에 등록돼 행 클릭을 통째로 가로챈다(egui 히트 테스트는 겹칠 때 나중에
    // 등록된 위젯을 위로 본다). 항목이 화면을 채우면 빈 공간이 없어 배경 메뉴도 열리지
    // 않는데, 이는 Windows 탐색기와 같은 동작이다
    let content_bottom = output.inner.bottom();
    if content_bottom < output.inner_rect.bottom() {
        let empty = egui::Rect::from_min_max(
            egui::pos2(output.inner_rect.left(), content_bottom),
            output.inner_rect.max,
        );
        let resp = ui.interact(empty, ui.id().with("list_bg"), egui::Sense::click());
        if resp.secondary_clicked()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            outcome.action = FileListAction::Context { index: None, pos };
        }
        if resp.clicked() {
            outcome.clear_selection = true;
        }
    }
    outcome
}

/// 셀 하나에 보일 문자열.
///
/// **심볼릭 링크는 이름 뒤에 `→ 대상`이 붙는다** (FR-31·README §3) — 링크인지 아닌지가
/// 목록에서 드러나야 지우거나 옮길 때 실수하지 않는다
fn cell_text<R: ListRow>(
    entry: &R,
    type_name: &str,
    kind: ColumnKind,
    show_extensions: bool,
) -> String {
    match kind {
        ColumnKind::Name => match entry.link_target() {
            // 확장자를 뗀 이름 뒤에 링크 대상을 붙인다 — 대상은 서버가 준 경로라 그대로 둔다
            Some(target) => format!("{} → {target}", entry.display_name(show_extensions)),
            None => entry.display_name(show_extensions),
        },
        ColumnKind::Size => {
            if entry.is_dir() {
                String::new()
            } else {
                format_size(entry.size())
            }
        }
        ColumnKind::Type => type_name.to_owned(),
        // 시각을 모르는 줄(상위 이동 `..`·서버가 시각을 주지 않은 항목)은 빈칸이다 —
        // 0을 그대로 옮기면 FILETIME의 기점인 `1601-01-01`이 진짜 날짜인 양 보인다
        ColumnKind::Modified => match entry.modified_key() {
            0 => String::new(),
            ticks => format_filetime(ticks),
        },
        // 서버가 주지 않았으면 빈칸이다 — 없는 값을 지어내지 않는다 (plan Edge Case)
        ColumnKind::Permissions => entry.permissions().unwrap_or_default(),
        ColumnKind::Owner => entry.owner().unwrap_or_default().to_owned(),
    }
}

/// 머리글이 그리기에 필요한 것 — 인자가 많아 한 묶음으로 든다
struct HeaderInput<'a> {
    columns: &'a mut Columns,
    visible: &'a [ColumnKind],
    widths: &'a [f32],
    offsets: &'a [f32],
    left: f32,
    top: f32,
    sort_key: SortKey,
    ascending: bool,
    is_remote: bool,
    column_flags: ColumnFlags,
}

/// 열 머리글 — 클릭으로 정렬, 경계 드래그로 폭 조절, 현재 정렬 열에 방향 표시,
/// 우클릭으로 열 메뉴 (인벤토리 #22~28)
fn show_header(ui: &mut egui::Ui, input: HeaderInput<'_>, outcome: &mut DetailsOutcome) {
    let HeaderInput {
        columns,
        visible,
        widths,
        offsets,
        left,
        top,
        sort_key,
        ascending,
        is_remote,
        column_flags,
    } = input;
    let header_rect = egui::Rect::from_min_size(
        egui::pos2(left, top),
        egui::vec2(widths.iter().sum(), HEADER_HEIGHT),
    );
    ui.painter().rect_filled(header_rect, 0.0, theme::HEADER_BG);
    let font = egui::TextStyle::Body.resolve(ui.style());

    // 이번 프레임에 놓인 열 — 고리를 다 돈 뒤에 반영한다(고리 안에서 `columns`를 바꾸면
    // 이미 계산해 둔 `visible`·`offsets`와 어긋난 채로 나머지 열을 그리게 된다)
    let mut dropped: Option<(ColumnKind, ColumnKind)> = None;

    for (slot, kind) in visible.iter().enumerate() {
        let width = widths[slot];
        if width <= 0.0 {
            continue;
        }
        let x = left + offsets[slot];
        let cell = egui::Rect::from_min_size(egui::pos2(x, top), egui::vec2(width, HEADER_HEIGHT));
        // **`click_and_drag`인 이유**(D4): 머리글 가운데를 끌면 열 차례가 바뀐다. 경계
        // 근처는 아래에서 더 나중에 등록하는 폭 조절 핸들이 가져가므로 둘이 겹치지 않는다
        let resp = ui.interact(
            cell,
            ui.id().with(("head", kind.label())),
            egui::Sense::click_and_drag(),
        );
        // 정렬할 수 없는 열(권한·소유자)은 눌러도 기준이 바뀌지 않는다
        if resp.clicked()
            && let Some(key) = kind.sort_key()
        {
            outcome.sort_click = Some(key);
        }
        // 열이 하나뿐이면 옮길 자리가 없다
        if visible.len() > 1
            && (resp.dragged() || resp.drag_stopped())
            && let Some(pointer) = ui.ctx().pointer_interact_pos()
            && let Some(target) = slot_at_x(left, offsets, widths, pointer.x)
            && target != slot
        {
            // 왼쪽으로 끌면 그 열의 왼쪽 경계에, 오른쪽으로 끌면 오른쪽 경계에 놓인다
            outcome.reorder_guide_x = Some(if target < slot {
                left + offsets[target]
            } else {
                left + offsets[target] + widths[target]
            });
            if resp.drag_stopped() {
                dropped = Some((*kind, visible[target]));
            }
        }
        column_menu_popup(&resp, is_remote, column_flags, outcome);
        // 정렬 화살표도 아이콘 글꼴에서 온다 (프로젝트 규약 — 원본 삼각형은 두부 위험)
        let arrow = match kind.sort_key() {
            Some(key) if key == sort_key => {
                if ascending {
                    egui_phosphor::regular::CARET_UP
                } else {
                    egui_phosphor::regular::CARET_DOWN
                }
            }
            _ => "",
        };
        // 색을 갤리에 구워 넣는다 — 그리면서 넘기는 색은 갤리가 이미 색을 가졌으면 무시되어,
        // 머리글이 `HEADER_TEXT`가 아니라 기본 글자색으로 그려지고 있었다 (T20 리뷰가 지목)
        let galley = elided_galley_colored(
            ui.painter(),
            format!("{}{arrow}", kind.label()),
            font.clone(),
            width - CELL_PAD * 2.0,
            theme::HEADER_TEXT,
        );
        ui.painter().galley(
            egui::pos2(x + CELL_PAD, cell.center().y - galley.size().y / 2.0),
            galley,
            theme::HEADER_TEXT,
        );
    }

    if let Some((moved, target)) = dropped {
        let (from, to) = (columns.order_index(moved), columns.order_index(target));
        columns.reorder(from, to);
    }

    // 드래그 핸들은 머리글 셀보다 **나중에** 등록한다 — egui는 겹칠 때 나중 위젯을 위로 보므로
    // 경계 위에서 누른 것이 정렬 클릭·차례 드래그로 새지 않는다
    for (slot, kind) in visible.iter().enumerate() {
        let boundary = left + offsets[slot] + widths[slot];
        // 평소에도 경계가 보여야 어디를 잡을지 알 수 있다 (2026-08-18 사용자 보고).
        // 마지막 열의 오른쪽 끝에는 긋지 않는다 — 그것은 표 바깥 경계다
        if slot + 1 < visible.len() {
            ui.painter().vline(
                boundary,
                top..=(top + HEADER_HEIGHT),
                egui::Stroke::new(1.0, theme::BORDER_SUBTLE),
            );
        }
        let handle = egui::Rect::from_min_size(
            egui::pos2(boundary - HANDLE_WIDTH / 2.0, top),
            egui::vec2(HANDLE_WIDTH, HEADER_HEIGHT),
        );
        // `click_and_drag`로 잡아 **클릭까지 이 핸들이 삼키게** 한다 — 드래그만 감지하면
        // 경계를 톡 눌렀을 때 아래 머리글 셀로 새어 의도치 않게 정렬이 바뀐다
        let resp = ui.interact(
            handle,
            ui.id().with(("col_handle", slot)),
            egui::Sense::click_and_drag(),
        );
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if resp.dragged() {
            columns.apply_drag(*kind, resp.drag_delta().x);
            outcome.resize_guide_x = Some(boundary);
        }
    }
}

/// 머리글 우클릭으로 여는 열 메뉴 (인벤토리 #22~28).
///
/// **로컬 패널에서는 열지 않는다** — 켤 수 있는 항목이 원격 전용 둘뿐이라, 로컬에서 열면
/// 끌 수 없는 항목 넷만 늘어선 메뉴가 뜬다
fn column_menu_popup(
    response: &egui::Response,
    is_remote: bool,
    flags: ColumnFlags,
    outcome: &mut DetailsOutcome,
) {
    if !is_remote {
        return;
    }
    egui::Popup::context_menu(response).show(|ui| {
        crate::ui::theme::menu_style(ui);
        crate::ui::menu::column_menu_items(ui, flags, &mut outcome.column_toggle);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::types::RemoteEntry;

    /// 로컬 패널이 보이는 열 — 앞 넷뿐이다
    fn local_visible() -> Vec<ColumnKind> {
        // 차례를 바꿔 재는 시험은 자기 인스턴스를 따로 만든다 — 이 헬퍼는 기본 차례다
        Columns::new().visible(false, ColumnFlags::default())
    }

    fn remote_entry(name: &str, mode: Option<u32>, owner: Option<&str>) -> RemoteEntry {
        RemoteEntry {
            name: name.to_owned(),
            is_dir: false,
            is_symlink: false,
            link_target: None,
            size: 1024,
            modified: None,
            mode,
            owner: owner.map(str::to_owned),
        }
    }

    #[test]
    fn 기본_폭은_여섯_열_모두_양수다() {
        let columns = Columns::new();
        for kind in ALL_COLUMNS {
            let width = columns.widths_of(&[kind])[0];
            assert!(
                width >= MIN_COL_WIDTH,
                "{kind:?}의 기본 폭이 최소 폭보다 작다"
            );
        }
    }

    #[test]
    fn 드래그로_폭이_바뀐다() {
        let mut columns = Columns::new();
        let before = columns.widths_of(&[ColumnKind::Name])[0];
        columns.apply_drag(ColumnKind::Name, 50.0);
        assert_eq!(columns.widths_of(&[ColumnKind::Name])[0], before + 50.0);
    }

    #[test]
    fn 최소_폭_아래로는_줄지_않는다() {
        // 열이 0폭이 되면 다시 넓힐 핸들조차 잡을 수 없다
        let mut columns = Columns::new();
        columns.apply_drag(ColumnKind::Size, -10_000.0);
        assert_eq!(columns.widths_of(&[ColumnKind::Size])[0], MIN_COL_WIDTH);
    }

    #[test]
    fn 콘텐츠_폭은_보이는_열의_합이다() {
        let columns = Columns::new();
        let visible = local_visible();
        assert_eq!(
            columns.content_width(&visible),
            columns.widths_of(&visible).iter().sum::<f32>()
        );
    }

    #[test]
    fn 뷰포트가_넓으면_마지막_열만_늘어난다() {
        // 오른쪽에 빈 띠가 남지 않게 한다. 늘어나는 것은 표시뿐이라 저장 폭은 그대로다
        let columns = Columns::new();
        let visible = local_visible();
        let viewport = columns.content_width(&visible) + 200.0;
        let effective = columns.effective(&visible, viewport);
        let base = columns.widths_of(&visible);
        assert_eq!(effective.iter().sum::<f32>(), viewport);
        assert_eq!(effective[..3], base[..3]);
        assert_eq!(effective[3], base[3] + 200.0);
        assert_eq!(
            columns.to_saved(),
            Columns::new().to_saved(),
            "저장 폭이 바뀌었다"
        );
    }

    #[test]
    fn 뷰포트가_좁으면_폭을_줄이지_않는다() {
        // 줄이면 가로 스크롤이 생길 이유가 사라진다 — 좁을 때는 스크롤로 본다
        let columns = Columns::new();
        let visible = local_visible();
        assert_eq!(
            columns.effective(&visible, 100.0),
            columns.widths_of(&visible)
        );
        assert!(columns.content_width(&visible) > 100.0);
    }

    #[test]
    fn 열_오프셋은_앞_열_폭의_누적이다() {
        let widths = [100.0, 50.0, 70.0, 30.0];
        assert_eq!(x_offsets(&widths), vec![0.0, 100.0, 150.0, 220.0]);
    }

    #[test]
    fn 저장된_폭을_되살린다() {
        let saved = vec![200.0, 60.0, 120.0, 90.0, 80.0, 70.0];
        assert_eq!(Columns::from_saved(&saved).to_saved(), saved);
    }

    #[test]
    fn 열이_넷이던_옛_세션은_앞_넷만_되살린다() {
        // 권한·소유자가 없던 시절의 저장값 — 전부 버리면 사용자가 맞춰 둔 폭까지 사라진다
        // (plan Edge Case: 열 폭 저장값에 5·6번째가 없는 옛 세션 → 기본 폭)
        let old = vec![200.0, 60.0, 120.0, 90.0];
        let restored = Columns::from_saved(&old).to_saved();
        assert_eq!(restored[..4], old[..], "옛 세션의 폭이 버려졌다");
        assert_eq!(
            restored[4..],
            Columns::new().to_saved()[4..],
            "없던 열은 기본 폭이어야 한다"
        );
    }

    #[test]
    fn 손상된_저장값은_안전한_폭으로_보정된다() {
        let saved = vec![f32::NAN, -50.0, f32::INFINITY, 120.0, f32::NAN, 60.0];
        let columns = Columns::from_saved(&saved);
        for width in columns.to_saved() {
            assert!(
                width.is_finite() && width >= MIN_COL_WIDTH,
                "보정되지 않은 폭: {width}"
            );
        }
        assert_eq!(columns.to_saved()[3], 120.0, "정상 값까지 바뀌었다");
    }

    /// 머리글 위에서 누르고 끌어 놓는 네 프레임을 돌린다 — `columns`가 어떻게 바뀌는지 본다
    fn 머리글을_끈다(
        columns: &mut Columns,
        visible: &[ColumnKind],
        widths: &[f32],
        시작_x: f32,
        끝_x: f32,
    ) {
        fn 누름(pos: egui::Pos2, pressed: bool) -> egui::Event {
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }
        }
        let offsets = x_offsets(widths);
        let y = HEADER_HEIGHT / 2.0;
        let ctx = egui::Context::default();
        let frames = [
            vec![egui::Event::PointerMoved(egui::pos2(시작_x, y))],
            vec![누름(egui::pos2(시작_x, y), true)],
            vec![egui::Event::PointerMoved(egui::pos2(끝_x, y))],
            vec![누름(egui::pos2(끝_x, y), false)],
        ];
        for events in frames {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let mut outcome = DetailsOutcome::default();
                    show_header(
                        ui,
                        HeaderInput {
                            columns,
                            visible,
                            widths,
                            offsets: &offsets,
                            left: 0.0,
                            top: 0.0,
                            sort_key: SortKey::Name,
                            ascending: true,
                            is_remote: false,
                            column_flags: ColumnFlags::default(),
                        },
                        &mut outcome,
                    );
                });
            });
        }
    }

    #[test]
    fn 머리글_가운데를_끌면_차례가_바뀐다() {
        // Acceptance ⓐ (D4) — `수정일` 가운데를 잡아 `크기` 위로 끈다
        let mut columns = Columns::new();
        let visible = local_visible();
        let widths = [100.0, 60.0, 80.0, 90.0];
        let 폭_전 = columns.widths_of(&visible);
        // 열 넷의 가운데: 50 / 130 / 200 / 285
        머리글을_끈다(&mut columns, &visible, &widths, 285.0, 130.0);
        assert_eq!(
            columns.visible(false, ColumnFlags::default()),
            vec![
                ColumnKind::Name,
                ColumnKind::Modified,
                ColumnKind::Size,
                ColumnKind::Type
            ],
            "가운데를 끌었는데 차례가 그대로다"
        );
        assert_eq!(
            columns.widths_of(&local_visible()),
            폭_전,
            "차례를 바꾸는 드래그가 폭까지 건드렸다"
        );
    }

    #[test]
    fn 머리글_경계를_끌면_폭만_바뀐다() {
        // Acceptance ⓑ (D4) — 같은 머리글이라도 경계 근처는 종전대로 폭 조절이다.
        // 이 갈림이 깨지면 폭을 조절하려다 열이 옮겨진다
        let mut columns = Columns::new();
        let visible = local_visible();
        let widths = [100.0, 60.0, 80.0, 90.0];
        let 차례_전 = columns.order_saved();
        let 이름_폭 = columns.widths_of(&[ColumnKind::Name])[0];
        // 첫 열의 오른쪽 경계는 x=100이다
        머리글을_끈다(&mut columns, &visible, &widths, 100.0, 150.0);
        assert_eq!(
            columns.order_saved(),
            차례_전,
            "경계를 끌었는데 차례가 바뀌었다"
        );
        assert!(
            columns.widths_of(&[ColumnKind::Name])[0] > 이름_폭,
            "경계를 끌었는데 폭이 그대로다"
        );
    }

    /// 그 열이 몇째 자리로 옮겨지도록 끈다 — 화면 자리가 아니라 `order` 배열 자리다
    fn 옮긴다(columns: &mut Columns, moved: ColumnKind, target: ColumnKind) {
        let (from, to) = (columns.order_index(moved), columns.order_index(target));
        columns.reorder(from, to);
    }

    #[test]
    fn 열을_옮기면_그_차례로_보인다() {
        // Acceptance ⓐ — `수정일`을 `크기` 왼쪽으로 끈다
        let mut columns = Columns::new();
        옮긴다(&mut columns, ColumnKind::Modified, ColumnKind::Size);
        assert_eq!(
            columns.visible(false, ColumnFlags::default()),
            vec![
                ColumnKind::Name,
                ColumnKind::Modified,
                ColumnKind::Size,
                ColumnKind::Type
            ]
        );
    }

    #[test]
    fn 열_폭은_그_열을_따라_움직인다() {
        // Acceptance ⓐ — 폭은 자리가 아니라 열에 딸린 값이다(폭 배열은 고정 자리를 쓴다)
        let mut columns = Columns::new();
        let 수정일_폭 = columns.widths_of(&[ColumnKind::Modified])[0];
        옮긴다(&mut columns, ColumnKind::Modified, ColumnKind::Size);
        let visible = columns.visible(false, ColumnFlags::default());
        let widths = columns.widths_of(&visible);
        assert_eq!(visible[1], ColumnKind::Modified);
        assert_eq!(widths[1], 수정일_폭, "옮긴 열이 제 폭을 그대로 가진다");
    }

    #[test]
    fn 이름_열도_옮길_수_있다() {
        // Acceptance ⓓ — 첫 열을 고정하지 않는다
        let mut columns = Columns::new();
        옮긴다(&mut columns, ColumnKind::Name, ColumnKind::Modified);
        assert_eq!(
            columns.visible(false, ColumnFlags::default()),
            vec![
                ColumnKind::Size,
                ColumnKind::Type,
                ColumnKind::Modified,
                ColumnKind::Name
            ]
        );
    }

    #[test]
    fn 제자리나_범위_밖으로_옮기면_아무_일도_없다() {
        let mut columns = Columns::new();
        let before = columns.order_saved();
        columns.reorder(2, 2);
        columns.reorder(0, Columns::COUNT);
        columns.reorder(Columns::COUNT, 0);
        assert_eq!(columns.order_saved(), before);
    }

    #[test]
    fn 열_차례가_저장_키로_왕복한다() {
        // Acceptance ⓒ — 이 왕복이 끊기면 재시작할 때마다 기본 차례로 돌아간다
        let mut columns = Columns::new();
        옮긴다(&mut columns, ColumnKind::Modified, ColumnKind::Size);
        let saved = columns.order_saved();
        assert_eq!(saved[1], "modified");

        let mut restored = Columns::new();
        restored.apply_saved_order(&saved);
        assert_eq!(restored.order_saved(), saved);
        assert_eq!(
            restored.visible(false, ColumnFlags::default()),
            columns.visible(false, ColumnFlags::default())
        );
    }

    #[test]
    fn 모르는_키와_빠진_키는_기본_차례로_채운다() {
        // Acceptance ⓔ — 열이 늘어난 판에서 돌아온 파일·손으로 고친 파일 대비.
        // 여섯 열이 하나도 빠지지 않아야 목록이 온전히 그려진다
        let mut columns = Columns::new();
        columns.apply_saved_order(&[
            "owner".to_owned(),
            "없는_열".to_owned(),
            "name".to_owned(),
            "name".to_owned(),
        ]);
        assert_eq!(
            columns.order_saved(),
            vec!["owner", "name", "size", "type", "modified", "permissions"]
        );
    }

    #[test]
    fn 원격에서는_켠_열만_바뀐_차례에_든다() {
        // Acceptance ⓕ — 꺼 둔 열도 차례에는 남지만 화면에는 서지 않는다
        let mut columns = Columns::new();
        옮긴다(&mut columns, ColumnKind::Owner, ColumnKind::Name);
        let 권한만 = ColumnFlags {
            permissions: true,
            owner: false,
        };
        let visible = columns.visible(true, 권한만);
        assert!(
            !visible.contains(&ColumnKind::Owner),
            "꺼 둔 열은 서지 않는다"
        );
        assert_eq!(visible.first(), Some(&ColumnKind::Name));

        let 둘_다 = ColumnFlags {
            permissions: true,
            owner: true,
        };
        assert_eq!(
            columns.visible(true, 둘_다).first(),
            Some(&ColumnKind::Owner),
            "켜면 옮겨 둔 자리에 선다"
        );
    }

    #[test]
    fn 표_밖의_x는_어느_열도_가리키지_않는다() {
        let widths = [100.0, 50.0, 80.0];
        let offsets = x_offsets(&widths);
        assert_eq!(slot_at_x(10.0, &offsets, &widths, 5.0), None);
        assert_eq!(slot_at_x(10.0, &offsets, &widths, 10.0), Some(0));
        assert_eq!(slot_at_x(10.0, &offsets, &widths, 115.0), Some(1));
        assert_eq!(slot_at_x(10.0, &offsets, &widths, 240.0), None);
    }

    #[test]
    fn 로컬_패널에는_권한과_소유자_열이_없다() {
        // Acceptance ① — 로컬 파일의 권한은 ACL이라 이 두 열로 표현할 수 없다.
        // 토글을 켜 둔 채 로컬 탭으로 돌아와도 나타나지 않아야 한다
        let 켠_상태 = ColumnFlags {
            permissions: true,
            owner: true,
        };
        for flags in [ColumnFlags::default(), 켠_상태] {
            let visible = Columns::new().visible(false, flags);
            assert_eq!(
                visible,
                vec![
                    ColumnKind::Name,
                    ColumnKind::Size,
                    ColumnKind::Type,
                    ColumnKind::Modified
                ],
                "로컬 패널에 원격 전용 열이 나타났다"
            );
        }
    }

    #[test]
    fn 원격_패널은_켠_열만_뒤에_붙는다() {
        // Acceptance ④ — 열을 켜면 폭 합이 늘어 가로 스크롤이 생긴다
        let columns = Columns::new();
        let 꺼짐 = columns.visible(true, ColumnFlags::default());
        assert_eq!(꺼짐, local_visible(), "기본값은 둘 다 꺼짐이다");

        let 권한만 = columns.visible(
            true,
            ColumnFlags {
                permissions: true,
                owner: false,
            },
        );
        assert_eq!(권한만.last(), Some(&ColumnKind::Permissions));
        assert!(columns.content_width(&권한만) > columns.content_width(&꺼짐));

        let 둘_다 = columns.visible(
            true,
            ColumnFlags {
                permissions: true,
                owner: true,
            },
        );
        assert_eq!(둘_다.len(), 6);
        assert!(columns.content_width(&둘_다) > columns.content_width(&권한만));
    }

    #[test]
    fn 앞_넷은_끌_수_없고_뒤_둘만_뒤집힌다() {
        // Acceptance ② · 인벤토리 #23~28
        let mut flags = ColumnFlags::default();
        for kind in [
            ColumnKind::Name,
            ColumnKind::Size,
            ColumnKind::Type,
            ColumnKind::Modified,
        ] {
            assert!(kind.is_fixed(), "{kind:?}는 고정 열이어야 한다");
            assert!(flags.shows(kind), "고정 열은 늘 켜져 있다");
            flags.toggle(kind);
            assert!(flags.shows(kind), "고정 열이 꺼졌다");
        }
        for kind in [ColumnKind::Permissions, ColumnKind::Owner] {
            assert!(!kind.is_fixed());
            assert!(!flags.shows(kind), "기본값은 꺼짐이다");
            flags.toggle(kind);
            assert!(flags.shows(kind));
            flags.toggle(kind);
            assert!(!flags.shows(kind));
        }
    }

    #[test]
    fn 권한과_소유자_열은_정렬_대상이_아니다() {
        // 디자인이 이 두 열에 정렬을 주지 않았다 — 눌러도 기준이 바뀌지 않는다
        assert_eq!(ColumnKind::Name.sort_key(), Some(SortKey::Name));
        assert_eq!(ColumnKind::Modified.sort_key(), Some(SortKey::Modified));
        assert_eq!(ColumnKind::Permissions.sort_key(), None);
        assert_eq!(ColumnKind::Owner.sort_key(), None);
    }

    #[test]
    fn 심볼릭_링크는_이름_뒤에_대상이_붙는다() {
        // Acceptance ③ · FR-31 — 링크인지 아닌지가 목록에서 드러나야 실수하지 않는다
        let mut link = remote_entry("current", None, None);
        link.is_symlink = true;
        link.link_target = Some("/releases/2026-08".to_owned());
        assert_eq!(
            cell_text(&link, "링크", ColumnKind::Name, true),
            "current → /releases/2026-08"
        );

        // 링크가 아니면 이름만 나온다
        let plain = remote_entry("app.log", None, None);
        assert_eq!(cell_text(&plain, "로그", ColumnKind::Name, true), "app.log");
    }

    #[test]
    fn 확장자를_꺼도_링크_대상은_그대로다() {
        // plan Edge Case — 자를 것은 **이름뿐**이다. 대상 경로까지 자르면
        // 링크가 어디를 가리키는지 화면이 거짓말을 한다
        let mut link = remote_entry("current.bak", None, None);
        link.is_symlink = true;
        link.link_target = Some("/releases/2026-08.tar.gz".to_owned());
        assert_eq!(
            cell_text(&link, "링크", ColumnKind::Name, false),
            "current → /releases/2026-08.tar.gz"
        );
    }

    #[test]
    fn 권한을_주지_않는_서버의_칸은_비어_있다() {
        // plan Edge Case — `0o777` 같은 기본값을 지어내면 화면이 서버가 하지 않은 말을 한다
        let 없음 = remote_entry("a.txt", None, None);
        assert_eq!(
            cell_text(&없음, "텍스트", ColumnKind::Permissions, true),
            ""
        );
        assert_eq!(cell_text(&없음, "텍스트", ColumnKind::Owner, true), "");

        let 있음 = remote_entry("a.txt", Some(0o755), Some("deploy"));
        assert_eq!(
            cell_text(&있음, "텍스트", ColumnKind::Permissions, true),
            "rwxr-xr-x"
        );
        assert_eq!(
            cell_text(&있음, "텍스트", ColumnKind::Owner, true),
            "deploy"
        );
    }

    /// 머리글만 한 프레임 그리고 세로선을 모은다 (x, 색)
    fn draw_header_lines(visible: &[ColumnKind], widths: &[f32]) -> Vec<(f32, egui::Color32)> {
        let mut columns = Columns::new();
        let mut outcome = DetailsOutcome::default();
        let offsets = x_offsets(widths);
        let ctx = egui::Context::default();
        let output = ctx.run_ui(Default::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                show_header(
                    ui,
                    HeaderInput {
                        columns: &mut columns,
                        visible,
                        widths,
                        offsets: &offsets,
                        left: 0.0,
                        top: 0.0,
                        sort_key: SortKey::Name,
                        ascending: true,
                        is_remote: false,
                        column_flags: ColumnFlags::default(),
                    },
                    &mut outcome,
                );
            });
        });
        let mut lines = Vec::new();
        for clipped in &output.shapes {
            if let egui::Shape::LineSegment { points, stroke } = &clipped.shape
                && (points[0].x - points[1].x).abs() < 0.01
            {
                lines.push((points[0].x, stroke.color));
            }
        }
        lines
    }

    #[test]
    fn 머리글_열_경계마다_구분선이_선다() {
        // 2026-08-18 사용자 보고 — 선이 없어 어디를 끌어야 할지 알 수 없었다
        let visible = local_visible();
        let widths = vec![100.0, 50.0, 80.0, 70.0];
        let lines = draw_header_lines(&visible, &widths);
        let 구분선: Vec<f32> = lines
            .iter()
            .filter(|(_, color)| *color == theme::BORDER_SUBTLE)
            .map(|(x, _)| *x)
            .collect();
        // 보이는 열이 넷이면 선은 셋이다 — **마지막 열의 오른쪽 끝에는 긋지 않는다**
        assert_eq!(구분선, vec![100.0, 150.0, 230.0], "{lines:?}");

        // 끌고 있지 않으면 강조선(가이드)은 없다
        assert!(
            !lines.iter().any(|(_, color)| *color == theme::ACCENT),
            "끌지 않았는데 가이드가 그려졌다: {lines:?}"
        );
    }

    #[test]
    fn 소유자가_숫자_uid만_있어도_그대로_보인다() {
        // plan Edge Case — 서버가 이름을 안 주면 숫자가 곧 소유자다
        let entry = remote_entry("a.txt", None, Some("1000"));
        assert_eq!(cell_text(&entry, "텍스트", ColumnKind::Owner, true), "1000");
    }
}
