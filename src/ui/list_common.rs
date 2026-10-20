//! 보기 모드별 렌더 모듈이 함께 쓰는 조각 (FR-4·FR-23).
//!
//! `ui::list_details`(자세히)와 `ui::list_grid`(나머지 보기)가 같은 조작 타입과 텍스트
//! 배치 규칙을 쓴다. 한쪽에 두고 다른 쪽이 참조하면 두 렌더 모듈이 서로를 알게 되므로
//! 공용 조각만 여기로 모은다.
use eframe::egui;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// 자세히 보기가 한 프레임에 주고받는 폴더 크기 (FR-71).
///
/// 알려진 값은 패널의 크기 워커가 들고, 그리는 쪽은 **보이는 폴더 행 중 값이 없는 것**을
/// `wanted`에 담아 돌려준다 — 계산을 요청하는 것은 패널이다
pub struct DirSizes<'a> {
    pub known: &'a HashMap<PathBuf, u64>,
    pub wanted: Vec<PathBuf>,
}

/// 목록이 상위(패널)에 돌려주는 사용자 조작.
/// 즉시 모드라 콜백을 등록하지 않고 이번 프레임의 조작을 값으로 반환한다
#[derive(Clone, PartialEq, Debug, Default)]
pub enum FileListAction {
    #[default]
    None,
    /// 항목 실행 — 폴더면 진입, 파일이면 연결 프로그램 (호출부가 판정)
    Open(usize),
    /// 컨텍스트 메뉴 요청 — `index`가 `None`이면 빈 영역(폴더 배경 메뉴)
    Context {
        index: Option<usize>,
        pos: egui::Pos2,
    },
    /// 목록에서 고친 이름이 확정됐다 (FR-64) — 실제로 바꾸는 것은 셸에 맡긴다.
    ///
    /// `index`를 함께 싣는 이유: 경로만으로는 확정 시점에 그 행이 아직 그 자리에 있는지
    /// 부르는 쪽이 확인할 수 없다(폴더가 그 사이에 다시 읽혔을 수 있다)
    Rename { index: usize, new_name: String },
}

/// 끌어 옮기는 항목 하나 (FR-38).
///
/// **로컬과 원격을 한 타입에 섞지 않는다** — 로컬은 파일시스템 경로, 원격은 서버 경로이고
/// 크기도 원격만 미리 안다. 하나로 뭉치면 받는 쪽이 매번 "이게 어느 쪽이지"를 되묻게 된다
#[derive(Debug, Clone, PartialEq)]
pub enum DragItem {
    Local {
        path: std::path::PathBuf,
        is_dir: bool,
    },
    Remote {
        path: crate::remote::types::RemotePath,
        is_dir: bool,
        size: u64,
    },
}

impl DragItem {
    /// 폴더인가 — 로컬·원격 어느 쪽이든 같은 물음이다
    pub fn is_dir(&self) -> bool {
        match self {
            DragItem::Local { is_dir, .. } | DragItem::Remote { is_dir, .. } => *is_dir,
        }
    }

    /// 옮겨 놓을 때 쓸 이름 — 경로의 마지막 조각
    pub fn name(&self) -> String {
        match self {
            DragItem::Local { path, .. } => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            DragItem::Remote { path, .. } => path.file_name().unwrap_or_default().to_owned(),
        }
    }
}

/// 목록에서 끌기 시작할 때 싣는 값 (FR-38).
///
/// **패널 번호를 싣지 않는다** — 받는 쪽이 알아야 하는 것은 "어디서 왔나"가 아니라
/// "로컬인가 원격인가"뿐이고(그 조합만으로 전송인지 복사인지가 정해진다), 번호를 실으면
/// 그 사이 패널이 닫혔을 때 가리키는 곳이 사라진다
#[derive(Debug, Clone, PartialEq)]
pub struct FileDrag {
    pub items: Vec<DragItem>,
    /// 원격에서 끌어온 것이면 그 사이트 — 받는 쪽이 어느 서버에서 받을지 알아야 한다
    pub source_site: Option<crate::remote::types::SiteId>,
}

