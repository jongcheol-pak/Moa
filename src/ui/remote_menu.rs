//! 원격 목록의 우클릭 메뉴와 그 대화들 (FR-39).
//!
//! **로컬 셸 메뉴와 공통 부품으로 묶지 않는다**(plan 비추상화 선언) — 한쪽은 OS가 그리고
//! (`IContextMenu`, 로컬 PIDL 전용 — D21) 한쪽은 우리가 그린다. 겉모습이 비슷하다는 이유로
//! 한 인터페이스에 넣으면 양쪽 제약이 서로를 갉는다.
//!
//! **실행하지 않는다** — 고른 것을 값으로 돌려주고 연결에 명령을 보내는 것은 `ExplorerApp`이다.
use crate::remote::types::RemotePath;
use crate::ui::dialog;
use crate::ui::list_common::ConflictChoice;
use crate::ui::tabs::TransferTargets;
use crate::ui::theme;
use crate::ui::widgets;
use eframe::egui;

/// 메뉴 폭 — 일반 메뉴와 같은 값 (`FileExplorer-FTP.dc.html:355` 계열)
const MENU_WIDTH: f32 = 180.0;

/// 확인 대화 제목 글꼴 크기
const TITLE_FONT_PX: f32 = 16.0;

/// 이름·권한 대화의 본문 폭 — 한 줄짜리 입력과 체크박스 세 줄이 드는 너비
const INPUT_BODY_WIDTH: f32 = 360.0;

/// 권한 대화의 8진 세 자리 입력칸 폭
const OCTAL_FIELD_WIDTH: f32 = 80.0;

/// 목록을 보이는 확인 대화의 본문 폭 — 경로가 길어 입력 대화보다 넓다
const LIST_BODY_WIDTH: f32 = 420.0;

/// 목록 미리보기에 보일 항목 수 — 그보다 많으면 말줄임표 한 줄로 줄인다
const PREVIEW_LIMIT: usize = 5;

/// 메뉴가 다룰 원격 항목 하나.
///
/// 폴더 여부와 크기까지 함께 든다 — `받기`가 폴더를 만나면 그 아래를 훑어야 하고(FR-38),
/// 큐는 크기를 알아야 진행률을 그린다. 메뉴가 뜨는 **그 순간의 선택**을 담아 두는 값이다
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteTarget {
    pub path: RemotePath,
    pub is_dir: bool,
    pub size: u64,
    /// 서버가 알려 준 권한 비트 — 권한 대화의 시작값이다. 주지 않는 서버면 `None`
    pub mode: Option<u32>,
}

/// 사용자가 원격 메뉴에서 고른 것. 실행은 앱이 한다
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteMenuAction {
    /// 고른 항목을 로컬로 받는다 (전송 큐로 들어간다)
    Download,
    /// **받기 아이콘이 붙은 로컬 탭에서 고른 항목**을 올리기 아이콘이 붙은 원격 폴더로
    /// 올린다 (FR-54).
    ///
    /// 올릴 것을 파일 대화로 고르게 하지 않는다 — 목록에서 이미 "고른 것"이 있고,
    /// 끌어다 놓기(FR-38)도 같은 짝을 쓴다. 어디서 어디로 가는지는 두 아이콘이 보여 준다
    Upload,
    /// 이름 바꾸기 대화를 연다 — **하나만 고른 때**만 뜬다(새 이름은 하나뿐이다)
    Rename,
    /// 고른 항목의 **서버 안 경로**를 클립보드에 담는다 (FR-39).
    ///
    /// 호스트·프로토콜을 붙이지 않는다 — 그러면 내부 서버 주소가 클립보드로 새 나가고,
    /// 붙여넣는 자리는 대개 그 서버에 이미 붙어 있는 FTP 클라이언트나 셸이다
    CopyPath,
    /// 새 폴더 대화를 연다
    NewFolder,
    /// 권한 변경 대화를 연다
    Chmod,
    /// 삭제 확인 대화를 연다
    Delete,
    /// 목록을 다시 읽는다
    Refresh,
}

/// 원격 목록 우클릭 메뉴를 띄운다 (FR-39).
///
/// `selected`는 지금 고른 원격 항목 수다 — 0이면 **항목이 있어야 뜻이 있는 것**이 비활성이고,
/// 둘 이상이면 `이름 바꾸기`가 비활성이다(새 이름은 하나뿐이라 여럿에 줄 수 없다).
/// `connected`가 거짓이면 서버에 닿는 것이 전부 비활성이다 (plan Edge Case)
pub fn show_remote_menu(
    ui: &mut egui::Ui,
    selected: usize,
    connected: bool,
    targets: TransferTargets,
) -> Option<RemoteMenuAction> {
    ui.set_width(MENU_WIDTH);
    let mut chosen = None;
    for row in menu_rows(selected, connected, targets) {
        // 구분선 앞뒤로 "고른 것에 하는 일"과 "이 폴더에 하는 일"이 나뉜다
        if row.separator_before {
            ui.separator();
        }
        if widgets::menu_row(ui, row.label, row.enabled) {
            chosen = Some(row.action);
        }
    }
    chosen
}

