//! 일괄 이름 바꾸기 대화 (FR-73).
//!
//! 방식 고르기 → 입력칸 → 「확장자도 바꾸기」 → 미리보기 표(현재 → 새 이름, 막힌 줄은 사유)
//! → `적용`·`취소` 차례로 그린다(plan D11). 새 이름을 짓고 막을 사유를 가리는 것은
//! `app::bulk_rename`이 하고, 여기는 입력과 그리기만 한다.
//!
//! **충돌 판정에 쓸 폴더 안 이름은 열 때 워커가 모은다** — 숨김·필터로 목록에 안 보이는
//! 항목까지 디스크 기준이어야 하고, 폴더를 읽는 것은 UI 스레드에서 하지 않는다(AGENTS).
//! 다 오기 전에는 `적용`이 흐리다.
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui;

use crate::app::bulk_rename::{self, Block, FolderNames, Item, Rule};
use crate::ui::{dialog, theme, widgets};

/// 본문 폭 — 미리보기 표의 두 열이 긴 이름도 반쯤은 보이게 잡았다
const BODY_WIDTH: f32 = 560.0;

/// 미리보기 표의 최대 높이 — 넘으면 표 안에서 굴린다
const TABLE_MAX_HEIGHT: f32 = 260.0;

const TITLE_FONT_PX: f32 = 16.0;

/// 이름 짓는 방식 — 화면의 세 칸
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Replace,
    Number,
    Regex,
}

/// 열려 있는 동안의 입력 상태
struct Open {
    items: Vec<Item>,
    mode: Mode,
    find: String,
    with: String,
    prefix: String,
    suffix: String,
    start: u32,
    digits: u32,
    keep_name: bool,
    pattern: String,
    regex_with: String,
    include_ext: bool,
    names: FolderNames,
    names_rx: Option<Receiver<FolderNames>>,
}

impl Open {
    fn rule(&self) -> Rule {
        match self.mode {
            Mode::Replace => Rule::Replace {
                find: self.find.clone(),
                with: self.with.clone(),
            },
            Mode::Number => Rule::Number {
                prefix: self.prefix.clone(),
                suffix: self.suffix.clone(),
                start: self.start,
                digits: self.digits,
                keep_name: self.keep_name,
            },
            Mode::Regex => Rule::Regex {
                pattern: self.pattern.clone(),
                with: self.regex_with.clone(),
            },
        }
    }

    fn rows(&self) -> Vec<bulk_rename::Row> {
        bulk_rename::preview(&self.items, &self.rule(), self.include_ext, &self.names)
    }

    fn ready(&self) -> bool {
        bulk_rename::ready(&self.items, &self.names)
    }

    /// `적용`을 누를 수 있는가 — 이름 목록이 다 왔고, 막힌 줄이 없고, 바뀌는 것이 있다
    fn can_apply(&self, rows: &[bulk_rename::Row]) -> bool {
        self.ready() && bulk_rename::pairs(rows).is_some()
    }

    /// 워커가 모은 이름 목록을 거둔다
    fn poll_names(&mut self) {
        let Some(rx) = &self.names_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(names) => {
                self.names = names;
                self.names_rx = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.names_rx = None,
        }
    }
}

/// 일괄 이름 바꾸기 대화 — 앱이 하나를 들고 연다
#[derive(Default)]
pub struct BulkRenameDialog {
    open: Option<Open>,
}

impl BulkRenameDialog {
    pub fn new() -> BulkRenameDialog {
        BulkRenameDialog::default()
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// 고른 항목들로 연다 — 그 폴더들의 이름 목록은 워커가 모은다
    pub fn open(&mut self, items: Vec<Item>, ctx: &egui::Context) {
        let dirs: Vec<PathBuf> = items
            .iter()
            .filter_map(|item| item.path.parent().map(std::path::Path::to_path_buf))
            .collect();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // 받을 쪽이 이미 닫혔으면(대화를 닫았다) 보내지 못해도 무해하다
            let _ = tx.send(scan_names(&dirs));
            ctx.request_repaint();
        });
        self.open_with(items, rx);
    }

    /// 이름 목록을 줄 수신부와 함께 연다 — 시험은 워커 대신 채널에 직접 넣는다
    fn open_with(&mut self, items: Vec<Item>, names_rx: Receiver<FolderNames>) {
        let Rule::Number { start, digits, .. } = Rule::number_default() else {
            return;
        };
        self.open = Some(Open {
            items,
            mode: Mode::Replace,
            find: String::new(),
            with: String::new(),
            prefix: String::new(),
            suffix: String::new(),
            start,
            digits,
            keep_name: false,
            pattern: String::new(),
            regex_with: String::new(),
            include_ext: false,
            names: FolderNames::new(),
            names_rx: Some(names_rx),
        });
    }

    /// 지금 `적용`을 누를 수 있는가 — 버튼의 흐림과 같은 판정이다
    pub fn apply_enabled(&self) -> bool {
        self.open
            .as_ref()
            .is_some_and(|open| open.can_apply(&open.rows()))
    }

    /// 그린다 — `적용`을 누르면 셸에 걸 `(경로, 새 이름)` 목록을 돌려주고 닫힌다
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Vec<(PathBuf, String)>> {
        let open = self.open.as_mut()?;
        open.poll_names();
        if open.names_rx.is_some() {
            // 워커가 끝나면 다시 그리게 하지만, 그 사이에도 스스로 깨어나 확인한다
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let rows = open.rows();
        let ready = open.ready();
        let can_apply = open.can_apply(&rows);
        let buttons = [
            dialog::ButtonSpec::strong(crate::i18n::bulk_apply()).enabled_if(can_apply),
            dialog::ButtonSpec::plain(crate::i18n::cancel()),
        ];
        let shell = dialog::show(
            ctx,
            egui::Id::new("bulk_rename_dialog"),
            BODY_WIDTH,
            &buttons,
            |ui| draw_body(ui, open, &rows, ready),
        );
        let applied = match shell.clicked {
            Some(0) if can_apply => bulk_rename::pairs(&rows),
            _ => None,
        };
        if applied.is_some() || shell.clicked == Some(1) || shell.should_close {
            self.open = None;
        }
        applied
    }
}

