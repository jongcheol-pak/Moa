//! 세션 저장/복원 — 실행 파일 옆의 `settings.json` (FR-11·FR-20, plan D9·D18, NFR-7)
//!
//! 스키마 v3: v2 + {sites, queue[], dock} 이며 탭이 문자열에서 `TabSession{kind,path,site}`로
//! 넓어졌다 (FR-44). v2 파일은 **승격**되어 그대로 살아난다(`promote_v2`) — 원격 쪽 필드는
//! 비어 있는 기본값이 된다. v1과 손상·미래 버전은 종전대로 "세션 없음"으로 폴백한다.
//!
//! 스키마 v2: {version, window{x,y,w,h,maximized}, sidebar{width,collapsed}, active_workspace,
//! workspaces[{name, layout<트리 재귀>, panels[{tabs,active_tab}], active_panel}]}.
//! 각 워크스페이스의 panels 배열은 그 layout 리프의 walk 순서(좌→우, 상→하)와 1:1 대응한다.
//! 히스토리는 저장하지 않는다 — 경로만 (D15: 재시작 후 히스토리 초기화는 관례적 체감).
//! 손상·구버전(v1)·미래 version 파일은 전부 "세션 없음"으로 폴백한다 (사용자 결정: 마이그레이션 없음).
use crate::app::layout::{SplitDir, TreeShape};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 현재 스키마 버전 — 필드가 바뀌면 올리고 하위 호환 처리를 추가한다 (D15).
/// v1(워크스페이스 개념 이전) 파일은 폴백되어 기본 워크스페이스 1개로 시작한다
pub const SESSION_VERSION: u32 = 3;

/// 승격해 읽을 수 있는 가장 낮은 버전 (D17) — v1은 워크스페이스 개념 이전이라 폴백이다
const PROMOTABLE_VERSION: u32 = 2;

/// 세션에 담는 전송 큐 항목 수의 상한 (plan Halt Forecast ii-a).
///
/// 큐가 1만 건이어도 저장은 앞의 1000건까지만 한다 — 세션 파일은 시작할 때 통째로 읽히므로
/// 무한정 커지면 창이 뜨는 시간이 그만큼 늘어난다. 넘친 것은 조용히 버린다(전송은 다시
/// 끌어다 놓으면 된다)
pub const QUEUE_SESSION_LIMIT: usize = 1000;

/// 사이드바 기본·최소·최대 폭(px, 96DPI 기준 — plan `## 시각 요소 분해`).
/// 저장값 검증이 이 범위를 쓰므로 세션 모듈이 소유하고, 사이드바 창(T4·T7)이 같은 상수를 참조한다
pub const SIDEBAR_DEFAULT_WIDTH: i32 = 260;
pub const SIDEBAR_MIN_WIDTH: i32 = 160;
pub const SIDEBAR_MAX_WIDTH: i32 = 480;

const FILE_NAME: &str = "settings.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub window: WindowState,
    pub sidebar: SidebarSession,
    /// 재시작 시 화면에 띄울 워크스페이스 (workspaces 인덱스)
    pub active_workspace: usize,
    pub workspaces: Vec<WorkspaceSession>,
    /// 등록된 사이트 (FR-44). v2 파일에는 없어 승격 시 빈 목록이 된다
    #[serde(default)]
    pub sites: SiteSession,
    /// 아직 끝나지 않은 전송들 — **다시 시작하지는 않는다**(FR-44)
    #[serde(default)]
    pub queue: Vec<QueueSession>,
    /// 하단 도크의 열림 상태
    #[serde(default)]
    pub dock: DockSession,
    /// 앱 전역 설정 (FR-47). 스키마 버전을 올리지 않고 더한다 — 버전이 다르면
    /// `parse_session`이 통째로 폴백해 기존 워크스페이스가 전부 초기화되기 때문이다 (D2)
    #[serde(default, deserialize_with = "settings_or_default")]
    pub settings: AppSettings,
    /// 폴더 트리 즐겨찾기 (FR-56) — 더한 차례 그대로의 로컬 폴더 경로들.
    ///
    /// **경로가 아니라 문자열로 담는다** — `PathBuf`는 UTF-8이 아닌 경로에서 직렬화 자체가
    /// 실패하는데 `save_session`은 그 실패를 조용히 삼켜 **세션 저장이 통째로 무산**된다.
    /// 즐겨찾기 하나 때문에 창 위치·탭까지 잃지 않으려는 것이며, 탭 경로(`TabSession.path`)도
    /// 같은 이유로 문자열이다 (plan D9).
    /// `settings`와 같이 스키마 버전을 올리지 않고 더한다 (D2)
    #[serde(default, deserialize_with = "favorites_or_default")]
    pub favorites: Vec<String>,
}

/// 손상된 `settings`를 **그 자리에서만** 삼킨다 — 세션 전체를 잃지 않기 위해서다.
///
/// `#[serde(default)]`는 키가 **없을 때만** 기본값을 준다. 키는 있는데 타입이 어긋나면
/// (`"auto_start": "yes"`처럼 손으로 편집됐거나, 설정이 객체가 아닌 값으로 덮인 경우)
/// 그 오류가 `Session` 역직렬화 전체로 번져 워크스페이스·분할·탭까지 통째로 폴백된다.
/// 설정 하나 때문에 탐색 상태를 잃는 것은 대가가 맞지 않으므로 여기서 끊는다
fn settings_or_default<'de, D>(deserializer: D) -> Result<AppSettings, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // 일단 원시 값으로 받아 낸 뒤 변환을 시도한다 — 이 단계에서 실패하면 그것은
    // JSON 자체가 깨진 것이라 어차피 세션 전체가 읽히지 않는다
    let raw = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}

/// 손상된 `favorites`를 **그 자리에서만** 삼킨다 — `settings_or_default`와 같은 판단이다.
///
/// 즐겨찾기 목록이 손으로 편집돼 타입이 어긋나면(문자열 하나가 통째로 들어오는 등) 그 오류가
/// `Session` 역직렬화 전체로 번져 워크스페이스·분할·탭·큐까지 함께 폴백된다. 즐겨찾기 하나
/// 때문에 탐색 상태를 잃는 것은 대가가 맞지 않으므로 여기서 끊는다 (plan D5)
fn favorites_or_default<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(raw).unwrap_or_default())
}

