//! egui(eframe/glow) UI 계층 — 화면 구성·입력 처리 전부.
//!
//! 이 모듈은 `app`(레이아웃 트리·워크스페이스·세션)·`panel`(탭 모델·히스토리·정렬)의
//! 순수 로직과 `fs`(열거·감시·아이콘·셸)를 조립해 그린다. 단방향이며 하위 모듈은 `ui`를 모른다.
pub mod about_dialog;
pub mod address_bar;
pub mod app;
pub mod app_icon;
pub mod bulk_rename_dialog;
pub mod dialog;
pub mod dock;
pub mod drag_preview;
pub mod file_list;
pub mod font_scan;
pub mod icon_tex;
pub mod license_dialog;
pub mod list_common;
pub mod list_details;
pub mod list_grid;
pub mod log_panel;
pub mod menu;
pub mod paint_guard;
pub mod palette;
pub mod panel;
pub mod queue_panel;
pub mod remote_menu;
pub mod remote_states;
pub mod session;
pub mod settings_dialog;
pub mod shell_context_menu;
pub mod shell_host;
pub mod sidebar;
pub mod site_manager;
pub mod splitter;
pub mod status_bar;
pub mod tabs;
pub mod theme;
pub mod titlebar;
pub mod toast;
pub mod tray;
pub mod tree;
pub mod view_mode;
pub mod widgets;
pub mod window_start;

#[cfg(test)]
mod tests {
    /// 검사 대상이 되는 부분만 남긴 소스 — 주석 줄과 **시험 모듈**을 걷어낸다.
    /// 시험이 적은 문자열이 얹히면 규약을 어긴 생산 코드가 가려진다
    fn 생산_코드만(source: &str) -> String {
        let 본문 = match source.find("#[cfg(test)]") {
            Some(at) => &source[..at],
            None => source,
        };
        본문
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join(
                "
",
            )
    }

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

    /// `double_clicked()`를 보면서 `triple_clicked()`를 함께 보지 않는 자리가 있는가
    fn 짝_없는_더블클릭(source: &str) -> bool {
        let mut rest = 생산_코드만(source);
        while let Some(at) = rest.find("double_clicked()") {
            let tail = &rest[at..];
            // 같은 조건식 안에 짝이 있어야 한다 — `resp.double_clicked() || resp.triple_clicked()`
            let 구간 = &tail[..tail.len().min(80)];
            if !구간.contains("triple_clicked()") {
                return true;
            }
            rest = tail["double_clicked()".len()..].to_owned();
        }
        false
    }

    #[test]
    fn 더블클릭을_보는_곳은_트리플클릭도_함께_본다() {
        // 규약: egui는 **앞선 클릭에서 0.6초**(`max_double_click_delay`의 두 배) 안에 든
        // 더블클릭을 트리플클릭으로 세고, 그때는 `double_clicked()`가 서지 않는다
        // (`input_state`의 클릭 수 계산). 메뉴 항목을 고른 직후처럼 바로 앞에 클릭이
        // 있으면 이어지는 더블클릭이 죽는다.
        // 이 앱에 트리플클릭으로 하는 일은 하나도 없으므로 둘을 늘 같이 받는다
        let ui_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        let mut sources = Vec::new();
        ui_sources(&ui_dir, &mut sources);
        assert!(!sources.is_empty(), "ui 소스를 하나도 읽지 못했다");

        // 이 파일은 규약을 설명하느라 그 문자열을 코드에 담는다
        let self_path = ui_dir.join("mod.rs");
        let mut 발견 = Vec::new();
        for path in sources {
            if path == self_path {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("소스를 읽지 못했다");
            if 짝_없는_더블클릭(&source) {
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
            "트리플클릭을 함께 보지 않는 곳(앞선 클릭 뒤의 더블클릭이 죽는다): {발견:?}"
        );
    }

    #[test]
    fn 검사기는_짝의_유무를_가른다() {
        // 검사기 자신을 시험한다 — 늘 거짓을 돌려주면 위 시험이 아무것도 보증하지 못한다
        assert!(짝_없는_더블클릭("if resp.double_clicked() {"));
        assert!(!짝_없는_더블클릭(
            "if resp.double_clicked() || resp.triple_clicked() {"
        ));
        // 주석에 적힌 것은 세지 않는다
        assert!(!짝_없는_더블클릭(
            "// `double_clicked()`가 서지 않는다"
        ));
    }
}