/// 메뉴 한 줄의 구성 — 그리기 전에 정해지는 것들
#[derive(Debug, Clone, PartialEq)]
pub struct MenuRow {
    pub label: &'static str,
    pub action: RemoteMenuAction,
    pub enabled: bool,
    pub separator_before: bool,
}

/// 이번에 보일 메뉴 줄들과 각각의 활성 여부 (plan Edge Case).
///
/// 끊긴 연결에서는 **서버에 닿는 줄이** 비활성이다 — 눌러도 되지 않는 것을 눌리게 두면
/// 사용자는 눌렀다가 아무 일도 안 일어나는 것을 보게 된다. 그 줄들에서는 이 판정이
/// **가장 먼저**다: `targets`가 무엇이든 연결이 끊겼으면 열리지 않는다.
///
/// **다만 모든 줄이 서버에 닿는 것은 아니다** — `경로로 복사`는 이미 손에 든 문자열을
/// 클립보드에 담을 뿐이라 연결과 무관하게 열린다(그래서 연결 판정을 줄마다 `Reach`로
/// 가른다 — 통짜로 걸면 끊긴 탭에서 경로조차 복사할 수 없다).
///
/// `받기`·`올리기`는 **대상 탭이 정해져 있을 때만** 열린다 (FR-54) — 대상이 없는데 눌리면
/// 아무 일도 일어나지 않고 사용자는 그 까닭을 알 수 없다.
/// `새 폴더`·`새로 고침`은 원격 선택이 없어도 뜻이 있다(지금 폴더가 대상이다)
pub fn menu_rows(selected: usize, connected: bool, targets: TransferTargets) -> Vec<MenuRow> {
    /// 그 줄이 요구하는 선택 개수
    #[derive(Clone, Copy)]
    enum Needs {
        /// 몇 개든 좋다 — 고른 것이 없어도 뜻이 있다
        Any,
        /// 하나 이상
        Some,
        /// **정확히 하나** — 이름 바꾸기는 새 이름이 하나뿐이라 여럿을 한 번에 다룰 수 없다
        One,
    }
    /// 그 줄이 서버에 닿는가 — 닿지 않는 줄은 연결이 끊겨도 열린다
    #[derive(Clone, Copy, PartialEq)]
    enum Reach {
        /// 서버에 명령을 보낸다
        Server,
        /// 이 앱 안에서 끝난다 (클립보드 등)
        Local,
    }
    [
        (
            crate::i18n::remote_download(),
            RemoteMenuAction::Download,
            Needs::Some,
            Reach::Server,
            false,
        ),
        (
            crate::i18n::remote_upload(),
            RemoteMenuAction::Upload,
            Needs::Any,
            Reach::Server,
            false,
        ),
        (
            crate::i18n::remote_rename(),
            RemoteMenuAction::Rename,
            Needs::One,
            Reach::Server,
            false,
        ),
        (
            crate::i18n::remote_copy_path(),
            RemoteMenuAction::CopyPath,
            Needs::Some,
            Reach::Local,
            false,
        ),
        (
            crate::i18n::remote_chmod(),
            RemoteMenuAction::Chmod,
            Needs::Some,
            Reach::Server,
            false,
        ),
        (
            crate::i18n::remote_delete(),
            RemoteMenuAction::Delete,
            Needs::Some,
            Reach::Server,
            false,
        ),
        (
            crate::i18n::remote_new_folder(),
            RemoteMenuAction::NewFolder,
            Needs::Any,
            Reach::Server,
            true,
        ),
        (
            crate::i18n::menu_refresh(),
            RemoteMenuAction::Refresh,
            Needs::Any,
            Reach::Server,
            false,
        ),
    ]
    .into_iter()
    .map(|(label, action, needs, reach, separator_before)| MenuRow {
        enabled: (reach == Reach::Local || connected)
            && match needs {
                Needs::Any => true,
                Needs::Some => selected > 0,
                Needs::One => selected == 1,
            }
            // 전송 두 줄은 갈 곳(과 올릴 것)이 있어야 뜻이 있다 (FR-54)
            && match action {
                RemoteMenuAction::Download => targets.can_download,
                RemoteMenuAction::Upload => targets.can_upload,
                _ => true,
            },
        label,
        action,
        separator_before,
    })
    .collect()
}

