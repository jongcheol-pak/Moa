//! 앱이 **원격 서버를 다루는 배선** (FR-27~FR-46) — `ui::app`의 자식 모듈.
//!
//! 연결을 열고 닫는 일, 목록·트리를 조회해 결과를 패널에 잇는 일, 원격 메뉴의 명령을
//! 실행하는 일이 여기 있다. 프로토콜 자체는 `crate::remote`가 맡고 이 파일은 그것을
//! **앱 상태에 잇는 층**이다 — 어느 패널이 어느 연결을 보는지, 답이 왔을 때 무엇을
//! 갱신하는지가 이쪽 관심사다.
//!
//! **부모(`ui::app`)의 자식으로 둔 이유**: 이 배선은 `ExplorerApp`의 private 필드
//! (`manager`·`tree`·`sites`·`conns` 등)를 직접 만진다. 형제 모듈로 두면 그 필드를
//! `pub(crate)`로 넓혀야 하지만, 자식이면 가시성을 그대로 두고 나눌 수 있다.
//!
//! 같은 이름 확인(`app::transfer_conflict`)과 서로 부른다 — `poll_remote`가 서버 조회
//! 결과로 `settle_conflict`를 부르고, 그쪽 `start_transfer`는 `site_connection`을 쓴다.
//!
//! **전송을 거는 길은 셋이다** — 앱 안의 끌어다 놓기(FR-38)·원격 메뉴의 받기·올리기(FR-39)·
//! OS에서 끌어온 것을 원격 탭에 놓는 것(FR-61). 셋 다 `start_transfer` 한 앞문을 지나며,
//! 앞의 둘은 `ui::app`이 여기로 보내고 마지막은 `ui::app`의 `pump_os_drop`이 조립한다.

use super::transfer_conflict::conflict_names;
use super::{ExplorerApp, NOTICE_SECS, WorkspaceView};
use crate::app::layout::{PanelId, Rect as LayoutRect, SplitDir, SplitPlace};
use crate::app::workspace::WorkspaceId;
use crate::panel::tabs::TabPhase;
use crate::remote::connection::TransferDirection;
use crate::remote::connection::{
    ConnCommand, ConnEvent, ConnPhase, ConnectionId, ListSource, OpKind, TransferId,
};
use crate::remote::ftp::FtpSession;
use crate::remote::log::LogKind;
use crate::remote::sftp::SftpSession;
use crate::remote::sites::SiteStore;
use crate::remote::types::{LogonType, RemoteError, RemotePath, RemoteSession, SiteId};
use crate::remote::url::RemoteUrl;
use crate::ui::list_common::{DragItem, DropOutcome, DropTarget};
use crate::ui::menu::Command;
use crate::ui::panel::{PanelState, RemoteAction};
use crate::ui::remote_menu::{self, DialogOutcome, Permissions, RemoteMenuAction, RemoteTarget};
use crate::ui::tabs::TransferTargets;
use eframe::egui;
use std::collections::HashMap;
use std::path::PathBuf;

/// 단축키 명령을 원격 기능으로 옮긴다 (FR-12·plan D5) — 대응이 없으면 `None`.
///
/// **`Refresh`는 여기 없다** — `ExplorerApp::refresh_panel`이 이미 원격 탭을 가려
/// 그 패널의 목록을 다시 청한다. 여기에도 넣으면 같은 뜻에 길이 둘이 된다
/// (Design ③ — 새 실행 경로를 만들지 않는다).
///
/// **둘의 범위는 정확히 같지는 않다**: `refresh_panel`은 **그 패널 하나**를 다시 읽고
/// (`PanelState::request_remote_list`), `RemoteMenuAction::Refresh`는 **그 연결을 쓰는
/// 모든 패널**을 다시 읽는다(`ExplorerApp::request_remote_list`). 단축키는 활성 패널 하나에
/// 거는 것이므로 앞의 범위가 맞다 — 옆 패널까지 흔드는 것은 사용자가 청한 일이 아니다
/// (이 차이는 T11 이전부터 있던 것이며, 우클릭 메뉴의 `새로 고침`은 종전대로 뒤를 쓴다).
///
/// **클립보드 단축키는 원격에 대응 개념이 없다** — `Ctrl+C`·`Ctrl+X`·`Ctrl+V`가 옮기려는
/// 것은 파일 자체인데 원격 항목은 이 PC에 파일이 없다(전송은 큐가 담당한다). 그래서 이
/// 표에는 클립보드 명령이 없다.
///
/// **`경로로 복사`는 그것과 다른 일이다** — 파일이 아니라 **경로 문자열**을 담으므로 원격에도
/// 뜻이 있고 실제로 우클릭 메뉴에 있다(`RemoteMenuAction::CopyPath`). 다만 단축키가 없어
/// 이 표에는 오지 않는다 — 새 클립보드 형식을 만들지 않는다는 것은 그대로다.
///
/// 그 밖의 명령이 `None`인 것은 "원격에서 안 된다"는 뜻이 아니라 **탭·분할·보기처럼
/// 패널 층에서 이미 원격 탭에도 그대로 듣는다**는 뜻이다 — 그래서 새 명령이 늘어도
/// 기본값(종전 경로)이 안전하다
pub(super) fn remote_action_for(command: Command) -> Option<RemoteMenuAction> {
    match command {
        Command::Rename => Some(RemoteMenuAction::Rename),
        Command::Delete { .. } => Some(RemoteMenuAction::Delete),
        Command::NewFolder => Some(RemoteMenuAction::NewFolder),
        Command::ClipboardCopy | Command::ClipboardCut | Command::ClipboardPaste => None,
        _ => None,
    }
}

impl ExplorerApp {
    /// 지금 연결이 열려 있는 사이트들 — 사이드바의 상태 점이 이것으로 갈린다.
    ///
    /// 사이드바에 `ConnectionManager`를 통째로 넘기지 않는 이유: 사이드바가 알아야 하는 것은
    /// "이 사이트에 연결이 있는가" 하나뿐이라, 연결 계층을 알게 하면 의존만 넓어진다
    pub(super) fn connected_sites(&self) -> Vec<SiteId> {
        self.manager
            .ids()
            .iter()
            .filter_map(|id| self.manager.get(*id))
            .map(|connection| connection.site)
            .collect()
    }

    /// 연결이 **실패한 상태로 남아 있는** 사이트들 — 큐의 연결별 탭이 그 점을 빨강으로 그린다.
    ///
    /// `connected_sites`(연결 객체의 유무)와 다른 값이다: 실패한 연결도 탭을 닫기 전까지
    /// 매니저에 남아 있어, 유무로 가르면 실패한 사이트가 "연결됨"으로 보인다
    pub(super) fn failed_sites(&self) -> Vec<SiteId> {
        self.manager
            .ids()
            .iter()
            .filter_map(|id| self.manager.get(*id))
            .filter(|connection| matches!(connection.phase(), ConnPhase::Failed { .. }))
            .map(|connection| connection.site)
            .collect()
    }

    /// 원격 메뉴에서 고른 것을 실행한다 (FR-39).
    ///
    /// 대화가 필요한 것(이름 바꾸기·새 폴더·권한·삭제)은 여기서 **열기만** 하고, 실제 명령은
    /// 사용자가 확인한 뒤에 나간다 — 특히 삭제는 확인 없이 도는 경로를 만들지 않는다
    /// (plan Halt Forecast)
    pub(super) fn apply_remote_menu(
        &mut self,
        panel: PanelId,
        action: RemoteMenuAction,
        targets: Vec<RemoteTarget>,
    ) {
        // **연결보다 먼저 처리하는 줄이 하나 있다** — `경로로 복사`는 서버에 닿지 않아
        // 끊긴 탭에서도 열리므로(`menu_rows`의 `Reach::Local`), 아래 `conn` 게이트에 걸리면
        // 메뉴에서는 눌리는데 아무 일도 일어나지 않는다.
        //
        // 담기지 못해도 알리지 않는다 — 로컬 목록의 `복사`(FR-64)가 같은 클립보드에 같은
        // 실패 모드를 갖고 이미 그렇게 한다(`fs::clipboard::put`의 doc). 원격만 알리면
        // 같은 조작이 목록에 따라 다르게 굴어 보인다
        if action == RemoteMenuAction::CopyPath {
            let paths: Vec<RemotePath> = targets.into_iter().map(|item| item.path).collect();
            let _ = crate::fs::clipboard::put_text(&remote_menu::join_paths(&paths));
            return;
        }
        let Some(conn) = self.panel_conn(panel) else {
            return;
        };
        let site = self.manager.get(conn).map(|connection| connection.site);
        self.remote_ops.conn = Some(conn);
        // 경로만이 아니라 항목째로 든다 — 삭제가 파일·폴더에 따라 다른 명령을 보내야 한다
        self.remote_ops.targets = targets.clone();
        self.remote_ops.error = None;
        match action {
            // 받기·올리기는 **끌어다 놓기와 같은 길**로 보낸다 (FR-38) — 폴더를 훑는 것도,
            // 큐에 넣는 것도 이미 그쪽에 있다. 메뉴만 따로 두면 두 길이 곧 어긋난다
            RemoteMenuAction::Download => {
                // 받는 곳은 **받기 아이콘이 붙은 탭**이다 (FR-54) — 우클릭한 패널의 반대편이
                // 아니다. 대상이 없으면 메뉴 줄이 비활성이라 여기까지 오지 않는다
                let (Some(site), Some(dir)) = (site, self.download_dir()) else {
                    return;
                };
                let items = targets
                    .into_iter()
                    .map(|item| DragItem::Remote {
                        path: item.path,
                        is_dir: item.is_dir,
                        size: item.size,
                    })
                    .collect();
                self.start_transfer(DropOutcome {
                    items,
                    source_site: Some(site),
                    target: DropTarget::Local(dir),
                });
            }
            RemoteMenuAction::Upload => {
                // 올리는 곳은 **올리기 아이콘이 붙은 탭**, 올릴 것은 **받기 아이콘 탭의 선택**이다
                let (Some((site, dir)), selected) = (self.upload_dir(), self.upload_source())
                else {
                    return;
                };
                if selected.is_empty() {
                    return;
                }
                let items = selected
                    .into_iter()
                    .map(|(path, is_dir)| DragItem::Local { path, is_dir })
                    .collect();
                self.start_transfer(DropOutcome {
                    items,
                    source_site: None,
                    target: DropTarget::Remote { site, dir },
                });
            }
            // 위 `conn` 게이트 앞에서 이미 처리하고 돌아갔다
            RemoteMenuAction::CopyPath => {}
            RemoteMenuAction::Refresh => self.request_remote_list(conn),
            RemoteMenuAction::Rename => {
                self.remote_ops.name = self
                    .remote_ops
                    .targets
                    .first()
                    .and_then(|item| item.path.file_name())
                    .unwrap_or_default()
                    .to_owned();
                self.remote_ops.dialog = Some(RemoteDialog::Rename);
            }
            RemoteMenuAction::NewFolder => {
                self.remote_ops.name = String::new();
                self.remote_ops.dialog = Some(RemoteDialog::NewFolder);
            }
            RemoteMenuAction::Chmod => {
                // 서버가 알려 준 권한에서 시작한다 — 엉뚱한 기본값에서 출발하면 사용자가
                // 만지지 않은 비트까지 함께 바뀐다(spec 리뷰 N4). 안 알려 주는 서버에서만
                // 흔한 기본값을 쓴다
                let mode = targets.first().and_then(|item| item.mode);
                self.remote_ops.permissions = Permissions::from_mode(mode.unwrap_or(0o644));
                self.remote_ops.octal = self.remote_ops.permissions.to_octal_text();
                self.remote_ops.dialog = Some(RemoteDialog::Chmod);
            }
            RemoteMenuAction::Delete => {
                self.remote_ops.dialog = Some(RemoteDialog::Delete);
            }
        }
    }