fn draw_body(ui: &mut egui::Ui, open: &mut Open, rows: &[bulk_rename::Row], ready: bool) {
    ui.label(
        egui::RichText::new(crate::i18n::bulk_rename_title())
            .size(TITLE_FONT_PX)
            .color(theme::TEXT),
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        for (mode, label) in [
            (Mode::Replace, crate::i18n::bulk_mode_replace()),
            (Mode::Number, crate::i18n::bulk_mode_number()),
            (Mode::Regex, crate::i18n::bulk_mode_regex()),
        ] {
            if ui.selectable_label(open.mode == mode, label).clicked() {
                open.mode = mode;
            }
        }
    });
    ui.add_space(8.0);
    let field = egui::vec2(BODY_WIDTH / 2.0 - 8.0, widgets::FORM_FIELD_HEIGHT);
    match open.mode {
        Mode::Replace => {
            labeled_pair(
                ui,
                (crate::i18n::bulk_find(), "bulk_find", &mut open.find),
                (crate::i18n::bulk_with(), "bulk_with", &mut open.with),
                field,
            );
        }
        Mode::Number => {
            labeled_pair(
                ui,
                (crate::i18n::bulk_prefix(), "bulk_prefix", &mut open.prefix),
                (crate::i18n::bulk_suffix(), "bulk_suffix", &mut open.suffix),
                field,
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(crate::i18n::bulk_start()).color(theme::HEADER_TEXT));
                ui.add(egui::DragValue::new(&mut open.start).range(0..=999_999));
                ui.add_space(12.0);
                ui.label(egui::RichText::new(crate::i18n::bulk_digits()).color(theme::HEADER_TEXT));
                ui.add(egui::DragValue::new(&mut open.digits).range(1..=9));
                ui.add_space(12.0);
                ui.checkbox(&mut open.keep_name, crate::i18n::bulk_keep_name());
            });
        }
        Mode::Regex => {
            labeled_pair(
                ui,
                (
                    crate::i18n::bulk_pattern(),
                    "bulk_pattern",
                    &mut open.pattern,
                ),
                (
                    crate::i18n::bulk_with(),
                    "bulk_regex_with",
                    &mut open.regex_with,
                ),
                field,
            );
        }
    }
    ui.add_space(6.0);
    ui.checkbox(&mut open.include_ext, crate::i18n::bulk_include_ext());
    ui.add_space(8.0);
    if !ready {
        ui.label(egui::RichText::new(crate::i18n::bulk_scanning()).color(theme::TEXT_DIM));
        ui.add_space(4.0);
    }
    draw_table(ui, rows);
}

/// 라벨이 위에 붙은 입력칸 둘을 나란히 그린다
fn labeled_pair(
    ui: &mut egui::Ui,
    left: (&str, &str, &mut String),
    right: (&str, &str, &mut String),
    size: egui::Vec2,
) {
    ui.horizontal(|ui| {
        for (label, id, value) in [left, right] {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(label).color(theme::HEADER_TEXT));
                widgets::text_field(ui, id, value, size, true, false);
            });
        }
    });
}

