//! 자유 분할 레이아웃 — 분할 트리를 화면에 배치하고 스플리터를 드래그한다 (FR-1·FR-2).
//!
//! 배치 계산은 `app::layout::LayoutTree`(순수 로직·단위 테스트 완비)가 그대로 하고,
//! 이 파일은 그 결과를 egui 좌표로 옮겨 그리고 입력을 되돌려주는 일만 한다.
//! egui의 `SidePanel`류 도킹 컨테이너는 쓰지 않는다 — 중첩 자유 분할을 표현하지 못한다.
use crate::app::favorites::{FavoriteAction, FavoriteEntry};
use crate::app::layout::{LayoutTree, PanelId, Rect as LayoutRect, SplitDir};
use crate::fs::drives::DriveRow;
use crate::fs::icons::IconCache;
use crate::remote::connection::ConnectionId;
use crate::remote::url::RemoteUrl;
use crate::ui::icon_tex::IconTextures;
use crate::ui::list_common::DropOutcome;
use crate::ui::menu::{Command, PanelMenuState};
use crate::ui::panel::{
    DisplayRules, MenuRequest, PanelOutcome, PanelState, RemoteAction, RemoteMenuPick,
    RenameRequest,
};
use crate::ui::remote_states::RemoteView;
use crate::ui::tabs::TransferTargets;
use crate::ui::theme;
use crate::ui::tree::TreeRequest;
use eframe::egui;
use std::collections::HashMap;
use std::path::PathBuf;

/// 스플리터 히트 영역을 좌우(상하)로 넓히는 여유.
/// 경계선은 얇아야 하고 잡기는 쉬워야 한다 — **조작 영역만** 넓힌다(그리기는 그 두께 그대로)
const SPLITTER_GRAB_PAD: f32 = 3.0;

/// 패널 테두리 두께 — 일반 경계와 활성 강조가 같은 두께를 쓴다
const PANE_BORDER_WIDTH: f32 = 1.0;

pub fn to_egui_rect(r: LayoutRect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(r.x as f32, r.y as f32),
        egui::vec2(r.w.max(0) as f32, r.h.max(0) as f32),
    )
}

pub fn to_layout_rect(r: egui::Rect) -> LayoutRect {
    LayoutRect {
        x: r.left() as i32,
        y: r.top() as i32,
        w: r.width() as i32,
        h: r.height() as i32,
    }
}

/// OS에서 끌어온 파일이 놓일 패널에 테두리를 두른다 (FR-61).
///
/// **`show_layout`의 인자로 받지 않고 따로 부르게 둔 이유**: 강조할 패널을 정하려면
/// 그 함수가 **반환하는** `pane_rects`가 필요해, 인자로 되먹이면 같은 프레임 안에서
/// 순환이 된다. 그리기가 끝난 뒤 같은 `Ui`에 덧그리면 나중에 그린 것이 위에 오므로
/// 활성 테두리 위에 정상적으로 얹힌다(위 그리기 순서 주석과 같은 근거).
///
/// 굵기·모양은 활성 테두리와 같고 **색만 가른다** — 같은 자리에 다른 뜻의 선이 두 종류
/// 서는 것이라, 굵기까지 다르면 어느 쪽이 무엇인지 알 수 없다
pub fn draw_drop_highlight(ui: &egui::Ui, rect: egui::Rect) {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return;
    }
    ui.painter().rect_stroke(
        rect,
        0.0,
        egui::Stroke::new(PANE_BORDER_WIDTH, theme::ACCENT),
        egui::StrokeKind::Inside,
    );
}

