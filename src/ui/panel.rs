//! 패널 — 탭 스트립 / 주소창 / 파일 목록을 담는 탐색 단위 (FR-3).
//!
//! 패널은 자기 탐색 상태(탭·히스토리·목록·열거)를 온전히 소유하며 서로를 모른다.
//! 아이콘 캐시·셸 호스트처럼 앱 전역에서 하나면 충분한 것은 `show`가 인자로 받는다.
//!
//! 탐색은 **pending-커밋** 모델이다: 열거가 성공했을 때만 경로·히스토리를 커밋한다.
//! 실패(삭제·권한)하면 사유만 표시하고 현 위치·목록을 그대로 둔다.
//!
//! 워커 스레드에 맡기는 일(열거·만들기)은 `workers`가, 테스트는 `tests`가 든다.
use crate::app::favorites::{FavoriteAction, FavoriteEntry};
use crate::app::layout::PanelId;
use crate::app::workspace::WorkspaceId;
use crate::fs::create;
use crate::fs::drives::DriveRow;
use crate::fs::enumerate::{EnumChunk, EnumOutcome, FileEntry};
use crate::fs::icons::IconCache;
use crate::fs::thumbnail::ThumbnailCache;
use crate::fs::watcher::DirWatcher;
use crate::panel::dir_cache::DirCache;
use crate::panel::file_list::{ListRow, PARENT_ENTRY, SortKey};
use crate::panel::history::History;
use crate::panel::tabs::{CloseOutcome, TabId, TabPhase, TabSource, TabState, TabsModel};
use crate::remote::connection::{ConnCommand, ConnectionId, ListSource};
use crate::remote::manager::ConnectionManager;
use crate::remote::types::{RemoteEntry, RemotePath, SiteId};
use crate::remote::url::RemoteUrl;
use crate::ui::address_bar::{AddressBar, NavAction};
use crate::ui::drag_preview;
use crate::ui::file_list::{FileListAction, FileListView};
use crate::ui::icon_tex::{IconTextures, ThumbnailTextures};
use crate::ui::list_common::{DropOutcome, DropTarget, FileDrag};
use crate::ui::menu::{Command, PanelMenuState, clamp_menu_pos};
use crate::ui::panel::workers::{CreateOp, DirLoad};
use crate::ui::remote_menu::{self, RemoteMenuAction, RemoteTarget};
use crate::ui::remote_states::{self, FailedAction, RemoteView};
use crate::ui::session::PanelTabs;
use crate::ui::session::TabSpec;
use crate::ui::shell_host;
use crate::ui::tabs::TabAction;
use crate::ui::tabs::TransferTargets;
use crate::ui::theme;
use crate::ui::tree::{FolderTreeView, TREE_WIDTH, TreeChoice, TreeRequest, TreeSource};
use crate::ui::view_mode::ViewMode;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

mod workers;

#[cfg(test)]
mod tests;

/// 트리 토글 아이콘 — 로컬·원격이 같은 표식을 쓰고, 무엇의 트리인지는 툴팁이 말한다.
///
/// 문구(`폴더 트리`·`원격 트리`)를 그대로 두면 좁은 패널에서 상태 줄의 절반을 먹는다
const TREE_TOGGLE_ICON: &str = egui_phosphor::regular::TREE_VIEW;

/// 트리와 목록을 가르는 세로 선 두께 — 현행 판 트리의 테두리(`WS_EX_CLIENTEDGE`)를 대신한다
const TREE_BORDER: f32 = 1.0;

/// 트리 영역 안쪽 여백 — 항목이 패널 가장자리에 붙지 않게 한다
const TREE_PAD: f32 = 4.0;

/// 항목 수 표기와 패널 오른쪽 경계 사이 여백 — 경계선에 글자가 붙어 보이지 않게 한다
const COUNT_RIGHT_PAD: f32 = 6.0;

/// 썸네일 도착을 확인하러 스스로 깨어나는 간격 (FR-24).
///
/// 매 프레임 깨우면 만드는 동안 앱이 쉬지 않고 그려 배터리를 먹고, 너무 길면 사진이
/// 뒤늦게 뜬다. 20장 남짓이 수백 ms 안에 만들어지므로 그 사이 몇 번 확인하는 값으로 잡았다
const THUMB_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// 안내 문구와 목록 첫 줄(`..`) 사이의 틈 — 가운데에 두면 목록이 짧은 창에서 잘리므로
/// 위쪽에 두되, 첫 줄과 겹치지 않을 만큼만 내린다 (2026-08-16 사용자 보고)
const EMPTY_HINT_GAP: f32 = 12.0;
const EMPTY_HINT_FONT_PX: f32 = 13.0;

/// 안내 문구가 목록 영역 위쪽에서 떨어지는 거리 — **첫 줄 아래**다.
///
/// 첫 줄 높이는 보기 모드마다 다르다 — 자세히 보기는 머리글과 한 행, 나머지는 격자 한 칸이다
fn empty_hint_top(mode: ViewMode) -> f32 {
    let first_row = if mode.is_details() {
        crate::ui::list_details::HEADER_HEIGHT + crate::ui::list_details::ROW_HEIGHT
    } else {
        mode.cell_size().y
    };
    first_row + EMPTY_HINT_GAP
}

/// 열거 성공 시 히스토리에 적용할 동작
#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingNav {
    /// 폴더만 다시 읽는다 — 첫 로드·탭 전환·새로고침
    None,
    /// 새 이동 — 커서 뒤를 자르고 추가
    Push,
    Back,
    Forward,
}

/// 캐시로 **먼저 옮겨 둔** 상태 — 실제 열거가 그 폴더를 열지 못하면 이것으로 되돌린다 (FR-68).
///
/// 되돌리기를 「히스토리 조작의 역연산」으로 할 수 없어 스냅샷을 든다 — `History::push`가
/// 앞으로 가기 목록을 **잘라내므로** 잘린 항목은 어떤 역연산으로도 살아나지 않는다.
///
/// `target`을 함께 드는 것은 **그 폴더의 결과일 때만** 되돌리기가 발동하게 하기 위해서다 —
/// 이 상태가 남은 채 무관한 폴더에서 `NotFound`가 나면 엉뚱한 곳으로 끌려간다
struct OptimisticNav {
    /// 캐시로 옮겨 간 폴더 — 도착한 결과가 이 폴더의 것일 때만 쓴다
    target: PathBuf,
    /// 옮기기 전에 보고 있던 폴더
    prev_dir: PathBuf,
    /// 옮기기 전 히스토리 전체
    history: History,
    /// 적중 중에 도착한 배치 — **그리지 않고 모아 둔다**.
    ///
    /// 이미 캐시로 전부 그린 화면에 중간 배치를 반영하면 목록이 줄었다가 다시 늘어난다.
    /// 완료 조각이 오면 이 몫과 잔여분을 합쳐 한 번에 갈아 끼운다
    buffered: Vec<FileEntry>,
}

/// 목록을 읽지 못한 사유 — 그 자리에 적을 말이 이것으로 갈린다 (2026-08-17 사용자 결정).
///
/// 사유마다 사용자가 할 일이 다르다 — 권한은 관리자에게, 네트워크는 연결을 살피고,
/// 그 밖은 다시 열어 본다. 세 갈래가 문구 하나씩만 달라 전략 트레이트를 두지 않았다
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ListBlock {
    /// 권한이 없다 (2026-08-16)
    AccessDenied,
    /// 네트워크 드라이브·서버에 닿지 못했다
    NetworkUnavailable,
    /// 그 밖의 사유로 열지 못했다
    OpenFailed,
}

impl ListBlock {
    /// 목록 자리에 적을 말
    fn hint(self) -> &'static str {
        match self {
            ListBlock::AccessDenied => crate::i18n::list_access_denied(),
            ListBlock::NetworkUnavailable => crate::i18n::list_network_unavailable(),
            ListBlock::OpenFailed => crate::i18n::list_open_failed(),
        }
    }
}

/// 원격 목록 메뉴에서 고른 것과 그때의 대상들 (FR-39)
pub type RemoteMenuPick = (RemoteMenuAction, Vec<RemoteTarget>);

/// `show_content`가 한 프레임에서 거둔 것들 — 목록 조작·단계 화면의 조치·놓기·원격 메뉴
type ContentOutcome = (
    FileListAction,
    Option<RemoteAction>,
    Option<DropOutcome>,
    Option<RemoteMenuPick>,
);

/// 목록에서 확정된 이름 바꾸기 한 건 (FR-64).
///
/// 경로와 새 이름만 담는다 — 어느 행이었는지는 여기서 쓸모가 없다(거는 쪽은 경로만 안다)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameRequest {
    pub path: PathBuf,
    pub new_name: String,
}

/// 패널이 상위(레이아웃)에 올려보내는 요청.
/// 전부 이 패널을 그리는 도중에는 실행할 수 없어 값으로 돌려준다
pub struct PanelOutcome {
    pub menu: Option<MenuRequest>,
    /// 목록에서 고쳐 확정한 이름 (FR-64) — **거는 것은 앱의 몫이다**.
    ///
    /// 패널이 직접 걸지 않는 이유: 셸이 띄우는 오류 대화의 주인 창(HWND)을 아는 곳은
    /// 앱뿐이고, 결과를 받을 채널도 앱이 이미 들고 있다(메뉴의 삭제·복사와 같은 채널)
    pub rename: Option<RenameRequest>,
    /// 패널 메뉴에서 고른 명령 — 대상은 **이 패널**이다 (plan D16)
    pub command: Option<Command>,
    /// 원격 단계 화면에서 고른 조치 (FR-29·FR-32)
    pub remote: Option<RemoteAction>,
    /// 주소창에 적은 원격 주소 (FR-34) — 사이트로 해소해 새 탭을 여는 것은 앱의 몫이다.
    ///
    /// 패널이 직접 열지 못하는 이유: 주소에는 `SiteId`가 없고, 어느 사이트로 볼지(이미 등록된
    /// 서버인지 새로 만들지)는 사이트 목록을 쥔 앱만 판정할 수 있다
    pub remote_url: Option<RemoteUrl>,
    /// 마지막 원격 탭이 닫혀 이 패널이 더 쓰지 않게 된 연결 (FR-32).
    ///
    /// `remote`와 **따로 두는 이유**: 한 프레임에 둘 다 일어날 수 있는데 한 필드로 합치면
    /// 먼저 채워진 쪽에 밀려 연결이 닫히지 않은 채 워커와 소켓이 남는다
    pub closed_conn: Option<ConnectionId>,
    /// 이 패널의 목록에 끌어다 놓은 것 (FR-38) — 큐에 넣는 것은 앱의 몫이다
    pub drop: Option<DropOutcome>,
    /// 원격 목록 우클릭 메뉴에서 고른 것과 그 대상들 (FR-39)
    pub remote_menu: Option<RemoteMenuPick>,
    /// 트리 우클릭 메뉴에서 고른 즐겨찾기 조작 (FR-56) — 목록을 고치는 것은 앱이다
    pub favorite: Option<FavoriteAction>,
    /// 이 패널이 열어 본 드라이브와 그 결과 `(경로, 닿았는가)` (T6) — 드라이브 목록에
    /// 반영하는 것은 앱이다. 로컬 드라이브의 실패는 `DriveList::observe`가 걸러낸다
    pub drive_observed: Option<(PathBuf, bool)>,
    /// 원격 트리가 청한 하위 조회들 (FR-9 원격판) — 연결에 보내는 것은 앱이다.
    ///
    /// **여럿을 그대로 올린다** — 한 프레임에 형제 노드 여럿이 펼쳐져 있으면 요청도 여럿이다.
    /// 하나로 압착하면 나머지는 그 프레임에서 버려져 노드마다 프레임이 하나씩 밀린다
    /// (quality 리뷰 M1)
    pub tree_requests: Vec<TreeRequest>,
}

/// 원격 단계 화면에서 사용자가 고른 조치 — 실행은 앱이 한다(연결을 앱이 쥔다)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteAction {
    /// 실패한 연결을 다시 건다 (인벤토리 #18)
    Retry,
    /// 사이트 관리자를 연다 (인벤토리 #19)
    OpenSettings,
    /// 서버 로그를 보인다 (인벤토리 #20)
    ViewLog,
    /// 연결 중인 것을 그만둔다 (인벤토리 #21)
    CancelConnect,
    /// 세션에서 되살아난 탭을 그 사이트로 다시 연결한다 (사용자 보고 2026-08-13).
    ///
    /// `Retry`와 나누는 이유: 저쪽은 **살아 있는 워커**에 명령만 다시 보내는데, 이 탭에는
    /// 워커가 아예 없다(재시작하면 자동으로 붙지 않는다) — 사이트부터 새로 열어야 한다
    Reconnect,
}

/// 셸 컨텍스트 메뉴 요청 — 그리기가 모두 끝난 뒤 앱이 실행한다.
/// `items`가 비면 폴더 배경 메뉴다
pub struct MenuRequest {
    pub folder: PathBuf,
    pub items: Vec<PathBuf>,
    /// `items`와 **같은 차례로** 그 항목이 폴더인가 (FR-8 재개정).
    ///
    /// 메뉴의 `즐겨찾기에 추가`·`새 탭에서 열기`가 폴더에만 열리는데, 받는 쪽에서
    /// `is_dir()`을 부르면 UI 스레드가 파일시스템을 묻게 된다. 목록은 열거할 때 이미
    /// 그 값을 쥐고 있어 실어 보내면 그만이다
    pub dirs: Vec<bool>,
    /// egui 논리 좌표 — 화면 좌표 변환은 실행 시점에 한다
    pub pos: egui::Pos2,
}

