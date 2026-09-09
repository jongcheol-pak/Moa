//! egui 앱 골격 — 창·폰트·팔레트·COM·셸 호스트와 전역 공유 자원을 보유한다.
//!
//! 실제 탐색은 `ui::panel::PanelState`가 담당하고, 그 패널들을 담은 분할 화면 한 벌이
//! `WorkspaceView`다. 이 구조체는 워크스페이스 목록(사이드바)과 뷰들을 잇는 그릇이다.
use crate::app::drives::DriveList;
use crate::app::favorites::FavoriteStore;
use crate::app::layout::TreeShape;
use crate::app::layout::{LayoutTree, PanelId, Rect as LayoutRect, SplitDir, SplitPlace};
use crate::app::settings::{
    AppSettings, SIDEBAR_DEFAULT_WIDTH, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, Session,
    SidebarSession, WindowState, remote_refresh_secs, save_session,
};
use crate::app::workspace::{WorkspaceId, WorkspaceList};
use crate::fs::icons::IconCache;
use crate::panel::dir_cache::DirCache;
use crate::panel::tabs::{TabId, TabSource};
use crate::remote::connection::{ConnectionId, TransferDirection};
use crate::remote::log::LogBuffer;
use crate::remote::manager::ConnectionManager;
use crate::remote::queue::TransferQueue;
use crate::remote::sites::SiteStore;
use crate::remote::transfer::TransferRunner;
use crate::remote::tree_cache::TreeCache;
use crate::remote::types::{RemotePath, SiteId};
use crate::ui::about_dialog::AboutDialog;
use crate::ui::app_icon;
use crate::ui::dialog;
use crate::ui::dock::{self, DockAction, DockPanel, DockState, DockView};
use crate::ui::font_scan::FontScan;
use crate::ui::icon_tex::IconTextures;
use crate::ui::license_dialog::LicenseDialog;
use crate::ui::list_common;
use crate::ui::log_panel;
use crate::ui::menu::{self, Command};
use crate::ui::panel::{DisplayRules, PanelState};
use crate::ui::queue_panel::{self, QueueAction};
use crate::ui::remote_states::{HostKeyGate, RemoteView};
use crate::ui::session::{self, PanelTabs, RemoteSnapshot, TabSpec, WorkspaceState};
use crate::ui::settings_dialog::{FontChoices, SettingsDialog};
use crate::ui::shell_host::ShellHost;
use crate::ui::sidebar::{SidebarAction, WorkspaceSidebar};
use crate::ui::site_manager::{FileRequest, SiteManager, SiteManagerOutcome};
use crate::ui::splitter;
use crate::ui::status_bar::{self, StatusAction, StatusView};
use crate::ui::tabs::TransferTargets;
use crate::ui::theme;
use crate::ui::titlebar::{self, WindowRequest};
use crate::ui::toast::{self, Toast};
use crate::ui::tray::{Tray, TrayEvent};
use crate::ui::tree::TreeRequest;
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};

mod autosave;
mod remote;
mod shell_menu;
mod transfer_conflict;
mod update;

use remote::{RelistPending, RemoteOps};
use transfer_conflict::ConflictCheck;

/// 맑은 고딕 — egui 기본 폰트에는 한글 글리프가 없어 파일명이 두부(□)로 보인다
const KOREAN_FONT_PATH: &str = r"C:\Windows\Fonts\malgun.ttf";
/// 타이틀바 앱 아이콘으로 올릴 원본 크기(px) — 20px 자리에 그리므로 고배율 화면에서도
/// 늘어나지 않게 한 단계 큰 항목을 쓴다
const APP_ICON_TEXTURE_PX: u32 = 48;

/// 삭제 확인 대화 폭 — 문구 두 줄이 접히지 않을 만큼
const REMOVE_DIALOG_WIDTH: f32 = 360.0;

/// 세션이 없을 때의 창 크기·위치 (첫 실행)
const DEFAULT_WINDOW: WindowState = WindowState {
    x: 0,
    y: 0,
    w: 1100,
    h: 700,
    maximized: false,
};

/// 세션의 최대화 상태를 되살릴 때 명령을 다시 걸어 볼 프레임 수.
///
/// 보통 한두 프레임이면 걸린다. 그럼에도 세는 것은 명령이 먹지 않는 환경에서 **영영
/// 창 크기를 저장하지 못하는 상태**로 남지 않기 위함이다 — 다 쓰면 평소 추적으로 돌아간다
const MAXIMIZE_RETRY_FRAMES: u8 = 30;

/// UI 스레드의 COM 아파트먼트 상태.
/// 셸 컨텍스트 메뉴(`IContextMenu`)는 STA를 요구하므로 셸 연동 가용 여부가 여기서 갈린다
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ComStatus {
    /// STA 확보 — 이 프로세스가 초기화했거나(S_OK) 이미 초기화돼 있었다(S_FALSE)
    Sta { owned: bool },
    /// 다른 아파트먼트로 이미 초기화됨 — 셸 메뉴 사용 불가
    WrongApartment,
    /// 그 외 실패
    Failed,
}

impl ComStatus {
    /// 셸 메뉴를 띄울 수 있는 상태인가.
    /// 실패 원인(`WrongApartment`/`Failed`)은 사용자가 취할 행동이 같아 화면에서는 구분하지 않는다
    pub fn is_available(self) -> bool {
        matches!(self, ComStatus::Sta { .. })
    }
}

/// COM을 STA로 초기화한다. 반환을 세 갈래로 처리한다 —
/// `S_OK`(이번에 초기화)·`S_FALSE`(이미 초기화됨) 모두 STA 확보로 보고 진행한다.
pub fn init_com() -> ComStatus {
    // 안전성: UI 스레드에서 1회 호출. 인자는 정적 상수이며 반환 HRESULT로만 분기한다
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if hr.is_ok() {
        // S_FALSE면 이미 초기화된 상태라 이 프로세스가 해제 책임을 지지 않는다
        ComStatus::Sta {
            owned: hr == windows::Win32::Foundation::S_OK,
        }
    } else if hr == RPC_E_CHANGED_MODE {
        ComStatus::WrongApartment
    } else {
        ComStatus::Failed
    }
}

/// 이 프로세스가 초기화한 경우에만 COM을 해제한다.
///
/// # Safety
///
/// `init_com`이 `S_OK`를 받은 **같은 스레드에서 1회만** 호출해야 한다.
/// 다른 스레드에서 부르거나 중복 호출하면 COM 참조 계수가 어긋난다.
pub unsafe fn uninit_com(com: ComStatus) {
    if let ComStatus::Sta { owned: true } = com {
        unsafe { CoUninitialize() };
    }
}

/// 앱이 쓰는 글꼴을 등록한다 — 한글 본문 글꼴과 아이콘 글꼴(Phosphor).
///
/// `family`가 있으면 그 글꼴을, 없거나 읽지 못하면 **맑은 고딕**을 쓴다 (FR-48).
/// 사용자가 고른 글꼴이 나중에 지워졌을 때 화면이 두부(□)로 덮이지 않게 하는 폴백이며,
/// **설정 값은 건드리지 않는다** — 글꼴을 다시 설치하면 되살아나야 한다.
///
/// 반환값은 **한글 글꼴 적용 여부**다. 어느 것도 읽지 못해도 아이콘 글꼴은 등록되므로
/// 타이틀바 버튼은 그대로 보인다(파일명만 기본 글꼴로 표시된다).
/// 등록은 한 번에 끝낸다 — `set_fonts`를 두 번 부르면 뒤엣것이 앞엣것을 덮어쓴다.
///
/// **반영은 다음 pass부터다**(egui `Context::set_fonts` 문서) — 부르는 쪽이 그 프레임을
/// 보장하려면 `ctx.request_repaint()`를 함께 부른다
pub fn install_fonts(ctx: &egui::Context, family: Option<&str>) -> bool {
    let mut fonts = egui::FontDefinitions::default();
    // 고른 글꼴은 face 인덱스까지 함께 온다 — 모음 글꼴(굴림·바탕 등)은 파일 하나에
    // 여러 글꼴이 들어 있어 인덱스가 없으면 언제나 첫 번째만 나온다
    let picked = family.and_then(crate::app::fonts::load_font);
    let loaded = match picked {
        Some(font) => Some((font.bytes, font.index)),
        // 기본 글꼴(맑은 고딕)은 단일 파일이라 인덱스가 0이다
        None => std::fs::read(KOREAN_FONT_PATH).ok().map(|bytes| (bytes, 0)),
    };
    let korean = match loaded {
        Some((bytes, index)) => {
            fonts.font_data.insert(
                "malgun".to_owned(),
                Arc::new(egui::FontData {
                    font: bytes.into(),
                    index,
                    tweak: Default::default(),
                }),
            );
            // 기본 폰트보다 앞에 두어 한글이 우선 매칭되게 한다
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "malgun".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("malgun".to_owned());
            true
        }
        None => false,
    };
    // 아이콘 글꼴은 exe에 정적으로 담겨 있어 실패 경로가 없다 — 한글 글꼴 성공 여부와 무관하게 등록한다
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
    korean
}

/// 닫기 요청을 창 숨김으로 바꿔야 하는가 (FR-50).
///
/// 셋 다 참일 때만 숨긴다:
/// - 사용자가 `종료` 토글을 켰다
/// - 트레이 아이콘이 **실제로** 올라가 있다 — 없는데 숨기면 창을 되부를 방법이 사라진다
/// - 트레이 메뉴 `종료`로 끝내는 중이 아니다 (그건 진짜 종료다)
///
/// 판정만 떼어 둔 이유: `ViewportCommand` 전송은 시험할 수 없지만 이 판정은 할 수 있다
fn should_hide_on_close(tray_on_close: bool, tray_present: bool, quitting: bool) -> bool {
    tray_on_close && tray_present && !quitting
}

/// 워크스페이스 한 벌의 탐색 상태 — 분할 트리와 그 패널들 (FR-17).
///
/// 워크스페이스마다 이것을 하나씩 갖고, **처음 선택될 때 비로소 만들어진다**(D1 지연 생성).
/// 한 번도 열지 않은 워크스페이스는 패널도 열거 스레드도 없다
pub struct WorkspaceView {
    /// 분할 트리 — 어느 패널이 화면 어디를 차지하는지 (FR-1)
    layout: LayoutTree,
    /// 패널 실체. 트리는 `PanelId`만 알고 상태는 여기에 있다
    panels: HashMap<PanelId, PanelState>,
    active: PanelId,
    /// 마지막으로 누른 패널의 활성 탭이 로컬이었을 때 그 탭 — **받기 목적지**다 (FR-54).
    ///
    /// 패널이 아니라 **탭**을 기억한다: 한 패널에 로컬 탭과 원격 탭이 섞여 있을 때
    /// (원격 탭을 나눌 자리가 없어 같은 패널에 열린 경우) 패널만 기억하면 원격 탭을 보는
    /// 순간 받기 대상이 사라져 받기가 영영 비활성이 된다.
    /// 세션에는 담지 않는다 — 다시 켜면 아래 폴백 규칙이 곧바로 합리적인 값을 준다
    last_local_tab: Option<TabId>,
    /// 같은 규칙의 원격 쪽 — **올리기 목적지**다 (FR-54)
    last_remote_tab: Option<TabId>,
}

impl WorkspaceView {
    fn new(start: PathBuf) -> WorkspaceView {
        let (layout, first) = LayoutTree::new();
        let mut panels = HashMap::new();
        panels.insert(first, PanelState::new(start));
        WorkspaceView {
            layout,
            panels,
            active: first,
            // 전송 대상은 첫 조회에서 폴백 규칙이 정한다 (FR-54)
            last_local_tab: None,
            last_remote_tab: None,
        }
    }

    /// 지정한 패널을 나눈다. 새 패널은 원래 패널과 같은 폴더에서 시작한다.
    ///
    /// 패널마다 있는 분할 버튼은 **자기 패널**을 대상으로 하므로 활성 패널과 다를 수 있다 (D3)
    /// 나뉘었으면 새 패널의 id — **공간이 부족하면 `None`**이다.
    ///
    /// 반환하는 이유: 연결 시 좌우 분할(FR-35)은 분할이 서지 못했을 때 현재 패널의 새 탭으로
    /// 물러서야 하는데, 성공 여부를 알 수 없으면 그 판단을 할 수 없다 (T14 Acceptance ④)
    fn split_panel(
        &mut self,
        target: PanelId,
        dir: SplitDir,
        place: SplitPlace,
        area: LayoutRect,
    ) -> Option<PanelId> {
        // 원격 탭을 보고 있는 패널은 로컬 자리가 없다 — `dir()`이 빈 경로다.
        // 그대로 물려주면 새 패널이 열거할 수 없는 곳을 가리켜 목록이 빈 채로 선다
        // (사용자 보고) — 그때는 시작 폴더에서 시작한다
        let start = self
            .panels
            .get(&target)
            .map(|p| p.dir().to_path_buf())
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or_else(start_dir);
        // 공간이 부족하면 나눌 수 없다 (사용자는 창을 키우면 된다)
        let new_id = self.layout.split(target, dir, place, area).ok()?;
        self.panels.insert(new_id, PanelState::new(start));
        self.active = new_id;
        Some(new_id)
    }

    /// 지정한 패널을 닫는다. 마지막 하나는 닫히지 않는다 (FR-2).
    ///
    /// 닫은 **자리를 흡수한 패널**을 다음 활성으로 삼는다 — 트리 순서상 첫 패널을 고르면
    /// 포커스가 화면 반대편으로 튀어 방금 조작한 위치와 멀어진다.
    ///
    /// 대상이 활성 패널과 다를 수 있다 — 패널 메뉴는 자기 패널을 가리키기 때문이다 (plan D16)
    fn close_panel(&mut self, target: PanelId, area: LayoutRect) {
        let closed = self
            .layout
            .compute_rects(area)
            .panes
            .iter()
            .find(|(id, _)| *id == target)
            .map(|(_, rect)| *rect);
        if self.layout.close(target).is_err() {
            return;
        }
        self.panels.remove(&target);
        let next = closed
            .and_then(|closed| {
                self.layout
                    .compute_rects(area)
                    .panes
                    .into_iter()
                    .max_by_key(|(_, rect)| overlap_area(*rect, closed))
                    .map(|(id, _)| id)
            })
            .or_else(|| self.layout.panel_ids().first().copied());
        if let Some(next) = next {
            self.active = next;
        }
    }

    /// 저장된 상태로 워크스페이스를 되살린다 (FR-11·FR-20).
    /// 패널은 분할 트리 리프의 walk 순서로 짝지어진다(`settings` 스키마 계약)
    fn from_state(state: &WorkspaceState) -> WorkspaceView {
        let (layout, ids) = LayoutTree::from_shape(&state.shape);
        let Some(&first) = ids.first() else {
            return WorkspaceView::new(start_dir());
        };
        let mut panels = HashMap::new();
        for (&id, tabs) in ids.iter().zip(&state.panels) {
            let panel = PanelState::from_tabs(tabs).unwrap_or_else(|| PanelState::new(start_dir()));
            panels.insert(id, panel);
        }
        WorkspaceView {
            layout,
            panels,
            active: ids.get(state.active_panel).copied().unwrap_or(first),
            // 세션에 담지 않는 값이라 되살릴 것이 없다 — 첫 조회의 폴백이 정한다 (FR-54)
            last_local_tab: None,
            last_remote_tab: None,
        }
    }

    /// 세션 저장용 스냅숏 — 분할 구조와 패널별 탭을 리프 walk 순서로 담는다
    fn to_state(&self, name: String) -> WorkspaceState {
        let ids = self.layout.panel_ids();
        let panels = ids
            .iter()
            .map(|id| match self.panels.get(id) {
                Some(panel) => panel.to_tabs(),
                // 트리에는 있는데 상태가 없는 리프 — 분할 직후가 아니면 생기지 않는다
                None => PanelTabs {
                    tabs: vec![TabSpec::Local(start_dir())],
                    active_tab: 0,
                    ..Default::default()
                },
            })
            .collect();
        WorkspaceState {
            name,
            shape: self.layout.shape(),
            panels,
            active_panel: ids.iter().position(|id| *id == self.active).unwrap_or(0),
        }
    }

    /// 사이드바 부제에 쓸 현재 폴더 — 활성 패널의 활성 탭 경로
    fn active_dir(&self) -> Option<PathBuf> {
        self.panels.get(&self.active).map(|p| p.dir().to_path_buf())
    }

    /// 내용이 직접 눌린 패널을 전송 대상으로 삼는다 (FR-54).
    ///
    /// 부르는 쪽이 `LayoutOutcome::pressed_panel`로 걸러 주므로 **팝업에 가린 클릭은 오지
    /// 않는다** — 그 판정을 여기서 다시 하지 않는 이유는 가림 여부를 아는 것이 그리기 쪽뿐이기
    /// 때문이다. 종류가 다른 쪽(로컬을 눌렀을 때의 원격 대상)은 그대로 둔다
    fn note_pressed(&mut self, panel: PanelId) {
        let Some(state) = self.panels.get(&panel) else {
            return;
        };
        let id = state.active_tab_id();
        if state.is_remote() {
            self.last_remote_tab = Some(id);
        } else {
            self.last_local_tab = Some(id);
        }
    }

    /// 지금의 전송 대상 — 사라진 탭을 가리키고 있으면 먼저 되돌린다 (FR-54)
    fn transfer_targets(&mut self) -> TransferTargets {
        self.last_local_tab = self.resolve_target(self.last_local_tab, false);
        self.last_remote_tab = self.resolve_target(self.last_remote_tab, true);
        TransferTargets {
            download: self.last_local_tab,
            upload: self.last_remote_tab,
            can_download: self.download_dir().is_some(),
            can_upload: self.upload_dir().is_some() && !self.upload_source().is_empty(),
        }
    }

    /// 기억해 둔 대상이 아직 살아 있으면 그대로, 아니면 **활성 탭이 그 종류인 첫 패널**로.
    ///
    /// 폴백이 활성 탭만 보는 이유: 아무 배경 탭이나 집으면 화면에서 아이콘을 찾기 어렵고,
    /// 올리기 원본도 활성 탭의 선택에서만 나온다
    fn resolve_target(&self, current: Option<TabId>, remote: bool) -> Option<TabId> {
        if let Some(id) = current
            && self.tab_source(id).is_some()
        {
            return Some(id);
        }
        self.layout
            .panel_ids()
            .into_iter()
            .filter_map(|id| self.panels.get(&id))
            .find(|panel| panel.is_remote() == remote)
            .map(PanelState::active_tab_id)
    }

    /// 그 신원의 탭이 가리키는 곳 — 어느 패널에 있든 찾는다 (배경 탭 포함)
    fn tab_source(&self, id: TabId) -> Option<&TabSource> {
        self.panels.values().find_map(|panel| panel.tab_source(id))
    }

    /// 받기 목적지 — 받기 아이콘이 붙은 탭의 폴더 (FR-54)
    fn download_dir(&self) -> Option<PathBuf> {
        let source = self.tab_source(self.last_local_tab?)?;
        source.local_path().map(Path::to_path_buf)
    }

    /// 올리기 목적지 — 올리기 아이콘이 붙은 탭의 사이트와 원격 폴더 (FR-54).
    ///
    /// **연결된 탭만** 대상이 된다: 연결이 없으면 목록을 조회할 수도 파일을 보낼 수도 없다
    fn upload_dir(&self) -> Option<(SiteId, RemotePath)> {
        match self.tab_source(self.last_remote_tab?)? {
            TabSource::Remote {
                site,
                conn: Some(_),
                path,
                ..
            } => Some((*site, path.clone())),
            _ => None,
        }
    }

    /// 업로드 하위 메뉴에 세울 원격 탭들 (FR-8·FR-39).
    ///
    /// **워크스페이스 안 모든 패널의 모든 탭**을 훑는다 — 배경 탭도 대상이다(2026-08-28
    /// 사용자 결정). 화면에 보이는 것만 세면 배경으로 밀어 둔 원격에는 보낼 길이 없다.
    ///
    /// **연결된 탭만** 담는다(`conn: Some(_)`) — 연결이 없으면 파일을 보낼 수 없다
    /// (`upload_dir`의 기존 판정과 같은 기준). 사이트가 사라진 탭도 뺀다(보낼 곳을 모른다).
    ///
    /// `sites`를 인자로 받는 이유: 탭 순회에 필요한 `panels`는 여기(`WorkspaceView`)에 있고
    /// 사이트 이름은 `ExplorerApp`이 든다 — 한 쪽만으로는 라벨을 만들 수 없다
    fn upload_menu_targets(&self, sites: &SiteStore) -> Vec<UploadTarget> {
        self.layout
            .panel_ids()
            .into_iter()
            .filter_map(|id| self.panels.get(&id))
            .flat_map(|panel| panel.tabs().iter())
            .filter_map(|tab| upload_target_of(&tab.source, sites))
            .collect()
    }

    /// 올릴 것 — **받기 대상 탭이 활성인 패널**의 로컬 선택 (FR-54).
    ///
    /// 그 탭이 배경으로 밀려 있으면 빈 벡터다 — 목록 선택은 패널마다 하나뿐이라 배경 탭의
    /// 선택은 화면에 남아 있지 않다
    fn upload_source(&self) -> Vec<(PathBuf, bool)> {
        let Some(id) = self.last_local_tab else {
            return Vec::new();
        };
        self.panels
            .values()
            .find(|panel| panel.is_active_tab(id))
            .map(PanelState::selected_local)
            .unwrap_or_default()
    }
}

/// 업로드 하위 메뉴의 한 줄 — 어느 원격 탭의 어느 폴더로 보낼 것인가 (FR-8·FR-39)
#[derive(Debug, Clone, PartialEq)]
pub(super) struct UploadTarget {
    pub site: SiteId,
    pub dir: RemotePath,
    /// 화면에 적을 문구 — `upload_menu_label`이 만든다
    pub label: String,
}

/// 탭 하나를 업로드 대상으로 바꾼다 — 대상이 아니면 `None` (FR-8·FR-39).
///
/// **순회에서 떼어 둔 순수 판정**이라 탭을 직접 만들어 시험할 수 있다(패널을 세우지 않고).
///
/// 빠지는 것 셋: 로컬 탭 · **연결이 없는 원격 탭**(보낼 수 없다 — `upload_dir`의 기존 기준) ·
/// 사이트가 사라진 탭(보낼 곳을 모른다)
fn upload_target_of(source: &TabSource, sites: &SiteStore) -> Option<UploadTarget> {
    let &TabSource::Remote {
        site,
        conn: Some(_),
        ref path,
        ..
    } = source
    else {
        return None;
    };
    // 이름은 그때그때 읽는다 — `이름 바꾸기`로 바뀌는데 사본을 들면 옛 이름이 남는다
    let name = sites.get(site)?.name.clone();
    Some(UploadTarget {
        site,
        dir: path.clone(),
        label: upload_menu_label(&name, path),
    })
}

/// 업로드 하위 메뉴 한 줄의 문구 (2026-08-28 사용자 결정).
///
/// **루트를 보고 있으면 사이트 이름만**, 하위 폴더면 사이트 이름과 **마지막 폴더 이름만**
/// 적는다 — 경로 전체를 적으면 깊은 곳에서 줄이 길어져 어느 서버인지가 묻힌다.
///
/// 클립보드·전송에 쓰이는 값이 아니라 **보여 주는 글자**라 여기서 만든다
pub(super) fn upload_menu_label(site_name: &str, path: &RemotePath) -> String {
    match path.file_name() {
        Some(folder) => format!("{site_name} › {folder}"),
        // 루트(`/`)이거나 이름을 뽑을 수 없는 경로 — 사이트 이름만으로 충분하다
        None => site_name.to_owned(),
    }
}

