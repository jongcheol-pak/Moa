//! 전송 큐 표 (FR-36) — 원본 `FileExplorer-FTP.dc.html:272-294`.
//!
//! 연결별 탭 한 줄 · 머리글 한 줄 · 항목 행들로 이뤄진다.
//!
//! **열 구성은 탭마다 다르다** (FR-36) — `전송 큐`는 진행률·상태를 든 일곱 열,
//! `성공`은 그 둘 대신 `시간`을 든 여섯 열, `실패`는 거기에 `이유`가 붙은 일곱 열이다
//! (`QueueColumnKind`·`columns_for`). 폭도 탭마다 한 벌씩 기억하고(`QueueColumns`),
//! 합이 표 폭보다 좁으면 **그 탭의 마지막 열**이 차이를 흡수한다 — 흡수 열이 앞자리면
//! 그 오른쪽 경계를 끌어도 흡수분이 같은 양을 반대로 먹어 폭 조절이 성립하지 않는다
//! (2026-08-18 plan D6).
//!
//! 그래도 `ui::list_details`의 열 부품과 합치지는 않는다(plan 비추상화 선언) — 두 열거값이
//! 겹치는 것은 `크기` 하나뿐이고, 그쪽은 **사용자가 켜고 끄는** 열이지만 이쪽은 **탭이 정하는**
//! 열이라 다루는 규칙이 다르다.
//!
//! **큐를 고치지 않는다** — 읽어서 그리고, 사용자가 고른 것은 값으로 돌려준다.
use crate::panel::file_list::LocalTime;
use crate::remote::connection::TransferId;
use crate::remote::queue::{QueueFilter, TransferItem, TransferState, UNKNOWN};
use crate::remote::sites::SiteStore;
use crate::remote::types::SiteId;
use crate::ui::dock::{DockState, DockView};
use crate::ui::list_common::elided_galley_colored;
use crate::ui::theme;
use crate::ui::widgets;
use eframe::egui;
use std::collections::HashSet;

// ── 시각 토큰 (원본 `:272-292`) ──
/// 연결별 탭 행 (`:272`)
pub const SITE_ROW_HEIGHT: f32 = 28.0;
const SITE_ROW_PAD_X: f32 = 4.0;
const SITE_TAB_GAP: f32 = 2.0;
const SITE_TAB_PAD_X: f32 = 12.0;
const SITE_TAB_INNER_GAP: f32 = 7.0;
const SITE_DOT: f32 = 6.0;
/// 활성 탭 아래 강조 띠
const SITE_TAB_EDGE: f32 = 2.0;
/// 머리글·항목 행 (`:279`·`:283`)
const HEADER_HEIGHT: f32 = 22.0;
const ROW_HEIGHT: f32 = 24.0;
const CELL_PAD_X: f32 = 6.0;
const FONT_PX: f32 = 13.0;
/// 빈 표 안내가 머리글 아래에서 떨어지는 거리
const EMPTY_HINT_TOP: f32 = 24.0;
/// 진행 막대 (`:290`)
const BAR_WIDTH: f32 = 110.0;
const BAR_HEIGHT: f32 = 6.0;
/// 열 경계 드래그 핸들 폭 — 경계 중심에서 좌우로 절반씩.
/// `list_details::HANDLE_WIDTH`는 private이라 같은 값을 여기 둔다
const HANDLE_WIDTH: f32 = 6.0;

/// 큐 표의 열 한 종류 (FR-36).
///
/// **자리 번호가 아니라 종류로 든다** — 탭마다 열 구성이 달라(성공은 진행률·상태 대신 `시간`,
/// 실패는 거기에 `이유`가 더 붙는다) 번호로 두면 같은 인덱스가 탭에 따라 다른 열을 가리킨다.
/// `ui::list_details`가 `ColumnKind`로 같은 판단을 이미 했고, 그렇다고 그 타입과 합치지는
/// 않는다(그쪽은 파일 목록의 열이라 뜻이 겹치는 것이 `크기` 하나뿐이다)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueColumnKind {
    Direction,
    Local,
    Remote,
    Server,
    Size,
    Progress,
    State,
    /// 시작 ~ 끝을 한 칸에 (성공·실패 탭)
    Time,
    /// 서버가 준 실패 사유 원문 (실패 탭)
    Reason,
}

impl QueueColumnKind {
    /// 머리글 문구 — 언어를 따르므로 상수가 아니라 그때그때 만든다 (인벤토리 #37~#43)
    fn header(self) -> &'static str {
        match self {
            QueueColumnKind::Direction => crate::i18n::queue_column_direction(),
            QueueColumnKind::Local => crate::i18n::queue_column_local(),
            QueueColumnKind::Remote => crate::i18n::queue_column_remote(),
            QueueColumnKind::Server => crate::i18n::queue_column_server(),
            QueueColumnKind::Size => crate::i18n::queue_column_size(),
            QueueColumnKind::Progress => crate::i18n::queue_column_progress(),
            QueueColumnKind::State => crate::i18n::queue_column_state(),
            QueueColumnKind::Time => crate::i18n::queue_column_time(),
            QueueColumnKind::Reason => crate::i18n::queue_column_reason(),
        }
    }

    /// 기본 폭 — 앞 일곱은 원본 `34px 1fr 300px 120px 84px 118px 150px` (`:279`)에서
    /// `1fr`(로컬 파일)만 고정값으로 바꾼 값이다. 흡수 열이 앞자리에 있으면 그 오른쪽 경계를
    /// 끌어도 흡수분이 같은 양을 반대로 먹어 **잡은 경계가 제자리에 선다** (plan D6).
    ///
    /// `시간`은 `08-27 14:03:21 ~ 08-27 14:03:48`이 드는 폭이고, `이유`는 서버 사유가
    /// 대개 한 줄로 보이는 폭이다
    fn default_width(self) -> f32 {
        match self {
            QueueColumnKind::Direction => 34.0,
            QueueColumnKind::Local => 280.0,
            QueueColumnKind::Remote => 300.0,
            QueueColumnKind::Server => 120.0,
            QueueColumnKind::Size => 84.0,
            QueueColumnKind::Progress => 118.0,
            QueueColumnKind::State => 150.0,
            QueueColumnKind::Time => 170.0,
            QueueColumnKind::Reason => 220.0,
        }
    }

    /// 세션에 담는 키 (FR-11) — `list_details::ColumnKind::as_key`와 같은 방식이다.
    ///
    /// **숫자가 아니라 문자열인 이유**: 열이 늘거나 `ALL_QUEUE_COLUMNS`의 차례가 바뀌면
    /// 숫자는 값이 밀려 옛 파일이 엉뚱한 열을 가리킨다
    pub fn as_key(self) -> &'static str {
        match self {
            QueueColumnKind::Direction => "direction",
            QueueColumnKind::Local => "local",
            QueueColumnKind::Remote => "remote",
            QueueColumnKind::Server => "server",
            QueueColumnKind::Size => "size",
            QueueColumnKind::Progress => "progress",
            QueueColumnKind::State => "state",
            QueueColumnKind::Time => "time",
            QueueColumnKind::Reason => "reason",
        }
    }

    /// 저장 키를 되읽는다 — 모르는 키는 `None`이라 그 자리를 버린다
    fn from_key(key: &str) -> Option<QueueColumnKind> {
        ALL_QUEUE_COLUMNS
            .into_iter()
            .find(|kind| kind.as_key() == key)
    }

    /// 끌 수 없는 열인가 — `방향`·`로컬`·`원격`은 열 메뉴에서 **항상 체크된 채 비활성**이다.
    ///
    /// 그 셋이 빠지면 「무엇을 어디서 어디로 옮기는가」가 사라져 행이 읽히지 않는다
    /// (2026-09-07 사용자 결정 — `list_details::ColumnKind::is_fixed`가 `이름`에 대해 하는
    /// 판단과 같은 성질이다)
    pub fn is_fixed(self) -> bool {
        matches!(
            self,
            QueueColumnKind::Direction | QueueColumnKind::Local | QueueColumnKind::Remote
        )
    }

    /// 폭 배열에서의 자리 — **탭 안의 차례가 아니라 종류에 매인 고정 자리다**.
    ///
    /// 차례를 바꿔도 각 열이 자기 폭을 그대로 갖게 하는 근거다
    /// (`list_details::ColumnKind::slot`과 같은 규칙)
    fn slot(self) -> usize {
        match self {
            QueueColumnKind::Direction => 0,
            QueueColumnKind::Local => 1,
            QueueColumnKind::Remote => 2,
            QueueColumnKind::Server => 3,
            QueueColumnKind::Size => 4,
            QueueColumnKind::Progress => 5,
            QueueColumnKind::State => 6,
            QueueColumnKind::Time => 7,
            QueueColumnKind::Reason => 8,
        }
    }
}

/// 큐 열 전부 — 폭 배열의 자리 수와 저장 키 되읽기의 후보 목록이다.
/// **차례는 `slot()`과 같아야 한다** (시험이 고정한다)
pub const ALL_QUEUE_COLUMNS: [QueueColumnKind; QUEUE_COLUMN_COUNT] = [
    QueueColumnKind::Direction,
    QueueColumnKind::Local,
    QueueColumnKind::Remote,
    QueueColumnKind::Server,
    QueueColumnKind::Size,
    QueueColumnKind::Progress,
    QueueColumnKind::State,
    QueueColumnKind::Time,
    QueueColumnKind::Reason,
];

/// 큐 열의 가짓수 — 폭 배열의 길이다
pub const QUEUE_COLUMN_COUNT: usize = 9;

// **앞 다섯 열은 세 탭이 함께 쓴다** — 무엇을 어디로 옮기는가는 어느 탭에서 보든 같다.
// 그 다섯을 상수로 빼 이어붙이지 않는 것은 const 문맥에서 배열을 잇는 길이 마땅치 않아서다.
// 대신 「세 탭의 앞 다섯이 서로 같다」를 시험이 고정한다(`성공과_실패_탭은_…`)

/// `전송 큐` 탭 — 종전 일곱 열 그대로다(진행 중인 것을 보는 자리라 진행률·상태가 필요하다)
const ALL_TAB_COLUMNS: [QueueColumnKind; 7] = [
    QueueColumnKind::Direction,
    QueueColumnKind::Local,
    QueueColumnKind::Remote,
    QueueColumnKind::Server,
    QueueColumnKind::Size,
    QueueColumnKind::Progress,
    QueueColumnKind::State,
];

/// `성공` 탭 — 끝난 것만 모이므로 진행률은 언제나 가득이고 상태는 언제나 `완료`다.
/// 그 두 칸을 빼고 **언제 오갔는지**를 대신 적는다
const DONE_TAB_COLUMNS: [QueueColumnKind; 6] = [
    QueueColumnKind::Direction,
    QueueColumnKind::Local,
    QueueColumnKind::Remote,
    QueueColumnKind::Server,
    QueueColumnKind::Size,
    QueueColumnKind::Time,
];

/// `실패` 탭 — `성공`과 같되 **왜 실패했는지**가 한 열 더 붙는다.
/// 종전에는 그 사유가 `상태` 열에 있었다
const ERROR_TAB_COLUMNS: [QueueColumnKind; 7] = [
    QueueColumnKind::Direction,
    QueueColumnKind::Local,
    QueueColumnKind::Remote,
    QueueColumnKind::Server,
    QueueColumnKind::Size,
    QueueColumnKind::Time,
    QueueColumnKind::Reason,
];

/// 그 탭에 설 열들 (FR-36).
///
/// **거르개가 곧 탭이다** — `DockState`가 이미 그 값을 들고 있어(`dock.rs`) 화면이 따로
/// 탭 종류를 알 필요가 없다
pub fn columns_for(filter: QueueFilter) -> &'static [QueueColumnKind] {
    match filter {
        QueueFilter::All => &ALL_TAB_COLUMNS,
        QueueFilter::Done => &DONE_TAB_COLUMNS,
        QueueFilter::Error => &ERROR_TAB_COLUMNS,
    }
}
// 상태 문구(인벤토리 #45~#47)와 행 우클릭 메뉴는 카탈로그에서 가져온다.
// 그 메뉴는 **디자인에 진입점이 없어 이 구현이 정한 문구**다 — 큐 항목을 하나씩
// 다시 걸거나 그만두는 길이 달리 없다(`⏸`·`✕`는 큐 전체를 다룬다)

/// 사용자가 큐에서 고른 조작.
///
/// **번호를 여럿 든다** — 행 메뉴는 우클릭한 그 행이 아니라 **고른 것 전부**를 대상으로
/// 삼는다(2026-08-28 사용자 결정). 선택이 없거나 선택 밖을 우클릭하면 그 행 하나가 대상이다
/// (`effective_selection`).
///
/// `…All`은 **지금 보고 있는 목록**(상단 거르개 ∩ 연결별 탭)이 대상이다 — 대상 계산은
/// 화면이 아니라 앱이 한다(`ui::app::apply_queue_action`). 이 모듈은 큐를 고치지 않는다
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueAction {
    /// 실패·취소한 항목을 다시 대기로 되돌린다
    Retry(Vec<TransferId>),
    /// 보이는 목록의 실패한 항목을 모두 다시 대기로
    RetryAll,
    /// 아직 끝나지 않은 항목을 그만둔다
    Cancel(Vec<TransferId>),
    /// 끝난 항목을 목록에서 지운다
    Remove(Vec<TransferId>),
    /// 보이는 목록을 통째로 지운다 (진행 중인 것도 멈추고 지운다)
    RemoveAll,
}

/// 행 우클릭 메뉴에 설 항목 — **탭이 아니라 행 상태가 정한다** (2026-08-18 사용자 결정).
///
/// `전송 취소`와 `삭제`는 동작이 같아(전송 중단 + 목록 제거 + `.part` 삭제) 나란히 두면
/// 무엇을 눌러야 할지 헷갈린다 — 상태별로 한 쪽만 보인다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowMenuItem {
    Retry,
    RetryAll,
    Cancel,
    Remove,
    RemoveAll,
}

