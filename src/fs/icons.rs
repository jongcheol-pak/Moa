//! 셸 아이콘·종류 문자열 조회 — 시스템 이미지 리스트 공유 + 확장자 캐시 (plan D8).
//!
//! 아이콘 인덱스는 **크기와 무관하게 같은 체계**를 쓴다 — 같은 인덱스를 16px 리스트에서 꺼내면
//! 작은 아이콘이, 256px 리스트에서 꺼내면 큰 아이콘이 나온다. 그래서 크기별 리스트만
//! 따로 들고 있으면 조회 로직은 하나로 충분하다 (FR-23·FR-24).
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Controls::{HIMAGELIST, IImageList};
use windows::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_DISPLAYNAME, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_TYPENAME,
    SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW, SHGetImageList, SHIL_EXTRALARGE, SHIL_JUMBO,
    SHIL_LARGE, SHIL_SMALL,
};
use windows::core::{HSTRING, Interface};

use crate::fs::enumerate::FileStamp;

/// 시스템 이미지 리스트의 아이콘 크기 (FR-23의 보기 모드가 고르는 단계).
///
/// 96px 단계는 시스템에 없다 — `Jumbo`(256px)를 받아 그리는 쪽에서 줄인다 (plan D8)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconSize {
    /// 16px — 자세히·목록·작은 아이콘
    Small,
    /// 32px — 내용 보기
    Large,
    /// 48px — 보통 아이콘·타일
    ExtraLarge,
    /// 256px — 아주 큰 아이콘·큰 아이콘(줄여서 씀)
    Jumbo,
}

impl IconSize {
    /// 이 단계에 맞는 시스템 이미지 리스트를 고른다.
    ///
    /// 그리려는 크기보다 **작지 않은** 가장 가까운 단계를 쓴다 — 늘리면 뭉개지고 줄이면
    /// 멀쩡하기 때문이다(plan D8)
    pub fn for_px(px: f32) -> IconSize {
        if px <= 16.0 {
            IconSize::Small
        } else if px <= 32.0 {
            IconSize::Large
        } else if px <= 48.0 {
            IconSize::ExtraLarge
        } else {
            IconSize::Jumbo
        }
    }

    /// `SHGetImageList`에 넘길 셸 상수
    fn shil(self) -> u32 {
        match self {
            IconSize::Small => SHIL_SMALL,
            IconSize::Large => SHIL_LARGE,
            IconSize::ExtraLarge => SHIL_EXTRALARGE,
            IconSize::Jumbo => SHIL_JUMBO,
        }
    }
}

/// 개별(파일별) 아이콘이 필요한 확장자 — 실행 파일·바로가기는 파일마다 아이콘이 다르다
const PER_FILE_ICON_EXTS: [&str; 3] = ["exe", "lnk", "ico"];

/// 항목 아이콘 조회 결과.
///
/// `settled`가 거짓이면 `index`는 **확장자 기본 아이콘**이고 파일 고유 아이콘은 워커가
/// 찾는 중이다 — 부르는 쪽은 이 값을 행 캐시에 담지 말고 다음 프레임에 다시 물어야 한다.
/// 담으면 그 행이 기본 아이콘에 굳는다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconLookup {
    pub index: i32,
    pub settled: bool,
}

/// 확장자 하나의 아이콘·종류 문자열 — 열거 워커가 미리 물어 UI 캐시에 넣는다(`accept_ext_info`)
#[derive(Debug, Clone)]
pub struct ExtInfo {
    pub ext: String,
    pub icon: i32,
    pub type_name: String,
}

/// 확장자 하나를 셸에 묻는다 — 디스크는 보지 않지만 **확장자마다 첫 조회가 3~60ms**다
/// (2026-09-28 실측: exe 5.5~8.8 · ico 9.7~22.3 · txt 18.5~31.0ms, 재조회 0.4~0.7ms).
/// 그래서 UI 스레드 대신 폴더를 읽는 워커가 부른다
pub fn lookup_ext(ext: &str) -> ExtInfo {
    let (icon, type_name) = {
        let _guard = shell_guard();
        lookup_by_attributes(ext)
    };
    ExtInfo {
        ext: ext.to_string(),
        icon,
        type_name,
    }
}