/// 탐색기 앱 상태.
pub struct ExplorerApp {
    com: ComStatus,
    /// 셸 메뉴용 창 핸들 — HWND를 얻지 못하면 `None`(셸 메뉴 비활성)
    shell: Option<ShellHost>,
    /// 한글 폰트 적용 여부 — 실패 시 화면에 알린다
    korean_font: bool,
    /// 아이콘 캐시 — 앱 전역 공유 (패널마다 두면 같은 아이콘을 중복 보관하게 된다)
    icons: IconCache,
    /// 폴더 목록 캐시 — 앱 전역 1벌 (FR-68).
    ///
    /// 패널마다 두면 메모리가 패널·워크스페이스 수만큼 곱해지고, 탭·패널을 옮겨 다닐 때
    /// 적중하지 않는다. 전달 방식은 `icons`와 같다 — 프레임마다 `poll`에 빌려준다
    dir_cache: DirCache,
    textures: IconTextures,
    /// 타이틀바 왼쪽에 그릴 앱 아이콘 — 시작할 때 한 번 올린다.
    /// 읽지 못하면 `None`(그 자리를 비워 두고 나머지는 그대로 그린다)
    app_icon: Option<egui::TextureHandle>,
    /// 워크스페이스 목록(이름·부제·활성) — 표시 데이터의 정본 (FR-15)
    workspaces: WorkspaceList,
    /// 워크스페이스별 탐색 상태 — **방문한 것만** 들어 있다 (D1).
    /// 목록의 인덱스가 아니라 `WorkspaceId`로 잡는다: 순서 변경·삭제로 인덱스는 흔들린다
    views: HashMap<WorkspaceId, WorkspaceView>,
    /// 세션에서 불러왔지만 **아직 열지 않은** 워크스페이스의 저장 상태 (D1 지연 생성).
    /// 처음 선택될 때 이것으로 뷰를 만들고, 그 전에는 저장할 때 그대로 다시 내보낸다
    restored: HashMap<WorkspaceId, WorkspaceState>,
    sidebar: WorkspaceSidebar,
    /// 마지막으로 관측한 사이드바 폭 — 세션 저장용
    sidebar_width: f32,
    sidebar_collapsed: bool,
    /// 무수식 키(`F2`·`Delete`)를 받는 영역 (FR-12) — 마지막으로 누른 쪽이 갖는다.
    ///
    /// 세션에 담지 않는다: 앱을 다시 띄우면 아무것도 누르지 않은 상태이므로 기본값이 옳다
    key_owner: menu::KeyOwner,
    /// 마지막으로 관측한 창 위치·크기 (최대화가 아닐 때만 갱신 — 최대화 상태를 저장하면
    /// 다음 실행에서 창을 되돌릴 "일반 크기"가 사라진다)
    window: WindowState,
    /// 첫 프레임에 화면 안으로 보정할 복원 위치 — 모니터 크기는 창이 뜬 뒤에야 알 수 있다
    restore_window: Option<WindowState>,
    /// 세션이 최대화였는데 아직 그 상태로 만들지 못했다면 **남은 시도 프레임 수**.
    ///
    /// **최대화는 창을 만들 때가 아니라 첫 프레임을 그린 뒤에 건다** — 창 생성 단계에서 걸면
    /// winit가 `ShowWindow(SW_MAXIMIZE)`로 아직 그리지 않은 창을 드러내 흰 사각형이 번쩍인다
    /// (`ui::window_start` 참조). 최대화가 실제로 걸릴 때까지는 관측한 사각형을 저장하지
    /// 않는다 — 그러지 않으면 되돌릴 "일반 크기"가 작업 영역 크기로 덮인다.
    /// 세어 두는 이유는 명령이 먹지 않는 환경에서도 **언젠가는 평소 추적으로 돌아가기** 위함이다
    restoring_maximized: u8,
    /// 삭제 확인을 기다리는 워크스페이스 (FR-18).
    /// 인덱스가 아니라 id로 잡는다 — 확인 대화는 프레임을 넘겨 살아 있는데,
    /// 그 사이 순서가 바뀌면 인덱스는 다른 워크스페이스를 가리킨다 (D12 ①과 같은 이유)
    pending_remove: Option<WorkspaceId>,
    /// 사이드바에서 지우기를 기다리는 사이트 (FR-29) — 진행·대기 중인 전송이 있을 때만
    /// 확인을 받는다. 인덱스가 아니라 식별자로 잡는 이유는 위 `pending_remove`와 같다
    pending_site_remove: Option<SiteId>,
    /// 방금 걷어낸 사이트들 (FR-29) — **늦게 도착하는 워커 결과를 버리는 표시**다.
    ///
    /// 폴더 펼치기(`expand_rx`)와 OS 드롭 스캔(`os_drop_rx`)은 다음 프레임에 큐로 들어가므로,
    /// 지운 뒤에 답이 오면 비운 큐가 되살아나 연결별 탭이 다시 선다. 그 사이트를 다시
    /// 연결하면(`connect_site`) 표시를 지운다 — 그때부터는 종전대로 큐에 들어가야 한다
    detached_sites: HashSet<SiteId>,
    /// 앱 전역 설정 (FR-47) — 설정 대화가 바꾸고 각 기능이 읽는다
    settings: AppSettings,
    /// 앱 설정 대화 (FR-47) — 타이틀바 설정 메뉴의 `설정`이 연다
    settings_dialog: SettingsDialog,
    /// 오픈소스 라이선스 대화 (FR-57) — 같은 메뉴의 `오픈소스 라이선스`가 연다
    license_dialog: LicenseDialog,
    /// 정보 대화 (FR-58) — 같은 메뉴의 `정보`가 연다
    about_dialog: AboutDialog,
    /// 고를 수 있는 글꼴 목록 (FR-48) — 워커가 만든다. 앱이 시작할 때 한 번만 읽는다
    font_scan: FontScan,
    /// 알림 영역 아이콘 (FR-50) — `종료` 토글이 켜져 있을 때만 있다. 없애면 아이콘이 사라진다
    tray: Option<Tray>,
    /// 트레이 조작이 오는 통로 — 창 프로시저가 보낸다
    tray_rx: std::sync::mpsc::Receiver<TrayEvent>,
    /// 창을 숨긴 상태인가 (FR-50) — 닫기를 눌렀지만 앱은 살아 있다.
    ///
    /// **`ctx.input(|i| i.viewport().visible())`로 파생시키지 않는다** — Windows에서 그 값은
    /// 늘 `None`이라(winit이 `occluded`를 채우지 않는다) 숨김과 표시를 구분하지 못한다
    hidden: bool,
    /// 트레이 메뉴 `종료`로 끝내는 중인가 — 그때는 닫기를 가로채지 않는다
    quitting: bool,
    /// 자동 실행으로 시작해 **창 없이 트레이로만** 올라와야 하는가 (FR-49).
    ///
    /// 곧바로 숨기지 않고 요청으로 들고 있는 이유: 세션이 최대화 상태였으면 그 복원이
    /// 여러 프레임에 걸쳐 재시도되는데(`restoring_maximized`), 그 전에 숨기면 프레임이
    /// 멈춰 복원이 영영 끝나지 않는다. 복원이 끝난 뒤에 숨긴다
    hide_on_start: bool,
    /// 열린 원격 연결 전부 — 워크스페이스가 아니라 앱이 쥔다.
    /// 연결은 탭보다 오래 살고 워크스페이스를 넘나들 수 있다 (FR-45·NFR-11)
    manager: ConnectionManager,
    /// 등록된 사이트 (FR-27). 탭·사이드바가 이름·프로토콜을 여기서 읽는다
    sites: SiteStore,
    /// 폴더 트리 즐겨찾기 (FR-56) — 앱에 하나뿐이라 모든 패널·탭이 같은 목록을 본다
    favorites: FavoriteStore,
    /// 트리의 드라이브 줄과 그 연결 상태 (T4) — 즐겨찾기와 같은 이유로 앱에 하나뿐이다.
    /// 값은 시작할 때 띄운 워커가 채우고, 사용자가 드라이브를 열어 볼 때 갱신된다
    drives: DriveList,
    /// 그 워커의 결과를 받는 통로 — 목록과 접근 판정이 따로 도착한다
    drive_scan: Option<std::sync::mpsc::Receiver<crate::fs::drives::DriveScan>>,
    /// 드라이브가 꽂히거나 빠진 것을 알려 오는 통로 (2026-09-09 사용자 요청).
    ///
    /// **`Option`이 아니다** — 위 `drive_scan`은 한 벌의 확인이 끝나면 통로를 놓지만,
    /// 이쪽 워커는 앱이 사는 동안 계속 지켜보므로 놓을 시점이 없다.
    /// 오는 것은 목록뿐이라 접근 판정은 종전대로 주기 확인(FR-67)이 채운다
    drive_watch: std::sync::mpsc::Receiver<Vec<crate::fs::drives::DriveRow>>,
    /// 패널마다 마지막으로 **자동** 재조회한 시각 (FR-67) — 손으로 누른 새로 고침은 세지 않는다.
    /// 세션에 담지 않는다: 앱을 다시 켜면 첫 주기부터 새로 세는 것이 자연스럽다
    auto_refresh_at: HashMap<PanelId, f64>,
    /// 마지막으로 **로컬 재확인**(끊긴 드라이브·즐겨찾기 실재)을 띄운 시각 (FR-67).
    ///
    /// 원격 자동 갱신 설정과 무관하게 기본 틱 위에서 돈다 — 서버 부담이 없는 로컬 일이다
    last_rescan_at: f64,
    /// SFTP 지문 확인 대화와 연결 워커를 잇는 통로 (D15)
    hostkey: HostKeyGate,
    /// 사이트 관리자 대화 (FR-27) — 연결 메뉴의 `사이트 관리자`와 실패 화면의 `설정 열기`가 연다
    site_manager: SiteManager,
    /// 짧게 떴다 사라지는 알림 (FR-43) — 창 전역이라 워크스페이스를 넘나든다
    toast: Toast,
    /// 전송 큐 (FR-36) — 연결과 같은 이유로 앱이 쥔다(워크스페이스를 넘나든다)
    queue: TransferQueue,
    /// 큐를 실제 전송으로 옮기는 실행기 (FR-37)
    runner: TransferRunner,
    /// 하단 도크의 화면 상태 (FR-36·FR-40)
    dock: DockState,
    /// 클립보드로 내보낼 것 — 그리기 도중에는 `ctx`를 빌릴 수 없어 프레임 끝에 보낸다
    pending_clipboard: Option<String>,
    /// 서버 로그의 우클릭 메뉴 상태 (FR-40) — 메뉴가 떠 있는 동안 고른 글자를 지키고,
    /// `복사`를 누르면 다음 프레임에 egui가 그것을 클립보드로 옮긴다
    log_menu: log_panel::MenuState,
    /// 훑어 달라고 청한 원격 폴더들 — 답이 오면 그 아래 파일을 큐에 넣는다 (FR-38)
    pending_trees: HashMap<u64, (SiteId, RemotePath, PathBuf)>,
    /// 훑기 요청 번호 — 목록 조회의 세대 번호와 섞이지 않게 따로 센다
    next_tree: u64,
    /// 원격 파일 작업 대화의 상태 (FR-39)
    remote_ops: RemoteOps,
    /// 전송이 끝나 다시 읽어야 할 원격 폴더들 (FR-37) — `pump_relist`가 프레임마다 거둔다
    relist: RelistPending,
    /// 상태 줄에 잠깐 띄울 사유와 그 만료 시각 — 원격 파일 작업의 실패(FR-39)와
    /// 로컬 복사의 실패·취소(FR-60)가 같은 자리를 쓴다
    notice: Option<(String, f64)>,
    /// 지금 펼치고 있는 로컬 폴더 수 (T22 Edge Case) — 상태 줄이 이것을 알린다.
    /// 펼치기는 별도 스레드에서 돌아 화면이 멈추지는 않지만, 아무 표시가 없으면
    /// 사용자는 아무 일도 안 일어난 줄 안다 (F-7 리뷰 M1)
    expanding: usize,
    /// 원격 트리가 읽어 둔 하위 폴더들 (T24)
    tree_cache: TreeCache,
    /// 트리가 청한 조회의 답을 기다리는 자리 — 세대 → (연결, 경로, 캐시 세대)
    pending_tree_lists: HashMap<u64, (ConnectionId, RemotePath, u64)>,
    /// 트리 조회의 세대 번호
    next_tree_list: u64,
    /// 로컬 폴더를 펼친 결과가 오는 통로 (FR-38) — 펼치기는 워커 스레드가 한다
    expand_tx: std::sync::mpsc::Sender<ExpandResult>,
    expand_rx: std::sync::mpsc::Receiver<ExpandResult>,
    /// 같은 이름 확인을 기다리는 전송들 (FR-55) — 답이 온 순서대로 처리한다
    pending_conflicts: Vec<ConflictCheck>,
    /// 확인 번호 발급기
    next_conflict: u64,
    /// 받는 곳의 존재 확인 결과가 오는 통로 — `(확인 번호, 겹친 이름들)`
    conflict_tx: std::sync::mpsc::Sender<(u64, Vec<String>)>,
    conflict_rx: std::sync::mpsc::Receiver<(u64, Vec<String>)>,
    /// 지금 열려 있는 Win11 모양 컨텍스트 메뉴 (FR-8) — 없으면 메뉴가 떠 있지 않다
    shell_menu: Option<shell_menu::OpenShellMenu>,
    /// 곧 띄울 **종전 표준 메뉴** (`기본 메뉴`가 여는 것) — 그것은 `TrackPopupMenuEx`가
    /// 자체 메시지 루프를 돌려 그리기 도중에 부를 수 없고, **우리 메뉴가 없는 화면이 실제로
    /// 표시된 뒤**에 띄워야 두 메뉴가 겹치지 않는다(`shell_menu::SHOW_MORE_SKIP_FRAMES`)
    pending_show_more: Option<shell_menu::PendingShowMore>,
    /// 이름 바꾸기·삭제의 결과를 받는 자리 (FR-64) — 복사와 같은 방식이다
    file_op_tx: std::sync::mpsc::Sender<crate::fs::file_op::FileOpOutcome>,
    file_op_rx: std::sync::mpsc::Receiver<crate::fs::file_op::FileOpOutcome>,
    /// 셸 복사의 결과를 받는 자리 (FR-60) — `pump_local_copy`가 프레임마다 거둔다.
    /// 워커가 여럿 돌 수 있어 채널 하나를 나눠 쓴다
    copy_tx: std::sync::mpsc::Sender<crate::fs::file_op::CopyOutcome>,
    copy_rx: std::sync::mpsc::Receiver<crate::fs::file_op::CopyOutcome>,
    /// OS에서 끌어온 경로의 폴더 여부를 워커가 재 보낸 결과 (FR-61) —
    /// `(사이트, 놓인 원격 폴더, 항목들)`. 로컬 대상은 재 볼 것이 없어 이 통로를 쓰지 않는다
    os_drop_tx: std::sync::mpsc::Sender<(
        crate::remote::types::SiteId,
        crate::remote::types::RemotePath,
        Vec<list_common::DragItem>,
    )>,
    os_drop_rx: std::sync::mpsc::Receiver<(
        crate::remote::types::SiteId,
        crate::remote::types::RemotePath,
        Vec<list_common::DragItem>,
    )>,
    /// 올리기 확인이 서버에 물어 둔 것 — `조회 세대 → (물어본 연결, 올릴 최상위 이름들)`.
    ///
    /// 키가 확인 번호가 아니라 **보낸 조회 세대**다 — 답에는 세대만 실려 오므로 그것으로
    /// 찾을 수 있어야 한다(`pending_tree_lists`와 같은 규칙).
    ///
    /// 답이 오면 이 이름들을 원격 목록과 대조한다. **사이트가 아니라 연결을** 함께 드는 이유:
    /// 한 사이트는 연결을 여럿 쓰므로(FR-37 — 탐색 1 + 전송 2), 사이트로 거두면 끊긴 것과
    /// 무관한 연결에 물어 둔 확인까지 함께 포기돼 그 전송이 묻지 않고 나간다
    conflict_lists: HashMap<u64, (ConnectionId, Vec<String>)>,
    /// 확인이 끝나 **물어보기를 기다리는** 전송들 — 온 순서대로 하나씩 대화로 올린다
    conflict_queue: Vec<(ConflictCheck, Vec<String>)>,
    /// 지금 떠 있는 확인 대화가 다루는 전송 — 대화는 한 번에 하나만 뜬다
    conflict_dialog: Option<(ConflictCheck, Vec<String>)>,
    /// 워커가 일을 마쳤을 때 화면을 깨우는 통로 — 연결 관리자에 준 것과 같다
    repaint: Arc<dyn Fn() + Send + Sync>,
    /// 자동 업데이트 상태와 워커 (FR-62)
    update: crate::app::update::UpdateService,
    /// 이번 확인이 **손으로 누른 것**인가 — 그때만 결과를 알린다.
    /// 앱이 뜰 때 도는 확인까지 알리면 켤 때마다 알림이 뜬다
    update_asked_by_hand: bool,
    /// 전송이 도는 중에 업데이트를 누르면 뜨는 확인 대화 — 그 시점의 건수를 담는다.
    ///
    /// 건수를 **대화를 띄운 시점의 값으로 굳힌다** — 대화가 떠 있는 동안 전송이 끝나면
    /// 문장이 저 혼자 바뀌어 읽던 사람이 무엇에 동의하는지 흔들린다
    update_confirm: Option<usize>,
    /// 세션이 바뀌었는지 지켜보다 곧 적는 관측기 (`app::autosave`)
    autosave: autosave::AutoSave<Session>,
}