    /// 원격 탭이면 이 명령을 원격 기능으로 보낸다 (FR-12·plan D5) — 보냈으면 참.
    ///
    /// 거짓이면 부르는 쪽이 **종전 로컬 처리를 그대로** 이어 간다. 원격 탭인데 조건이
    /// 맞지 않는 경우(고른 것이 0개·이름 바꾸기에 2개 이상·연결이 끊김)도 거짓인데,
    /// 그때 이어지는 로컬 처리는 원격 목록에서 아무 대상도 얻지 못해 결국 아무 일도
    /// 일어나지 않는다 — 원격 메뉴에서 그 줄이 비활성인 것과 같은 결과다.
    ///
    /// **활성 판정을 여기서 새로 적지 않는다** — 원격 메뉴가 쓰는 `menu_rows`를 그대로
    /// 물어본다(plan Halt Forecast). 두 곳에 적으면 메뉴에서는 흐린 줄이 단축키로는
    /// 눌리는 일이 생긴다
    pub(super) fn route_to_remote(&mut self, command: Command, target: Option<PanelId>) -> bool {
        let Some(action) = remote_action_for(command) else {
            return false;
        };
        self.ensure_active_view();
        let view_id = self.workspaces.active().id;
        let Some(view) = self.views.get(&view_id) else {
            return false;
        };
        let panel_id = target.unwrap_or(view.active);
        let Some(panel) = view.panels.get(&panel_id) else {
            return false;
        };
        if !panel.is_remote() {
            // 로컬 탭이다 — 종전 처리로 돌려보낸다
            return false;
        }
        let targets = panel.selected_remote();
        // 연결이 살아 있는가 — 끊긴 탭에서는 서버에 닿는 줄이 전부 비활성이다
        let connected = panel
            .active_conn()
            .and_then(|conn| self.manager.get(conn))
            .is_some_and(|connection| matches!(connection.phase(), ConnPhase::Ready));
        // `TransferTargets`는 받기·올리기 줄만 가리므로 여기서는 기본값으로 둔다 —
        // 이 함수가 옮기는 셋(이름 바꾸기·삭제·새 폴더)은 그 값을 보지 않는다
        let allowed = remote_menu::menu_rows(targets.len(), connected, TransferTargets::default())
            .into_iter()
            .any(|row| row.action == action && row.enabled);
        if !allowed {
            return false;
        }
        self.apply_remote_menu(panel_id, action, targets);
        true
    }

    /// 그 패널이 쓰는 연결
    pub(super) fn panel_conn(&self, panel: PanelId) -> Option<ConnectionId> {
        self.views
            .get(&self.workspaces.active().id)
            .and_then(|view| view.panels.get(&panel))
            .and_then(|panel| panel.active_conn())
    }

