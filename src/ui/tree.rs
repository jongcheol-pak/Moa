//! 폴더 트리 — 계층 표시와 지연 확장 (FR-9).
//!
//! 드라이브 루트만 미리 나열하고, 노드를 **처음 펼칠 때** 그 폴더의 하위 1단계만
//! 워커 스레드로 열거한다 (part2 D5 — UI 무정지). egui `CollapsingHeader`의 본문 클로저는
//! 접힌 상태에서 호출되지 않으므로, 그 클로저가 곧 "처음 펼쳐진 순간"의 신호가 된다.
//!
//! 목록과의 동기화는 단방향이다 — 트리에서 폴더를 고르면 목록이 이동하지만,
//! 목록이 이동해도 트리를 펼치지 않는다 (D6).
//!
//! **로컬과 원격이 같은 트리를 쓴다**(T24) — 다른 것은 "무엇을 뿌리로 삼고 하위를 어디서
//! 읽는가"뿐이라, 화면 코드를 두 벌로 두면 들여쓰기·선택 강조·`읽는 중…` 같은 것이 곧
//! 어긋난다. 로컬은 이 모듈이 워커 스레드로 직접 읽고, 원격은 **읽어 달라는 요청을 값으로
//! 올려보낸다**(연결을 아는 것은 앱이다 — `remote::tree_cache`가 받아 둔다).
use crate::app::favorites::{FavoriteAction, FavoriteEntry};
use crate::fs::drives::DriveRow;
use crate::fs::enumerate::{EnumOutcome, FileEntry, enumerate_dir};
use crate::fs::icons::IconCache;
use crate::panel::file_list::{ListRow, SortKey, compare_entries};
use crate::remote::connection::ConnectionId;
use crate::remote::tree_cache::{TreeCache, TreeNode};
use crate::remote::types::RemotePath;
use crate::ui::icon_tex::IconTextures;
use crate::ui::menu::clamp_menu_pos;
use crate::ui::theme;
use crate::ui::widgets;
use eframe::egui;
use eframe::egui::collapsing_header::CollapsingState;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use windows::Win32::UI::Controls::HIMAGELIST;

/// 트리 고정 폭.
/// 폭 조절은 요구에 없다(FR-9는 표시 토글까지)
pub const TREE_WIDTH: f32 = 200.0;

/// 즐겨찾기 제목 글자 크기 — 항목보다 작고 흐리다(묶음 이름이지 항목이 아니다)
const FAVORITES_TITLE_PX: f32 = 11.0;

/// 트리 줄의 아이콘 한 변 — 목록의 작은 아이콘과 같은 16px (탐색기와 같은 눈높이)
const ROW_ICON: f32 = 16.0;
/// 아이콘과 글자 사이
const ROW_ICON_GAP: f32 = 4.0;

/// 이 트리가 보여 주는 것 (T24).
///
/// 원격일 때 하위 목록은 **앱이 가진 캐시**에서 읽는다 — 트리는 연결을 모르고, 캐시를
/// 채우는 것도 앱이다(트리는 "이 폴더를 읽어 달라"고 청하기만 한다)
pub enum TreeSource<'a> {
    Local,
    Remote {
        conn: ConnectionId,
        /// 트리의 뿌리 — 지금 보고 있는 원격 경로를 부모로 거슬러 올라간 최상단이다.
        /// 루트가 `/`가 아닌 서버도 있어 `/`로 못 박지 않는다 (plan Edge Case)
        root: RemotePath,
        cache: &'a TreeCache,
    },
}

/// 트리에서 고른 폴더 — 이동은 호출부(패널)가 한다
#[derive(Debug, Clone, PartialEq)]
pub enum TreeChoice {
    Local(PathBuf),
    Remote(RemotePath),
}

/// 하위를 읽어 달라는 청 (plan 신규 심볼 `TreeRequest`).
///
/// 로컬도 값으로 올려보낸다 — 실행은 패널이 곧바로 `start_local_load`로 되돌려주지만,
/// "펼침 → 요청"이라는 흐름이 두 소스에서 같은 모양이라야 한쪽만 고쳐지는 일이 없다
#[derive(Debug, Clone, PartialEq)]
pub enum TreeRequest {
    Local(PathBuf),
    Remote {
        conn: ConnectionId,
        path: RemotePath,
    },
}

/// 이번 프레임에 트리가 낸 것
#[derive(Debug, Default)]
pub struct TreeOutcome {
    pub chosen: Option<TreeChoice>,
    /// 이번 프레임에 처음 펼쳐진 폴더들 — 보통 0~1개다
    pub requests: Vec<TreeRequest>,
    /// 우클릭 메뉴에서 고른 즐겨찾기 조작 (FR-56) — 실제로 목록을 바꾸는 것은 앱이다.
    /// 트리가 직접 고치지 않는 이유는 `chosen`과 같다 — 그리는 도중에 상태를 바꾸면
    /// 같은 프레임의 다른 패널이 옛 목록을 보게 된다
    pub favorite: Option<FavoriteAction>,
}

/// 트리 우클릭 메뉴가 다루는 줄 — 어디서 열렸는지에 따라 메뉴 항목이 갈린다
#[derive(Debug, Clone, PartialEq, Eq)]
enum MenuTarget {
    /// 드라이브·하위 폴더 줄 — `즐겨찾기`가 뜬다(이미 담긴 폴더면 비활성)
    Node(PathBuf),
    /// 즐겨찾기 줄 — `해제`가 뜬다. 여기에 `즐겨찾기`를 함께 두지 않는 이유는
    /// 이미 담겨 있다는 것이 그 자리 자체로 자명하기 때문이다
    Favorite(PathBuf),
}

/// 즐겨찾기 줄을 끌어 차례를 바꾸는 중의 상태 (FR-56).
///
/// **사이드바의 워크스페이스 정렬과 같은 벌이다**(`ui::sidebar`) — 다만 좌표계와 줄 높이가
/// 달라 코드를 함께 쓰지 않는다. 세 번째 자리가 나오면 그때 합친다(AGENTS 3회 규약)
struct FavoriteDrag {
    /// 잡은 줄의 **화면 자리** — 기본 두 줄이 앞에 서 있어 저장 자리와 어긋난다
    from: usize,
    start: egui::Pos2,
    /// 임계를 넘어 실제 재정렬로 전환됐는가 — 넘기 전에는 단순 클릭일 수 있다
    active: bool,
}

/// 트리 메뉴 폭 — 원격 목록 메뉴와 같은 값으로 맞춘다.
/// 한 줄의 높이·여백·모서리는 여기서 정하지 않는다(`theme::MENU_ITEM_*`) —
/// 줄 그리기 자체를 `widgets::menu_row`가 맡는다
const MENU_WIDTH: f32 = 180.0;

/// 노드의 하위 폴더 상태. 열거 실패(접근 거부·삭제)도 빈 `Loaded`로 수렴한다 —
/// 트리에서 사용자에게 알릴 것이 없고, 화살표만 사라지면 충분하다
enum Node {
    /// 워커가 열거 중
    Loading,
    /// 하위 폴더 목록 (이름 자연 정렬).
    /// `Rc`인 이유: 그리기 중 자식을 순회하면서 자기 자신을 재귀 호출해야 해
    /// 매 프레임 목록을 복제하는 대신 참조만 늘린다
    Loaded(Rc<Vec<PathBuf>>),
}

