//! 앱 상태 ↔ 세션 파일 변환 (FR-11·FR-20, NFR-7).
//!
//! 저장 스키마(`app::settings::Session`)는 그대로 쓴다 — 현행 판이 쓰던 v2 그대로이며
//! 이 이식에서 형식을 바꾸지 않는다. 이 모듈은 그 스키마와 앱 상태 사이를 옮기는 **순수 변환**만
//! 담당해 UI 없이 왕복을 검증할 수 있게 한다. eframe의 `persistence`는 쓰지 않는다(D8) —
//! `settings.json`이 정본이며 저장 경로가 둘이 되면 서로 어긋난다.
use crate::app::layout::TreeShape;
use crate::app::settings::{
    DockSession, LayoutNode, PanelSession, QUEUE_SESSION_LIMIT, QueueSession, SESSION_VERSION,
    Session, SidebarSession, SiteSession, TabSession, WindowState, WorkspaceSession,
};
use crate::remote::connection::TransferDirection;
use crate::remote::queue::{TransferQueue, TransferState};
use crate::remote::types::{RemotePath, SiteId};
use std::path::PathBuf;

/// 저장·복원 사이를 오가는 워크스페이스 한 벌.
/// UI 타입(`WorkspaceView`·`PanelState`)에 기대지 않아 단위 테스트에서 그대로 만들 수 있다
#[derive(Debug, Clone, PartialEq)]
pub struct WorkspaceState {
    pub name: String,
    /// 분할 구조 (패널 id는 담지 않는다 — 복원 시 새로 부여된다)
    pub shape: TreeShape,
    /// 분할 트리 리프의 walk 순서와 1:1 (`settings` 스키마 계약)
    pub panels: Vec<PanelTabs>,
    pub active_panel: usize,
}

/// 탭 하나가 가리키는 곳 (FR-44).
///
/// 원격 탭은 **사이트와 경로만** 담는다 — 연결은 담지 않는다. 되살아난 원격 탭은
/// `연결 없음`으로 서 있고, 연결은 사용자가 연다(FR-44 — 시작하자마자 서버로 나가지 않는다)
#[derive(Debug, Clone, PartialEq)]
pub enum TabSpec {
    Local(PathBuf),
    Remote { site: SiteId, path: RemotePath },
}

impl TabSpec {
    /// 사이드바 부제·시작 폴더에 쓸 로컬 경로 — 원격 탭이면 없다
    pub fn local_path(&self) -> Option<&PathBuf> {
        match self {
            TabSpec::Local(path) => Some(path),
            TabSpec::Remote { .. } => None,
        }
    }
}

/// 패널 하나의 탭 구성과 목록 표시 상태
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PanelTabs {
    pub tabs: Vec<TabSpec>,
    pub active_tab: usize,
    /// 자세히 보기 열 폭 — 비면 기본 폭으로 시작한다
    pub columns: Vec<f32>,
    /// 보기 모드 키 — **활성 탭의 것**이며 탭별 키가 비었을 때의 대체다. 이것도 비면 기본값(자세히)
    pub view_mode: String,
    /// 탭마다의 보기 모드 키 (FR-23) — `tabs`와 순서가 1:1이다.
    ///
    /// **비었거나 모자라도 여기서 채우지 않는다** — 저장된 그대로 옮기고, 패널 값으로 대신하는
    /// 것은 `PanelState::from_tabs` 한 곳이다(여기서 채우면 왕복이 입력과 달라진다)
    pub tab_view_modes: Vec<String>,
    /// 정렬 기준 키 — 비면 기본값(이름 오름차순)으로 시작한다
    pub sort_key: String,
    /// 정렬 방향 — 참이면 오름차순. **`sort_key`가 비면 읽지 않는다**(둘은 함께 담긴다)
    pub sort_ascending: bool,
    /// 자세히 보기 열이 서는 차례 — 비면 기본 차례로 시작한다
    pub column_order: Vec<String>,
}

/// 저장할 때 함께 담는 원격 쪽 상태 (FR-44).
///
/// 이름이 `RemoteSession`이 아닌 이유: `remote::types::RemoteSession`은 프로토콜 세션
/// 트레이트라 같은 이름이면 읽는 사람이 둘을 헷갈린다
pub struct RemoteSnapshot<'a> {
    pub sites: &'a SiteSession,
    pub queue: &'a TransferQueue,
    /// 열려 있던 도크 패널 키(`queue`/`log`)와 거르개 — 닫혀 있었으면 빈 문자열
    pub dock: DockSession,
}

/// 현재 상태를 저장 스키마로 옮긴다
pub fn to_session(
    window: WindowState,
    sidebar: SidebarSession,
    active_workspace: usize,
    workspaces: &[WorkspaceState],
    remote: RemoteSnapshot<'_>,
) -> Session {
    Session {
        version: SESSION_VERSION,
        window,
        sidebar,
        active_workspace,
        sites: remote.sites.clone(),
        queue: to_queue_session(remote.queue),
        dock: remote.dock,
        // 앱 설정과 즐겨찾기는 창·워크스페이스와 성격이 달라 이 함수가 받지 않는다 —
        // 부르는 쪽(`ExplorerApp::collect_session`)이 자기 값으로 덮어쓴다.
        // 즐겨찾기를 덮는 것은 아래 `with_favorites`다
        settings: Default::default(),
        favorites: Vec::new(),
        workspaces: workspaces
            .iter()
            .map(|workspace| WorkspaceSession {
                name: workspace.name.clone(),
                layout: LayoutNode::from_shape(&workspace.shape),
                panels: workspace
                    .panels
                    .iter()
                    .map(|panel| PanelSession {
                        tabs: panel
                            .tabs
                            .iter()
                            .enumerate()
                            .map(|(index, tab)| TabSession {
                                // 모자라면 빈 값("저장 안 됨")이다 — 채우는 것은 패널이다
                                view_mode: panel
                                    .tab_view_modes
                                    .get(index)
                                    .cloned()
                                    .unwrap_or_default(),
                                ..to_tab_session(tab)
                            })
                            .collect(),
                        active_tab: panel.active_tab,
                        columns: panel.columns.clone(),
                        view_mode: panel.view_mode.clone(),
                        sort_key: panel.sort_key.clone(),
                        sort_ascending: panel.sort_ascending,
                        column_order: panel.column_order.clone(),
                    })
                    .collect(),
                active_panel: workspace.active_panel,
            })
            .collect(),
    }
}