/// 확장자 → 시스템 이미지 리스트 인덱스/종류명 캐시.
/// 아이콘 자체를 복사하지 않고 시스템 공유 이미지 리스트 인덱스만 보관한다 (NFR-2)
pub struct IconCache {
    himl: HIMAGELIST,
    /// 크기별 시스템 이미지 리스트 — 못 얻은 단계는 담기지 않고 `himl_for`가 16px로 폴백한다
    himl_by_size: HashMap<IconSize, HIMAGELIST>,
    icon_by_ext: HashMap<String, i32>,
    type_by_ext: HashMap<String, String>,
    /// 드라이브·특수 폴더·트리 줄의 경로별 아이콘 — `icon_index_for_path`가 채운다
    icon_by_path: HashMap<String, i32>,
    /// exe·lnk·ico 파일의 고유 아이콘과 **그것을 물은 내용의 도장** — 워커가 채운다.
    ///
    /// 위 표와 나눈 이유: 이쪽은 같은 이름의 파일이 새 내용으로 바뀌면 다시 물어야 하는데
    /// (2026-09-29), 드라이브 쪽은 도장이 없다
    icon_by_file: HashMap<String, (FileStamp, i32)>,
    /// 경로별 아이콘 워커 — 처음 맡길 때 띄운다(`request_path_icon`)
    path_worker: PathWorker,
    /// 워커에 맡기고 아직 답을 거두지 않은 경로와 그 도장 — 같은 내용을 두 번 맡기지 않는다
    pending_paths: HashMap<String, FileStamp>,
    /// 경로별 셸 표시 이름 캐시 — 드라이브 이름을 매 프레임 묻지 않는다
    name_by_path: HashMap<String, String>,
    dir_icon: i32,
    dir_type: String,
    /// 실제로 셸에 물은 횟수 — **캐시가 듣는지 시험이 관측하는 유일한 값**이다.
    ///
    /// 맵 크기로는 이것을 알 수 없다(같은 키를 다시 넣어도 크기가 그대로다). 조회는 끊긴
    /// 네트워크 드라이브에서 UI를 멈출 수 있는 실경로 I/O라, "한 번만 묻는다"가 성능의 전제다 (plan D9)
    #[cfg(test)]
    shell_queries: usize,
    /// 워커에 맡긴 횟수 — 대기 중 재요청이 없는지 시험이 본다
    #[cfg(test)]
    path_requests: usize,
    /// 확장자 정보를 UI 쪽에서 직접 셸에 물은 횟수 — 열거 워커가 미리 채웠는지 시험이 본다
    #[cfg(test)]
    type_queries: usize,
}

impl Default for IconCache {
    fn default() -> IconCache {
        IconCache::new()
    }
}

impl IconCache {
    pub fn new() -> IconCache {
        // 본문이 통째로 셸 호출이라 첫 줄에서 잡는다 (`system_image_list`는 이 잠금 안에서
        // 불리므로 그쪽은 잠그지 않는다 — 잠그면 재진입 데드락)
        let _guard = shell_guard();
        // 시스템 작은 아이콘 이미지 리스트 핸들 확보 (폴더 속성 기준 1회 조회)
        let mut info = SHFILEINFOW::default();
        // 안전성: info는 스택 소유, USEFILEATTRIBUTES라 실제 디스크 접근 없음
        let himl = unsafe {
            SHGetFileInfoW(
                &HSTRING::from("folder"),
                FILE_ATTRIBUTE_DIRECTORY,
                Some(&mut info),
                size_of::<SHFILEINFOW>() as u32,
                SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES,
            )
        };
        let himl = HIMAGELIST(himl as isize);
        let mut himl_by_size = HashMap::new();
        himl_by_size.insert(IconSize::Small, himl);
        // 큰 단계는 시작할 때 한 번에 얻는다 — 4회 COM 호출이라 시작 시간(NFR-1)에 영향이 없고,
        // 지연 획득으로 두면 스크롤 중에 COM 호출이 끼어든다
        for size in [IconSize::Large, IconSize::ExtraLarge, IconSize::Jumbo] {
            if let Some(list) = system_image_list(size) {
                himl_by_size.insert(size, list);
            }
        }
        IconCache {
            himl,
            himl_by_size,
            icon_by_ext: HashMap::new(),
            type_by_ext: HashMap::new(),
            icon_by_path: HashMap::new(),
            path_worker: PathWorker::NotStarted,
            icon_by_file: HashMap::new(),
            pending_paths: HashMap::new(),
            name_by_path: HashMap::new(),
            #[cfg(test)]
            shell_queries: 0,
            #[cfg(test)]
            path_requests: 0,
            #[cfg(test)]
            type_queries: 0,
            dir_icon: info.iIcon,
            dir_type: wide_to_string(&info.szTypeName),
        }
    }

    /// ListView LVSIL_SMALL에 연결할 시스템 이미지 리스트 (16px)
    pub fn himl(&self) -> HIMAGELIST {
        self.himl
    }

    /// 요청한 크기의 시스템 이미지 리스트.
    ///
    /// 그 단계를 얻지 못했으면 **16px로 폴백한다** — 아이콘이 작게 나올지언정
    /// 목록이 그려지지 않는 것보다 낫다
    pub fn himl_for(&self, size: IconSize) -> HIMAGELIST {
        self.himl_by_size.get(&size).copied().unwrap_or(self.himl)
    }

    /// 폴더 아이콘 인덱스 — 워크스페이스 사이드바가 항목 아이콘으로 직접 그린다 (plan D14)
    pub fn dir_icon(&self) -> i32 {
        self.dir_icon
    }