/// 레이아웃이 상위(앱)에 올려보내는 요청 — 어느 패널에서 왔는지까지 담는다
#[derive(Default)]
pub struct LayoutOutcome {
    pub menu: Option<MenuRequest>,
    /// 목록에서 고쳐 확정한 이름 (FR-64) — 셸에 거는 것은 앱이다.
    ///
    /// 어느 패널이었는지는 담지 않는다: 요청에 **전체 경로**가 들어 있어 대상이 이미
    /// 하나로 정해진다(`command`처럼 "어느 패널에 적용할지"를 가릴 일이 없다)
    pub rename: Option<RenameRequest>,
    /// 패널 메뉴에서 고른 명령과 그 패널. **활성 패널이 아니라 메뉴를 연 패널**이다.
    ///
    /// 활성 판정은 포인터가 눌린 위치로만 이뤄지는데, 메뉴 팝업은 자기 패널 밖으로 뻗을 수
    /// 있어 그 위에서 고르면 아래 깔린 패널이 활성이 된다 — 그대로 활성 패널에 적용하면
    /// 닫기·새 파일이 엉뚱한 패널에 간다 (plan D16)
    pub command: Option<(PanelId, Command)>,
    /// 원격 단계 화면에서 고른 조치와 그 패널 (T10) — 대상 판정은 `command`와 같은 이유다
    pub remote: Option<(PanelId, RemoteAction)>,
    /// 주소창에 적힌 원격 주소와 그 패널 (FR-34)
    pub remote_url: Option<(PanelId, RemoteUrl)>,
    /// 마지막 원격 탭이 닫혀 아무도 쓰지 않게 된 연결들 (FR-32)
    pub closed_conns: Vec<ConnectionId>,
    /// 목록에 끌어다 놓은 것과 그 패널 (FR-38)
    pub drop: Option<(PanelId, DropOutcome)>,
    /// 원격 메뉴에서 고른 것과 그 패널 (FR-39)
    pub remote_menu: Option<(PanelId, RemoteMenuPick)>,
    /// 트리 우클릭 메뉴에서 고른 즐겨찾기 조작과 그 패널 (FR-56).
    ///
    /// 한 프레임에 메뉴는 하나만 떠 있으므로 first-wins다 — 다른 필드와 같은 규칙이다
    pub favorite: Option<(PanelId, FavoriteAction)>,
    /// 패널들이 열어 본 드라이브와 그 결과 `(경로, 닿았는가)` (T6) — **모아서** 올린다.
    ///
    /// 한 프레임에 두 패널이 서로 다른 드라이브를 열어 볼 수 있어 first-wins로 하나만
    /// 남기면 한쪽 관측이 버려진다(`tree_requests`와 같은 이유)
    pub drive_observed: Vec<(PathBuf, bool)>,
    /// 원격 트리가 청한 하위 조회들 (T24) — **모아서** 올린다.
    ///
    /// first-wins로 하나만 남기면 두 패널이 같은 프레임에 노드를 펼쳤을 때 한쪽이 영영
    /// `읽는 중…`에 머문다(다시 청하려면 접었다 펴야 한다 — 캐시가 `Loading`으로 막는다)
    pub tree_requests: Vec<(PanelId, TreeRequest)>,
    /// 이번 프레임에 **내용이 직접 눌린** 패널 — 전송 대상 탭을 정하는 신호다 (FR-54).
    ///
    /// 위 `command`가 쓰는 활성 판정(`active`)을 그대로 쓰지 않는 이유가 그 필드 설명에 있다:
    /// 메뉴 팝업이 자기 패널 밖으로 뻗으면 그 위를 눌러도 **아래 깔린 패널**이 활성이 된다.
    /// 활성 테두리가 잠깐 옮겨 가는 것과 달리 전송 대상은 파일이 어디로 갈지를 정하므로,
    /// 팝업에 가린 클릭까지 대상으로 세면 사용자가 누른 적 없는 폴더로 파일이 간다.
    /// 그래서 여기서는 **레이어 가림을 존중하는** `rect_contains_pointer`로 따로 판정한다
    pub pressed_panel: Option<PanelId>,
    /// 이번 프레임에 패널들이 차지한 자리 (FR-61) — OS에서 끌어온 파일이 **어느 패널에**
    /// 놓였는지 판정하는 데 쓴다.
    ///
    /// **egui의 포인터로는 판정할 수 없어 좌표를 위로 올린다** — OS 드래그 중에는
    /// `WM_MOUSEMOVE`가 오지 않아 egui가 아는 포인터 자리가 낡아 있고, 파일 드롭
    /// 이벤트에는 좌표가 실려 있지 않다. 앱이 Win32로 잰 커서 자리를 이 목록과 대조한다
    pub pane_rects: Vec<(PanelId, egui::Rect)>,
}

