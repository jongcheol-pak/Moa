//! 파일 목록의 순수 규칙 (FR-4·FR-5) — 정렬 비교·크기/날짜 표기·행 추상(`ListRow`).
//!
//! 화면은 `ui::file_list`·`ui::list_details`·`ui::list_grid`가 그린다.
use crate::fs::enumerate::FileEntry;
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM};
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::Win32::UI::Shell::StrCmpLogicalW;
use windows::core::PCWSTR;

/// 정렬 열
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SortKey {
    #[default]
    Name,
    Size,
    Type,
    Modified,
}

impl SortKey {
    /// 정렬할 수 있는 열 전부 — 저장 키를 되읽을 때 이 목록을 훑는다
    pub const ALL: [SortKey; 4] = [
        SortKey::Name,
        SortKey::Size,
        SortKey::Type,
        SortKey::Modified,
    ];

    /// 세션에 담는 키 (FR-4) — `ui::view_mode::ViewMode::as_key`와 같은 방식이다.
    ///
    /// **숫자가 아니라 문자열인 이유**: 나중에 정렬 열이 늘거나 차례가 바뀌면 숫자는 값이
    /// 밀려 옛 파일이 엉뚱한 열로 정렬된다
    pub fn as_key(self) -> &'static str {
        match self {
            SortKey::Name => "name",
            SortKey::Size => "size",
            SortKey::Type => "type",
            SortKey::Modified => "modified",
        }
    }

    /// 저장된 키로 되살린다. 비었거나 모르는 키는 기본값(이름)으로 — 손으로 고친 설정
    /// 파일이나 옛 판이 쓰던 키가 남아 있어도 목록이 어긋난 채로 열리지 않게 한다
    pub fn from_key(key: &str) -> SortKey {
        SortKey::ALL
            .into_iter()
            .find(|sort| sort.as_key() == key)
            .unwrap_or_default()
    }
}

/// 정렬 비교 — 폴더 우선(D7), 이름은 탐색기와 동일한 숫자 인지 정렬.
/// `pub`인 이유: egui UI 계층(`ui::file_list`)이 같은 정렬 규칙을 쓰기 위해 (이식 plan part1 4-D).
///
/// 주의: 폴더 우선은 여기서 **방향과 무관하게** 결정된다 — 내림차순을 만들려고 반환값 전체를
/// `reverse()`하면 폴더 우선까지 뒤집힌다. 같은 종류끼리만 뒤집어야 한다(part1 D13)
pub fn compare_entries(
    a: &FileEntry,
    type_a: &str,
    b: &FileEntry,
    type_b: &str,
    key: SortKey,
) -> std::cmp::Ordering {
    compare_rows(a, type_a, b, type_b, key)
}

/// 목록 맨 위의 상위 이동 줄 이름 — 로컬·원격이 같은 값을 쓴다.
///
/// 만드는 곳(`ui::panel`)과 해석하는 곳(선택·개수·드래그 제외)이 갈라져 있어 한 자리에 둔다
pub const PARENT_ENTRY: &str = "..";

/// 목록 한 줄이 정렬·표시에 내주어야 하는 것 (plan T7).
///
/// 로컬(`fs::enumerate::FileEntry`)과 원격(`remote::types::RemoteEntry`)은 이름·시각을 담는
/// 방식이 서로 다르지만(널 종단 UTF-16 + FILETIME ↔ `String` + 유닉스 초), **정렬 규칙은
/// 한 벌이어야** 화면이 프로토콜마다 달라지지 않는다. 그 한 벌을 이 트레이트가 잇는다.
///
/// 렌더·선택·아이콘은 여기 두지 않는다 — 정렬과 표시 문자열에 필요한 것만이다.
/// 트레이트 객체가 아니라 **제네릭**으로 쓴다(10만 항목 정렬에 가상 호출을 넣지 않는다).
pub trait ListRow {
    /// 표시용 이름
    fn name(&self) -> String;

    /// 목록에 그릴 이름 — `show_extensions`가 꺼져 있으면 확장자를 뗀다 (FR-52).
    ///
    /// **`name()`을 대신하지 않는다**: 경로 조립(`dir.join(name)`)·정렬 키·선택 복원은
    /// 여전히 원래 이름을 써야 한다. 잘린 이름으로 경로를 만들면 파일 실행과 셸 메뉴가
    /// 통째로 깨진다 (D7). 그래서 **그리는 자리만** 이 메서드를 쓴다.
    ///
    /// 폴더·`..`·앞이 빈 이름(`.gitignore`)·끝이 점인 이름(`a.`)은 확장자 개념이
    /// 없어 그대로 둔다
    fn display_name(&self, show_extensions: bool) -> String {
        let name = self.name();
        if show_extensions || self.is_dir() || self.is_parent() {
            return name;
        }
        match name.rsplit_once('.') {
            // 앞이 비면 확장자가 아니라 이름 자체이고(`.gitignore`),
            // 뒤가 비면 잘라도 얻는 게 없다(`a.`)
            Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => stem.to_owned(),
            _ => name,
        }
    }