/// 고른 항목들의 경로를 클립보드에 담을 한 덩이로 잇는다 (FR-39).
///
/// 한 줄에 하나씩 — 탐색기의 `경로로 복사`와 같은 규칙이다. 줄바꿈을 `\r\n`으로 두는 것은
/// 붙여넣는 자리가 대개 Windows 프로그램이기 때문이다(메모장은 `\n`만으로는 줄을 나누지 않는다).
///
/// **클립보드를 건드리지 않는 순수 함수다** — 그래야 이 규칙을 시험에서 확인할 수 있다
pub fn join_paths(paths: &[RemotePath]) -> String {
    paths
        .iter()
        .map(RemotePath::as_str)
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// 메뉴가 차지할 자리 — 화면 밖으로 나가지 않게 위치를 잡는 데 쓴다.
///
/// 테두리·구분선까지 정확히 재지 않는다 — 조금 넉넉하게 잡으면 안쪽으로 더 당겨질 뿐이라
/// 잘리는 것보다 낫다
pub fn menu_size(style: &egui::Style) -> egui::Vec2 {
    // 줄 **수**만 센다 — 비활성 줄도 그려지므로 대상 유무는 높이를 바꾸지 않는다
    let 줄 = menu_rows(0, false, TransferTargets::default());
    let rows = 줄.len();
    let 구분선 = 줄.iter().filter(|row| row.separator_before).count();
    // 줄 사이 간격까지 센다 — egui가 위젯마다 `item_spacing.y`를 더한다
    let gap = style.spacing.item_spacing.y;
    let inner = egui::vec2(
        MENU_WIDTH,
        theme::MENU_ITEM_HEIGHT * rows as f32
            + SEPARATOR_HEIGHT * 구분선 as f32
            + gap * (rows + 구분선).saturating_sub(1) as f32,
    );
    inner + crate::ui::menu::menu_frame_pad(style)
}

/// 구분선 한 줄이 차지하는 높이 — egui `Separator`의 기본 `spacing`이다
/// (`ui::shell_context_menu`의 같은 값과 근거가 같다)
const SEPARATOR_HEIGHT: f32 = 6.0;

/// 원격 이름으로 쓸 수 있는가 (plan Edge Case).
///
/// `/`를 막는 이유: 원격 경로의 구분자라 이름에 들어가면 **다른 폴더를 가리키게** 된다.
/// 서버가 거부해 주기를 기대하지 않는다 — 어떤 서버는 그대로 만들어 버린다
pub fn validate_name(name: &str) -> Result<&str, &'static str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(crate::i18n::remote_error_empty());
    }
    if trimmed.contains('/') {
        return Err(crate::i18n::remote_error_slash());
    }
    Ok(trimmed)
}

/// 권한 비트 아홉 개 ↔ 8진 세 자리 (FR-39 chmod).
///
/// 화면은 체크박스 아홉 개로 보이고 서버에는 숫자로 간다 — **한 곳에서 옮겨** 두 표현이
/// 어긋나지 않게 한다
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Permissions {
    /// `[소유자, 그룹, 기타] × [읽기, 쓰기, 실행]`
    pub bits: [[bool; 3]; 3],
}

impl Permissions {
    /// POSIX 비트에서 만든다 — 아래 세 자리(0o777)만 본다
    pub fn from_mode(mode: u32) -> Permissions {
        let mut bits = [[false; 3]; 3];
        for (group, row) in bits.iter_mut().enumerate() {
            for (bit, slot) in row.iter_mut().enumerate() {
                let shift = (2 - group) * 3 + (2 - bit);
                *slot = mode & (1 << shift) != 0;
            }
        }
        Permissions { bits }
    }

    /// 서버에 보낼 8진 값
    pub fn to_mode(self) -> u32 {
        let mut mode = 0;
        for (group, row) in self.bits.iter().enumerate() {
            for (bit, on) in row.iter().enumerate() {
                if *on {
                    mode |= 1 << ((2 - group) * 3 + (2 - bit));
                }
            }
        }
        mode
    }

    /// 화면에 보이는 세 자리 — `755`
    pub fn to_octal_text(self) -> String {
        format!("{:03o}", self.to_mode())
    }

    /// 적어 넣은 세 자리를 되읽는다. 숫자가 아니거나 범위를 넘으면 `None`
    pub fn from_octal_text(text: &str) -> Option<Permissions> {
        let mode = u32::from_str_radix(text.trim(), 8).ok()?;
        (mode <= 0o777).then(|| Permissions::from_mode(mode))
    }
}