/// 패널 하나가 소유하는 폴더 트리.
///
/// 확장 상태(열림/닫힘)는 egui가 위젯 id로 보관하고, 이 구조체는 **무엇을 읽었는지**만 갖는다
pub struct FolderTreeView {
    /// 펼친 적이 있는 폴더의 하위 목록. 한 번 읽으면 다시 읽지 않는다
    nodes: HashMap<PathBuf, Node>,
    /// 트리에서 마지막으로 고른 폴더 — 강조 표시용. 원격 트리도 이 자리를 쓴다
    selected: Option<TreeChoice>,
    tx: Sender<(PathBuf, Vec<PathBuf>)>,
    rx: Receiver<(PathBuf, Vec<PathBuf>)>,
    /// 우클릭 메뉴가 열린 자리와 그 대상 — `None`이면 닫혀 있다 (FR-56).
    ///
    /// **트리를 감출 때 패널이 `close_menu`로 비운다** — 트리가 그려지지 않는 프레임에는
    /// 이 안의 어떤 코드도 돌지 않아 스스로 닫을 수 없기 때문이다
    menu_at: Option<(egui::Pos2, MenuTarget)>,
    /// 메뉴를 **이번 프레임에 막 열었는가** — 그 프레임의 닫기 판정을 건너뛰는 데 쓴다.
    ///
    /// 메뉴는 우클릭을 받은 **그 프레임에** 그려지는데 그 프레임의 `any_click()`은 방금 그
    /// 우클릭이다. 그대로 판정하면 **자기를 연 클릭을 바깥 클릭으로 세어** 열리자마자 닫힌다
    /// (2026-08-17 사용자 보고 — 화면 가장자리라 메뉴가 안으로 당겨져 클릭 자리를 품지 못할 때 드러났다).
    /// 원격 목록 메뉴가 멀쩡한 것은 그쪽이 그리기가 끝난 뒤 자리를 세워 **다음 프레임부터** 그리기 때문이다
    menu_opened_this_frame: bool,
    /// 숨김 속성 폴더를 보일지 (FR-13) — 목록과 같은 값을 받는다.
    ///
    /// 트리만 따로 두면 같은 창에서 목록의 숨긴 폴더는 사라지는데 트리에는 남아,
    /// 설정이 반만 듣는 것처럼 보인다
    show_hidden: bool,
    /// 시스템 속성 폴더를 보일지 (FR-13) — 기본값을 `AppSettings`와 같이 `false`로 둔다
    /// (어긋나면 첫 프레임마다 하위 목록을 통째로 버리고 다시 읽는다)
    show_system: bool,
    /// 줄마다 쓸 셸 아이콘 인덱스 — **보이는 줄만** 조회하고 그 결과를 여기 담는다.
    ///
    /// `IconCache`도 경로별로 캐시하지만 그쪽은 프로세스 전체가 함께 쓰는 자리라,
    /// 트리가 매 프레임 다시 묻지 않으려면 이 맵이 먼저 답해야 한다
    icon_indices: HashMap<PathBuf, i32>,
    /// 즐겨찾기 줄을 끌어 차례를 바꾸는 중인가 (FR-56) — `None`이면 끌고 있지 않다
    favorite_drag: Option<FavoriteDrag>,
}

impl Default for FolderTreeView {
    fn default() -> FolderTreeView {
        FolderTreeView::new()
    }
}

impl FolderTreeView {
    pub fn new() -> FolderTreeView {
        let (tx, rx) = channel();
        FolderTreeView {
            icon_indices: HashMap::new(),
            nodes: HashMap::new(),
            selected: None,
            menu_at: None,
            menu_opened_this_frame: false,
            favorite_drag: None,
            tx,
            rx,
            show_hidden: true,
            show_system: false,
        }
    }

    /// 숨김·시스템 폴더 표시 여부를 받는다 (FR-13). **어느 한쪽이라도 바뀌었으면 `true`**.
    ///
    /// 바뀌면 읽어 둔 하위 목록을 **통째로 버린다** — 항목마다 속성을 쥐고 있지 않아
    /// 걸러진 것을 되돌릴 수 없다(목록이 폴더를 다시 읽는 것과 같은 이유). 펼침 상태도
    /// 함께 풀리지만, 설정을 바꾸는 일이 드물어 다시 읽는 편이 단순하다
    pub fn set_hidden_rules(&mut self, show_hidden: bool, show_system: bool) -> bool {
        if self.show_hidden == show_hidden && self.show_system == show_system {
            return false;
        }
        self.show_hidden = show_hidden;
        self.show_system = show_system;
        self.nodes.clear();
        true
    }

