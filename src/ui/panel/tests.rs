//! 패널 테스트 — 탐색·탭·원격·목록 조작을 한자리에서 검증한다.
//!
//! 본체(`ui::panel`)의 자식 모듈이라 그 파일의 비공개 항목에 그대로 닿는다.

use super::*;
use crate::app::favorites::FavoriteEntry;
use crate::app::layout::PanelId;
use crate::app::workspace::WorkspaceId;
use crate::remote::sites::SiteStore;
use crate::remote::types::{RemotePath, SiteId};

/// 원격 항목 하나 — 여러 테스트가 함께 쓴다
fn remote_entry(name: &str, is_dir: bool) -> RemoteEntry {
    RemoteEntry {
        name: name.to_owned(),
        is_dir,
        is_symlink: false,
        link_target: None,
        size: 0,
        modified: None,
        mode: None,
        owner: None,
    }
}

/// 한 프레임에 그려진 글자를 전부 모은다 — 화면에 실제로 무엇이 보이는지 판정한다
fn drawn_texts(output: &eframe::egui::FullOutput) -> Vec<String> {
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
    let mut found = Vec::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut found);
    }
    found
}

/// 한 프레임에 그려진 글과 그 왼쪽 위 자리 — 배치를 견주는 시험이 쓴다
fn drawn_text_positions(output: &eframe::egui::FullOutput) -> Vec<(String, egui::Pos2)> {
    fn collect(shape: &egui::Shape, found: &mut Vec<(String, egui::Pos2)>) {
        match shape {
            egui::Shape::Text(text) => found.push((text.galley.text().to_owned(), text.pos)),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in &output.shapes {
        collect(&clipped.shape, &mut found);
    }
    found
}

/// 한 프레임에 그려진 가로 구분선 수 — 구분선은 글이 아니라 얇은 사각형이라 글 목록에 없다.
/// 절대 개수가 아니라 **늘고 주는 것**을 보는 데 쓴다(패널에는 원래 상태 줄 아래 구분선이 있다)
fn separator_count(output: &eframe::egui::FullOutput) -> usize {
    fn count(shape: &egui::Shape, found: &mut usize) {
        match shape {
            // egui `separator()`는 얇은 사각형으로도, 선분으로도 그려진다 — 둘 다 센다
            egui::Shape::Rect(rect) => {
                let size = rect.rect.size();
                if size.y <= 2.0 && size.x > size.y * 4.0 {
                    *found += 1;
                }
            }
            egui::Shape::LineSegment { points, .. } => {
                let (a, b) = (points[0], points[1]);
                if (a.y - b.y).abs() <= 1.0 && (a.x - b.x).abs() > 4.0 {
                    *found += 1;
                }
            }
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

/// 이 PC 드라이브의 **셸 표시 이름** 목록 — 시험이 드라이브 줄을 가려내는 기준.
///
/// 패턴 매칭(`ends_with(":\\")`)을 쓰지 않는다. 표시 이름에는 규칙이 없고(`로컬 디스크 (C:)`·
/// 볼륨 레이블·네트워크 이름), "토글 아래 + 트리 폭 안"으로만 좁히면 제목 줄·즐겨찾기 줄까지
/// 걸려 첫 매치를 쓰는 자리가 엉뚱한 줄을 드라이브로 오인한다
///
/// 조회는 `drive_rows`가 한다 — 이 함수는 그 결과에서 `(경로, 표시 이름)`만 뽑는다
fn drive_labels() -> Vec<(std::path::PathBuf, String)> {
    drive_rows()
        .iter()
        .map(|row| (row.path.clone(), row.label.clone()))
        .collect()
}

/// 트리에 내려보낼 드라이브 줄 — 앱이 워커로 만드는 것을 시험에서는 곧바로 만든다 (T4).
///
/// **접근 판정을 하지 않는 `list_drives`를 쓴다** — 판정까지 하면 끊긴 네트워크
/// 드라이브가 있는 PC에서 시험 하나가 초 단위로 늘어진다
fn drive_rows() -> &'static [crate::fs::drives::DriveRow] {
    // **프로세스에 한 번만 조회한다.** 그리기 하네스가 프레임마다 이것을 부르는데,
    // 매번 새로 조회하면 ⓐ `IconCache`를 새로 만들어 캐시가 비고 ⓑ 드라이브마다 셸을
    // 다시 물으며 ⓒ 그 셸 호출마다 전역 잠금을 잡아 다른 시험과 직렬화된다. 실측으로
    // 전체 스위트가 10분을 넘겼다 — 드라이브 구성은 시험이 도는 동안 바뀌지 않으므로
    // 한 번 만들어 나눠 쓴다
    static ROWS: std::sync::OnceLock<Vec<crate::fs::drives::DriveRow>> = std::sync::OnceLock::new();
    ROWS.get_or_init(|| {
        let mut icons = crate::fs::icons::IconCache::new();
        crate::fs::drives::list_drives(&mut icons)
    })
}

/// **트리 구역**(x < `TREE_WIDTH`)에 그려진 아이콘 수.
///
/// 목록도 같은 프레임에 아이콘을 그리므로 구역으로 좁혀 센다. 아이콘은 텍스처를 입힌
/// 사각형이라 `egui::Shape::Mesh`로 나온다(글자·선은 다른 종류라 섞이지 않는다)
fn tree_icon_count(output: &eframe::egui::FullOutput) -> usize {
    fn count(shape: &egui::Shape, found: &mut usize) {
        match shape {
            egui::Shape::Mesh(mesh) => {
                let bounds = mesh.calc_bounds();
                if bounds.max.x < crate::ui::tree::TREE_WIDTH {
                    *found += 1;
                }
            }
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

/// 트리 구역에 그려진 **연결 끊김 배지** 수 (T5).
///
/// 배지는 글자가 아니라 채워진 원이라 `drawn_texts`로는 보이지 않는다 — 색으로 가려낸다
fn offline_badges(output: &eframe::egui::FullOutput) -> usize {
    fn count(shape: &egui::Shape, found: &mut usize) {
        match shape {
            egui::Shape::Circle(circle) if circle.fill == crate::ui::theme::OFFLINE_BADGE => {
                *found += 1;
            }
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

/// 한 프레임에 그려진 자리표시 막대 수 — 목록 자리에 자리표시가 섰는지 판정한다.
/// 막대는 글자가 아니라 사각형이라 `drawn_texts`로는 보이지 않는다
fn skeleton_bars(output: &eframe::egui::FullOutput) -> usize {
    fn count(shape: &egui::Shape, found: &mut usize) {
        match shape {
            egui::Shape::Rect(rect) if rect.fill == crate::ui::remote_states::SKELETON_FILL => {
                *found += 1;
            }
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

/// egui는 같은 ID가 한 프레임에 두 번 쓰이면 화면에 경고 텍스트를 그린다.
/// 그 텍스트를 그려진 글자에서 찾아 ID 충돌 여부를 판정한다
fn id_clash_warnings(output: &eframe::egui::FullOutput) -> Vec<String> {
    drawn_texts(output)
        .into_iter()
        .filter(|body| body.contains("use of"))
        .collect()
}

/// 패널을 한 프레임 그린다 — 사이트 목록은 호출부가 준다
fn draw_once(panel: &mut PanelState, sites: &SiteStore) -> eframe::egui::FullOutput {
    draw_once_with_favorites(panel, sites, &[])
}

/// 사용자가 손으로 담은 즐겨찾기 한 줄 — 라벨 없이 경로만이고 해제할 수 있다.
/// **기본 항목(바탕 화면·다운로드)은 여기서 만들지 않는다** — 하네스는 시험이 준 것만
/// 그려야 화면 배치를 보는 시험이 실제 PC의 폴더 유무에 흔들리지 않는다
fn user_favorite(path: &str) -> FavoriteEntry {
    FavoriteEntry {
        path: std::path::PathBuf::from(path),
        label: None,
        removable: true,
        missing: false,
    }
}

/// 그 자리를 누르거나 떼는 이벤트
fn press_at(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

/// 가리키는 폴더가 사라진 즐겨찾기 한 줄 (FR-56)
fn missing_favorite(path: &str) -> FavoriteEntry {
    FavoriteEntry {
        missing: true,
        ..user_favorite(path)
    }
}

/// 윈도우가 정해 준 기본 즐겨찾기 한 줄 — 셸 표시 이름을 들고 해제할 수 없다.
/// **실제 PC의 폴더를 찾지 않는다** — 화면 배치를 보는 시험이라 경로가 실재할 필요가 없다
fn default_favorite(path: &str, label: &str) -> FavoriteEntry {
    FavoriteEntry {
        path: std::path::PathBuf::from(path),
        label: Some(label.to_owned()),
        removable: false,
        missing: false,
    }
}

/// 즐겨찾기를 든 채 한 프레임 그린다 — 트리 위쪽 구역을 보는 시험이 쓴다
fn draw_once_with_favorites(
    panel: &mut PanelState,
    sites: &SiteStore,
    favorites: &[FavoriteEntry],
) -> eframe::egui::FullOutput {
    draw_once_with(panel, sites, favorites, drive_rows())
}

/// 즐겨찾기와 **드라이브 줄**을 손으로 지정해 한 프레임 그린다 — 연결 끊김 배지를 보는
/// 시험이 쓴다(실제 PC에 끊긴 네트워크 드라이브가 있어야 하면 시험이 환경에 매인다)
fn draw_once_with(
    panel: &mut PanelState,
    sites: &SiteStore,
    favorites: &[FavoriteEntry],
    drives: &[crate::fs::drives::DriveRow],
) -> eframe::egui::FullOutput {
    let tree = crate::remote::tree_cache::TreeCache::new();
    let remote = RemoteView {
        sites,
        connected: &[],
        tree: &tree,
    };
    let ctx = egui::Context::default();
    let mut icons = crate::fs::icons::IconCache::new();
    let mut textures = crate::ui::icon_tex::IconTextures::new();
    ctx.run_ui(Default::default(), |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            let ctx = ui.ctx().clone();
            // 앱은 프레임마다 이것을 부른다(`ui::app`) — 하네스가 빠뜨리면 텍스처 생성
            // 상한(프레임당 8개)이 통틀어 8개로 굳어 아이콘이 많은 PC에서 시험이 어긋난다
            textures.begin_frame();
            panel.show(
                ui,
                &ctx,
                &mut icons,
                &mut textures,
                remote,
                PanelMenuState::for_panes(1, ViewMode::Details),
                // 전송 대상이 없는 상태 — 이 시험들은 탭 아이콘이 아니라 배치·상태를 본다
                crate::ui::tabs::TransferTargets::default(),
                favorites,
                drives,
            );
        });
    })
}

/// 패널을 한 프레임 그리고 ID 충돌 경고를 모은다
fn draw_panel(tree_visible: bool) -> Vec<String> {
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = tree_visible;
    id_clash_warnings(&draw_once(&mut panel, &SiteStore::new()))
}

/// 사이트 하나를 등록하고 그 사이트의 원격 탭을 활성으로 둔 패널.
/// 단계별 화면(README §4·§5)이 실제 렌더 경로를 지나게 하는 준비다
fn remote_panel_in(phase: TabPhase) -> (PanelState, SiteStore) {
    let mut sites = SiteStore::new();
    let site = sites.add("배포 서버");
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\테스트"));
    panel.tabs.add(crate::panel::tabs::TabState::remote(
        site,
        RemotePath::new("/var/www"),
    ));
    panel.attach_conn(ConnectionId(1));
    panel.set_phase_for(ConnectionId(1), &phase);
    (panel, sites)
}

/// 그 단계의 원격 패널을 한 프레임 그리고 화면 글자를 모은다
fn remote_screen_texts(phase: TabPhase) -> Vec<String> {
    let (mut panel, sites) = remote_panel_in(phase);
    drawn_texts(&draw_once(&mut panel, &sites))
}

/// 열거 결과가 도착한 상황을 만들어 `poll_load`를 실제로 지나게 한다.
/// **헬퍼만 직접 부르면 안 된다** — 호출부가 죽어 있어도 통과하기 때문이다(F-7 B1)
fn commit_dir(panel: &mut PanelState, dir: &str, icons: &mut IconCache) {
    panel.pending_dir = std::path::PathBuf::from(dir);
    panel.pending_nav = PendingNav::None;
    panel.apply_enumerated(
        EnumOutcome::Ok(Vec::new()),
        icons,
        &mut crate::panel::dir_cache::DirCache::new(),
        &egui::Context::default(),
    );
}

/// 배치 하나를 만든다 — 이름만 다른 파일 항목
fn batch(names: &[&str]) -> Vec<crate::fs::enumerate::FileEntry> {
    names
        .iter()
        .map(|name| {
            let mut wide: Vec<u16> = name.encode_utf16().collect();
            wide.push(0);
            crate::fs::enumerate::FileEntry {
                name: wide,
                is_dir: false,
                size: 0,
                modified: 0,
                attributes: 0,
            }
        })
        .collect()
}

#[test]
fn 첫_배치가_커밋하고_완료_조각이_빈_경로를_덮어쓰지_않는다() {
    // FR-69 + 계획 D6 — 이른 커밋은 `pending_dir`을 **비우지 않고 clone**해 넘긴다.
    // 비우면 뒤이어 오는 `Done`의 `mem::take`가 빈 경로를 커밋해 탭 경로가 망가진다
    let mut icons = IconCache::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    panel.pending_dir = std::path::PathBuf::from(r"C:\Docs");
    panel.pending_nav = PendingNav::Push;

    panel.apply_partial(batch(&["a.txt"]), &mut icons);
    assert_eq!(
        panel.dir(),
        std::path::Path::new(r"C:\Docs"),
        "첫 배치가 커밋하지 않았다"
    );
    // `..` 줄이 함께 서므로 항목은 2개다
    assert_eq!(panel.list.len(), 2, "첫 배치가 그려지지 않았다");

    panel.apply_enumerated(
        EnumOutcome::Ok(batch(&["b.txt"])),
        &mut icons,
        &mut crate::panel::dir_cache::DirCache::new(),
        &egui::Context::default(),
    );
    assert_eq!(
        panel.dir(),
        std::path::Path::new(r"C:\Docs"),
        "완료 조각이 빈 경로를 커밋했다"
    );
    // 잔여분만 이어 붙는다 — 갈아 끼우면 `..`+b 둘만 남는다
    assert_eq!(panel.list.len(), 3, "완료 조각이 앞선 배치를 지웠다");
}

#[test]
fn 배치를_흘리던_중_끊기면_그린_것을_지우지_않는다() {
    // FR-69 — `block_list`는 목록을 `..` 한 줄만 남기고 갈아 끼운다. 그 길로 가면 안 된다
    let mut icons = IconCache::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    panel.pending_dir = std::path::PathBuf::from(r"C:\Docs");
    panel.pending_nav = PendingNav::None;

    panel.apply_partial(batch(&["a.txt", "b.txt"]), &mut icons);
    assert_eq!(panel.list.len(), 3);

    panel.apply_enumerated(
        EnumOutcome::Error { network: true },
        &mut icons,
        &mut crate::panel::dir_cache::DirCache::new(),
        &egui::Context::default(),
    );
    assert_eq!(panel.list.len(), 3, "부분 목록이 지워졌다");
    assert!(!panel.status.is_empty(), "사유가 상태 줄에 적히지 않았다");
    assert!(
        panel.pending_dir.as_os_str().is_empty(),
        "끝난 요청의 경로가 남았다"
    );
}

#[test]
fn 조각이_없었으면_완료_조각이_종전대로_목록을_갈아_끼운다() {
    // 임계 아래 폴더의 길 — `streamed`가 거짓이라 `set_entries` 전량 교체다
    let mut icons = IconCache::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    panel.pending_dir = std::path::PathBuf::from(r"C:\Docs");
    panel.pending_nav = PendingNav::None;
    panel.apply_enumerated(
        EnumOutcome::Ok(batch(&["a.txt", "b.txt"])),
        &mut icons,
        &mut crate::panel::dir_cache::DirCache::new(),
        &egui::Context::default(),
    );
    assert_eq!(panel.dir(), std::path::Path::new(r"C:\Docs"));
    assert_eq!(panel.list.len(), 3, "`..` + 항목 둘이어야 한다");
}

/// 캐시 적중 시험 한 벌 — 목록 캐시·아이콘 캐시·컨텍스트를 함께 세운다
fn cache_fixture() -> (egui::Context, IconCache, crate::panel::dir_cache::DirCache) {
    (
        egui::Context::default(),
        IconCache::new(),
        crate::panel::dir_cache::DirCache::new(),
    )
}

#[test]
fn 캐시가_있으면_그_자리에서_목록과_경로가_함께_옮겨간다() {
    // FR-68 — 다시 들어간 폴더는 자리표시 없이 즉시 선다
    let (ctx, mut icons, mut cache) = cache_fixture();
    let target = std::path::PathBuf::from(r"C:\캐시곳");
    // 캐시에는 `..` 줄이 포함된 최종 목록이 담긴다(계획 D7)
    cache.put(&target, &batch(&["..", "a.txt"]));

    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\원래곳"));
    panel.start_load(target.clone(), PendingNav::Push, &ctx);
    panel.try_cache_hit(&mut icons, &mut cache);

    assert_eq!(panel.dir(), target, "캐시가 있는데 커밋되지 않았다");
    assert_eq!(panel.list.len(), 2, "`..`를 다시 붙여 목록이 늘었다");
    assert!(panel.optimistic.is_some(), "되돌릴 자리를 챙기지 않았다");
}

#[test]
fn 이미_그_폴더를_그리고_있으면_캐시를_쓰지_않는다() {
    // F5·감시 갱신·숨김 토글이 같은 폴더를 다시 읽는 길 — 그 목적이 최신 상태 확인이다
    let (ctx, mut icons, mut cache) = cache_fixture();
    let target = std::path::PathBuf::from(r"C:\보던곳");
    cache.put(&target, &batch(&["a.txt"]));

    let mut panel = PanelState::new(target.clone());
    panel.pending_dir = target.clone();
    panel.pending_nav = PendingNav::None;
    panel.apply_enumerated(
        EnumOutcome::Ok(batch(&["a.txt", "b.txt"])),
        &mut icons,
        &mut cache,
        &ctx,
    );
    let before = panel.list.len();

    panel.start_load(target.clone(), PendingNav::None, &ctx);
    panel.try_cache_hit(&mut icons, &mut cache);
    assert!(
        panel.optimistic.is_none(),
        "같은 폴더를 다시 읽는데 캐시를 썼다"
    );
    assert_eq!(panel.list.len(), before, "목록이 캐시로 덮였다");
}

#[test]
fn 탭을_전환해_같은_경로를_다시_읽어도_캐시가_적중한다() {
    // `reload_active_tab`은 활성 탭을 **바꾼 뒤** 그 탭의 committed 경로로 다시 읽는다 —
    // 그 순간 `pending_dir == dir()`이 된다. 적중 판정을 커밋된 경로로 재면 이 길이 새고,
    // 그러면 캐시가 가장 잦은 경로(탭 전환)를 놓친다
    let (ctx, mut icons, mut cache) = cache_fixture();
    let other = std::path::PathBuf::from(r"C:\다른탭곳");
    let target = std::path::PathBuf::from(r"C:\돌아올곳");
    cache.put(&target, &batch(&["..", "a.txt"]));

    // 지금 그려져 있는 것은 다른 폴더의 목록이다
    let mut panel = PanelState::new(other.clone());
    panel.pending_dir = other.clone();
    panel.pending_nav = PendingNav::None;
    panel.apply_enumerated(
        EnumOutcome::Ok(batch(&["b.txt", "c.txt", "d.txt"])),
        &mut icons,
        &mut cache,
        &ctx,
    );

    // 탭이 바뀌어 그 탭의 경로가 커밋된 뒤, 같은 경로로 다시 읽는다
    panel.tabs.active_mut().set_committed(target.clone());
    panel.start_load(target.clone(), PendingNav::None, &ctx);
    assert_eq!(
        panel.dir(),
        target,
        "시험 전제 — 커밋된 경로와 대기 경로가 같다"
    );

    panel.try_cache_hit(&mut icons, &mut cache);
    assert!(
        panel.optimistic.is_some(),
        "탭 전환 경로가 캐시를 타지 못했다"
    );
    assert_eq!(panel.list.len(), 2, "캐시 목록이 서지 않았다");
}

#[test]
fn 원격_탭을_거쳐_돌아와도_캐시가_적중한다() {
    // 적중 판정을 `list.dir()`만으로 하면 이 경로가 샌다 — `clear_entries`가 그 필드를
    // 갱신하지 않아 목록이 빈 채 폴더만 남고, 그러면 미적중으로 판정돼 자리표시가 뜬다
    let (ctx, mut icons, mut cache) = cache_fixture();
    let target = std::path::PathBuf::from(r"C:\오갈곳");
    cache.put(&target, &batch(&["..", "a.txt"]));

    let mut panel = PanelState::new(target.clone());
    panel.pending_dir = target.clone();
    panel.pending_nav = PendingNav::None;
    panel.apply_enumerated(
        EnumOutcome::Ok(batch(&["a.txt"])),
        &mut icons,
        &mut cache,
        &ctx,
    );
    // 원격 탭으로 옮겨 간 상태 — 목록은 비지만 그려 둔 폴더 이름은 남는다
    panel.list.clear_entries();

    panel.start_load(target.clone(), PendingNav::None, &ctx);
    panel.try_cache_hit(&mut icons, &mut cache);
    assert!(
        panel.optimistic.is_some(),
        "원격을 거쳐 돌아온 탭이 캐시를 타지 못했다"
    );
    assert_eq!(panel.list.len(), 2);
}

#[test]
fn 캐시로_옮긴_폴더가_없으면_앞으로_가기_목록까지_되돌린다() {
    // FR-68 — `History::push`가 앞으로 가기 목록을 자르므로 스냅샷 복원만이 되살린다
    let (ctx, mut icons, mut cache) = cache_fixture();
    let start = std::path::PathBuf::from(r"C:\처음곳");
    let mut panel = PanelState::new(start.clone());
    panel
        .tabs
        .active_mut()
        .history
        .push(std::path::PathBuf::from(r"C:\뒤곳"));
    panel.tabs.active_mut().history.back();
    assert!(
        panel.tabs.active().history.can_forward(),
        "시험 전제를 잘못 세웠다"
    );

    let ghost = std::path::PathBuf::from(r"C:\사라진곳");
    cache.put(&ghost, &batch(&["a.txt"]));
    panel.start_load(ghost.clone(), PendingNav::Push, &ctx);
    panel.try_cache_hit(&mut icons, &mut cache);
    assert_eq!(panel.dir(), ghost);
    assert!(
        !panel.tabs.active().history.can_forward(),
        "push가 자르지 않았다"
    );

    panel.apply_enumerated(EnumOutcome::NotFound, &mut icons, &mut cache, &ctx);
    assert_eq!(panel.dir(), start, "직전 폴더로 되돌아가지 않았다");
    assert!(
        panel.tabs.active().history.can_forward(),
        "앞으로 가기 목록이 되살아나지 않았다"
    );
    assert!(cache.get(&ghost).is_none(), "없는 폴더의 캐시가 남았다");
}

#[test]
fn 되돌린_뒤_다시_건_열거가_끝나도_빈_경로로_커밋되지_않는다() {
    // 되돌리기는 직전 폴더로 **새 열거를 건다** — 그 요청의 대상이 `pending_dir`인데
    // 되돌린 직후 그것을 비우면, 그 열거가 끝났을 때 `mem::take`가 빈 경로를 꺼내
    // 방금 되돌려 놓은 커밋을 덮는다(리뷰가 잡은 결함의 회귀 시험)
    let (ctx, mut icons, mut cache) = cache_fixture();
    let start = std::path::PathBuf::from(r"C:\돌아갈곳");
    let ghost = std::path::PathBuf::from(r"C:\없어진곳");
    cache.put(&ghost, &batch(&["a.txt"]));

    let mut panel = PanelState::new(start.clone());
    panel.start_load(ghost.clone(), PendingNav::Push, &ctx);
    panel.try_cache_hit(&mut icons, &mut cache);
    panel.apply_enumerated(EnumOutcome::NotFound, &mut icons, &mut cache, &ctx);
    assert_eq!(panel.dir(), start, "되돌아가지 않았다");

    // 되돌리기가 건 열거가 성공해 도착한다
    panel.apply_enumerated(
        EnumOutcome::Ok(batch(&["x.txt"])),
        &mut icons,
        &mut cache,
        &ctx,
    );
    assert_eq!(panel.dir(), start, "재열거 완료가 빈 경로를 커밋했다");
}

#[test]
fn 되돌릴_때_사라진_폴더의_목록을_남기지_않는다() {
    // 경로·히스토리만 되돌리면 되돌아간 폴더 이름 아래 **사라진 폴더의 항목**이 그대로 남아
    // 주소창·트리가 가리키는 곳과 목록이 갈린다 — 이 앱이 결함으로 못 박아 둔 형태다
    let (ctx, mut icons, mut cache) = cache_fixture();
    let start = std::path::PathBuf::from(r"C:\남은곳");
    let ghost = std::path::PathBuf::from(r"C:\지워진곳");
    cache.put(&ghost, &batch(&["..", "유령.txt"]));

    let mut panel = PanelState::new(start.clone());
    panel.start_load(ghost.clone(), PendingNav::Push, &ctx);
    panel.try_cache_hit(&mut icons, &mut cache);
    assert_eq!(panel.list.len(), 2, "캐시 목록이 서지 않았다");

    panel.apply_enumerated(EnumOutcome::NotFound, &mut icons, &mut cache, &ctx);
    assert_eq!(panel.dir(), start);
    assert_eq!(panel.list.len(), 0, "사라진 폴더의 목록이 남았다");
}

#[test]
fn 되돌릴_때_직전_폴더가_캐시에_있으면_그_목록이_즉시_선다() {
    // 되돌아간 자리도 캐시가 있으면 자리표시 없이 바로 그린다
    let (ctx, mut icons, mut cache) = cache_fixture();
    let start = std::path::PathBuf::from(r"C:\담긴곳");
    let ghost = std::path::PathBuf::from(r"C:\없앤곳");
    cache.put(&start, &batch(&["..", "a.txt", "b.txt"]));
    cache.put(&ghost, &batch(&["..", "유령.txt"]));

    let mut panel = PanelState::new(start.clone());
    panel.start_load(ghost.clone(), PendingNav::Push, &ctx);
    panel.try_cache_hit(&mut icons, &mut cache);
    panel.apply_enumerated(EnumOutcome::NotFound, &mut icons, &mut cache, &ctx);

    assert_eq!(panel.dir(), start);
    assert_eq!(
        panel.list.len(),
        3,
        "되돌아간 폴더의 캐시 목록이 서지 않았다"
    );
}

#[test]
fn 캐시로_옮긴_뒤_실제_결과가_와도_히스토리는_한_번만_늘어난다() {
    // 계획 D6 — 이른 커밋이 히스토리를 쌓고, 뒤이어 오는 완료 조각은 `pending_nav`가
    // `None`이라 재적용뿐이어야 한다. 두 번 쌓이면 뒤로 가기를 두 번 눌러야 원래로 돌아간다
    let (ctx, mut icons, mut cache) = cache_fixture();
    let start = std::path::PathBuf::from(r"C:\처음곳");
    let target = std::path::PathBuf::from(r"C:\옮길곳");
    cache.put(&target, &batch(&["..", "a.txt"]));

    let mut panel = PanelState::new(start.clone());
    panel.start_load(target.clone(), PendingNav::Push, &ctx);
    panel.try_cache_hit(&mut icons, &mut cache);
    panel.apply_enumerated(
        EnumOutcome::Ok(batch(&["a.txt"])),
        &mut icons,
        &mut cache,
        &ctx,
    );

    let history = &mut panel.tabs.active_mut().history;
    assert_eq!(
        history.back().map(std::path::Path::to_path_buf),
        Some(start),
        "뒤로 한 번에 처음 자리로 돌아가지 않았다"
    );
    assert!(!history.can_back(), "히스토리에 같은 이동이 두 번 쌓였다");
}

#[test]
fn 캐시_적중이_끝난_뒤에는_무관한_폴더의_없음이_히스토리를_되감지_않는다() {
    // `optimistic`을 결과 갈래 끝에서 내리지 않으면, 그 상태가 남아 **나중에 다른 폴더가**
    // 없을 때 옛 스냅샷으로 되감긴다
    let (ctx, mut icons, mut cache) = cache_fixture();
    let start = std::path::PathBuf::from(r"C:\맨처음곳");
    let target = std::path::PathBuf::from(r"C:\거쳐간곳");
    cache.put(&target, &batch(&["..", "a.txt"]));

    let mut panel = PanelState::new(start.clone());
    panel.start_load(target.clone(), PendingNav::Push, &ctx);
    panel.try_cache_hit(&mut icons, &mut cache);
    panel.apply_enumerated(
        EnumOutcome::Ok(batch(&["a.txt"])),
        &mut icons,
        &mut cache,
        &ctx,
    );
    assert!(
        panel.optimistic.is_none(),
        "결과를 받고도 낙관 상태가 남았다"
    );

    // 이제 캐시와 무관한 폴더로 가려는데 그것이 없다 — 제자리를 지켜야 한다
    panel.start_load(
        std::path::PathBuf::from(r"C:\생판다른곳"),
        PendingNav::Push,
        &ctx,
    );
    panel.apply_enumerated(EnumOutcome::NotFound, &mut icons, &mut cache, &ctx);
    assert_eq!(
        panel.dir(),
        target,
        "무관한 폴더의 없음이 옛 스냅샷으로 되감았다"
    );
}

#[test]
fn 읽지_못한_폴더는_되돌리지_않되_캐시를_버린다() {
    // 권한·네트워크 실패는 폴더가 실재하는 경우다 — 그 자리에 머물러 사유를 보이는 것이 종전 규칙
    let (ctx, mut icons, mut cache) = cache_fixture();
    let target = std::path::PathBuf::from(r"C:\막힌곳");
    cache.put(&target, &batch(&["a.txt"]));
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\원래곳"));
    panel.start_load(target.clone(), PendingNav::Push, &ctx);
    panel.try_cache_hit(&mut icons, &mut cache);

    panel.apply_enumerated(EnumOutcome::AccessDenied, &mut icons, &mut cache, &ctx);
    assert_eq!(panel.dir(), target, "읽지 못했다고 되돌아갔다");
    assert!(cache.get(&target).is_none(), "낡은 캐시가 남았다");
}

#[test]
fn 확정된_목록만_캐시에_담긴다() {
    // 중간 조각을 담으면 다음 재진입이 잘린 목록으로 즉시 선다
    let (ctx, mut icons, mut cache) = cache_fixture();
    let target = std::path::PathBuf::from(r"C:\담을곳");
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\원래곳"));
    panel.pending_dir = target.clone();
    panel.pending_nav = PendingNav::None;

    panel.apply_partial(batch(&["a.txt"]), &mut icons);
    assert!(cache.get(&target).is_none(), "중간 조각이 캐시에 담겼다");

    panel.apply_enumerated(
        EnumOutcome::Ok(batch(&["b.txt"])),
        &mut icons,
        &mut cache,
        &ctx,
    );
    // `..` + a + b
    assert_eq!(cache.get(&target).map(<[_]>::len), Some(3));
}

#[test]
fn 캐시로_그린_뒤_도착한_배치는_목록을_줄이지_않는다() {
    // 캐시 임계(5000)와 배치 임계(2000) 사이 구간 — 두 기능에 함께 걸린다.
    // 중간 조각을 그대로 반영하면 목록이 줄었다가 다시 늘어난다
    let (ctx, mut icons, mut cache) = cache_fixture();
    let target = std::path::PathBuf::from(r"C:\겹치는곳");
    cache.put(&target, &batch(&["..", "a.txt", "b.txt", "c.txt"]));
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\원래곳"));
    panel.start_load(target.clone(), PendingNav::Push, &ctx);
    panel.try_cache_hit(&mut icons, &mut cache);
    assert_eq!(panel.list.len(), 4);

    // 실제 열거의 첫 배치가 도착 — 그리지 않고 모아 둔다
    panel.apply_partial(batch(&["a.txt", "b.txt"]), &mut icons);
    assert_eq!(panel.list.len(), 4, "배치가 목록을 줄였다");

    // 완료 조각에서 모아 둔 것과 잔여분을 합쳐 한 번에 갈아 끼운다
    panel.apply_enumerated(
        EnumOutcome::Ok(batch(&["c.txt"])),
        &mut icons,
        &mut cache,
        &ctx,
    );
    assert_eq!(panel.list.len(), 4, "합쳐 넣은 목록이 어긋났다");
}

#[test]
fn 폴더를_옮기면_썸네일을_놓는다() {
    // 이 해제는 `ThumbnailCache`의 세대를 올리는 유일한 지점이기도 하다 —
    // 죽으면 떠난 폴더의 썸네일이 계속 남고(NFR-9), 늦게 도착한 결과도 못 거른다.
    // 커밋을 먼저 하고 비교하면 항상 같아져 이 경로가 통째로 죽는다(F-7 B1)
    let mut icons = IconCache::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    commit_dir(&mut panel, r"C:\Users", &mut icons);

    panel.thumbs.accept_for_test(
        std::path::PathBuf::from(r"C:\Users\사진.jpg"),
        Some(sample_thumb()),
    );
    assert_eq!(panel.thumbs.len(), 1, "사전 준비 실패");

    commit_dir(&mut panel, r"C:\Windows", &mut icons);
    assert_eq!(
        panel.thumbs.len(),
        0,
        "폴더를 옮겼는데 이전 폴더의 썸네일이 남았다"
    );
}

#[test]
fn 탭을_바꿔_폴더가_달라져도_썸네일을_놓는다() {
    // 탭 전환은 `tabs.switch`로 **활성 탭을 먼저 바꾼 뒤** 그 경로를 읽는다 —
    // 커밋 직전 경로와 비교하는 방식이면 이 경로만 빠져나간다(F-7 m1).
    // 그래서 판정을 캐시(`set_folder`)로 옮겼고, 이 테스트가 그것을 지킨다
    let mut icons = IconCache::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    commit_dir(&mut panel, r"C:\Users", &mut icons);
    panel.thumbs.accept_for_test(
        std::path::PathBuf::from(r"C:\Users\사진.jpg"),
        Some(sample_thumb()),
    );

    // 다른 폴더를 보는 탭을 더한다 — `add`가 그 탭을 곧바로 활성으로 만든다
    panel
        .tabs
        .add(crate::panel::tabs::TabState::new(std::path::PathBuf::from(
            r"C:\Windows",
        )));
    commit_dir(&mut panel, r"C:\Windows", &mut icons);
    assert_eq!(
        panel.thumbs.len(),
        0,
        "탭을 바꿔 폴더가 달라졌는데 이전 폴더의 썸네일이 남았다"
    );

    // 되돌아가는 전환도 같아야 한다 — 새 폴더 썸네일을 담아 두고 원래 탭으로 돌아간다
    panel.thumbs.accept_for_test(
        std::path::PathBuf::from(r"C:\Windows\그림.png"),
        Some(sample_thumb()),
    );
    assert!(panel.tabs.switch(0), "첫 탭으로 되돌아가지 못했다");
    commit_dir(&mut panel, r"C:\Users", &mut icons);
    assert_eq!(panel.thumbs.len(), 0, "되돌아가는 전환에서 남았다");
}

#[test]
fn 같은_폴더를_다시_읽으면_썸네일을_지킨다() {
    // 감시 갱신(FR-10)은 같은 폴더를 다시 읽는다 — 그때마다 버리면
    // 다른 앱이 파일 하나만 만들어도 폴더 전체를 다시 만들게 된다
    let mut icons = IconCache::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    commit_dir(&mut panel, r"C:\Users", &mut icons);
    panel.thumbs.accept_for_test(
        std::path::PathBuf::from(r"C:\Users\사진.jpg"),
        Some(sample_thumb()),
    );

    commit_dir(&mut panel, r"C:\Users", &mut icons); // 감시 갱신
    assert_eq!(panel.thumbs.len(), 1, "같은 폴더인데 썸네일을 버렸다");
}

fn sample_thumb() -> crate::fs::thumbnail::ThumbnailImage {
    crate::fs::thumbnail::ThumbnailImage {
        width: 2,
        height: 2,
        rgba: vec![255; 16],
    }
}

#[test]
fn 썸네일을_올린_프레임은_곧바로_다시_그리라고_알린다() {
    // egui는 입력이 없으면 프레임을 돌리지 않는다 — 이 신호가 빠지면 워커가 늦게 준
    // 썸네일이 사용자가 마우스를 움직일 때까지 형식 아이콘에 머문다 (F-8에서 실제로 그랬다)
    let ctx = egui::Context::default();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    panel.thumbs.accept_for_test(
        std::path::PathBuf::from(r"C:\Users\사진.jpg"),
        Some(sample_thumb()),
    );

    assert_eq!(
        panel.poll_thumbnails(&ctx),
        Some(Duration::ZERO),
        "썸네일을 올린 프레임인데 곧바로 다시 그리라고 알리지 않았다"
    );
    assert_eq!(panel.thumb_textures.len(), 1, "텍스처가 올라가지 않았다");
    // 올릴 것도 기다릴 것도 없으면 알리지 않는다 — 늘 알리면 앱이 쉬지 않고 그린다
    assert_eq!(
        panel.poll_thumbnails(&ctx),
        None,
        "할 일이 없는데도 다시 그리라고 알렸다"
    );
}

#[test]
fn 썸네일을_기다리는_동안은_스스로_깨어난다() {
    // 썸네일 워커는 `fs` 계층이라 egui를 모른다 — 결과가 채널에 들어와도 앱은 알 수 없다.
    // 이 신호가 없으면 사진이 사용자가 마우스를 움직일 때까지 안 나타난다(F-8 실측)
    let ctx = egui::Context::default();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    panel
        .thumbs
        .request(std::path::Path::new(r"C:\Users\아직없음.jpg"));

    assert_eq!(
        panel.poll_thumbnails(&ctx),
        Some(THUMB_POLL_INTERVAL),
        "결과를 기다리는데 다시 깨어날 시점을 알리지 않았다"
    );
}

#[test]
fn 열_차례가_세션을_왕복한다() {
    // FR-4 — 이 왕복이 끊기면 재시작할 때마다 기본 차례로 돌아간다
    let saved = crate::ui::session::PanelTabs {
        tabs: vec![crate::ui::session::TabSpec::Local(
            std::path::PathBuf::from(r"C:\"),
        )],
        column_order: vec!["modified".into(), "name".into()],
        ..Default::default()
    };
    let panel = PanelState::from_tabs(&saved).expect("탭이 있으니 되살아난다");
    let round = panel.to_tabs();
    // 빠진 키는 기본 차례대로 뒤에 채워진다
    assert_eq!(
        round.column_order,
        vec!["modified", "name", "size", "type", "permissions", "owner"]
    );
}

#[test]
fn 저장된_열_차례가_없으면_기본_차례로_연다() {
    let saved = crate::ui::session::PanelTabs {
        tabs: vec![crate::ui::session::TabSpec::Local(
            std::path::PathBuf::from(r"C:\"),
        )],
        ..Default::default()
    };
    let panel = PanelState::from_tabs(&saved).expect("탭이 있으니 되살아난다");
    assert_eq!(
        panel.to_tabs().column_order,
        vec!["name", "size", "type", "modified", "permissions", "owner"]
    );
}

#[test]
fn 정렬_기준과_방향이_세션을_왕복한다() {
    // FR-4 — 이 왕복이 끊기면 재시작할 때마다 이름 오름차순으로 되돌아간다
    let saved = crate::ui::session::PanelTabs {
        tabs: vec![crate::ui::session::TabSpec::Local(
            std::path::PathBuf::from(r"C:\"),
        )],
        active_tab: 0,
        columns: Vec::new(),
        view_mode: "tiles".into(),
        sort_key: "size".into(),
        sort_ascending: false,
        column_order: vec!["modified".into(), "name".into()],
    };
    let panel = PanelState::from_tabs(&saved).expect("탭이 있으니 되살아난다");
    let round = panel.to_tabs();
    assert_eq!(round.sort_key, "size");
    assert!(!round.sort_ascending);
    assert_eq!(round.view_mode, "tiles", "보기 모드도 함께 살아 있다");
}

#[test]
fn 저장된_정렬이_없으면_이름_오름차순으로_연다() {
    // 정렬을 한 번도 저장한 적 없는 옛 세션 (FR-4 acceptance ⓒ)
    let saved = crate::ui::session::PanelTabs {
        tabs: vec![crate::ui::session::TabSpec::Local(
            std::path::PathBuf::from(r"C:\"),
        )],
        ..Default::default()
    };
    let panel = PanelState::from_tabs(&saved).expect("탭이 있으니 되살아난다");
    let round = panel.to_tabs();
    assert_eq!(round.sort_key, "name");
    assert!(round.sort_ascending);
}

#[test]
fn 모르는_정렬_키가_담겨_있어도_기본값으로_연다() {
    // 손으로 고친 설정 파일 대비 (FR-4 acceptance ⓓ)
    let saved = crate::ui::session::PanelTabs {
        tabs: vec![crate::ui::session::TabSpec::Local(
            std::path::PathBuf::from(r"C:\"),
        )],
        sort_key: "없는_기준".into(),
        sort_ascending: false,
        ..Default::default()
    };
    let panel = PanelState::from_tabs(&saved).expect("탭이 있으니 되살아난다");
    assert_eq!(panel.to_tabs().sort_key, "name");
}

#[test]
fn 보기_모드를_바꿔도_정렬은_유지된다() {
    // D3 — 정렬은 패널당 하나이고 보기 모드마다 따로 기억하지 않는다 (FR-23)
    let saved = crate::ui::session::PanelTabs {
        tabs: vec![crate::ui::session::TabSpec::Local(
            std::path::PathBuf::from(r"C:\"),
        )],
        sort_key: "modified".into(),
        sort_ascending: false,
        ..Default::default()
    };
    let mut panel = PanelState::from_tabs(&saved).expect("탭이 있으니 되살아난다");
    panel.set_view_mode(ViewMode::LargeIcons);
    let round = panel.to_tabs();
    assert_eq!(round.sort_key, "modified");
    assert!(!round.sort_ascending);
}

#[test]
fn 보기_모드는_패널을_거쳐_목록까지_전달된다() {
    // `Command::SetViewMode`가 닿는 지점이다 — 여기서 끊기면 메뉴에서 골라도
    // 목록은 이전 모드로 그려진다 (FR-23)
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    assert_eq!(panel.view_mode(), ViewMode::Details);
    panel.set_view_mode(ViewMode::SmallIcons);
    assert_eq!(panel.view_mode(), ViewMode::SmallIcons);
}

#[test]
fn 보기_모드는_패널마다_독립이다() {
    // 한 패널에서 바꾼 모드가 다른 패널에 번지면 "패널마다 독립"(FR-23)이 깨진다
    let mut first = PanelState::new(std::path::PathBuf::from(r"C:\"));
    let second = PanelState::new(std::path::PathBuf::from(r"D:\"));
    first.set_view_mode(ViewMode::Tiles);
    assert_eq!(first.view_mode(), ViewMode::Tiles);
    assert_eq!(
        second.view_mode(),
        ViewMode::Details,
        "다른 패널까지 바뀌었다"
    );
}

#[test]
fn 패널_안에서_같은_위젯_id가_두_번_쓰이지_않는다() {
    // 탭 스트립·폴더 트리·파일 목록이 각자 스크롤 영역을 갖는데, 이들이 같은 id를 쓰면
    // 스크롤 위치가 서로 섞인다(화면에는 빨간 경고로 드러난다)
    assert!(
        draw_panel(false).is_empty(),
        "위젯 ID 충돌(트리 숨김): {:?}",
        draw_panel(false)
    );
    assert!(
        draw_panel(true).is_empty(),
        "위젯 ID 충돌(트리 표시): {:?}",
        draw_panel(true)
    );
}

#[test]
fn 원격_목록의_첫_줄은_언제나_상위_이동이다() {
    // 서버가 `..`를 주기도 하고 안 주기도 한다 — 화면은 어느 쪽이든 같아야 한다 (plan T9 ③)
    fn entry(name: &str, is_dir: bool) -> RemoteEntry {
        RemoteEntry {
            name: name.to_owned(),
            is_dir,
            is_symlink: false,
            link_target: None,
            size: 0,
            modified: None,
            mode: None,
            owner: None,
        }
    }

    let 없는_경우 = with_parent_first(vec![entry("public_html", true), entry("a.txt", false)]);
    let names: Vec<&str> = 없는_경우.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["..", "public_html", "a.txt"]);

    let 있는_경우 = with_parent_first(vec![
        entry("..", true),
        entry("public_html", true),
        entry("..", true),
    ]);
    let names: Vec<&str> = 있는_경우.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["..", "public_html"], "`..`가 둘이 되면 안 된다");

    // 빈 폴더에도 상위 이동은 남는다
    assert_eq!(with_parent_first(Vec::new()).len(), 1);
}

/// 로컬 항목 하나 — 이름은 널 종단 UTF-16이라는 불변식을 지켜 만든다
fn local_entry(name: &str, is_dir: bool) -> FileEntry {
    FileEntry {
        name: name.encode_utf16().chain(std::iter::once(0)).collect(),
        is_dir,
        size: 0,
        modified: 0,
        attributes: 0,
    }
}

#[test]
fn 로컬_목록에도_상위_이동_줄이_붙는다() {
    // 사용자 보고(2026-08-13): 원격 목록에는 `..`가 있는데 로컬 목록에는 없었다
    let 보통_폴더 = with_local_parent_first(
        Path::new(r"C:\Program Files"),
        vec![local_entry("Android", true), local_entry("a.txt", false)],
    );
    let names: Vec<String> = 보통_폴더.iter().map(|e| e.name_string()).collect();
    assert_eq!(names, vec!["..", "Android", "a.txt"]);

    // 드라이브 루트에는 올라갈 곳이 없다 — 눌러도 아무 일 없는 줄을 두지 않는다
    let 루트 = with_local_parent_first(Path::new(r"C:\"), vec![local_entry("Windows", true)]);
    let names: Vec<String> = 루트.iter().map(|e| e.name_string()).collect();
    assert_eq!(names, vec!["Windows"]);

    // 열거가 `..`를 함께 주더라도 둘이 되지 않는다
    let 중복 = with_local_parent_first(
        Path::new(r"C:\Users"),
        vec![local_entry("..", true), local_entry("Public", true)],
    );
    let names: Vec<String> = 중복.iter().map(|e| e.name_string()).collect();
    assert_eq!(names, vec!["..", "Public"]);
}

#[test]
fn 로컬_상위_이동을_더블클릭하면_위_폴더로_간다() {
    let ctx = egui::Context::default();
    let mut icons = IconCache::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    // 열거 결과가 도착한 것처럼 커밋시킨다 — 목록을 채우는 실제 경로를 그대로 지난다
    panel.start_load(
        std::path::PathBuf::from(r"C:\Users\Public"),
        PendingNav::Push,
        &ctx,
    );
    panel.apply_enumerated(
        EnumOutcome::Ok(vec![local_entry("Documents", true)]),
        &mut icons,
        &mut crate::panel::dir_cache::DirCache::new(),
        &egui::Context::default(),
    );

    let names: Vec<String> = match panel.list.model() {
        crate::ui::file_list::ListModel::Local(rows) => {
            rows.iter().map(|e| e.name_string()).collect()
        }
        crate::ui::file_list::ListModel::Remote(_) => Vec::new(),
    };
    assert_eq!(names, vec!["..", "Documents"], "첫 줄이 상위 이동이 아니다");
    assert_eq!(
        panel.list.counts(),
        (1, 0),
        "상위 이동 줄을 폴더로 세면 개수가 실제와 달라진다"
    );

    // 첫 줄을 더블클릭하면 위 폴더를 읽으러 간다 — `C:\Users\Public\..`이 아니다
    panel.handle_list_action(FileListAction::Open(0), &ctx);
    assert_eq!(panel.pending_dir, std::path::PathBuf::from(r"C:\Users"));
}

#[test]
fn 원격_탭에서는_로컬_전용_작업이_일어나지_않는다() {
    // 열거·감시·썸네일·새 파일은 로컬에만 있는 일이다 (plan T9 ②)
    let ctx = egui::Context::default();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\테스트"));
    panel.tabs.add(crate::panel::tabs::TabState::remote(
        SiteId(1),
        RemotePath::new("/pub"),
    ));
    assert!(panel.is_remote(), "원격 탭이 활성이어야 한다");

    // 새 폴더·새 파일은 아무 일도 하지 않는다
    panel.new_folder(&ctx);
    panel.new_file(&ctx);
    assert!(
        !panel.create.is_running(),
        "원격 탭에서 로컬 생성이 시작됐다"
    );
    // 연결이 없는 원격 탭에서는 목록 요청도 나가지 않는다
    let manager = ConnectionManager::new(std::sync::Arc::new(|| {}));
    assert_eq!(
        panel.request_remote_list(WorkspaceId(0), PanelId(0), &manager),
        None
    );
}

#[test]
fn 한_패널에_로컬_탭과_원격_탭을_섞을_수_있다() {
    // 탭마다 자기 소스로 그려져야 한다 (plan T9 ⑤)
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\테스트"));
    panel.tabs.add(crate::panel::tabs::TabState::remote(
        SiteId(3),
        RemotePath::new("/var/www"),
    ));

    let sources = panel.tabs.sources();
    assert_eq!(sources.len(), 2);
    assert!(!sources[0].is_remote(), "첫 탭은 로컬이어야 한다");
    assert!(sources[1].is_remote(), "둘째 탭은 원격이어야 한다");
    assert_eq!(sources[1].site(), Some(SiteId(3)));
    assert_eq!(
        sources[1].remote_path().map(|p| p.as_str()),
        Some("/var/www")
    );

    // 로컬 탭으로 돌아오면 다시 로컬 전용 일이 열린다
    assert!(panel.tabs.switch(0));
    assert!(!panel.is_remote());
    assert_eq!(panel.dir(), std::path::Path::new(r"C:\테스트"));
}

/// 원격 탭 하나를 더해 활성으로 만든 패널
fn panel_with_remote_tab(path: &str) -> PanelState {
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\테스트"));
    panel.tabs.add(crate::panel::tabs::TabState::remote(
        SiteId(1),
        RemotePath::new(path),
    ));
    assert!(panel.is_remote(), "원격 탭이 활성이어야 한다");
    panel
}

#[test]
fn 원격_탭에서_새로_고침은_로컬_열거를_걸지_않는다() {
    let ctx = egui::Context::default();
    let mut panel = panel_with_remote_tab("/var/www");
    panel.refresh(&ctx);
    assert!(
        !panel.load.is_loading(),
        "원격 탭에서 로컬 열거 워커가 떴다"
    );
}

#[test]
fn 원격_탭의_상위_이동은_원격_경로로_가고_루트에서_머문다() {
    // plan T9 Edge Case — 루트를 넘어가지 않는다
    let ctx = egui::Context::default();
    let mut panel = panel_with_remote_tab("/var/www");

    panel.handle_nav(NavAction::Up, &ctx);
    assert_eq!(
        panel.tabs.active().source.remote_path().map(|p| p.as_str()),
        Some("/var")
    );
    panel.handle_nav(NavAction::Up, &ctx);
    assert_eq!(
        panel.tabs.active().source.remote_path().map(|p| p.as_str()),
        Some("/")
    );
    // 루트에서 한 번 더 눌러도 그대로다
    panel.handle_nav(NavAction::Up, &ctx);
    assert_eq!(
        panel.tabs.active().source.remote_path().map(|p| p.as_str()),
        Some("/")
    );
    // 로컬 열거 워커도 뜨지 않았다
    assert!(!panel.load.is_loading());
}

#[test]
fn 원격_탭에서는_셸_메뉴를_요청하지_않는다() {
    // 셸은 로컬 PIDL만 다룬다 (D21)
    let ctx = egui::Context::default();
    let mut panel = panel_with_remote_tab("/var/www");
    let request = panel.handle_list_action(
        FileListAction::Context {
            index: None,
            pos: egui::pos2(0.0, 0.0),
        },
        &ctx,
    );
    assert!(request.is_none(), "원격 탭에서 셸 메뉴가 요청됐다");

    // 항목 열기도 로컬 경로를 만들지 않는다
    let opened = panel.handle_list_action(FileListAction::Open(0), &ctx);
    assert!(opened.is_none());
    assert!(!panel.load.is_loading());
}

#[test]
fn 원격_탭을_보는_동안_로컬_감시_통지는_무시된다() {
    // 이전 폴더의 감시가 아직 살아 있어도 원격 화면이 로컬 목록으로 덮이면 안 된다
    let ctx = egui::Context::default();
    let mut panel = panel_with_remote_tab("/var/www");
    let (tx, rx) = std::sync::mpsc::channel();
    panel.watch = Some(DirWatch {
        watcher: crate::fs::watcher::DirWatcher::start(
            std::path::PathBuf::from(r"C:\테스트"),
            tx,
            None,
        ),
        rx,
    });

    panel.poll_watch(&ctx);
    assert!(
        !panel.load.is_loading(),
        "감시 통지로 로컬 열거 워커가 떴다"
    );
}

#[test]
fn 원격_탭에는_사이트_이름과_단계_배지가_함께_보인다() {
    // 인벤토리 #11~13 — 이름은 사이트 설정에서, 배지 문구는 단계에서 온다 (Acceptance ①).
    // 탭이 이름 사본을 들면 `이름 바꾸기(R)` 뒤에 탭만 옛 이름으로 남는다
    let 빈_탭 = remote_screen_texts(TabPhase::New);
    assert!(
        빈_탭.iter().any(|t| t == "배포 서버"),
        "사이트 이름이 탭에 없다: {빈_탭:?}"
    );
    assert!(
        빈_탭.iter().any(|t| t == "연결 없음"),
        "미연결 배지가 없다: {빈_탭:?}"
    );
    assert!(
        remote_screen_texts(TabPhase::Connecting)
            .iter()
            .any(|t| t == "연결 중…"),
        "연결 중 배지가 없다"
    );
    // 연결되면 배지가 프로토콜 이름으로 바뀐다 (새 사이트의 기본값은 FTP다)
    assert!(
        remote_screen_texts(TabPhase::Ok).iter().any(|t| t == "ftp"),
        "연결됨 배지가 프로토콜을 보이지 않는다"
    );
}

#[test]
fn 사이트를_아는_미연결_탭은_안내_대신_다시_연결을_보인다() {
    // 사용자 보고(2026-08-13): 재시작하면 원격 탭이 사이트·경로를 되찾고도 "주소창에
    // sftp://호스트 를 입력해 연결하세요"를 보였다 — 이미 아는 것을 다시 묻는 화면이다
    let 화면 = remote_screen_texts(TabPhase::New);
    assert!(
        화면.iter().any(|t| t == "다시 연결"),
        "다시 연결 버튼이 없다: {화면:?}"
    );
    for 사라져야_할_문구 in ["sftp://호스트", "끌어다 놓아도 됩니다"] {
        assert!(
            !화면.iter().any(|t| t.contains(사라져야_할_문구)),
            "'{사라져야_할_문구}'가 남아 있다: {화면:?}"
        );
    }
    // 붙을 사이트를 화면 밖(앱)이 알아낼 수 있어야 버튼이 일을 한다
    let (panel, _) = remote_panel_in(TabPhase::New);
    assert!(panel.active_site().is_some(), "탭이 사이트를 잃었다");
}

#[test]
fn 사이트를_찾을_수_없는_탭에는_주소_안내가_남는다() {
    // 사이트를 지운 뒤 남은 탭은 붙을 곳을 모른다 — 그 탭에는 다시 알려 주어야 한다
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\테스트"));
    panel.tabs.add(crate::panel::tabs::TabState::remote(
        SiteId(999),
        RemotePath::new("/var/www"),
    ));
    let 화면 = drawn_texts(&draw_once(&mut panel, &SiteStore::new()));
    assert!(
        화면.iter().any(|t| t.contains("sftp://호스트")),
        "주소 안내가 사라졌다: {화면:?}"
    );
    assert!(
        !화면.iter().any(|t| t == "다시 연결"),
        "붙을 곳을 모르는데 다시 연결을 보인다: {화면:?}"
    );
}

#[test]
fn 단계마다_본문이_통째로_달라진다() {
    // README §4·§5 — 연결 전·중·실패에 목록 대신 그 단계의 화면이 보인다 (Acceptance ①③④)
    // 사이트를 아는 미연결 탭에는 `다시 연결` 버튼이 선다 (아래 두 테스트가 자세히 본다)
    let 미연결 = remote_screen_texts(TabPhase::New);
    assert!(
        미연결.iter().any(|t| t == "다시 연결"),
        "미연결 화면에 다시 연결이 없다: {미연결:?}"
    );

    let 연결_중 = remote_screen_texts(TabPhase::Connecting);
    assert!(
        연결_중.iter().any(|t| t == "취소"),
        "연결 중 취소 버튼이 없다: {연결_중:?}"
    );

    let 실패 = remote_screen_texts(TabPhase::Error {
        message: "530 Login incorrect".to_owned(),
        kind: crate::remote::types::FailureKind::Auth,
    });
    for 문구 in [
        "연결하지 못했습니다",
        "재시도",
        "설정 열기",
        "서버 로그 보기",
    ] {
        assert!(
            실패.iter().any(|t| t.contains(문구)),
            "실패 화면에 '{문구}'가 없다: {실패:?}"
        );
    }
    assert!(
        실패.iter().any(|t| t.contains("530 Login incorrect")),
        "서버가 준 사유가 보이지 않는다: {실패:?}"
    );
}

#[test]
fn 연결되지_않은_원격_패널은_항목_수를_모른다고_보인다() {
    // 인벤토리 #95 — `폴더 0 파일 0`으로 보이면 "빈 폴더"라는 없는 말을 하게 된다
    for phase in [
        TabPhase::New,
        TabPhase::Connecting,
        TabPhase::Error {
            message: "530".to_owned(),
            kind: crate::remote::types::FailureKind::Auth,
        },
    ] {
        let texts = remote_screen_texts(phase.clone());
        assert!(
            texts
                .iter()
                .any(|t| t == crate::ui::remote_states::UNKNOWN_COUNT),
            "{phase:?}에서 `—`가 보이지 않는다: {texts:?}"
        );
        assert!(
            !texts.iter().any(|t| is_item_count(t)),
            "{phase:?}인데 항목 수를 세어 보였다: {texts:?}"
        );
    }
    // 연결되면 보통의 항목 수로 돌아온다
    let 연결됨 = remote_screen_texts(TabPhase::Ok);
    assert!(
        연결됨.iter().any(|t| is_item_count(t)),
        "연결됐는데 항목 수가 없다: {연결됨:?}"
    );
}

/// 상태 줄의 항목 수 표시인가 — 트리 토글(`폴더 트리`)과 구분한다
fn is_item_count(text: &str) -> bool {
    text.starts_with("폴더 ") && text.contains("파일 ")
}

#[test]
fn 원격_탭_화면에서도_위젯_id가_겹치지_않는다() {
    // Acceptance ⑧ — 단계별 화면이 목록 자리에 들어와도 id 공간이 섞이면 안 된다
    for phase in [
        TabPhase::New,
        TabPhase::Connecting,
        TabPhase::Error {
            message: "530".to_owned(),
            kind: crate::remote::types::FailureKind::Auth,
        },
        TabPhase::Ok,
    ] {
        let (mut panel, sites) = remote_panel_in(phase.clone());
        let clashes = id_clash_warnings(&draw_once(&mut panel, &sites));
        assert!(
            clashes.is_empty(),
            "{phase:?}에서 위젯 ID 충돌: {clashes:?}"
        );
    }
}

#[test]
fn 패널의_마지막_원격_탭을_닫으면_연결을_접는다() {
    // FR-32 — 같은 연결을 쓰는 탭이 남아 있으면 접지 않는다 (Acceptance ⑥)
    let ctx = egui::Context::default();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\테스트"));
    panel.tabs.add(crate::panel::tabs::TabState::remote(
        SiteId(1),
        RemotePath::new("/a"),
    ));
    panel.attach_conn(ConnectionId(5));
    panel.tabs.add(crate::panel::tabs::TabState::remote(
        SiteId(1),
        RemotePath::new("/b"),
    ));
    panel.attach_conn(ConnectionId(5));

    assert_eq!(
        panel.handle_tab(TabAction::Close(2), &ctx),
        None,
        "같은 연결을 쓰는 탭이 남았는데 연결을 접으려 했다"
    );
    assert_eq!(
        panel.handle_tab(TabAction::Close(1), &ctx),
        Some(ConnectionId(5)),
        "마지막 원격 탭을 닫았는데 연결이 남았다"
    );
    // 로컬 탭만 남았으니 더 접을 것이 없다
    assert!(!panel.is_remote());
}

#[test]
fn 연결_단계는_그_연결을_쓰는_모든_탭에_퍼진다() {
    // 배경 탭이 옛 단계로 남으면 그 탭으로 돌아갔을 때 화면이 실제와 어긋난다
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\테스트"));
    panel.tabs.add(crate::panel::tabs::TabState::remote(
        SiteId(1),
        RemotePath::new("/a"),
    ));
    panel.attach_conn(ConnectionId(5));
    panel.tabs.add(crate::panel::tabs::TabState::remote(
        SiteId(1),
        RemotePath::new("/b"),
    ));
    panel.attach_conn(ConnectionId(7)); // 다른 연결을 쓰는 탭

    assert!(panel.set_phase_for(ConnectionId(5), &TabPhase::Ok));
    let phases: Vec<TabPhase> = panel
        .tabs
        .sources()
        .iter()
        .filter_map(|source| match source {
            TabSource::Remote { phase, .. } => Some(phase.clone()),
            TabSource::Local(_) => None,
        })
        .collect();
    assert_eq!(
        phases,
        vec![TabPhase::Ok, TabPhase::Connecting],
        "다른 연결의 탭까지 바뀌었거나, 대상 탭이 바뀌지 않았다"
    );
    // 없는 연결에는 아무 일도 일어나지 않는다
    assert!(!panel.set_phase_for(ConnectionId(99), &TabPhase::Ok));
}

#[test]
fn 남의_답이나_지난_위치의_목록은_받지_않는다() {
    // 세대만 보면 한 연결을 두 패널이 나눠 쓸 때 남의 답을 제 목록으로 삼는다
    let mut icons = IconCache::new();
    let mut panel = panel_with_remote_tab("/var/www");
    panel.attach_conn(ConnectionId(1));
    let manager = ConnectionManager::new(std::sync::Arc::new(|| {}));
    // 연결이 죽어 있어도 세대는 올라간다 — 여기서는 세대·위치 판정만 본다
    panel.request_remote_list(WorkspaceId(0), PanelId(0), &manager);
    let generation = panel.remote_seq;

    assert!(!panel.awaits_remote_list(generation + 1, &RemotePath::new("/var/www")));
    assert!(!panel.awaits_remote_list(generation, &RemotePath::new("/etc")));
    assert!(panel.awaits_remote_list(generation, &RemotePath::new("/var/www")));
    assert!(!panel.apply_remote_listed(
        generation,
        &RemotePath::new("/etc"),
        Vec::new(),
        &mut icons
    ));
    assert!(panel.apply_remote_listed(
        generation,
        &RemotePath::new("/var/www"),
        Vec::new(),
        &mut icons
    ));
}

#[test]
fn 만드는_중_원격_탭으로_옮겨_가면_로컬_열거를_걸지_않는다() {
    // 새 폴더를 만드는 워커가 도는 사이 원격 탭으로 옮겨 가면, 완료 시점의 활성 탭은
    // 원격이다 — 그때 활성 탭 기준으로 다시 읽으면 빈 경로를 열거하게 된다
    use std::time::{Duration, Instant};

    let ctx = egui::Context::default();
    let dir = std::env::temp_dir().join(format!("fe_t9_생성_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);

    let mut panel = PanelState::new(dir.clone());
    panel.new_folder(&ctx);
    assert!(panel.create.is_running(), "생성이 시작되지 않았다");

    // 만드는 사이 원격 탭으로 옮겨 간다
    panel.tabs.add(crate::panel::tabs::TabState::remote(
        SiteId(1),
        RemotePath::new("/var/www"),
    ));
    assert!(panel.is_remote());

    let deadline = Instant::now() + Duration::from_secs(3);
    while panel.create.is_running() && Instant::now() < deadline {
        panel.poll_create(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!panel.create.is_running(), "생성이 끝나지 않았다");
    // **`load.pending`으로 본다** — `pending_dir`은 원격 탭에서 어차피 빈 경로라
    // 가드 유무를 가리지 못한다. 열거 워커가 떴는지가 유일하게 둘을 가르는 신호다
    assert!(
        !panel.load.is_loading(),
        "원격 탭인데 로컬 열거 워커가 떴다"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn 새_탭에서_열기는_그_폴더를_가리키는_탭을_더한다() {
    // 우클릭 메뉴의 `새 탭에서 열기` (FR-3·FR-8 재개정) — `Ctrl+T`(`new_tab`)와 달리
    // **열 곳을 받는다**. 그쪽은 보고 있는 곳을 복제하는 길이라 대상을 고를 수 없다
    let ctx = egui::Context::default();
    let mut panel = PanelState::new(PathBuf::from(r"C:\시작"));
    assert_eq!(panel.tabs.len(), 1);

    let 대상 = PathBuf::from(r"D:\작업\하위");
    panel.open_local_tab(대상.clone(), &ctx);

    assert_eq!(panel.tabs.len(), 2, "탭이 하나 늘어야 한다");
    assert_eq!(
        panel.tabs.active().source.local_path(),
        Some(대상.as_path()),
        "새 탭이 그 폴더를 가리켜야 한다"
    );
    // 먼저 있던 탭은 그대로다 — 새 탭이 그 자리를 빼앗지 않는다
    assert_eq!(
        panel.tabs.sources().first().and_then(|s| s.local_path()),
        Some(std::path::Path::new(r"C:\시작"))
    );
}

#[test]
fn 우클릭_요청은_폴더_여부를_목록과_같은_차례로_싣는다() {
    // 메뉴의 `즐겨찾기에 추가`·`새 탭에서 열기`가 그 값으로 열릴지를 가린다 (FR-8 재개정).
    // **받는 쪽에서 `is_dir()`을 부르면 UI 스레드가 파일시스템을 묻게 되므로** 목록이
    // 이미 아는 값을 실어 보낸다 — 그 짝이 어긋나면 엉뚱한 항목을 폴더로 보게 된다
    let (mut panel, ctx) = panel_with_local_rows(
        r"C:\테스트",
        vec![local_entry("하위", true), local_entry("보고서.txt", false)],
    );
    let request = panel
        .handle_list_action(
            FileListAction::Context {
                index: Some(1),
                pos: egui::pos2(0.0, 0.0),
            },
            &ctx,
        )
        .expect("로컬 탭에서는 요청이 만들어져야 한다");
    // 목록이 주는 짝을 그대로 갈라 실은 것이다 — 길이도 차례도 같다
    let 고른것 = panel.selected_local();
    assert_eq!(request.items.len(), request.dirs.len(), "짝이 어긋났다");
    assert_eq!(
        request.items,
        고른것
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        request.dirs,
        고른것.iter().map(|(_, is_dir)| *is_dir).collect::<Vec<_>>()
    );

    // 빈 곳 우클릭이면 둘 다 빈다 — 배경 메뉴라 대상이 없다
    let 배경 = panel
        .handle_list_action(
            FileListAction::Context {
                index: None,
                pos: egui::pos2(0.0, 0.0),
            },
            &ctx,
        )
        .expect("배경 메뉴도 요청은 만들어진다");
    assert!(배경.items.is_empty() && 배경.dirs.is_empty());
}

#[test]
fn 원격_목록의_우클릭은_셸_메뉴를_띄우지_않는다() {
    // Acceptance ⑤ — 셸 메뉴는 로컬 경로가 있어야 뜬다(D21). 원격 탭에서는 자체 메뉴다
    let mut panel = PanelState::new(PathBuf::from(r"C:\"));
    let pos = egui::pos2(120.0, 80.0);

    let ctx = egui::Context::default();
    let request = panel.handle_list_action(
        FileListAction::Context {
            index: Some(0),
            pos,
        },
        &ctx,
    );
    assert!(request.is_some(), "로컬 탭에서는 셸 메뉴를 청해야 한다");
    assert!(panel.remote_menu_at.is_none(), "로컬 탭에 원격 메뉴가 떴다");

    panel.open_remote_tab(SiteId(1), RemotePath::new("/var/www"));
    let request = panel.handle_list_action(
        FileListAction::Context {
            index: Some(0),
            pos,
        },
        &ctx,
    );
    assert!(request.is_none(), "원격 탭에서 셸 메뉴를 청했다");
    assert_eq!(panel.remote_menu_at, Some(pos), "원격 메뉴가 뜨지 않았다");
}

#[test]
fn 갓_나뉜_패널은_원격_탭_하나만_갖는다() {
    // 사용자 보고 — 연결을 열면 시작 폴더 탭이 함께 남아 탭이 둘이었다
    let mut panel = PanelState::new(PathBuf::from(r"C:\"));
    assert_eq!(panel.tabs.len(), 1, "새 패널은 탭 하나로 시작한다");

    panel.open_remote_tab_only(SiteId(1), RemotePath::new("/var/www"));
    assert_eq!(panel.tabs.len(), 1, "원격 탭만 남아야 한다");
    assert!(
        matches!(panel.tabs.active().source, TabSource::Remote { .. }),
        "남은 탭이 원격이 아니다"
    );

    // 쓰던 패널에 여는 길(`open_remote_tab`)은 그대로 더한다 — 그 탭들은 사용자가 열었다
    panel.open_remote_tab(SiteId(2), RemotePath::root());
    assert_eq!(panel.tabs.len(), 2, "기존 탭을 지우면 안 된다");
}

#[test]
fn 마지막_탭을_닫으면_패널_닫기를_청한다() {
    // 사용자 보고 — 원격 탭이 홀로 있는 패널에서 ✕가 아무 반응도 하지 않았다
    let ctx = egui::Context::default();
    let mut alone = PanelState::new(PathBuf::from(r"C:\"));
    alone.open_remote_tab_only(SiteId(1), RemotePath::root());
    alone.handle_tab(TabAction::Close(0), &ctx);
    assert_eq!(alone.tabs.len(), 1, "마지막 탭 자체는 남는다");
    assert!(alone.close_requested, "패널 닫기를 청하지 않았다");

    // 탭이 둘이면 탭만 닫힌다 — 패널은 그대로 둔다
    let mut pair = PanelState::new(PathBuf::from(r"C:\"));
    pair.open_remote_tab(SiteId(1), RemotePath::root());
    pair.handle_tab(TabAction::Close(1), &ctx);
    assert_eq!(pair.tabs.len(), 1);
    assert!(!pair.close_requested, "탭이 남았는데 패널을 닫으려 했다");
}

#[test]
fn 사이트를_지우면_그_사이트_탭만_닫힌다() {
    // 사이드바에서 사이트를 지울 때의 길 (FR-29) — 다른 사이트·로컬 탭은 그대로 둔다
    let ctx = egui::Context::default();
    let mut panel = PanelState::new(PathBuf::from(r"C:\"));
    panel.open_remote_tab(SiteId(1), RemotePath::new("/a"));
    panel.attach_conn(ConnectionId(7));
    panel.open_remote_tab(SiteId(2), RemotePath::new("/b"));
    panel.attach_conn(ConnectionId(8));
    panel.open_remote_tab(SiteId(1), RemotePath::new("/c"));
    panel.attach_conn(ConnectionId(7));
    // 아직 연결이 붙지 않은 탭(`conn: None`)도 그 사이트의 것이면 닫힌다
    panel.open_remote_tab(SiteId(1), RemotePath::new("/d"));
    assert_eq!(panel.tabs.len(), 5);

    assert!(
        !panel.close_site_tabs(SiteId(1), &ctx),
        "다른 탭이 남았는데 마지막 탭이라고 알렸다"
    );
    assert_eq!(panel.tabs.len(), 2, "사이트 1의 탭 셋이 닫히지 않았다");
    assert_eq!(
        panel.conns(),
        vec![ConnectionId(8)],
        "지운 사이트의 연결이 탭에 남았다"
    );
    assert!(!panel.close_requested, "패널 닫기를 앱 대신 청했다");
    // 없는 사이트를 지우면 아무 일도 없다
    assert!(!panel.close_site_tabs(SiteId(9), &ctx));
    assert_eq!(panel.tabs.len(), 2);
}

#[test]
fn 그_사이트_탭뿐인_패널은_마지막_하나를_남긴다() {
    // 탭 목록은 비울 수 없다 — 패널을 닫을지 로컬 탭으로 되돌릴지는 앱이 정한다(FR-2)
    let ctx = egui::Context::default();
    let mut panel = PanelState::new(PathBuf::from(r"C:\"));
    panel.open_remote_tab_only(SiteId(1), RemotePath::new("/a"));
    panel.open_remote_tab(SiteId(1), RemotePath::new("/b"));
    assert_eq!(panel.tabs.len(), 2);

    assert!(
        panel.close_site_tabs(SiteId(1), &ctx),
        "마지막 하나가 남았다는 것을 알리지 않았다"
    );
    assert_eq!(panel.tabs.len(), 1, "탭 목록을 비웠다");
    assert!(
        matches!(panel.tabs.active().source, TabSource::Remote { site, .. } if site == SiteId(1)),
        "남은 탭이 그 사이트의 것이 아니다"
    );

    // 앱이 로컬 탭을 열어 주면 그 뒤에는 마저 닫힌다 (T4의 마지막 패널 폴백)
    panel.new_tab(&ctx);
    assert!(!panel.close_site_tabs(SiteId(1), &ctx));
    assert_eq!(panel.tabs.len(), 1);
    assert!(
        matches!(panel.tabs.active().source, TabSource::Local(_)),
        "로컬 탭이 남지 않았다"
    );
}

#[test]
fn 패널이_쓰는_연결은_중복_없이_모인다() {
    // 패널을 닫을 때 회수 대상을 고르는 근거다 (FR-32)
    let mut panel = PanelState::new(PathBuf::from(r"C:\"));
    assert!(panel.conns().is_empty(), "로컬 탭만 있는데 연결이 잡혔다");

    panel.open_remote_tab(SiteId(1), RemotePath::new("/a"));
    panel.attach_conn(ConnectionId(7));
    panel.open_remote_tab(SiteId(1), RemotePath::new("/b"));
    panel.attach_conn(ConnectionId(7));
    assert_eq!(
        panel.conns(),
        vec![ConnectionId(7)],
        "같은 연결이 두 번 담겼다"
    );
}

#[test]
fn 권한이_없으면_그_폴더로_옮기고_목록_자리에_사유를_적는다() {
    // 2026-08-16 사용자 요청 — 종전에는 이전 목록을 그대로 둔 채 상태 줄에만 사유를 적어,
    // 주소창·트리가 가리키는 곳과 목록이 갈렸다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let mut icons = IconCache::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    // 첫 프레임의 시작 열거를 걸지 않는다 — 그 결과가 오면 이 시험이 만든 상태를 덮는다
    panel.deferred_start = None;
    commit_dir(&mut panel, r"C:\Users", &mut icons);

    let denied = std::path::PathBuf::from(r"C:\Documents and Settings");
    panel.pending_dir = denied.clone();
    panel.pending_nav = PendingNav::Push;
    panel.apply_enumerated(
        EnumOutcome::AccessDenied,
        &mut icons,
        &mut crate::panel::dir_cache::DirCache::new(),
        &egui::Context::default(),
    );

    assert_eq!(
        panel.tabs.active().source.local_path(),
        Some(denied.as_path()),
        "권한이 막힌 폴더로 옮기지 않았다"
    );
    assert!(
        panel.status.is_empty(),
        "상태 줄에 사유가 남았다: {}",
        panel.status
    );
    assert_eq!(panel.list.counts(), (0, 0), "이전 목록이 남았다");
    assert_eq!(
        panel.blocked_hint(),
        Some("이 폴더를 열 권한이 없어 내용을 표시할 수 없습니다"),
        "사유를 적을 상태가 아니다"
    );
    assert!(panel.watch.is_none(), "읽지 못한 폴더를 감시하고 있다");

    // 그린 화면에도 그 말이 있다 — 판정 헬퍼만 보면 그리기가 죽어도 통과한다(F-7 B1)
    let texts = drawn_texts(&draw_once(&mut panel, &SiteStore::new()));
    assert!(
        texts
            .iter()
            .any(|text| text == "이 폴더를 열 권한이 없어 내용을 표시할 수 없습니다"),
        "목록 자리에 사유가 없다: {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text.contains("권한이 없습니다")),
        "상태 줄 문구가 남아 있다: {texts:?}"
    );

    // 안내는 `..` 줄과 겹치지 않는다 — 겹치면 두 글이 포개져 둘 다 읽히지 않는다
    // (2026-08-16 사용자 보고)
    let placed = drawn_text_positions(&draw_once(&mut panel, &SiteStore::new()));
    let 안내 = placed
        .iter()
        .find(|(text, _)| text.starts_with("이 폴더를 열 권한이"))
        .expect("권한 안내")
        .1;
    let 첫줄 = placed
        .iter()
        .find(|(text, _)| text == "..")
        .expect("`..` 줄")
        .1;
    assert!(
        안내.y > 첫줄.y + crate::ui::list_details::ROW_HEIGHT,
        "안내가 `..` 줄과 겹친다 (안내 {}, 첫 줄 {})",
        안내.y,
        첫줄.y
    );

    // 읽어 낸 폴더로 옮기면 안내는 사라진다
    commit_dir(&mut panel, r"C:\Users", &mut icons);
    assert!(panel.blocked_hint().is_none(), "안내가 그대로 남았다");
}

/// 열거 실패 하나를 겪은 패널을 돌려준다 — 사유별 시험이 같은 무대를 쓴다
fn panel_after_failure(
    outcome: EnumOutcome,
    icons: &mut IconCache,
) -> (PanelState, std::path::PathBuf) {
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    // 첫 프레임의 시작 열거를 걸지 않는다 — 그 결과가 오면 이 시험이 만든 상태를 덮는다
    panel.deferred_start = None;
    commit_dir(&mut panel, r"C:\Users", icons);

    let target = std::path::PathBuf::from(r"Z:\");
    panel.pending_dir = target.clone();
    panel.pending_nav = PendingNav::Push;
    panel.apply_enumerated(
        outcome,
        icons,
        &mut crate::panel::dir_cache::DirCache::new(),
        &egui::Context::default(),
    );
    (panel, target)
}

#[test]
fn 네트워크가_끊기면_그_경로로_옮기고_목록_자리에_사유를_적는다() {
    // T2 Acceptance — 종전에는 이전 폴더의 목록이 그대로 남아, 트리에서 끊긴 드라이브를
    // 눌러도 오른쪽에는 엉뚱한 폴더가 보였다 (2026-08-17 사용자 요청)
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let mut icons = IconCache::new();
    let (mut panel, target) = panel_after_failure(EnumOutcome::Error { network: true }, &mut icons);

    assert_eq!(
        panel.tabs.active().source.local_path(),
        Some(target.as_path()),
        "끊긴 드라이브로 옮기지 않았다"
    );
    assert!(
        panel.status.is_empty(),
        "상태 줄에 사유가 남았다: {}",
        panel.status
    );
    assert_eq!(panel.list.counts(), (0, 0), "이전 목록이 남았다");
    assert!(panel.watch.is_none(), "읽지 못한 폴더를 감시하고 있다");

    // 그린 화면에도 그 말이 있다 — 판정 헬퍼만 보면 그리기가 죽어도 통과한다(F-7 B1)
    let texts = drawn_texts(&draw_once(&mut panel, &SiteStore::new()));
    assert!(
        texts
            .iter()
            .any(|text| text == "네트워크 드라이브에 연결할 수 없어 내용을 표시할 수 없습니다"),
        "목록 자리에 네트워크 사유가 없다: {texts:?}"
    );
}

#[test]
fn 그_밖의_열기_실패도_같은_자리에_사유를_적는다() {
    // T2 Acceptance — 네트워크가 아닌 실패는 문구만 다르고 처리는 같다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let mut icons = IconCache::new();
    let (mut panel, target) =
        panel_after_failure(EnumOutcome::Error { network: false }, &mut icons);

    assert_eq!(
        panel.tabs.active().source.local_path(),
        Some(target.as_path()),
        "실패한 경로로 옮기지 않았다"
    );
    assert!(panel.status.is_empty(), "상태 줄에 사유가 남았다");
    let texts = drawn_texts(&draw_once(&mut panel, &SiteStore::new()));
    assert!(
        texts
            .iter()
            .any(|text| text == "이 폴더를 여는 중 문제가 생겨 내용을 표시할 수 없습니다"),
        "목록 자리에 일반 실패 사유가 없다: {texts:?}"
    );
}

#[test]
fn 찾을_수_없는_폴더는_현_위치를_지킨다() {
    // T2 Acceptance — 실재하지 않는 곳에는 옮길 자리가 없어 종전 규칙을 지킨다.
    // 사유는 목록 자리가 아니라 상태 줄에 적힌다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let mut icons = IconCache::new();
    let here = std::path::PathBuf::from(r"C:\Users");
    let (panel, _) = panel_after_failure(EnumOutcome::NotFound, &mut icons);

    assert_eq!(
        panel.tabs.active().source.local_path(),
        Some(here.as_path()),
        "없는 폴더로 옮겼다"
    );
    assert!(
        panel.status.contains("찾을 수 없습니다"),
        "상태 줄에 사유가 없다: {}",
        panel.status
    );
    assert!(
        panel.blocked_hint().is_none(),
        "목록 자리에 사유를 적을 상태가 됐다"
    );
}

#[test]
fn 즐겨찾기는_드라이브_뿌리보다_위에_구분선과_함께_선다() {
    // FR-56 — 트리 맨 위가 바로가기 자리다. 구분선이 그 아래를 가른다
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;
    let favorites = [user_favorite(r"D:\작업"), user_favorite(r"C:\Users")];

    let output = draw_once_with_favorites(&mut panel, &SiteStore::new(), &favorites);
    let texts = drawn_text_positions(&output);

    let 작업 = texts
        .iter()
        .find(|(text, pos)| text == "작업" && pos.x < TREE_WIDTH)
        .expect("즐겨찾기 `작업` 줄")
        .1;
    let users = texts
        .iter()
        .find(|(text, pos)| text == "Users" && pos.x < TREE_WIDTH)
        .expect("즐겨찾기 `Users` 줄")
        .1;
    // 더한 차례 그대로다 (사용자 결정: 이름순이 아니다)
    assert!(작업.y < users.y, "추가한 차례가 뒤바뀌었다");

    // 드라이브 뿌리는 그 아래에 선다 — 주소창에도 `C:\` 같은 경로가 있어 **트리 구역만** 본다
    // (트리는 상태 줄 아래에서 시작한다)
    let 토글 = texts
        .iter()
        .find(|(text, _)| text == TREE_TOGGLE_ICON)
        .expect("트리 토글 아이콘")
        .1;
    let 이름들 = drive_labels();
    let 드라이브 = texts
        .iter()
        .filter(|(text, pos)| {
            pos.x < TREE_WIDTH && pos.y > 토글.y && 이름들.iter().any(|(_, label)| label == text)
        })
        .map(|(_, pos)| pos.y)
        .fold(f32::INFINITY, f32::min);
    assert!(
        드라이브.is_finite(),
        "드라이브 뿌리가 그려지지 않았다: {texts:?}"
    );
    assert!(
        users.y < 드라이브,
        "즐겨찾기가 드라이브 아래로 내려갔다 (즐겨찾기 {}, 드라이브 {드라이브})",
        users.y
    );
}

#[test]
fn 즐겨찾기가_없으면_구분선도_그리지_않는다() {
    // 사용자 결정 — 쓰지 않는 사람의 화면은 지금과 똑같아야 한다
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;

    let 빈_즐겨찾기 = separator_count(&draw_once_with_favorites(
        &mut panel,
        &SiteStore::new(),
        &[],
    ));
    let 한_건 = separator_count(&draw_once_with_favorites(
        &mut panel,
        &SiteStore::new(),
        &[user_favorite(r"D:\작업")],
    ));

    assert_eq!(
        한_건,
        빈_즐겨찾기 + 1,
        "즐겨찾기 구분선이 하나 늘지 않았다 (빈 {빈_즐겨찾기}, 한 건 {한_건})"
    );
}

#[test]
fn 원격_트리에는_즐겨찾기가_서지_않는다() {
    // 사용자 명시 제외 — 원격 패널에서는 바로가기 자체가 뜻이 없다(로컬 경로다)
    let (mut panel, sites) = remote_panel_in(TabPhase::Ok);
    panel.tree_visible = true;
    let favorites = [user_favorite(r"D:\작업")];

    let texts = drawn_texts(&draw_once_with_favorites(&mut panel, &sites, &favorites));

    assert!(
        !texts.iter().any(|text| text == "작업"),
        "원격 트리에 즐겨찾기가 그려졌다: {texts:?}"
    );
}

#[test]
fn 즐겨찾기를_끌어_차례를_바꾼다() {
    // FR-56 acceptance ⓐ — 셋째를 첫째 위로 끈다.
    // `FavoriteStore::reorder`를 직접 부르면 트리의 드래그 판정이 시험을 비켜간다
    let favorites = [
        user_favorite(r"D:\하나"),
        user_favorite(r"D:\둘"),
        user_favorite(r"D:\셋"),
    ];
    let mut harness = FavoriteHarness::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;

    let first = harness.frame(&mut panel, &favorites);
    let spot = |output: &eframe::egui::FullOutput, name: &str| {
        drawn_text_positions(output)
            .into_iter()
            .find(|(text, pos)| text == name && pos.x < TREE_WIDTH)
            .map(|(_, pos)| egui::pos2(pos.x + 8.0, pos.y + 6.0))
    };
    let 셋 = spot(&first, "셋").expect("셋째 줄");
    let 하나 = spot(&first, "하나").expect("첫째 줄");

    // 누르고 → 임계를 넘겨 위로 끌고 → 첫째 줄 위쪽에서 놓는다
    let 놓는_자리 = egui::pos2(하나.x, 하나.y - 6.0);
    let mut picked = None;
    for (time, event) in [
        // **누르기 전에 그 자리로 옮긴다** — 포인터 자리를 모른 채 누르면 egui가 끌기의
        // 출발점을 잡지 못한다
        (0.00, egui::Event::PointerMoved(셋)),
        (0.05, press_at(셋, true)),
        (
            0.10,
            egui::Event::PointerMoved(egui::pos2(놓는_자리.x, (셋.y + 놓는_자리.y) / 2.0)),
        ),
        (0.15, egui::Event::PointerMoved(놓는_자리)),
        (0.20, press_at(놓는_자리, false)),
    ] {
        let input = egui::RawInput {
            time: Some(time),
            events: vec![event],
            ..Default::default()
        };
        let (_, action) = harness.draw(&mut panel, &favorites, input);
        picked = picked.or(action);
    }

    assert_eq!(
        picked,
        Some(FavoriteAction::Reorder { from: 2, to: 0 }),
        "끌어 놓았는데 차례 바꾸기가 올라오지 않았다"
    );
}

#[test]
fn 임계를_넘기_전에는_클릭으로_처리된다() {
    // FR-56 acceptance ⓒ — 조금 흔들린 클릭이 재정렬이 되면 폴더 이동이 사라진다
    let favorite = std::path::PathBuf::from(r"D:\하나");
    let favorites = [user_favorite(r"D:\하나"), user_favorite(r"D:\둘")];
    let mut harness = FavoriteHarness::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;

    let first = harness.frame(&mut panel, &favorites);
    let spot = drawn_text_positions(&first)
        .into_iter()
        .find(|(text, pos)| text == "하나" && pos.x < TREE_WIDTH)
        .expect("첫째 줄")
        .1;
    let spot = egui::pos2(spot.x + 8.0, spot.y + 6.0);

    // 임계(8.0)보다 덜 움직인다
    let 살짝 = egui::pos2(spot.x + 2.0, spot.y + 1.0);
    let mut picked = None;
    for (time, event) in [
        (0.00, press_at(spot, true)),
        (0.05, egui::Event::PointerMoved(살짝)),
        (0.10, press_at(살짝, false)),
    ] {
        let input = egui::RawInput {
            time: Some(time),
            events: vec![event],
            ..Default::default()
        };
        let (_, action) = harness.draw(&mut panel, &favorites, input);
        picked = picked.or(action);
    }

    assert_eq!(picked, None, "임계를 못 넘었는데 차례가 바뀌었다");
    assert_eq!(
        panel.pending_dir, favorite,
        "클릭으로 그 폴더에 가지 않았다"
    );
}

#[test]
fn 사라진_즐겨찾기는_눌러도_옮겨가지_않는다() {
    // FR-56 — 없는 곳으로 옮기려다 실패하는 것보다 흐린 글씨로 「지금은 갈 수 없다」를
    // 보이는 편이 낫다. 줄 자체는 남아 있어 우클릭 `해제`로 뺄 수 있다
    fn press(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    let favorite = std::path::PathBuf::from(r"D:\작업");
    let favorites = [missing_favorite(r"D:\작업")];
    let sites = SiteStore::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;

    let ctx = egui::Context::default();
    let mut icons = crate::fs::icons::IconCache::new();
    let mut textures = crate::ui::icon_tex::IconTextures::new();
    let tree_cache = crate::remote::tree_cache::TreeCache::new();
    let draw = |input: egui::RawInput,
                panel: &mut PanelState,
                icons: &mut crate::fs::icons::IconCache,
                textures: &mut crate::ui::icon_tex::IconTextures| {
        let remote = RemoteView {
            sites: &sites,
            connected: &[],
            tree: &tree_cache,
        };
        ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let inner = ui.ctx().clone();
                textures.begin_frame();
                panel.show(
                    ui,
                    &inner,
                    icons,
                    textures,
                    remote,
                    PanelMenuState::for_panes(1, ViewMode::Details),
                    crate::ui::tabs::TransferTargets::default(),
                    &favorites,
                    drive_rows(),
                );
            });
        })
    };

    let first = draw(Default::default(), &mut panel, &mut icons, &mut textures);
    let spot = drawn_text_positions(&first)
        .into_iter()
        .find(|(text, pos)| text == "작업" && pos.x < TREE_WIDTH)
        .expect("사라진 즐겨찾기도 줄은 그대로 선다")
        .1;
    let spot = egui::pos2(spot.x + 8.0, spot.y + 6.0);

    for pressed in [true, false] {
        let input = egui::RawInput {
            events: vec![press(spot, pressed)],
            ..Default::default()
        };
        draw(input, &mut panel, &mut icons, &mut textures);
    }
    assert_ne!(
        panel.pending_dir, favorite,
        "사라진 즐겨찾기를 눌렀는데 그 폴더로 향했다"
    );
}

#[test]
fn 즐겨찾기를_누르면_그_폴더로_옮겨간다() {
    // 바로가기의 본래 목적 — 그 줄을 실제로 눌러 활성 탭이 그리로 가는지 본다.
    // `navigate`를 직접 부르면 트리의 클릭 처리(그 줄 → `TreeChoice` → 이동)가 시험을 비켜간다
    fn press(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    let favorite = std::path::PathBuf::from(r"D:\작업");
    let favorites = [user_favorite(r"D:\작업")];
    let sites = SiteStore::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;

    // 첫 프레임 — 즐겨찾기 줄이 어디에 그려졌는지 얻는다
    let ctx = egui::Context::default();
    let mut icons = crate::fs::icons::IconCache::new();
    let mut textures = crate::ui::icon_tex::IconTextures::new();
    let tree_cache = crate::remote::tree_cache::TreeCache::new();
    let draw = |input: egui::RawInput,
                panel: &mut PanelState,
                icons: &mut crate::fs::icons::IconCache,
                textures: &mut crate::ui::icon_tex::IconTextures| {
        let remote = RemoteView {
            sites: &sites,
            connected: &[],
            tree: &tree_cache,
        };
        ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let inner = ui.ctx().clone();
                textures.begin_frame();
                panel.show(
                    ui,
                    &inner,
                    icons,
                    textures,
                    remote,
                    PanelMenuState::for_panes(1, ViewMode::Details),
                    crate::ui::tabs::TransferTargets::default(),
                    &favorites,
                    drive_rows(),
                );
            });
        })
    };

    let first = draw(Default::default(), &mut panel, &mut icons, &mut textures);
    let spot = drawn_text_positions(&first)
        .into_iter()
        .find(|(text, pos)| text == "작업" && pos.x < TREE_WIDTH)
        .expect("즐겨찾기 줄")
        .1;
    // 글의 왼쪽 위 자리다 — 글자 높이의 절반쯤 내려 그 줄 한가운데를 누른다
    let spot = egui::pos2(spot.x + 8.0, spot.y + 6.0);
    assert_ne!(
        panel.pending_dir, favorite,
        "누르기도 전에 그 폴더로 향했다"
    );

    for (time, event) in [(0.05, press(spot, true)), (0.10, press(spot, false))] {
        let input = egui::RawInput {
            time: Some(time),
            events: vec![event],
            ..Default::default()
        };
        let _ = draw(input, &mut panel, &mut icons, &mut textures);
    }

    assert_eq!(
        panel.pending_dir, favorite,
        "즐겨찾기를 눌렀는데 그 폴더로 열거를 걸지 않았다"
    );
    assert!(
        matches!(panel.pending_nav, PendingNav::Push),
        "히스토리에 쌓지 않았다"
    );
}
/// 즐겨찾기를 든 패널을 프레임 단위로 그린다 — 우클릭 메뉴처럼 **두 프레임에 걸쳐**
/// 일어나는 일을 보는 시험들이 쓴다(메뉴는 우클릭을 받은 다음 프레임에 그려진다)
struct FavoriteHarness {
    ctx: egui::Context,
    icons: IconCache,
    textures: crate::ui::icon_tex::IconTextures,
    tree: crate::remote::tree_cache::TreeCache,
    sites: SiteStore,
}

impl FavoriteHarness {
    fn new() -> FavoriteHarness {
        FavoriteHarness {
            ctx: egui::Context::default(),
            icons: IconCache::new(),
            textures: crate::ui::icon_tex::IconTextures::new(),
            tree: crate::remote::tree_cache::TreeCache::new(),
            sites: SiteStore::new(),
        }
    }

    /// 한 프레임 그리고 화면과 그 프레임의 결과를 함께 돌려준다
    fn draw(
        &mut self,
        panel: &mut PanelState,
        favorites: &[FavoriteEntry],
        input: egui::RawInput,
    ) -> (eframe::egui::FullOutput, Option<FavoriteAction>) {
        let remote = RemoteView {
            sites: &self.sites,
            connected: &[],
            tree: &self.tree,
        };
        let (icons, textures) = (&mut self.icons, &mut self.textures);
        let mut picked = None;
        let output = self.ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let inner = ui.ctx().clone();
                // 앱과 같은 자리에서 프레임 상한을 푼다 — 이 하네스는 한 인스턴스로 여러
                // 프레임을 그리므로, 빠뜨리면 텍스처 8개가 하네스 수명 전체의 상한이 된다
                textures.begin_frame();
                let outcome = panel.show(
                    ui,
                    &inner,
                    icons,
                    textures,
                    remote,
                    PanelMenuState::for_panes(1, ViewMode::Details),
                    crate::ui::tabs::TransferTargets::default(),
                    favorites,
                    drive_rows(),
                );
                picked = outcome.favorite;
            });
        });
        (output, picked)
    }

    /// 아무 입력 없이 한 프레임 — 화면만 본다
    fn frame(
        &mut self,
        panel: &mut PanelState,
        favorites: &[FavoriteEntry],
    ) -> eframe::egui::FullOutput {
        self.draw(panel, favorites, Default::default()).0
    }

    /// 그 자리를 누르고 뗀다 — 마지막 프레임의 결과를 돌려준다
    fn click(
        &mut self,
        panel: &mut PanelState,
        favorites: &[FavoriteEntry],
        at: egui::Pos2,
        button: egui::PointerButton,
        start: f64,
    ) -> (eframe::egui::FullOutput, Option<FavoriteAction>) {
        let mut last = None;
        for (step, pressed) in [(0.0, true), (0.05, false)] {
            let input = egui::RawInput {
                time: Some(start + step),
                events: vec![egui::Event::PointerButton {
                    pos: at,
                    button,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            };
            last = Some(self.draw(panel, favorites, input));
        }
        last.expect("두 프레임을 그렸다")
    }
}

/// 트리 구역에 그려진 그 글의 한가운데 — 없으면 시험을 세운다
fn tree_text_spot(output: &eframe::egui::FullOutput, label: &str) -> egui::Pos2 {
    let pos = drawn_text_positions(output)
        .into_iter()
        .find(|(text, pos)| text == label && pos.x < TREE_WIDTH)
        .unwrap_or_else(|| panic!("트리에 `{label}` 줄이 없다"))
        .1;
    // 글의 왼쪽 위 자리다 — 그 줄 한가운데를 겨눈다
    egui::pos2(pos.x + 8.0, pos.y + 6.0)
}

#[test]
fn 트리_노드를_우클릭하면_즐겨찾기_메뉴가_뜬다() {
    // FR-56 — 등록 진입점이다. 아직 담기지 않은 폴더라 활성으로 뜬다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let mut harness = FavoriteHarness::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;

    let first = harness.frame(&mut panel, &[]);
    // 주소창에도 `C:\` 같은 경로가 있어 **트리 구역만** 본다 — 트리는 상태 줄 아래에서
    // 시작하므로 토글 아이콘보다 아래인 것이 트리의 줄이다
    let texts = drawn_text_positions(&first);
    let 토글 = texts
        .iter()
        .find(|(text, _)| text == TREE_TOGGLE_ICON)
        .expect("트리 토글 아이콘")
        .1;
    let 이름들 = drive_labels();
    let 드라이브 = texts
        .iter()
        .find(|(text, pos)| {
            pos.x < TREE_WIDTH && pos.y > 토글.y && 이름들.iter().any(|(_, label)| label == text)
        })
        .expect("트리의 드라이브 줄")
        .1;
    let at = egui::pos2(드라이브.x + 8.0, 드라이브.y + 6.0);

    harness.click(&mut panel, &[], at, egui::PointerButton::Secondary, 0.05);
    let opened = harness.frame(&mut panel, &[]);

    let texts = drawn_texts(&opened);
    assert!(
        texts.iter().any(|text| text == "즐겨찾기에 담기"),
        "우클릭했는데 메뉴가 뜨지 않았다: {texts:?}"
    );
}

#[test]
fn 즐겨찾기_줄의_메뉴는_해제다() {
    // 담긴 폴더에는 `해제`만 둔다 — 이미 담겨 있다는 것이 그 자리로 자명하다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let mut harness = FavoriteHarness::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;
    let favorites = [user_favorite(r"D:\작업")];

    let first = harness.frame(&mut panel, &favorites);
    let at = tree_text_spot(&first, "작업");
    harness.click(
        &mut panel,
        &favorites,
        at,
        egui::PointerButton::Secondary,
        0.05,
    );
    let opened = harness.frame(&mut panel, &favorites);

    let texts = drawn_texts(&opened);
    assert!(
        texts.iter().any(|text| text == "해제"),
        "`해제`가 없다: {texts:?}"
    );
    assert!(
        !texts.iter().any(|text| text == "즐겨찾기에 담기"),
        "이미 담긴 줄에 `즐겨찾기에 담기`가 함께 떴다: {texts:?}"
    );
}

#[test]
fn 즐겨찾기_목록_위에_제목이_선다() {
    // 사용자 결정 — 목록이 무엇인지 알리는 흐린 작은 글씨다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;
    let favorites = [user_favorite(r"D:\작업")];

    let texts = drawn_text_positions(&draw_once_with_favorites(
        &mut panel,
        &SiteStore::new(),
        &favorites,
    ));

    let 제목 = texts
        .iter()
        .find(|(text, pos)| text == "즐겨찾기" && pos.x < TREE_WIDTH)
        .expect("제목 줄이 없다")
        .1;
    let 첫줄 = texts
        .iter()
        .find(|(text, pos)| text == "작업" && pos.x < TREE_WIDTH)
        .expect("즐겨찾기 `작업` 줄")
        .1;

    assert!(
        제목.y < 첫줄.y,
        "제목이 목록 위에 서지 않았다 (제목 {제목:?}, 첫 줄 {첫줄:?})"
    );
}

#[test]
fn 기본_항목만_있어도_제목과_목록이_그려진다() {
    // 사용자 항목이 하나도 없는 첫 실행 화면 — 빈 목록이 아니므로 종전의
    // `아무것도 안 그림` 규칙에 걸리지 않는다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;
    let favorites = [
        default_favorite(r"C:\Users\누구\Desktop", "바탕 화면"),
        default_favorite(r"C:\Users\누구\Downloads", "다운로드"),
    ];

    let output = draw_once_with_favorites(&mut panel, &SiteStore::new(), &favorites);
    let texts = drawn_texts(&output);

    for 기대 in ["즐겨찾기", "바탕 화면", "다운로드"] {
        assert!(
            texts.iter().any(|text| text == 기대),
            "`{기대}`가 그려지지 않았다: {texts:?}"
        );
    }
    assert_eq!(
        separator_count(&output),
        separator_count(&draw_once_with_favorites(
            &mut panel,
            &SiteStore::new(),
            &[]
        )) + 1,
        "기본 항목만 있을 때 구분선이 서지 않았다"
    );
}

#[test]
fn 기본_항목_줄은_우클릭해도_메뉴가_뜨지_않는다() {
    // 사용자 결정 — 바탕 화면·다운로드는 해제할 수 없다. 항목이 하나뿐인 메뉴에서
    // `해제`만 빼면 눌러도 아무 일이 없는 빈 상자가 뜬다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let mut harness = FavoriteHarness::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;
    let favorites = [default_favorite(r"C:\Users\누구\Desktop", "바탕 화면")];

    let first = harness.frame(&mut panel, &favorites);
    let at = tree_text_spot(&first, "바탕 화면");
    harness.click(
        &mut panel,
        &favorites,
        at,
        egui::PointerButton::Secondary,
        0.05,
    );
    let opened = harness.frame(&mut panel, &favorites);

    let texts = drawn_texts(&opened);
    assert!(
        !texts
            .iter()
            .any(|text| text == "해제" || text == "즐겨찾기에 담기"),
        "기본 항목 줄에 메뉴가 떴다: {texts:?}"
    );
}

#[test]
fn 원격_트리_노드에는_메뉴가_뜨지_않는다() {
    // 사용자 명시 제외 — 원격 폴더는 즐겨찾기 대상이 아니다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let (mut panel, sites) = remote_panel_in(TabPhase::Ok);
    panel.tree_visible = true;
    let mut harness = FavoriteHarness::new();
    harness.sites = sites;

    let first = harness.frame(&mut panel, &[]);
    let at = tree_text_spot(&first, "/");
    harness.click(&mut panel, &[], at, egui::PointerButton::Secondary, 0.05);
    let opened = harness.frame(&mut panel, &[]);

    let texts = drawn_texts(&opened);
    assert!(
        !texts
            .iter()
            .any(|text| text == "즐겨찾기에 담기" || text == "해제"),
        "원격 트리에 메뉴가 떴다: {texts:?}"
    );
}

#[test]
fn 메뉴에서_고른_조작이_패널_밖으로_올라간다() {
    // 목록을 고치는 것은 앱이다 — 패널은 고른 것을 값으로 올려보내기만 한다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let mut harness = FavoriteHarness::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;
    let favorites = [user_favorite(r"D:\작업")];

    let first = harness.frame(&mut panel, &favorites);
    let at = tree_text_spot(&first, "작업");
    harness.click(
        &mut panel,
        &favorites,
        at,
        egui::PointerButton::Secondary,
        0.05,
    );
    let opened = harness.frame(&mut panel, &favorites);
    let 해제 = drawn_text_positions(&opened)
        .into_iter()
        .find(|(text, _)| text == "해제")
        .expect("`해제` 줄")
        .1;
    let 해제 = egui::pos2(해제.x + 8.0, 해제.y + 6.0);

    let (_, picked) = harness.click(
        &mut panel,
        &favorites,
        해제,
        egui::PointerButton::Primary,
        0.20,
    );

    assert_eq!(
        picked,
        Some(FavoriteAction::Remove(std::path::PathBuf::from(r"D:\작업"))),
        "`해제`를 눌렀는데 그 조작이 올라오지 않았다"
    );
}

#[test]
fn 이미_담긴_폴더는_즐겨찾기_줄이_비활성이다() {
    // 사용자 요청 — 눌러도 되지 않는 것을 눌리게 두면 사용자는 눌렀다가 아무 일도
    // 일어나지 않는 것을 본다.
    //
    // **같은 자리를 두 조건에서 눌러 견준다** — 미등록이면 조작이 올라오고 등록됐으면
    // 올라오지 않아야 한다. 한쪽만 보면 "좌표가 빗나가서 아무것도 안 올라온 것"과
    // 구분할 수 없어 시험이 죽은 채 통과한다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);

    /// 트리의 **맨 아래** 드라이브 줄을 우클릭하고, 뜬 `즐겨찾기` 줄을 눌러 그 결과를 돌려준다.
    ///
    /// 맨 아래를 고르는 이유는 즐겨찾기 줄이 위에 서면 같은 이름이 화면에 둘이 되기 때문이다 —
    /// 두 실행이 같은 규칙을 쓰므로 ①②가 같은 줄을 누른다
    fn pick_on_last_drive(
        favorites: &[FavoriteEntry],
    ) -> (std::path::PathBuf, Option<FavoriteAction>) {
        let mut harness = FavoriteHarness::new();
        let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
        panel.tree_visible = true;
        panel.deferred_start = None;

        let drawn = harness.frame(&mut panel, favorites);
        let texts = drawn_text_positions(&drawn);
        let 토글 = texts
            .iter()
            .find(|(text, _)| text == TREE_TOGGLE_ICON)
            .expect("트리 토글 아이콘")
            .1;
        // 즐겨찾기 줄과 트리 줄에 같은 이름이 함께 있을 수 있다 — **아래쪽**이 트리 줄이다.
        // 드라이브 판정은 `drive_labels`가 준 (경로, 표시 이름) 쌍으로 한다 — 표시 이름에는
        // 규칙이 없어 패턴으로는 가려낼 수 없고, **기대 경로도 그 쌍에서 가져와야** 한다
        // (그려진 글자로 경로를 만들면 표시 이름이 그대로 경로가 되어 어긋난다)
        let 이름들 = drive_labels();
        let (이름, 자리) = texts
            .iter()
            .filter(|(text, pos)| {
                pos.x < TREE_WIDTH && pos.y > 토글.y && 이름들.iter().any(|(_, l)| l == text)
            })
            .max_by(|a, b| a.1.y.total_cmp(&b.1.y))
            .cloned()
            .expect("트리의 드라이브 줄");
        let 경로 = 이름들
            .iter()
            .find(|(_, label)| *label == 이름)
            .map(|(path, _)| path.clone())
            .expect("그 이름의 드라이브 경로");

        harness.click(
            &mut panel,
            favorites,
            egui::pos2(자리.x + 8.0, 자리.y + 6.0),
            egui::PointerButton::Secondary,
            0.05,
        );
        let opened = harness.frame(&mut panel, favorites);
        let 메뉴줄 = drawn_text_positions(&opened)
            .into_iter()
            .find(|(text, _)| text == "즐겨찾기에 담기")
            .expect("`즐겨찾기에 담기` 줄이 뜨지 않았다")
            .1;

        let (_, picked) = harness.click(
            &mut panel,
            favorites,
            egui::pos2(메뉴줄.x + 8.0, 메뉴줄.y + 6.0),
            egui::PointerButton::Primary,
            0.20,
        );
        (경로, picked)
    }

    // ① 아직 담기지 않았으면 눌러서 담긴다 — 이 좌표가 실제로 그 줄을 누른다는 증거다
    let (경로, picked) = pick_on_last_drive(&[]);
    assert_eq!(
        picked,
        Some(FavoriteAction::Add(경로.clone())),
        "미등록 폴더인데 `즐겨찾기`를 눌러도 조작이 올라오지 않았다"
    );

    // ② 이미 담겼으면 같은 자리를 눌러도 아무것도 올라오지 않는다
    let (_, picked) = pick_on_last_drive(&[user_favorite(&경로.to_string_lossy())]);
    assert_eq!(picked, None, "이미 담긴 폴더인데 다시 담겼다");
}

#[test]
fn 바깥을_누르거나_esc를_치면_메뉴가_닫힌다() {
    // 메뉴가 화면에 눌어붙지 않게 한다 — 원격 목록 메뉴와 같은 규칙이다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);

    /// 즐겨찾기 줄에서 메뉴를 연 상태를 만든다
    fn open_menu(
        harness: &mut FavoriteHarness,
        panel: &mut PanelState,
        favorites: &[FavoriteEntry],
    ) {
        let first = harness.frame(panel, favorites);
        let at = tree_text_spot(&first, "작업");
        harness.click(panel, favorites, at, egui::PointerButton::Secondary, 0.05);
        let opened = harness.frame(panel, favorites);
        assert!(
            drawn_texts(&opened).iter().any(|text| text == "해제"),
            "메뉴가 열리지 않아 이 시험이 무의미하다"
        );
    }

    fn panel_with_tree() -> PanelState {
        let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
        panel.tree_visible = true;
        panel.deferred_start = None;
        panel
    }

    let favorites = [user_favorite(r"D:\작업")];

    // ① 메뉴 바깥을 누르면 닫힌다 — 목록 쪽 빈자리를 누른다
    let mut harness = FavoriteHarness::new();
    let mut panel = panel_with_tree();
    open_menu(&mut harness, &mut panel, &favorites);
    harness.click(
        &mut panel,
        &favorites,
        egui::pos2(TREE_WIDTH + 240.0, 420.0),
        egui::PointerButton::Primary,
        0.20,
    );
    let after_outside = harness.frame(&mut panel, &favorites);
    assert!(
        !drawn_texts(&after_outside)
            .iter()
            .any(|text| text == "해제"),
        "바깥을 눌렀는데 메뉴가 남아 있다"
    );

    // ② Esc를 쳐도 닫힌다
    let mut harness = FavoriteHarness::new();
    let mut panel = panel_with_tree();
    open_menu(&mut harness, &mut panel, &favorites);
    let esc = egui::RawInput {
        time: Some(0.30),
        events: vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    };
    harness.draw(&mut panel, &favorites, esc);
    let after_esc = harness.frame(&mut panel, &favorites);
    assert!(
        !drawn_texts(&after_esc).iter().any(|text| text == "해제"),
        "Esc를 쳤는데 메뉴가 남아 있다"
    );
}

#[test]
fn 화면_가장자리에서_연_메뉴가_그_자리에서_닫히지_않는다() {
    // 2026-08-17 사용자 보고 — 메뉴가 떴다가 곧바로 사라졌다.
    //
    // 메뉴는 우클릭을 **받은 그 프레임에** 그려지는데, 그 프레임의 `any_click()`은 방금 그
    // 우클릭이라 참이다. 그래서 메뉴가 클릭 자리를 품지 못하면(화면 가장자리라 위치가 안으로
    // 당겨진 경우) 자기를 연 클릭을 "바깥 클릭"으로 세어 즉시 닫혔다.
    //
    // 창을 좁게 잡아 그 당김을 강제한다 — 종전 시험들은 클릭 좌표가 메뉴 왼쪽 위 **모서리에
    // 정확히 걸쳐** 간신히 통과했다(1px만 밀리면 깨지는 자리였다)
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let favorites = [user_favorite(r"D:\작업")];
    let mut harness = FavoriteHarness::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;

    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(500.0, 160.0));
    let base = || egui::RawInput {
        screen_rect: Some(screen),
        ..Default::default()
    };

    let first = harness.draw(&mut panel, &favorites, base()).0;
    let spot = tree_text_spot(&first, "작업");

    for (time, pressed) in [(0.05, true), (0.10, false)] {
        let input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(time),
            events: vec![egui::Event::PointerButton {
                pos: spot,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        harness.draw(&mut panel, &favorites, input);
    }

    let opened = harness.draw(&mut panel, &favorites, base()).0;
    assert!(
        drawn_texts(&opened).iter().any(|text| text == "해제"),
        "우클릭한 그 자리에서 메뉴가 곧바로 닫혔다: {:?}",
        drawn_texts(&opened)
    );
}

#[test]
fn 트리를_감추면_열린_메뉴가_닫힌다() {
    // 트리가 그려지지 않는 프레임에는 메뉴 코드가 통째로 건너뛰어져 스스로 닫지 못한다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let mut harness = FavoriteHarness::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;
    let favorites = [user_favorite(r"D:\작업")];

    let first = harness.frame(&mut panel, &favorites);
    let at = tree_text_spot(&first, "작업");
    harness.click(
        &mut panel,
        &favorites,
        at,
        egui::PointerButton::Secondary,
        0.05,
    );
    let opened = harness.frame(&mut panel, &favorites);
    assert!(
        drawn_texts(&opened).iter().any(|text| text == "해제"),
        "메뉴가 열리지 않아 이 시험이 무의미하다"
    );

    // 트리를 껐다 켠다
    panel.tree_visible = false;
    harness.frame(&mut panel, &favorites);
    panel.tree_visible = true;
    let reopened = harness.frame(&mut panel, &favorites);

    assert!(
        !drawn_texts(&reopened).iter().any(|text| text == "해제"),
        "트리를 다시 켜니 옛 메뉴가 되살아났다"
    );
}

#[test]
fn 활성_탭이_원격으로_바뀌면_열린_메뉴가_닫힌다() {
    // 로컬에서 연 메뉴가 원격 화면 위에 떠서도, 돌아왔을 때 되살아나서도 안 된다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let (mut panel, sites) = remote_panel_in(TabPhase::Ok);
    panel.tree_visible = true;
    let mut harness = FavoriteHarness::new();
    harness.sites = sites;
    let favorites = [user_favorite(r"D:\작업")];

    // 로컬 탭을 하나 열어 그 트리에서 메뉴를 연다
    let ctx = egui::Context::default();
    panel.handle_tab(TabAction::New, &ctx);
    let first = harness.frame(&mut panel, &favorites);
    let at = tree_text_spot(&first, "작업");
    harness.click(
        &mut panel,
        &favorites,
        at,
        egui::PointerButton::Secondary,
        0.05,
    );
    let opened = harness.frame(&mut panel, &favorites);
    assert!(
        drawn_texts(&opened).iter().any(|text| text == "해제"),
        "메뉴가 열리지 않아 이 시험이 무의미하다"
    );

    // 원격 탭으로 갔다가 다시 로컬로 돌아온다
    assert!(panel.tabs.switch(0), "원격 탭으로 돌아가지 못했다");
    harness.frame(&mut panel, &favorites);
    assert!(panel.tabs.switch(1), "로컬 탭으로 돌아오지 못했다");
    let back = harness.frame(&mut panel, &favorites);

    assert!(
        !drawn_texts(&back).iter().any(|text| text == "해제"),
        "원격을 거쳐 돌아오니 옛 메뉴가 되살아났다"
    );
}

#[test]
fn 트리_토글은_트리_위쪽_왼쪽_끝에_선다() {
    // 2026-08-16 사용자 결정 — 토글이 목록 쪽(트리 오른쪽)에 있으면 무엇을 여는 버튼인지
    // 읽히지 않는다. 상태 줄이 패널 전폭을 쓰고 토글은 트리 폭 안에 선다
    let (mut panel, sites) = remote_panel_in(TabPhase::Ok);
    panel.tree_visible = true;
    let texts = drawn_text_positions(&draw_once(&mut panel, &sites));
    let 토글 = texts
        .iter()
        .find(|(text, _)| text == TREE_TOGGLE_ICON)
        .expect("트리 토글 아이콘")
        .1;
    assert!(
        토글.x < TREE_WIDTH,
        "토글이 트리 폭({TREE_WIDTH}) 밖에 있다: {}",
        토글.x
    );
}

#[test]
fn 트리는_파일_목록과_같은_높이에서_시작한다() {
    // 2026-08-16 사용자 보고 — 트리 첫 줄이 상태 줄 옆까지 올라가 목록보다 위에 떠 있었다.
    // 트리 폭 안(왼쪽)에 그려진 글과 목록 열 머리글의 높이를 견준다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let (mut panel, sites) = remote_panel_in(TabPhase::Ok);
    panel.tree_visible = true;
    let output = draw_once(&mut panel, &sites);
    let texts = drawn_text_positions(&output);

    let 토글 = texts
        .iter()
        .find(|(text, _)| text == TREE_TOGGLE_ICON)
        .expect("트리 토글 아이콘")
        .1;
    // 열 머리글은 정렬 화살표가 붙어 앞글자로 찾는다 — 다만 **상태 줄 아래**로 한정한다.
    // 그러지 않으면 주소창 줄의 필터 자리표시자(`이름으로 거르기` — FR-65)가 먼저 걸려
    // 엉뚱한 높이를 재게 된다
    let 머리글 = texts
        .iter()
        .find(|(text, pos)| text.starts_with("이름") && pos.x > TREE_WIDTH && pos.y > 토글.y)
        .expect("목록의 `이름` 열 머리글")
        .1;
    // 원격 트리의 뿌리 줄 — 이름이 없는 루트는 경로 그대로(`/`) 그려진다
    let 첫줄 = texts
        .iter()
        .find(|(text, pos)| text == "/" && pos.x < TREE_WIDTH)
        .expect("트리 뿌리 줄")
        .1
        .y;

    assert!(
        첫줄 > 토글.y,
        "트리가 상태 줄까지 올라와 있다 (트리 {첫줄}, 토글 {})",
        토글.y
    );
    // 열 머리글과 나란하다 — 위젯 안쪽 여백만큼의 차이는 남는다
    assert!(
        (첫줄 - 머리글.y).abs() < 12.0,
        "트리 첫 줄과 목록 머리글의 높이가 어긋난다 (트리 {첫줄}, 머리글 {})",
        머리글.y
    );
}

#[test]
fn 원격_패널의_트리_토글은_원격_트리다() {
    // Acceptance ① (인벤토리 #94) — 같은 자리의 **툴팁**이 소스에 따라 갈린다.
    // 문구는 카탈로그가 정하므로 한국어로 고정하고 원문과 견준다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let 로컬 = PanelState::new(std::path::PathBuf::from(r"C:\"));
    assert_eq!(로컬.tree_toggle_tooltip(), "폴더 트리");
    let (원격, _) = remote_panel_in(TabPhase::Ok);
    assert_eq!(원격.tree_toggle_tooltip(), "원격 트리");
}

#[test]
fn 트리_토글은_문구가_아니라_아이콘으로_그린다() {
    // 사용자 요청 — 상태 줄의 `폴더 트리`·`원격 트리` 문구를 아이콘 하나로 줄였다.
    // 툴팁은 hover해야 뜨므로 그려진 글에는 문구가 남지 않아야 한다
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    for 화면 in [
        drawn_texts(&draw_once(
            &mut PanelState::new(std::path::PathBuf::from(r"C:\")),
            &SiteStore::new(),
        )),
        remote_screen_texts(TabPhase::Ok),
    ] {
        assert!(
            화면.iter().any(|text| text == TREE_TOGGLE_ICON),
            "트리 토글 아이콘이 없다: {화면:?}"
        );
        assert!(
            !화면
                .iter()
                .any(|text| text == "폴더 트리" || text == "원격 트리"),
            "트리 토글 문구가 남아 있다: {화면:?}"
        );
    }
}

#[test]
fn 자동_재조회로_나간_요청만_조용한_것으로_표시된다() {
    // FR-67 — 그 세대의 실패는 화면 알림을 띄우지 않는다(서버 로그에는 남는다).
    // 표시가 세대에 묶이지 않으면 손으로 부른 조회의 실패까지 함께 삼켜진다
    let (mut panel, _) = remote_panel_in(TabPhase::Ok);
    let manager = ConnectionManager::new(std::sync::Arc::new(|| {}));

    // 세대는 요청을 세울 때 붙는다 — 이 하네스의 가짜 관리자는 실제로 보내지 못하므로
    // 반환값이 아니라 그 세대를 본다(같은 파일의 다른 원격 시험도 같은 방식이다)
    panel.request_remote_list_quiet(WorkspaceId(0), PanelId(0), &manager);
    let 자동 = panel.remote_seq;
    assert!(panel.is_quiet_request(자동));

    // 손으로 부른 조회가 그 자리를 덮는다 — 옛 표시가 남아 알림을 삼키면 안 된다
    panel.request_remote_list(WorkspaceId(0), PanelId(0), &manager);
    let 손으로 = panel.remote_seq;
    assert!(
        !panel.is_quiet_request(손으로),
        "손으로 부른 조회가 조용하다"
    );
    assert!(
        !panel.is_quiet_request(자동),
        "지나간 자동 요청의 표시가 남아 있다"
    );
}

#[test]
fn 트리에서_고른_원격_폴더로_목록이_옮겨간다() {
    // Acceptance ⑤ — 옮기는 것으로 끝나면 화면은 옛 목록 그대로다(spec 리뷰 B1).
    // 옮긴 뒤 **깃발이 서고**, 그 깃발을 거둔 쪽이 실제로 조회를 보내야 한다
    let (mut panel, _) = remote_panel_in(TabPhase::Ok);
    panel.take_remote_dirty();
    panel.navigate_remote(RemotePath::new("/var/www/html"));
    assert_eq!(
        panel.tabs.active().source.remote_path().map(|p| p.as_str()),
        Some("/var/www/html")
    );
    assert!(panel.take_remote_dirty(), "다시 읽어 달라는 표시가 없다");
    assert!(!panel.take_remote_dirty(), "깃발이 한 번에 거둬지지 않았다");

    // 거둔 쪽이 그 위치로 조회를 보낸다 — 세대와 위치가 함께 맞아야 답을 받는다
    let manager = ConnectionManager::new(std::sync::Arc::new(|| {}));
    panel.request_remote_list(WorkspaceId(0), PanelId(0), &manager);
    assert!(panel.awaits_remote_list(panel.remote_seq, &RemotePath::new("/var/www/html")));

    // 상위 이동도 같은 길을 쓴다 — 옮기고 나서 아무도 다시 읽지 않던 자리였다
    let ctx = egui::Context::default();
    panel.handle_nav(NavAction::Up, &ctx);
    assert_eq!(
        panel.tabs.active().source.remote_path().map(|p| p.as_str()),
        Some("/var/www")
    );
    assert!(panel.take_remote_dirty(), "상위 이동 뒤에 표시가 없다");
}

#[test]
fn 원격_트리의_뿌리는_최상단까지_거슬러_올라간다() {
    // plan Edge Case — 루트가 `/`가 아닌 서버도 있어 `/`로 못 박지 않는다
    let (panel, _) = remote_panel_in(TabPhase::Ok);
    let (conn, root) = panel.remote_tree_root().expect("연결된 원격 탭");
    assert_eq!(conn, ConnectionId(1));
    assert_eq!(root.as_str(), "/");
    // 로컬 탭에는 원격 트리가 없다
    let 로컬 = PanelState::new(std::path::PathBuf::from(r"C:\"));
    assert!(로컬.remote_tree_root().is_none());
}

#[test]
fn 다른_폴더로_옮길_때만_이름_필터가_비워진다() {
    // FR-65·D11 — 이동의 단일 통로가 `start_load`다. 같은 폴더를 다시 읽는 길
    // (변경 감시(FR-10)·F5·숨김 토글)에서 지우면 치던 글자가 예고 없이 사라진다
    let ctx = egui::Context::default();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    panel.list.set_filter("txt");

    panel.start_load(
        std::path::PathBuf::from(r"C:\Users"),
        PendingNav::None,
        &ctx,
    );
    assert_eq!(
        panel.list.filter(),
        "txt",
        "같은 폴더를 다시 읽었을 뿐인데 필터가 사라졌다"
    );

    panel.start_load(
        std::path::PathBuf::from(r"C:\Windows"),
        PendingNav::Push,
        &ctx,
    );
    assert_eq!(panel.list.filter(), "", "폴더를 옮겼는데 필터가 남았다");
}

#[test]
fn 탭을_바꾸면_이름_필터가_비워진다() {
    // D2 — 옮겨 간 탭에 옛 필터가 걸린 채로 보이면 그 폴더가 비어 보이고 이유를 알 수 없다.
    // 폴더가 같은지로는 가릴 수 없어(두 탭이 같은 곳을 볼 수 있다) 탭 전환은 언제나 비운다
    let ctx = egui::Context::default();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    panel.list.set_filter("txt");

    panel.open_local_tab(std::path::PathBuf::from(r"C:\Users"), &ctx);
    assert_eq!(panel.list.filter(), "", "탭을 바꿨는데 필터가 남았다");
}

#[test]
fn 원격도_자리를_옮길_때만_이름_필터가_비워진다() {
    // 로컬과 같은 규칙 (D2·D11) — `set_remote_path`가 원격 이동의 단일 통로다
    let (mut panel, _) = remote_panel_in(TabPhase::Ok);
    panel.list.set_filter("log");

    let 제자리 = panel
        .tabs
        .active()
        .source
        .remote_path()
        .cloned()
        .expect("원격 탭이어야 한다");
    panel.set_remote_path(제자리);
    assert_eq!(
        panel.list.filter(),
        "log",
        "같은 자리를 다시 읽었을 뿐인데 필터가 사라졌다"
    );

    panel.set_remote_path(RemotePath::new("/var/log"));
    assert_eq!(
        panel.list.filter(),
        "",
        "원격 자리를 옮겼는데 필터가 남았다"
    );
}

#[test]
fn 성공한_이동은_되돌릴_자리를_남기지_않는다() {
    // F-7 2라운드 B1 — 자리가 남으면 **나중의 무관한 실패**(새로 고침·작업 후 재조회)가
    // 옛 폴더로 경로만 되돌린다
    let mut icons = IconCache::new();
    let manager = ConnectionManager::new(std::sync::Arc::new(|| {}));
    let (mut panel, _) = remote_panel_in(TabPhase::Ok);
    panel.set_remote_path(RemotePath::new("/var/www/html"));
    panel.request_remote_list(WorkspaceId(0), PanelId(0), &manager);
    let moved = panel.remote_seq;
    // 답이 도착해 이동이 섰다
    assert!(panel.apply_remote_listed(
        moved,
        &RemotePath::new("/var/www/html"),
        Vec::new(),
        &mut icons
    ));

    // 그 뒤의 새로 고침이 실패해도 경로는 그대로여야 한다
    panel.request_remote_list(WorkspaceId(0), PanelId(0), &manager);
    let refreshed = panel.remote_seq;
    assert!(
        !panel.revert_remote_path(refreshed),
        "성공한 이동이 되돌려졌다"
    );
    assert_eq!(
        panel.tabs.active().source.remote_path().map(|p| p.as_str()),
        Some("/var/www/html")
    );
}

#[test]
fn 다른_요청의_실패는_경로를_건드리지_않는다() {
    // F-7 2라운드 B2 — 같은 연결을 두 패널이 나눠 쓰면 세대가 겹친다.
    // 되돌리기는 **그 요청의 세대**이면서 **아직 그 자리에 있을 때**만 일어나야 한다
    let manager = ConnectionManager::new(std::sync::Arc::new(|| {}));
    let (mut panel, _) = remote_panel_in(TabPhase::Ok);
    panel.set_remote_path(RemotePath::new("/root"));
    panel.request_remote_list(WorkspaceId(0), PanelId(0), &manager);
    let generation = panel.remote_seq;

    // 남의 세대로는 되돌지 않는다
    assert!(!panel.revert_remote_path(generation + 1));
    assert_eq!(
        panel.tabs.active().source.remote_path().map(|p| p.as_str()),
        Some("/root")
    );

    // 그 사이 다른 곳으로 또 옮겼으면 지난 되돌리기는 무효다
    panel.set_remote_path(RemotePath::new("/etc"));
    assert!(
        !panel.revert_remote_path(generation),
        "지난 요청이 지금 위치를 되돌렸다"
    );
    assert_eq!(
        panel.tabs.active().source.remote_path().map(|p| p.as_str()),
        Some("/etc")
    );
}

#[test]
fn 조회가_실패하면_옮기기를_무른다() {
    // F-7 리뷰 B2 — 주소창은 새 폴더를, 목록은 이전 폴더를 가리킨 채 갈라지면
    // 그 위에서 연 메뉴가 보이는 것과 다른 경로에 삭제·권한 변경을 건다
    let (mut panel, _) = remote_panel_in(TabPhase::Ok);
    assert_eq!(
        panel.tabs.active().source.remote_path().map(|p| p.as_str()),
        Some("/var/www")
    );
    let manager = ConnectionManager::new(std::sync::Arc::new(|| {}));
    panel.set_remote_path(RemotePath::new("/root"));
    panel.request_remote_list(WorkspaceId(0), PanelId(0), &manager);
    let generation = panel.remote_seq;

    assert!(
        panel.revert_remote_path(generation),
        "되돌릴 자리가 없다고 했다"
    );
    assert_eq!(
        panel.tabs.active().source.remote_path().map(|p| p.as_str()),
        Some("/var/www"),
        "이전 폴더로 돌아오지 않았다"
    );
    // 되돌린 뒤에는 다시 청하지 않는다 — 실패·성공이 번갈아 도는 고리를 만들지 않는다
    panel.take_remote_dirty();
    assert!(!panel.take_remote_dirty());
    // 돌아갈 자리는 한 번만 쓴다
    assert!(!panel.revert_remote_path(generation));
}

#[test]
fn 원격_탭에서_연_새_탭은_로컬_시작_폴더다() {
    // 사용자 보고 — 원격 탭에서 `+`를 누르면 연결이 없는 원격 탭이 복제돼 목록이 빈 채로 섰다.
    // 새 탭은 로컬 시작 폴더를 가리켜야 한다
    let ctx = egui::Context::default();
    let (mut panel, _) = remote_panel_in(TabPhase::Ok);

    panel.handle_tab(TabAction::New, &ctx);

    let source = &panel.tabs.active().source;
    assert!(!source.is_remote(), "원격 탭이 그대로 복제됐다");
    let path = source.local_path().expect("로컬 탭이어야 한다");
    assert!(
        !path.as_os_str().is_empty(),
        "새 탭이 열거할 수 없는 빈 경로를 가리킨다"
    );
}

#[test]
fn 원격_탭을_바꾸면_그_탭의_목록을_다시_읽는다() {
    // F-7 3라운드 B1 — 목록은 탭이 아니라 패널 하나가 든다. 탭만 바꾸고 목록을 그대로 두면
    // 주소창은 이 탭을, 목록은 저 탭의 폴더를 보인다 — 그 위에서 연 원격 메뉴가
    // **화면에 없는 경로**에 삭제·권한 변경을 건다
    let ctx = egui::Context::default();
    let (mut panel, _) = remote_panel_in(TabPhase::Ok);
    // 같은 사이트의 다른 폴더를 원격 탭으로 하나 더 연다
    // (`Ctrl+T`는 원격 위치를 복제하지 않는다 — 로컬 시작 폴더를 연다)
    let site = panel.active_site().expect("원격 탭");
    panel.open_remote_tab(site, RemotePath::new("/var/log"));
    panel.take_remote_dirty();

    // 첫 원격 탭으로 돌아간다 — 그 탭이 보는 곳을 다시 읽어야 한다
    let first = panel
        .tabs
        .sources()
        .iter()
        .position(|source| matches!(source, TabSource::Remote { .. }))
        .expect("원격 탭");
    panel.handle_tab(TabAction::Switch(first), &ctx);
    assert!(
        panel.take_remote_dirty(),
        "원격 탭으로 바꿨는데 목록을 다시 읽지 않는다"
    );
    // 답이 오기 전까지 목록은 비어 있어야 한다 — 옛 탭의 항목이 남으면 그 사이에
    // 연 메뉴가 화면에 없는 경로를 겨눈다 (F-7 4라운드 M1)
    assert_eq!(
        panel
            .list
            .selected_remote(&RemotePath::new("/var/www"))
            .len(),
        0,
        "전환 직후 옛 항목이 남아 있다"
    );

    // 로컬 탭으로 바꾸면 로컬 열거가 도므로 이 깃발은 서지 않는다
    let local = panel
        .tabs
        .sources()
        .iter()
        .position(|source| matches!(source, TabSource::Local(_)))
        .expect("로컬 탭");
    panel.handle_tab(TabAction::Switch(local), &ctx);
    assert!(!panel.take_remote_dirty(), "로컬 탭에 원격 조회를 청했다");
}

#[test]
fn 원격_탭을_여는_사이_도착한_로컬_열거는_버린다() {
    // 실사용 결함(2026-08-05): 사이트를 더블클릭하면 앱이 죽었다. 분할로 만들어진 패널이
    // 시작 폴더를 읽는 중에 활성 탭이 원격이 되고, 뒤늦게 온 로컬 결과를 그 탭에 커밋하려
    // 했기 때문이다(개발 빌드는 단언으로 종료, 배포 빌드는 원격 탭이 로컬 탭으로 둔갑)
    let ctx = egui::Context::default();
    let mut icons = IconCache::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    panel.start_load(
        std::path::PathBuf::from(r"C:\Windows"),
        PendingNav::Push,
        &ctx,
    );
    assert!(panel.load.is_loading(), "열거가 시작되지 않았다");

    // 그 사이 사이트를 연다 — 활성 탭이 원격이 된다
    panel.open_remote_tab(SiteId(1), RemotePath::new("/var/www"));
    assert!(panel.is_remote());
    assert!(!panel.load.is_loading(), "원격 탭인데 `읽는 중…`이 남았다");

    // 이미 채널에 실려 있던 결과가 뒤늦게 도착해도 원격 탭은 그대로여야 한다
    panel.apply_enumerated(
        EnumOutcome::Ok(Vec::new()),
        &mut icons,
        &mut crate::panel::dir_cache::DirCache::new(),
        &egui::Context::default(),
    );
    assert!(panel.is_remote(), "원격 탭이 로컬 탭으로 둔갑했다");
    assert_eq!(
        panel.tabs.active().source.remote_path().map(|p| p.as_str()),
        Some("/var/www")
    );
    assert!(panel.status.is_empty(), "원격 탭에 로컬 상태 문구가 남았다");
}

#[test]
fn 원격_폴더를_더블클릭하면_그_안으로_들어간다() {
    // 사용자 보고(2026-08-05): 원격 목록에서 폴더를 더블클릭해도 아무 일도 없었다 —
    // 여는 경로가 로컬 경로를 요구해 원격 탭에서는 통째로 빠져나갔기 때문이다
    let ctx = egui::Context::default();
    let mut icons = IconCache::new();
    let (mut panel, _) = remote_panel_in(TabPhase::Ok);
    let generation = panel.request_remote_list(
        WorkspaceId(0),
        PanelId(0),
        &ConnectionManager::new(std::sync::Arc::new(|| {})),
    );
    let _ = generation;
    panel.list.set_remote_entries(
        RemotePath::root(),
        with_parent_first(vec![
            remote_entry("public_html", true),
            remote_entry("a.txt", false),
        ]),
        &mut icons,
    );
    panel.take_remote_dirty();

    // 폴더(인덱스 1 — 0은 `..`)로 들어간다
    panel.handle_list_action(FileListAction::Open(1), &ctx);
    assert_eq!(
        panel.tabs.active().source.remote_path().map(|p| p.as_str()),
        Some("/var/www/public_html")
    );
    assert!(
        panel.take_remote_dirty(),
        "들어간 폴더의 목록을 청하지 않는다"
    );

    // `..`로 위로 올라간다
    panel.list.set_remote_entries(
        RemotePath::root(),
        with_parent_first(Vec::new()),
        &mut icons,
    );
    panel.handle_list_action(FileListAction::Open(0), &ctx);
    assert_eq!(
        panel.tabs.active().source.remote_path().map(|p| p.as_str()),
        Some("/var/www")
    );

    // 파일은 열지 않는다 — 원격 파일 열기는 범위 밖이다
    panel.list.set_remote_entries(
        RemotePath::root(),
        with_parent_first(vec![remote_entry("a.txt", false)]),
        &mut icons,
    );
    panel.take_remote_dirty();
    panel.handle_list_action(FileListAction::Open(1), &ctx);
    assert_eq!(
        panel.tabs.active().source.remote_path().map(|p| p.as_str()),
        Some("/var/www"),
        "파일을 눌렀는데 위치가 바뀌었다"
    );
    assert!(!panel.take_remote_dirty());
}

#[test]
fn 처음_읽는_중에는_목록_자리에_자리표시를_세운다() {
    let _언어 = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let ctx = egui::Context::default();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    panel.start_load(
        std::path::PathBuf::from(r"C:\Users"),
        PendingNav::None,
        &ctx,
    );
    assert!(
        panel.shows_loading_placeholder(),
        "아직 아무것도 못 읽었는데 자리표시를 세우지 않는다"
    );

    let 프레임 = draw_once(&mut panel, &SiteStore::new());
    let 화면 = drawn_texts(&프레임);
    assert!(
        화면.iter().any(|t| t == "읽는 중…"),
        "읽는 중이라는 것이 화면에 없다: {화면:?}"
    );
    assert_eq!(
        skeleton_bars(&프레임),
        crate::ui::remote_states::SKELETON_BARS,
        "목록 자리가 빈칸이다 — 자리표시가 서지 않았다"
    );
}

#[test]
fn 목록이_있는_폴더에서_옮기는_중에는_이전_목록을_둔다() {
    let _언어 = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let ctx = egui::Context::default();
    let mut icons = IconCache::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    panel.apply_enumerated(
        EnumOutcome::Ok(vec![local_entry("Documents", true)]),
        &mut icons,
        &mut crate::panel::dir_cache::DirCache::new(),
        &egui::Context::default(),
    );
    panel.start_load(
        std::path::PathBuf::from(r"C:\Users\Public"),
        PendingNav::Push,
        &ctx,
    );
    assert!(
        !panel.shows_loading_placeholder(),
        "보여줄 목록이 있는데 자리표시로 덮었다 — 옮길 때마다 화면이 한 번 더 깜빡인다"
    );

    let 프레임 = draw_once(&mut panel, &SiteStore::new());
    let 화면 = drawn_texts(&프레임);
    assert!(
        화면.iter().any(|t| t == "Documents"),
        "이전 폴더의 목록이 사라졌다: {화면:?}"
    );
    assert_eq!(
        skeleton_bars(&프레임),
        0,
        "보여줄 목록이 있는데 자리표시로 덮었다"
    );
}

#[test]
fn 다_읽고_나면_자리표시를_거둔다() {
    let mut icons = IconCache::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    panel.apply_enumerated(
        EnumOutcome::Ok(Vec::new()),
        &mut icons,
        &mut crate::panel::dir_cache::DirCache::new(),
        &egui::Context::default(),
    );
    assert!(
        !panel.shows_loading_placeholder(),
        "다 읽었는데 자리표시가 남았다"
    );
}

#[test]
fn 트리_줄에는_셸_아이콘이_붙는다() {
    // T2 Acceptance — 즐겨찾기 줄과 보이는 드라이브 줄마다 아이콘 하나씩.
    // 비교 대상을 "그려진 줄 수"가 아니라 **입력에서 계산한다** — T4가 아이콘 없는 제목
    // 줄을 더하므로 줄을 세는 방식이면 그때 이 시험이 깨진다(계획 M3)
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;
    let favorites = [user_favorite(r"C:\Users"), user_favorite(r"C:\Windows")];
    let 드라이브 = { drive_rows().len() };
    let 예상 = favorites.len() + 드라이브;

    // 텍스처는 프레임당 8개까지만 새로 만들어진다(`icon_tex`) — 몇 프레임 돌려 채운다
    let mut 그려진 = 0;
    for _ in 0..8 {
        let output = draw_once_with_favorites(&mut panel, &SiteStore::new(), &favorites);
        그려진 = tree_icon_count(&output);
        if 그려진 >= 예상 {
            break;
        }
    }
    assert_eq!(
        그려진,
        예상,
        "트리 구역 아이콘 수가 즐겨찾기 {}개 + 드라이브 {드라이브}개와 다르다",
        favorites.len()
    );
}

/// 시험용 드라이브 줄 — 실제 PC 구성과 무관하게 원하는 상태를 만든다 (T5).
///
/// 아이콘 인덱스는 셸이 준 실값을 쓴다(0을 넣으면 텍스처가 만들어지지 않아 배지를
/// 그리는 자리 자체가 사라진다 — 배지는 아이콘이 올라온 프레임에만 그려진다)
fn drive_row(path: &str, network: bool, offline: bool) -> crate::fs::drives::DriveRow {
    let icon = drive_rows()
        .first()
        .map(|row| row.icon)
        .expect("이 PC에 드라이브가 하나는 있다");
    crate::fs::drives::DriveRow {
        path: std::path::PathBuf::from(path),
        label: format!("드라이브 ({path})"),
        icon,
        network,
        offline,
    }
}

/// 열거 결과 하나를 겪은 뒤 패널이 올릴 관측을 돌려준다 (T6)
fn observation_after(
    outcome: EnumOutcome,
    icons: &mut IconCache,
) -> Option<(std::path::PathBuf, bool)> {
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\Users"));
    panel.deferred_start = None;
    commit_dir(&mut panel, r"C:\Users", icons);
    panel.pending_dir = std::path::PathBuf::from(r"Z:\Docs");
    panel.pending_nav = PendingNav::Push;
    panel.apply_enumerated(
        outcome,
        icons,
        &mut crate::panel::dir_cache::DirCache::new(),
        &egui::Context::default(),
    );
    panel.observed_drive.clone()
}

#[test]
fn 열어_본_결과를_닿았는가로_갈라_올린다() {
    // T6 Design(갈래 규칙) — **`network` 깃발이 아니라 "닿았는가"로 판정한다**.
    // 배지를 그 깃발에 걸면 T1의 오류 코드 목록에서 빠진 실패 하나가 곧
    // "X가 영영 안 붙는" 결함이 된다(plan Risks)
    let _guard = crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
    let mut icons = IconCache::new();
    let 경로 = std::path::PathBuf::from(r"Z:\Docs");

    // 닿은 것 — 읽어 냈거나, 권한이 없을 뿐 드라이브에는 닿았다
    for outcome in [EnumOutcome::Ok(Vec::new()), EnumOutcome::AccessDenied] {
        // 어느 갈래가 틀렸는지 메시지로 알린다 — 루프라 실패 자리만으로는 가릴 수 없다
        let 갈래 = format!("{outcome:?}");
        assert_eq!(
            observation_after(outcome, &mut icons),
            Some((경로.clone(), true)),
            "닿은 결과를 못 닿은 것으로 올렸다: {갈래}"
        );
    }

    // 못 닿은 것 — 네트워크 깃발과 무관하게 열지 못했으면 못 닿았다
    for outcome in [
        EnumOutcome::Error { network: true },
        EnumOutcome::Error { network: false },
        EnumOutcome::NotFound,
    ] {
        let 갈래 = format!("{outcome:?}");
        assert_eq!(
            observation_after(outcome, &mut icons),
            Some((경로.clone(), false)),
            "열지 못한 결과를 닿은 것으로 올렸다: {갈래}"
        );
    }
}

#[test]
fn 관측은_한_번_올리면_비워진다() {
    // T6 — `show`가 `take()`로 가져가므로 다음 프레임에 같은 관측이 되풀이되지 않는다.
    // 남아 있으면 앱이 매 프레임 같은 값을 반영해 헛일을 한다
    let mut icons = IconCache::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.deferred_start = None;
    panel.pending_dir = std::path::PathBuf::from(r"Z:\");
    panel.pending_nav = PendingNav::Push;
    panel.apply_enumerated(
        EnumOutcome::Error { network: true },
        &mut icons,
        &mut crate::panel::dir_cache::DirCache::new(),
        &egui::Context::default(),
    );
    assert!(panel.observed_drive.is_some(), "관측이 세워지지 않았다");

    let _ = draw_once(&mut panel, &SiteStore::new());
    assert!(
        panel.observed_drive.is_none(),
        "한 프레임을 그린 뒤에도 관측이 남았다"
    );
}

#[test]
fn 끊긴_네트워크_드라이브_줄에만_배지가_붙는다() {
    // T5 Acceptance — 탐색기처럼 누르기 전에도 끊긴 것이 보인다 (2026-08-17 사용자 요청)
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;
    let drives = [
        drive_row(r"C:\", false, false),
        drive_row(r"Z:\", true, true),
    ];

    // 텍스처는 프레임당 8개까지만 만들어진다 — 몇 프레임 돌려 아이콘을 채운다
    let mut 배지 = 0;
    for _ in 0..8 {
        let output = draw_once_with(&mut panel, &SiteStore::new(), &[], &drives);
        배지 = offline_badges(&output);
        if 배지 > 0 {
            break;
        }
    }
    assert_eq!(배지, 1, "끊긴 드라이브 줄 하나에만 배지가 붙어야 한다");
}

#[test]
fn 닿는_드라이브에는_배지가_없다() {
    // T5 Acceptance — 로컬 드라이브·연결된 네트워크 드라이브에는 배지를 두지 않는다
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;
    let drives = [
        drive_row(r"C:\", false, false),
        drive_row(r"Z:\", true, false),
    ];

    for _ in 0..8 {
        let output = draw_once_with(&mut panel, &SiteStore::new(), &[], &drives);
        assert_eq!(offline_badges(&output), 0, "배지가 없어야 하는데 그려졌다");
    }
}

#[test]
fn 즐겨찾기와_하위_폴더에는_배지가_붙지_않는다() {
    // T5 Acceptance — 하위 폴더는 드라이브 줄과 **같은 `show_node`**를 지나므로,
    // 그 자리에서 `false`가 흐르는지 확인한다. 즐겨찾기는 `tree_row`의 다른 호출부다
    let mut panel = PanelState::new(std::path::PathBuf::from(r"C:\"));
    panel.tree_visible = true;
    panel.deferred_start = None;
    // 드라이브는 전부 닿는 상태로 두고 즐겨찾기만 얹는다 — 배지가 그려지면 그것은
    // 드라이브 줄이 아닌 자리에서 온 것이다
    let drives = [drive_row(r"C:\", false, false)];
    let favorites = [user_favorite(r"C:\Users"), user_favorite(r"C:\Windows")];

    for _ in 0..8 {
        let output = draw_once_with(&mut panel, &SiteStore::new(), &favorites, &drives);
        assert_eq!(
            offline_badges(&output),
            0,
            "드라이브 줄이 아닌 자리에 배지가 그려졌다"
        );
    }
}

/// 목록을 채워 둔 로컬 패널 — 이름 바꾸기 시험들이 함께 쓴다
fn panel_with_local_rows(dir: &str, rows: Vec<FileEntry>) -> (PanelState, egui::Context) {
    let ctx = egui::Context::default();
    let mut icons = IconCache::new();
    let mut panel = PanelState::new(std::path::PathBuf::from(dir));
    // 열거 결과가 도착한 것처럼 커밋시킨다 — 목록을 채우는 실제 경로를 그대로 지난다
    panel.start_load(std::path::PathBuf::from(dir), PendingNav::None, &ctx);
    panel.apply_enumerated(
        EnumOutcome::Ok(rows),
        &mut icons,
        &mut crate::panel::dir_cache::DirCache::new(),
        &egui::Context::default(),
    );
    (panel, ctx)
}

#[test]
fn 확정된_이름은_폴더_경로와_함께_올라간다() {
    // 셸에 거는 쪽(`ui::app`)은 전체 경로만 안다 — 행 번호를 경로로 바꾸는 것이 이 함수다
    let (panel, _ctx) = panel_with_local_rows(r"C:\테스트", vec![local_entry("보고서.txt", false)]);
    // 첫 줄은 상위 이동(`..`)이라 실제 항목은 1번이다
    let request = panel
        .take_rename(&FileListAction::Rename {
            index: 1,
            new_name: "새 이름.txt".to_owned(),
        })
        .expect("로컬 탭에서는 요청이 만들어져야 한다");
    assert_eq!(
        request.path,
        std::path::PathBuf::from(r"C:\테스트\보고서.txt")
    );
    assert_eq!(request.new_name, "새 이름.txt");
}

#[test]
fn 없는_행을_가리키면_이름을_걸지_않는다() {
    // 확정과 폴더 갱신이 같은 프레임에 겹치면 가리키던 항목이 이미 사라져 있을 수 있다 —
    // 그때는 아무것도 걸지 않는 편이 옳다(엉뚱한 항목의 이름을 바꾸는 것보다)
    let (panel, _ctx) = panel_with_local_rows(r"C:\테스트", vec![local_entry("a.txt", false)]);
    assert!(
        panel
            .take_rename(&FileListAction::Rename {
                index: 99,
                new_name: "b.txt".to_owned(),
            })
            .is_none()
    );
}

#[test]
fn 원격_탭에서는_인라인_이름_바꾸기가_올라가지_않는다() {
    // 원격은 대화로 이름을 묻는다 (FR-39) — 목록에 입력칸이 열리지도 않지만,
    // 열렸다 해도 셸에 로컬 경로로 걸리는 일이 없어야 한다
    let panel = panel_with_remote_tab("/pub");
    assert!(panel.is_remote(), "원격 탭이 활성이어야 한다");
    assert!(
        panel
            .take_rename(&FileListAction::Rename {
                index: 0,
                new_name: "b.txt".to_owned(),
            })
            .is_none()
    );
}

#[test]
fn 이름_바꾸기가_아닌_조작은_요청이_되지_않는다() {
    let (panel, _ctx) = panel_with_local_rows(r"C:\테스트", vec![local_entry("a.txt", false)]);
    assert!(panel.take_rename(&FileListAction::None).is_none());
    assert!(panel.take_rename(&FileListAction::Open(1)).is_none());
}

#[test]
fn 탭을_옮기면_고치던_이름은_버린다() {
    // 두 탭이 **같은 폴더**를 보고 있으면 폴더 판정으로는 가릴 수 없다 —
    // 탭 전환 자체가 취소여야 한다 (FR-64 Edge Case)
    let (mut panel, ctx) = panel_with_local_rows(r"C:\테스트", vec![local_entry("a.txt", false)]);
    assert!(
        !panel.begin_rename_selected(),
        "고른 것이 없으면 열리지 않는다"
    );
    // 실제 항목은 1번이다(0번은 상위 이동 줄) — 그것으로 편집을 연다
    assert!(panel.list.begin_rename(1), "편집이 열려야 한다");
    // 같은 폴더를 보는 탭을 하나 더 만들어 옮겨 간다
    panel.new_tab(&ctx);
    assert!(!panel.list.is_renaming(), "탭을 옮겼는데 편집이 남아 있다");
}

#[test]
fn 잘라내기_표시는_패널을_거쳐_목록에_닿는다() {
    let (mut panel, _ctx) = panel_with_local_rows(r"C:\테스트", vec![local_entry("a.txt", false)]);
    let 담은것 = [std::path::PathBuf::from(r"C:\테스트\a.txt")];
    panel.set_cut_marks(&담은것);
    assert!(panel.list.is_cut(std::path::Path::new(r"C:\테스트\a.txt")));
    panel.clear_cut_marks();
    assert!(!panel.list.is_cut(std::path::Path::new(r"C:\테스트\a.txt")));
}

#[test]
fn 표시_설정을_바꿔도_고치던_이름은_남는다() {
    // 숨김·시스템 표시를 켜면 같은 폴더를 다시 읽지만 탭은 그대로다 — 그 체크박스는
    // 마우스로 누르는 것이라 입력칸의 포커스를 뺏지 않는데, 거기서 편집을 버리면
    // 입력하던 이름이 예고 없이 사라진다 (품질 리뷰 M1)
    let (mut panel, ctx) = panel_with_local_rows(r"C:\테스트", vec![local_entry("a.txt", false)]);
    assert!(panel.list.begin_rename(1), "편집이 열려야 한다");
    panel.apply_display_rules(
        DisplayRules {
            show_extensions: true,
            show_hidden: false,
            show_system: true,
        },
        &ctx,
    );
    assert!(
        panel.list.is_renaming(),
        "표시 설정을 바꿨다고 편집을 버려서는 안 된다"
    );
}

#[test]
fn 고른_것이_없으면_파일_대상_명령은_대상을_얻지_못한다() {
    // 고른 것 없이 `F2`·`Delete`·`Ctrl+C`를 눌렀을 때다 (FR-12) — 아무 일도 일어나지
    // 않아야 한다. 실행하는 쪽(`ui::app`)이 보는 것은 이 두 값뿐이라 여기서 잡는다
    let (mut panel, _ctx) = panel_with_local_rows(r"C:\테스트", vec![local_entry("a.txt", false)]);
    assert!(panel.selected_local().is_empty(), "고른 것이 없다");
    assert!(!panel.begin_rename_selected(), "편집도 열리지 않는다");
}

#[test]
fn 원격_탭에서는_로컬_대상_목록이_비어_있다() {
    // 원격 탭의 삭제·클립보드는 로컬 경로를 얻지 못한다 — T11이 원격 기능으로 잇는다
    let panel = panel_with_remote_tab("/pub");
    assert!(panel.selected_local().is_empty());
}

#[test]
fn 로컬_탭에서는_원격_대상_목록이_비어_있다() {
    // 원격 라우팅(`route_to_remote`)이 이 값으로 대상을 고른다 — 로컬 탭에서 무엇이든
    // 돌려주면 원격 명령이 엉뚱한 곳에 걸린다
    let (panel, _ctx) = panel_with_local_rows(r"C:\테스트", vec![local_entry("a.txt", false)]);
    assert!(panel.selected_remote().is_empty());
}

// ── 앞선 클릭 직후의 더블클릭 (2026-09-03 사용자 보고) ──

/// 패널을 여러 프레임에 걸쳐 그리는 하네스 — 더블클릭 판정은 프레임에 걸친 포인터
/// 상태에서 나오므로 한 프레임짜리 `draw_once`로는 볼 수 없다
struct 클릭하네스 {
    panel: PanelState,
    sites: SiteStore,
    ctx: egui::Context,
    icons: IconCache,
    textures: crate::ui::icon_tex::IconTextures,
    시계: f64,
}

impl 클릭하네스 {
    /// 폴더 하나를 담은 로컬 패널
    fn 폴더하나() -> 클릭하네스 {
        let mut h = 클릭하네스 {
            panel: PanelState::new(std::path::PathBuf::from(r"C:\")),
            sites: SiteStore::new(),
            ctx: egui::Context::default(),
            icons: IconCache::new(),
            textures: crate::ui::icon_tex::IconTextures::new(),
            시계: 0.0,
        };
        let mut icons = IconCache::new();
        h.panel.pending_dir = std::path::PathBuf::from(r"C:\테스트");
        h.panel.pending_nav = PendingNav::None;
        let mut 폴더 = batch(&["하위폴더"]);
        폴더[0].is_dir = true;
        h.panel.apply_enumerated(
            EnumOutcome::Ok(폴더),
            &mut icons,
            &mut crate::panel::dir_cache::DirCache::new(),
            &egui::Context::default(),
        );
        h
    }

    /// `간격`초만큼 시계를 밀고 한 프레임 그린다 — egui는 시간이 뒤로 가면 패닉한다
    fn 프레임(&mut self, 간격: f64, events: Vec<egui::Event>) -> eframe::egui::FullOutput {
        self.시계 += 간격;
        let time = self.시계;
        let tree = crate::remote::tree_cache::TreeCache::new();
        let input = egui::RawInput {
            time: Some(time),
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 600.0),
            )),
            ..Default::default()
        };
        let panel = &mut self.panel;
        let icons = &mut self.icons;
        let textures = &mut self.textures;
        let sites = &self.sites;
        let mode = panel.view_mode();
        self.ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let ctx = ui.ctx().clone();
                textures.begin_frame();
                panel.show(
                    ui,
                    &ctx,
                    icons,
                    textures,
                    RemoteView {
                        sites,
                        connected: &[],
                        tree: &tree,
                    },
                    PanelMenuState::for_panes(1, mode),
                    crate::ui::tabs::TransferTargets::default(),
                    &[],
                    drive_rows(),
                );
            });
        })
    }

    /// 그 자리를 한 번 누르고 뗀다
    fn 클릭(&mut self, pos: egui::Pos2) {
        self.프레임(0.02, vec![egui::Event::PointerMoved(pos)]);
        self.프레임(0.02, vec![press_at(pos, true)]);
        self.프레임(0.02, vec![press_at(pos, false)]);
    }

    /// 폴더 줄을 더블클릭하고 그때 정해진 이동 대상을 돌려준다
    fn 폴더행_더블클릭(&mut self) -> std::path::PathBuf {
        let out = self.프레임(0.02, Vec::new());
        let (_, 자리) = drawn_text_positions(&out)
            .into_iter()
            .find(|(글, _)| 글 == "하위폴더")
            .expect("폴더 줄이 그려지지 않았다");
        let 행 = 자리 + egui::vec2(6.0, 6.0);
        self.panel.pending_dir = std::path::PathBuf::new();
        self.프레임(0.02, vec![egui::Event::PointerMoved(행)]);
        self.프레임(0.05, vec![press_at(행, true)]);
        self.프레임(0.05, vec![press_at(행, false)]);
        self.프레임(0.05, vec![press_at(행, true)]);
        self.프레임(0.05, vec![press_at(행, false)]);
        self.panel.pending_dir.clone()
    }
}

#[test]
fn 다른_곳을_누른_직후에도_더블클릭이_폴더를_연다() {
    // 사용자 보고(2026-09-03): 탭 줄 메뉴로 보기 모드를 바꾼 뒤로 목록을 더블클릭해도
    // 아무 일도 일어나지 않았다. 원인은 보기 모드가 아니라 **직전 클릭**이다 —
    // egui는 앞선 클릭에서 0.6초 안에 든 더블클릭을 **트리플클릭**으로 세고
    // (`max_double_click_delay * 2`), 그러면 `double_clicked()`가 서지 않는다.
    // 메뉴 항목을 고른 클릭이 바로 그 앞선 클릭이 되어, 이어지는 더블클릭이 통째로 죽었다
    let mut h = 클릭하네스::폴더하나();
    h.프레임(0.02, Vec::new());
    h.클릭(egui::pos2(400.0, 300.0));
    assert_eq!(
        h.폴더행_더블클릭(),
        std::path::PathBuf::from(r"C:\테스트\하위폴더"),
        "앞선 클릭 뒤의 더블클릭이 폴더를 열지 못했다"
    );
}

#[test]
fn 앞선_클릭이_없어도_더블클릭이_폴더를_연다() {
    // 위 시험의 짝 — 앞선 클릭이 없을 때는 종전대로 열린다(하네스가 옳다는 바탕)
    let mut h = 클릭하네스::폴더하나();
    h.프레임(0.02, Vec::new());
    assert_eq!(
        h.폴더행_더블클릭(),
        std::path::PathBuf::from(r"C:\테스트\하위폴더"),
        "앞선 클릭이 없는데도 더블클릭이 폴더를 열지 못했다"
    );
}

// ── 격자 보기의 클릭 (2026-09-03 사용자 보고) ──

#[test]
fn 격자_보기에서도_더블클릭이_폴더를_연다() {
    // 사용자 보고: 분할한 패널의 보기 모드를 아이콘 보기로 바꾼 뒤로 그 패널에서
    // 선택도 열기도 되지 않았고 **앱을 다시 켜도 그대로였다**(보기 모드가 세션에 남는다).
    // 원인은 빈 영역 위젯(`grid_bg`)이 스크롤 영역 전체를 차지한 채 **칸보다 나중에**
    // 등록된 것이다 — egui는 겹친 위젯 중 나중 등록을 위로 보므로(hit_test
    // "In tie, pick last = topmost") 배경이 모든 칸의 클릭을 가로챘다.
    // 자세히 보기는 배경을 마지막 행 아래에만 걸어 멀쩡했다
    let mut h = 클릭하네스::폴더하나();
    h.panel
        .set_view_mode(crate::ui::view_mode::ViewMode::LargeIcons);
    h.프레임(0.02, Vec::new());
    assert_eq!(
        h.폴더행_더블클릭(),
        std::path::PathBuf::from(r"C:\테스트\하위폴더"),
        "격자 보기에서 더블클릭이 폴더를 열지 못했다"
    );
}

#[test]
fn 격자_보기에서_한_번_클릭이_항목을_고른다() {
    // 같은 결함의 다른 얼굴 — 배경이 칸을 덮으면 선택도 함께 죽는다
    let mut h = 클릭하네스::폴더하나();
    h.panel
        .set_view_mode(crate::ui::view_mode::ViewMode::LargeIcons);
    let out = h.프레임(0.02, Vec::new());
    let (_, 자리) = drawn_text_positions(&out)
        .into_iter()
        .find(|(글, _)| 글 == "하위폴더")
        .expect("폴더 칸이 그려지지 않았다");
    h.클릭(자리 + egui::vec2(6.0, 6.0));
    h.프레임(0.02, Vec::new());
    assert_eq!(
        h.panel.selected_local().len(),
        1,
        "격자 보기에서 한 번 클릭이 항목을 고르지 못했다"
    );
}

#[test]
fn 격자_보기의_빈_영역_클릭은_선택을_푼다() {
    // 등록 순서를 앞당긴 뒤에도 배경 자체는 제 일을 해야 한다 (자세히 보기와 같은 규칙)
    let mut h = 클릭하네스::폴더하나();
    h.panel
        .set_view_mode(crate::ui::view_mode::ViewMode::LargeIcons);
    let out = h.프레임(0.02, Vec::new());
    let (_, 자리) = drawn_text_positions(&out)
        .into_iter()
        .find(|(글, _)| 글 == "하위폴더")
        .expect("폴더 칸이 그려지지 않았다");
    h.클릭(자리 + egui::vec2(6.0, 6.0));
    h.프레임(0.02, Vec::new());
    assert_eq!(h.panel.selected_local().len(), 1, "고르기가 되지 않았다");

    // 항목이 없는 아래쪽 빈 자리를 누른다
    h.클릭(egui::pos2(500.0, 550.0));
    h.프레임(0.02, Vec::new());
    assert!(
        h.panel.selected_local().is_empty(),
        "빈 영역을 눌러도 선택이 풀리지 않았다"
    );
}

#[test]
fn 한_프레임에_쌓인_배치를_합쳐_반영해도_목록이_온전하다() {
    // 회귀 — `poll_load`가 한 프레임에 쌓인 여러 배치를 **합쳐** 반영해도 항목이
    // 누락·중복·뒤섞이지 않는다. 합치기 이전에는 배치마다 `rebuild_visible`이 돌아
    // (누적 전체 재필터·재정렬·재복제) 워커가 UI보다 빠른 큰 폴더에서 그 한 프레임에
    // 정렬이 수십 번 몰려 앱이 멎었다(10만 항목 ≈ 1초 — 조사 시 실측).
    let dir = std::env::temp_dir().join(format!("moa_coalesce_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("임시 폴더");
    // 여러 배치가 나오도록 임계의 두 배 남짓을 만든다
    let count = crate::ui::panel::workers::PARTIAL_BATCH * 2 + 37;
    for i in 0..count {
        std::fs::write(dir.join(format!("f{i:05}.txt")), b"x").expect("파일");
    }

    let ctx = egui::Context::default();
    let mut icons = IconCache::new();
    let mut cache = crate::panel::dir_cache::DirCache::new();
    let mut panel = PanelState::new(dir.clone());
    panel.start_load(dir.clone(), PendingNav::None, &ctx);

    // 워커가 여러 배치를 채널에 쌓도록 짧게 기다린 뒤 소비 → 「한 프레임에 합치기」 경로를 태운다
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while panel.load.is_loading() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        panel.poll_load(&ctx, &mut icons, &mut cache);
    }
    assert!(!panel.load.is_loading(), "열거가 제때 끝나지 않았다");

    let entries = panel.list.entries().expect("로컬 목록");
    // 맨 위 `..` 줄을 뺀 나머지가 만든 파일 전부여야 한다
    let names: Vec<String> = entries
        .iter()
        .map(|e| e.name_string())
        .filter(|n| n != "..")
        .collect();
    assert_eq!(names.len(), count, "합치기 반영에서 항목이 누락·중복됐다");

    use std::collections::HashSet;
    let unique: HashSet<&String> = names.iter().collect();
    assert_eq!(unique.len(), count, "중복된 항목이 있다");

    // 이름 오름차순으로 정렬돼 있어야 한다(전량 재정렬과 같은 결과)
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "합쳐 반영한 목록이 정렬돼 있지 않다");

    let _ = std::fs::remove_dir_all(&dir);
}