/// 대화가 이번 프레임에 낸 결론 (FR-39).
///
/// **"아직 고르지 않았다"와 "취소했다"를 구분한다** — 둘을 `None` 하나로 뭉개면 호출부가
/// 취소를 알아채지 못해 대화가 닫히지 않는다(spec 리뷰 M1이 이 결함을 짚었다)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogOutcome<T> {
    /// 아직 고르지 않았다 — 다음 프레임에도 그대로 떠 있어야 한다
    Pending,
    /// 확인했다
    Confirmed(T),
    /// 취소·Esc·바깥 클릭 — 대화를 닫는다
    Cancelled,
}

/// 이름 입력 대화 — 이름 바꾸기와 새 폴더가 같은 모양이라 함께 쓴다.
///
/// 확인을 누르면 `Confirmed(이름)`, 취소·Esc·바깥 클릭이면 `Cancelled`다
pub fn show_name_dialog(
    ctx: &egui::Context,
    title: &str,
    name: &mut String,
    error: &mut Option<String>,
) -> DialogOutcome<String> {
    let mut confirmed = None;
    let mut closed = false;
    let buttons = [
        dialog::ButtonSpec::strong(crate::i18n::remote_ok()),
        dialog::ButtonSpec::plain(crate::i18n::cancel()),
    ];
    let shell = dialog::show(
        ctx,
        egui::Id::new(("원격 이름 대화", title)),
        INPUT_BODY_WIDTH,
        &buttons,
        |ui| {
            ui.label(
                egui::RichText::new(title)
                    .size(TITLE_FONT_PX)
                    .color(theme::TEXT),
            );
            ui.add_space(10.0);
            // 앱 공통 입력 필드를 쓴다 — egui 기본 `TextEdit`은 포커스가 없을 때 테두리가
            // 없고 배경도 대화와 같아, 어디에 입력하는지 보이지 않았다(사용자 보고)
            widgets::text_field(
                ui,
                "remote_name",
                name,
                egui::vec2(ui.available_width(), widgets::FORM_FIELD_HEIGHT),
                true,
                false,
            );
            if let Some(message) = error.as_ref() {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(message).color(theme::ERROR_TEXT));
            }
        },
    );
    match shell.clicked {
        Some(0) => match validate_name(name) {
            Ok(valid) => confirmed = Some(valid.to_owned()),
            Err(message) => *error = Some(message.to_owned()),
        },
        Some(_) => closed = true,
        None => {}
    }
    if shell.should_close {
        closed = true;
    }
    if closed {
        *error = None;
        return DialogOutcome::Cancelled;
    }
    match confirmed {
        Some(name) => DialogOutcome::Confirmed(name),
        None => DialogOutcome::Pending,
    }
}

/// 권한 변경 대화 — 8진 세 자리와 체크박스 아홉 개가 서로 따라간다 (Acceptance ③)
pub fn show_chmod_dialog(
    ctx: &egui::Context,
    permissions: &mut Permissions,
    octal: &mut String,
) -> DialogOutcome<u32> {
    // 문구가 언어를 따르므로 상수가 아니라 그때그때 만든다
    let groups = [
        crate::i18n::remote_owner(),
        crate::i18n::remote_group(),
        crate::i18n::remote_others(),
    ];
    let bits = [
        crate::i18n::remote_read(),
        crate::i18n::remote_write(),
        crate::i18n::remote_execute(),
    ];
    let mut confirmed = None;
    let mut closed = false;
    let buttons = [
        dialog::ButtonSpec::strong(crate::i18n::remote_apply()),
        dialog::ButtonSpec::plain(crate::i18n::cancel()),
    ];
    let shell = dialog::show(
        ctx,
        egui::Id::new("원격 권한 변경"),
        INPUT_BODY_WIDTH,
        &buttons,
        |ui| {
            ui.label(
                egui::RichText::new(crate::i18n::remote_chmod_title())
                    .size(TITLE_FONT_PX)
                    .color(theme::TEXT),
            );
            ui.add_space(10.0);
            for (group, label) in groups.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(*label).color(theme::HEADER_TEXT));
                    for (bit, name) in bits.iter().enumerate() {
                        if ui
                            .checkbox(&mut permissions.bits[group][bit], *name)
                            .changed()
                        {
                            // 체크를 바꾸면 숫자가 따라간다
                            *octal = permissions.to_octal_text();
                        }
                    }
                });
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(crate::i18n::remote_chmod_octal())
                        .color(theme::HEADER_TEXT),
                );
                // 이름 대화와 같은 앱 공통 입력 필드다 — 테두리가 있어 포커스가 없어도
                // 입력 자리가 보인다
                if widgets::text_field(
                    ui,
                    "remote_octal",
                    octal,
                    egui::vec2(OCTAL_FIELD_WIDTH, widgets::FORM_FIELD_HEIGHT),
                    true,
                    false,
                )
                .changed()
                    && let Some(parsed) = Permissions::from_octal_text(octal)
                {
                    // 숫자를 고치면 체크가 따라간다 — 잘못 적은 동안에는 그대로 둔다
                    *permissions = parsed;
                }
            });
        },
    );
    match shell.clicked {
        Some(0) => confirmed = Some(permissions.to_mode()),
        Some(_) => closed = true,
        None => {}
    }
    if shell.should_close {
        closed = true;
    }
    if closed {
        return DialogOutcome::Cancelled;
    }
    match confirmed {
        Some(mode) => DialogOutcome::Confirmed(mode),
        None => DialogOutcome::Pending,
    }
}