/// 앱 전역 설정 한 벌 (FR-47~FR-53).
///
/// 화면(설정 대화)이 이 값을 바꾸고 `ui::app`이 각 기능에 나눠 준다.
/// **항목별 트레이트·옵저버를 두지 않는다**(plan 비추상화 선언) — 값 몇 개를 매 프레임
/// 읽는 것으로 충분하고, 갈래를 만들면 설정 하나 늘 때마다 배선이 함께 는다
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// 앱 글꼴 이름 (FR-48). `None`이면 기본 글꼴(맑은 고딕)
    pub font_family: Option<String>,
    /// 윈도우 시작 시 자동 실행 (FR-49).
    ///
    /// **정본은 레지스트리이고 이 값은 사본이다** — 다른 도구가 Run 키를 지웠을 수 있어
    /// 화면에 보일 때는 레지스트리를 다시 읽는다 (T6). 그래서 **읽는 코드가 없다** —
    /// 지우지 않는 이유는 세션 파일의 스키마를 이 구조체가 그대로 정하기 때문이다
    pub auto_start: bool,
    /// 닫기를 누르면 종료 대신 트레이로 보낸다 (FR-50)
    pub tray_on_close: bool,
    /// 목록에 파일 확장명을 보인다 (FR-52). 끄면 이름만 보인다
    pub show_extensions: bool,
    /// 숨김 속성이 붙은 항목을 목록에 보인다 (FR-13)
    pub show_hidden: bool,
    /// 시스템 속성이 붙은 항목을 목록에 보인다 (FR-13).
    ///
    /// **숨김과 따로 두는 이유**: 두 속성에 각자의 토글이 대응한다. 둘 다 붙은 항목
    /// (`pagefile.sys` 등)은 두 값이 모두 켜져야 보인다 — 탐색기의 `숨김 파일 표시`와
    /// `보호된 운영 체제 파일 숨기기` 조합과 같은 규칙이다
    pub show_system: bool,
    /// 화면 문구 언어 (FR-53). part2가 실제 전환에 쓰고, 이 part는 저장만 한다
    pub language: LanguageSetting,
    /// 보이는 원격 탭의 목록을 주기적으로 다시 조회한다 (FR-67).
    ///
    /// 끄면 **원격 재조회만** 멈춘다 — 네트워크 드라이브·즐겨찾기 재확인은 로컬 일이라
    /// 서버 부담과 무관해서 계속 돈다
    pub remote_auto_refresh: bool,
    /// 원격 재조회 주기(초) (FR-67).
    ///
    /// **읽을 때 `REMOTE_REFRESH_CHOICES`의 범위로 자른다** — 손으로 고친 설정 파일이
    /// 1초를 담고 있으면 서버가 막는다
    pub remote_refresh_secs: u32,

    /// 전송이 실패했을 때 다시 걸어 볼 횟수 (FR-37).
    ///
    /// **읽을 때 `TRANSFER_RETRY_RANGE`로 자른다** — 손으로 고친 설정 파일이 0이나 99를
    /// 담고 있으면 드롭다운이 지금 값을 찾지 못해 첫 항목으로 보인다
    pub transfer_retry_count: u32,
}

/// 원격 재조회 주기로 고를 수 있는 값(초) — 설정 화면의 선택지이자 허용 범위다 (FR-67).
///
/// **자유 입력이 아니라 목록인 이유**: 쓸모 있는 값이 몇 개뿐인데 1초 단위 입력칸을 두면
/// 사용자가 서버가 막을 만큼 짧은 값을 넣을 수 있고, 화살표로 올리는 칸은 30초에서 10분까지
/// 가는 데 수백 번을 눌러야 한다
pub const REMOTE_REFRESH_CHOICES: [u32; 5] = [10, 30, 60, 300, 600];

/// 기본 주기(초) — 사용자가 조작 중임을 눈치채지 못할 만큼 길고,
/// 협업 중 남의 변경을 놓치지 않을 만큼 짧은 값으로 잡았다 (D6)
pub const REMOTE_REFRESH_DEFAULT_SECS: u32 = 30;

/// 전송 재시도 횟수로 고를 수 있는 범위 — 설정 화면의 선택지이자 허용 범위다 (FR-37).
///
/// 아래를 1로 두는 것은 **0이 「재시도 안 함」이 아니라 「설정이 없다」로 읽히기 때문**이다 —
/// 끄고 싶으면 1을 골라 한 번만 더 시도하게 두는 편이 뜻이 분명하다.
/// 위를 10으로 두는 것은 그보다 많은 시도가 뜻을 갖는 상황(서버가 오래 죽어 있다)에서는
/// 사용자가 손으로 다시 거는 편이 낫기 때문이다
pub const TRANSFER_RETRY_RANGE: std::ops::RangeInclusive<u32> = 1..=10;

/// 기본 재시도 횟수 — 잠깐 끊긴 연결은 대개 두세 번 안에 돌아오고,
/// 그보다 많이 시도해도 안 되는 실패는 대개 다시 걸어도 안 된다
pub const TRANSFER_RETRY_DEFAULT: u32 = 3;

/// 담긴 값을 허용 범위로 죈다 — 벗어난 값은 가장 가까운 끝으로 간다.
///
/// 기본값으로 떨어뜨리지 않는 이유: 사용자가 크게 잡아 둔 뜻(많이 시도해 달라)을
/// 3으로 되돌리면 의도가 뒤집힌다. 10으로 죄면 그 뜻이 남는다
pub fn transfer_retry_count(saved: u32) -> u32 {
    saved.clamp(*TRANSFER_RETRY_RANGE.start(), *TRANSFER_RETRY_RANGE.end())
}

/// 담긴 값을 고를 수 있는 값 가운데 하나로 맞춘다 — 모르는 값은 기본 주기로 떨어진다
pub fn remote_refresh_secs(saved: u32) -> u32 {
    if REMOTE_REFRESH_CHOICES.contains(&saved) {
        saved
    } else {
        REMOTE_REFRESH_DEFAULT_SECS
    }
}

