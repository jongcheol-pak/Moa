//! 빠른 이동 팔레트 (`Ctrl+P`, FR-75) — 입력란 하나와 후보 줄들.
//!
//! 후보를 모으고 순위를 매기는 규칙은 `app::palette` 가 갖고, 여기는 입력·그리기·키 처리만 한다.
//! 고른 곳으로 옮기는 것은 앱이다(워크스페이스·패널·탭을 쥔 곳이 거기다).
use eframe::egui;

use crate::app::palette::{self, Candidate, Source, Target};
use crate::ui::{dialog, theme, widgets};

/// 본문 폭 — 긴 경로도 반쯤은 보이게 잡았다
const BODY_WIDTH: f32 = 560.0;

/// 후보 줄 높이와 목록의 최대 높이(넘으면 목록 안에서 굴린다)
const ROW_HEIGHT: f32 = 30.0;
const LIST_MAX_HEIGHT: f32 = 360.0;

/// 열려 있는 동안의 상태
struct Open {
    query: String,
    candidates: Vec<Candidate>,
    /// 지금 질의에 걸린 후보의 자리 — 보일 차례대로
    shown: Vec<usize>,
    /// `shown` 안에서 고른 줄
    cursor: usize,
    /// 처음 그리는 프레임 — 입력란에 포커스를 준다
    first_frame: bool,
}

impl Open {
    fn refresh(&mut self) {
        self.shown = palette::rank(&self.candidates, &self.query);
        self.cursor = 0;
    }

    fn chosen(&self) -> Option<Target> {
        self.shown
            .get(self.cursor)
            .and_then(|&index| self.candidates.get(index))
            .map(|candidate| candidate.target.clone())
    }
}

/// 팔레트 — 닫혀 있으면 아무것도 그리지 않는다
#[derive(Default)]
pub struct QuickOpen {
    open: Option<Open>,
}

impl QuickOpen {
    pub fn new() -> QuickOpen {
        QuickOpen::default()
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// 후보를 받아 연다 — 열 때 한 번 모은 것을 닫을 때까지 쓴다(모달이라 그사이 바뀌지 않는다)
    pub fn open(&mut self, candidates: Vec<Candidate>) {
        let mut open = Open {
            query: String::new(),
            candidates,
            shown: Vec::new(),
            cursor: 0,
            first_frame: true,
        };
        open.refresh();
        self.open = Some(open);
    }

    /// 그린다 — 줄을 고르면(`Enter`·클릭) 그 대상을 돌려주고 닫힌다
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Target> {
        let open = self.open.as_mut()?;
        // 입력란이 화살표·Enter·Esc 를 먹기 전에 먼저 거둔다 — 한 줄 입력란은 위아래 화살표로
        // 커서를 줄 끝으로 옮기고 Enter 로 포커스를 놓는다
        let (down, up, enter, escape) = ctx.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                input.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                input.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                input.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        if escape {
            self.open = None;
            return None;
        }
        if down && open.cursor + 1 < open.shown.len() {
            open.cursor += 1;
        }
        if up {
            open.cursor = open.cursor.saturating_sub(1);
        }
        let mut clicked = None;
        let shell = dialog::show(
            ctx,
            egui::Id::new("quick_open_palette"),
            BODY_WIDTH,
            &[],
            |ui| clicked = draw_body(ui, open),
        );
        if let Some(row) = clicked {
            open.cursor = row;
        }
        let chosen = if enter || clicked.is_some() {
            open.chosen()
        } else {
            None
        };
        if chosen.is_some() || shell.should_close {
            self.open = None;
        }
        chosen
    }
}

/// 입력란과 후보 줄들 — 눌린 줄의 `shown` 자리를 돌려준다
fn draw_body(ui: &mut egui::Ui, open: &mut Open) -> Option<usize> {
    let size = egui::vec2(BODY_WIDTH, widgets::FORM_FIELD_HEIGHT);
    let before = open.query.clone();
    let field = widgets::text_field(ui, "quick_open_query", &mut open.query, size, true, false);
    if open.first_frame {
        open.first_frame = false;
        ui.memory_mut(|memory| memory.request_focus(field.id));
    }
    if open.query.is_empty() {
        let rect = field.rect.shrink2(egui::vec2(10.0, 0.0));
        ui.painter().text(
            egui::pos2(rect.left(), rect.center().y),
            egui::Align2::LEFT_CENTER,
            crate::i18n::palette_placeholder(),
            egui::FontId::proportional(widgets::FORM_FONT_PX),
            theme::TEXT_DIM,
        );
    }
    if open.query != before {
        open.refresh();
    }
    ui.add_space(8.0);
    if open.shown.is_empty() {
        ui.label(egui::RichText::new(crate::i18n::palette_empty()).color(theme::TEXT_MUTED));
        return None;
    }
    let mut clicked = None;
    egui::ScrollArea::vertical()
        .max_height(LIST_MAX_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for (row, &index) in open.shown.iter().enumerate() {
                let candidate = &open.candidates[index];
                let (rect, response) = ui
                    .allocate_exact_size(egui::vec2(BODY_WIDTH, ROW_HEIGHT), egui::Sense::click());
                if row == open.cursor {
                    ui.scroll_to_rect(rect, None);
                }
                draw_row(ui, rect, candidate, row == open.cursor, response.hovered());
                if response.clicked() {
                    clicked = Some(row);
                }
            }
        });
    clicked
}