/// 패널이 낸 결과를 위로 올린다 — **필드를 골라 담지 않고 통째로** 받는다.
///
/// 골라 담으면 `PanelOutcome`에 필드가 늘 때마다 이 파일을 함께 고쳐야 한다(T22의 드래그 전송·
/// T23의 원격 메뉴가 그렇다). 대신 **필드별 first-wins**로 병합한다 — 한 프레임에 A패널이
/// 메뉴를 내고 B패널이 명령을 내면 둘 다 살아남아야 하므로, 비어 있는 필드만 채운다
fn merge_panel_outcome(outcome: &mut LayoutOutcome, id: PanelId, panel: PanelOutcome) {
    let PanelOutcome {
        menu,
        rename,
        command,
        remote,
        remote_url,
        closed_conn,
        drop,
        remote_menu,
        favorite,
        tree_requests,
        drive_observed,
    } = panel;
    // 한 프레임에 메뉴는 하나만 뜬다 — 먼저 요청한 패널 것을 쓴다
    if outcome.menu.is_none() {
        outcome.menu = menu;
    }
    // 이름 확정도 한 프레임에 하나뿐이다 — 편집칸은 포커스를 가진 한 곳에만 있다
    if outcome.rename.is_none() {
        outcome.rename = rename;
    }
    // 명령도 한 프레임에 하나만 — 어느 패널이 요청했는지 함께 담는다
    if outcome.command.is_none()
        && let Some(command) = command
    {
        outcome.command = Some((id, command));
    }
    if outcome.remote.is_none()
        && let Some(remote) = remote
    {
        outcome.remote = Some((id, remote));
    }
    // 드롭도 한 프레임에 하나뿐이다 — 마우스 버튼은 하나이고 놓는 자리도 하나다.
    // 어느 패널에 놓였는지 함께 담는 것은 **다른 필드와 같은 모양을 지키기 위한 것**이며,
    // 방향 판정에는 쓰이지 않는다(항목의 종류와 놓은 자리의 종류만으로 정해진다)
    if outcome.drop.is_none()
        && let Some(drop) = drop
    {
        outcome.drop = Some((id, drop));
    }
    // 원격 메뉴도 한 프레임에 하나뿐이다 — 어느 패널이 요청했는지 함께 담는다
    // (명령이 그 패널의 연결로 나가야 한다)
    if outcome.remote_menu.is_none()
        && let Some(menu) = remote_menu
    {
        outcome.remote_menu = Some((id, menu));
    }
    // 주소는 한 프레임에 하나만 확정된다 — 입력칸이 패널마다 하나뿐이다
    if outcome.remote_url.is_none()
        && let Some(url) = remote_url
    {
        outcome.remote_url = Some((id, url));
    }
    // 즐겨찾기 조작도 한 프레임에 하나뿐이다 — 메뉴가 하나만 떠 있기 때문이다
    if outcome.favorite.is_none()
        && let Some(action) = favorite
    {
        outcome.favorite = Some((id, action));
    }
    outcome
        .tree_requests
        .extend(tree_requests.into_iter().map(|request| (id, request)));
    // 열어 본 드라이브도 모아서 올린다 (T6) — 어느 패널이 열었는지는 쓰지 않는다
    // (드라이브 목록이 앱에 하나뿐이라 모든 패널이 같은 것을 본다)
    outcome.drive_observed.extend(drive_observed);
    // 닫힌 연결은 **모아서** 올린다 — 한 프레임에 여러 패널이 각자의 마지막 원격 탭을 닫을 수
    // 있고, first-wins로 하나만 남기면 나머지 연결의 워커·소켓이 그대로 남는다
    outcome.closed_conns.extend(closed_conn);
}