    /// 항목의 아이콘 인덱스 — 보이는 행에 닿았을 때 부른다.
    ///
    /// exe·lnk·ico는 파일마다 아이콘이 달라 **실제 경로를 물어야** 하는데, 처음 보는 exe는 그
    /// 조회가 한 건에 10~93ms다(실행 파일 리소스를 읽고 백신이 검사한다 — 2026-09-28 실측).
    /// 보이는 행 전부를 한 프레임에 물으면 창이 수백 ms 멈추므로 **워커에 맡기고** 그 사이에는
    /// 확장자 아이콘을 `settled: false`로 준다. 결과는 `pump_path_icons`가 거둔다.
    ///
    /// `full_path`는 `(경로, 도장)`이다. **도장이 담긴 것과 다르면 다시 묻는다**(2026-09-29) —
    /// 같은 이름의 파일이 새 내용으로 바뀌었다. 그동안은 옛 아이콘을 잠정으로 준다(깜빡임 없음).
    /// 도장이 `None`(끌기 미리보기)이면 담긴 것을 그대로 쓰고, 없으면 확장자 아이콘으로 확정한다
    pub fn icon_index(
        &mut self,
        ext: &str,
        is_dir: bool,
        full_path: Option<(&str, Option<FileStamp>)>,
    ) -> IconLookup {
        if is_dir {
            return IconLookup {
                index: self.dir_icon,
                settled: true,
            };
        }
        if PER_FILE_ICON_EXTS.contains(&ext)
            && let Some((path, stamp)) = full_path
        {
            let held = self.icon_by_file.get(path).copied();
            let Some(stamp) = stamp else {
                // 도장 없는 조회 — 워커에 맡기지 않는다(D7)
                let index = held.map_or_else(|| self.ext_icon(ext), |(_, index)| index);
                return IconLookup {
                    index,
                    settled: true,
                };
            };
            if let Some((held_stamp, index)) = held
                && held_stamp == stamp
            {
                return IconLookup {
                    index,
                    settled: true,
                };
            }
            // 처음 보거나 내용이 바뀌었다 — 옛 아이콘이 있으면 그것을, 없으면 확장자 아이콘을 잠정으로
            let index = held.map_or_else(|| self.ext_icon(ext), |(_, index)| index);
            // 워커를 쓸 수 없으면 잠정 아이콘으로 확정한다 — 행이 매 프레임 다시 묻지 않게
            let settled = !self.request_path_icon(path, stamp);
            return IconLookup { index, settled };
        }
        IconLookup {
            index: self.ext_icon(ext),
            settled: true,
        }
    }

    /// 열거 워커가 미리 물어 둔 확장자 정보를 담는다 — 이미 아는 확장자는 건드리지 않는다
    pub fn accept_ext_info(&mut self, infos: Vec<ExtInfo>) {
        for info in infos {
            if self.icon_by_ext.contains_key(&info.ext) {
                continue;
            }
            self.icon_by_ext.insert(info.ext.clone(), info.icon);
            self.type_by_ext.insert(info.ext, info.type_name);
        }
    }

    /// 확장자 정보를 UI 쪽에서 직접 셸에 물은 횟수
    #[cfg(test)]
    pub fn type_queries(&self) -> usize {
        self.type_queries
    }

    /// 확장자만으로 정해지는 아이콘 — 대개 열거 워커가 미리 채워 둔 캐시에 맞는다(`accept_ext_info`).
    /// 못 맞으면 여기서 묻는다(확장자마다 첫 조회 3~60ms)
    fn ext_icon(&mut self, ext: &str) -> i32 {
        if let Some(&idx) = self.icon_by_ext.get(ext) {
            return idx;
        }
        #[cfg(test)]
        {
            self.type_queries += 1;
        }
        let (idx, type_name) = {
            let _guard = shell_guard();
            lookup_by_attributes(ext)
        };
        self.icon_by_ext.insert(ext.to_string(), idx);
        self.type_by_ext.insert(ext.to_string(), type_name);
        idx
    }

    /// 경로를 워커에 맡긴다 — 맡겼거나 이미 대기 중이면 참, 워커를 쓸 수 없으면 거짓.
    ///
    /// 워커는 **처음 맡길 때 띄운다** — `IconCache`는 시험·드라이브 워커 등 수십 곳에서
    /// 만들어지는데, 경로별 조회를 하는 것은 앱의 캐시 하나뿐이다
    fn request_path_icon(&mut self, path: &str, stamp: FileStamp) -> bool {
        if self.pending_paths.get(path) == Some(&stamp) {
            return true;
        }
        if matches!(self.path_worker, PathWorker::NotStarted) {
            self.path_worker =
                spawn_path_worker().map_or(PathWorker::Unavailable, PathWorker::Running);
        }
        let PathWorker::Running(channels) = &self.path_worker else {
            return false;
        };
        if channels.requests.send((path.to_string(), stamp)).is_err() {
            self.path_worker = PathWorker::Unavailable;
            return false;
        }
        #[cfg(test)]
        {
            self.path_requests += 1;
        }
        // 옛 도장으로 기다리던 것은 새 도장으로 바꿔 단다 — 옛 결과는 `pump_path_icons`가 버린다
        self.pending_paths.insert(path.to_string(), stamp);
        true
    }

    /// 워커가 돌려준 경로별 아이콘을 거둔다 — 새로 온 것이 있으면 참.
    ///
    /// 워커가 사라졌으면(채널 끊김) 대기를 비운다 — 남겨 두면 앱이 영영 다시 그리기를
    /// 청하고, 비워 두면 행이 다시 물어 확장자 아이콘으로 확정된다(`request_path_icon`)
    pub fn pump_path_icons(&mut self) -> bool {
        let PathWorker::Running(channels) = &self.path_worker else {
            return false;
        };
        let mut arrived = false;
        let mut lost = false;
        loop {
            match channels.results.try_recv() {
                Ok((path, stamp, index)) => {
                    // **파일이 또 바뀌어 새 도장으로 기다리는 중이면 옛 결과는 버린다**
                    if self
                        .pending_paths
                        .get(&path)
                        .is_some_and(|waiting| *waiting != stamp)
                    {
                        continue;
                    }
                    self.pending_paths.remove(&path);
                    self.icon_by_file.insert(path, (stamp, index));
                    arrived = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    lost = true;
                    break;
                }
            }
        }
        if lost {
            self.path_worker = PathWorker::Unavailable;
            self.pending_paths.clear();
        }
        arrived
    }