    /// 트리를 그리고, 고른 폴더와 새로 펼쳐진 폴더의 조회 요청을 돌려준다.
    /// 이동도 조회도 호출부의 몫이다 — 트리는 목록도 연결도 모른다.
    ///
    /// `favorites`는 **로컬 트리 맨 위에 설 바로가기들**이다 (FR-56) — 앱이 하나만 들고
    /// 모든 패널에 같은 것을 내려보낸다. 트리는 저장소 타입을 모르고 경로 목록만 본다.
    ///
    /// `drives`도 같은 모양이다 (T4) — 드라이브 줄과 그 연결 상태를 앱이 워커로 만들어
    /// 내려보낸다. 트리가 각자 조회하지 않는 이유는 패널마다 트리가 있어 조회가
    /// 되풀이되고, 연결 상태가 패널마다 갈리면 같은 드라이브에 X가 있는 트리와 없는
    /// 트리가 한 화면에 서기 때문이다. 원격 트리에는 쓰이지 않는다(빈 슬라이스여도 된다)
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        source: TreeSource<'_>,
        favorites: &[FavoriteEntry],
        drives: &[DriveRow],
        icons: &mut IconCache,
        textures: &mut IconTextures,
    ) -> TreeOutcome {
        self.poll();
        let mut outcome = TreeOutcome::default();
        let ctx = ui.ctx().clone();
        let himl = icons.himl();
        let mut row = RowCtx {
            textures,
            ctx: &ctx,
            himl,
        };
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| match source {
                TreeSource::Local => {
                    // 즐겨찾기는 **로컬 트리에만** 선다 (사용자 결정 — 원격은 제외)
                    self.show_favorites(ui, favorites, &mut outcome, icons, &mut row);
                    // 드라이브 줄은 **앱이 워커로 만들어 내려보낸다** — 트리는 패널마다
                    // 있어 각자 조회하면 셸·네트워크 왕복이 그만큼 되풀이된다 (T4)
                    for drive in drives {
                        self.show_node(
                            ui,
                            &drive.path,
                            &drive.label,
                            Some(drive),
                            &mut outcome,
                            icons,
                            &mut row,
                        );
                    }
                }
                TreeSource::Remote { conn, root, cache } => {
                    // 원격 트리에는 즐겨찾기가 없다 — 로컬에서 열어 둔 메뉴가 남아 있으면
                    // 여기서 비운다. 안 그러면 로컬 탭으로 돌아올 때 옛 메뉴가 되살아난다
                    self.menu_at = None;
                    self.menu_opened_this_frame = false;
                    // 원격은 서버 아이콘을 얻을 수 없다 — 모든 줄에 같은 폴더 아이콘을 쓴다
                    // (사용자 결정)
                    let remote = RemoteRowCtx {
                        conn,
                        cache,
                        folder: icons.dir_icon(),
                    };
                    self.show_remote_node(ui, &root, 0, &mut outcome, &mut row, &remote);
                }
            });
        // 메뉴는 **스크롤 영역 밖**에 그린다 — 안에 그리면 스크롤에 딸려가고 잘린다
        self.show_menu(ui, favorites, &mut outcome);
        outcome
    }

    /// 우클릭 메뉴를 닫는다 — **트리를 감출 때 패널이 부른다**.
    ///
    /// 트리가 그려지지 않는 프레임에는 `show`가 통째로 건너뛰어져 스스로 닫을 수 없다.
    /// 비우지 않으면 트리를 다시 켤 때 옛 메뉴가 그대로 떠 있다
    pub fn close_menu(&mut self) {
        self.menu_at = None;
        self.menu_opened_this_frame = false;
    }

    /// 우클릭 메뉴 한 장 (FR-56) — 대상이 트리 노드면 `즐겨찾기`, 즐겨찾기 줄이면 `해제`다.
    ///
    /// 원격 목록 메뉴(`ui::remote_menu`)와 **부품을 나누지 않는다**(plan 비추상화 선언) —
    /// 다루는 것도 항목도 달라서다. 다만 화면 밖으로 나가지 않게 당기는 계산만은 같은
    /// 함수(`ui::menu::clamp_menu_pos`)를 쓴다
    fn show_menu(
        &mut self,
        ui: &mut egui::Ui,
        favorites: &[FavoriteEntry],
        outcome: &mut TreeOutcome,
    ) {
        let Some((at, target)) = self.menu_at.clone() else {
            return;
        };
        // 이 메뉴는 한 줄뿐이라 줄 사이 간격이 없다
        let size = egui::vec2(MENU_WIDTH, theme::MENU_ITEM_HEIGHT)
            + crate::ui::menu::menu_frame_pad(ui.style());
        let viewport = ui.ctx().input(|input| input.viewport_rect());
        let at = clamp_menu_pos(viewport, at, size);
        let mut chosen = None;
        let response = egui::Area::new(ui.id().with("트리 메뉴"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ui.ctx(), |ui| {
                // 모서리는 적지 않는다 — `Frame::menu`가 테마의 공통 값을 읽는다
                // (`theme::MENU_CORNER_RADIUS`)
                egui::Frame::menu(ui.style())
                    .fill(theme::SURFACE_BG)
                    .stroke(egui::Stroke::new(
                        theme::MENU_FRAME_STROKE,
                        theme::PANE_BORDER,
                    ))
                    .show(ui, |ui| {
                        ui.set_width(MENU_WIDTH);
                        match &target {
                            MenuTarget::Node(path) => {
                                // 이미 담긴 폴더면 비활성 — 눌러도 되지 않는 것을 눌리게 두면
                                // 사용자는 눌렀다가 아무 일도 안 일어나는 것을 본다
                                let enabled = !favorites.iter().any(|e| &e.path == path);
                                if widgets::menu_row(ui, crate::i18n::tree_favorite_add(), enabled)
                                {
                                    chosen = Some(FavoriteAction::Add(path.clone()));
                                }
                            }
                            MenuTarget::Favorite(path) => {
                                if widgets::menu_row(ui, crate::i18n::tree_favorite_remove(), true)
                                {
                                    chosen = Some(FavoriteAction::Remove(path.clone()));
                                }
                            }
                        }
                    });
            })
            .response;

        // 바깥을 누르거나 Esc면 닫는다 — 메뉴가 화면에 눌어붙지 않게 한다.
        //
        // **막 연 프레임은 세지 않는다** — 그 프레임의 클릭은 이 메뉴를 연 우클릭 자신이라,
        // 메뉴가 클릭 자리를 품지 못하면(가장자리라 안으로 당겨진 경우) 곧바로 자기를 닫는다
        let just_opened = std::mem::take(&mut self.menu_opened_this_frame);
        let outside = !just_opened
            && ui.input(|input| {
                input.pointer.any_click()
                    && input
                        .pointer
                        .interact_pos()
                        .is_none_or(|pos| !response.rect.contains(pos))
            });
        let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
        if chosen.is_some() || outside || escape {
            self.menu_at = None;
        }
        outcome.favorite = chosen;
    }

    /// 즐겨찾기 줄들과 그 아래를 가르는 구분선 (FR-56).
    ///
    /// **비어 있으면 줄도 구분선도 그리지 않는다**(사용자 결정) — 쓰지 않는 사람의 화면에
    /// 빈 자리가 남지 않는다. 항목은 폴더 이름만 보이고 전체 경로는 툴팁이 든다 —
    /// 트리 폭이 200px이라 경로를 그대로 적으면 대부분 잘린다
    fn show_favorites(
        &mut self,
        ui: &mut egui::Ui,
        favorites: &[FavoriteEntry],
        outcome: &mut TreeOutcome,
        icons: &mut IconCache,
        row: &mut RowCtx<'_>,
    ) {
        if favorites.is_empty() {
            return;
        }
        // 목록 위 제목 — 흐린 작은 글씨다(사용자 결정). 폴더 구역에는 제목을 두지 않는다
        ui.label(
            egui::RichText::new(crate::i18n::tree_favorites_title())
                .size(FAVORITES_TITLE_PX)
                .color(theme::TEXT_MUTED),
        );
        // 놓일 자리를 계산하려면 줄이 어디에 그려졌는지 알아야 한다 — 그리면서 모은다
        let mut row_rects: Vec<egui::Rect> = Vec::with_capacity(favorites.len());
        // 사용자 항목이 하나뿐이면 끌어도 옮길 자리가 없다
        let draggable = favorites.iter().filter(|entry| entry.removable).count() > 1;

        for (index, entry) in favorites.iter().enumerate() {
            let path = &entry.path;
            let choice = TreeChoice::Local(path.clone());
            let is_selected = self.selected.as_ref() == Some(&choice);
            let icon = self.icon_for(path, icons);
            // 기본 항목은 셸 표시 이름(`바탕 화면`), 사용자 항목은 폴더명이다
            let label = match &entry.label {
                Some(label) => label.clone(),
                None => display_name(path),
            };
            // 펼침 화살표가 없는 줄이라 그 자리만큼 들여쓴다 — 하위 없는 트리 잎과 같은 자리다
            ui.horizontal(|ui| {
                ui.add_space(ui.spacing().indent);
                let response = tree_row(
                    ui,
                    row,
                    icon,
                    &label,
                    RowLook {
                        selected: is_selected,
                        dimmed: entry.missing,
                        // **기본 두 줄은 끌 수 없다** — 언제나 맨 위다 (FR-56)
                        draggable: draggable && entry.removable,
                        ..Default::default()
                    },
                )
                .on_hover_text(path.to_string_lossy());
                row_rects.push(response.rect);
                // **가리키는 폴더가 없으면 눌러도 이동하지 않는다** (FR-56) — 없는 곳으로
                // 옮기려다 실패하는 것보다 흐린 글씨로 「지금은 갈 수 없다」를 보이는 편이 낫다.
                // 줄 자체는 남겨 두어 사용자가 우클릭 `해제`로 직접 뺄 수 있다
                if response.clicked() && !entry.missing {
                    self.select(choice, outcome);
                }
                // 즐겨찾기 줄의 메뉴는 `해제` 하나다 (FR-56).
                // **기본 항목에는 메뉴 자체를 띄우지 않는다**(사용자 결정: 해제할 수 없음) —
                // 그 줄만 빼고 빈 상자를 띄우면 눌러도 아무 일이 없는 화면이 된다
                if entry.removable
                    && response.secondary_clicked()
                    && let Some(at) = response.interact_pointer_pos()
                {
                    self.menu_at = Some((at, MenuTarget::Favorite(path.clone())));
                    self.menu_opened_this_frame = true;
                }
                // 끌기 시작 — 임계를 넘기 전에는 아직 클릭일 수 있다
                if response.drag_started()
                    && let Some(at) = response.interact_pointer_pos()
                {
                    self.favorite_drag = Some(FavoriteDrag {
                        from: index,
                        start: at,
                        active: false,
                    });
                }
                if response.dragged()
                    && let (Some(drag), Some(at)) =
                        (self.favorite_drag.as_mut(), response.interact_pointer_pos())
                    && (at - drag.start).length() >= widgets::DRAG_THRESHOLD
                {
                    drag.active = true;
                }
            });
        }
        self.finish_favorite_drag(ui, favorites, &row_rects, outcome);
        ui.separator();
    }

    /// 끌던 즐겨찾기를 놓은 자리를 계산해 조작을 올린다. 끄는 중이면 놓일 자리에 선을 긋는다.
    ///
    /// **기본 두 줄은 놓을 자리에서 빠진다** (FR-56) — 그 둘은 언제나 맨 위이고, 사용자
    /// 항목을 그 위로 끌어도 맨 앞 사용자 자리에 놓인다
    fn finish_favorite_drag(
        &mut self,
        ui: &egui::Ui,
        favorites: &[FavoriteEntry],
        row_rects: &[egui::Rect],
        outcome: &mut TreeOutcome,
    ) {
        let Some(drag) = self.favorite_drag.as_ref() else {
            return;
        };
        if !drag.active {
            // 임계를 못 넘은 제스처 — 클릭으로 이미 처리됐다
            if ui.input(|input| !input.pointer.any_down()) {
                self.favorite_drag = None;
            }
            return;
        }
        let (Some(first_user), Some(from), Some(at)) = (
            favorites.iter().position(|entry| entry.removable),
            user_index(favorites, drag.from),
            ui.input(|input| input.pointer.interact_pos()),
        ) else {
            self.favorite_drag = None;
            return;
        };
        // 위 고리가 줄마다 하나씩 담으므로 `row_rects`는 `favorites`와 길이가 같고,
        // `first_user`는 그 `favorites`에서 찾은 자리라 언제나 범위 안이다
        let user_rects = &row_rects[first_user..];
        let insert_at = insert_index_at(at.y, user_rects);
        if let Some(line_y) = insert_line_y(insert_at, user_rects) {
            let full = ui.max_rect();
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(full.left(), line_y - widgets::INSERT_LINE_HEIGHT / 2.0),
                    egui::vec2(full.width(), widgets::INSERT_LINE_HEIGHT),
                ),
                0.0,
                theme::ACCENT,
            );
        }
        if ui.input(|input| !input.pointer.any_down()) {
            if let Some(to) = widgets::reorder_target(from, insert_at) {
                outcome.favorite = Some(FavoriteAction::Reorder { from, to });
            }
            self.favorite_drag = None;
        }
    }

    /// 이 경로에 쓸 셸 아이콘 인덱스 — 한 번 물으면 담아 두고 다시 묻지 않는다
    fn icon_for(&mut self, path: &Path, icons: &mut IconCache) -> i32 {
        if let Some(idx) = self.icon_indices.get(path) {
            return *idx;
        }
        let idx = icons.icon_index_for_path(&path.to_string_lossy());
        self.icon_indices.insert(path.to_path_buf(), idx);
        idx
    }

    /// 원격 노드 하나와 (펼쳐져 있으면) 그 하위를 그린다.
    ///
    /// 하위 목록은 캐시에서만 읽는다 — 없으면 요청을 올려보내고 이번 프레임에는 `읽는 중…`을
    /// 보인다. 조회가 도는 동안에도 이 트리는 계속 그려지므로 목록·다른 패널이 멈추지 않는다
    /// (Acceptance ③)
    fn show_remote_node(
        &mut self,
        ui: &mut egui::Ui,
        path: &RemotePath,
        depth: usize,
        outcome: &mut TreeOutcome,
        row: &mut RowCtx<'_>,
        remote: &RemoteRowCtx<'_>,
    ) {
        let RemoteRowCtx {
            conn,
            cache,
            folder,
        } = *remote;
        let choice = TreeChoice::Remote(path.clone());
        let is_selected = self.selected.as_ref() == Some(&choice);
        let label = remote_display_name(path);

        // 하위 폴더가 없다고 확인된 노드, 그리고 **상한까지 내려온 노드**는 펼침 화살표를
        // 그리지 않는다 — 후자는 순환 링크를 끊는 자리다 (plan Edge Case)
        if !can_expand(depth)
            || matches!(cache.node(conn, path), Some(TreeNode::Loaded(children)) if children.is_empty())
        {
            ui.horizontal(|ui| {
                ui.add_space(ui.spacing().indent);
                if tree_row(
                    ui,
                    row,
                    folder,
                    &label,
                    RowLook {
                        selected: is_selected,
                        ..Default::default()
                    },
                )
                .clicked()
                {
                    self.select(choice, outcome);
                }
            });
            return;
        }

        let id = ui.make_persistent_id(path.as_str());
        // 뿌리는 펼친 채로 시작한다 — 원격 트리를 켜자마자 접힌 줄 하나만 보이면
        // 한 번 더 눌러야 무엇이 있는지 알 수 있다(로컬은 드라이브가 여럿이라 접어 둔다)
        let header = CollapsingState::load_with_default_open(ui.ctx(), id, depth == 0).show_header(
            ui,
            |ui| {
                if tree_row(
                    ui,
                    row,
                    folder,
                    &label,
                    RowLook {
                        selected: is_selected,
                        ..Default::default()
                    },
                )
                .clicked()
                {
                    self.select(choice, outcome);
                }
            },
        );
        header.body(|ui| {
            match cache.node(conn, path) {
                Some(TreeNode::Loaded(children)) => {
                    // 자식 경로를 먼저 뜬다 — 아래 재귀가 `cache`를 다시 빌린다
                    let paths: Vec<RemotePath> = children
                        .iter()
                        .map(|entry| path.join(&entry.name))
                        .collect();
                    for child in paths {
                        self.show_remote_node(ui, &child, depth + 1, outcome, row, remote);
                    }
                }
                Some(TreeNode::Failed(detail)) => {
                    // 그 노드만 사유를 보이고 트리는 그대로 둔다 (plan Edge Case).
                    // 사유는 이미 완결된 문장이라 앞에 말을 덧붙이지 않는다(quality 리뷰 m1)
                    ui.colored_label(theme::ERROR_TEXT, detail);
                }
                Some(TreeNode::Loading) => {
                    ui.colored_label(theme::TEXT_MUTED, crate::i18n::tree_loading());
                }
                None => {
                    ui.colored_label(theme::TEXT_MUTED, crate::i18n::tree_loading());
                    if needs_children(cache, conn, path) {
                        outcome.requests.push(TreeRequest::Remote {
                            conn,
                            path: path.clone(),
                        });
                    }
                }
            }
        });
    }

    /// 하위 1단계를 워커 스레드로 읽는다 — 패널이 `TreeRequest::Local`을 받아 되돌려준다
    pub fn start_local_load(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.start_load(path, ctx);
    }

    /// 워커가 끝낸 열거 결과를 모두 반영한다
    fn poll(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok((path, children)) => {
                    self.nodes.insert(path, Node::Loaded(Rc::new(children)));
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }

    /// 노드 하나와 (펼쳐져 있으면) 그 하위를 그린다.
    ///
    /// **드라이브 뿌리와 하위 폴더가 같은 이 함수를 지난다** — `drive`가 그 둘을 가른다.
    /// 뿌리 호출은 `Some`이라 아이콘을 워커가 준 값에서 얻고, 하위 재귀 호출은 `None`이라
    /// 지금처럼 `icon_for`로 UI 스레드에서 얻는다(하위는 로컬 경로라 대개 빠르다).
    /// 연결 끊김 배지도 이 갈림에서 켜진다 — 그것을 그리는 것은 T5의 몫이다
    #[allow(clippy::too_many_arguments)]
    fn show_node(
        &mut self,
        ui: &mut egui::Ui,
        path: &Path,
        label: &str,
        drive: Option<&DriveRow>,
        outcome: &mut TreeOutcome,
        icons: &mut IconCache,
        row: &mut RowCtx<'_>,
    ) {
        let choice = TreeChoice::Local(path.to_path_buf());
        let is_selected = self.selected.as_ref() == Some(&choice);
        // 드라이브 뿌리는 워커가 준 아이콘과 연결 상태를 쓰고, 하위 폴더는 종전처럼
        // UI 스레드에서 아이콘을 얻고 배지도 없다 (T4·T5)
        let (icon, offline) = match drive {
            Some(drive) => (drive.icon, drive.offline),
            None => (self.icon_for(path, icons), false),
        };

        // 하위 폴더가 없다고 확인된 노드는 펼침 화살표를 그리지 않는다
        // (하위가 없으면 화살표를 그리지 않는다). 화살표 자리만큼 들여쓴다
        if matches!(self.nodes.get(path), Some(Node::Loaded(children)) if children.is_empty()) {
            ui.horizontal(|ui| {
                ui.add_space(ui.spacing().indent);
                let response = tree_row(
                    ui,
                    row,
                    icon,
                    label,
                    RowLook {
                        selected: is_selected,
                        offline,
                        ..Default::default()
                    },
                );
                if response.clicked() {
                    self.select(choice, outcome);
                }
                self.open_node_menu(&response, path);
            });
            return;
        }

        let id = ui.make_persistent_id(path);
        let header =
            CollapsingState::load_with_default_open(ui.ctx(), id, false).show_header(ui, |ui| {
                let response = tree_row(
                    ui,
                    row,
                    icon,
                    label,
                    RowLook {
                        selected: is_selected,
                        offline,
                        ..Default::default()
                    },
                );
                if response.clicked() {
                    self.select(choice, outcome);
                }
                self.open_node_menu(&response, path);
            });
        header.body(|ui| {
            // 이 클로저는 펼쳐졌을 때만 호출된다 — 여기가 지연 열거의 시작점이다
            let started = self.nodes.contains_key(path);
            let children = match self.nodes.get(path) {
                Some(Node::Loaded(children)) => Some(Rc::clone(children)),
                _ => None,
            };
            if !started {
                outcome
                    .requests
                    .push(TreeRequest::Local(path.to_path_buf()));
            }
            match children {
                Some(children) => {
                    for child in children.iter() {
                        // 하위는 셸 이름이 아니라 폴더명이다 — 탐색기도 그렇게 보인다
                        let child_label = display_name(child);
                        self.show_node(ui, child, &child_label, None, outcome, icons, row);
                    }
                }
                None => {
                    ui.colored_label(theme::TEXT_MUTED, crate::i18n::tree_loading());
                }
            }
        });
    }

    /// 트리 노드 줄에서 우클릭이 있었으면 그 자리에 메뉴를 연다 (FR-56).
    ///
    /// 잎 노드와 펼칠 수 있는 노드 두 자리에서 같은 판정을 하므로 한 곳에 둔다 —
    /// 어느 한쪽만 고쳐 메뉴가 안 열리는 줄이 생기지 않게 한다
    fn open_node_menu(&mut self, response: &egui::Response, path: &Path) {
        if response.secondary_clicked()
            && let Some(at) = response.interact_pointer_pos()
        {
            self.menu_at = Some((at, MenuTarget::Node(path.to_path_buf())));
            self.menu_opened_this_frame = true;
        }
    }

    fn select(&mut self, choice: TreeChoice, outcome: &mut TreeOutcome) {
        self.selected = Some(choice.clone());
        outcome.chosen = Some(choice);
    }

    /// 하위 1단계를 워커 스레드로 읽는다.
    /// UI 스레드에서 직접 읽으면 항목이 많은 폴더나 응답이 느린 네트워크 드라이브에서 창이 멈춘다
    fn start_load(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.nodes.insert(path.clone(), Node::Loading);
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let show_hidden = self.show_hidden;
        let show_system = self.show_system;
        std::thread::spawn(move || {
            let children = match enumerate_dir(&path) {
                EnumOutcome::Ok(entries) => child_dirs(&path, entries, show_hidden, show_system),
                // 접근 거부·삭제·오류 — 하위 없음으로 다룬다
                _ => Vec::new(),
            };
            // 수신부가 이미 사라졌으면(앱 종료) 전송 실패는 무해하다
            let _ = tx.send((path, children));
            ctx.request_repaint();
        });
    }
}