/// 같은 이름이 있을 때 사용자가 고른 것 (FR-55).
///
/// 취소는 이 값이 아니라 대화가 `Cancelled`로 알린다 — 취소는 "무엇을 할지"가 아니라
/// "아무것도 하지 않는다"라서 여기 넣으면 호출부가 그것도 처리해야 할 선택지로 읽는다.
///
/// `DragItem`·`DropOutcome`과 같은 자리에 둔다 — 전송을 여는 쪽(`ui::app`)과 물어보는
/// 쪽(`ui::remote_menu`)이 둘 다 쓰므로, 어느 한쪽에 두면 두 모듈이 서로를 알게 된다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    /// 있던 것을 덮어쓰고 전부 보낸다
    Overwrite,
    /// 겹치는 것만 빼고 나머지를 보낸다
    Skip,
}

/// 끌어다 놓은 자리 — 그 패널이 지금 보고 있는 폴더다
#[derive(Debug, Clone, PartialEq)]
pub enum DropTarget {
    Local(std::path::PathBuf),
    Remote {
        site: crate::remote::types::SiteId,
        dir: crate::remote::types::RemotePath,
    },
}

/// 드롭 한 번의 결과 — 실제로 큐에 넣는 것은 앱이 한다 (plan T22 의존 방향)
#[derive(Debug, Clone, PartialEq)]
pub struct DropOutcome {
    pub items: Vec<DragItem>,
    /// 원격에서 끌어왔으면 그 사이트
    pub source_site: Option<crate::remote::types::SiteId>,
    pub target: DropTarget,
}

/// 끌어다 놓은 항목 하나가 실제로 옮겨지는가, 옮겨진다면 어느 방향인가 (FR-38).
///
/// **로컬 → 원격은 올리기, 원격 → 로컬은 받기**뿐이다. 로컬끼리·원격끼리는 `None`이다 —
/// 원격↔원격은 PRD Out of Scope이고, **로컬↔로컬은 전송이 아니라 셸 복사**라서
/// 아래 `local_copy_target`이 따로 판정한다(FR-60).
///
/// **이 함수에 복사를 섞지 않는다** — 이 값은 전송 큐에 넣을 항목을 거르는 필터로도
/// 쓰이므로(`ui::app::transfer_conflict`), 로컬↔로컬이 여기를 통과하면 복사할 것이
/// 전송 큐에 들어간다
pub fn drop_direction(
    item: &DragItem,
    target: &DropTarget,
) -> Option<crate::remote::connection::TransferDirection> {
    use crate::remote::connection::TransferDirection;
    match (item, target) {
        (DragItem::Local { .. }, DropTarget::Remote { .. }) => Some(TransferDirection::Upload),
        (DragItem::Remote { .. }, DropTarget::Local(_)) => Some(TransferDirection::Download),
        _ => None,
    }
}

/// 이 드롭이 **로컬끼리의 복사**인가 — 맞으면 놓인 폴더를 돌려준다 (FR-60).
///
/// 항목이 전부 로컬이고 놓인 자리도 로컬일 때만 성립한다. 원격이 하나라도 섞이면
/// `None`이며 그 드롭은 종전대로 전송 경로(`drop_direction`)로 간다.
///
/// **끌어온 자리에 그대로 놓으면 `None`이다** — 항목들이 있던 폴더가 놓인 폴더와 같으면
/// 복사를 걸지 않는다(2026-08-21 사용자 요청). 종전에는 셸에 넘겨 `- 복사본`이 생겼는데
/// 사본을 만들려던 조작이 아니었다. **탭이 아니라 폴더로 견주므로** 다른 탭이 우연히 같은
/// 폴더를 보고 있을 때도 취소된다.
///
/// `None`이 곧 "아무 일도 없음"인 이유: 호출부(`ui::app`)는 `None`을 전송 경로로 보내는데
/// 로컬↔로컬은 `drop_direction`이 전부 `None`이라 보낼 것이 하나도 남지 않아 큐 등록도
/// 확인 대화도 열리지 않는다.
///
/// 빈 항목도 `None`이다: 복사를 걸 것이 없으면 아무 일도 일어나지 않아야 한다
pub fn local_copy_target(drop: &DropOutcome) -> Option<&std::path::Path> {
    let DropTarget::Local(dir) = &drop.target else {
        return None;
    };
    if drop.items.is_empty()
        || !drop
            .items
            .iter()
            .all(|item| matches!(item, DragItem::Local { .. }))
    {
        return None;
    }
    let from_same_dir = drop.items.iter().all(|item| match item {
        DragItem::Local { path, .. } => path.parent().is_some_and(|from| same_dir(from, dir)),
        // 위 `all`이 이미 걸렀으므로 닿지 않는다. 닿는다면 로컬 복사가 아니므로 제자리도 아니다
        DragItem::Remote { .. } => false,
    });
    if from_same_dir {
        return None;
    }
    Some(dir.as_path())
}