    /// **정렬용 널 종단 UTF-16 이름.**
    ///
    /// 이름 비교는 탐색기와 같은 `StrCmpLogicalW`로 한다("파일2" < "파일10") — 그 API가
    /// 널 종단 UTF-16을 받으므로 여기서 그 형태로 내준다. 로컬 항목은 이미 그 모양이라
    /// 빌려주고, 원격 항목만 그때 만든다
    fn name_sort_key(&self) -> std::borrow::Cow<'_, [u16]>;

    /// 상위 이동(`..`) 줄인가 — 실제 항목이 아니므로 **선택·개수·끌기에서 빠진다**.
    ///
    /// 기본 구현은 표시 이름을 만들어 견준다. 이름을 UTF-16으로 든 로컬 항목은 문자열을
    /// 새로 만들지 않도록 따로 구현한다(10만 항목 폴더에서 매 정렬마다 도는 자리다 — NFR-3)
    fn is_parent(&self) -> bool {
        self.name() == PARENT_ENTRY
    }

    fn is_dir(&self) -> bool;

    /// 숨김 항목인가 — 숨김을 보이는 설정이 꺼져 있으면 목록에서 빠진다 (FR-13).
    ///
    /// 로컬은 `FILE_ATTRIBUTE_HIDDEN`을, 원격은 이름이 `.`으로 시작하는지를 본다 —
    /// 유닉스 계열 서버에는 숨김 속성이 따로 없고 그 관례가 곧 규칙이다.
    ///
    /// **화면 문구를 여기 적지 않는다** — 설정 라벨은 `i18n` 카탈로그가 언어마다 정하므로
    /// 그것을 주석에 박으면 문구를 바꿀 때마다 이 자리가 조용히 낡는다
    fn is_hidden(&self) -> bool;

    /// 시스템 항목인가 — 시스템 항목을 보이는 설정이 꺼져 있으면 목록에서 빠진다 (FR-13).
    ///
    /// **숨김과 따로 묻는 이유**: 두 속성에 각자의 설정이 대응하고, 둘 다 붙은 항목
    /// (`pagefile.sys` 등)은 두 설정이 모두 켜져야 보인다. 원격은 언제나 `false`다 —
    /// 유닉스 계열 서버에 이 속성이 없어 흉내 낼 근거가 없다
    fn is_system(&self) -> bool;

    /// 목록에서 흐리게 그릴 항목인가 — 숨김이거나 시스템이면 보통 항목이 아니다 (FR-13).
    ///
    /// 거르기와 달리 **둘을 함께 본다**: 설정을 켜서 보이기로 한 항목이 어느 쪽이든
    /// 보통 항목과 구분돼야 하고, 그 판정이 그리는 자리마다 흩어지면 보기 모드에 따라
    /// 다르게 보인다
    fn is_dimmed(&self) -> bool {
        self.is_hidden() || self.is_system()
    }

    fn is_symlink(&self) -> bool;

    /// 심볼릭 링크가 가리키는 곳 — 이름 뒤에 `→ 대상`으로 붙는다 (FR-31)
    fn link_target(&self) -> Option<&str>;

    fn size(&self) -> u64;

    /// **정렬 전용 시각 키** — 1601-01-01부터의 100나노초 단위(FILETIME 눈금)다.
    ///
    /// 유닉스 초로 맞추지 않는 이유: 로컬 항목의 시각은 100나노초 정밀도라, 초 단위로 깎으면
    /// **같은 초에 만들어진 파일들의 차례가 바뀐다**(지금 동작이 달라진다). 표시용 변환은
    /// 각 목록이 자기 원본 값으로 한다
    fn modified_key(&self) -> u64;

    /// 소문자 확장자 (`""` = 없음/폴더)
    fn extension(&self) -> String;

    /// POSIX 권한 표기(`rwxr-xr-x`) — **원격 항목만** 갖는다 (FR-31).
    ///
    /// 로컬은 `None`이다: Windows 파일의 권한은 ACL이라 이 아홉 글자로 옮길 수 없고,
    /// 억지로 흉내 내면 화면이 실제 권한과 다른 말을 하게 된다
    fn permissions(&self) -> Option<String>;