/// 메뉴에 보일 항목들 (순수 판정 — 그리기와 나눠 두어 시험할 수 있게 한다).
///
/// **대상은 고른 것 전부다** — 여러 줄을 골라 놓고 메뉴를 열면 상태가 섞일 수 있어
/// **「하나라도」 규칙**으로 가린다(하나라도 다시 걸 수 있으면 `다시 시도`가 서고, 하나라도
/// 진행 중·대기면 `전송 취소`가 선다). 한 줄만 골랐을 때의 결과는 종전과 같다.
///
/// 섞인 선택에서 `전송 취소`와 `삭제`가 나란히 서는 것은 정상이다 — 둘은 하는 일이 다르다
/// (취소는 `취소됨`으로 남기고 삭제는 목록에서 지운다 — 2026-08-28 개정).
///
/// `has_retryable_in_view`는 **보이는 목록에 다시 걸 것이 하나라도 있는가**다(실패·취소).
/// 없으면 `전체 다시 시도`를 내지 않는다 — 눌러도 아무 일이 없는 메뉴는 두지 않는다
pub fn row_menu_items(states: &[TransferState], has_retryable_in_view: bool) -> Vec<RowMenuItem> {
    let mut items = Vec::new();
    // 취소한 줄에도 `다시 시도`가 선다 — 목록에 남기기로 한 이유가 그것이다 (2026-08-28)
    if states.iter().any(TransferState::is_retryable) {
        items.push(RowMenuItem::Retry);
    }
    if states.iter().any(TransferState::is_pending) {
        items.push(RowMenuItem::Cancel);
    }
    if has_retryable_in_view {
        items.push(RowMenuItem::RetryAll);
    }
    // 진행 중·대기는 `전송 취소`가 그 자리를 맡는다
    if states.iter().any(|state| !state.is_pending()) {
        items.push(RowMenuItem::Remove);
    }
    items.push(RowMenuItem::RemoveAll);
    items
}

/// 얼룩 규칙 — 원본은 **거른 뒤의 자리 번호**(0부터)가 홀수인 행을 칠한다 (`:721`)
fn stripe(index: usize) -> bool {
    !index.is_multiple_of(2)
}

/// 큐의 크기 표기 — 표시 규칙은 파일 목록과 **한 벌**이고(`panel::file_list::format_size`),
/// 여기서는 큐에만 있는 판정 하나를 앞에 둔다.
///
/// 0은 "크기를 모른다"는 뜻이라 `—`다 — 목록에서는 같은 0이 "빈 파일"이라 `0.00 KB`로
/// 나가야 해서, 이 갈래를 코어 함수에 넣지 않는다 (plan D2)
pub fn format_size(bytes: u64) -> String {
    if bytes == 0 {
        return UNKNOWN.to_owned();
    }
    crate::panel::file_list::format_size(bytes)
}

/// 시간 열의 표기 — `시작 ~ 끝`을 한 칸에 적는다 (성공·실패 탭).
///
/// **오늘이면 시각만, 아니면 `MM-DD`를 앞에 붙인다** — 전송은 대개 그날 안에 끝나 날짜를
/// 늘 적으면 칸만 넓어진다. 다만 세션에서 되살아난 실패 항목은 며칠 전 것일 수 있어,
/// 그때는 날짜가 없으면 사용자가 방금 실패한 것으로 오해한다.
///
/// **연도는 적지 않는다** — 칸을 넘긴다. 큐 세션은 대기·실패만 담아 해를 넘겨 남는 일이
/// 드물고, 넘긴 경우에도 `MM-DD`가 다르므로 오늘이 아님은 드러난다.
///
/// `today`를 인자로 받는 것은 시계에 기대지 않고 시험하기 위해서다
/// (속도 계산이 시각을 인자로 받는 것과 같은 이유)
pub fn format_time_range(start: Option<u64>, end: Option<u64>, today: LocalTime) -> String {
    let stamp = |ft: u64| {
        crate::panel::file_list::local_time_parts(ft).map(|t| {
            if t.same_day(today) {
                format!("{:02}:{:02}:{:02}", t.hour, t.minute, t.second)
            } else {
                format!(
                    "{:02}-{:02} {:02}:{:02}:{:02}",
                    t.month, t.day, t.hour, t.minute, t.second
                )
            }
        })
    };
    let started = start.and_then(&stamp);
    let finished = end.and_then(&stamp);
    match (started, finished) {
        (Some(a), Some(b)) => format!("{a} ~ {b}"),
        // 아직 끝나지 않았다 — 시작만 적고 뒤를 비워 둔다
        (Some(a), None) => format!("{a} ~"),
        // 시작을 모르는데 끝만 아는 길은 없지만, 값이 그렇게 오면 아는 것만 적는다
        (None, Some(b)) => format!("~ {b}"),
        // 크기를 모를 때와 같은 글자를 쓴다 (`format_size`)
        (None, None) => UNKNOWN.to_owned(),
    }
}

/// 속도 표기 — 원본 `12.4 MB/s` (`:704`).
///
/// GB/s까지 올린다 — 로컬 네트워크·SSD 사이에서는 실제로 그 단위가 나오는데, MB/s에서 멈추면
/// `2048.0 MB/s` 같은 읽기 어려운 숫자가 된다 (plan T21 Edge Case)
pub fn format_speed(bytes_per_sec: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const KB: f64 = 1024.0;
    let value = bytes_per_sec as f64;
    if value >= GB {
        format!("{:.1} GB/s", value / GB)
    } else if value >= MB {
        format!("{:.1} MB/s", value / MB)
    } else if value >= KB {
        format!("{:.1} KB/s", value / KB)
    } else {
        format!("{bytes_per_sec} B/s")
    }
}

/// 상태 열의 문구와 색 (인벤토리 #45~#48).
///
/// 실패는 **서버가 준 사유를 그대로** 보인다 — 우리가 다듬으면 사용자가 서버 관리자에게
/// 전할 말이 사라진다
pub fn state_text(item: &TransferItem, retry_limit: u32) -> (String, egui::Color32) {
    // 자동 재시도로 다시 도는 중이면 몇 번째인지 붙인다 (FR-37) — 붙이지 않으면 사용자는
    // 같은 전송이 왜 두 번 도는지 알 수 없다. `attempts`는 **실패한** 횟수이므로
    // 지금 도는 것은 그다음 시도다
    let retry = if item.attempts > 0 && item.state.is_pending() {
        format!(
            " · {}",
            crate::i18n::dynamic::transfer_retrying(item.attempts, retry_limit)
        )
    } else {
        String::new()
    };
    match &item.state {
        TransferState::Wait => (
            format!("{}{retry}", crate::i18n::queue_state_pending()),
            theme::TEXT_MUTED,
        ),
        TransferState::Active { speed, .. } => {
            let speed = if *speed > 0 {
                format!(" · {}", format_speed(*speed))
            } else {
                String::new()
            };
            (
                format!("{}{speed}{retry}", crate::i18n::queue_state_active()),
                theme::ACCENT,
            )
        }
        TransferState::Done => (crate::i18n::queue_state_done().to_owned(), theme::OK_TEXT),
        TransferState::Error { message } => (message.clone(), theme::ERROR),
        // 사용자가 그만둔 것은 실패와 색을 나눈다 — 서버가 거부한 것이 아니라 우리가 멈춘 것이다
        TransferState::Cancelled => (
            crate::i18n::queue_state_cancelled().to_owned(),
            theme::TEXT_MUTED,
        ),
    }
}

/// `이유` 열의 글과 색 — **끝내지 못한 사유만 적는다** (실패 탭).
///
/// 이 열은 실패 탭에만 서므로 다른 상태가 올 일이 없지만, 와도 빈칸이 맞다(사유가 없는 것과
/// 「완료」는 다르다). **취소는 서버가 준 문자열이 없어 우리가 적고**, 서버가 거부한 것과
/// 구별되게 색도 가른다 — 같은 탭에 섞여 있으므로 한눈에 갈려야 한다
pub fn reason_text(state: &TransferState) -> (String, egui::Color32) {
    match state {
        TransferState::Error { message } => (message.clone(), theme::ERROR),
        TransferState::Cancelled => (
            crate::i18n::queue_reason_user_cancelled().to_owned(),
            theme::TEXT_MUTED,
        ),
        _ => (String::new(), theme::ERROR),
    }
}

/// 진행 막대 채움 색 — 상태별로 갈린다 (`:701`)
fn bar_color(state: &TransferState) -> egui::Color32 {
    match state {
        TransferState::Error { .. } => theme::ERROR,
        TransferState::Done => theme::OK_BAR,
        _ => theme::ACCENT,
    }
}

/// 연결별 탭의 점 색 — **실패한 연결만 빨강**이고 그 밖(연결 중·연결됨·연결 없음)은 초록이다.
///
/// 원본 `:728`이 `phase === "error"`만 빨강으로 가른다. 연결 객체의 유무로 가르면 실패한
/// 사이트가 초록으로, 정상 종료한 사이트가 빨강으로 뒤집힌다
fn site_dot_color(failed: &[SiteId], site: SiteId) -> egui::Color32 {
    if failed.contains(&site) {
        theme::ERROR
    } else {
        theme::OK_DOT
    }
}

/// 지금 화면에 보일 항목들 — 필터와 연결별 탭을 함께 적용한다.
///
/// 둘을 한자리에서 거르는 이유: 화면·건수·빈 상태 판정이 전부 같은 목록을 봐야 한다
pub fn visible_items(
    queue: &crate::remote::queue::TransferQueue,
    filter: QueueFilter,
    site: Option<SiteId>,
) -> Vec<&TransferItem> {
    queue
        .filter(filter)
        .into_iter()
        .filter(|item| site.is_none_or(|site| item.site == site))
        .collect()
}

/// 클릭 하나가 선택을 어떻게 바꾸는가 — **파일 목록과 같은 규칙**이다
/// (`ui::file_list::select` — `Shift`는 기준점부터 범위, `Ctrl`은 토글, 맨클릭은 단독).
///
/// **상태를 고치지 않고 값을 돌려준다** — 이 모듈은 큐도 도크 상태도 직접 쓰지 않으며,
/// 대입은 `show_queue` 한 자리에서 한다. 기준점을 자리 번호가 아니라 `TransferId`로 드는
/// 이유는 목록이 프레임마다 다시 걸러지기 때문이다(번호로 들면 거르개가 바뀔 때 어긋난다).
pub fn select_rows(
    selection: &HashSet<TransferId>,
    anchor: Option<TransferId>,
    items: &[&TransferItem],
    index: usize,
    modifiers: egui::Modifiers,
) -> (HashSet<TransferId>, Option<TransferId>) {
    let Some(clicked) = items.get(index).map(|item| item.id) else {
        return (selection.clone(), anchor);
    };
    // 기준점이 지금 목록에 없으면 범위를 지을 수 없다 — 단독 선택으로 떨어진다
    let anchor_index = anchor.and_then(|id| items.iter().position(|item| item.id == id));
    if modifiers.shift
        && let Some(from) = anchor_index
    {
        let (lo, hi) = if from <= index {
            (from, index)
        } else {
            (index, from)
        };
        // **기준점은 그대로 둔다** — 파일 목록과 같게, 범위를 늘렸다 줄였다 할 수 있어야 한다
        return (items[lo..=hi].iter().map(|item| item.id).collect(), anchor);
    }
    let mut next = selection.clone();
    if modifiers.ctrl {
        if !next.remove(&clicked) {
            next.insert(clicked);
        }
    } else {
        next.clear();
        next.insert(clicked);
    }
    (next, Some(clicked))
}

/// 보이는 목록에 없는 번호를 걷어낸다 — 지워지거나 걸러진 항목이 선택에 남지 않게 한다.
///
/// **되살리지 않는다** — 파일 목록은 갱신 뒤 이름으로 선택을 복원하지만(`matching_selection`),
/// 큐는 번호가 안정적이라 사라진 것은 정말 사라진 것이다
pub fn prune_selection(
    selection: &HashSet<TransferId>,
    anchor: Option<TransferId>,
    items: &[&TransferItem],
) -> (HashSet<TransferId>, Option<TransferId>) {
    let alive: HashSet<TransferId> = items.iter().map(|item| item.id).collect();
    let kept = selection.intersection(&alive).copied().collect();
    let anchor = anchor.filter(|id| alive.contains(id));
    (kept, anchor)
}

/// 이 행을 우클릭했을 때 **이번 프레임의 대상**은 무엇인가 — 고른 것 안이면 선택 전체,
/// 밖이면 그 행 하나다.
///
/// 이 계산이 따로 있는 이유: 메뉴(`Popup::context_menu`)는 우클릭한 **그 프레임에 바로**
/// 그려지는데 선택 대입은 행을 다 그린 뒤에 오므로, 그 프레임의 메뉴는 아직 옛 선택을 본다.
/// 상태를 고치지 않고 값으로만 답해 대입 자리가 늘지 않게 한다.
///
/// **항목 자체를 돌려준다** — 부르는 쪽이 번호와 상태를 둘 다 쓰는데, 번호만 주면 그것으로
/// 상태를 다시 찾느라 목록을 한 번 더 훑게 된다. 차례는 **보이는 목록의 차례**다(집합을
/// 그대로 쓰면 조작 대상의 순서가 실행할 때마다 달라진다).
pub fn effective_selection<'a>(
    selection: &HashSet<TransferId>,
    item: &'a TransferItem,
    items: &[&'a TransferItem],
) -> Vec<&'a TransferItem> {
    if selection.contains(&item.id) {
        items
            .iter()
            .copied()
            .filter(|item| selection.contains(&item.id))
            .collect()
    } else {
        vec![item]
    }
}

/// 큐 표의 열 폭 — **탭마다 한 벌씩** 든다 (FR-36).
///
/// 각 벌은 그 탭의 열 수만큼이고 모두 고정 폭이며, 합이 표 폭보다 좁을 때만 마지막 열이
/// 그 차이를 표시 폭으로 흡수한다 (plan D6 · `list_details::Columns`와 같은 규칙).
///
/// **탭마다 따로 두는 이유**: 열 구성이 달라 한 벌로는 자리가 어긋난다(성공 탭 6열의 넷째를
/// 끌었는데 실패 탭 7열의 넷째가 함께 움직이면 사용자가 맞춰 둔 화면이 탭을 옮길 때마다
/// 흔들린다). 넘칠 때는 저장 폭 그대로 그려 오른쪽이 잘린다 — 가로 스크롤은 두지 않는다
#[derive(Debug, Clone, PartialEq)]
pub struct QueueColumns {
    all: Vec<f32>,
    done: Vec<f32>,
    error: Vec<f32>,
}

impl Default for QueueColumns {
    fn default() -> QueueColumns {
        QueueColumns {
            all: default_widths(QueueFilter::All),
            done: default_widths(QueueFilter::Done),
            error: default_widths(QueueFilter::Error),
        }
    }
}

