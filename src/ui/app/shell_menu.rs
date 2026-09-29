//! Windows 11 모양 컨텍스트 메뉴의 상태와 배선 (FR-8).
//!
//! **`ExplorerApp`의 자식으로 둔 이유**는 이 흐름이 그 private 필드(`shell`·`shell_menu`·
//! `pending_show_more`·`file_op_tx`)를 직접 만지기 때문이다 — 자식이면 가시성을 그대로 두고
//! 나눌 수 있다(`ui::app::transfer_conflict`와 같은 판단).
//!
//! **그리기는 `ui::shell_context_menu`가 한다** — 이 모듈은 언제 열고 닫을지, 고른 것으로
//! 무엇을 할지만 정한다.
use eframe::egui;

use super::ExplorerApp;
use crate::ui::list_common::{DragItem, DropOutcome, DropTarget};
use crate::ui::menu;
use crate::ui::panel;
use crate::ui::shell_context_menu;
use crate::ui::theme;

/// **아이콘 줄이 이미 가진 셸 verb들** — 그 줄에 있으므로 아래 목록에서는 뺀다 (FR-8).
///
/// 2026-08-22 사용자 보고: 목록에 `잘라내기(T)`·`복사(C)`·`삭제(D)`가 그대로 남아 아이콘
/// 줄과 같은 일을 하는 줄이 두 벌씩 보였다. **화면 문구가 아니라 셸이 정한 식별자로 거른다**
/// — 앱 언어나 Windows 언어를 따르면 그 항목을 못 찾는다.
///
/// `paste`는 여기 없다 — 아이콘 줄에 붙여넣기 칸이 없어서다. **그 줄 자체는 2026-08-26에
/// 앱이 세우는 것으로 바뀌었고**(`AppMenuItem::Paste`) 셸 것은 [`HIDDEN_VERBS`]가 숨긴다
const ACTION_ROW_VERBS: [&str; 4] = ["cut", "copy", "delete", "rename"];

/// **아이콘 줄에도 목록에도 두지 않는 셸 verb들** (FR-8 재개정).
///
/// 위 [`ACTION_ROW_VERBS`]와 축이 다르다 — 그쪽은 *"다른 자리에 이미 있으니 중복을 뺀다"*이고
/// 여기는 *"이 앱에 두지 않기로 한 것"*이다. 둘을 한 배열로 합치면 그 이유가 섞인다.
///
/// - **공유 둘** — 2026-08-22 사용자 요청으로 뺐다. 셸이 두 벌을 준다(T1 실측):
///   `Windows.Share`가 `액세스 권한 부여`, `Windows.ModernShare`가 `공유`다. 하나만 빼면
///   나머지가 목록에 남는다
/// - **즐겨찾기 둘** — 셸의 그 항목은 **탐색기 홈에 고정**하는 것이라 이 앱의 즐겨찾기
///   (FR-56)와 다른 곳을 가리킨다. 앱 자체 항목으로 대신 세운다. 파일과 폴더의 verb가
///   다르다(T1 실측): 파일은 `pintohomefile`, 폴더는 `pintohome`
/// - **다섯 줄** — 2026-08-26 사용자 요청으로 뺐다. verb가 확인된 셋이 여기 있다(T1 실측):
///   `이전 버전 복원`=`PreviousVersions` · `보내기`=`sendto` · `시작 화면에 고정`=
///   `PinToStartScreen`. 나머지 둘(`Microsoft Defender(으)로 검사...`·`Copilot에게 질문하기`)은
///   verb를 재지 못해 [`HIDDEN_LABELS`]의 문구로만 건다
///
/// **`PinToStartScreen`은 종전에 [`STANDARD_VERBS`]에 있었다** — 표준 자리(차례 8)에서 빼
/// 여기로 옮긴 것이라, 그 표의 차례 8은 비어 있다
const HIDDEN_VERBS: [&str; 8] = [
    "Windows.Share",
    "Windows.ModernShare",
    "pintohomefile",
    "pintohome",
    "PreviousVersions",
    "sendto",
    "PinToStartScreen",
    "paste",
];

/// 숨김 판정의 **2차 폴백 라벨** — verb를 주지 않는 확장이 흔해서 둔다 (FR-8 재개정).
///
/// **`(한국어, 영어)` 짝이다.** 셸이 준 문구를 견주는 것이라 앱 언어와 무관하다 — AGENTS
/// 「화면 문구」가 막는 것은 *앱이 만든 문구*로 동작을 가르는 것이고, 여기서 보는 것은
/// *Windows가 준 문구*다. 그 밖의 표시 언어에서는 이 폴백이 걸리지 않아 항목이 그대로
/// 남는데, 사라지는 것보다 낫다.
///
/// `메뉴 사용자 지정`은 T1 실측에서 **레거시 메뉴에 오지 않는 것으로 확인됐다** — 그래도
/// 남겨 두는 것은 Windows가 그 항목을 레거시 쪽에 내려보내기 시작해도 조용히 걸리게 하려는
/// 것이다(비용이 문자열 한 짝이다)
/// verb가 확인된 셋(`이전 버전 복원`·`보내기`·`시작 화면에 고정`)도 문구를 함께 두는 것은
/// **verb를 주지 않는 확장이 같은 문구로 그 자리를 대신 채우는 경우**를 잡기 위해서다
/// ([`STANDARD_LABELS`]와 같은 근거).
///
/// `Microsoft Defender`는 접두만 적는다 — 실제 문구가 `Microsoft Defender(으)로 검사...`라
/// [`same_label`]의 「접두 + 액셀러레이터·말줄임」 규칙이 그대로 잡는다.
/// `Copilot에게 질문하기`는 **실측·화면 어디에서도 보지 못했다** — 그 줄이 오면 걸리고
/// 오지 않으면 아무 일도 하지 않는 안전장치다
const HIDDEN_LABELS: [(&str, &str); 9] = [
    ("공유", "Share"),
    ("액세스 권한 부여", "Give access to"),
    ("메뉴 사용자 지정", "Customize menu"),
    ("이전 버전 복원", "Restore previous versions"),
    ("보내기", "Send to"),
    ("시작 화면에 고정", "Pin to Start"),
    ("Microsoft Defender", "Scan with Microsoft Defender"),
    ("Copilot에게 질문하기", "Ask Copilot"),
    ("붙여넣기", "Paste"),
];

/// 선택 메뉴에서 **표준 자리에 세울 셸 verb와 그 차례** (FR-8 재개정).
///
/// 차례는 Windows 11 탐색기의 메뉴를 따른다(plan 「기준 항목 차례」). 여기 없는 것은
/// [`Slot::Extension`]으로 가 `앱 확장` 하위 메뉴에 모인다 — **숨는 것이 아니라 한 단계
/// 뒤로 간다.**
///
/// 앱이 스스로 세우는 줄(`새 탭에서 열기`·`즐겨찾기에 추가`·`붙여넣기`)의 자리는 여기 없다 —
/// 셸이 주지 않아 verb가 없기 때문이며, 그 자리는 [`AppMenuItem::order`]가 정한다
/// **차례 8은 비어 있다** — `시작 화면에 고정`이 있던 자리이며 2026-08-26에 [`HIDDEN_VERBS`]로
/// 옮겼다. 나머지를 당기지 않는 것은 이 값들이 탐색기의 차례를 그대로 옮긴 것이기 때문이다
const STANDARD_VERBS: [VerbSlot; 4] = [
    ("open", 1, egui_phosphor::regular::FOLDER_OPEN),
    ("openas", 4, egui_phosphor::regular::APP_WINDOW),
    ("copyaspath", 5, egui_phosphor::regular::CLIPBOARD_TEXT),
    ("properties", 9, egui_phosphor::regular::WRENCH),
];

/// [`STANDARD_VERBS`]의 2차 폴백 라벨 — `(한국어, 영어, 차례)`.
///
/// verb를 주지 않는 확장이 표준 항목 자리를 대신 채우는 경우를 잡는다. 근거는
/// [`HIDDEN_LABELS`]와 같다 — 셸이 준 문구를 견줄 뿐이다
const STANDARD_LABELS: [LabelSlot; 4] = [
    ("열기", "Open", 1, egui_phosphor::regular::FOLDER_OPEN),
    (
        "연결 프로그램",
        "Open with",
        4,
        egui_phosphor::regular::APP_WINDOW,
    ),
    (
        "경로로 복사",
        "Copy as path",
        5,
        egui_phosphor::regular::CLIPBOARD_TEXT,
    ),
    ("속성", "Properties", 9, egui_phosphor::regular::WRENCH),
];

/// 배경 메뉴(빈 곳 우클릭)에서 표준 자리에 세울 verb와 그 차례 (FR-8 재개정).
///
/// **선택 메뉴와 표가 다르다** — 배경에는 `열기`·`경로로 복사`가 없고 대신 `새로 만들기`가
/// 있다. 한 표로 두면 그 줄이 표준에 없어 `앱 확장` 두 단계 아래로 밀린다.
///
/// **`paste`는 여기 없다** — 2026-08-26에 앱이 세우는 줄로 바뀌었다
/// ([`shell_context_menu::AppMenuItem::Paste`]). 셸은 클립보드가 비면 그 줄 자체를 주지 않아
/// 「비었으면 흐리게」를 만들 수 없고, 그래서 셸 것은 [`HIDDEN_VERBS`]가 숨긴다.
///
/// `보기`·`정렬 기준`·`새로 고침`도 여기 없다: 탐색기 자신의 뷰 항목이라 다른 호스트에
/// 주지 않는 것을 실측으로 확인했다
const BACKGROUND_VERBS: [VerbSlot; 2] = [
    ("New", 3, egui_phosphor::regular::FILE_PLUS),
    ("properties", 4, egui_phosphor::regular::WRENCH),
];

/// [`BACKGROUND_VERBS`]의 2차 폴백 라벨 — `(한국어, 영어, 차례)`
const BACKGROUND_LABELS: [LabelSlot; 2] = [
    ("새로 만들기", "New", 3, egui_phosphor::regular::FILE_PLUS),
    ("속성", "Properties", 4, egui_phosphor::regular::WRENCH),
];

/// `(verb, 차례, 아이콘 글리프)` 표 한 줄 — 이 모듈의 표 넷이 같은 모양이라 이름을 준다.
///
/// **글리프를 차례와 같은 표에 두는 이유**는 둘이 같은 항목의 속성이기 때문이다 —
/// 표를 나누면 항목 하나가 두 곳에 걸려 한쪽만 고쳐진다 (2026-08-26 D11)
type VerbSlot = (&'static str, u8, &'static str);
/// `(한국어, 영어, 차례, 아이콘 글리프)` 폴백 표 한 줄
type LabelSlot = (&'static str, &'static str, u8, &'static str);

/// 셸이 준 줄 하나가 갈 자리 (FR-8 재개정)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Slot {
    /// 상위 목록의 그 차례에 선다 — 값이 작을수록 위다.
    ///
    /// `glyph`는 **셸이 비트맵을 주지 않았을 때** 그 자리에 그릴 아이콘이다 (2026-08-26 D1)
    Standard { order: u8, glyph: &'static str },
    /// 어디에도 두지 않는다
    Hidden,
    /// `다음으로 압축` 하위 메뉴로 모은다 (2026-08-26) — verb가 없어 라벨로만 가른다
    Compress,
    /// `압축 풀기` 하위 메뉴로 모은다 (2026-08-26) — 위와 같은 성질이다.
    ///
    /// **verb를 보지 않는다** — `STANDARD_VERBS`가 넷뿐이라 verb가 있어도 그 표에 없으면
    /// 확장으로 가므로, 「확장에 있었다」에서 verb 부재를 끌어낼 수 없다
    Extract,
    /// `앱 확장` 하위 메뉴로 모은다
    Extension,
}

/// 셸이 준 줄을 표준·숨김·확장으로 가른다 (FR-8 재개정).
///
/// **verb가 1차, 라벨이 2차다.** verb를 주지 않는 확장이 흔해(`fs::shell_menu::verb`의 doc)
/// verb만 보면 표준 항목까지 `앱 확장`으로 밀릴 수 있다. 라벨 폴백은 그 경우의 안전망이며,
/// 둘 다 빗나가면 [`Slot::Extension`]이다 — **모르는 것을 숨기지 않는다.**
///
/// `background`는 빈 곳 우클릭인지다. 그때는 표가 통째로 다른 것을 본다
/// ([`BACKGROUND_VERBS`] — 배경에는 `열기`가 없고 `새로 만들기`가 있다).
///
/// **`ShellMenu`가 아니라 문자열을 받는다** — 셸 조회(COM)를 떼어 내면 이 판정을 OS 없이
/// 시험할 수 있다
pub(super) fn classify(verb: Option<&str>, label: &str, background: bool) -> Slot {
    // 숨김이 먼저다 — 표준·확장 어느 쪽으로도 새지 않게 한다
    if verb.is_some_and(hidden_verb) || hidden_label(label) {
        return Slot::Hidden;
    }
    // **압축·해제는 표준·확장 판정보다 먼저다** — 그 줄들은 verb를 표준 표에 갖지 않아
    // 그냥 두면 `앱 확장`으로 흩어진다. 배경 메뉴에는 오지 않으므로(실측) 거기서는 보지 않는다.
    //
    // **해제를 먼저 본다** — `반디집으로 압축 풀기(B)...`는 `압축하기`를 포함하지 않아 순서가
    // 결과를 바꾸지는 않지만, 두 판정이 나란히 있으면 읽는 쪽이 순서를 확인해야 한다
    if !background && extract_label(label) {
        return Slot::Extract;
    }
    if !background && compress_label(label) {
        return Slot::Compress;
    }
    let (verbs, labels): (&[VerbSlot], &[LabelSlot]) = if background {
        (&BACKGROUND_VERBS, &BACKGROUND_LABELS)
    } else {
        (&STANDARD_VERBS, &STANDARD_LABELS)
    };
    if let Some(verb) = verb
        && let Some((_, order, glyph)) = verbs
            .iter()
            .find(|(known, ..)| known.eq_ignore_ascii_case(verb))
    {
        return Slot::Standard {
            order: *order,
            glyph,
        };
    }
    // verb가 없거나 모르는 것이면 셸이 준 문구로 한 번 더 본다
    if let Some((.., order, glyph)) = labels
        .iter()
        .find(|(ko, en, ..)| same_label(label, ko) || same_label(label, en))
    {
        return Slot::Standard {
            order: *order,
            glyph,
        };
    }
    Slot::Extension
}

/// 셸이 준 문구가 **압축 해제** 항목인가 — 압축과 같은 축이다 (2026-08-26).
///
/// 실측 문구(사용자 확장 스크린샷): `압축 풀기(T)...`(Windows 기본) · `여기에 풀기(X)` ·
/// `알아서 풀기(Z)` · `반디집으로 압축 풀기(B)...`.
///
/// **`반디집으로 열기`는 걸리지 않는다** — 여는 것이지 푸는 것이 아니다. 반디집의
/// 「폴더로 풀기」는 파일 이름이 문구 전체라(`LGUpdateRenew_Setup_…`) 규칙으로 가릴 수 없어
/// `앱 확장`에 남는다. 영어는 실측하지 못했고(이 PC가 한국어) 틀려도 그 줄이 `앱 확장`에
/// 남을 뿐이라 성립을 좌우하지 않는다
fn extract_label(label: &str) -> bool {
    let label = label.trim();
    label.contains("풀기") || label.to_ascii_lowercase().starts_with("extract")
}

/// 셸이 준 문구가 압축 항목인가 — **부분 문자열로 본다**.
///
/// 압축 항목은 verb를 주지 않고(T1 실측) **앞에 파일 이름이 붙는다**(`보기.zip으로 압축하기(Z)`).
/// 그래서 [`same_label`](전체·접두 비교)로는 잡히지 않는다.
///
/// **`압축 풀기` 계열은 걸리지 않는다** — 그쪽에는 `압축하기`가 없다. 영어는 실측하지
/// 못했고(이 PC가 한국어) 틀리면 그 줄이 `앱 확장`에 남을 뿐이라 성립을 좌우하지 않는다
fn compress_label(label: &str) -> bool {
    let label = label.trim();
    label.contains("압축하기") || label.to_ascii_lowercase().starts_with("compress to")
}

/// 셸이 준 문구가 그 이름인가 — **앞부분만 견준다**.
///
/// 셸 문구에는 액셀러레이터 자리(`열기(O)`)와 말줄임(`반디집으로 압축하기...`)이 붙어
/// 정확히 같은 경우가 드물다. `fs::shell_menu`가 `&`는 이미 떼어 주지만 괄호는 남는다.
///
/// **전체 일치와 접두 일치 둘 다 대소문자를 접는다** — 영어 문구의 표기가 Windows 판·로케일
/// 데이터에 따라 다를 수 있고(`Copy as path` ↔ `Copy As Path`), 한쪽만 접으면 접두가 붙는
/// 실제 다수 경우에서 표준 항목이 조용히 `앱 확장`으로 밀린다
fn same_label(label: &str, known: &str) -> bool {
    let label = label.trim();
    if label.eq_ignore_ascii_case(known) {
        return true;
    }
    // 접두도 같은 규칙으로 견준다 — `strip_prefix`는 바이트 정확 비교라 쓰지 않는다
    let (앞, 남은것) = label.split_at_checked(known.len()).unwrap_or((label, ""));
    // 뒤에 남는 것이 액셀러레이터·말줄임뿐이어야 한다 — `열기`가 `열기 위치`를
    // 잡아 버리면 엉뚱한 줄이 표준 자리에 선다
    앞.eq_ignore_ascii_case(known) && 남은것.trim_start().starts_with(['(', '.', '\u{2026}'])
}

/// `즐겨찾기에 추가`가 대상으로 삼을 폴더 — 없으면 그 줄이 비활성이다 (FR-8 재개정·FR-56).
///
/// **폴더 하나를 골랐을 때와 빈 곳을 우클릭했을 때만 열린다**(D2):
/// - 고른 것이 없으면(배경 메뉴) **보고 있는 폴더**가 대상이다
/// - 폴더 하나면 그 폴더
/// - 파일이거나 여럿이면 대상이 없다 — 앱 즐겨찾기는 폴더 목록이라 파일을 담을 자리가 없고,
///   여럿을 한 번에 담는 것은 탐색기에도 없는 동작이다
///
/// **이미 담긴 폴더도 대상이 없다** — 트리 메뉴의 같은 규칙이다(눌러도 아무 일이 없는 것은
/// 비활성보다 나쁘다). `already`가 그 판정이며 `FavoriteStore`가 아니라 **함수로 받는다**:
/// 저장소를 받으면 이 판정을 화면 타입 밖에서 시험할 수 없다(`favorite_action_for`와 같은 이유)
fn favorite_target<'a>(
    folder: &'a std::path::Path,
    items: &'a [std::path::PathBuf],
    dirs: &[bool],
    already: impl Fn(&std::path::Path) -> bool,
) -> Option<&'a std::path::Path> {
    // 짝은 `MenuRequest`를 만들 때 `unzip`으로 갈라진 것이라 언제나 같아야 한다 —
    // 어긋났다면 그 사이 어딘가가 한쪽만 걸러 낸 것이고, 조용히 비활성으로 끝나면
    // 그 실수가 드러나지 않는다
    debug_assert_eq!(items.len(), dirs.len(), "items/dirs 짝이 어긋났다");
    let 대상 = match items.len() {
        0 => folder,
        // **길이가 어긋나면 열지 않는다** — 짝이 맞지 않는 값으로 폴더 여부를 가릴 수 없다
        1 if dirs.first() == Some(&true) => items[0].as_path(),
        _ => return None,
    };
    (!already(대상)).then_some(대상)
}