/// 두 경로가 같은 폴더를 가리키는가 — **ASCII 대소문자를 접어서** 견준다.
///
/// `Path`의 기본 비교는 구성요소 단위라 끝 구분자 차이(`C:\a\b\` vs `C:\a\b`)는 이미
/// 흡수하지만 대소문자는 흡수하지 않는다. Windows 파일시스템은 대소문자를 가리지 않으므로
/// 주소창으로 `C:\Work`를 연 탭과 `C:\work`를 연 탭은 같은 폴더다.
///
/// **파일시스템을 두드리지 않는다**(`canonicalize` 등) — 드롭마다 부르는 자리라
/// 블로킹 호출을 넣을 수 없다(AGENTS: UI 스레드 파일시스템 블로킹 금지). ASCII만 접는 것은
/// 한글·한자에는 대소문자가 없어 이 근사로 충분하기 때문이다
fn same_dir(a: &std::path::Path, b: &std::path::Path) -> bool {
    let mut left = a.components();
    let mut right = b.components();
    loop {
        match (left.next(), right.next()) {
            (None, None) => return true,
            (Some(x), Some(y)) => {
                if !x
                    .as_os_str()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(&y.as_os_str().to_string_lossy())
                {
                    return false;
                }
            }
            _ => return false,
        }
    }
}

/// 숨김·시스템 항목을 그릴 때 글자·아이콘에 곱하는 불투명도 (FR-13).
///
/// 탐색기가 숨김 항목을 흐리게 보이는 것과 같은 표시다 — 목록에 있지만 보통 항목은
/// 아니라는 것을 색만으로 알린다. **글자와 아이콘에 같은 값을 쓴다**: 한쪽만 흐리면
/// 항목이 반쯤 지워진 것처럼 보인다
pub const HIDDEN_ALPHA: f32 = 0.5;

/// 흐리게 그릴 항목이면 흐린 색으로 바꾼다 — 글자색과 아이콘 tint가 함께 쓰는 한 벌의 규칙.
///
/// **무엇이 그 대상인지는 여기서 정하지 않는다** — 호출부가 `ListRow::is_dimmed()`로
/// 판정해 넘긴다(숨김이거나 시스템). 색 변환과 판정을 한 함수에 두면 목록·트리처럼
/// 판정 기준이 다른 자리가 이 함수를 함께 쓸 수 없다
pub fn dim_if_hidden(color: egui::Color32, hidden: bool) -> egui::Color32 {
    if hidden {
        color.gamma_multiply(HIDDEN_ALPHA)
    } else {
        color
    }
}

/// 목록에서 이름을 고치는 중인 상태 (FR-64).
///
/// **행 번호로 든다** — 편집 중에는 그 행이 화면에 그려지고 있어야 하고, 그리는 쪽이 아는
/// 것은 번호다. 폴더가 다시 읽혀 번호가 달라지는 경우는 상태를 쥔 `ui::file_list`가
/// 이름으로 다시 찾아 고친다(그쪽이 옛 목록과 새 목록을 둘 다 아는 유일한 자리다)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameEdit {
    pub index: usize,
    /// 편집을 열 때의 이름 — 폴더가 다시 읽혔을 때 그 행을 **이름으로 다시 찾는** 데 쓴다.
    ///
    /// `text`로는 찾을 수 없다: 사용자가 이미 고치고 있는 글자라 목록의 어느 이름과도
    /// 맞지 않는다
    pub original: String,
    /// 지금 입력칸에 든 글자 — 그리는 쪽이 곧바로 고친다
    pub text: String,
    /// 아직 포커스를 주지 않았는가. 첫 프레임에만 참이며 거기서 선택 범위도 잡는다
    pub first_frame: bool,
    /// 폴더인가 — 첫 프레임의 선택 범위를 가른다(폴더는 이름 전체)
    pub is_dir: bool,
}