/// 그 탭의 기본 폭 한 벌
fn default_widths(filter: QueueFilter) -> Vec<f32> {
    columns_for(filter)
        .iter()
        .map(|kind| kind.default_width())
        .collect()
}

/// 그 열이 줄 수 있는 하한 — 대개 `MIN_COL_WIDTH`(40px)지만, **기본 폭이 그보다 좁은 열**
/// (`방향` 34px)은 그 기본값이 하한이다.
///
/// 하한을 일괄로 40px에 맞추면 저장했다 되살릴 때 `방향`이 34 → 40으로 넓어져
/// **사용자가 맞춰 둔 화면이 그대로 돌아오지 않는다**(2026-08-18 시험이 잡았다)
fn min_column_width(kind: QueueColumnKind) -> f32 {
    let floor = crate::ui::list_details::MIN_COL_WIDTH;
    kind.default_width().min(floor)
}

impl QueueColumns {
    /// 저장된 폭으로 되살린다 (FR-11 세션 복원).
    ///
    /// **앞에서부터 있는 만큼만 받는다** — 열 수가 달라진 옛 세션이 와도 나머지는 기본값이다.
    /// 옛 파일에는 `전송 큐` 탭 몫(일곱)만 있고 나머지 두 벌은 비어 있으므로 기본값이 선다.
    /// 유한하지 않은 값은 그 자리만 되돌린다(설정 파일이 손상돼도 표를 못 그리지 않게)
    pub fn from_saved(all: &[f32], done: &[f32], error: &[f32]) -> QueueColumns {
        QueueColumns {
            all: restore_widths(QueueFilter::All, all),
            done: restore_widths(QueueFilter::Done, done),
            error: restore_widths(QueueFilter::Error, error),
        }
    }

    /// 세션에 저장할 폭 — 그 탭 몫 한 벌
    pub fn to_saved(&self, filter: QueueFilter) -> Vec<f32> {
        self.widths(filter).to_vec()
    }

    /// 그 탭의 저장 폭
    fn widths(&self, filter: QueueFilter) -> &[f32] {
        match filter {
            QueueFilter::All => &self.all,
            QueueFilter::Done => &self.done,
            QueueFilter::Error => &self.error,
        }
    }

    fn widths_mut(&mut self, filter: QueueFilter) -> &mut Vec<f32> {
        match filter {
            QueueFilter::All => &mut self.all,
            QueueFilter::Done => &mut self.done,
            QueueFilter::Error => &mut self.error,
        }
    }

    /// 실제로 그릴 폭. 합이 표 폭보다 좁으면 **마지막 열만 늘려** 오른쪽 빈틈을 없앤다.
    /// 늘리는 것은 표시뿐이며 저장 폭은 그대로다 — 창 크기를 바꿀 때마다 사용자가 정한
    /// 폭이 덮어써지면 안 된다
    fn effective(&self, filter: QueueFilter, total: f32) -> Vec<f32> {
        let mut widths = self.widths(filter).to_vec();
        let slack = total - widths.iter().sum::<f32>();
        if slack > 0.0
            && let Some(last) = widths.last_mut()
        {
            *last += slack;
        }
        widths
    }

    /// 경계 드래그 — 그 **왼쪽 열**의 폭을 바꾼다. 최소 폭 아래로는 줄지 않는다.
    ///
    /// 마지막 열의 오른쪽에는 핸들이 없어 그 열의 저장 폭은 여기서 바뀌지 않는다
    fn apply_drag(&mut self, filter: QueueFilter, slot: usize, delta: f32) {
        let Some(kind) = columns_for(filter).get(slot).copied() else {
            return;
        };
        let floor = min_column_width(kind);
        if let Some(width) = self.widths_mut(filter).get_mut(slot) {
            *width = (*width + delta).max(floor);
        }
    }
}

/// 저장된 한 벌을 그 탭의 열 수에 맞춰 되살린다
fn restore_widths(filter: QueueFilter, saved: &[f32]) -> Vec<f32> {
    let kinds = columns_for(filter);
    let mut widths = default_widths(filter);
    for (slot, (width, &value)) in widths.iter_mut().zip(saved).enumerate() {
        if value.is_finite()
            && let Some(kind) = kinds.get(slot).copied()
        {
            *width = value.max(min_column_width(kind));
        }
    }
    widths
}

/// 큐 표를 그린다 (인벤토리 #35~#48)
pub fn show_queue(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    state: &mut DockState,
    view: &DockView<'_>,
    sites: &SiteStore,
    today: Option<LocalTime>,
    retry_limit: u32,
) -> Option<QueueAction> {
    let site_row = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), SITE_ROW_HEIGHT));
    show_site_tabs(ui, site_row, state, view, sites, true);

    let header = egui::Rect::from_min_size(
        egui::pos2(rect.left(), site_row.bottom()),
        egui::vec2(rect.width(), HEADER_HEIGHT),
    );
    // **열 구성은 지금 고른 탭이 정한다** (FR-36) — `DockState`가 이미 그 값을 들고 있어
    // 화면이 따로 탭 종류를 알 필요가 없다
    let filter = state.filter;
    let kinds = columns_for(filter);
    let widths = state.columns.effective(filter, rect.width());
    let guide_x = show_header(ui, header, kinds, &widths, &mut state.columns, filter);

    let body = egui::Rect::from_min_max(
        egui::pos2(rect.left(), header.bottom()),
        egui::pos2(rect.right(), rect.bottom()),
    );
    // 가이드 선은 행을 다 그린 뒤에 긋는다 (`show_header` 주석) — 빈 목록 갈래에서도 그어야
    // 끌던 손이 허공에 뜨지 않으므로, 그리는 자리를 닫는 헬퍼로 둔다
    let draw_guide = |ui: &egui::Ui, bottom: f32| {
        if let Some(x) = guide_x {
            ui.painter().vline(
                x,
                header.top()..=bottom,
                egui::Stroke::new(1.0, theme::ACCENT),
            );
        }
    };
    let items = visible_items(view.queue, state.filter, state.site);
    // **죄기는 빈 목록 갈래보다 앞이다** — 뒤에 두면 목록이 통째로 빈 프레임에 돌지 않아
    // 고른 번호가 사라진 항목을 가리킨 채 남는다
    let (kept, anchor) = prune_selection(&state.queue_selection, state.queue_anchor, &items);
    state.queue_selection = kept;
    state.queue_anchor = anchor;
    // 보일 것이 없으면 그 사실을 적는다 (2026-08-16 검토) — 머리글만 남은 표는
    // 아직 아무것도 안 한 것인지 거른 결과가 없는 것인지 알려 주지 않는다
    if items.is_empty() {
        ui.painter().text(
            egui::pos2(body.center().x, body.top() + EMPTY_HINT_TOP),
            egui::Align2::CENTER_TOP,
            crate::i18n::queue_empty(),
            egui::FontId::proportional(FONT_PX),
            theme::TEXT_MUTED,
        );
        draw_guide(ui, body.bottom());
        return None;
    }
    // `전체 다시 시도`를 낼지 — 보이는 목록 전체를 본다(화면 밖 행도 대상이므로
    // 그려지는 범위가 아니라 거른 목록 전량에서 판정한다). **취소분도 대상이다** —
    // `retry`가 그것을 되살리므로 취소만 있는 목록에서도 눌러 일이 일어난다
    let has_retryable_in_view = items.iter().any(|item| item.state.is_retryable());
    let row_ctx = RowContext {
        kinds,
        widths: &widths,
        sites,
        selection: &state.queue_selection,
        items: &items,
        today,
        has_retryable_in_view,
        retry_limit,
    };
    let mut action = None;
    let mut click = None;
    let mut secondary_on = None;
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(body)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(body);
    // 1만 건에서도 보이는 만큼만 그린다 (plan Edge Case)
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(&mut child, ROW_HEIGHT, items.len(), |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for index in range {
                if let Some(item) = items.get(index) {
                    let outcome = show_row(ui, item, index, &row_ctx);
                    if let Some(picked) = outcome.action {
                        action = Some(picked);
                    }
                    if let Some(picked) = outcome.click {
                        click = Some(picked);
                    }
                    if let Some(picked) = outcome.secondary_on {
                        secondary_on = Some(picked);
                    }
                }
            }
        });
    // **고른 것을 바꾸는 자리는 여기 하나다** — 행은 관측만 하고 값으로 올린다.
    // 선택 밖의 행을 우클릭하면 그 행을 단독 선택한다 (파일 목록과 같은 규칙 —
    // `ui::list_details`).
    //
    // **메뉴는 이 대입을 기다리지 않는다** — `context_menu`는 우클릭한 그 프레임에 바로
    // 그려지는데 이 대입은 행을 다 그린 뒤에 오므로, 그 프레임의 메뉴는 옛 선택을 보게 된다.
    // 그래서 행이 `effective_selection`으로 그 프레임의 대상을 국소로 계산한다
    if let Some(index) = secondary_on
        && let Some(item) = items.get(index)
        && !state.queue_selection.contains(&item.id)
    {
        state.queue_selection = std::iter::once(item.id).collect();
        state.queue_anchor = Some(item.id);
    }
    if let Some((index, modifiers)) = click {
        let (next, anchor) = select_rows(
            &state.queue_selection,
            state.queue_anchor,
            &items,
            index,
            modifiers,
        );
        state.queue_selection = next;
        state.queue_anchor = anchor;
    }
    draw_guide(ui, body.bottom());
    action
}

/// 연결별 탭 한 줄 (인벤토리 #35·#36) — **큐와 로그가 함께 쓴다**(도크에 줄은 하나다).
///
/// `show_counts`가 꺼지면 이름만 적는다 — 로그 화면에는 셀 대상이 없어 `(N)`이 붙으면
/// 그 수가 무엇의 개수인지 알 수 없다 (2026-08-18 사용자 결정)
pub fn show_site_tabs(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    state: &mut DockState,
    view: &DockView<'_>,
    sites: &SiteStore,
    show_counts: bool,
) {
    ui.painter().rect_filled(rect, 0.0, theme::SURFACE_BG);
    ui.painter().line_segment(
        [
            egui::pos2(rect.left(), rect.bottom() - 0.5),
            egui::pos2(rect.right(), rect.bottom() - 0.5),
        ],
        egui::Stroke::new(1.0, theme::BORDER_SUBTLE),
    );

    // `전체` 다음에 **큐에 항목이 있거나 지금 연결된** 사이트들이 온다.
    // 원본은 큐에서만 이름을 모으지만(`:722`), 그러면 연결만 하고 아직 아무것도 옮기지 않은
    // 서버가 탭에 없어 고를 수 없다 (2026-08-05 사용자 보고)
    // **멤버십과 건수를 따로 센다** — 멤버십까지 거르면 그 거르개에 항목이 없는 서버가
    // 탭에서 사라져, `성공` 탭에서 실패만 있는 서버를 고를 수 없게 된다
    let members = view.queue.counts_by_site(QueueFilter::All);
    let counts = view.queue.counts_by_site(state.filter);
    let mut order: Vec<SiteId> = sites
        .sites()
        .iter()
        .map(|record| record.id)
        .filter(|id| members.contains_key(id) || view.connected.contains(id))
        .collect();
    // 저장소에 없는 사이트의 항목도 빠뜨리지 않는다(지운 사이트의 잔여 전송)
    let mut extra: Vec<SiteId> = members
        .keys()
        .copied()
        .chain(view.connected.iter().copied())
        .filter(|id| !order.contains(id))
        .collect();
    extra.dedup();
    extra.sort();
    order.append(&mut extra);

    let mut left = rect.left() + SITE_ROW_PAD_X;
    // 건수는 지금 고른 거르개를 따른다 — `전체` 탭도 큐 전량이 아니라 그 거르개의 수다
    let label_with_count = |name: &str, count: usize| {
        if show_counts {
            format!("{name} ({count})")
        } else {
            name.to_owned()
        }
    };
    let all_label = label_with_count(
        crate::i18n::queue_filter_all(),
        view.queue.count(state.filter),
    );
    let tabs: Vec<(Option<SiteId>, String)> = std::iter::once((None, all_label))
        .chain(order.into_iter().map(|id| {
            let name = sites
                .get(id)
                .map(|record| record.name.clone())
                .unwrap_or_else(|| crate::i18n::dynamic::queue_site_fallback(id.0));
            (
                Some(id),
                label_with_count(&name, *counts.get(&id).unwrap_or(&0)),
            )
        }))
        .collect();

    for (site, label) in tabs {
        // 색은 굽지 않고 그릴 때 정한다 — 구워 두면 아래 `galley`의 색이 무시된다
        // (도크 탭과 같은 함정. `list_common`의 주석 참고)
        let text = ui.painter().layout_no_wrap(
            label,
            egui::FontId::proportional(FONT_PX),
            egui::Color32::PLACEHOLDER,
        );
        // `전체`는 점이 없다 (인벤토리 #35) — 그만큼 폭도 줄어든다
        let dot_width = if site.is_some() {
            SITE_DOT + SITE_TAB_INNER_GAP
        } else {
            0.0
        };
        let width = SITE_TAB_PAD_X * 2.0 + dot_width + text.size().x;
        let tab = egui::Rect::from_min_size(
            egui::pos2(left, rect.top()),
            egui::vec2(width, SITE_ROW_HEIGHT),
        );
        left += width + SITE_TAB_GAP;
        let active = state.site == site;
        let response = ui.interact(
            tab,
            ui.id().with(("queue_site", site.map(|id| id.0))),
            egui::Sense::click(),
        );
        if active {
            ui.painter().rect_filled(tab, 0.0, theme::HEADER_BG);
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(tab.left(), tab.bottom() - SITE_TAB_EDGE),
                    egui::vec2(tab.width(), SITE_TAB_EDGE),
                ),
                0.0,
                theme::ACCENT,
            );
        }
        let mut text_left = tab.left() + SITE_TAB_PAD_X;
        if let Some(id) = site {
            ui.painter().circle_filled(
                egui::pos2(text_left + SITE_DOT / 2.0, tab.center().y),
                SITE_DOT / 2.0,
                site_dot_color(view.failed, id),
            );
            text_left += SITE_DOT + SITE_TAB_INNER_GAP;
        }
        ui.painter().galley(
            egui::pos2(text_left, tab.center().y - text.size().y / 2.0),
            text,
            if active {
                theme::TEXT_SELECTED
            } else if response.hovered() {
                theme::TEXT
            } else {
                theme::TEXT_MUTED
            },
        );
        if response.clicked() && state.site != site {
            // 도크 탭과 같은 규칙 — **바뀔 때만** 비운다. 보이는 목록이 통째로 갈리므로
            // 고른 것을 남겨 둘 뜻이 없다
            state.site = site;
            state.queue_selection.clear();
            state.queue_anchor = None;
        }
    }
}