/// `새 탭에서 열기`가 대상으로 삼을 폴더 — 없으면 **그 줄을 세우지 않는다** (FR-3·FR-8 재개정).
///
/// [`favorite_target`]과 세 가지가 다르다:
/// - **빈 곳 우클릭에는 서지 않는다** — 보고 있는 폴더를 새 탭에 여는 것은 `Ctrl+T`와 같아
///   중복이다
/// - **이미 담겼는지 같은 조건이 없다** — 같은 폴더를 여러 탭에 여는 것은 정상이다
/// - **없으면 비활성이 아니라 아예 빠진다** — 파일 메뉴에 흐린 `새 탭에서 열기`가 서 있으면
///   탐색기와 달라 보인다(탐색기는 파일 메뉴에 그 줄 자체가 없다)
///
/// 폴더 여럿은 대상이 아니다 — 탐색기는 각각 열지만 이 앱은 탭이 패널마다라 여러 탭이
/// 한꺼번에 생기면 어느 것이 활성인지 어지럽다(D3의 같은 판단)
fn new_tab_target<'a>(
    items: &'a [std::path::PathBuf],
    dirs: &[bool],
) -> Option<&'a std::path::Path> {
    debug_assert_eq!(items.len(), dirs.len(), "items/dirs 짝이 어긋났다");
    (items.len() == 1 && dirs.first() == Some(&true)).then(|| items[0].as_path())
}

/// 앱 줄에서 고른 것을 즐겨찾기 조작으로 바꾼다 — 없으면 아무 일도 하지 않는다 (FR-56).
///
/// **`ExplorerApp`을 받지 않는다** — 이 매핑을 화면 타입 안에 두면 시험으로 덮을 수 없다.
/// `app::favorites`가 *"적용 규칙을 화면 쪽에 두면 시험으로 덮을 수 없다"*며 같은 판단을
/// 이미 한 자리다.
///
/// `target`이 `None`이면 그 줄이 비활성이라 눌릴 수 없지만, 그래도 여기서 한 번 더 막는다
/// — 활성 판정과 실행이 다른 프레임에 있어 그 사이에 상태가 바뀔 수 있다
fn favorite_action_for(
    item: shell_context_menu::AppMenuItem,
    target: Option<&std::path::Path>,
) -> Option<crate::app::favorites::FavoriteAction> {
    match item {
        shell_context_menu::AppMenuItem::AddFavorite => Some(
            crate::app::favorites::FavoriteAction::Add(target?.to_path_buf()),
        ),
        // 나머지 둘은 즐겨찾기와 무관하다 — 새 탭 열기와 붙여넣기는 패널을 만진다
        shell_context_menu::AppMenuItem::OpenInNewTab | shell_context_menu::AppMenuItem::Paste => {
            None
        }
    }
}

/// 그 문구를 숨길 것인가 — [`HIDDEN_LABELS`]의 2차 폴백
fn hidden_label(label: &str) -> bool {
    HIDDEN_LABELS
        .iter()
        .any(|(ko, en)| same_label(label, ko) || same_label(label, en))
}

/// 셸이 준 목록을 **탐색기 모양의 상위 목록과 세 묶음**으로 가른다 (FR-8 재개정).
///
/// 돌려주는 것은 [`Arranged`]다 — 상위 목록과 `앱 확장`·`다음으로 압축`·`압축 풀기` 셋이다.
/// **확장 묶음만 셸이 준 원래 자리를 함께 든다** — 그 자리로 아이콘을 찾기 때문이다
/// (`MenuIcons::subset`). 압축·해제 묶음은 그 묶음만으로 캐시를 다시 올린다(D14).
///
/// 구분선은 여기서 새로 세운다 — 셸이 준 자리는 원래 구성에 맞춰진 것이라 재정렬 뒤에는
/// 뜻이 없다. 탐색기처럼 **표준 묶음 / `앱 확장` / 마지막 줄** 사이에만 긋는다.
///
/// **`ShellMenu`가 아니라 verb 조회 함수를 받는다** — 그 타입은 COM 핸들이라 시험에서
/// 세울 수 없고, 그러면 이 함수의 본체(자리 계산·재정렬·확장 0건 분기)를 검증할 길이
/// 사라진다. 조회만 떼어 내면 나머지는 순수 셈이라 그대로 시험할 수 있다
/// 업로드 하위 메뉴에서 고른 것을 **전송 요청으로 바꾼다** (FR-8·FR-38).
///
/// 조립만 하고 보내지는 않는다 — `ExplorerApp`(COM에 매여 시험에서 세울 수 없다) 없이도
/// 「어느 탭의 어느 폴더로, 무엇을」이 맞는지 확인할 수 있어야 하기 때문이다
/// (`upload_target_of`를 뽑은 것과 같은 이유).
///
/// 대상이 없거나(그 사이 탭이 닫혔다) 올릴 것이 없으면 `None` — 부르는 쪽은 아무 일도 하지 않는다
fn upload_drop_outcome(
    target: Option<&super::UploadTarget>,
    items_paths: &[std::path::PathBuf],
) -> Option<DropOutcome> {
    let target = target?;
    let items: Vec<DragItem> = items_paths
        .iter()
        .map(|path| DragItem::Local {
            path: path.clone(),
            is_dir: path.is_dir(),
        })
        .collect();
    if items.is_empty() {
        return None;
    }
    Some(DropOutcome {
        items,
        source_site: None,
        target: DropTarget::Remote {
            site: target.site,
            dir: target.dir.clone(),
        },
    })
}

fn arrange(
    items: &[crate::fs::shell_menu::ShellMenuItem],
    verb_of: impl Fn(u32) -> Option<String>,
    background: bool,
    app_rows: &[(shell_context_menu::AppMenuItem, bool)],
    // 연결된 원격 탭이 하나라도 있는가 — 없으면 `업로드` 줄이 **흐린 채로** 선다.
    // 줄 자체를 빼지 않는 이유: 있다가 없어지면 메뉴 줄 수가 달라져 사용자가 누르려던
    // 자리가 밀린다. 흐린 줄은 「지금은 보낼 곳이 없다」를 말해 준다
    upload_enabled: bool,
) -> Arranged {
    let mut 표준: Vec<(u8, shell_context_menu::ShellMenuRow)> = Vec::new();
    let mut extensions = Vec::new();
    let mut compressions = Vec::new();
    let mut extractions = Vec::new();
    for (origin, item) in items.iter().enumerate() {
        // 셸이 준 구분선은 버린다 — 자리가 바뀌므로 원래 구획이 뜻을 잃는다
        if item.separator {
            continue;
        }
        let verb = verb_of(item.id);
        match classify(verb.as_deref(), &item.label, background) {
            Slot::Hidden => {}
            Slot::Standard { order, glyph } => 표준.push((
                order,
                shell_context_menu::ShellMenuRow::Shell {
                    item: item.clone(),
                    origin,
                    glyph: Some(glyph),
                },
            )),
            Slot::Extension => extensions.push((item.clone(), origin)),
            // 압축 묶음은 **아이콘을 원래 자리로 찾지 않는다**(D14) — 그 묶음만으로 캐시를
            // 다시 올린다. 여기 담기는 것이 그 재료다
            Slot::Compress => compressions.push(item.clone()),
            // 해제 묶음도 아이콘을 원래 자리로 찾지 않는다 — 압축과 같은 처리다(D14)
            Slot::Extract => extractions.push(item.clone()),
        }
    }
    // 앱이 세우는 줄도 **같은 차례 축**에 끼운다 — 탐색기에서 `새 탭에서 열기`가 `열기`
    // 바로 아래이고 `즐겨찾기에 추가`가 `속성` 위인 것을 그대로 따른다.
    //
    // **`붙여넣기`는 빈 곳 우클릭에서만 세운다**(2026-08-26 D8-2) — 탐색기도 파일을 골랐을
    // 때는 그 줄을 보이지 않는다. 이 게이트를 `open_shell_menu`가 아니라 여기 두는 것은
    // 그쪽이 COM을 요구해 **시험에서 부를 수 없기** 때문이다
    for (item, enabled) in app_rows {
        if !background && matches!(item, shell_context_menu::AppMenuItem::Paste) {
            continue;
        }
        표준.push((
            item.order(),
            shell_context_menu::ShellMenuRow::App {
                item: *item,
                enabled: *enabled,
            },
        ));
    }
    // **차례 6은 압축과 해제가 번갈아 쓴다** — 압축 파일을 골랐으면 `압축 풀기`가 서고
    // `다음으로 압축`은 서지 않는다(2026-08-26 D3 — 사용자 선택). 서로 다른 차례를 주면
    // 파일 종류에 따라 메뉴 줄이 위아래로 움직인다.
    //
    // 해제가 없을 때 `다음으로 압축`을 **항상** 세우는 것은 Windows 기본 `Zip 파일` 줄이
    // 셸 항목과 무관하게 늘 있어(D13-3·D13-4) 하위 메뉴가 빌 일이 없기 때문이다.
    // 배경 메뉴에는 고를 것이 없어 둘 다 세우지 않는다
    // **`업로드`는 선택 메뉴에만 선다** — 빈 곳 우클릭에는 올릴 항목이 없다.
    // 차례 3은 비어 있다(1 `열기` · 2 `새 탭에서 열기` · 4 `연결 프로그램` · 5 `경로로 복사`)
    if !background {
        표준.push((
            3,
            shell_context_menu::ShellMenuRow::Virtual {
                kind: shell_context_menu::VirtualSubmenu::Upload,
                enabled: upload_enabled,
            },
        ));
    }
    if !background {
        if extractions.is_empty() {
            표준.push((
                6,
                shell_context_menu::ShellMenuRow::Virtual {
                    kind: shell_context_menu::VirtualSubmenu::Compress,
                    enabled: true,
                },
            ));
        } else {
            표준.push((
                6,
                shell_context_menu::ShellMenuRow::Virtual {
                    kind: shell_context_menu::VirtualSubmenu::Extract,
                    enabled: true,
                },
            ));
        }
    }
    // **차례가 같으면 셸이 준 순서를 지킨다** — 확장이 표준 라벨을 흉내 내 같은 자리에
    // 서는 경우가 있고(D14 폴백), 그때 둘 다 보이는 편이 하나가 사라지는 것보다 낫다.
    // `sort_by_key`는 안정 정렬이라 그 순서가 유지된다
    표준.sort_by_key(|(order, _)| *order);

    let mut rows: Vec<shell_context_menu::ShellMenuRow> = Vec::new();
    for (_, row) in 표준 {
        rows.push(row);
    }
    // **확장이 하나도 없으면 그 줄과 앞 구분선을 두지 않는다** — 빈 하위 메뉴는 고장으로
    // 보인다
    if !extensions.is_empty() {
        rows.push(shell_context_menu::ShellMenuRow::Separator);
        rows.push(shell_context_menu::ShellMenuRow::Virtual {
            kind: shell_context_menu::VirtualSubmenu::Extensions,
            enabled: true,
        });
    }
    Arranged {
        rows,
        extensions,
        compressions,
        extractions,
    }
}

/// [`arrange`]가 가른 네 묶음 — 튜플로 두면 어느 것이 무엇인지 호출부에서 `.2`가 된다.
///
/// **묶음이 셋일 때는 튜플로 충분했다** — 넷이 되면서 clippy가 복잡도를 지적했고, 이름을
/// 주는 편이 그 지적을 없애면서 읽기도 낫다
struct Arranged {
    /// 상위 목록에 그릴 줄들 — 표준 차례로 세운 셸 항목과 앱이 세운 줄이 섞여 있다
    rows: Vec<shell_context_menu::ShellMenuRow>,
    /// `앱 확장` 하위로 갈 줄들과 **셸이 준 원래 자리**(그 자리로 아이콘을 찾는다)
    extensions: Vec<(crate::fs::shell_menu::ShellMenuItem, usize)>,
    /// `다음으로 압축` 하위로 갈 셸 줄들
    compressions: Vec<crate::fs::shell_menu::ShellMenuItem>,
    /// `압축 풀기` 하위로 갈 셸 줄들
    extractions: Vec<crate::fs::shell_menu::ShellMenuItem>,
}

