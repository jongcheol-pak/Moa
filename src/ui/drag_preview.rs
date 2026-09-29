//! 앱 **안**에서 끄는 동안 커서를 따라오는 미리보기 그림 (FR-38·FR-60).
//!
//! egui는 페이로드를 세워도 그림을 그리지 않는다 — 커서 아이콘만 `Grabbing`으로 바꾼다
//! (`egui::DragAndDrop`의 `on_end_pass`). 그래서 끄는 동안 무엇을 끌고 있는지 보이지 않았고,
//! 이 모듈이 그 그림을 직접 그린다.
//!
//! **픽셀을 만들지 않는다** — 목록이 이미 올려 둔 텍스처(`ThumbnailTextures`·`IconTextures`)를
//! 그대로 읽는다. 없으면 그 프레임은 그리지 않고 다음 프레임에 채워진다.
//!
//! 창 **밖**으로 나간 뒤의 그림은 이 모듈이 아니라 셸이 그린다(`fs::drag_image` — FR-61 ⓑ).
//! 그 경계에서 그림이 둘이 되지 않도록 포인터가 뷰포트를 벗어나면 여기서는 그리지 않는다.
use crate::fs::icons::{IconCache, IconSize};
use crate::ui::icon_tex::{IconTextures, ThumbnailTextures};
use crate::ui::list_common::DragItem;
use eframe::egui;

/// 그림 한 변의 논리 픽셀.
///
/// 창 밖으로 끌어낼 때의 96px(`ui::app::pump_export_drag`)보다 작다 — 그쪽은 바탕화면·탐색기
/// 위에 뜨지만 이 그림은 행 높이가 20px 안팎인 파일 목록 위에 뜨므로, 96px이면 다섯 줄을 덮어
/// 어디에 놓는지가 보이지 않는다 (plan D6)
const PREVIEW_PX: f32 = 64.0;

/// 그림에 곱하는 불투명도 — 아래 목록이 비쳐 보일 만큼만 남긴다 (≈0.78)
const PREVIEW_ALPHA: u8 = 200;

/// 커서에서 오른쪽 아래로 비키는 거리.
///
/// 그림 중심을 커서에 맞추면 커서가 덮여 **정확히 어느 줄에 놓는지**가 보이지 않는다 —
/// 놓인 자리는 커서 한 점이 정하므로(`PanelState::take_drop`) 그 점을 가리면 안 된다 (plan D7)
const CURSOR_OFFSET: f32 = 12.0;

/// 이번 프레임에 미리보기로 들 항목을 정한다 — **순수 판정**.
///
/// 판정 순서가 계약이다:
/// 1. 이번 프레임에 끌기가 시작됐으면(`started`가 `Some`) 그 **첫 항목**. `has_payload`를 보지
///    않는다 — 호출부가 페이로드를 세우기 **전에** 이 함수를 부르므로 시작 프레임에는
///    `has_payload`가 거짓인데, 여기서 그것을 먼저 보면 그 프레임의 그림이 버려진다.
/// 2. 아니고 페이로드가 사라졌으면 없음. 드롭·`Esc` 취소·창 밖 넘김·다른 패널의 수거가 모두
///    이 한 갈래로 모인다(넷 다 `DragAndDrop`의 페이로드를 비운다).
/// 3. 그 밖에는 들고 있던 것 그대로.
///
/// UI 없이 도는 순수 함수로 떼어 둔 이유는 이 수명 판정만 시험할 수 있게 하기 위함이다
/// (AGENTS: HWND가 필요한 UI 로직은 테스트 비대상 — 순수 로직을 분리해 테스트)
pub fn next_item(
    held: Option<DragItem>,
    started: Option<&[DragItem]>,
    has_payload: bool,
) -> Option<DragItem> {
    if let Some(items) = started {
        // 여러 개를 끌어도 첫 항목 한 장이다 (plan D4)
        return items.first().cloned();
    }
    if !has_payload {
        return None;
    }
    held
}