/// 같은 이름 확인 대화 (FR-55).
///
/// `names`는 대상에 이미 있는 최상위 항목들이다 — `(이름, 폴더인가)`. 세 버튼은 **목록
/// 전체에 한 번에** 적용된다: 항목마다 물으면 겹치는 것이 많을 때 대화가 반복해서 뜬다.
/// 취소·Esc·바깥 클릭은 `Cancelled`이며 그때는 **아무것도 전송하지 않는다**(D6).
///
/// 삭제 확인 대화와 같은 규격(여백·버튼 크기·목록 5개 미리보기)을 쓴다 — 같은 자리에서
/// 뜨는 확인 대화가 서로 달라 보이면 사용자가 매번 새로 읽어야 한다
pub fn show_conflict_dialog(
    ctx: &egui::Context,
    names: &[(String, bool)],
) -> DialogOutcome<ConflictChoice> {
    let mut chosen = None;
    let mut closed = false;
    // 왼쪽부터 덮어쓰기·건너뛰기·취소 — 종전 배치를 그대로 옮겼다
    let buttons = [
        dialog::ButtonSpec::strong(crate::i18n::conflict_overwrite()),
        dialog::ButtonSpec::plain(crate::i18n::conflict_skip()),
        dialog::ButtonSpec::plain(crate::i18n::cancel()),
    ];
    let shell = dialog::show(
        ctx,
        egui::Id::new("같은 이름 확인"),
        LIST_BODY_WIDTH,
        &buttons,
        |ui| {
            ui.label(
                egui::RichText::new(crate::i18n::conflict_title())
                    .size(TITLE_FONT_PX)
                    .color(theme::TEXT),
            );
            ui.add_space(8.0);
            ui.label(crate::i18n::dynamic::conflict_count(names.len()));
            for (name, is_dir) in names.iter().take(PREVIEW_LIMIT) {
                let line = if *is_dir {
                    format!("{name} {}", crate::i18n::conflict_folder_mark())
                } else {
                    name.clone()
                };
                ui.label(egui::RichText::new(line).color(theme::TEXT_MUTED));
            }
            if names.len() > PREVIEW_LIMIT {
                ui.label(egui::RichText::new("…").color(theme::TEXT_MUTED));
            }
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(crate::i18n::conflict_irreversible()).color(theme::ERROR_TEXT),
            );
        },
    );
    match shell.clicked {
        Some(0) => chosen = Some(ConflictChoice::Overwrite),
        Some(1) => chosen = Some(ConflictChoice::Skip),
        Some(_) => closed = true,
        None => {}
    }
    if shell.should_close {
        closed = true;
    }
    if closed {
        return DialogOutcome::Cancelled;
    }
    match chosen {
        Some(choice) => DialogOutcome::Confirmed(choice),
        None => DialogOutcome::Pending,
    }
}

/// 고른 것에 폴더가 있는가 — 있으면 삭제 확인에 「안에 든 것까지 지워진다」를 더한다.
/// 폴더 삭제는 재귀라(`ConnCommand::RemoveTree`) 한 번의 확인으로 트리 전체가 사라진다
pub fn has_folder(targets: &[RemoteTarget]) -> bool {
    targets.iter().any(|item| item.is_dir)
}

