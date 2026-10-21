//! 모달 대화의 공통 껍데기 — 여덟 대화가 같은 모서리·같은 하단 버튼을 쓴다.
//!
//! 종전에는 대화마다 프레임을 따로 세웠다. 기본 `Modal`(여백 6·모서리 6)을 그대로 쓴 것,
//! `Frame::popup`에 여백만 넓힌 것, 손수 구성한 `Frame`(모서리 6과 0) — 셋으로 갈려 같은
//! 자리에서 뜨는 대화가 서로 달라 보였다. 프레임과 하단 버튼을 여기 한 벌만 두고 모두
//! 그것을 거치게 한다.
//!
//! 대화는 높이를 정하는 방식이 둘로 나뉜다. 확인 대화는 **본문이 높이를 정하고**(`show`),
//! 설정·사이트 관리자는 **자기 크기를 스스로 잡는다**(`show_fixed`). 그래서 받는 값도 다르다
//! — 앞은 본문 폭, 뒤는 프레임 크기다. 둘을 하나로 합치면 여섯 대화가 자기 본문 폭을
//! 프레임 폭으로 손수 환산해야 하고, 그 계산이 틀리면 글줄이 접힌다.
//!
//! **새로 만드는 대화도 이 모듈을 거친다** — `Modal`을 직접 쓰면 아래 시험이 잡는다.
use crate::ui::theme;
use eframe::egui;

/// 프레임 네 모서리 반경 (2026-08-15 사용자 결정)
pub const CORNER_RADIUS: u8 = 12;

/// 하단 버튼 줄 높이.
///
/// 기준 디자인에서 버튼 줄이 팝업 폭의 약 16%였고, 그 비율은 iOS alert의 규격(폭 270pt에
/// 버튼 44pt)과 같다. 종전 값(원격 대화 30 · 설정·사이트 관리자 58)이 제각각이라 하나로 모은다
pub const FOOTER_HEIGHT: f32 = 44.0;

/// 본문과 하단 버튼 줄 사이 — **`show_fixed`가 content 사각형에서 빼 준다**.
///
/// `show`(본문이 높이를 정하는 대화)는 본문 Frame의 `BODY_MARGIN`이 이미 그 자리를 만들지만,
/// `show_fixed`는 대화가 받은 사각형을 바닥까지 채워 쓰기 때문에 셸이 띄워 주지 않으면
/// 목록 웰·표의 아래 테두리가 버튼 줄 구분선에 닿아 **두 선이 한 줄처럼 보인다**
/// (2026-08-19 화면 확인 — 라이선스 대화에서 드러났고 사용자가 모든 팝업에 같은 규칙을 요청했다).
///
/// 여기 두는 이유: 대화마다 각자 빼게 하면 새로 만드는 팝업이 그것을 빠뜨린다
pub const BODY_GAP_BOTTOM: f32 = 12.0;

/// 본문 안쪽 여백 — `show`가 본문에 입힌다. `show_fixed`를 쓰는 대화는 스스로 관리한다
pub const BODY_MARGIN: i8 = 18;

/// 구분선 굵기
const DIVIDER: f32 = 1.0;

/// 굵게 그릴 때 좌우로 벌리는 간격 — 얕게 둔다. 크게 하면 획이 겹쳐 뭉개진다
const FAUX_BOLD_OFFSET: f32 = 0.6;

/// 대화 뒤를 덮는 어둠
const SCRIM_ALPHA: u8 = 140;

/// 프레임 그림자 — 종전 설정·사이트 관리자가 쓰던 값을 그대로 가져왔다
const SHADOW_OFFSET_Y: i8 = 18;
const SHADOW_BLUR: u8 = 60;
const SHADOW_ALPHA: u8 = 153;