/// 이름 편집이 이번 프레임에 끝난 방식 (FR-64)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameEnd {
    /// `Enter` — 이 이름으로 확정한다
    Commit(String),
    /// `Esc`·다른 곳 누르기 — 되돌린다
    Cancel,
}

/// 잘라내기로 담긴 항목을 그릴 때 곱하는 불투명도 (FR-64).
///
/// 숨김 항목과 **같은 0.5를 쓰되 상수를 따로 둔다** — 둘은 뜻이 다르고(하나는 파일 속성,
/// 하나는 클립보드 상태) 한쪽 값을 바꿀 때 다른 쪽이 딸려 가면 안 된다
pub const CUT_ALPHA: f32 = 0.5;

/// 잘라내기로 담긴 항목이면 흐린 색으로 바꾼다 — 탐색기와 같은 표시다.
///
/// **글자는 `theme::TEXT_DIM`으로 낮추고 거기에 불투명도까지 곱한다** — 색만 낮추면 숨김
/// 항목과 구분이 되지 않고, 불투명도만 낮추면 잘라낸 것인지 흐린 화면인지 알기 어렵다
pub fn cut_text_color(cut: bool) -> Option<egui::Color32> {
    cut.then(|| crate::ui::theme::TEXT_DIM.gamma_multiply(CUT_ALPHA))
}

/// 잘라내기로 담긴 항목의 아이콘에 곱할 tint — 담기지 않았으면 `None`
pub fn cut_icon_tint(cut: bool) -> Option<egui::Color32> {
    cut.then(|| egui::Color32::WHITE.gamma_multiply(CUT_ALPHA))
}

/// 편집을 열었을 때 처음 잡아 둘 선택 범위 — **글자 수 기준**이다 (FR-64).
///
/// 탐색기와 같은 규칙이다: 폴더는 이름 전체, 파일은 **마지막 점 앞까지**를 잡는다
/// (`report.tar.gz` → `report.tar`). 확장자만 그대로 두고 이름을 고치는 것이 대부분이라
/// 매번 손으로 범위를 줄이지 않게 한다.
///
/// **맨 앞의 점은 확장자 구분이 아니다** — `.gitignore`는 이름 전체가 잡힌다
pub fn name_edit_range(name: &str, is_dir: bool) -> std::ops::Range<usize> {
    let chars = name.chars().count();
    if is_dir {
        return 0..chars;
    }
    let last_dot = name
        .chars()
        .enumerate()
        .filter(|(_, ch)| *ch == '.')
        .map(|(at, _)| at)
        .last();
    match last_dot {
        Some(at) if at > 0 => 0..at,
        _ => 0..chars,
    }
}

/// 이름 칸 자리에 입력칸을 얹고, 이번 프레임에 편집이 끝났으면 그 방식을 돌려준다 (FR-64).
///
/// **두 렌더 모듈이 같은 함수를 쓴다** — 자세히 보기와 격자 보기는 칸의 자리만 다르고
/// 편집이 시작되고 끝나는 규칙은 같다. 각자 적으면 한쪽만 고쳐져 보기 모드에 따라
/// `Esc`가 다르게 듣는 일이 생긴다.
///
/// `Enter`만 확정이고 `Esc`·다른 곳 누르기는 취소다 — 실수로 바뀐 이름이 조용히 확정되는
/// 것보다 되돌리는 쪽이 안전하다
pub fn rename_editor(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    edit: &mut RenameEdit,
) -> Option<RenameEnd> {
    // 행 번호를 id에 섞는다 — 편집이 다른 행으로 옮겨 가면 egui가 옛 상태(커서·선택)를
    // 물려주지 않아야 한다
    let id = ui.id().with(("rename", edit.index));
    let resp = ui.put(
        rect,
        egui::TextEdit::singleline(&mut edit.text)
            .id(id)
            // 이름이 칸보다 길면 잘라 보인다 — 넘치는 만큼 칸이 늘어나면 옆 열을 덮는다
            .clip_text(true)
            .desired_width(rect.width()),
    );
    if edit.first_frame {
        edit.first_frame = false;
        resp.request_focus();
        // 확장자 앞까지 잡아 둔다 — 상태는 위젯이 자기 id로 들고 있어 여기서 덮어쓴다
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
            let range = name_edit_range(&edit.text, edit.is_dir);
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(range.start),
                    egui::text::CCursor::new(range.end),
                )));
            egui::TextEdit::store_state(ui.ctx(), id, state);
        }
    }
    if resp.lost_focus() {
        // `Esc`는 egui가 포커스를 거두므로 여기로 온다 — `Enter`로 끝난 것만 확정이다
        let by_enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
        return Some(if by_enter {
            RenameEnd::Commit(edit.text.clone())
        } else {
            RenameEnd::Cancel
        });
    }
    None
}