    /// 소유자 — **원격 항목만** 갖는다. 서버가 이름을 주지 않으면 숫자 uid 그대로다
    fn owner(&self) -> Option<&str>;

    /// 목록 폴더 아래 어느 하위 폴더에 있는가 — **하위 폴더 검색 결과만** 갖는다 (FR-70).
    ///
    /// 목록 폴더 기준 상대 경로다. 목록 폴더 바로 아래 항목은 `None`이다
    fn location(&self) -> Option<&std::path::Path> {
        None
    }

    /// 이 행의 로컬 전체 경로 — 목록 폴더 `dir`에 하위 폴더(`location`)와 이름을 잇는다.
    ///
    /// 이름만 이으면 검색 결과의 하위 폴더 항목이 엉뚱한 경로를 가리킨다 — 경로를 짓는 자리는
    /// 모두 이것을 쓴다
    fn path_in(&self, dir: &std::path::Path) -> std::path::PathBuf {
        match self.location() {
            Some(sub) => dir.join(sub).join(self.name()),
            None => dir.join(self.name()),
        }
    }
}

/// 유닉스 초(UTC) → FILETIME 눈금. 두 목록이 같은 자로 정렬되게 맞춘다
pub fn unix_seconds_to_filetime(seconds: i64) -> u64 {
    const EPOCH_DIFFERENCE_SECONDS: i64 = 11_644_473_600;
    let since_1601 = seconds.saturating_add(EPOCH_DIFFERENCE_SECONDS).max(0);
    (since_1601 as u64).saturating_mul(10_000_000)
}

impl ListRow for FileEntry {
    fn name(&self) -> String {
        self.name_string()
    }

    fn name_sort_key(&self) -> std::borrow::Cow<'_, [u16]> {
        // 이미 널 종단 UTF-16이라 그대로 빌려준다
        std::borrow::Cow::Borrowed(&self.name)
    }

    /// 널 종단 UTF-16 `..`과 곧바로 견준다 — 표시용 문자열을 만들지 않는다
    fn is_parent(&self) -> bool {
        *self.name == [b'.' as u16, b'.' as u16, 0]
    }

    fn is_dir(&self) -> bool {
        self.is_dir
    }

    fn is_hidden(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_HIDDEN.0 != 0
    }

    fn is_system(&self) -> bool {
        self.attributes & FILE_ATTRIBUTE_SYSTEM.0 != 0
    }

    fn is_symlink(&self) -> bool {
        // 로컬 목록은 링크를 따로 표시하지 않는다 (원격 전용 표시다 — FR-31)
        false
    }

    fn link_target(&self) -> Option<&str> {
        None
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn modified_key(&self) -> u64 {
        self.modified
    }

    fn extension(&self) -> String {
        self.extension()
    }

    /// 로컬 파일의 권한은 ACL이라 아홉 글자 표기로 옮길 수 없다 — 흉내 내지 않는다
    fn permissions(&self) -> Option<String> {
        None
    }

    fn owner(&self) -> Option<&str> {
        None
    }

    fn location(&self) -> Option<&std::path::Path> {
        self.location.as_deref()
    }
}

impl ListRow for crate::remote::types::RemoteEntry {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn name_sort_key(&self) -> std::borrow::Cow<'_, [u16]> {
        let mut wide: Vec<u16> = self.name.encode_utf16().collect();
        wide.push(0);
        std::borrow::Cow::Owned(wide)
    }

    /// 이름과 곧바로 견준다 — 기본 구현은 이름을 복제해 견줘, 정렬 비교마다 할당이 붙는다
    fn is_parent(&self) -> bool {
        self.name == PARENT_ENTRY
    }

    fn is_dir(&self) -> bool {
        self.is_dir
    }

    fn is_hidden(&self) -> bool {
        // 유닉스 계열 서버에는 숨김 속성이 없다 — `.`으로 시작하는 이름이 그 관례다.
        // `..`는 이름이 점으로 시작하지만 화면 장치이므로 걸러지면 안 된다
        !self.is_parent() && self.name.starts_with('.')
    }

    /// 원격에는 시스템 속성이 없다 — 서버가 주지 않는 것을 이름으로 흉내 내지 않는다
    fn is_system(&self) -> bool {
        false
    }

    fn is_symlink(&self) -> bool {
        self.is_symlink
    }

    fn link_target(&self) -> Option<&str> {
        self.link_target.as_deref()
    }

    fn size(&self) -> u64 {
        self.size
    }

    fn modified_key(&self) -> u64 {
        self.modified.map_or(0, unix_seconds_to_filetime)
    }

    fn extension(&self) -> String {
        self.extension()
    }

    /// 서버가 권한을 주지 않았으면 `None`이다 — 빈칸으로 보인다 (plan Edge Case).
    /// `0o777` 같은 기본값을 지어내지 않는 것은 서버가 하지 않은 말을 화면이 하지 않게 하기 위함이다
    fn permissions(&self) -> Option<String> {
        self.permissions_string()
    }

    fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }
}