/// 하단 버튼 하나
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonSpec<'a> {
    pub label: &'a str,
    /// 굵게 그린다 — 확인·삭제·연결처럼 그 대화의 주 동작인 자리
    pub emphasis: bool,
    /// 누를 수 있는가 — 거짓이면 흐리게 그리고 눌림을 받지 않는다
    pub enabled: bool,
}

impl<'a> ButtonSpec<'a> {
    /// 보통 버튼 (취소·건너뛰기 등)
    pub fn plain(label: &'a str) -> Self {
        Self {
            label,
            emphasis: false,
            enabled: true,
        }
    }

    /// 주 동작 버튼 — 굵게 그린다
    pub fn strong(label: &'a str) -> Self {
        Self {
            label,
            emphasis: true,
            enabled: true,
        }
    }

    /// 지금은 누를 수 없게 한다 — 일괄 이름 바꾸기의 `적용`처럼 막을 사유가 있을 때
    pub fn enabled_if(self, enabled: bool) -> Self {
        Self { enabled, ..self }
    }
}

/// 대화가 이번 프레임에 낸 반응.
///
/// `should_close`는 배경 클릭·`Esc`다 — 종전 대화들이 `ModalResponse::should_close()`로
/// 받던 것과 같은 판정이며, 셸을 거쳐도 그 경로가 사라지지 않게 그대로 돌려준다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shell {
    /// 눌린 하단 버튼의 자리 번호 (`buttons`에 준 순서)
    pub clicked: Option<usize>,
    /// 배경 클릭·`Esc`로 닫아야 하는가
    pub should_close: bool,
}

/// 본문이 높이를 정하는 대화 — 확인·입력 대화가 쓴다.
///
/// `body_width`는 **본문 폭**이다(종전 `ui.set_width`에 넘기던 값 그대로). 프레임은 그보다
/// 여백 두 번만큼 넓어지며, 하단 버튼은 그 프레임 전폭을 나눠 갖는다
pub fn show(
    ctx: &egui::Context,
    id: egui::Id,
    body_width: f32,
    buttons: &[ButtonSpec<'_>],
    body: impl FnOnce(&mut egui::Ui),
) -> Shell {
    let frame_width = body_width + f32::from(BODY_MARGIN) * 2.0;
    let mut clicked = None;
    let response = egui::Modal::new(id)
        .backdrop_color(egui::Color32::from_black_alpha(SCRIM_ALPHA))
        .frame(frame())
        .show(ctx, |ui| {
            ui.set_width(frame_width);
            // 여백은 셸이 입힌다 — 대화마다 다시 넣게 하면 어딘가는 빠뜨린다
            egui::Frame::new()
                .inner_margin(egui::Margin::same(BODY_MARGIN))
                .show(ui, |ui| {
                    ui.set_width(body_width);
                    body(ui);
                });
            // 버튼이 없으면 자리도 잡지 않는다 — 빈 44px이 본문 아래에 남지 않게
            if !buttons.is_empty() {
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(frame_width, FOOTER_HEIGHT),
                    egui::Sense::hover(),
                );
                clicked = footer(ui, rect, buttons);
            }
        });
    Shell {
        clicked,
        should_close: response.should_close(),
    }
}