/// 머리글 (인벤토리 #37~#43) — 열 경계에 구분선을 긋고 그 위에서 폭을 조절한다.
///
/// 돌려주는 값은 **지금 끌고 있는 경계의 x**다. 그 선은 여기서 긋지 않는다 —
/// 머리글이 본문 행보다 먼저 그려져 행 배경(`ROW_HOT`·얼룩)이 같은 레이어에서 덮어 버린다.
/// 호출부가 행을 다 그린 뒤에 긋는다
fn show_header(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    kinds: &[QueueColumnKind],
    widths: &[f32],
    columns: &mut QueueColumns,
    filter: QueueFilter,
) -> Option<f32> {
    ui.painter().rect_filled(rect, 0.0, theme::HEADER_BG);
    let mut left = rect.left();
    for (index, kind) in kinds.iter().enumerate() {
        ui.painter().text(
            egui::pos2(left + CELL_PAD_X, rect.center().y),
            egui::Align2::LEFT_CENTER,
            kind.header(),
            egui::FontId::proportional(FONT_PX),
            theme::HEADER_TEXT,
        );
        left += widths.get(index).copied().unwrap_or_default();
    }

    // 평소에도 경계가 보여야 어디를 잡을지 알 수 있다 (2026-08-18 사용자 보고).
    // 마지막 열의 오른쪽 끝에는 긋지 않는다 — 그것은 표 바깥 경계다
    let mut boundary = rect.left();
    let mut dragging = None;
    for (slot, width) in widths.iter().take(widths.len() - 1).enumerate() {
        boundary += width;
        ui.painter().vline(
            boundary,
            rect.top()..=rect.bottom(),
            egui::Stroke::new(1.0, theme::BORDER_SUBTLE),
        );
        let handle = egui::Rect::from_min_size(
            egui::pos2(boundary - HANDLE_WIDTH / 2.0, rect.top()),
            egui::vec2(HANDLE_WIDTH, rect.height()),
        );
        let response = ui.interact(
            handle,
            ui.id().with(("queue_col_handle", slot)),
            egui::Sense::click_and_drag(),
        );
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if response.dragged() {
            columns.apply_drag(filter, slot, response.drag_delta().x);
            dragging = Some(boundary);
        }
    }
    dragging
}

/// 항목 한 줄
/// 한 칸을 그린다 — **열 종류가 무엇을 적을지 정한다** (FR-36).
///
/// 자리 번호로 갈랐다면 탭마다 같은 번호가 다른 열을 가리켜, 성공 탭의 여섯째 칸에
/// 진행 막대가 그려지는 식으로 어긋난다
fn draw_cell(
    ui: &mut egui::Ui,
    kind: QueueColumnKind,
    at: egui::Rect,
    item: &TransferItem,
    sites: &SiteStore,
    today: Option<LocalTime>,
    retry_limit: u32,
) {
    /// 길면 끝을 줄여 그린다 (plan Edge Case) — 경로·사유가 칸을 넘는 것이 흔하다
    fn elided(ui: &egui::Ui, at: egui::Rect, text: String, color: egui::Color32) {
        // 색을 갤리에 구워 넣는다 — 그리면서 넘기는 색은 갤리가 기본색이면 무시된다(T20 리뷰)
        let galley = elided_galley_colored(
            ui.painter(),
            text,
            egui::FontId::proportional(FONT_PX),
            at.width(),
            color,
        );
        ui.painter().galley(
            egui::pos2(at.left(), at.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
    }

    match kind {
        QueueColumnKind::Direction => {
            let (glyph, glyph_color) = widgets::direction_mark(item.direction);
            ui.painter().text(
                egui::pos2(at.left(), at.center().y),
                egui::Align2::LEFT_CENTER,
                glyph,
                egui::FontId::proportional(FONT_PX),
                glyph_color,
            );
        }
        QueueColumnKind::Local => elided(
            ui,
            at,
            item.local.to_string_lossy().into_owned(),
            theme::HEADER_TEXT,
        ),
        QueueColumnKind::Remote => {
            elided(ui, at, item.remote.as_str().to_owned(), theme::HEADER_TEXT)
        }
        QueueColumnKind::Server => elided(
            ui,
            at,
            sites
                .get(item.site)
                .map(|record| record.name.clone())
                .unwrap_or_else(|| crate::i18n::dynamic::queue_site_fallback(item.site.0)),
            theme::TEXT_MUTED,
        ),
        QueueColumnKind::Size => {
            ui.painter().text(
                egui::pos2(at.left(), at.center().y),
                egui::Align2::LEFT_CENTER,
                format_size(item.size),
                egui::FontId::proportional(FONT_PX),
                theme::TEXT,
            );
        }
        QueueColumnKind::Progress => {
            let mut bar_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_size(
                        egui::pos2(at.left(), at.center().y - BAR_HEIGHT / 2.0),
                        egui::vec2(BAR_WIDTH, BAR_HEIGHT),
                    ))
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            widgets::progress_bar(
                &mut bar_ui,
                egui::vec2(BAR_WIDTH, BAR_HEIGHT),
                item.progress(),
                bar_color(&item.state),
            );
        }
        QueueColumnKind::State => {
            let (text, color) = state_text(item, retry_limit);
            elided(ui, at, text, color);
        }
        QueueColumnKind::Time => {
            // 지금 시각을 모르면(시계를 읽지 못했다) 오늘 여부를 가릴 수 없다 —
            // 그때는 날짜를 늘 붙이는 쪽이 안전하다(없는 날짜를 지어내지 않는다)
            let today = today.unwrap_or(LocalTime {
                year: 0,
                month: 0,
                day: 0,
                hour: 0,
                minute: 0,
                second: 0,
            });
            elided(
                ui,
                at,
                format_time_range(item.started_at, item.finished_at, today),
                theme::TEXT_MUTED,
            );
        }
        QueueColumnKind::Reason => {
            let (text, color) = reason_text(&item.state);
            elided(ui, at, text, color);
        }
    }
}

/// 행을 그릴 때 **행마다 달라지지 않는 것들** — 열 구성·폭·사이트 이름·오늘 판정.
///
/// 인자로 늘어놓으면 여덟이 되어 무엇이 무엇인지 부르는 자리에서 알아보기 어렵다
struct RowContext<'a> {
    kinds: &'a [QueueColumnKind],
    widths: &'a [f32],
    sites: &'a SiteStore,
    /// 지금 고른 전송들 — **읽기만 한다**. 행이 자기 배경을 칠할지 정하는 데 쓰고,
    /// 고치는 것은 `show_queue` 한 자리다
    selection: &'a HashSet<TransferId>,
    /// 지금 보이는 목록 — `effective_selection`이 조작 대상의 **차례**를 여기서 가져온다
    items: &'a [&'a TransferItem],
    /// 지금 로컬 시각 — 시간 열이 「오늘인가」를 가리는 데 쓴다. 시계를 읽지 못했으면 `None`
    today: Option<LocalTime>,
    /// 보이는 목록에 실패가 있는가 — 우클릭 메뉴의 `전체 다시 시도` 유무를 가른다
    has_retryable_in_view: bool,
    /// 설정된 자동 재시도 상한 — 상태 열의 `재시도 2/3`에서 뒤 숫자다 (FR-37)
    retry_limit: u32,
}

/// 한 행이 이번 프레임에 **관측한 것** — 상태를 고치지 않고 값으로 올린다.
///
/// 셋을 한 값에 담는 이유: 대입이 `show_queue` 한 자리에서만 일어나야 하는데(그래야 클릭·
/// 우클릭·메뉴가 서로 다른 순서로 상태를 건드리지 않는다), 반환을 나누면 호출부가 세 갈래를
/// 따로 엮게 된다. 선례는 `ui::list_details`의 `outcome.select_request`·`outcome.action`이다
#[derive(Debug, Default)]
struct RowOutcome {
    /// 왼쪽 버튼으로 누른 자리와 그때의 보조키 — `select_rows`가 받는다
    click: Option<(usize, egui::Modifiers)>,
    /// 오른쪽 버튼으로 누른 자리 — 선택 밖이면 그 행을 단독 선택한다 (파일 목록과 같은 규칙)
    secondary_on: Option<usize>,
    /// 메뉴에서 고른 조작
    action: Option<QueueAction>,
}