/// 원격 트리가 내려갈 수 있는 최대 깊이.
///
/// **끊지 않으면 순환 심볼릭 링크에서 끝없이 깊어진다**(plan Edge Case) — 서버 쪽 링크는
/// 목록만으로는 순환인지 알 수 없어(가리키는 곳을 따라가 봐야 안다) 깊이로 끊는다.
/// 훑기(`ConnCommand::ListTree`)가 쓰는 상한과 같은 자리의 판단이다
const MAX_TREE_DEPTH: usize = 40;

/// 이 깊이에서 더 내려가도 되는가 (plan Edge Case — 심볼릭 링크 순환)
fn can_expand(depth: usize) -> bool {
    depth < MAX_TREE_DEPTH
}

/// 이 폴더의 하위를 서버에 청해야 하는가 — 캐시가 아무것도 모를 때만이다.
///
/// 그리기에서 떼어 둔 이유는 **판정만 따로 시험하기 위해서**다 (egui의 접힘 상태는
/// 위젯 id에 묶여 있어 시험이 원하는 노드를 펼쳐 둘 수 없다)
fn needs_children(cache: &TreeCache, conn: ConnectionId, path: &RemotePath) -> bool {
    cache.node(conn, path).is_none()
}

/// 원격 노드에 보일 이름 — 뿌리(`/`)는 이름이 없어 경로 자체를 쓴다
fn remote_display_name(path: &RemotePath) -> String {
    path.file_name()
        .map(str::to_owned)
        .unwrap_or_else(|| path.as_str().to_owned())
}