impl ExplorerApp {
    /// eframe 창 생성 직후 호출된다 — 폰트·팔레트·셸 호스트를 이 시점에 준비한다.
    /// `session`이 있으면 지난 실행의 워크스페이스·사이드바·창 상태를 되살린다 (FR-11·FR-20)
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        com: ComStatus,
        session: Option<Session>,
        // 자동 실행으로 시작했는가 (FR-49) — 트레이 설정이 켜져 있으면 창 없이 올라온다
        start_hidden: bool,
    ) -> ExplorerApp {
        // **저장된 글꼴을 첫 프레임부터 적용한다** (FR-48) — 여기서 기본값으로 등록해 두고
        // 세션을 읽은 뒤에 다시 등록하면, 시작할 때마다 맑은 고딕으로 한 번 그려졌다가
        // 고른 글꼴로 바뀌는 것이 눈에 보인다
        let korean_font = install_fonts(
            &cc.egui_ctx,
            session
                .as_ref()
                .and_then(|session| session.settings.selected_font()),
        );
        theme::apply_dark(&cc.egui_ctx);
        // HWND 획득·서브클래스 설치는 창이 만들어진 이 시점에만 가능하다
        let shell = ShellHost::new(cc);
        // 최대화·복원 때 OS가 옛 화면과 새 화면을 겹쳐 페이드하면 글자가 이중으로 보인다 (FR-22)
        if let Some(shell) = &shell {
            crate::app::theme::disable_window_transitions(shell.hwnd());
            // 아직 그리지 않은 자리가 흰색으로 번쩍이지 않게 한다 (창 표시·최대화 순간)
            crate::app::theme::paint_unpainted_as_window_bg(shell.hwnd());
            // **창이 보이기 전에** 아이콘을 붙인다 — 작업 표시줄은 버튼을 만드는 순간
            // 아이콘을 읽어 가므로 그보다 앞서야 한다. winit이 창 클래스를 비워 두고
            // eframe은 창이 활성이 된 뒤에야 붙여서, 둘 다 이 시점을 놓친다
            // (사유·실측은 `app_icon::apply_to_window`)
            app_icon::apply_to_window(shell.hwnd());
        }
        let (expand_tx, expand_rx) = std::sync::mpsc::channel();
        let (conflict_tx, conflict_rx) = std::sync::mpsc::channel();
        let (copy_tx, copy_rx) = std::sync::mpsc::channel();
        let (file_op_tx, file_op_rx) = std::sync::mpsc::channel();
        let (os_drop_tx, os_drop_rx) = std::sync::mpsc::channel();
        let repaint: Arc<dyn Fn() + Send + Sync> = {
            let ctx = cc.egui_ctx.clone();
            Arc::new(move || ctx.request_repaint())
        };
        // 트레이 조작은 창 프로시저가 보낸다 — 창이 숨은 동안에도 오는 유일한 경로다 (FR-50)
        let (tray_tx, tray_rx) = std::sync::mpsc::channel();
        crate::ui::tray::install_channel(tray_tx, cc.egui_ctx.clone());

        // 셸에서 아이콘·이름을 얻는 자리라 즐겨찾기 기본 항목보다 먼저 만든다
        let mut icons = IconCache::new();
        let default_favorites = crate::fs::known_folders::default_favorites(&mut icons);
        // 첫 확인에는 **기본 두 줄만** 넘긴다 — 사용자 항목은 세션을 읽은 뒤에야 정해지고,
        // 그것은 곧 이어지는 주기 확인이 함께 본다 (FR-67)
        let first_watched: Vec<PathBuf> = default_favorites
            .iter()
            .map(|(path, _)| path.clone())
            .collect();

        let mut app = ExplorerApp {
            auto_refresh_at: HashMap::new(),
            com,
            shell,
            korean_font,
            icons,
            dir_cache: DirCache::new(),
            textures: IconTextures::new(),
            app_icon: app_icon::load_texture(&cc.egui_ctx, APP_ICON_TEXTURE_PX),
            workspaces: WorkspaceList::new(),
            views: HashMap::new(),
            restored: HashMap::new(),
            sidebar: WorkspaceSidebar::new(),
            sidebar_width: SIDEBAR_DEFAULT_WIDTH as f32,
            sidebar_collapsed: false,
            key_owner: menu::KeyOwner::default(),
            window: DEFAULT_WINDOW,
            restore_window: None,
            restoring_maximized: 0,
            pending_remove: None,
            pending_site_remove: None,
            detached_sites: HashSet::new(),
            // 워커가 소식을 올리면 창을 다시 그리게 한다 — 입력이 없으면 egui는 프레임을
            // 돌리지 않아, 이 신호가 없으면 목록이 사용자가 마우스를 움직일 때까지 안 나타난다
            manager: ConnectionManager::new({
                let ctx = cc.egui_ctx.clone();
                Arc::new(move || ctx.request_repaint())
            }),
            sites: SiteStore::new(),
            // 즐겨찾기 맨 위에 바탕 화면·다운로드가 선다 (FR-56). 셸에 물어 얻으므로
            // 그 폴더를 다른 드라이브로 옮겼어도 옮긴 자리를 가리킨다
            favorites: FavoriteStore::with_defaults(default_favorites, []),
            // 드라이브 줄은 **워커가 만든다** — 끊긴 네트워크 드라이브의 접근 판정이
            // 첫 시도에 2.8초까지 걸려(실측) UI 스레드에서는 할 수 없다 (T4)
            drives: DriveList::default(),
            drive_scan: Some(crate::fs::drives::spawn_scan(&cc.egui_ctx, first_watched)),
            // 꽂히고 빠지는 것은 따로 지켜본다 — 30초 주기 확인만으로는 USB를 꽂고
            // 그만큼 기다려야 트리에 선다(그 확인이 도는 중이면 더 밀린다)
            drive_watch: crate::fs::drives::spawn_watch(&cc.egui_ctx),
            // 시작 확인이 곧 첫 확인이다 — 여기서 0으로 두면 그 확인이 끝나자마자
            // 주기가 지난 것으로 보여 한 번을 헛되이 더 띄운다
            last_rescan_at: 0.0,
            hostkey: HostKeyGate::new(),
            site_manager: SiteManager::new(),
            toast: Toast::new(),
            queue: TransferQueue::new(),
            runner: TransferRunner::new(),
            dock: DockState::default(),
            settings: AppSettings::default(),
            settings_dialog: SettingsDialog::new(),
            license_dialog: LicenseDialog::new(),
            about_dialog: AboutDialog::new(),
            font_scan: FontScan::new(),
            tray: None,
            tray_rx,
            hidden: false,
            quitting: false,
            hide_on_start: start_hidden,
            pending_clipboard: None,
            log_menu: log_panel::MenuState::default(),
            pending_trees: HashMap::new(),
            next_tree: 0,
            remote_ops: RemoteOps::default(),
            relist: RelistPending::default(),
            notice: None,
            expanding: 0,
            tree_cache: TreeCache::new(),
            pending_tree_lists: HashMap::new(),
            next_tree_list: 0,
            expand_tx,
            expand_rx,
            pending_conflicts: Vec::new(),
            next_conflict: 0,
            shell_menu: None,
            file_op_tx,
            file_op_rx,
            pending_show_more: None,
            copy_tx,
            copy_rx,
            os_drop_tx,
            os_drop_rx,
            conflict_tx,
            conflict_rx,
            conflict_lists: HashMap::new(),
            conflict_queue: Vec::new(),
            conflict_dialog: None,
            repaint,
            // 설치본 판정은 이 자리에서 한 번만 한다 — 서비스는 그 결과를 받아 쥔다
            update: crate::app::update::UpdateService::new(
                crate::app::update::install::is_installed_build(),
            ),
            update_asked_by_hand: false,
            update_confirm: None,
            autosave: autosave::AutoSave::default(),
        };
        if let Some(session) = session {
            app.apply_session(session);
        }
        // 지난 회차에 받아 둔 설치 파일을 치운다 (FR-62) — 설치가 끝나는 시점에는 앱이
        // 이미 죽어 있어 스스로 지울 수 없으므로, 새 판으로 다시 뜬 지금이 그 자리다.
        // 설치가 중간에 취소돼 남은 것도 여기서 함께 사라진다 (D8)
        crate::app::update::install::clear_update_dir();
        // 새 판이 있는지 워커에서 한 번 묻는다 — 앱이 뜨는 길은 이 결과를 기다리지 않는다.
        // 설치본이 아니면 서비스가 스스로 아무 일도 하지 않는다 (D4·D7)
        app.update.start_check(app.repaint.clone());

        // 글꼴 목록은 만드는 데 1.5초쯤 걸린다 — 설정 대화를 열고 나서 시작하면 그 시간이
        // 그대로 「글꼴 목록을 읽는 중…」으로 보인다. 시작할 때 미리 띄워 두면 대화를
        // 열 때는 대개 이미 준비돼 있다. 워커 스레드가 하므로 시작 화면은 멈추지 않는다
        app.font_scan.ensure_started(&cc.egui_ctx);
        app
    }

    /// 불러온 세션을 상태에 반영한다. 워크스페이스 **뷰는 만들지 않는다** —
    /// 활성 워크스페이스도 첫 프레임의 `ensure_active_view`에서 비로소 만들어진다(D1).
    /// 그 확보는 프레임의 **`logic`에서** 한다 — 그리기 때 만들면 폴더 열거를 거는
    /// `panel.poll`이 이미 지나간 뒤라 목록이 한 프레임 늦게 시작된다
    fn apply_session(&mut self, session: Session) {
        let states = session::restore(&session);
        let names: Vec<String> = states.iter().map(|state| state.name.clone()).collect();
        let Some(workspaces) = WorkspaceList::from_names(names, session.active_workspace) else {
            return; // 빈 목록 — 기본 워크스페이스 1개로 시작한다
        };
        self.workspaces = workspaces;
        // `from_names`는 id를 0부터 순서대로 준다 — 그 순서가 곧 저장된 순서다
        self.restored = self
            .workspaces
            .items()
            .iter()
            .map(|item| item.id)
            .zip(states)
            .collect();
        for index in 0..self.workspaces.len() {
            if let Some(dir) = self.restored_active_dir(index) {
                self.workspaces.set_subtitle(index, &dir);
            }
        }
        // 사이트·큐·도크를 되살린다 (FR-44) — **연결은 열지 않고 전송도 시작하지 않는다**.
        // 원격 탭은 `연결 없음`으로 서 있고, 큐는 대기·실패인 채로 기다린다
        self.sites = session.sites.clone();
        // 즐겨찾기는 문자열로 담겨 있다(D9) — 여기서 경로로 되돌린다.
        // **기본 항목을 다시 싣는다** — `from_paths`로 통째로 갈아치우면 설정 파일이 있는
        // 모든 사용자(=정상 경로)에게서 바탕 화면·다운로드가 사라진다
        self.favorites = FavoriteStore::with_defaults(
            crate::fs::known_folders::default_favorites(&mut self.icons),
            session.favorites.iter().map(PathBuf::from),
        );
        self.queue = session::restore_queue(&session);
        self.dock = DockState::from_session(&session.dock);
        self.settings = session.settings.clone();
        self.sidebar_width = session.sidebar.width as f32;
        self.sidebar_collapsed = session.sidebar.collapsed;
        self.restoring_maximized = if session.window.maximized {
            MAXIMIZE_RETRY_FRAMES
        } else {
            0
        };
        self.window = session.window.clone();
        self.restore_window = Some(session.window);
    }

    /// 아직 열지 않은 워크스페이스의 활성 폴더 — 사이드바 부제를 복원 직후에도 채우기 위함
    fn restored_active_dir(&self, index: usize) -> Option<PathBuf> {
        let id = self.workspaces.items().get(index)?.id;
        let state = self.restored.get(&id)?;
        let panel = state.panels.get(state.active_panel)?;
        // 원격 탭이 활성이면 부제로 쓸 로컬 경로가 없다 — 그때는 부제를 비운다
        panel.tabs.get(panel.active_tab)?.local_path().cloned()
    }

    /// 지금 상태를 세션으로 모은다 (종료 시 저장)
    fn collect_session(&self) -> Session {
        let workspaces: Vec<WorkspaceState> = self
            .workspaces
            .items()
            .iter()
            .map(|item| match self.views.get(&item.id) {
                Some(view) => view.to_state(item.name.clone()),
                // 한 번도 열지 않은 워크스페이스는 불러온 상태를 그대로 다시 내보낸다.
                // 이름만 최신값으로 — 열지 않고도 이름은 바꿀 수 있다
                None => match self.restored.get(&item.id) {
                    Some(state) => WorkspaceState {
                        name: item.name.clone(),
                        ..state.clone()
                    },
                    None => WorkspaceState {
                        name: item.name.clone(),
                        shape: TreeShape::Leaf,
                        panels: vec![PanelTabs {
                            tabs: vec![TabSpec::Local(start_dir())],
                            active_tab: 0,
                            ..Default::default()
                        }],
                        active_panel: 0,
                    },
                },
            })
            .collect();
        // 앱 설정과 즐겨찾기는 `to_session`이 아니라 여기서 싣는다 — `to_session`은 창·
        // 워크스페이스를 옮기는 자리라 인자를 더 받으면 그 책임이 흐려지고, 호출부(이 파일과
        // `ui::session`의 시험들)도 함께 는다.
        // 즐겨찾기만 `with_favorites`를 거치는 이유는 **이 자리가 스프레드**라는 데 있다 —
        // 필드를 빠뜨려도 컴파일이 통과하므로, 덮는 규칙을 순수 함수로 떼어 시험이 그것을
        // 직접 부르게 했다 (plan D7)
        session::with_favorites(
            Session {
                settings: self.settings.clone(),
                ..session::to_session(
                    self.window.clone(),
                    SidebarSession {
                        width: self.sidebar_width as i32,
                        collapsed: self.sidebar_collapsed,
                    },
                    self.workspaces.active_index(),
                    &workspaces,
                    RemoteSnapshot {
                        sites: &self.sites,
                        queue: &self.queue,
                        dock: self.dock.to_session(),
                    },
                )
            },
            self.favorites.paths(),
        )
    }

    /// 셸 메뉴를 쓸 수 있는가 — COM STA와 창 핸들이 모두 있어야 한다
    fn shell_available(&self) -> bool {
        self.com.is_available() && self.shell.is_some()
    }

    /// 활성 워크스페이스의 탐색 상태를 확보한다 — 처음 열리는 워크스페이스면 여기서 만들어진다(D1).
    /// 세션에서 불러온 것이면 저장된 분할·탭으로 되살린다
    fn ensure_active_view(&mut self) -> &mut WorkspaceView {
        let id = self.workspaces.active().id;
        // 뷰가 이미 있으면 저장 상태를 꺼내지 않는다 — 꺼내 놓고 쓰지 않으면 그대로 버려진다
        let restored = if self.views.contains_key(&id) {
            None
        } else {
            self.restored.remove(&id)
        };
        self.views.entry(id).or_insert_with(|| match restored {
            Some(state) => WorkspaceView::from_state(&state),
            None => WorkspaceView::new(start_dir()),
        })
    }

    /// 사이드바 조작 반영 — 목록 변경은 전부 여기서만 일어난다
    fn handle_sidebar(
        &mut self,
        action: SidebarAction,
        area: LayoutRect,
        now: f64,
        ctx: &egui::Context,
    ) {
        match action {
            SidebarAction::Select(index) => {
                self.workspaces.set_active(index);
            }
            SidebarAction::Add => {
                self.workspaces.add();
            }
            SidebarAction::Rename(index, name) => {
                self.workspaces.rename(index, &name);
            }
            SidebarAction::Remove(index) => {
                // 바로 지우지 않고 한 번 묻는다 — 되돌릴 수 없다(현행 판과 같은 규칙)
                self.pending_remove = self.workspaces.items().get(index).map(|item| item.id);
            }
            SidebarAction::Reorder(from, to) => {
                self.workspaces.reorder(from, to);
            }
            // 어느 사이트를 골랐는지는 사이드바가 스스로 들고 그린다 — 앱이 따로 둘 상태가 없다
            SidebarAction::SelectSite(_) => {}
            SidebarAction::ConnectSite(site) => {
                // 진입점 셋과 **같은 경로**로 보낸다 — 사이드바만 분할 없이 연결하면
                // 여는 방법에 따라 배치가 달라진다
                self.open_site_tab(site, None, area);
            }
            // 목록에서 지우면 **그 사이트의 실행 중 상태를 함께 걷어낸다** (FR-29) —
            // 연결·원격 탭·전송 큐 항목이 그대로 남으면 지웠는데도 전송 큐 화면의
            // 연결별 탭에 그 서버가 계속 선다 (2026-08-21 사용자 요청).
            // 사이트 기록 자체는 사이트 관리자에 남아 되돌릴 수 있다
            SidebarAction::RemoveSite(site) => {
                // 도는 전송을 끊는 것은 되돌릴 수 없다 — 그때만 한 번 묻는다 (D4)
                if self
                    .queue
                    .site_items(site)
                    .iter()
                    .any(|item| item.state.is_pending())
                {
                    self.pending_site_remove = Some(site);
                } else {
                    self.detach_site(site, area, ctx, now);
                }
            }
            // 사이트 목록은 메모리에 있어 지금은 다시 읽을 것이 없다.
            // 파일로 오가는 저장이 붙는 T25(세션 v3)에서 이 자리가 다시 읽기가 된다
            SidebarAction::RefreshSites => {}
            // 연결 메뉴는 사이드바가 직접 띄운다 — 이 조작은 알림일 뿐이다
            SidebarAction::OpenConnectMenu => {}
            // 연결 메뉴에서 온 길이라 **빈 초안**으로 연다 — 기존 사이트를 골라 두면
            // `확인(O)`이 그것을 덮어써 새로 더할 길이 사라진다 (인벤토리 #8)
            SidebarAction::OpenSiteManager => self.site_manager.open_new(),
        }
    }

    /// 창 위치·크기를 따라간다. 최대화 중에는 갱신하지 않는다 —
    /// 그때의 사각형은 화면 전체라, 저장해 버리면 다음 실행에서 되돌릴 일반 크기가 사라진다.
    /// 복원 위치가 화면 밖이면(모니터 구성 변경) 첫 프레임에 화면 안으로 옮긴다.
    ///
    /// 세션이 최대화였다면 **여기서 비로소 최대화를 건다** — 창을 만들 때 걸면 아직 그리지
    /// 않은 창이 드러나 흰 사각형이 번쩍인다(`ui::window_start` 참조)
    fn track_window(&mut self, ctx: &egui::Context) {
        // **숨긴 동안에는 관측하지 않는다** — 숨은 창의 viewport 정보로 위치·크기를 덮으면
        // 숨기기 직전에 저장해 둔 좋은 값이 그것으로 밀리고, 종료 시 그 밀린 값이 저장된다
        if self.hidden {
            return;
        }
        // 화면 크기(`viewport.monitor_size`)는 보지 않는다 — 그것은 **창이 있는 화면 하나**의
        // 크기라, 화면이 여럿이면 옆 화면에 둔 창이 늘 화면 밖으로 읽힌다.
        // 화면 밖 판정은 아래에서 붙어 있는 화면 전부를 보고 한다(`ui::window_start`)
        let (rect, maximized) = ctx.input(|input| {
            let viewport = input.viewport();
            (viewport.outer_rect, viewport.maximized.unwrap_or(false))
        });
        // 최대화가 실제로 걸릴 때까지는 관측한 사각형도, 최대화 여부도 반영하지 않는다 —
        // 그 사이의 창은 "작업 영역만 한 일반 창"이라, 그대로 받으면 되돌릴 일반 크기가 덮인다
        if self.restoring_maximized > 0 {
            if maximized {
                self.restoring_maximized = 0;
                self.window.maximized = true;
            } else if ctx.cumulative_pass_nr() > 0 {
                // **첫 프레임에는 걸지 않는다** — eframe은 첫 프레임을 그린 뒤에야 창을 보이게
                // 하는데(glow_integration `post_rendering`), 그 전에 최대화를 걸면
                // `ShowWindow(SW_MAXIMIZE)`가 아직 아무것도 그려지지 않은 창을 먼저 드러내
                // 흰 사각형이 번쩍인다(2026-08-13 실측 — 이것이 이 결함의 두 번째 경로다)
                self.restoring_maximized -= 1;
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
            }
            return;
        }
        self.window.maximized = maximized;
        if let Some(rect) = rect
            && !maximized
        {
            self.window.x = rect.left() as i32;
            self.window.y = rect.top() as i32;
            self.window.w = rect.width() as i32;
            self.window.h = rect.height() as i32;
        }
        // 저장된 자리가 지금 붙어 있는 화면 어디에도 없으면 안으로 끌어온다 (화면을 떼거나
        // 배치를 바꾼 경우). 화면 목록은 Win32에서 바로 읽으므로 첫 프레임부터 판정할 수 있다.
        //
        // **최대화 상태에서는 건드리지 않는다** — 저장된 사각형은 최대화를 풀었을 때 돌아갈
        // 자리라, 그것으로 위치·크기 명령을 보내면 방금 건 최대화가 그 자리에서 풀린다
        if let Some(saved) = self.restore_window.take()
            && !self.window.maximized
            && let Some(fixed) = crate::ui::window_start::rescue_offscreen(&saved)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                fixed.x as f32,
                fixed.y as f32,
            )));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                fixed.w as f32,
                fixed.h as f32,
            )));
            self.window = WindowState {
                x: fixed.x,
                y: fixed.y,
                w: fixed.w,
                h: fixed.h,
                maximized: saved.maximized,
            };
        }
    }

    /// 타이틀바를 그리고 앱 명령을 돌려준다 (FR-22).
    /// 창 조작(최소화·최대화·닫기·끌기·가장자리 크기 조절)은 여기서 바로 창에 전달하고,
    /// 앱 명령(사이드바 토글)만 호출부로 넘긴다 — 분할 영역이 확정된 뒤에 실행해야 하기 때문이다
    fn show_titlebar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) -> Option<Command> {
        let state = titlebar::TitlebarState {
            maximized: self.window.maximized,
            sidebar_collapsed: self.sidebar_collapsed,
        };
        let title = self.workspaces.active().name.clone();
        let badge = self.update_badge();
        let outcome = egui::Panel::top(egui::Id::new("titlebar"))
            .resizable(false)
            .exact_size(titlebar::TITLEBAR_HEIGHT)
            // 구분선은 `titlebar`가 앱 팔레트 색으로 직접 그린다 — egui 기본 구분선은 끈다
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(theme::WINDOW_BG))
            .show(ui, |ui| {
                titlebar::show_titlebar(
                    ui,
                    &title,
                    state,
                    self.app_icon.as_ref().map(|t| t.id()),
                    badge,
                )
            })
            .inner;
        // 창 가장자리 크기 조절 — 모서리는 크기 조절이 우선이고(버튼이 가져가면 대각선으로 창을
        // 잡을 자리가 사라진다), 버튼 위쪽 변은 버튼이 우선한다(`show_resize_handles`가 가른다)
        // 같은 프레임에 잰 우측 폭을 그대로 넘긴다 — 배지가 선 만큼 위쪽 변 크기 조절이
        // 비켜야 한다(D13). 직전 프레임 값을 기억해 넘기면 한 프레임 어긋난다
        let resize =
            titlebar::show_resize_handles(ctx, self.window.maximized, outcome.right_group_width);
        if let Some(request) = resize.or(outcome.window) {
            let command = match request {
                WindowRequest::Minimize => egui::ViewportCommand::Minimized(true),
                WindowRequest::ToggleMaximize => {
                    egui::ViewportCommand::Maximized(!self.window.maximized)
                }
                // 닫기는 창 닫기 요청으로 보낸다 — eframe이 종료 절차를 거치며 `on_exit`를
                // 부르므로 세션이 저장된다(프로세스를 직접 끝내면 그 경로가 사라진다)
                WindowRequest::Close => egui::ViewportCommand::Close,
                WindowRequest::Drag => egui::ViewportCommand::StartDrag,
                WindowRequest::Resize(direction) => egui::ViewportCommand::BeginResize(direction),
            };
            ctx.send_viewport_cmd(command);
        }
        outcome.command
    }

    /// 삭제 확인 대화 (FR-18) — 워크스페이스 하나를 지우면 그 화면 구성과 탭이 함께 사라지고
    /// 되돌릴 수 없어 한 번 묻는다
    fn show_remove_confirm(&mut self, ctx: &egui::Context) {
        let Some(id) = self.pending_remove else {
            return;
        };
        // 대화가 떠 있는 동안 순서가 바뀔 수 있으므로 자리를 매번 다시 찾는다
        let found = self
            .workspaces
            .items()
            .iter()
            .position(|item| item.id == id)
            .map(|index| (index, self.workspaces.items()[index].name.clone()));
        let Some((index, name)) = found else {
            // 목록이 그 사이 바뀌어 대상이 사라졌다 — 물을 것이 없다
            self.pending_remove = None;
            return;
        };
        let mut confirmed = None;
        let buttons = [
            dialog::ButtonSpec::strong(crate::i18n::delete()),
            dialog::ButtonSpec::plain(crate::i18n::cancel()),
        ];
        let shell = dialog::show(
            ctx,
            egui::Id::new("workspace_remove_confirm"),
            REMOVE_DIALOG_WIDTH,
            &buttons,
            |ui| {
                ui.heading(crate::i18n::app_workspace_delete_title());
                ui.add_space(8.0);
                ui.label(crate::i18n::dynamic::workspace_delete_confirm(&name));
                ui.label(crate::i18n::app_workspace_delete_detail());
            },
        );
        match shell.clicked {
            Some(0) => confirmed = Some(true),
            Some(_) => confirmed = Some(false),
            None => {}
        }
        // 종전에는 이 대화가 `Esc`를 직접 잡았다 — 이제 셸이 배경 클릭까지 함께 판정한다
        if shell.should_close {
            confirmed = Some(false);
        }
        match confirmed {
            Some(true) => {
                self.pending_remove = None;
                self.remove_workspace(index);
            }
            Some(false) => self.pending_remove = None,
            None => {}
        }
    }

    /// 상태 표시줄 — **언제나 보인다** (FR-41·D18). 도크보다 먼저 붙어 창 맨 아래에 남는다.
    ///
    /// 여기 캐럿이 도크를 여는 문이다(README §8) — 열고 나면 그 안의 탭에서 큐·로그를 고른다
    fn show_status_bar(&mut self, ui: &mut egui::Ui) {
        // 지난 사유는 지운다 — 남겨 두면 이미 끝난 실패가 계속 떠 있다(로그에는 그대로 남는다)
        let now = ui.input(|input| input.time);
        if self.notice.as_ref().is_some_and(|(_, until)| now >= *until) {
            self.notice = None;
        }
        let action = egui::Panel::bottom(egui::Id::new("status_bar"))
            .resizable(false)
            .default_size(status_bar::HEIGHT)
            .size_range(egui::Rangef::new(status_bar::HEIGHT, status_bar::HEIGHT))
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let rect = ui.max_rect();
                let view = StatusView {
                    queue: &self.queue,
                    expanding: self.expanding,
                    notice: self.notice.as_ref().map(|(text, _)| text.as_str()),
                };
                status_bar::show_status_bar(ui, rect, &self.dock, &view)
            })
            .inner;
        match action {
            Some(StatusAction::ToggleQueue) => self.dock.toggle(DockPanel::Queue),
            None => {}
        }
    }

    /// 하단 도크 — 전송 큐·서버 로그가 번갈아 쓰는 자리 (FR-36·FR-40·D19).
    ///
    /// **egui의 아래쪽 패널로 뗀다** — 사이드바(`Panel::left`)와 같은 방식이다. 직접 사각형을
    /// 잡아 `allocate_rect`로 떼면 위→아래 배치에서 커서가 **화면 바닥 너머**로 밀려,
    /// 뒤에 오는 패널 그리드가 높이 0이 된다(egui 0.35 `advance_after_rects` — T19 quality
    /// 리뷰 B1에서 실측). 창이 낮으면 도크가 줄어든다(`dock::dock_height`)
    fn show_dock(&mut self, ui: &mut egui::Ui) {
        if !self.dock.is_open() {
            return;
        }
        let height = dock::dock_height(ui.available_height());
        if height <= 0.0 {
            return;
        }
        let failed = self.failed_sites();
        let log_conn = self.log_connection();
        let panel = egui::Panel::bottom(egui::Id::new("transfer_dock"))
            .resizable(false)
            .default_size(height)
            .size_range(egui::Rangef::new(height, height))
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let rect = ui.max_rect();
                self.show_dock_body(ui, rect, &failed, log_conn)
            });
        let (dock_action, queue_action) = panel.inner;
        if let Some(action) = dock_action {
            self.apply_dock_action(action);
        }
        if let Some(action) = queue_action {
            self.apply_queue_action(action);
        }
    }

    /// 도크 안쪽 — 탭 스트립과 본문. 조작은 값으로 돌려주고 호출부가 실행한다
    fn show_dock_body(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        failed: &[SiteId],
        log_conn: Option<ConnectionId>,
    ) -> (Option<DockAction>, Option<QueueAction>) {
        let mut dock_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        dock_ui.set_clip_rect(rect);
        dock_ui.painter().rect_filled(rect, 0.0, theme::SURFACE_BG);
        dock_ui.painter().line_segment(
            [
                egui::pos2(rect.left(), rect.top() + 0.5),
                egui::pos2(rect.right(), rect.top() + 0.5),
            ],
            egui::Stroke::new(1.0, theme::PANE_BORDER),
        );

        let strip =
            egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), dock::STRIP_HEIGHT));
        let body = egui::Rect::from_min_max(egui::pos2(rect.left(), strip.bottom()), rect.max);
        {
            let connected = self.connected_conn_sites();
            let view = DockView {
                queue: &self.queue,
                failed,
                connected: &connected,
            };
            let dock_action = dock::show_strip(&mut dock_ui, strip, &mut self.dock, &view);
            let queue_action = match self.dock.panel {
                Some(DockPanel::Queue) => queue_panel::show_queue(
                    &mut dock_ui,
                    body,
                    &mut self.dock,
                    &view,
                    &self.sites,
                    crate::panel::file_list::local_time_parts(system_time_now()),
                    // **실행기에 미는 값과 같은 것을 넘긴다** — 죄지 않으면 손으로 고친
                    // 설정 파일이 99를 담았을 때 실행기는 10에서 멈추는데 화면만
                    // `재시도 N/99`로 보인다(동작과 표시가 어긋난다)
                    crate::app::settings::transfer_retry_count(self.settings.transfer_retry_count),
                ),
                Some(DockPanel::Log) => {
                    // 큐와 **같은 연결별 탭 줄**을 먼저 그린다 — 도크에 줄은 하나다(디자인 `:272`).
                    // 여기서 고른 사이트가 곧 "어느 서버의 로그를 볼지"가 된다
                    let site_row = egui::Rect::from_min_size(
                        body.min,
                        egui::vec2(body.width(), queue_panel::SITE_ROW_HEIGHT),
                    );
                    // 로그 화면에서는 건수를 적지 않는다 — 셀 대상이 없다 (2026-08-18)
                    queue_panel::show_site_tabs(
                        &mut dock_ui,
                        site_row,
                        &mut self.dock,
                        &view,
                        &self.sites,
                        false,
                    );
                    // 지금 보고 있는 연결의 로그를 그린다 — 연결이 없으면 빈 화면이다
                    let body = egui::Rect::from_min_max(
                        egui::pos2(body.left(), site_row.bottom() + log_panel::BODY_PAD_Y),
                        body.max,
                    );
                    match log_conn.and_then(|conn| self.manager.get(conn)) {
                        Some(connection) => log_panel::show_log(
                            &mut dock_ui,
                            body,
                            connection.log(),
                            &mut self.log_menu,
                        ),
                        None => log_panel::show_log(
                            &mut dock_ui,
                            body,
                            &LogBuffer::new(),
                            &mut self.log_menu,
                        ),
                    }
                    None
                }
                None => None,
            };
            (dock_action, queue_action)
        }
    }

    /// 지금 워크스페이스의 받기 목적지 (FR-54) — 판정은 `WorkspaceView`가 한다
    fn download_dir(&self) -> Option<PathBuf> {
        self.views.get(&self.workspaces.active().id)?.download_dir()
    }

    /// 지금 워크스페이스의 올리기 목적지 (FR-54)
    fn upload_dir(&self) -> Option<(SiteId, RemotePath)> {
        self.views.get(&self.workspaces.active().id)?.upload_dir()
    }

    /// 지금 워크스페이스의 업로드 하위 메뉴 대상들 (FR-8·FR-39).
    ///
    /// 사이트 이름이 `ExplorerApp`에 있어(`self.sites`) 여기서 뷰에 넘긴다
    pub(super) fn upload_menu_targets(&self) -> Vec<UploadTarget> {
        self.views
            .get(&self.workspaces.active().id)
            .map(|view| view.upload_menu_targets(&self.sites))
            .unwrap_or_default()
    }

    /// 지금 워크스페이스에서 올릴 것 (FR-54)
    fn upload_source(&self) -> Vec<(PathBuf, bool)> {
        self.views
            .get(&self.workspaces.active().id)
            .map(WorkspaceView::upload_source)
            .unwrap_or_default()
    }

    /// 도크 탭 스트립의 조작 (인벤토리 #33·#34)
    fn apply_dock_action(&mut self, action: DockAction) {
        match action {
            DockAction::TogglePause => {
                if self.queue.is_paused() {
                    self.runner.resume(&mut self.queue);
                } else {
                    self.runner.pause(&mut self.queue, &self.manager);
                }
            }
            DockAction::ClearDone => self.queue.clear_done(),
            DockAction::CopyLog => self.copy_log(),
        }
    }

    /// `⧉` — 지금 보고 있는 연결의 로그를 클립보드로 (Acceptance ③).
    ///
    /// **복사본에도 비밀번호는 없다** — 가리기는 로그가 버퍼에 들어가기 전에 이미 끝났다
    /// (D14·T5). 여기서 다시 가리지 않는 것은 두 벌이 되면 한쪽만 고쳐질 수 있어서다
    fn copy_log(&mut self) {
        let Some(text) = self
            .log_connection()
            .and_then(|conn| self.manager.get(conn))
            .map(|connection| connection.log().to_text())
        else {
            return;
        };
        if text.is_empty() {
            return;
        }
        self.pending_clipboard = Some(text);
    }

    /// 큐 행에서 고른 조작 (T19 우클릭 메뉴).
    ///
    /// **대상은 고른 것 전부다** — 화면이 `effective_selection`으로 골라 실어 보낸다
    /// (선택 밖의 행을 우클릭했으면 그 행 하나다).
    ///
    /// `…All`의 대상은 **지금 보고 있는 목록**이다 — 화면이 그린 범위가 아니라 거른 목록
    /// 전량이며, 그 계산을 여기서 하는 이유는 `ui::queue_panel`이 큐를 고치지 않기 때문이다
    fn apply_queue_action(&mut self, action: QueueAction) {
        match action {
            // 실패·취소를 대기로 되돌리면 다음 `start_ready`가 다시 건다.
            // **`전체 다시 시도`와 같은 길을 쓴다** — 종전에는 여기서 상태만 바꿔
            // 자동 재시도 횟수가 0으로 돌아가지 않았다(README가 돌아간다고 적은 것과 어긋났다)
            QueueAction::Retry(ids) => self.queue.retry(&ids),
            QueueAction::RetryAll => {
                let ids = self.visible_transfer_ids();
                self.queue.retry(&ids);
            }
            // **`전송 취소`와 `삭제`는 이제 하는 일이 다르다** — 취소는 `취소됨`으로 남기고
            // 삭제는 목록에서 지운다. 둘 다 워커를 먼저 멈춰야 `.part`가 정리된다.
            // **대상이 아닌 상태가 섞여 있어도 안전하다** — 큐가 그 번호를 그 자리에서 무시한다
            QueueAction::Cancel(ids) => {
                for id in &ids {
                    self.runner.cancel(&self.manager, *id);
                    self.queue.cancel(*id);
                }
            }
            QueueAction::Remove(ids) => {
                for id in &ids {
                    self.runner.cancel(&self.manager, *id);
                }
                self.queue.remove(&ids);
            }
            QueueAction::RemoveAll => {
                let ids = self.visible_transfer_ids();
                // 진행 중인 것은 워커를 먼저 멈춰야 `.part`가 정리된다.
                // 배정되지 않은 번호에 불러도 `cancel`이 그 자리에서 돌아온다
                for id in &ids {
                    self.runner.cancel(&self.manager, *id);
                }
                self.queue.remove(&ids);
            }
        }
    }

    /// 지금 도크에 보이는 목록의 전송 번호들 — `전체 …` 조작의 대상이다 (plan D4).
    ///
    /// 화면과 **같은 함수**로 거른다 — 다른 식으로 다시 세면 눈에 보이는 것과 지워지는 것이
    /// 어긋난다
    fn visible_transfer_ids(&self) -> Vec<crate::remote::connection::TransferId> {
        queue_panel::visible_items(&self.queue, self.dock.filter, self.dock.site)
            .into_iter()
            .map(|item| item.id)
            .collect()
    }

    /// 자동 실행으로 시작했으면 창을 숨긴다 — **최대화 복원이 끝난 뒤에** (FR-49).
    ///
    /// 트레이 아이콘이 올라간 것을 확인하고 숨긴다: 아이콘 없이 숨기면 창을 되부를
    /// 방법이 사라진다(`종료` 토글이 꺼져 있는 경우가 그렇다 — 그때는 그냥 창을 띄운다)
    fn hide_on_start(&mut self, ctx: &egui::Context) {
        if !self.hide_on_start {
            return;
        }
        // 최대화 복원이 도는 중이면 기다린다 — 숨기면 그 프레임이 멈춰 복원이 끝나지 않는다
        if self.restoring_maximized > 0 {
            return;
        }
        // 이 요청은 한 번만 쓴다
        self.hide_on_start = false;
        if !self.settings.tray_on_close || self.tray.is_none() {
            // 트레이로 갈 수 없으면 창을 띄운 채로 둔다 — 부를 방법이 없어지면 안 된다
            return;
        }
        self.persist_session();
        self.hide_window(ctx);
    }

    /// 닫기 요청을 가로채 창만 숨긴다 (FR-50).
    ///
    /// 타이틀바 `✕`뿐 아니라 `Alt+F4`·작업 표시줄 닫기·시스템 메뉴까지 **모든 종료 경로가
    /// 이 한 지점으로 모인다** — 버튼 핸들러에서 막으면 나머지 길로 들어온 종료를 놓친다 (D4)
    fn intercept_close(&mut self, ctx: &egui::Context) {
        if !ctx.input(|input| input.viewport().close_requested()) {
            return;
        }
        if !should_hide_on_close(
            self.settings.tray_on_close,
            self.tray.is_some(),
            self.quitting,
        ) {
            return;
        }
        // **숨기기 전에 저장한다** — 종료 때 도는 `on_exit`가 이번에는 오지 않는다
        self.persist_session();
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        self.hide_window(ctx);
    }

    /// 창을 트레이로 숨긴다 — **숨는 길이 둘이라 그 뒷정리를 한 곳에 모은다** (FR-50).
    ///
    /// 열려 있던 컨텍스트 메뉴를 함께 닫는 것이 그 정리다 — 창이 다시 보일 때 그 사이
    /// 사라졌을 수도 있는 대상을 쥔 메뉴가 남아 있으면 안 된다. 두 길에 각각 적으면
    /// 한쪽을 빠뜨려도 아무도 모른다(실제로 그렇게 빠뜨렸다)
    fn hide_window(&mut self, ctx: &egui::Context) {
        self.hidden = true;
        self.shell_menu = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
    }

    /// `종료` 토글에 맞춰 트레이 아이콘을 올리거나 내린다 (FR-50).
    ///
    /// 매 프레임 확인하는 이유: 토글이 바뀌는 자리가 설정 대화 한 곳이지만, 그 값과
    /// 실제 아이콘 유무가 어긋나면 화면에서 그것을 알 길이 없다
    fn sync_tray(&mut self, now: f64) {
        let want = self.settings.tray_on_close;
        if want == self.tray.is_some() {
            return;
        }
        if !want {
            // `Drop`이 아이콘을 거둔다
            self.tray = None;
            return;
        }
        // 창 핸들이 없으면(다른 백엔드·headless) 아이콘을 올릴 수 없다 —
        // 조용히 넘기면 토글은 켜져 있는데 아이콘이 영영 뜨지 않는다
        self.tray = self
            .shell
            .as_ref()
            .and_then(|shell| Tray::add(shell.hwnd()));
        if self.tray.is_none() {
            // 아이콘을 못 올렸으면 토글을 되돌린다 — 켜져 있다고 보이는데 아이콘이
            // 없으면 닫기를 눌렀을 때 앱을 되살릴 방법이 사라진다
            self.settings.tray_on_close = false;
            self.notice = Some((crate::i18n::app_tray_failed().to_owned(), now + NOTICE_SECS));
        }
    }

    /// 드라이브 줄 워커의 결과를 거둔다 (T4 · FR-67).
    ///
    /// **세 번 온다** — 목록이 먼저, 네트워크 드라이브가 있으면 그 접근 판정이 뒤이어,
    /// 즐겨찾기 실재가 **언제나 마지막**이다. 마지막 소식을 받으면 통로를 놓는다.
    ///
    /// **한 번뿐인 조회가 아니다** — `pump_rescan`이 기본 틱마다 워커를 다시 띄운다(FR-67).
    /// 그 사이의 상태 갱신은 사용자가 그 드라이브를 열어 볼 때 `DriveList::observe`가 한다.
    ///
    /// **통로가 둘이다** — 위의 것과 별개로 `drive_watch`가 드라이브의 꽂힘·빠짐을 알려 온다
    /// (2026-09-09). 그쪽은 목록만 보내고 끝나지 않으므로 반영도 갈래도 다르다
    fn poll_drives(&mut self) {
        // 꽂히고 빠지는 것을 지켜보는 워커가 보낸 목록 — **`refresh`로 반영한다**.
        // `replace`를 쓰면 그 순간 끊긴 네트워크 드라이브의 X 배지가 사라졌다가
        // 다음 주기 확인(30초)에야 돌아온다(`DriveList::refresh` doc)
        while let Ok(rows) = self.drive_watch.try_recv() {
            self.drives.refresh(rows);
        }
        let Some(rx) = &self.drive_scan else {
            return;
        };
        let mut done = false;
        loop {
            match rx.try_recv() {
                Ok(crate::fs::drives::DriveScan::Listed(rows)) => self.drives.replace(rows),
                Ok(crate::fs::drives::DriveScan::Reachability(judged)) => {
                    self.drives.apply_reachable(&judged);
                }
                Ok(crate::fs::drives::DriveScan::Missing(missing)) => {
                    // 즐겨찾기 실재가 **마지막 소식**이다 (FR-67) — 네트워크 드라이브가
                    // 하나도 없어 판정이 오지 않는 PC에서도 이것은 온다
                    self.favorites.set_missing(&missing);
                    done = true;
                }
                // 워커가 보낼 것을 다 보내고 끝났다(네트워크 드라이브가 없으면 판정도 없다)
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    done = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
            }
        }
        if done {
            self.drive_scan = None;
        }
    }

    /// 트레이에서 온 통지를 처리한다 — 창을 띄우는 일은 프로시저가 이미 끝냈다
    fn poll_tray(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.tray_rx.try_recv() {
            match event {
                // 창은 프로시저가 이미 띄웠다. 여기서는 ⓐ 숨김 상태를 내리고
                // ⓑ winit의 가시성 캐시를 맞춘다 — 그러지 않으면 이후 최대화 등
                // 창 플래그가 바뀔 때 `apply_diff`가 `SW_HIDE`를 다시 적용해 창이 사라진다
                TrayEvent::Shown => {
                    if self.hidden {
                        self.hidden = false;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    }
                }
                // 탐색기가 되살아나 아이콘이 사라졌다 — 비워 두면 다음 `sync_tray`가 다시 올린다
                TrayEvent::Recreated => self.tray = None,
                // 메뉴 `종료` — 평소 닫기와 같은 길로 보낸다(세션 저장이 그 길에 있다)
                TrayEvent::Quit => {
                    // 이 닫기는 가로채지 않는다 — 트레이로 보내는 것이 아니라 실제 종료다.
                    // 프로시저가 창을 이미 되살렸으므로 숨김 상태도 함께 내린다 —
                    // 그러지 않으면 마지막 프레임에 `track_window`가 멈춘 채로 끝난다
                    self.quitting = true;
                    self.hidden = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
    }

    /// 앱 설정 대화 (FR-47) — 바뀐 값은 그 자리에서 저장한다.
    ///
    /// 즉시 저장인 이유는 이 화면에 `취소`가 없기 때문이다(사용자 결정) — 닫기만 있는
    /// 화면에서 저장을 닫을 때로 미루면, 앱이 그 사이에 죽었을 때 바꾼 값이 사라진다
    fn show_settings_dialog(&mut self, ctx: &egui::Context) {
        if !self.settings_dialog.is_open() {
            return;
        }
        // 목록은 앱이 시작할 때 미리 읽어 둔다 — 여기서 다시 부르는 것은 그 워커가
        // 결과 없이 사라졌을 때를 위한 대비다(이미 목록이 있으면 아무 일도 하지 않는다)
        self.font_scan.ensure_started(ctx);
        self.font_scan.poll();

        let outcome = self.settings_dialog.show(
            ctx,
            &mut self.settings,
            FontChoices {
                names: self.font_scan.names(),
            },
        );
        if outcome.language_changed {
            // 이 프레임은 이미 옛 언어로 그려졌다 — 다음 프레임이 오도록 청한다 (전제 3-b)
            crate::i18n::set_language(self.settings.language);
            // 창 밖에 있는 두 이름은 다시 그린다고 따라오지 않는다 — 앱이 직접 알린다 (FR-53).
            // 창 제목은 작업 표시줄·Alt+Tab에, 툴팁은 알림 영역에 보인다
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(
                crate::i18n::app_name().to_owned(),
            ));
            if let Some(tray) = &self.tray {
                tray.update_tooltip();
            }
            ctx.request_repaint();
        }
        if outcome.font_changed {
            // 글꼴은 다음 pass부터 적용된다 — 그 프레임이 오도록 다시 그리기를 요청한다
            self.korean_font = install_fonts(ctx, self.settings.selected_font());
            ctx.request_repaint();
        }
        // 자동 실행 등록이 막힌 환경 등 — 조용히 넘기면 토글이 왜 안 움직였는지 알 수 없다
        if let Some(notice) = outcome.notice {
            self.toast
                .show(notice.to_owned(), ctx.input(|input| input.time));
        }
        if outcome.changed {
            self.persist_session();
        }
    }

    /// 사이드바에서 사이트를 지우기 전의 확인 (FR-29 · D4).
    ///
    /// **도는 전송이 있을 때만 뜬다** — 끝난·실패한 항목만 남았으면 묻지 않는다(그것들은 기록일
    /// 뿐 다시 걸 수 있다). 배경 클릭·`Esc`는 취소로 본다: 지우는 쪽이 기본값이 되면 실수로 지운다.
    ///
    /// `area`가 아직 없으면(첫 프레임) 그 프레임에는 실행하지 않고 기다린다 — 패널을 닫으려면
    /// 배치를 알아야 하고, 임의의 영역을 지어내면 엉뚱한 패널이 닫힌다
    fn show_site_remove_confirm(&mut self, ctx: &egui::Context, area: Option<LayoutRect>) {
        let Some(site) = self.pending_site_remove else {
            return;
        };
        let Some(name) = self.sites.get(site).map(|record| record.name.clone()) else {
            // 묻는 사이에 대상이 사라졌다(사이트 관리자에서 지웠다) — 물을 것이 없다
            self.pending_site_remove = None;
            return;
        };
        let running = self
            .queue
            .site_items(site)
            .iter()
            .filter(|item| item.state.is_pending())
            .count();
        let buttons = [
            dialog::ButtonSpec::strong(crate::i18n::delete()),
            dialog::ButtonSpec::plain(crate::i18n::cancel()),
        ];
        let shell = dialog::show(
            ctx,
            egui::Id::new("사이드바 사이트 삭제 확인"),
            REMOVE_DIALOG_WIDTH,
            &buttons,
            |ui| {
                ui.heading(crate::i18n::sidebar_site_remove_title());
                ui.add_space(8.0);
                ui.label(crate::i18n::dynamic::sidebar_site_remove_confirm(
                    &name, running,
                ));
                ui.label(crate::i18n::sidebar_site_remove_detail());
            },
        );
        let confirmed = match shell.clicked {
            Some(0) => Some(true),
            Some(_) => Some(false),
            None => shell.should_close.then_some(false),
        };
        match confirmed {
            Some(true) => {
                // 배치를 알아야 패널을 닫을 수 있다 — 모르는 프레임에는 대기를 유지한다
                let Some(area) = area else {
                    return;
                };
                self.pending_site_remove = None;
                let now = ctx.input(|input| input.time);
                self.detach_site(site, area, ctx, now);
            }
            Some(false) => self.pending_site_remove = None,
            None => {}
        }
    }

    /// 사이트 관리자 대화 (FR-27) — 등록 결과를 받아 연결까지 잇는다.
    ///
    /// `area`는 `연결(C)`이 패널을 좌우로 나눌 때 쓴다(T14와 같은 착지점). 아직 배치를 모르는
    /// 첫 프레임에는 `None`이라, 그때는 연결 대신 등록만 하고 사용자가 다시 누르면 된다 —
    /// 임의의 영역을 지어내 엉뚱한 자리에 패널을 만들지 않는다
    fn show_site_manager(&mut self, ctx: &egui::Context, area: Option<LayoutRect>) {
        if !self.site_manager.is_open() {
            return;
        }
        let connected = self.connected_sites();
        let outcome = self.site_manager.show(ctx, &mut self.sites, &connected);
        match outcome {
            SiteManagerOutcome::None => {}
            // 대화 안에서 목록이 바뀌었을 수 있다(이름·삭제·복제·추가·차례) — 닫을 때 함께 적는다
            SiteManagerOutcome::Close => self.persist_session(),
            // 등록만 했으면 그 사실을 짧게 알린다 (인벤토리 #89·#91)
            SiteManagerOutcome::Register(site) => {
                self.persist_session();
                let host = self
                    .sites
                    .get(site)
                    .map(|record| record.host.clone())
                    .unwrap_or_default();
                let now = ctx.input(|input| input.time);
                self.toast.show(toast::registered_text(&host), now);
            }
            SiteManagerOutcome::RegisterAndConnect(site) => {
                self.persist_session();
                if let Some(area) = area {
                    self.open_site_tab(site, None, area);
                }
            }
        }
        // 겹치는 사이트 확인에서 고른 결과가 여기서 나온다 (FR-59)
        self.flush_site_notice(ctx);
    }

    /// 사이트 관리자가 청한 파일 대화를 띄운다 (FR-59).
    ///
    /// **부르는 자리가 정해져 있다** — `IFileDialog::Show`는 셸 컨텍스트 메뉴와 마찬가지로 자체
    /// 메시지 루프를 돌려 이벤트 루프를 재진입시키므로, 위젯 트리를 만드는 도중이 아니라
    /// **그리기가 모두 끝난 뒤**에 띄운다. 그래서 이 함수는 `update`의 맨 끝에서만 불린다.
    ///
    /// 대화가 닫힌 뒤 이어지는 일(파일을 읽고 쓰며 봉투를 여닫는 것)도 **이 스레드에서
    /// 그대로 돈다** (plan D13) — 사용자가 직접 누른 드문 조작이고 그 앞뒤가 모두 모달이라
    /// 그 사이에는 화면을 만질 일이 없다. 지금 내보내기는 앱 내장 키라 파생이 1회로 끝나지만,
    /// 직전 버전이 만든 암호 보호 파일을 가져올 때는 사용자 암호 파생이 돌아 릴리즈 빌드에서
    /// 0.126초가 걸린다 (`remote::envelope`의 반복 횟수 주석).
    ///
    /// 이 배선은 **리뷰가 지키는 자리**다 — `ExplorerApp`은 실 창 핸들이 있어야 만들어져
    /// 프레임을 돌리는 시험을 세울 수 없다(AGENTS: HWND가 필요한 UI 로직은 시험 비대상)
    fn pump_site_file_dialog(&mut self, ctx: &egui::Context) {
        let Some(request) = self.site_manager.take_file_request() else {
            return;
        };
        let Some(shell) = self.shell.as_ref() else {
            // 창 핸들이 없으면 띄울 수 없다 — 조용히 접지 않고 관리자에 사유를 남긴다
            self.site_manager
                .fail_file_request(crate::i18n::site_file_dialog_unavailable());
            return;
        };
        let picked = match request {
            FileRequest::Save { suggested } => {
                crate::fs::file_dialog::pick_save(shell.hwnd(), &suggested)
            }
            FileRequest::Open => crate::fs::file_dialog::pick_open(
                shell.hwnd(),
                crate::fs::file_dialog::FileFilter::SiteList,
            ),
            // 개인 키는 확장자가 없는 것이 보통이라 사이트 목록 필터를 걸면 보이지 않는다
            FileRequest::OpenKey => crate::fs::file_dialog::pick_open(
                shell.hwnd(),
                crate::fs::file_dialog::FileFilter::AnyFile,
            ),
        };
        self.site_manager.supply_file(picked, &mut self.sites);
        self.flush_site_notice(ctx);
    }

    /// 내보내기·가져오기 결과를 알린다 (FR-59).
    ///
    /// 목록이 바뀌었을 수 있으므로 함께 적는다 — 내보내기는 목록을 바꾸지 않지만, 그때 한 번 더
    /// 적는 값이 설정 파일 쓰기 한 번뿐이라 「바뀌었는가」를 따로 알리는 계약을 두지 않는다
    fn flush_site_notice(&mut self, ctx: &egui::Context) {
        let Some(text) = self.site_manager.take_notice() else {
            return;
        };
        self.persist_session();
        let now = ctx.input(|input| input.time);
        self.toast.show(text, now);
    }

    /// 워크스페이스와 그 탐색 상태(패널·탭·열거 스레드)를 함께 버린다
    fn remove_workspace(&mut self, index: usize) {
        let removed_id = self.workspaces.items().get(index).map(|item| item.id);
        if self.workspaces.remove(index).is_ok()
            && let Some(id) = removed_id
        {
            self.views.remove(&id);
            // 한 번도 열지 않은 워크스페이스였다면 불러온 상태가 여기 남아 있다
            self.restored.remove(&id);
        }
    }

    /// 활성 워크스페이스의 부제를 현재 폴더로 맞춘다 (FR-15 2줄 카드)
    fn sync_subtitle(&mut self) {
        let index = self.workspaces.active_index();
        let id = self.workspaces.active().id;
        let Some(dir) = self.views.get(&id).and_then(|v| v.active_dir()) else {
            return;
        };
        self.workspaces.set_subtitle(index, &dir);
    }

    /// 메뉴·단축키 명령 실행 (FR-12·FR-26).
    ///
    /// `target`은 명령이 향할 패널이다 — 패널 메뉴에서 온 명령은 **메뉴를 연 패널**을 담아
    /// 오고(plan D16), 단축키·타이틀바에서 온 명령은 `None`이라 활성 패널이 대상이 된다.
    /// `area`는 분할·닫기에 쓰이며 레이아웃을 그리기 전에 확정된 영역이어야 한다
    fn apply_command(
        &mut self,
        command: Command,
        target: Option<PanelId>,
        area: LayoutRect,
        ctx: &egui::Context,
    ) {
        match command {
            // 분할·닫기는 활성 워크스페이스의 뷰를 대상으로 한다(없으면 여기서 만들어진다)
            Command::Split(to) => {
                let (dir, place) = to.to_layout();
                let view = self.ensure_active_view();
                let panel = target.unwrap_or(view.active);
                view.split_panel(panel, dir, place, area);
            }
            Command::ClosePanel => {
                let view = self.ensure_active_view();
                let panel = target.unwrap_or(view.active);
                // 패널이 사라지면 그 탭들이 쓰던 연결도 쓸 곳이 없어진다 — 닫기 전에 모아 둔다
                let conns = view
                    .panels
                    .get(&panel)
                    .map(PanelState::conns)
                    .unwrap_or_default();
                view.close_panel(panel, area);
                // 마지막 패널은 닫히지 않는다 (FR-2) — 그때는 연결도 그대로 둔다
                let closed = !view.panels.contains_key(&panel);
                if closed {
                    for conn in conns {
                        // 다른 패널이 아직 그 연결을 쓰고 있으면 접지 않는다 (FR-32)
                        if !self.conn_in_use(conn) {
                            self.release_conn(conn);
                        }
                    }
                }
            }
            Command::ToggleSidebar => self.sidebar_collapsed = !self.sidebar_collapsed,
            Command::OpenAppSettings => self.settings_dialog.open(),
            Command::OpenLicenses => self.license_dialog.open(),
            Command::OpenAbout => self.about_dialog.open(),
            Command::CheckUpdate => self.check_update_by_hand(),
            Command::StartUpdate => self.start_update(ctx),
            Command::OpenReleaseNotes => {
                // 특정 판이 아니라 목록을 연다 — 이력 전체가 릴리즈 노트이고, 릴리즈가
                // 하나도 없어도 그 페이지는 성립한다 (D10)
                ctx.open_url(egui::OpenUrl::new_tab(crate::i18n::releases_url()));
            }
            // 파일 대상 명령 — 셸에 걸려면 주인 창과 결과 채널이 필요해 여기서 따로 받는다.
            //
            // **원격 탭이면 원격 기능으로 간다** (FR-12·D5) — `route_to_remote`가 참을
            // 돌려주면 이미 처리된 것이고, 거짓이면 아래 로컬 처리가 이어진다
            Command::Rename => {
                if self.route_to_remote(command, target) {
                    return;
                }
                if let Some(panel) = self.command_panel_mut(target) {
                    panel.begin_rename_selected();
                }
            }
            Command::Delete { permanent } => {
                if self.route_to_remote(command, target) {
                    return;
                }
                self.delete_selected(target, permanent);
            }
            Command::ClipboardCopy => self.clipboard_put(target, false),
            Command::ClipboardCut => self.clipboard_put(target, true),
            Command::ClipboardPaste => self.clipboard_paste(target),
            // 이 셋은 연결(`manager`)에 닿아야 해서 패널만 빌리는 아래 묶음에 들어갈 수 없다
            Command::OpenSiteTab(site) => self.open_site_tab_here(site, target),
            Command::Refresh => self.refresh_panel(target, ctx),
            Command::CloseTab => self.close_tab(target, ctx),
            Command::NewTab
            | Command::Back
            | Command::Forward
            | Command::Up
            | Command::NewFile
            | Command::NewFolder
            | Command::SetViewMode(_) => {
                // 새 폴더만 원격 대응이 있다 (FR-12·D5) — 나머지 여섯은 패널 층에서
                // 원격 탭에도 그대로 듣는다
                if self.route_to_remote(command, target) {
                    return;
                }
                let Some(panel) = self.command_panel_mut(target) else {
                    return;
                };
                match command {
                    Command::NewTab => panel.new_tab(ctx),
                    Command::Back => panel.go_back(ctx),
                    Command::Forward => panel.go_forward(ctx),
                    Command::Up => panel.go_up(ctx),
                    Command::NewFile => panel.new_file(ctx),
                    Command::NewFolder => panel.new_folder(ctx),
                    Command::SetViewMode(mode) => panel.set_view_mode(mode),
                    // 위 분기에서 걸러진 명령들 — 여기 오지 않는다
                    _ => {}
                }
            }
        }
    }

    /// 명령이 향하는 패널 — 대상이 지정되면 그 패널, 아니면 활성 패널
    fn command_panel_mut(&mut self, target: Option<PanelId>) -> Option<&mut PanelState> {
        let view = self.ensure_active_view();
        let id = target.unwrap_or(view.active);
        view.panels.get_mut(&id)
    }

    /// 보고 있는 위치를 다시 읽는다 — 원격 탭은 서버에 다시 묻고, 로컬 탭은 폴더를 다시 연다.
    ///
    /// `apply_command`의 다른 명령들과 갈라 둔 이유: 원격 요청은 연결(`manager`)이 필요한데
    /// `command_panel_mut`이 `self`를 통째로 빌려 그 안에서는 연결에 닿을 수 없다
    fn refresh_panel(&mut self, target: Option<PanelId>, ctx: &egui::Context) {
        self.ensure_active_view();
        let id = self.workspaces.active().id;
        let ExplorerApp { views, manager, .. } = self;
        let Some(view) = views.get_mut(&id) else {
            return;
        };
        let panel_id = target.unwrap_or(view.active);
        let Some(panel) = view.panels.get_mut(&panel_id) else {
            return;
        };
        if panel.is_remote() {
            panel.request_remote_list(id, panel_id, manager);
        } else {
            panel.refresh(ctx);
        }
    }

    /// 활성 탭을 닫는다 — 그 탭이 패널의 마지막 원격 탭이었으면 연결도 함께 접는다 (FR-32)
    fn close_tab(&mut self, target: Option<PanelId>, ctx: &egui::Context) {
        let Some(panel) = self.command_panel_mut(target) else {
            return;
        };
        if let Some(conn) = panel.close_tab(ctx) {
            // 접기 전에 물어 둔 확인을 거둔다 — `release_conn`과 같은 이유다
            self.abandon_conflict_lists(conn);
            self.manager.close(conn);
        }
    }

    /// 끌던 로컬 항목이 창 밖으로 나갔으면 OS 드래그로 넘긴다 (FR-61 내보내기).
    ///
    /// **끌기 시작과 동시에 넘기지 않고 창을 벗어날 때까지 미룬다** — `DoDragDrop`이
    /// 자기 메시지 루프를 돌려 그동안 앱이 다시 그려지지 않으므로, 앱 안에서 끝날 드래그
    /// (탭↔탭 복사·원격 전송)까지 그 길로 보내면 드롭 대상 강조·목록 반응이 멎는다.
    /// 창 안에서는 종전의 egui 경로가 그대로 처리한다.
    ///
    /// 넘길 때 egui의 페이로드를 거둔다 — 그러지 않으면 손을 놓는 순간 앱 안 드롭까지
    /// 함께 성립해 같은 것을 두 번 처리한다
    fn pump_export_drag(&mut self, ctx: &egui::Context) {
        // 끌고 있는 것이 **전부 로컬 항목**일 때만 대상이다 — 원격 항목은 끌기 시작
        // 시점에 로컬에 파일이 없어 셸에 넘길 것이 없다(지연 렌더링은 이번 범위 밖)
        let Some(drag) = egui::DragAndDrop::payload::<list_common::FileDrag>(ctx) else {
            return;
        };
        let Some(sources) = drag
            .items
            .iter()
            .map(|item| match item {
                list_common::DragItem::Local { path, .. } => Some(path.clone()),
                list_common::DragItem::Remote { .. } => None,
            })
            .collect::<Option<Vec<std::path::PathBuf>>>()
        else {
            return;
        };
        if sources.is_empty() {
            return;
        }
        // 포인터가 창 밖으로 나갔는가 — 나가기 전에는 앱 안 드래그다
        let inside = ctx.input(|input| {
            input
                .pointer
                .latest_pos()
                .is_some_and(|pos| input.viewport_rect().contains(pos))
        });
        if inside {
            return;
        }
        // 여기서부터는 OS가 드래그를 쥔다 — 앱 안 드롭이 겹치지 않게 페이로드를 거둔다
        egui::DragAndDrop::clear_payload(ctx);
        // 그림 크기는 여기서 정해 내려보낸다 — `fs`는 화면 배율을 모른다.
        // 96 논리 픽셀은 탐색기가 그리는 드래그 그림과 비슷한 크기다
        let preview_px = (96.0 * ctx.pixels_per_point()).round() as i32;
        crate::fs::drag_source::start_copy_drag(&sources, preview_px);
    }

    /// OS(탐색기·바탕화면)에서 끌어온 파일을 받는다 (FR-61).
    ///
    /// **놓인 자리는 Win32 커서로 잰다** — 파일 드롭 이벤트에 좌표가 실려 있지 않고
    /// (`egui::DroppedFile`은 경로만 채운다) OS 드래그 중에는 `WM_MOUSEMOVE`가 오지 않아
    /// egui가 아는 포인터 자리도 낡아 있다.
    ///
    /// 대상 패널의 종류가 처리를 가른다 — 로컬 탭이면 셸 복사(경로만 있으면 되므로 그대로
    /// 넘긴다), 원격 탭이면 올리기다. 올리기는 `DragItem::Local`이 폴더 여부를 요구하는데
    /// 그것을 재는 것은 파일시스템 호출이라 **워커에 맡긴다**(수천 개를 끌어다 놓을 수 있다 —
    /// AGENTS: UI 스레드에서 파일시스템 블로킹 호출 금지)
    fn pump_os_drop(&mut self, ctx: &egui::Context, pane_rects: &[(PanelId, egui::Rect)]) {
        let dropped: Vec<std::path::PathBuf> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });
        if dropped.is_empty() {
            return;
        }
        // 커서를 읽지 못하면 어디에 놓았는지 알 수 없다 — 아무 일도 하지 않는다
        let Some(shell) = self.shell.as_ref() else {
            return;
        };
        let Some(cursor) = shell.cursor_client_pos(ctx.pixels_per_point()) else {
            return;
        };
        let Some(target) = panel_at(pane_rects, cursor) else {
            // 사이드바·전송 큐·제목 표시줄 위에 놓았다 (FR-61)
            return;
        };
        // 놓인 자리를 값으로 뽑아 뷰 빌림을 끝낸다 — 아래 두 갈래가 모두 `self`를 쓴다
        let landed = self
            .views
            .get(&self.workspaces.active().id)
            .and_then(|view| view.panels.get(&target))
            .map(|panel| match (panel.active_site(), panel.remote_dir()) {
                (Some(site), Some(dir)) => OsDropTarget::Remote { site, dir },
                _ => OsDropTarget::Local(panel.dir().to_path_buf()),
            });
        match landed {
            // 원격 탭 — 폴더 여부를 워커가 재고 그 결과로 전송을 건다
            Some(OsDropTarget::Remote { site, dir }) => self.spawn_os_drop_scan(site, dir, dropped),
            // 로컬 탭 — `IFileOperation`은 경로만 받으므로 재 볼 것이 없다
            Some(OsDropTarget::Local(dest)) => self.start_local_copy_paths(dest, dropped),
            None => {}
        }
    }

    /// OS에서 끌어온 경로들의 폴더 여부를 워커에서 재 전송으로 잇는다 (FR-61).
    ///
    /// 결과를 채널로 받아 다음 프레임에 `start_transfer`로 보낸다 — 펼치기(`expand_rx`)와
    /// 같은 방식이며, 그래야 수천 개를 끌어다 놓아도 프레임이 멈추지 않는다
    fn spawn_os_drop_scan(
        &mut self,
        site: crate::remote::types::SiteId,
        dir: crate::remote::types::RemotePath,
        paths: Vec<std::path::PathBuf>,
    ) {
        let tx = self.os_drop_tx.clone();
        let wake = self.repaint.clone();
        std::thread::spawn(move || {
            let items = paths
                .into_iter()
                .map(|path| {
                    let is_dir = path.is_dir();
                    list_common::DragItem::Local { path, is_dir }
                })
                .collect();
            if tx.send((site, dir, items)).is_ok() {
                wake();
            }
        });
    }

    /// 폴더 여부를 다 잰 OS 드롭을 전송으로 보낸다 (FR-61)
    fn pump_os_drop_scan(&mut self) {
        while let Ok((site, dir, items)) = self.os_drop_rx.try_recv() {
            // 재는 사이에 지운 사이트면 버린다 (FR-29 — 위 `expand_rx`와 같은 이유)
            if self.detached_sites.contains(&site) {
                continue;
            }
            self.start_transfer(list_common::DropOutcome {
                items,
                source_site: None,
                target: list_common::DropTarget::Remote { site, dir },
            });
        }
    }

    /// 경로 목록을 셸 복사에 건다 — **복사를 거는 유일한 자리다**.
    ///
    /// 앱 안의 드래그(FR-60)는 `start_local_copy`가 `DropOutcome`에서 경로를 뽑아
    /// 여기로 넘기고, OS 드롭(FR-61)은 받은 목록을 그대로 넘긴다
    fn start_local_copy_paths(
        &mut self,
        dest: std::path::PathBuf,
        sources: Vec<std::path::PathBuf>,
    ) {
        let owner = self.owner_hwnd();
        crate::fs::file_op::copy_into(
            dest,
            sources,
            owner,
            self.copy_tx.clone(),
            self.repaint.clone(),
        );
    }

    /// 로컬끼리의 복사를 셸에 건다 (FR-60).
    ///
    /// **FR-55의 같은 이름 확인을 거치지 않는다** — `IFileOperation`이 자기 충돌 대화를
    /// 띄우므로 앞에 하나 더 두면 같은 것을 두 번 묻게 되고, 앱 대화에서 `덮어쓰기`를 골라도
    /// 셸이 다시 묻는다 (plan D9)
    fn start_local_copy(&mut self, dest: std::path::PathBuf, drop: &list_common::DropOutcome) {
        let sources: Vec<std::path::PathBuf> = drop
            .items
            .iter()
            .filter_map(|item| match item {
                list_common::DragItem::Local { path, .. } => Some(path.clone()),
                list_common::DragItem::Remote { .. } => None,
            })
            .collect();
        self.start_local_copy_paths(dest, sources);
    }

    /// 끝난 셸 복사의 결과를 알린다 (FR-60).
    ///
    /// **성공은 알리지 않는다** — 셸이 자기 진행률 대화로 이미 알렸고 대상 목록에 파일이
    /// 나타나는 것이 곧 확인이라, 여기서 또 띄우면 조작 하나에 알림이 둘이 된다
    fn pump_local_copy(&mut self, now: f64) {
        while let Ok(outcome) = self.copy_rx.try_recv() {
            let text = match (&outcome.error, outcome.cancelled) {
                (Some(detail), _) => crate::i18n::dynamic::local_copy_failed(detail),
                (None, true) => crate::i18n::dynamic::local_copy_cancelled(outcome.requested),
                (None, false) => continue,
            };
            self.notice = Some((text, now + NOTICE_SECS));
        }
    }

    /// 끝난 이름 바꾸기·삭제의 결과를 알린다 (FR-64).
    ///
    /// **성공도 취소도 알리지 않는다** — 셸이 자기 대화로 이미 알렸고, 목록에서 사라지거나
    /// 이름이 바뀌는 것(또는 그대로 남는 것)이 곧 확인이라 여기서 또 띄우면 조작 하나에
    /// 알림이 둘이 된다. 복사(`pump_local_copy`)가 취소를 알리는 것은 **여러 개를 끌어다
    /// 놓은 뒤라 몇 개가 갔는지 목록만 보고는 알기 어렵기** 때문이며, 여기는 대상이 눈앞에
    /// 그대로 있어 그 어려움이 없다
    /// 고른 것을 지운다 (FR-12·FR-64) — 확인 대화는 셸이 띄운다.
    ///
    /// 고른 것이 없으면 아무 일도 하지 않는다 — `Delete`를 헛눌렀을 때 대화만 뜨는 것보다
    /// 조용한 편이 낫다. 원격 탭은 여기 오지 않는다(`selected_local`이 빈 목록을 준다)
    fn delete_selected(&mut self, target: Option<PanelId>, permanent: bool) {
        let owner = self.owner_hwnd();
        let Some(panel) = self.command_panel_mut(target) else {
            return;
        };
        let paths: Vec<std::path::PathBuf> = panel
            .selected_local()
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        crate::fs::file_op::delete_items(
            paths,
            permanent,
            owner,
            self.file_op_tx.clone(),
            self.repaint.clone(),
        );
    }

    /// 고른 것을 클립보드에 담는다 (FR-12·FR-64) — `cut`이면 잘라내기다.
    ///
    /// 담긴 뒤에만 화면의 잘라내기 표시를 손댄다 — 담기지 못했으면 클립보드에는 종전 것이
    /// 그대로 남아 있어, 표시를 바꾸면 화면과 클립보드가 어긋난다
    fn clipboard_put(&mut self, target: Option<PanelId>, cut: bool) {
        let Some(panel) = self.command_panel_mut(target) else {
            return;
        };
        let paths: Vec<std::path::PathBuf> = panel
            .selected_local()
            .into_iter()
            .map(|(path, _)| path)
            .collect();
        if paths.is_empty() || !crate::fs::clipboard::put(&paths, cut) {
            return;
        }
        if cut {
            self.set_cut_marks(&paths);
        } else {
            self.clear_cut_marks();
        }
    }

    /// 클립보드에 담긴 것을 지금 폴더에 붙여넣는다 (FR-12·FR-64).
    ///
    /// 하는 일은 둘이다. **먼저 읽은 값으로 화면의 잘라내기 표시를 맞추고**(FR-64 —
    /// 다른 앱이 그 사이 클립보드를 가져갔을 수 있다), 그다음 로컬 대상이면 옮기거나 복사한다.
    ///
    /// 복사로 담겼으면 복사하고 **잘라내기로 담겼으면 옮긴다** — 어느 쪽인지는 클립보드가
    /// 함께 든 `Preferred DropEffect`가 말한다(탐색기가 담은 것도 같은 형식이라 그대로 읽힌다).
    ///
    /// 옮기기가 걸렸으면 잘라내기 표시를 푼다 — 셸이 실제로 옮기기를 마치기 전이지만,
    /// 그 결과를 기다려 푸는 것은 워커가 돌아올 때까지 화면이 거짓을 보이게 한다
    fn clipboard_paste(&mut self, target: Option<PanelId>) {
        let files = crate::fs::clipboard::take();
        // **읽은 값으로 화면의 잘라내기 표시부터 맞춘다** (FR-64 세 번째 해제 조건).
        // 우리가 잘라낸 뒤 다른 앱이 클립보드를 가져갔으면 흐린 줄은 이미 클립보드에 없는
        // 것을 가리킨다 — 여기서 풀지 않으면 영영 흐린 채 남는다. **붙여넣기에 성공했을
        // 때가 아니라 눌렀을 때** 맞추는 것이 요점이라, 아래 되돌아가는 갈래보다 앞에 둔다
        let marks = crate::fs::clipboard::cut_marks_for(files.as_ref());
        if marks.is_empty() {
            self.clear_cut_marks();
        } else {
            self.set_cut_marks(marks);
        }
        let Some(files) = files else {
            return;
        };
        let owner = self.owner_hwnd();
        let Some(panel) = self.command_panel_mut(target) else {
            return;
        };
        // 원격 탭에는 붙여넣을 로컬 폴더가 없다 (plan Out of Scope — 전송은 큐가 담당한다)
        if panel.is_remote() {
            return;
        }
        let dest = panel.dir().to_path_buf();
        let started = if files.cut {
            crate::fs::file_op::move_into(
                dest,
                files.paths,
                owner,
                self.copy_tx.clone(),
                self.repaint.clone(),
            )
        } else {
            crate::fs::file_op::copy_into(
                dest,
                files.paths,
                owner,
                self.copy_tx.clone(),
                self.repaint.clone(),
            )
        };
        if started && files.cut {
            self.clear_cut_marks();
        }
    }

    /// 셸 대화·메뉴 실행의 **주인 창** — 창이 아직 없으면 주인 없이 띄운다.
    ///
    /// 창을 얻지 못했을 때 셸 대화가 앱 위에 서지 않을 뿐, 하려던 일은 그대로 된다.
    /// 부르는 자리가 다섯이라 한 곳에 모았다(복사·이동·삭제·이름 바꾸기·메뉴 실행)
    pub(super) fn owner_hwnd(&self) -> windows::Win32::Foundation::HWND {
        self.shell
            .as_ref()
            .map(|shell| shell.hwnd())
            .unwrap_or_default()
    }

    /// 잘라내기 표시를 이 워크스페이스의 **모든 패널**에 건다 (FR-64).
    ///
    /// 표시를 연 패널에만 두지 않는 이유: 한쪽에서 잘라내 다른 쪽에 붙여넣는 것이 보통이라,
    /// 같은 폴더를 보고 있는 옆 패널에서 그 항목만 멀쩡해 보이면 어느 쪽이 맞는지 알 수 없다
    fn set_cut_marks(&mut self, paths: &[std::path::PathBuf]) {
        let view = self.ensure_active_view();
        for panel in view.panels.values_mut() {
            panel.set_cut_marks(paths);
        }
    }

    /// 잘라내기 표시를 모두 푼다 — 다른 것이 클립보드에 담겼을 때 (FR-64)
    fn clear_cut_marks(&mut self) {
        let view = self.ensure_active_view();
        for panel in view.panels.values_mut() {
            panel.clear_cut_marks();
        }
    }

    /// 목록에서 확정한 이름을 셸에 건다 (FR-64).
    ///
    /// 결과는 메뉴의 삭제·복사와 **같은 채널**로 온다 — 실패하면 `pump_file_op`가 알린다.
    /// 성공했을 때 목록을 다시 읽지 않는 이유: 폴더 감시(FR-10)가 그 변화를 통지하며,
    /// 이는 메뉴에서 지웠을 때와 같은 처리다
    fn start_rename(&mut self, request: crate::ui::panel::RenameRequest) {
        let owner = self.owner_hwnd();
        crate::fs::file_op::rename_item(
            request.path,
            request.new_name,
            owner,
            self.file_op_tx.clone(),
            self.repaint.clone(),
        );
    }

    fn pump_file_op(&mut self, now: f64) {
        while let Ok(outcome) = self.file_op_rx.try_recv() {
            let Some(detail) = outcome.error else {
                continue;
            };
            self.notice = Some((detail, now + NOTICE_SECS));
        }
    }

    /// 지금 상태를 곧바로 세션 파일에 적는다 (FR-44).
    ///
    /// **사이트 목록이 바뀌면 그 자리에서 적는다** — 종료 때만 적으면 그 사이에 앱이
    /// 비정상 종료됐을 때(패닉·강제 종료·전원 차단) 등록한 사이트가 통째로 사라진다.
    /// 파일이 작고 사이트 등록은 드문 일이라 그때마다 적어도 부담이 없다
    fn persist_session(&mut self) {
        let session = self.collect_session();
        save_session(&session);
        // 방금 적은 것을 기준으로 삼는다 — 알리지 않으면 자동 저장이 같은 내용을 한 번 더 적는다
        self.autosave.mark(session);
    }

    /// 바뀐 것이 있으면 곧 세션 파일에 적는다 (`app::autosave`).
    ///
    /// **탭·분할·창 크기·즐겨찾기처럼 저장을 부르지 않던 것까지 여기서 담긴다** —
    /// 상태를 바꾸는 자리마다 저장을 심는 대신 바뀐 결과를 지켜본다 (2026-08-21 사용자 요청)
    /// 보이는 원격 탭의 목록을 주기적으로 다시 조회한다 (FR-67).
    ///
    /// **틱은 두 층이다** — 깨어나는 간격은 `min(기본 틱, 설정 주기)`이고 설정을 꺼도 계속
    /// 건다. 끄면 아래 원격 분기만 건너뛰고, 그 틱 위에서 도는 로컬 재확인(드라이브·즐겨찾기)은
    /// 서버 부담과 무관해 함께 멎으면 안 되기 때문이다. 깨어나 아무것도 하지 않고 다시 자는
    /// 비용이라 유휴 부담이 사실상 없다
    fn pump_auto_refresh(&mut self, ctx: &egui::Context, now: f64) {
        let period = f64::from(remote_refresh_secs(self.settings.remote_refresh_secs));
        let tick = if self.settings.remote_auto_refresh {
            period.min(f64::from(BASE_TICK_SECS))
        } else {
            f64::from(BASE_TICK_SECS)
        };
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(tick));

        // 닫힌 패널의 자리를 지운다 — `PanelId`는 다시 쓰이지 않아 두면 계속 쌓인다.
        // 패널이 몇 개뿐이라 훑는 비용이 무시할 만하고, 닫는 자리마다 지우게 하면
        // 패널을 닫는 길이 늘 때마다 여기를 함께 고쳐야 한다
        let ExplorerApp {
            auto_refresh_at,
            views,
            ..
        } = self;
        auto_refresh_at.retain(|id, _| views.values().any(|view| view.panels.contains_key(id)));

        // 전송이 도는 중이면 서버를 더 붙잡지 않는다 — 판정은 아래 순수 함수가 한다
        let transferring = self
            .queue
            .items()
            .iter()
            .any(|item| item.state.is_pending());
        let enabled = self.settings.remote_auto_refresh;
        let workspace = self.workspaces.active().id;
        // **보이는 워크스페이스의 패널만** 본다 (D6) — 감춰진 워크스페이스의 원격 탭까지
        // 서버에 물으면 화면에 없는 것 때문에 부담이 는다
        let Some(view) = self.views.get(&workspace) else {
            return;
        };
        let due: Vec<PanelId> = view
            .panels
            .iter()
            .filter(|(id, panel)| {
                should_auto_refresh(AutoRefresh {
                    enabled,
                    remote: panel.is_remote(),
                    transferring,
                    period,
                    now,
                    last_refresh: self.auto_refresh_at.get(*id).copied().unwrap_or(0.0),
                    last_input: panel.last_input_at(),
                })
            })
            .map(|(id, _)| *id)
            .collect();
        for id in due {
            let ExplorerApp { views, manager, .. } = self;
            let sent = views
                .get_mut(&workspace)
                .and_then(|view| view.panels.get_mut(&id))
                .and_then(|panel| panel.request_remote_list_quiet(workspace, id, manager))
                .is_some();
            // **보내지 못했으면 시각을 갱신하지 않는다** — 연결이 끊긴 탭은 다음 주기에 다시
            // 본다(자동 재연결은 하지 않는다 — 그것은 별개 기능이다)
            if sent {
                self.auto_refresh_at.insert(id, now);
            }
        }
    }

    /// 끊긴 네트워크 드라이브와 즐겨찾기 경로를 주기적으로 다시 확인한다 (FR-67).
    ///
    /// **기본 틱 위에서 돌고 설정과 무관하다** — 서버에 묻지 않는 로컬 일이라, 원격 자동
    /// 갱신을 꺼도 함께 멎으면 안 된다. 틱 자체는 `pump_auto_refresh`가 건다
    fn pump_rescan(&mut self, ctx: &egui::Context, now: f64) {
        if !should_rescan(Rescan {
            running: self.drive_scan.is_some(),
            period: f64::from(BASE_TICK_SECS),
            now,
            last: self.last_rescan_at,
        }) {
            return;
        }
        self.last_rescan_at = now;
        self.drive_scan = Some(crate::fs::drives::spawn_scan(
            ctx,
            self.favorites.watched_paths(),
        ));
    }

    fn pump_autosave(&mut self, ctx: &egui::Context, now: f64) {
        if self.autosave.due(now) {
            let snapshot = self.collect_session();
            if let Some(session) = self.autosave.observe(now, snapshot) {
                save_session(session);
            }
        }
        // 아직 적지 못한 변화가 있으면 화면을 깨워 둔다 — egui는 할 일이 없으면 프레임을
        // 그리지 않아, 그러지 않으면 마지막 변화가 다음 조작 때까지 파일에 닿지 못한다
        if self.autosave.pending() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(autosave::QUIET_SECS));
        }
    }
}