/// 패널 하나의 탐색 상태
pub struct PanelState {
    /// 탭 목록 — 탭마다 커밋된 경로와 독립 히스토리를 갖는다 (FR-3)
    tabs: TabsModel,
    list: FileListView,
    address: AddressBar,
    load: DirLoad,
    /// 마지막 목록 요청이 **자동 재조회**였으면 그 일련번호 (FR-67) — 손으로 부른 조회면 `None`.
    ///
    /// 그 실패를 화면 알림 없이 넘기는 데 쓴다(서버 로그에는 그대로 남는다 — D9)
    quiet_request: Option<u64>,
    /// 이 패널에서 마지막으로 조작이 있었던 시각 — egui가 프레임마다 주는 경과 초다
    /// (`InputState::time`. `Instant`를 쓰지 않는 것은 `ui::toast`가 정한 관례다).
    ///
    /// 자동 재조회가 **사용자 조작 직후를 건너뛰는** 데 쓴다 (FR-67 · D10)
    last_input_at: f64,
    /// 열거 중인 대상 — 성공해야 활성 탭의 경로로 커밋된다
    pending_dir: PathBuf,
    /// 열거가 성공하면 히스토리에 무엇을 할지.
    /// 커서 이동도 성공 후로 미뤄야 한다 — 먼저 옮기면 실패했을 때 화면과 히스토리가 어긋난다
    pending_nav: PendingNav,
    /// 이번 요청이 **중간 조각을 한 번이라도 그렸는가** (FR-69).
    ///
    /// 참이면 완료 조각(`Done`)은 목록을 갈아 끼우지 않고 **잔여분만 이어 붙이고**,
    /// 실패로 끝나도 이미 그린 것을 지우지 않는다. 커밋 시점도 첫 조각으로 앞당겨진다
    streamed: bool,
    /// 캐시로 먼저 옮겨 둔 상태 — 되돌릴 것이 없으면 `None` (FR-68)
    optimistic: Option<OptimisticNav>,
    /// 열거 실패 사유 — 성공 시 빈 문자열
    status: String,
    /// 이번 프레임에 열어 본 드라이브와 그 결과 `(경로, 닿았는가)` (T6).
    ///
    /// **중간에 담아 두는 이유**: 열거 결과를 받는 `apply_enumerated`는 `poll_load` 안에서
    /// 돌아 그 자리에서 `PanelOutcome`을 만들 수 없다. 여기 두었다가 `show`가 끝나며
    /// `take()`로 올려보낸다 — 반영하는 것은 앱이다(패널은 `DriveList`를 모른다)
    observed_drive: Option<(PathBuf, bool)>,
    /// 내용을 읽지 못한 폴더와 그 사유 (2026-08-16·2026-08-17 사용자 요청).
    ///
    /// **경로째로 담는다** — 활성 탭이 여기를 볼 때만 안내를 띄우려면 깃발 하나로는 모자란다.
    /// 탭을 옮기거나 원격을 보는 동안 옛 안내가 남는 것을 이 대조가 막는다
    blocked: Option<(PathBuf, ListBlock)>,
    /// 첫 프레임을 그린 뒤 열거를 시작하기 위한 대기 경로.
    /// 생성자에서 바로 열거하면 창이 늦게 뜬다
    deferred_start: Option<PathBuf>,
    /// 폴더 트리 — 패널마다 독립이다 (FR-9)
    tree: FolderTreeView,
    /// 트리 표시 여부. 현행 판과 같이 숨김으로 시작한다
    tree_visible: bool,
    /// 표시 중인 폴더의 변경 감시 (FR-10). 폴더가 바뀌면 통째로 교체된다
    watch: Option<DirWatch>,
    /// 진행 중인 새 폴더·새 파일 생성 (FR-25)
    create: CreateOp,
    /// 주소창에 적힌 원격 주소 — 이번 프레임의 결과로 앱에 올려 보낸다
    pending_remote_url: Option<RemoteUrl>,
    /// 마지막 탭을 닫으려 했다 — 이 패널을 닫아 달라는 뜻으로 앱에 올려 보낸다.
    /// 마지막 **패널**을 지키는 것은 앱의 몫이다 (FR-2)
    close_requested: bool,
    /// 원격 목록 우클릭 메뉴가 뜰 자리 — `None`이면 닫혀 있다 (FR-39)
    remote_menu_at: Option<egui::Pos2>,
    /// 아직 조회를 청하지 않은 "직전에 보고 있던 곳" — `set_remote_path`가 세우고
    /// 곧이어 `request_remote_list`가 일련번호와 함께 `revert_at`으로 옮긴다
    pending_revert: Option<RemotePath>,
    /// 되돌릴 자리 — `(그 조회의 일련번호, 돌아갈 곳, 옮겨 간 곳)`.
    ///
    /// **요청 하나에 묶는다**(F-7 2라운드 B1·B2): 번호만 보고 되돌리면 ⓐ 이미 성공한 이동의
    /// 자리가 남아 나중의 새로 고침 실패가 옛 폴더로 되돌리고 ⓑ 같은 연결의 다른 패널·탭까지
    /// 함께 되돌아간다. 그래서 **성공하면 지우고**, 되돌릴 때는 지금 보고 있는 곳이
    /// `옮겨 간 곳` 그대로일 때만 손댄다
    revert_at: Option<(u64, RemotePath, RemotePath)>,
    /// 원격 위치가 바뀌어 목록을 다시 읽어야 한다 — 앱이 다음 프레임에 거둬 간다.
    ///
    /// **위치를 옮기는 것과 서버에 묻는 것은 다른 일이다** — 옮기는 쪽(트리 선택·상위 이동)은
    /// 연결을 모르고, 명령을 보내는 쪽(`ConnectionManager`)은 앱이 쥐고 있다. 그 사이를
    /// 이 깃발이 잇는다(spec 리뷰 B1: 옮기기만 하고 아무도 다시 읽지 않던 자리)
    remote_dirty: bool,
    /// 이 패널이 낸 원격 목록 요청의 일련번호 — 늦게 도착한 이전 요청의 결과를 버린다 (D7).
    /// 로컬 열거의 `DirLoad`가 쓰는 것과 같은 기법이다.
    ///
    /// **패널마다 따로 센다** — 전역 유일하지 않으므로 어느 패널의 답인지는 이 번호가
    /// 아니라 `ListSource::Panel`의 워크스페이스·패널 식별자가 가린다
    remote_seq: u64,
    /// 끄는 동안 커서를 따라오는 그림의 대상 — 이 패널에서 시작한 끌기의 **첫 항목** (FR-38).
    ///
    /// 그림을 이 패널이 그리는 이유는 썸네일 텍스처(`thumb_textures`)를 패널이 쥐기 때문이다 —
    /// 앱에는 썸네일 캐시가 없어 첫 항목의 썸네일에 닿을 길이 없다 (plan D5)
    drag_preview: Option<crate::ui::list_common::DragItem>,
    /// 썸네일 픽셀 캐시 (FR-24) — 상한이 패널당이라 패널이 소유한다 (NFR-9)
    thumbs: ThumbnailCache,
    /// 올라간 썸네일 텍스처 — 픽셀 캐시와 함께 비워진다
    thumb_textures: ThumbnailTextures,
}

/// 감시자와 그 통지 채널 — 둘의 수명이 같아야 해서 함께 둔다
struct DirWatch {
    watcher: DirWatcher,
    /// 변경 통지 (내용 없음 — 받으면 폴더를 통째로 다시 읽는다)
    rx: Receiver<()>,
}

/// 앱 설정이 정하는 목록 표시 규칙 (FR-13·FR-52).
///
/// 둘을 묶는 이유: 같은 곳(앱 설정)에서 와서 같은 자리에 내려가고, 낱개로 넘기면
/// 이미 `#[allow(clippy::too_many_arguments)]`가 붙은 `show_layout`의 인자가 더 늘어난다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayRules {
    /// 이름 뒤 확장자를 보인다 (FR-52)
    pub show_extensions: bool,
    /// 숨김 속성 항목을 보인다 (FR-13)
    pub show_hidden: bool,
    /// 시스템 속성 항목을 보인다 (FR-13) — 둘 다 붙은 항목은 두 값이 모두 켜져야 보인다
    pub show_system: bool,
}

impl PanelState {
    pub fn new(start: PathBuf) -> PanelState {
        PanelState {
            quiet_request: None,
            last_input_at: 0.0,
            tabs: TabsModel::new(TabState::new(start.clone())),
            list: FileListView::new(),
            address: AddressBar::new(),
            load: DirLoad::new(),
            pending_dir: PathBuf::new(),
            pending_nav: PendingNav::None,
            streamed: false,
            optimistic: None,
            status: String::new(),
            observed_drive: None,
            blocked: None,
            deferred_start: Some(start),
            tree: FolderTreeView::new(),
            tree_visible: false,
            watch: None,
            create: CreateOp::new(),
            pending_remote_url: None,
            close_requested: false,
            remote_menu_at: None,
            pending_revert: None,
            revert_at: None,
            remote_dirty: false,
            remote_seq: 0,
            drag_preview: None,
            thumbs: ThumbnailCache::new(),
            thumb_textures: ThumbnailTextures::new(),
        }
    }

    /// 저장된 탭 구성과 목록 표시 상태로 패널을 되살린다 (FR-11). 탭 목록이 비면 `None`.
    ///
    /// 히스토리는 복원하지 않는다 — 세션에는 경로만 저장한다(현행과 같은 규칙).
    /// 열 폭·보기 모드·정렬은 저장된 값이 없으면 각자의 기본으로 시작한다.
    ///
    /// **인자를 하나씩 늘리지 않고 `PanelTabs`를 통째로 받는다**(D12) — 저장할 것이 늘 때마다
    /// 인자가 늘면 이 시그니처와 호출부가 함께 흔들린다. `PanelTabs`는 이미 저장 형식과
    /// 1:1로 대응하는 구조체라 필드가 늘어도 여기는 그대로다
    pub fn from_tabs(saved: &PanelTabs) -> Option<PanelState> {
        // 원격 탭은 **연결 없이** 되살아난다 — 연결은 사용자가 연다 (FR-44)
        let states: Vec<TabState> = saved
            .tabs
            .iter()
            .map(|tab| match tab {
                TabSpec::Local(path) => TabState::new(path.clone()),
                TabSpec::Remote { site, path } => TabState::remote(*site, path.clone()),
            })
            .collect();
        let model = TabsModel::from_tabs(states, saved.active_tab)?;
        let start = model.active().committed().to_path_buf();
        let mut panel = PanelState::new(start);
        panel.tabs = model;
        if !saved.columns.is_empty() {
            panel.list.set_columns(&saved.columns);
        }
        if !saved.view_mode.is_empty() {
            panel.set_view_mode(ViewMode::from_key(&saved.view_mode));
        }
        // **비었으면 방향도 읽지 않는다** — 둘은 함께 담기므로, 정렬을 저장한 적 없는 옛
        // 세션에서 `bool`의 기본값이 내림차순으로 새어 나오지 않는다
        if !saved.sort_key.is_empty() {
            panel
                .list
                .set_sort(SortKey::from_key(&saved.sort_key), saved.sort_ascending);
        }
        if !saved.column_order.is_empty() {
            panel.list.set_column_order(&saved.column_order);
        }
        Some(panel)
    }

    /// 세션에 담을 탭 구성과 목록 표시 상태 (FR-11·FR-44).
    ///
    /// `from_tabs`의 거울상이다 — 저장할 것이 늘면 두 함수만 고치면 되고 호출부는 그대로다(D12).
    /// `list`가 private이라 밖에서 `FileListView`에 직접 손댈 수 없는 것도 이 통로가 있는 이유다
    pub fn to_tabs(&self) -> PanelTabs {
        let (sort_key, sort_ascending) = self.list.sort();
        PanelTabs {
            tabs: self.tab_specs(),
            active_tab: self.active_tab(),
            columns: self.list.columns(),
            view_mode: self.view_mode().as_key().to_owned(),
            sort_key: sort_key.as_key().to_owned(),
            sort_ascending,
            column_order: self.list.column_order(),
        }
    }

    /// 현재 표시 중인 폴더 — 활성 탭이 커밋한 경로가 정본이다
    pub fn dir(&self) -> &Path {
        self.tabs.active().committed()
    }

    /// 목록 자리에 자리표시를 세울 때 — **읽는 중인데 아직 보여줄 것이 없을 때**만이다.
    ///
    /// 이미 목록이 있는 폴더에서 다른 폴더로 옮기는 중이라면 종전대로 이전 목록을 둔다
    /// (열거가 실패하면 그 자리에 그대로 머무는 것이 이 앱의 규칙이다) — 그때마다 자리를
    /// 비우면 폴더를 옮길 때마다 화면이 한 번 더 깜빡인다
    pub fn shows_loading_placeholder(&self) -> bool {
        self.load.is_loading() && self.list.is_empty()
    }

    /// 세션 저장용 — 탭들이 가리키는 곳(탭 순서). 원격 탭은 사이트와 원격 경로로 담긴다
    pub fn tab_specs(&self) -> Vec<TabSpec> {
        self.tabs
            .sources()
            .iter()
            .map(|source| match source {
                TabSource::Local(path) => TabSpec::Local(path.clone()),
                TabSource::Remote { site, path, .. } => TabSpec::Remote {
                    site: *site,
                    path: path.clone(),
                },
            })
            .collect()
    }

    pub fn active_tab(&self) -> usize {
        self.tabs.active_index()
    }

    /// 프레임마다 호출 — 지연 시작·열거 완료·변경 감시를 처리하고, 열거 중이면 다시 그리게 한다
    pub fn poll(&mut self, ctx: &egui::Context, icons: &mut IconCache, cache: &mut DirCache) {
        if let Some(path) = self.deferred_start.take() {
            // 시작 경로는 이미 히스토리에 들어 있으므로 다시 쌓지 않는다
            self.start_load(path, PendingNav::None, ctx);
        }
        self.poll_load(ctx, icons, cache);
        self.poll_watch(ctx);
        self.poll_create(ctx);
        if let Some(delay) = self.poll_thumbnails(ctx) {
            ctx.request_repaint_after(delay);
        }
        if self.load.is_loading() {
            ctx.request_repaint();
        }
    }

    /// 표시 중인 폴더를 감시한다 (FR-10). 이전 폴더의 감시는 여기서 끝난다 —
    /// 교체하면 옛 `DirWatcher`가 drop되며 그 스레드가 정지·회수된다
    fn watch(&mut self, path: &Path) {
        if self
            .watch
            .as_ref()
            .is_some_and(|watch| watch.watcher.path() == path)
        {
            return;
        }
        let (tx, rx) = channel();
        // 감시는 창 메시지를 쓰지 않는다 — 채널만 받아 프레임마다 확인한다 (D7)
        let watcher = DirWatcher::start(path.to_path_buf(), tx, None);
        self.watch = Some(DirWatch { watcher, rx });
    }

    /// 감시 통지를 확인해 변경이 있으면 폴더를 다시 읽는다.
    /// 폴더가 열리지 않아 감시가 즉시 끝난 경우에도 통지 1회가 오며, 재열거가 사유를 표시한다
    fn poll_watch(&mut self, ctx: &egui::Context) {
        // 원격 탭을 보는 동안에는 로컬 감시 통지를 처리하지 않는다 — 이전 폴더의 감시가
        // 아직 살아 있을 수 있는데, 그 통지로 로컬 열거를 걸면 원격 화면이 로컬 목록으로 덮인다
        if self.is_remote() {
            return;
        }
        let Some(watch) = self.watch.as_ref() else {
            return;
        };
        // 쌓인 통지가 여러 개여도 다시 읽는 것은 한 번이면 된다
        let mut changed = false;
        while watch.rx.try_recv().is_ok() {
            changed = true;
        }
        if changed {
            let dir = self.dir().to_path_buf();
            self.start_load(dir, PendingNav::None, ctx);
            ctx.request_repaint();
        }
    }

    /// 폴더 이동 (사용자 조작) — 성공하면 히스토리에 남는다
    fn navigate(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.start_load(path, PendingNav::Push, ctx);
    }

