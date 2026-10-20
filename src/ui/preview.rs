//! 미리보기 창 (`Space`, FR-76) — 탐색기 영역 오른쪽에 하나, 활성 패널의 포커스 행을 따라간다.
//!
//! 내용은 `fs::preview` 워커가 만들고 여기는 대상이 바뀌면 청하고, 온 것을 그린다.
//! **창이 닫혀 있으면 아무것도 쥐지 않는다** — 워커는 처음 열 때 띄우고, 닫으면 워커·텍스처·
//! 글을 모두 놓는다(메모리 NFR-2 를 더 키우지 않는다). 열린 동안 쥐는 것은 결과 한 장이다.
use std::path::PathBuf;

use eframe::egui;

use crate::fs::enumerate::FileStamp;
use crate::fs::preview::{PreviewContent, PreviewWorker};
use crate::ui::theme;

/// 창 폭 — 기본과 끌어 바꿀 수 있는 범위
pub const DEFAULT_WIDTH: f32 = 320.0;
pub const MIN_WIDTH: f32 = 200.0;
pub const MAX_WIDTH: f32 = 800.0;

/// 무엇을 미리볼 것인가 — 활성 패널이 정한다(`PanelState::preview_target`)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewTarget {
    /// 고른 파일이 없다(폴더·`..` 줄·빈 선택 포함)
    Nothing,
    /// 원격 탭 — 원격 파일은 미리보지 않는다
    Remote,
    File {
        path: PathBuf,
        stamp: FileStamp,
    },
}

/// 창이 지금 보이는 것
enum Shown {
    Nothing,
    Remote,
    Loading,
    Image(egui::TextureHandle),
    Text(String),
    Binary,
    Failed,
}

pub struct PreviewPane {
    open: bool,
    worker: Option<PreviewWorker>,
    /// 지금 보이는(또는 기다리는) 파일 — 경로나 도장이 바뀌면 다시 청한다
    current: Option<(PathBuf, FileStamp)>,
    shown: Shown,
}

impl Default for PreviewPane {
    fn default() -> PreviewPane {
        PreviewPane::new()
    }
}

impl PreviewPane {
    pub fn new() -> PreviewPane {
        PreviewPane {
            open: false,
            worker: None,
            current: None,
            shown: Shown::Nothing,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// 켜고 끈다 — 끌 때 쥔 것을 모두 놓는다
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.worker = None;
            self.current = None;
            self.shown = Shown::Nothing;
        }
    }

    /// 이번 프레임의 대상을 받고 도착한 결과를 거둔다 — 열려 있을 때만 부른다
    pub fn update(&mut self, target: PreviewTarget, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        match target {
            PreviewTarget::Nothing => {
                self.current = None;
                self.shown = Shown::Nothing;
            }
            PreviewTarget::Remote => {
                self.current = None;
                self.shown = Shown::Remote;
            }
            PreviewTarget::File { path, stamp } => {
                let same = self
                    .current
                    .as_ref()
                    .is_some_and(|(at, at_stamp)| *at == path && *at_stamp == stamp);
                if !same {
                    // 새 대상 — 옛 그림을 곧바로 놓는다(한 번에 한 장)
                    self.shown = Shown::Loading;
                    self.worker
                        .get_or_insert_with(PreviewWorker::start)
                        .request(path.clone());
                    self.current = Some((path, stamp));
                }
            }
        }
        if let Some(content) = self.worker.as_mut().and_then(PreviewWorker::poll) {
            self.shown = match content {
                PreviewContent::Image(image) => {
                    let pixels = egui::ColorImage::from_rgba_unmultiplied(
                        [image.width, image.height],
                        &image.rgba,
                    );
                    Shown::Image(ctx.load_texture("preview_pane", pixels, Default::default()))
                }
                PreviewContent::Text(text) => Shown::Text(text),
                PreviewContent::Binary => Shown::Binary,
                PreviewContent::Failed => Shown::Failed,
            };
        }
        if matches!(self.shown, Shown::Loading) {
            // 워커는 창을 깨우지 않는다 — 기다리는 동안 스스로 다시 본다
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    /// 창 안을 그린다
    pub fn show(&self, ui: &mut egui::Ui) {
        let reason = match &self.shown {
            Shown::Nothing => Some(crate::i18n::preview_nothing()),
            Shown::Remote => Some(crate::i18n::preview_remote()),
            Shown::Loading => Some(crate::i18n::preview_loading()),
            Shown::Binary => Some(crate::i18n::preview_binary()),
            Shown::Failed => Some(crate::i18n::preview_failed()),
            Shown::Image(_) | Shown::Text(_) => None,
        };
        egui::Frame::new()
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                if let Some((path, _)) = &self.current
                    && let Some(name) = path.file_name()
                {
                    ui.label(egui::RichText::new(name.to_string_lossy()).color(theme::TEXT));
                    ui.add_space(6.0);
                }
                if let Some(reason) = reason {
                    ui.label(egui::RichText::new(reason).color(theme::TEXT_MUTED));
                    return;
                }
                match &self.shown {
                    Shown::Image(texture) => {
                        // 창 안에 비율을 지켜 맞춘다 — 원래보다 키우지는 않는다
                        let size = texture.size_vec2();
                        let room = ui.available_size();
                        let scale = (room.x / size.x).min(room.y / size.y).min(1.0);
                        ui.image((texture.id(), size * scale));
                    }
                    Shown::Text(text) => {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new(text.as_str()).color(theme::TEXT));
                            });
                    }
                    _ => {}
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64) -> PreviewTarget {
        PreviewTarget::File {
            path: std::env::temp_dir().join(name),
            stamp: FileStamp { size, modified: 0 },
        }
    }