/// 열거 결과에서 하위 폴더만 골라 이름 자연 정렬로 돌려준다.
/// 정렬은 목록과 같은 규칙을 쓴다 — 종류 인자는 이름 정렬에서 쓰이지 않아 빈 문자열을 넘긴다
fn child_dirs(
    parent: &Path,
    mut entries: Vec<FileEntry>,
    show_hidden: bool,
    show_system: bool,
) -> Vec<PathBuf> {
    // 속성마다 대응하는 설정을 본다 — 목록과 같은 규칙이다(`FileListView::shows`)
    entries
        .retain(|e| e.is_dir && (show_hidden || !e.is_hidden()) && (show_system || !e.is_system()));
    entries.sort_by(|a, b| compare_entries(a, "", b, "", SortKey::Name));
    entries
        .iter()
        .map(|e| parent.join(e.name_string()))
        .collect()
}

/// 메뉴 한 줄 — 눌렸으면 `true`. 비활성 줄은 흐리게 그리고 클릭을 받지 않는다.
///
/// 원격 목록 메뉴의 같은 이름 함수와 값·모양을 맞췄다(plan 4-D) — 부품을 공유하지 않는
/// 이유는 그 모듈 주석과 같다(대상·항목이 다르다)
/// 줄을 그리는 데 필요한 것들 — 세 값이 늘 함께 다녀 한 자리에 묶었다.
///
/// 트레이트가 아니라 평범한 구조체다(계획 비추상화 선언) — 갈아 끼울 구현이 없고,
/// 묶는 목적은 재귀 호출마다 같은 셋을 늘어놓지 않는 것뿐이다
struct RowCtx<'a> {
    textures: &'a mut IconTextures,
    ctx: &'a egui::Context,
    himl: HIMAGELIST,
}

/// 원격 노드를 그리는 동안 **재귀 내내 바뀌지 않는 것들**.
///
/// `RowCtx`에 얹지 않는 이유는 그쪽을 로컬 경로도 쓰기 때문이다 — 원격 전용 필드를 더하면
/// 로컬 호출마다 채울 것이 없는 자리가 생긴다
#[derive(Clone, Copy)]
struct RemoteRowCtx<'a> {
    conn: ConnectionId,
    cache: &'a TreeCache,
    /// 원격은 서버 아이콘을 얻을 수 없어 모든 줄이 같은 폴더 아이콘을 쓴다
    folder: i32,
}

/// 트리 줄 하나 — 아이콘과 라벨을 나란히 그리고 그 줄 전체의 반응을 돌려준다.
///
/// 아이콘을 `selectable_label` **밖에** 두면 강조 배경이 라벨에만 깔려 탐색기와 달라 보인다.
/// 그래서 줄 전체를 한 번에 잡고(`allocate_at_least`) 배경·아이콘·글자를 직접 그린다
/// 줄 하나의 표시·조작 성질.
///
/// **불리언을 인자로 늘어놓지 않는다** — 넷이 되면 호출부에서 자리가 뒤바뀌어도
/// 컴파일이 통과해 화면에서야 드러난다
#[derive(Clone, Copy, Default)]
struct RowLook {
    selected: bool,
    /// 연결 끊김 배지를 아이콘 자리에 겹친다 (FR-9)
    offline: bool,
    /// 글자를 흐리게 — 지금 갈 수 없는 줄이다 (FR-56의 사라진 즐겨찾기)
    dimmed: bool,
    /// 끌어서 차례를 바꿀 수 있는 줄인가 (FR-56).
    ///
    /// **이 줄만 드래그를 감지한다** — 폴더·드라이브 줄까지 함께 감지하게 하면 살짝
    /// 흔들린 클릭이 이동으로 처리되지 않는 회귀가 그 줄들에도 번진다
    draggable: bool,
}