    /// 원격 대화들을 그리고 확인된 명령을 연결에 보낸다 (FR-39).
    ///
    /// **확인이든 취소든 대화는 그 자리에서 닫힌다** — 취소를 "아직 안 골랐다"와 같이 다루면
    /// 다음 프레임에 같은 대화가 다시 떠 빠져나올 수 없다 (spec 리뷰 M1)
    pub(super) fn show_remote_dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.remote_ops.dialog else {
            return;
        };
        let Some(conn) = self.remote_ops.conn else {
            self.remote_ops.dialog = None;
            return;
        };
        match dialog {
            RemoteDialog::Rename | RemoteDialog::NewFolder => {
                let rename = dialog == RemoteDialog::Rename;
                let title = if rename {
                    crate::i18n::rename()
                } else {
                    crate::i18n::menu_new_folder()
                };
                let outcome = remote_menu::show_name_dialog(
                    ctx,
                    title,
                    &mut self.remote_ops.name,
                    &mut self.remote_ops.error,
                );
                let Some(name) = settle_dialog(outcome, &mut self.remote_ops.dialog) else {
                    return;
                };
                let command = if rename {
                    self.remote_ops.targets.first().and_then(|item| {
                        Some(ConnCommand::Rename {
                            from: item.path.clone(),
                            to: item.path.parent()?.join(&name),
                        })
                    })
                } else {
                    self.remote_dir(conn)
                        .map(|dir| ConnCommand::Mkdir(dir.join(&name)))
                };
                if let Some(command) = command {
                    self.manager.send(conn, command);
                }
            }
            RemoteDialog::Chmod => {
                let outcome = remote_menu::show_chmod_dialog(
                    ctx,
                    &mut self.remote_ops.permissions,
                    &mut self.remote_ops.octal,
                );
                let Some(mode) = settle_dialog(outcome, &mut self.remote_ops.dialog) else {
                    return;
                };
                for item in std::mem::take(&mut self.remote_ops.targets) {
                    self.manager.send(
                        conn,
                        ConnCommand::Chmod {
                            path: item.path,
                            mode,
                        },
                    );
                }
            }
            RemoteDialog::Delete => {
                let outcome = remote_menu::show_delete_confirm(ctx, &self.remote_ops.targets);
                let Some(()) = settle_dialog(outcome, &mut self.remote_ops.dialog) else {
                    return;
                };
                for item in std::mem::take(&mut self.remote_ops.targets) {
                    // 파일이냐 폴더냐는 목록이 알려 준 그대로 쓴다 — 폴더에 파일 삭제 명령을
                    // 보내면 서버가 거절할 뿐이고, 그 사유가 로그와 상태 줄에 남는다 (D22)
                    self.manager
                        .send(conn, delete_command(item.path, item.is_dir));
                }
            }
        }
    }

    /// 그 연결을 보고 있는 패널의 현재 원격 폴더
    pub(super) fn remote_dir(&self, conn: ConnectionId) -> Option<RemotePath> {
        self.views
            .values()
            .flat_map(|view| view.panels.values())
            .find(|panel| panel.active_conn() == Some(conn))
            .and_then(|panel| panel.remote_dir())
    }

    /// 트리가 청한 하위 조회를 연결에 보낸다 (T24).
    ///
    /// 캐시가 "아직 안 읽었다"고 할 때만 실제로 나간다 — 펼침이 반복돼도 서버에는 한 번만
    /// 묻는다 (Acceptance ②)
    pub(super) fn request_tree_children(&mut self, conn: ConnectionId, path: RemotePath) {
        let Some(cache_generation) = self.tree_cache.begin(conn, &path) else {
            return;
        };
        // 번호 공간을 나눌 필요가 없다 — 출처를 `ListSource::Tree`가 실어 나르므로 패널
        // 조회와 번호가 겹쳐도 답이 뒤바뀌지 않는다
        self.next_tree_list += 1;
        let seq = self.next_tree_list;
        self.pending_tree_lists
            .insert(seq, (conn, path.clone(), cache_generation));
        self.manager.send(
            conn,
            ConnCommand::List {
                source: ListSource::Tree { seq },
                path,
                quiet: false,
            },
        );
    }

    /// 원격 폴더를 훑어 달라고 워커에 청한다 (FR-38).
    ///
    /// 화면이 한 겹씩 요청해 가며 훑지 않는 이유: 목록 응답 라우팅과 뒤섞이고 프레임마다
    /// 상태를 이어 붙여야 한다. 워커는 어차피 블로킹이라 한 번에 끝내는 편이 단순하다
    pub(super) fn request_tree(&mut self, site: SiteId, root: RemotePath, local_dir: PathBuf) {
        let Some(conn) = self.site_connection(site) else {
            return;
        };
        let generation = self.next_tree;
        self.next_tree += 1;
        self.pending_trees
            .insert(generation, (site, root.clone(), local_dir));
        self.manager
            .send(conn, ConnCommand::ListTree { generation, root });
    }

    /// 그 사이트의 연결 하나 — 여럿이면 먼저 연 것을 쓴다
    pub(super) fn site_connection(&self, site: SiteId) -> Option<ConnectionId> {
        self.manager
            .ids()
            .iter()
            .copied()
            .find(|id| self.manager.get(*id).is_some_and(|conn| conn.site == site))
    }

    /// 로그 화면이 보여 줄 연결 — **지금 보고 있는 원격 탭의 것**이 먼저다.
    ///
    /// 그 탭이 로컬이면 마지막으로 연 연결을 보인다: 로그를 여는 까닭은 대개 방금 무슨 일이
    /// 있었는지 보려는 것이라, 아무것도 안 보이는 것보다 최근 연결을 보이는 편이 쓸모 있다
    pub(super) fn log_connection(&self) -> Option<ConnectionId> {
        // 연결별 탭에서 고른 사이트가 있으면 그 사이트의 연결이 먼저다 — 사용자가 고른 것이다.
        // 그 사이트의 연결이 이미 접혔으면 아래의 자동 선택으로 돌아간다
        if let Some(site) = self.dock.site
            && let Some(conn) = self.site_connection(site)
        {
            return Some(conn);
        }
        let active = self
            .views
            .get(&self.workspaces.active().id)
            .and_then(|view| view.panels.get(&view.active))
            .and_then(|panel| panel.active_conn());
        active.or_else(|| self.manager.ids().last().copied())
    }

    /// 지금 **연결이 열려 있는** 사이트들 — 연결별 탭이 큐가 비어 있어도 이들을 세운다
    pub(super) fn connected_conn_sites(&self) -> Vec<SiteId> {
        let mut sites: Vec<SiteId> = self
            .manager
            .ids()
            .iter()
            .filter_map(|id| self.manager.get(*id).map(|connection| connection.site))
            .collect();
        sites.dedup();
        sites
    }

    /// 이 연결을 아직 쓰는 탭이 있는가 — 지금 보이지 않는 워크스페이스까지 본다.
    /// 한 연결을 여러 패널이 나눠 쓸 수 있어, 한 곳이 놓았다고 접으면 나머지가 끊긴다
    pub(super) fn conn_in_use(&self, conn: ConnectionId) -> bool {
        self.views
            .values()
            .any(|view| view.panels.values().any(|panel| panel.uses_conn(conn)))
    }

    /// 그 사이트로 열려 있는 연결 **전부** — 사이트 하나가 연결 셋을 쓴다 (FR-37 탐색 1 + 전송 2).
    ///
    /// 위 `site_connection`은 아무 하나면 되는 일(조회를 보낼 상대 고르기)에 쓰고, 이쪽은
    /// 사이트를 통째로 거둘 때처럼 **하나도 남기면 안 되는** 일에 쓴다
    pub(super) fn site_connections(&self, site: SiteId) -> Vec<ConnectionId> {
        self.manager
            .ids()
            .iter()
            .copied()
            .filter(|id| {
                self.manager
                    .get(*id)
                    .is_some_and(|connection| connection.site == site)
            })
            .collect()
    }

    /// 사이드바 목록에서 지운 사이트의 **실행 중 상태를 통째로 걷어낸다** (FR-29).
    ///
    /// 사이트 기록은 남기고(사이트 관리자에 그대로 있다) 연결·원격 탭·전송만 거둔다.
    /// 그러지 않으면 지웠는데도 전송 큐 화면의 연결별 탭에 그 서버가 계속 선다 —
    /// 그 탭은 **큐에 항목이 있거나 연결이 열려 있으면** 서기 때문이다(`ui::queue_panel`).
    ///
    /// **순서가 이 함수의 핵심이다** (plan D2):
    /// 1. 확인 대기를 먼저 버린다 — 연결을 닫으면 그것이 「겹침 없음」으로 큐에 들어간다
    /// 2. 원격 탭을 닫는다 — 패널이 비면 닫고, 마지막 패널이면 로컬 탭으로 되돌린다(FR-2)
    /// 3. **연결이 살아 있는 동안** 전송을 취소하고 큐를 비운다 — `TransferRunner::cancel`은
    ///    워커에 명령을 보내야 받다 만 `.part`를 정리 대상으로 잡는다
    /// 4. 연결을 접고 곧바로 `forget_connection` — 워커가 사라지면 완료 통지가 오지 않아
    ///    그 `.part`가 영영 남는다
    pub(super) fn detach_site(
        &mut self,
        site: SiteId,
        area: LayoutRect,
        ctx: &egui::Context,
        now: f64,
    ) {
        let name = self.sites.get(site).map(|record| record.name.clone());
        self.sites.hide(site);

        // 1. 이 사이트로 물어 둔 확인은 답을 받을 곳이 없다
        self.drop_site_conflicts(site);

        // 2. 그 사이트를 가리키는 원격 탭을 모든 워크스페이스에서 닫는다
        for view in self.views.values_mut() {
            let mut emptied: Vec<PanelId> = Vec::new();
            for (id, panel) in view.panels.iter_mut() {
                // 그 사이트 탭만 남아 마지막 하나를 닫지 못한 패널이다
                if panel.close_site_tabs(site, ctx) {
                    emptied.push(*id);
                }
            }
            for panel in emptied {
                view.close_panel(panel, area);
                // 마지막 패널은 닫히지 않는다 (FR-2) — 그때는 로컬 시작 폴더 탭을 세우고
                // 남은 원격 탭을 마저 닫는다(세션 복원이 사이트 잃은 탭을 되돌리는 것과 같다)
                if let Some(state) = view.panels.get_mut(&panel) {
                    state.new_tab(ctx);
                    state.close_site_tabs(site, ctx);
                }
            }
        }

        // 2-1. 아직 한 번도 열지 않은 워크스페이스는 뷰가 없다 (D1 지연 생성) — 저장된
        //      상태에서 직접 걷어낸다. 그러지 않으면 그 탭이 그대로 다시 저장돼
        //      (`collect_session`) 다음에 그 워크스페이스를 열 때 지운 사이트가 되살아난다.
        //      **빼지 않고 로컬 시작 폴더로 바꾼다** — 탭 목록이 비면 그 패널을 되살릴 수
        //      없고(`PanelState::from_tabs`가 `None`), 활성 탭 번호도 어긋난다
        for state in self.restored.values_mut() {
            crate::ui::session::detach_site_from_state(state, site, &crate::ui::app::start_dir());
        }

        // 3. 연결이 살아 있는 동안 전송을 그만두고 큐에서 뺀다
        let ids: Vec<TransferId> = self
            .queue
            .site_items(site)
            .iter()
            .map(|item| item.id)
            .collect();
        for id in &ids {
            self.runner.cancel(&self.manager, *id);
        }
        self.queue.remove(&ids);

        // 4. 연결을 접는다. 접힌 뒤에는 워커의 통지가 오지 않으므로 실행기가 붙들고 있던
        //    자리(취소 뒤 정리를 기다리던 `.part`)를 그 자리에서 거둔다
        for conn in self.site_connections(site) {
            self.release_conn(conn);
            self.runner.forget_connection(conn);
        }

        // 5. 늦게 도착할 워커 결과를 버릴 표시를 세우고, 도크가 그 사이트를 고른 채면 `전체`로
        self.detached_sites.insert(site);
        if self.dock.site == Some(site) {
            self.dock.site = None;
        }
        self.persist_session();
        if let Some(name) = name {
            self.toast
                .show(crate::i18n::dynamic::site_removed(&name), now);
        }
    }

    /// 연결 하나를 접고 그에 딸린 대기 자리를 함께 지운다 (FR-32).
    ///
    /// 워커와 소켓이 여기서 회수된다. 그 연결에 청해 둔 훑기는 답이 오지 않으므로
    /// 기다리는 자리도 함께 지운다 (T24 Acceptance ④) — 남기면 영영 기다린다
    pub(super) fn release_conn(&mut self, conn: ConnectionId) {
        let site = self.manager.get(conn).map(|connection| connection.site);
        // 접기 **전에** 물어 둔 확인을 거둔다 — `manager.close`가 연결을 지우고 나면
        // 그 사이트를 알 길이 없어, 답을 기다리던 전송이 대화도 못 뜨고 큐에도 못 들어간
        // 채 남는다. 연결이 스스로 끊기는 길(`ConnPhase::Closed|Failed`)과 달리 이쪽은
        // 워커의 단계 통지를 거치지 않는다
        self.abandon_conflict_lists(conn);
        self.manager.close(conn);
        self.tree_cache.forget(conn);
        self.pending_tree_lists
            .retain(|_, (waiting, _, _)| *waiting != conn);
        if let Some(site) = site
            && self.site_connection(site).is_none()
        {
            self.pending_trees
                .retain(|_, (waiting, _, _)| *waiting != site);
        }
    }

    /// 연결 워커가 올린 소식을 화면에 반영한다 (NFR-10 — UI 스레드는 채널만 확인한다).
    ///
    /// **모든 워크스페이스의 패널**에 뿌린다. 이벤트는 한 번만 오므로 지금 보이지 않는
    /// 워크스페이스를 건너뛰면 그쪽 탭이 영영 옛 단계로 남는다
    pub(super) fn poll_remote(&mut self, now: f64) {
        for (conn, event) in self.manager.poll() {
            match event {
                ConnEvent::Phase(phase) => {
                    let tab_phase = to_tab_phase(&phase);
                    for view in self.views.values_mut() {
                        for panel in view.panels.values_mut() {
                            panel.set_phase_for(conn, &tab_phase);
                        }
                    }
                    // 연결되면 곧바로 첫 목록을 청한다 — 그러지 않으면 연결만 되고 화면이 빈 채 남는다
                    // 트리도 새로 읽게 한다 — `다시 시도`는 같은 연결 번호로 다시 서므로,
                    // 끊긴 동안 실패로 담긴 노드가 남으면 트리는 그 오류만 계속 보인다
                    // (실패도 "읽은 것"이라 `TreeCache::begin`이 다시 묻지 않는다)
                    if matches!(phase, ConnPhase::Ready) {
                        self.tree_cache.forget(conn);
                        self.request_remote_list(conn);
                    }
                    // 연결이 끊기면 물어 둔 확인의 답이 영영 오지 않는다 — 그 전송을 붙잡아
                    // 두지 말고 확인을 포기하고 보낸다 (D10). 서버가 거절하면 큐가 알린다
                    if matches!(phase, ConnPhase::Failed { .. } | ConnPhase::Closed) {
                        self.abandon_conflict_lists(conn);
                    }
                }
                // 출처가 갈래를 정한다 — 종전에는 세 갈래를 **차례로** 걷어냈고 그 순서가
                // 곧 정확성이었다(확인 조회를 먼저 빼지 않으면 엉뚱한 패널이 그 목록을 자기
                // 것으로 그렸다). 이제는 타입이 그 일을 하므로 순서에 기대지 않는다
                ConnEvent::Listed {
                    source,
                    path,
                    entries,
                } => match source {
                    // 같은 이름 확인이 청한 답이면 여기서 끝난다 — 목록 화면·트리는 모른다
                    ListSource::Conflict { id } => {
                        if let Some((_, names)) = self.conflict_lists.remove(&id) {
                            let existing: Vec<String> =
                                entries.into_iter().map(|entry| entry.name).collect();
                            // 올리는 곳은 대개 POSIX라 대소문자를 가린다 — 가리지 않으면 헛경고다 (D5)
                            self.settle_conflict(id, conflict_names(&names, &existing, false));
                        }
                    }
                    // 트리가 청한 답이면 캐시로 간다 — 목록 화면은 이것을 모른다
                    ListSource::Tree { seq } => {
                        if let Some((conn, path, cache_generation)) =
                            self.pending_tree_lists.remove(&seq)
                        {
                            let mut entries = entries;
                            sort_tree_children(&mut entries);
                            self.tree_cache.fill(conn, cache_generation, &path, entries);
                        }
                    }
                    ListSource::Panel {
                        workspace,
                        panel,
                        seq,
                    } => {
                        let ExplorerApp { views, icons, .. } = self;
                        // 요청 하나에 답 하나다 — 받을 패널을 먼저 고르고 목록은 **복사 없이** 한 번만 넘긴다
                        if let Some(target) = list_target(views, workspace, panel, conn) {
                            target.apply_remote_listed(seq, &path, entries, icons);
                        }
                    }
                },
                // 훑기 결과 — 찾은 파일을 통째로 큐에 넣는다 (FR-38)
                ConnEvent::TreeListed {
                    generation,
                    root,
                    files,
                } => {
                    if let Some((site, _, local_dir)) = self.pending_trees.remove(&generation) {
                        for (path, size) in files {
                            // 서버 쪽 구조를 로컬에도 그대로 만든다 — 뿌리 폴더 이름부터 붙인다
                            let relative = path
                                .as_str()
                                .strip_prefix(root.as_str())
                                .unwrap_or(path.file_name().unwrap_or_default())
                                .trim_start_matches('/');
                            let root_name = root.file_name().unwrap_or_default();
                            let local = local_dir.join(root_name).join(relative.replace('/', "\\"));
                            self.queue.enqueue(
                                site,
                                TransferDirection::Download,
                                local,
                                path,
                                size,
                            );
                        }
                    }
                }
                // 전송 소식은 실행기가 큐에 반영한다 (FR-37)
                ConnEvent::TransferProgress { id, transferred } => {
                    self.runner
                        .on_progress(&mut self.queue, id, transferred, now)
                }
                ConnEvent::TransferDone { id, result } => {
                    self.runner.on_done(
                        &mut self.queue,
                        id,
                        result.map_err(|err| err.to_string()),
                        now,
                    );
                    // 올린 것이 끝났으면 그 폴더를 다시 읽을 자리로 표시해 둔다 (FR-37).
                    //
                    // **완료 판정은 워커의 결과가 아니라 큐에서 다시 읽는다** — `on_done`이
                    // 성공을 실패로 뒤집는 길이 있어(받은 파일의 이름 바꾸기 실패) 워커가 준
                    // 값만 보면 실제 상태와 어긋난다
                    if let Some((site, dir)) = self.queue.get(id).and_then(relist_target) {
                        self.relist.mark(site, dir);
                    }
                }
                // 조회 실패 — 트리가 청한 것이면 그 노드에만 사유를 남기고(T24 Edge Case),
                // 패널이 청한 것이면 **옮기기를 무르고** 사유를 상태 줄에 남긴다 (F-7 리뷰 B2).
                // 무르지 않으면 주소창은 새 폴더를, 목록은 이전 폴더를 가리킨 채 갈라진다
                ConnEvent::ListFailed { source, detail } => match source {
                    // 확인은 안전장치이지 관문이 아니다 — 물어보지 못했다고 전송을 막지
                    // 않는다. 사유는 서버 로그에 남는다 (D10)
                    ListSource::Conflict { id } => {
                        if self.conflict_lists.remove(&id).is_some() {
                            self.settle_conflict(id, Vec::new());
                        }
                    }
                    ListSource::Tree { seq } => {
                        if let Some((conn, path, cache_generation)) =
                            self.pending_tree_lists.remove(&seq)
                        {
                            self.tree_cache.fail(conn, cache_generation, &path, detail);
                        }
                    }
                    ListSource::Panel {
                        workspace,
                        panel,
                        seq,
                    } => self.revert_remote_move(conn, workspace, panel, seq, detail, now),
                },
                // 파일 작업의 답 — 성공하면 목록을 다시 읽고, 실패하면 사유를 남긴다 (FR-39)
                ConnEvent::OpDone { op, result } => self.on_op_done(conn, op, result, now),
                // 서버 로그는 `Connection`이 자기 버퍼에 이미 쌓는다(화면은 T20이 만든다)
                _ => {}
            }
        }
    }

    /// 파일 작업의 결과를 반영한다 (FR-39).
    ///
    /// 성공하면 **목록을 다시 읽는다** — 서버가 바뀐 것을 앱이 짐작해 그리면 실제와 어긋난다.
    /// 실패는 상태 줄과 로그 양쪽에 남긴다: 상태 줄은 곧 사라지므로 되짚을 자리가 필요하다.
    /// 서버가 `SITE CHMOD`를 모르는 것은 흔한 일이라 이때도 앱은 그대로 돈다 (D22)
    pub(super) fn on_op_done(
        &mut self,
        conn: ConnectionId,
        op: OpKind,
        result: Result<(), RemoteError>,
        now: f64,
    ) {
        match op_outcome(op, result) {
            OpOutcome::Relist => self.request_remote_list(conn),
            OpOutcome::Notice(text) => {
                self.manager.note(conn, LogKind::Error, text.clone());
                self.notice = Some((text, now + NOTICE_SECS));
            }
            OpOutcome::Ignore => {}
        }
    }

    /// 조회가 실패한 패널의 옮기기를 무르고 사유를 알린다 (F-7 리뷰 B2).
    ///
    /// **보이는 목록과 경로가 어긋나지 않게** 하는 것이 목적이다 — 어긋난 채로 두면 그 위에서
    /// 연 원격 메뉴가 사용자가 보는 것과 다른 경로에 삭제·권한 변경을 건다
    pub(super) fn revert_remote_move(
        &mut self,
        conn: ConnectionId,
        workspace: u32,
        panel: u32,
        seq: u64,
        detail: String,
        now: f64,
    ) {
        // **청한 패널 하나만** 본다 — 종전에는 그 연결을 쓰는 패널을 전부 훑었고, 같은
        // 사이트를 두 패널에 나란히 열면 한쪽의 실패가 다른 쪽까지 되돌렸다
        let mut reverted = false;
        // **자동 재조회의 실패인가** (FR-67) — 그렇다면 알림을 띄우지 않는다
        let mut quiet = false;
        if let Some(target) = list_target(&mut self.views, workspace, panel, conn) {
            quiet = target.is_quiet_request(seq);
            reverted = target.revert_remote_path(seq);
        }
        let text = if reverted {
            crate::i18n::dynamic::remote_open_failed(&detail)
        } else {
            crate::i18n::dynamic::remote_list_failed(&detail)
        };
        self.manager.note(conn, LogKind::Error, text.clone());
        // **자동 재조회가 실패했을 때는 배너를 띄우지 않는다** (FR-67) — 연결이 불안정하면
        // 사용자가 아무것도 누르지 않았는데 주기마다 알림이 뜬다. 사유는 바로 위 서버
        // 로그에 남으므로 사라지지 않는다(D9의 「실패는 남긴다」가 그 자리다)
        if !quiet {
            self.notice = Some((text, now + NOTICE_SECS));
        }
    }

    /// 원격 위치가 바뀐 패널들이 새 위치의 목록을 청한다 (T24 Acceptance ⑤).
    ///
    /// 옮긴 쪽(트리·상위 이동)은 연결을 모르고 명령을 보낼 수단도 없다 — 깃발만 세워 두고
    /// 여기서 거둔다
    pub(super) fn list_moved_panels(&mut self) {
        self.relist_panels(|panel| panel.take_remote_dirty());
    }

    /// 조건에 맞는 패널들이 목록을 다시 청한다 — 세 갈래가 함께 쓰는 순회.
    ///
    /// 뼈대(모든 워크스페이스 × 모든 패널 → 조건 → `request_remote_list`)가 같고 **조건만**
    /// 다르다: 옮긴 패널(위)·그 연결을 쓰는 패널(`request_remote_list`)·전송 목적지를 보는
    /// 패널(`pump_relist`). 조건을 클로저로 받으면 세 곳의 다른 점이 호출부에 그대로 남는다.
    ///
    /// **조건이 `&mut`를 받는 이유**: 옮김 판정(`take_remote_dirty`)이 깃발을 세워 둔 것을
    /// **거두는** 조작이라 불변 참조로는 표현할 수 없다
    fn relist_panels(&mut self, mut wants: impl FnMut(&mut PanelState) -> bool) {
        let ExplorerApp { views, manager, .. } = self;
        // 요청에 자리를 실어야 해 `values_mut` 대신 `iter_mut`로 돈다 — 패널은 자기 키를
        // 들고 있지 않으므로 여기서 꺼내 넘긴다
        for (workspace, view) in views.iter_mut() {
            for (id, panel) in view.panels.iter_mut() {
                if wants(panel) {
                    panel.request_remote_list(*workspace, *id, manager);
                }
            }
        }
    }

    /// 전송이 끝나 표시해 둔 폴더들을 실제로 다시 읽는다 (FR-37).
    ///
    /// **대상은 연결이 아니라 「사이트 + 폴더」로 고른다** — 한 사이트가 탭마다 연결을 따로
    /// 열어(FR-37의 탐색 1 + 전송 2) 전송을 처리한 연결이 패널의 연결과 다를 수 있다.
    /// 연결로 고르면 갱신이 새어 나가고, 반대로 그 연결로 다른 폴더를 보는 패널까지
    /// 헛되이 다시 읽게 된다
    pub(super) fn pump_relist(&mut self, now: f64) {
        // 표시된 것이 없으면 큐를 훑지 않는다 — 이 자리는 매 프레임 불리고 큐는 1만 건까지 간다.
        // **표시가 남아 있는 동안에는 훑는다**: 전송이 끝난 사이트는 아래에서 곧바로 비워지고,
        // 아직 도는 사이트만 간격(`RELIST_MIN_INTERVAL`)만큼 남아 그 사이에 훑기가 반복된다
        if self.relist.is_empty() {
            return;
        }
        let busy: std::collections::HashSet<SiteId> = self
            .queue
            .items()
            .iter()
            .filter(|item| item.state.is_pending())
            .map(|item| item.site)
            .collect();
        let ready = self.relist.take_ready(&busy, now);
        if ready.is_empty() {
            return;
        }
        for (site, dir) in ready {
            self.relist_panels(|panel| {
                panel.active_site() == Some(site) && panel.remote_dir().as_ref() == Some(&dir)
            });
        }
    }

    /// 그 연결을 활성 탭으로 쓰는 패널들이 목록을 다시 청한다.
    ///
    /// 위 `pump_relist`와 **고르는 기준이 다르다** — 이쪽은 연결 하나에 딸린 패널 전부이고
    /// (연결이 서거나 파일 작업이 끝났을 때처럼 그 연결의 상태 자체가 바뀐 경우),
    /// 그쪽은 사이트와 폴더가 모두 맞는 패널만이다(전송의 목적지 한 곳만 바뀐 경우)
    pub(super) fn request_remote_list(&mut self, conn: ConnectionId) {
        self.relist_panels(|panel| panel.active_conn() == Some(conn));
    }

    /// 원격 단계 화면에서 고른 조치를 실행한다 (인벤토리 #18~21)
    pub(super) fn apply_remote_action(&mut self, target: PanelId, action: RemoteAction) {
        // `다시 연결`만은 **연결이 없을 때** 누르는 것이라 아래 연결 가드보다 앞에 둔다
        if action == RemoteAction::Reconnect {
            self.reconnect_panel(target);
            return;
        }
        let Some(conn) = self
            .views
            .get(&self.workspaces.active().id)
            .and_then(|view| view.panels.get(&target))
            .and_then(|panel| panel.active_conn())
        else {
            return;
        };
        match action {
            // 워커는 살아 있고 명령만 다시 받는다 — 새 연결을 열면 탭이 옛 연결을 가리킨 채 남는다
            RemoteAction::Retry => {
                self.manager.send(conn, ConnCommand::Connect);
            }
            RemoteAction::CancelConnect => {
                self.manager.send(conn, ConnCommand::Disconnect);
            }
            // 방금 실패한 그 사이트를 고른 채 연다 — 고치러 온 사용자가 목록에서 다시 찾지
            // 않게 한다 (인벤토리 #19)
            RemoteAction::OpenSettings => {
                let site = self.manager.get(conn).map(|connection| connection.site);
                self.site_manager.open(&self.sites, site);
            }
            // 서버 로그 패널은 T20이 만든다 — 그때 이 자리에서 연다
            RemoteAction::ViewLog => {}
            // 위에서 이미 처리하고 돌아갔다
            RemoteAction::Reconnect => {}
        }
    }

    /// 세션에서 되살아난 원격 탭을 그 사이트로 다시 연결한다 (사용자 보고 2026-08-13).
    ///
    /// 사이트를 새로 여는 길(`connect_site`)을 그대로 탄다 — 재시작 뒤에는 워커가 없어
    /// `Retry`처럼 명령만 보낼 상대가 없다. 연결이 서면 그 다음은 사이드바에서 사이트를 열
    /// 때와 같은 흐름이다(단계가 `Ok`가 되면 앱이 목록을 청한다)
    pub(super) fn reconnect_panel(&mut self, target: PanelId) {
        let Some(view) = self.views.get_mut(&self.workspaces.active().id) else {
            return;
        };
        let Some(site) = view
            .panels
            .get(&target)
            .and_then(|panel| panel.active_site())
        else {
            return;
        };
        // `connect_site`는 **활성 패널의 활성 탭**에 연결을 붙인다 — 버튼을 누른 패널이
        // 활성이 아니면 엉뚱한 탭이 붙으므로 먼저 맞춘다(누른 패널이 활성이 되는 것이 자연스럽다)
        view.active = target;
        self.connect_site(site);
    }

    /// 주소창에 적은 원격 주소로 새 탭을 연다 (FR-34).
    ///
    /// **이미 등록된 서버면 그 사이트를 쓴다** — 프로토콜·호스트·포트가 같으면 같은 서버이고,
    /// 그때마다 사이트를 새로 만들면 목록이 같은 서버로 뒤덮인다.
    /// 처음 보는 주소는 **숨긴 사이트**로 들인다: 연결에 필요한 설정(사용자·포트)을 담을 곳이
    /// 있어야 하지만, 한 번 적어 본 주소가 사이드바에 눌러앉지는 않게 한다(사이트 관리자에는 보인다)
    pub(super) fn open_remote_url(&mut self, target: PanelId, url: RemoteUrl, area: LayoutRect) {
        let port = url.effective_port();
        let site = match matching_site(&self.sites, &url) {
            Some(site) => site,
            None => {
                let site = self.sites.add(&url.host);
                if let Some(record) = self.sites.get_mut(site) {
                    record.protocol = url.protocol;
                    record.host = url.host.clone();
                    record.port = port;
                    if let Some(user) = &url.user {
                        record.logon = LogonType::Normal;
                        record.user = user.clone();
                    }
                }
                // 주소로 한 번 열어 본 서버가 사이드바에 눌러앉지 않게 한다
                self.sites.hide(site);
                self.persist_session();
                site
            }
        };
        self.open_site_tab_at(site, Some(target), url.path, area);
    }

    /// 사이트를 그 패널의 **새 원격 탭**으로 열고 연결을 건다 (FR-33·FR-34·FR-38).
    ///
    /// 진입점 셋(탭 스트립 드롭다운·주소창 URL·사이드바 드래그)이 모두 여기로 착지한다 —
    /// 여는 방법마다 다른 경로를 두면 셋이 조금씩 다르게 동작하게 된다.
    ///
    /// 사이트가 그 사이 지워졌으면 아무 일도 하지 않는다 (plan Edge Case: 드래그 도중 삭제)
    pub(super) fn open_site_tab(
        &mut self,
        site: SiteId,
        target: Option<PanelId>,
        area: LayoutRect,
    ) {
        // 서버가 정한 홈에서 시작한다 — 연결이 서면 워커가 실제 위치를 알려 준다
        self.open_site_tab_at(site, target, RemotePath::root(), area);
    }

    /// 사이트를 **그 패널의 새 탭**으로 연다 — 나누지 않는다.
    ///
    /// 탭 스트립의 `연결 사이트를 새 탭으로` 드롭다운과 스트립에 끌어다 놓기가 이 길을 쓴다.
    /// 이름 그대로 새 탭이며, `+`(새 탭)와 같은 자리에 열려야 한다 (사용자 보고) — 사이드바·
    /// 사이트 관리자에서 여는 길만 좌우로 나눠 연다 (`open_site_tab` — FR-35)
    pub(super) fn open_site_tab_here(&mut self, site: SiteId, target: Option<PanelId>) {
        if self.sites.get(site).is_none() {
            return;
        }
        let view = self.ensure_active_view();
        let opened = target.unwrap_or(view.active);
        let Some(panel) = view.panels.get_mut(&opened) else {
            return;
        };
        panel.open_remote_tab(site, RemotePath::root());
        // 연결은 **활성 패널의 활성 탭**에 붙는다 — 다른 패널에서 연 경우까지 맞춰 둔다
        view.active = opened;
        self.connect_site(site);
    }

    /// 위와 같되 시작 위치를 지정한다 — 주소에 경로가 함께 적힌 경우(`sftp://host/pub`).
    ///
    /// **연결은 활성 패널을 좌우로 나눠 오른쪽에 연다** (FR-35·README) — 로컬과 원격을 나란히
    /// 두고 주고받는 것이 이 기능의 쓰임이라, 같은 패널에서 열면 그 배치를 사용자가 매번 손으로
    /// 만들어야 한다. **나눌 자리가 없으면 현재 패널의 새 탭**으로 물러선다 (Acceptance ④) —
    /// 조용히 아무 일도 일어나지 않으면 사용자는 연결 자체가 실패한 줄 안다
    pub(super) fn open_site_tab_at(
        &mut self,
        site: SiteId,
        target: Option<PanelId>,
        path: RemotePath,
        area: LayoutRect,
    ) {
        if self.sites.get(site).is_none() {
            return;
        }
        let view = self.ensure_active_view();
        let source = target.unwrap_or(view.active);
        // 기존 분할 구조는 그대로 두고 대상 패널만 나눈다 (Acceptance ②)
        let created = view.split_panel(source, SplitDir::Horizontal, SplitPlace::After, area);
        let opened = created.unwrap_or(source);
        let Some(panel) = view.panels.get_mut(&opened) else {
            return;
        };
        if created.is_some() {
            // 새로 나온 패널에는 연결만 남긴다 — 시작 폴더 탭은 사용자가 연 적이 없다
            panel.open_remote_tab_only(site, path);
        } else {
            // 나눌 자리가 없어 현재 패널로 물러선 길 — 쓰던 탭은 그대로 두고 하나 더 연다
            panel.open_remote_tab(site, path);
        }
        // 방금 만든 탭이 활성이라 연결이 그 탭에 붙는다
        self.connect_site(site);
    }

    /// 사이트에 연결하고 활성 원격 탭을 그 연결에 붙인다 (FR-28).
    ///
    /// **세션 조립이 화면 쪽에 있는 이유**: SFTP는 지문 확인 통로가 필요하고 그 통로는 화면이
    /// 쥔다 — 연결 관리자가 세션을 만들면 `remote`가 화면을 알아야 한다 (T4 결정).
    ///
    /// 사이트를 새 탭으로 여는 진입점(사이드바·주소창·드롭다운)은 T12·T13이 붙인다
    pub fn connect_site(&mut self, site: SiteId) -> Option<ConnectionId> {
        let record = self.sites.get(site)?.clone();
        // 다시 여는 사이트는 더 이상 「지운 것」이 아니다 — 표시를 풀지 않으면 그 사이트로
        // 펼친 올리기가 영영 큐에 들어가지 않는다 (FR-29 — `detach_site` 5단계)
        self.detached_sites.remove(&site);
        // 익명 로그온이면 비밀번호가 없다 — 서버가 관례대로 무시한다.
        // **키 파일 로그온이면 넘기는 것이 키 암호다**(D10) — 세션은 둘째 인자를
        // 「인증 비밀」로 받으며 어느 쪽인지는 `record.logon`이 말해 준다 (FR-66)
        // 포괄 갈래(`_`)를 쓰지 않는다 — 다음에 인증 수단이 늘면 여기서 **컴파일 오류로**
        // 드러나야 한다. 포괄로 두면 새 수단이 조용히 비밀번호 갈래로 떨어진다
        let secret = match record.logon {
            LogonType::KeyFile => self.sites.key_passphrase(site),
            LogonType::Anonymous | LogonType::Normal => self.sites.password(site),
        }
        .unwrap_or_default();
        // 세션은 **껍데기만** 만들어 넘긴다 — 소켓도 지문 표도 워커 스레드가 연결할 때 연다
        // (AGENTS: UI 스레드에서 블로킹 I/O 금지)
        let session: Box<dyn RemoteSession> = if record.protocol.is_ssh() {
            Box::new(SftpSession::new(Some(
                self.hostkey.prompt(record.address()),
            )))
        } else {
            Box::new(FtpSession::new())
        };
        let id = self.manager.open(&record, secret, session);
        let view = self.ensure_active_view();
        let active = view.active;
        if let Some(panel) = view.panels.get_mut(&active) {
            panel.attach_conn(id);
        }
        Some(id)
    }
}