/// 커서 자리에 그림 한 장을 그린다. 그릴 것이 없으면 아무 일도 하지 않는다.
///
/// **HWND·실제 그리기가 걸려 테스트 비대상이다** — 수명 판정은 위 `next_item`이 맡는다
pub fn show(
    ctx: &egui::Context,
    icons: &mut IconCache,
    textures: &mut IconTextures,
    thumbs: &ThumbnailTextures,
    item: &DragItem,
) {
    let Some(pos) = ctx.pointer_interact_pos() else {
        return;
    };
    // 창 밖은 셸 드래그가 이어받는 구간이다 — 여기서 계속 그리면 그림이 둘이 된다
    if !ctx.input(|input| input.viewport_rect().contains(pos)) {
        return;
    }
    let Some(texture) = texture_for(ctx, icons, textures, thumbs, item) else {
        // 프레임당 텍스처 생성 상한에 걸린 프레임 — 다음 몇 프레임 안에 채워진다
        return;
    };
    // 목록·도크·대화 위에 떠야 하므로 자기 레이어를 쓴다. `interactable(false)`로 상호작용을
    // 끊어, 드롭 판정(`PanelState::take_drop`)이 이 그림에 가리지 않게 한다
    // 열쇠는 ASCII로 둔다 — 한글 열쇠는 `i18n`의 소스 훑기 시험이 화면 문구로 보아
    // 예외 목록(`EXEMPT_LITERALS`)에 손으로 등재해야 한다(`app.rs`의 `titlebar`·`status_bar`와 같은 표기)
    egui::Area::new(egui::Id::new("drag_preview"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .fixed_pos(pos + egui::vec2(CURSOR_OFFSET, CURSOR_OFFSET))
        .show(ctx, |ui| {
            let (rect, _) =
                ui.allocate_exact_size(egui::Vec2::splat(PREVIEW_PX), egui::Sense::hover());
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
            ui.painter().image(
                texture,
                rect,
                uv,
                egui::Color32::from_white_alpha(PREVIEW_ALPHA),
            );
        });
}

/// 이 항목에 쓸 텍스처 — **이미 올라와 있는 것만** 쓴다.
///
/// 썸네일이 있으면 그것, 없으면 형식 아이콘이다(창 밖으로 끌어낼 때와 같은 규칙 — plan D4).
/// 원격 항목은 끌기 시작 시점에 로컬에 파일이 없어 썸네일 자체가 없으므로 언제나 아이콘이다
fn texture_for(
    ctx: &egui::Context,
    icons: &mut IconCache,
    textures: &mut IconTextures,
    thumbs: &ThumbnailTextures,
    item: &DragItem,
) -> Option<egui::TextureId> {
    // 썸네일은 파일만 — 폴더는 폴더 아이콘이 맞다 (목록과 같은 규칙)
    if let DragItem::Local {
        path,
        is_dir: false,
    } = item
        && let Some(tex) = thumbs.get(path)
    {
        return Some(tex.id());
    }
    let himl = icons.himl_for(IconSize::for_px(PREVIEW_PX));
    let ext = extension_of(item);
    // 로컬은 전체 경로를 함께 준다 — exe·lnk·ico는 파일마다 아이콘이 다르다.
    // **도장은 없다**(끄는 항목에 크기·시각이 없다) — 목록이 이미 같은 조회를 맡겨 대개 캐시에
    // 맞고, 없으면 확장자 아이콘으로 확정된다(여기서 워커에 맡기지 않는다)
    let index = match item {
        DragItem::Local { path, is_dir } => {
            let full = path.to_string_lossy();
            icons.icon_index(&ext, *is_dir, Some((&full, None))).index
        }
        DragItem::Remote { is_dir, .. } => icons.icon_index(&ext, *is_dir, None).index,
    };
    textures.get(ctx, himl, index).map(|tex| tex.id())
}

/// 아이콘 캐시가 열쇠로 쓰는 확장자 표기 — 점 없는 소문자, 없으면 빈 문자열.
///
/// 목록이 `FileEntry::extension`·`RemoteEntry::extension`으로 얻는 것과 같은 값이어야
/// 같은 캐시 항목에 맞는다. 폴더는 `IconCache::icon_index`가 확장자를 보기 전에
/// 폴더 아이콘으로 갈라내므로 여기서 따로 거르지 않는다
fn extension_of(item: &DragItem) -> String {
    std::path::Path::new(&item.name())
        .extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 항목(name: &str) -> DragItem {
        DragItem::Local {
            path: std::path::PathBuf::from(r"C:\work").join(name),
            is_dir: false,
        }
    }

    #[test]
    fn 끌기를_시작하면_첫_항목을_든다() {
        let 끄는_것 = vec![항목("a.txt"), 항목("b.txt")];
        assert_eq!(
            next_item(None, Some(&끄는_것), true),
            Some(항목("a.txt")),
            "여러 개를 끌어도 첫 항목 한 장이다"
        );
    }

    #[test]
    fn 끌_항목이_없으면_담지_않는다() {
        assert_eq!(next_item(None, Some(&[]), false), None);
    }

    #[test]
    fn 페이로드가_사라지면_비운다() {
        // 드롭·`Esc` 취소·창 밖 넘김·다른 패널의 수거가 모두 이 한 갈래로 모인다
        assert_eq!(next_item(Some(항목("a.txt")), None, false), None);
    }

    #[test]
    fn 끄는_동안에는_들고_있던_것을_지킨다() {
        assert_eq!(
            next_item(Some(항목("a.txt")), None, true),
            Some(항목("a.txt"))
        );
    }

    #[test]
    fn 시작_프레임에는_페이로드_판정보다_시작이_먼저다() {
        // 호출부가 페이로드를 세우기 **전에** 부르므로 시작 프레임의 `has_payload`는 거짓이다.
        // 여기서 페이로드를 먼저 보면 이번 프레임의 그림이 버려진다
        assert_eq!(
            next_item(Some(항목("옛것.txt")), Some(&[항목("새것.txt")]), false),
            Some(항목("새것.txt"))
        );
    }

    #[test]
    fn 확장자는_점_없는_소문자다() {
        // 아이콘 캐시의 열쇠라 목록이 쓰는 표기와 같아야 한다
        assert_eq!(extension_of(&항목("보고서.TXT")), "txt");
        assert_eq!(extension_of(&항목("확장자없음")), "");
        assert_eq!(
            extension_of(&항목(".gitignore")),
            "",
            "앞점 이름은 확장자가 아니다"
        );
    }
}