impl Default for AppSettings {
    fn default() -> AppSettings {
        AppSettings {
            font_family: None,
            auto_start: false,
            // **켜진 채로 시작한다** — 협업 중 남의 변경을 놓치지 않는 것이 이 기능의 목적이고,
            // 부담이 크다고 느끼면 설정에서 끄면 된다 (D6)
            remote_auto_refresh: true,
            remote_refresh_secs: REMOTE_REFRESH_DEFAULT_SECS,
            transfer_retry_count: TRANSFER_RETRY_DEFAULT,
            tray_on_close: false,
            // 확장자·숨김 항목의 기본값이 `true`인 것은 **지금 동작을 그대로 두기 위해서다**
            // — 이 설정이 생기기 전의 앱은 둘 다 보였다(사용자 확정). 탐색기 기본값(둘 다 숨김)을
            // 따르면 이미 쓰던 사람의 화면이 업데이트만으로 달라진다
            show_extensions: true,
            show_hidden: true,
            // 시스템 항목만 기본이 `false`다 (사용자 확정) — 위 원칙의 예외이며, 그래서
            // **업데이트 직후 시스템 파일이 목록에서 사라진다**(의도된 변화다). 저장된
            // 설정 파일에 이 키가 없을 때도 이 값이 쓰인다(`#[serde(default)]`)
            show_system: false,
            language: LanguageSetting::System,
        }
    }
}

impl AppSettings {
    /// 실제로 적용할 글꼴 이름 — 빈 문자열은 "고르지 않음"과 같게 본다.
    /// 저장 파일이 손으로 편집돼 `""`가 들어오는 경우를 한곳에서 걸러 낸다.
    ///
    /// **이름을 필드(`font_family`)와 다르게 둔 이유**: 같으면 `settings.font_family`(저장값)와
    /// `settings.font_family()`(적용값)가 둘 다 컴파일돼, 글꼴을 적용하는 쪽이 필드를 그대로
    /// 읽어도 아무 경고 없이 이 정규화가 건너뛰어진다
    pub fn selected_font(&self) -> Option<&str> {
        self.font_family
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
    }
}

/// 언어 선택 (FR-53) — `시스템 기본`이면 Windows UI 언어를 따른다
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
// 문자열로 주고받는다 — 알 수 없는 값이 와도 `From`이 기본값으로 받아 주므로
// **설정 파일이 손상돼도 파싱이 실패하지 않는다**(실패하면 세션 전체가 폴백된다)
#[serde(from = "String", into = "String")]
pub enum LanguageSetting {
    #[default]
    System,
    Korean,
    English,
}

impl LanguageSetting {
    /// 저장 키 — 화면 표시 이름과 분리한다(표시 이름은 언어에 따라 바뀌지만 이 값은 고정)
    pub fn key(self) -> &'static str {
        match self {
            LanguageSetting::System => "system",
            LanguageSetting::Korean => "ko",
            LanguageSetting::English => "en",
        }
    }
}

impl From<String> for LanguageSetting {
    fn from(value: String) -> LanguageSetting {
        match value.as_str() {
            "ko" => LanguageSetting::Korean,
            "en" => LanguageSetting::English,
            // "system"과 알 수 없는 값 모두 여기로 — 모르는 키에 기본값을 주는 것이 이 변환의 목적이다
            _ => LanguageSetting::System,
        }
    }
}

impl From<LanguageSetting> for String {
    fn from(value: LanguageSetting) -> String {
        value.key().to_owned()
    }
}

/// 저장되는 사이트 목록.
///
/// `remote::sites::SiteStore`를 **그대로** 담는다 — 필드를 하나하나 옮겨 적는 사본을 두면
/// 사이트에 항목이 늘 때마다(문자셋·동시 연결 수가 그랬다) 두 곳을 함께 고쳐야 하고,
/// 한쪽만 고쳐지면 저장은 되는데 복원이 안 되는 조용한 손실이 생긴다.
/// 비밀번호는 이 타입 안에서 이미 DPAPI로 봉인돼 있다(평문은 어디에도 없다 — FR-28)
pub type SiteSession = crate::remote::sites::SiteStore;

/// 저장되는 전송 큐 항목 하나 (FR-44).
///
/// **끝난 것은 담지 않는다** — 되살릴 이유가 없다. 담는 것은 대기·실패뿐이고, 복원 뒤에도
/// 스스로 시작하지 않는다(연결부터 사용자가 연다)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueueSession {
    /// 어느 사이트의 전송인가 — 그 사이트가 사라졌으면 복원 때 이 항목을 버린다
    pub site: u32,
    /// `upload`/`download`
    pub direction: String,
    pub local: String,
    pub remote: String,
    #[serde(default)]
    pub size: u64,
    /// 실패로 저장된 것이면 그 사유 — 비어 있으면 대기였다
    #[serde(default)]
    pub error: String,
    /// 처음 시작한 시각(FILETIME) — 아직 시작하지 않았으면 `0`.
    ///
    /// **스키마 버전을 올리지 않는다** — `#[serde(default)]`라 이 키가 없는 옛 파일도 그대로
    /// 읽힌다. 버전을 올리면 `parse_session`이 통째로 폴백해 워크스페이스·분할·탭까지
    /// 초기화된다(`DockSession::columns`와 같은 근거)
    #[serde(default)]
    pub started_at: u64,
    /// 끝난 시각(FILETIME) — 아직이면 `0`
    #[serde(default)]
    pub finished_at: u64,
}