/// 같은 사이트에 잇달아 다시 묻지 않을 최소 간격(초) — 오래 걸리는 전송 중에도
/// 이 간격으로는 목록이 따라온다 (FR-37).
///
/// **이 값이 없으면 둘 중 하나가 된다**: 건마다 물으면 수천 건 업로드에서 서버 왕복이
/// 파일 수에 비례하고, 큐가 빌 때만 물으면 오래 걸리는 전송 내내 화면이 그대로다
const RELIST_MIN_INTERVAL: f64 = 2.0;

/// 전송이 끝나 다시 읽어야 할 원격 폴더들 (FR-37).
///
/// **순수 상태다** — 연결도 패널도 모르고 "무엇을 언제 다시 물을지"만 안다. 그래서 시점
/// 판정(아래 `take_ready`)을 서버 없이 시험할 수 있다. 실제 조회는 `pump_relist`가 한다
#[derive(Debug, Default)]
pub(super) struct RelistPending {
    /// 다시 읽어야 할 자리들. 같은 폴더로 100건이 끝나도 항목은 하나다
    dirty: std::collections::HashSet<(SiteId, RemotePath)>,
    /// 사이트마다 마지막으로 조회를 보낸 시각.
    ///
    /// **사이트별로 두는 이유**: 하나로 두면 사이트 A에 보낸 것이 사이트 B의 간격까지
    /// 먹어, B가 조건을 채웠는데도 내주지 않는다
    last_sent: std::collections::HashMap<SiteId, f64>,
}