/// 설정과 무관하게 도는 **기본 틱**(초) — 원격 자동 갱신을 꺼도 이 간격으로 깨어난다.
///
/// 로컬 재확인(끊긴 네트워크 드라이브·즐겨찾기 실재)이 이 위에서 돌기 때문이다.
/// 원격 주기를 이보다 짧게 잡으면 그만큼 자주 깨어난다 — 그러지 않으면 설정한 주기가
/// 이 값의 해상도에 갇혀 10초로 낮춰도 30초마다만 갱신된다
pub const BASE_TICK_SECS: u32 = 30;

/// 자동 재조회 판정에 드는 값들 — 앱 상태를 참조하지 않아 그대로 시험할 수 있다 (FR-67)
struct AutoRefresh {
    /// 설정의 원격 자동 갱신이 켜져 있는가
    enabled: bool,
    /// 이 패널이 원격 탭을 보이고 있는가
    remote: bool,
    /// 전송이 도는 중인가 — 앱 전체에 하나의 값이다
    transferring: bool,
    /// 재조회 주기(초)
    period: f64,
    /// 지금 (egui가 주는 경과 초)
    now: f64,
    /// 이 패널을 마지막으로 자동 재조회한 시각
    last_refresh: f64,
    /// 이 패널에서 마지막으로 조작이 있었던 시각
    last_input: f64,
}