/// 하단 도크의 상태 (FR-36·FR-40)
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DockSession {
    // **열려 있었는지는 담지 않는다** — 앱은 언제나 도크가 닫힌 채로 시작하므로
    // 되살릴 값이 없다(2026-08-21 사용자 요청 — FR-44). 종전에는 `panel: String`이
    // 여기 있었고, 옛 설정 파일에 남은 그 키는 serde가 무시한다(`deny_unknown_fields`를
    // 쓰지 않는다). 그래서 스키마 버전(v3)도 올리지 않는다
    /// 큐 화면의 거르개 키
    #[serde(default)]
    pub filter: String,
    /// `전송 큐` 탭의 열 폭 (FR-11).
    ///
    /// **필드가 없는 옛 파일도 그대로 읽혀야 하므로 `default`를 쓴다** — 스키마 버전을 올리면
    /// `parse_session`이 통째로 폴백해 워크스페이스·분할·탭까지 초기화된다
    /// (`PanelSession::columns`와 같은 이유)
    #[serde(default)]
    pub columns: Vec<f32>,
    /// `성공` 탭의 열 폭 — 열 구성이 달라 따로 든다 (FR-36).
    ///
    /// 이 키가 없는 옛 파일은 기본 폭으로 선다. **`columns`를 물려받지 않는다** —
    /// 그쪽은 일곱 열이고 이쪽은 여섯이라 자리가 어긋난다
    #[serde(default)]
    pub columns_done: Vec<f32>,
    /// `실패` 탭의 열 폭 — 위와 같은 이유로 따로 든다
    #[serde(default)]
    pub columns_error: Vec<f32>,
    /// 탭마다 열이 서는 차례 — 저장 키(`QueueColumnKind::as_key`)를 담는다 (FR-36).
    ///
    /// **폭과 마찬가지로 탭마다 따로 든다.** 폭 배열이 그 탭의 **기본** 차례를 뜻하는 것과
    /// 달리 이 목록은 **사용자가 바꾼 차례**다 — 둘을 한 배열로 합치면 폭의 뜻이 순서에
    /// 딸려 흔들린다. 이 키가 없는 옛 파일은 기본 차례로 선다
    #[serde(default)]
    pub column_order: Vec<String>,
    /// `성공` 탭의 열 차례
    #[serde(default)]
    pub column_order_done: Vec<String>,
    /// `실패` 탭의 열 차례
    #[serde(default)]
    pub column_order_error: Vec<String>,
    /// 탭마다 숨긴 열 — 저장 키를 담는다. 고정 열(방향·로컬·원격)은 담기지 않는다
    #[serde(default)]
    pub column_hidden: Vec<String>,
    /// `성공` 탭의 숨긴 열
    #[serde(default)]
    pub column_hidden_done: Vec<String>,
    /// `실패` 탭의 숨긴 열
    #[serde(default)]
    pub column_hidden_error: Vec<String>,
}

/// 사이드바 표시 상태 (FR-19·FR-20) — 화면 상태 타입과 구분해 `Session` 접미사
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SidebarSession {
    pub width: i32,
    pub collapsed: bool,
}

impl Default for SidebarSession {
    fn default() -> SidebarSession {
        SidebarSession {
            width: SIDEBAR_DEFAULT_WIDTH,
            collapsed: false,
        }
    }
}

/// 워크스페이스 한 벌 — 이름 + 분할 구조 + 패널별 탭 (FR-20)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceSession {
    pub name: String,
    pub layout: LayoutNode,
    pub panels: Vec<PanelSession>,
    /// 사이드바 부제(경로) 산출에 쓰는 활성 패널 인덱스 (D18 — 승격 시 활성 패널 복원까지는 하지 않는다)
    pub active_panel: usize,
}

/// 창 위치·크기 (일반 상태 기준 사각형) + 최대화 여부
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub maximized: bool,
}

/// 분할 트리 (직렬화 전용 미러 — layout::TreeShape와 상호 변환)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LayoutNode {
    Leaf,
    Split {
        horizontal: bool,
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

/// 탭 하나가 가리키는 곳 (FR-44).
///
/// v2까지는 문자열 하나(로컬 경로)였다 — 그 형태도 그대로 읽는다(`Deserialize` 참조)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TabSession {
    /// `local`/`remote`
    pub kind: String,
    /// 로컬 경로 또는 원격 경로
    pub path: String,
    /// 원격 탭이 가리키는 사이트 — 로컬이면 `None`
    pub site: Option<u32>,
}

/// 원격 탭이 담는 것 — 사이트와 그 안의 경로 (plan 신규 심볼 `RemoteTabSession`)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteTabSession {
    pub site: u32,
    pub path: String,
}

impl TabSession {
    pub const LOCAL: &'static str = "local";
    pub const REMOTE: &'static str = "remote";

    pub fn local(path: String) -> TabSession {
        TabSession {
            kind: TabSession::LOCAL.to_owned(),
            path,
            site: None,
        }
    }

    pub fn remote(remote: RemoteTabSession) -> TabSession {
        TabSession {
            kind: TabSession::REMOTE.to_owned(),
            path: remote.path,
            site: Some(remote.site),
        }
    }

    /// 원격 탭이면 그 사이트와 경로 — 로컬이면 `None`
    pub fn as_remote(&self) -> Option<RemoteTabSession> {
        if self.kind != TabSession::REMOTE {
            return None;
        }
        Some(RemoteTabSession {
            site: self.site?,
            path: self.path.clone(),
        })
    }
}

impl From<&str> for TabSession {
    /// 시험과 레거시 어댑트가 로컬 탭을 짧게 적기 위한 길
    fn from(path: &str) -> TabSession {
        TabSession::local(path.to_owned())
    }
}

impl<'de> Deserialize<'de> for TabSession {
    /// v2의 문자열 탭과 v3의 객체 탭을 **둘 다** 읽는다.
    ///
    /// 승격(`promote_v2`)이 형태까지 바꾸려면 세션 전체를 두 번 파싱해야 한다 —
    /// 여기서 받아들이면 한 번으로 끝난다
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<TabSession, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Legacy(String),
            Tab {
                kind: String,
                path: String,
                #[serde(default)]
                site: Option<u32>,
            },
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Legacy(path) => TabSession::local(path),
            Repr::Tab { kind, path, site } => TabSession { kind, path, site },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PanelSession {
    /// 탭이 가리키는 곳들 (탭 순서)
    pub tabs: Vec<TabSession>,
    pub active_tab: usize,
    /// 자세히 보기 열 폭 4개 (이름·크기·종류·수정한 날짜). 패널마다 독립이다.
    ///
    /// **필드가 없는 옛 파일도 그대로 읽혀야 하므로 `default`를 쓴다** — 스키마 버전을 올리면
    /// `parse_session`이 통째로 폴백해 워크스페이스·분할·탭까지 초기화된다 (plan D5).
    /// 빈 벡터는 "저장된 폭 없음"이며 복원 시 기본 폭이 된다
    #[serde(default)]
    pub columns: Vec<f32>,
    /// 보기 모드 키 (FR-23). 빈 문자열은 "저장 안 됨"이며 복원 시 기본값(자세히)이 된다.
    /// 열 폭과 같은 이유로 `default`를 쓴다 — 스키마 버전을 올리면 옛 세션이 통째로 버려진다
    #[serde(default)]
    pub view_mode: String,
    /// 정렬 기준 키 (FR-4). 빈 문자열은 "저장 안 됨"이며 복원 시 기본값(이름)이 된다.
    /// 위 둘과 같은 이유로 `default`를 쓴다
    #[serde(default)]
    pub sort_key: String,
    /// 정렬 방향 — 참이면 오름차순 (FR-4).
    ///
    /// **`sort_key`가 비면 이 값을 읽지 않는다** — 둘은 언제나 함께 담기므로, 정렬을 한 번도
    /// 저장한 적 없는 옛 파일에서 `bool`의 기본값 `false`가 내림차순으로 새어 나오지 않는다
    #[serde(default)]
    pub sort_ascending: bool,
    /// 자세히 보기 열이 서는 차례 — 열 키 배열이다 (FR-4). 비면 "저장 안 됨"이며 기본 차례가 된다.
    /// 위 값들과 같은 이유로 `default`를 쓴다
    #[serde(default)]
    pub column_order: Vec<String>,
}

impl LayoutNode {
    fn leaf_count(&self) -> usize {
        match self {
            LayoutNode::Leaf => 1,
            LayoutNode::Split { first, second, .. } => first.leaf_count() + second.leaf_count(),
        }
    }