/// 자기 크기를 스스로 잡는 대화 — 설정·사이트 관리자·라이선스가 쓴다.
///
/// `content`가 받는 사각형은 **하단 버튼 줄과 그 위 여백(`BODY_GAP_BOTTOM`)을 뺀 나머지**다.
/// 그 안에서 헤더·본문을 어떻게 나눌지는 대화가 정한다(좌우 여백은 각자 관리한다 —
/// 세 대화가 서로 다른 값을 쓴다). **아래 여백만 셸이 쥔다** — 그것을 대화에 맡기면
/// 새 팝업이 빠뜨려 본문이 버튼 줄에 붙는다
pub fn show_fixed(
    ctx: &egui::Context,
    id: egui::Id,
    frame_size: egui::Vec2,
    buttons: &[ButtonSpec<'_>],
    content: impl FnOnce(&mut egui::Ui, egui::Rect),
) -> Shell {
    let mut clicked = None;
    let response = egui::Modal::new(id)
        .backdrop_color(egui::Color32::from_black_alpha(SCRIM_ALPHA))
        .frame(frame())
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(frame_size, egui::Sense::hover());
            let footer_rect = egui::Rect::from_min_max(
                egui::pos2(rect.left(), rect.bottom() - FOOTER_HEIGHT),
                rect.max,
            );
            let content_rect = egui::Rect::from_min_max(
                rect.min,
                egui::pos2(rect.right(), footer_rect.top() - BODY_GAP_BOTTOM),
            );
            content(ui, content_rect);
            clicked = footer(ui, footer_rect, buttons);
        });
    Shell {
        clicked,
        should_close: response.should_close(),
    }
}

/// 여덟 대화가 함께 쓰는 프레임.
///
/// **안쪽 여백이 0이다** — 하단 버튼이 좌우 테두리에 닿아야 하기 때문이다. 본문 여백은
/// `show`가 안쪽에서 따로 입히고, `show_fixed`를 쓰는 대화는 자기 자리 계산에 포함한다
fn frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::SURFACE_BG)
        .stroke(egui::Stroke::new(1.0, theme::BORDER_CONTROL))
        .corner_radius(CORNER_RADIUS)
        .shadow(egui::epaint::Shadow {
            offset: [0, SHADOW_OFFSET_Y],
            blur: SHADOW_BLUR,
            spread: 0,
            color: egui::Color32::from_black_alpha(SHADOW_ALPHA),
        })
}

/// 하단을 버튼 수로 나눈 칸들.
///
/// **나머지 픽셀은 마지막 칸이 가져간다** — 폭이 버튼 수로 나눠떨어지지 않을 때 칸을 모두
/// 같은 폭으로 잡으면 오른쪽 끝에 틈이 남는다
pub fn footer_slots(rect: egui::Rect, count: usize) -> Vec<egui::Rect> {
    if count == 0 {
        return Vec::new();
    }
    let width = rect.width() / count as f32;
    (0..count)
        .map(|index| {
            let left = rect.left() + width * index as f32;
            // 오른쪽 끝도 **왼쪽과 같은 식**으로 센다 — `left + width`로 두면 부동소수
            // 누적이 달라져 다음 칸의 왼쪽과 머리카락만큼 어긋난다
            let right = if index + 1 == count {
                rect.right()
            } else {
                rect.left() + width * (index + 1) as f32
            };
            egui::Rect::from_min_max(
                egui::pos2(left, rect.top()),
                egui::pos2(right, rect.bottom()),
            )
        })
        .collect()
}

/// 그 칸의 hover 채움이 가질 모서리.
///
/// 버튼이 프레임 바닥에 닿아 있으므로 **양 끝 칸의 아래쪽만** 둥글다 — 각지게 두면 채움이
/// 둥근 테두리 밖으로 삐져나온다. 버튼이 하나뿐이면 그 칸이 양쪽을 다 갖는다
pub fn slot_corners(index: usize, count: usize, radius: u8) -> egui::CornerRadius {
    egui::CornerRadius {
        nw: 0,
        ne: 0,
        sw: if index == 0 { radius } else { 0 },
        se: if index + 1 == count { radius } else { 0 },
    }
}