impl RelistPending {
    /// 이 자리를 다시 읽어야 한다고 표시한다
    pub(super) fn mark(&mut self, site: SiteId, dir: RemotePath) {
        self.dirty.insert((site, dir));
    }

    pub(super) fn is_empty(&self) -> bool {
        self.dirty.is_empty()
    }

    /// 지금 물어도 되는 자리들을 꺼낸다 — 꺼낸 것은 목록에서 빠진다.
    ///
    /// `busy`는 **대기·진행 중인 전송이 남은 사이트들**이다. 그 사이트는 아직 끝나지
    /// 않았으므로 간격(`RELIST_MIN_INTERVAL`)을 채웠을 때만 내주고, 전송이 다 끝난
    /// 사이트는 곧바로 내준다 — 마지막 한 번은 지체 없이 화면에 반영돼야 한다
    pub(super) fn take_ready(
        &mut self,
        busy: &std::collections::HashSet<SiteId>,
        now: f64,
    ) -> Vec<(SiteId, RemotePath)> {
        let mut ready: Vec<(SiteId, RemotePath)> = Vec::new();
        self.dirty.retain(|(site, dir)| {
            let waited = self
                .last_sent
                .get(site)
                // 이 사이트에 한 번도 보낸 적이 없으면 기다릴 것이 없다 —
                // 대량 전송의 첫 완료가 곧바로 화면에 보인다
                .is_none_or(|last| now - last >= RELIST_MIN_INTERVAL);
            if !busy.contains(site) || waited {
                ready.push((*site, dir.clone()));
                return false;
            }
            true
        });
        for (site, _) in &ready {
            self.last_sent.insert(*site, now);
        }
        ready
    }
}