    /// 직렬화 노드 → 레이아웃 스냅숏
    pub fn to_shape(&self) -> TreeShape {
        match self {
            LayoutNode::Leaf => TreeShape::Leaf,
            LayoutNode::Split {
                horizontal,
                ratio,
                first,
                second,
            } => TreeShape::Split {
                dir: if *horizontal {
                    SplitDir::Horizontal
                } else {
                    SplitDir::Vertical
                },
                ratio: *ratio,
                first: Box::new(first.to_shape()),
                second: Box::new(second.to_shape()),
            },
        }
    }

    /// 레이아웃 스냅숏 → 직렬화 노드
    pub fn from_shape(shape: &TreeShape) -> LayoutNode {
        match shape {
            TreeShape::Leaf => LayoutNode::Leaf,
            TreeShape::Split {
                dir,
                ratio,
                first,
                second,
            } => LayoutNode::Split {
                horizontal: matches!(dir, SplitDir::Horizontal),
                ratio: *ratio,
                first: Box::new(LayoutNode::from_shape(first)),
                second: Box::new(LayoutNode::from_shape(second)),
            },
        }
    }
}

/// 세션 파일 경로 — **실행 파일과 같은 폴더**다 (2026-08-21 사용자 결정).
///
/// 설치본은 자기 설치 폴더 안에, 개발 실행은 `target\debug\` 안에 설정을 둔다 — 설치 프로그램이
/// 폴더를 지우면 설정도 함께 사라지고(제거 후 흔적이 남지 않는다), 개발 빌드와 설치본이
/// 서로의 설정을 건드리지 않는다. 실행 파일 위치를 알 수 없는 비정상 환경이면 `None`이다
fn settings_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(FILE_NAME))
}

/// 종료 시 저장 — 디렉터리가 없으면 생성. 실패는 조용히 생략 (T4 Edge: 디스크 풀 등 —
/// 다음 실행은 이전/기본값으로 뜬다)
pub fn save_session(session: &Session) {
    let Some(path) = settings_path() else {
        return;
    };
    let Ok(json) = serde_json::to_string_pretty(session) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, json);
}

/// 시작 시 로드 — 없음/손상/버전 불일치/무결성 위반은 전부 None (기본 레이아웃 폴백)
pub fn load_session() -> Option<Session> {
    let path = settings_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    parse_session(&text)
}

/// 파싱 + 무결성 검증 (파일 I/O와 분리 — 단위테스트 대상).
/// 사이드바 폭만 클램프로 교정하고(정상 사용 중에도 범위를 벗어날 수 있음 — D9),
/// 나머지 위반은 파일 오염으로 보고 전체 폴백한다
pub fn parse_session(text: &str) -> Option<Session> {
    let mut session: Session = serde_json::from_str(text).ok()?;
    match session.version {
        SESSION_VERSION => {}
        // v2는 승격해 살린다 — 워크스페이스·분할·탭·열 폭·보기 모드가 그대로 남는다 (D17)
        PROMOTABLE_VERSION => session = promote_v2(session),
        // v1과 미래 버전 — 기본 레이아웃 폴백 (D15)
        _ => return None,
    }
    if session.workspaces.is_empty() || session.active_workspace >= session.workspaces.len() {
        return None;
    }
    for ws in &session.workspaces {
        // panels는 layout 리프와 1:1 — 어긋나면 파일 오염으로 보고 전체 폴백
        if ws.panels.len() != ws.layout.leaf_count() {
            return None;
        }
        if ws.active_panel >= ws.panels.len() {
            return None;
        }
        if ws
            .panels
            .iter()
            .any(|p| p.tabs.is_empty() || p.active_tab >= p.tabs.len())
        {
            return None;
        }
        // 알 수 없는 탭 종류는 파일 오염으로 본다 — 조용히 로컬로 취급하면 원격 경로가
        // 로컬 경로인 척 되살아난다(quality 리뷰 m1). 종류가 늘어나면 버전을 올린다
        if ws.panels.iter().any(|p| {
            p.tabs.iter().any(|tab| {
                !matches!(tab.kind.as_str(), TabSession::LOCAL | TabSession::REMOTE)
                    || (tab.kind == TabSession::REMOTE && tab.site.is_none())
            })
        }) {
            return None;
        }
        if !layout_ratios_valid(&ws.layout) {
            return None;
        }
    }
    if session.window.w <= 0 || session.window.h <= 0 {
        return None;
    }
    session.sidebar.width = session
        .sidebar
        .width
        .clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
    Some(session)
}

/// v2 세션을 v3로 올린다 (D17).
///
/// 원격 쪽은 v2에 **아무것도 없으므로** 전부 기본값이다(사이트 없음·큐 없음·도크 닫힘).
/// 탭은 읽는 자리(`TabSession`의 `Deserialize`)에서 이미 로컬 탭으로 받아들여져 있다.
///
/// **마이그레이션 프레임워크를 만들지 않는다**(plan 비추상화 선언) — 한 단계뿐이고,
/// 다음 단계가 생기면 그때 v3→v4 함수를 하나 더 둔다
pub fn promote_v2(session: Session) -> Session {
    Session {
        version: SESSION_VERSION,
        ..session
    }
}