fn tree_row(
    ui: &mut egui::Ui,
    row: &mut RowCtx<'_>,
    icon: i32,
    label: &str,
    look: RowLook,
) -> egui::Response {
    let RowLook {
        selected,
        offline,
        dimmed,
        draggable,
    } = look;
    let height = ui.spacing().interact_size.y.max(ROW_ICON);
    let sense = if draggable {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::click()
    };
    let (rect, response) = ui.allocate_at_least(egui::vec2(ui.available_width(), height), sense);
    if selected || response.hovered() {
        let fill = if selected {
            ui.visuals().selection.bg_fill
        } else {
            ui.visuals().widgets.hovered.bg_fill
        };
        ui.painter().rect_filled(rect, 2.0, fill);
    }
    // 키보드 포커스도 눈에 보여야 한다 — `Sense::click()`이 Tab 이동과 Space·Enter 활성화를
    // 함께 주는데(egui `sense.rs`), 표식이 없으면 지금 어느 줄에 있는지 알 수 없다.
    // 종전 `selectable_label`은 egui가 포커스 테두리를 그려 주던 자리다
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            2.0,
            ui.visuals().selection.stroke,
            egui::StrokeKind::Inside,
        );
    }
    // 아이콘 자리는 텍스처가 있든 없든 같다 — 배지가 그 자리에 겹치므로 밖에서 정한다
    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.center().y - ROW_ICON / 2.0),
        egui::vec2(ROW_ICON, ROW_ICON),
    );
    if let Some(tex) = row.textures.get(row.ctx, row.himl, icon) {
        // painter를 다시 얻는다 — textures가 ui를 빌리는 사이 앞의 painter가 무효화된다
        ui.painter().image(
            tex.id(),
            icon_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
    // 연결 끊김 배지는 **아이콘 자리 안에** 겹친다 (FR-9) — 자리를 따로 계산하면 어긋난다.
    //
    // **텍스처와 묶지 않는다**: 아이콘 변환은 실패할 수 있고 `IconTextures`가 몇 번 다시
    // 시도하지만 **늘 실패하는 자리는 끝내 포기한다** — 묶어 두면 그 드라이브는 아이콘도
    // 배지도 없어 **끊긴 것을 화면으로 알 수 없다**(이 기능의 목적이 통째로 무력해진다).
    // 텍스처는 대개 몇 프레임 안에 올라오므로 배지만 보이는 프레임은 스쳐 지나가고,
    // 끝내 실패한 자리에서는 아무것도 없는 것보다 배지만이라도 보이는 편이 낫다
    // (2026-08-17 판정 — plan T5 Edge)
    if offline {
        draw_offline_badge(ui.painter(), icon_rect);
    }
    let text_color = if selected {
        ui.visuals().selection.stroke.color
    } else if dimmed {
        // 새 색을 만들지 않는다 — 목록의 잘라내기·숨김 항목이 쓰는 흐린 토큰 그대로다
        theme::TEXT_DIM
    } else {
        ui.visuals().text_color()
    };
    ui.painter().text(
        egui::pos2(rect.left() + ROW_ICON + ROW_ICON_GAP, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::TextStyle::Body.resolve(ui.style()),
        text_color,
    );
    response
}

/// 화면 자리를 **사용자 항목 자리**로 옮긴다 — 기본 두 줄이 앞에 서 있어 둘이 어긋난다.
/// 기본 줄이면 `None`이다(끌 수 없는 줄이라 옮길 자리도 없다)
fn user_index(favorites: &[FavoriteEntry], row: usize) -> Option<usize> {
    if !favorites.get(row)?.removable {
        return None;
    }
    Some(
        favorites[..row]
            .iter()
            .filter(|entry| entry.removable)
            .count(),
    )
}

/// 마우스 y가 줄들 사이 어디에 놓이는가 — `0..=rects.len()`.
/// 줄 가운데를 지났으면 그 줄 아래로 본다
fn insert_index_at(y: f32, rects: &[egui::Rect]) -> usize {
    rects.iter().filter(|rect| y > rect.center().y).count()
}

/// 그 삽입 자리에 그을 가로선의 y — 줄이 하나도 없으면 `None`
fn insert_line_y(insert_at: usize, rects: &[egui::Rect]) -> Option<f32> {
    match insert_at {
        0 => Some(rects.first()?.top()),
        _ => Some(rects.get(insert_at - 1)?.bottom()),
    }
}

/// 배지 지름 — 아이콘 한 변에 대한 비율.
///
/// 탐색기 스크린샷 실측: 배지 높이 10px ÷ 아이콘 높이 18px. 그 상자는 23×18로 정사각이
/// 아니었는데(드라이브 아이콘이 가로로 넓다) 우리 `icon_rect`는 16×16이라 **세로 축**을 썼다
const BADGE_RATIO: f32 = 0.55;
/// X 선 굵기 — 배지 지름에 대한 비율. 배지가 9px 남짓이라 스크린샷에서 정밀히 잴 수 없어
/// 어림한 값이다(화면 확인으로 판정한다)
const BADGE_STROKE_RATIO: f32 = 0.15;
/// X가 차지하는 **사각 영역의 한 변** — 배지 지름에 대한 비율(중심에서 양쪽 절반씩).
/// 대각선 하나의 실제 길이는 이 값의 √2배다. 원 안쪽에 여백이 남는다
const BADGE_MARK_RATIO: f32 = 0.5;

/// 연결 끊김 배지 — 아이콘 오른쪽 아래에 겹치는 **채워진 빨간 원과 흰 X** (FR-9).
///
/// 셸에 이 오버레이를 물어보는 길이 없어 직접 그린다 — `SHGFI_OVERLAYINDEX`·
/// `SHGFI_ADDOVERLAYS`를 경로와 PIDL 두 경로로 물어도 끊긴 드라이브의 오버레이 인덱스가
/// 전부 0으로 왔다(2026-08-17 실측). 아이콘 글꼴을 쓰지 않는 이유는 phosphor regular의
/// `X_CIRCLE`이 윤곽선이라 이 크기에서 탐색기의 채워진 배지와 다르게 읽히기 때문이다
fn draw_offline_badge(painter: &egui::Painter, icon_rect: egui::Rect) {
    let diameter = icon_rect.height() * BADGE_RATIO;
    let radius = diameter / 2.0;
    // 배지의 오른쪽·아래 끝을 아이콘의 오른쪽·아래 끝에 맞춘다 — 아이콘 **안쪽**에
    // 들어가고 밖으로 넘치지 않는다(탐색기 실측: 두 오른쪽 끝이 같은 자리다)
    let center = egui::pos2(icon_rect.right() - radius, icon_rect.bottom() - radius);
    painter.circle_filled(center, radius, theme::OFFLINE_BADGE);
    let arm = diameter * BADGE_MARK_RATIO / 2.0;
    let stroke = egui::Stroke::new(diameter * BADGE_STROKE_RATIO, egui::Color32::WHITE);
    painter.line_segment(
        [
            egui::pos2(center.x - arm, center.y - arm),
            egui::pos2(center.x + arm, center.y + arm),
        ],
        stroke,
    );
    painter.line_segment(
        [
            egui::pos2(center.x - arm, center.y + arm),
            egui::pos2(center.x + arm, center.y - arm),
        ],
        stroke,
    );
}

/// 트리에 보일 이름 — 드라이브 루트는 이름이 없어 경로 자체(`C:\`)를 쓴다
fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::types::RemoteEntry;

    /// 시험용 드라이브 줄 — 아이콘 인덱스는 셸이 준 실값을 쓴다(화면과 같은 값으로 그린다)
    fn offline_drive(icons: &mut IconCache) -> DriveRow {
        offline_drive_with_icon(icons.icon_index_for_path(r"C:\"))
    }

    /// 아이콘 인덱스를 손으로 지정한 시험용 드라이브 줄 — **없는 인덱스**를 주어
    /// 텍스처 변환이 실패하는 상황을 만드는 데 쓴다
    fn offline_drive_with_icon(icon: i32) -> DriveRow {
        DriveRow {
            path: PathBuf::from(r"C:\"),
            label: "드라이브".to_owned(),
            icon,
            network: true,
            offline: true,
        }
    }

    /// 그려진 셰이프에서 **아이콘 수**를 센다 — 아이콘은 텍스처를 입힌 사각형이라 `Mesh`다.
    /// 배지 시험이 "텍스처가 정말 없었는가"를 가리는 데 쓴다
    fn count_icons(output: &egui::FullOutput) -> usize {
        fn count(shape: &egui::Shape, found: &mut usize) {
            match shape {
                egui::Shape::Mesh(_) => *found += 1,
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        count(shape, found);
                    }
                }
                _ => {}
            }
        }
        let mut found = 0;
        for clipped in &output.shapes {
            count(&clipped.shape, &mut found);
        }
        found
    }

    /// 그려진 셰이프에서 연결 끊김 배지 수를 센다 — 배지는 글자가 아니라 채워진 원이다
    fn count_badges(output: &egui::FullOutput) -> usize {
        fn count(shape: &egui::Shape, found: &mut usize) {
            match shape {
                egui::Shape::Circle(circle) if circle.fill == theme::OFFLINE_BADGE => *found += 1,
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        count(shape, found);
                    }
                }
                _ => {}
            }
        }
        let mut found = 0;
        for clipped in &output.shapes {
            count(&clipped.shape, &mut found);
        }
        found
    }

    /// `show_node`를 직접 그려 배지 수를 돌려준다 (T5).
    ///
    /// **`show`를 거치지 않는 이유**: 드라이브 뿌리는 기본 접힌 상태로 그려지므로
    /// (`CollapsingState::load_with_default_open(.., false)`) 화면을 몇 프레임 돌려도
    /// **하위 폴더 재귀 호출부를 지나지 않는다**. 그 갈래(`drive: None`)에서 배지가
    /// 꺼지는지 보려면 이렇게 직접 부르는 수밖에 없다 (2026-08-17 리뷰 지적)
    fn badges_of_node(drive: Option<&DriveRow>, icons: &mut IconCache) -> (usize, usize) {
        let ctx = egui::Context::default();
        let mut view = FolderTreeView::new();
        let mut textures = IconTextures::inline();
        let himl = icons.himl();
        let mut counted = (0, 0);
        // 한 프레임이면 충분하다 — 배지는 아이콘 텍스처와 묶여 있지 않다(`tree_row`).
        // 그래도 몇 프레임 돌리는 것은 첫 프레임에 글꼴·레이아웃이 준비되는 egui의
        // 성질에 기대지 않으려는 것이다
        for _ in 0..8 {
            let output = ctx.run_ui(Default::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    textures.begin_frame();
                    let inner = ui.ctx().clone();
                    let mut row = RowCtx {
                        textures: &mut textures,
                        ctx: &inner,
                        himl,
                    };
                    let mut outcome = TreeOutcome::default();
                    view.show_node(
                        ui,
                        Path::new(r"C:\"),
                        "드라이브",
                        drive,
                        &mut outcome,
                        icons,
                        &mut row,
                    );
                });
            });
            counted = (count_badges(&output), count_icons(&output));
            // 아이콘까지 올라온 프레임을 기다린다 — 배지만 보고 끝내면 "텍스처가 없었다"와
            // "아직 안 올라왔다"를 가릴 수 없다
            if counted.1 > 0 {
                break;
            }
        }
        counted
    }

    /// 즐겨찾기 한 줄 — 화면 자리 계산만 보는 시험이 쓴다
    fn 즐겨찾기(path: &str, removable: bool) -> FavoriteEntry {
        FavoriteEntry {
            path: PathBuf::from(path),
            label: None,
            removable,
            missing: false,
        }
    }

    /// 세로로 20px씩 쌓인 줄 자리들
    fn 줄자리(count: usize) -> Vec<egui::Rect> {
        (0..count)
            .map(|index| {
                egui::Rect::from_min_size(
                    egui::pos2(0.0, index as f32 * 20.0),
                    egui::vec2(100.0, 20.0),
                )
            })
            .collect()
    }

    #[test]
    fn 화면_자리를_사용자_항목_자리로_옮긴다() {
        // FR-56 — 기본 두 줄이 앞에 서 있어 화면 자리와 저장 자리가 어긋난다
        let favorites = [
            즐겨찾기(r"C:\Desktop", false),
            즐겨찾기(r"C:\Downloads", false),
            즐겨찾기(r"D:\하나", true),
            즐겨찾기(r"D:\둘", true),
        ];
        assert_eq!(user_index(&favorites, 0), None, "기본 줄은 끌 수 없다");
        assert_eq!(user_index(&favorites, 1), None);
        assert_eq!(user_index(&favorites, 2), Some(0));
        assert_eq!(user_index(&favorites, 3), Some(1));
        assert_eq!(user_index(&favorites, 9), None, "없는 줄");
    }

    #[test]
    fn 줄_가운데를_지나면_그_아래로_놓는다() {
        let rects = 줄자리(3);
        assert_eq!(insert_index_at(-5.0, &rects), 0, "맨 위보다 위");
        assert_eq!(insert_index_at(5.0, &rects), 0, "첫 줄 위쪽 절반");
        assert_eq!(insert_index_at(15.0, &rects), 1, "첫 줄 아래쪽 절반");
        assert_eq!(insert_index_at(500.0, &rects), 3, "맨 아래보다 아래");
    }

    #[test]
    fn 놓일_자리_선은_그_줄의_경계에_선다() {
        let rects = 줄자리(3);
        assert_eq!(insert_line_y(0, &rects), Some(0.0));
        assert_eq!(insert_line_y(1, &rects), Some(20.0));
        assert_eq!(insert_line_y(3, &rects), Some(60.0));
        assert_eq!(insert_line_y(0, &[]), None, "줄이 없으면 그을 것도 없다");
    }

    #[test]
    fn 드라이브_갈래는_끊긴_상태를_배지로_그린다() {
        // 이 시험이 짝의 기준선이다 — 하네스가 배지를 그릴 수 있음을 먼저 입증한다.
        // 그러지 않으면 아래 시험의 `0`이 "갈래가 꺼져서"인지 "아이콘이 안 올라와서"인지
        // 가릴 수 없다
        let mut icons = IconCache::new();
        let drive = offline_drive(&mut icons);
        assert_eq!(
            badges_of_node(Some(&drive), &mut icons).0,
            1,
            "끊긴 드라이브 갈래에 배지가 그려지지 않았다"
        );
    }

    #[test]
    fn 아이콘_텍스처가_없어도_배지는_그려진다() {
        // **이 시험이 배지를 텍스처와 분리한 이유 그 자체다** (2026-08-17 리뷰 지적).
        // `IconTextures`는 변환에 실패한 인덱스를 몇 번 다시 시도하지만 **늘 실패하는
        // 자리는 끝내 포기한다** — 배지를 텍스처 성공에 묶어 두면 그 드라이브는 아이콘도
        // 배지도 없어 **끊긴 것을 화면으로 알 수 없다**. 없는 인덱스로 그 상황을 만들어
        // 확인한다. 이 시험이 없으면 분리가 실수로 되돌아가도 `cargo test`가 잡지 못한다
        let mut icons = IconCache::new();
        let drive = offline_drive_with_icon(i32::MAX);
        let (배지, 아이콘) = badges_of_node(Some(&drive), &mut icons);
        assert_eq!(
            아이콘, 0,
            "없는 인덱스인데 텍스처가 만들어졌다 — 이 시험이 겨냥한 상황이 아니다"
        );
        assert_eq!(
            배지, 1,
            "텍스처가 없는 줄에서 배지가 사라졌다 — 배지가 다시 텍스처에 묶였다"
        );
    }

    #[test]
    fn 하위_폴더_갈래에는_배지가_꺼진다() {
        // T5 Acceptance — 하위 폴더는 드라이브 줄과 **같은 `show_node`**를 지나므로,
        // 그 자리에서 `false`가 흐르는지 본다. 위 시험이 같은 하네스로 배지 1을 관측하므로
        // 여기의 0은 갈래가 꺼진 결과다
        let mut icons = IconCache::new();
        assert_eq!(
            badges_of_node(None, &mut icons).0,
            0,
            "하위 폴더 갈래에 배지가 그려졌다"
        );
    }

    fn entry(name: &str, is_dir: bool) -> FileEntry {
        let mut n: Vec<u16> = name.encode_utf16().collect();
        n.push(0);
        FileEntry {
            name: n,
            is_dir,
            size: 0,
            modified: 0,
            attributes: 0,
        }
    }

    #[test]
    fn 하위_폴더만_남기고_파일은_거른다() {
        let entries = vec![
            entry("문서.txt", false),
            entry("사진", true),
            entry("설치.exe", false),
        ];
        let children = child_dirs(Path::new(r"C:\Users"), entries, true, true);
        assert_eq!(children, vec![PathBuf::from(r"C:\Users\사진")]);
    }

    #[test]
    fn 하위_폴더는_자연_정렬된다() {
        // "폴더10"이 "폴더2"보다 뒤에 와야 한다 (사전식이면 앞에 온다)
        let entries = vec![entry("폴더10", true), entry("폴더2", true)];
        let children = child_dirs(Path::new(r"D:\"), entries, true, true);
        assert_eq!(
            children,
            vec![PathBuf::from(r"D:\폴더2"), PathBuf::from(r"D:\폴더10")]
        );
    }

    #[test]
    fn 트리도_숨김과_시스템을_각자의_토글로_거른다() {
        // 목록에서만 사라지면 같은 창의 두 곳이 다르게 보인다 (FR-13)
        use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM};
        let mut 숨김 = entry("숨긴폴더", true);
        숨김.attributes = FILE_ATTRIBUTE_HIDDEN.0;
        let mut 시스템 = entry("System Volume Information", true);
        시스템.attributes = FILE_ATTRIBUTE_SYSTEM.0;
        let 항목 = || vec![entry("보통", true), 숨김.clone(), 시스템.clone()];

        let 켬 = child_dirs(Path::new(r"D:\"), 항목(), true, true);
        assert_eq!(켬.len(), 3, "둘 다 켜져 있으면 전부 보여야 한다");

        // 시스템 폴더는 **시스템 토글로** 판정된다 — 숨김 토글에 묶여 있으면 안 된다
        assert_eq!(
            child_dirs(Path::new(r"D:\"), 항목(), true, false),
            vec![PathBuf::from(r"D:\보통"), PathBuf::from(r"D:\숨긴폴더")]
        );
        assert_eq!(
            child_dirs(Path::new(r"D:\"), 항목(), false, true),
            vec![
                PathBuf::from(r"D:\System Volume Information"),
                PathBuf::from(r"D:\보통")
            ]
        );
        assert_eq!(
            child_dirs(Path::new(r"D:\"), 항목(), false, false),
            vec![PathBuf::from(r"D:\보통")]
        );
    }

    #[test]
    fn 트리_설정이_바뀌면_읽어_둔_하위를_버린다() {
        let mut tree = FolderTreeView::new();
        assert!(
            !tree.set_hidden_rules(true, false),
            "기본값과 같은데 바뀌었다고 한다"
        );

        // 숨김만 바뀌어도 캐시를 버린다
        tree.nodes
            .insert(PathBuf::from(r"D:\"), Node::Loaded(Rc::new(Vec::new())));
        assert!(
            tree.set_hidden_rules(false, false),
            "숨김이 바뀌었는데 알리지 않았다"
        );
        assert!(
            tree.nodes.is_empty(),
            "걸러진 것을 되돌릴 수 없는데 캐시가 남았다"
        );

        // 시스템만 바뀌어도 마찬가지다 — 한쪽만 보면 그 토글이 트리에 듣지 않는다
        tree.nodes
            .insert(PathBuf::from(r"D:\"), Node::Loaded(Rc::new(Vec::new())));
        assert!(
            tree.set_hidden_rules(false, true),
            "시스템이 바뀌었는데 알리지 않았다"
        );
        assert!(tree.nodes.is_empty(), "시스템 토글에는 캐시가 남았다");
    }

    #[test]
    fn 표시_이름은_폴더명이고_드라이브는_경로다() {
        assert_eq!(display_name(Path::new(r"C:\Users\jongc")), "jongc");
        assert_eq!(display_name(Path::new(r"C:\")), r"C:\");
    }

    /// 원격 트리를 한 프레임 그리고 결과를 돌려준다
    fn draw_remote(cache: &TreeCache, root: &str) -> TreeOutcome {
        let ctx = egui::Context::default();
        let mut view = FolderTreeView::new();
        let mut icons = IconCache::new();
        let mut textures = IconTextures::inline();
        let mut outcome = TreeOutcome::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                textures.begin_frame();
                outcome = view.show(
                    ui,
                    TreeSource::Remote {
                        conn: ConnectionId(1),
                        root: RemotePath::new(root),
                        cache,
                    },
                    // 원격 트리에는 즐겨찾기가 서지 않는다 — 그래도 인자는 받는다
                    &[FavoriteEntry {
                        path: PathBuf::from(r"D:\작업"),
                        label: None,
                        removable: true,
                        missing: false,
                    }],
                    // 드라이브 줄도 로컬 트리의 것이라 원격에는 서지 않는다
                    &[],
                    &mut icons,
                    &mut textures,
                );
            });
        });
        outcome
    }

    #[test]
    fn 처음_펼친_원격_폴더만_조회를_청한다() {
        // Acceptance ②의 화면 절반 — 캐시가 답을 들고 있으면 청하지 않는다.
        // (같은 폴더를 두 번 펼쳐도 서버에 한 번만 가는 것은 `TreeCache::begin`이 막는다)
        let mut cache = TreeCache::new();
        let conn = ConnectionId(1);
        let root = RemotePath::new("/");

        // 뿌리는 펼친 채로 시작하므로 첫 프레임에 곧바로 하위를 청한다
        let first = draw_remote(&cache, "/");
        assert_eq!(
            first.requests,
            vec![TreeRequest::Remote {
                conn,
                path: root.clone()
            }],
            "펼쳐진 뿌리가 하위를 청하지 않았다"
        );

        // 답이 담기면 화면은 그것을 그린다 — 이때는 새 요청이 없다
        cache.begin(conn, &root);
        cache.fill(
            conn,
            0,
            &root,
            vec![RemoteEntry {
                name: "var".to_owned(),
                is_dir: true,
                is_symlink: false,
                link_target: None,
                size: 0,
                modified: None,
                mode: None,
                owner: None,
            }],
        );
        let loaded = draw_remote(&cache, "/");
        assert!(loaded.requests.is_empty(), "이미 읽은 것을 또 청했다");
        assert!(loaded.chosen.is_none());
    }

    #[test]
    fn 원격_노드_이름은_마지막_조각이고_뿌리는_경로다() {
        assert_eq!(remote_display_name(&RemotePath::new("/var/www")), "www");
        assert_eq!(remote_display_name(&RemotePath::new("/")), "/");
    }

    #[test]
    fn 형제_노드가_함께_펼쳐지면_각자_조회를_청한다() {
        // quality 리뷰 M1 — 한 프레임에 여럿이 펼쳐지면 요청도 여럿이다. 하나로 압착하면
        // 나머지가 그 프레임에서 버려져 노드마다 프레임이 하나씩 밀린다.
        // (egui의 접힘 상태는 위젯 id에 묶여 있어 시험이 임의 노드를 펼칠 수 없다 —
        //  그래서 그리기에서 떼어 둔 판정을 직접 확인한다)
        let mut cache = TreeCache::new();
        let conn = ConnectionId(1);
        let siblings = [RemotePath::new("/var"), RemotePath::new("/etc")];
        for path in &siblings {
            assert!(needs_children(&cache, conn, path), "{path:?}");
        }
        // 청하기 시작한 것은 다시 청하지 않는다
        cache.begin(conn, &siblings[0]);
        assert!(!needs_children(&cache, conn, &siblings[0]));
        assert!(
            needs_children(&cache, conn, &siblings[1]),
            "형제가 함께 막혔다"
        );
    }

    #[test]
    fn 상한까지_내려가면_더_펼치지_않는다() {
        // plan Edge Case — 순환 심볼릭 링크는 목록만으로 알 수 없어 깊이로 끊는다
        assert!(can_expand(0));
        assert!(can_expand(MAX_TREE_DEPTH - 1));
        assert!(!can_expand(MAX_TREE_DEPTH));
        assert!(!can_expand(MAX_TREE_DEPTH + 1));
    }
}