/// 즐겨찾기를 세션에 싣는다 (FR-56).
///
/// **`to_session`이 받지 않고 이 함수로 덮는 이유**: `to_session`은 창·워크스페이스를 옮기는
/// 자리라 인자를 더하면 책임이 흐려지고 호출부(이 파일의 시험 여러 곳)가 함께 는다 — 앱 설정이
/// 이미 같은 방식이다.
///
/// **부르는 쪽이 스프레드(`..to_session(..)`)라 이 한 줄을 빠뜨려도 컴파일이 통과한다**.
/// 그래서 덮는 규칙을 순수 함수로 떼어 두고 시험이 이 함수를 직접 부른다 (plan D7).
/// 경로는 여기서 문자열이 된다 — 저장 형식이 문자열이기 때문이다(D9, `to_tab_session`과 같다)
pub fn with_favorites(session: Session, favorites: &[PathBuf]) -> Session {
    Session {
        favorites: favorites
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        ..session
    }
}

/// 저장된 세션에서 워크스페이스 목록을 되살린다.
///
/// 무결성(패널 수 = 리프 수, 활성 인덱스 범위, 빈 탭 없음)은 `settings::parse_session`이
/// 이미 확인한 뒤이므로 여기서는 형 변환만 한다
pub fn restore(session: &Session) -> Vec<WorkspaceState> {
    session
        .workspaces
        .iter()
        .map(|workspace| WorkspaceState {
            name: workspace.name.clone(),
            shape: workspace.layout.to_shape(),
            panels: workspace
                .panels
                .iter()
                .map(|panel| PanelTabs {
                    tabs: panel
                        .tabs
                        .iter()
                        .map(|tab| from_tab_session(tab, &session.sites))
                        .collect(),
                    active_tab: panel.active_tab,
                    columns: panel.columns.clone(),
                    view_mode: panel.view_mode.clone(),
                    tab_view_modes: panel.tabs.iter().map(|tab| tab.view_mode.clone()).collect(),
                    sort_key: panel.sort_key.clone(),
                    sort_ascending: panel.sort_ascending,
                    column_order: panel.column_order.clone(),
                })
                .collect(),
            active_panel: workspace.active_panel,
        })
        .collect()
}

/// 탭 하나를 저장 형태로
fn to_tab_session(tab: &TabSpec) -> TabSession {
    match tab {
        TabSpec::Local(path) => TabSession::local(path.to_string_lossy().into_owned()),
        TabSpec::Remote { site, path } => {
            TabSession::remote(crate::app::settings::RemoteTabSession {
                site: site.0,
                path: path.as_str().to_owned(),
            })
        }
    }
}

/// 저장 형태에서 탭 하나를 되살린다.
///
/// **가리키던 사이트가 사라졌으면 로컬 홈으로 되돌린다**(plan Edge Case) — 없는 사이트를
/// 가리키는 원격 탭은 열 수도 지울 수도 없는 자리가 된다
fn from_tab_session(tab: &TabSession, sites: &SiteSession) -> TabSpec {
    let Some(remote) = tab.as_remote() else {
        return TabSpec::Local(PathBuf::from(&tab.path));
    };
    let site = SiteId(remote.site);
    if sites.get(site).is_none() {
        return TabSpec::Local(home_dir());
    }
    TabSpec::Remote {
        site,
        path: RemotePath::new(&remote.path),
    }
}

/// 사이트를 잃은 원격 탭이 돌아갈 자리 — 사용자 폴더, 없으면 `C:\`
fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\"))
}

/// 큐에서 **아직 끝나지 않은 것**만 저장 형태로 옮긴다 (FR-44).
///
/// 끝난 것은 되살릴 이유가 없고, 옮기는 중이던 것은 대기로 되돌린다(연결이 끊긴 채
/// 재시작했으므로 이어받기는 다음에 다시 시작할 때 정해진다).
/// 상한(`QUEUE_SESSION_LIMIT`)을 넘으면 앞의 것만 담는다 — 세션 파일이 무한정 커지면
/// 창이 뜨는 시간이 그만큼 늘어난다
fn to_queue_session(queue: &TransferQueue) -> Vec<QueueSession> {
    queue
        .items()
        .iter()
        // 완료와 취소는 담지 않는다 — 끝난 일이고, 사용자가 스스로 그만둔 것을 다음 실행까지
        // 끌고 갈 이유가 없다 (2026-08-28 결정). 복원 대상은 대기·진행·실패다
        .filter(|item| !matches!(item.state, TransferState::Done | TransferState::Cancelled))
        .take(QUEUE_SESSION_LIMIT)
        .map(|item| QueueSession {
            site: item.site.0,
            direction: match item.direction {
                TransferDirection::Upload => DIRECTION_UPLOAD.to_owned(),
                TransferDirection::Download => DIRECTION_DOWNLOAD.to_owned(),
            },
            local: item.local.to_string_lossy().into_owned(),
            remote: item.remote.as_str().to_owned(),
            size: item.size,
            error: match &item.state {
                TransferState::Error { message } => message.clone(),
                _ => String::new(),
            },
            // `0`이 「모른다」다 — `Option`을 그대로 담으면 세션 형식이 `null`을 들게 되고,
            // 옛 파일과의 호환을 위해 어차피 `default`(=0)를 받아야 한다
            started_at: item.started_at.unwrap_or(0),
            finished_at: item.finished_at.unwrap_or(0),
        })
        .collect()
}

