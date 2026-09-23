//! 화면 문구 카탈로그와 현재 언어 (FR-53).
//!
//! **이 모듈은 화면 계층을 참조하지 않는다** — `ui`·`remote`·`fs` 어디서나 부르므로
//! 최상위에 두고 의존을 한 방향으로 유지한다(AGENTS 계층 규약). 참조하는 것은
//! `app::settings::LanguageSetting` 하나뿐이며, 그것은 저장 값을 담는 순수 데이터라
//! 이쪽을 되참조하지 않는다
//!
//! 문구는 [`strings!`] 매크로에 한 줄씩 적고, 매크로가 그것을 함수로 펼친다.
//! 키가 곧 함수 이름이라 **오타는 컴파일 오류**이고, 한·영 둘 중 하나를 빠뜨려도
//! 마찬가지다 — 화면을 띄워 봐야 아는 실수를 컴파일러가 먼저 잡는다.
//!
//! 문구에 값이 끼어드는 것(`"폴더 3 파일 12"`)은 매크로가 아니라 **손수 쓴 함수**로 둔다
//! — 조사·어순·복수형이 언어마다 달라 자리표시자로는 담기지 않는다. 그 함수들은 이 파일
//! 아래쪽의 [`dynamic`] 모듈에 모여 있다.
use std::sync::atomic::{AtomicU8, Ordering};

use crate::app::settings::LanguageSetting;

/// 화면에 실제로 쓰이는 언어 — `시스템 기본`은 시작할 때 이 둘 중 하나로 풀린다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Korean,
    English,
}

/// 현재 언어. **잠금 없이 스레드 경계를 넘는다** — 화면은 UI 스레드가 그리지만
/// 원격 워커도 오류 문구를 만든다(`remote::connection`)
static CURRENT: AtomicU8 = AtomicU8::new(KOREAN);

const KOREAN: u8 = 0;
const ENGLISH: u8 = 1;

/// 지금 쓰는 언어. 매 프레임 수백 번 불리므로 원자적 읽기 하나로 끝낸다
pub fn current() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        ENGLISH => Language::English,
        // 알 수 없는 값은 한국어로 — 이 앱의 화면이 원래 한국어다
        _ => Language::Korean,
    }
}

/// 설정 값을 받아 현재 언어를 정한다. `시스템 기본`은 **여기서 한 번** 풀어 둔다.
///
/// 문구를 만들 때마다 Win32에 묻지 않는 이유: 시스템 UI 언어는 앱이 도는 동안 바뀌지
/// 않는다(바꾸려면 로그아웃해야 한다) — 매 프레임 수백 번 부를 값이 아니다 (D5)
pub fn set_language(setting: LanguageSetting) {
    let language = match setting {
        LanguageSetting::Korean => Language::Korean,
        LanguageSetting::English => Language::English,
        LanguageSetting::System => system_language(),
    };
    let value = match language {
        Language::Korean => KOREAN,
        Language::English => ENGLISH,
    };
    CURRENT.store(value, Ordering::Relaxed);
}

/// Windows 사용자 UI 언어가 한국어인가.
///
/// 하위 10비트가 언어를 가리키고 상위 6비트가 지역이다 — `ko-KR`뿐 아니라 다른 한국어
/// 변종도 함께 잡으려면 하위 10비트만 본다
pub fn system_language() -> Language {
    use windows::Win32::Globalization::GetUserDefaultUILanguage;
    /// `LANG_KOREAN` — winnt.h의 값
    const LANG_KOREAN: u16 = 0x12;
    // 안전성: 인자도 반환 자원도 없는 순수 조회다
    let id = unsafe { GetUserDefaultUILanguage() };
    // 실패(0)면 한국어로 — 이 앱의 기존 화면이 한국어이므로 그것이 안전한 쪽이다
    if id == 0 || id & 0x3FF == LANG_KOREAN {
        Language::Korean
    } else {
        Language::English
    }
}