/// 이 패널을 지금 다시 조회해야 하는가 (FR-67).
///
/// 다섯을 차례로 본다 — 설정 · 원격 탭 여부 · 전송 중 · 주기 · 사용자 조작 직후.
/// **조작 직후의 기준을 주기의 절반으로 둔 이유**(D10): 고정 값으로 두면 주기를 10초로
/// 낮춘 사람에게는 그 유예가 주기만큼 길어 갱신이 거의 일어나지 않는다
fn should_auto_refresh(check: AutoRefresh) -> bool {
    if !check.enabled || !check.remote || check.transferring {
        return false;
    }
    if check.now - check.last_refresh < check.period {
        return false;
    }
    check.now - check.last_input >= check.period / 2.0
}

/// 로컬 재확인 판정에 드는 값들 — 위 `AutoRefresh`와 같은 이유로 묶는다.
/// 셋이 나란히 `f64`라 인자로 늘어놓으면 자리가 뒤바뀌어도 컴파일이 통과한다
struct Rescan {
    /// 앞선 확인이 아직 돌고 있는가
    running: bool,
    /// 재확인 주기(초)
    period: f64,
    /// 지금 (egui가 주는 경과 초)
    now: f64,
    /// 마지막으로 확인을 띄운 시각
    last: f64,
}