    fn start_load(&mut self, path: PathBuf, nav: PendingNav, ctx: &egui::Context) {
        // **다른 폴더로 옮길 때만** 이름 필터를 비운다 (D2·D11). 같은 폴더를 다시 읽는 길
        // (변경 감시(FR-10)·F5·숨김 토글)에서 지우면 사용자가 치던 글자가 예고 없이 사라진다
        if path != self.dir() {
            self.list.set_filter("");
        }
        self.pending_dir = path.clone();
        self.pending_nav = nav;
        self.streamed = false;
        // 새 요청이 서므로 앞선 낙관적 이동은 되돌릴 대상이 아니다 (FR-68 — 해제 3지점 중 하나)
        self.optimistic = None;
        self.status.clear();
        self.load.start(path, ctx);
    }

    /// 워커가 흘려보낸 조각을 목록에 반영한다.
    ///
    /// **그 프레임에 도착한 것을 모두 소비하되, 중간 조각(`Partial`)은 한 번에 합쳐 반영한다.**
    /// `apply_partial`이 매번 `rebuild_visible`로 누적 전체를 다시 정렬·복제하므로(FR-69),
    /// 워커가 UI보다 빨라 한 프레임에 배치가 여럿 쌓이면 그 프레임에서만 정렬이 수십 번 돌아
    /// 앱이 멎는다(10만 항목 ≈ 1초 — 실측). 합쳐 반영하면 **프레임당 정렬이 한 번**으로 준다.
    /// 하나씩만 꺼내면 반대로 배치 수만큼 프레임이 들어 완주가 늦어지므로, 그 절충으로
    /// 「이 프레임에 온 것을 모아 한 번」을 택한다
    fn poll_load(&mut self, ctx: &egui::Context, icons: &mut IconCache, cache: &mut DirCache) {
        self.try_cache_hit(icons, cache);
        let mut pending: Vec<crate::fs::enumerate::FileEntry> = Vec::new();
        while let Some(chunk) = self.load.poll() {
            match chunk {
                EnumChunk::Partial(entries) => pending.extend(entries),
                EnumChunk::Done(outcome) => {
                    // 완료 조각은 **직전까지 모은 중간 몫을 먼저 반영한 뒤** 처리한다 —
                    // 순서가 뒤집히면 `apply_enumerated`의 `streamed` 분기가 어긋난다
                    self.flush_pending_partial(&mut pending, icons);
                    let t_apply = std::time::Instant::now();
                    self.apply_enumerated(outcome, icons, cache, ctx);
                    let d_apply = t_apply.elapsed();
                    crate::perf::log(|| {
                        let (dirs, files) = self.list.counts();
                        format!(
                            "apply done dirs={dirs} files={files} | apply={:.1} (ms)",
                            d_apply.as_secs_f32() * 1000.0
                        )
                    });
                }
            }
        }
        // 이 프레임에 `Done`이 오지 않았으면 모은 중간 몫만 반영한다
        self.flush_pending_partial(&mut pending, icons);
    }

    /// 이 프레임에 모은 중간 배치를 **한 번에** 목록에 반영한다 (위 `poll_load` 참조).
    fn flush_pending_partial(
        &mut self,
        pending: &mut Vec<crate::fs::enumerate::FileEntry>,
        icons: &mut IconCache,
    ) {
        if pending.is_empty() {
            return;
        }
        let entries = std::mem::take(pending);
        // 임시 계측 (`crate::perf`) — UI 스레드가 이 프레임 몫을 목록에 세우는 시간이다
        let t_apply = std::time::Instant::now();
        let added = entries.len();
        self.apply_partial(entries, icons);
        let d_apply = t_apply.elapsed();
        crate::perf::log(|| {
            let (dirs, files) = self.list.counts();
            format!(
                "apply partial+{added} dirs={dirs} files={files} | apply={:.1} (ms)",
                d_apply.as_secs_f32() * 1000.0
            )
        });
    }

    /// 탐색을 커밋한다 — 활성 탭의 경로·히스토리와 썸네일 세대를 새 폴더에 맞춘다.
    ///
    /// 목록과 감시는 부르는 쪽이 정한다 — 읽어 낸 폴더와 권한이 막힌 폴더가 서로 다르다
    fn commit_navigation(&mut self, dir: &Path) {
        let tab = self.tabs.active_mut();
        tab.set_committed(dir.to_path_buf());
        match self.pending_nav {
            PendingNav::None => {}
            PendingNav::Push => tab.history.push(dir.to_path_buf()),
            PendingNav::Back => {
                tab.history.back();
            }
            PendingNav::Forward => {
                tab.history.forward();
            }
        }
        // 폴더가 바뀌면 이전 폴더의 썸네일을 즉시 놓는다 (NFR-9).
        // **판정은 캐시가 한다** — 탐색·탭 전환·탭 닫기가 각자 다른 순서로 폴더를
        // 바꾸므로, 여기서 비교하면 한 경로만 빠뜨려도 조용히 새어나간다(F-7 B1·m1).
        // 이 해제는 세대를 올리는 지점이기도 해서, 늦게 도착한 이전 폴더의 결과도 함께 걸러진다
        if self.thumbs.set_folder(dir) {
            self.thumb_textures.clear();
        }
    }

    /// 대기 중인 폴더가 캐시에 있으면 **먼저 그려 두고 옮긴다** (FR-68).
    ///
    /// 적중 판정은 네 가지를 함께 본다 — ⓐ 열거가 진행 중이고 ⓑ **그려져 있는 로컬 목록이
    /// 그 폴더의 것이 아니며** ⓒ 원격 탭이 아니고 ⓓ 캐시에 있다.
    ///
    /// **ⓑ를 커밋된 경로(`dir()`)로 재지 않는다** — `reload_active_tab`이 활성 탭을 바꾼 **뒤**
    /// 그 탭의 경로로 다시 읽으므로 그 순간 둘이 같아져 **탭 전환이 캐시를 못 탄다**.
    /// 그리고 `list.dir()`만으로도 부족하다: `clear_entries`가 그 필드를 갱신하지 않아
    /// 「로컬 A → 원격 탭 → 다시 A」에서 목록이 빈 채 폴더만 A로 남는다. 그래서
    /// `entries()`가 로컬 목록을 주는지와 함께 본다.
    ///
    /// 같은 폴더를 다시 읽는 길(F5·감시 갱신·숨김 토글)은 ⓑ가 거짓이라 저절로 걸러진다
    fn try_cache_hit(&mut self, icons: &mut IconCache, cache: &mut DirCache) {
        if !self.load.is_loading() || self.is_remote() {
            return;
        }
        if self.list.entries().is_some() && self.list.dir() == self.pending_dir {
            return;
        }
        let Some(entries) = cache.get(&self.pending_dir) else {
            return;
        };
        let entries = entries.to_vec();
        let dir = self.pending_dir.clone();
        // 되돌릴 자리를 먼저 챙긴다 — 커밋하고 나면 이전 폴더를 알 수 없다
        self.optimistic = Some(OptimisticNav {
            target: dir.clone(),
            prev_dir: self.dir().to_path_buf(),
            history: self.tabs.active().history.clone(),
            buffered: Vec::new(),
        });
        // 이른 커밋 — `pending_dir`을 비우지 않고 `pending_nav`만 내린다 (계획 D6)
        self.commit_navigation(&dir);
        self.pending_nav = PendingNav::None;
        self.blocked = None;
        self.watch(&dir);
        // 캐시는 `..` 줄이 포함된 최종 목록이다 — 다시 붙이지 않는다 (계획 D7)
        self.list.set_entries(dir, entries, icons);
    }

    /// 다 읽지 못한 폴더의 중간 몫을 반영한다 (FR-69).
    ///
    /// **첫 조각이 커밋 지점이다** — 그것이 도착했다는 것은 폴더를 열 수 있다는 뜻이므로,
    /// 종전에 `Done`이 하던 일(경로·히스토리·감시 옮기기)을 여기서 한다. 그때
    /// **`pending_dir`을 비우지 않고 clone해 넘기고 `pending_nav`만 내린다**(계획 D6) —
    /// 비우면 뒤이어 오는 `Done`이 `mem::take`로 빈 경로를 커밋해 탭 경로가 망가진다.
    /// `pending_nav`를 내려 두면 그 두 번째 커밋은 같은 경로의 재적용뿐이라 무해하다
    fn apply_partial(&mut self, entries: Vec<FileEntry>, icons: &mut IconCache) {
        // 활성 탭이 원격이면 이 결과는 남의 것이다 — `apply_enumerated`와 같은 가드다
        if self.is_remote() {
            self.abandon_local_load();
            return;
        }
        // 캐시로 이미 전부 그려 둔 화면에 중간 배치를 반영하면 목록이 줄었다가 다시 늘어난다 —
        // 모아 두었다가 완료 조각에서 한 번에 갈아 끼운다 (FR-68·FR-69가 겹치는 구간)
        if let Some(optimistic) = self.optimistic.as_mut() {
            optimistic.buffered.extend(entries);
            return;
        }
        if self.streamed {
            self.list.append_entries(entries, icons);
            return;
        }
        self.streamed = true;
        let dir = self.pending_dir.clone();
        self.commit_navigation(&dir);
        self.pending_nav = PendingNav::None;
        self.blocked = None;
        // 열어 본 결과를 드라이브 상태에 올려보낸다 — 조각이 왔다는 것은 닿았다는 것이다
        self.observed_drive = Some((dir.clone(), true));
        self.watch(&dir);
        let entries = with_local_parent_first(&dir, entries);
        self.list.set_entries(dir, entries, icons);
    }

    /// 이미 그린 부분 목록을 **그대로 두고** 실패 사유만 알린다 (FR-69).
    ///
    /// `block_list`를 쓸 수 없다 — 그 함수는 목록을 `..` 한 줄만 남기고 갈아 끼우므로
    /// 배치로 그려 둔 것을 지운다. 사유는 목록 자리가 아니라 상태 줄에 적는다.
    ///
    /// **감시도 놓지 않는다** — `block_list`는 「읽지 못한 폴더는 감시하지 않는다」(FR-6)를
    /// 지키려 감시를 끊지만, 이쪽은 **부분이라도 읽힌 폴더**라 그것이 바뀌면 다시 읽어
    /// 마저 채울 수 있다. 이 갈래가 FR-6의 예외임은 FR-69 문면에 적혀 있다
    fn keep_partial(&mut self, reason: ListBlock) {
        // 이 요청은 여기서 끝난다 — 비우지 않으면 `observed_drive`·`pending_name`이
        // 끝난 요청의 경로를 계속 읽는다
        self.pending_dir = PathBuf::new();
        self.status = reason.hint().to_string();
    }

    /// 열거 결과 하나를 상태에 반영한다.
    ///
    /// `poll_load`에서 갈라낸 이유는 **테스트가 이 경로를 실제로 지나게** 하기 위해서다 —
    /// 판정 헬퍼만 직접 부르는 테스트는 호출부가 죽어도 통과한다(F-7 B1이 그렇게 새어나갔다)
    fn apply_enumerated(
        &mut self,
        outcome: EnumOutcome,
        icons: &mut IconCache,
        cache: &mut DirCache,
        ctx: &egui::Context,
    ) {
        // **활성 탭이 원격이면 이 결과는 남의 것이다.** 열거를 걸어 둔 사이에 탭이 원격으로
        // 바뀔 수 있고(사이트 연결·탭 전환), 그대로 커밋하면 원격 탭이 로컬 탭으로 둔갑한다
        // (개발 빌드에서는 `TabState::set_committed`의 단언이 앱을 끝낸다).
        // 세대가 맞아도 **가는 곳이 사라진** 답이라 여기서 버린다
        if self.is_remote() {
            self.abandon_local_load();
            return;
        }
        // 열어 본 결과를 드라이브 상태에 올려보낸다 (T6) — **`network` 깃발이 아니라
        // "닿았는가"로 판정한다**. 그 깃발에 배지까지 걸면 T1의 오류 코드 목록에서 빠진
        // 실패 하나가 곧 "X가 영영 안 붙는" 결함이 된다(plan Risks).
        // 권한 없음은 **닿은 것**이다 — 권한이 없을 뿐 드라이브에는 닿았다
        let reachable = matches!(outcome, EnumOutcome::Ok(_) | EnumOutcome::AccessDenied);
        self.observed_drive = Some((self.pending_dir.clone(), reachable));
        // **배치를 흘리던 중 끊겼다** — 이미 그린 것을 지우지 않는다 (FR-69).
        // 아래 `match`보다 앞에 두는 것은 `outcome`이 그 안에서 부분 이동되기 때문이다
        if self.streamed && !matches!(outcome, EnumOutcome::Ok(_)) {
            let reason = match outcome {
                EnumOutcome::AccessDenied => ListBlock::AccessDenied,
                EnumOutcome::Error { network: true } => ListBlock::NetworkUnavailable,
                _ => ListBlock::OpenFailed,
            };
            self.keep_partial(reason);
            // 읽지 못한 폴더의 캐시는 버린다 — 두면 재진입마다 낡은 목록이 한 프레임 섰다가 지워진다
            self.forget_optimistic(cache);
            self.pending_nav = PendingNav::None;
            self.streamed = false;
            return;
        }
        match outcome {
            EnumOutcome::Ok(entries) => {
                // 여기서 비로소 커밋한다 — 이 지점 전에는 화면이 이전 폴더를 유지한다.
                // **배치로 이미 커밋했으면**(`streamed`) `pending_nav`가 `None`이라 이 재호출은
                // 같은 경로의 `set_committed` 재적용뿐이다 — 히스토리는 한 번만 늘어난다(D6)
                let dir = std::mem::take(&mut self.pending_dir);
                self.commit_navigation(&dir);
                self.blocked = None;
                // 감시 대상도 이 시점에 맞춘다 — 읽어 낸 폴더만 감시한다
                self.watch(&dir);
                match self.optimistic.take() {
                    // 캐시로 그려 둔 화면을 실제 결과로 **한 번에** 갈아 끼운다 —
                    // 모아 둔 배치와 잔여분을 합쳐야 목록이 잘리지 않는다
                    Some(optimistic) if optimistic.target == dir => {
                        let mut all = optimistic.buffered;
                        all.extend(entries);
                        let entries = with_local_parent_first(&dir, all);
                        self.list.set_entries(dir.clone(), entries, icons);
                    }
                    _ if self.streamed => {
                        // 배치로 그린 목록에 **잔여분만** 이어 붙인다 — `..` 줄은 첫 조각이 이미 세웠다
                        self.list.append_entries(entries, icons);
                    }
                    _ => {
                        // 첫 줄은 상위 이동(`..`)이다 — 원격 목록과 같은 자리에서 같은 조작이 되게 한다
                        let entries = with_local_parent_first(&dir, entries);
                        self.list.set_entries(dir.clone(), entries, icons);
                    }
                }
                // **확정된 목록만** 담는다 (FR-68) — 중간 조각을 담으면 다음 재진입이
                // 잘린 목록으로 즉시 선다. `..` 줄이 포함된 형태 그대로다(계획 D7)
                if let Some(rows) = self.list.entries() {
                    cache.put(&dir, rows);
                }
            }
            // 읽지 못했어도 **그 폴더로 옮긴다** (2026-08-16·2026-08-17 사용자 요청) —
            // 이전 목록을 그대로 두면 주소창·트리가 가리키는 곳과 목록이 갈린다. 사유는
            // 상태 줄이 아니라 빈 목록 자리에 적는다 — 올라갈 곳이 있으면 `..` 줄만
            // 남고, 드라이브 뿌리에는 그 줄도 없다(`with_local_parent_first`)
            // 읽지 못한 폴더는 **되돌리지 않되 캐시를 버린다** — 그 폴더는 실재하므로
            // 그 자리에 머물러 사유를 보이고, 낡은 목록만 다음 진입에서 서지 않게 한다
            EnumOutcome::AccessDenied => {
                self.forget_optimistic(cache);
                self.block_list(ListBlock::AccessDenied, icons);
            }
            EnumOutcome::Error { network: true } => {
                self.forget_optimistic(cache);
                self.block_list(ListBlock::NetworkUnavailable, icons);
            }
            EnumOutcome::Error { network: false } => {
                self.forget_optimistic(cache);
                self.block_list(ListBlock::OpenFailed, icons);
            }
            // **찾을 수 없는 폴더만** 현 위치를 지킨다 (2026-08-17 사용자 결정) —
            // 실재하지 않는 곳에는 옮길 자리가 없어, 그리로 주소창을 옮기면 `..`말고는
            // 할 수 있는 것이 없는 화면이 된다. 사유는 종전대로 상태 줄에 적는다
            EnumOutcome::NotFound => {
                let reason = crate::i18n::dynamic::open_not_found(&self.pending_name());
                // 캐시를 믿고 이미 옮겨 갔는데 그 폴더가 없다 — 옮긴 자리를 되돌린다.
                // **`pending_dir`을 여기서 비우지 않는다** — 되돌리기가 이미 직전 폴더로
                // 새 열거를 걸었고 그 요청의 대상이 바로 그 값이다. 비우면 그 열거가
                // 끝났을 때 `mem::take`가 빈 경로를 꺼내 그것으로 커밋한다
                self.revert_optimistic(cache, icons, ctx);
                self.status = reason;
            }
        }
        self.pending_nav = PendingNav::None;
        // 이 요청의 조각은 여기서 끝났다 — 깃발을 내려 수명을 「이번 요청 동안」으로 못 박는다.
        // 남겨 두면 다음 탐색이 시작되기 전까지 참으로 남아, 그 사이에 이 값을 보는 코드가
        // 생기면 「아직 흘리는 중」으로 오판한다
        self.streamed = false;
        // 낙관적 이동도 이 결과로 매듭지어졌다 — 남겨 두면 **무관한 폴더의** `NotFound`가
        // 옛 스냅샷으로 히스토리를 되감는다 (해제 3지점 중 마지막)
        self.optimistic = None;
    }