/// 한 줄 — 이름 · 흐린 경로 · 오른쪽 끝의 출처
fn draw_row(ui: &egui::Ui, rect: egui::Rect, candidate: &Candidate, current: bool, hovered: bool) {
    let fill = if current {
        theme::MENU_HOT
    } else if hovered {
        theme::ROW_HOT
    } else {
        egui::Color32::TRANSPARENT
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, theme::MENU_ITEM_CORNER_RADIUS, fill);
    let font = egui::FontId::proportional(widgets::FORM_FONT_PX);
    let source = match candidate.source {
        Source::Recent => crate::i18n::palette_source_recent(),
        Source::Favorite => crate::i18n::palette_source_favorite(),
        Source::Tab => crate::i18n::palette_source_tab(),
        Source::Workspace => crate::i18n::palette_source_workspace(),
    };
    let source_rect = painter.text(
        egui::pos2(rect.right() - 10.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        source,
        font.clone(),
        theme::TEXT_DIM,
    );
    let left = rect.left() + 10.0;
    let label = crate::ui::list_common::elided_galley_colored(
        &painter,
        candidate.label.clone(),
        font.clone(),
        BODY_WIDTH * 0.4,
        theme::TEXT,
    );
    let label_width = label.size().x;
    painter.galley(
        egui::pos2(left, rect.center().y - label.size().y / 2.0),
        label,
        theme::TEXT,
    );
    let detail_left = left + label_width + 12.0;
    let detail_width = (source_rect.left() - 12.0 - detail_left).max(0.0);
    let detail = crate::ui::list_common::elided_galley_colored(
        &painter,
        candidate.detail.clone(),
        font,
        detail_width,
        theme::TEXT_MUTED,
    );
    painter.galley(
        egui::pos2(detail_left, rect.center().y - detail.size().y / 2.0),
        detail,
        theme::TEXT_MUTED,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::workspace::WorkspaceId;
    use std::path::PathBuf;

    fn candidates() -> Vec<Candidate> {
        palette::collect(
            &[PathBuf::from(r"C:\첫째"), PathBuf::from(r"C:\둘째")],
            &[],
            &[],
            &[(WorkspaceId(1), "기본".to_owned())],
        )
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// 한 프레임 — 이벤트를 넣어 그리고, 그린 글자들과 고른 대상을 돌려준다
    fn frame(
        ctx: &egui::Context,
        palette: &mut QuickOpen,
        events: Vec<egui::Event>,
    ) -> (Vec<String>, Option<Target>) {
        let mut chosen = None;
        let input = egui::RawInput {
            events,
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ctx| {
            chosen = palette.show(ctx);
        });
        let texts = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        (texts, chosen)
    }

    #[test]
    fn 닫혀_있으면_아무것도_그리지_않는다() {
        let ctx = egui::Context::default();
        let mut palette = QuickOpen::new();
        let output = ctx.run_ui(Default::default(), |ctx| {
            palette.show(ctx);
        });
        let shapes = output
            .shapes
            .iter()
            .filter(|shape| !matches!(shape.shape, egui::epaint::Shape::Noop))
            .count();
        assert_eq!(shapes, 0);
    }

    #[test]
    fn 열면_후보를_그린다() {
        // 떠 있는 영역은 첫 프레임에 자리만 잡는다 — 두 프레임을 돌린다
        let ctx = egui::Context::default();
        let mut palette = QuickOpen::new();
        palette.open(candidates());
        frame(&ctx, &mut palette, Vec::new());
        let (texts, _) = frame(&ctx, &mut palette, Vec::new());
        for label in ["첫째", "둘째", "기본"] {
            assert!(
                texts.iter().any(|t| t == label),
                "{label} 줄이 없다: {texts:?}"
            );
        }
    }

    #[test]
    fn 아래_화살표와_enter_로_둘째_후보를_고른다() {
        let ctx = egui::Context::default();
        let mut palette = QuickOpen::new();
        palette.open(candidates());
        frame(&ctx, &mut palette, Vec::new());
        frame(&ctx, &mut palette, vec![key(egui::Key::ArrowDown)]);
        let (_, chosen) = frame(&ctx, &mut palette, vec![key(egui::Key::Enter)]);
        assert_eq!(chosen, Some(Target::Path(PathBuf::from(r"C:\둘째"))));
        assert!(!palette.is_open(), "고른 뒤에도 열려 있다");
    }

    #[test]
    fn 질의를_치면_걸리는_줄만_남고_enter_가_그것을_고른다() {
        let ctx = egui::Context::default();
        let mut palette = QuickOpen::new();
        palette.open(candidates());
        frame(&ctx, &mut palette, Vec::new());
        frame(
            &ctx,
            &mut palette,
            vec![egui::Event::Text("기본".to_owned())],
        );
        let (_, chosen) = frame(&ctx, &mut palette, vec![key(egui::Key::Enter)]);
        assert_eq!(chosen, Some(Target::Workspace(WorkspaceId(1))));
    }

    #[test]
    fn esc_는_고르지_않고_닫는다() {
        let ctx = egui::Context::default();
        let mut palette = QuickOpen::new();
        palette.open(candidates());
        frame(&ctx, &mut palette, Vec::new());
        let (_, chosen) = frame(&ctx, &mut palette, vec![key(egui::Key::Escape)]);
        assert_eq!(chosen, None);
        assert!(!palette.is_open());
    }
}