/// 하단 버튼 줄을 그리고 눌린 칸을 돌려준다. 버튼이 없으면 아무것도 그리지 않는다.
///
/// 세 번에 나눠 그린다 — 채움·선·라벨 순이다. 한 칸씩 셋을 몰아 그리면 hover 채움이 바로
/// 앞 칸의 구분선을 덮는다
fn footer(ui: &egui::Ui, rect: egui::Rect, buttons: &[ButtonSpec<'_>]) -> Option<usize> {
    if buttons.is_empty() {
        return None;
    }
    let painter = ui.painter();
    let slots = footer_slots(rect, buttons.len());
    let mut clicked = None;

    for (index, (slot, spec)) in slots.iter().zip(buttons).enumerate() {
        let sense = if spec.enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        };
        let response = ui.interact(*slot, ui.id().with(("dialog_footer", index)), sense);
        if !spec.enabled {
            continue;
        }
        // 포커스에도 같은 표식을 준다 — `Sense::click()`이 포커스와 Space·Enter 활성화를
        // 함께 주는데(egui `context.rs`), 표식이 없으면 키보드로 옮겨 다닐 때 지금 어느
        // 버튼에 있는지 보이지 않는다. 종전 `ui.button`은 egui가 포커스 테두리를 그려 줬다
        if response.hovered() || response.has_focus() {
            painter.rect_filled(
                *slot,
                slot_corners(index, buttons.len(), CORNER_RADIUS),
                theme::ROW_HOT,
            );
        }
        if response.clicked() {
            clicked = Some(index);
        }
    }

    let line = egui::Stroke::new(DIVIDER, theme::BORDER_SUBTLE);
    // 선은 반 픽셀 안으로 밀어 그어야 1px로 또렷하다
    painter.line_segment(
        [
            egui::pos2(rect.left(), rect.top() + DIVIDER / 2.0),
            egui::pos2(rect.right(), rect.top() + DIVIDER / 2.0),
        ],
        line,
    );
    for slot in slots.iter().skip(1) {
        painter.line_segment(
            [
                egui::pos2(slot.left() + DIVIDER / 2.0, slot.top()),
                egui::pos2(slot.left() + DIVIDER / 2.0, slot.bottom()),
            ],
            line,
        );
    }

    // 글꼴은 앱 전역 버튼 글꼴을 그대로 쓴다 — 여기서 크기를 따로 정하면 대화 버튼만
    // 다른 화면의 버튼과 달라진다(`widgets::design_button`도 이 값을 쓴다)
    let font = egui::TextStyle::Button.resolve(ui.style());
    for (slot, spec) in slots.iter().zip(buttons) {
        // 라벨이 칸보다 길면 잘라 옆 칸을 침범하지 않게 한다
        let label = ui.painter_at(*slot);
        let color = if spec.enabled {
            theme::TEXT_BUTTON
        } else {
            theme::TEXT_DIM
        };
        if spec.emphasis {
            faux_bold_text(&label, slot.center(), spec.label, font.clone(), color);
        } else {
            label.text(
                slot.center(),
                egui::Align2::CENTER_CENTER,
                spec.label,
                font.clone(),
                color,
            );
        }
    }
    clicked
}