/// 로컬 재확인(끊긴 드라이브·즐겨찾기 실재)을 지금 다시 띄워야 하는가 (FR-67).
///
/// **앞선 확인이 아직 돌고 있으면 띄우지 않는다** — 끊긴 경로 하나의 판정이 첫 시도에
/// 2.8초까지 걸리고(`fs::drives::is_reachable` 실측) 즐겨찾기까지 더하면 한 번이 수십 초가
/// 될 수 있다. 주기마다 그냥 띄우면 워커가 겹쳐 쌓인다
fn should_rescan(check: Rescan) -> bool {
    !check.running && check.now - check.last >= check.period
}

/// 두 사각형이 겹치는 넓이 — 닫힌 자리를 누가 이어받았는지 고르는 데 쓴다
fn overlap_area(a: LayoutRect, b: LayoutRect) -> i64 {
    let w = (a.x + a.w).min(b.x + b.w) - a.x.max(b.x);
    let h = (a.y + a.h).min(b.y + b.h) - a.y.max(b.y);
    if w <= 0 || h <= 0 {
        return 0;
    }
    w as i64 * h as i64
}

impl eframe::App for ExplorerApp {
    /// 창 클리어 색 — eframe 기본값은 하드코딩된 회색이라 팔레트와 어긋난다.
    /// 이것을 덮어써야 크기 조절 중 노출되는 여백까지 창 배경색으로 칠해진다
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::WINDOW_BG.to_normalized_gamma_f32()
    }

    /// 종료 직전 — 지금 상태를 **실행 파일 옆의 `settings.json`**에 저장한다 (FR-11·NFR-7).
    /// 저장 실패(디스크 풀·권한)는 조용히 넘어간다 — 종료를 막을 이유가 없다
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // 진행 중이던 전송을 대기로 되돌린다 — 저장된 큐가 "전송 중"이라 주장하면
        // 다음 실행의 화면이 실제로는 아무것도 돌지 않는데 진행 중으로 보인다 (T18)
        self.runner.shutdown(&mut self.queue);
        save_session(&self.collect_session());
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.intercept_close(ctx);
        self.track_window(ctx);
        self.textures.begin_frame();
        // 연결 소식은 **워크스페이스와 무관하게** 받는다 — 채널에 쌓인 것을 건너뛰면
        // 보이지 않는 워크스페이스의 원격 탭이 옛 단계로 굳는다
        let now = ctx.input(|input| input.time);
        // 큐가 상태 전이에 적을 **벽시계** 시각을 밀어 넣는다 — 위 `now`는 프레임 상대
        // 시각이라 화면에 `14:03`을 적을 수 없다. 프레임마다 한 번이면 족하다: 큐 전이는
        // 이 프레임 안에서만 일어나고 표시 단위가 초라 프레임 내 오차는 보이지 않는다.
        // 파일시스템에 닿지 않아 UI 스레드에서 불러도 프레임이 멈추지 않는다
        self.queue.set_wall_now(system_time_now());
        // 설정의 재시도 횟수를 실행기에 민다 (FR-37) — 설정 화면이 값을 바꾸면 다음 프레임에
        // 반영된다. 매 프레임 넣는 것은 대입 하나라 값싸고, 설정이 바뀌는 자리를 일일이
        // 찾아 잇는 것보다 빠뜨릴 여지가 없다
        self.runner
            .set_retry_limit(crate::app::settings::transfer_retry_count(
                self.settings.transfer_retry_count,
            ));
        self.poll_remote(now);
        // 끝난 전송의 목적지 폴더를 다시 읽는다 (FR-37) — 표시는 `poll_remote`가 남긴다
        self.pump_relist(now);
        // 드라이브 줄·연결 상태·즐겨찾기 실재 (T4 · FR-67) — 워커가 나눠 올린다
        self.poll_drives();
        // 펼쳐진 로컬 폴더를 큐로 옮긴다 (FR-38)
        while let Ok((site, files, skipped)) = self.expand_rx.try_recv() {
            // 이 펼치기가 끝났다 — 상태 줄의 `펼치는 중`이 그만큼 줄어든다
            self.expanding = self.expanding.saturating_sub(1);
            // 그 사이 사이드바에서 지운 사이트면 버린다 (FR-29) — 넣으면 방금 비운 큐가
            // 되살아나 연결별 탭이 다시 선다. 읽지 못한 폴더 알림도 함께 뜻을 잃는다
            if self.detached_sites.contains(&site) {
                continue;
            }
            for (local, remote, size) in files {
                self.queue
                    .enqueue(site, TransferDirection::Upload, local, remote, size);
            }
            // 읽지 못한 폴더가 있으면 그 사실을 알린다 — 조용히 빼면 사용자는
            // 그 파일들이 왜 큐에 없는지 알 길이 없다 (plan Edge Case)
            if skipped > 0 {
                self.toast
                    .show(crate::i18n::dynamic::skipped_folders(skipped), now);
            }
        }
        self.drain_conflict_checks();
        // 셸 복사가 끝났으면 그 사실을 알린다 (FR-60)
        self.pump_local_copy(now);
        self.pump_file_op(now);
        // 폴더 여부를 다 잰 OS 드롭을 전송으로 보낸다 (FR-61)
        self.pump_os_drop_scan();
        // 자리가 나면 대기 중인 전송을 워커에 맡긴다 (FR-37)
        self.runner
            .start_ready(&mut self.queue, &self.manager, &self.sites, now);
        // 활성 뷰를 **여기서** 확보한다 — 그리기(`ui`)의 `ensure_active_view`에만 맡기면
        // 첫 프레임의 이 자리는 뷰가 없어 빈손으로 지나가고, 폴더 열거가 다음 프레임에야
        // 시작된다. 그 한 프레임 사이에 창이 이미 표시돼 **빈 목록**이 보인다(2026-08-14 실측)
        self.ensure_active_view();
        // 화면에 없는 워크스페이스는 폴링하지 않는다 — 전환하면 그때 밀린 결과가 반영된다
        let id = self.workspaces.active().id;
        if let Some(view) = self.views.get_mut(&id) {
            // 패널은 서로 독립이라 각자 자기 열거 결과만 처리한다
            for panel in view.panels.values_mut() {
                panel.poll(ctx, &mut self.icons, &mut self.dir_cache);
            }
        }
        self.sync_subtitle();
        self.pump_auto_refresh(ctx, now);
        self.pump_rescan(ctx, now);
        self.pump_autosave(ctx, now);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut menu = None;
        // 키 소유는 그리기 클로저 안에서 옮겨진다 — 그 안은 `self`가 통째로 빌려져 있어
        // 지역 값으로 들고 나와 클로저가 끝난 뒤에 돌려놓는다
        let mut key_owner = self.key_owner;
        // **접힌 사이드바는 키를 쥐지 못한다** (FR-12) — 접힌 프레임에는 사이드바를 그리는
        // 블록 자체가 돌지 않아 소유를 놓을 기회가 없고, 그대로 두면 파일 목록을 누르기
        // 전까지 `F2`·`Delete`가 아무 데도 가지 않는다(품질 리뷰 m1)
        if self.sidebar_collapsed {
            key_owner = menu::KeyOwner::FileList;
        }
        // 목록에서 확정된 이름 (FR-64) — 셸에 거는 것은 그리기가 끝난 뒤다
        let mut rename = None;
        let mut panel_command = None;
        let mut remote_action = None;
        let mut remote_url = None;
        let mut closed_conns = Vec::new();
        // 목록에 끌어다 놓은 것 (FR-38) — 큐에 넣는 것은 그리기가 끝난 뒤다
        let mut dropped = None;
        // 이번 프레임의 패널 자리 (FR-61) — OS 드롭이 놓인 패널을 고르는 데 쓴다
        let mut pane_rects: Vec<(PanelId, egui::Rect)> = Vec::new();
        // 원격 목록에서 고른 메뉴 항목 (FR-39)
        let mut remote_menu = None;
        // 원격 트리가 청한 하위 조회 (T24)
        let mut tree_requests = Vec::new();
        let mut favorite = None;
        // 사이트 관리자의 `연결(C)`이 쓸 분할 영역 — 모달은 CentralPanel 밖에서 그리므로
        // 안에서 정해지는 이 값을 밖으로 들고 나온다
        let mut layout_area = None;
        // 타이틀바를 먼저 그린다 — 남는 영역이 아래 CentralPanel의 몫이 된다 (FR-22)
        let titlebar_command = self.show_titlebar(ui, &ctx);
        // eframe이 주는 Ui는 여백·배경이 없다 — CentralPanel로 감싸야 바탕이 칠해진다.
        // **기본 여백은 끈다**(`Frame::NONE` + 바탕색): egui의 중앙 패널은 사방에 여백을 두는데,
        // 그만큼 패널 줄이 타이틀바 선에서 떨어져 뜬다(사용자 보고). 이 앱의 화면은 창
        // 가장자리까지 꽉 차는 탐색기 배치다 — 여백이 필요한 곳은 각 패널이 스스로 둔다
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(theme::WINDOW_BG))
            .show(ui, |ui| {
                if !self.korean_font {
                    ui.colored_label(theme::TEXT_MUTED, crate::i18n::app_font_fallback());
                }
                if !self.shell_available() {
                    ui.colored_label(theme::TEXT_MUTED, crate::i18n::app_shell_menu_unavailable());
                }
                // 하단 상태 표시줄·도크를 사이드바보다 **먼저** 뗀다 — egui 패널은 먼저 그린 쪽이
                // 넓은 자리를 가져가므로, 순서를 뒤집으면 둘 다 사이드바를 뺀 폭에만 그려진다.
                // 창 폭 전체를 가로지르는 것이 디자인이다 (FR-36·FR-40)
                self.show_status_bar(ui);
                self.show_dock(ui);

                let mut sidebar_actions = Vec::new();
                if !self.sidebar_collapsed {
                    // 연결 상태를 먼저 모은다 — 아래 클로저가 `self`를 통째로 빌린다
                    let connected = self.connected_sites();
                    // 사이드바가 자기 배경·여백을 직접 그리므로 egui 기본 프레임은 끈다
                    let panel = egui::Panel::left(egui::Id::new("workspace_sidebar"))
                        .resizable(true)
                        .default_size(self.sidebar_width)
                        .size_range(egui::Rangef::new(
                            SIDEBAR_MIN_WIDTH as f32,
                            SIDEBAR_MAX_WIDTH as f32,
                        ))
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| {
                            // 이 프레임에 F2를 받을 자격을 먼저 알린다 (FR-12) —
                            // 인자로 넘기지 않는 이유는 `set_owns_keys` 주석에 있다
                            self.sidebar
                                .set_owns_keys(key_owner == menu::KeyOwner::Sidebar);
                            self.sidebar.show(
                                ui,
                                &self.workspaces,
                                &self.sites,
                                &connected,
                                &mut self.icons,
                                &mut self.textures,
                            )
                        });
                    self.sidebar_width = panel.response.rect.width();
                    // 사이드바를 누른 프레임에는 무수식 키가 그쪽으로 간다 (FR-12).
                    // 패널 쪽 전이는 아래 `show_layout`이 돌려주는 `pressed_panel`이 맡는다 —
                    // 그 판정을 여기서 다시 하지 않는 이유는 분할 영역을 아직 모르기 때문이다
                    let pressed_sidebar = ctx
                        .input(|input| {
                            input
                                .pointer
                                .any_pressed()
                                .then(|| input.pointer.interact_pos())
                        })
                        .flatten()
                        .is_some_and(|pos| panel.response.rect.contains(pos));
                    key_owner = menu::next_key_owner(
                        key_owner,
                        pressed_sidebar.then_some(menu::KeyOwner::Sidebar),
                    );
                    // 조작은 모아 두었다가 아래에서 처리한다 — 연결은 분할 영역을 알아야 하는데
                    // 그 영역은 **사이드바를 뺀 나머지**라 여기서는 아직 정해지지 않았다
                    sidebar_actions = panel.inner;
                }

                let area = splitter::to_layout_rect(ui.available_rect_before_wrap());
                layout_area = Some(area);
                let now = ui.input(|input| input.time);
                for action in sidebar_actions {
                    self.handle_sidebar(action, area, now, &ctx);
                }
                // 단축키는 프레임당 한 번만 소비한다(`consume_shortcut`이 입력을 소모한다).
                // 메뉴와 단축키가 같은 프레임에 겹쳐도 둘 다 실행한다
                // 모달이 떠 있는 동안에는 단축키를 받지 않는다 — 모달은 입력을 막는다는 뜻이다
                // (워크스페이스 삭제 확인 · 서버 지문 확인)
                // **창이 본 `Ctrl+V`는 조건과 무관하게 프레임마다 거둔다** — 거두지 않으면
                // 모달이 떠 있던 동안 눌린 것이 표시로 남아 모달을 닫는 순간 뒤늦게 나간다
                let paste_pressed = crate::ui::shell_host::take_paste_pressed();
                // 마우스 옆 버튼도 같은 이유로 프레임마다 거둔다 — 모달이 떠 있던 동안
                // 눌린 것이 표시로 남아 뒤늦게 나가면 안 된다
                let app_nav = crate::ui::shell_host::take_app_nav();
                let shortcut_command = if self.pending_remove.is_some()
                    || self.pending_site_remove.is_some()
                    || self.hostkey.is_open()
                    || self.site_manager.is_open()
                {
                    None
                } else {
                    // 클립보드 키는 egui가 키 이벤트로 주지 않아 따로 받는다
                    // (`poll_clipboard_keys` 주석 — 2026-08-22 사용자 보고)
                    // 마우스 옆 버튼(뒤로·앞으로)도 같은 자리에서 받는다 — 모달이 떠 있으면
                    // 키와 마찬가지로 듣지 않는다
                    menu::poll_shortcuts(&ctx, self.key_owner)
                        .or_else(|| menu::poll_clipboard_keys(&ctx, self.key_owner, paste_pressed))
                        .or_else(|| menu::poll_mouse_nav(&ctx, app_nav))
                };
                // 단축키·타이틀바 명령은 대상을 지정하지 않는다 — 활성 패널에 적용된다
                for command in shortcut_command.into_iter().chain(titlebar_command) {
                    self.apply_command(command, None, area, &ctx);
                }
                // 확보와 사용을 나눈다 — 아래 호출이 `views`와 `icons`·`textures`를 동시에 빌린다
                let id = self.workspaces.active().id;
                // 탭 배지·드롭다운이 상태 점을 그리는 데 쓴다 — 빌림이 겹치지 않게 미리 모은다
                let connected = self.connected_sites();
                // `logic`에서 이미 확보했지만 여기서 한 번 더 확인한다 — 사이드바 조작이
                // 이 프레임 안에서 활성 워크스페이스를 바꿀 수 있고, 그 워크스페이스는
                // 아직 뷰가 없을 수 있다
                self.ensure_active_view();
                if let Some(view) = self.views.get_mut(&id) {
                    // 전송 대상은 **그리기 전에** 정한다 — 아이콘이 가리키는 곳과 실제로 가는
                    // 곳이 같은 값에서 나와야 한다 (FR-54)
                    let targets = view.transfer_targets();
                    let outcome = splitter::show_layout(
                        ui,
                        &ctx,
                        &mut view.layout,
                        &mut view.panels,
                        &mut view.active,
                        &mut self.icons,
                        &mut self.textures,
                        RemoteView {
                            sites: &self.sites,
                            connected: &connected,
                            tree: &self.tree_cache,
                        },
                        DisplayRules {
                            show_extensions: self.settings.show_extensions,
                            show_hidden: self.settings.show_hidden,
                            show_system: self.settings.show_system,
                        },
                        targets,
                        &self.favorites.entries(),
                        self.drives.rows(),
                    );
                    // 이번 프레임에 눌린 패널이 다음 전송의 대상이 된다 — 팝업에 가린 클릭은
                    // 여기 오지 않는다(`pressed_panel` 설명). 메뉴 실행보다 **앞서** 반영해야
                    // 방금 우클릭한 패널이 곧 올리기 대상이 된다
                    if let Some(pressed) = outcome.pressed_panel {
                        view.note_pressed(pressed);
                    }
                    // 패널을 누르면 무수식 키가 파일 목록으로 돌아온다 (FR-12)
                    key_owner = menu::next_key_owner(
                        key_owner,
                        outcome.pressed_panel.map(|_| menu::KeyOwner::FileList),
                    );
                    menu = outcome.menu;
                    rename = outcome.rename;
                    panel_command = outcome.command;
                    remote_action = outcome.remote;
                    remote_url = outcome.remote_url;
                    closed_conns = outcome.closed_conns;
                    dropped = outcome.drop;
                    pane_rects = outcome.pane_rects;
                    remote_menu = outcome.remote_menu;
                    tree_requests = outcome.tree_requests;
                    favorite = outcome.favorite;
                    // 열어 본 드라이브의 연결 상태를 드라이브 목록에 반영한다 (T6).
                    //
                    // **이 홉은 컴파일러도 시험도 미치지 않는다** — 필드별 대입이라 빠뜨려도
                    // 빌드가 통과하고, `ExplorerApp`은 실 HWND(`CreationContext`)가 있어야
                    // 만들어져 프레임을 돌리는 시험을 세울 수 없다(AGENTS: UI 로직 비대상).
                    // 이 `for`문을 지워도 붉어지는 시험이 없으니 **리뷰가 지키는 자리**다.
                    // 앞뒤 층은 시험이 덮는다 — 패널이 관측을 세우는 것(`ui/panel/tests.rs`),
                    // 여럿을 모으는 것(`ui/splitter.rs`), 반영 규칙(`app/drives.rs`)
                    for (path, reachable) in outcome.drive_observed {
                        self.drives.observe(&path, reachable);
                    }

                    // OS에서 끌어온 파일이 창 위를 지나는 동안 놓일 패널을 두른다 (FR-61).
                    // **`show_layout`이 돌아온 뒤에 그린다** — 강조할 패널을 정하려면 그
                    // 함수가 반환한 `pane_rects`가 필요해, 인자로 되먹이면 순환이 된다
                    let hovering = ctx.input(|input| !input.raw.hovered_files.is_empty());
                    // 끌고 있지 않으면 커서를 읽지 않는다 — 이 자리는 매 프레임 도는데
                    // `GetCursorPos`·`ScreenToClient`는 끌고 있을 때만 쓸 값을 만든다
                    let cursor = hovering
                        .then(|| {
                            self.shell
                                .as_ref()
                                .and_then(|shell| shell.cursor_client_pos(ctx.pixels_per_point()))
                        })
                        .flatten();
                    if let Some(target) = drop_highlight(hovering, cursor, &pane_rects)
                        && let Some((_, rect)) = pane_rects.iter().find(|(id, _)| *id == target)
                    {
                        splitter::draw_drop_highlight(ui, *rect);
                    }
                }
                // 패널 메뉴 명령은 그리기가 끝난 뒤에 실행한다 — 분할·닫기는 트리를 바꾸므로
                // 이번 프레임의 배치와 어긋나고, `apply_command`가 앱 전체를 빌려야 한다.
                // 대상은 메뉴를 연 패널이지 활성 패널이 아니다 (D16)
                if let Some((target, command)) = panel_command {
                    self.apply_command(command, Some(target), area, &ctx);
                }
                if let Some((target, action)) = remote_action {
                    self.apply_remote_action(target, action);
                }
                if let Some((target, url)) = remote_url {
                    self.open_remote_url(target, url, area);
                }
                // 끌어다 놓은 것을 처리한다 — 어느 패널에 놓였는지는 쓰지 않는다(항목의
                // 종류와 놓은 자리의 종류만으로 무엇을 할지가 정해진다).
                //
                // **로컬끼리는 전송이 아니라 셸 복사다** (FR-60) — `start_transfer`보다
                // 먼저 갈라야 한다. 그쪽은 전송 큐에 넣는 앞문이라 복사할 것이 흘러들면
                // 옮기지도 못한 채 큐에 쌓인다
                if let Some((_, drop)) = dropped.take() {
                    match list_common::local_copy_target(&drop) {
                        Some(dir) => self.start_local_copy(dir.to_path_buf(), &drop),
                        None => self.start_transfer(drop),
                    }
                }
                // OS(탐색기·바탕화면)에서 끌어온 것 (FR-61) — 앱 안의 드래그와 통로가
                // 다르다. 그리기가 끝난 뒤라야 이번 프레임의 패널 자리를 쓸 수 있다
                self.pump_os_drop(&ctx, &pane_rects);
                // 원격 메뉴가 고른 것 — 대화가 필요한 것은 여기서 열리기만 한다
                if let Some((target, (action, targets))) = remote_menu.take() {
                    self.apply_remote_menu(target, action, targets);
                }
                // 트리 메뉴가 고른 즐겨찾기 조작 (FR-56) — 어느 패널에서 골랐는지는 쓰지
                // 않는다(목록이 앱에 하나뿐이라 모든 패널이 같은 것을 본다).
                // 무엇이 늘고 주는지의 규칙은 `FavoriteStore::apply`에 있다
                if let Some((_, action)) = favorite.take() {
                    self.favorites.apply(action);
                }
                // 원격 위치가 바뀐 패널은 목록을 다시 읽는다 — 트리 선택(T24 Acceptance ⑤)과
                // 상위 이동이 이 길을 함께 쓴다
                self.list_moved_panels();
                // 트리가 펼쳐진 폴더의 하위를 청한다 (T24)
                for (_, request) in tree_requests.drain(..) {
                    if let TreeRequest::Remote { conn, path } = request {
                        self.request_tree_children(conn, path);
                    }
                }
                // 마지막 원격 탭이 닫힌 연결을 접는다 — 워커와 소켓이 여기서 회수된다 (FR-32)
                for conn in closed_conns {
                    self.release_conn(conn);
                }
            });

        // 삭제 확인은 egui 모달이라 `CentralPanel` 밖에서 그려도 된다(자체 레이어를 쓴다)
        self.show_remove_confirm(&ctx);
        // 사이드바에서 사이트를 지우기 전의 확인도 같다 (FR-29) — 배치를 알아야
        // 패널을 닫을 수 있어 그 영역을 함께 넘긴다
        self.show_site_remove_confirm(&ctx, layout_area);
        // 서버 지문 확인도 같다. 사용자가 고를 때까지 그 연결의 워커는 기다리고 있다 (D15)
        self.hostkey.show(&ctx);
        // 사이트 관리자도 자체 레이어를 쓴다 (FR-27)
        self.show_site_manager(&ctx, layout_area);
        self.sync_tray(ctx.input(|input| input.time));
        self.poll_tray(&ctx);
        self.hide_on_start(&ctx);
        self.show_settings_dialog(&ctx);
        // 오픈소스 라이선스 대화 (FR-57) — 상태를 저장하지 않아 배선이 한 줄이다
        self.license_dialog.show(&ctx);
        // 정보 대화 (FR-58) — 마찬가지로 상태를 저장하지 않는다
        self.about_dialog.show(&ctx);
        // 원격 파일 작업 대화 (FR-39)
        self.show_remote_dialogs(&ctx);
        // 같은 이름 확인 대화 (FR-55) — 이것이 닫히기 전에는 그 전송이 큐에 들어가지 않는다
        self.show_conflict_dialog(&ctx);
        // 업데이트 워커가 보내온 것을 거둔다 (FR-62) — 상태가 바뀌면 배지·알림이 따라온다
        self.pump_update(&ctx);
        // 전송 중 업데이트 확인 대화 (D5)
        self.show_update_confirm(&ctx);
        // 알림은 모든 것 위에 뜬다 — 대화가 닫힌 뒤에도 남아 있어야 한다 (FR-43)
        self.toast.show_ui(&ctx);
        // 로그 복사는 그리기가 끝난 뒤에 보낸다 (`⧉` — FR-40)
        if let Some(text) = self.pending_clipboard.take() {
            ctx.copy_text(text);
        }

        // 우클릭 요청이 왔으면 **이 프레임 안에서** 메뉴를 연다 — `ShellMenu::open`은
        // 메시지 루프를 돌리지 않아(`TrackPopupMenuEx`와 다르다) 그리기 도중에 불러도 된다
        self.key_owner = key_owner;
        if let Some(request) = rename {
            self.start_rename(request);
        }
        if let Some(request) = menu {
            self.open_shell_menu(&ctx, request);
        }
        // 열려 있으면 그린다 — 모든 패널 위에 떠야 해서 그리기 맨 끝이다
        self.show_shell_menu(&ctx);

        // **종전 표준 메뉴만** 그리기가 모두 끝난 뒤에 띄운다 — `TrackPopupMenuEx`가 자체
        // 메시지 루프를 돌려 이벤트 루프를 재진입시키므로, 위젯 트리가 절반만 구성된 상태로
        // 들어가면 안 된다
        // **우리가 그리는 메뉴는 세지 않는다** — 종전에는 우클릭 요청 자체를 셌지만
        // (`TrackPopupMenuEx`가 그 프레임에서 바로 모달로 들어갔다), 지금 그 자리에서 뜨는
        // 것은 egui 메뉴라 메시지 루프를 돌리지 않는다. 미뤄야 하는 것은 **자체 메시지 루프를
        // 도는 것끼리**이고 그것은 아래 표준 메뉴 하나뿐이다
        // **띄울 차례인가는 `take_ready`가 정한다** — 아직이면 프레임을 하나 줄이고
        // `None`을 준다(그동안 우리 메뉴가 없는 화면이 표시된다).
        //
        // 아래 두 pump를 미루는 판정은 **띄우는 프레임에만** 참이어야 한다 — 기다리는
        // 동안에도 참으로 두면 그 프레임들의 파일 대화·드래그가 까닭 없이 밀린다
        let ready = shell_menu::take_ready(&mut self.pending_show_more);
        let shell_menu_pending = ready.is_some();
        if self.pending_show_more.is_some() {
            // 기다리는 중이다 — 입력이 없어도 다음 프레임을 그려야 그 화면이 표시된다
            ctx.request_repaint();
        }
        if let (Some(pending), Some(shell)) = (ready, self.shell.as_ref()) {
            // egui 좌표는 논리 포인트라 물리 픽셀로 되돌린 뒤 화면 좌표로 바꾼다
            let scale = ctx.pixels_per_point();
            let (x, y) = shell.to_screen(
                (pending.pos.x * scale) as i32,
                (pending.pos.y * scale) as i32,
            );
            shell.popup(&pending.folder, &pending.items, x, y);
        }
        // 사이트 목록 파일 대화도 같은 제약이다 (FR-59). **셸 메뉴가 뜬 프레임에는 미룬다** —
        // 두 모달을 겹쳐 띄우면 어느 쪽이 답을 기다리는지 알 수 없다. 요청은 그대로 남아
        // 다음 프레임에 뜬다
        if !shell_menu_pending {
            self.pump_site_file_dialog(&ctx);
        }
        // 창 밖으로 끌고 나간 로컬 항목을 OS 드래그로 넘긴다 (FR-61 내보내기).
        // **셸 메뉴가 뜬 프레임에는 미룬다** — `DoDragDrop`도 자체 메시지 루프를 돌려
        // 겹치면 어느 쪽이 답을 기다리는지 알 수 없다. 바로 위 파일 대화와는 같은 프레임에
        // 이어 불릴 수 있지만, 그쪽이 답을 받고 돌아온 뒤라 실제로 겹치지는 않는다
        if !shell_menu_pending {
            self.pump_export_drag(&ctx);
        }
    }
}