/// 지금 열려 있는 Win11 모양 컨텍스트 메뉴 한 판 (FR-8).
///
/// **셸 인터페이스와 그 판의 아이콘·항목을 함께 든다** — `ShellMenu`가 살아 있어야 고른 것을
/// 실행할 수 있고(`invoke`), 아이콘 텍스처는 이 판에서만 쓰는 그림이라 함께 버려야 한다.
///
/// 하위 메뉴는 **펼친 하나만** 든다 — 셸 메뉴는 두 단계를 넘지 않고, 여러 개를 동시에 펼치는
/// 것은 어느 것이 열려 있는지 화면에서 읽기 어렵다
pub(super) struct OpenShellMenu {
    menu: crate::fs::shell_menu::ShellMenu,
    /// 상위 목록에 그릴 줄들 — 표준 차례로 세운 셸 항목과 앱이 세운 줄이 섞여 있다
    rows: Vec<crate::ui::shell_context_menu::ShellMenuRow>,
    /// `앱 확장` 하위 메뉴에 모은 줄들과 **셸이 준 원래 자리** — 그 자리로 아이콘을 찾는다
    extensions: Vec<(crate::fs::shell_menu::ShellMenuItem, usize)>,
    /// `압축 풀기` 하위에 모은 셸 줄들 — 압축 묶음과 같은 처리다(D14)
    extractions: Vec<crate::fs::shell_menu::ShellMenuItem>,
    /// `다음으로 압축` 하위에 모은 셸 줄들 — **원래 자리를 들지 않는다**(D14).
    ///
    /// `ShellMenuItem`이 자기 비트맵을 이미 들고 있어 그 묶음만으로 캐시를 다시 올린다
    compressions: Vec<crate::fs::shell_menu::ShellMenuItem>,
    /// 셸이 준 원래 목록과 1:1로 정렬된 아이콘 캐시 — 찾을 때는 `origin`을 쓴다
    icons: crate::ui::shell_context_menu::MenuIcons,
    /// 펼쳐 둔 하위 메뉴 — 셸의 것과 **우리가 모은 넷**(`앱 확장`·`다음으로 압축`·`압축 풀기`·
    /// `업로드`) 중 하나다. 앞의 셋은 재료가 셸 항목이라 `Virtual`로 함께 들지만 `업로드`는
    /// 앱이 모은 글자 목록이라 변형이 따로다(`OpenSubmenu::Upload`의 doc 참조)
    submenu: Option<OpenSubmenu>,
    /// 펼쳐 둔 하위 메뉴가 붙을 **줄의 위쪽 y** (2026-08-26).
    ///
    /// 펼칠 때 저장한다 — 포인터가 하위 팝업으로 가면 부모 줄 얹힘이 끊겨 그 값을 다시
    /// 받을 수 없다. 하위 메뉴가 없을 때는 쓰이지 않는다
    submenu_top: f32,
    /// 메뉴가 뜬 자리 (논리 pt)
    pos: egui::Pos2,
    /// 이 메뉴가 대상으로 삼은 폴더와 항목들 — 마지막 줄이 그대로 다시 쓴다
    folder: std::path::PathBuf,
    items_paths: Vec<std::path::PathBuf>,
    /// 업로드 하위 메뉴에 세울 원격 탭들 (2026-08-28).
    ///
    /// **메뉴를 열 때 한 번 굳힌다** — 매 프레임 다시 모으면 탭 목록을 프레임마다 훑고,
    /// 고른 자리 번호가 그 사이 바뀐 목록을 가리킬 수 있다(즐겨찾기·클립보드와 같은 판단)
    uploads: Vec<super::UploadTarget>,
    /// `즐겨찾기에 추가`가 담을 폴더 — 그 줄이 비활성이면 `None`이다.
    ///
    /// **메뉴를 열 때 한 번 정한다** — 매 프레임 다시 재면 즐겨찾기 목록을 프레임마다 훑게 된다
    favorite: Option<std::path::PathBuf>,
    /// `새 탭에서 열기`가 열 폴더 — 대상이 없으면 그 줄 자체가 서지 않아 `None`이다
    new_tab: Option<std::path::PathBuf>,
    /// 아이콘 줄 판정에 쓰는 상태
    state: crate::ui::shell_context_menu::MenuState,
    /// **이 메뉴를 연 프레임인가** — 그 프레임의 클릭은 바깥 클릭으로 세지 않는다.
    ///
    /// 메뉴는 우클릭한 그 프레임에 열리고 곧바로 그려지는데, 그 우클릭이 아직 이번 프레임의
    /// 입력에 남아 있다. 메뉴가 커서 자리에 그대로 뜨면 클릭 지점이 메뉴 안이라 넘어가지만,
    /// **화면 끝이라 메뉴가 안쪽으로 당겨지면**(`clamp_menu_pos`) 클릭 지점이 메뉴 밖이 되어
    /// 뜨자마자 닫혔다 (2026-08-22 사용자 보고 — "다시 열면 바로 닫힘")
    just_opened: bool,
}

/// 펼쳐 둔 하위 메뉴 한 판 (FR-8 재개정).
///
/// **셸 갈래와 우리 갈래로 나뉜다**: 셸 하위 메뉴는 손잡이로 다시 접을지 가리지만, 우리가
/// 만든 묶음(`앱 확장`·`다음으로 압축`·`압축 풀기`)은 손잡이가 없다. 줄과 아이콘을 드는
/// 방식은 같아 그리는 쪽은 구분하지 않는다.
///
/// **우리 묶음 셋을 하나로 합치지 않는다** — 반복이 3회라 프로젝트의 공통화 문턱에 닿았지만,
/// 그 규칙은 *"검토하라"*이지 *"반드시 합쳐라"*가 아니다. 합치면 [`apply_shell_menu_pick`]의
/// **네 번째가 생겨 셋을 하나로 접었다** (2026-08-28) — 종류가 늘 때마다 네 자리를 각각
/// 고쳐야 하는 비용이 「어느 묶음이 열리는지 이름으로 본다」는 이득을 넘겼다. 값이 그 종류를
/// 그대로 들고 있어(`Virtual(VirtualSubmenu::Compress, ..)`) `match`는 여전히 갈린다
enum OpenSubmenu {
    /// 셸이 준 하위 메뉴 — `보내기` 같은 것
    Shell(
        crate::fs::shell_menu::SubmenuHandle,
        Vec<crate::fs::shell_menu::ShellMenuItem>,
        crate::ui::shell_context_menu::MenuIcons,
    ),
    /// **앱이 모은 묶음** — 어느 것인지는 첫 값이 든다.
    ///
    /// - `Extensions`: 표준 표에 없는 셸 항목을 모아 둔 것
    /// - `Compress`: 설치된 압축 프로그램의 항목들. **맨 앞의 `Zip 파일` 줄은 여기 없다** —
    ///   그것은 셸 메뉴로 오지 않아(D13-3) 그리는 자리가 세운다(`show_submenu`의 `zip_row`)
    /// - `Extract`: 셸이 준 해제 항목들. **맨 위 Windows 기본 `압축 풀기(T)...`도 여기 있다** —
    ///   압축과 달리 그 항목이 메뉴로 오므로 그리는 자리가 따로 세울 것이 없다
    Virtual(
        crate::ui::shell_context_menu::VirtualSubmenu,
        Vec<crate::fs::shell_menu::ShellMenuItem>,
        crate::ui::shell_context_menu::MenuIcons,
    ),
    /// `업로드` — 연결된 원격 탭들 (2026-08-28).
    ///
    /// **위 `Virtual`과 나눠 두는 것은 재료가 다르기 때문**이다 — 앞의 셋은 셸이 준
    /// `ShellMenuItem`이지만 이것은 앱이 모은 글자 목록이다. 억지로 한 변형에 넣으려면
    /// `id`가 셸 명령 번호인 척해야 하고, 그러면 고른 것이 `InvokeCommand`로 새어 나갈
    /// 길이 생긴다. 줄 자체(`ShellMenuRow::Virtual`)는 넷이 함께 쓴다
    Upload(Vec<String>),
}

impl OpenSubmenu {
    /// 그릴 줄과 아이콘 — 어느 갈래든 그리는 방식은 같다
    fn rows(
        &self,
    ) -> Option<(
        &[crate::fs::shell_menu::ShellMenuItem],
        &crate::ui::shell_context_menu::MenuIcons,
    )> {
        match self {
            OpenSubmenu::Shell(_, rows, icons) | OpenSubmenu::Virtual(_, rows, icons) => {
                Some((rows, icons))
            }
            // 셸 항목이 아니다 — 그리는 쪽이 `upload_labels`로 따로 가져간다
            OpenSubmenu::Upload(_) => None,
        }
    }
}

/// 이번 프레임에 메뉴를 닫아야 하는가 (FR-8).
///
/// **연 프레임의 클릭은 세지 않는다** — 메뉴를 연 그 클릭이 곧바로 메뉴를 닫는 것은
/// 어떤 자리에서 열든 틀린 동작이다. `Esc`도 같다(우클릭과 함께 눌릴 일이 없어 뜻은 없지만
/// 규칙을 하나로 둔다)
fn should_close(just_opened: bool, clicked_outside: bool, escape: bool) -> bool {
    !just_opened && (clicked_outside || escape)
}

/// 이번 프레임이 끝난 뒤 띄울 **종전 표준 메뉴** 한 건 (FR-8 재개정).
///
/// `기본 메뉴`를 누르면 우리 메뉴를 닫고 이 값을 세운다. 실제로 띄우는 것은
/// [`take_ready`]가 「띄울 차례」라고 할 때다
pub(super) struct PendingShowMore {
    pub folder: std::path::PathBuf,
    pub items: Vec<std::path::PathBuf>,
    pub pos: egui::Pos2,
    /// **화면에 실제로 나타나기(present)를 기다릴 프레임 수** — [`SHOW_MORE_SKIP_FRAMES`].
    ///
    /// 0이 되어야 띄운다
    pub skip_frames: u8,
}

/// 표준 메뉴를 띄우기 전에 넘길 프레임 수 — **2다**.
///
/// 우리 메뉴가 없는 화면이 **실제로 표시된 뒤**에 띄워야 두 메뉴가 겹쳐 보이지 않는다.
/// eframe은 `update()`가 **반환한 뒤** 그린 것을 화면에 올리므로, 소비 지점(`update()` 안)에서
/// 보이는 마지막 화면은 **직전 프레임**이다:
///
/// - 프레임 N — `기본 메뉴`를 눌러 이 값을 세운다. 그 프레임에는 우리 메뉴가 이미 그려졌다
/// - 프레임 N+1 — 메뉴 없이 그린다
/// - 프레임 N+2 — 띄운다. 이때 화면에 올라가 있는 것이 N+1(메뉴 없는 화면)이다
///
/// **1이면 부족하다** — 그때 표시돼 있는 것은 메뉴가 그려진 N이라 증상이 그대로다
const SHOW_MORE_SKIP_FRAMES: u8 = 2;

/// 띄울 차례면 값을 꺼내고, 아니면 한 프레임 줄인다 (FR-8 재개정).
///
/// **호출부가 이 함수만 부른다** — 필드를 직접 만지면 시험이 값 전이만 재고 **호출부의
/// 오배선은 못 잡는다**. 그 경계를 함수로 두면 둘 다 이 자리에서 막힌다
pub(super) fn take_ready(pending: &mut Option<PendingShowMore>) -> Option<PendingShowMore> {
    match pending.as_mut() {
        Some(it) if it.skip_frames > 0 => {
            it.skip_frames -= 1;
            None
        }
        _ => pending.take(),
    }
}

/// 펼치라는 신호가 가리키는 하위 메뉴 — [`already_expanded`]의 입력 (FR-8 재개정)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpandTarget {
    /// 셸이 준 하위 메뉴
    Shell(crate::fs::shell_menu::SubmenuHandle),
    /// 앱이 모은 묶음 — 어느 것인지는 값이 든다
    Virtual(crate::ui::shell_context_menu::VirtualSubmenu),
}

/// 그 하위 메뉴가 **이미 펼쳐져 있는가** (FR-8 재개정).
///
/// 참이면 부르는 쪽은 아무 일도 하지 않는다 — 마우스가 얹혀 있는 동안 펼치라는 신호가
/// **매 프레임** 오는데, 그때마다 `ShellMenu::expand`를 부르면 **매 프레임 COM 호출**이
/// 난다(`WM_INITMENUPOPUP` 전송 + 메뉴 재읽기).
///
/// **`OpenShellMenu`가 아니라 하위 메뉴 상태만 받는다** — 그 타입은 COM 핸들을 들어
/// 시험에서 세울 수 없고, 그러면 이 판정을 검증할 길이 사라진다
fn already_expanded(submenu: Option<&OpenSubmenu>, target: ExpandTarget) -> bool {
    match (submenu, target) {
        (Some(OpenSubmenu::Shell(had, ..)), ExpandTarget::Shell(want)) => *had == want,
        (Some(OpenSubmenu::Virtual(had, ..)), ExpandTarget::Virtual(want)) => *had == want,
        (
            Some(OpenSubmenu::Upload(..)),
            ExpandTarget::Virtual(crate::ui::shell_context_menu::VirtualSubmenu::Upload),
        ) => true,
        _ => false,
    }
}

/// 이번 프레임에 메뉴가 할 일 (FR-8 재개정)
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FrameOutcome {
    /// 아무 일도 하지 않는다
    Nothing,
    /// 메뉴를 닫는다
    Close,
    /// 고른 것을 수행한다
    Apply(shell_context_menu::ShellMenuPick),
}

/// 하위 팝업이 붙을 y — **펼침 신호일 때만 갱신되고 그 밖에는 지금 값을 지킨다** (2026-08-26).
///
/// **유지가 요점이다.** 포인터가 하위 팝업으로 넘어가면 부모 줄 얹힘이 끊겨 펼침 신호가
/// 아예 오지 않는데(`pick_for` — 얹힘이 없으면 `None`), 하위 메뉴는 그때도 열린 채 그려진다.
/// 그 프레임에 값을 잃으면 팝업이 부모 맨 위로 튄다.
///
/// `row_top`이 `None`인 것은 **하위 팝업이 올린 pick**이다 — 그 줄은 부모 목록에 없어 붙일
/// y가 없고, 그런 pick은 펼침이 아니라 실행이라 위치를 쓰지도 않는다
fn submenu_anchor(
    current: f32,
    pick: &shell_context_menu::ShellMenuPick,
    row_top: Option<f32>,
) -> f32 {
    let 펼침 = matches!(
        pick,
        shell_context_menu::ShellMenuPick::Expand(_)
            | shell_context_menu::ShellMenuPick::ExpandVirtual(_)
    );
    match row_top {
        Some(top) if 펼침 => top,
        _ => current,
    }
}

/// 고른 것과 닫기 신호를 한 프레임의 결론으로 바꾼다 (FR-8 재개정).
///
/// **닫기가 가장 앞선다.** 마우스가 줄에 얹혀 있으면 펼침·접기 신호가 **매 프레임** 나오는데,
/// 닫기를 「고른 것이 없을 때만」 보면 그 가지에 영영 닿지 않아 **줄 위에서 `Esc`를 눌러도
/// 메뉴가 닫히지 않는다**(2026-08-26 리뷰가 잡은 회귀).
///
/// **`pointer_in_submenu`는 접기만 막는다.** 하위 팝업으로 마우스를 옮기는 동안 부모 줄을
/// 지나며 접히면 하위 항목을 고를 수 없다. 다만 그 자리는 **하위 팝업이 올린 값도 지나므로**,
/// 종류를 가리지 않고 막으면 그 항목의 실행까지 함께 막혀 **하위 메뉴가 영영 눌리지 않는다**
pub(super) fn resolve_frame(
    picked: Option<shell_context_menu::ShellMenuPick>,
    close: bool,
    pointer_in_submenu: bool,
) -> FrameOutcome {
    if close {
        return FrameOutcome::Close;
    }
    match picked {
        Some(shell_context_menu::ShellMenuPick::CollapseSubmenu) if pointer_in_submenu => {
            FrameOutcome::Nothing
        }
        Some(pick) => FrameOutcome::Apply(pick),
        None => FrameOutcome::Nothing,
    }
}

/// 그 verb를 목록에서 뺄 것인가 — 아이콘 줄에 이미 있거나 두지 않기로 한 것 (FR-8).
///
/// **`ShellMenu`가 아니라 문자열을 받는다** — 셸 조회(COM)를 떼어 내면 두 목록을 함께 보는
/// 이 판정 자체를 시험할 수 있다. `ShellMenu`를 받으면 OS 없이는 부를 수 없어, 한쪽 목록을
/// 빠뜨려도 시험이 통과한다.
///
/// **대소문자를 접어 견준다** — verb는 셸·확장이 정한 문자열이라 표기가 보장되지 않는다
/// (`Windows.Share`가 그 예다)
fn hidden_verb(verb: &str) -> bool {
    ACTION_ROW_VERBS
        .iter()
        .chain(HIDDEN_VERBS.iter())
        .any(|known| known.eq_ignore_ascii_case(verb))
}