/// 텍스트를 **한 줄로만** 배치하고, 폭을 넘으면 끝을 `…`로 줄인 갤리를 만든다.
///
/// `Painter::layout`을 쓰면 안 된다 — 그 함수의 폭 인자는 자르는 폭이 아니라 **줄바꿈 폭**이라
/// 긴 이름이 여러 줄이 된다. 행 높이가 고정인 자세히 보기에서는 2줄이 되는 순간 아래 행과
/// 겹쳐 글자가 포개져 보인다(사용자 보고 4번)
///
/// **갤리는 색을 구워 넣는다** — `Painter::galley`에 넘기는 색은 갤리 안이 `PLACEHOLDER`일
/// 때만 쓰인다. 그래서 기본색으로 만든 갤리를 그리면서 다른 색을 넘겨도 **아무 일도 일어나지
/// 않는다**(로그·큐 표에서 실제로 그랬다 — T20 리뷰). 그래서 색을 인자로 받는다
pub fn elided_galley_colored(
    painter: &egui::Painter,
    text: String,
    font: egui::FontId,
    max_width: f32,
    color: egui::Color32,
) -> Arc<egui::Galley> {
    elided_galley_rows(painter, text, font, max_width, 1, color)
}

/// 줄 수를 지정하는 변형 — 격자 보기의 이름은 두 줄까지 쓴다 (plan 시각 속성 표).
///
/// `max_rows`를 넘으면 마지막 줄 끝에 `…`가 붙는다. 파일 이름은 공백 없는 긴 토큰이 흔해
/// 단어 단위로만 끊으면 폭을 넘은 채 잘리므로 아무 곳에서나 끊게 한다
pub fn elided_galley_rows(
    painter: &egui::Painter,
    text: String,
    font: egui::FontId,
    max_width: f32,
    max_rows: usize,
    color: egui::Color32,
) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text, font, color, max_width);
    job.wrap = egui::text::TextWrapping {
        max_width,
        max_rows,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    painter.layout_job(job)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::connection::TransferDirection;
    use crate::remote::types::{RemotePath, SiteId};
    use crate::ui::theme;

    fn local_item() -> DragItem {
        DragItem::Local {
            path: std::path::PathBuf::from(r"C:\work\app.js"),
            is_dir: false,
        }
    }

    fn remote_item() -> DragItem {
        DragItem::Remote {
            path: RemotePath::new("/var/www/app.js"),
            is_dir: false,
            size: 10,
        }
    }

    fn local_target() -> DropTarget {
        DropTarget::Local(std::path::PathBuf::from(r"C:\down"))
    }

    fn remote_target() -> DropTarget {
        DropTarget::Remote {
            site: SiteId(1),
            dir: RemotePath::new("/var/www"),
        }
    }

    #[test]
    fn 로컬에서_원격으로_끌면_올리기다() {
        // Acceptance ①
        assert_eq!(
            drop_direction(&local_item(), &remote_target()),
            Some(TransferDirection::Upload)
        );
        assert_eq!(
            drop_direction(&remote_item(), &local_target()),
            Some(TransferDirection::Download)
        );
    }

    #[test]
    fn 같은_쪽끼리_끌면_아무_일도_없다() {
        // Acceptance ② — 이 함수는 **전송 방향만** 답한다. 원격↔원격은 PRD Out of Scope이고,
        // 로컬↔로컬은 전송이 아니라 셸 복사라 `local_copy_target`이 판정한다(FR-60)
        assert_eq!(drop_direction(&local_item(), &local_target()), None);
        assert_eq!(drop_direction(&remote_item(), &remote_target()), None);
    }

    #[test]
    fn 로컬끼리_놓으면_그_폴더로_복사한다() {
        // Acceptance ⓐ (FR-60)
        let drop = DropOutcome {
            items: vec![local_item()],
            source_site: None,
            target: local_target(),
        };
        assert_eq!(
            local_copy_target(&drop),
            Some(std::path::Path::new(r"C:\down"))
        );
    }

    #[test]
    fn 원격이_섞이거나_대상이_원격이면_복사가_아니다() {
        // Acceptance ⓑⓒ — 그 드롭은 종전대로 전송 경로로 간다
        let 섞임 = DropOutcome {
            items: vec![local_item(), remote_item()],
            source_site: None,
            target: local_target(),
        };
        assert_eq!(
            local_copy_target(&섞임),
            None,
            "원격이 하나라도 섞이면 복사가 아니다"
        );

        let 원격_대상 = DropOutcome {
            items: vec![local_item()],
            source_site: None,
            target: remote_target(),
        };
        assert_eq!(local_copy_target(&원격_대상), None, "올리기는 전송이다");
    }

    #[test]
    fn 놓은_것이_없으면_복사할_것도_없다() {
        // Acceptance ⓓ
        let 빈_드롭 = DropOutcome {
            items: Vec::new(),
            source_site: None,
            target: local_target(),
        };
        assert_eq!(local_copy_target(&빈_드롭), None);
    }

    #[test]
    fn 같은_폴더에_놓으면_복사하지_않는다() {
        // 2026-08-21 사용자 요청 — 끌어온 자리에 그대로 놓으면 사본을 만들지 않는다
        let 제자리 = DropOutcome {
            items: vec![local_item()],
            source_site: None,
            target: DropTarget::Local(std::path::PathBuf::from(r"C:\work")),
        };
        assert_eq!(local_copy_target(&제자리), None);
    }

    #[test]
    fn 대소문자만_다른_같은_폴더도_취소다() {
        // Windows 파일시스템은 대소문자를 가리지 않는다 — 주소창으로 연 탭이 다른
        // 대소문자를 들고 있어도 같은 폴더다
        let 대소문자_다름 = DropOutcome {
            items: vec![DragItem::Local {
                path: std::path::PathBuf::from(r"C:\Work\app.js"),
                is_dir: false,
            }],
            source_site: None,
            target: DropTarget::Local(std::path::PathBuf::from(r"C:\work")),
        };
        assert_eq!(local_copy_target(&대소문자_다름), None);
    }

    #[test]
    fn 여러_항목_중_하나라도_다른_폴더에서_왔으면_복사다() {
        // 전부 제자리일 때만 취소다 — 하나라도 밖에서 왔으면 복사할 것이 있다
        let 섞인_출처 = DropOutcome {
            items: vec![
                DragItem::Local {
                    path: std::path::PathBuf::from(r"C:\work\a.txt"),
                    is_dir: false,
                },
                DragItem::Local {
                    path: std::path::PathBuf::from(r"C:\other\b.txt"),
                    is_dir: false,
                },
            ],
            source_site: None,
            target: DropTarget::Local(std::path::PathBuf::from(r"C:\work")),
        };
        assert_eq!(
            local_copy_target(&섞인_출처),
            Some(std::path::Path::new(r"C:\work"))
        );
    }

    #[test]
    fn 끌_항목의_이름은_경로의_마지막_조각이다() {
        assert_eq!(local_item().name(), "app.js");
        assert_eq!(remote_item().name(), "app.js");
    }

    #[test]
    fn 숨김_항목만_흐려진다() {
        // 숨김이 아닌 항목의 색은 손대지 않는다 — 목록 전체가 흐려지면 표시가 뜻을 잃는다
        assert_eq!(dim_if_hidden(theme::TEXT, false), theme::TEXT);
        let dimmed = dim_if_hidden(theme::TEXT, true);
        assert!(
            dimmed.a() < theme::TEXT.a(),
            "숨김 항목인데 불투명도가 그대로다"
        );
        // 아이콘 tint도 같은 규칙을 쓴다 (한쪽만 흐리면 항목이 반쯤 지워진 것처럼 보인다)
        assert_eq!(
            dim_if_hidden(egui::Color32::WHITE, true).a(),
            dimmed.a(),
            "글자와 아이콘의 흐림 정도가 다르다"
        );
    }

    /// 갤리에 실제로 구워진 색 — `Painter::galley`에 넘기는 색은 이 값이 `PLACEHOLDER`일 때만 쓰인다
    fn baked_color(galley: &egui::Galley) -> egui::Color32 {
        galley.job.sections[0].format.color
    }

    #[test]
    fn 색을_지정한_갤리는_그_색을_구워_넣는다() {
        // T20 리뷰 — 기본색으로 만든 갤리를 그리면서 다른 색을 넘겨도 아무 일도 일어나지 않는다.
        // 로그 본문·큐 표의 색이 실제로는 전부 기본색으로 나오던 결함이 여기서 비롯됐다
        let ctx = egui::Context::default();
        let mut plain = None;
        let mut colored = None;
        let _ = ctx.run_ui(Default::default(), |ui| {
            let painter = ui.painter();
            plain = Some(elided_galley_colored(
                painter,
                "본문".to_owned(),
                egui::FontId::proportional(13.0),
                100.0,
                theme::TEXT,
            ));
            colored = Some(elided_galley_colored(
                painter,
                "본문".to_owned(),
                egui::FontId::proportional(13.0),
                100.0,
                theme::TEXT_LOG,
            ));
        });
        assert_eq!(baked_color(&plain.expect("기본")), theme::TEXT);
        assert_eq!(baked_color(&colored.expect("지정")), theme::TEXT_LOG);
    }

    /// 앱과 같은 글꼴을 설치한 뒤 배치한다 — 이 crate는 egui 기본 글꼴 기능을 끄고
    /// 맑은 고딕을 직접 등록하므로, 글꼴 없이 배치하면 모든 글자 폭이 0이 되어
    /// 폭 기준 검증이 무의미해진다
    fn layout(text: &str, width: f32, rows: usize) -> (usize, bool) {
        let ctx = egui::Context::default();
        let has_font = crate::ui::app::install_fonts(&ctx, None);
        let mut result = (0, false);
        let _ = ctx.run_ui(Default::default(), |ui| {
            let font = egui::TextStyle::Body.resolve(ui.style());
            let galley = elided_galley_rows(
                ui.painter(),
                text.to_owned(),
                font,
                width,
                rows,
                theme::TEXT,
            );
            result = (galley.rows.len(), galley.elided);
        });
        assert!(has_font, "맑은 고딕을 읽지 못해 폭 기준 검증을 할 수 없다");
        result
    }

    #[test]
    fn 한_줄_배치는_폭을_넘으면_줄인다() {
        let long = "NTUSER.DAT{71e7eeb8-8e0f-11f0-80fa-000d3aa7ca88}.TM.blf";
        let (rows, elided) = layout(long, 100.0, 1);
        assert_eq!(rows, 1);
        assert!(elided);
    }

    #[test]
    fn 격자_이름은_두_줄까지_쓴다() {
        // 한 줄로 자르면 격자에서 이름이 지나치게 짧아진다 — 탐색기도 두 줄을 쓴다
        let long = "아주긴한글파일이름입니다그리고더깁니다.txt";
        let (one, _) = layout(long, 100.0, 1);
        let (two, _) = layout(long, 100.0, 2);
        assert_eq!(one, 1);
        assert_eq!(two, 2, "두 줄을 허용했는데 한 줄로 잘렸다");
    }

    #[test]
    fn 두_줄에_들어가면_줄이지_않는다() {
        let (rows, elided) = layout("짧은이름.txt", 300.0, 2);
        assert_eq!(rows, 1);
        assert!(!elided);
    }

    #[test]
    fn 아주_좁은_폭에서도_패닉하지_않는다() {
        for width in [0.0, 1.0, 3.0, -10.0] {
            for rows in [1, 2] {
                let (drawn, _) = layout("아주긴한글파일이름.txt", width, rows);
                assert!(drawn <= rows, "폭 {width}·{rows}줄: 허용 줄 수를 넘겼다");
            }
        }
    }
}