/// OS에서 끌어온 것이 놓인 자리 (FR-61) — 탭의 종류가 처리를 가른다
enum OsDropTarget {
    Local(std::path::PathBuf),
    Remote {
        site: crate::remote::types::SiteId,
        dir: crate::remote::types::RemotePath,
    },
}

/// OS 드래그가 창 위를 지나는 동안 강조할 패널 — 없으면 `None` (FR-61).
///
/// **대상은 OS 드래그뿐이다** — 앱 안의 탭↔탭 드래그는 끌고 있는 첫 항목의 그림이 커서를
/// 따라오고(`ui::drag_preview` — FR-38) 놓일 자리는 그 커서 아래 목록이 그대로 보이므로,
/// 테두리까지 더하면 표시가 둘이 된다. OS 드래그에는 그 그림도 목록 반응도 없다.
///
/// **2026-08-21 정정**: 종전 주석은 egui가 끄는 항목의 그림을 알아서 그려 준다는 전제로
/// 이 한정을 설명했으나 사실이 아니다 — egui는 페이로드를 세워도 커서 아이콘만 `Grabbing`으로
/// 바꾼다(`egui::DragAndDrop`의 `on_end_pass`). 그림은 이제 앱이 직접 그린다.
///
/// 판정을 그리기와 떼어 둔 이유는 이것만 시험할 수 있게 하기 위함이다 — 그리는 쪽은
/// 실제 `Ui`가 있어야 돈다
fn drop_highlight(
    hovering: bool,
    cursor: Option<egui::Pos2>,
    pane_rects: &[(PanelId, egui::Rect)],
) -> Option<PanelId> {
    if !hovering {
        return None;
    }
    panel_at(pane_rects, cursor?)
}

/// 그 자리에 있는 패널 — 어느 사각형에도 들지 않으면 `None` (FR-61).
///
/// **뒤에서부터 찾는다** — `pane_rects`는 그리기 순서대로라 겹치면 나중에 그린 것이 위다.
/// 겹치는 일은 드물지만(분할 트리는 자리를 나눈다) 경계에서 두 사각형이 한 점을 함께
/// 담을 수 있어, 그때 위에 보이는 쪽을 고른다
fn panel_at(pane_rects: &[(PanelId, egui::Rect)], pos: egui::Pos2) -> Option<PanelId> {
    pane_rects
        .iter()
        .rev()
        .find(|(_, rect)| rect.contains(pos))
        .map(|(id, _)| *id)
}

/// 파일 작업 실패 사유가 상태 줄에 머무는 시간(초) — 알림(FR-43)보다 조금 길게 둔다
const NOTICE_SECS: f64 = 6.0;

/// 지금 벽시계 시각을 FILETIME으로 읽는다 — 큐가 전송 시각을 적는 데 쓴다.
///
/// **환산은 `panel::file_list::unix_seconds_to_filetime`에 맡긴다** — 기점 차이(1601 ↔ 1970)
/// 상수를 여기 또 적으면 한쪽만 틀렸을 때 아무도 알아채지 못한다. 그 함수는 두 목록이 같은
/// 자로 정렬하려고 이미 두고 있던 것이다.
///
/// **초 단위면 충분하다** — 큐의 시간 열이 초까지만 적는다(`queue_panel::format_time_range`).
/// 시계를 읽는 것은 파일시스템에 닿지 않아 UI 스레드에서 불러도 프레임이 멈추지 않는다
fn system_time_now() -> u64 {
    let Ok(since_epoch) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        // 시스템 시계가 1970 이전을 가리킨다 — 「모른다」로 두고 화면은 `—`를 보인다
        return 0;
    };
    crate::panel::file_list::unix_seconds_to_filetime(since_epoch.as_secs() as i64)
}

/// 로컬 폴더를 펼친 결과 — 올릴 파일들과 **읽지 못해 건너뛴 폴더 수**.
///
/// 이름을 `transfer::Expanded`와 달리 두는 이유: 그쪽은 뿌리 기준 상대 경로를 들고
/// 이쪽은 사이트와 서버 경로까지 든다 — 같은 이름이면 오가며 읽을 때 헷갈린다
///
/// 건너뛴 것을 함께 나르는 이유: 권한 없는 폴더 하나 때문에 나머지를 버리지는 않지만,
/// 조용히 빼면 사용자는 그 파일들이 왜 큐에 없는지 알 길이 없다 (plan Edge Case)
type ExpandResult = (SiteId, Vec<(PathBuf, RemotePath, u64)>, usize);

/// 시작 폴더 — 인자로 폴더를 받으면 그곳에서, 없으면 홈 폴더에서 시작한다
/// (탐색기의 "여기서 열기"처럼 쓰이며, 대량 폴더 성능 측정에도 이 경로를 쓴다).
///
/// 패널이 **로컬 자리를 잃었을 때의 폴백**이기도 하다 — 원격 탭에서 새 탭을 열 때와
/// 원격 탭만 보고 있는 패널을 나눌 때 갈 곳이 여기다
pub(crate) fn start_dir() -> PathBuf {
    let from_arg = std::env::args().nth(1).filter(|a| !a.starts_with("--"));
    if let Some(arg) = from_arg {
        let path = PathBuf::from(arg);
        if path.is_dir() {
            return path;
        }
    }
    std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"C:\"))
}

#[cfg(test)]
mod tests {
    /// 행 메뉴의 `다시 시도`가 **`전체 다시 시도`와 같은 길**을 쓰는가 (`queue.retry`).
    ///
    /// 종전에는 `queue.update(id, Wait)`를 불러 자동 재시도 횟수가 0으로 돌아가지 않았다 —
    /// README·PRD가 「손으로 누른 `다시 시도`는 그 횟수를 0으로 되돌린다」고 적은 것과
    /// 어긋나 있었다. 소스를 훑어 재는 이유는 `apply_queue_action`이 앱 상태 전체를 들고
    /// 있어 시험이 그것을 세울 수 없기 때문이다(이 레포의 규약 강제 시험과 같은 방식)
    /// 드라이브 감시 워커가 보낸 목록이 **`refresh`로** 반영되는가 (2026-09-09).
    ///
    /// `replace`를 쓰면 그 순간 끊긴 네트워크 드라이브의 X 배지가 사라졌다가 다음 주기
    /// 확인(30초)에야 돌아온다 — 사용자에게는 USB를 꽂을 때마다 배지가 깜빡이는 것으로 보인다.
    /// 소스를 훑어 재는 이유는 `poll_drives`가 앱 상태 전체를 들고 있어 시험이 그것을 세울 수
    /// 없기 때문이다(위 `행_메뉴_다시_시도는_전체와_같은_길을_쓴다`와 같은 방식)
    #[test]
    fn 드라이브_감시_결과는_배지를_지키는_길로_간다() {
        let source = include_str!("app.rs");
        let start = source
            .find("self.drive_watch.try_recv()")
            .expect("감시 채널을 읽는 자리를 찾지 못했다");
        // **고정 길이로 자르지 않는다** — 그 갈래가 몇 바이트만 늘어도 경계가 한글 문자
        // 안으로 들어가 무관한 수정이 이 시험을 깨뜨린다. 다음 갈래 머리까지로 자른다
        let rest = &source[start..];
        let end = rest
            .find("let Some(rx) = &self.drive_scan")
            .expect("다음 갈래를 찾지 못했다");
        let arm = &rest[..end];
        assert!(
            arm.contains("self.drives.refresh("),
            "감시 결과가 `refresh`를 거치지 않는다 — 꽂을 때마다 끊김 배지가 깜빡인다"
        );
    }

    #[test]
    fn 행_메뉴_다시_시도는_전체와_같은_길을_쓴다() {
        let source = include_str!("app.rs");
        let start = source
            .find("QueueAction::Retry(ids)")
            .expect("행 메뉴 다시 시도 갈래를 찾지 못했다");
        // **고정 길이로 자르지 않는다** — 그 갈래가 몇 바이트만 늘어도 경계가 한글 문자
        // 안으로 들어가 무관한 수정이 이 시험을 깨뜨린다. 다음 갈래 머리까지로 자른다
        let rest = &source[start..];
        let end = rest
            .find("QueueAction::RetryAll")
            .expect("다음 갈래를 찾지 못했다");
        let arm = &rest[..end];
        assert!(
            arm.contains("queue.retry(&ids)"),
            "행 메뉴 `다시 시도`가 `retry`를 거치지 않는다 — 재시도 횟수가 0으로 돌아가지 않는다"
        );
    }

    use super::*;