/// 정렬 비교 — 로컬·원격이 함께 쓰는 한 벌의 규칙. `compare_entries`가 이것에 위임한다.
///
/// 폴더 우선은 정렬 방향과 무관하게 결정된다 — 위 `compare_entries` 주석과 같은 주의다
pub fn compare_rows<R: ListRow + ?Sized>(
    a: &R,
    type_a: &str,
    b: &R,
    type_b: &str,
    key: SortKey,
) -> std::cmp::Ordering {
    compare_rows_keyed(
        a,
        &a.name_sort_key(),
        type_a,
        b,
        &b.name_sort_key(),
        type_b,
        key,
    )
}

/// `compare_rows`와 같은 규칙이되 **이름 키를 부르는 쪽이 미리 만들어 넘긴다**.
///
/// 원격 항목의 이름 키(널 종단 UTF-16)는 부를 때마다 새로 만들어진다 — 1만 항목을 정렬하면
/// 비교 약 13만 번에 할당이 두 번씩 붙는다. 정렬하는 쪽이 행마다 한 번 만들어 두고 이것을 부른다
#[allow(clippy::too_many_arguments)]
pub fn compare_rows_keyed<R: ListRow + ?Sized>(
    a: &R,
    name_a: &[u16],
    type_a: &str,
    b: &R,
    name_b: &[u16],
    type_b: &str,
    key: SortKey,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a.is_dir(), b.is_dir()) {
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        _ => {}
    }
    let by_name = || logical_name_cmp(name_a, name_b);
    match key {
        SortKey::Name => by_name(),
        SortKey::Size => a.size().cmp(&b.size()).then_with(by_name),
        SortKey::Type => type_a.cmp(type_b).then_with(by_name),
        SortKey::Modified => a.modified_key().cmp(&b.modified_key()).then_with(by_name),
    }
}

/// StrCmpLogicalW 래퍼 — 널 종단 UTF-16 이름 비교 ("파일2" < "파일10")
fn logical_name_cmp(a: &[u16], b: &[u16]) -> std::cmp::Ordering {
    // 안전성: 두 버퍼 모두 널 종단이 보장된다 — 로컬은 `FileEntry.name` 불변식이,
    // 원격은 `ListRow::name_sort_key` 구현이 끝에 0을 붙여 만든다
    let r = unsafe { StrCmpLogicalW(PCWSTR(a.as_ptr()), PCWSTR(b.as_ptr())) };
    r.cmp(&0)
}

/// 크기 표시: 소수점 둘째자리 + KB·MB·GB 자동 승격.
/// `pub`인 이유: egui UI 계층(자세히·격자 보기와 전송 큐·상태 표시줄)이 같은 표시 규칙을
/// 쓰기 위해 — 복제하면 표시 형식이 두 벌로 갈라진다 (이식 plan part1 4-D)
///
/// **KB 아래 단위(B)는 쓰지 않는다** — 목록의 `크기` 열에 B·KB·MB·GB 넷이 섞이면
/// 자릿수만 보고 크기를 견줄 수 없다 (2026-08-18 사용자 결정)
pub fn format_size(bytes: u64) -> String {
    const STEP: f64 = 1024.0;
    const UNITS: [&str; 3] = ["KB", "MB", "GB"];
    let mut value = bytes as f64 / STEP;
    let mut unit = 0;
    // 승격 판정을 **반올림한 값**으로 한다 — 그러지 않으면 1,048,571바이트가
    // `1024.00 KB`로 나온다(표시값은 한 칸을 채웠는데 단위가 안 올라간다)
    while unit + 1 < UNITS.len() && (value * 100.0).round() / 100.0 >= STEP {
        value /= STEP;
        unit += 1;
    }
    // 0이 아닌데 `0.00 KB`면 빈 파일과 구분되지 않는다 — 최소 한 칸은 채운다
    if bytes > 0 && unit == 0 && value < 0.01 {
        value = 0.01;
    }
    format!("{value:.2} {}", UNITS[unit])
}

/// 로컬 시각으로 푼 FILETIME — 화면에 적을 조각들.
///
/// 포맷을 함께 정하지 않고 **조각만** 내주는 이유: 같은 시각을 쓰는 자리마다 적는 모양이
/// 다르다(파일 목록은 분까지, 전송 큐는 초까지 + 오늘이면 날짜를 생략한다). 변환은 한 번만
/// 하고 모양은 부르는 쪽이 정한다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