/// 저장된 큐를 되살린다 — **스스로 시작하지 않는다**(FR-44).
///
/// 사이트가 사라진 항목은 버린다(plan Edge Case) — 어디로 보낼지 알 수 없다
/// 저장된 워크스페이스 상태에서 그 사이트의 원격 탭을 **로컬 폴더로 되돌린다** (FR-29).
///
/// 사이드바에서 사이트를 지울 때 아직 한 번도 열지 않은 워크스페이스를 위해 쓴다 —
/// 그쪽은 뷰가 없어(`ui::app`의 지연 생성) 탭을 닫을 수 없고, 손대지 않으면 저장된 상태가
/// 그대로 다시 저장돼 다음에 그 워크스페이스를 열 때 지운 사이트가 되살아난다.
///
/// **탭을 빼지 않고 바꾼다** — 목록이 비면 그 패널을 되살릴 수 없고(`PanelState::from_tabs`)
/// 활성 탭 번호도 어긋난다. 사이트를 잃은 원격 탭을 로컬로 되돌리는 것은
/// 복원 경로(`from_tab_session`)가 이미 하는 처리와 같다
pub fn detach_site_from_state(state: &mut WorkspaceState, site: SiteId, local: &std::path::Path) {
    for panel in &mut state.panels {
        for tab in &mut panel.tabs {
            if matches!(tab, TabSpec::Remote { site: at, .. } if *at == site) {
                *tab = TabSpec::Local(local.to_path_buf());
            }
        }
    }
}

pub fn restore_queue(session: &Session) -> TransferQueue {
    let mut queue = TransferQueue::new();
    for saved in &session.queue {
        let site = SiteId(saved.site);
        if session.sites.get(site).is_none() {
            continue;
        }
        let direction = if saved.direction == DIRECTION_UPLOAD {
            TransferDirection::Upload
        } else {
            TransferDirection::Download
        };
        let id = queue.enqueue(
            site,
            direction,
            PathBuf::from(&saved.local),
            RemotePath::new(&saved.remote),
            saved.size,
        );
        // 실패로 저장된 것은 실패인 채로 선다 — 다시 시도는 사용자가 고른다
        if !saved.error.is_empty() {
            queue.update(
                id,
                TransferState::Error {
                    message: saved.error.clone(),
                },
            );
        }
        // **시각은 `update` 다음에, 그리고 위 `if` 밖에서 심는다** — `update(Error)`가
        // `finished_at`을 그때의 시각으로 덮어쓰므로 순서를 뒤집으면 저장된 값이 사라지고,
        // `if` 안에 두면 대기로 복원되는 항목(저장 대상은 `Done`이 아닌 전부다)의
        // `started_at`이 조용히 없어진다
        queue.set_times(
            id,
            (saved.started_at != 0).then_some(saved.started_at),
            (saved.finished_at != 0).then_some(saved.finished_at),
        );
    }
    queue
}