    /// 업로드 하위 메뉴 한 줄의 문구 (2026-08-28 사용자 결정).
    ///
    /// **경로 전체가 아니라 마지막 폴더 이름만** 적는다 — 깊은 곳에서 줄이 길어지면
    /// 어느 서버인지가 묻힌다
    #[test]
    fn 연결되지_않은_원격_탭은_업로드_대상이_아니다() {
        // 연결이 없으면 파일을 보낼 수 없다 — `upload_dir`의 기존 기준과 같다 (FR-54).
        // 사이트가 사라진 탭도 뺀다(보낼 곳을 모른다)
        use crate::panel::tabs::TabPhase;
        use crate::remote::connection::ConnectionId;
        use crate::remote::types::RemotePath;

        let mut sites = SiteStore::new();
        let site = sites.add("웹서버");
        let 연결 = ConnectionId(1);

        // ① 연결된 원격 탭 — 대상이다
        let 이어짐 = TabSource::Remote {
            site,
            conn: Some(연결),
            path: RemotePath::new("/pub/htdocs"),
            phase: TabPhase::Ok,
        };
        let target = upload_target_of(&이어짐, &sites).expect("연결된 탭은 대상이다");
        assert_eq!(target.site, site);
        assert_eq!(target.dir, RemotePath::new("/pub/htdocs"));
        assert_eq!(target.label, "웹서버 › htdocs");

        // ② 연결이 없는 원격 탭 — 빠진다
        let 끊김 = TabSource::Remote {
            site,
            conn: None,
            path: RemotePath::new("/pub"),
            phase: TabPhase::Ok,
        };
        assert_eq!(
            upload_target_of(&끊김, &sites),
            None,
            "끊긴 탭이 대상에 들었다"
        );

        // ③ 로컬 탭 — 빠진다
        let 로컬 = TabSource::Local(PathBuf::from(r"C:\"));
        assert_eq!(upload_target_of(&로컬, &sites), None);

        // ④ 사이트가 사라진 탭 — 빠진다(보낼 곳을 모른다)
        let 없는_사이트 = TabSource::Remote {
            site: SiteId(9999),
            conn: Some(연결),
            path: RemotePath::new("/pub"),
            phase: TabPhase::Ok,
        };
        assert_eq!(
            upload_target_of(&없는_사이트, &sites),
            None,
            "사라진 사이트의 탭이 대상에 들었다"
        );
    }

    #[test]
    fn 업로드_메뉴_라벨은_사이트와_마지막_폴더만_적는다() {
        use crate::remote::types::RemotePath;

        // 루트를 보고 있으면 사이트 이름만
        assert_eq!(upload_menu_label("웹서버", &RemotePath::root()), "웹서버");
        assert_eq!(upload_menu_label("웹서버", &RemotePath::new("/")), "웹서버");

        // 하위 폴더면 **마지막 이름만** — 경로를 다 적지 않는다
        assert_eq!(
            upload_menu_label("웹서버", &RemotePath::new("/pub/htdocs")),
            "웹서버 › htdocs"
        );
        assert_eq!(
            upload_menu_label("웹서버", &RemotePath::new("/a/b/c/깊은 폴더")),
            "웹서버 › 깊은 폴더",
            "경로 전체가 적혔다"
        );

        // 이름에 공백·비ASCII가 있어도 그대로다
        assert_eq!(
            upload_menu_label("사내 FTP", &RemotePath::new("/자료실")),
            "사내 FTP › 자료실"
        );
    }
    use std::path::Path;

    /// 다시 띄워야 하는 정상 상태 — 각 시험이 한 값씩 어긋뜨려 본다
    fn 재확인() -> Rescan {
        Rescan {
            running: false,
            period: 30.0,
            now: 100.0,
            last: 60.0,
        }
    }

    #[test]
    fn 주기가_지나면_로컬_재확인을_다시_띄운다() {
        assert!(should_rescan(재확인()));
    }

    #[test]
    fn 앞선_확인이_아직_돌면_새로_띄우지_않는다() {
        // FR-67 acceptance ⓔ — 끊긴 경로 하나가 수십 초를 먹을 수 있어, 주기마다 그냥
        // 띄우면 워커가 겹쳐 쌓인다
        assert!(!should_rescan(Rescan {
            running: true,
            ..재확인()
        }));
    }

    #[test]
    fn 주기가_지나지_않으면_로컬_재확인을_띄우지_않는다() {
        assert!(!should_rescan(Rescan {
            last: 80.0,
            ..재확인()
        }));
        // 경계는 포함이다 — 딱 주기가 지난 프레임에서 미루면 다음 프레임까지 밀린다
        assert!(should_rescan(Rescan {
            now: 90.0,
            ..재확인()
        }));
    }

    /// 재조회해야 하는 정상 상태 — 각 시험이 한 값씩 어긋뜨려 본다
    fn 재조회() -> AutoRefresh {
        AutoRefresh {
            enabled: true,
            remote: true,
            transferring: false,
            period: 30.0,
            now: 100.0,
            last_refresh: 60.0,
            last_input: 60.0,
        }
    }

    #[test]
    fn 주기가_지난_보이는_원격_탭은_다시_조회한다() {
        assert!(should_auto_refresh(재조회()));
    }

    #[test]
    fn 설정을_끄면_재조회하지_않는다() {
        // FR-67 acceptance ⓕ — 끄면 원격 재조회만 멈춘다(기본 틱은 계속 돈다)
        assert!(!should_auto_refresh(AutoRefresh {
            enabled: false,
            ..재조회()
        }));
    }

    #[test]
    fn 원격_탭이_아니면_재조회하지_않는다() {
        // FR-67 acceptance ⓖ — 로컬 목록은 변경 감시(FR-10)가 따로 본다
        assert!(!should_auto_refresh(AutoRefresh {
            remote: false,
            ..재조회()
        }));
    }

    #[test]
    fn 전송이_도는_중에는_재조회하지_않는다() {
        // FR-67 acceptance ⓓ — 같은 서버를 조회로 더 붙잡지 않는다
        assert!(!should_auto_refresh(AutoRefresh {
            transferring: true,
            ..재조회()
        }));
    }

    #[test]
    fn 주기가_지나지_않았으면_재조회하지_않는다() {
        assert!(!should_auto_refresh(AutoRefresh {
            last_refresh: 80.0,
            ..재조회()
        }));
    }

    #[test]
    fn 사용자_조작_직후에는_건너뛴다() {
        // FR-67 acceptance ⓔ (D10) — 주기의 절반 이내에 조작이 있었으면 미룬다.
        // 그 절반이 지나면 다시 조회한다
        assert!(!should_auto_refresh(AutoRefresh {
            last_input: 90.0,
            ..재조회()
        }));
        assert!(should_auto_refresh(AutoRefresh {
            last_input: 85.0,
            ..재조회()
        }));
    }

    #[test]
    fn 놓은_자리에_있는_패널을_고른다() {
        // Acceptance ⓐⓑ (FR-61)
        let rects = vec![
            (
                PanelId(1),
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 100.0)),
            ),
            (
                PanelId(2),
                egui::Rect::from_min_size(egui::pos2(100.0, 0.0), egui::vec2(100.0, 100.0)),
            ),
        ];
        assert_eq!(panel_at(&rects, egui::pos2(50.0, 50.0)), Some(PanelId(1)));
        assert_eq!(panel_at(&rects, egui::pos2(150.0, 50.0)), Some(PanelId(2)));
        // 사이드바·전송 큐 위에 놓은 경우 — 어느 패널도 아니다
        assert_eq!(panel_at(&rects, egui::pos2(50.0, 500.0)), None);
        assert_eq!(panel_at(&[], egui::pos2(50.0, 50.0)), None);
    }

    #[test]
    fn 끌고_있을_때만_그_패널을_강조한다() {
        // Acceptance ⓐ~ⓓ (FR-61)
        let rects = vec![(
            PanelId(1),
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 100.0)),
        )];
        let 안 = Some(egui::pos2(50.0, 50.0));

        // ⓐ 끌고 있지 않으면 강조하지 않는다 — 앱 안의 드래그는 대상이 아니다
        assert_eq!(drop_highlight(false, 안, &rects), None);
        // ⓑ 끌고 있고 커서가 패널 안이면 그 패널
        assert_eq!(drop_highlight(true, 안, &rects), Some(PanelId(1)));
        // ⓒ 끌고 있어도 패널 밖이면 강조하지 않는다 (사이드바·전송 큐 위)
        assert_eq!(
            drop_highlight(true, Some(egui::pos2(50.0, 500.0)), &rects),
            None
        );
        // ⓓ 커서를 읽지 못하면 강조하지 않는다
        assert_eq!(drop_highlight(true, None, &rects), None);
    }

    #[test]
    fn 사각형이_겹치면_나중에_그린_것이_이긴다() {
        // Acceptance ⓒ — `pane_rects`는 그리기 순서라 뒤엣것이 위에 보인다
        let 겹침 = vec![
            (
                PanelId(1),
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 100.0)),
            ),
            (
                PanelId(2),
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 100.0)),
            ),
        ];
        assert_eq!(panel_at(&겹침, egui::pos2(50.0, 50.0)), Some(PanelId(2)));
    }

    fn rect(x: i32, y: i32, w: i32, h: i32) -> LayoutRect {
        LayoutRect { x, y, w, h }
    }

    #[test]
    fn 닫기는_전달받은_패널을_대상으로_한다() {
        // 패널 메뉴 팝업은 자기 패널 밖으로 뻗을 수 있고, 그 위에서 고르면 아래 깔린 패널이
        // 활성이 된다(활성 판정이 포인터 위치 기반). 그 상태로 활성 패널을 닫으면 사용자가
        // 누른 것과 다른 패널이 사라진다 — 이 테스트가 그것을 막는다 (D16)
        let area = rect(0, 0, 1200, 800);
        let mut view = WorkspaceView::new(PathBuf::from(r"C:\"));
        let left = view.active;
        let right = view
            .layout
            .split(left, SplitDir::Horizontal, SplitPlace::After, area)
            .unwrap();
        view.panels
            .insert(right, PanelState::new(PathBuf::from(r"D:\")));
        // 활성은 왼쪽인데, 닫으라고 지시받은 것은 오른쪽이다
        view.active = left;

        view.close_panel(right, area);

        assert_eq!(view.layout.panel_count(), 1);
        assert!(
            view.layout.panel_ids().contains(&left),
            "대상이 아닌 활성 패널이 닫혔다"
        );
        assert!(!view.panels.contains_key(&right), "닫힌 패널 상태가 남았다");
    }

    #[test]
    fn 원격_패널을_나누면_새_패널은_시작_폴더를_연다() {
        // 사용자 보고 — 원격 탭을 보던 패널을 나누면 새 패널의 목록이 비어 있었다.
        // 원격 탭은 로컬 자리가 없어(`dir()`이 빈 경로) 그 값을 그대로 물려받았기 때문이다
        let area = rect(0, 0, 1200, 800);
        let mut view = WorkspaceView::new(PathBuf::from(r"C:\"));
        let target = view.active;
        view.panels
            .get_mut(&target)
            .expect("패널")
            .open_remote_tab_only(SiteId(1), RemotePath::new("/var/www"));

        let added = view
            .split_panel(target, SplitDir::Horizontal, SplitPlace::After, area)
            .expect("나뉘어야 한다");

        let dir = view
            .panels
            .get(&added)
            .expect("새 패널")
            .dir()
            .to_path_buf();
        assert!(
            !dir.as_os_str().is_empty(),
            "새 패널이 열거할 수 없는 빈 경로를 가리킨다"
        );
        assert_eq!(dir, start_dir(), "새 패널이 시작 폴더에서 시작하지 않는다");
    }

    #[test]
    fn 겹치지_않으면_0이다() {
        assert_eq!(overlap_area(rect(0, 0, 10, 10), rect(20, 20, 10, 10)), 0);
        // 변끼리 맞닿기만 한 경우도 넓이는 0
        assert_eq!(overlap_area(rect(0, 0, 10, 10), rect(10, 0, 10, 10)), 0);
    }

    #[test]
    fn 부분_겹침은_교집합_넓이다() {
        assert_eq!(overlap_area(rect(0, 0, 10, 10), rect(5, 5, 10, 10)), 25);
    }

    #[test]
    fn 닫힌_자리를_더_많이_덮는_쪽이_크다() {
        // 닫힌 패널이 오른쪽 절반이었다면, 그 자리를 흡수한 패널의 겹침이 더 커야 한다
        let closed = rect(500, 0, 500, 600);
        let absorbed = rect(0, 0, 1000, 600);
        let far = rect(0, 0, 200, 600);
        assert!(overlap_area(absorbed, closed) > overlap_area(far, closed));
    }

    #[test]
    fn 분할은_전달받은_패널을_대상으로_한다() {
        // 패널 메뉴는 패널마다 있어 활성 패널이 아닌 곳에서도 열린다 (D3·D16).
        // 활성 패널을 대상으로 삼으면 엉뚱한 패널이 나뉘므로 이 테스트가 그것을 막는다
        let area = rect(0, 0, 1200, 800);
        let mut view = WorkspaceView::new(PathBuf::from(r"C:\"));
        let left = view.active;
        let right = view
            .layout
            .split(left, SplitDir::Horizontal, SplitPlace::After, area)
            .unwrap();
        view.panels
            .insert(right, PanelState::new(PathBuf::from(r"D:\")));
        view.active = left;

        let left_before = pane_rect(&view, left, area);
        view.split_panel(right, SplitDir::Vertical, SplitPlace::After, area);

        assert_eq!(view.layout.panel_count(), 3);
        assert_eq!(
            pane_rect(&view, left, area),
            left_before,
            "대상이 아닌 패널(활성 패널)의 자리는 그대로여야 한다"
        );
        // 분할하면 새로 생긴 패널이 활성이 된다 — 이어서 조작할 곳이 거기이기 때문
        let added = view
            .layout
            .panel_ids()
            .into_iter()
            .find(|id| *id != left && *id != right)
            .expect("새 패널이 트리에 있어야 한다");
        assert_eq!(view.active, added);
    }

    /// 지정 패널의 현재 사각형 — 어느 패널이 나뉘었는지 자리로 판별한다
    fn pane_rect(view: &WorkspaceView, id: PanelId, area: LayoutRect) -> LayoutRect {
        view.layout
            .compute_rects(area)
            .panes
            .iter()
            .find(|(pane_id, _)| *pane_id == id)
            .map(|(_, r)| *r)
            .expect("패널이 트리에 있어야 한다")
    }

    #[test]
    fn 앞에_둔_분할도_세션_왕복에서_탭이_제자리에_남는다() {
        // 세션은 패널 배열을 리프 walk 순서로 짝지으므로, 새 리프가 앞에 들어가도
        // 저장·복원에서 짝이 밀리면 안 된다 (왼쪽·위쪽 분할의 회귀 방지)
        let area = rect(0, 0, 1200, 800);
        let mut view = WorkspaceView::new(PathBuf::from(r"C:\"));
        let kept = view.active;
        let added = view
            .layout
            .split(kept, SplitDir::Horizontal, SplitPlace::Before, area)
            .unwrap();
        view.panels
            .insert(added, PanelState::new(PathBuf::from(r"D:\")));

        let state = view.to_state("워크스페이스 1".into());
        assert_eq!(
            state.panels[0].tabs,
            vec![TabSpec::Local(PathBuf::from(r"D:\"))],
            "walk 순서상 앞에 온 새 패널이 첫 번째로 저장된다"
        );
        assert_eq!(
            state.panels[1].tabs,
            vec![TabSpec::Local(PathBuf::from(r"C:\"))]
        );

        let restored = WorkspaceView::from_state(&state);
        let ids = restored.layout.panel_ids();
        assert_eq!(restored.panels[&ids[0]].dir(), Path::new(r"D:\"));
        assert_eq!(restored.panels[&ids[1]].dir(), Path::new(r"C:\"));
    }

    #[test]
    fn 연결하면_활성_패널이_오른쪽으로_나뉜다() {
        // Acceptance ①③ — 로컬과 원격을 나란히 두는 것이 이 기능의 쓰임이다 (FR-35)
        let area = rect(0, 0, 1200, 800);
        let mut view = WorkspaceView::new(PathBuf::from(r"C:\"));
        let source = view.active;
        let before = pane_rect(&view, source, area);

        let opened = view
            .split_panel(source, SplitDir::Horizontal, SplitPlace::After, area)
            .expect("공간이 넉넉하면 나뉜다");

        assert_eq!(view.layout.panel_count(), 2, "패널 수가 늘지 않았다");
        assert_eq!(view.active, opened, "새 패널이 활성이 아니다");
        // 새 패널은 **원래 패널의 오른쪽**에 온다
        let left = pane_rect(&view, source, area);
        let right = pane_rect(&view, opened, area);
        assert!(left.x < right.x, "새 패널이 왼쪽에 생겼다");
        assert_eq!(left.x, before.x, "원래 패널이 옮겨졌다");
    }

    #[test]
    fn 연결해도_기존_분할_구조는_그대로다() {
        // Acceptance ② — 4분할 상태에서 연결해도 나머지 패널의 자리가 흔들리면 안 된다 (FR-1 회귀)
        let area = rect(0, 0, 1600, 900);
        let mut view = WorkspaceView::new(PathBuf::from(r"C:\"));
        let first = view.active;
        let second = view
            .split_panel(first, SplitDir::Horizontal, SplitPlace::After, area)
            .expect("분할 1");
        let third = view
            .split_panel(second, SplitDir::Vertical, SplitPlace::After, area)
            .expect("분할 2");
        let fourth = view
            .split_panel(first, SplitDir::Vertical, SplitPlace::After, area)
            .expect("분할 3");
        assert_eq!(view.layout.panel_count(), 4, "4분할 상태를 만들지 못했다");
        let untouched: Vec<(PanelId, LayoutRect)> = [first, third, fourth]
            .into_iter()
            .map(|id| (id, pane_rect(&view, id, area)))
            .collect();

        // 이 4분할 상태에서 두 번째 패널을 대상으로 연결한다
        view.split_panel(second, SplitDir::Horizontal, SplitPlace::After, area)
            .expect("연결 분할");

        assert_eq!(view.layout.panel_count(), 5);
        for (id, before) in untouched {
            assert_eq!(
                pane_rect(&view, id, area),
                before,
                "대상이 아닌 패널의 자리가 바뀌었다"
            );
        }
    }

    #[test]
    fn 나눌_자리가_없으면_분할하지_않는다() {
        // Acceptance ④ — 이때 호출부는 현재 패널의 새 탭으로 물러선다
        let tiny = rect(0, 0, crate::app::layout::MIN_PANE_SIZE, 400);
        let mut view = WorkspaceView::new(PathBuf::from(r"C:\"));
        let source = view.active;

        assert_eq!(
            view.split_panel(source, SplitDir::Horizontal, SplitPlace::After, tiny),
            None,
            "좁은 화면에서 분할이 섰다"
        );
        assert_eq!(view.layout.panel_count(), 1, "패널이 늘었다");
        assert_eq!(view.active, source, "활성 패널이 바뀌었다");
    }

    #[test]
    fn 워크스페이스_뷰는_패널_하나로_시작한다() {
        let view = WorkspaceView::new(PathBuf::from(r"C:\"));
        assert_eq!(view.layout.panel_count(), 1);
        assert_eq!(view.panels.len(), 1);
        assert_eq!(view.active_dir(), Some(PathBuf::from(r"C:\")));
    }

    #[test]
    fn 트레이가_켜져_있을_때만_닫기를_숨김으로_바꾼다() {
        // Acceptance — 토글이 꺼져 있으면 `✕`는 종전대로 종료한다
        assert!(
            should_hide_on_close(true, true, false),
            "숨겨야 하는데 종료한다"
        );
        assert!(
            !should_hide_on_close(false, true, false),
            "토글이 꺼졌는데 숨긴다"
        );
    }

    #[test]
    fn 아이콘이_없으면_숨기지_않는다() {
        // 아이콘 없이 숨기면 창을 되부를 방법이 사라진다 — 작업 관리자로 죽여야 한다
        assert!(
            !should_hide_on_close(true, false, false),
            "트레이 아이콘이 없는데 창을 숨긴다"
        );
    }

    #[test]
    fn 트레이_메뉴_종료는_가로채지_않는다() {
        // 그 닫기는 트레이로 보내는 것이 아니라 진짜 종료다 — 가로채면 앱을 끌 수 없다
        assert!(
            !should_hide_on_close(true, true, true),
            "종료 중인데 창만 숨긴다"
        );
    }

    /// 로컬 탭 하나를 든 패널
    fn local_panel(dir: &str) -> PanelState {
        PanelState::new(PathBuf::from(dir))
    }

    /// 연결까지 붙은 원격 탭을 활성으로 둔 패널 — 앞의 로컬 탭은 배경으로 남는다
    fn remote_panel(dir: &str, site: u32, path: &str, conn: u32) -> PanelState {
        let mut panel = local_panel(dir);
        panel.open_remote_tab(SiteId(site), RemotePath::new(path));
        assert!(
            panel.attach_conn(ConnectionId(conn)),
            "원격 탭에 연결이 붙지 않았다"
        );
        panel
    }

    #[test]
    fn 마지막으로_누른_패널의_활성_탭이_전송_대상이_된다() {
        // 화면에 아이콘으로 보이는 곳과 실제로 가는 곳이 같아야 한다 (FR-54)
        let area = rect(0, 0, 1200, 800);
        let mut view = WorkspaceView::new(PathBuf::from(r"C:\"));
        let left = view.active;
        view.panels.insert(left, local_panel(r"D:\받은 파일"));
        let right = view
            .layout
            .split(left, SplitDir::Horizontal, SplitPlace::After, area)
            .unwrap();
        view.panels
            .insert(right, remote_panel(r"C:\", 7, "/pub", 3));

        view.note_pressed(left);
        view.note_pressed(right);

        let targets = view.transfer_targets();
        assert_eq!(
            targets.download,
            view.panels[&left].active_tab_id().into(),
            "받기 대상이 마지막으로 누른 로컬 탭이 아니다"
        );
        assert_eq!(targets.upload, view.panels[&right].active_tab_id().into());
        assert_eq!(view.download_dir(), Some(PathBuf::from(r"D:\받은 파일")));
        assert_eq!(
            view.upload_dir(),
            Some((SiteId(7), RemotePath::new("/pub")))
        );
    }

    #[test]
    fn 원격_탭으로_옮겨도_받기_대상은_그_로컬_탭을_지킨다() {
        // 한 패널에 로컬·원격 탭이 섞이면(나눌 자리가 없어 같은 패널에 열린 경우) 패널만
        // 기억하는 방식은 원격 탭을 보는 순간 받기 대상을 잃는다 — 그래서 탭을 기억한다 (D2)
        let mut view = WorkspaceView::new(PathBuf::from(r"C:\"));
        let only = view.active;
        view.panels.insert(only, local_panel(r"D:\받은 파일"));
        view.note_pressed(only);
        let 로컬_탭 = view.panels[&only].active_tab_id();

        // 같은 패널에 원격 탭을 열면 로컬 탭은 배경으로 밀린다
        view.panels
            .get_mut(&only)
            .unwrap()
            .open_remote_tab(SiteId(1), RemotePath::new("/pub"));
        view.note_pressed(only);

        let targets = view.transfer_targets();
        assert_eq!(targets.download, Some(로컬_탭), "받기 대상이 사라졌다");
        assert_eq!(view.download_dir(), Some(PathBuf::from(r"D:\받은 파일")));
    }

    #[test]
    fn 배경_탭이_대상이면_올릴_것이_없다() {
        // 목록 선택은 패널마다 하나뿐이라 배경 탭의 선택은 화면에 남아 있지 않다 (D3)
        let mut view = WorkspaceView::new(PathBuf::from(r"C:\"));
        let only = view.active;
        view.panels
            .insert(only, remote_panel(r"D:\받은 파일", 1, "/pub", 2));
        view.note_pressed(only);

        let targets = view.transfer_targets();
        assert!(targets.upload.is_some(), "올릴 곳은 정해져 있어야 한다");
        assert!(view.upload_source().is_empty());
        assert!(!targets.can_upload, "배경 탭의 선택으로 올리기가 열렸다");
    }

    #[test]
    fn 대상_탭이_사라지면_같은_종류의_활성_탭으로_되돌아간다() {
        let mut view = WorkspaceView::new(PathBuf::from(r"C:\"));
        let only = view.active;
        view.panels.insert(only, local_panel(r"D:\받은 파일"));
        view.note_pressed(only);
        let 옛_탭 = view.panels[&only].active_tab_id();

        // 그 패널을 통째로 갈아 끼우면 옛 탭은 어디에도 없다
        view.panels.insert(only, local_panel(r"E:\새 폴더"));
        let targets = view.transfer_targets();
        assert_ne!(targets.download, Some(옛_탭));
        assert_eq!(view.download_dir(), Some(PathBuf::from(r"E:\새 폴더")));

        // 활성 탭이 로컬인 패널이 하나도 없으면 받기 대상 자체가 없다
        view.panels
            .insert(only, remote_panel(r"E:\새 폴더", 1, "/pub", 2));
        let targets = view.transfer_targets();
        assert_eq!(targets.download, None);
        assert!(!targets.can_download);
    }
}