    /// 워커가 찾는 중인 경로가 남았는가 — 앱이 다시 그릴지 정하는 데 쓴다
    pub fn has_pending_paths(&self) -> bool {
        !self.pending_paths.is_empty()
    }

    /// 워커에 맡긴 경로 수 — 대기 중 같은 경로를 다시 보내지 않는지 시험이 본다
    #[cfg(test)]
    pub fn path_requests(&self) -> usize {
        self.path_requests
    }

    /// **경로로 직접 물어 얻는** 아이콘 인덱스 — 드라이브·특수 폴더가 각자의 아이콘을 갖는다.
    ///
    /// `icon_index`를 쓰지 않는 이유는 그 함수가 `is_dir`을 먼저 걸러 **폴더든 드라이브든 같은
    /// 일반 폴더 아이콘**을 주기 때문이다. 탐색기처럼 보이려면 셸에 그 경로를 그대로 물어야 한다.
    ///
    /// `SHGFI_USEFILEATTRIBUTES`를 **주지 않는다** — 그 플래그는 "디스크를 보지 말고 속성만으로
    /// 판단하라"는 뜻이라 드라이브 종류가 사라진다. 대신 실제 조회라 느릴 수 있어 경로별로 캐시한다
    pub fn icon_index_for_path(&mut self, path: &str) -> i32 {
        if let Some(&idx) = self.icon_by_path.get(path) {
            return idx;
        }
        let found = lookup_by_path(path);
        #[cfg(test)]
        {
            self.shell_queries += 1;
        }
        // 조회가 실패하면 일반 폴더 아이콘으로 떨어진다 — 빈 자리를 남기지 않는다
        let idx = found.unwrap_or(self.dir_icon);
        self.icon_by_path.insert(path.to_string(), idx);
        idx
    }

    /// 셸이 보이는 이름 — 드라이브면 `로컬 디스크 (C:)`처럼 지역화된 문자열이다.
    ///
    /// 볼륨 레이블을 읽어 우리가 조립하지 않는 이유는 `로컬 디스크`·`(C:)` 같은 표기가
    /// 언어마다 다르기 때문이다. 얻지 못하면 `None`이고 부르는 쪽이 경로로 폴백한다.
    ///
    /// 셸 조회를 트레이트로 감싸지 않는다 — 이 메서드와 `icon_index_for_path`를 부르는 곳이
    /// **여럿이지만 모두 얇은 래퍼일 뿐**이라(`fs::drives`·`fs::known_folders`·`ui::tree`),
    /// 감싸도 갈아 끼울 구현이 생기지 않고 `unsafe` 격리 지점만 늘어난다
    /// (2026-08-17: 드라이브 줄 조회가 `fs::drives`로 옮겨오며 호출처가 둘 이상이 됐다 —
    /// 종전 주석은 "부르는 곳이 하나씩뿐"이라는 전제를 들었는데 그 전제만 바뀌고 판단은 같다)
    pub fn shell_display_name(&mut self, path: &str) -> Option<String> {
        if let Some(name) = self.name_by_path.get(path) {
            return Some(name.clone());
        }
        let mut info = SHFILEINFOW::default();
        let ok = {
            let _guard = shell_guard();
            // 안전성: 실제 경로 조회 — 실패하면 0을 돌려주고 `szDisplayName`은 비어 있다
            unsafe {
                SHGetFileInfoW(
                    &HSTRING::from(path),
                    Default::default(),
                    Some(&mut info),
                    size_of::<SHFILEINFOW>() as u32,
                    SHGFI_DISPLAYNAME,
                )
            }
        };
        #[cfg(test)]
        {
            self.shell_queries += 1;
        }
        if ok == 0 {
            return None;
        }
        // 같은 파일의 기존 변환을 그대로 쓴다 — 널 앞까지만 디코드한다
        let name = wide_to_string(&info.szDisplayName);
        if name.is_empty() {
            return None;
        }
        self.name_by_path.insert(path.to_string(), name.clone());
        Some(name)
    }

    /// 셸에 실제로 물은 횟수 — 캐시가 듣는지 시험이 본다 (plan D9)
    #[cfg(test)]
    pub fn shell_queries(&self) -> usize {
        self.shell_queries
    }

    /// 항목의 종류(형식) 문자열 — 셸이 주는 지역화 문자열 그대로
    pub fn type_name(&mut self, ext: &str, is_dir: bool) -> String {
        if is_dir {
            return self.dir_type.clone();
        }
        if let Some(t) = self.type_by_ext.get(ext) {
            return t.clone();
        }
        #[cfg(test)]
        {
            self.type_queries += 1;
        }
        let (idx, type_name) = {
            let _guard = shell_guard();
            lookup_by_attributes(ext)
        };
        self.icon_by_ext.insert(ext.to_string(), idx);
        self.type_by_ext.insert(ext.to_string(), type_name.clone());
        type_name
    }
}