impl LocalTime {
    /// 같은 날인가 — 「오늘이면 날짜를 생략한다」 판정이 쓴다
    pub fn same_day(self, other: LocalTime) -> bool {
        self.year == other.year && self.month == other.month && self.day == other.day
    }
}

/// FILETIME(u64)을 로컬 시각 조각으로 푼다 — 풀지 못하면 `None`.
///
/// **0은 「시각을 모른다」로 본다** — Windows는 그 값을 기점인 `1601-01-01`로 멀쩡히 풀어
/// 주므로, 거르지 않으면 시각을 주지 않은 항목(상위 이동 `..`·서버가 시각을 빠뜨린 파일·
/// 아직 시작하지 않은 전송)에 **1601년이 진짜 날짜인 양 뜬다**. 그 함정은 `ui::list_details`가
/// 자기 자리에서 따로 막고 있었는데(`Modified` 열의 `0 =>` 갈래) 같은 함수를 쓰는 다른 곳은
/// 막지 않아 실제로 새고 있었다 — 변환하는 이 자리에서 한 번 거르면 부르는 쪽마다
/// 되풀이하지 않아도 된다.
///
/// 그 밖에 실패하는 길은 값이 FILETIME 범위 밖일 때다(손상된 세션 값)
pub fn local_time_parts(ft: u64) -> Option<LocalTime> {
    use windows::Win32::Foundation::FILETIME;
    if ft == 0 {
        return None;
    }
    let ft = FILETIME {
        dwLowDateTime: (ft & 0xffff_ffff) as u32,
        dwHighDateTime: (ft >> 32) as u32,
    };
    let mut st_utc = Default::default();
    let mut st_local = Default::default();
    // 안전성: 모든 인자 스택 소유 — 실패 시 `None`
    unsafe {
        if FileTimeToSystemTime(&ft, &mut st_utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &st_utc, &mut st_local).is_err()
        {
            return None;
        }
    }
    Some(LocalTime {
        year: st_local.wYear,
        month: st_local.wMonth,
        day: st_local.wDay,
        hour: st_local.wHour,
        minute: st_local.wMinute,
        second: st_local.wSecond,
    })
}