/// 분할된 패널들을 그리고 스플리터 드래그·활성 패널 전환을 처리한다.
///
/// `panels`에 없는 `PanelId`가 트리에 있으면 그 자리는 비워 둔다 —
/// 분할 직후 새 패널을 만들어 넣는 것은 호출부(`ExplorerApp`)의 몫이다
#[allow(clippy::too_many_arguments)]
pub fn show_layout(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    tree: &mut LayoutTree,
    panels: &mut HashMap<PanelId, PanelState>,
    active: &mut PanelId,
    icons: &mut IconCache,
    textures: &mut IconTextures,
    remote: RemoteView<'_>,
    display: DisplayRules,
    targets: TransferTargets,
    // 폴더 트리 맨 위에 설 즐겨찾기 (FR-56) — 앱에 하나뿐이라 모든 패널이 같은 것을 받는다
    favorites: &[FavoriteEntry],
    // 트리의 드라이브 줄과 그 연결 상태 (T4) — 즐겨찾기와 같은 이유로 앱에 하나뿐이다
    drives: &[DriveRow],
) -> LayoutOutcome {
    let mut outcome = LayoutOutcome::default();
    let area = ui.available_rect_before_wrap();
    let computed = tree.compute_rects(to_layout_rect(area));
    // 패널은 서로를 모르므로(모듈 주석) 닫기 가능 여부는 트리를 아는 이곳에서 정해 내려준다 (plan D15).
    // 보기 모드는 패널마다 다르므로 아래 루프에서 각자의 것을 싣는다
    let pane_count = computed.panes.len();
    // 패널 자리를 그대로 위로 올린다 (FR-61) — 그리기 순서와 같아 겹치면 뒤엣것이 위다
    outcome.pane_rects = computed
        .panes
        .iter()
        .map(|(id, rect)| (*id, to_egui_rect(*rect)))
        .collect();

    // 클릭이 일어난 패널을 활성으로 삼는다.
    // 패널 rect에 `interact`를 걸면 그 위젯이 목록·버튼 클릭을 가로채므로
    // (겹치는 위젯은 나중에 등록된 쪽이 이긴다) 포인터 위치로만 판정한다
    let pressed_at = ctx
        .input(|i| i.pointer.any_pressed().then(|| i.pointer.interact_pos()))
        .flatten();

    for (id, rect) in &computed.panes {
        let pane = to_egui_rect(*rect);
        if pane.width() <= 0.0 || pane.height() <= 0.0 {
            continue;
        }
        if let Some(pos) = pressed_at
            && pane.contains(pos)
        {
            *active = *id;
        }
        // 전송 대상은 **가려지지 않은 클릭**만 센다 (FR-54) — 위 `active`와 달리 이 판정은
        // `rect_contains_pointer`를 거쳐 팝업·모달에 덮인 자리를 걸러 낸다
        if pressed_at.is_some() && ui.rect_contains_pointer(pane) {
            outcome.pressed_panel = Some(*id);
        }
        let Some(panel) = panels.get_mut(id) else {
            continue;
        };
        // 패널마다 독립된 id 공간을 준다 — 같은 위젯이 여러 패널에 있어도 상태가 섞이지 않는다
        let builder = egui::UiBuilder::new()
            .max_rect(pane)
            .id_salt(("pane", id.0));
        let menu_state = PanelMenuState::for_panes(pane_count);
        let requested = ui
            .scope_builder(builder, |ui| {
                ui.set_clip_rect(pane);
                {
                    panel.apply_display_rules(display, ctx);
                    panel.show(
                        ui, ctx, icons, textures, remote, menu_state, targets, favorites, drives,
                    )
                }
            })
            .inner;
        merge_panel_outcome(&mut outcome, *id, requested);
    }

    // 패널 경계 — **활성 패널에만** 두른다. 패널끼리의 경계는 아래 스플리터가 1px 선으로
    // 긋고 있어(`theme::PANE_BORDER`), 여기서 모든 패널을 다시 두르면 그 선 양옆에 테두리가
    // 겹쳐 세 겹이 된다. 이 테두리가 남기는 뜻은 하나다 — **어디에 입력이 가는가**.
    //
    // 패널 내용을 **모두 그린 뒤** 두른다 — egui는 나중에 그린 도형이 위에 오므로
    // 먼저 그으면 목록·트리에 덮여 보이지 않는다
    if computed.panes.len() > 1
        && let Some((_, rect)) = computed.panes.iter().find(|(id, _)| id == active)
    {
        let pane = to_egui_rect(*rect);
        // 그리기 루프의 0크기 가드는 이 패스에 이어지지 않는다 — 여기서 다시 거른다
        if pane.width() > 0.0 && pane.height() > 0.0 {
            ui.painter().rect_stroke(
                pane,
                0.0,
                egui::Stroke::new(PANE_BORDER_WIDTH, theme::PANE_BORDER_ACTIVE),
                egui::StrokeKind::Inside,
            );
        }
    }

    // id는 목록 인덱스로 만든다 — `NodePath`는 Hash를 구현하지 않기 때문이다.
    // 드래그 도중에는 인덱스가 흔들리지 않는다: 트리 구조를 바꾸는 분할·닫기는 버튼 클릭,
    // 즉 **별도의 포인터 누름**으로만 일어나므로 같은 드래그 제스처 안에서 함께 발생할 수 없다
    for (index, splitter) in computed.splitters.iter().enumerate() {
        let rect = to_egui_rect(splitter.rect);
        // **이 1px이 곧 패널 사이의 경계선**이다 — 배경색으로 칠하면 그 자리가 틈으로 벌어져
        // 보인다(사용자 보고). 잡기는 아래에서 좌우로 넓힌다
        ui.painter().rect_filled(rect, 0.0, theme::PANE_BORDER);
        let grab = match splitter.dir {
            SplitDir::Horizontal => rect.expand2(egui::vec2(SPLITTER_GRAB_PAD, 0.0)),
            SplitDir::Vertical => rect.expand2(egui::vec2(0.0, SPLITTER_GRAB_PAD)),
        };
        let id = ui.id().with(("splitter", index));
        let resp = ui.interact(grab, id, egui::Sense::drag());
        let cursor = match splitter.dir {
            SplitDir::Horizontal => egui::CursorIcon::ResizeHorizontal,
            SplitDir::Vertical => egui::CursorIcon::ResizeVertical,
        };
        if resp.hovered() || resp.dragged() {
            ctx.set_cursor_icon(cursor);
        }
        if resp.dragged()
            && let Some(pos) = resp.interact_pointer_pos()
        {
            let node = splitter.node_area;
            let (start, len, at) = match splitter.dir {
                SplitDir::Horizontal => (node.x as f32, node.w, pos.x),
                SplitDir::Vertical => (node.y as f32, node.h, pos.y),
            };
            if len > 0 {
                // 최소 패널 크기 클램프는 set_ratio가 축 길이를 받아 처리한다
                let ratio = (at - start) / len as f32;
                let _ = tree.set_ratio(splitter.node_path, ratio, len);
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 사각형_변환은_왕복해도_같다() {
        let r = LayoutRect {
            x: 10,
            y: 20,
            w: 300,
            h: 400,
        };
        assert_eq!(to_layout_rect(to_egui_rect(r)), r);
    }

    #[test]
    fn 음수_크기는_0으로_잘린다() {
        let r = LayoutRect {
            x: 0,
            y: 0,
            w: -5,
            h: -5,
        };
        let e = to_egui_rect(r);
        assert_eq!(e.width(), 0.0);
        assert_eq!(e.height(), 0.0);
    }

    #[test]
    fn 두_패널이_열어_본_드라이브가_모두_모인다() {
        // T6 Acceptance — 한 프레임에 두 패널이 서로 다른 드라이브를 열어 볼 수 있다.
        // first-wins로 하나만 남기면 한쪽 관측이 버려져 그 드라이브의 배지가 어긋난다
        let mut outcome = LayoutOutcome::default();
        for (id, path, reachable) in [(PanelId(1), r"Z:\", false), (PanelId(2), r"Y:\", true)] {
            merge_panel_outcome(
                &mut outcome,
                id,
                PanelOutcome {
                    rename: None,
                    menu: None,
                    command: None,
                    remote: None,
                    remote_url: None,
                    closed_conn: None,
                    drop: None,
                    remote_menu: None,
                    favorite: None,
                    tree_requests: Vec::new(),
                    drive_observed: Some((PathBuf::from(path), reachable)),
                },
            );
        }
        assert_eq!(
            outcome.drive_observed,
            vec![
                (PathBuf::from(r"Z:\"), false),
                (PathBuf::from(r"Y:\"), true)
            ],
            "두 패널의 관측 중 하나가 버려졌다"
        );
    }

    #[test]
    fn 열어_본_것이_없으면_관측도_비어_있다() {
        // 폴더를 열지 않은 프레임에 빈 관측이 쌓이면 앱이 매 프레임 헛일을 한다
        let mut outcome = LayoutOutcome::default();
        merge_panel_outcome(
            &mut outcome,
            PanelId(1),
            PanelOutcome {
                rename: None,
                menu: None,
                command: None,
                remote: None,
                remote_url: None,
                closed_conn: None,
                drop: None,
                remote_menu: None,
                favorite: None,
                tree_requests: Vec::new(),
                drive_observed: None,
            },
        );
        assert!(outcome.drive_observed.is_empty());
    }

    #[test]
    fn 여러_패널의_서로_다른_결과가_함께_살아남는다() {
        // A패널이 메뉴를, B패널이 명령을 낸 프레임에서 한쪽이 통째로 버려지면 안 된다.
        // relay를 통째로 올리도록 바꾸면서 필드별 first-wins를 유지하는지 고정한다 (plan T9 ⑦)
        let mut outcome = LayoutOutcome::default();
        merge_panel_outcome(
            &mut outcome,
            PanelId(1),
            PanelOutcome {
                rename: None,
                menu: Some(MenuRequest {
                    folder: std::path::PathBuf::from(r"C:\A"),
                    items: Vec::new(),
                    dirs: Vec::new(),
                    pos: egui::pos2(0.0, 0.0),
                }),
                command: None,
                remote: None,
                remote_url: None,
                closed_conn: None,
                drop: None,
                remote_menu: None,
                favorite: None,
                tree_requests: Vec::new(),
                drive_observed: None,
            },
        );
        merge_panel_outcome(
            &mut outcome,
            PanelId(2),
            PanelOutcome {
                rename: None,
                menu: None,
                command: Some(Command::NewFolder),
                remote: None,
                remote_url: None,
                closed_conn: None,
                drop: None,
                remote_menu: None,
                favorite: None,
                tree_requests: Vec::new(),
                drive_observed: None,
            },
        );

        assert!(outcome.menu.is_some(), "A패널의 메뉴가 버려졌다");
        assert_eq!(
            outcome.command,
            Some((PanelId(2), Command::NewFolder)),
            "B패널의 명령이 버려졌다"
        );
        assert_eq!(
            outcome.menu.as_ref().map(|m| m.folder.clone()),
            Some(std::path::PathBuf::from(r"C:\A"))
        );
    }

    #[test]
    fn 같은_종류의_결과는_먼저_낸_패널_것을_쓴다() {
        // 한 프레임에 메뉴는 하나만 뜬다
        let mut outcome = LayoutOutcome::default();
        for (id, folder) in [(PanelId(1), r"C:\먼저"), (PanelId(2), r"C:\나중")] {
            merge_panel_outcome(
                &mut outcome,
                id,
                PanelOutcome {
                    rename: None,
                    menu: Some(MenuRequest {
                        folder: std::path::PathBuf::from(folder),
                        items: Vec::new(),
                        dirs: Vec::new(),
                        pos: egui::pos2(0.0, 0.0),
                    }),
                    command: Some(Command::NewFolder),
                    remote: None,
                    remote_url: None,
                    closed_conn: None,
                    drop: None,
                    remote_menu: None,
                    favorite: None,
                    tree_requests: Vec::new(),
                    drive_observed: None,
                },
            );
        }
        assert_eq!(
            outcome.menu.as_ref().map(|m| m.folder.clone()),
            Some(std::path::PathBuf::from(r"C:\먼저"))
        );
        assert_eq!(outcome.command.map(|(id, _)| id), Some(PanelId(1)));
    }

    #[test]
    fn 닫힌_연결은_하나도_버리지_않고_모은다() {
        // 두 패널이 같은 프레임에 각자의 마지막 원격 탭을 닫으면 연결도 둘 다 접혀야 한다 —
        // first-wins로 하나만 남기면 나머지 워커·소켓이 그대로 남는다 (FR-32)
        let mut outcome = LayoutOutcome::default();
        for (id, conn) in [(PanelId(1), ConnectionId(7)), (PanelId(2), ConnectionId(9))] {
            merge_panel_outcome(
                &mut outcome,
                id,
                PanelOutcome {
                    rename: None,
                    menu: None,
                    command: None,
                    remote: Some(RemoteAction::Retry),
                    remote_url: None,
                    closed_conn: Some(conn),
                    drop: None,
                    remote_menu: None,
                    favorite: None,
                    tree_requests: Vec::new(),
                    drive_observed: None,
                },
            );
        }
        assert_eq!(
            outcome.closed_conns,
            vec![ConnectionId(7), ConnectionId(9)],
            "닫힌 연결이 버려졌다"
        );
        // 조치는 한 프레임에 하나만 — 먼저 낸 패널 것을 쓴다
        assert_eq!(outcome.remote, Some((PanelId(1), RemoteAction::Retry)));
    }

    #[test]
    fn 한_패널이_낸_트리_요청은_전부_올라간다() {
        // quality 리뷰 M1 — first-wins로 하나만 남기면 나머지 노드가 그 프레임을 헛돈다
        let mut outcome = LayoutOutcome::default();
        let id = PanelId(1);
        let requests = vec![
            TreeRequest::Remote {
                conn: ConnectionId(1),
                path: crate::remote::types::RemotePath::new("/var"),
            },
            TreeRequest::Remote {
                conn: ConnectionId(1),
                path: crate::remote::types::RemotePath::new("/etc"),
            },
        ];
        merge_panel_outcome(
            &mut outcome,
            id,
            PanelOutcome {
                rename: None,
                menu: None,
                command: None,
                remote: None,
                remote_url: None,
                closed_conn: None,
                drop: None,
                remote_menu: None,
                favorite: None,
                tree_requests: requests.clone(),
                drive_observed: None,
            },
        );
        assert_eq!(
            outcome.tree_requests,
            requests.into_iter().map(|r| (id, r)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn 즐겨찾기_조작은_어느_패널에서_왔는지와_함께_한_건만_올라간다() {
        // 이 홉은 **컴파일러가 잡아 주지 않는다**(`LayoutOutcome`은 `default()`로 만들어져
        // 필드별로 대입된다 — plan 전제 4-b). 한 줄을 빠뜨려도 빌드가 통과하므로 시험이 지킨다.
        // 메뉴는 한 프레임에 하나만 떠 있으니 다른 필드와 같은 first-wins다
        let mut outcome = LayoutOutcome::default();
        merge_panel_outcome(
            &mut outcome,
            PanelId(1),
            PanelOutcome {
                rename: None,
                menu: None,
                command: None,
                remote: None,
                remote_url: None,
                closed_conn: None,
                drop: None,
                remote_menu: None,
                favorite: Some(FavoriteAction::Add(std::path::PathBuf::from(r"D:\작업"))),
                tree_requests: Vec::new(),
                drive_observed: None,
            },
        );
        merge_panel_outcome(
            &mut outcome,
            PanelId(2),
            PanelOutcome {
                rename: None,
                menu: None,
                command: None,
                remote: None,
                remote_url: None,
                closed_conn: None,
                drop: None,
                remote_menu: None,
                favorite: Some(FavoriteAction::Remove(std::path::PathBuf::from(
                    r"C:\Users",
                ))),
                tree_requests: Vec::new(),
                drive_observed: None,
            },
        );

        assert_eq!(
            outcome.favorite,
            Some((
                PanelId(1),
                FavoriteAction::Add(std::path::PathBuf::from(r"D:\작업"))
            )),
            "먼저 올라온 조작이 어느 패널 것인지와 함께 남아야 한다"
        );
    }
}