/// 크기별 시스템 이미지 리스트를 얻는다. 실패하면 `None`(호출부가 16px로 폴백).
///
/// `SHGetImageList`는 `IImageList` COM 인터페이스를 주는데, 이미지 리스트 API가 쓰는
/// `HIMAGELIST`는 그 인터페이스 포인터와 같은 값이다(셸의 오래된 계약).
/// 인터페이스를 그대로 두면 drop 시 Release되어 핸들이 죽으므로 **참조를 의도적으로 넘긴다**
fn system_image_list(size: IconSize) -> Option<HIMAGELIST> {
    // 안전성: 프로세스 수명 동안 유지되는 시스템 공유 리스트를 받는다.
    // `into_raw`로 소유권을 넘겨 Release되지 않게 하며, 해제하지 않는 것이 이 API의 관례다
    // (시스템이 소유하고 앱은 빌려 쓴다 — `SHGetFileInfoW`가 주는 핸들과 같은 성질)
    unsafe {
        let list: IImageList = SHGetImageList(size.shil() as i32).ok()?;
        Some(HIMAGELIST(list.into_raw() as isize))
    }
}

/// 실제 경로로 아이콘 인덱스를 묻는다 — 실패하면 `None`.
///
/// 디스크를 읽는 조회라 느릴 수 있다(처음 보는 exe 10~93ms, 끊긴 네트워크 경로는 초 단위).
/// **셸 잠금을 여기서 잡는다** — 부르는 쪽은 `icon_index_for_path`(UI 스레드의 드라이브·트리 줄)와
/// 경로별 아이콘 워커 둘이고, 둘 다 잠금을 쥐지 않은 채 부른다
fn lookup_by_path(path: &str) -> Option<i32> {
    let mut info = SHFILEINFOW::default();
    let _guard = shell_guard();
    // 안전성: 스택의 `info`에 쓰는 읽기 전용 조회 — 실패는 반환값 0으로 오고 그때는 `info`를
    // 읽지 않는다
    let ok = unsafe {
        SHGetFileInfoW(
            &HSTRING::from(path),
            Default::default(),
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_SMALLICON,
        )
    };
    (ok != 0).then_some(info.iIcon)
}

/// 경로별 아이콘 워커와 잇는 통로
struct PathWorkerChannels {
    requests: Sender<(String, FileStamp)>,
    results: Receiver<(String, FileStamp, i32)>,
}

/// 경로별 아이콘 워커의 상태 — 처음 맡길 때 띄우고, 띄우지 못했거나 사라지면 다시 띄우지 않는다
enum PathWorker {
    NotStarted,
    Running(PathWorkerChannels),
    Unavailable,
}

/// 경로별 아이콘 워커를 띄운다 — 스레드를 만들지 못하면 `None`(부르는 쪽이 확장자 아이콘으로 확정한다).
///
/// 워커는 요청 통로가 닫히면(캐시가 버려지면) 끝난다. 셸 조회가 COM을 쓰므로 STA로 초기화한다
/// (`fs::drives`·`fs::thumbnail` 워커와 같다). 실패한 조회는 `0`(셸 기본 아이콘)을 돌려준다 —
/// 동기 조회 시절 `SHFILEINFOW::default()`의 `iIcon`이 그대로 쓰이던 것과 같은 값이다
fn spawn_path_worker() -> Option<PathWorkerChannels> {
    let (request_tx, request_rx) = channel::<(String, FileStamp)>();
    let (result_tx, result_rx) = channel();
    std::thread::Builder::new()
        .name("path-icons".into())
        .spawn(move || {
            // 안전성: 이 스레드에서 열고 루프를 벗어난 뒤 짝을 맞춰 닫는다
            let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
            for (path, stamp) in request_rx {
                let index = lookup_by_path(&path).unwrap_or(0);
                if result_tx.send((path, stamp, index)).is_err() {
                    break;
                }
            }
            if com {
                // 안전성: 위에서 성공한 초기화와 짝을 맞춘다
                unsafe { CoUninitialize() };
            }
        })
        .ok()?;
    Some(PathWorkerChannels {
        requests: request_tx,
        results: result_rx,
    })
}

/// 확장자만으로 아이콘·종류 조회 (디스크 접근 없음 — 대량 폴더에서도 빠름)
fn lookup_by_attributes(ext: &str) -> (i32, String) {
    let dummy = if ext.is_empty() {
        "file".to_string()
    } else {
        format!("file.{ext}")
    };
    let mut info = SHFILEINFOW::default();
    // 안전성: USEFILEATTRIBUTES — 가상 파일명으로 속성 기반 조회만 수행
    unsafe {
        SHGetFileInfoW(
            &HSTRING::from(dummy.as_str()),
            FILE_ATTRIBUTE_NORMAL,
            Some(&mut info),
            size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES,
        );
    }
    (info.iIcon, wide_to_string(&info.szTypeName))
}

fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// 셸을 만지는 **함수들**의 직렬화 잠금.
///
/// `SHGetFileInfoW`·`SHGetKnownFolderPath`·`SHGetImageList`·`ImageList_GetIcon`은 프로세스
/// 전역 셸 상태를 함께 쓰는데, Rust 시험은 기본이 병렬이라 서로 다른 스레드에서 동시에
/// 부르면 `SHGetImageList`가 실패해 **16px로 폴백**한다(그러면 크기별 리스트 시험이 깨진다).
///
/// **잠금은 시험이 아니라 자원을 만지는 함수가 잡는다** — 호출부가 잡으면 계층마다 재진입
/// 위험이 생기고, `std::sync::Mutex`는 재진입 불가라 그 자리에서 **타임아웃 없이 멎는다**.
///
/// **잡는 곳(8)**: `IconCache::new` · `ext_icon` · `lookup_by_path` · `lookup_ext` ·
/// `shell_display_name` · `type_name`(이 파일) · `known_folders::known_folder` ·
/// `ui::icon_tex::icon_to_image`. 잡는 자리는 **셸 호출 직전**이다 — 캐시 히트 앞에 두면
/// 렌더 경로가 프레임마다 전역 잠금을 잡아 시험 스위트가 10분을 넘긴다(실측).
/// `lookup_by_path`는 경로별 아이콘 **워커 스레드**에서, `lookup_ext`는 폴더 **열거 워커**에서 불린다 — 시험 빌드에서는 그
/// 조회(처음 보는 exe 수십 ms) 동안 다른 시험의 셸 호출이 기다린다.
///
/// **잡지 않는 곳(6)**: `system_image_list`·`lookup_by_attributes`(위 함수들 안에서만 불리는
/// private) · `icon_index`·`icon_index_for_path`(`ext_icon`·`lookup_by_path`를 부르는 조합
/// 메서드) · `fs::drives::list_drives`·`fs::known_folders::default_favorites`(안에서 잠금
/// 함수를 부르는 조합 함수). **이 여섯을 잠그면 재진입 데드락이다.**
#[cfg(test)]
static SHELL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 셸 잠금을 쥔 표시 — 시험 빌드에서만 실제로 잠근다
#[cfg(test)]
pub(crate) struct ShellGuard {
    _inner: std::sync::MutexGuard<'static, ()>,
}

/// 실행 파일에서는 빈 구조체다 — 위 `SHELL_LOCK`이 적은 대로 이 잠금은 **시험이 병렬로 도는
/// 동안** 셸을 동시에 물어 `SHGetImageList`가 16px로 폴백하는 것을 막으려고 둔 것이고, 실행
/// 파일에는 그렇게 몰아치는 동시 호출이 없다
#[cfg(not(test))]
pub(crate) struct ShellGuard;

/// 셸 호출 직전에 잡는다. 앞선 시험이 패닉해 독이 올랐어도 이어서 쓴다
/// (그 시험의 실패만으로 충분하고, 여기서 또 패닉하면 원인이 가려진다)
#[cfg(test)]
pub(crate) fn shell_guard() -> ShellGuard {
    ShellGuard {
        _inner: SHELL_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    }
}