/// 전송 하나가 끝났을 때 다시 읽어야 할 자리 — 없으면 `None` (FR-37).
///
/// **올리기만 대상이다** — 받기는 로컬 폴더 감시(`ui::panel`의 `DirWatcher`)가 이미
/// 갱신하므로 여기서 다루면 같은 일을 두 번 한다. 실패·진행 중인 것도 대상이 아니다
pub(super) fn relist_target(
    item: &crate::remote::queue::TransferItem,
) -> Option<(SiteId, RemotePath)> {
    if !item.state.is_done() || item.direction != TransferDirection::Upload {
        return None;
    }
    // 서버 루트에 바로 올린 것은 그 위가 없다 — 루트 자신이 다시 읽을 자리다
    Some((
        item.site,
        item.remote.parent().unwrap_or_else(RemotePath::root),
    ))
}

/// 원격 파일 작업이 띄운 대화의 상태 (FR-39).
///
/// **대상 경로를 대화가 뜰 때 붙잡아 둔다** — 대화가 떠 있는 동안 목록이 다시 읽히거나
/// 선택이 바뀔 수 있는데, 그때 다시 읽으면 사용자가 고른 것과 **다른 항목**에 명령이 간다
#[derive(Debug, Default)]
pub(super) struct RemoteOps {
    /// 어느 연결에 보낼 것인가
    conn: Option<ConnectionId>,
    /// 지금 뜬 대화
    dialog: Option<RemoteDialog>,
    /// 대화가 다루는 대상들 — 폴더 여부까지 들고 있어야 삭제가 명령을 고를 수 있다
    targets: Vec<RemoteTarget>,
    /// 이름 입력값과 그 오류
    name: String,
    error: Option<String>,
    /// 권한 대화의 상태
    permissions: Permissions,
    octal: String,
}

/// 그 출처가 가리키는 패널을 찾아 준다 — 워크스페이스·패널이 모두 맞고, 그 패널이
/// **지금도 그 연결을 보고 있어야** 한다.
///
/// **`PanelId`만으로는 못 가린다** — 워크스페이스마다 `0`부터 다시 매겨져 전역 유일하지
/// 않은데 응답은 모든 워크스페이스를 대상으로 오기 때문이다.
///
/// **연결 검사를 여기 둔 이유**: 자리는 맞아도 그 사이 다른 사이트로 옮겨 갔을 수 있고,
/// 그때 답을 받으면 남의 서버 목록을 그린다.
///
/// `ExplorerApp`의 메서드가 아니라 자유 함수인 것은 **시험이 부를 수 있게** 하기 위해서다 —
/// 그 구조체는 `eframe::CreationContext`가 있어야 만들어져 단위 시험에서 세울 수 없다
pub(super) fn list_target(
    views: &mut HashMap<WorkspaceId, WorkspaceView>,
    workspace: u32,
    panel: u32,
    conn: ConnectionId,
) -> Option<&mut PanelState> {
    views
        .get_mut(&WorkspaceId(workspace))?
        .panels
        .get_mut(&PanelId(panel))
        .filter(|panel| panel.active_conn() == Some(conn))
}

/// 트리에 보일 차례로 줄을 세운다 — **목록과 같은 규칙**이라야 화면이 두 벌로 갈리지 않는다.
/// (`remote::tree_cache`는 `panel`을 모르므로 정렬은 이쪽에서 맞춰 넘긴다)
pub(super) fn sort_tree_children(entries: &mut [crate::remote::types::RemoteEntry]) {
    entries.sort_by(|a, b| {
        crate::panel::file_list::compare_rows(a, "", b, "", crate::panel::file_list::SortKey::Name)
    });
}

/// 상태 줄에 남길 실패 문장 — 사용자가 시키지 않은 작업(`Cwd`·`Disconnect`)은 알리지 않는다
pub(super) fn op_failure_message(op: OpKind, error: &RemoteError) -> Option<String> {
    let detail = error.to_string();
    match op {
        OpKind::Mkdir => Some(crate::i18n::dynamic::op_mkdir_failed(&detail)),
        OpKind::Remove | OpKind::Rmdir => Some(crate::i18n::dynamic::op_delete_failed(&detail)),
        OpKind::Rename => Some(crate::i18n::dynamic::op_rename_failed(&detail)),
        OpKind::Chmod => Some(crate::i18n::dynamic::op_chmod_failed(&detail)),
        OpKind::Cwd | OpKind::Disconnect => None,
    }
}

/// 파일 작업의 답을 어떻게 다룰지 (FR-39) — 화면·연결을 건드리지 않고 판정만 한다
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OpOutcome {
    /// 서버 쪽이 바뀌었다 — 목록을 다시 읽는다
    Relist,
    /// 실패 사유를 상태 줄과 로그에 남긴다
    Notice(String),
    /// 사용자가 시킨 작업이 아니라 알리지 않는다
    Ignore,
}

/// 작업 결과의 처리 방법을 정한다.
///
/// 실패해도 **사유만 남기고 앱은 그대로 돈다** — `SITE CHMOD`를 모르는 FTP 서버가 흔한데
/// 그때마다 앱이 멈추거나 연결이 끊기면 쓸 수 없다 (D22)
pub(super) fn op_outcome(op: OpKind, result: Result<(), RemoteError>) -> OpOutcome {
    // 사용자가 시킨 작업이 아니면 성공도 실패도 알리지 않는다 — 목록도 다시 읽지 않는다
    // (`Disconnect` 뒤에 목록을 걸면 이미 닫힌 연결에 말을 거는 셈이다)
    if matches!(op, OpKind::Cwd | OpKind::Disconnect) {
        return OpOutcome::Ignore;
    }
    match result {
        Ok(()) => OpOutcome::Relist,
        Err(err) => match op_failure_message(op, &err) {
            Some(message) => OpOutcome::Notice(message),
            None => OpOutcome::Ignore,
        },
    }
}

/// 대화의 결론을 상태에 반영한다 (FR-39).
///
/// 확인이면 그 값을 내주고 대화를 닫는다. **취소도 똑같이 닫는다** — 이 한 줄이 빠져서
/// 취소 단추가 아무 일도 하지 않았다(spec 리뷰 M1). 아직 고르지 않았으면 그대로 둔다
pub(super) fn settle_dialog<T>(
    outcome: DialogOutcome<T>,
    dialog: &mut Option<RemoteDialog>,
) -> Option<T> {
    match outcome {
        DialogOutcome::Pending => None,
        DialogOutcome::Confirmed(value) => {
            *dialog = None;
            Some(value)
        }
        DialogOutcome::Cancelled => {
            *dialog = None;
            None
        }
    }
}

/// 확인을 마친 삭제가 보낼 명령 (FR-39).
///
/// **이 함수를 부르는 곳은 확인 대화가 `Some`을 돌려준 자리 하나뿐이다** — 메뉴에서 곧바로
/// 삭제로 가는 길은 없다(plan Halt Forecast).
///
/// 폴더냐 파일이냐로 갈린다 — 폴더에는 `RMD`/`rmdir`, 파일에는 `DELE`/`unlink`가 나간다.
/// **둘 다 재귀가 아니다**: 안이 빈 폴더가 아니면 서버가 거절하고 그 사유가 로그에 남는다
pub(super) fn delete_command(path: RemotePath, is_dir: bool) -> ConnCommand {
    if is_dir {
        ConnCommand::Rmdir(path)
    } else {
        ConnCommand::Remove(path)
    }
}