/// 굵은 글꼴 없이 굵게 — 같은 글자를 좌우로 아주 조금 벌려 두 번 그린다.
///
/// 이 앱은 글꼴을 한 벌만 등록하므로(`ui::app::install_fonts`) `RichText::strong()`은 색만
/// 바꾸고 획이 굵어지지 않는다. 사용자가 고른 글꼴의 굵은 짝을 찾는 길도 없다 — 글꼴 목록은
/// 가족 이름으로만 색인해 `맑은 고딕`과 그 굵은 짝이 같은 자리에 묻힌다(`app::fonts`).
/// 겹쳐 그리면 어떤 글꼴이든 그 글꼴 그대로 굵어지고 메모리도 더 쓰지 않는다
pub(crate) fn faux_bold_text(
    painter: &egui::Painter,
    center: egui::Pos2,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
) {
    for offset in [-FAUX_BOLD_OFFSET / 2.0, FAUX_BOLD_OFFSET / 2.0] {
        painter.text(
            egui::pos2(center.x + offset, center.y),
            egui::Align2::CENTER_CENTER,
            text,
            font.clone(),
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 시험용 하단 자리 — 왼쪽 위를 원점에 두고 폭만 바꾼다
    fn 자리(width: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(width, FOOTER_HEIGHT))
    }

    /// 고정 크기 대화의 본문이 버튼 줄에 닿지 않는지 — 새 팝업이 여백을 빠뜨려도 셸이 지킨다.
    ///
    /// 2026-08-19 화면 확인에서 라이선스 대화의 목록 웰 테두리가 버튼 줄 구분선과 겹쳐
    /// 한 줄처럼 보였다. 각 대화가 각자 빼던 것을 셸로 올렸으므로 그 계약을 여기서 못 박는다
    #[test]
    fn 고정_크기_대화의_본문은_버튼_줄과_떨어진다() {
        let ctx = egui::Context::default();
        let 프레임 = egui::vec2(480.0, 560.0);
        let mut 받은_자리 = egui::Rect::ZERO;
        let _ = ctx.run_ui(Default::default(), |ctx| {
            show_fixed(
                ctx,
                egui::Id::new("여백 시험"),
                프레임,
                &[ButtonSpec::plain("닫기")],
                |_ui, rect| 받은_자리 = rect,
            );
        });
        // 프레임 = 본문 + 여백 + 버튼 줄
        assert!(
            (프레임.y - 받은_자리.height() - BODY_GAP_BOTTOM - FOOTER_HEIGHT).abs() < 0.5,
            "본문 높이 {}px + 여백 {BODY_GAP_BOTTOM}px + 버튼 {FOOTER_HEIGHT}px이 프레임 {}px과 맞지 않는다",
            받은_자리.height(),
            프레임.y
        );
    }

    #[test]
    fn 버튼_둘이면_반씩_나눠_갖는다() {
        // 워크스페이스 삭제·이름 입력 등 — 본문 360이면 프레임은 396이다
        let slots = footer_slots(자리(396.0), 2);
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].width(), 198.0);
        assert_eq!(slots[1].width(), 198.0);
        assert_eq!(slots[0].right(), slots[1].left(), "칸 사이가 벌어졌다");
        assert_eq!(
            slots[1].right(),
            396.0,
            "마지막 칸이 오른쪽 끝에 닿지 않는다"
        );
    }

    #[test]
    fn 버튼_셋이면_삼등분한다() {
        // 같은 이름 확인 — 본문 420이면 프레임은 456이고 칸은 152다
        let slots = footer_slots(자리(456.0), 3);
        assert_eq!(slots.len(), 3);
        for slot in &slots {
            assert_eq!(slot.width(), 152.0);
        }
        assert_eq!(slots[2].right(), 456.0);
    }

    #[test]
    fn 나눠떨어지지_않아도_틈_없이_끝까지_채운다() {
        // 100을 셋으로 나누면 딱 떨어지지 않는다. 칸을 모두 같은 폭으로 잡으면 오른쪽 끝에
        // 틈이 남고, 경계를 서로 다른 식으로 세면 칸 사이에 머리카락만 한 금이 생긴다
        let slots = footer_slots(자리(100.0), 3);
        assert_eq!(slots[0].left(), 0.0);
        assert_eq!(
            slots[0].right(),
            slots[1].left(),
            "첫 칸과 둘째 칸 사이가 벌어졌다"
        );
        assert_eq!(
            slots[1].right(),
            slots[2].left(),
            "둘째 칸과 셋째 칸 사이가 벌어졌다"
        );
        assert_eq!(
            slots[2].right(),
            100.0,
            "마지막 칸이 오른쪽 끝에 닿지 않는다"
        );
    }

    #[test]
    fn 버튼이_없으면_칸도_없다() {
        assert!(footer_slots(자리(396.0), 0).is_empty());
    }

    #[test]
    fn 버튼_하나면_아래_두_모서리를_다_갖는다() {
        let corners = slot_corners(0, 1, CORNER_RADIUS);
        assert_eq!(corners.sw, CORNER_RADIUS);
        assert_eq!(corners.se, CORNER_RADIUS);
        assert_eq!(corners.nw, 0);
        assert_eq!(corners.ne, 0);
    }

    #[test]
    fn 가운데_칸은_어느_모서리도_둥글지_않다() {
        // 양 끝 칸만 프레임 테두리에 닿는다
        assert_eq!(slot_corners(0, 3, CORNER_RADIUS).sw, CORNER_RADIUS);
        assert_eq!(slot_corners(0, 3, CORNER_RADIUS).se, 0);
        assert_eq!(slot_corners(1, 3, CORNER_RADIUS), egui::CornerRadius::ZERO);
        assert_eq!(slot_corners(2, 3, CORNER_RADIUS).se, CORNER_RADIUS);
        assert_eq!(slot_corners(2, 3, CORNER_RADIUS).sw, 0);
    }

    #[test]
    fn 버튼_한_개면_가로선만_긋는다() {
        // `settings_dialog`의 `바닥_줄_위에_구분선을_긋는다`에서 옮겨 왔다 — 그 대화의 바닥은
        // 이제 이 모듈이 그린다. 세로 구분선은 칸이 둘 이상일 때만 생긴다
        let ctx = egui::Context::default();
        let output = ctx.run_ui(Default::default(), |ui| {
            footer(ui, 자리(480.0), &[ButtonSpec::plain("닫기")]);
        });
        let 선 = 선_개수(&output);
        assert_eq!(선, 1, "본문과 가르는 가로선이 없다");
    }

    #[test]
    fn 버튼_셋이면_세로선_둘을_더_긋는다() {
        let ctx = egui::Context::default();
        let output = ctx.run_ui(Default::default(), |ui| {
            footer(
                ui,
                자리(456.0),
                &[
                    ButtonSpec::strong("덮어쓰기"),
                    ButtonSpec::plain("건너뛰기"),
                    ButtonSpec::plain("취소"),
                ],
            );
        });
        assert_eq!(선_개수(&output), 3, "가로선 하나에 세로선 둘이어야 한다");
    }

    fn 선_개수(output: &egui::FullOutput) -> usize {
        output
            .shapes
            .iter()
            .filter(|clipped| matches!(clipped.shape, egui::Shape::LineSegment { .. }))
            .count()
    }

    /// `src/ui` 아래의 `.rs`를 **하위 폴더까지** 모아 온다.
    ///
    /// 비재귀로 훑으면 `ui/app/`·`ui/panel/` 같은 하위가 통째로 빠져 규약이 조용히 샌다
    /// (`ui::theme`의 같은 시험이 그 함정을 먼저 피했고, 2026-08-20에 이쪽도 맞췄다)
    fn ui_sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("ui 디렉터리") {
            let path = entry.expect("항목").path();
            if path.is_dir() {
                ui_sources(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn 대화는_모두_이_모듈을_거친다() {
        // 규약: 모달은 `ui::dialog`의 셸을 거친다. 문서로만 두면 다음 작업자가 `Modal`을
        // 곧바로 써도 아무것도 걸리지 않아, 팝업 모양이 다시 제각각이 된다
        let ui_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        // 자기 자신은 견주지 않는다 — **파일 이름이 아니라 전체 경로로** 가른다.
        // 이름만 보면 하위 폴더에 같은 이름이 생겼을 때 그 파일까지 조용히 빠진다
        let 자기 = ui_dir.join("dialog.rs");
        let mut 소스 = Vec::new();
        ui_sources(&ui_dir, &mut 소스);
        let mut 발견 = Vec::new();
        for path in 소스 {
            if path == 자기 {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("소스를 읽지 못했다");
            if source.contains("Modal::new") {
                발견.push(
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
        assert!(
            발견.is_empty(),
            "이 모듈을 거치지 않고 Modal을 직접 쓴 곳: {발견:?}"
        );
    }
}