impl ExplorerApp {
    /// 아이콘 줄에서 고른 것을 수행한다 (FR-8·FR-64).
    ///
    /// **넷 다 앱이 자체 기능으로 한다** — 셸에 넘기는 칸이 없다. 셸의 `rename`은 탐색기
    /// 자신의 목록 뷰가 처리하는 것이라 여기서 불러도 아무 일이 없고, 잘라내기·복사도
    /// verb로 부르면 셸이 자기 클립보드 상태를 쥐어 우리 화면의 잘라내기 표시와 어긋난다.
    ///
    /// **`이름 바꾸기`는 목록의 인라인 편집을 연다** (FR-64) — 여기서 셸에 거는 것이 아니라
    /// 편집을 열어 두고, 사용자가 `Enter`로 확정한 뒤에 걸린다
    fn apply_menu_action(
        &mut self,
        action: shell_context_menu::MenuAction,
        open: &OpenShellMenu,
        owner: windows::Win32::Foundation::HWND,
    ) {
        use shell_context_menu::MenuAction;
        let targets = &open.items_paths;
        match action {
            MenuAction::Copy => {
                // 담기지 못했으면 클립보드에는 **종전 것이 그대로** 남아 있다 —
                // 그때 잘라내기 표시를 풀면 화면과 클립보드가 어긋난다
                if crate::fs::clipboard::put(targets, false) {
                    self.clear_cut_marks();
                }
            }
            MenuAction::Cut => {
                if crate::fs::clipboard::put(targets, true) {
                    self.set_cut_marks(targets);
                }
            }
            MenuAction::Delete => {
                // **휴지통으로 보낸다** — 메뉴에는 영구 삭제를 가를 자리가 없다(탐색기와 같다).
                // 곧바로 지우는 것은 `Shift+Delete`뿐이다(FR-64)
                crate::fs::file_op::delete_items(
                    targets.clone(),
                    false,
                    owner,
                    self.file_op_tx.clone(),
                    self.repaint.clone(),
                );
            }
            MenuAction::Rename => {
                // 메뉴를 연 패널이 곧 활성 패널이다 — 우클릭이 그 패널을 활성으로 만든다.
                // 고른 것이 하나일 때만 이 칸이 열리므로(`MenuState::allows`) 첫 항목이
                // 곧 그 항목이다
                if let Some(panel) = self.command_panel_mut(None) {
                    panel.begin_rename_selected();
                }
            }
        }
    }

    /// 앱이 세운 줄에서 고른 것을 수행한다 (FR-8 재개정).
    ///
    /// 셸에 넘기는 것이 하나도 없다 — 셸이 주지 않는 항목이라 이 앱의 기능으로만 잇는다.
    ///
    /// **무엇을 걸지는 [`favorite_action_for`]가 정한다** — 그 판정을 여기 두면 `ExplorerApp`
    /// 없이는 시험할 수 없다(`app::favorites`가 같은 이유로 적용 규칙을 화면 밖에 뒀다)
    fn apply_app_menu_item(
        &mut self,
        ctx: &egui::Context,
        item: shell_context_menu::AppMenuItem,
        open: &OpenShellMenu,
    ) {
        match item {
            shell_context_menu::AppMenuItem::AddFavorite => {
                // **저장은 따로 부르지 않는다** — 세션을 수집할 때 `favorites.paths()`가
                // 함께 실린다(`ui::app`의 `collect_session`)
                if let Some(action) = favorite_action_for(item, open.favorite.as_deref()) {
                    self.favorites.apply(action);
                }
            }
            shell_context_menu::AppMenuItem::Paste => {
                // **마지막으로 누른 패널에 붙여넣는다** — 아래 `새 탭에서 열기`와 같은 규칙이고
                // 우클릭이 그 패널을 활성으로 만든다. `Command::ClipboardPaste`를 거치지 않는
                // 것은 그 사이에 할 일이 없어서다
                self.clipboard_paste(None);
            }
            shell_context_menu::AppMenuItem::OpenInNewTab => {
                // 메뉴를 연 패널이 곧 활성 패널이다 — 우클릭이 그 패널을 활성으로 만든다
                // (`MenuAction::Rename`이 쓰는 같은 길)
                if let Some(path) = open.new_tab.clone()
                    && let Some(panel) = self.command_panel_mut(None)
                {
                    panel.open_local_tab(path, ctx);
                }
            }
        }
    }

    /// 우클릭 요청을 받아 Win11 모양 메뉴를 연다 (FR-8).
    ///
    /// 셸이 메뉴를 주지 못하면(COM 실패·다룰 수 없는 경로) **아무것도 열지 않는다** — 종전
    /// 경로도 그런 경우 조용히 지나갔고, 빈 메뉴를 띄우면 고장으로 보인다
    pub(super) fn open_shell_menu(&mut self, ctx: &egui::Context, request: panel::MenuRequest) {
        let Some(shell) = self.shell.as_ref() else {
            return;
        };
        // 임시 계측 (`crate::perf`) — 어느 단계가 느린지 가르기 위한 것이며 `MOA_PERF_LOG`를
        // 켜지 않으면 `Instant` 몇 개를 잡는 것 말고는 아무 일도 하지 않는다
        let t_open = std::time::Instant::now();
        let Some(menu) = shell.open_menu(&request.folder, &request.items) else {
            return;
        };
        let d_open = t_open.elapsed();
        // 셸이 준 원래 목록 — **아이콘 캐시가 이것과 1:1로 정렬된다**. 아래에서 줄을
        // 고르고 재정렬해도 그림은 이 자리(`origin`)로 찾는다
        let t_model = std::time::Instant::now();
        let items = menu.model();
        let d_model = t_model.elapsed();
        let t_icons = std::time::Instant::now();
        let icons = shell_context_menu::MenuIcons::build(ctx, &items);
        let d_icons = t_icons.elapsed();
        let background = request.items.is_empty();
        let sel_count = request.items.len();

        // 앱이 세우는 줄의 대상과 활성 여부는 **메뉴를 열 때 한 번** 정한다 — 매 프레임
        // 다시 재면 즐겨찾기 목록을 프레임마다 훑게 된다
        let favorites = &self.favorites;
        let favorite = favorite_target(
            &request.folder,
            &request.items,
            &request.dirs,
            // 이미 담긴 폴더면 그 줄이 비활성이다 — 트리 메뉴의 같은 규칙(D9)
            |path| favorites.contains(path),
        )
        .map(std::path::Path::to_path_buf);
        // **`새 탭에서 열기`는 대상이 없으면 줄 자체를 세우지 않는다** — 즐겨찾기 줄과 다른
        // 규칙이다(탐색기의 파일 메뉴에는 그 줄이 아예 없다)
        let new_tab =
            new_tab_target(&request.items, &request.dirs).map(std::path::Path::to_path_buf);
        // **클립보드는 메뉴를 열 때 한 번만 본다**(D8-1) — 매 프레임 재면 COM을 프레임마다
        // 문다. 담긴 것이 파일이 아니면 `None`이라 그 줄이 흐려진다
        let t_clip = std::time::Instant::now();
        let paste_enabled = crate::fs::clipboard::take().is_some();
        let d_clip = t_clip.elapsed();
        let mut app_rows = vec![
            (
                shell_context_menu::AppMenuItem::AddFavorite,
                favorite.is_some(),
            ),
            // 빈 곳 우클릭에서만 실제로 선다 — 그 게이트는 `arrange`가 진다
            (shell_context_menu::AppMenuItem::Paste, paste_enabled),
        ];
        if new_tab.is_some() {
            app_rows.push((shell_context_menu::AppMenuItem::OpenInNewTab, true));
        }

        // **업로드 대상도 메뉴를 열 때 한 번 모은다** (2026-08-28) — 즐겨찾기·클립보드와
        // 같은 판단이다. 빈 곳 우클릭에는 올릴 항목이 없어 그 줄을 세우지 않는다
        let uploads = if background {
            Vec::new()
        } else {
            self.upload_menu_targets()
        };
        let t_arrange = std::time::Instant::now();
        let arranged = arrange(
            &items,
            |id| menu.verb(id),
            background,
            &app_rows,
            !uploads.is_empty(),
        );
        let d_arrange = t_arrange.elapsed();
        // **경로·파일 이름은 싣지 않는다** — 이 기록은 그대로 밖으로 나갈 수 있다
        crate::perf::log(|| {
            let ms = |d: std::time::Duration| d.as_secs_f32() * 1000.0;
            format!(
                "menu sel={sel_count} rows={} | open={:.1} model={:.1} icons={:.1} clip={:.1} arrange={:.1} | total={:.1} (ms)",
                items.len(),
                ms(d_open),
                ms(d_model),
                ms(d_icons),
                ms(d_clip),
                ms(d_arrange),
                ms(d_open + d_model + d_icons + d_clip + d_arrange),
            )
        });
        self.shell_menu = Some(OpenShellMenu {
            menu,
            uploads,
            rows: arranged.rows,
            extensions: arranged.extensions,
            compressions: arranged.compressions,
            extractions: arranged.extractions,
            icons,
            submenu: None,
            submenu_top: request.pos.y,
            pos: request.pos,
            folder: request.folder,
            new_tab,
            state: shell_context_menu::MenuState {
                selected: request.items.len(),
                // 목록의 인라인 편집이 받는다 (FR-64) — 로컬 목록에서만 열리는데
                // 이 메뉴 자체가 로컬 전용이라(D21) 여기서 더 가릴 것이 없다
                can_rename: true,
            },
            items_paths: request.items,
            favorite,
            just_opened: true,
        });
    }

    /// 열려 있는 메뉴를 그리고 고른 것을 실행한다 (FR-8).
    ///
    /// 바깥을 누르거나 `Esc`면 닫는다 — 메뉴가 화면에 눌어붙지 않게 한다(원격 메뉴와 같은 규칙)
    pub(super) fn show_shell_menu(&mut self, ctx: &egui::Context) {
        let Some(open) = self.shell_menu.as_ref() else {
            return;
        };
        // **보고 있는 폴더가 바뀌면 닫는다** — 메뉴가 가리키는 곳과 화면이 어긋난 채 남으면
        // 엉뚱한 폴더의 항목을 실행하게 된다. 같은 폴더 안에서 파일이 지워지는 경우는 닫지
        // 않는다 — 그때는 셸이 실행 시점에 자기 대화로 알린다(종전과 같은 규칙)
        let 보고_있는_폴더 = self
            .views
            .get(&self.workspaces.active().id)
            .and_then(|view| view.active_dir());
        if 보고_있는_폴더.is_some_and(|dir| dir != open.folder) {
            self.shell_menu = None;
            return;
        }
        let viewport = ctx.input(|input| input.viewport_rect());
        let size = shell_context_menu::menu_size_at(ctx, &open.rows);
        let at = menu::clamp_menu_pos(viewport, open.pos, size);
        // 아래로 뻗을 수 있는 만큼이 목록이 쓸 수 있는 최대 높이다
        let max_height = (viewport.bottom() - at.y).max(theme::MENU_ITEM_HEIGHT);

        let (picked, rect) = shell_context_menu::show_popup(
            ctx,
            egui::Id::new("shell_context_menu"),
            at,
            open.state,
            &open.rows,
            &open.icons,
            max_height,
        );

        // **여기서 y를 갈라낸다** — `resolve_frame`·`FrameOutcome`은 「닫을지 실행할지」를
        // 뜻하는 타입이라 좌표를 실으면 그 뜻이 흐려진다. 하위 팝업이 올린 pick에는 붙일
        // y가 없으므로(그 줄은 부모 목록에 없다) **병합보다 앞**이어야 한다 — 그 경우
        // `row_top`은 부모에서 온 값이 남지만, 하위 항목 실행은 y를 쓰지 않아 무해하다
        let row_top = picked.as_ref().map(|(_, top)| *top);
        let mut picked = picked.map(|(pick, _)| pick);

        // 펼쳐 둔 하위 메뉴는 부모 오른쪽에 붙인다 — 셸의 것이든 `앱 확장`이든 같다.
        // 화면 오른쪽을 넘으면 부모 왼쪽으로 뒤집는다(`menu::submenu_pos`)
        let parent_x = egui::Rangef::new(at.x, at.x + size.x);
        let mut submenu_rect = None;
        // **업로드 하위 메뉴는 재료가 달라 따로 그린다** — 셸 항목이 아니라 앱이 모은 글자다
        if let Some(OpenSubmenu::Upload(labels)) = open.submenu.as_ref() {
            let sub_at = menu::submenu_pos(
                viewport,
                // 셸 하위 메뉴와 **같은 자리 계산**이다 — 부모 오른쪽(넘치면 왼쪽), 펼친 줄의 높이
                parent_x,
                open.submenu_top,
                shell_context_menu::upload_submenu_size(ctx, labels.len()),
            );
            let (sub_pick, rect) = shell_context_menu::show_upload_submenu_popup(
                ctx,
                egui::Id::new("셸 메뉴 업로드 하위"),
                sub_at,
                labels,
            );
            submenu_rect = Some(rect);
            if sub_pick.is_some() {
                picked = sub_pick;
            }
        } else if let Some((rows, icons)) = open.submenu.as_ref().and_then(OpenSubmenu::rows) {
            // **`Zip 파일` 줄은 압축 묶음에만 선다** — 그리기와 크기 셈이 같은 값을 봐야
            // 높이가 어긋나지 않으므로 한 번만 재서 둘에 넘긴다
            let zip_row = matches!(
                open.submenu,
                Some(OpenSubmenu::Virtual(
                    shell_context_menu::VirtualSubmenu::Compress,
                    ..
                ))
            );
            let sub_at = menu::submenu_pos(
                viewport,
                parent_x,
                // **펼친 줄의 높이에 붙인다**(2026-08-26) — 종전에는 `at.y`(부모 맨 위)라
                // 어느 줄을 펼쳤든 같은 자리에 떴다. 화면 아래로 넘치면 위로 민다
                open.submenu_top,
                shell_context_menu::submenu_size_at(ctx, zip_row, rows),
            );
            let (sub_picked, sub_rect) = shell_context_menu::show_submenu_popup(
                ctx,
                egui::Id::new("shell_context_submenu"),
                sub_at,
                zip_row,
                rows,
                icons,
            );
            if sub_picked.is_some() {
                picked = sub_picked;
            }
            submenu_rect = Some(sub_rect);
        }

        let inside = |pos: egui::Pos2| {
            rect.contains(pos) || submenu_rect.is_some_and(|sub| sub.contains(pos))
        };
        let outside = ctx.input(|input| {
            input.pointer.any_click() && input.pointer.interact_pos().is_none_or(|pos| !inside(pos))
        });
        let escape = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        // 연 프레임 표시는 그렸으니 곧바로 내린다 — 다음 프레임부터는 보통 규칙이다
        let just_opened = self
            .shell_menu
            .as_mut()
            .is_some_and(|open| std::mem::take(&mut open.just_opened));

        // 하위 팝업 위에 마우스가 있는가 — 접기를 막는 판정에 쓴다(`resolve_frame`)
        let pointer_in_submenu = ctx.input(|input| {
            input
                .pointer
                .hover_pos()
                .is_some_and(|pos| submenu_rect.is_some_and(|sub| sub.contains(pos)))
        });
        match resolve_frame(
            picked,
            should_close(just_opened, outside, escape),
            pointer_in_submenu,
        ) {
            FrameOutcome::Close => self.shell_menu = None,
            FrameOutcome::Apply(pick) => self.apply_shell_menu_pick(ctx, pick, row_top),
            FrameOutcome::Nothing => {}
        }
    }