/// 미리보기 표 — 현재 이름 · 새 이름(막힌 줄은 사유)
fn draw_table(ui: &mut egui::Ui, rows: &[bulk_rename::Row]) {
    let half = BODY_WIDTH / 2.0 - 8.0;
    ui.horizontal(|ui| {
        header_cell(ui, crate::i18n::bulk_col_current(), half);
        header_cell(ui, crate::i18n::bulk_col_new(), half);
    });
    egui::ScrollArea::vertical()
        .max_height(TABLE_MAX_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for row in rows {
                let current = row
                    .from
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                ui.horizontal(|ui| {
                    cell(ui, &current, half, theme::TEXT);
                    match row.block {
                        Some(block) => cell(
                            ui,
                            &format!("{} — {}", row.to, block_text(block)),
                            half,
                            theme::ERROR_TEXT,
                        ),
                        None if row.unchanged() => cell(ui, &row.to, half, theme::TEXT_DIM),
                        None => cell(ui, &row.to, half, theme::TEXT),
                    }
                });
            }
        });
}

fn header_cell(ui: &mut egui::Ui, text: &str, width: f32) {
    cell(ui, text, width, theme::HEADER_TEXT);
}

/// 폭이 고정된 한 칸 — 넘치는 글은 줄임표로 자른다
fn cell(ui: &mut egui::Ui, text: &str, width: f32, color: egui::Color32) {
    ui.add_sized(
        [width, 20.0],
        egui::Label::new(egui::RichText::new(text).color(color)).truncate(),
    );
}

fn block_text(block: Block) -> &'static str {
    match block {
        Block::Empty => crate::i18n::bulk_block_empty(),
        Block::InvalidChars => crate::i18n::bulk_block_invalid(),
        Block::Duplicate => crate::i18n::bulk_block_duplicate(),
        Block::Exists => crate::i18n::bulk_block_exists(),
        Block::BadPattern => crate::i18n::bulk_block_pattern(),
    }
}

/// 폴더들의 이름을 모은다 — **워커에서 부른다**. 읽지 못한 폴더는 빈 목록이다
/// (그 겹침은 셸이 자기 대화로 묻는다)
fn scan_names(dirs: &[PathBuf]) -> FolderNames {
    let mut names = FolderNames::new();
    for dir in dirs {
        let key = bulk_rename::folder_key(dir);
        if names.contains_key(&key) {
            continue;
        }
        let found: HashSet<String> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().to_lowercase())
                    .collect()
            })
            .unwrap_or_default();
        names.insert(key, found);
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 파일(path: &str) -> Item {
        Item {
            path: PathBuf::from(path),
            is_dir: false,
        }
    }

    fn 이름표(dir: &str, names: &[&str]) -> FolderNames {
        let mut table = FolderNames::new();
        table.insert(
            bulk_rename::folder_key(std::path::Path::new(dir)),
            names.iter().map(|name| name.to_lowercase()).collect(),
        );
        table
    }

    /// 대화를 두 프레임 그린다 — 떠 있는 영역은 첫 프레임에 자리만 잡는다
    fn 그린다(dialog: &mut BulkRenameDialog) {
        let ctx = egui::Context::default();
        for _ in 0..2 {
            let _ = ctx.run_ui(Default::default(), |ctx| {
                dialog.show(ctx);
            });
        }
    }

    #[test]
    fn 이름_목록이_오기_전에는_적용이_흐리다() {
        let mut dialog = BulkRenameDialog::new();
        let (tx, rx) = channel();
        dialog.open_with(vec![파일(r"C:\w\a.txt"), 파일(r"C:\w\b.txt")], rx);
        if let Some(open) = dialog.open.as_mut() {
            open.find = "a".to_owned();
            open.with = "z".to_owned();
        }
        그린다(&mut dialog);
        assert!(
            !dialog.apply_enabled(),
            "이름 목록이 오기 전에는 겹침을 가를 수 없다"
        );
        tx.send(이름표(r"C:\w", &["a.txt", "b.txt"]))
            .expect("보내기");
        그린다(&mut dialog);
        assert!(dialog.apply_enabled(), "목록이 오고 겹침이 없으면 열린다");
    }

    #[test]
    fn 충돌이_있으면_적용이_흐리다() {
        let mut dialog = BulkRenameDialog::new();
        let (tx, rx) = channel();
        dialog.open_with(vec![파일(r"C:\w\a.txt"), 파일(r"C:\w\b.txt")], rx);
        if let Some(open) = dialog.open.as_mut() {
            open.find = "a".to_owned();
            open.with = "b".to_owned();
        }
        tx.send(이름표(r"C:\w", &["a.txt", "b.txt"]))
            .expect("보내기");
        그린다(&mut dialog);
        assert!(dialog.is_open());
        assert!(!dialog.apply_enabled(), "a→b 는 b 가 남아 있어 겹친다");
    }

    #[test]
    fn 닫혀_있으면_아무것도_그리지_않는다() {
        let ctx = egui::Context::default();
        let mut dialog = BulkRenameDialog::new();
        let output = ctx.run_ui(Default::default(), |ctx| {
            dialog.show(ctx);
        });
        let shapes = output
            .shapes
            .iter()
            .filter(|shape| !matches!(shape.shape, egui::epaint::Shape::Noop))
            .count();
        assert_eq!(shapes, 0);
    }
}