    /// 캐시로 옮겨 간 폴더가 실은 없었다 — 옮긴 자리를 되돌린다 (FR-68). 되돌렸으면 `true`.
    ///
    /// **그 폴더의 결과일 때만** 발동한다(`target` 대조) — 이 상태가 남은 채 무관한 폴더에서
    /// `NotFound`가 나면 엉뚱한 곳으로 끌려간다.
    ///
    /// 히스토리와 탭 경로를 **그 자리에서** 되돌린다 — 다시 건 열거의 완료를 기다리면
    /// 그 사이 주소창·트리가 사라진 폴더를 가리키고, 되돌아간 폴더마저 읽히지 않으면
    /// 커밋이 없어 그대로 남는다
    fn revert_optimistic(
        &mut self,
        cache: &mut DirCache,
        icons: &mut IconCache,
        ctx: &egui::Context,
    ) {
        // **대상을 먼저 견주고 꺼낸다** — 먼저 꺼내면 대상이 다를 때 그 상태가 되돌려지지도,
        // 캐시가 무효화되지도 않은 채 조용히 사라진다. 지금은 `start_load`가 매번 이 값을
        // 내려 대상이 어긋날 길이 없지만, 순서가 거꾸로면 나중에 그 길이 생겼을 때 유실이 된다
        if self
            .optimistic
            .as_ref()
            .is_none_or(|it| it.target != self.pending_dir)
        {
            return;
        }
        let Some(optimistic) = self.optimistic.take() else {
            return;
        };
        cache.invalidate(&optimistic.target);
        let tab = self.tabs.active_mut();
        tab.history = optimistic.history;
        tab.set_committed(optimistic.prev_dir.clone());
        // `commit_navigation`을 거치지 않는 길이라 **썸네일 폴더도 직접 맞춘다** — 그 함수가
        // 유일한 동기화 지점이라, 빼먹으면 사라진 폴더를 가리킨 채 남아 그 사이에 같은 곳으로
        // 다시 들어가면 「같은 폴더」로 오판해 캐시를 놓지 않는다 (NFR-9)
        if self.thumbs.set_folder(&optimistic.prev_dir) {
            self.thumb_textures.clear();
        }
        // **목록도 함께 되돌린다** — 경로만 되돌리면 되돌아간 폴더 이름 아래 사라진 폴더의
        // 항목이 그대로 남아, 주소창·트리가 가리키는 곳과 목록이 갈린다(이 앱이 결함으로
        // 못 박아 둔 형태다). 직전 폴더가 캐시에 있으면 그것을 세우고, 없으면 비워
        // 자리표시가 서게 한 뒤 아래 재열거를 기다린다
        match cache.get(&optimistic.prev_dir) {
            Some(rows) => {
                let rows = rows.to_vec();
                self.list
                    .set_entries(optimistic.prev_dir.clone(), rows, icons);
            }
            None => self.list.clear_entries(),
        }
        // 다시 읽는다 — 이 호출이 세대를 올려 떠 있던 요청도 무효화한다
        self.start_load(optimistic.prev_dir, PendingNav::None, ctx);
    }

    /// 낙관적으로 옮겨 간 폴더를 읽지 못했다 — **되돌리지는 않고** 그 캐시만 버린다.
    ///
    /// 권한·네트워크 실패는 폴더가 실재하는 경우라 그 자리에 머무는 것이 종전 규칙이고,
    /// 낡은 목록만 남겨 두면 재진입마다 그것이 한 프레임 섰다가 사유 화면으로 지워진다
    fn forget_optimistic(&mut self, cache: &mut DirCache) {
        if let Some(optimistic) = self.optimistic.take() {
            cache.invalidate(&optimistic.target);
        }
    }

    /// 읽지 못한 폴더로 옮기고 목록 자리에 사유를 적을 상태를 세운다.
    ///
    /// 세 사유(권한·네트워크·그 밖)가 **같은 처리**를 거치므로 한 자리에 모았다 —
    /// 갈라지는 것은 목록 자리에 적을 말뿐이다
    fn block_list(&mut self, reason: ListBlock, icons: &mut IconCache) {
        let dir = std::mem::take(&mut self.pending_dir);
        self.commit_navigation(&dir);
        self.blocked = Some((dir.clone(), reason));
        // 읽지 못한 폴더는 감시하지 않는다 — 이전 폴더의 감시도 함께 놓는다.
        // 남겨 두면 그 폴더가 바뀔 때마다 여기를 다시 읽어 실패를 되풀이한다
        self.watch = None;
        let entries = with_local_parent_first(&dir, Vec::new());
        self.list.set_entries(dir, entries, icons);
    }

    /// 도착한 썸네일을 텍스처로 올린다 (FR-24).
    ///
    /// 채널을 비운 뒤 **픽셀 캐시 전체와 동기화**한다 — 방금 도착한 것만 올리면
    /// 프레임 상한에 걸린 경로가 영영 올라가지 못하고, 축출된 픽셀의 텍스처도 남는다
    /// 다시 그려야 하면 그 지연을 돌려준다. `None`이면 그릴 이유가 없다.
    ///
    /// `Duration::ZERO`는 **이번 프레임에 올린 것이 있다**는 뜻이고, 짧은 지연은
    /// **워커 결과를 기다린다**는 뜻이다. 열거(`DirLoad`)는 워커가 `ctx`를 들고 있어
    /// 직접 깨우지만, 썸네일 워커는 `fs` 계층이라 egui를 모른다(AGENTS: 의존 단방향) —
    /// 그래서 화면 쪽이 스스로 깨어나 채널을 확인한다.
    ///
    /// **판정을 값으로 돌려주는 이유**: 여기서 `ctx`에 직접 요청하면 `load_texture`가
    /// 스스로 일으키는 repaint와 섞여 테스트가 둘을 구분할 수 없다. 이 신호가 빠지면
    /// 워커가 늦게 준 썸네일이 사용자가 마우스를 움직일 때까지 형식 아이콘에 머문다
    /// (F-8 화면 확인에서 실제로 그랬다)
    fn poll_thumbnails(&mut self, ctx: &egui::Context) -> Option<Duration> {
        let arrived = self.thumbs.poll();
        let before = self.thumb_textures.len();
        self.thumb_textures.sync(ctx, &self.thumbs);
        // 프레임 상한(`MAX_NEW_THUMBS_PER_FRAME`)에 걸려 남은 것도 이 신호로 이어 올라간다
        if !arrived.is_empty() || self.thumb_textures.len() != before {
            return Some(Duration::ZERO);
        }
        self.thumbs.is_pending().then_some(THUMB_POLL_INTERVAL)
    }