/// 지금 뜬 원격 대화의 종류
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RemoteDialog {
    Rename,
    NewFolder,
    Chmod,
    Delete,
}

/// 이 주소와 **같은 서버**로 이미 등록된 사이트 — 프로토콜·호스트·포트가 모두 같아야 한다.
///
/// 호스트 대소문자는 구분하지 않는다(DNS가 그렇다). 사용자 이름은 견주지 않는다 —
/// 같은 서버에 다른 계정으로 붙는 것은 흔하고, 그때마다 사이트를 새로 만들면 목록이 뒤덮인다
pub(super) fn matching_site(sites: &SiteStore, url: &RemoteUrl) -> Option<SiteId> {
    let port = url.effective_port();
    sites
        .sites()
        .iter()
        .find(|record| {
            record.protocol == url.protocol
                && record.host.eq_ignore_ascii_case(&url.host)
                && record.port == port
        })
        .map(|record| record.id)
}

/// 연결 단계를 탭이 보이는 단계로 옮긴다.
///
/// 둘을 따로 두는 이유는 탭이 **연결 없이도** 존재하기 때문이다(빈 탭·세션 복원 직후) —
/// `Idle`·`Closed`는 "이 탭에는 지금 연결이 없다"와 같은 뜻이라 `New`로 모은다
pub(super) fn to_tab_phase(phase: &ConnPhase) -> TabPhase {
    match phase {
        ConnPhase::Idle | ConnPhase::Closed => TabPhase::New,
        ConnPhase::Connecting => TabPhase::Connecting,
        ConnPhase::Ready => TabPhase::Ok,
        ConnPhase::Failed { detail, kind } => TabPhase::Error {
            message: detail.clone(),
            kind: *kind,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::remote::queue::{TransferItem, TransferState};
    use std::collections::HashSet;

    /// 원격 탭 하나를 가진 패널 — 연결까지 붙인다
    fn 원격_패널(conn: ConnectionId, path: &str) -> PanelState {
        let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\테스트"));
        panel.open_remote_tab_only(SiteId(1), RemotePath::new(path));
        assert!(panel.attach_conn(conn), "원격 탭이어야 연결이 붙는다");
        panel
    }

    /// 뷰 하나를 세우고 그 안에 패널들을 심는다.
    ///
    /// **`layout`은 갱신하지 않는다** — 생성자가 만든 리프 하나를 그대로 두고 `panels`만
    /// 바꾸므로 둘이 어긋난 뷰가 된다. `list_target`이 `panels`만 보기 때문에 이 시험들에는
    /// 영향이 없지만, **`layout`을 읽는 함수에는 이 헬퍼를 그대로 쓰면 안 된다**
    fn 뷰(패널들: Vec<(PanelId, PanelState)>) -> WorkspaceView {
        let mut view = WorkspaceView::new(std::path::PathBuf::from(r"C:\테스트"));
        view.panels.clear();
        for (id, panel) in 패널들 {
            view.panels.insert(id, panel);
        }
        view
    }

    #[test]
    fn 같은_워크스페이스의_두_패널은_서로의_답을_가져가지_않는다() {
        // 흡수된 대장 항목 「원격 조회 요청에 패널 식별자」가 지적한 바로 그 자리 —
        // 같은 연결을 나눠 쓰는 두 패널이 **같은 경로를 향해 같은 번호로** 대기해도
        // 청한 쪽만 답을 받아야 한다. 종전에는 세대 번호만 봤고 그 번호가 패널마다
        // 따로 세어져 겹쳤다
        let conn = ConnectionId(1);
        let mut views = HashMap::new();
        views.insert(
            WorkspaceId(0),
            뷰(vec![
                (PanelId(0), 원격_패널(conn, "/var/www")),
                (PanelId(1), 원격_패널(conn, "/var/www")),
            ]),
        );

        let 첫째 = list_target(&mut views, 0, 0, conn).map(|p| p as *const PanelState);
        let 둘째 = list_target(&mut views, 0, 1, conn).map(|p| p as *const PanelState);
        assert!(
            첫째.is_some() && 둘째.is_some(),
            "둘 다 찾을 수 있어야 한다"
        );
        assert_ne!(
            첫째, 둘째,
            "같은 패널을 두 번 돌려줬다 — 자리를 가리지 못한다"
        );
    }

    #[test]
    fn 다른_워크스페이스의_같은_패널_번호도_갈린다() {
        // **`PanelId`는 전역 유일하지 않다** — `LayoutTree::new()`가 워크스페이스마다
        // `PanelId(0)`부터 다시 매기는데 응답 라우팅은 모든 워크스페이스를 대상으로 온다.
        // 패널 번호만 보면 이 둘이 같아 보여 한쪽의 답이 다른 쪽으로 간다
        let conn = ConnectionId(1);
        let mut views = HashMap::new();
        views.insert(
            WorkspaceId(0),
            뷰(vec![(PanelId(0), 원격_패널(conn, "/var/www"))]),
        );
        views.insert(
            WorkspaceId(1),
            뷰(vec![(PanelId(0), 원격_패널(conn, "/var/www"))]),
        );

        let 앞 = list_target(&mut views, 0, 0, conn).map(|p| p as *const PanelState);
        let 뒤 = list_target(&mut views, 1, 0, conn).map(|p| p as *const PanelState);
        assert!(
            앞.is_some() && 뒤.is_some(),
            "두 워크스페이스 모두 찾을 수 있어야 한다"
        );
        assert_ne!(앞, 뒤, "워크스페이스가 다른데 같은 패널을 돌려줬다");
    }

    #[test]
    fn 그_사이_다른_연결로_옮겨_갔으면_답을_주지_않는다() {
        // 자리는 맞아도 그 패널이 다른 사이트를 보고 있으면 남의 서버 목록을 그리게 된다
        let mut views = HashMap::new();
        views.insert(
            WorkspaceId(0),
            뷰(vec![(PanelId(0), 원격_패널(ConnectionId(1), "/var/www"))]),
        );

        assert!(list_target(&mut views, 0, 0, ConnectionId(1)).is_some());
        assert!(
            list_target(&mut views, 0, 0, ConnectionId(2)).is_none(),
            "다른 연결의 답을 이 패널이 받았다"
        );
    }

    #[test]
    fn 워크스페이스나_패널이_닫혔으면_조용히_버린다() {
        // 답이 늦게 왔는데 그 사이 자리가 사라진 경우 — 패닉이 아니라 `None`이어야 한다
        let conn = ConnectionId(1);
        let mut views = HashMap::new();
        views.insert(
            WorkspaceId(0),
            뷰(vec![(PanelId(0), 원격_패널(conn, "/var/www"))]),
        );

        assert!(
            list_target(&mut views, 9, 0, conn).is_none(),
            "없는 워크스페이스인데 무언가를 돌려줬다"
        );
        assert!(
            list_target(&mut views, 0, 9, conn).is_none(),
            "없는 패널인데 무언가를 돌려줬다"
        );
    }

    #[test]
    fn 원격_탭에서_잇는_것은_셋뿐이다() {
        // plan D5 — 원격에 **이미 있는 기능만** 잇는다
        assert_eq!(
            remote_action_for(Command::Rename),
            Some(RemoteMenuAction::Rename)
        );
        assert_eq!(
            remote_action_for(Command::NewFolder),
            Some(RemoteMenuAction::NewFolder)
        );
        for 영구 in [false, true] {
            assert_eq!(
                remote_action_for(Command::Delete { permanent: 영구 }),
                Some(RemoteMenuAction::Delete),
                "영구={영구} — 원격에는 휴지통이 없어 둘 다 같은 삭제다"
            );
        }
    }

    #[test]
    fn 클립보드는_원격으로_가지_않는다() {
        // 원격에는 대응 개념이 없다 — 전송은 큐가 담당한다 (plan Out of Scope)
        for command in [
            Command::ClipboardCopy,
            Command::ClipboardCut,
            Command::ClipboardPaste,
        ] {
            assert_eq!(remote_action_for(command), None, "{command:?}");
        }
    }

    #[test]
    fn 새로_고침은_이_길로_오지_않는다() {
        // `refresh_panel`이 이미 원격 탭을 가려 `request_remote_list`를 보낸다 —
        // 여기에도 넣으면 같은 일에 길이 둘이 된다
        assert_eq!(remote_action_for(Command::Refresh), None);
    }

    #[test]
    fn 패널_층에서_이미_듣는_명령은_옮기지_않는다() {
        // 탭·탐색·분할·보기는 원격 탭에서도 종전대로 동작한다 — 옮길 것이 없다
        for command in [
            Command::NewTab,
            Command::CloseTab,
            Command::Back,
            Command::Forward,
            Command::Up,
            Command::ClosePanel,
            Command::NewFile,
            Command::ToggleSidebar,
        ] {
            assert_eq!(remote_action_for(command), None, "{command:?}");
        }
    }

    #[test]
    fn 라우팅이_기대는_활성_규칙은_원격_메뉴의_것이다() {
        // `route_to_remote`가 판정을 새로 적지 않고 `menu_rows`에 물어본다는 것이
        // 이 시험이 지키는 계약이다 — 메뉴에서 흐린 줄이 단축키로는 눌리면 안 된다
        let 열렸나 = |고른_수: usize, 연결: bool, action: RemoteMenuAction| {
            remote_menu::menu_rows(고른_수, 연결, TransferTargets::default())
                .into_iter()
                .any(|row| row.action == action && row.enabled)
        };
        // 이름 바꾸기는 **정확히 하나**일 때만 — 새 이름은 하나뿐이다
        assert!(열렸나(1, true, RemoteMenuAction::Rename));
        assert!(!열렸나(0, true, RemoteMenuAction::Rename));
        assert!(!열렸나(2, true, RemoteMenuAction::Rename));
        // 삭제는 하나 이상
        assert!(열렸나(1, true, RemoteMenuAction::Delete));
        assert!(!열렸나(0, true, RemoteMenuAction::Delete));
        // 새 폴더는 고른 것이 없어도 뜻이 있다 — 지금 폴더가 대상이다
        assert!(열렸나(0, true, RemoteMenuAction::NewFolder));
        // 연결이 끊기면 셋 다 닫힌다
        for action in [
            RemoteMenuAction::Rename,
            RemoteMenuAction::Delete,
            RemoteMenuAction::NewFolder,
        ] {
            assert!(!열렸나(1, false, action.clone()), "{action:?}");
        }
    }

    fn 올린_항목(site: u32, remote: &str, state: TransferState) -> TransferItem {
        TransferItem {
            id: crate::remote::connection::TransferId(1),
            site: SiteId(site),
            direction: TransferDirection::Upload,
            local: PathBuf::from(r"C:\work\app.js"),
            remote: RemotePath::new(remote),
            size: 10,
            state,
            started_at: None,
            finished_at: None,
            attempts: 0,
            retry_at: None,
        }
    }

    #[test]
    fn 성공한_올리기는_그_폴더를_다시_읽을_자리로_준다() {
        // Acceptance ⓐ
        let item = 올린_항목(1, "/var/www/app.js", TransferState::Done);
        assert_eq!(
            relist_target(&item),
            Some((SiteId(1), RemotePath::new("/var/www")))
        );
        // 서버 루트에 바로 올린 것은 루트 자신이 대상이다 (Edge Case)
        let 루트 = 올린_항목(1, "/app.js", TransferState::Done);
        assert_eq!(relist_target(&루트), Some((SiteId(1), RemotePath::root())));
    }

    #[test]
    fn 끝나지_않았거나_실패한_전송은_다시_읽지_않는다() {
        // Acceptance ⓑ — 실패를 다시 읽으면 없는 파일이 생긴 것처럼 보이지는 않지만 헛왕복이다
        for state in [
            TransferState::Wait,
            TransferState::Active { sent: 5, speed: 1 },
            TransferState::Error {
                message: "550".to_owned(),
            },
        ] {
            assert_eq!(relist_target(&올린_항목(1, "/var/www/app.js", state)), None);
        }
    }

    #[test]
    fn 받기는_다시_읽을_자리가_아니다() {
        // Acceptance ⓒ — 로컬은 폴더 감시가 이미 갱신한다
        let mut item = 올린_항목(1, "/var/www/app.js", TransferState::Done);
        item.direction = crate::remote::connection::TransferDirection::Download;
        assert_eq!(relist_target(&item), None);
    }

    #[test]
    fn 전송이_남은_사이트는_간격을_채워야_내준다() {
        // Acceptance ⓓⓔ
        let mut pending = RelistPending::default();
        pending.mark(SiteId(1), RemotePath::new("/pub"));
        let busy: HashSet<SiteId> = [SiteId(1)].into_iter().collect();

        // 한 번도 보낸 적이 없으면 곧바로 내준다 — 대량 전송의 첫 완료가 화면에 보인다
        assert_eq!(
            pending.take_ready(&busy, 100.0),
            vec![(SiteId(1), RemotePath::new("/pub"))]
        );

        // 방금 보냈으므로 간격 안에서는 내주지 않는다 (ⓓ)
        pending.mark(SiteId(1), RemotePath::new("/pub"));
        assert!(pending.take_ready(&busy, 101.0).is_empty());
        assert!(!pending.is_empty(), "내주지 않은 것은 그대로 남는다");

        // 간격이 지나면 진행 중이어도 한 번 내준다 (ⓔ)
        assert_eq!(
            pending.take_ready(&busy, 102.0),
            vec![(SiteId(1), RemotePath::new("/pub"))]
        );
    }

    #[test]
    fn 전송이_다_끝난_사이트는_간격을_기다리지_않는다() {
        // 마지막 한 번은 지체 없이 반영돼야 한다
        let mut pending = RelistPending::default();
        pending.mark(SiteId(1), RemotePath::new("/pub"));
        let busy: HashSet<SiteId> = [SiteId(1)].into_iter().collect();
        assert_eq!(pending.take_ready(&busy, 100.0).len(), 1);

        pending.mark(SiteId(1), RemotePath::new("/pub"));
        // 큐가 비었다(busy 아님) → 간격 안이어도 내준다
        assert_eq!(pending.take_ready(&HashSet::new(), 100.5).len(), 1);
    }

    #[test]
    fn 같은_폴더로_여러_건이_끝나도_한_번만_묻는다() {
        // Acceptance ⓕ
        let mut pending = RelistPending::default();
        for _ in 0..100 {
            pending.mark(SiteId(1), RemotePath::new("/pub"));
        }
        assert_eq!(pending.take_ready(&HashSet::new(), 100.0).len(), 1);
        assert!(pending.is_empty());
    }

    #[test]
    fn 간격은_사이트마다_따로_센다() {
        // Acceptance ⓖ — 하나로 두면 A에 보낸 것이 B의 창을 먹는다
        let mut pending = RelistPending::default();
        pending.mark(SiteId(1), RemotePath::new("/a"));
        let busy: HashSet<SiteId> = [SiteId(1), SiteId(2)].into_iter().collect();
        assert_eq!(pending.take_ready(&busy, 100.0).len(), 1);

        // 같은 순간 사이트 2가 표시돼도 그쪽은 자기 이력만 본다
        pending.mark(SiteId(2), RemotePath::new("/b"));
        assert_eq!(
            pending.take_ready(&busy, 100.1),
            vec![(SiteId(2), RemotePath::new("/b"))]
        );
    }

    #[test]
    fn 같은_서버의_주소는_사이트를_새로_만들지_않는다() {
        // 주소창으로 같은 서버를 여러 번 열어도 사이트 목록이 그 서버로 뒤덮이면 안 된다
        use crate::remote::types::Protocol;
        use crate::remote::url::parse_remote_url;

        let mut sites = SiteStore::new();
        let id = sites.add("배포 서버");
        if let Some(record) = sites.get_mut(id) {
            record.protocol = Protocol::Sftp;
            record.host = "example.test".to_owned();
            record.port = 22;
        }

        let 같은_서버 = parse_remote_url("sftp://example.test/pub").expect("파싱");
        assert_eq!(matching_site(&sites, &같은_서버), Some(id));
        // 호스트 대소문자는 구분하지 않는다
        let 대문자 = parse_remote_url("sftp://EXAMPLE.test").expect("파싱");
        assert_eq!(matching_site(&sites, &대문자), Some(id));
        // 계정이 달라도 같은 서버다
        let 다른_계정 = parse_remote_url("sftp://other@example.test").expect("파싱");
        assert_eq!(matching_site(&sites, &다른_계정), Some(id));

        // 포트·프로토콜·호스트가 다르면 다른 서버다
        for 다른 in [
            "sftp://example.test:2222",
            "ftp://example.test",
            "sftp://other.test",
        ] {
            let url = parse_remote_url(다른).expect("파싱");
            assert_eq!(matching_site(&sites, &url), None, "{다른}");
        }
    }

    #[test]
    fn 연결_단계는_탭_단계로_옮겨진다() {
        // 탭은 연결 없이도 존재하므로 둘을 따로 둔다 — `Idle`·`Closed`는 "이 탭에 연결이 없다"와
        // 같은 뜻이라 `New`로 모인다. 실패는 **사유를 잃지 않아야** 실패 화면이 그것을 보인다
        assert_eq!(to_tab_phase(&ConnPhase::Idle), TabPhase::New);
        assert_eq!(to_tab_phase(&ConnPhase::Closed), TabPhase::New);
        assert_eq!(to_tab_phase(&ConnPhase::Connecting), TabPhase::Connecting);
        assert_eq!(to_tab_phase(&ConnPhase::Ready), TabPhase::Ok);
        assert_eq!(
            to_tab_phase(&ConnPhase::Failed {
                detail: "530 Login incorrect".to_owned(),
                kind: crate::remote::types::FailureKind::Auth
            }),
            TabPhase::Error {
                message: "530 Login incorrect".to_owned(),
                kind: crate::remote::types::FailureKind::Auth
            }
        );
    }

    #[test]
    fn 작업이_성공하면_목록을_다시_읽고_실패하면_사유가_남는다() {
        // Acceptance ② — 성공 응답 뒤에는 서버를 다시 읽는다. 앱이 짐작해 그리면 실제와 어긋난다
        for op in [OpKind::Mkdir, OpKind::Rename, OpKind::Remove, OpKind::Rmdir] {
            assert_eq!(op_outcome(op, Ok(())), OpOutcome::Relist, "{op:?}");
        }
        // Acceptance ④ — SITE CHMOD를 모르는 서버(D22)의 답은 사유로 남고, 그것으로 끝이다
        let unsupported = RemoteError::Unsupported {
            operation: "SITE CHMOD".to_owned(),
            detail: "500 Unknown command".to_owned(),
        };
        let OpOutcome::Notice(text) = op_outcome(OpKind::Chmod, Err(unsupported)) else {
            panic!("실패가 사유로 남지 않았다");
        };
        assert!(text.starts_with("권한을 변경하지 못했습니다"), "{text}");
        assert!(text.contains("SITE CHMOD"), "서버 원문이 빠졌다: {text}");
        // 사용자가 시키지 않은 작업까지 알리면 상태 줄이 잡음으로 찬다
        assert_eq!(
            op_outcome(
                OpKind::Cwd,
                Err(RemoteError::Protocol {
                    detail: "x".to_owned()
                })
            ),
            OpOutcome::Ignore
        );
    }

    #[test]
    fn 삭제는_폴더_여부에_따라_다른_명령이_된다() {
        // 목록이 알려 준 폴더 여부가 그대로 명령이 된다 — 사용자에게 묻지 않는다
        let path = RemotePath::new("/var/www/old");
        assert_eq!(
            delete_command(path.clone(), false),
            ConnCommand::Remove(path.clone())
        );
        assert_eq!(delete_command(path.clone(), true), ConnCommand::Rmdir(path));
    }

    #[test]
    fn 취소한_대화는_그_자리에서_닫힌다() {
        // spec 리뷰 M1의 회귀 방지선 — 취소를 "아직 안 골랐다"와 같이 다루면 다음 프레임에
        // 같은 대화가 다시 떠 빠져나올 수 없다
        let mut dialog = Some(RemoteDialog::Rename);
        assert_eq!(
            settle_dialog(DialogOutcome::<String>::Pending, &mut dialog),
            None
        );
        assert_eq!(dialog, Some(RemoteDialog::Rename), "고르기 전에 닫혔다");

        assert_eq!(
            settle_dialog(DialogOutcome::<String>::Cancelled, &mut dialog),
            None
        );
        assert_eq!(dialog, None, "취소했는데 대화가 남았다");

        let mut dialog = Some(RemoteDialog::Chmod);
        assert_eq!(
            settle_dialog(DialogOutcome::Confirmed(0o755), &mut dialog),
            Some(0o755)
        );
        assert_eq!(dialog, None, "확인 뒤에도 대화가 남았다");
    }
}