#[cfg(not(test))]
pub(crate) fn shell_guard() -> ShellGuard {
    ShellGuard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 요청한_크기보다_작지_않은_단계를_고른다() {
        // 늘린 아이콘은 뭉개지고 줄인 아이콘은 멀쩡하다 — 항상 같거나 큰 단계를 써야 한다
        assert_eq!(IconSize::for_px(16.0), IconSize::Small);
        assert_eq!(IconSize::for_px(32.0), IconSize::Large);
        assert_eq!(IconSize::for_px(48.0), IconSize::ExtraLarge);
        assert_eq!(
            IconSize::for_px(96.0),
            IconSize::Jumbo,
            "96px는 256px를 줄여 쓴다"
        );
        assert_eq!(IconSize::for_px(256.0), IconSize::Jumbo);
    }

    #[test]
    fn 경계_바로_위는_다음_단계로_넘어간다() {
        assert_eq!(IconSize::for_px(17.0), IconSize::Large);
        assert_eq!(IconSize::for_px(33.0), IconSize::ExtraLarge);
        assert_eq!(IconSize::for_px(49.0), IconSize::Jumbo);
    }

    #[test]
    fn 크기별로_서로_다른_이미지_리스트를_얻는다() {
        // 이 획득이 실패하면 모든 단계가 16px로 폴백해 보기 모드를 바꿔도 아이콘이 그대로다.
        // 화면 표시는 T10(격자 렌더)에서 확인하고, 여기서는 **리스트를 실제로 얻었는지**만 본다
        let cache = IconCache::new();
        assert_eq!(
            cache.himl_for(IconSize::Small),
            cache.himl(),
            "16px 단계가 기존 핸들과 달라졌다 — 자세히 보기가 회귀한다"
        );
        let jumbo = cache.himl_for(IconSize::Jumbo);
        assert!(!jumbo.is_invalid(), "256px 리스트가 유효하지 않다");
        assert_ne!(
            jumbo,
            cache.himl(),
            "256px 요청이 16px로 폴백했다 — SHGetImageList가 실패했거나 상수가 틀렸다"
        );
        // 네 단계가 모두 서로 다른 리스트여야 크기 구분이 성립한다
        let mut handles: Vec<isize> = [
            IconSize::Small,
            IconSize::Large,
            IconSize::ExtraLarge,
            IconSize::Jumbo,
        ]
        .iter()
        .map(|size| cache.himl_for(*size).0)
        .collect();
        handles.sort_unstable();
        let before = handles.len();
        handles.dedup();
        assert_eq!(handles.len(), before, "같은 리스트를 가리키는 단계가 있다");
    }

    #[test]
    fn 셸_상수는_서로_겹치지_않는다() {
        // 겹치면 두 단계가 같은 리스트를 가리켜 크기 구분이 사라진다
        let mut shils: Vec<u32> = [
            IconSize::Small,
            IconSize::Large,
            IconSize::ExtraLarge,
            IconSize::Jumbo,
        ]
        .iter()
        .map(|size| size.shil())
        .collect();
        shils.sort_unstable();
        let before = shils.len();
        shils.dedup();
        assert_eq!(shils.len(), before);
    }

    #[test]
    fn 드라이브는_일반_폴더와_다른_아이콘을_받는다() {
        // 요구의 핵심 — 탐색기처럼 드라이브가 제 아이콘을 갖는다.
        // `icon_index`는 `is_dir`을 먼저 걸러 폴더 아이콘 하나만 주므로 그것으로는 안 된다
        let mut icons = IconCache::new();
        let drive = icons.icon_index_for_path("C:\\");
        let folder = icons.dir_icon();

        assert_ne!(
            drive, folder,
            "드라이브가 일반 폴더와 같은 아이콘을 받았다 — 경로 실조회가 듣지 않는다"
        );
    }

    #[test]
    fn 같은_경로는_한_번만_셸에_묻는다() {
        // 실경로 조회는 끊긴 네트워크 드라이브에서 UI를 멈출 수 있어 캐시가 성능의 전제다.
        // **맵 크기로는 이것을 볼 수 없다** — 다시 물어 다시 넣어도 크기가 그대로다 (plan D9)
        let mut icons = IconCache::new();
        let before = icons.shell_queries();

        let first = icons.icon_index_for_path("C:\\");
        let after_first = icons.shell_queries();
        let second = icons.icon_index_for_path("C:\\");

        assert_eq!(first, second, "같은 경로인데 다른 아이콘이 나왔다");
        assert_eq!(after_first, before + 1, "첫 조회가 셸을 부르지 않았다");
        assert_eq!(
            icons.shell_queries(),
            after_first,
            "두 번째 요청이 셸을 다시 물었다 — 캐시가 듣지 않는다"
        );
    }

    /// 도장을 가리지 않는 시험이 쓰는 한 가지 도장
    const S: FileStamp = FileStamp {
        size: 0,
        modified: 0,
    };

    /// 워커 결과가 거둬질 때까지 묻는다(최대 10초) — 마지막 조회 결과를 준다
    fn settle(icons: &mut IconCache, path: &str, stamp: FileStamp) -> IconLookup {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut lookup = icons.icon_index("exe", false, Some((path, Some(stamp))));
        while !lookup.settled && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            icons.pump_path_icons();
            lookup = icons.icon_index("exe", false, Some((path, Some(stamp))));
        }
        lookup
    }

    #[test]
    fn 도장이_바뀌면_옛_아이콘을_잠정으로_두고_다시_묻는다() {
        let path = exe_path();
        let mut icons = IconCache::new();
        let first = settle(&mut icons, &path, S);
        assert!(first.settled);
        let before = icons.path_requests();

        let changed = FileStamp {
            size: 1,
            modified: 1,
        };
        let lookup = icons.icon_index("exe", false, Some((&path, Some(changed))));
        assert!(!lookup.settled, "내용이 바뀌었는데 옛 아이콘으로 확정했다");
        assert_eq!(
            lookup.index, first.index,
            "잠정 아이콘이 옛 아이콘이 아니다"
        );
        assert_eq!(icons.path_requests(), before + 1, "다시 묻지 않았다");
    }

    #[test]
    fn 도장_없는_조회는_담긴_것을_쓰고_없으면_확장자로_확정한다() {
        let path = exe_path();
        let mut icons = IconCache::new();
        let ext_only = icons.icon_index("exe", false, None);
        let blank = icons.icon_index("exe", false, Some((&path, None)));
        assert_eq!(blank, ext_only);
        assert_eq!(icons.path_requests(), 0, "도장 없는 조회가 워커에 맡겼다");

        let settled = settle(&mut icons, &path, S);
        let held = icons.icon_index("exe", false, Some((&path, None)));
        assert_eq!(held, settled, "담긴 아이콘을 쓰지 않았다");
    }

    #[test]
    fn 영바이트_exe를_덮어쓰면_새_아이콘을_받는다() {
        // 2026-09-29 실측과 같은 형태 — 0바이트 exe는 기본 아이콘(2), 실제 exe로 덮은 뒤
        // 다시 물으면 그 exe의 아이콘이 온다. 경로만 키로 삼으면 기본 아이콘에 굳는다
        let dir = std::env::temp_dir().join(format!("fe_icon_stamp_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("임시 폴더");
        let file = dir.join("덮을것.exe");
        std::fs::write(&file, b"").expect("0바이트 파일");
        let path = file.to_string_lossy().into_owned();

        let mut icons = IconCache::new();
        let empty = settle(&mut icons, &path, S);
        // 고유 아이콘이 있는 exe로 덮는다 — 시험 하네스 exe는 아이콘 리소스가 없어 0바이트와
        // 같은 기본 아이콘을 받으므로 이 판정에 쓸 수 없다
        std::fs::copy(r"C:\Windows\regedit.exe", &file).expect("덮어쓰기");
        let changed = FileStamp {
            size: 1,
            modified: 1,
        };
        let real = settle(&mut icons, &path, changed);
        let expected = IconCache::new().icon_index_for_path(&path);
        let _ = std::fs::remove_dir_all(&dir);

        assert!(empty.settled && real.settled);
        assert_eq!(real.index, expected, "덮어쓴 exe의 아이콘이 아니다");
        assert_ne!(real.index, empty.index, "0바이트 때의 아이콘에 굳었다");
    }

    /// 경로별 조회 대상인 실제 파일 — 시험 하네스 exe는 어느 PC에서나 있다
    fn exe_path() -> String {
        std::env::current_exe()
            .expect("시험 실행 파일 경로")
            .to_string_lossy()
            .into_owned()
    }

    #[test]
    fn 첫_경로별_조회는_확장자_아이콘을_잠정으로_준다() {
        // 처음 보는 exe의 셸 조회는 10~93ms라(실측) UI 스레드에서 기다리면 창이 멈춘다 —
        // 그 자리에서는 확장자 아이콘을 주고 「아직 확정 아님」을 알려야 한다
        let path = exe_path();
        let mut icons = IconCache::new();
        let ext_only = icons.icon_index("exe", false, None);

        let first = icons.icon_index("exe", false, Some((&path, Some(S))));

        assert!(
            !first.settled,
            "첫 조회가 곧바로 확정됐다 — 셸을 동기로 물었다"
        );
        assert_eq!(
            first.index, ext_only.index,
            "잠정 아이콘이 확장자 아이콘이 아니다"
        );
    }

    #[test]
    fn 거둔_경로별_아이콘은_확정되고_경로_조회와_같다() {
        let path = exe_path();
        // 기대값은 **다른 인스턴스**에서 얻는다 — 같은 인스턴스로 물으면 경로 캐시를 미리
        // 채워 아래 조회가 워커를 거치지 않는다
        let expected = IconCache::new().icon_index_for_path(&path);
        let mut icons = IconCache::new();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut lookup = icons.icon_index("exe", false, Some((&path, Some(S))));
        while !lookup.settled && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            icons.pump_path_icons();
            lookup = icons.icon_index("exe", false, Some((&path, Some(S))));
        }

        assert!(
            lookup.settled,
            "5초 안에 확정되지 않았다 — 워커 결과를 거두지 못한다"
        );
        assert_eq!(
            lookup.index, expected,
            "워커가 준 아이콘이 경로 조회와 다르다"
        );
        assert!(!icons.has_pending_paths(), "거둔 뒤에도 대기가 남았다");
    }

    #[test]
    fn 대기_중인_경로는_다시_맡기지_않는다() {
        // 보이는 행은 확정될 때까지 매 프레임 다시 묻는다 — 그때마다 워커에 보내면 큐가 쌓인다
        let path = exe_path();
        let mut icons = IconCache::new();

        icons.icon_index("exe", false, Some((&path, Some(S))));
        icons.icon_index("exe", false, Some((&path, Some(S))));

        assert!(
            icons.has_pending_paths(),
            "맡긴 경로가 대기로 잡히지 않았다"
        );
        assert_eq!(icons.path_requests(), 1, "같은 경로를 두 번 맡겼다");
    }

    #[test]
    fn 폴더와_일반_확장자와_경로_없는_exe는_곧바로_확정된다() {
        let mut icons = IconCache::new();

        assert!(
            icons
                .icon_index("", true, Some((r"C:\Windows", Some(S))))
                .settled,
            "폴더"
        );
        assert!(
            icons
                .icon_index("txt", false, Some((r"C:\a.txt", Some(S))))
                .settled,
            "txt"
        );
        assert!(
            icons.icon_index("exe", false, None).settled,
            "경로 없는 exe(원격)"
        );
        assert_eq!(icons.path_requests(), 0, "워커를 거칠 이유가 없는데 맡겼다");
    }

    #[test]
    fn 미리_받은_확장자는_셸을_묻지_않고_그_종류와_아이콘을_준다() {
        // 열거 워커가 물어 둔 값이 UI 캐시에 들어가면 목록 반영이 셸을 기다리지 않는다.
        // 가짜 확장자라 셸에 물으면 「ZZQTEST 파일」 같은 일반 종류가 온다
        let mut icons = IconCache::new();
        icons.accept_ext_info(vec![ExtInfo {
            ext: "zzqtest".into(),
            icon: 7,
            type_name: "미리 받은 종류".into(),
        }]);

        assert_eq!(icons.type_name("zzqtest", false), "미리 받은 종류");
        assert_eq!(icons.icon_index("zzqtest", false, None).index, 7);
        assert_eq!(icons.type_queries(), 0, "받아 둔 확장자를 셸에 다시 물었다");
    }

    #[test]
    fn 드라이브의_셸_표시_이름을_얻는다() {
        // `로컬 디스크 (C:)`처럼 화면 언어를 따르는 문자열이라 값 자체는 단언하지 않는다
        let mut icons = IconCache::new();
        let name = icons
            .shell_display_name("C:\\")
            .expect("C 드라이브 표시 이름");

        assert!(!name.is_empty(), "이름이 비었다");
        assert_ne!(name, "C:\\", "경로가 그대로 왔다 — 표시 이름이 아니다");
        // 널 문자가 남으면 화면에 두부(`?`)로 그려진다
        assert!(!name.contains('\0'), "널 문자가 남았다: {name:?}");
    }

    #[test]
    fn 없는_경로의_표시_이름은_없음이거나_패닉하지_않는다() {
        let mut icons = IconCache::new();
        // 실재하지 않는 드라이브 — 셸이 무엇을 주든 앱이 죽지 않아야 한다.
        // 돌려주는 값도 계약대로여야 한다: 없거나(None), 있다면 경로에서 온 이름이다
        let path = r"QQ:\없는폴더\깊이";
        if let Some(name) = icons.shell_display_name(path) {
            assert!(!name.is_empty(), "빈 이름을 돌려줬다");
            assert!(
                path.contains(name.as_str()),
                "실재하지 않는 경로인데 경로와 무관한 이름이 왔다: {name:?}"
            );
        }
    }
}