    /// 실패 문구에 쓸 대상 폴더 이름 — 전체 경로는 길어 끝 이름만 보여준다
    fn pending_name(&self) -> String {
        self.pending_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.pending_dir.to_string_lossy().into_owned())
    }

    /// 메뉴·단축키에서 온 탐색 명령 (FR-12).
    /// 탭 스트립·주소창 클릭과 **같은 경로**를 타므로 동작이 갈리지 않는다
    pub fn new_tab(&mut self, ctx: &egui::Context) {
        // 새 탭은 연결을 접을 일이 없다
        self.handle_tab(TabAction::New, ctx);
    }

    /// 활성 탭을 닫는다. 그 탭이 이 패널의 마지막 원격 탭이었으면 접어야 할 연결을 돌려준다 (FR-32)
    pub fn close_tab(&mut self, ctx: &egui::Context) -> Option<ConnectionId> {
        self.handle_tab(TabAction::Close(self.tabs.active_index()), ctx)
    }

    /// 그 사이트를 가리키는 원격 탭을 모두 닫는다 — 사이드바에서 사이트를 지웠을 때 (FR-29).
    ///
    /// **모든 탭이 그 사이트라 마지막 하나를 닫지 못했으면 `true`**다. 그때 패널을 닫을지
    /// 로컬 탭으로 되돌릴지는 **앱이 정한다**(`ui::app::detach_site`) — 이 패널이 창의 마지막
    /// 하나인지는 여기서 알 수 없고, 마지막 패널은 닫히지 않기 때문이다(FR-2).
    ///
    /// 닫은 탭이 쓰던 연결은 돌려주지 않는다 — 사이트를 통째로 거두는 길이라 연결도 탭이
    /// 아니라 매니저에서 사이트로 고른다(`close_tab`은 그 반대라 연결을 돌려준다).
    /// `close_requested`도 세우지 않는다: 패널을 닫는 판단이 앱에 있어 여기서 세우면 두 번 닫는다
    pub fn close_site_tabs(&mut self, site: SiteId, ctx: &egui::Context) -> bool {
        // 닫을 때마다 뒤 탭의 자리가 당겨진다 — **매번 처음부터 다시 찾는다**
        loop {
            let found = self.tabs.sources().iter().position(
                |source| matches!(source, TabSource::Remote { site: at, .. } if *at == site),
            );
            let Some(index) = found else {
                return false;
            };
            let was_active = index == self.tabs.active_index();
            if let CloseOutcome::Removed(_) = self.tabs.close(index) {
                if was_active {
                    self.switch_active_tab(ctx);
                }
            } else {
                // 이 패널에 남은 탭이 그것 하나다 — 탭 목록은 비울 수 없다
                return true;
            }
        }
    }

    pub fn go_back(&mut self, ctx: &egui::Context) {
        self.handle_nav(NavAction::Back, ctx);
    }

    pub fn go_forward(&mut self, ctx: &egui::Context) {
        self.handle_nav(NavAction::Forward, ctx);
    }

    pub fn go_up(&mut self, ctx: &egui::Context) {
        self.handle_nav(NavAction::Up, ctx);
    }

    /// 보고 있는 폴더를 다시 읽는다 — 경로도 히스토리도 그대로다.
    ///
    /// **원격 탭에서는 로컬 열거를 걸지 않는다** — 원격 목록은 연결 워커에게 다시 물어야 하고
    /// (`request_remote_list`), 그 배선은 연결 화면과 함께 들어온다(T10)
    pub fn refresh(&mut self, ctx: &egui::Context) {
        let Some(dir) = self.tabs.active().source.local_path() else {
            return;
        };
        let dir = dir.to_path_buf();
        self.start_load(dir, PendingNav::None, ctx);
    }

    /// 지금 쓰는 보기 모드 — 메뉴가 현재 표시를 그리는 데 쓴다 (FR-23)
    pub fn view_mode(&self) -> ViewMode {
        self.list.view_mode()
    }

    /// 보기 모드를 바꾼다 (FR-23)
    pub fn set_view_mode(&mut self, mode: ViewMode) {
        self.list.set_view_mode(mode);
    }

    /// 활성 탭이 원격을 가리키는가 — 로컬에만 있는 일(열거·감시·썸네일·새 파일)이 이것으로 갈린다
    pub fn is_remote(&self) -> bool {
        self.tabs.active().source.is_remote()
    }

    /// 지금 보고 있는 탭의 신원 (FR-54) — 전송 대상을 정하는 쪽이 쓴다
    pub fn active_tab_id(&self) -> TabId {
        self.tabs.active_id()
    }

    /// 이 패널의 탭들 — **배경 탭까지** 훑어야 하는 곳이 쓴다.
    ///
    /// 업로드 하위 메뉴가 연결된 원격 탭을 모을 때가 그렇다(FR-8) — 화면에 보이는 활성 탭만
    /// 세면 배경으로 밀어 둔 원격에는 보낼 길이 없다
    pub fn tabs(&self) -> &[crate::panel::tabs::TabState] {
        self.tabs.tabs()
    }

    /// 그 신원의 탭이 가리키는 곳 — 이 패널에 없으면 `None` (FR-54).
    ///
    /// **배경 탭도 찾는다** — 전송 대상은 활성 탭에서 밀려나도 유지되므로(대상 sticky),
    /// 활성 탭만 보는 `remote_dir` 같은 조회로는 그 자리를 읽을 수 없다
    pub fn tab_source(&self, id: TabId) -> Option<&TabSource> {
        self.tabs
            .tabs()
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| &tab.source)
    }

    /// 그 신원의 탭이 **이 패널의 활성 탭**인가 (FR-54).
    ///
    /// 올리기 원본을 고를 때 쓴다 — 목록 선택은 패널마다 하나뿐이라 활성 탭의 것이다.
    /// 대상 탭이 배경으로 밀렸으면 그 탭의 선택은 화면에 없다
    pub fn is_active_tab(&self, id: TabId) -> bool {
        self.tabs.active_id() == id
    }

    /// 활성 탭이 쓰는 연결 — 로컬 탭이거나 아직 연결하지 않았으면 `None`
    pub fn active_conn(&self) -> Option<ConnectionId> {
        match &self.tabs.active().source {
            TabSource::Remote { conn, .. } => *conn,
            TabSource::Local(_) => None,
        }
    }

    /// 활성 탭이 가리키는 사이트 — 로컬 탭이면 `None`.
    /// 연결이 없어도 알 수 있다(세션에서 되살아난 탭이 그렇다 — `다시 연결`이 이것을 쓴다)
    pub fn active_site(&self) -> Option<SiteId> {
        match &self.tabs.active().source {
            TabSource::Remote { site, .. } => Some(*site),
            TabSource::Local(_) => None,
        }
    }

    /// 로컬 목록에서 고른 항목들 — 원격 탭이면 빈 벡터다 (FR-39 `올리기`)
    pub fn selected_local(&self) -> Vec<(PathBuf, bool)> {
        if self.is_remote() {
            return Vec::new();
        }
        self.list.selected_local()
    }

    /// 원격 목록에서 고른 항목들 — 로컬 탭이면 빈 목록이다 (FR-39).
    ///
    /// **우클릭 메뉴(`show_remote_menu`)와 단축키 라우팅이 함께 부른다** — 각자 고르면
    /// 메뉴와 단축키가 다른 항목에 명령을 걸 수 있다
    pub fn selected_remote(&self) -> Vec<RemoteTarget> {
        let Some(dir) = self.tabs.active().source.remote_path() else {
            return Vec::new();
        };
        self.list.selected_remote(dir)
    }

    /// 활성 탭이 보고 있는 원격 폴더 — 로컬 탭이면 `None`
    pub fn remote_dir(&self) -> Option<RemotePath> {
        self.tabs.active().source.remote_path().cloned()
    }

    /// 이 패널의 탭들이 쓰고 있는 연결들 — 패널을 닫을 때 회수 대상을 고르는 데 쓴다 (FR-32).
    /// 한 연결을 여러 탭이 나눠 쓸 수 있어 같은 값은 한 번만 담는다
    pub fn conns(&self) -> Vec<ConnectionId> {
        let mut conns = Vec::new();
        for source in self.tabs.sources() {
            if let TabSource::Remote {
                conn: Some(conn), ..
            } = source
                && !conns.contains(&conn)
            {
                conns.push(conn);
            }
        }
        conns
    }

    /// 이 패널의 탭 중 그 연결을 쓰는 것이 있는가 — 연결을 접어도 되는지 판정한다 (FR-32)
    pub fn uses_conn(&self, target: ConnectionId) -> bool {
        self.tabs.sources().iter().any(|source| {
            matches!(source, TabSource::Remote { conn: Some(conn), .. } if *conn == target)
        })
    }

    /// 사이트를 가리키는 **새 원격 탭**을 열고 활성으로 만든다 (FR-33·FR-34·FR-38).
    ///
    /// 연결 붙이기는 이어서 `attach_conn`이 한다 — 탭을 먼저 만드는 이유는 연결이 서기 전에도
    /// 그 자리에 탭이 보여야 사용자가 "열리고 있다"는 것을 알기 때문이다
    pub fn open_remote_tab(&mut self, site: SiteId, path: RemotePath) {
        self.tabs.add(TabState::remote(site, path));
        // 이 패널이 로컬 폴더를 읽는 중이었으면 그 결과는 갈 곳이 없다 — 활성 탭이 방금
        // 원격이 됐기 때문이다. 접지 않으면 `읽는 중…`이 남고, 도착한 결과가 원격 탭에
        // 커밋되려다 죽는다(개발 빌드) 또는 원격 탭을 로컬 탭으로 둔갑시킨다(배포 빌드)
        self.abandon_local_load();
    }

    /// 그 폴더를 **새 탭**에서 연다 — 우클릭 메뉴의 `새 탭에서 열기` (FR-3·FR-8 재개정).
    ///
    /// [`new_tab`](Self::new_tab)과 다른 점은 **열 곳을 받는다**는 것이다 — 그쪽은 보고 있는
    /// 곳을 복제하는 `Ctrl+T`의 길이라 대상을 고를 수 없다.
    ///
    /// `open_remote_tab`의 로컬 짝이며 그쪽처럼 **곧바로 읽지 않는다** — 활성 탭이 바뀌는
    /// 길이라 고치던 이름을 접어야 하고, 그 규칙은 `switch_active_tab` 하나에 있다
    pub fn open_local_tab(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.tabs.add(TabState::new(path));
        self.switch_active_tab(ctx);
    }

    /// 위와 같되 **원격 탭 하나만** 남긴다 — 갓 나뉘어 나온 패널에 쓴다.
    ///
    /// 분할로 만든 패널은 시작 폴더를 가리키는 로컬 탭 하나를 들고 태어나는데, 사용자가 청한
    /// 것은 연결 하나다. 그대로 두면 연결을 열 때마다 쓰지도 않을 탭을 손으로 닫아야 한다
    /// (사용자 보고). 이미 쓰고 있던 패널에는 쓰지 않는다 — 그 탭들은 사용자가 연 것이다
    pub fn open_remote_tab_only(&mut self, site: SiteId, path: RemotePath) {
        self.open_remote_tab(site, path);
        // 방금 만든 원격 탭이 활성이다. 그 앞에 있는 것이 태어날 때 딸려 온 탭이다
        while self.tabs.active_index() > 0 {
            self.tabs.close(self.tabs.active_index() - 1);
        }
    }

    /// 진행 중인 로컬 열거를 버리고 그에 딸린 상태(대기 경로·이동 방향·상태 문구)를 지운다
    fn abandon_local_load(&mut self) {
        self.load.cancel();
        self.pending_dir = PathBuf::new();
        self.pending_nav = PendingNav::None;
        self.streamed = false;
        self.optimistic = None;
        self.status.clear();
    }

    /// 활성 원격 탭에 연결을 붙이고 연결 중으로 표시한다 — 사이트를 막 열었을 때.
    /// 원격 탭이 아니면 아무 일도 하지 않고 `false`
    pub fn attach_conn(&mut self, id: ConnectionId) -> bool {
        let TabSource::Remote { conn, phase, .. } = &mut self.tabs.active_mut().source else {
            return false;
        };
        *conn = Some(id);
        *phase = TabPhase::Connecting;
        true
    }

    /// 그 연결을 쓰는 **모든 탭**의 단계를 바꾼다 — 워커의 단계 변화를 화면에 투영한다.
    ///
    /// 활성 탭만 바꾸지 않는 이유: 한 연결을 여러 탭이 나눠 쓸 수 있고(같은 서버의 다른 폴더),
    /// 배경 탭이 옛 단계로 남으면 그 탭으로 돌아갔을 때 화면이 실제와 어긋난다
    pub fn set_phase_for(&mut self, target: ConnectionId, next: &TabPhase) -> bool {
        let mut changed = false;
        for source in self.tabs.sources_mut() {
            if let TabSource::Remote {
                conn: Some(conn),
                phase,
                ..
            } = source
                && *conn == target
            {
                *phase = next.clone();
                changed = true;
            }
        }
        changed
    }

    /// 표시 중인 폴더에 새 폴더를 만든다 (FR-25).
    /// **원격 탭에서는 아무것도 하지 않는다** — 원격 폴더 만들기는 원격 파일 작업(T23)이 맡는다
    pub fn new_folder(&mut self, ctx: &egui::Context) {
        if self.is_remote() {
            return;
        }
        let dir = self.dir().to_path_buf();
        self.create.start(
            dir,
            crate::i18n::panel_kind_folder(),
            create::new_folder,
            ctx,
        );
    }

    /// 표시 중인 폴더에 빈 텍스트 문서를 만든다 (FR-25). 원격 탭에서는 하지 않는다
    pub fn new_file(&mut self, ctx: &egui::Context) {
        if self.is_remote() {
            return;
        }
        let dir = self.dir().to_path_buf();
        self.create.start(
            dir,
            crate::i18n::panel_kind_file(),
            create::new_text_file,
            ctx,
        );
    }

    /// 활성 원격 탭이 가리키는 위치를 옮긴다. 연결·단계는 그대로 둔다.
    ///
    /// 목록 다시 읽기는 **깃발을 세워** 앱에 맡긴다(`take_remote_dirty`) — 여기서 직접 보내지
    /// 않는 이유는 패널이 `ConnectionManager`를 쥐고 있지 않기 때문이다
    pub fn set_remote_path(&mut self, target: RemotePath) {
        let mut moved = false;
        if let TabSource::Remote { path, .. } = &mut self.tabs.active_mut().source {
            // 실패했을 때 돌아갈 자리를 남긴다 — 일련번호는 조회를 청할 때 붙는다 (F-7 리뷰 B2)
            self.pending_revert = Some(path.clone());
            moved = *path != target;
            *path = target;
            self.remote_dirty = true;
        }
        // 로컬과 같은 규칙 — 자리를 옮길 때만 필터를 비운다 (D2·D11). 같은 자리를 다시
        // 읽는 길(원격 메뉴의 새로 고침·F5)에서는 치던 글자를 그대로 둔다
        if moved {
            self.list.set_filter("");
        }
    }

    /// 원격 목록에서 더블클릭한 항목으로 들어간다.
    ///
    /// `..`는 위로, 그 밖의 폴더는 안으로. **파일은 열지 않는다** — 원격 파일을 여는 것은
    /// 받아서 로컬에서 여는 일이고(Out of Scope: 열어 편집 후 자동 업로드), 여기서 셸에
    /// 넘기면 있지도 않은 로컬 경로를 실행하게 된다
    fn open_remote_entry(&mut self, index: usize) {
        let Some(dir) = self.tabs.active().source.remote_path().cloned() else {
            return;
        };
        let Some(entry) = self.list.remote_at(index) else {
            return;
        };
        if !entry.is_dir {
            return;
        }
        let target = if entry.name == PARENT_ENTRY {
            match dir.parent() {
                Some(parent) => parent,
                // 루트에서는 그대로 머문다 — 위가 없다
                None => return,
            }
        } else {
            dir.join(&entry.name)
        };
        // 옮기고 나면 앱이 그 자리의 목록을 청한다(`take_remote_dirty`)
        self.set_remote_path(target);
    }

    /// 활성 탭이 **바뀌었을** 때 그 탭의 것을 다시 읽는다 (FR-64 Edge Case).
    ///
    /// `reload_active_tab`과 갈라 두는 이유는 하나다 — **고치던 이름을 접는 것은 탭이
    /// 바뀔 때뿐**이다. 표시 규칙(숨김·시스템)이 바뀌어 같은 탭·같은 폴더를 다시 읽는 길은
    /// 편집을 그대로 둬야 한다: 그 체크박스를 누르는 것은 마우스 조작이라 입력칸의 포커스를
    /// 뺏지 않는데, 거기서 편집을 버리면 입력하던 이름이 예고 없이 사라진다(품질 리뷰 M1).
    ///
    /// **폴더가 바뀌는지로는 가릴 수 없다** — 두 탭이 같은 폴더를 보고 있으면
    /// `FileListView::drop_rename_on_dir_change`가 아무 일도 하지 않아, 옮겨 간 탭에서
    /// 편집이 그대로 살아 있게 된다(spec 리뷰 N1).
    ///
    /// **감시 갱신(FR-10)도 이 길로 오지 않는다** — `poll_watch`가 `start_load`를 곧바로
    /// 불러, 폴더가 밖에서 바뀌어도 편집은 유지된다(plan Edge Case 그대로)
    fn switch_active_tab(&mut self, ctx: &egui::Context) {
        self.list.cancel_rename();
        // 탭이 바뀌면 필터를 비운다 (D2) — 옮겨 간 탭의 폴더에 옛 필터가 걸린 채로 보이면
        // 그 폴더가 비어 보이고 사용자는 이유를 알 수 없다
        self.list.set_filter("");
        self.reload_active_tab(ctx);
    }

    /// 활성 탭이 보는 곳을 다시 읽는다 (F-7 3라운드 B1).
    ///
    /// **목록은 탭이 아니라 패널 하나가 든다** — 그래서 탭만 바꾸고 목록을 그대로 두면
    /// 주소창은 이 탭을, 목록은 저 탭의 폴더를 보이게 된다. 그 위에서 연 원격 메뉴는
    /// **화면에 없는 경로**에 삭제·권한 변경을 걸 수 있다(로컬은 종전부터 다시 읽어 왔고,
    /// 원격만 빠져 있었다).
    ///
    /// **부르는 이유는 둘이며 그것이 편집 취소를 가른다** — 탭이 바뀌어 부르는 길은
    /// `switch_active_tab`을 거쳐 고치던 이름을 접고, 표시 규칙(숨김·시스템)이 바뀌어
    /// 같은 탭을 다시 읽는 `apply_display_rules`는 이 함수를 곧바로 불러 편집을 그대로 둔다.
    /// **이 함수 자체는 편집을 건드리지 않는다** — 그 판정은 부르는 쪽에 있다
    fn reload_active_tab(&mut self, ctx: &egui::Context) {
        match self.tabs.active().source.local_path() {
            Some(path) => {
                let path = path.to_path_buf();
                self.start_load(path, PendingNav::None, ctx);
            }
            None => {
                // 원격은 연결을 아는 앱이 청한다 — 여기서는 깃발만 세운다.
                // **답이 오기 전까지 목록을 비운다** — 옛 탭의 항목을 남겨 두면 그 몇 백 밀리초
                // 동안(또는 조회가 실패하면 계속) 주소창과 목록이 다른 곳을 가리키고,
                // 그 위에서 연 메뉴가 화면에 없는 경로에 삭제·권한 변경을 건다 (F-7 4라운드 M1)
                self.list.clear_entries();
                self.remote_dirty = true;
            }
        }
    }

    /// 그 일련번호의 조회가 실패해 옮기기를 무른다 — 자리가 없거나 **다른 요청의 것**이면 그대로 둔다.
    ///
    /// 지금 보고 있는 곳이 `옮겨 간 곳` 그대로일 때만 손댄다 — 그 사이 탭을 바꿨거나 다시
    /// 옮겼으면 이 되돌리기는 이미 과거의 것이다(F-7 2라운드 B2).
    /// 되돌린 뒤에는 **다시 읽지 않는다**(`remote_dirty`를 세우지 않는다) — 그 폴더의 목록은
    /// 이미 화면에 있고, 다시 청하면 실패한 조회와 성공한 조회가 번갈아 도는 고리가 된다
    pub fn revert_remote_path(&mut self, seq: u64) -> bool {
        let Some((waiting, previous, moved_to)) = self.revert_at.clone() else {
            return false;
        };
        if waiting != seq {
            return false;
        }
        let TabSource::Remote { path, .. } = &mut self.tabs.active_mut().source else {
            return false;
        };
        if *path != moved_to {
            return false;
        }
        *path = previous;
        self.revert_at = None;
        true
    }

    /// 옮긴 뒤 아직 다시 읽지 않았는가 — 앱이 프레임마다 거둬 `request_remote_list`로 잇는다
    pub fn take_remote_dirty(&mut self) -> bool {
        std::mem::take(&mut self.remote_dirty)
    }

    /// 활성 원격 탭의 목록을 요청한다. 돌려주는 값은 이번 요청의 일련번호다.
    ///
    /// **연결이 없으면 아무것도 보내지 않는다**(plan Edge Case) — 아직 연결하지 않은 탭에서
    /// 새로 고침을 눌러도 서버로 나가는 것이 없어야 한다.
    ///
    /// `workspace`·`panel`은 **이 패널이 자기 자리를 모르기 때문에** 밖에서 받는다 — 패널은
    /// 뷰의 맵에 담겨 있을 뿐 자기 키를 들고 있지 않다. 필드로 심지 않는 이유는 세션 직렬화
    /// 대상이 늘고 맵 키와 이중 관리가 되기 때문이다
    pub fn request_remote_list(
        &mut self,
        workspace: WorkspaceId,
        panel: PanelId,
        manager: &ConnectionManager,
    ) -> Option<u64> {
        self.send_remote_list(workspace, panel, manager, false)
    }

    /// 그 일련번호가 **자동 재조회로** 나간 것인가 (FR-67).
    ///
    /// 그 실패는 화면 알림을 띄우지 않는다 — 사용자가 아무것도 누르지 않았는데 주기마다
    /// 배너가 뜨면, 로그만 조용하고 화면은 더 시끄러운 기능이 된다
    pub fn is_quiet_request(&self, seq: u64) -> bool {
        self.quiet_request == Some(seq)
    }

    /// 자동 재조회용 목록 요청 (FR-67) — **성공 기록을 서버 로그에 남기지 않는다**.
    ///
    /// 위 `request_remote_list`와 나란히 두는 이유: 부르는 쪽에서 갈래를 고르는 편이
    /// 매 호출부에 `quiet` 인자를 늘어놓는 것보다 읽기 쉽다. 조용한 갈래는 한 곳만 쓴다
    pub fn request_remote_list_quiet(
        &mut self,
        workspace: WorkspaceId,
        panel: PanelId,
        manager: &ConnectionManager,
    ) -> Option<u64> {
        self.send_remote_list(workspace, panel, manager, true)
    }

    /// 위 둘의 공통 몸통 — `quiet`면 서버 로그에 성공 기록을 남기지 않게 청한다
    fn send_remote_list(
        &mut self,
        workspace: WorkspaceId,
        panel: PanelId,
        manager: &ConnectionManager,
        quiet: bool,
    ) -> Option<u64> {
        let TabSource::Remote {
            conn: Some(conn),
            path,
            ..
        } = &self.tabs.active().source
        else {
            return None;
        };
        let (conn, path) = (*conn, path.clone());
        self.remote_seq += 1;
        let seq = self.remote_seq;
        // 돌아갈 자리를 **이 요청에** 묶는다. 옮기기가 아닌 조회(새로 고침·작업 후 재조회)는
        // 돌아갈 곳이 없으므로 그 자리도 비운다 — 남겨 두면 그 실패가 엉뚱한 되돌리기를 부른다
        self.revert_at = self
            .pending_revert
            .take()
            .map(|previous| (seq, previous, path.clone()));
        // 마지막 요청이 어느 갈래였는지 기억한다 — 그 실패를 알릴지 가리는 데 쓴다.
        // 손으로 부른 조회가 그 자리를 덮으므로 옛 값이 남아 알림을 삼키지 않는다
        self.quiet_request = quiet.then_some(seq);
        let source = ListSource::Panel {
            workspace: workspace.0,
            panel: panel.0,
            seq,
        };
        manager
            .send(
                conn,
                ConnCommand::List {
                    source,
                    path,
                    quiet,
                },
            )
            .then_some(seq)
    }

    /// 이 패널이 그 목록 답을 기다리고 있는가 — 일련번호와 **보고 있는 위치**가 모두 맞아야 한다.
    ///
    /// **어느 패널의 답인지는 이제 `ListSource::Panel`이 가린다** — 종전에 경로까지 본 것은
    /// 번호가 패널마다 따로 세어져 겹쳤기 때문인데, 그 구분은 출처가 대신한다.
    ///
    /// **그런데도 경로를 계속 보는 이유는 따로 있다** — 번호는 요청을 *보낼 때* 오르는데
    /// 경로는 그 *전에* 바뀐다(`set_remote_path`는 깃발만 세운다). 그 깃발을 거두는 길에
    /// 활성 탭이 원격이 아니면 번호를 올리기 전에 빠져나가므로, 깃발만 소진되고 번호는
    /// 그대로 남는다. 번호만 보면 그 상태에서 **이전 위치의 답을 새 위치에 그린다**
    pub fn awaits_remote_list(&self, seq: u64, path: &RemotePath) -> bool {
        seq == self.remote_seq && self.tabs.active().source.remote_path() == Some(path)
    }

    /// 워커가 돌려준 원격 목록을 반영한다.
    ///
    /// 일련번호가 지금 것과 다르면 버린다(늦게 도착한 이전 폴더의 결과 — D7).
    /// **`..`는 언제나 첫 줄에 하나만 둔다** — 서버가 주기도 하고 안 주기도 해서 목록이
    /// 서버마다 달라 보이면 안 된다
    pub fn apply_remote_listed(
        &mut self,
        seq: u64,
        path: &RemotePath,
        entries: Vec<RemoteEntry>,
        icons: &mut IconCache,
    ) -> bool {
        if !self.awaits_remote_list(seq, path) {
            return false;
        }
        // 이 이동은 섰다 — 돌아갈 자리를 지운다. 남겨 두면 **나중의 무관한 조회 실패**가
        // 그 낡은 값으로 경로만 되돌린다(F-7 2라운드 B1)
        if matches!(&self.revert_at, Some((waiting, _, _)) if *waiting == seq) {
            self.revert_at = None;
        }
        self.list
            .set_remote_entries(path.clone(), with_parent_first(entries), icons);
        true
    }

    /// 생성 결과를 반영한다 — 실패하면 사유만 상태 줄에 보인다 (plan D12).
    ///
    /// 성공하면 **감시 여부와 무관하게** 폴더를 다시 읽는다. 감시(FR-10)가 살아 있으면 통지로도
    /// 갱신되지만, `DirWatcher`는 폴더 열기에 실패해도 조용히 끝나 그 실패가 밖으로 드러나지
    /// 않는다 — 감시 객체가 있다는 것만으로 통지를 믿으면, 감시가 죽은 위치에서 방금 만든
    /// 항목이 목록에 나타나지 않는다. 재열거 한 번이 그 침묵보다 싸다
    fn poll_create(&mut self, ctx: &egui::Context) {
        let Some((kind, result)) = self.create.poll() else {
            return;
        };
        match result {
            Ok(_) => {
                // 만드는 동안 사용자가 원격 탭으로 옮겨 갔을 수 있다 — 그때 활성 탭 기준으로
                // 다시 읽으면 빈 경로를 열거하게 된다. 로컬 탭일 때만 다시 읽는다
                if let Some(dir) = self.tabs.active().source.local_path() {
                    let dir = dir.to_path_buf();
                    self.start_load(dir, PendingNav::None, ctx);
                }
            }
            Err(error) => {
                self.status = crate::i18n::dynamic::create_failed(kind, &error.to_string());
            }
        }
    }

    /// 탭 스트립에서 올라온 조작 처리.
    ///
    /// 닫힌 탭이 쓰던 연결을 **이 패널에서 아무도 쓰지 않게 되면** 그 연결 식별자를 돌려준다 —
    /// 실제로 접는 것은 연결을 쥔 앱의 몫이다 (FR-32)
    fn handle_tab(&mut self, action: TabAction, ctx: &egui::Context) -> Option<ConnectionId> {
        match action {
            TabAction::Switch(index) => {
                if self.tabs.switch(index) {
                    // 탭마다 소스가 다르므로 전환하면 그 탭의 것을 다시 읽는다.
                    // 히스토리는 이미 그 탭의 것이라 손대지 않는다
                    self.switch_active_tab(ctx);
                }
            }
            TabAction::Close(index) => {
                // 보고 있던 탭을 닫을 때만 화면이 바뀐다 — 배경 탭을 닫으면 그대로 유지된다
                let was_active = index == self.tabs.active_index();
                // 닫기 전에 잡아 둔다 — 닫은 뒤에는 그 탭이 무엇을 쓰던 탭인지 알 수 없다
                let closing = self
                    .tabs
                    .sources()
                    .get(index)
                    .and_then(|source| match source {
                        TabSource::Remote { conn, .. } => *conn,
                        TabSource::Local(_) => None,
                    });
                if let CloseOutcome::Removed(_) = self.tabs.close(index) {
                    if was_active {
                        self.switch_active_tab(ctx);
                    }
                    // 이 패널의 마지막 원격 탭이었으면 연결을 접는다 (FR-32·README §3)
                    if let Some(conn) = closing
                        && !self.uses_conn(conn)
                    {
                        return Some(conn);
                    }
                } else {
                    // 패널의 마지막 탭이면 탭 대신 **패널**이 닫힌다 (탐색기·브라우저 관례).
                    // 그러지 않으면 ✕가 아무 반응도 하지 않는다 — 원격 탭은 늘 자기 패널에
                    // 혼자 열리므로(`open_remote_tab_only`) 연결을 닫을 길이 없어진다.
                    // 마지막 패널을 지키는 것은 앱이다 (FR-2)
                    self.close_requested = true;
                }
            }
            TabAction::New => {
                // 새 탭은 지금 보고 있는 곳을 복제해 연다 (탐색기 관례).
                //
                // **원격 탭은 복제하지 않는다** — 복제해 봐야 연결이 없는(`conn: None`) 원격
                // 탭이 서고, 사용자가 `다시 연결`을 누르기 전까지 목록이 빈 채로 남는다
                // (사용자 보고). 그 자리에는 시작 폴더를 가리키는 로컬 탭을 연다
                let path = match self.tabs.active().source.clone() {
                    TabSource::Local(path) => path,
                    TabSource::Remote { .. } => crate::ui::app::start_dir(),
                };
                self.tabs.add(TabState::new(path));
                // `start_load`를 곧바로 부르지 않는다 — 새 탭도 **활성 탭이 바뀌는 길**이라
                // 고치던 이름을 접어야 하고, 그 규칙은 이 함수 하나에 있다
                self.switch_active_tab(ctx);
            }
        }
        None
    }

    /// 주소창에서 올라온 탐색 요청 처리
    fn handle_nav(&mut self, action: NavAction, ctx: &egui::Context) {
        match action {
            NavAction::Back => {
                if let Some(path) = self.tabs.active().history.peek_back().map(PathBuf::from) {
                    self.start_load(path, PendingNav::Back, ctx);
                }
            }
            NavAction::Forward => {
                if let Some(path) = self.tabs.active().history.peek_forward().map(PathBuf::from) {
                    self.start_load(path, PendingNav::Forward, ctx);
                }
            }
            NavAction::Up => match &self.tabs.active().source {
                TabSource::Local(path) => {
                    if let Some(parent) = path.parent().map(PathBuf::from) {
                        self.navigate(parent, ctx);
                    }
                }
                // 원격은 원격 경로로 올라간다. **루트에서는 그대로 머문다** — `parent()`가
                // `None`을 주므로 아무것도 하지 않는 것이 곧 "루트에 머문다"이다
                TabSource::Remote { path, .. } => {
                    if let Some(parent) = path.parent() {
                        self.set_remote_path(parent);
                    }
                }
            },
            NavAction::Goto(path) => self.navigate(path, ctx),
            // 주소로 여는 일은 앱이 한다 — 여기서는 값만 받아 둔다
            NavAction::GotoRemote(url) => self.pending_remote_url = Some(url),
        }
    }

    /// 확정된 이름 바꾸기를 요청으로 바꾼다 (FR-64) — 대상이 아니면 `None`.
    ///
    /// 원격 목록에는 인라인 편집이 없어(FR-39는 대화로 묻는다) 로컬 탭에서만 성립한다.
    /// 행 번호로 항목을 다시 찾는 이유: 확정과 폴더 갱신이 같은 프레임에 겹칠 수 있어,
    /// 그때는 가리키던 항목이 없어져 아무것도 걸지 않는 편이 옳다
    fn take_rename(&self, action: &FileListAction) -> Option<RenameRequest> {
        let FileListAction::Rename { index, new_name } = action else {
            return None;
        };
        let dir = self.tabs.active().source.local_path()?;
        let entry = self.list.entry_at(*index)?;
        Some(RenameRequest {
            path: dir.join(entry.name_string()),
            new_name: new_name.clone(),
        })
    }

    /// 고른 것 중 첫 항목의 이름 편집을 연다 (FR-64) — 열지 못했으면 거짓.
    ///
    /// 메뉴의 아이콘 줄과 `F2`가 함께 쓰는 진입점이다
    pub fn begin_rename_selected(&mut self) -> bool {
        self.list.begin_rename_selected()
    }

    /// 잘라내기로 담긴 경로들을 이 패널의 목록에 표시한다 (FR-64)
    pub fn set_cut_marks(&mut self, paths: &[PathBuf]) {
        self.list.set_cut_marks(paths);
    }

    /// 잘라내기 표시를 푼다 — 붙여넣었거나 다른 것이 클립보드에 담겼을 때 (FR-64)
    pub fn clear_cut_marks(&mut self) {
        self.list.clear_cut_marks();
    }

    /// 목록에서 올라온 조작 처리. 셸 메뉴 요청은 **실행하지 않고 값으로 돌려준다**.
    ///
    /// `TrackPopupMenuEx`는 자체 메시지 루프를 돌려 winit 이벤트 루프를 재진입시킨다 —
    /// egui 위젯 트리가 절반만 구성된 상태에서 그 루프에 들어가면 안 되므로,
    /// 그리기 클로저를 전부 빠져나온 뒤에 띄워야 한다(`ExplorerApp::ui`가 처리)
    fn handle_list_action(
        &mut self,
        action: FileListAction,
        ctx: &egui::Context,
    ) -> Option<MenuRequest> {
        match action {
            FileListAction::None => None,
            FileListAction::Open(index) => {
                // 원격 항목에는 로컬 경로가 없다 — 폴더면 그 원격 폴더로 들어가고,
                // 파일은 아무 일도 하지 않는다(원격 파일 열기는 범위 밖 — 받아서 쓰면 된다)
                if self.is_remote() {
                    self.open_remote_entry(index);
                    return None;
                }
                let entry = self.list.entry_at(index)?;
                let dir = self.tabs.active().source.local_path()?;
                // 첫 줄(`..`)은 위 폴더로 간다. `dir.join("..")`을 그대로 넘기지 않는 이유는
                // 그 경로가 정리되지 않은 채 주소창·히스토리에 그대로 남기 때문이다
                if entry.is_parent() {
                    let parent = dir.parent()?.to_path_buf();
                    self.navigate(parent, ctx);
                    return None;
                }
                let target = dir.join(entry.name_string());
                if entry.is_dir {
                    self.navigate(target, ctx);
                } else {
                    shell_host::execute(&target);
                }
                None
            }
            FileListAction::Context { index, pos } => {
                // 행 메뉴는 선택 전체가 대상이다 — 여러 항목을 골라 한 번에 복사·삭제할 수 있다.
                // 빈 영역이면 항목 없이 요청해 폴더 배경 메뉴("새로 만들기")를 띄운다.
                //
                // **폴더 여부를 함께 싣는다** — 메뉴의 `즐겨찾기에 추가`·`새 탭에서 열기`가
                // 폴더에만 열리는데, 받는 쪽에서 `is_dir()`을 부르면 UI 스레드가 매번
                // 파일시스템을 묻게 된다(AGENTS 「UI 스레드에서 파일시스템 블로킹 호출」).
                // 목록은 열거할 때 이미 그 값을 알고 있다
                let (items, dirs): (Vec<_>, Vec<_>) = if index.is_some() {
                    self.list.selected_local().into_iter().unzip()
                } else {
                    (Vec::new(), Vec::new())
                };
                // 셸 메뉴는 로컬 전용이다 (D21) — 원격 목록의 우클릭은 자체 메뉴가 받는다
                let Some(folder) = self.tabs.active().source.local_path() else {
                    self.remote_menu_at = Some(pos);
                    return None;
                };
                let folder = folder.to_path_buf();
                Some(MenuRequest {
                    folder,
                    items,
                    dirs,
                    pos,
                })
            }
            // 이 갈래는 메뉴가 아니라 이름 바꾸기라 `handle_list_action`이 돌려주는 값에
            // 실리지 않는다 — 아래 `show`가 목록에서 곧바로 받아 `PanelOutcome`에 담는다
            FileListAction::Rename { .. } => None,
        }
    }

    /// 앱 설정이 정한 표시 규칙을 목록에 내려 준다 — **`show` 직전에 부른다**.
    ///
    /// `show`의 인자로 받지 않는 이유: 그리기 인자가 이미 일곱이라 하나만 더해도
    /// clippy가 막고, 무엇이 배치이고 무엇이 설정인지도 읽기 어려워진다. 목록이 스스로
    /// 설정을 읽게 하지 않는 이유는 `FileListView::set_show_extensions` 주석에 있다.
    ///
    /// 숨김·시스템 설정이 **바뀐 프레임에는 폴더를 다시 읽는다** — 목록이 원본을 쥐게 된
    /// 지금은 다시 읽지 않고도 되돌릴 수 있으나, 그 경로로 갈아 끼우는 것은 별개 변경이다
    /// (`FileListView::set_hidden_rules` 주석)
    pub fn apply_display_rules(&mut self, display: DisplayRules, ctx: &egui::Context) {
        self.list.set_show_extensions(display.show_extensions);
        // 트리도 같은 값을 받는다 — 목록에서만 사라지면 설정이 반만 듣는 것처럼 보인다
        self.tree
            .set_hidden_rules(display.show_hidden, display.show_system);
        if self
            .list
            .set_hidden_rules(display.show_hidden, display.show_system)
        {
            self.reload_active_tab(ctx);
        }
    }

    /// 이 패널에서 마지막으로 조작이 있었던 시각 (FR-67 · D10) — 자동 재조회가 본다
    pub fn last_input_at(&self) -> f64 {
        self.last_input_at
    }

    /// 이 패널을 지금 쓰고 있는지 재 둔다 (FR-67 · D10).
    ///
    /// **이 패널 위의 누름·스크롤과, 어디서 왔든 글쇠 입력을 함께 센다** — egui는 글쇠가
    /// 어느 패널의 것인지 알려 주지 않는다. 넉넉히 세는 쪽이 안전하다: 이 값은 재조회를
    /// **한 주기 미루는** 데만 쓰여 과하게 세면 목록이 조금 늦게 갱신될 뿐이다.
    ///
    /// **마우스가 지나가는 것만으로는 세지 않는다** — 읽느라 커서를 올려 둔 것뿐인데
    /// 갱신이 영영 멎으면 자동 갱신이 있으나 마나가 된다
    fn note_input(&mut self, ui: &egui::Ui) {
        let rect = ui.max_rect();
        let (now, touched) = ui.ctx().input(|input| {
            let over = input
                .pointer
                .interact_pos()
                .is_some_and(|at| rect.contains(at));
            let pointer =
                over && (input.pointer.any_down() || input.smooth_scroll_delta != egui::Vec2::ZERO);
            let typed = input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::Key { pressed: true, .. } | egui::Event::Text(_)
                )
            });
            (input.time, pointer || typed)
        });
        if touched {
            self.last_input_at = now;
        }
    }

    /// 패널 하나를 그리고 이번 프레임의 조작을 처리한다.
    /// 세로 구성은 탭 스트립 / 주소창 / 상태 줄 / (폴더 트리 | 파일 목록)이며,
    /// 트리를 숨기면 목록이 그 폭까지 차지한다 (FR-9).
    ///
    /// 셸 메뉴 요청과 분할 요청은 실행하지 않고 반환한다 — 셸 메뉴는 모달이라 그리기가 끝난 뒤에
    /// 띄워야 하고, 분할은 트리를 바꾸므로 이 패널을 그리는 도중에 할 수 없다
    // 인자가 여덟인 이유는 `show_layout`과 같다 — 그리기에 필요한 자원과 앱이 정한 규칙이
    // 그만큼이고, 묶어 넘기면 여기서 다시 풀어야 한다
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        icons: &mut IconCache,
        textures: &mut IconTextures,
        remote: RemoteView<'_>,
        menu_state: PanelMenuState,
        targets: TransferTargets,
        favorites: &[FavoriteEntry],
        drives: &[DriveRow],
    ) -> PanelOutcome {
        self.note_input(ui);
        let strip = crate::ui::tabs::show_tab_strip(ui, &self.tabs, remote, menu_state, targets);
        let tab = self.tabs.active();
        let address = self
            .address
            .show(ui, tab.committed(), &tab.history, self.list.filter());
        let nav = address.nav;
        if let Some(text) = address.filter {
            // 폴더를 다시 읽지 않는다 — 목록이 원본에서 그 자리에서 다시 만든다 (D1)
            self.list.set_filter(&text);
        }

        // 상태 줄은 트리 위까지 걸쳐 **패널 전폭**을 쓴다 — 트리 토글이 자기가 여는 트리
        // 위쪽에 서야 무엇을 여는 버튼인지 읽힌다 (2026-08-16 사용자 결정).
        // 연결되지 않은 원격 탭은 항목 수를 모른다 (인벤토리 #95)
        let connected = !matches!(
            &self.tabs.active().source,
            TabSource::Remote { phase, .. } if *phase != TabPhase::Ok
        );
        self.show_status_bar(ui, connected);
        ui.separator();

        // 트리는 상태 줄 아래 좌측 고정폭을 차지한다.
        // 여기서 나뉘므로 트리와 목록은 저절로 같은 높이에서 시작한다
        let area = ui.available_rect_before_wrap();
        let split_x = area.left() + TREE_WIDTH.min(area.width());
        let mut tree_outcome = None;
        let content = if self.tree_visible {
            let tree_rect = egui::Rect::from_min_max(area.min, egui::pos2(split_x, area.bottom()));
            ui.painter().rect_filled(tree_rect, 0.0, theme::SURFACE_BG);
            ui.painter().vline(
                split_x,
                area.y_range(),
                egui::Stroke::new(TREE_BORDER, theme::TREE_LINE),
            );
            // 원격 탭이면 지금 보는 곳의 최상단을 뿌리로 삼는다 (#94 — 툴팁도 함께 갈린다)
            let source = match self.remote_tree_root() {
                Some((conn, root)) => TreeSource::Remote {
                    conn,
                    root,
                    cache: remote.tree,
                },
                None => TreeSource::Local,
            };
            tree_outcome = ui
                .scope_builder(
                    // 이름을 붙이지 않으면 형제 영역과 같은 id를 갖게 되고, 그 안의 위젯 id까지
                    // 함께 겹친다(egui는 이름 없는 하위 영역에 전부 같은 이름을 준다)
                    egui::UiBuilder::new()
                        .id_salt("tree")
                        .max_rect(tree_rect.shrink(TREE_PAD)),
                    |ui| {
                        ui.set_clip_rect(tree_rect);
                        self.tree
                            .show(ui, source, favorites, drives, icons, textures)
                    },
                )
                .inner
                .into();
            egui::Rect::from_min_max(egui::pos2(split_x + TREE_BORDER, area.top()), area.max)
        } else {
            // 트리를 감추면 그 코드가 통째로 건너뛰어져 우클릭 메뉴가 스스로 닫히지 못한다 —
            // 여기서 비우지 않으면 다시 켤 때 옛 메뉴가 그대로 떠 있다 (FR-56)
            self.tree.close_menu();
            area
        };
        let (action, remote_action, drop, remote_menu) = ui
            .scope_builder(
                egui::UiBuilder::new().id_salt("content").max_rect(content),
                |ui| {
                    ui.set_clip_rect(content);
                    self.show_content(ui, icons, textures, remote, targets)
                },
            )
            .inner;

        // 탭·탐색은 여기서 바로 처리해도 된다(모달이 없다). 셸 메뉴만 호출부로 올려보낸다
        let mut tree_requests = Vec::new();
        let mut favorite = None;
        if let Some(outcome) = tree_outcome {
            // 즐겨찾기 목록은 앱이 하나만 들고 있으므로 조작은 값으로 올려보낸다
            favorite = outcome.favorite;
            match outcome.chosen {
                // 트리에서 고른 폴더로 목록이 이동한다 (Acceptance ⑤)
                Some(TreeChoice::Local(path)) => self.navigate(path, ctx),
                Some(TreeChoice::Remote(path)) => self.navigate_remote(path),
                None => {}
            }
            for request in outcome.requests {
                match request {
                    // 로컬 열거는 트리가 직접 워커를 띄운다 — 연결이 필요 없다
                    TreeRequest::Local(path) => self.tree.start_local_load(path, ctx),
                    // 원격은 앱이 보낸다 — 연결을 아는 것은 앱이다
                    remote => tree_requests.push(remote),
                }
            }
        }
        let closed_conn = strip
            .tab
            .and_then(|tab_action| self.handle_tab(tab_action, ctx));
        if let Some(nav) = nav {
            self.handle_nav(nav, ctx);
        }
        // 확정된 이름은 메뉴 요청과 갈래가 달라 먼저 꺼낸다 — 한 프레임에 둘이 함께
        // 일어나지 않으므로(편집 중에는 우클릭 메뉴가 열리지 않는다) 순서는 문제되지 않는다
        let rename = self.take_rename(&action);
        PanelOutcome {
            rename,
            menu: self.handle_list_action(action, ctx),
            // 드롭다운·드롭존에서 고른 사이트는 명령으로 올려 보낸다 —
            // 새 탭 생성·연결은 앱이 한다 (T13 착지 규약).
            // 마지막 탭을 닫은 프레임에는 패널 닫기가 우선이다 — 그 패널은 사라진다
            command: if std::mem::take(&mut self.close_requested) {
                Some(Command::ClosePanel)
            } else {
                strip.open_site.map(Command::OpenSiteTab).or(strip.command)
            },
            remote: remote_action,
            remote_url: self.pending_remote_url.take(),
            closed_conn,
            drop,
            remote_menu,
            favorite,
            tree_requests,
            drive_observed: self.observed_drive.take(),
        }
    }

    /// 원격 트리의 뿌리 — 활성 탭이 연결된 원격일 때만 있다.
    ///
    /// 지금 보는 경로를 부모로 계속 거슬러 올라간 최상단이다. `/`로 못 박지 않는 이유는
    /// 루트가 `/`가 아닌 서버가 있기 때문이다 (plan Edge Case)
    fn remote_tree_root(&self) -> Option<(ConnectionId, RemotePath)> {
        let conn = self.active_conn()?;
        let mut root = self.tabs.active().source.remote_path()?.clone();
        while let Some(parent) = root.parent() {
            root = parent;
        }
        Some((conn, root))
    }

    /// 트리에서 고른 원격 폴더로 목록을 옮긴다 (Acceptance ⑤).
    ///
    /// 상위 이동과 **같은 길**을 쓴다 — 옮기고 깃발을 세우면 앱이 다시 읽는다
    fn navigate_remote(&mut self, path: RemotePath) {
        self.set_remote_path(path);
    }

    /// 트리 토글 아이콘에 붙는 툴팁 — 아이콘만으로는 무엇의 트리인지 알 수 없다.
    ///
    /// 로컬·원격이 같은 자리를 쓰므로 문구가 갈린다 (인벤토리 #94)
    fn tree_toggle_tooltip(&self) -> &'static str {
        if self.is_remote() {
            crate::i18n::panel_remote_tree()
        } else {
            crate::i18n::panel_folder_tree()
        }
    }

    /// 패널 상태 줄 — 트리 토글·진행 상황과 항목 수를 **패널 전폭**에 둔다.
    ///
    /// 트리 위가 아니라 트리를 포함한 폭을 쓰는 이유는, 토글이 여는 것이 왼쪽 트리라서
    /// 버튼이 그 트리 위쪽에 있어야 무엇을 여는지 읽히기 때문이다 (2026-08-16 사용자 결정)
    fn show_status_bar(&mut self, ui: &mut egui::Ui, connected: bool) {
        // 클로저 밖에서 정한다 — `Sides`의 클로저가 `self`를 통째로 빌린다 (인벤토리 #94)
        let tree_tip = self.tree_toggle_tooltip();
        // 왼쪽에 트리 토글·진행 상황, 오른쪽 끝에 항목 수를 둔다 (사용자 요청 7).
        // `Sides`는 오른쪽 것을 먼저 자리잡게 하므로, 오류 문구가 길어져도 항목 수가 밀리지 않는다
        egui::Sides::new().show(
            ui,
            |ui| {
                if ui
                    .selectable_label(self.tree_visible, TREE_TOGGLE_ICON)
                    .on_hover_text(tree_tip)
                    .clicked()
                {
                    // `Sides` 클로저 안이라 `&mut self` 메서드를 부를 수 없어 필드를 직접 뒤집는다.
                    // 토글 진입점이 이 버튼 하나뿐이라 규칙이 흩어질 여지도 없다
                    self.tree_visible = !self.tree_visible;
                }
                if self.load.is_loading() {
                    ui.spinner();
                    ui.colored_label(theme::TEXT_MUTED, crate::i18n::tree_loading());
                }
                if !self.status.is_empty() {
                    ui.colored_label(theme::TEXT_MUTED, &self.status);
                }
            },
            |ui| {
                // 오른쪽 경계에 붙지 않게 띄운다 (2026-08-16 사용자 요청) —
                // 이 클로저는 오른쪽에서 왼쪽으로 쌓이므로 먼저 넣은 공백이 바깥 여백이 된다
                ui.add_space(COUNT_RIGHT_PAD);
                if !connected {
                    // 연결되지 않은 원격 패널은 항목 수를 모른다 — 0으로 보이면
                    // "빈 폴더"라는 없는 말을 하게 된다 (인벤토리 #95)
                    ui.colored_label(theme::TEXT_MUTED, remote_states::UNKNOWN_COUNT);
                } else if !self.load.is_loading() {
                    // 읽는 중에는 세지 않는다 — 이전 폴더의 수가 남아 새 폴더의 것처럼 보인다
                    let (dirs, files) = self.list.counts();
                    ui.colored_label(
                        theme::TEXT_MUTED,
                        crate::i18n::dynamic::item_counts(dirs, files),
                    );
                }
            },
        );
    }

    /// 상태 줄 아래의 본문 — 트리 오른쪽 자리를 채운다.
    ///
    /// 원격 탭의 본문은 **연결 단계에 따라 통째로 달라진다**(README §4·§5) — 아직 연결하지
    /// 않았으면 안내 문구(또는 `다시 연결`), 연결 중이면 자리 표시 막대, 실패했으면 사유와 조치,
    /// 연결됐으면 로컬과 같은 파일 목록이다
    fn show_content(
        &mut self,
        ui: &mut egui::Ui,
        icons: &mut IconCache,
        textures: &mut IconTextures,
        remote: RemoteView<'_>,
        targets: TransferTargets,
    ) -> ContentOutcome {
        // 단계를 먼저 떼어 둔다 — 아래에서 목록(`&mut self.list`)을 그리는 동안 탭을 빌릴 수 없다
        let phase = match &self.tabs.active().source {
            TabSource::Remote { phase, .. } => Some(phase.clone()),
            TabSource::Local(_) => None,
        };
        // 붙을 곳을 아는 탭인가 — 세션에서 되살아난 탭은 사이트를 알고 연결만 없다.
        // 사이트가 지워졌으면 알 수 없으므로 종전 안내를 그대로 보인다
        let site_known = matches!(
            &self.tabs.active().source,
            TabSource::Remote { site, .. } if remote.sites.get(*site).is_some()
        );
        match phase {
            Some(TabPhase::New) => {
                let action = if site_known {
                    remote_states::show_reconnect(ui).then_some(RemoteAction::Reconnect)
                } else {
                    remote_states::show_empty(ui);
                    None
                };
                (FileListAction::None, action, None, None)
            }
            Some(TabPhase::Connecting) => {
                // 취소는 자리 표시 막대 **위** 오른쪽에 둔다 (원본 `:223-228`의 상태 줄)
                let cancelled = ui
                    .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        remote_states::show_cancel(ui)
                    })
                    .inner;
                remote_states::show_skeleton(ui);
                (
                    FileListAction::None,
                    cancelled.then_some(RemoteAction::CancelConnect),
                    None,
                    None,
                )
            }
            Some(TabPhase::Error { message, kind }) => {
                let action =
                    remote_states::show_failed(ui, &message, kind).map(|chosen| match chosen {
                        FailedAction::Retry => RemoteAction::Retry,
                        FailedAction::OpenSettings => RemoteAction::OpenSettings,
                        FailedAction::ViewLog => RemoteAction::ViewLog,
                    });
                (FileListAction::None, action, None, None)
            }
            // 아직 아무것도 읽지 못한 채 읽는 중이면 목록 자리에 자리표시를 세운다 —
            // 빈칸을 보이면 "빈 폴더"라는 없는 말을 하게 되고, 시작 직후에는 그 빈칸이
            // 목록으로 바뀌는 것이 깜빡임으로 보인다(2026-08-14 사용자 보고)
            Some(TabPhase::Ok) | None if self.shows_loading_placeholder() => {
                remote_states::show_skeleton(ui);
                (FileListAction::None, None, None, None)
            }
            // 연결된 원격 탭은 로컬과 **같은 목록 부품**으로 그린다 (T8)
            Some(TabPhase::Ok) | None => {
                let (action, drop) = self.show_list(ui, icons, textures);
                // 메뉴는 목록을 그린 **뒤에** 띄운다 — 먼저 그리면 목록이 그 위를 덮는다
                let menu = self.show_remote_menu(ui, phase == Some(TabPhase::Ok), targets);
                (action, None, drop, menu)
            }
        }
    }

    /// 지금 보고 있는 곳이 **읽지 못한 그 폴더**면 목록 자리에 적을 말.
    ///
    /// 깃발이 아니라 경로로 견주므로, 탭을 옮기거나 원격을 보는 동안에는 저절로 꺼진다
    fn blocked_hint(&self) -> Option<&'static str> {
        match (&self.blocked, self.tabs.active().source.local_path()) {
            (Some((blocked, reason)), Some(here)) if blocked == here => Some(reason.hint()),
            _ => None,
        }
    }

    /// 파일 목록 본문 — 로컬 탭과 연결된 원격 탭이 함께 쓴다.
    ///
    /// 목록 위에서 시작한 끌기와 이 목록에 놓인 드롭도 여기서 다룬다 (FR-38) —
    /// 두 쪽 다 "지금 이 탭이 어디를 보고 있는가"를 알아야 해서 목록 부품이 아니라 패널의 몫이다
    fn show_list(
        &mut self,
        ui: &mut egui::Ui,
        icons: &mut IconCache,
        textures: &mut IconTextures,
    ) -> (FileListAction, Option<DropOutcome>) {
        // 이번 프레임에 화면에 보인 파일들을 받는다. `request`는 아직 없으면 만들라고
        // 시키고, 이미 있으면 최근 사용으로 올린다 — 보이는 썸네일이 축출되지 않게 하는
        // 유일한 지점이다(그리기는 텍스처만 보고 픽셀 캐시를 건드리지 않는다)
        let mut visible = Vec::new();
        let list_rect = ui.available_rect_before_wrap();
        let interaction = self
            .list
            .show(ui, icons, textures, &self.thumb_textures, &mut visible);
        for path in visible {
            self.thumbs.request(&path);
        }
        // 다 읽었는데 아무것도 없으면 그 사실을 적는다 (2026-08-16 검토).
        // **목록을 대신 그리지 않고 그 위에 얹는다** — 목록 자리가 그대로 있어야
        // 빈 폴더에 파일을 끌어다 놓을 수 있다.
        // 세는 것은 `counts()`다 — `..` 줄은 항목이 아니라 거기에 들지 않는다
        if self.list.counts() == (0, 0) && !self.load.is_loading() {
            // 비어 있어서인지 읽지 못해서인지, 읽지 못했다면 왜인지를 갈라 적는다
            // (2026-08-16·2026-08-17 사용자 요청)
            let hint = self
                .blocked_hint()
                .unwrap_or_else(crate::i18n::list_empty_folder);
            ui.painter().text(
                egui::pos2(
                    list_rect.center().x,
                    list_rect.top() + empty_hint_top(self.list.view_mode()),
                ),
                egui::Align2::CENTER_TOP,
                hint,
                egui::FontId::proportional(EMPTY_HINT_FONT_PX),
                theme::TEXT_MUTED,
            );
        }

        // 끌기 시작 — 무엇을 싣는지는 지금 보고 있는 곳이 정한다
        let started = interaction.drag_started.map(|index| {
            let source = &self.tabs.active().source;
            let remote_dir = source.remote_path().cloned();
            let source_site = match source {
                TabSource::Remote { site, .. } => Some(*site),
                TabSource::Local(_) => None,
            };
            (
                self.list.drag_items(index, remote_dir.as_ref()),
                source_site,
            )
        });
        // 커서를 따라오는 그림의 수명은 `drag_preview::next_item`이 쥔다 (FR-38 — plan D10).
        // **페이로드를 세우기 전에** 부르므로 시작 프레임의 `has_payload`는 거짓인데,
        // 그 함수가 시작 신호를 먼저 보므로 이번 프레임의 그림이 버려지지 않는다
        self.drag_preview = drag_preview::next_item(
            self.drag_preview.take(),
            started.as_ref().map(|(items, _)| items.as_slice()),
            egui::DragAndDrop::has_payload_of_type::<FileDrag>(ui.ctx()),
        );
        if let Some((items, source_site)) = started
            && !items.is_empty()
        {
            egui::DragAndDrop::set_payload(ui.ctx(), FileDrag { items, source_site });
        }
        if let Some(item) = &self.drag_preview {
            drag_preview::show(ui.ctx(), icons, textures, &self.thumb_textures, item);
        }

        // 드롭 — 이 목록 위에서 손을 놓았는가
        let drop = self.take_drop(ui, list_rect);
        (interaction.action, drop)
    }

    /// 원격 목록의 우클릭 메뉴를 그린다 (FR-39).
    ///
    /// 고른 것과 **그때의 대상 경로들**을 함께 올린다 — 대화가 뜨는 동안 선택이 바뀔 수 있어,
    /// 나중에 다시 읽으면 사용자가 고른 것과 다른 항목에 명령이 갈 수 있다
    fn show_remote_menu(
        &mut self,
        ui: &mut egui::Ui,
        connected: bool,
        targets: TransferTargets,
    ) -> Option<RemoteMenuPick> {
        let at = self.remote_menu_at?;
        if !self.is_remote() {
            // 원격 탭이 아니면 열 메뉴가 없다 — 요청도 함께 지운다
            self.remote_menu_at = None;
            return None;
        }
        // 단축키 라우팅(`ui::app::remote::route_to_remote`)과 **같은 함수**로 대상을 고른다 —
        // 두 길이 각자 고르면 메뉴와 단축키가 다른 항목에 명령을 걸 수 있다(품질 리뷰 S1)
        let picked = self.selected_remote();
        let mut chosen = None;
        // 창 가장자리에서 눌러도 메뉴가 밖으로 넘어가지 않게 안으로 당긴다 — 셸 메뉴는
        // OS가 해 주는 일이라(D21) 우리가 그리는 이쪽에서는 직접 해야 한다 (quality 리뷰 m1)
        let viewport = ui.ctx().input(|input| input.viewport_rect());
        let at = clamp_menu_pos(viewport, at, remote_menu::menu_size(ui.style()));
        let response = egui::Area::new(ui.id().with("원격 메뉴"))
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
                        chosen =
                            remote_menu::show_remote_menu(ui, picked.len(), connected, targets);
                    });
            })
            .response;
        // 바깥을 누르거나 Esc면 닫는다 — 메뉴가 화면에 눌어붙지 않게 한다
        let outside = ui.input(|input| {
            input.pointer.any_click()
                && input
                    .pointer
                    .interact_pos()
                    .is_none_or(|pos| !response.rect.contains(pos))
        });
        let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
        if chosen.is_some() || outside || escape {
            self.remote_menu_at = None;
        }
        chosen.map(|action| (action, picked))
    }

    /// 이 목록 위에 놓인 드롭을 거둔다 (FR-38).
    ///
    /// 놓은 자리가 이 패널의 목록 밖이면 아무것도 가져가지 않는다 — 페이로드를 남겨 두어야
    /// 실제로 놓인 패널이 가져간다
    fn take_drop(&mut self, ui: &egui::Ui, list_rect: egui::Rect) -> Option<DropOutcome> {
        let released = ui.input(|input| input.pointer.any_released());
        if !released {
            return None;
        }
        let pointer = ui.input(|input| input.pointer.interact_pos())?;
        if !list_rect.contains(pointer) {
            return None;
        }
        let drag = egui::DragAndDrop::take_payload::<FileDrag>(ui.ctx())?;
        let target = match &self.tabs.active().source {
            TabSource::Local(dir) => DropTarget::Local(dir.clone()),
            TabSource::Remote { site, path, .. } => DropTarget::Remote {
                site: *site,
                dir: path.clone(),
            },
        };
        Some(DropOutcome {
            items: drag.items.clone(),
            source_site: drag.source_site,
            target,
        })
    }
}