    #[test]
    fn 닫혀_있으면_아무것도_쥐지_않는다() {
        let ctx = egui::Context::default();
        let mut pane = PreviewPane::new();
        pane.update(file("없는파일.txt", 1), &ctx);
        assert!(!pane.is_open());
        assert!(pane.worker.is_none(), "닫힌 창이 워커를 띄웠다");
        assert!(pane.current.is_none());
    }

    #[test]
    fn 닫으면_결과와_텍스처를_놓는다() {
        let ctx = egui::Context::default();
        let mut pane = PreviewPane::new();
        pane.toggle();
        pane.update(file("없는파일.txt", 1), &ctx);
        assert!(pane.worker.is_some(), "열어 고른 파일이 있는데 워커가 없다");
        pane.shown = Shown::Text("내용".to_owned());
        pane.toggle();
        assert!(pane.worker.is_none(), "닫았는데 워커가 남았다");
        assert!(pane.current.is_none());
        assert!(
            matches!(pane.shown, Shown::Nothing),
            "닫았는데 내용을 쥐고 있다"
        );
    }

    #[test]
    fn 같은_대상은_다시_요청하지_않는다() {
        let ctx = egui::Context::default();
        let mut pane = PreviewPane::new();
        pane.toggle();
        pane.update(file("같은파일.txt", 1), &ctx);
        // 결과가 온 것으로 친다 — 같은 대상이면 그것을 지키고 다시 읽는 중으로 돌아가지 않는다
        pane.shown = Shown::Text("내용".to_owned());
        pane.update(file("같은파일.txt", 1), &ctx);
        assert!(matches!(pane.shown, Shown::Text(_)));
    }

    #[test]
    fn 도장이_바뀌면_다시_요청한다() {
        let ctx = egui::Context::default();
        let mut pane = PreviewPane::new();
        pane.toggle();
        pane.update(file("바뀐파일.txt", 1), &ctx);
        pane.shown = Shown::Text("옛 내용".to_owned());
        pane.update(file("바뀐파일.txt", 2), &ctx);
        assert!(
            matches!(pane.shown, Shown::Loading),
            "내용이 바뀌었는데 옛 것을 보인다"
        );
    }

    #[test]
    fn 열면_사유_문구를_그린다() {
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        let ctx = egui::Context::default();
        let mut pane = PreviewPane::new();
        pane.toggle();
        pane.update(PreviewTarget::Remote, &ctx);
        let output = ctx.run_ui(Default::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| pane.show(ui));
        });
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(
            texts.iter().any(|t| t == "원격 파일은 미리보지 않습니다"),
            "{texts:?}"
        );
    }
}