/// 정적 문구를 함수로 펼친다.
///
/// 한 줄이 `키 => "한국어" / "English"` 형태이고, 매크로는 그 줄마다
/// `pub fn 키() -> &'static str`을 만든다. **하는 일은 이 펼침 하나**이며 조건·분기·
/// 타입 조작이 없다 — 반복을 줄이는 것이지 간접화를 더하는 것이 아니다.
macro_rules! strings {
    ($($(#[$doc:meta])* $name:ident => $ko:literal / $en:literal;)*) => {
        $(
            $(#[$doc])*
            pub fn $name() -> &'static str {
                match current() {
                    Language::Korean => $ko,
                    Language::English => $en,
                }
            }
        )*
    };
}

strings! {
    /// 설정 대화 제목
    settings_title => "설정" / "Settings";
    /// 설정 대화의 그룹 제목 — 모양
    settings_group_appearance => "모양" / "Appearance";
    settings_group_startup => "시작" / "Startup";
    settings_group_exit => "종료" / "Exit";
    settings_group_files => "파일 보기" / "Files";
    settings_group_language => "언어" / "Language";
    /// 원격 목록 자동 갱신 그룹 (FR-67)
    settings_group_remote => "원격" / "Remote";
    settings_font => "글꼴" / "Font";
    /// 글꼴을 고르지 않은 상태 — 목록 맨 앞 항목이기도 하다
    settings_font_default => "기본값 (맑은 고딕)" / "Default (Malgun Gothic)";
    /// 목록을 읽는 동안 보이는 안내 — 워커가 1.5초쯤 걸린다
    settings_font_scanning => "글꼴 목록을 읽는 중…" / "Reading font list…";
    settings_auto_start => "윈도우 시작 시 실행" / "Run at Windows startup";
    /// 레지스트리 쓰기가 막힌 환경에서 보이는 안내 — 조용히 되돌리면 왜 안 켜지는지 알 수 없다
    settings_auto_start_failed
        => "시작 프로그램 설정을 바꾸지 못했습니다"
        / "Could not change the startup setting";
    settings_tray_on_close => "닫으면 트레이로 보내기" / "Send to tray on close";
    /// 토글 라벨은 **켰을 때 일어나는 일**을 적는다 — 이름만 두면 켜고 끄는 것이 무엇인지 모른다
    settings_show_extensions => "파일 확장명 표시" / "Show file name extensions";
    settings_show_hidden => "숨김 파일 및 폴더 표시" / "Show hidden files and folders";
    /// 시스템 파일은 숨김과 따로 켠다 — 둘 다 붙은 항목은 두 토글이 모두 켜져야 보인다
    settings_show_system => "시스템 파일 표시" / "Show system files";
    /// 언어 선택지 — `English`는 두 언어에서 같다
    settings_language_system => "시스템 기본" / "System default";
    settings_language_korean => "한국어" / "Korean";
    settings_language_english => "English" / "English";
    close => "닫기" / "Close";
    delete => "삭제" / "Delete";
    rename => "이름 바꾸기" / "Rename";
    connect => "연결" / "Connect";
    /// 컨텍스트 메뉴 아이콘 줄의 툴팁 (FR-8) — 라벨이 없는 줄이라 이름이 여기서만 드러난다.
    /// `삭제`·`이름 바꾸기`는 위의 것을 그대로 쓴다
    menu_cut => "잘라내기" / "Cut";
    menu_copy => "복사" / "Copy";
    /// 컨텍스트 메뉴에서 **앱이 스스로 세우는 두 줄** (FR-8 재개정) — 셸이 주지 않는 것이라
    /// 문구도 우리가 쥔다. 탐색기가 쓰는 말과 같게 맞춘다
    menu_add_favorite => "즐겨찾기에 추가" / "Add to favorites";
    menu_open_new_tab => "새 탭에서 열기" / "Open in new tab";
    /// 빈 곳 우클릭에서 세우는 붙여넣기 줄 (FR-8 재개정 — 2026-08-26).
    ///
    /// **셸이 주는 같은 줄은 숨긴다** — 그쪽은 클립보드가 비면 아예 오지 않아 흐린 상태를
    /// 만들 수 없다
    menu_paste => "붙여넣기" / "Paste";
    /// 서드파티 셸 확장을 모아 두는 하위 메뉴의 머리 (FR-8 재개정)
    menu_app_extensions => "앱 확장" / "App extensions";
    menu_upload => "업로드" / "Upload";
    /// 컨텍스트 메뉴 맨 아래 줄 (FR-8 재개정) — 종전 Windows 표준 메뉴를 연다.
    ///
    /// **탐색기에는 없는 줄이다.** 그쪽은 같은 자리에 「옵션을 더 보인다」는 뜻의 줄을 두는데,
    /// 이 앱은 그 위를 이미 우리가 그리므로 「표준 메뉴로 간다」는 뜻이 더 곧다
    /// (2026-08-22 사용자 요청)
    menu_default => "기본 메뉴" / "Default menu";
    /// 압축 항목을 모아 두는 하위 메뉴의 머리 (FR-8 재개정 — 2026-08-26)
    menu_compress_to => "다음으로 압축" / "Compress to";
    /// 그 하위의 첫 줄 — **Windows 기본 압축**이다(`fs::zip_shell`).
    ///
    /// 탐색기의 같은 자리 문구를 따른다. 그 아래에 서는 것들은 설치된 압축 프로그램이
    /// 셸에 등록한 항목이라 문구를 우리가 정하지 않는다
    menu_compress_zip => "Zip 파일" / "Zip file";
    /// 압축 해제 항목을 모아 두는 하위 메뉴의 머리 (FR-8 재개정 — 2026-08-26).
    ///
    /// **`다음으로 압축`과 같은 자리를 번갈아 쓴다** — 압축 파일을 골랐으면 이 줄이 선다
    menu_extract_to => "압축 풀기" / "Extract";

    // ── 타이틀바 (FR-22) ──
    titlebar_show_workspaces => "워크스페이스 목록 보이기" / "Show workspace list";
    titlebar_hide_workspaces => "워크스페이스 목록 숨기기" / "Hide workspace list";
    titlebar_restore => "이전 크기로" / "Restore";
    titlebar_maximize => "최대화" / "Maximize";
    titlebar_minimize => "최소화" / "Minimize";
    /// 설정 메뉴 다섯 항목이 모두 동작한다 — `설정`은 FR-47, `업데이트`는 FR-62,
    /// `릴리즈 노트`는 FR-63, `오픈소스 라이선스`는 FR-57, `정보`는 FR-58이다
    /// (`오픈소스 라이선스`는 그 대화의 제목으로도 쓰인다)
    titlebar_updates => "업데이트" / "Updates";
    titlebar_release_notes => "릴리즈 노트" / "Release notes";
    titlebar_licenses => "오픈소스 라이선스" / "Open source licenses";

    // ── 자동 업데이트 (FR-62·FR-63) ──
    /// 타이틀바 배지 — 새 판이 있을 때. 제목 줄에 서므로 짧게 둔다
    update_available => "업데이트" / "Update";
    /// 배지 — 받는 중. 말줄임표는 「아직 끝나지 않았다」는 뜻으로 화면 관례를 따른다
    update_downloading => "다운로드 중..." / "Downloading...";
    /// 손으로 확인했을 때, 이미 최신이면 알린다 (그냥 확인만 하고 아무 표시도 없으면
    /// 눌린 것인지 알 수 없다)
    update_latest => "최신 버전입니다" / "You are up to date";
    /// 확인이 실패했을 때 — 사용자가 할 수 있는 일(연결 확인·나중에 다시)을 함께 적는다
    update_check_failed
        => "업데이트를 확인하지 못했습니다. 인터넷 연결을 확인해 주세요"
        / "Could not check for updates. Please check your internet connection";
    /// 내려받기가 실패했을 때
    update_download_failed
        => "업데이트를 받지 못했습니다. 잠시 후 다시 시도해 주세요"
        / "Could not download the update. Please try again later";
    /// 받은 파일이 릴리즈에 적힌 값과 다를 때 — 「손상」이라고만 적는다(체크섬·해시 같은
    /// 말은 일반 사용자에게 뜻이 닿지 않는다)
    update_verify_failed
        => "받은 파일이 손상되어 설치를 멈췄습니다"
        / "The downloaded file was damaged, so the update was stopped";
    /// 설치 프로그램을 띄우지 못했을 때
    update_launch_failed
        => "설치 프로그램을 실행하지 못했습니다. 잠시 후 다시 시도해 주세요"
        / "Could not start the installer. Please try again later";
    /// 전송이 도는 중에 설치를 누르면 뜨는 확인 대화의 제목
    update_confirm_title => "지금 업데이트할까요?" / "Update now?";
    /// 그 대화의 실행 버튼
    update_confirm_ok => "업데이트" / "Update";
    titlebar_about => "정보" / "About";

    // ── 앱 이름 (FR-53·FR-58) ──
    /// 화면에 보이는 앱 이름 — 한국어는 `모아`, 영어는 `MOA`다.
    ///
    /// 정보 대화뿐 아니라 **창 제목(작업 표시줄·Alt+Tab)과 트레이 툴팁**도 이 값을 쓴다.
    /// 데이터가 걸린 이름(레지스트리 값·설정 파일·단일 인스턴스 뮤텍스)은
    /// **여기를 따르지 않는다** — 언어에 따라 바뀌면 자동 실행 등록과 기존 설정을 잃는다
    app_name => "모아" / "MOA";

    // ── 오픈소스 라이선스 대화 (FR-57) ──
    /// 목록 위 안내 — 이 앱이 무엇을 쓰는지 한 줄로 밝힌다
    licenses_intro
        => "이 앱은 아래 오픈소스 구성 요소를 사용합니다"
        / "This app uses the open source components listed below";
    licenses_copyright => "저작권" / "Copyright";
    /// 배포 패키지에 원문이 없어 표준 전문을 대신 보이는 항목에 붙는다
    licenses_standard_note
        => "이 구성 요소는 배포 파일에 라이선스 원문을 담고 있지 않아 해당 라이선스의 표준 전문을 보입니다"
        / "This component ships without its license text, so the standard text for that license is shown";
    /// 크레이트가 아니라 함께 담겨 나가는 것(C 라이브러리·글꼴)에 붙는다
    licenses_bundled_note
        => "이 앱의 실행 파일에 함께 담겨 나갑니다"
        / "Bundled into this app's executable";
    /// 자산을 읽지 못했을 때 목록 자리에 적는다
    licenses_unavailable
        => "라이선스 정보를 읽지 못했습니다"
        / "Could not read the license information";

    // ── 정보 대화 (FR-58) ──
    /// 이름·버전 줄 아래에 서는 세 줄.
    ///
    /// **두 언어에서 값이 같다** — 저작권 표기와 라이선스 이름은 번역 대상이 아니고
    /// (SPDX 식별자 `MIT`가 그 자체로 이름이다) 주소도 언어를 타지 않는다. 그런데도
    /// 카탈로그를 거치는 것은 화면 문구를 소스에 박지 않는다는 규약(AGENTS) 때문이다
    about_copyright
        => "Copyright (c) 2026 jongcheol-pak"
        / "Copyright (c) 2026 jongcheol-pak";
    about_license => "MIT License" / "MIT License";
    about_repository_url
        => "https://github.com/jongcheol-pak/Moa"
        / "https://github.com/jongcheol-pak/Moa";
    /// 릴리즈 노트 페이지 (FR-63) — **특정 판이 아니라 목록**이다. 두 언어에서 값이 같고
    /// 카탈로그를 거치는 이유는 위 저장소 주소와 같다(화면 문구를 소스에 박지 않는다)
    releases_url
        => "https://github.com/jongcheol-pak/Moa/releases"
        / "https://github.com/jongcheol-pak/Moa/releases";

    // ── 패널 메뉴 (FR-23) ──
    /// 열 메뉴 캡션 (인벤토리 #22)
    menu_columns => "표시할 컬럼" / "Columns";
    menu_view => "보기" / "View";
    menu_refresh => "새로 고침" / "Refresh";
    menu_new_file => "새 파일" / "New file";
    menu_new_folder => "새 폴더" / "New folder";
    menu_split_right => "오른쪽 분할" / "Split right";
    menu_split_left => "왼쪽 분할" / "Split left";
    menu_split_up => "위쪽 분할" / "Split up";
    menu_split_down => "아래쪽 분할" / "Split down";

    // ── 보기 모드 8종 (FR-23) ──
    view_extra_large_icons => "아주 큰 아이콘" / "Extra large icons";
    view_large_icons => "큰 아이콘" / "Large icons";
    view_medium_icons => "보통 아이콘" / "Medium icons";
    view_small_icons => "작은 아이콘" / "Small icons";
    view_list => "목록" / "List";
    view_details => "자세히" / "Details";
    view_tiles => "타일" / "Tiles";
    view_content => "내용" / "Content";

    // ── 워크스페이스 사이드바 (FR-20) ──
    sidebar_workspaces => "워크스페이스" / "Workspaces";
    sidebar_saved_sites => "등록된 사이트" / "Saved sites";
    /// 연결 메뉴의 마지막 항목 — 사이트 관리자를 연다.
    ///
    /// **원본 인벤토리 #8의 문구와 갈린다** — 그쪽은 「추가」만 가리켜 이 항목이 하는 일보다
    /// 좁게 읽혔고, 사용자가 2026-08-20에 바꿔 달라고 했다(등록만이 아니라 관리자 전체를 연다).
    /// 대화 제목(`site_title`)과 값이 같아졌지만 키는 따로 둔다 — 한쪽만 바꿀 때 다시 갈라야 한다
    sidebar_site_manager => "사이트 관리자" / "Site Manager";
    sidebar_new_workspace => "새 워크스페이스" / "New workspace";
    sidebar_refresh_sites => "사이트 목록 새로 고침" / "Refresh site list";
    /// `+` 버튼 툴팁 — 누르면 일어나는 일을 그대로 적는다(`연결`은 무엇이 열리는지 알 수 없다)
    sidebar_connect_menu => "사이트 연결 메뉴" / "Connect to a site";
    /// 사이트가 하나도 없을 때의 안내 — 첫 화면에서 다음에 무엇을 할지 알려 준다
    sidebar_no_sites => "+ 를 눌러 서버를 등록하세요" / "Press + to add a server";

    // ── 탭 스트립·주소창 ──
    tabs_new => "새 탭" / "New tab";
    /// 탭 줄 `+` 버튼의 안내 — 누르면 곧바로 탭이 생기는 것이 아니라 메뉴가 열린다.
    /// `새 탭`을 그대로 쓰면 버튼이 즉시 탭을 만드는 것처럼 읽힌다 (2026-08-19)
    tabs_new_menu => "새 탭 열기" / "Open a new tab";
    tabs_close => "탭 닫기" / "Close tab";
    tabs_menu => "메뉴" / "Menu";
    address_back => "뒤로" / "Back";
    address_forward => "앞으로" / "Forward";
    address_up => "상위 폴더" / "Up";
    /// 주소창 오른쪽 이름 필터 입력란의 자리표시자 (FR-65)
    address_filter_hint => "이름으로 거르기" / "Filter by name";

    // ── 폴더 트리 (FR-9) ──
    /// 아직 읽는 중인 노드의 자리 표시 — 로컬·원격 트리가 같은 문구를 쓴다
    tree_loading => "읽는 중…" / "Loading…";

    /// 트리 항목 우클릭 메뉴 — 이 폴더를 즐겨찾기에 담는다 (FR-56).
    /// 이미 담긴 폴더면 비활성으로 보인다
    /// 즐겨찾기 목록 위 제목 — 목록과 같은 낱말이라 메뉴 쪽을 `…에 담기`로 갈랐다
    tree_favorites_title => "즐겨찾기" / "Favorites";
    tree_favorite_add => "즐겨찾기에 담기" / "Add to favorites";
    /// 즐겨찾기 줄 우클릭 메뉴 — 목록에서 뺀다
    tree_favorite_remove => "해제" / "Remove from favorites";

    // ── 패널·목록·상태 줄 ──
    /// 트리 토글의 툴팁 — 아이콘은 로컬·원격이 같고 이 문구만 갈린다
    panel_folder_tree => "폴더 트리" / "Folder tree";
    panel_remote_tree => "원격 트리" / "Remote tree";
    /// 새로 만들 대상 — 실패 문구에 끼워 넣는다
    panel_kind_folder => "폴더" / "folder";
    panel_kind_file => "파일" / "file";
    column_name => "이름" / "Name";
    column_size => "크기" / "Size";
    column_type => "종류" / "Type";
    column_modified => "수정한 날짜" / "Date modified";
    column_permissions => "권한" / "Permissions";
    column_owner => "소유자" / "Owner";
    /// 상태 표시줄의 큐 토글 (인벤토리 #53)
    status_queue => "전송 큐" / "Transfer queue";
    /// 실패 알약의 낱말 (인벤토리 #57) — 건수는 `dynamic::status_failed_count`가 붙인다
    /// 다 읽었는데 보일 것이 없을 때 (2026-08-16 검토) — 종전에는 빈칸만 남아,
    /// 다 읽은 것인지 실패한 것인지 화면으로 가릴 수 없었다
    list_empty_folder => "이 폴더는 비어 있습니다" / "This folder is empty";
    /// 권한이 막혀 목록을 읽지 못했을 때 그 자리에 적는 말 (2026-08-16 사용자 요청) —
    /// 폴더 이름은 주소창이 이미 보여 주므로 문장에 넣지 않는다
    list_access_denied => "이 폴더를 열 권한이 없어 내용을 표시할 수 없습니다"
        / "You do not have permission to view the contents of this folder";
    /// 네트워크 드라이브가 끊겨 목록을 읽지 못했을 때 (2026-08-17 사용자 요청) —
    /// 일반 실패와 갈라 적는다. 연결을 살피는 것과 다시 열어 보는 것은 할 일이 다르다
    list_network_unavailable => "네트워크 드라이브에 연결할 수 없어 내용을 표시할 수 없습니다"
        / "Cannot connect to the network drive, so its contents cannot be shown";
    /// 그 밖의 사유로 폴더를 열지 못했을 때 (2026-08-17 사용자 요청).
    /// 폴더 이름은 주소창이 이미 보여 주므로 문장에 넣지 않는다
    list_open_failed => "이 폴더를 여는 중 문제가 생겨 내용을 표시할 수 없습니다"
        / "Something went wrong while opening this folder, so its contents cannot be shown";
    /// 폴더를 펼치는 중임을 알리는 문구
    status_expanding => "펼치는 중…" / "Expanding…";
    /// 새로 만드는 폴더·파일의 기본 이름 — 화면 언어를 따라 실제 이름이 정해진다.
    /// 파일 쪽은 Windows 탐색기의 `새로 만들기 > 텍스트 문서`와 같은 이름이다 (사용자 확정)
    create_folder_base => "새 폴더" / "New folder";
    create_file_base => "새 텍스트 문서" / "New Text Document";
    /// 고른 것을 하나도 삭제에 걸지 못했을 때 (FR-64) — 그 사이 전부 사라졌거나 셸이
    /// 그 경로를 다루지 못하는 경우다
    delete_no_source => "지울 파일을 찾지 못했습니다" / "Could not find the files to delete";
    /// 바꿀 이름에 파일 이름으로 쓸 수 없는 글자가 있을 때 (FR-64) — 어떤 글자가 안 되는지
    /// 함께 보여야 사용자가 고칠 수 있다. 셸에 걸기 전에 우리가 먼저 거른다
    rename_invalid_chars
        => r#"이름에 \ / : * ? " < > | 는 쓸 수 없습니다"#
        / r#"A name cannot contain \ / : * ? " < > |"#;
    /// 끌어다 놓은 것을 하나도 복사에 걸지 못했을 때 (FR-60) — 원본이 그 사이 전부
    /// 사라졌거나 셸이 그 경로를 다루지 못하는 경우다
    copy_no_source => "복사할 파일을 찾지 못했습니다" / "Could not find the files to copy";
    /// 같은 이름이 이미 여럿이라 번호를 붙일 자리를 다 써 버렸을 때 —
    /// "무엇을 하다 실패했는지"가 없으면 사용자는 이 말을 알아들을 수 없다
    create_no_name
        => "같은 이름이 너무 많아 새 이름을 붙이지 못했습니다"
        / "Too many items share this name — could not pick a new one";
    /// 자동 워크스페이스 이름의 앞부분 — `워크스페이스 3`처럼 뒤에 번호가 붙는다 (D7)
    workspace_auto_prefix => "워크스페이스 " / "Workspace ";
    /// 사이트를 찾을 수 없을 때 탭에 보일 이름 (사이트가 지워진 뒤 남은 탭)
    tabs_missing_site => "알 수 없는 사이트" / "Unknown site";

    // ── 사이트 관리자 (FR-27) ──
    // 라벨 뒤 `(S)`·`(R)` 같은 접근 키 알파벳은 **영어에서도 그대로 둔다**
    // — 키 배정이 바뀌면 사용자가 익힌 조작이 깨진다
    site_title => "사이트 관리자" / "Site Manager";
    /// 무엇을 고르는 목록인지 이름으로 말한다 — `항목`은 이 화면에서 뜻이 닿지 않는다
    site_list_label => "사이트(S):" / "Site(S):";
    site_rename => "이름 바꾸기(R)" / "Rename(R)";
    site_delete => "삭제(D)" / "Delete(D)";
    site_duplicate => "복제(I)" / "Duplicate(I)";
    /// 삭제 확인 대화 (2026-08-16 검토) — 워크스페이스·원격 파일 삭제에는 확인이 있는데
    /// 사이트만 곧바로 지워, 되돌릴 수 없기로는 같은 일에 안전장치가 자리마다 달랐다
    site_delete_title => "사이트 삭제" / "Delete site";
    site_delete_detail
        => "저장한 주소와 로그인 정보가 함께 사라집니다."
        / "Its address and sign-in details will be removed as well.";
    /// 사이드바 목록에서 지울 때의 확인 (FR-29) — **진행·대기 중인 전송이 있을 때만** 뜬다.
    /// 위 `site_delete_*`(사이트 관리자의 `삭제(D)`)와 다른 일이다: 이쪽은 사이트 기록을 남긴다
    sidebar_site_remove_title => "목록에서 삭제" / "Remove from list";
    sidebar_site_remove_detail
        => "사이트 자체는 사이트 관리자에 그대로 남습니다."
        / "The site itself remains in Site Manager.";
    site_tab_general => "일반" / "General";
    site_tab_transfer => "전송 설정" / "Transfer";
    site_tab_charset => "문자셋" / "Charset";
    site_label_protocol => "프로토콜(T):" / "Protocol(T):";
    site_label_host => "호스트(H):" / "Host(H):";
    site_label_port => "포트(P):" / "Port(P):";
    site_label_encryption => "암호화(E):" / "Encryption(E):";
    site_label_logon => "로그온 유형(L):" / "Logon type(L):";
    site_label_user => "사용자(U):" / "User(U):";
    site_label_password => "비밀번호(W):" / "Password(W):";
    /// 개인 키 파일 경로·그 암호 (FR-66) — 로그온 유형이 `키 파일`일 때만 활성이다
    site_label_key_path => "키 파일(K):" / "Key file(K):";
    site_label_key_passphrase => "키 암호(S):" / "Key passphrase(S):";
    site_key_browse => "찾아보기" / "Browse";
    site_connect => "연결(C)" / "Connect(C)";
    site_ok => "확인(O)" / "OK(O)";
    site_label_transfer_mode => "전송 모드(T):" / "Transfer mode(T):";
    site_label_limit => "동시 연결 수 제한(L)" / "Limit simultaneous connections(L)";
    site_label_limit_value => "최대 동시 연결 수(M):" / "Maximum connections(M):";
    site_charset_heading
        => "서버에서 파일명에 사용하는 문자셋"
        / "Character set the server uses for file names";
    site_charset_label => "인코딩:" / "Encoding:";
    site_label_encoding => "인코딩(E):" / "Encoding(E):";
    /// 알아듣지 못하는 인코딩 이름을 적었을 때 — 조용히 UTF-8로 처리하면 파일명이 깨진 채로 굳는다
    site_charset_unknown_hint
        => "이 이름은 알지 못해 UTF-8로 처리합니다."
        / "This name is not recognized, so UTF-8 is used.";
    site_charset_warning
        => "문자셋을 잘못 지정하면 파일 이름이 깨져 보일 수 있습니다."
        / "A wrong character set can make file names display incorrectly.";
    /// 이미 연결된 서버의 전송 모드를 바꿨을 때 — 지금 연결에 바로 듣지 않는다는 것을
    /// 알리지 않으면 사용자는 같은 실패를 다시 겪는다
    site_transfer_mode_notice
        => "이미 연결된 서버입니다. 바꾼 전송 모드는 다음 연결부터 적용됩니다."
        / "Already connected. The new transfer mode applies from the next connection.";
    /// 고를 사이트가 없는데 `연결(C)`을 눌렀을 때 (사용자 보고 2026-09-02) — 사이트를 지운
    /// 직후가 그 상태다. **까닭을 정확히 적는다**: 등록할 값이 모자란 것이 아니라 붙을
    /// 대상이 없는 것이라, 여기에 `site_error_no_host`를 쓰면 원인을 잘못 알린다
    site_error_no_selection
        => "연결할 사이트를 목록에서 고르세요."
        / "Select a site from the list to connect.";
    /// 호스트를 비운 채 등록하려 할 때 — 무엇을 해야 하는지까지 알린다
    site_error_no_host
        => "호스트 주소를 입력해야 등록할 수 있습니다."
        / "Enter a host address to save this site.";
    /// 비밀번호 봉인이 실패했을 때 — 평문으로 대신 담지 않는다 (FR-28)
    site_error_password
        => "비밀번호를 저장하지 못했습니다. 연결할 때 다시 입력해 주세요."
        / "Could not save the password. Enter it again when connecting.";
    /// 키 암호 봉인이 실패했을 때 (FR-66) — 비밀번호와 문구를 가른다:
    /// 키 파일로 접속하는 사용자에게 「비밀번호」라고 알리면 엉뚱한 칸을 고치게 된다
    site_error_key_passphrase
        => "키 암호를 저장하지 못했습니다. 연결할 때 다시 입력해 주세요."
        / "Could not save the key passphrase. Enter it again when connecting.";
    site_protocol_ftp => "FTP - 파일 전송 프로토콜" / "FTP - File Transfer Protocol";
    site_protocol_ftps
        => "FTPS - TLS로 보호되는 파일 전송 프로토콜"
        / "FTPS - File Transfer Protocol over TLS";
    site_protocol_sftp => "SFTP - SSH 파일 전송 프로토콜" / "SFTP - SSH File Transfer Protocol";
    site_encryption_plain => "일반 FTP 사용 (안전하지 않음)" / "Use plain FTP (insecure)";
    site_encryption_explicit_optional
        => "TLS를 통한 명시적 FTP가 가능한 경우 사용"
        / "Use explicit FTP over TLS if available";
    site_encryption_explicit => "TLS를 통한 명시적 FTP 필요" / "Require explicit FTP over TLS";
    site_encryption_implicit => "TLS를 통한 묵시적 FTP 필요" / "Require implicit FTP over TLS";
    site_logon_normal => "일반" / "Normal";
    site_logon_anonymous => "익명" / "Anonymous";
    site_logon_key_file => "키 파일" / "Key file";
    site_mode_default => "기본(E)" / "Default(E)";
    site_mode_active => "능동형(A)" / "Active(A)";
    site_mode_passive => "수동형(P)" / "Passive(P)";
    site_charset_custom => "문자셋 직접 설정(C)" / "Set character set manually(C)";
    /// 전송 모드 세 선택지의 설명 (2026-08-16 검토) — `능동형`·`수동형`은 FTP를 아는 사람에게만
    /// 뜻이 닿는 말이라, 낱말은 그대로 두고 무슨 일이 일어나는지를 툴팁으로 붙인다
    site_hint_mode_default
        => "수동형으로 먼저 붙어 보고, 안 되면 능동형으로 한 번 더 시도합니다."
        / "Tries passive first, then active once if that fails.";
    site_hint_mode_active
        => "서버가 이쪽으로 연결을 겁니다. 공유기·방화벽 뒤에서는 막히는 일이 많습니다."
        / "The server connects back to you — often blocked behind a router or firewall.";
    site_hint_mode_passive
        => "이쪽에서 서버로 연결을 겁니다. 공유기·방화벽 뒤에서는 보통 이쪽이 통합니다."
        / "You connect out to the server — usually the one that works behind a router or firewall.";

    // ── 원격 메뉴 (FR-31) ──
    remote_download => "받기" / "Download";
    remote_upload => "올리기" / "Upload";
    remote_rename => "이름 바꾸기…" / "Rename…";
    remote_copy_path => "경로로 복사" / "Copy as path";
    remote_new_folder => "새 폴더…" / "New folder…";
    remote_chmod => "권한 변경…" / "Change permissions…";
    remote_delete => "삭제…" / "Delete…";
    /// 이름에 쓸 수 없는 글자를 적었을 때
    remote_error_slash => "이름에 /는 쓸 수 없습니다." / "A name cannot contain /.";
    remote_error_empty => "이름을 입력해 주세요." / "Enter a name.";
    remote_ok => "확인" / "OK";
    cancel => "취소" / "Cancel";
    remote_chmod_title => "권한 변경" / "Change permissions";
    remote_chmod_octal => "숫자(8진):" / "Octal:";
    remote_apply => "적용" / "Apply";
    remote_owner => "소유자" / "Owner";
    remote_group => "그룹" / "Group";
    remote_others => "기타" / "Others";
    remote_read => "읽기" / "Read";
    remote_write => "쓰기" / "Write";
    remote_execute => "실행" / "Execute";
    remote_delete_title => "원격 항목 삭제" / "Delete remote items";
    // ── 같은 이름 확인 (FR-55) ──
    conflict_title => "같은 이름이 이미 있습니다" / "Items with the same name already exist";
    conflict_irreversible => "덮어쓰면 되돌릴 수 없습니다." / "Overwriting cannot be undone.";
    conflict_overwrite => "덮어쓰기" / "Overwrite";
    conflict_skip => "건너뛰기" / "Skip";
    /// 목록에서 폴더임을 알리는 꼬리표
    conflict_folder_mark => "(폴더)" / "(folder)";
    remote_delete_irreversible => "되돌릴 수 없습니다." / "This cannot be undone.";
    remote_delete_folder_contents => "폴더는 안에 든 것까지 모두 지워집니다." / "Folders are deleted together with everything inside them.";

    // ── 원격 탭 상태 (FR-31) ──
    remote_hint_head => "주소창에 " / "Type ";
    /// 앞 공백이 **한국어에만 없다** — 화면은 이 조각을 앞 조각(`sftp://호스트`)에 그대로
    /// 이어 붙이는데(`remote_states::show_empty`가 낱말 간격을 0으로 둔다), 한국어는 조사라
    /// 붙여 써야 하고 영어는 앞 낱말과 떨어져야 한다
    remote_hint_tail => "를 입력해 연결하세요" / " in the address bar to connect";
    /// 미연결 탭 안내 둘째 줄 (인벤토리 #15)
    remote_hint_drag
        => "사이드바의 사이트를 이 탭으로 끌어다 놓아도 됩니다"
        / "You can also drag a site from the sidebar onto this tab";
    /// 사이트를 아는 미연결 탭의 버튼 — 재시작 뒤 복원된 탭이 이것을 보인다
    remote_reconnect => "다시 연결" / "Reconnect";
    /// 실패 화면 제목 (인벤토리 #16)
    remote_fail_title => "연결하지 못했습니다" / "Could not connect";
    /// 실패 화면 버튼·링크 (인벤토리 #18~20)
    remote_fail_retry => "재시도" / "Retry";
    remote_fail_settings => "설정 열기" / "Open settings";
    remote_fail_view_log => "서버 로그 보기" / "View server log";
    /// 서버가 사유를 주지 않았을 때 보일 문구
    remote_fail_reason_fallback => "서버가 응답하지 않았습니다." / "The server did not respond.";
    /// 실패 사유 뒤에 붙는 안내 (인벤토리 #17) — **실패 갈래마다 다르다**.
    ///
    /// 종전에는 이 연결 안내 하나를 종류를 가리지 않고 붙였다. 비밀번호가 틀린 사람에게도
    /// 암호화 설정을 의심하게 만들어, 맞는 설정을 계속 바꿔 보게 했다 (2026-08-16 검토)
    remote_fail_reason_hint
        => "암호화 설정이 서버와 다를 수도 있습니다."
        / "The encryption setting may not match the server.";
    remote_fail_hint_auth
        => "사용자 이름과 비밀번호를 확인해 주세요."
        / "Check the user name and password.";
    remote_fail_hint_hostkey
        => "서버 지문이 바뀌었는지 확인해 주세요."
        / "Check whether the server fingerprint has changed.";
    /// 서 있던 연결이 끊겼을 때 사유 **앞**에 서는 말 (사용자 보고 2026-09-16).
    ///
    /// **문장 틀을 워커가 아니라 화면이 만든다** — 워커가 조합하면 그 문자열이 탭 상태에
    /// 굳어, 언어를 바꿔도 그때 언어로 남는다(대장의 2026-08-14 항목이 그 결함이다)
    remote_fail_lost => "연결이 끊어졌습니다" / "The connection was lost";
    remote_connecting => "연결 중…" / "Connecting…";
    remote_not_connected => "연결 없음" / "Not connected";
    remote_hostkey_first => "이 서버를 처음 연결합니다" / "Connecting to this server for the first time";
    remote_hostkey_changed => "서버 지문이 전과 다릅니다" / "The server fingerprint has changed";
    remote_hostkey_accept => "수락하고 연결" / "Accept and connect";

    // ── 앱 본체 ──
    /// 셸 메뉴를 쓸 수 없을 때 화면에 보일 문구 — 원인이 무엇이든 사용자가 할 수 있는 일은
    /// 재시작뿐이라 한 문구로 통일한다
    app_shell_menu_unavailable
        => "마우스 오른쪽 버튼 메뉴를 사용할 수 없습니다 (앱을 다시 시작해 주세요)"
        / "The right-click menu is unavailable (please restart the app)";
    app_workspace_delete_title => "워크스페이스 삭제" / "Delete workspace";
    app_workspace_delete_detail
        => "이 워크스페이스의 화면 구성과 탭이 함께 사라집니다."
        / "Its layout and tabs will be removed as well.";
    /// 트레이 아이콘을 올리지 못했을 때 — 조용히 끄면 왜 안 뜨는지 알 수 없다 (FR-50)
    app_tray_failed => "트레이 아이콘을 만들지 못했습니다" / "Could not create the tray icon";
    /// 한글 글꼴을 하나도 등록하지 못했을 때 (FR-48)
    app_font_fallback
        => "한글 글꼴을 불러오지 못해 기본 글꼴로 표시합니다"
        / "Could not load a Korean font, so the default font is used";

    // ── 전송 큐 (FR-35) ──
    queue_filter_all => "전체" / "All";
    queue_column_direction => "방향" / "Direction";
    queue_column_local => "로컬 파일" / "Local file";
    queue_column_remote => "원격 파일" / "Remote file";
    queue_column_server => "서버" / "Server";
    queue_column_size => "크기" / "Size";
    queue_column_progress => "진행률" / "Progress";
    queue_column_state => "상태" / "State";
    queue_column_time => "시간" / "Time";
    queue_column_reason => "이유" / "Reason";
    queue_state_pending => "대기 중" / "Pending";
    queue_state_done => "완료" / "Done";
    queue_state_active => "전송 중" / "Transferring";
    queue_state_cancelled => "취소됨" / "Cancelled";
    queue_reason_user_cancelled => "사용자 취소" / "Cancelled by user";
    queue_retry => "다시 시도" / "Retry";
    queue_retry_all => "전체 다시 시도" / "Retry all";
    queue_cancel => "전송 취소" / "Cancel transfer";
    queue_remove => "삭제" / "Remove";
    queue_remove_all => "전체 삭제" / "Remove all";

    // ── 하단 도크 (FR-35·FR-40) ──
    dock_queue => "전송 큐" / "Transfer queue";
    dock_log => "서버 로그" / "Server log";
    dock_success => "성공" / "Succeeded";
    dock_failed => "실패" / "Failed";
    /// 우측 아이콘 넷의 툴팁 (2026-08-16 검토) — 아이콘만 있고 설명이 없어,
    /// 특히 빗자루가 무엇을 지우는지 짐작할 수 없었다. 앱의 다른 아이콘 버튼은 모두 툴팁이 있다
    dock_pause => "전송 일시 정지" / "Pause transfers";
    dock_resume => "전송 다시 시작" / "Resume transfers";
    dock_clear_done => "끝난 항목 지우기" / "Clear finished items";
    dock_copy_log => "로그 복사" / "Copy log";
    dock_collapse => "아래 패널 접기" / "Collapse the bottom panel";
    /// 전송한 것이 하나도 없을 때 표에 보일 안내
    queue_empty => "아직 전송한 파일이 없습니다" / "No transfers yet";

    /// 새로 만드는 사이트의 기본 이름 (FR-27)
    site_default_name => "새 사이트" / "New site";

    /// 사이트 관리자 좌측 아랫줄의 첫 칸 — 빈 사이트를 목록에 곧바로 더한다 (FR-27).
    /// **`site_default_name`과 글자가 같아도 키를 나눈다** — 하나는 버튼 라벨이고
    /// 다른 하나는 새 기록의 기본 이름이라, 한쪽을 고칠 때 다른 쪽이 딸려 가면 안 된다
    site_new => "새 사이트" / "New site";

    // ── 사이트 목록 내보내기·가져오기 (FR-59) ──
    /// 파일 대화의 형식 필터 — 괄호 안은 실제 확장자다
    file_dialog_filter
        => "MOA 사이트 목록 (*.moasites)"
        / "MOA site list (*.moasites)";
    /// 내보내기 대화에 미리 채워 두는 파일 이름
    file_dialog_export_name => "MOA 사이트.moasites" / "MOA sites.moasites";
    /// 아랫줄 세 칸 중 뒤의 둘 (앞 칸은 `site_new`)
    site_export => "내보내기" / "Export";
    site_import => "가져오기" / "Import";
    /// 가져오기 암호 대화 — **내보내기 쪽에는 대화가 없다**(앱 내장 키로 봉하므로 물을 것이 없다)
    site_import_title => "사이트 목록 가져오기" / "Import site list";
    site_import_passphrase_hint
        => "이 파일은 암호로 보호되어 있습니다."
        / "This file is protected with a password.";
    site_import_passphrase => "암호:" / "Password:";
    site_import_open => "가져오기" / "Import";
    /// 가져오기가 막힌 까닭들
    site_import_wrong_passphrase
        => "암호가 맞지 않습니다"
        / "That password is not correct";
    site_import_broken
        => "이 파일은 MOA 사이트 목록이 아니거나 손상되었습니다"
        / "This file is not a MOA site list, or it is damaged";
    site_import_unsupported
        => "더 새로운 버전에서 만든 파일이라 읽을 수 없습니다"
        / "This file was made by a newer version and cannot be read";
    site_import_read_failed => "파일을 읽지 못했습니다" / "Could not read the file";
    site_import_empty => "파일에 사이트가 없습니다" / "The file contains no sites";
    site_export_write_failed => "파일을 저장하지 못했습니다" / "Could not save the file";
    site_export_seal_failed
        => "암호로 보호하지 못해 저장을 멈췄습니다"
        / "Stopped saving because the passwords could not be protected";
    /// 파일 대화를 띄울 창을 찾지 못한 경우 — 실제로는 거의 없다
    site_file_dialog_unavailable
        => "파일 창을 열지 못했습니다"
        / "Could not open the file window";
    /// 겹치는 사이트 확인
    site_conflict_title => "같은 서버가 이미 있습니다" / "These servers are already registered";
    site_conflict_detail
        => "덮어쓰면 그 사이트의 설정과 로그인 정보가 파일의 것으로 바뀝니다."
        / "Overwriting replaces their settings and sign-in details with the ones in the file.";
    site_conflict_overwrite => "덮어쓰기" / "Overwrite";
    site_conflict_skip => "건너뛰기" / "Skip";

    // ── 원격 계층: 작업 이름 (오류 문구 앞에 붙는 동사 조각) ──
    /// `RemoteOp`가 이 함수들로 풀린다 — 조각과 문장 틀을 함께 옮겨야 뜻이 통한다
    op_session_setup => "세션 준비" / "Session setup";
    op_ssh_handshake => "SSH 협상" / "SSH handshake";
    op_sftp_start => "SFTP 시작" / "SFTP startup";
    op_home => "홈 확인" / "Home lookup";
    op_move => "이동" / "Change directory";
    op_list => "목록" / "Listing";
    op_mkdir => "폴더 만들기" / "Create folder";
    op_remove => "삭제" / "Delete";
    op_rmdir => "폴더 삭제" / "Delete folder";
    op_rename_op => "이름 바꾸기" / "Rename";
    /// 메뉴·대화와 **같은 말**이어야 한다 — 종전 `권한 바꾸기`는 같은 동작을 두 이름으로 불렀다
    op_chmod => "권한 변경" / "Change permissions";
    op_open => "열기" / "Open";
    op_resume => "이어 올리기" / "Resume upload";
    op_create => "만들기" / "Create";
    op_close => "닫기" / "Close";
    op_keepalive => "연결 유지" / "Keep-alive";
    op_quit => "종료" / "Quit";
    op_connect => "연결" / "Connect";
    op_connect_implicit => "묵시적 TLS 연결" / "Implicit TLS connect";
    op_tls_upgrade => "TLS 승격" / "TLS upgrade";
    op_login => "로그인" / "Log in";

    // ── 원격 계층: 상태·오류 ──
    remote_not_connected_err => "서버에 연결되어 있지 않습니다" / "Not connected to the server";
    remote_not_logged_in => "아직 로그인하지 않았습니다" / "Not logged in yet";
    remote_ftp_site_on_sftp
        => "FTP 사이트는 SFTP 세션으로 연결할 수 없습니다"
        / "An FTP site cannot be opened with an SFTP session";
    remote_sftp_site_on_ftp
        => "SFTP 사이트는 FTP 세션으로 연결할 수 없습니다"
        / "An SFTP site cannot be opened with an FTP session";
    remote_no_fingerprint
        => "서버가 지문을 알려 주지 않아 서버를 확인할 수 없습니다"
        / "The server gave no fingerprint, so it cannot be verified";
    remote_login_rejected
        => "서버가 로그인을 받아들이지 않았습니다"
        / "The server did not accept the login";
    /// 개인 키를 고르는 대화의 종류 이름 — 키 파일은 확장자가 없는 것이 보통이다 (FR-66)
    file_dialog_any => "모든 파일" / "All files";
    /// 키 파일로 접속하겠다고 해 놓고 경로를 비워 둔 경우 (FR-66)
    remote_key_path_missing
        => "개인 키 파일을 고르지 않았습니다. 사이트 관리자에서 키 파일을 골라 주세요"
        / "No private key file was chosen. Pick one in the site manager";
    remote_not_a_folder => "폴더가 아닙니다" / "Not a folder";
    /// 사유만 적으면 사용자는 무엇을 해야 하는지 알 수 없다 — 다음 행동까지 함께 적는다
    remote_unknown_server
        => "처음 보는 서버라 연결하지 않았습니다. 서버 지문을 확인한 뒤 다시 연결해 주세요"
        / "Did not connect — this server has not been seen before. Check its fingerprint and connect again";
    /// `classify`가 조각으로 쓰는 이름 — 조사와 함께 조립된다
    remote_subject_home => "서버의 시작 폴더 이름" / "the name of the server's home folder";
    remote_subject_names => "이 폴더의 파일 이름" / "the file names in this folder";
    /// 연결 진행 로그 (FR-40)
    remote_log_connected
        => "연결 수립, 환영 메시지를 기다림…"
        / "Connected, waiting for the welcome message…";
    remote_log_tls => "TLS로 암호화된 연결입니다." / "The connection is encrypted with TLS.";
    remote_log_plain
        => "암호화되지 않은 연결입니다. 이 서버는 TLS를 지원하지 않습니다."
        / "This connection is not encrypted. The server does not support TLS.";
    remote_log_login => "로그인…" / "Logging in…";
    remote_log_login_done => "로그인 완료" / "Logged in";
    /// 서버 로그의 줄 종류 접두 (FR-40)
    log_kind_status => "상태:" / "Status:";
    log_kind_command => "명령:" / "Command:";
    log_kind_response => "응답:" / "Response:";
    log_kind_error => "오류:" / "Error:";
    /// 사용자가 그만둔 것 — 실패가 아니다
    remote_cancelled => "취소했습니다" / "Cancelled";

    // ── 트레이 메뉴 (FR-50) ──
    /// 우클릭 메뉴 항목 — 요청 문구 그대로다
    /// 이미 도는 앱을 다시 `실행`한다고 적으면 뜻이 어긋난다 — 창을 여는 것이다(영어와도 맞다)
    tray_show => "열기" / "Open";
    tray_quit => "종료" / "Quit";

    /// 설정 대화의 언어 항목 라벨
    settings_language_label => "앱 언어" / "App language";

    // ── 원격 목록 자동 갱신 (FR-67) ──
    settings_remote_auto_refresh => "원격 목록 자동 갱신" / "Refresh remote list automatically";
    settings_remote_refresh_label => "갱신 주기" / "Refresh interval";
    settings_transfer_retry_label => "전송 재시도 횟수" / "Transfer retry attempts";
    /// 갱신 주기 선택지 — 값이 글자에 들어가지만 **고정된 다섯 개**라 자리표시자가 필요 없다
    settings_refresh_10s => "10초" / "10 seconds";
    settings_refresh_30s => "30초" / "30 seconds";
    settings_refresh_1m => "1분" / "1 minute";
    settings_refresh_5m => "5분" / "5 minutes";
    settings_refresh_10m => "10분" / "10 minutes";
}

/// 값이 끼어드는 문구 — 조사·어순이 언어마다 달라 자리표시자로 담기지 않는다 (D2).
///
/// 정적 문구와 달리 매크로로 펼치지 않는다: 인자 개수·순서를 컴파일러가 검사해야 하고,
/// 언어마다 문장을 통째로 다시 쓸 수 있어야 한다
pub mod dynamic {
    use super::{Language, current};

    /// 전송이 도는 중에 업데이트를 누르면 뜨는 확인 문구 (FR-62).
    ///
    /// 건수가 문장 안에 들어가고 영어는 단수·복수가 갈려 카탈로그 매크로로는 담을 수 없다.
    /// **잃는 것을 먼저 말한다** — 사용자가 판단할 근거가 그것이다
    pub fn update_confirm_body(active: usize) -> String {
        match current() {
            Language::Korean => format!(
                "전송 {active}건이 진행 중입니다. 지금 업데이트하면 그 전송이 중단되고 앱이 다시 시작됩니다."
            ),
            Language::English => {
                let count = if active == 1 {
                    "1 transfer is".to_owned()
                } else {
                    format!("{active} transfers are")
                };
                format!("{count} still running. Updating now stops them and restarts the app.")
            }
        }
    }

    /// 타이틀바 배지 — 받는 중이고 서버가 전체 크기를 알려 준 경우 (FR-62).
    ///
    /// 백분율이 문장 안에 들어가 카탈로그 매크로로는 담을 수 없다.
    /// **전체 크기를 모르면 부르는 쪽이 `update_downloading()`을 그대로 쓴다**(D1) —
    /// 여기서 `0%`를 지어내면 받고 있는데 멈춘 것처럼 보인다
    pub fn update_downloading_percent(percent: u8) -> String {
        match current() {
            Language::Korean => format!("다운로드 중 {percent}%"),
            Language::English => format!("Downloading {percent}%"),
        }
    }

    /// 한국어 목적격 조사 — 앞 글자에 받침이 있으면 `을`, 없으면 `를`.
    ///
    /// **`을(를)` 병기를 화면에 내보내지 않기 위한 것이다** — 완성된 문장으로 보이지 않는다.
    /// 한글로 끝나지 않는 말(경로·명령 이름)은 발음으로 갈려 판정할 수 없으므로, 그런 값이
    /// 들어가는 문장은 애초에 조사가 붙지 않게 다시 썼다(`err_not_found` 등) —
    /// 이 함수에는 **카탈로그가 쥐고 있는 한국어 낱말만** 들어온다
    fn object_particle(word: &str) -> &'static str {
        match word.chars().next_back() {
            // 한글 음절 = 초성×588 + 중성×28 + 종성. 나머지가 0이면 받침이 없다
            Some(ch) if ('가'..='힣').contains(&ch) => {
                if (ch as u32 - '가' as u32).is_multiple_of(28) {
                    "를"
                } else {
                    "을"
                }
            }
            _ => "를",
        }
    }

    /// 폴더를 열지 못한 사유 — 경로 이름이 문장 안에 들어간다.
    /// 권한이 막힌 경우는 여기 오지 않는다 — 그 폴더로 옮긴 뒤 목록 자리에
    /// `list_access_denied`를 적는다 (2026-08-16 사용자 요청)
    pub fn open_not_found(name: &str) -> String {
        match current() {
            Language::Korean => format!("'{name}' 폴더를 찾을 수 없습니다"),
            Language::English => format!("Could not find '{name}'"),
        }
    }

    /// 새로 만들기 실패 — 한국어는 조사가 붙고 영어는 관사가 붙는다 (D2).
    /// `kind`는 `폴더`·`파일` 둘 중 하나라 받침이 갈린다 — 조사를 그때그때 고른다
    pub fn create_failed(kind: &str, error: &str) -> String {
        match current() {
            Language::Korean => {
                let particle = object_particle(kind);
                format!("새 {kind}{particle} 만들지 못했습니다 — {error}")
            }
            Language::English => format!("Could not create the new {kind} — {error}"),
        }
    }

    /// 상태 줄 실패 알약 (인벤토리 #57) — 종전 `실패 3`은 무엇이 3인지 적지 않았다.
    /// 한국어는 수 뒤에 단위가 붙고 영어는 수가 앞에 선다
    pub fn status_failed_count(count: usize) -> String {
        match current() {
            Language::Korean => format!("실패 {count}건"),
            Language::English if count == 1 => "1 failed".to_owned(),
            Language::English => format!("{count} failed"),
        }
    }

    /// 정보 대화의 이름·버전 줄 (FR-58) — 한국어 `모아 0.1.0`, 영어 `MOA 0.1.0`.
    ///
    /// 두 언어에서 어순이 같아 갈래를 두지 않는다(이름만 갈린다). 값이 끼어드는 문구라
    /// `strings!`가 아니라 여기 있으며, 버전은 `Cargo.toml`이 정본이라 컴파일 시점에 박는다
    pub fn about_version_line() -> String {
        format!("{} {}", super::app_name(), env!("CARGO_PKG_VERSION"))
    }

    /// 라이선스 목록의 구성 요소 수 (FR-57) — 한국어는 수 뒤에 단위가 붙는다
    pub fn licenses_component_count(count: usize) -> String {
        match current() {
            Language::Korean => format!("구성 요소 {count}개"),
            Language::English if count == 1 => "1 component".to_owned(),
            Language::English => format!("{count} components"),
        }
    }

    /// 상태 줄의 폴더·파일 개수 — 어순이 언어마다 다르다
    pub fn item_counts(dirs: usize, files: usize) -> String {
        match current() {
            Language::Korean => format!("폴더 {dirs} 파일 {files}"),
            Language::English => format!("{dirs} folders, {files} files"),
        }
    }

    /// 전송 큐 요약 — `3건 대기 · 12.4 MB/s · 00:41 남음` / `3 pending · 12.4 MB/s · 00:41 left`.
    ///
    /// **개수와 남은 시간은 언어마다 붙는 자리가 다르다**(한국어는 뒤에 `남음`, 영어는 `left`)
    /// — 그 두 조각만 언어별로 만들고, 사이에 끼는 속도·구분점은 순서가 같아 이어 붙인다.
    ///
    /// 남은 시간을 모를 때 쓸 값(`unknown`)은 **호출부가 준다** — 그 값의 정본은 전송 큐 쪽에
    /// 있고, 여기서 그것을 참조하면 문구 카탈로그가 원격 계층을 거꾸로 참조하게 된다
    pub fn queue_summary(
        pending: usize,
        speed: Option<&str>,
        eta: Option<&str>,
        unknown: &str,
    ) -> String {
        let language = current();
        let mut out = match language {
            Language::Korean => format!("{pending}건 대기"),
            Language::English => format!("{pending} pending"),
        };
        if let Some(speed) = speed {
            out.push_str(" · ");
            out.push_str(speed);
        }
        out.push_str(" · ");
        match eta {
            Some(eta) => match language {
                Language::Korean => out.push_str(&format!("{eta} 남음")),
                Language::English => out.push_str(&format!("{eta} left")),
            },
            None => out.push_str(unknown),
        }
        out
    }

    /// 같은 이름 확인 대화의 첫 줄 (FR-55) — 영어는 하나일 때 단수형이다
    pub fn conflict_count(count: usize) -> String {
        match current() {
            Language::Korean => format!("{count}개 항목이 대상에 이미 있습니다."),
            Language::English if count == 1 => {
                "1 item already exists at the destination.".to_owned()
            }
            Language::English => format!("{count} items already exist at the destination."),
        }
    }

    /// 원격 삭제 확인 — 영어는 하나일 때 단수형이다
    pub fn remote_delete_count(count: usize) -> String {
        match current() {
            Language::Korean => format!("{count}개 항목을 서버에서 지웁니다."),
            Language::English if count == 1 => "1 item will be deleted from the server.".to_owned(),
            Language::English => format!("{count} items will be deleted from the server."),
        }
    }

    /// 워크스페이스 삭제 확인의 첫 줄 — 이름이 문장 안에 들어간다
    pub fn workspace_delete_confirm(name: &str) -> String {
        match current() {
            Language::Korean => format!("'{name}' 워크스페이스를 삭제할까요?"),
            Language::English => format!("Delete the workspace '{name}'?"),
        }
    }

    /// 원격 폴더를 열지 못했을 때 — 서버가 준 사유가 뒤에 붙는다
    pub fn remote_open_failed(detail: &str) -> String {
        match current() {
            Language::Korean => format!("폴더를 열지 못했습니다 — {detail}"),
            Language::English => format!("Could not open the folder — {detail}"),
        }
    }

    /// 로컬 복사가 셸 오류로 끝났을 때 (FR-60).
    ///
    /// `detail`은 셸이 준 사유라 **그대로 싣는다** — 문장 틀만 카탈로그가 만든다
    /// (전송 실패 사유를 서버 원문으로 보이는 것과 같은 방식)
    pub fn local_copy_failed(detail: &str) -> String {
        match current() {
            Language::Korean => format!("복사하지 못했습니다 — {detail}"),
            Language::English => format!("Could not copy — {detail}"),
        }
    }

    /// 사용자가 셸 복사 대화에서 그만뒀을 때 (FR-60) — 오류가 아니라 사실 전달이다.
    ///
    /// **몇 개가 실제로 복사됐는지는 적지 않는다** — 셸만 아는 값이라 앱이 적으면 틀린다.
    /// `count`는 걸었던 항목 수다
    pub fn local_copy_cancelled(count: usize) -> String {
        match current() {
            Language::Korean => format!("복사를 그만뒀습니다 — {count}개 항목"),
            Language::English if count == 1 => "Copy stopped — 1 item".to_owned(),
            Language::English => format!("Copy stopped — {count} items"),
        }
    }

    pub fn remote_list_failed(detail: &str) -> String {
        match current() {
            Language::Korean => format!("목록을 읽지 못했습니다 — {detail}"),
            Language::English => format!("Could not read the listing — {detail}"),
        }
    }

    /// 읽을 수 없어 건너뛴 폴더 수 — 영어는 하나일 때 단수형이다
    pub fn skipped_folders(skipped: usize) -> String {
        match current() {
            Language::Korean => format!("읽을 수 없는 폴더 {skipped}개는 건너뛰었습니다"),
            Language::English if skipped == 1 => {
                "Skipped 1 folder that could not be read".to_owned()
            }
            Language::English => format!("Skipped {skipped} folders that could not be read"),
        }
    }

    // ── 원격 파일 작업 실패 (FR-39) ──
    //
    // **작업마다 문장을 따로 둔다** — 종전에는 메뉴 라벨을 빌려 `{이름} 실패`로 이었는데,
    // 그러면 `새 폴더 실패`처럼 한국어 문장이 되지 않는 말이 상태 줄에 그대로 나갔다.
    // 카탈로그의 다른 실패 문구(`폴더를 열지 못했습니다 — …`)와 같은 꼴로 맞춘다

    pub fn op_mkdir_failed(error: &str) -> String {
        match current() {
            Language::Korean => format!("새 폴더를 만들지 못했습니다 — {error}"),
            Language::English => format!("Could not create the folder — {error}"),
        }
    }

    pub fn op_delete_failed(error: &str) -> String {
        match current() {
            Language::Korean => format!("삭제하지 못했습니다 — {error}"),
            Language::English => format!("Could not delete — {error}"),
        }
    }

    pub fn op_rename_failed(error: &str) -> String {
        match current() {
            Language::Korean => format!("이름을 바꾸지 못했습니다 — {error}"),
            Language::English => format!("Could not rename — {error}"),
        }
    }

    pub fn op_chmod_failed(error: &str) -> String {
        match current() {
            Language::Korean => format!("권한을 변경하지 못했습니다 — {error}"),
            Language::English => format!("Could not change permissions — {error}"),
        }
    }

    /// 사이트 이름을 모를 때 번호로 부른다 (전송 큐)
    pub fn queue_site_fallback(id: u32) -> String {
        match current() {
            Language::Korean => format!("사이트 {id}"),
            Language::English => format!("Site {id}"),
        }
    }

    /// 받은 파일을 제자리로 옮기지 못했을 때 (FR-36)
    pub fn transfer_finalize_failed(error: &str) -> String {
        match current() {
            Language::Korean => format!("받은 파일을 제자리에 두지 못했습니다: {error}"),
            Language::English => format!("Could not move the downloaded file into place: {error}"),
        }
    }

    /// 자동 재시도를 다 쓰고도 실패했을 때 — 서버가 준 사유 뒤에 시도 횟수를 붙인다 (FR-37).
    ///
    /// 그 숫자가 없으면 사용자는 앱이 한 번만 해 보고 포기한 줄 안다
    pub fn transfer_failed_after(reason: &str, attempts: u32) -> String {
        match current() {
            Language::Korean => format!("{reason} ({attempts}회 시도)"),
            Language::English => format!("{reason} (tried {attempts} times)"),
        }
    }

    /// 자동 재시도로 다시 도는 중 — 상태 열에 몇 번째인지 붙인다 (FR-37).
    ///
    /// `attempt`는 지금이 몇 번째 재시도인가이고 `limit`은 설정된 상한이다
    pub fn transfer_retrying(attempt: u32, limit: u32) -> String {
        match current() {
            Language::Korean => format!("재시도 {attempt}/{limit}"),
            Language::English => format!("retry {attempt}/{limit}"),
        }
    }

    // ── 원격 오류 문장 (조사·어순이 언어마다 갈린다) ──
    pub fn err_connect(detail: &str) -> String {
        match current() {
            Language::Korean => format!("연결하지 못했습니다 — {detail}"),
            Language::English => format!("Could not connect — {detail}"),
        }
    }

    pub fn err_login(detail: &str) -> String {
        match current() {
            Language::Korean => format!("로그인하지 못했습니다 — {detail}"),
            Language::English => format!("Could not log in — {detail}"),
        }
    }

    pub fn err_host_key(detail: &str) -> String {
        match current() {
            Language::Korean => format!("호스트 키를 확인하지 못했습니다 — {detail}"),
            Language::English => format!("Could not verify the host key — {detail}"),
        }
    }

    /// 경로는 한글로 끝나지 않을 수 있어 조사를 붙일 수 없다 — `경로`를 세워 그 뒤에 붙인다
    pub fn err_not_found(path: &str, detail: &str) -> String {
        match current() {
            Language::Korean => format!("'{path}' 경로를 찾을 수 없습니다 — {detail}"),
            Language::English => format!("Could not find '{path}' — {detail}"),
        }
    }

    pub fn err_permission(path: &str, detail: &str) -> String {
        match current() {
            Language::Korean => format!("'{path}'에 접근할 권한이 없습니다 — {detail}"),
            Language::English => format!("No permission to access '{path}' — {detail}"),
        }
    }

    pub fn err_interrupted(transferred: u64, detail: &str) -> String {
        match current() {
            Language::Korean => {
                format!("전송이 중단됐습니다 ({transferred}바이트 진행) — {detail}")
            }
            Language::English => {
                format!("The transfer was interrupted ({transferred} bytes done) — {detail}")
            }
        }
    }

    pub fn err_unsupported(operation: &str, detail: &str) -> String {
        match current() {
            // 명령 이름은 영문이라 조사를 붙일 수 없다 — `명령`을 세워 그 뒤에 붙인다
            Language::Korean => format!("서버가 '{operation}' 명령을 지원하지 않습니다 — {detail}"),
            Language::English => format!("The server does not support '{operation}' — {detail}"),
        }
    }

    /// 폴더를 지우다 일부가 남았을 때 — 첫 실패의 사유를 붙인다
    pub fn err_incomplete(failed: usize, detail: &str) -> String {
        match current() {
            Language::Korean => format!("항목 {failed}개를 지우지 못했습니다 — {detail}"),
            Language::English if failed == 1 => format!("1 item could not be deleted — {detail}"),
            Language::English => format!("{failed} items could not be deleted — {detail}"),
        }
    }

    pub fn err_protocol(detail: &str) -> String {
        match current() {
            Language::Korean => format!("서버와 통신하지 못했습니다 — {detail}"),
            Language::English => format!("Could not talk to the server — {detail}"),
        }
    }

    /// 목록 조회 로그 — 경로가 따옴표 안에 들어간다 (FR-40)
    pub fn log_list_start(path: &str) -> String {
        match current() {
            Language::Korean => format!("\"{path}\" 디렉터리 목록 조회…"),
            Language::English => format!("Listing directory \"{path}\"…"),
        }
    }

    pub fn log_list_done(path: &str) -> String {
        match current() {
            Language::Korean => format!("\"{path}\" 디렉터리 목록 조회 성공"),
            Language::English => format!("Listed directory \"{path}\""),
        }
    }

    pub fn log_connecting(target: &str) -> String {
        match current() {
            Language::Korean => format!("{target}에 연결…"),
            Language::English => format!("Connecting to {target}…"),
        }
    }

    /// 재시도 안내 — 영어는 1초일 때 단수형이다
    pub fn log_retry(secs: u64, error: &str) -> String {
        match current() {
            Language::Korean => format!("연결에 실패해 {secs}초 뒤 다시 시도합니다 — {error}"),
            Language::English if secs == 1 => {
                format!("Connection failed, retrying in 1 second — {error}")
            }
            Language::English => {
                format!("Connection failed, retrying in {secs} seconds — {error}")
            }
        }
    }

    pub fn log_too_deep(path: &str) -> String {
        match current() {
            Language::Korean => format!("{path} 아래는 너무 깊어 건너뜁니다"),
            Language::English => format!("Skipping below {path} — too deep"),
        }
    }

    /// 폴더 재귀 삭제 중 항목 하나를 지우지 못했을 때 — 서버 로그에 남는다
    pub fn log_delete_failed(path: &str, error: &str) -> String {
        match current() {
            Language::Korean => format!("{path} 를 지우지 못했습니다: {error}"),
            Language::English => format!("Could not delete {path}: {error}"),
        }
    }

    pub fn log_read_failed(path: &str, error: &str) -> String {
        match current() {
            Language::Korean => format!("{path} 를 읽지 못했습니다: {error}"),
            Language::English => format!("Could not read {path}: {error}"),
        }
    }

    /// 저장해 둔 지문과 다를 때 — 두 값을 함께 보인다 (FR-30)
    pub fn hostkey_changed_detail(old: &str) -> String {
        match current() {
            Language::Korean => format!(
                "전에 저장한 지문은 {old} 였습니다. 서버를 다시 설치했거나, 중간에 다른 서버가 끼어든 것일 수 있습니다."
            ),
            Language::English => format!(
                "The stored fingerprint was {old}. The server may have been reinstalled, or another server may be in the middle."
            ),
        }
    }

    /// 원격 계층이 거절 사유로 남기는 같은 뜻의 문장 (화면이 아니라 오류에 실린다)
    pub fn hostkey_changed_reason(old: &str, new: &str) -> String {
        match current() {
            Language::Korean => format!(
                "서버 지문이 전에 저장해 둔 것과 다릅니다 (저장된 값 {old}, 이번 값 {new}) — 서버를 다시 설치했거나 중간에 다른 서버가 끼어든 것일 수 있습니다"
            ),
            Language::English => format!(
                "The server fingerprint differs from the stored one (stored {old}, now {new}) — the server may have been reinstalled, or another server may be in the middle"
            ),
        }
    }

    /// 지문을 확인할 수단이 없을 때 사유 뒤에 붙인다
    pub fn hostkey_unverifiable(detail: &str) -> String {
        match current() {
            Language::Korean => format!("{detail} (확인할 수단이 없습니다)"),
            Language::English => format!("{detail} (no way to verify)"),
        }
    }

    /// 서버가 UTF-8이 아닌 이름을 쓸 때 — `subject`가 조사와 함께 조립된다
    pub fn name_decode_failed(subject: &str) -> String {
        match current() {
            Language::Korean => {
                let particle = object_particle(subject);
                format!(
                    "{subject}{particle} 읽지 못했습니다 (서버가 UTF-8이 아닌 이름을 쓰는 것 같습니다)"
                )
            }
            Language::English => {
                format!("Could not read {subject} (the server seems to use non-UTF-8 names)")
            }
        }
    }

    /// 비밀번호 인증을 받지 않는 서버 — 받는 방식 목록은 프로토콜 식별자라 번역하지 않는다
    pub fn auth_no_password(error: &str, list: &str) -> String {
        match current() {
            Language::Korean => {
                format!("{error} — 이 서버는 비밀번호 인증을 받지 않습니다 (받는 방식: {list})")
            }
            Language::English => {
                format!(
                    "{error} — this server does not accept password authentication (accepted: {list})"
                )
            }
        }
    }

    pub fn tls_setup_failed(error: &str) -> String {
        match current() {
            Language::Korean => format!("TLS 설정을 준비하지 못했습니다 — {error}"),
            Language::English => format!("Could not prepare the TLS setting — {error}"),
        }
    }

    pub fn reply_unreadable(operation: &str) -> String {
        match current() {
            Language::Korean => format!("{operation}: 서버 응답을 해석하지 못했습니다"),
            Language::English => format!("{operation}: could not read the server reply"),
        }
    }

    pub fn data_connection_open(operation: &str) -> String {
        match current() {
            Language::Korean => format!("{operation}: 데이터 연결이 이미 열려 있습니다"),
            Language::English => format!("{operation}: the data connection is already open"),
        }
    }

    /// 사이트 삭제 확인의 첫 줄 — 워크스페이스 삭제 대화와 같은 꼴이다
    pub fn site_delete_confirm(name: &str) -> String {
        match current() {
            Language::Korean => format!("'{name}' 사이트를 삭제할까요?"),
            Language::English => format!("Delete the site '{name}'?"),
        }
    }

    /// 사이드바 목록에서 지운 뒤 뜨는 알림 (FR-29) — **연결·전송까지 걷혔다는 것**과
    /// 사이트 기록은 남았다는 것을 함께 알린다. 이름 뒤에 `사이트`를 세워 조사를 피한다
    /// (이름은 사용자가 지은 말이라 받침을 알 수 없다)
    pub fn site_removed(name: &str) -> String {
        match current() {
            Language::Korean => {
                format!("'{name}' 사이트를 목록에서 지웠습니다 · 사이트 관리자에 그대로 있습니다")
            }
            Language::English => {
                format!("Removed '{name}' from the list · it remains in Site Manager")
            }
        }
    }

    /// 사이드바 목록에서 지울 때의 확인 첫 줄 (FR-29) — 몇 건이 함께 취소되는지 밝힌다.
    /// 그 수가 곧 되돌릴 수 없는 손실이라 확인을 띄우는 이유이기도 하다
    pub fn sidebar_site_remove_confirm(name: &str, running: usize) -> String {
        match current() {
            Language::Korean => format!(
                "'{name}' 사이트를 목록에서 지웁니다. 진행 중이거나 기다리는 전송 {running}건이 함께 취소됩니다."
            ),
            Language::English => format!(
                "Removing '{name}' from the list. {running} transfer(s) in progress or waiting will be cancelled."
            ),
        }
    }

    /// 사이트를 등록한 뒤 뜨는 알림 (FR-27)
    pub fn site_registered(host: &str) -> String {
        match current() {
            Language::Korean => format!("{host} 등록됨 · 더블클릭하여 연결"),
            Language::English => format!("{host} added · double-click to connect"),
        }
    }

    /// 내보내기를 마친 뒤의 알림 (FR-59).
    ///
    /// **비밀번호가 함께 담겼다는 것을 여기서 알린다** — 내보내기에 대화가 없어져(plan D2)
    /// 그 파일의 성질을 사용자에게 알릴 자리가 이 알림뿐이다. 비밀번호를 읽지 못한 것이
    /// 있으면 그 사실도 뒤에 잇는다
    pub fn site_export_done(count: usize, unreadable: usize) -> String {
        let mut out = match current() {
            Language::Korean => {
                format!("사이트 {count}개를 저장했습니다 · 비밀번호가 함께 담겼습니다")
            }
            Language::English if count == 1 => "Saved 1 site · passwords included".to_owned(),
            Language::English => format!("Saved {count} sites · passwords included"),
        };
        if unreadable > 0 {
            match current() {
                Language::Korean => {
                    out.push_str(&format!(
                        " · {unreadable}개는 비밀번호를 읽지 못해 뺐습니다"
                    ));
                }
                Language::English => {
                    out.push_str(&format!(
                        " · {unreadable} of them lost their password (it could not be read)"
                    ));
                }
            }
        }
        out
    }

    /// 가져오기를 마친 뒤의 알림 (FR-59)
    pub fn site_import_done(
        added: usize,
        replaced: usize,
        skipped: usize,
        password_failed: usize,
    ) -> String {
        let mut out = match current() {
            Language::Korean => format!("{added}개 추가 · {replaced}개 덮어씀"),
            Language::English => format!("{added} added · {replaced} replaced"),
        };
        if skipped > 0 {
            match current() {
                Language::Korean => out.push_str(&format!(" · {skipped}개 건너뜀")),
                Language::English => out.push_str(&format!(" · {skipped} skipped")),
            }
        }
        if password_failed > 0 {
            match current() {
                Language::Korean => {
                    out.push_str(&format!(
                        " · {password_failed}개는 비밀번호를 저장하지 못했습니다"
                    ));
                }
                Language::English => {
                    out.push_str(&format!(
                        " · {password_failed} could not have their password saved"
                    ));
                }
            }
        }
        out
    }

    /// 겹치는 사이트 확인 대화의 첫 줄 (FR-59)
    pub fn site_conflict_count(count: usize) -> String {
        match current() {
            Language::Korean => format!("사이트 {count}개가 이미 등록되어 있습니다."),
            Language::English if count == 1 => "1 site is already registered.".to_owned(),
            Language::English => format!("{count} sites are already registered."),
        }
    }
}

/// 전역 언어를 건드리는 시험끼리 겹치지 않게 한다.
///
/// `cargo test`는 시험을 **여러 스레드에서 동시에** 돌린다 — 값을 끝에 되돌리는
/// 것만으로는 부족하다: 한 시험이 영어로 두고 단언하는 그 찰나에 다른 시험이
/// 값을 바꾸면 엉뚱한 언어를 읽는다. 그래서 **본문이 도는 동안 잠근다**
#[cfg(test)]
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 언어를 바꾸는 시험이 드는 가드 — 들고 있는 동안 다른 시험이 끼어들지 못하고,
/// 떨어질 때 시험 전의 언어로 되돌아간다.
///
/// **문구를 쓰는 모든 모듈의 시험이 이것을 쓴다** — `ui`·`remote`의 시험도 화면 문구를
/// 단언하려면 언어를 정해야 하고, 그때 잠그지 않으면 서로를 흔든다.
///
/// 둘째 자리를 읽는 코드는 없다 — **들고 있는 것 자체가 하는 일**이다
#[cfg(test)]
pub struct LanguageGuard(
    Language,
    #[allow(dead_code)] std::sync::MutexGuard<'static, ()>,
);

#[cfg(test)]
impl LanguageGuard {
    /// 언어를 고정하고 가드를 든다. 가드가 떨어지면 원래 언어로 돌아간다
    pub fn lock(setting: LanguageSetting) -> LanguageGuard {
        // 앞선 시험이 단언에 실패해 잠금이 오염됐어도 이어서 돈다 —
        // 우리가 지키는 것은 `AtomicU8` 하나이고 그 값은 곧 덮인다
        let guard = TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let before = current();
        set_language(setting);
        LanguageGuard(before, guard)
    }
}

#[cfg(test)]
impl Drop for LanguageGuard {
    fn drop(&mut self) {
        let setting = match self.0 {
            Language::Korean => LanguageSetting::Korean,
            Language::English => LanguageSetting::English,
        };
        set_language(setting);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 소스를 훑어 **카탈로그를 거치지 않은 한글 UI 문구**를 찾는다 (FR-53·NFR-6).
    ///
    /// 이 파일에 사는 이유: 지키는 규약이 이 모듈의 것이다. 같은 기법을 `ui::widgets`의
    /// 아이콘 규약 시험이 이미 쓴다.
    ///
    /// **찾지 못하는 것** 셋: ① 리터럴만 본다 — 문구를 변수에 담아 옮기거나 바깥에서
    /// 받아 그리면 걸리지 않는다 ② `mod tests` **뒤**의 코드는 통째로 뺀다(이 레포는
    /// 시험 모듈이 파일 끝에 있다는 전제) ③ 단언·`expect`가 있는 **그 줄**의 리터럴은
    /// 개발자용으로 보고 건너뛴다. 그런 우회를 막는 장치는 없고, 리뷰가 본다
    #[test]
    fn 화면_문구가_카탈로그를_거치지_않은_곳이_없다() {
        use std::path::Path;

        /// 훑을 곳 — 화면에 닿는 계층
        const ROOTS: [&str; 5] = [
            "src/ui",
            "src/remote",
            "src/fs",
            "src/panel/tabs.rs",
            "src/app/workspace.rs",
        ];

        /// **리터럴 단위 예외** — 화면 문구가 아닌 것들.
        ///
        /// 위젯 상태를 잇는 열쇠(`Id::new`·`id_salt`)는 바꾸면 대화 상태가 초기화되고,
        /// 나머지는 화면에 나오지 않는 내부 값이다
        const EXEMPT_LITERALS: [&str; 51] = [
            // 위젯 ID
            "정보 대화",
            "라이선스 대화",
            "라이선스 목록",
            "라이선스 전문",
            "앱 설정",
            "설정 글꼴",
            "설정 언어",
            "설정 원격 갱신 주기",
            "설정 전송 재시도 횟수",
            "셸 메뉴 업로드 하위",
            "원격 메뉴",
            "트리 메뉴",
            "원격 이름 대화",
            "원격 권한 변경",
            "원격 삭제 확인",
            "같은 이름 확인",
            "원격 호스트 키 확인",
            "사이트 관리자",
            "사이트 삭제 확인",
            "사이드바 사이트 삭제 확인",
            "사이트 이름 바꾸기",
            "사이트 목록 빈 자리",
            "사이트 가져오기 암호",
            "사이트 가져오기 충돌",
            "가져오기 암호",
            "원격 알림",
            // 글꼴 검증에 쓰는 내부 값 (`ui::font_scan`)
            "한글",
            "글꼴 검증",
            // 서버가 보낸 응답을 살피는 낱말 (`remote::ftp::mentions_permission`) —
            // 화면 언어를 따르면 안 되는 자리다
            "권한",
            // 가짜 서버(`remote::testing`)가 쓰는 값 — 화면에 나오지 않는다
            "연결되어 있지 않습니다",
            "가짜 서버 상태가 오염됐습니다",
            "없는 폴더",
            // 여러 줄에 걸친 단언·`expect`의 메시지 — 개발자에게만 보인다
            "직렬화",
            "성공한 블롭은 널일 수 없다",
            "items_menu는 항목이 하나 이상이어야 한다 (빈 목록은 background_menu 담당)",
            "원격 탭에 로컬 경로를 커밋하려 했다",
            // **Windows가 준 메뉴 문구를 살피는 낱말** (`ui::app::shell_menu`의 표 넷) —
            // 서버 응답을 살피는 위 `"권한"`과 같은 성질이다. 이 앱이 그리는 문구가 아니라
            // **셸이 준 문구**를 견주는 자리라 앱 언어를 따르면 판정이 통째로 어긋난다.
            // verb가 1차이고 이 낱말들은 verb를 주지 않는 확장을 잡는 2차 폴백이다
            "공유",
            "액세스 권한 부여",
            "메뉴 사용자 지정",
            "열기",
            "연결 프로그램",
            "경로로 복사",
            "시작 화면에 고정",
            "속성",
            "붙여넣기",
            "새로 만들기",
            "이전 버전 복원",
            "보내기",
            "Copilot에게 질문하기",
            "압축하기",
            "풀기",
        ];

        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        for entry in ROOTS {
            collect_rs(&root.join(entry), &mut files);
        }
        assert!(files.len() > 30, "훑을 파일을 찾지 못했다: {}", files.len());

        let mut 발견 = Vec::new();
        for path in files {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let source = std::fs::read_to_string(&path).expect("소스를 읽지 못했다");
            // 인라인 시험 모듈부터는 화면이 아니다. `*/tests.rs`는 파일 전체가 시험이다
            if rel.ends_with("/tests.rs") {
                continue;
            }
            let body = match source.find("\nmod tests {") {
                Some(cut) => &source[..cut],
                None => &source[..],
            };
            for (번호, 줄) in body.lines().enumerate() {
                let 코드 = 줄.split("//").next().unwrap_or("");
                // 개발자에게만 보이는 문구 — 화면에 나오지 않는다.
                // **같은 줄만 본다** — 앞 몇 줄까지 넓히면 단언 근처에 놓인 화면 문구가
                // 통째로 빠져나간다. 여러 줄에 걸친 단언의 문구는 아래 리터럴 예외로 가린다
                if 코드.contains("assert")
                    || 코드.contains("expect(")
                    || 코드.contains("panic!")
                    || 코드.contains("must_use")
                    || 코드.contains("Error::other")
                {
                    continue;
                }
                for literal in string_literals(코드) {
                    if !literal.chars().any(|c| ('가'..='힣').contains(&c)) {
                        continue;
                    }
                    if EXEMPT_LITERALS.contains(&literal.as_str()) {
                        continue;
                    }
                    발견.push(format!("{rel}:{}: \"{literal}\"", 번호 + 1));
                }
            }
        }
        assert!(
            발견.is_empty(),
            "카탈로그를 거치지 않은 화면 문구가 있다 — `i18n`에 키를 만들어 쓴다: {발견:#?}"
        );
    }

    /// `.rs` 파일을 모은다 (하위 폴더까지)
    fn collect_rs(path: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        if path.is_file() {
            out.push(path.to_path_buf());
            return;
        }
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let child = entry.path();
            if child.is_dir() {
                collect_rs(&child, out);
            } else if child.extension().is_some_and(|ext| ext == "rs") {
                out.push(child);
            }
        }
    }

    /// 한 줄에서 문자열 리터럴만 뽑는다 (이스케이프는 건너뛴다)
    fn string_literals(line: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '"' {
                continue;
            }
            let mut lit = String::new();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        chars.next();
                    }
                    '"' => break,
                    _ => lit.push(c),
                }
            }
            out.push(lit);
        }
        out
    }

    #[test]
    fn 컨텍스트_메뉴가_스스로_세우는_문구는_두_언어를_모두_낸다() {
        // FR-8 재개정 — 셸이 주지 않아 **우리가 문구를 쥐는** 넷이다.
        // 기대값은 원문 리터럴이고 언어를 잠근다 (AGENTS 시험 규약)
        let _guard = LanguageGuard::lock(LanguageSetting::Korean);
        assert_eq!(menu_default(), "기본 메뉴");
        assert_eq!(menu_app_extensions(), "앱 확장");
        assert_eq!(menu_add_favorite(), "즐겨찾기에 추가");
        assert_eq!(menu_open_new_tab(), "새 탭에서 열기");

        set_language(LanguageSetting::English);
        assert_eq!(menu_default(), "Default menu");
        assert_eq!(menu_app_extensions(), "App extensions");
        assert_eq!(menu_add_favorite(), "Add to favorites");
        assert_eq!(menu_open_new_tab(), "Open in new tab");
    }

    #[test]
    fn 다운로드_진행률_문구는_두_언어를_모두_낸다() {
        // T3 acceptance ⓐ — 기대값은 원문 리터럴이고 언어를 잠근다 (AGENTS 시험 규약)
        let _guard = LanguageGuard::lock(LanguageSetting::Korean);
        assert_eq!(dynamic::update_downloading_percent(42), "다운로드 중 42%");
        assert_eq!(dynamic::update_downloading_percent(0), "다운로드 중 0%");
        assert_eq!(dynamic::update_downloading_percent(100), "다운로드 중 100%");

        set_language(LanguageSetting::English);
        assert_eq!(dynamic::update_downloading_percent(42), "Downloading 42%");
        assert_eq!(dynamic::update_downloading_percent(100), "Downloading 100%");
    }

    #[test]
    fn 로컬_복사_알림은_두_언어를_모두_낸다() {
        // Acceptance ⓕ (FR-60) — 기대값은 원문 리터럴이고 언어를 잠근다 (AGENTS 시험 규약)
        let _guard = LanguageGuard::lock(LanguageSetting::Korean);
        assert_eq!(
            dynamic::local_copy_failed("액세스가 거부되었습니다."),
            "복사하지 못했습니다 — 액세스가 거부되었습니다."
        );
        assert_eq!(
            dynamic::local_copy_cancelled(3),
            "복사를 그만뒀습니다 — 3개 항목"
        );

        set_language(LanguageSetting::English);
        assert_eq!(
            dynamic::local_copy_failed("Access is denied."),
            "Could not copy — Access is denied."
        );
        // 영어는 한 개일 때 복수형을 쓰지 않는다
        assert_eq!(dynamic::local_copy_cancelled(1), "Copy stopped — 1 item");
        assert_eq!(dynamic::local_copy_cancelled(3), "Copy stopped — 3 items");
    }

    #[test]
    fn 언어에_따라_다른_문구를_돌려준다() {
        let _guard = LanguageGuard::lock(LanguageSetting::Korean);
        assert_eq!(settings_title(), "설정");
        set_language(LanguageSetting::English);
        assert_eq!(settings_title(), "Settings");
    }

    #[test]
    fn 시스템_기본은_윈도우_ui_언어를_따른다() {
        let _guard = LanguageGuard::lock(LanguageSetting::System);
        assert_eq!(
            current(),
            system_language(),
            "시스템 기본인데 시스템 언어와 다르다"
        );
    }

    #[test]
    fn 값이_끼어드는_문구의_한국어는_이관_전_그대로다() {
        // 이관하며 조사·어순이 바뀌면 화면이 조용히 달라진다 — 문장 전체를 고정한다
        let _guard = LanguageGuard::lock(LanguageSetting::Korean);
        assert_eq!(
            dynamic::open_not_found("문서"),
            "'문서' 폴더를 찾을 수 없습니다"
        );
        assert_eq!(
            dynamic::create_failed("폴더", "접근 거부"),
            "새 폴더를 만들지 못했습니다 — 접근 거부"
        );
        assert_eq!(dynamic::item_counts(3, 12), "폴더 3 파일 12");
        assert_eq!(
            dynamic::remote_delete_count(3),
            "3개 항목을 서버에서 지웁니다."
        );
        // 같은 이름 확인 (FR-55) — 기대값은 언제나 원문 리터럴이다
        assert_eq!(conflict_title(), "같은 이름이 이미 있습니다");
        assert_eq!(conflict_overwrite(), "덮어쓰기");
        assert_eq!(conflict_skip(), "건너뛰기");
        assert_eq!(conflict_folder_mark(), "(폴더)");
        assert_eq!(conflict_irreversible(), "덮어쓰면 되돌릴 수 없습니다.");
        assert_eq!(
            dynamic::conflict_count(3),
            "3개 항목이 대상에 이미 있습니다."
        );
        // 한국어는 하나여도 문장이 같다 — 영어만 단수형으로 갈린다
        assert_eq!(
            dynamic::remote_delete_count(1),
            "1개 항목을 서버에서 지웁니다."
        );
        assert_eq!(
            dynamic::site_registered("example.test"),
            "example.test 등록됨 · 더블클릭하여 연결"
        );
        assert_eq!(
            dynamic::workspace_delete_confirm("작업"),
            "'작업' 워크스페이스를 삭제할까요?"
        );
        assert_eq!(
            dynamic::remote_open_failed("550 Denied"),
            "폴더를 열지 못했습니다 — 550 Denied"
        );
        assert_eq!(
            dynamic::remote_list_failed("timeout"),
            "목록을 읽지 못했습니다 — timeout"
        );
        assert_eq!(
            dynamic::skipped_folders(2),
            "읽을 수 없는 폴더 2개는 건너뛰었습니다"
        );
        assert_eq!(
            dynamic::op_delete_failed("550 Denied"),
            "삭제하지 못했습니다 — 550 Denied"
        );
        assert_eq!(
            dynamic::op_mkdir_failed("550 Denied"),
            "새 폴더를 만들지 못했습니다 — 550 Denied"
        );
        // 조사는 앞말의 받침을 따라 갈린다 — `을(를)` 병기를 화면에 내보내지 않는다
        assert_eq!(
            dynamic::create_failed("파일", "거부됨"),
            "새 파일을 만들지 못했습니다 — 거부됨"
        );
        assert_eq!(
            dynamic::create_failed("폴더", "거부됨"),
            "새 폴더를 만들지 못했습니다 — 거부됨"
        );
        assert_eq!(
            dynamic::err_not_found("/srv/data", "550"),
            "'/srv/data' 경로를 찾을 수 없습니다 — 550"
        );
        assert_eq!(dynamic::status_failed_count(3), "실패 3건");
        assert_eq!(dynamic::queue_site_fallback(3), "사이트 3");
        assert_eq!(
            dynamic::transfer_finalize_failed("access denied"),
            "받은 파일을 제자리에 두지 못했습니다: access denied"
        );
    }

    #[test]
    fn 영어_문구도_값을_제자리에_끼운다() {
        // 자리표시자가 빠지거나 뒤바뀌면 영어에서만 조용히 틀어진다
        let _guard = LanguageGuard::lock(LanguageSetting::English);
        assert_eq!(
            dynamic::workspace_delete_confirm("Work"),
            "Delete the workspace 'Work'?"
        );
        assert_eq!(
            dynamic::remote_open_failed("550 Denied"),
            "Could not open the folder — 550 Denied"
        );
        assert_eq!(
            dynamic::op_delete_failed("550 Denied"),
            "Could not delete — 550 Denied"
        );
        assert_eq!(dynamic::status_failed_count(1), "1 failed");
        assert_eq!(dynamic::status_failed_count(3), "3 failed");
        assert_eq!(dynamic::queue_site_fallback(3), "Site 3");
        // 영어는 하나일 때 단수형이다
        assert_eq!(
            dynamic::skipped_folders(1),
            "Skipped 1 folder that could not be read"
        );
        assert_eq!(
            dynamic::skipped_folders(2),
            "Skipped 2 folders that could not be read"
        );
    }

    #[test]
    fn 원격_오류_여덟_종의_한국어는_이관_전_그대로다() {
        // 조사(`을(를)`)까지 원문 그대로여야 한다 — 다듬으면 사용자가 보던 문구가 달라진다
        let _guard = LanguageGuard::lock(LanguageSetting::Korean);
        assert_eq!(
            dynamic::err_connect("timeout"),
            "연결하지 못했습니다 — timeout"
        );
        assert_eq!(dynamic::err_login("530"), "로그인하지 못했습니다 — 530");
        assert_eq!(
            dynamic::err_host_key("mismatch"),
            "호스트 키를 확인하지 못했습니다 — mismatch"
        );
        assert_eq!(
            dynamic::err_not_found("/none", "550"),
            "'/none' 경로를 찾을 수 없습니다 — 550"
        );
        assert_eq!(
            dynamic::err_permission("/etc", "550"),
            "'/etc'에 접근할 권한이 없습니다 — 550"
        );
        assert_eq!(
            dynamic::err_interrupted(1024, "reset"),
            "전송이 중단됐습니다 (1024바이트 진행) — reset"
        );
        assert_eq!(
            dynamic::err_unsupported("SITE CHMOD", "502"),
            "서버가 'SITE CHMOD' 명령을 지원하지 않습니다 — 502"
        );
        assert_eq!(
            dynamic::err_protocol("bad reply"),
            "서버와 통신하지 못했습니다 — bad reply"
        );
        assert_eq!(remote_cancelled(), "취소했습니다");
    }

    #[test]
    fn 원격_로그_문구의_한국어도_이관_전_그대로다() {
        // 서버 로그는 사용자가 그대로 읽는 화면이다 (FR-40)
        let _guard = LanguageGuard::lock(LanguageSetting::Korean);
        assert_eq!(
            dynamic::log_list_start("/var"),
            "\"/var\" 디렉터리 목록 조회…"
        );
        assert_eq!(
            dynamic::log_list_done("/var"),
            "\"/var\" 디렉터리 목록 조회 성공"
        );
        assert_eq!(dynamic::log_connecting("host:21"), "host:21에 연결…");
        assert_eq!(
            dynamic::log_retry(5, "timeout"),
            "연결에 실패해 5초 뒤 다시 시도합니다 — timeout"
        );
        assert_eq!(
            dynamic::log_too_deep("/deep"),
            "/deep 아래는 너무 깊어 건너뜁니다"
        );
        assert_eq!(
            dynamic::log_read_failed("/x", "denied"),
            "/x 를 읽지 못했습니다: denied"
        );
        assert_eq!(
            dynamic::hostkey_unverifiable("바뀜"),
            "바뀜 (확인할 수단이 없습니다)"
        );
        assert_eq!(
            dynamic::tls_setup_failed("bad cert"),
            "TLS 설정을 준비하지 못했습니다 — bad cert"
        );
        assert_eq!(
            dynamic::auth_no_password("denied", "publickey"),
            "denied — 이 서버는 비밀번호 인증을 받지 않습니다 (받는 방식: publickey)"
        );
        // 줄을 이어 붙인 문장은 **공백이 접히는 자리**가 어긋나기 쉽다 — 한 칸인지 본다
        assert_eq!(
            dynamic::hostkey_changed_detail("ab:cd"),
            "전에 저장한 지문은 ab:cd 였습니다. 서버를 다시 설치했거나, 중간에 다른 서버가 끼어든 것일 수 있습니다."
        );
        assert_eq!(
            dynamic::hostkey_changed_reason("ab:cd", "ef:gh"),
            "서버 지문이 전에 저장해 둔 것과 다릅니다 (저장된 값 ab:cd, 이번 값 ef:gh) — 서버를 다시 설치했거나 중간에 다른 서버가 끼어든 것일 수 있습니다"
        );
    }

    #[test]
    fn 영어_오류도_서버_원문을_그대로_싣는다() {
        // 서버가 준 말(`530 Login incorrect`)은 번역하지 않는다 — 사용자가 서버 관리자에게
        // 전할 값이라 원문이 오히려 쓸모 있다
        let _guard = LanguageGuard::lock(LanguageSetting::English);
        assert_eq!(
            dynamic::err_login("530 Login incorrect"),
            "Could not log in — 530 Login incorrect"
        );
        assert_eq!(
            dynamic::err_not_found("/none", "550 No such file"),
            "Could not find '/none' — 550 No such file"
        );
        // 재시도 안내는 1초일 때 단수형이다
        assert_eq!(
            dynamic::log_retry(1, "timeout"),
            "Connection failed, retrying in 1 second — timeout"
        );
        assert_eq!(
            dynamic::log_retry(5, "timeout"),
            "Connection failed, retrying in 5 seconds — timeout"
        );
    }

    #[test]
    fn 영어_삭제_확인은_하나일_때_단수형이다() {
        let _guard = LanguageGuard::lock(LanguageSetting::English);
        assert_eq!(
            dynamic::remote_delete_count(1),
            "1 item will be deleted from the server."
        );
        assert_eq!(
            dynamic::remote_delete_count(3),
            "3 items will be deleted from the server."
        );
    }

    #[test]
    fn 내보내기_알림은_비밀번호가_담겼음을_두_언어로_알린다() {
        // 내보내기에 대화가 없어져(FR-59) 이 알림이 파일의 성질을 알릴 유일한 자리다.
        // 비밀번호를 읽지 못한 것이 겹칠 때 문장이 어떻게 이어지는지도 여기서 고정한다
        let _guard = LanguageGuard::lock(LanguageSetting::Korean);
        assert_eq!(
            dynamic::site_export_done(3, 0),
            "사이트 3개를 저장했습니다 · 비밀번호가 함께 담겼습니다"
        );
        assert_eq!(
            dynamic::site_export_done(3, 1),
            "사이트 3개를 저장했습니다 · 비밀번호가 함께 담겼습니다 · 1개는 비밀번호를 읽지 못해 뺐습니다"
        );
        // 사이트가 없어도 문장이 성립한다
        assert_eq!(
            dynamic::site_export_done(0, 0),
            "사이트 0개를 저장했습니다 · 비밀번호가 함께 담겼습니다"
        );

        set_language(LanguageSetting::English);
        assert_eq!(
            dynamic::site_export_done(1, 0),
            "Saved 1 site · passwords included"
        );
        assert_eq!(
            dynamic::site_export_done(3, 2),
            "Saved 3 sites · passwords included · 2 of them lost their password (it could not be read)"
        );
    }

    #[test]
    fn 큐_요약은_언어마다_조각_순서가_다르다() {
        // 한국어는 `남음`이 뒤에, 영어는 `left`가 뒤에 온다 — 조각을 이어 붙이면 어순이 깨진다
        let _guard = LanguageGuard::lock(LanguageSetting::Korean);
        assert_eq!(
            dynamic::queue_summary(3, Some("12.4 MB/s"), Some("00:41"), "—"),
            "3건 대기 · 12.4 MB/s · 00:41 남음"
        );
        // 남은 시간을 모르면 호출부가 준 값이 그 자리에 들어간다
        assert_eq!(dynamic::queue_summary(3, None, None, "—"), "3건 대기 · —");
        set_language(LanguageSetting::English);
        assert_eq!(
            dynamic::queue_summary(3, Some("12.4 MB/s"), Some("00:41"), "—"),
            "3 pending · 12.4 MB/s · 00:41 left"
        );
    }

    #[test]
    fn 앱_이름과_버전_줄이_언어를_따른다() {
        // 이름은 정보 대화·창 제목·트레이 툴팁 셋이 함께 쓴다 (FR-53·FR-58).
        // 데이터가 걸린 이름(레지스트리 값·설정 파일)은 이 값을 따르지 않는다
        let version = env!("CARGO_PKG_VERSION");
        {
            let _guard = LanguageGuard::lock(LanguageSetting::Korean);
            assert_eq!(app_name(), "모아");
            assert_eq!(dynamic::about_version_line(), format!("모아 {version}"));
        }
        let _guard = LanguageGuard::lock(LanguageSetting::English);
        assert_eq!(app_name(), "MOA");
        assert_eq!(dynamic::about_version_line(), format!("MOA {version}"));
    }

    #[test]
    fn 알_수_없는_값은_한국어로_읽는다() {
        // 이 앱의 화면이 원래 한국어다 — 모르는 값에 영어를 주면 갑자기 화면이 바뀐다
        let _guard = LanguageGuard::lock(LanguageSetting::Korean);
        CURRENT.store(99, Ordering::Relaxed);
        assert_eq!(current(), Language::Korean);
    }
}