/// 삭제 확인 대화 (Acceptance ①·Halt Forecast).
///
/// **자동으로 지우는 경로는 없다** — 메뉴에서 곧바로 삭제로 가는 길이 없고, 이 대화가
/// `Confirmed`를 돌려준 자리에서만 명령이 나간다.
///
/// **재귀 여부는 묻지 않는다** — 파일이냐 폴더냐는 목록이 이미 알고(`RemoteTarget::is_dir`)
/// 그에 맞는 명령은 앱이 고른다. 예전에는 `안에 든 것까지 지웁니다` 체크로 그것을
/// 사용자에게 물었는데, 켜도 나가는 것은 `RMD`/`rmdir`(빈 폴더 전용)이라 안에 든 것은
/// 어차피 지워지지 않았다 — 문구가 하는 일과 달랐다.
///
/// 돌려주는 값: `Confirmed(())`면 지운다. 다른 대화들과 같은 결론 타입을 쓴다
pub fn show_delete_confirm(ctx: &egui::Context, targets: &[RemoteTarget]) -> DialogOutcome<()> {
    let mut confirmed = None;
    let mut closed = false;
    let buttons = [
        dialog::ButtonSpec::strong(crate::i18n::delete()),
        dialog::ButtonSpec::plain(crate::i18n::cancel()),
    ];
    let shell = dialog::show(
        ctx,
        egui::Id::new("원격 삭제 확인"),
        LIST_BODY_WIDTH,
        &buttons,
        |ui| {
            ui.label(
                egui::RichText::new(crate::i18n::remote_delete_title())
                    .size(TITLE_FONT_PX)
                    .color(theme::TEXT),
            );
            ui.add_space(8.0);
            ui.label(crate::i18n::dynamic::remote_delete_count(targets.len()));
            for item in targets.iter().take(PREVIEW_LIMIT) {
                ui.label(egui::RichText::new(item.path.as_str()).color(theme::TEXT_MUTED));
            }
            if targets.len() > PREVIEW_LIMIT {
                ui.label(egui::RichText::new("…").color(theme::TEXT_MUTED));
            }
            ui.add_space(6.0);
            if has_folder(targets) {
                ui.label(
                    egui::RichText::new(crate::i18n::remote_delete_folder_contents())
                        .color(theme::ERROR_TEXT),
                );
            }
            ui.label(
                egui::RichText::new(crate::i18n::remote_delete_irreversible())
                    .color(theme::ERROR_TEXT),
            );
        },
    );
    match shell.clicked {
        Some(0) => confirmed = Some(()),
        Some(_) => closed = true,
        None => {}
    }
    if shell.should_close {
        closed = true;
    }
    if closed {
        return DialogOutcome::Cancelled;
    }
    match confirmed {
        Some(()) => DialogOutcome::Confirmed(()),
        None => DialogOutcome::Pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 권한_비트와_팔진_값이_서로_따라간다() {
        // Acceptance ③ — 화면은 체크박스, 서버는 숫자다. 한 곳에서 옮겨야 어긋나지 않는다
        let permissions = Permissions::from_mode(0o755);
        assert_eq!(permissions.to_mode(), 0o755);
        assert_eq!(permissions.to_octal_text(), "755");
        // 소유자 rwx / 그룹 r-x / 기타 r-x
        assert_eq!(permissions.bits[0], [true, true, true]);
        assert_eq!(permissions.bits[1], [true, false, true]);
        assert_eq!(permissions.bits[2], [true, false, true]);

        // 숫자를 고치면 체크가 따라온다
        let parsed = Permissions::from_octal_text("640").expect("8진");
        assert_eq!(parsed.bits[0], [true, true, false]);
        assert_eq!(parsed.bits[1], [true, false, false]);
        assert_eq!(parsed.bits[2], [false, false, false]);
        assert_eq!(parsed.to_mode(), 0o640);

        // 왕복해도 같다
        for mode in [0o000, 0o600, 0o644, 0o700, 0o777] {
            assert_eq!(Permissions::from_mode(mode).to_mode(), mode);
        }
    }

    #[test]
    fn 잘못_적은_팔진_값은_받아들이지_않는다() {
        // 적는 도중의 값(빈 문자열·`8`)에 체크가 흔들리면 안 된다
        assert!(Permissions::from_octal_text("").is_none());
        assert!(Permissions::from_octal_text("8").is_none());
        assert!(
            Permissions::from_octal_text("1000").is_none(),
            "0o777을 넘는다"
        );
        assert!(Permissions::from_octal_text(" 644 ").is_some());
    }

    #[test]
    fn 이름에_구분자를_넣으면_거부한다() {
        // plan Edge Case — `/`가 들어가면 다른 폴더를 가리키게 된다.
        // 사유 문구는 카탈로그가 정하므로 언어를 고정하고 원문과 견준다
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        assert_eq!(validate_name("보고서.txt"), Ok("보고서.txt"));
        assert_eq!(validate_name("  여백  "), Ok("여백"));
        assert_eq!(validate_name(""), Err("이름을 입력해 주세요."));
        assert_eq!(validate_name("   "), Err("이름을 입력해 주세요."));
        assert_eq!(validate_name("위/아래"), Err("이름에 /는 쓸 수 없습니다."));
    }

    /// 받을 곳도 올릴 곳도 정해진 상태 — 대상 판정을 시험 대상에서 빼고 볼 때 쓴다
    fn 대상_있음() -> TransferTargets {
        TransferTargets {
            can_download: true,
            can_upload: true,
            ..TransferTargets::default()
        }
    }

    #[test]
    fn 메뉴가_한_프레임을_그린다() {
        // 선택 개수·연결 유무·대상 유무 조합이 모두 패닉 없이 도는지 본다
        let ctx = egui::Context::default();
        for selected in [0, 1, 3] {
            for connected in [false, true] {
                for targets in [TransferTargets::default(), 대상_있음()] {
                    let _ = ctx.run_ui(Default::default(), |ui| {
                        egui::CentralPanel::default().show(ui, |ui| {
                            show_remote_menu(ui, selected, connected, targets);
                        });
                    });
                }
            }
        }
    }

    #[test]
    fn 끊긴_연결에서는_서버에_닿는_줄이_모두_비활성이다() {
        // plan Edge Case — 연결이 끊긴 상태 → 서버에 닿는 항목 비활성.
        // **전송 대상이 다 정해져 있어도** 연결 판정이 앞선다 (FR-54가 이 규칙을 뒤집지 않는다).
        //
        // **`경로로 복사`는 대상이 아니다** — 손에 든 문자열을 담을 뿐이라 서버에 닿지 않는다
        for selected in [0, 1, 3] {
            for targets in [TransferTargets::default(), 대상_있음()] {
                assert!(
                    menu_rows(selected, false, targets)
                        .iter()
                        .filter(|row| row.action != RemoteMenuAction::CopyPath)
                        .all(|row| !row.enabled),
                    "끊긴 연결에서 서버에 닿는 줄이 눌린 채 남았다"
                );
            }
        }
    }

    #[test]
    fn 경로로_복사는_연결이_끊겨도_열린다() {
        // 그 줄은 이미 손에 든 경로 문자열을 클립보드에 담을 뿐이라 서버에 닿지 않는다 —
        // 통짜 연결 게이트에 걸리면 끊긴 탭에서 경로조차 복사할 수 없다
        let 복사줄 = |selected, connected| {
            menu_rows(selected, connected, TransferTargets::default())
                .into_iter()
                .find(|row| row.action == RemoteMenuAction::CopyPath)
                .expect("`경로로 복사` 줄이 있어야 한다")
                .enabled
        };
        assert!(복사줄(1, false), "끊긴 연결에서도 열려야 한다");
        assert!(복사줄(3, false), "여럿을 골라도 열린다");
        assert!(복사줄(1, true), "연결된 상태에서도 물론 열린다");
        // 다만 고른 것이 없으면 담을 경로가 없다
        assert!(!복사줄(0, false), "고른 것이 없으면 담을 것이 없다");
        assert!(!복사줄(0, true), "연결돼 있어도 고른 것이 없으면 닫힌다");
    }

    #[test]
    fn 경로로_복사가_이름_바꾸기와_권한_변경_사이에_선다() {
        // 「고른 것에 하는 일」 묶음 안이며 구분선 앞이다 (plan T1 「줄의 자리」).
        // **줄이 하나 늘어 여덟이다** — 종전 일곱에 `경로로 복사`가 더해졌다
        assert_eq!(
            menu_rows(1, true, 대상_있음()).len(),
            8,
            "메뉴 줄 수가 여덟이 아니다"
        );
        let 차례: Vec<RemoteMenuAction> = menu_rows(1, true, 대상_있음())
            .into_iter()
            .map(|row| row.action)
            .collect();
        let 자리 =
            |action: RemoteMenuAction| 차례.iter().position(|a| *a == action).expect("메뉴 줄");
        assert_eq!(
            자리(RemoteMenuAction::CopyPath),
            자리(RemoteMenuAction::Rename) + 1
        );
        assert_eq!(
            자리(RemoteMenuAction::Chmod),
            자리(RemoteMenuAction::CopyPath) + 1
        );
        // 구분선은 `새 폴더` 앞에만 있다 — 새 줄이 그 앞 묶음에 들어갔다는 뜻이다
        let 복사 = menu_rows(1, true, 대상_있음())
            .into_iter()
            .find(|row| row.action == RemoteMenuAction::CopyPath)
            .expect("메뉴 줄");
        assert!(!복사.separator_before, "구분선 뒤로 밀리면 안 된다");
    }

    #[test]
    fn 여러_경로를_줄바꿈으로_잇는다() {
        // 탐색기의 `경로로 복사`와 같은 규칙 — 한 줄에 하나씩.
        // `\r\n`인 것은 붙여넣는 자리가 대개 Windows 프로그램이기 때문이다
        let 하나 = [RemotePath::new("/pub/보고서.txt")];
        assert_eq!(join_paths(&하나), "/pub/보고서.txt");

        let 여럿 = [
            RemotePath::new("/pub/a.txt"),
            RemotePath::new("/pub/하위 폴더"),
            RemotePath::new("/b.bin"),
        ];
        assert_eq!(join_paths(&여럿), "/pub/a.txt\r\n/pub/하위 폴더\r\n/b.bin");

        // 고른 것이 없으면 빈 글이다 — 메뉴 줄이 비활성이라 실제로는 오지 않는 길
        assert_eq!(join_paths(&[]), "");
    }

    #[test]
    fn 갈_곳이_없으면_전송_두_줄이_비활성이다() {
        // 대상 탭이 없는데 눌리면 아무 일도 일어나지 않고 사용자는 그 까닭을 알 수 없다 (FR-54)
        let 줄 = |targets| {
            menu_rows(1, true, targets)
                .into_iter()
                .map(|row| (row.action, row.enabled))
                .collect::<Vec<_>>()
        };
        let 없음 = 줄(TransferTargets::default());
        assert_eq!(
            없음
                .iter()
                .filter(|(_, on)| *on)
                .map(|(action, _)| action.clone())
                .collect::<Vec<_>>(),
            vec![
                RemoteMenuAction::Rename,
                RemoteMenuAction::CopyPath,
                RemoteMenuAction::Chmod,
                RemoteMenuAction::Delete,
                RemoteMenuAction::NewFolder,
                RemoteMenuAction::Refresh,
            ],
            "갈 곳이 없는데 받기·올리기가 열려 있다"
        );

        // 받을 곳만 정해지면 받기만 열린다
        let 받기만 = TransferTargets {
            can_download: true,
            ..TransferTargets::default()
        };
        let 줄들 = 줄(받기만);
        let on = |action| 줄들.iter().any(|(a, on)| *a == action && *on);
        assert!(on(RemoteMenuAction::Download));
        assert!(!on(RemoteMenuAction::Upload));
    }

    #[test]
    fn 여럿을_고르면_이름_바꾸기가_비활성이다() {
        // quality 리뷰 M1 — 여럿을 고른 채 이름을 바꾸면 첫 항목만 바뀌고 나머지는
        // 아무 말 없이 버려졌다. 새 이름은 하나뿐이므로 애초에 누를 수 없게 한다
        let enabled = |rows: &[MenuRow], action: RemoteMenuAction| {
            rows.iter()
                .find(|row| row.action == action)
                .expect("메뉴 줄")
                .enabled
        };
        let one = menu_rows(1, true, 대상_있음());
        assert!(enabled(&one, RemoteMenuAction::Rename));
        let many = menu_rows(3, true, 대상_있음());
        assert!(!enabled(&many, RemoteMenuAction::Rename));
        // 여럿에도 뜻이 있는 것은 그대로 열려 있다
        assert!(enabled(&many, RemoteMenuAction::Delete));
        assert!(enabled(&many, RemoteMenuAction::Chmod));
        assert!(enabled(&many, RemoteMenuAction::Download));
    }

    #[test]
    fn 고른_것이_없어도_할_수_있는_일은_남는다() {
        let rows = menu_rows(0, true, 대상_있음());
        let enabled: Vec<_> = rows
            .iter()
            .filter(|row| row.enabled)
            .map(|row| row.action.clone())
            .collect();
        assert_eq!(
            enabled,
            vec![
                // 올리기는 **받기 아이콘 탭의 선택**이 대상이라 원격 선택과 무관하다
                RemoteMenuAction::Upload,
                RemoteMenuAction::NewFolder,
                RemoteMenuAction::Refresh,
            ]
        );
        // 하나를 고르면 나머지도 열린다
        assert!(
            menu_rows(1, true, 대상_있음())
                .iter()
                .all(|row| row.enabled)
        );
        // 문구는 구현이 정한 신규 문구 그대로다 (디자인 미제공)
        let labels: Vec<_> = rows.iter().map(|row| row.label).collect();
        assert_eq!(
            labels,
            vec![
                "받기",
                "올리기",
                "이름 바꾸기…",
                "경로로 복사",
                "권한 변경…",
                "삭제…",
                "새 폴더…",
                "새로 고침"
            ]
        );
    }

    #[test]
    fn 폴더가_섞이면_삭제_확인에_경고를_더한다() {
        let item = |path: &str, is_dir: bool| RemoteTarget {
            path: RemotePath::new(path),
            is_dir,
            size: 0,
            mode: None,
        };
        assert!(!has_folder(&[item("/a.txt", false), item("/b.txt", false)]));
        assert!(has_folder(&[item("/a.txt", false), item("/dir", true)]));
        assert!(!has_folder(&[]));
    }
}