/// 원격 목록의 첫 줄을 언제나 상위 이동(`..`)으로 맞춘다.
///
/// 서버·프로토콜에 따라 `..`를 주기도 하고 안 주기도 한다 — 그대로 두면 같은 조작이
/// 서버마다 다르게 보인다. 여기서 **한 번만, 맨 앞에** 오도록 정리한다
fn with_parent_first(entries: Vec<RemoteEntry>) -> Vec<RemoteEntry> {
    let mut out = Vec::with_capacity(entries.len() + 1);
    out.push(RemoteEntry {
        name: PARENT_ENTRY.to_owned(),
        is_dir: true,
        is_symlink: false,
        link_target: None,
        size: 0,
        modified: None,
        mode: None,
        owner: None,
    });
    out.extend(
        entries
            .into_iter()
            .filter(|entry| entry.name != PARENT_ENTRY),
    );
    out
}

/// 로컬 목록에도 상위 이동(`..`) 줄을 얹는다 — 원격 목록과 같은 자리, 같은 조작이다.
///
/// **드라이브 루트(`C:\`)에서는 얹지 않는다** — 올라갈 곳이 없는데 줄만 있으면 눌러도
/// 아무 일이 없는 항목이 된다(원격은 서버 루트에서도 `..`를 주는 곳이 있어 그대로 두지만,
/// 로컬은 위가 있는지 여기서 곧바로 알 수 있다).
/// 열거는 `.`·`..`을 이미 걸러 내지만, 두 벌이 되지 않게 여기서도 한 번 더 거른다
fn with_local_parent_first(dir: &Path, entries: Vec<FileEntry>) -> Vec<FileEntry> {
    if dir.parent().is_none() {
        return entries;
    }
    let mut out = Vec::with_capacity(entries.len() + 1);
    out.push(FileEntry {
        // `FileEntry.name`은 널 종단 UTF-16이라는 불변식을 지킨다 — 정렬이 그 형태를 요구한다
        name: PARENT_ENTRY
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect(),
        is_dir: true,
        size: 0,
        modified: 0,
        // 실제 파일이 아니라 화면 장치다 — 어떤 필터에도 걸리지 않게 속성을 비운다.
        // 숨김 항목을 꺼도 `..`는 첫 줄에 남아야 한다 (FR-31)
        attributes: 0,
    });
    out.extend(entries.into_iter().filter(|entry| !entry.is_parent()));
    out
}