fn show_row(
    ui: &mut egui::Ui,
    item: &TransferItem,
    index: usize,
    ctx: &RowContext<'_>,
) -> RowOutcome {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_HEIGHT),
        egui::Sense::click(),
    );
    // 칠하는 차례는 파일 목록과 같다 (`ui::list_details`) — 얼룩 → 선택 → hover이고
    // **선택이 hover를 이긴다**. 색도 그쪽과 같은 것을 쓴다(고른 행이 화면마다 다르면 안 된다)
    if ctx.selection.contains(&item.id) {
        ui.painter()
            .rect_filled(rect, 0.0, ui.visuals().selection.bg_fill);
    } else if response.hovered() {
        ui.painter().rect_filled(rect, 0.0, theme::ROW_HOT);
    } else if stripe(index) {
        ui.painter().rect_filled(rect, 0.0, theme::HEADER_BG);
    }

    let mut left = rect.left();
    for (slot, kind) in ctx.kinds.iter().enumerate() {
        let width = ctx.widths.get(slot).copied().unwrap_or_default();
        let at = egui::Rect::from_min_size(
            egui::pos2(left + CELL_PAD_X, rect.top()),
            egui::vec2((width - CELL_PAD_X * 2.0).max(0.0), rect.height()),
        );
        left += width;
        draw_cell(ui, *kind, at, item, ctx.sites, ctx.today, ctx.retry_limit);
    }

    let mut outcome = RowOutcome::default();
    if response.clicked() {
        outcome.click = Some((index, ui.input(|i| i.modifiers)));
    }
    if response.secondary_clicked() {
        outcome.secondary_on = Some(index);
    }

    // 항목을 다시 걸거나 지우는 길 — 디자인에 진입점이 없어 우클릭으로 둔다.
    // **대상은 고른 것 전부다** — 선택 밖의 행이면 그 행 하나다
    let mut action = None;
    response.context_menu(|ui| {
        theme::menu_style(ui);
        // **대상 계산은 메뉴가 열린 이 안에서만 돈다** — 밖에 두면 보이는 행마다 매 프레임
        // 목록을 훑게 되어, 수천 건을 골라 둔 채 스크롤하면 그리기가 밀린다
        let targets = effective_selection(ctx.selection, item, ctx.items);
        let ids: Vec<TransferId> = targets.iter().map(|item| item.id).collect();
        let states: Vec<TransferState> = targets.iter().map(|item| item.state.clone()).collect();
        for entry in row_menu_items(&states, ctx.has_retryable_in_view) {
            let (label, picked) = match entry {
                RowMenuItem::Retry => (crate::i18n::queue_retry(), QueueAction::Retry(ids.clone())),
                RowMenuItem::RetryAll => (crate::i18n::queue_retry_all(), QueueAction::RetryAll),
                RowMenuItem::Cancel => (
                    crate::i18n::queue_cancel(),
                    QueueAction::Cancel(ids.clone()),
                ),
                RowMenuItem::Remove => (
                    crate::i18n::queue_remove(),
                    QueueAction::Remove(ids.clone()),
                ),
                RowMenuItem::RemoveAll => (crate::i18n::queue_remove_all(), QueueAction::RemoveAll),
            };
            if ui.button(label).clicked() {
                action = Some(picked);
                ui.close();
            }
        }
    });
    outcome.action = action;
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::connection::TransferDirection;
    use crate::remote::queue::TransferQueue;
    use crate::remote::types::RemotePath;
    use std::path::PathBuf;

    #[test]
    fn 열_저장_키가_왕복한다() {
        // 키가 갈리지 않으면 두 열이 같은 자리를 가리켜 순서·숨김이 뒤섞인다
        let mut keys: Vec<&str> = ALL_QUEUE_COLUMNS.iter().map(|k| k.as_key()).collect();
        keys.sort_unstable();
        let 원래수 = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), 원래수, "키가 겹치는 열이 있다");

        for kind in ALL_QUEUE_COLUMNS {
            assert_eq!(
                QueueColumnKind::from_key(kind.as_key()),
                Some(kind),
                "{}이 왕복하지 않는다",
                kind.as_key()
            );
        }
        assert_eq!(QueueColumnKind::from_key("없는열"), None);
        assert_eq!(QueueColumnKind::from_key(""), None);
    }

    #[test]
    fn 방향_로컬_원격만_끌_수_없다() {
        // 그 셋이 빠지면 「무엇을 어디서 어디로」가 사라진다 (2026-09-07 사용자 결정)
        let 고정: Vec<QueueColumnKind> = ALL_QUEUE_COLUMNS
            .into_iter()
            .filter(|k| k.is_fixed())
            .collect();
        assert_eq!(
            고정,
            vec![
                QueueColumnKind::Direction,
                QueueColumnKind::Local,
                QueueColumnKind::Remote
            ]
        );
    }

    #[test]
    fn 열_자리가_전체_목록의_차례와_같다() {
        // `slot()`이 폭 배열의 색인이고 `ALL_QUEUE_COLUMNS`가 그 길이를 정한다 —
        // 둘이 어긋나면 폭이 남의 열로 간다
        for (index, kind) in ALL_QUEUE_COLUMNS.into_iter().enumerate() {
            assert_eq!(kind.slot(), index, "{}의 자리가 어긋난다", kind.as_key());
        }
        assert_eq!(ALL_QUEUE_COLUMNS.len(), QUEUE_COLUMN_COUNT);
    }

    #[test]
    fn 탭별_열은_전부_전체_목록에_있다() {
        // 탭 구성에 전체 목록 밖의 열이 들어오면 그 열은 폭도 저장 자리도 갖지 못한다
        for filter in [QueueFilter::All, QueueFilter::Done, QueueFilter::Error] {
            for kind in columns_for(filter) {
                assert!(
                    ALL_QUEUE_COLUMNS.contains(kind),
                    "{}이 전체 목록에 없다",
                    kind.as_key()
                );
            }
        }
    }

    #[test]
    fn 표_치수는_원본과_같다() {
        // Acceptance ①·② — 머리글 22px·행 24px·열 폭 `34/1fr/300/120/84/118/150`
        assert_eq!(SITE_ROW_HEIGHT, 28.0);
        assert_eq!(HEADER_HEIGHT, 22.0);
        assert_eq!(ROW_HEIGHT, 24.0);
        assert_eq!(BAR_WIDTH, 110.0);
        assert_eq!(BAR_HEIGHT, 6.0);
        // `전송 큐` 탭의 일곱 열이 원본 폭 그대로다 (T4가 탭별로 갈랐어도 이 탭은 무변경)
        let all = default_widths(QueueFilter::All);
        assert_eq!(all[0], 34.0);
        assert_eq!(&all[2..], &[300.0, 120.0, 84.0, 118.0, 150.0]);
        // 원본의 `1fr` 자리만 고정값이 됐다 — 합이 기본 창 폭(1100px)보다 좁아야
        // 흡수가 실제로 돈다 (plan D6)
        assert_eq!(all[1], 280.0);
        assert_eq!(all.iter().sum::<f32>(), 1086.0);
    }

    #[test]
    fn 남는_자리는_마지막_열이_갖는다() {
        // plan D6 — 흡수 열이 앞자리면 그 오른쪽 경계가 손을 따라오지 않아
        // 폭 조절 자체가 성립하지 않는다. 그래서 `상태`(마지막)가 잔여를 먹는다
        let columns = QueueColumns::default();
        let all = default_widths(QueueFilter::All);
        let widths = columns.effective(QueueFilter::All, 1200.0);
        assert_eq!(widths.iter().sum::<f32>(), 1200.0);
        assert_eq!(widths[6], 150.0 + (1200.0 - 1086.0));
        assert_eq!(&widths[..6], &all[..6], "앞 여섯 열은 그대로다");

        // 합이 표 폭을 넘으면 저장 폭 그대로 그리고 오른쪽이 잘린다(가로 스크롤 없음)
        let widths = columns.effective(QueueFilter::All, 800.0);
        assert_eq!(widths, all);

        // 탭마다 마지막 열이 다르다 — 성공은 `시간`, 실패는 `이유`가 잔여를 먹는다
        let done = columns.effective(QueueFilter::Done, 2000.0);
        assert_eq!(done.len(), 6);
        assert_eq!(done.iter().sum::<f32>(), 2000.0);
        let error = columns.effective(QueueFilter::Error, 2000.0);
        assert_eq!(error.len(), 7);
        assert_eq!(error.iter().sum::<f32>(), 2000.0);
    }

    #[test]
    fn 머리글_열_경계마다_구분선이_선다() {
        // 2026-08-18 사용자 보고 — 선이 없어 어디를 끌어야 할지 알 수 없었다.
        // 파일 목록(`list_details`)과 같은 규칙이라 같은 방식으로 잰다
        let widths = [34.0f32, 280.0, 300.0, 120.0, 84.0, 118.0, 150.0];
        let mut columns = QueueColumns::default();
        let ctx = egui::Context::default();
        let output = ctx.run_ui(Default::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(widths.iter().sum(), HEADER_HEIGHT),
                );
                show_header(
                    ui,
                    rect,
                    columns_for(QueueFilter::All),
                    &widths,
                    &mut columns,
                    QueueFilter::All,
                );
            });
        });
        let mut 세로선 = Vec::new();
        for clipped in &output.shapes {
            if let egui::Shape::LineSegment { points, stroke } = &clipped.shape
                && (points[0].x - points[1].x).abs() < 0.01
                && stroke.color == theme::BORDER_SUBTLE
            {
                세로선.push(points[0].x);
            }
        }
        // 일곱 열이면 선은 여섯이다 — **마지막 열의 오른쪽 끝에는 긋지 않는다**
        let mut 기대 = Vec::new();
        let mut acc = 0.0;
        for width in &widths[..widths.len() - 1] {
            acc += width;
            기대.push(acc);
        }
        assert_eq!(세로선, 기대);

        // 끌고 있지 않으면 강조선(가이드)은 없다 — 그 선은 행을 다 그린 뒤 호출부가 긋는다
        assert!(
            !output.shapes.iter().any(|clipped| matches!(
                &clipped.shape,
                egui::Shape::LineSegment { stroke, .. } if stroke.color == theme::ACCENT
            )),
            "끌지 않았는데 가이드가 그려졌다"
        );
    }

    #[test]
    fn 경계_드래그는_왼쪽_열을_바꾸고_하한을_지킨다() {
        // plan D6 — 경계 k는 열 k−1의 폭을 바꾼다. 마지막 열은 핸들이 없어 여기 오지 않는다
        let mut columns = QueueColumns::default();
        columns.apply_drag(QueueFilter::All, 4, 30.0);
        assert_eq!(columns.effective(QueueFilter::All, 2000.0)[4], 84.0 + 30.0);

        // 최소 폭 아래로는 줄지 않는다
        columns.apply_drag(QueueFilter::All, 4, -1000.0);
        assert_eq!(
            columns.effective(QueueFilter::All, 2000.0)[4],
            crate::ui::list_details::MIN_COL_WIDTH
        );

        // **기본이 하한보다 좁은 열은 그 기본값이 하한**이다 — `방향`(34px)을 40px로 올리면
        // 저장했다 되살릴 때 화면이 넓어진다
        columns.apply_drag(QueueFilter::All, 0, -1000.0);
        assert_eq!(columns.effective(QueueFilter::All, 2000.0)[0], 34.0);
    }

    #[test]
    fn 성공과_실패_탭은_진행률과_상태_대신_시간을_세운다() {
        // 요청 — 끝난 것만 모이는 탭이라 진행률은 언제나 가득이고 상태는 언제나 같다.
        // 그 두 칸을 빼고 **언제 오갔는지**를 적는다 (FR-36)
        let 종류 = |filter| columns_for(filter).to_vec();

        assert_eq!(
            종류(QueueFilter::All),
            vec![
                QueueColumnKind::Direction,
                QueueColumnKind::Local,
                QueueColumnKind::Remote,
                QueueColumnKind::Server,
                QueueColumnKind::Size,
                QueueColumnKind::Progress,
                QueueColumnKind::State,
            ],
            "`전송 큐` 탭은 종전 일곱 열 그대로여야 한다"
        );
        assert_eq!(
            종류(QueueFilter::Done),
            vec![
                QueueColumnKind::Direction,
                QueueColumnKind::Local,
                QueueColumnKind::Remote,
                QueueColumnKind::Server,
                QueueColumnKind::Size,
                QueueColumnKind::Time,
            ]
        );
        assert_eq!(
            종류(QueueFilter::Error),
            vec![
                QueueColumnKind::Direction,
                QueueColumnKind::Local,
                QueueColumnKind::Remote,
                QueueColumnKind::Server,
                QueueColumnKind::Size,
                QueueColumnKind::Time,
                QueueColumnKind::Reason,
            ]
        );

        // 진행률·상태가 두 탭 어디에도 없어야 한다
        for filter in [QueueFilter::Done, QueueFilter::Error] {
            for 빠진_것 in [QueueColumnKind::Progress, QueueColumnKind::State] {
                assert!(
                    !종류(filter).contains(&빠진_것),
                    "{filter:?} 탭에 {빠진_것:?}이 남아 있다"
                );
            }
        }
        // `이유`는 실패 탭에만 선다 — 성공한 것에는 적을 사유가 없다
        assert!(종류(QueueFilter::Error).contains(&QueueColumnKind::Reason));
        assert!(!종류(QueueFilter::Done).contains(&QueueColumnKind::Reason));
        assert!(!종류(QueueFilter::All).contains(&QueueColumnKind::Reason));

        // 앞 다섯은 세 탭이 함께 쓴다 — 한 탭만 순서가 어긋나면 같은 자리가 다른 뜻이 된다
        let 공통 = 종류(QueueFilter::All)[..5].to_vec();
        for filter in [QueueFilter::Done, QueueFilter::Error] {
            assert_eq!(
                종류(filter)[..5],
                공통[..],
                "{filter:?} 탭의 앞 다섯이 어긋났다"
            );
        }
    }

    #[test]
    fn 실패_탭의_이유_칸에_서버_사유가_그대로_적힌다() {
        // 종전에는 그 사유가 `상태` 열에 있었다 — 열이 옮겨져도 문구는 서버가 준 원문 그대로다
        let ctx = egui::Context::default();
        let mut sites = SiteStore::new();
        let site = sites.add("웹서버");
        let mut queue = TransferQueue::new();
        let id = queue.enqueue(
            site,
            TransferDirection::Upload,
            PathBuf::from(r"C:\보고서.txt"),
            RemotePath::new("/pub/보고서.txt"),
            100,
        );
        queue.update(
            id,
            TransferState::Error {
                message: "550 권한이 없습니다".to_owned(),
            },
        );
        let item = queue.get(id).expect("항목");

        let output = ctx.run_ui(Default::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let at = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 24.0));
                draw_cell(ui, QueueColumnKind::Reason, at, item, &sites, None, 3);
            });
        });
        let 글 = 셰이프_글자(&output.shapes);
        assert!(
            글.iter().any(|text| text.contains("550 권한이 없습니다")),
            "이유 칸에 서버 사유가 없다: {글:?}"
        );
    }

    /// 그린 셰이프에서 글자만 거둔다 (이 파일의 다른 시험과 같은 방식)
    fn 셰이프_글자(shapes: &[egui::epaint::ClippedShape]) -> Vec<String> {
        fn 모은다(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
                egui::Shape::Vec(list) => {
                    for inner in list {
                        모은다(inner, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in shapes {
            모은다(&clipped.shape, &mut out);
        }
        out
    }

    #[test]
    fn 탭마다_열_폭을_따로_기억한다() {
        // 열 구성이 달라 한 벌로는 자리가 어긋난다 — 성공 탭 여섯째를 끌었는데
        // 실패 탭 여섯째가 함께 움직이면 탭을 옮길 때마다 화면이 흔들린다
        let mut columns = QueueColumns::default();
        let 원래_전송큐 = columns.to_saved(QueueFilter::All);
        let 원래_실패 = columns.to_saved(QueueFilter::Error);

        columns.apply_drag(QueueFilter::Done, 1, 50.0);

        assert_eq!(
            columns.to_saved(QueueFilter::Done)[1],
            280.0 + 50.0,
            "끈 탭의 폭이 바뀌어야 한다"
        );
        assert_eq!(
            columns.to_saved(QueueFilter::All),
            원래_전송큐,
            "다른 탭의 폭이 함께 움직였다"
        );
        assert_eq!(
            columns.to_saved(QueueFilter::Error),
            원래_실패,
            "다른 탭의 폭이 함께 움직였다"
        );
    }

    #[test]
    fn 열_폭이_세션을_왕복한다() {
        // FR-11 — 파일 목록 열 폭과 같은 관례
        let mut columns = QueueColumns::default();
        columns.apply_drag(QueueFilter::All, 1, 40.0);
        columns.apply_drag(QueueFilter::Done, 2, -20.0);
        columns.apply_drag(QueueFilter::Error, 5, 25.0);
        let back = QueueColumns::from_saved(
            &columns.to_saved(QueueFilter::All),
            &columns.to_saved(QueueFilter::Done),
            &columns.to_saved(QueueFilter::Error),
        );
        assert_eq!(back, columns, "세 벌이 각각 왕복해야 한다");

        let 기본 = QueueColumns::default();
        let all = default_widths(QueueFilter::All);
        // 저장된 것이 없으면 기본값이다(옛 세션 파일)
        assert_eq!(QueueColumns::from_saved(&[], &[], &[]), 기본);
        // **옛 파일에는 `전송 큐` 몫만 있다** — 나머지 두 탭은 기본 폭으로 서야 한다
        let 옛것 = QueueColumns::from_saved(&all, &[], &[]);
        assert_eq!(옛것, 기본, "옛 세션이 두 탭의 폭을 흔들었다");
        // 개수가 모자라면 앞에서부터 받고 나머지는 기본값
        let 부분 = QueueColumns::from_saved(&[50.0, 60.0], &[], &[]);
        assert_eq!(부분.to_saved(QueueFilter::All)[..2], [50.0, 60.0]);
        assert_eq!(부분.to_saved(QueueFilter::All)[2..], all[2..]);
        // 유한하지 않은 값은 그 자리만 되돌리고, 하한 미만은 하한으로 올린다
        let 손상 = QueueColumns::from_saved(&[f32::NAN, 5.0], &[], &[]);
        assert_eq!(손상.to_saved(QueueFilter::All)[0], all[0]);
        assert_eq!(
            손상.to_saved(QueueFilter::All)[1],
            crate::ui::list_details::MIN_COL_WIDTH
        );
        // `방향`은 기본 34px이 곧 하한이라 그 값으로 되살아난다(왕복이 깨지지 않는다)
        assert_eq!(
            QueueColumns::from_saved(&[34.0], &[], &[]).to_saved(QueueFilter::All)[0],
            34.0
        );
    }

    #[test]
    fn 머리글_문구는_인벤토리_원문_그대로다() {
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        // 인벤토리 #37~#43
        let 문구 = |filter| {
            columns_for(filter)
                .iter()
                .map(|kind| kind.header())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            문구(QueueFilter::All),
            [
                "방향",
                "로컬 파일",
                "원격 파일",
                "서버",
                "크기",
                "진행률",
                "상태"
            ]
        );
        assert_eq!(crate::i18n::queue_filter_all(), "전체");
        // 방향 표시는 **아이콘 글꼴**에서 온다 (프로젝트 규약 — 원본 화살표는 두부가 된다)
        assert!(crate::ui::widgets::is_icon_font(widgets::UPLOAD_GLYPH));
        assert!(crate::ui::widgets::is_icon_font(widgets::DOWNLOAD_GLYPH));
        assert_ne!(widgets::UPLOAD_GLYPH, widgets::DOWNLOAD_GLYPH);
        assert_eq!(crate::i18n::queue_state_pending(), "대기 중");
        assert_eq!(crate::i18n::queue_state_done(), "완료");
        assert_eq!(crate::i18n::queue_state_active(), "전송 중");
    }

    /// 오늘 안에서 끝난 전송은 시각만 적는다 — 날짜를 늘 적으면 칸만 넓어진다
    #[test]
    fn 오늘_안의_전송은_시각만_적는다() {
        let today = 시각(2026, 8, 28, 14, 3, 21);
        let start = 파일시각(2026, 8, 28, 14, 3, 21);
        let end = 파일시각(2026, 8, 28, 14, 3, 48);
        assert_eq!(
            format_time_range(Some(start), Some(end), today),
            "14:03:21 ~ 14:03:48"
        );
    }

    /// 세션에서 되살아난 실패 항목은 며칠 전 것일 수 있다 — 날짜가 없으면 방금 실패한
    /// 것으로 오해한다
    #[test]
    fn 오늘이_아니면_날짜를_붙인다() {
        let today = 시각(2026, 8, 28, 9, 0, 0);
        let start = 파일시각(2026, 8, 27, 23, 59, 5);
        let end = 파일시각(2026, 8, 28, 0, 0, 12);
        // 자정을 걸친 전송 — 시작만 어제라 그쪽에만 날짜가 붙는다
        assert_eq!(
            format_time_range(Some(start), Some(end), today),
            "08-27 23:59:05 ~ 00:00:12"
        );

        // 둘 다 다른 날이면 둘 다 붙는다
        let 어제만 = 파일시각(2026, 8, 27, 10, 0, 0);
        assert_eq!(
            format_time_range(Some(어제만), Some(start), today),
            "08-27 10:00:00 ~ 08-27 23:59:05"
        );
    }

    /// 아직 끝나지 않았거나 시각을 모르는 갈래
    #[test]
    fn 모르는_시각은_비우거나_물결로_적는다() {
        let today = 시각(2026, 8, 28, 14, 0, 0);
        let start = 파일시각(2026, 8, 28, 14, 3, 21);

        // 시작만 안다 — 끝을 비워 둔다
        assert_eq!(format_time_range(Some(start), None, today), "14:03:21 ~");
        // 둘 다 모르면 크기를 모를 때와 같은 글자다
        assert_eq!(format_time_range(None, None, today), UNKNOWN);
        // 0은 FILETIME으로 풀리지 않아 모르는 것과 같다
        assert_eq!(format_time_range(Some(0), Some(0), today), UNKNOWN);
    }

    /// 시계가 뒤로 조정돼 끝이 시작보다 앞서도 **값을 고치지 않는다** —
    /// 화면이 사실과 다른 것을 지어내는 것보다 어긋난 값을 그대로 보이는 편이 낫다
    #[test]
    fn 끝이_시작보다_앞서도_그대로_적는다() {
        let today = 시각(2026, 8, 28, 14, 0, 0);
        let 늦은 = 파일시각(2026, 8, 28, 14, 3, 48);
        let 이른 = 파일시각(2026, 8, 28, 14, 3, 21);
        assert_eq!(
            format_time_range(Some(늦은), Some(이른), today),
            "14:03:48 ~ 14:03:21"
        );
    }

    /// 시험용 `LocalTime` — 시계에 기대지 않는다
    fn 시각(y: u16, mo: u16, d: u16, h: u16, mi: u16, se: u16) -> LocalTime {
        LocalTime {
            year: y,
            month: mo,
            day: d,
            hour: h,
            minute: mi,
            second: se,
        }
    }

    /// 그 **로컬** 시각을 가리키는 FILETIME을 만든다.
    ///
    /// 로컬 → UTC 변환을 시험이 직접 하지 않고 `TzSpecificLocalTimeToSystemTime`에 맡긴다 —
    /// 시간대 오프셋을 손으로 빼면 서머타임이 있는 곳에서 하루가 어긋난다
    fn 파일시각(y: u16, mo: u16, d: u16, h: u16, mi: u16, se: u16) -> u64 {
        use windows::Win32::Foundation::SYSTEMTIME;
        use windows::Win32::System::Time::{SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime};
        let local = SYSTEMTIME {
            wYear: y,
            wMonth: mo,
            wDay: d,
            wHour: h,
            wMinute: mi,
            wSecond: se,
            ..Default::default()
        };
        // 안전성: 인자가 모두 스택 소유다. 실패하면 0(= 모르는 시각)으로 둔다
        unsafe {
            let mut utc = Default::default();
            if TzSpecificLocalTimeToSystemTime(None, &local, &mut utc).is_err() {
                return 0;
            }
            let mut ft = Default::default();
            if SystemTimeToFileTime(&utc, &mut ft).is_err() {
                return 0;
            }
            u64::from(ft.dwLowDateTime) | (u64::from(ft.dwHighDateTime) << 32)
        }
    }
    #[test]
    fn 얼룩은_거른_뒤의_홀수_자리다() {
        // Acceptance ⑥ — 원본은 거른 목록의 0부터 센 자리 번호가 홀수인 행을 칠한다 (`:721`)
        assert!(!stripe(0));
        assert!(stripe(1));
        assert!(!stripe(2));
        assert!(stripe(3));
    }

    #[test]
    fn 상태_문구와_색이_상태별로_갈린다() {
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        // Acceptance ⑦ (인벤토리 #45~#48)
        let 항목 = |state| {
            let mut queue = TransferQueue::new();
            let mut sites = SiteStore::new();
            let site = sites.add("웹서버");
            let id = queue.enqueue(
                site,
                TransferDirection::Upload,
                PathBuf::from(r"C:.txt"),
                RemotePath::new("/a.txt"),
                100,
            );
            queue.update(id, state);
            (queue, id)
        };
        let 문구 = |state| {
            let (queue, id) = 항목(state);
            let item = queue.get(id).expect("항목").clone();
            state_text(&item, 3)
        };

        let (text, color) = 문구(TransferState::Wait);
        assert_eq!(text, "대기 중");
        assert_eq!(color, theme::TEXT_MUTED);

        let (text, color) = 문구(TransferState::Active {
            sent: 10,
            speed: 13_002_342,
        });
        assert_eq!(text, "전송 중 · 12.4 MB/s");
        assert_eq!(color, theme::ACCENT);

        // 속도를 아직 못 쟀으면 군더더기를 붙이지 않는다
        let (text, _) = 문구(TransferState::Active { sent: 0, speed: 0 });
        assert_eq!(text, "전송 중");

        let (text, color) = 문구(TransferState::Done);
        assert_eq!(text, "완료");
        assert_eq!(color, theme::OK_TEXT);

        // 실패는 서버가 준 사유를 그대로 보인다
        let (text, color) = 문구(TransferState::Error {
            message: "550 권한 거부".to_owned(),
        });
        assert_eq!(text, "550 권한 거부");
        assert_eq!(color, theme::ERROR);
    }

    #[test]
    fn 자동_재시도로_다시_도는_항목은_회차를_보인다() {
        // FR-37 — 붙이지 않으면 사용자는 같은 전송이 왜 두 번 도는지 알 수 없다
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        let mut queue = TransferQueue::new();
        let mut sites = SiteStore::new();
        let site = sites.add("웹서버");
        let id = queue.enqueue(
            site,
            TransferDirection::Upload,
            PathBuf::from(r"C:.txt"),
            RemotePath::new("/a.txt"),
            100,
        );
        queue.update(
            id,
            TransferState::Error {
                message: "550 실패".to_owned(),
            },
        );
        // 자동으로 한 번 되걸렸다
        assert!(queue.retry_automatically(id, 3, 0.0));
        let item = queue.get(id).expect("항목").clone();
        let (text, _) = state_text(&item, 3);
        assert_eq!(text, "대기 중 · 재시도 1/3");

        // 다시 도는 중에도 회차가 남는다
        queue.update(id, TransferState::Active { sent: 0, speed: 0 });
        let item = queue.get(id).expect("항목").clone();
        let (text, _) = state_text(&item, 3);
        assert_eq!(text, "전송 중 · 재시도 1/3");

        // **끝나면 회차를 적지 않는다** — 완료·실패는 그 자체가 결과다
        queue.update(id, TransferState::Done);
        let item = queue.get(id).expect("항목").clone();
        assert_eq!(state_text(&item, 3).0, "완료");
    }

    #[test]
    fn 진행_막대_색이_상태별로_갈린다() {
        // Acceptance ⑦ (`:701`)
        assert_eq!(bar_color(&TransferState::Wait), theme::ACCENT);
        assert_eq!(bar_color(&TransferState::Done), theme::OK_BAR);
        assert_eq!(
            bar_color(&TransferState::Error {
                message: String::new()
            }),
            theme::ERROR
        );
    }

    #[test]
    fn 크기와_속도_표기가_원본_꼴이다() {
        // 크기는 파일 목록과 같은 규칙이다 — 소수 둘째자리 + KB·MB·GB (2026-08-18)
        assert_eq!(format_size(12 * 1024), "12.00 KB");
        assert_eq!(format_size(900 * 1024), "900.00 KB");
        // MB·GB로 올라간다 — KB에 고정하면 `1,887,437 KB` 같은 수가 나온다 (2026-08-16 검토)
        assert_eq!(format_size(1_884_160), "1.80 MB");
        assert_eq!(format_size(2 * 1024 * 1024 * 1024), "2.00 GB");
        assert_eq!(format_size(1), "0.01 KB", "1KB 미만도 한 칸은 채운다");
        // 큐에서만 0이 "모른다"다 — 파일 목록의 같은 0은 빈 파일이라 `0.00 KB`다
        assert_eq!(format_size(0), "—", "크기를 모르면 표기가 없다");
        assert_eq!(crate::panel::file_list::format_size(0), "0.00 KB");

        assert_eq!(format_speed(13_002_342), "12.4 MB/s");
        assert_eq!(format_speed(2048), "2.0 KB/s");
        assert_eq!(format_speed(512), "512 B/s");
        // 로컬 네트워크·SSD 사이에서는 GB/s가 실제로 나온다 (T21 Edge Case)
        assert_eq!(format_speed(2 * 1024 * 1024 * 1024), "2.0 GB/s");
    }

    fn queue_with_two_sites() -> (TransferQueue, SiteStore, SiteId, SiteId) {
        let mut sites = SiteStore::new();
        let first = sites.add("web-prod");
        let second = sites.add("cdn-assets");
        let mut queue = TransferQueue::new();
        for _ in 0..3 {
            queue.enqueue(
                first,
                TransferDirection::Upload,
                PathBuf::from(r"C:\a.js"),
                RemotePath::new("/a.js"),
                1024,
            );
        }
        let done = queue.enqueue(
            second,
            TransferDirection::Download,
            PathBuf::from(r"C:\b.log"),
            RemotePath::new("/b.log"),
            2048,
        );
        queue.update(done, TransferState::Done);
        (queue, sites, first, second)
    }

    #[test]
    fn 필터와_연결별_탭이_함께_걸린다() {
        // Acceptance ④·⑤ — 성공·실패 탭이 같은 집합을 거르고, 연결별 탭이 그 위에 얹힌다
        let (queue, _, first, second) = queue_with_two_sites();
        assert_eq!(visible_items(&queue, QueueFilter::All, None).len(), 4);
        assert_eq!(
            visible_items(&queue, QueueFilter::All, Some(first)).len(),
            3
        );
        assert_eq!(visible_items(&queue, QueueFilter::Done, None).len(), 1);
        assert_eq!(
            visible_items(&queue, QueueFilter::Done, Some(first)).len(),
            0,
            "그 사이트에는 끝난 것이 없다"
        );
        assert_eq!(
            visible_items(&queue, QueueFilter::Done, Some(second)).len(),
            1
        );
    }

    #[test]
    fn 연결별_탭의_점은_실패한_사이트만_빨강이다() {
        // spec 리뷰 M1 — 연결 객체의 유무로 가르면 **실패한 사이트가 초록**으로 뒤집힌다.
        // 원본은 `phase === "error"`일 때만 빨강이고 그 밖은 전부 초록이다 (`:728`)
        assert_eq!(site_dot_color(&[SiteId(1)], SiteId(1)), theme::ERROR);
        assert_eq!(site_dot_color(&[SiteId(1)], SiteId(2)), theme::OK_DOT);
        assert_eq!(site_dot_color(&[], SiteId(1)), theme::OK_DOT);
    }

    #[test]
    fn 큐가_비면_그릴_행이_없다() {
        // plan Edge Case — 머리글만 남는다
        let queue = TransferQueue::new();
        assert!(visible_items(&queue, QueueFilter::All, None).is_empty());
    }

    /// 큐 표를 여러 프레임 그리며 입력을 먹인다 — 마지막 프레임의 **채운 사각형**을
    /// `(자리, 색)`으로 모은다.
    ///
    /// egui의 상호작용은 **직전 프레임의 위젯 자리**로 판정하므로 자리 잡기 프레임이 먼저
    /// 필요하고, 클릭은 누름과 뗌이 갈라진 두 프레임으로 만들어야 `clicked()`가 선다
    fn draw_queue_frames(
        state: &mut DockState,
        view: &DockView<'_>,
        sites: &SiteStore,
        inputs: Vec<Vec<egui::Event>>,
    ) -> Vec<(egui::Rect, egui::Color32)> {
        let ctx = egui::Context::default();
        let mut shapes = Vec::new();
        let mut time = 0.0;
        for events in inputs {
            time += 0.1;
            let input = egui::RawInput {
                time: Some(time),
                events,
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let rect =
                        egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(1200.0, 300.0));
                    show_queue(ui, rect, state, view, sites, None, 3);
                });
            });
            shapes.clear();
            for clipped in &output.shapes {
                if let egui::Shape::Rect(rect) = &clipped.shape {
                    shapes.push((rect.rect, rect.fill));
                }
            }
        }
        shapes
    }

    /// 왼쪽 버튼 누름·뗌 — 자리 잡기 · 누름 · 뗌 세 프레임으로 나눈다
    fn 클릭(at: egui::Pos2, modifiers: egui::Modifiers) -> Vec<Vec<egui::Event>> {
        vec![
            vec![egui::Event::PointerMoved(at)],
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers,
            }],
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers,
            }],
        ]
    }

    /// 첫 행의 한가운데 — 연결별 탭 줄과 머리글 아래 첫 줄이다
    fn 첫_행_자리() -> egui::Pos2 {
        egui::pos2(40.0, SITE_ROW_HEIGHT + HEADER_HEIGHT + ROW_HEIGHT / 2.0)
    }

    fn 선택색() -> egui::Color32 {
        egui::Context::default()
            .style_of(egui::Theme::Dark)
            .visuals
            .selection
            .bg_fill
    }

    fn 선택색_사각형(shapes: &[(egui::Rect, egui::Color32)]) -> usize {
        let 색 = 선택색();
        shapes.iter().filter(|(_, fill)| *fill == 색).count()
    }

    fn 큐_상태() -> DockState {
        DockState {
            panel: Some(crate::ui::dock::DockPanel::Queue),
            ..DockState::default()
        }
    }

    #[test]
    fn 행을_클릭하면_선택_배경이_칠해진다() {
        // Acceptance ① — 클릭 전에는 그 색이 0개, 클릭 뒤 1개.
        // 색은 파일 목록과 같은 `visuals().selection.bg_fill`이며, 큐 표가 칠하는 다른 색
        // (얼룩 #252525·hover #2E2E2E·진행률 바)과 값이 겹치지 않는다
        let (queue, sites, first, _) = queue_with_two_sites();
        let view = DockView {
            connected: &[],
            queue: &queue,
            failed: &[first],
        };

        let mut state = 큐_상태();
        let 그대로 = draw_queue_frames(&mut state, &view, &sites, vec![vec![], vec![]]);
        assert_eq!(
            선택색_사각형(&그대로),
            0,
            "고르기 전인데 선택 배경이 그려졌다"
        );
        assert!(state.queue_selection.is_empty());

        let mut state = 큐_상태();
        let 누른뒤 = draw_queue_frames(
            &mut state,
            &view,
            &sites,
            [
                vec![vec![]],
                클릭(첫_행_자리(), egui::Modifiers::NONE),
                vec![vec![]],
            ]
            .concat(),
        );
        assert_eq!(
            state.queue_selection.len(),
            1,
            "클릭이 선택을 만들지 않았다"
        );
        assert_eq!(선택색_사각형(&누른뒤), 1, "고른 행에 선택 배경이 없다");
    }

    #[test]
    fn 선택이_hover를_이긴다() {
        // Acceptance ③ — 고른 행에 마우스를 올려도 칠해지는 것은 선택색이다
        // (파일 목록의 차례와 같다: 얼룩 → 선택 → hover)
        let (queue, sites, first, _) = queue_with_two_sites();
        let view = DockView {
            connected: &[],
            queue: &queue,
            failed: &[first],
        };
        let mut state = 큐_상태();
        let 자리 = 첫_행_자리();
        // 클릭한 자리에 마우스를 그대로 둔다 — hover와 선택이 같은 행에 겹친다
        let shapes = draw_queue_frames(
            &mut state,
            &view,
            &sites,
            [
                vec![vec![]],
                클릭(자리, egui::Modifiers::NONE),
                vec![vec![]],
            ]
            .concat(),
        );
        assert_eq!(
            state.queue_selection.len(),
            1,
            "시험 자체가 성립하지 않았다"
        );
        assert_eq!(선택색_사각형(&shapes), 1);
        assert!(
            !shapes
                .iter()
                .any(|(rect, fill)| *fill == theme::ROW_HOT && rect.contains(자리)),
            "고른 행이 hover 색으로 덮였다"
        );
    }

    #[test]
    fn 클릭은_단독_ctrl은_토글_shift는_범위다() {
        // Acceptance ② — 파일 목록(`ui::file_list`의 `select`)과 같은 규칙
        let (queue, _, _, _) = queue_with_two_sites();
        let items = visible_items(&queue, QueueFilter::All, None);
        let ids: Vec<TransferId> = items.iter().map(|item| item.id).collect();
        let 빈것 = HashSet::new();

        // 맨클릭 — 단독 선택, 기준점은 그 자리
        let (단독, 기준) = select_rows(&빈것, None, &items, 1, egui::Modifiers::NONE);
        assert_eq!(단독, std::iter::once(ids[1]).collect::<HashSet<_>>());
        assert_eq!(기준, Some(ids[1]));

        // Ctrl — 없으면 더한다
        let ctrl = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let (더한것, _) = select_rows(&단독, 기준, &items, 3, ctrl);
        assert_eq!(더한것.len(), 2);
        assert!(더한것.contains(&ids[1]) && 더한것.contains(&ids[3]));
        // Ctrl — 있으면 뺀다
        let (뺀것, _) = select_rows(&더한것, Some(ids[3]), &items, 1, ctrl);
        assert_eq!(뺀것, std::iter::once(ids[3]).collect::<HashSet<_>>());

        // Shift — 기준점부터 범위
        let shift = egui::Modifiers {
            shift: true,
            ..Default::default()
        };
        let (범위, 기준그대로) = select_rows(&단독, Some(ids[1]), &items, 3, shift);
        assert_eq!(범위.len(), 3, "1~3 세 줄이 잡혀야 한다");
        assert!(범위.contains(&ids[1]) && 범위.contains(&ids[2]) && 범위.contains(&ids[3]));
        assert_eq!(
            기준그대로,
            Some(ids[1]),
            "범위 선택은 기준점을 옮기지 않는다"
        );

        // Shift — 기준점이 없으면 단독으로 떨어진다
        let (기준없음, _) = select_rows(&빈것, None, &items, 2, shift);
        assert_eq!(기준없음, std::iter::once(ids[2]).collect::<HashSet<_>>());
    }

    #[test]
    fn 사라진_항목은_선택에서_빠진다() {
        // Acceptance ⑤ — `✕`(끝난 항목 지우기)·사이트 삭제로 항목이 없어지면 죈다.
        // 기준점이 사라졌으면 기준점도 함께 비운다
        let (mut queue, _, _, _) = queue_with_two_sites();
        let 살아있는 = visible_items(&queue, QueueFilter::All, None);
        let ids: Vec<TransferId> = 살아있는.iter().map(|item| item.id).collect();
        let 고른것: HashSet<TransferId> = ids.iter().copied().collect();

        queue.remove(&[ids[0], ids[1]]);
        let 남은것 = visible_items(&queue, QueueFilter::All, None);
        let (죈것, 기준) = prune_selection(&고른것, Some(ids[0]), &남은것);
        assert_eq!(죈것.len(), 2, "지운 둘이 선택에 남았다");
        assert!(!죈것.contains(&ids[0]) && !죈것.contains(&ids[1]));
        assert_eq!(기준, None, "사라진 기준점이 남았다");

        // 살아 있는 기준점은 그대로다
        let (_, 살아있는기준) = prune_selection(&고른것, Some(ids[2]), &남은것);
        assert_eq!(살아있는기준, Some(ids[2]));
    }

    #[test]
    fn 연결별_탭을_바꾸면_선택이_빈다() {
        // Acceptance ④(연결별 탭) — 보이는 목록이 통째로 갈리므로 고른 것을 남기지 않는다
        let (queue, sites, first, second) = queue_with_two_sites();
        let view = DockView {
            connected: &[first, second],
            queue: &queue,
            failed: &[],
        };
        let 고른것: HashSet<TransferId> = visible_items(&queue, QueueFilter::All, Some(first))
            .iter()
            .map(|item| item.id)
            .collect();
        assert!(!고른것.is_empty(), "시험 자체가 성립하지 않았다");

        let mut state = DockState {
            panel: Some(crate::ui::dock::DockPanel::Queue),
            site: Some(first),
            queue_selection: 고른것.clone(),
            queue_anchor: 고른것.iter().next().copied(),
            ..DockState::default()
        };
        // 첫 연결별 탭(`전체`)을 누른다
        let 전체_탭 = egui::pos2(SITE_ROW_PAD_X + 20.0, SITE_ROW_HEIGHT / 2.0);
        let _ = draw_queue_frames(
            &mut state,
            &view,
            &sites,
            [
                vec![vec![]],
                클릭(전체_탭, egui::Modifiers::NONE),
                vec![vec![]],
            ]
            .concat(),
        );
        assert_eq!(state.site, None, "연결별 탭 클릭이 먹지 않았다");
        assert!(
            state.queue_selection.is_empty(),
            "연결별 탭을 바꿨는데 고른 것이 남았다"
        );
        assert_eq!(state.queue_anchor, None);
    }

    #[test]
    fn 유효_선택은_고른_것_안이면_전체_밖이면_그_행이다() {
        // Acceptance ④-b — 메뉴는 우클릭한 **그 프레임에 바로** 그려지는데 선택 대입은
        // 행을 다 그린 뒤에 오므로, 그 프레임의 메뉴는 옛 선택을 본다. 이 계산이 그 간극을 메운다
        let (queue, _, _, _) = queue_with_two_sites();
        let items = visible_items(&queue, QueueFilter::All, None);
        let ids: Vec<TransferId> = items.iter().map(|item| item.id).collect();

        let 번호 = |대상: Vec<&TransferItem>| -> Vec<TransferId> {
            대상.iter().map(|item| item.id).collect()
        };

        // 고른 것 안의 행 → 선택 전체, **보이는 목록의 차례로**
        let 고른것: HashSet<TransferId> = [ids[2], ids[0]].into_iter().collect();
        assert_eq!(
            번호(effective_selection(&고른것, items[0], &items)),
            vec![ids[0], ids[2]],
            "고른 것 안을 우클릭했는데 대상이 선택 전체가 아니다"
        );
        assert_eq!(
            번호(effective_selection(&고른것, items[2], &items)),
            vec![ids[0], ids[2]],
            "같은 선택 안이면 어느 행을 눌러도 대상이 같아야 한다"
        );

        // 고른 것 밖의 행 → 그 행 하나
        assert_eq!(
            번호(effective_selection(&고른것, items[1], &items)),
            vec![ids[1]]
        );
        // 아무것도 고르지 않았어도 그 행 하나 — 빈 대상이 나가지 않는다
        assert_eq!(
            번호(effective_selection(&HashSet::new(), items[1], &items)),
            vec![ids[1]]
        );
    }

    #[test]
    fn 메뉴는_고른_것_전부의_상태를_본다() {
        // Acceptance ①·③ — 「하나라도」 규칙. 섞인 선택에서는 `전송 취소`와 `삭제`가
        // 나란히 선다(둘은 하는 일이 다르다 — 취소는 `취소됨`으로 남기고 삭제는 지운다)
        use RowMenuItem::*;
        let 실패 = TransferState::Error {
            message: "550".to_owned(),
        };

        // 대기 + 실패 — 다시 시도(실패 때문)·전송 취소(대기 때문)·삭제(실패 때문)가 모두 선다
        assert_eq!(
            row_menu_items(&[TransferState::Wait, 실패.clone()], true),
            vec![Retry, Cancel, RetryAll, Remove, RemoveAll]
        );
        // 완료만 여럿 — 다시 걸 것도 그만둘 것도 없다
        assert_eq!(
            row_menu_items(&[TransferState::Done, TransferState::Done], false),
            vec![Remove, RemoveAll]
        );
        // 대기만 여럿 — 삭제는 서지 않는다(`전송 취소`가 그 자리를 맡는다)
        assert_eq!(
            row_menu_items(&[TransferState::Wait, TransferState::Wait], false),
            vec![Cancel, RemoveAll]
        );
        // 빈 선택은 있을 수 없지만(대상이 최소 하나다) 들어와도 터지지 않는다
        assert_eq!(row_menu_items(&[], false), vec![RemoveAll]);
    }

    #[test]
    fn 메뉴_조작은_고른_것_전부를_싣는다() {
        // Acceptance ① — 여러 줄을 골라 놓고 `다시 시도`를 누르면 고른 것 전부가 대상이다.
        // 메뉴를 실제로 열어 항목을 누르고, 돌아온 `QueueAction`이 무엇을 실었는지 본다
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        let (mut queue, sites, first, _) = queue_with_two_sites();
        // 앞의 셋을 실패로 만든다 — `다시 시도`가 서려면 다시 걸 것이어야 한다
        let 실패한것: Vec<TransferId> = visible_items(&queue, QueueFilter::All, None)
            .iter()
            .take(3)
            .map(|item| item.id)
            .collect();
        for id in &실패한것 {
            queue.update(
                *id,
                TransferState::Error {
                    message: "550".to_owned(),
                },
            );
        }
        let view = DockView {
            connected: &[],
            queue: &queue,
            failed: &[first],
        };
        let 고른것: HashSet<TransferId> = 실패한것.iter().copied().collect();
        let mut state = DockState {
            panel: Some(crate::ui::dock::DockPanel::Queue),
            queue_selection: 고른것.clone(),
            queue_anchor: Some(실패한것[0]),
            ..DockState::default()
        };
        let 골라진것 =
            메뉴에서_고른다(&mut state, &view, &sites, crate::i18n::queue_retry());
        assert_eq!(
            골라진것,
            Some(QueueAction::Retry(실패한것.clone())),
            "고른 것 전부가 아니라 다른 것이 실렸다"
        );
    }

    /// 첫 행을 우클릭해 메뉴를 띄우고 그 문구의 항목을 누른다 — 돌아온 조작을 준다.
    ///
    /// 메뉴는 우클릭한 **그 프레임에 바로** 뜨므로, 항목 자리는 그려진 글자에서 찾는다
    fn 메뉴에서_고른다(
        state: &mut DockState,
        view: &DockView<'_>,
        sites: &SiteStore,
        문구: &str,
    ) -> Option<QueueAction> {
        let ctx = egui::Context::default();
        let mut time = 0.0;
        let mut 골라진것 = None;
        let mut 항목_자리 = None;
        let 첫행 = 첫_행_자리();
        // 자리 잡기 → 우클릭(누름·뗌) → 메뉴가 뜬 프레임에서 항목 자리를 잰다 →
        // 그 자리로 옮겨 왼쪽 버튼 누름·뗌
        let mut frames: Vec<Vec<egui::Event>> = vec![vec![]];
        frames.extend(우클릭(첫행));
        frames.push(vec![]);
        for events in frames {
            time += 0.1;
            let output = 한_프레임(&ctx, state, view, sites, time, events, &mut 골라진것);
            if 항목_자리.is_none() {
                항목_자리 = 글자_자리(&output, 문구);
            }
        }
        let at = 항목_자리.unwrap_or_else(|| panic!("메뉴에서 `{문구}`를 찾지 못했다"));
        let mut frames: Vec<Vec<egui::Event>> = vec![vec![egui::Event::PointerMoved(at)]];
        frames.extend([
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        ]);
        for events in frames {
            time += 0.1;
            let _ = 한_프레임(&ctx, state, view, sites, time, events, &mut 골라진것);
        }
        골라진것
    }

    fn 한_프레임(
        ctx: &egui::Context,
        state: &mut DockState,
        view: &DockView<'_>,
        sites: &SiteStore,
        time: f64,
        events: Vec<egui::Event>,
        골라진것: &mut Option<QueueAction>,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            time: Some(time),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let rect = egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(1200.0, 300.0));
                if let Some(picked) = show_queue(ui, rect, state, view, sites, None, 3) {
                    *골라진것 = Some(picked);
                }
            });
        })
    }

    /// 그 문구로 그려진 글자의 한가운데 — 메뉴 항목을 누를 자리다
    fn 글자_자리(output: &egui::FullOutput, 문구: &str) -> Option<egui::Pos2> {
        for clipped in &output.shapes {
            if let egui::Shape::Text(text) = &clipped.shape
                && text.galley.text() == 문구
            {
                return Some(egui::pos2(
                    text.pos.x + text.galley.size().x / 2.0,
                    text.pos.y + text.galley.size().y / 2.0,
                ));
            }
        }
        None
    }

    /// 오른쪽 버튼 누름·뗌 — 자리 잡기 · 누름 · 뗌 세 프레임
    fn 우클릭(at: egui::Pos2) -> Vec<Vec<egui::Event>> {
        vec![
            vec![egui::Event::PointerMoved(at)],
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
            vec![egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Secondary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        ]
    }

    #[test]
    fn 선택_밖의_행을_우클릭하면_그_행만_고른다() {
        // 파일 목록과 같은 규칙 (`ui::list_details` — 선택 밖 행 우클릭은 단독 선택 후 메뉴).
        // **선택 안의 행을 우클릭하면 고른 것을 건드리지 않는다** — 여럿을 골라 놓고 메뉴를
        // 여는 길이 막히면 안 된다
        let (queue, sites, first, _) = queue_with_two_sites();
        let view = DockView {
            connected: &[],
            queue: &queue,
            failed: &[first],
        };
        let items = visible_items(&queue, QueueFilter::All, None);
        let ids: Vec<TransferId> = items.iter().map(|item| item.id).collect();

        // 둘째·셋째 행을 골라 둔 채 **첫 행**(선택 밖)을 우클릭한다
        let mut state = DockState {
            panel: Some(crate::ui::dock::DockPanel::Queue),
            queue_selection: [ids[1], ids[2]].into_iter().collect(),
            queue_anchor: Some(ids[1]),
            ..DockState::default()
        };
        let _ = draw_queue_frames(
            &mut state,
            &view,
            &sites,
            [vec![vec![]], 우클릭(첫_행_자리()), vec![vec![]]].concat(),
        );
        assert_eq!(
            state.queue_selection,
            std::iter::once(ids[0]).collect::<HashSet<_>>(),
            "선택 밖 행을 우클릭했는데 그 행이 단독으로 잡히지 않았다"
        );
        assert_eq!(state.queue_anchor, Some(ids[0]));

        // 이미 고른 행을 우클릭하면 그대로다
        let 고른것: HashSet<TransferId> = [ids[0], ids[1]].into_iter().collect();
        let mut state = DockState {
            panel: Some(crate::ui::dock::DockPanel::Queue),
            queue_selection: 고른것.clone(),
            queue_anchor: Some(ids[1]),
            ..DockState::default()
        };
        let _ = draw_queue_frames(
            &mut state,
            &view,
            &sites,
            [vec![vec![]], 우클릭(첫_행_자리()), vec![vec![]]].concat(),
        );
        assert_eq!(
            state.queue_selection, 고른것,
            "이미 고른 행을 우클릭했는데 선택이 줄었다"
        );
        assert_eq!(state.queue_anchor, Some(ids[1]), "기준점이 함께 옮겨졌다");
    }

    #[test]
    fn 같은_연결별_탭을_다시_눌러도_선택은_남는다() {
        // Acceptance ④의 짝 — 대입이 「누를 때」가 아니라 「바뀔 때」임을 못 박는다.
        // 견주지 않으면 같은 탭 재클릭만으로 고른 것이 사라진다
        let (queue, sites, first, second) = queue_with_two_sites();
        let view = DockView {
            connected: &[first, second],
            queue: &queue,
            failed: &[],
        };
        let 고른것: HashSet<TransferId> = visible_items(&queue, QueueFilter::All, None)
            .iter()
            .map(|item| item.id)
            .collect();
        let mut state = DockState {
            panel: Some(crate::ui::dock::DockPanel::Queue),
            site: None,
            queue_selection: 고른것.clone(),
            queue_anchor: 고른것.iter().next().copied(),
            ..DockState::default()
        };
        // 이미 활성인 `전체` 탭을 다시 누른다
        let 전체_탭 = egui::pos2(SITE_ROW_PAD_X + 20.0, SITE_ROW_HEIGHT / 2.0);
        let _ = draw_queue_frames(
            &mut state,
            &view,
            &sites,
            [
                vec![vec![]],
                클릭(전체_탭, egui::Modifiers::NONE),
                vec![vec![]],
            ]
            .concat(),
        );
        assert_eq!(state.site, None);
        assert_eq!(
            state.queue_selection, 고른것,
            "같은 탭을 다시 눌렀는데 고른 것이 사라졌다"
        );
    }

    #[test]
    fn 표가_한_프레임을_그린다() {
        // 자리 계산이 뒤집힌 사각형·id 충돌 없이 도는지 본다 (Acceptance ⑧)
        let (queue, sites, first, _) = queue_with_two_sites();
        let ctx = egui::Context::default();
        let mut state = DockState {
            panel: Some(crate::ui::dock::DockPanel::Queue),
            ..DockState::default()
        };
        let view = DockView {
            connected: &[],
            queue: &queue,
            failed: &[first],
        };
        let _ = ctx.run_ui(Default::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let rect = egui::Rect::from_min_size(ui.max_rect().min, egui::vec2(1200.0, 200.0));
                show_queue(ui, rect, &mut state, &view, &sites, None, 3);
            });
        });
        // 연결별 탭을 고르지 않았으면 `전체`가 그대로다
        assert_eq!(state.site, None);
    }

    #[test]
    fn 연결된_서버는_큐가_비어도_탭에_선다() {
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        // 사용자 보고(2026-08-05): 연결한 서버가 탭 줄에 없어 고를 수 없었다 —
        // 원본대로 **큐에 든 항목**에서만 이름을 모았기 때문이다
        let mut sites = SiteStore::new();
        let 연결된 = sites.add("web-prod");
        let 연결_없는 = sites.add("legacy");
        let queue = TransferQueue::new();
        let view = DockView {
            queue: &queue,
            failed: &[],
            connected: &[연결된],
        };
        let mut state = DockState::default();
        let ctx = egui::Context::default();
        let mut texts = Vec::new();
        let output = ctx.run_ui(Default::default(), |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let rect = egui::Rect::from_min_size(
                    ui.max_rect().min,
                    egui::vec2(900.0, SITE_ROW_HEIGHT),
                );
                show_site_tabs(ui, rect, &mut state, &view, &sites, true);
            });
        });
        for clipped in &output.shapes {
            if let egui::Shape::Text(text) = &clipped.shape {
                texts.push(text.galley.text().to_owned());
            }
        }
        assert!(
            texts.iter().any(|text| text.starts_with("web-prod")),
            "연결된 서버가 탭에 없다: {texts:?}"
        );
        assert!(
            texts
                .iter()
                .any(|text| text.starts_with(crate::i18n::queue_filter_all())),
            "`전체` 탭이 없다: {texts:?}"
        );
        // 연결도 없고 큐에도 없는 사이트는 서지 않는다 — 목록이 등록한 사이트 전부로 늘어나면
        // 지금 무엇에 붙어 있는지가 오히려 안 보인다
        let _ = 연결_없는;
        assert!(
            !texts.iter().any(|text| text.starts_with("legacy")),
            "연결도 전송도 없는 사이트가 탭에 섰다: {texts:?}"
        );
    }

    #[test]
    fn 행_메뉴는_탭이_아니라_행_상태가_정한다() {
        // 2026-08-18 사용자 결정(D5) — `전송 취소`와 `삭제`는 동작이 같아 한 쪽만 보인다
        use RowMenuItem::*;
        let 실패 = TransferState::Error {
            message: "550".to_owned(),
        };

        assert_eq!(
            row_menu_items(std::slice::from_ref(&실패), true),
            vec![Retry, RetryAll, Remove, RemoveAll]
        );
        assert_eq!(
            row_menu_items(&[TransferState::Wait], true),
            vec![Cancel, RetryAll, RemoveAll],
            "진행 중·대기에는 `삭제`가 없다 — 그 자리는 `전송 취소`가 맡는다"
        );
        assert_eq!(
            row_menu_items(&[TransferState::Active { sent: 1, speed: 1 }], true),
            vec![Cancel, RetryAll, RemoveAll]
        );
        assert_eq!(
            row_menu_items(&[TransferState::Done], true),
            vec![RetryAll, Remove, RemoveAll]
        );

        // 보이는 목록에 실패가 없으면 `전체 다시 시도`를 내지 않는다
        assert_eq!(
            row_menu_items(&[TransferState::Done], false),
            vec![Remove, RemoveAll]
        );
        assert_eq!(
            row_menu_items(&[TransferState::Wait], false),
            vec![Cancel, RemoveAll]
        );
    }

    /// 취소한 줄은 실패 줄과 **같은 넷**을 든다 — 목록에 남기기로 한 이유가 다시 거는 것이다
    #[test]
    fn 취소한_줄의_메뉴는_실패_줄과_같다() {
        use RowMenuItem::*;
        assert_eq!(
            row_menu_items(&[TransferState::Cancelled], true),
            vec![Retry, RetryAll, Remove, RemoveAll]
        );
    }

    /// 목록에 취소분만 있어도 `전체 다시 시도`가 선다 — `retry`가 그것을 되살리므로
    /// 눌러 일이 일어난다
    #[test]
    fn 취소분만_있어도_전체_다시_시도가_선다() {
        use RowMenuItem::*;
        // `has_retryable_in_view` 판정과 같은 식이다(`show`가 목록 전량에서 계산한다)
        let 목록 = [TransferState::Cancelled];
        let 다시_걸_것이_있다 = 목록.iter().any(|state| state.is_retryable());
        assert!(다시_걸_것이_있다);
        assert_eq!(
            row_menu_items(&[TransferState::Cancelled], 다시_걸_것이_있다),
            vec![Retry, RetryAll, Remove, RemoveAll]
        );
    }

    /// `이유` 열은 취소를 서버 사유와 갈라 적는다 — 취소는 서버가 준 문자열이 없다
    #[test]
    fn 이유_열은_취소를_사용자_취소로_적는다() {
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        let (취소, 취소색) = reason_text(&TransferState::Cancelled);
        assert_eq!(취소, "사용자 취소");

        let (실패, 실패색) = reason_text(&TransferState::Error {
            message: "550 권한 거부".to_owned(),
        });
        assert_eq!(실패, "550 권한 거부", "서버가 준 사유는 그대로 적는다");
        assert_ne!(취소색, 실패색, "같은 탭에 섞이므로 색으로 갈린다");

        assert_eq!(
            reason_text(&TransferState::Done).0,
            "",
            "사유가 없는 상태는 빈칸이다"
        );
    }

    /// 상태 열도 취소를 적는다 — T2가 컴파일 때문에 먼저 넣은 자리를 여기서 고정한다
    #[test]
    fn 상태_열은_취소를_취소됨으로_적는다() {
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        let item = 취소된_항목();
        assert_eq!(state_text(&item, 3).0, "취소됨");
    }

    fn 취소된_항목() -> TransferItem {
        let mut queue = crate::remote::queue::TransferQueue::new();
        let id = queue.enqueue(
            crate::remote::types::SiteId(1),
            crate::remote::connection::TransferDirection::Upload,
            std::path::PathBuf::from(r"C:\그만둔.zip"),
            crate::remote::types::RemotePath::new("/그만둔.zip"),
            100,
        );
        queue.cancel(id);
        queue.get(id).expect("항목").clone()
    }

    #[test]
    fn 행_메뉴_문구는_카탈로그를_거친다() {
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        // 사용자 요청 원문 그대로다 — `모두 …`가 아니라 `전체 …`
        assert_eq!(crate::i18n::queue_retry(), "다시 시도");
        assert_eq!(crate::i18n::queue_retry_all(), "전체 다시 시도");
        assert_eq!(crate::i18n::queue_cancel(), "전송 취소");
        assert_eq!(crate::i18n::queue_remove(), "삭제");
        assert_eq!(crate::i18n::queue_remove_all(), "전체 삭제");
    }

    #[test]
    fn 연결별_탭_건수가_거르개를_따르고_로그에서는_사라진다() {
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        // 사용자 보고(2026-08-18): `성공` 탭인데 아래 줄이 `전체 (1) · LG (1)`이었다
        let mut sites = SiteStore::new();
        let lg = sites.add("LG");
        let mut queue = TransferQueue::new();
        let 실패 = queue.enqueue(
            lg,
            TransferDirection::Upload,
            PathBuf::from(r"C:\a"),
            RemotePath::new("/a"),
            1,
        );
        queue.update(
            실패,
            TransferState::Error {
                message: "550".to_owned(),
            },
        );
        // 연결은 없다 — 그래도 큐에 항목이 있으니 탭 자리는 남아야 한다
        let view = DockView {
            queue: &queue,
            failed: &[],
            connected: &[],
        };

        let 그린다 = |filter: QueueFilter, show_counts: bool| {
            let mut state = DockState {
                filter,
                ..DockState::default()
            };
            let ctx = egui::Context::default();
            let mut texts = Vec::new();
            let output = ctx.run_ui(Default::default(), |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    let rect = egui::Rect::from_min_size(
                        ui.max_rect().min,
                        egui::vec2(900.0, SITE_ROW_HEIGHT),
                    );
                    show_site_tabs(ui, rect, &mut state, &view, &sites, show_counts);
                });
            });
            for clipped in &output.shapes {
                if let egui::Shape::Text(text) = &clipped.shape {
                    texts.push(text.galley.text().to_owned());
                }
            }
            texts
        };

        let 실패_탭 = 그린다(QueueFilter::Error, true);
        assert!(실패_탭.contains(&"전체 (1)".to_owned()), "{실패_탭:?}");
        assert!(실패_탭.contains(&"LG (1)".to_owned()), "{실패_탭:?}");

        let 성공_탭 = 그린다(QueueFilter::Done, true);
        assert!(성공_탭.contains(&"전체 (0)".to_owned()), "{성공_탭:?}");
        assert!(
            성공_탭.contains(&"LG (0)".to_owned()),
            "성공 0건이어도 LG 탭은 남고 건수만 0이어야 한다: {성공_탭:?}"
        );

        // 로그 화면 — 셀 대상이 없어 건수를 적지 않는다
        let 로그 = 그린다(QueueFilter::All, false);
        assert!(로그.contains(&"전체".to_owned()), "{로그:?}");
        assert!(로그.contains(&"LG".to_owned()), "{로그:?}");
        assert!(
            !로그.iter().any(|text| text.contains('(')),
            "로그 화면에 건수가 남았다: {로그:?}"
        );
    }
}