/// 저장 파일에 적히는 방향 키
const DIRECTION_UPLOAD: &str = "upload";
const DIRECTION_DOWNLOAD: &str = "download";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::settings::{AppSettings, LanguageSetting};

    /// JSON 어디에 있든 주어진 키를 지운다 — **그 필드가 없던 시절의 세션**을 흉내 낸다.
    ///
    /// 값 리터럴을 문자열로 지우던 종전 방식은 기본 열 폭·보기 모드가 바뀌면 걷어내지 못한 채
    /// 통과할 수 있었다(값을 코드 두 곳에 적어 둔 셈이다). 키만 보면 값이 무엇이든 걸린다.
    ///
    /// **이름이 같은 다른 필드도 함께 지워진다** — `columns`는 패널 열 폭과 큐 표 열 폭
    /// (`DockSession`) 둘이 같은 이름을 쓴다. 그쪽까지 지우는 편이 「더 옛날 세션」에 가깝고
    /// 둘 다 `#[serde(default)]`라 복원되므로, 좁히는 대신 이 사실을 적어 둔다
    fn strip_keys(value: &mut serde_json::Value, keys: &[&str]) {
        match value {
            serde_json::Value::Object(map) => {
                for key in keys {
                    map.remove(*key);
                }
                for nested in map.values_mut() {
                    strip_keys(nested, keys);
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    strip_keys(item, keys);
                }
            }
            _ => {}
        }
    }

    /// 원격 쪽이 비어 있는 저장 — 로컬만 다루는 시험이 쓴다
    fn empty_remote() -> RemoteSnapshot<'static> {
        // 빈 사이트 목록·빈 큐는 시험이 사는 동안만 있으면 되므로 흘려 둔다
        static EMPTY_SITES: std::sync::OnceLock<SiteSession> = std::sync::OnceLock::new();
        static EMPTY_QUEUE: std::sync::OnceLock<TransferQueue> = std::sync::OnceLock::new();
        RemoteSnapshot {
            sites: EMPTY_SITES.get_or_init(SiteSession::default),
            queue: EMPTY_QUEUE.get_or_init(TransferQueue::new),
            dock: DockSession::default(),
        }
    }

    /// 시험이 로컬 탭을 짧게 적기 위한 길
    fn local(path: &str) -> TabSpec {
        TabSpec::Local(PathBuf::from(path))
    }
    use crate::app::layout::SplitDir;
    use crate::app::settings::parse_session;

    fn sample() -> Vec<WorkspaceState> {
        vec![
            WorkspaceState {
                name: "워크스페이스 1".into(),
                shape: TreeShape::Split {
                    dir: SplitDir::Horizontal,
                    ratio: 0.4,
                    first: Box::new(TreeShape::Leaf),
                    second: Box::new(TreeShape::Leaf),
                },
                panels: vec![
                    PanelTabs {
                        tabs: vec![local(r"C:\Users"), local(r"D:\")],
                        active_tab: 1,
                        columns: vec![200.0, 60.0, 120.0, 90.0],
                        view_mode: "tiles".into(),
                        // 활성 탭(1)의 것이 `view_mode`와 같다 — 앱이 저장하는 모양 그대로다
                        tab_view_modes: vec!["list".into(), "tiles".into()],
                        sort_key: "size".into(),
                        sort_ascending: false,
                        column_order: vec!["size".into(), "name".into()],
                    },
                    PanelTabs {
                        tabs: vec![local(r"C:\Windows")],
                        active_tab: 0,
                        columns: Vec::new(),
                        view_mode: String::new(),
                        // 탭과 1:1 — 저장본은 탭마다 키 자리를 하나씩 든다(비었으면 "저장 안 됨")
                        tab_view_modes: vec![String::new()],
                        sort_key: String::new(),
                        sort_ascending: true,
                        column_order: Vec::new(),
                    },
                ],
                active_panel: 1,
            },
            WorkspaceState {
                name: "자료 정리".into(),
                shape: TreeShape::Leaf,
                panels: vec![PanelTabs {
                    tabs: vec![local(r"D:\작업")],
                    active_tab: 0,
                    columns: Vec::new(),
                    view_mode: "large_icons".into(),
                    tab_view_modes: vec![String::new()],
                    sort_key: "modified".into(),
                    sort_ascending: true,
                    column_order: Vec::new(),
                }],
                active_panel: 0,
            },
        ]
    }

    fn window() -> WindowState {
        WindowState {
            x: 100,
            y: 50,
            w: 1200,
            h: 800,
            maximized: false,
        }
    }

    #[test]
    fn 워크스페이스는_왕복해도_같다() {
        let states = sample();
        let session = to_session(
            window(),
            SidebarSession::default(),
            1,
            &states,
            empty_remote(),
        );
        assert_eq!(restore(&session), states);
    }

    #[test]
    fn 저장한_세션은_무결성_검사를_통과한다() {
        // 앱이 만든 세션이 자기 파서에 거부되면 재시작마다 조용히 초기화된다
        let session = to_session(
            window(),
            SidebarSession::default(),
            0,
            &sample(),
            empty_remote(),
        );
        let json = serde_json::to_string(&session).unwrap();
        assert_eq!(parse_session(&json), Some(session));
    }

    #[test]
    fn 나중에_더한_필드가_없는_옛_세션도_그대로_복원된다() {
        // 열 폭(T3)·보기 모드(T12)는 나중에 더한 필드다. 이것 때문에 스키마 버전을 올리면
        // `parse_session`이 통째로 폴백해 **기존 사용자의 워크스페이스·분할·탭이 전부
        // 초기화된다** (plan D5). 그래서 버전을 2로 둔 채 `#[serde(default)]`로 더했고,
        // 이 테스트가 그 계약을 지킨다 — 앞으로 필드를 더할 때는 아래 `strip_keys`에
        // 그 키 이름을 더한다
        let session = to_session(
            window(),
            SidebarSession::default(),
            0,
            &sample(),
            empty_remote(),
        );
        let mut value: serde_json::Value = serde_json::to_value(&session).unwrap();
        strip_keys(
            &mut value,
            &["columns", "columns_done", "columns_error", "view_mode"],
        );
        let without_columns = serde_json::to_string(&value).unwrap();
        assert!(
            !without_columns.contains("columns") && !without_columns.contains("view_mode"),
            "테스트가 새 필드를 실제로 걷어내지 못했다"
        );

        let parsed = parse_session(&without_columns).expect("옛 세션이 거부됐다");
        let restored = restore(&parsed);
        // 열 폭만 비고 나머지 구성은 그대로여야 한다
        assert_eq!(restored.len(), sample().len());
        assert_eq!(restored[0].shape, sample()[0].shape);
        assert_eq!(restored[0].panels[0].tabs, sample()[0].panels[0].tabs);
        assert_eq!(
            restored[0].panels[0].active_tab,
            sample()[0].panels[0].active_tab
        );
        assert!(restored[0].panels[0].columns.is_empty(), "없던 폭이 생겼다");
        assert!(
            restored[0].panels[0].view_mode.is_empty(),
            "없던 보기 모드가 생겼다"
        );
    }

    #[test]
    fn 열_폭은_패널마다_따로_왕복한다() {
        // 한 패널에서 조절한 폭이 다른 패널에 번지면 "패널마다 독립"이 깨진다
        let states = sample();
        let session = to_session(
            window(),
            SidebarSession::default(),
            0,
            &states,
            empty_remote(),
        );
        let restored = restore(&session);
        assert_eq!(
            restored[0].panels[0].columns,
            vec![200.0, 60.0, 120.0, 90.0]
        );
        assert!(restored[0].panels[1].columns.is_empty());
    }

    #[test]
    fn 보기_모드는_패널마다_따로_왕복한다() {
        // 한 패널에서 고른 모드가 다른 패널에 번지면 "패널마다 독립"(FR-23)이 깨진다
        let session = to_session(
            window(),
            SidebarSession::default(),
            0,
            &sample(),
            empty_remote(),
        );
        let restored = restore(&session);
        assert_eq!(restored[0].panels[0].view_mode, "tiles");
        assert!(restored[0].panels[1].view_mode.is_empty());
        assert_eq!(restored[1].panels[0].view_mode, "large_icons");
    }

    #[test]
    fn 탭마다의_보기_모드는_파일을_거쳐도_남는다() {
        // 재시작하면 탭별 보기가 사라지고 패널 하나의 모드로 뭉개지면 "탭마다 따로"가 반쪽이다
        let session = to_session(
            window(),
            SidebarSession::default(),
            0,
            &sample(),
            empty_remote(),
        );
        let text = serde_json::to_string(&session).expect("직렬화");
        let parsed = parse_session(&text).expect("앱이 쓴 세션이 거부됐다");
        let restored = restore(&parsed);
        assert_eq!(restored[0].panels[0].tab_view_modes, ["list", "tiles"]);
        // 탭별로 바꾼 적 없는 패널은 빈 채로 돌아온다 — 채우는 것은 패널이다
        assert!(
            restored[1].panels[0]
                .tab_view_modes
                .iter()
                .all(String::is_empty)
        );
    }

    #[test]
    fn 저장된_보기_모드_키는_실제_모드로_되살아난다() {
        // 세션에 담기는 것은 문자열이라, 그것이 모드로 되돌아오는지까지 확인해야
        // "재시작하면 모드가 유지된다"가 성립한다
        use crate::ui::view_mode::ViewMode;
        let session = to_session(
            window(),
            SidebarSession::default(),
            0,
            &sample(),
            empty_remote(),
        );
        let restored = restore(&session);
        assert_eq!(
            ViewMode::from_key(&restored[0].panels[0].view_mode),
            ViewMode::Tiles
        );
        assert_eq!(
            ViewMode::from_key(&restored[1].panels[0].view_mode),
            ViewMode::LargeIcons
        );
        // 빈 문자열은 기본값으로 — 저장된 적 없는 패널이다
        assert_eq!(
            ViewMode::from_key(&restored[0].panels[1].view_mode),
            ViewMode::Details
        );
    }

    /// 사이트 하나를 등록한 저장본 — 원격 탭·큐가 가리킬 곳이 있어야 한다
    fn session_with_site() -> (Session, SiteId) {
        let mut sites = SiteSession::default();
        let site = sites.add("배포 서버");
        let mut session = to_session(
            window(),
            SidebarSession::default(),
            0,
            &[WorkspaceState {
                name: "작업".into(),
                shape: TreeShape::Leaf,
                panels: vec![PanelTabs {
                    tabs: vec![
                        local(r"C:\Users"),
                        TabSpec::Remote {
                            site,
                            path: RemotePath::new("/var/www"),
                        },
                    ],
                    active_tab: 1,
                    ..Default::default()
                }],
                active_panel: 0,
            }],
            RemoteSnapshot {
                sites: &sites,
                queue: &TransferQueue::new(),
                dock: DockSession::default(),
            },
        );
        session.sites = sites;
        (session, site)
    }

    #[test]
    fn 원격_탭은_사이트와_경로로_왕복한다() {
        // Acceptance ②③ — 연결은 담기지 않는다(되살아난 탭은 `연결 없음`으로 선다)
        let (session, site) = session_with_site();
        let restored = restore(&session);
        assert_eq!(
            restored[0].panels[0].tabs[1],
            TabSpec::Remote {
                site,
                path: RemotePath::new("/var/www")
            }
        );
        // 저장 형태에도 연결 흔적이 없다
        let saved = &session.workspaces[0].panels[0].tabs[1];
        assert_eq!(saved.site, Some(site.0));
        assert_eq!(saved.kind, crate::app::settings::TabSession::REMOTE);
    }

    #[test]
    fn 사라진_사이트를_가리키는_원격_탭은_로컬로_되돌아간다() {
        // plan Edge Case — 없는 사이트를 가리키는 탭은 열 수도 지울 수도 없는 자리가 된다
        let (mut session, _) = session_with_site();
        session.sites = SiteSession::default();
        let restored = restore(&session);
        assert!(
            matches!(restored[0].panels[0].tabs[1], TabSpec::Local(_)),
            "사이트가 사라졌는데 원격 탭으로 남았다"
        );
    }

    #[test]
    fn 열지_않은_워크스페이스에서도_지운_사이트_탭이_로컬로_바뀐다() {
        // FR-29 — 그 워크스페이스는 뷰가 없어 탭을 닫을 수 없다. 손대지 않으면 저장된 상태가
        // 그대로 다시 저장돼 다음에 열 때 지운 사이트가 되살아난다 (F-7 M1)
        let (session, site) = session_with_site();
        let mut state = restore(&session).remove(0);
        let 탭_수 = state.panels[0].tabs.len();
        assert!(
            matches!(state.panels[0].tabs[1], TabSpec::Remote { .. }),
            "원격 탭이 있는 상태로 시작해야 한다"
        );

        detach_site_from_state(&mut state, site, std::path::Path::new(r"C:\시작"));

        assert!(
            matches!(&state.panels[0].tabs[1], TabSpec::Local(path) if path == std::path::Path::new(r"C:\시작")),
            "지운 사이트의 탭이 로컬로 바뀌지 않았다"
        );
        // 탭을 빼지 않는다 — 목록이 비면 패널을 되살릴 수 없고 활성 탭 번호도 어긋난다
        assert_eq!(state.panels[0].tabs.len(), 탭_수);

        // 다른 사이트의 탭은 건드리지 않는다
        let mut 남는다 = restore(&session).remove(0);
        detach_site_from_state(
            &mut 남는다,
            SiteId(site.0 + 1),
            std::path::Path::new(r"C:\시작"),
        );
        assert!(matches!(남는다.panels[0].tabs[1], TabSpec::Remote { .. }));
    }

    /// 사용자가 그만둔 전송은 재시작 뒤에 남지 않는다 — 끝난 것과 같은 자리에서 버린다
    #[test]
    fn 취소한_항목은_세션에_담기지_않는다() {
        let (mut session, site) = session_with_site();
        let mut queue = TransferQueue::new();
        let cancelled = queue.enqueue(
            site,
            TransferDirection::Upload,
            PathBuf::from(r"C:\그만둔.zip"),
            RemotePath::new("/그만둔.zip"),
            100,
        );
        let failed = queue.enqueue(
            site,
            TransferDirection::Upload,
            PathBuf::from(r"C:\실패.zip"),
            RemotePath::new("/실패.zip"),
            200,
        );
        queue.cancel(cancelled);
        queue.update(
            failed,
            TransferState::Error {
                message: "550 권한 거부".to_owned(),
            },
        );
        assert_eq!(queue.len(), 2, "취소분도 화면에는 남아 있다");

        session.queue = to_queue_session(&queue);
        let restored = restore_queue(&session);
        assert_eq!(restored.len(), 1, "취소분은 복원되지 않는다");
        assert_eq!(
            restored.items()[0].remote.as_str(),
            "/실패.zip",
            "남는 것은 실패분이다"
        );
    }

    #[test]
    fn 큐는_끝나지_않은_것만_저장되고_스스로_시작하지_않는다() {
        // Acceptance ④ — 대기·실패만 담고, 되살아난 항목은 대기 상태로 선다
        let (mut session, site) = session_with_site();
        let mut queue = TransferQueue::new();
        let done = queue.enqueue(
            site,
            TransferDirection::Upload,
            PathBuf::from(r"C:\끝난.txt"),
            RemotePath::new("/끝난.txt"),
            10,
        );
        queue.update(done, TransferState::Done);
        let failed = queue.enqueue(
            site,
            TransferDirection::Download,
            PathBuf::from(r"C:\실패.bin"),
            RemotePath::new("/실패.bin"),
            20,
        );
        queue.update(
            failed,
            TransferState::Error {
                message: "550 권한 거부".to_owned(),
            },
        );
        queue.enqueue(
            site,
            TransferDirection::Upload,
            PathBuf::from(r"C:\대기.zip"),
            RemotePath::new("/대기.zip"),
            30,
        );

        session.queue = to_queue_session(&queue);
        let saved: Vec<&str> = session.queue.iter().map(|q| q.remote.as_str()).collect();
        assert_eq!(saved, vec!["/실패.bin", "/대기.zip"], "끝난 것이 담겼다");

        let restored = restore_queue(&session);
        assert_eq!(restored.items().len(), 2);
        let states: Vec<bool> = restored
            .items()
            .iter()
            .map(|item| item.state.is_active())
            .collect();
        assert_eq!(states, vec![false, false], "되살아나자마자 전송이 시작됐다");
        assert!(matches!(
            restored.items()[0].state,
            TransferState::Error { .. }
        ));
    }

    #[test]
    fn 큐_항목의_시각이_왕복한다() {
        // **복원 시각과 다른 값을 밀어 넣고 복원한다** — `restore_queue`가 `update(Error)`로
        // 끝 시각을 「지금」으로 덮어쓰면 이 시험이 그것을 잡는다
        const 시작: u64 = 133_000_000_000_000_000;
        const 끝: u64 = 133_000_000_010_000_000;
        const 복원_시각: u64 = 133_999_999_990_000_000;

        let (mut session, site) = session_with_site();
        let mut queue = TransferQueue::new();
        queue.set_wall_now(시작);

        // ① 실패로 저장될 항목 — 시작·끝이 다 있다
        let 실패 = queue.enqueue(
            site,
            TransferDirection::Download,
            PathBuf::from(r"C:\실패.bin"),
            RemotePath::new("/실패.bin"),
            10,
        );
        queue.update(실패, TransferState::Active { sent: 0, speed: 0 });
        queue.set_wall_now(끝);
        queue.update(
            실패,
            TransferState::Error {
                message: "550 실패".to_owned(),
            },
        );

        // ② **대기로 저장될 항목** — 한 번 시작했다 멈춘 것이라 시작 시각만 있다.
        //    `set_times`를 실패 분기 안에 두면 이 항목의 시작 시각이 조용히 사라진다
        queue.set_wall_now(시작);
        let 대기 = queue.enqueue(
            site,
            TransferDirection::Upload,
            PathBuf::from(r"C:\대기.zip"),
            RemotePath::new("/대기.zip"),
            30,
        );
        queue.update(대기, TransferState::Active { sent: 0, speed: 0 });
        queue.update(대기, TransferState::Wait);

        // ③ 한 번도 시작하지 않은 항목 — 시각이 둘 다 없다
        queue.enqueue(
            site,
            TransferDirection::Upload,
            PathBuf::from(r"C:\새것.txt"),
            RemotePath::new("/새것.txt"),
            5,
        );

        session.queue = to_queue_session(&queue);
        let mut restored = restore_queue(&session);
        // 복원 뒤에 시계를 밀어 넣어도 저장된 값이 그대로여야 한다
        restored.set_wall_now(복원_시각);

        let items = restored.items();
        assert_eq!(items.len(), 3);
        assert_eq!(
            (items[0].started_at, items[0].finished_at),
            (Some(시작), Some(끝)),
            "실패 항목의 시각이 복원 시각으로 덮였다"
        );
        assert_eq!(
            (items[1].started_at, items[1].finished_at),
            (Some(시작), None),
            "대기로 복원되는 항목의 시작 시각이 사라졌다 — `set_times`가 실패 분기 안에 갇혔다"
        );
        assert_eq!(
            (items[2].started_at, items[2].finished_at),
            (None, None),
            "시작한 적 없는 항목에 시각이 생겼다"
        );
    }

    #[test]
    fn 시각_키가_없는_옛_세션도_그대로_읽힌다() {
        // `#[serde(default)]`라 스키마 버전을 올리지 않았다 — 버전을 올리면 `parse_session`이
        // 통째로 폴백해 워크스페이스·분할·탭까지 초기화된다.
        //
        // **큐 항목만 따로 읽는다** — 이 시험의 관심사는 새 키 둘의 호환이고, 세션 전체를
        // 적으면 무관한 필수 필드가 늘 때마다 이 시험이 함께 깨진다
        let 옛_항목 = r#"{
            "site": 1,
            "direction": "download",
            "local": "C:/받은 것/a.txt",
            "remote": "/pub/a.txt",
            "size": 100,
            "error": "550 실패"
        }"#;
        let mut saved: crate::app::settings::QueueSession =
            serde_json::from_str(옛_항목).expect("시각 키가 없어도 읽혀야 한다");
        assert_eq!(saved.remote, "/pub/a.txt");
        assert_eq!(saved.error, "550 실패", "나머지 필드가 그대로다");
        assert_eq!(
            (saved.started_at, saved.finished_at),
            (0, 0),
            "없는 키는 0이다"
        );

        // 그 항목을 되살리면 시각이 「모른다」로 선다
        let (mut session, site) = session_with_site();
        // 사이트가 없으면 복원이 그 항목을 버린다 — 이 시험의 관심사는 그쪽이 아니다
        saved.site = site.0;
        session.queue = vec![saved];
        let restored = restore_queue(&session);
        assert_eq!(restored.items().len(), 1);
        assert_eq!(
            (
                restored.items()[0].started_at,
                restored.items()[0].finished_at
            ),
            (None, None)
        );
    }

    #[test]
    fn 사이트가_사라진_큐_항목은_버린다() {
        // plan Edge Case — 어디로 보낼지 알 수 없는 항목이다
        let (mut session, site) = session_with_site();
        let mut queue = TransferQueue::new();
        queue.enqueue(
            site,
            TransferDirection::Upload,
            PathBuf::from(r"C:\a.txt"),
            RemotePath::new("/a.txt"),
            1,
        );
        session.queue = to_queue_session(&queue);
        session.sites = SiteSession::default();
        assert!(restore_queue(&session).items().is_empty());
    }

    #[test]
    fn 큐는_상한까지만_저장한다() {
        // plan Edge Case — 1만 건이어도 앞의 1000건까지만 담는다
        let (_, site) = session_with_site();
        let mut queue = TransferQueue::new();
        for index in 0..QUEUE_SESSION_LIMIT + 500 {
            queue.enqueue(
                site,
                TransferDirection::Upload,
                PathBuf::from(format!(r"C:\{index}.bin")),
                RemotePath::new(&format!("/{index}.bin")),
                1,
            );
        }
        assert_eq!(to_queue_session(&queue).len(), QUEUE_SESSION_LIMIT);
    }

    #[test]
    fn 저장본에_평문_비밀번호가_없다() {
        // Acceptance ⑤ — 비밀번호는 DPAPI로 봉인된 바이트로만 담긴다 (FR-28)
        let (mut session, site) = session_with_site();
        let sealed = session.sites.set_password(site, "비밀!1234");
        let text = serde_json::to_string(&session).expect("직렬화");
        assert!(!text.contains("비밀!1234"), "평문이 저장본에 남았다");
        if sealed {
            // 봉인이 되는 환경에서는 봉인된 바이트가 실려 있어야 한다
            assert!(
                text.contains("password_sealed"),
                "봉인된 비밀번호 자리가 없다"
            );
        }
    }

    #[test]
    fn 앱_설정이_없는_옛_세션도_그대로_복원된다() {
        // T1 Acceptance — 설정은 스키마 버전을 올리지 않고 더했다(D2). 버전을 올렸다면
        // `parse_session`이 통째로 폴백해 기존 워크스페이스·분할·탭이 전부 초기화된다
        let session = to_session(
            window(),
            SidebarSession::default(),
            0,
            &sample(),
            empty_remote(),
        );
        let json = serde_json::to_string(&session).expect("직렬화");
        let settings_json = serde_json::to_string(&AppSettings::default()).expect("직렬화");
        let without_settings = json.replace(&format!(",\"settings\":{settings_json}"), "");
        assert!(
            !without_settings.contains("\"settings\""),
            "테스트가 settings 필드를 실제로 걷어내지 못했다"
        );

        let parsed = parse_session(&without_settings).expect("옛 세션이 거부됐다");
        assert_eq!(parsed.settings, AppSettings::default());
        assert_eq!(restore(&parsed).len(), sample().len());
    }

    #[test]
    fn 앱_설정이_왕복한다() {
        let saved = AppSettings {
            font_family: Some("굴림".into()),
            auto_start: true,
            tray_on_close: true,
            show_extensions: false,
            show_hidden: false,
            show_system: true,
            language: LanguageSetting::English,
            remote_auto_refresh: false,
            remote_refresh_secs: 300,
            transfer_retry_count: 7,
        };
        let mut session = to_session(
            window(),
            SidebarSession::default(),
            0,
            &sample(),
            empty_remote(),
        );
        session.settings = saved.clone();
        let text = serde_json::to_string(&session).expect("직렬화");
        let parsed = parse_session(&text).expect("파싱");
        assert_eq!(parsed.settings, saved);
    }

    #[test]
    fn 알_수_없는_언어_키는_기본값이_된다() {
        // 설정 파일이 손으로 편집돼도 **세션 전체가 폴백되지 않아야** 한다 —
        // 파싱이 실패하면 워크스페이스까지 함께 날아간다
        let mut session = to_session(
            window(),
            SidebarSession::default(),
            0,
            &sample(),
            empty_remote(),
        );
        session.settings.language = LanguageSetting::Korean;
        let text = serde_json::to_string(&session)
            .expect("직렬화")
            .replace("\"language\":\"ko\"", "\"language\":\"klingon\"");
        let parsed = parse_session(&text).expect("알 수 없는 언어 키에 세션이 통째로 폴백됐다");
        assert_eq!(parsed.settings.language, LanguageSetting::System);
    }

    #[test]
    fn 빈_글꼴_이름은_고르지_않은_것과_같다() {
        let blank = AppSettings {
            font_family: Some("   ".into()),
            ..Default::default()
        };
        assert_eq!(blank.selected_font(), None);
        let picked = AppSettings {
            font_family: Some("맑은 고딕".into()),
            ..Default::default()
        };
        assert_eq!(picked.selected_font(), Some("맑은 고딕"));
    }

    #[test]
    fn 손상된_설정은_그_자리만_기본값이_되고_세션은_살아남는다() {
        // T1 Edge Case — `#[serde(default)]`만으로는 **키가 없는 경우**만 막힌다.
        // 값이 있는데 타입이 어긋나면 그 오류가 `Session` 전체로 번져 워크스페이스까지 잃는다
        let session = to_session(
            window(),
            SidebarSession::default(),
            0,
            &sample(),
            empty_remote(),
        );
        let json = serde_json::to_string(&session).expect("직렬화");
        let settings_json = serde_json::to_string(&AppSettings::default()).expect("직렬화");

        for broken in [
            // bool 자리에 문자열
            r#"{"auto_start":"yes"}"#,
            // 설정이 객체가 아님
            r#""garbage""#,
            // 배열
            r#"[1,2,3]"#,
            // 일부 필드만 있고 그중 하나가 잘못된 타입
            r#"{"show_hidden":123,"tray_on_close":true}"#,
        ] {
            let damaged = json.replace(&settings_json, broken);
            assert_ne!(
                damaged, json,
                "테스트가 settings를 실제로 손상시키지 못했다"
            );
            let parsed = parse_session(&damaged)
                .unwrap_or_else(|| panic!("손상된 설정({broken})에 세션 전체가 폴백됐다"));
            assert_eq!(parsed.settings, AppSettings::default());
            // 세션의 나머지는 그대로여야 한다 — 이것이 이 테스트의 요점이다
            assert_eq!(restore(&parsed).len(), sample().len());
        }
    }

    #[test]
    fn 설정의_일부_필드만_있어도_나머지는_기본값이_된다() {
        let session = to_session(
            window(),
            SidebarSession::default(),
            0,
            &sample(),
            empty_remote(),
        );
        let json = serde_json::to_string(&session).expect("직렬화");
        let settings_json = serde_json::to_string(&AppSettings::default()).expect("직렬화");
        let partial = json.replace(&settings_json, r#"{"show_hidden":false}"#);

        let parsed = parse_session(&partial).expect("일부 필드만 있는 설정이 거부됐다");
        assert!(!parsed.settings.show_hidden, "적힌 값이 반영되지 않았다");
        assert!(
            parsed.settings.show_extensions,
            "없는 필드가 기본값이 아니다"
        );
        assert_eq!(parsed.settings.language, LanguageSetting::System);
    }

    #[test]
    fn 도크는_열려_있었어도_닫힌_채로_되살아난다() {
        let mut columns = crate::ui::dock::DockState::default().columns;
        columns = crate::ui::queue_panel::QueueColumns::from_saved(
            &{
                let mut widths = columns.to_saved(crate::remote::queue::QueueFilter::All);
                widths[1] = 333.0;
                widths
            },
            &[],
            &[],
        );
        let saved = crate::ui::dock::DockState {
            panel: Some(crate::ui::dock::DockPanel::Log),
            filter: crate::remote::queue::QueueFilter::Error,
            site: Some(SiteId(3)),
            columns: columns.clone(),
            queue_selection: std::iter::once(crate::remote::connection::TransferId(7)).collect(),
            queue_anchor: Some(crate::remote::connection::TransferId(7)),
        }
        .to_session();
        assert_eq!(saved.filter, "error");
        assert_eq!(saved.columns[1], 333.0, "큐 열 폭도 함께 담긴다");
        let back = crate::ui::dock::DockState::from_session(&saved);
        // 열려 있었어도 닫힌 채로 되살아난다 — 앱은 언제나 도크가 닫힌 채로 시작한다
        // (2026-08-21 사용자 요청 — FR-44). 트레이 복귀는 같은 실행이라 이 길을 타지 않는다
        assert_eq!(back.panel, None, "도크는 열려 있었어도 닫힌 채로 뜬다");
        assert_eq!(back.filter, crate::remote::queue::QueueFilter::Error);
        // 사이트 고르기는 담지 않는다 — 연결 없이 시작하므로 가리킬 곳이 없다
        assert_eq!(back.site, None);
        assert_eq!(back.columns, columns, "열 폭은 되살아난다");
        // 고른 행도 담지 않는다 (FR-36) — 큐가 완료·취소분을 버리고 되살아나므로
        // 번호를 되살려 봐야 가리킬 곳이 없다
        assert!(back.queue_selection.is_empty(), "고른 행이 세션을 건너왔다");
        assert_eq!(back.queue_anchor, None, "선택 기준점이 세션을 건너왔다");
    }

    #[test]
    fn 등록한_사이트는_저장하고_되살린다() {
        // 사용자 보고(2026-08-05): 앱을 껐다 켜면 연결 목록이 비어 있었다.
        // (직접 원인은 연결 시 앱이 죽어 `on_exit`가 돌지 못한 것이었지만, 저장·복원 경로
        //  자체가 사이트를 잃지 않는지 여기서 못 박는다)
        let (session, site) = session_with_site();
        let text = serde_json::to_string(&session).expect("직렬화");
        let back = crate::app::settings::parse_session(&text).expect("무결성 검사 통과");

        let names: Vec<&str> = back.sites.sites().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["배포 서버"], "저장본에서 사이트가 사라졌다");
        assert!(back.sites.get(site).is_some(), "사이트 식별자가 어긋났다");
        // 사이트를 담은 채 되살린 탭도 그 사이트를 그대로 가리킨다
        let restored = restore(&back);
        assert!(matches!(
            restored[0].panels[0].tabs[1],
            TabSpec::Remote { site: id, .. } if id == site
        ));
    }

    #[test]
    fn 즐겨찾기가_세션에_실려_왕복한다() {
        // plan D7 — 부르는 쪽(`collect_session`)이 스프레드라 컴파일러가 이 자리를 잡아 주지
        // 못한다. 그래서 덮는 규칙을 이 함수로 떼어 두고 시험이 직접 부른다
        let session = to_session(
            window(),
            SidebarSession::default(),
            0,
            &sample(),
            empty_remote(),
        );
        assert!(session.favorites.is_empty(), "`to_session`이 값을 지어냈다");

        let favorites = [PathBuf::from(r"D:\작업"), PathBuf::from(r"C:\Users")];
        let with = with_favorites(session, &favorites);
        assert_eq!(
            with.favorites,
            vec![r"D:\작업", r"C:\Users"],
            "차례가 뒤바뀌었다"
        );

        let text = serde_json::to_string(&with).expect("직렬화");
        let back = crate::app::settings::parse_session(&text).expect("왕복");
        assert_eq!(back.favorites, with.favorites, "저장했다 읽으니 달라졌다");
    }
}