    /// 메뉴에서 고른 것을 수행한다 (FR-8).
    ///
    /// **하위 메뉴를 여닫는 것만 메뉴를 열어 둔다**(`Expand`·`ExpandVirtual`·
    /// `CollapseSubmenu`) — 나머지는 무엇을 하든 메뉴를
    /// 먼저 닫는다.
    /// 셸 항목은 새 창을 띄우기도 해서, 닫지 않으면 그 창 뒤에 메뉴가 남는다
    fn apply_shell_menu_pick(
        &mut self,
        ctx: &egui::Context,
        pick: shell_context_menu::ShellMenuPick,
        row_top: Option<f32>,
    ) {
        let Some(open) = self.shell_menu.as_mut() else {
            return;
        };
        // **펼침 신호를 받았으면 최신 y를 남긴다** — `already_expanded` 조기 반환보다 앞이다
        open.submenu_top = submenu_anchor(open.submenu_top, &pick, row_top);
        match pick {
            shell_context_menu::ShellMenuPick::Expand(handle) => {
                // **이미 펼쳐 둔 것이면 아무 일도 하지 않는다** — 마우스가 얹혀 있는 동안
                // 이 신호가 매 프레임 오는데, 그때마다 `expand`를 부르면 **매 프레임 COM
                // 호출**이 난다(`WM_INITMENUPOPUP` 전송 + 메뉴 재읽기).
                //
                // 종전에는 여기서 접었다(토글) — 마우스로 펼치는 지금은 뜻이 없다.
                // 얹혀 있는 동안 매 프레임 뒤집혀 깜빡인다
                if already_expanded(open.submenu.as_ref(), ExpandTarget::Shell(handle)) {
                    return;
                }
                let rows = open.menu.expand(handle);
                let icons = shell_context_menu::MenuIcons::build(ctx, &rows);
                open.submenu = Some(OpenSubmenu::Shell(handle, rows, icons));
            }
            shell_context_menu::ShellMenuPick::CollapseSubmenu => {
                // 하위 메뉴 없는 줄에 마우스가 얹혔다 — 펼쳐 둔 것을 접는다.
                // 이미 접혀 있으면 그대로다(멱등)
                open.submenu = None;
            }
            shell_context_menu::ShellMenuPick::ExpandVirtual(kind) => {
                // 위 `Expand`와 같은 이유로 **이미 펼쳤으면 아무 일도 하지 않는다**
                if already_expanded(open.submenu.as_ref(), ExpandTarget::Virtual(kind)) {
                    return;
                }
                // **재료를 얻는 길이 묶음마다 다르다** — 그 차이가 이 갈래의 전부다
                let (rows, icons) = match kind {
                    shell_context_menu::VirtualSubmenu::Extensions => {
                        // 아이콘은 **셸이 준 원래 자리**로 뽑는다 — 하위 메뉴에서는 줄 번호가
                        // 0부터 다시 매겨져, 부모 캐시를 그대로 넘기면 그림이 어긋난다
                        let (rows, origins): (Vec<_>, Vec<_>) =
                            open.extensions.iter().cloned().unzip();
                        let icons = open.icons.subset(&origins);
                        (rows, icons)
                    }
                    shell_context_menu::VirtualSubmenu::Compress => {
                        // **그 묶음만으로 캐시를 다시 올린다**(D14) — 재료가 상위 목록에서만
                        // 오지만 맨 앞 `Zip 파일` 줄은 셸 것이 아니라, `subset`의 원래 자리
                        // 축과 맞지 않는다
                        let rows = open.compressions.clone();
                        let icons = shell_context_menu::MenuIcons::build(ctx, &rows);
                        (rows, icons)
                    }
                    shell_context_menu::VirtualSubmenu::Extract => {
                        // 압축 묶음과 같이 **그 묶음만으로 캐시를 다시 올린다**(D14)
                        let rows = open.extractions.clone();
                        let icons = shell_context_menu::MenuIcons::build(ctx, &rows);
                        (rows, icons)
                    }
                    shell_context_menu::VirtualSubmenu::Upload => {
                        // **재료가 셸이 아니라 앱의 탭 목록이다** — 메뉴를 여는 순간 굳혀 둔
                        // 것을 그대로 쓴다(그 사이 탭이 닫히면 자리 번호가 어긋난다)
                        open.submenu = Some(OpenSubmenu::Upload(
                            open.uploads.iter().map(|t| t.label.clone()).collect(),
                        ));
                        return;
                    }
                };
                open.submenu = Some(OpenSubmenu::Virtual(kind, rows, icons));
            }
            shell_context_menu::ShellMenuPick::UploadTo(index) => {
                // **먼저 닫는다** — 셸 항목·앱 줄과 같은 순서다(전송은 대화를 띄울 수 있다)
                let Some(open) = self.shell_menu.take() else {
                    return;
                };
                let Some(drop) = upload_drop_outcome(open.uploads.get(index), &open.items_paths)
                else {
                    return;
                };
                // **끌어다 놓기와 같은 앞문으로 보낸다** (FR-38) — 폴더를 훑는 것도, 같은 이름
                // 확인(FR-55)도, 큐에 넣는 것도 이미 그쪽에 있다. 여기서 따로 하면 두 길이
                // 곧 어긋난다(원격 메뉴의 `올리기`가 같은 판단을 한다)
                self.start_transfer(drop);
            }
            shell_context_menu::ShellMenuPick::CompressZip => {
                // **닫고 나서 실행한다** — 셸이 진행률 창을 띄우므로 `Command`와 같은 순서다
                let Some(open) = self.shell_menu.take() else {
                    return;
                };
                crate::fs::zip_shell::compress_to_zip(&open.items_paths);
            }
            shell_context_menu::ShellMenuPick::App(item) => {
                // 앱이 세운 줄도 **먼저 닫는다** — 셸 항목과 같은 순서로 둔다
                let Some(open) = self.shell_menu.take() else {
                    return;
                };
                self.apply_app_menu_item(ctx, item, &open);
            }
            shell_context_menu::ShellMenuPick::Command(id) => {
                let owner = self.owner_hwnd();
                // **닫고 나서 실행한다** — 셸 확장의 `InvokeCommand`는 새 창을 띄우거나 자기
                // 메시지 펌프를 돌기도 해서, 그 사이에 다시 그려지면 이미 고른 메뉴가 화면에
                // 남는다. 나머지 두 분기(`ShowMore`·`Action`)도 같은 순서다.
                // 메뉴를 지우기 전에 인터페이스를 옮겨 잡는다 — 실행은 그것이 살아 있어야 한다
                let Some(open) = self.shell_menu.take() else {
                    return;
                };
                open.menu.invoke(id, owner);
            }
            shell_context_menu::ShellMenuPick::ShowMore => {
                // 우리 메뉴를 먼저 닫고, 표준 메뉴는 그리기가 끝난 뒤에 띄운다.
                // 같은 값을 옮겨 담는 것이라 `take`가 준 것을 그대로 쓴다
                if let Some(open) = self.shell_menu.take() {
                    self.pending_show_more = Some(PendingShowMore {
                        folder: open.folder,
                        items: open.items_paths,
                        pos: open.pos,
                        skip_frames: SHOW_MORE_SKIP_FRAMES,
                    });
                }
            }
            shell_context_menu::ShellMenuPick::Action(action) => {
                let owner = self.owner_hwnd();
                // 무엇을 하든 메뉴를 **먼저 닫는다** — 셸 대화가 뜨는 갈래가 있어서다
                let Some(open) = self.shell_menu.take() else {
                    return;
                };
                self.apply_menu_action(action, &open, owner);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 이 줄이 표준 자리인가와 그 차례 — **글리프는 보지 않는다**.
    ///
    /// 차례를 재는 시험이 열다섯 곳이라, 그 전부가 글리프까지 적으면 관심사가 섞인다.
    /// 글리프는 [`표준_자리에는_아이콘_글리프가_붙는다`] 하나가 전건을 본다
    fn 차례(slot: Slot) -> Option<u8> {
        match slot {
            Slot::Standard { order, .. } => Some(order),
            _ => None,
        }
    }

    #[test]
    fn 메뉴를_연_클릭은_그_메뉴를_닫지_못한다() {
        // 2026-08-22 사용자 보고 — "메뉴가 한번 열리고 다시 열면 바로 닫히는 문제".
        // 메뉴는 우클릭한 그 프레임에 열리고 곧바로 그려지는데, 그 우클릭이 아직 이번
        // 프레임의 입력에 남아 있다. 커서 자리에 그대로 뜨면 클릭 지점이 메뉴 안이라
        // 넘어가지만, 화면 끝이라 안쪽으로 당겨지면 밖이 되어 뜨자마자 닫혔다
        assert!(!should_close(true, true, false), "연 프레임의 클릭");
        assert!(!should_close(true, false, true), "연 프레임의 Esc도 같다");

        // 다음 프레임부터는 보통 규칙이다
        assert!(should_close(false, true, false), "바깥 클릭");
        assert!(should_close(false, false, true), "Esc");
        assert!(
            !should_close(false, false, false),
            "아무 일도 없으면 열어 둔다"
        );
    }

    #[test]
    fn 표준_메뉴는_우리_메뉴가_사라진_화면이_나온_뒤에_뜬다() {
        // **그리기가 끝난 것과 화면에 표시된 것은 다르다** — eframe은 `update()`가 반환한
        // 뒤에 그린 것을 올린다. 소비 지점이 그 안이라, 세운 프레임에 바로 띄우면 화면에는
        // 아직 우리 메뉴가 그려진 그 프레임이 올라가 있어 두 메뉴가 겹쳐 보인다.
        //
        // 한 프레임만 미뤄도 같다 — 그때 표시된 것이 그 프레임이기 때문이다
        let mut 대기 = Some(PendingShowMore {
            folder: std::path::PathBuf::from(r"D:\작업"),
            items: Vec::new(),
            pos: egui::Pos2::ZERO,
            skip_frames: SHOW_MORE_SKIP_FRAMES,
        });

        // 프레임 N — 세운 그 프레임. 아직 띄우지 않는다
        assert!(take_ready(&mut 대기).is_none(), "세운 프레임");
        assert!(대기.is_some(), "값은 남아 있어야 한다");
        // 프레임 N+1 — 우리 메뉴 없이 그리는 프레임. 아직이다
        assert!(take_ready(&mut 대기).is_none(), "메뉴 없이 그리는 프레임");
        assert!(대기.is_some());
        // 프레임 N+2 — 이제 띄운다. 화면에 올라가 있는 것은 N+1(메뉴 없는 화면)이다
        let 꺼낸것 = take_ready(&mut 대기).expect("셋째 프레임에 띄운다");
        assert_eq!(꺼낸것.folder, std::path::PathBuf::from(r"D:\작업"));
        assert!(대기.is_none(), "꺼냈으면 비어야 한다 — 두 번 뜨지 않는다");

        // 빈 채로 더 불러도 아무 일이 없다
        assert!(take_ready(&mut 대기).is_none());
    }

    #[test]
    fn 이미_펼친_하위_메뉴는_다시_펼치지_않는다() {
        // **마우스가 얹혀 있는 동안 펼치라는 신호가 매 프레임 온다** — 그때마다
        // `ShellMenu::expand`를 부르면 매 프레임 COM 호출이 난다(`WM_INITMENUPOPUP`
        // 전송 + 메뉴 재읽기). 이 판정이 그 재호출을 막는다
        use crate::fs::shell_menu::SubmenuHandle;
        let 손잡이 = SubmenuHandle::for_test(7, 2);
        let 다른_손잡이 = SubmenuHandle::for_test(9, 0);
        let 셸 = OpenSubmenu::Shell(
            손잡이,
            Vec::new(),
            shell_context_menu::MenuIcons::for_test(),
        );
        let 확장 = OpenSubmenu::Virtual(
            shell_context_menu::VirtualSubmenu::Extensions,
            Vec::new(),
            shell_context_menu::MenuIcons::for_test(),
        );

        assert!(
            already_expanded(Some(&셸), ExpandTarget::Shell(손잡이)),
            "같은 것을 다시 펼치라는 신호"
        );
        assert!(
            !already_expanded(Some(&셸), ExpandTarget::Shell(다른_손잡이)),
            "다른 하위 메뉴면 새로 펼친다"
        );
        assert!(
            !already_expanded(
                Some(&셸),
                ExpandTarget::Virtual(shell_context_menu::VirtualSubmenu::Extensions)
            ),
            "갈래가 다르면 새로 펼친다"
        );
        assert!(already_expanded(
            Some(&확장),
            ExpandTarget::Virtual(shell_context_menu::VirtualSubmenu::Extensions)
        ));
        assert!(!already_expanded(Some(&확장), ExpandTarget::Shell(손잡이)));
        // 아무것도 펼쳐 있지 않으면 언제나 새로 펼친다
        assert!(!already_expanded(
            None,
            ExpandTarget::Virtual(shell_context_menu::VirtualSubmenu::Extensions)
        ));
        assert!(!already_expanded(None, ExpandTarget::Shell(손잡이)));
    }

    #[test]
    fn 줄_위에_마우스가_있어도_esc가_메뉴를_닫는다() {
        // **2026-08-26 리뷰가 잡은 회귀** — 마우스를 올리면 펼치는 규칙 때문에 펼침·접기
        // 신호가 매 프레임 나오는데, 닫기를 「고른 것이 없을 때만」 보면 그 가지에 영영
        // 닿지 않아 줄 위에서는 `Esc`가 먹지 않는다
        let 접기 = Some(shell_context_menu::ShellMenuPick::CollapseSubmenu);
        assert_eq!(
            resolve_frame(접기.clone(), true, false),
            FrameOutcome::Close,
            "고른 것이 있어도 닫기가 먼저다"
        );
        // 펼침 신호가 나오는 중에도 같다
        let 펼침 = Some(shell_context_menu::ShellMenuPick::ExpandVirtual(
            shell_context_menu::VirtualSubmenu::Extensions,
        ));
        assert_eq!(resolve_frame(펼침, true, false), FrameOutcome::Close);
        // 하위 팝업 위에 마우스가 있어도 닫기는 그대로 먹는다
        assert_eq!(resolve_frame(접기, true, true), FrameOutcome::Close);
    }

    #[test]
    fn 하위_팝업_위에서는_접지_않지만_실행은_막지_않는다() {
        // 팝업으로 마우스를 옮기는 동안 부모 줄을 지나며 접히면 하위 항목을 고를 수 없다.
        // **그러나 그 자리는 하위 팝업이 올린 값도 지난다** — 종류를 가리지 않고 막으면
        // 그 항목의 실행까지 함께 막혀 하위 메뉴가 영영 눌리지 않는다
        let 접기 = Some(shell_context_menu::ShellMenuPick::CollapseSubmenu);
        assert_eq!(
            resolve_frame(접기.clone(), false, true),
            FrameOutcome::Nothing,
            "팝업 위에서는 접지 않는다"
        );
        assert_eq!(
            resolve_frame(접기, false, false),
            FrameOutcome::Apply(shell_context_menu::ShellMenuPick::CollapseSubmenu),
            "팝업 밖이면 접는다"
        );
        // **하위 항목의 실행은 팝업 위에서도 통과한다**
        let 실행 = shell_context_menu::ShellMenuPick::Command(42);
        assert_eq!(
            resolve_frame(Some(실행.clone()), false, true),
            FrameOutcome::Apply(실행)
        );
    }

    #[test]
    fn 아무_신호도_없으면_아무_일도_하지_않는다() {
        assert_eq!(resolve_frame(None, false, false), FrameOutcome::Nothing);
        assert_eq!(resolve_frame(None, false, true), FrameOutcome::Nothing);
    }

    #[test]
    fn 아이콘_줄이_가진_것은_목록에서_뺀다() {
        // 2026-08-22 사용자 보고 — 목록에 `잘라내기(T)`·`복사(C)`·`삭제(D)`가 그대로 남아
        // 아이콘 줄과 같은 일을 하는 줄이 두 벌씩 보였다.
        // **화면 문구가 아니라 셸 verb로 거른다** — 언어를 따르면 그 항목을 못 찾는다
        assert_eq!(ACTION_ROW_VERBS, ["cut", "copy", "delete", "rename"]);
        // 붙여넣기는 아이콘 줄에 칸이 없다 — **진입점은 2026-08-26에 앱이 세우는 줄로
        // 옮겼다**(`AppMenuItem::Paste`). 셸의 그 항목은 `HIDDEN_VERBS`가 숨긴다
        assert!(!ACTION_ROW_VERBS.contains(&"paste"));
    }

    #[test]
    fn 공유는_아이콘_줄에서도_목록에서도_사라진다() {
        // 아이콘 줄에서 빼기만 하면 **셸이 준 그 항목이 목록에 되살아난다** —
        // 같은 회차에서 숨김 목록으로 옮겨야 둘 다 사라진다 (2026-08-22 사용자 요청).
        // 셸은 공유를 두 벌 준다(T1 실측): `액세스 권한 부여`와 `공유`
        for verb in ["Windows.Share", "Windows.ModernShare"] {
            assert!(hidden_verb(verb), "{verb}");
            assert!(!ACTION_ROW_VERBS.contains(&verb), "아이콘 줄에는 없다");
        }
    }

    #[test]
    fn 셸_즐겨찾기는_숨긴다() {
        // 셸의 그 항목은 **탐색기 홈**에 고정하는 것이라 이 앱의 즐겨찾기와 다른 곳을
        // 가리킨다. 파일과 폴더의 verb가 다르다(T1 실측)
        assert!(hidden_verb("pintohomefile"), "파일");
        assert!(hidden_verb("pintohome"), "폴더");
    }

    #[test]
    fn 숨김_판정은_두_목록을_함께_본다() {
        // `hidden_verb`가 한쪽 목록만 보면 그 목록의 verb는 그대로 새어 나온다 —
        // 두 목록에서 하나씩 골라 함께 단언해 그 누락을 잡는다
        assert!(hidden_verb("cut"), "아이콘 줄 쪽");
        assert!(hidden_verb("Windows.ModernShare"), "숨김 쪽");
        // 표기가 달라도 걸린다 — verb는 셸·확장이 정한 문자열이라 대소문자가 보장되지 않는다
        assert!(hidden_verb("WINDOWS.MODERNSHARE"));
        // 셸의 `붙여넣기`도 숨긴다 — 앱이 세우는 줄로 바뀌었다(2026-08-26)
        assert!(hidden_verb("paste"));
        // 모르는 것은 그대로 둔다 — 거르는 것은 아는 것뿐이다
        assert!(!hidden_verb("open"));
    }

    #[test]
    fn 표준_verb는_그_차례에_선다() {
        assert_eq!(차례(classify(Some("open"), "열기(O)", false)), Some(1));
        assert_eq!(
            차례(classify(Some("copyaspath"), "경로로 복사(A)", false)),
            Some(5)
        );
        assert_eq!(
            차례(classify(Some("properties"), "속성(R)", false)),
            Some(9)
        );
    }

    #[test]
    fn 숨김_verb는_어디에도_두지_않는다() {
        assert_eq!(
            classify(Some("Windows.ModernShare"), "공유(S)", false),
            Slot::Hidden
        );
        assert_eq!(
            classify(Some("pintohome"), "즐겨찾기에 고정(Q)", false),
            Slot::Hidden
        );
        assert_eq!(classify(Some("cut"), "잘라내기(T)", false), Slot::Hidden);
    }

    #[test]
    fn verb도_대소문자를_접어_견준다() {
        // verb는 셸·확장이 정한 문자열이라 표기가 보장되지 않는다 — 표에 적은 그대로만
        // 견주면 같은 항목이 표기 차이로 `앱 확장`에 밀린다
        assert_eq!(차례(classify(Some("OPEN"), "열기", false)), Some(1));
        assert_eq!(
            차례(classify(Some("COPYASPATH"), "경로로 복사", false)),
            Some(5)
        );
        // 숨김 표도 같은 규칙을 쓴다 — 표기가 어긋나면 뺐다고 믿은 줄이 그대로 남는다
        assert_eq!(
            classify(Some("pintostartscreen"), "시작 화면에 고정", false),
            Slot::Hidden
        );
        assert_eq!(차례(classify(Some("NEW"), "새로 만들기", true)), Some(3));
    }

    #[test]
    fn 모르는_verb는_앱_확장으로_간다() {
        // 서드파티 확장이다 — **숨기지 않는다**. 한 단계 뒤로 갈 뿐이다
        assert_eq!(
            classify(Some("AnyCode"), "Visual Studio로 열기(V)", false),
            Slot::Extension
        );
        assert_eq!(
            classify(Some("link"), "바로 가기 만들기(S)", false),
            Slot::Extension
        );
    }

    #[test]
    fn 두지_않기로_한_다섯_줄은_어디에도_없다() {
        // 2026-08-26 사용자 요청. verb가 확인된 셋은 verb로, 나머지 둘은 문구로 건다
        for (verb, label) in [
            (Some("PreviousVersions"), "이전 버전 복원(V)"),
            (Some("sendto"), "보내기(N)"),
            (Some("PinToStartScreen"), "시작 화면에 고정(P)"),
        ] {
            assert_eq!(classify(verb, label, false), Slot::Hidden, "{label}");
        }
        // verb를 주지 않는 확장이 같은 문구로 그 자리를 채워도 걸린다
        for label in [
            "이전 버전 복원(V)",
            "보내기(N)",
            "시작 화면에 고정(P)",
            "Microsoft Defender(으)로 검사...",
            "Copilot에게 질문하기",
        ] {
            assert_eq!(classify(None, label, false), Slot::Hidden, "{label}");
        }
    }

    #[test]
    fn 표준_자리에는_아이콘_글리프가_붙는다() {
        // 셸이 비트맵을 주지 않는 줄이 많다(실측) — 탐색기와 견줘 빈 자리가 남지 않게
        // 표준 항목마다 글리프를 지정한다 (2026-08-26 D1·D2)
        let 글리프 = |slot: Slot| match slot {
            Slot::Standard { glyph, .. } => Some(glyph),
            _ => None,
        };
        use egui_phosphor::regular as 아이콘;
        for (verb, label, background, 기대) in [
            ("open", "열기(O)", false, 아이콘::FOLDER_OPEN),
            ("openas", "연결 프로그램(H)", false, 아이콘::APP_WINDOW),
            (
                "copyaspath",
                "경로로 복사(A)",
                false,
                아이콘::CLIPBOARD_TEXT,
            ),
            ("properties", "속성(R)", false, 아이콘::WRENCH),
            ("New", "새로 만들기(W)", true, 아이콘::FILE_PLUS),
        ] {
            assert_eq!(
                글리프(classify(Some(verb), label, background)),
                Some(기대),
                "{verb}"
            );
            // verb를 주지 않는 확장이 그 자리를 채워도 같은 글리프가 붙는다
            assert_eq!(
                글리프(classify(None, label, background)),
                Some(기대),
                "{label}"
            );
        }
        // `앱 확장`으로 가는 줄에는 글리프가 없다 — 서로 다른 확장이 같은 아이콘을 달면
        // 아이콘이 구분에 쓸모없어진다 (D2)
        assert_eq!(
            글리프(classify(Some("AnyCode"), "Visual Studio로 열기(V)", false)),
            None
        );
    }

    #[test]
    fn 시작_화면에_고정은_표준_표에서_빠졌다() {
        // 종전에는 `Slot::Standard(8)`이었다. **`classify`로는 이 회귀를 잡을 수 없다** —
        // 숨김 판정이 표준 조회보다 먼저라(`classify`) 표준 표에 그 줄이 되살아나도
        // 결과는 여전히 `Hidden`이다. 그래서 표 자체를 본다
        assert!(
            !STANDARD_VERBS
                .iter()
                .any(|(verb, ..)| verb.eq_ignore_ascii_case("PinToStartScreen")),
            "표준 verb 표에 되살아났다: {STANDARD_VERBS:?}"
        );
        assert!(
            !STANDARD_LABELS
                .iter()
                .any(|(ko, en, ..)| *ko == "시작 화면에 고정" || *en == "Pin to Start"),
            "표준 라벨 표에 되살아났다: {STANDARD_LABELS:?}"
        );
        // 차례 8은 비운 자리다 — 다른 항목이 그리로 당겨 오지 않았는지 함께 본다
        assert!(
            !STANDARD_VERBS.iter().any(|(_, order, _)| *order == 8)
                && !STANDARD_LABELS.iter().any(|(_, _, order, _)| *order == 8),
            "차례 8은 비어 있어야 한다"
        );
    }

    #[test]
    fn verb가_없으면_셸이_준_문구로_한_번_더_본다() {
        // T1 실측: `반디집으로 압축하기(L)...` 같은 확장은 verb를 주지 않는다.
        // 표준 항목이 그런 경우에도 자리를 지키게 라벨 폴백을 둔다 (D14)
        assert_eq!(차례(classify(None, "열기", false)), Some(1));
        assert_eq!(차례(classify(None, "Open", false)), Some(1));
        assert_eq!(차례(classify(None, "속성(R)", false)), Some(9));
        // 문구도 모르면 확장이다 — 모르는 것을 숨기지 않는다
        assert_eq!(
            classify(None, "Visual Studio로 열기(V)", false),
            Slot::Extension
        );
    }

    #[test]
    fn 압축_줄은_상위에서_빠져_하위_묶음으로_간다() {
        // `앱 확장`과 같은 처리다 — 상위 목록에는 머리 줄 하나만 서고 항목은 하위로 간다
        let items = vec![
            줄(10, "열기(O)"),
            줄(11, "보기.zip으로 압축하기(Z)"),
            줄(12, "반디집으로 압축하기(L)..."),
        ];
        let verb = |id: u32| (id == 10).then(|| "open".to_owned());
        let Arranged {
            rows,
            extensions,
            compressions,
            ..
        } = arrange(&items, verb, false, &[], false);

        assert_eq!(compressions.len(), 2, "압축 둘이 묶음으로 가야 한다");
        assert!(extensions.is_empty(), "압축은 `앱 확장`으로 새지 않는다");
        let 문구: Vec<String> = rows
            .iter()
            .map(|row| match row {
                shell_context_menu::ShellMenuRow::Shell { item, .. } => item.label.clone(),
                // **가상 하위 메뉴는 종류 이름만 뽑는다** — `Debug` 전체를 쓰면 필드가
                // 하나만 늘어도 이 단언들이 이유 없이 깨진다(T7이 unit variant를 struct
                // variant로 바꾸며 그 취약성이 커졌다)
                shell_context_menu::ShellMenuRow::Virtual { kind, .. } => format!("{kind:?}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            문구,
            vec![
                "열기(O)".to_owned(),
                "Upload".to_owned(),
                "Compress".to_owned()
            ]
        );
    }

    #[test]
    fn 압축_항목이_없어도_머리_줄은_선다() {
        // **D10 개정** — 종전에는 「0건이면 머리도 두지 않는다」였다. Windows 기본
        // `Zip 파일` 줄이 셸 항목과 무관하게 늘 있으므로(D13-3) 하위 메뉴가 빌 일이 없다
        let items = vec![줄(10, "열기(O)")];
        let verb = |_: u32| Some("open".to_owned());
        let Arranged {
            rows, compressions, ..
        } = arrange(&items, verb, false, &[], false);
        assert!(compressions.is_empty(), "셸이 준 압축 항목이 없는 판이다");
        assert!(
            rows.iter().any(|row| matches!(
                row,
                shell_context_menu::ShellMenuRow::Virtual {
                    kind: shell_context_menu::VirtualSubmenu::Compress,
                    ..
                }
            )),
            "그래도 머리 줄은 서야 한다: {rows:?}"
        );
    }

    #[test]
    fn 배경_메뉴에는_압축_줄이_서지_않는다() {
        // 고를 것이 없어 압축 항목 자체가 오지 않는다(실측) — 빈 하위 메뉴를 세우지 않는다
        let items = vec![줄(10, "새로 만들기(W)")];
        let verb = |_: u32| Some("New".to_owned());
        let Arranged { rows, .. } = arrange(&items, verb, true, &[], false);
        assert!(
            !rows.iter().any(|row| matches!(
                row,
                shell_context_menu::ShellMenuRow::Virtual {
                    kind: shell_context_menu::VirtualSubmenu::Compress,
                    ..
                }
            )),
            "{rows:?}"
        );
    }

    #[test]
    fn 펼친_압축_묶음은_다시_펼치지_않는다() {
        // 마우스가 얹혀 있는 동안 신호가 매 프레임 오는데, 그때마다 캐시를 다시 올리면
        // 프레임마다 텍스처를 만든다 (D6과 같은 이유)
        let 압축 = OpenSubmenu::Virtual(
            shell_context_menu::VirtualSubmenu::Compress,
            Vec::new(),
            shell_context_menu::MenuIcons::for_test(),
        );
        assert!(already_expanded(
            Some(&압축),
            ExpandTarget::Virtual(shell_context_menu::VirtualSubmenu::Compress)
        ));
        // 다른 묶음과 섞이지 않는다
        assert!(!already_expanded(
            Some(&압축),
            ExpandTarget::Virtual(shell_context_menu::VirtualSubmenu::Extensions)
        ));
        let 확장 = OpenSubmenu::Virtual(
            shell_context_menu::VirtualSubmenu::Extensions,
            Vec::new(),
            shell_context_menu::MenuIcons::for_test(),
        );
        assert!(!already_expanded(
            Some(&확장),
            ExpandTarget::Virtual(shell_context_menu::VirtualSubmenu::Compress)
        ));
        assert!(!already_expanded(
            None,
            ExpandTarget::Virtual(shell_context_menu::VirtualSubmenu::Compress)
        ));
    }

    #[test]
    fn 붙여넣기는_빈_곳에서만_맨_위에_선다() {
        use shell_context_menu::AppMenuItem as App;
        let items = vec![줄(10, "새로 만들기(W)")];
        let verb = |_: u32| Some("New".to_owned());
        let app = [(App::Paste, true)];

        // 배경 메뉴 — 차례 1이라 맨 위다
        let Arranged { rows, .. } = arrange(&items, verb, true, &app, false);
        let 첫줄 = rows.first().expect("줄이 있어야 한다");
        assert!(
            matches!(
                첫줄,
                shell_context_menu::ShellMenuRow::App {
                    item: App::Paste,
                    ..
                }
            ),
            "{rows:?}"
        );

        // **선택 메뉴에는 서지 않는다** — 넣어 보내도 `arrange`가 거른다.
        // 그 게이트가 `open_shell_menu`(COM 요구) 안에 있으면 이 시험을 쓸 수 없다
        let Arranged { rows, .. } = arrange(&items, verb, false, &app, false);
        assert!(
            !rows.iter().any(|row| matches!(
                row,
                shell_context_menu::ShellMenuRow::App {
                    item: App::Paste,
                    ..
                }
            )),
            "{rows:?}"
        );
    }

    #[test]
    fn 클립보드가_비면_붙여넣기가_흐리다() {
        use shell_context_menu::AppMenuItem as App;
        // 활성 여부는 부르는 쪽이 정해 넘긴다(메뉴를 열 때 클립보드를 한 번 본다) —
        // 그 값이 줄에 그대로 실리는지 본다
        let items = vec![줄(10, "새로 만들기(W)")];
        let verb = |_: u32| Some("New".to_owned());
        for 활성 in [true, false] {
            let Arranged { rows, .. } = arrange(&items, verb, true, &[(App::Paste, 활성)], false);
            let 실린값 = rows.iter().find_map(|row| match row {
                shell_context_menu::ShellMenuRow::App {
                    item: App::Paste,
                    enabled,
                } => Some(*enabled),
                _ => None,
            });
            assert_eq!(실린값, Some(활성), "활성={활성}");
        }
    }

    #[test]
    fn 붙여넣기_줄에는_단축키가_붙는다() {
        use shell_context_menu::AppMenuItem as App;
        // 셸 줄은 자기 단축키를 들고 오지만 앱 줄에는 그 필드가 없어 종전에는 빈 칸이었다
        assert_eq!(App::Paste.shortcut(), "Ctrl+V");
        assert_eq!(App::AddFavorite.shortcut(), "");
        assert_eq!(App::OpenInNewTab.shortcut(), "");
    }

    #[test]
    fn 하위_팝업_자리는_펼칠_때만_바뀌고_그_밖에는_지켜진다() {
        use shell_context_menu::ShellMenuPick as Pick;
        // 펼침 넷은 그 줄의 y로 갱신한다
        for pick in [
            Pick::Expand(crate::fs::shell_menu::SubmenuHandle::for_test(7, 3)),
            Pick::ExpandVirtual(shell_context_menu::VirtualSubmenu::Extensions),
            Pick::ExpandVirtual(shell_context_menu::VirtualSubmenu::Compress),
            Pick::ExpandVirtual(shell_context_menu::VirtualSubmenu::Extract),
        ] {
            assert_eq!(submenu_anchor(10.0, &pick, Some(99.0)), 99.0, "{pick:?}");
        }
        // **펼침이 아닌 신호는 지금 값을 지킨다** — 하위 항목을 실행하는 프레임에 자리가
        // 흔들리면 안 된다
        for pick in [Pick::Command(42), Pick::CollapseSubmenu, Pick::ShowMore] {
            assert_eq!(submenu_anchor(10.0, &pick, Some(99.0)), 10.0, "{pick:?}");
        }
        // **y가 없으면 펼침이어도 지킨다** — 하위 팝업이 올린 pick에는 붙일 줄이 없다
        assert_eq!(
            submenu_anchor(
                10.0,
                &Pick::ExpandVirtual(shell_context_menu::VirtualSubmenu::Compress),
                None
            ),
            10.0
        );
    }

    #[test]
    fn 해제_항목도_문구로만_가른다() {
        // 사용자 확장 스크린샷 실측(2026-08-26) — 그 줄들은 `앱 확장`에 밀려 있었다.
        // **verb는 보지 않는다** — `STANDARD_VERBS`가 넷뿐이라 verb가 있어도 표준 표에
        // 없으면 확장으로 가므로, 「확장에 있었다」에서 verb 부재를 끌어낼 수 없다
        for label in [
            "압축 풀기(T)...",
            "여기에 풀기(X)",
            "알아서 풀기(Z)",
            "반디집으로 압축 풀기(B)...",
            "Extract All...",
        ] {
            assert_eq!(classify(None, label, false), Slot::Extract, "{label}");
        }
        // **여는 것은 푸는 것이 아니다** — 그 줄은 `앱 확장`에 남는다
        for label in ["반디집으로 열기", "보기.zip으로 압축하기(Z)", "열기(O)"] {
            assert_ne!(classify(None, label, false), Slot::Extract, "{label}");
        }
        // 배경 메뉴에서는 보지 않는다 — 고를 것이 없어 해제 항목이 오지 않는다
        assert_ne!(classify(None, "압축 풀기(T)...", true), Slot::Extract);
    }

    #[test]
    fn 업로드_줄은_차례_3에_서고_보낼_곳이_없으면_흐리다() {
        // 2026-08-28 요청 — 연결된 원격 탭이 없으면 **줄은 서되 비활성**이다.
        // 줄 자체를 빼지 않는 이유: 있다가 없어지면 메뉴 줄 수가 달라져 누르려던 자리가 밀린다
        use shell_context_menu::{ShellMenuRow as Row, VirtualSubmenu as Kind};
        let items = vec![줄(10, "열기(O)"), 줄(11, "경로로 복사(A)")];
        let verb = |id: u32| Some(if id == 10 { "open" } else { "copyaspath" }.to_owned());

        let 업로드줄 = |enabled| {
            let Arranged { rows, .. } = arrange(&items, verb, false, &[], enabled);
            rows.iter()
                .find_map(|row| match row {
                    Row::Virtual {
                        kind: Kind::Upload,
                        enabled,
                    } => Some(*enabled),
                    _ => None,
                })
                .expect("`업로드` 줄이 있어야 한다")
        };
        assert!(업로드줄(true), "보낼 곳이 있는데 흐리다");
        assert!(!업로드줄(false), "보낼 곳이 없는데 눌린다");

        // **차례 3** — `열기`(1) 다음, `경로로 복사`(5) 앞이다
        let Arranged { rows, .. } = arrange(&items, verb, false, &[], true);
        let 라벨: Vec<String> = rows
            .iter()
            .map(|row| match row {
                Row::Shell { item, .. } => item.label.clone(),
                Row::Virtual { kind, .. } => format!("{kind:?}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            라벨,
            vec![
                "열기(O)".to_owned(),
                "Upload".to_owned(),
                "경로로 복사(A)".to_owned(),
                // `다음으로 압축`은 압축 프로그램이 없어도 늘 선다(D13-4)
                "Compress".to_owned(),
            ],
            "`업로드`가 `열기`와 `경로로 복사` 사이에 서지 않았다"
        );
    }

    #[test]
    fn 고른_원격_탭으로_보내는_전송_요청이_만들어진다() {
        // 하위 항목을 고르면 **그 탭의 (site, path)** 로 가는 요청이 조립돼야 한다 (FR-8).
        // 끌어다 놓기와 같은 값이라 같은 앞문(`start_transfer`)이 받고, 같은 이름 확인(FR-55)도
        // 그 안에서 돈다
        use crate::remote::types::{RemotePath, SiteId};
        use std::path::PathBuf;

        let target = crate::ui::app::UploadTarget {
            site: SiteId(7),
            dir: RemotePath::new("/pub/htdocs"),
            label: "웹서버 › htdocs".to_owned(),
        };
        let 고른것 = vec![PathBuf::from(r"C:\보고서.txt"), PathBuf::from(r"C:\자료")];

        let drop = upload_drop_outcome(Some(&target), &고른것).expect("요청이 만들어져야 한다");
        assert_eq!(drop.items.len(), 2, "고른 것이 모두 담겨야 한다");
        assert_eq!(drop.source_site, None, "로컬에서 나가는 전송이다");
        match drop.target {
            DropTarget::Remote { site, dir } => {
                assert_eq!(site, SiteId(7), "고른 탭의 사이트가 아니다");
                assert_eq!(
                    dir,
                    RemotePath::new("/pub/htdocs"),
                    "고른 탭의 폴더가 아니다"
                );
            }
            other => panic!("원격이 아닌 곳으로 간다: {other:?}"),
        }
        // 담긴 것이 전부 로컬 항목이다 — 원격에서 원격으로 보내는 길이 아니다
        assert!(
            drop.items
                .iter()
                .all(|item| matches!(item, DragItem::Local { .. }))
        );

        // 대상이 없으면(그 사이 탭이 닫혔다) 아무 일도 하지 않는다
        assert_eq!(upload_drop_outcome(None, &고른것), None);
        // 올릴 것이 없어도 마찬가지다
        assert_eq!(upload_drop_outcome(Some(&target), &[]), None);
    }

    #[test]
    fn 빈_곳_우클릭에는_업로드_줄이_서지_않는다() {
        // 올릴 항목이 없다 — 배경 메뉴는 「이 폴더에 하는 일」이라 보낼 것이 없다
        use shell_context_menu::{ShellMenuRow as Row, VirtualSubmenu as Kind};
        let items = vec![줄(10, "새로 만들기(W)")];
        let verb = |_: u32| Some("new".to_owned());
        // 보낼 곳이 있다고 해도(참) 배경에서는 서지 않는다
        let Arranged { rows, .. } = arrange(&items, verb, true, &[], true);
        assert!(
            !rows.iter().any(|row| matches!(
                row,
                Row::Virtual {
                    kind: Kind::Upload,
                    ..
                }
            )),
            "배경 메뉴에 `업로드`가 섰다: {rows:?}"
        );
    }

    #[test]
    fn 가상_하위_메뉴_셋의_자리가_합친_뒤에도_그대로다() {
        // T7은 **동작을 바꾸지 않는다** — 셋을 한 변형으로 접었을 뿐이라 같은 줄이
        // 같은 차례에 서야 한다. `앱 확장`만 차례 축 밖(구분선 뒤 맨 마지막)인 것도 그대로다
        use shell_context_menu::{ShellMenuRow as Row, VirtualSubmenu as Kind};
        let 확장이_있는_메뉴 = arrange(
            &[
                줄(2, "열기(O)"),
                줄(4, "무언가로 압축하기"),
                줄(6, "낯선 확장"),
            ],
            |id| match id {
                2 => Some("open".to_owned()),
                _ => None,
            },
            false,
            &[],
            false,
        );
        let rows = &확장이_있는_메뉴.rows;

        // `앱 확장`은 **맨 마지막**이고 그 앞이 구분선이다
        assert!(
            matches!(
                rows.last(),
                Some(Row::Virtual {
                    kind: Kind::Extensions,
                    ..
                })
            ),
            "`앱 확장`이 맨 뒤가 아니다: {rows:?}"
        );
        assert!(
            matches!(rows[rows.len() - 2], Row::Separator),
            "`앱 확장` 앞 구분선이 사라졌다: {rows:?}"
        );

        // `다음으로 압축`은 차례 축 **안**이라 `앱 확장`보다 앞이다
        let 압축_자리 = rows.iter().position(|row| {
            matches!(
                row,
                Row::Virtual {
                    kind: Kind::Compress,
                    ..
                }
            )
        });
        let 확장_자리 = rows.iter().position(|row| {
            matches!(
                row,
                Row::Virtual {
                    kind: Kind::Extensions,
                    ..
                }
            )
        });
        assert!(
            압축_자리 < 확장_자리,
            "압축 묶음이 `앱 확장` 뒤로 밀렸다: {rows:?}"
        );

        // **셋**은 언제나 활성이다 — 흐릴 수 있는 것은 `업로드`뿐이고 그것은 T8이 따로 본다
        for row in rows {
            if let Row::Virtual { kind, enabled } = row
                && !matches!(kind, Kind::Upload)
            {
                assert!(*enabled, "{kind:?} 묶음이 흐리게 섰다");
            }
        }
    }

    #[test]
    fn 해제가_있으면_압축_줄은_서지_않는다() {
        // 2026-08-26 사용자 선택(D3) — 압축 파일을 골랐으면 그 자리가 `압축 풀기`다.
        // 둘은 **같은 차례 6**을 번갈아 쓴다
        let items = vec![
            줄(10, "열기(O)"),
            줄(11, "압축 풀기(T)..."),
            줄(12, "보기.zip으로 압축하기(Z)"),
        ];
        let verb = |id: u32| (id == 10).then(|| "open".to_owned());
        let Arranged {
            rows,
            compressions,
            extractions,
            ..
        } = arrange(&items, verb, false, &[], false);

        assert_eq!(extractions.len(), 1, "해제 하나가 묶음으로 간다");
        assert_eq!(compressions.len(), 1, "압축 항목은 여전히 모인다");
        let 문구: Vec<String> = rows
            .iter()
            .map(|row| match row {
                shell_context_menu::ShellMenuRow::Shell { item, .. } => item.label.clone(),
                // **가상 하위 메뉴는 종류 이름만 뽑는다** — `Debug` 전체를 쓰면 필드가
                // 하나만 늘어도 이 단언들이 이유 없이 깨진다(T7이 unit variant를 struct
                // variant로 바꾸며 그 취약성이 커졌다)
                shell_context_menu::ShellMenuRow::Virtual { kind, .. } => format!("{kind:?}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            문구,
            vec![
                "열기(O)".to_owned(),
                "Upload".to_owned(),
                "Extract".to_owned()
            ],
            "차례 6에는 `압축 풀기`만 선다"
        );
    }

    #[test]
    fn 해제가_없으면_종전대로_압축_줄이_선다() {
        // 회귀 방지 — 압축 파일이 아닌 것을 골랐을 때는 아무것도 달라지지 않는다
        let items = vec![줄(10, "열기(O)")];
        let verb = |_: u32| Some("open".to_owned());
        let Arranged {
            rows, extractions, ..
        } = arrange(&items, verb, false, &[], false);
        assert!(extractions.is_empty());
        assert!(
            rows.iter().any(|row| matches!(
                row,
                shell_context_menu::ShellMenuRow::Virtual {
                    kind: shell_context_menu::VirtualSubmenu::Compress,
                    ..
                }
            )),
            "{rows:?}"
        );
    }

    #[test]
    fn 펼친_해제_묶음은_다시_펼치지_않는다() {
        let 해제 = OpenSubmenu::Virtual(
            shell_context_menu::VirtualSubmenu::Extract,
            Vec::new(),
            shell_context_menu::MenuIcons::for_test(),
        );
        assert!(already_expanded(
            Some(&해제),
            ExpandTarget::Virtual(shell_context_menu::VirtualSubmenu::Extract)
        ));
        // 다른 묶음과 섞이지 않는다
        assert!(!already_expanded(
            Some(&해제),
            ExpandTarget::Virtual(shell_context_menu::VirtualSubmenu::Compress)
        ));
        let 압축 = OpenSubmenu::Virtual(
            shell_context_menu::VirtualSubmenu::Compress,
            Vec::new(),
            shell_context_menu::MenuIcons::for_test(),
        );
        assert!(!already_expanded(
            Some(&압축),
            ExpandTarget::Virtual(shell_context_menu::VirtualSubmenu::Extract)
        ));
        assert!(!already_expanded(
            None,
            ExpandTarget::Virtual(shell_context_menu::VirtualSubmenu::Extract)
        ));
    }

    #[test]
    fn 압축_항목은_문구로만_가른다() {
        // verb를 주지 않고 **앞에 파일 이름이 붙어**(`보기.zip으로 압축하기(Z)`) 전체·접두
        // 비교로는 잡히지 않는다 — 부분 문자열로 본다 (2026-08-26 D6)
        for label in [
            "보기.zip으로 압축하기(Z)",
            "보기.7z로 압축하기(7)",
            "반디집으로 압축하기(L)...",
            "Compress to ZIP file",
        ] {
            assert_eq!(classify(None, label, false), Slot::Compress, "{label}");
        }
        // **`압축 풀기` 계열은 걸리지 않는다** — 그쪽에는 `압축하기`가 없다
        for label in ["압축 풀기", "여기에 압축 풀기", "Extract All..."] {
            assert_ne!(classify(None, label, false), Slot::Compress, "{label}");
        }
        // **배경 메뉴에서는 보지 않는다** — 고를 것이 없어 압축 항목이 오지 않는다(D9)
        assert_ne!(
            classify(None, "보기.zip으로 압축하기(Z)", true),
            Slot::Compress
        );
    }

    #[test]
    fn 문구로도_숨긴다() {
        // verb 없이 오는 공유·`메뉴 사용자 지정`을 잡는 2차 폴백
        assert_eq!(classify(None, "공유(S)", false), Slot::Hidden);
        assert_eq!(classify(None, "메뉴 사용자 지정", false), Slot::Hidden);
        assert_eq!(classify(None, "Customize menu", false), Slot::Hidden);
    }

    #[test]
    fn 라벨_폴백은_앞부분만_견주되_아무거나_잡지_않는다() {
        // 액셀러레이터·말줄임이 붙는 것은 같은 항목이다
        assert!(same_label("열기(O)", "열기"));
        assert!(same_label("반디집으로 압축하기...", "반디집으로 압축하기"));
        // 그러나 뒤에 다른 낱말이 이어지면 다른 항목이다 — 그러지 않으면 `열기`가
        // `열기 위치`를 잡아 엉뚱한 줄이 표준 자리에 선다
        assert!(!same_label("열기 위치", "열기"));
        assert!(!same_label("연결 프로그램 관리", "연결 프로그램"));
        // **접두도 대소문자를 접는다** — 영어 문구 표기가 Windows 판마다 다를 수 있다
        assert!(same_label("COPY AS PATH", "Copy as path"), "전체 일치");
        assert!(same_label("Copy As Path(A)", "Copy as path"), "접두 일치");
        // 한글이 잘리지 않는다 — 바이트 길이로 자르면 여기서 패닉하거나 어긋난다
        assert!(same_label("시작 화면에 고정(P)", "시작 화면에 고정"));
        assert!(!same_label("시", "시작 화면에 고정"), "문구가 더 짧은 경우");
    }

    #[test]
    fn 배경_메뉴는_다른_표를_본다() {
        // 배경에는 `열기`가 없고 `새로 만들기`가 있다 — 한 표로 두면 그 줄이
        // `앱 확장` 두 단계 아래로 밀린다
        assert_eq!(차례(classify(Some("New"), "새로 만들기(W)", true)), Some(3));
        // **셸의 `붙여넣기`는 숨긴다** — 앱이 세우는 줄로 바뀌었다(2026-08-26).
        // verb로도 문구로도 걸린다
        assert_eq!(classify(Some("paste"), "붙여넣기(P)", true), Slot::Hidden);
        assert_eq!(classify(None, "붙여넣기(P)", true), Slot::Hidden);
        assert_eq!(classify(None, "Paste", true), Slot::Hidden);
        // 선택 메뉴 쪽 표는 배경에서 보지 않는다
        assert_eq!(
            classify(Some("openas"), "연결 프로그램(H)", true),
            Slot::Extension
        );
    }

    /// 시험용 셸 줄 하나 — `(명령 번호, 문구)`
    fn 줄(id: u32, label: &str) -> crate::fs::shell_menu::ShellMenuItem {
        crate::fs::shell_menu::ShellMenuItem {
            id,
            label: label.to_owned(),
            shortcut: String::new(),
            icon: None,
            enabled: true,
            checked: false,
            separator: false,
            submenu: None,
        }
    }

    #[test]
    fn 재정렬한_뒤에도_아이콘_자리가_원래_것을_가리킨다() {
        // **아이콘 캐시는 셸이 준 원래 목록과 1:1이다** — 표준 차례로 재정렬하고 숨김을
        // 걷어낸 뒤의 자리로 그림을 찾으면 모든 줄에 엉뚱한 것이 붙는다.
        // 셸이 준 차례와 표준 차례가 **일부러 어긋나게** 짠 목록이다
        let items = vec![
            줄(10, "속성(R)"),             // origin 0 — 표준 차례 9 (맨 뒤로 간다)
            줄(11, "잘라내기(T)"),         // origin 1 — 숨김
            줄(12, "열기(O)"),             // origin 2 — 표준 차례 1 (맨 앞으로 온다)
            줄(13, "바로 가기 만들기(S)"), // origin 3 — 확장
            줄(14, "경로로 복사(A)"),      // origin 4 — 표준 차례 5 (가운데)
        ];
        let verb = |id: u32| {
            Some(
                match id {
                    10 => "properties",
                    11 => "cut",
                    12 => "open",
                    13 => "link",
                    14 => "copyaspath",
                    _ => return None,
                }
                .to_owned(),
            )
        };
        let Arranged {
            rows, extensions, ..
        } = arrange(&items, verb, false, &[], false);

        // 표준 셋이 차례대로 서고, 각 줄의 `origin`이 **셸이 준 원래 자리**를 가리킨다
        let 셸줄: Vec<(usize, &str)> = rows
            .iter()
            .filter_map(|row| match row {
                shell_context_menu::ShellMenuRow::Shell { item, origin, .. } => {
                    Some((*origin, item.label.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            셸줄,
            vec![(2, "열기(O)"), (4, "경로로 복사(A)"), (0, "속성(R)")],
            "차례는 표준 표를 따르고 origin은 원래 자리를 가리킨다"
        );
        // 확장도 원래 자리를 든다 — `MenuIcons::subset`이 그 값으로 부분집합을 만든다
        assert_eq!(extensions.len(), 1);
        assert_eq!(extensions[0].0.label, "바로 가기 만들기(S)");
        assert_eq!(extensions[0].1, 3);
        // 숨김은 어디에도 없다
        assert!(!셸줄.iter().any(|(_, label)| *label == "잘라내기(T)"));
    }

    #[test]
    fn 확장이_없으면_그_줄도_구분선도_두지_않는다() {
        // 빈 하위 메뉴는 고장으로 보인다 — 화살표만 있고 눌러도 아무것도 없는 줄
        let items = vec![줄(10, "열기(O)"), 줄(11, "속성(R)")];
        let verb = |id: u32| Some(if id == 10 { "open" } else { "properties" }.to_owned());
        let Arranged {
            rows, extensions, ..
        } = arrange(&items, verb, false, &[], false);
        assert!(extensions.is_empty());
        assert!(
            !rows.iter().any(|row| matches!(
                row,
                shell_context_menu::ShellMenuRow::Virtual {
                    kind: shell_context_menu::VirtualSubmenu::Extensions,
                    ..
                } | shell_context_menu::ShellMenuRow::Separator
            )),
            "확장 줄도 그 앞 구분선도 없다: {rows:?}"
        );

        // 확장이 하나라도 있으면 둘 다 선다 — 그 앞에 `다음으로 압축` 머리가 늘 있다
        let items = vec![줄(10, "열기(O)"), 줄(11, "바로 가기 만들기(S)")];
        let verb = |id: u32| Some(if id == 10 { "open" } else { "link" }.to_owned());
        let Arranged {
            rows, extensions, ..
        } = arrange(&items, verb, false, &[], false);
        assert_eq!(extensions.len(), 1);
        assert!(
            matches!(
                rows.as_slice(),
                [
                    shell_context_menu::ShellMenuRow::Shell { .. },
                    // `업로드`가 차례 3에 늘 선다 (T8) — 보낼 곳이 없으면 흐릴 뿐이다
                    shell_context_menu::ShellMenuRow::Virtual {
                        kind: shell_context_menu::VirtualSubmenu::Upload,
                        ..
                    },
                    shell_context_menu::ShellMenuRow::Virtual {
                        kind: shell_context_menu::VirtualSubmenu::Compress,
                        ..
                    },
                    shell_context_menu::ShellMenuRow::Separator,
                    shell_context_menu::ShellMenuRow::Virtual {
                        kind: shell_context_menu::VirtualSubmenu::Extensions,
                        ..
                    },
                ]
            ),
            "{rows:?}"
        );
    }

    #[test]
    fn 차례가_같으면_셸이_준_순서를_지킨다() {
        // 확장이 표준 문구를 흉내 내면(verb 없이 `속성`) 같은 자리에 둘이 선다 —
        // 하나를 버리지 않고 셸이 준 순서대로 나란히 세운다
        let items = vec![줄(10, "속성"), 줄(11, "속성(R)")];
        let verb = |id: u32| (id == 11).then(|| "properties".to_owned());
        let Arranged { rows, .. } = arrange(&items, verb, false, &[], false);
        let 문구: Vec<&str> = rows
            .iter()
            .filter_map(|row| match row {
                shell_context_menu::ShellMenuRow::Shell { item, .. } => Some(item.label.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(문구, vec!["속성", "속성(R)"]);
    }

    #[test]
    fn 셸이_준_구분선은_버리고_새로_긋는다() {
        // 셸이 준 자리는 원래 구성에 맞춰진 것이라 재정렬 뒤에는 뜻이 없다
        let mut 구분선 = 줄(0, "");
        구분선.separator = true;
        let items = vec![줄(10, "열기(O)"), 구분선, 줄(11, "바로 가기 만들기(S)")];
        let verb = |id: u32| Some(if id == 10 { "open" } else { "link" }.to_owned());
        let Arranged { rows, .. } = arrange(&items, verb, false, &[], false);
        // 구분선은 하나뿐이고 그것은 `앱 확장` 앞의 것이다
        let 구분선_수 = rows
            .iter()
            .filter(|row| matches!(row, shell_context_menu::ShellMenuRow::Separator))
            .count();
        assert_eq!(구분선_수, 1);
    }

    #[test]
    fn 즐겨찾기는_폴더_하나일_때만_열린다() {
        use std::path::{Path, PathBuf};
        let 보는_폴더 = Path::new(r"D:\작업");
        let 하위 = PathBuf::from(r"D:\작업\하위");
        let 파일 = PathBuf::from(r"D:\작업\메모.txt");

        // 고른 것이 없으면 **보고 있는 폴더**가 대상이다 (배경 메뉴)
        assert_eq!(
            favorite_target(보는_폴더, &[], &[], |_| false),
            Some(보는_폴더),
            "빈 곳 우클릭"
        );
        // 폴더 하나면 그 폴더
        assert_eq!(
            favorite_target(보는_폴더, std::slice::from_ref(&하위), &[true], |_| false),
            Some(하위.as_path())
        );
        // 파일이면 대상이 없다 — 앱 즐겨찾기는 폴더 목록이다
        assert_eq!(
            favorite_target(보는_폴더, std::slice::from_ref(&파일), &[false], |_| false),
            None
        );
        // 여럿이면 대상이 없다 — 폴더 둘이어도 마찬가지다
        assert_eq!(
            favorite_target(보는_폴더, &[하위.clone(), 파일], &[true, true], |_| false),
            None
        );
        // **이미 담긴 폴더는 대상이 없다** — 눌러도 아무 일이 없는 것은 비활성보다 나쁘다.
        // 실제 저장소로도 확인한다 — `contains`의 경로 비교 규칙까지 그대로 지난다
        let 담긴것 = crate::app::favorites::FavoriteStore::with_defaults([], [하위.clone()]);
        assert_eq!(
            favorite_target(
                보는_폴더,
                std::slice::from_ref(&하위),
                &[true],
                |path| { 담긴것.contains(path) }
            ),
            None,
            "이미 담긴 폴더"
        );
        // 담기지 않은 폴더는 그대로 열린다 — 같은 저장소로 견줘 판정이 실제로 갈리는지 본다
        let 다른_폴더 = PathBuf::from(r"D:\작업\다른곳");
        assert_eq!(
            favorite_target(
                보는_폴더,
                std::slice::from_ref(&다른_폴더),
                &[true],
                |path| 담긴것.contains(path)
            ),
            Some(다른_폴더.as_path())
        );
        // 배경 메뉴에서도 같다 — 보고 있는 폴더가 이미 담겼으면 비활성이다
        let 보는곳도_담김 =
            crate::app::favorites::FavoriteStore::with_defaults([], [보는_폴더.to_path_buf()]);
        assert_eq!(
            favorite_target(보는_폴더, &[], &[], |path| 보는곳도_담김.contains(path)),
            None,
            "빈 곳 우클릭인데 그 폴더가 이미 담김"
        );
    }

    #[test]
    #[should_panic(expected = "items/dirs 짝이 어긋났다")]
    fn 짝이_어긋나면_개발_빌드에서_드러난다() {
        // `MenuRequest`를 만들 때 `unzip`으로 갈라진 짝이라 길이가 언제나 같아야 한다 —
        // 그 사이 어딘가가 한쪽만 걸러 내면 여기서 잡힌다.
        //
        // **릴리즈 빌드에서는 단언이 빠지고 「비활성」으로 조용히 떨어진다**(`_ => None`) —
        // 사용자에게 패닉을 보이는 것보다 그 줄이 흐린 편이 낫다. 그 갈래는 단언이 살아 있는
        // 이 시험 빌드에서 확인할 수 없어 코드로만 둔다
        let _ = favorite_target(
            std::path::Path::new(r"D:\작업"),
            std::slice::from_ref(&std::path::PathBuf::from(r"D:\작업\하위")),
            &[],
            |_| false,
        );
    }

    #[test]
    fn 고른_즐겨찾기_줄이_담기_조작으로_이어진다() {
        use crate::app::favorites::FavoriteAction;
        use std::path::{Path, PathBuf};
        let 폴더 = PathBuf::from(r"D:\작업");

        assert_eq!(
            favorite_action_for(
                shell_context_menu::AppMenuItem::AddFavorite,
                Some(폴더.as_path())
            ),
            Some(FavoriteAction::Add(폴더.clone()))
        );
        // 대상이 없으면 아무 일도 하지 않는다 — 그 줄은 비활성이지만 활성 판정과 실행이
        // 다른 프레임에 있어 여기서 한 번 더 막는다
        assert_eq!(
            favorite_action_for(shell_context_menu::AppMenuItem::AddFavorite, None),
            None
        );
        // 새 탭 열기는 즐겨찾기를 건드리지 않는다 — 대상이 있어도 마찬가지다
        assert_eq!(
            favorite_action_for(
                shell_context_menu::AppMenuItem::OpenInNewTab,
                Some(Path::new(r"D:\작업"))
            ),
            None
        );
    }

    #[test]
    fn 새_탭에서_열기는_폴더_하나일_때만_선다() {
        use std::path::PathBuf;
        let 하위 = PathBuf::from(r"D:\작업\하위");
        let 파일 = PathBuf::from(r"D:\작업\메모.txt");

        // 폴더 하나면 그 폴더가 대상이다
        assert_eq!(
            new_tab_target(std::slice::from_ref(&하위), &[true]),
            Some(하위.as_path())
        );
        // **빈 곳 우클릭에는 서지 않는다** — 보고 있는 폴더를 새 탭에 여는 것은
        // `Ctrl+T`와 같아 중복이다(즐겨찾기 줄과 다른 규칙)
        assert_eq!(new_tab_target(&[], &[]), None);
        // 파일이면 서지 않는다 — 탐색기의 파일 메뉴에도 그 줄이 없다
        assert_eq!(new_tab_target(std::slice::from_ref(&파일), &[false]), None);
        // 여럿이면 서지 않는다 — 폴더 둘이어도 마찬가지다(D3)
        assert_eq!(new_tab_target(&[하위.clone(), 하위], &[true, true]), None);
    }

    #[test]
    fn 앱_줄은_탐색기_차례에_끼어든다() {
        // `즐겨찾기에 추가`(7)는 `경로로 복사`(5)와 `속성`(9) 사이에 선다
        let items = vec![
            줄(10, "열기(O)"),
            줄(11, "속성(R)"),
            줄(12, "경로로 복사(A)"),
        ];
        let verb = |id: u32| {
            Some(
                match id {
                    10 => "open",
                    11 => "properties",
                    _ => "copyaspath",
                }
                .to_owned(),
            )
        };
        let app = [(shell_context_menu::AppMenuItem::AddFavorite, true)];
        let Arranged { rows, .. } = arrange(&items, verb, false, &app, false);
        let 차례: Vec<String> = rows
            .iter()
            .map(|row| match row {
                shell_context_menu::ShellMenuRow::Shell { item, .. } => item.label.clone(),
                shell_context_menu::ShellMenuRow::App { item, .. } => format!("[앱] {item:?}"),
                // 위 두 시험과 같은 이유로 종류 이름만 뽑는다
                shell_context_menu::ShellMenuRow::Virtual { kind, .. } => format!("{kind:?}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            차례,
            vec![
                "열기(O)".to_owned(),
                "Upload".to_owned(),
                "경로로 복사(A)".to_owned(),
                // 차례 6 — `경로로 복사`(5)와 `즐겨찾기에 추가`(7) 사이다(D7)
                "Compress".to_owned(),
                "[앱] AddFavorite".to_owned(),
                "속성(R)".to_owned(),
            ]
        );
    }

    #[test]
    fn 비활성_앱_줄도_자리는_지킨다() {
        // 대상이 없어도 줄은 선다 — 흐리게 보이는 편이 「있다가 없다가」보다 낫다
        let items = vec![줄(10, "열기(O)")];
        let verb = |_: u32| Some("open".to_owned());
        let app = [(shell_context_menu::AppMenuItem::AddFavorite, false)];
        let Arranged { rows, .. } = arrange(&items, verb, false, &app, false);
        assert!(rows.iter().any(|row| matches!(
            row,
            shell_context_menu::ShellMenuRow::App {
                item: shell_context_menu::AppMenuItem::AddFavorite,
                enabled: false
            }
        )));
    }

    #[test]
    fn 실측한_메뉴가_표준_하한을_채운다() {
        // **P5 붕괴 검출** — 표준 표가 비면 상위 목록이 비고 "탐색기처럼"이 무산된다.
        // T1이 이 PC에서 실제로 찍은 줄을 그대로 흘려 그 수를 센다
        let 세기 = |줄들: &[(Option<&str>, &str)], background: bool| {
            줄들
                .iter()
                .filter(|(verb, label)| 차례(classify(*verb, label, background)).is_some())
                .count()
        };
        // T1 실측 — 파일
        let 파일 = [
            (Some("open"), "열기(O)"),
            (Some("Edit"), "편집"),
            (Some("Print"), "인쇄"),
            (Some("pintohomefile"), "즐겨찾기에 추가(F)"),
            (None, "보기.zip으로 압축하기(Z)"),
            (Some("openas"), "연결 프로그램(H)"),
            (Some("Windows.Share"), "액세스 권한 부여 (G)"),
            (Some("copyaspath"), "경로로 복사(A)"),
            (Some("Windows.ModernShare"), "공유(S)"),
            (None, "PowerRename으로 이름 바꾸기(E)"),
            (Some("PreviousVersions"), "이전 버전 복원(V)"),
            (Some("sendto"), "보내기(N)"),
            (Some("cut"), "잘라내기(T)"),
            (Some("copy"), "복사(C)"),
            (Some("link"), "바로 가기 만들기(S)"),
            (Some("delete"), "삭제(D)"),
            (Some("properties"), "속성(R)"),
        ];
        assert!(세기(&파일, false) >= 4, "파일: {}", 세기(&파일, false));
        // T1 실측 — 폴더
        let 폴더 = [
            (Some("open"), "열기(O)"),
            (Some("pintohome"), "즐겨찾기에 고정(Q)"),
            (Some("AnyCode"), "Visual Studio로 열기(V)"),
            (None, "하위.zip으로 압축하기(Z)"),
            (Some("Windows.Share"), "액세스 권한 부여 (G)"),
            (Some("PreviousVersions"), "이전 버전 복원(V)"),
            (None, "라이브러리에 포함(I)"),
            (Some("PinToStartScreen"), "시작 화면에 고정(P)"),
            (Some("copyaspath"), "경로로 복사(A)"),
            (Some("sendto"), "보내기(N)"),
            (Some("cut"), "잘라내기(T)"),
            (Some("copy"), "복사(C)"),
            (Some("delete"), "삭제(D)"),
            (Some("properties"), "속성(R)"),
        ];
        // **하한이 파일(4)보다 하나 낮다** — 폴더 메뉴의 표준 넷 중 `시작 화면에 고정`을
        // 2026-08-26에 뺐고(사용자 요청), 남는 것은 `열기`·`경로로 복사`·`속성` 셋이다
        assert!(세기(&폴더, false) >= 3, "폴더: {}", 세기(&폴더, false));
        // T1 실측 — 배경(빈 곳). 잴 때 클립보드가 비어 `붙여넣기`가 오지 않았다
        let 배경 = [
            (Some("{0001DEAD-9BF7-4CFA-8A5C-DE8679340002}"), "새 폴더(N)"),
            (
                Some("{9F156763-7844-4DC4-B2B1-901F640F5155}"),
                "터미널에서 열기(T)",
            ),
            (Some("AnyCode"), "Visual Studio로 열기(V)"),
            (Some("Windows.Share"), "액세스 권한 부여 (G)"),
            (Some("New"), "새로 만들기(W)"),
            (Some("properties"), "속성(R)"),
        ];
        assert!(세기(&배경, true) >= 2, "배경: {}", 세기(&배경, true));
        // **셸의 `붙여넣기`는 숨긴다** — 그 자리는 앱이 세우는 줄이 대신한다(2026-08-26).
        // 아이콘 줄에 붙여넣기 칸이 없다는 사정은 그대로이고, 진입점이 앱 줄로 옮겼을 뿐이다
        assert_eq!(classify(Some("paste"), "붙여넣기(P)", true), Slot::Hidden);
    }
}