/// FILETIME(u64) → 로컬 "yyyy-MM-dd HH:mm".
/// `pub`인 이유는 `format_size`와 동일 (egui UI 계층과 표시 규칙 공유)
pub fn format_filetime(ft: u64) -> String {
    let Some(t) = local_time_parts(ft) else {
        return String::new();
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        t.year, t.month, t.day, t.hour, t.minute
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::types::RemoteEntry;

    /// 0은 「시각을 모른다」다 — 거르지 않으면 FILETIME 기점인 `1601-01-01`이
    /// 진짜 날짜인 양 화면에 뜬다(그리드 보기에서 실제로 그랬다)
    #[test]
    fn 시각을_모르는_항목은_빈칸이다() {
        assert_eq!(local_time_parts(0), None);
        assert_eq!(format_filetime(0), "");
    }

    /// 안을 가르기 전후로 표기가 같아야 한다 — `local_time_parts`는 변환만 떼어 낸 것이지
    /// 모양을 바꾼 것이 아니다
    #[test]
    fn 파일_시각은_분까지_적는다() {
        // 2026-08-28 14:03:21 (로컬)을 가리키는 FILETIME
        let ft = {
            use windows::Win32::Foundation::SYSTEMTIME;
            use windows::Win32::System::Time::{
                SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime,
            };
            let local = SYSTEMTIME {
                wYear: 2026,
                wMonth: 8,
                wDay: 28,
                wHour: 14,
                wMinute: 3,
                wSecond: 21,
                ..Default::default()
            };
            // 안전성: 인자가 모두 스택 소유다
            unsafe {
                let mut utc = Default::default();
                TzSpecificLocalTimeToSystemTime(None, &local, &mut utc).expect("로컬 → UTC");
                let mut ft = Default::default();
                SystemTimeToFileTime(&utc, &mut ft).expect("UTC → FILETIME");
                u64::from(ft.dwLowDateTime) | (u64::from(ft.dwHighDateTime) << 32)
            }
        };
        assert_eq!(format_filetime(ft), "2026-08-28 14:03");
        let t = local_time_parts(ft).expect("풀려야 한다");
        // 초까지 살아 있다 — 큐의 시간 열이 그것을 쓴다
        assert_eq!((t.hour, t.minute, t.second), (14, 3, 21));
    }

    fn entry(name: &str, is_dir: bool, size: u64, modified: u64) -> FileEntry {
        let mut v: Vec<u16> = name.encode_utf16().collect();
        v.push(0);
        FileEntry {
            name: v.into(),
            is_dir,
            size,
            modified,
            attributes: 0,
            location: None,
        }
    }

    #[test]
    fn 정렬_기준은_저장_키로_왕복한다() {
        // 세션에 담기는 값이라 키가 바뀌면 옛 파일의 정렬이 사라진다
        for key in SortKey::ALL {
            assert_eq!(SortKey::from_key(key.as_key()), key);
        }
        let mut keys: Vec<&str> = SortKey::ALL.iter().map(|sort| sort.as_key()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), SortKey::ALL.len(), "키가 겹치면 왕복이 깨진다");
    }

    #[test]
    fn 모르는_정렬_키는_이름으로_떨어진다() {
        // 손으로 고친 파일·옛 판이 쓰던 키 대비
        assert_eq!(SortKey::from_key(""), SortKey::Name);
        assert_eq!(SortKey::from_key("없는_기준"), SortKey::Name);
        assert_eq!(SortKey::from_key("Name"), SortKey::Name);
    }

    #[test]
    fn 폴더가_항상_우선한다() {
        let d = entry("zzz", true, 0, 0);
        let f = entry("aaa.txt", false, 10, 0);
        assert_eq!(
            compare_entries(&d, "폴더", &f, "텍스트", SortKey::Name),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn 이름은_숫자_인지_정렬이다() {
        let a = entry("파일2.txt", false, 0, 0);
        let b = entry("파일10.txt", false, 0, 0);
        assert_eq!(
            compare_entries(&a, "", &b, "", SortKey::Name),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn 크기_정렬은_수치_비교다() {
        let small = entry("b.bin", false, 512, 0);
        let big = entry("a.bin", false, 2048, 0);
        assert_eq!(
            compare_entries(&small, "", &big, "", SortKey::Size),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn 크기_표시는_소수_둘째자리에_단위를_올린다() {
        // 2026-08-18 사용자 결정 — 파일 목록·전송 큐·상태 표시줄이 같은 규칙을 쓴다
        assert_eq!(format_size(0), "0.00 KB");
        assert_eq!(format_size(512), "0.50 KB");
        assert_eq!(format_size(1024), "1.00 KB");
        assert_eq!(format_size(1_234), "1.21 KB");
        assert_eq!(format_size(1_234_567), "1.18 MB");
        assert_eq!(format_size(2 * 1024 * 1024 * 1024), "2.00 GB");
    }

    #[test]
    fn 크기_표시의_경계값() {
        // 0이 아닌데 `0.00 KB`면 빈 파일과 구분되지 않는다
        assert_eq!(format_size(1), "0.01 KB");
        assert_eq!(format_size(10), "0.01 KB");
        // 승격은 **반올림한 값**으로 판정한다 — 아니면 `1024.00 KB`가 나온다
        assert_eq!(format_size(1_048_570), "1023.99 KB");
        assert_eq!(format_size(1_048_571), "1.00 MB");
        // GB 위로는 올릴 단위가 없어 그대로 둔다(지수 표기로 새지 않는다)
        assert_eq!(format_size(u64::MAX), "17179869184.00 GB");
    }

    #[test]
    fn 날짜_정렬은_원시값_비교다() {
        let old = entry("old", false, 0, 100);
        let new = entry("new", false, 0, 200);
        assert_eq!(
            compare_entries(&old, "", &new, "", SortKey::Modified),
            std::cmp::Ordering::Less
        );
    }

    fn remote(name: &str, is_dir: bool, size: u64, modified: Option<i64>) -> RemoteEntry {
        RemoteEntry {
            name: name.to_owned(),
            is_dir,
            is_symlink: false,
            link_target: None,
            size,
            modified,
            mode: None,
            owner: None,
        }
    }

    #[test]
    fn 원격_상위_줄은_점_둘_하나뿐이다() {
        // `..`만 상위 이동 줄이다 — 점으로 시작하는 실제 이름은 숨김 항목일 뿐 상위 줄이 아니다
        assert!(remote("..", true, 0, None).is_parent());
        for name in [".git", "..a", ".", "...", "a.."] {
            assert!(
                !remote(name, true, 0, None).is_parent(),
                "{name}이 상위 줄로 읽혔다"
            );
        }
        assert!(
            !remote("..", true, 0, None).is_hidden(),
            "상위 줄이 숨김으로 걸러진다"
        );
        assert!(remote(".git", true, 0, None).is_hidden());
    }

    #[test]
    fn 원격_항목도_같은_규칙으로_정렬된다() {
        // 화면이 프로토콜마다 다르게 줄 세우면 안 된다 (plan T7)
        let mut rows = [
            remote("파일10.txt", false, 10, Some(100)),
            remote("파일2.txt", false, 20, Some(200)),
            remote("zzz", true, 0, Some(0)),
        ];
        rows.sort_by(|a, b| compare_rows(a, "", b, "", SortKey::Name));
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        // 폴더 우선 + 숫자 인지 정렬("파일2" < "파일10")
        assert_eq!(names, vec!["zzz", "파일2.txt", "파일10.txt"]);

        rows.sort_by(|a, b| compare_rows(a, "", b, "", SortKey::Size));
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["zzz", "파일10.txt", "파일2.txt"]);
    }

    #[test]
    fn 시각을_주지_않은_원격_항목이_가장_앞선다() {
        let mut rows = [
            remote("시각있음", false, 0, Some(1_700_000_000)),
            remote("시각없음", false, 0, None),
        ];
        rows.sort_by(|a, b| compare_rows(a, "", b, "", SortKey::Modified));
        assert_eq!(rows[0].name, "시각없음");
    }

    #[test]
    fn 정렬용_시각은_초로_깎이지_않는다() {
        // 유닉스 초로 맞추면 같은 초에 만들어진 로컬 파일들의 차례가 바뀐다
        let earlier = entry("a.txt", false, 0, 132_000_000_000_000_000);
        let later = entry("b.txt", false, 0, 132_000_000_005_000_000);
        assert_eq!(
            compare_entries(&earlier, "", &later, "", SortKey::Modified),
            std::cmp::Ordering::Less,
            "0.5초 차이가 같은 것으로 뭉개졌다"
        );
    }

    #[test]
    fn 유닉스_시각_변환은_기준점을_맞춘다() {
        // 1970-01-01 → FILETIME의 1601 기준 100나노초 단위
        assert_eq!(unix_seconds_to_filetime(0), 11_644_473_600 * 10_000_000);
        assert_eq!(
            unix_seconds_to_filetime(1),
            11_644_473_600 * 10_000_000 + 10_000_000
        );
        // 1970년 이전(음수)도 0 아래로 내려가지 않는다
        assert_eq!(unix_seconds_to_filetime(-11_644_473_600), 0);
        assert_eq!(unix_seconds_to_filetime(i64::MIN), 0);
    }

    #[test]
    fn 목록_한_줄의_표시값을_트레이트로_읽는다() {
        let local = entry("보고서.hwp", false, 1024, 7);
        assert_eq!(ListRow::name(&local), "보고서.hwp");
        assert_eq!(ListRow::extension(&local), "hwp");
        assert!(
            !local.is_symlink(),
            "로컬 목록은 링크를 따로 표시하지 않는다"
        );
        assert_eq!(ListRow::link_target(&local), None);
        assert_eq!(local.modified_key(), 7);

        let mut link = remote("current", false, 0, None);
        link.is_symlink = true;
        link.link_target = Some("releases/42".to_owned());
        assert!(link.is_symlink());
        assert_eq!(ListRow::link_target(&link), Some("releases/42"));
        // 폴더와 앞이 빈 이름은 확장자가 없다
        assert_eq!(ListRow::extension(&remote("폴더", true, 0, None)), "");
        assert_eq!(
            ListRow::extension(&remote(".gitignore", false, 0, None)),
            ""
        );
    }

    #[test]
    fn 이름이_같고_종류가_다르면_폴더가_앞선다() {
        let dir = remote("같은이름", true, 0, Some(1));
        let file = remote("같은이름", false, 0, Some(1));
        assert_eq!(
            compare_rows(&dir, "", &file, "", SortKey::Name),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_rows(&dir, "", &file, "", SortKey::Modified),
            std::cmp::Ordering::Less
        );
    }

    /// 속성이 붙은 로컬 항목 — 숨김 판정 시험용
    fn entry_with(name: &str, attributes: u32) -> FileEntry {
        let mut e = entry(name, false, 0, 0);
        e.attributes = attributes;
        e
    }

    #[test]
    fn 로컬_숨김과_시스템은_각자의_속성이다() {
        use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM};
        let 보통 = entry_with("보통.txt", 0);
        assert!(!보통.is_hidden() && !보통.is_system());
        assert!(!보통.is_dimmed(), "보통 항목을 흐리게 그리려 한다");

        // 두 속성에 각자의 설정이 대응한다 — 한쪽 판정이 다른 쪽을 물면 토글이 헛돈다
        let 숨김 = entry_with("숨김.txt", FILE_ATTRIBUTE_HIDDEN.0);
        assert!(숨김.is_hidden() && !숨김.is_system());

        let 시스템 = entry_with("pagefile.sys", FILE_ATTRIBUTE_SYSTEM.0);
        assert!(시스템.is_system() && !시스템.is_hidden());

        // 둘 다 붙은 항목 — 두 설정이 모두 켜져야 보인다(거르는 쪽 판정은 호출부가 한다)
        let 둘다 = entry_with(
            "System Volume Information",
            FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0,
        );
        assert!(둘다.is_hidden() && 둘다.is_system());

        // 어느 쪽이든 흐리게 그린다
        for e in [&숨김, &시스템, &둘다] {
            assert!(e.is_dimmed(), "{}를 흐리게 그리지 않는다", e.name());
        }

        // 이름이 점으로 시작해도 로컬은 속성으로만 판정한다 — 윈도우의 규칙이다
        assert!(!entry_with(".gitignore", 0).is_hidden());
    }

    #[test]
    fn 원격_숨김은_점으로_시작하는_이름이다() {
        // 유닉스 계열 서버에는 숨김 속성이 없다 (D8)
        assert!(!remote("보통.txt", false, 0, None).is_hidden());
        assert!(remote(".bashrc", false, 0, None).is_hidden());
        assert!(remote(".ssh", true, 0, None).is_hidden());
        // `..`는 이름이 점으로 시작하지만 화면 장치다 — 걸러지면 맨 윗줄이 사라진다
        assert!(!remote("..", true, 0, None).is_hidden());
        // 서버는 시스템 속성을 주지 않는다 — 어떤 이름이든 시스템이 아니다
        for name in ["보통.txt", ".bashrc", "pagefile.sys"] {
            assert!(!remote(name, false, 0, None).is_system());
        }
    }

    #[test]
    fn 확장자를_끄면_이름만_보인다() {
        // 켜져 있으면 손대지 않는다
        assert_eq!(
            entry("보고서.hwp", false, 0, 0).display_name(true),
            "보고서.hwp"
        );
        // 일반 파일만 확장자가 떨어진다
        assert_eq!(
            entry("보고서.hwp", false, 0, 0).display_name(false),
            "보고서"
        );
        // 폴더는 점이 있어도 이름의 일부다
        assert_eq!(entry("v1.2", true, 0, 0).display_name(false), "v1.2");
        // 상위 이동 줄을 건드리면 목록 맨 위가 깨진다
        assert_eq!(entry("..", true, 0, 0).display_name(false), "..");
        // 확장자가 없으면 그대로
        assert_eq!(entry("README", false, 0, 0).display_name(false), "README");
        // 앞이 비면 확장자가 아니라 이름 자체다
        assert_eq!(
            entry(".gitignore", false, 0, 0).display_name(false),
            ".gitignore"
        );
        // 점만 있는 이름·끝이 점인 이름은 잘라도 얻는 게 없다
        assert_eq!(entry(".", false, 0, 0).display_name(false), ".");
        assert_eq!(entry("a.", false, 0, 0).display_name(false), "a.");
        // 확장자가 여러 겹이면 마지막 것만 뗀다
        assert_eq!(entry("a.tar.gz", false, 0, 0).display_name(false), "a.tar");
    }

    #[test]
    fn 원격_항목도_같은_규칙으로_확장자를_뗀다() {
        // 화면이 프로토콜마다 다르면 안 된다 (FR-52)
        assert_eq!(
            remote("보고서.hwp", false, 0, None).display_name(true),
            "보고서.hwp"
        );
        assert_eq!(
            remote("보고서.hwp", false, 0, None).display_name(false),
            "보고서"
        );
        assert_eq!(remote("v1.2", true, 0, None).display_name(false), "v1.2");
        assert_eq!(remote("..", true, 0, None).display_name(false), "..");
        assert_eq!(
            remote("README", false, 0, None).display_name(false),
            "README"
        );
        assert_eq!(
            remote(".bashrc", false, 0, None).display_name(false),
            ".bashrc"
        );
    }

    #[test]
    fn 확장자를_꺼도_정렬은_원래_이름으로_한다() {
        // 표시만 바뀌어야 한다 — 정렬 키가 잘린 이름을 쓰면 순서가 달라진다 (D7)
        let a = entry("파일2.txt", false, 0, 0);
        let b = entry("파일10.txt", false, 0, 0);
        assert_eq!(
            compare_entries(&a, "", &b, "", SortKey::Name),
            std::cmp::Ordering::Less
        );
        // 정렬 키는 확장자를 포함한 원본이다
        assert_eq!(
            a.name_sort_key().as_ref(),
            "파일2.txt\0"
                .encode_utf16()
                .collect::<Vec<u16>>()
                .as_slice()
        );
    }
}