/// 비율 유한성 검사 (NaN/무한대 오염 방어 — 재구성 clamp의 1차 관문)
fn layout_ratios_valid(node: &LayoutNode) -> bool {
    match node {
        LayoutNode::Leaf => true,
        LayoutNode::Split {
            ratio,
            first,
            second,
            ..
        } => ratio.is_finite() && layout_ratios_valid(first) && layout_ratios_valid(second),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 전송_재시도_횟수의_기본값과_허용_범위() {
        // 요청 — 1~10을 고를 수 있고 기본은 3이다 (FR-37)
        assert_eq!(AppSettings::default().transfer_retry_count, 3);
        assert_eq!(TRANSFER_RETRY_DEFAULT, 3);
        assert_eq!(*TRANSFER_RETRY_RANGE.start(), 1);
        assert_eq!(*TRANSFER_RETRY_RANGE.end(), 10);

        // 범위 안은 그대로
        for n in 1..=10 {
            assert_eq!(transfer_retry_count(n), n);
        }
        // **벗어난 값은 가장 가까운 끝으로 죈다** — 기본값으로 떨어뜨리면 사용자가 크게
        // 잡아 둔 뜻(많이 시도해 달라)이 3으로 뒤집힌다
        assert_eq!(transfer_retry_count(0), 1);
        assert_eq!(transfer_retry_count(99), 10);
        assert_eq!(transfer_retry_count(u32::MAX), 10);
    }

    #[test]
    fn 재시도_키가_없는_옛_설정도_기본값으로_읽힌다() {
        // `#[serde(default)]`라 스키마 버전을 올리지 않았다
        let json = r#"{"font_family": null, "auto_start": true}"#;
        let parsed: AppSettings = serde_json::from_str(json).expect("옛 설정이 거부됐다");
        assert_eq!(parsed.transfer_retry_count, TRANSFER_RETRY_DEFAULT);
        assert!(parsed.auto_start, "나머지 필드가 그대로여야 한다");
    }

    #[test]
    fn 세션_파일은_실행_파일_옆에_놓인다() {
        // 2026-08-21 결정 — 설치 폴더 안에 두어야 제거할 때 함께 지워진다.
        // 경로가 %APPDATA%로 되돌아가면 이 단언이 깨진다
        let path = settings_path().expect("실행 파일 경로");
        let exe = std::env::current_exe().expect("실행 파일 경로");
        assert_eq!(
            path.parent(),
            exe.parent(),
            "실행 파일과 같은 폴더가 아니다"
        );
        assert_eq!(
            path.file_name(),
            Some(std::ffi::OsStr::new("settings.json"))
        );
    }

    fn sample() -> Session {
        Session {
            version: SESSION_VERSION,
            sites: SiteSession::default(),
            queue: Vec::new(),
            dock: DockSession::default(),
            settings: AppSettings::default(),
            favorites: Vec::new(),
            window: WindowState {
                x: 100,
                y: 50,
                w: 1200,
                h: 800,
                maximized: false,
            },
            sidebar: SidebarSession {
                width: 300,
                collapsed: false,
            },
            active_workspace: 1,
            workspaces: vec![
                WorkspaceSession {
                    name: "워크스페이스 1".into(),
                    layout: LayoutNode::Split {
                        horizontal: true,
                        ratio: 0.4,
                        first: Box::new(LayoutNode::Leaf),
                        second: Box::new(LayoutNode::Split {
                            horizontal: false,
                            ratio: 0.5,
                            first: Box::new(LayoutNode::Leaf),
                            second: Box::new(LayoutNode::Leaf),
                        }),
                    },
                    panels: vec![
                        PanelSession {
                            tabs: vec!["C:\\Users".into(), "D:\\".into()],
                            active_tab: 1,
                            ..Default::default()
                        },
                        PanelSession {
                            tabs: vec!["C:\\".into()],
                            active_tab: 0,
                            ..Default::default()
                        },
                        PanelSession {
                            tabs: vec!["C:\\Windows".into()],
                            active_tab: 0,
                            ..Default::default()
                        },
                    ],
                    active_panel: 2,
                },
                WorkspaceSession {
                    name: "자료 정리".into(),
                    layout: LayoutNode::Leaf,
                    panels: vec![PanelSession {
                        tabs: vec!["D:\\작업".into()],
                        active_tab: 0,
                        ..Default::default()
                    }],
                    active_panel: 0,
                },
            ],
        }
    }

    /// v1(워크스페이스 이전) 스키마 원문 — 폴백 검증용
    const V1_JSON: &str = r#"{
        "version": 1,
        "window": {"x": 0, "y": 0, "w": 1200, "h": 800, "maximized": false},
        "layout": "Leaf",
        "panels": [{"tabs": ["C:\\"], "active_tab": 0}]
    }"#;

    #[test]
    fn 직렬화_역직렬화_왕복_동일성() {
        let s = sample();
        let json = serde_json::to_string_pretty(&s).unwrap();
        let back = parse_session(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn 손상_json은_기본값_폴백이다() {
        assert_eq!(parse_session("{invalid json"), None);
        assert_eq!(parse_session(""), None);
        assert_eq!(parse_session("{}"), None);
    }

    #[test]
    fn 미래_버전은_폴백이다() {
        let mut s = sample();
        s.version = SESSION_VERSION + 1;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(parse_session(&json), None);
    }

    #[test]
    fn 구버전_v1_파일은_폴백이다() {
        // 사용자 결정: 마이그레이션 없이 초기화 (기본 워크스페이스 1개로 시작)
        assert_eq!(parse_session(V1_JSON), None);
    }

    #[test]
    fn 패널_리프_수_불일치는_폴백이다() {
        let mut s = sample();
        s.workspaces[0].panels.pop();
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(parse_session(&json), None);
    }

    #[test]
    fn 빈_탭이나_범위_밖_활성은_폴백이다() {
        let mut s = sample();
        s.workspaces[0].panels[1].tabs.clear();
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(parse_session(&json), None);

        let mut s = sample();
        s.workspaces[0].panels[0].active_tab = 9;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(parse_session(&json), None);
    }

    #[test]
    fn 빈_워크스페이스_목록이나_범위_밖_활성_워크스페이스는_폴백이다() {
        let mut s = sample();
        s.workspaces.clear();
        s.active_workspace = 0;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(parse_session(&json), None);

        let mut s = sample();
        s.active_workspace = 9;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(parse_session(&json), None);
    }

    #[test]
    fn 범위_밖_활성_패널은_폴백이다() {
        let mut s = sample();
        s.workspaces[0].active_panel = 3; // 패널은 3개(0~2)
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(parse_session(&json), None);
    }

    #[test]
    fn 사이드바_폭은_범위로_클램프된다() {
        let mut s = sample();
        s.sidebar.width = 100;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(
            parse_session(&json).unwrap().sidebar.width,
            SIDEBAR_MIN_WIDTH
        );

        let mut s = sample();
        s.sidebar.width = 9999;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(
            parse_session(&json).unwrap().sidebar.width,
            SIDEBAR_MAX_WIDTH
        );
    }

    #[test]
    fn 레이아웃_스냅숏_상호_변환_왕복() {
        let s = sample();
        let shape = s.workspaces[0].layout.to_shape();
        assert_eq!(LayoutNode::from_shape(&shape), s.workspaces[0].layout);
    }

    /// v2 파일 원문 — 원격 관련 필드가 하나도 없고 탭이 문자열 배열이다
    fn v2_text() -> String {
        r#"{
            "version": 2,
            "window": {"x": 10, "y": 20, "w": 1280, "h": 800, "maximized": false},
            "sidebar": {"width": 300, "collapsed": true},
            "active_workspace": 0,
            "workspaces": [{
                "name": "작업",
                "layout": {"Split": {"horizontal": true, "ratio": 0.5,
                    "first": "Leaf", "second": "Leaf"}},
                "panels": [
                    {"tabs": ["C:\\Users", "D:\\"], "active_tab": 1,
                     "columns": [200.0, 60.0, 120.0, 90.0], "view_mode": "tiles",
                     "sort_key": "size", "sort_ascending": false,
                     "column_order": ["size", "name", "type", "modified"]},
                    {"tabs": ["C:\\Windows"], "active_tab": 0}
                ],
                "active_panel": 1
            }]
        }"#
        .to_owned()
    }

    #[test]
    fn v2_파일은_승격되어_그대로_살아난다() {
        // Acceptance ① — 워크스페이스·분할·탭·열 폭·보기 모드가 전부 보존된다 (D17)
        let session = parse_session(&v2_text()).expect("v2는 승격돼야 한다");
        assert_eq!(session.version, SESSION_VERSION);
        assert_eq!(session.sidebar.width, 300);
        assert!(session.sidebar.collapsed);
        let workspace = &session.workspaces[0];
        assert_eq!(workspace.name, "작업");
        assert_eq!(workspace.active_panel, 1);
        assert_eq!(workspace.panels[0].active_tab, 1);
        assert_eq!(workspace.panels[0].columns, vec![200.0, 60.0, 120.0, 90.0]);
        assert_eq!(workspace.panels[0].view_mode, "tiles");
        assert_eq!(workspace.panels[0].sort_key, "size");
        assert_eq!(workspace.panels[0].column_order[0], "size");
        assert!(!workspace.panels[0].sort_ascending);
        // 정렬을 담지 않은 패널은 빈 키로 읽힌다 — 복원이 그것을 기본값으로 푼다 (FR-4)
        assert_eq!(workspace.panels[1].sort_key, "");
        // 문자열 탭은 로컬 탭으로 받아들여진다
        let tabs: Vec<&str> = workspace.panels[0]
            .tabs
            .iter()
            .map(|tab| tab.path.as_str())
            .collect();
        assert_eq!(tabs, vec![r"C:\Users", r"D:\"]);
        assert!(
            workspace.panels[0]
                .tabs
                .iter()
                .all(|tab| tab.site.is_none())
        );
        // 원격 쪽은 전부 기본값이다 (plan Edge Case)
        assert!(session.sites.is_empty());
        assert!(session.queue.is_empty());
        assert_eq!(session.dock, DockSession::default());
    }

    #[test]
    fn v1과_미래_버전은_종전대로_폴백이다() {
        // Acceptance ⑥ — 승격 경로는 v2 하나뿐이다
        let v1 = v2_text().replace(r#""version": 2"#, r#""version": 1"#);
        assert!(parse_session(&v1).is_none());
        let v4 = v2_text().replace(r#""version": 2"#, r#""version": 4"#);
        assert!(parse_session(&v4).is_none());
        assert!(parse_session("{망가진").is_none());
    }

    #[test]
    fn 원격_탭과_큐가_왕복해도_같다() {
        // Acceptance ② — v3 왕복이 동일하다
        let mut session = sample();
        session.workspaces[0].panels[0].tabs = vec![
            TabSession::local(r"C:\Users".to_owned()),
            TabSession::remote(RemoteTabSession {
                site: 7,
                path: "/var/www".to_owned(),
            }),
        ];
        session.workspaces[0].panels[0].active_tab = 1;
        session.queue = vec![QueueSession {
            site: 7,
            direction: "upload".to_owned(),
            local: r"C:\work\app.js".to_owned(),
            remote: "/var/www/app.js".to_owned(),
            size: 1234,
            error: String::new(),
            started_at: 0,
            finished_at: 0,
        }];
        session.dock = DockSession {
            filter: "error".to_owned(),
            columns: vec![34.0, 300.0, 300.0, 120.0, 84.0, 118.0, 150.0],
            // 나머지는 기본값이다 — **`..Default::default()`를 쓰는 이유**: 이 리터럴이
            // 모든 필드를 적고 있으면 도크 세션에 키가 하나 늘 때마다 여기가 컴파일되지
            // 않는다(2026-09-07에 실제로 그랬다)
            ..Default::default()
        };

        let text = serde_json::to_string(&session).expect("직렬화");
        let back = parse_session(&text).expect("왕복");
        assert_eq!(back, session);
        let remote_tab = back.workspaces[0].panels[0].tabs[1]
            .as_remote()
            .expect("원격 탭");
        assert_eq!(remote_tab.site, 7);
        assert_eq!(remote_tab.path, "/var/www");
    }

    #[test]
    fn 옛_파일에_남은_도크_열림_키는_무시한다() {
        // `DockSession.panel`을 걷어냈다(2026-08-21 — FR-44). 그 키가 남은 설정 파일이
        // 통째로 폴백하면 사용자의 워크스페이스·분할·탭까지 초기화되므로,
        // **모르는 키를 무시하고 나머지를 읽는다**는 계약을 여기서 못 박는다
        let mut session = sample();
        session.dock = DockSession {
            filter: "error".to_owned(),
            columns: vec![34.0, 300.0, 300.0, 120.0, 84.0, 118.0, 150.0],
            // 나머지는 기본값이다 — **`..Default::default()`를 쓰는 이유**: 이 리터럴이
            // 모든 필드를 적고 있으면 도크 세션에 키가 하나 늘 때마다 여기가 컴파일되지
            // 않는다(2026-09-07에 실제로 그랬다)
            ..Default::default()
        };
        let text = serde_json::to_string(&session).expect("직렬화");
        // 걷어낸 키를 손으로 다시 끼워 넣는다 — 옛 버전이 적어 둔 파일과 같은 모양이다
        let 옛_파일 = text.replace(r#""dock":{"#, r#""dock":{"panel":"queue","#);
        assert!(
            옛_파일.contains(r#""panel":"queue""#),
            "옛 키를 끼워 넣지 못했다 — 이 시험이 아무것도 보지 않는다"
        );
        let back = parse_session(&옛_파일).expect("옛 키 하나 때문에 세션을 통째로 잃었다");
        assert_eq!(back.dock.filter, "error", "나머지 값은 그대로 읽힌다");
        assert_eq!(back.dock.columns[1], 300.0);
        assert_eq!(back.workspaces.len(), session.workspaces.len());
    }

    #[test]
    fn 열_차례_키가_없는_옛_파일도_그대로_읽힌다() {
        // 2026-09-07에 도크 세션에 여섯 키(탭별 차례·숨김)가 늘었다. 그 키가 **없는** 파일이
        // 통째로 폴백하면 사용자의 워크스페이스·분할·탭까지 초기화된다 — 스키마 버전을
        // 올리지 않은 것이 그 방어이고, 이 시험이 그 계약을 못 박는다
        let mut session = sample();
        session.dock = DockSession {
            filter: "done".to_owned(),
            columns: vec![34.0, 300.0, 300.0, 120.0, 84.0, 118.0, 150.0],
            ..Default::default()
        };
        let text = serde_json::to_string(&session).expect("직렬화");
        // 새 키 여섯을 통째로 걷어낸다 — 그 키가 생기기 전 버전이 적은 파일과 같은 모양이다
        let mut 옛_파일 = text.clone();
        for key in [
            "column_order",
            "column_order_done",
            "column_order_error",
            "column_hidden",
            "column_hidden_done",
            "column_hidden_error",
        ] {
            // 마지막 필드에는 뒤따르는 콤마가 없으므로 두 모양을 다 본다
            let 뒤콤마 = format!(r#""{key}":[],"#);
            let 앞콤마 = format!(r#","{key}":[]"#);
            let 지운다 = if 옛_파일.contains(&뒤콤마) {
                뒤콤마
            } else {
                앞콤마
            };
            assert!(
                옛_파일.contains(&지운다),
                "{key}가 직렬화에 없다 — 이 시험이 아무것도 보지 않는다"
            );
            옛_파일 = 옛_파일.replace(&지운다, "");
        }
        let back = parse_session(&옛_파일).expect("새 키가 없다고 세션을 통째로 잃었다");
        assert_eq!(back.dock.filter, "done", "나머지 값은 그대로 읽힌다");
        assert_eq!(back.dock.columns[1], 300.0, "맞춰 둔 폭이 사라졌다");
        assert!(back.dock.column_order.is_empty(), "없는 키는 빈 목록이다");
        assert_eq!(back.workspaces.len(), session.workspaces.len());
    }

    #[test]
    fn 알_수_없는_탭_종류는_폴백이다() {
        // quality 리뷰 m1 — 조용히 로컬로 취급하면 원격 경로가 로컬 경로인 척 되살아난다
        let mut session = sample();
        session.workspaces[0].panels[0].tabs = vec![TabSession {
            kind: "무엇".to_owned(),
            path: "/var/www".to_owned(),
            site: Some(1),
        }];
        session.workspaces[0].panels[0].active_tab = 0;
        let text = serde_json::to_string(&session).expect("직렬화");
        assert!(parse_session(&text).is_none(), "모르는 종류를 받아들였다");

        // 사이트를 잃은 원격 탭도 파일 오염이다 — 어느 사이트인지 알 수 없다
        session.workspaces[0].panels[0].tabs = vec![TabSession {
            kind: TabSession::REMOTE.to_owned(),
            path: "/var/www".to_owned(),
            site: None,
        }];
        let text = serde_json::to_string(&session).expect("직렬화");
        assert!(
            parse_session(&text).is_none(),
            "사이트 없는 원격 탭을 받아들였다"
        );
    }

    #[test]
    fn 즐겨찾기_키가_없는_파일도_그대로_살아난다() {
        // 이 필드가 생기기 전에 저장된 파일 — 세션 전체가 폴백되면 안 된다 (plan 전제 1)
        let mut text = serde_json::to_string(&sample()).expect("직렬화");
        let mut value: serde_json::Value = serde_json::from_str(&text).expect("값");
        value.as_object_mut().expect("객체").remove("favorites");
        text = value.to_string();

        let back = parse_session(&text).expect("즐겨찾기 키가 없다고 폴백됐다");
        assert!(back.favorites.is_empty());
        assert_eq!(back.workspaces.len(), sample().workspaces.len());
    }

    #[test]
    fn 즐겨찾기가_깨져도_나머지_세션은_살아난다() {
        // 손으로 편집돼 타입이 어긋난 경우 — 그 자리만 비우고 탐색 상태는 지킨다 (plan D5)
        let mut session = sample();
        session.favorites = vec![r"D:\작업".to_owned()];
        // 큐도 함께 지켜지는지 본다 — 비어 있으면 "살아남았다"가 공허하게 참이 된다
        session.queue = vec![QueueSession {
            direction: "download".to_owned(),
            site: 1,
            local: r"D:\받은 것\app.js".to_owned(),
            remote: "/var/www/app.js".to_owned(),
            size: 1234,
            error: String::new(),
            started_at: 0,
            finished_at: 0,
        }];
        let text = serde_json::to_string(&session).expect("직렬화");
        let mut value: serde_json::Value = serde_json::from_str(&text).expect("값");
        value["favorites"] = serde_json::Value::String("망가짐".to_owned());

        let back = parse_session(&value.to_string()).expect("즐겨찾기 때문에 세션을 잃었다");
        assert!(back.favorites.is_empty(), "깨진 값을 그대로 받았다");
        assert_eq!(
            back.workspaces, session.workspaces,
            "워크스페이스까지 잃었다"
        );
        assert_eq!(back.queue, session.queue, "전송 큐까지 잃었다");
        assert_eq!(back.window, session.window);
    }
}
