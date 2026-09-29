//! 디렉터리 열거 — 워커 스레드에서 수행, UI 스레드는 절대 블로킹하지 않는다 (plan D5)
use std::path::Path;
use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_BAD_NET_NAME, ERROR_BAD_NETPATH, ERROR_DEV_NOT_EXIST,
    ERROR_FILE_NOT_FOUND, ERROR_NETNAME_DELETED, ERROR_NETWORK_UNREACHABLE, ERROR_NO_MORE_FILES,
    ERROR_NO_NET_OR_BAD_PATH, ERROR_PATH_NOT_FOUND, ERROR_REM_NOT_LIST, ERROR_UNEXP_NET_ERR,
    WIN32_ERROR,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic,
    FindExSearchNameMatch, FindFirstFileExW, FindNextFileW, WIN32_FIND_DATAW,
};
use windows::core::HSTRING;

/// 목록 항목 — 이름은 UTF-16(널 종단) 원본 유지 (정렬 API·표시 공용, 변환 손실 없음)
#[derive(Clone, Debug)]
pub struct FileEntry {
    /// 널 종단 포함 UTF-16 이름
    pub name: Vec<u16>,
    pub is_dir: bool,
    pub size: u64,
    /// FILETIME 원시값 (100ns 단위) — 정렬·표시 시 변환
    pub modified: u64,
    /// `WIN32_FIND_DATAW`의 원시 속성값 — 숨김·시스템 판정에 쓴다 (FR-13).
    ///
    /// 판정 결과(`bool`)가 아니라 원시값을 든다: 무엇을 숨길지는 화면 쪽 규칙이고,
    /// 열거는 OS가 준 사실만 실어 나른다
    pub attributes: u32,
}

/// 파일 내용이 바뀌었는지 가르는 도장 — 크기와 수정 시각 (2026-09-29).
///
/// 썸네일·경로별 아이콘 캐시가 경로만 보면, 같은 이름의 파일이 새 내용으로 바뀌어도(덮어쓰기·
/// 복사 중 0바이트였다가 채워짐) 옛 그림을 계속 준다. 둘 중 하나라도 다르면 다시 만든다.
/// 감시(`fs::watcher`)가 크기·쓰기 시각 변화를 통지하므로 새 도장은 다시 읽은 목록에 실린다
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct FileStamp {
    pub size: u64,
    /// FILETIME 원시값 (100ns 단위)
    pub modified: u64,
}

impl FileEntry {
    /// 이 항목의 도장 — 캐시가 내용 변화를 가르는 데 쓴다
    pub fn stamp(&self) -> FileStamp {
        FileStamp {
            size: self.size,
            modified: self.modified,
        }
    }

    /// 표시용 문자열 (널 종단 제외)
    pub fn name_string(&self) -> String {
        String::from_utf16_lossy(&self.name[..self.name.len().saturating_sub(1)])
    }

    /// 소문자 확장자 ("" = 없음/폴더)
    pub fn extension(&self) -> String {
        if self.is_dir {
            return String::new();
        }
        let s = self.name_string();
        match s.rsplit_once('.') {
            Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
            _ => String::new(),
        }
    }
}

/// 열거가 흘려보내는 한 조각 (FR-69).
///
/// **`Done(Ok(..))`의 페이로드는 「잔여분」이지 전량이 아니다** — 앞서 `Partial`로 보낸 것을
/// 워커가 다시 들고 있으면 10만 항목에서 메모리가 두 배가 된다. 받는 쪽은 `Partial`이
/// 한 번이라도 있었으면 누적하고, 없었으면 종전대로 전량 교체한다(`ui::panel`).
///
/// `Done(Err)` 계열(`AccessDenied`·`NotFound`·`Error`)에는 페이로드가 없다
#[derive(Debug)]
pub enum EnumChunk {
    /// 아직 다 읽지 않았다 — 지금까지 모은 몫
    Partial(Vec<FileEntry>),
    /// 다 읽었거나 실패했다. `Ok`의 항목은 **마지막 `Partial` 이후의 잔여분**이다
    Done(EnumOutcome),
}

/// 열거 결과
#[derive(Debug)]
pub enum EnumOutcome {
    Ok(Vec<FileEntry>),
    /// 접근 권한 없음
    AccessDenied,
    /// 경로 없음/삭제됨
    NotFound,
    /// 기타 오류.
    ///
    /// `network`는 **끊긴 네트워크 드라이브·서버처럼 네트워크가 원인인 실패**를 뜻한다 —
    /// 화면이 사유를 갈라 적는 데 쓴다(끊긴 연결을 살피는 것과 다시 열어 보는 것은
    /// 사용자가 할 일이 다르다). 이 깃발을 담는 자리가 열거 워커인 이유는 **OS 오류 코드를
    /// 아는 곳이 여기뿐**이라서다
    Error {
        network: bool,
    },
}

/// 이 오류 코드가 **네트워크가 원인인 실패**인가.
///
/// 끊긴 네트워크 드라이브(`Z:`)를 열면 실측으로 `ERROR_BAD_NETPATH`(53)가 온다.
/// 나머지는 같은 계열에서 알려진 코드들이며, **목록에 없는 코드는 일반 실패로 떨어진다**
/// (문구만 덜 구체적이 될 뿐 목록 표시·트리 배지는 영향받지 않는다)
fn is_network_error(code: WIN32_ERROR) -> bool {
    matches!(
        code,
        ERROR_REM_NOT_LIST            // 51 — 원격 목록을 얻지 못함
            | ERROR_BAD_NETPATH       // 53 — 네트워크 경로를 찾지 못함 (끊긴 드라이브의 실측값)
            | ERROR_DEV_NOT_EXIST     // 55 — 그 이름의 공유가 없음
            | ERROR_UNEXP_NET_ERR     // 59 — 네트워크에서 예상 밖의 오류
            | ERROR_NETNAME_DELETED   // 64 — 네트워크 이름이 사라짐
            | ERROR_BAD_NET_NAME      // 67 — 네트워크 이름을 찾지 못함
            | ERROR_NO_NET_OR_BAD_PATH // 1203 — 네트워크가 없거나 경로가 틀림
            | ERROR_NETWORK_UNREACHABLE // 1231 — 네트워크에 닿을 수 없음
    )
}

/// 디렉터리 1단계 열거 (동기 — 워커 스레드에서 호출).
/// 긴 경로(260+) 지원을 위해 `\\?\` 접두를 사용한다 (NFR-5)
pub fn enumerate_dir(path: &Path) -> EnumOutcome {
    // 상한을 최대로 주면 `Partial`이 한 번도 나가지 않아 종전과 같은 「한 번에 전부」가 된다
    let mut done = None;
    enumerate_dir_batched(path, usize::MAX, |chunk| {
        if let EnumChunk::Done(outcome) = chunk {
            done = Some(outcome);
        }
        true
    });
    // `Done`은 어느 갈래로 끝나든 반드시 한 번 나간다 — 이 기본값에 닿는 길은 없다
    done.unwrap_or(EnumOutcome::Error { network: false })
}

/// 디렉터리 1단계 열거 — **다 읽기 전에 중간 결과를 흘려보낸다** (FR-69, 워커 스레드에서 호출).
///
/// `batch_size`개를 모은 **뒤에도 항목이 더 있을 때만** 그 몫을 `Partial`로 보낸다. 그래서
/// 항목이 `batch_size` 이하인 폴더에서는 `Partial`이 한 번도 나가지 않고 `Done` 하나로 끝나
/// **임계 아래에서는 동작이 종전과 완전히 같다**. `Done`이 싣는 것은 마지막 `Partial` 이후의
/// **잔여분**이다.
///
/// `on_chunk`가 `false`를 돌려주면(받는 쪽이 사라졌다) 그 자리에서 멈춘다 — 아무도 기다리지
/// 않는 폴더를 끝까지 읽을 이유가 없다. 그때는 `Done`을 보내지 않는다
pub fn enumerate_dir_batched(
    path: &Path,
    batch_size: usize,
    mut on_chunk: impl FnMut(EnumChunk) -> bool,
) {
    let pattern = to_extended_pattern(path);
    let pattern_h = HSTRING::from(pattern.as_str());
    let mut data = WIN32_FIND_DATAW::default();

    // 안전성: data는 스택 소유, 핸들은 FindClose로 반드시 해제된다
    unsafe {
        let handle = match FindFirstFileExW(
            &pattern_h,
            FindExInfoBasic,
            &mut data as *mut _ as *mut core::ffi::c_void,
            FindExSearchNameMatch,
            None,
            FIND_FIRST_EX_LARGE_FETCH,
        ) {
            Ok(h) => h,
            Err(e) => {
                let code = WIN32_ERROR(e.code().0 as u32 & 0xffff);
                let outcome = match code {
                    ERROR_ACCESS_DENIED => EnumOutcome::AccessDenied,
                    ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => EnumOutcome::NotFound,
                    _ => EnumOutcome::Error {
                        network: is_network_error(code),
                    },
                };
                on_chunk(EnumChunk::Done(outcome));
                return;
            }
        };

        let mut entries = Vec::new();
        loop {
            push_entry(&data, &mut entries);
            data = WIN32_FIND_DATAW::default();
            if let Err(e) = FindNextFileW(handle, &mut data) {
                let _ = FindClose(handle);
                let code = WIN32_ERROR(e.code().0 as u32 & 0xffff);
                let outcome = if code == ERROR_NO_MORE_FILES {
                    EnumOutcome::Ok(entries)
                } else {
                    // 읽는 **중에** 끊기는 경우도 있다 — 여기도 같은 판정을 거친다
                    EnumOutcome::Error {
                        network: is_network_error(code),
                    }
                };
                on_chunk(EnumChunk::Done(outcome));
                return;
            }
            // **다음 항목이 있음을 확인한 뒤에** 흘려보낸다 — 여기서 재지 않고 담자마자 재면
            // 항목이 딱 `batch_size`인 폴더도 `Partial`을 한 번 내보내게 된다
            if entries.len() >= batch_size {
                let batch = std::mem::take(&mut entries);
                if !on_chunk(EnumChunk::Partial(batch)) {
                    let _ = FindClose(handle);
                    return;
                }
            }
        }
    }
}

/// `.`·`..`을 제외하고 항목 추가
fn push_entry(data: &WIN32_FIND_DATAW, out: &mut Vec<FileEntry>) {
    let len = data
        .cFileName
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(data.cFileName.len());
    let name_slice = &data.cFileName[..len];
    if name_slice == [b'.' as u16] || name_slice == [b'.' as u16, b'.' as u16] {
        return;
    }
    let mut name = name_slice.to_vec();
    name.push(0);
    out.push(FileEntry {
        name,
        is_dir: (data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0) != 0,
        size: ((data.nFileSizeHigh as u64) << 32) | data.nFileSizeLow as u64,
        modified: ((data.ftLastWriteTime.dwHighDateTime as u64) << 32)
            | data.ftLastWriteTime.dwLowDateTime as u64,
        attributes: data.dwFileAttributes,
    });
}

/// 검색 패턴 `\\?\<절대경로>\*` 생성 (이미 확장 접두가 있으면 유지)
fn to_extended_pattern(path: &Path) -> String {
    // `\\?\` 접두사는 Win32의 경로 정규화를 **건너뛰게** 한다 — 그래서 이 접두사가 붙은 경로에서는
    // `/`가 디렉터리 구분자로 인식되지 않는다. 주소창에 `C:/Users`처럼 입력하면 열거가 실패하므로
    // 접두사를 붙이기 전에 구분자를 통일한다
    let s = path.to_string_lossy().replace('/', r"\");
    let base = if s.starts_with(r"\\?\") {
        s
    } else if s.starts_with(r"\\") {
        // UNC 경로: \\server\share → \\?\UNC\server\share
        format!(r"\\?\UNC{}", &s[1..])
    } else {
        format!(r"\\?\{s}")
    };
    if base.ends_with('\\') {
        format!("{base}*")
    } else {
        format!("{base}\\*")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN;
    use windows::core::PCWSTR;

    fn make_temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fe_enum_test_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn 확장_패턴은_슬래시를_백슬래시로_바꾼다() {
        // `\\?\` 접두사는 정규화를 건너뛰므로 슬래시가 남으면 열거가 실패한다
        assert_eq!(
            to_extended_pattern(Path::new("C:/Windows/System32")),
            r"\\?\C:\Windows\System32\*"
        );
    }

    #[test]
    fn 확장_패턴은_경로_형태별로_접두사를_붙인다() {
        assert_eq!(
            to_extended_pattern(Path::new(r"C:\Users")),
            r"\\?\C:\Users\*"
        );
        // 끝에 구분자가 있으면 중복해서 붙이지 않는다
        assert_eq!(to_extended_pattern(Path::new(r"C:\")), r"\\?\C:\*");
        // UNC는 \\?\UNC\ 형태로
        assert_eq!(
            to_extended_pattern(Path::new(r"\\server\share")),
            r"\\?\UNC\server\share\*"
        );
        // 이미 확장 접두사가 붙었으면 그대로 둔다
        assert_eq!(
            to_extended_pattern(Path::new(r"\\?\D:\data")),
            r"\\?\D:\data\*"
        );
    }

    #[test]
    fn 슬래시_경로도_실제로_열거된다() {
        let dir = make_temp_dir("slash");
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        // 백슬래시 경로를 슬래시로 바꿔도 같은 결과가 나와야 한다
        let slash = PathBuf::from(dir.to_string_lossy().replace('\\', "/"));
        let outcome = enumerate_dir(&slash);
        let EnumOutcome::Ok(entries) = outcome else {
            panic!("슬래시 경로 열거 실패");
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name_string(), "a.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 파일과_폴더를_열거한다() {
        let dir = make_temp_dir("basic");
        std::fs::write(dir.join("a.txt"), b"hello").unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();

        let EnumOutcome::Ok(entries) = enumerate_dir(&dir) else {
            panic!("열거 실패");
        };
        assert_eq!(entries.len(), 2);
        let a = entries.iter().find(|e| e.name_string() == "a.txt").unwrap();
        assert!(!a.is_dir);
        assert_eq!(a.size, 5);
        let sub = entries.iter().find(|e| e.name_string() == "sub").unwrap();
        assert!(sub.is_dir);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 없는_경로는_notfound() {
        let ghost = std::env::temp_dir().join("fe_enum_test_ghost_없는폴더_12345");
        assert!(matches!(enumerate_dir(&ghost), EnumOutcome::NotFound));
    }

    #[test]
    fn 네트워크_계열_오류_코드를_가려낸다() {
        // T1 Acceptance — 끊긴 네트워크 드라이브의 실측값(53)을 포함해 같은 계열을 모두 본다.
        // 화면이 "연결을 살펴라"와 "다시 열어 보라"를 갈라 적는 근거가 이 판정이다
        for code in [
            ERROR_REM_NOT_LIST,
            ERROR_BAD_NETPATH,
            ERROR_DEV_NOT_EXIST,
            ERROR_UNEXP_NET_ERR,
            ERROR_NETNAME_DELETED,
            ERROR_BAD_NET_NAME,
            ERROR_NO_NET_OR_BAD_PATH,
            ERROR_NETWORK_UNREACHABLE,
        ] {
            assert!(
                is_network_error(code),
                "네트워크 계열인데 아니라고 했다: {}",
                code.0
            );
        }
    }

    #[test]
    fn 네트워크와_무관한_오류_코드는_일반_실패다() {
        // 목록 밖 코드는 일반 실패로 떨어져야 한다 — 넓게 잡아 엉뚱한 실패에
        // "연결을 확인하라"고 말하면 사용자를 헛되게 만든다
        for code in [
            WIN32_ERROR(112),  // ERROR_DISK_FULL
            WIN32_ERROR(32),   // ERROR_SHARING_VIOLATION
            WIN32_ERROR(1392), // ERROR_FILE_CORRUPT
            ERROR_ACCESS_DENIED,
            ERROR_PATH_NOT_FOUND,
        ] {
            assert!(
                !is_network_error(code),
                "네트워크 계열이 아닌데 그렇다고 했다: {}",
                code.0
            );
        }
    }

    #[test]
    fn 긴_경로도_열거된다() {
        // MAX_PATH(260)를 넘는 폴더 — `\\?\` 접두사가 붙지 않으면 여기서 실패한다 (NFR-5).
        // 만들 때는 확장 접두사를 직접 붙인다: std는 일반 경로에 그 변환을 해주지 않는다
        let base = make_temp_dir("long");
        let mut deep = base.clone();
        while deep.to_string_lossy().chars().count() < 300 {
            deep = deep.join("긴이름_폴더_레벨_구분자를_길게");
        }
        let verbatim = PathBuf::from(format!(r"\\?\{}", deep.to_string_lossy()));
        std::fs::create_dir_all(&verbatim).unwrap();
        std::fs::write(verbatim.join("깊은 파일.txt"), b"x").unwrap();

        // 열거는 접두사 없는 원래 경로로 시도한다 — 앱이 다루는 형태 그대로다
        let EnumOutcome::Ok(entries) = enumerate_dir(&deep) else {
            panic!("긴 경로 열거 실패 ({}자)", deep.to_string_lossy().len());
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name_string(), "깊은 파일.txt");

        let _ = std::fs::remove_dir_all(PathBuf::from(format!(r"\\?\{}", base.to_string_lossy())));
    }

    #[test]
    fn 유니코드_이름을_보존한다() {
        let dir = make_temp_dir("uni");
        std::fs::write(dir.join("한글 파일 😀.txt"), b"x").unwrap();

        let EnumOutcome::Ok(entries) = enumerate_dir(&dir) else {
            panic!("열거 실패");
        };
        assert_eq!(entries[0].name_string(), "한글 파일 😀.txt");
        assert_eq!(entries[0].extension(), "txt");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 숨김_속성이_그대로_실린다() {
        // 무엇을 숨길지는 화면 규칙이고(FR-13), 열거는 OS가 준 사실만 나른다
        let dir = make_temp_dir("hidden");
        std::fs::write(dir.join("보통.txt"), b"x").unwrap();
        let 숨긴것 = dir.join("숨김.txt");
        std::fs::write(&숨긴것, b"x").unwrap();
        set_hidden_for_test(&숨긴것);

        let EnumOutcome::Ok(entries) = enumerate_dir(&dir) else {
            panic!("열거 실패");
        };
        let 찾기 = |name: &str| {
            entries
                .iter()
                .find(|e| e.name_string() == name)
                .unwrap_or_else(|| panic!("{name}이 없다"))
        };
        assert_eq!(
            찾기("보통.txt").attributes & FILE_ATTRIBUTE_HIDDEN.0,
            0,
            "숨기지 않은 파일에 숨김 속성이 붙었다"
        );
        assert_ne!(
            찾기("숨김.txt").attributes & FILE_ATTRIBUTE_HIDDEN.0,
            0,
            "숨김 속성이 실리지 않았다"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 시험용으로 파일에 숨김 속성을 건다 — Rust 표준 라이브러리에는 이 조작이 없다
    fn set_hidden_for_test(path: &Path) {
        use windows::Win32::Storage::FileSystem::SetFileAttributesW;
        use windows::core::HSTRING;
        let wide = HSTRING::from(path.as_os_str());
        // 안전성: 널 종단 경로와 속성 상수를 넘기는 단순 호출이다
        unsafe {
            SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_HIDDEN).expect("속성 설정");
        }
    }

    #[test]
    fn 확장자_추출_규칙() {
        let mk = |name: &str, is_dir: bool| {
            let mut v: Vec<u16> = name.encode_utf16().collect();
            v.push(0);
            FileEntry {
                name: v,
                is_dir,
                size: 0,
                modified: 0,
                attributes: 0,
            }
        };
        assert_eq!(mk("A.TXT", false).extension(), "txt");
        assert_eq!(mk("no_ext", false).extension(), "");
        assert_eq!(mk(".gitignore", false).extension(), "");
        assert_eq!(mk("dir.name", true).extension(), "");
    }

    /// 배치 열거 시험용 — 조각을 모아 `(중간 배치들, 완료 결과)`로 돌려준다
    fn collect_chunks(dir: &Path, batch: usize) -> (Vec<Vec<FileEntry>>, Option<EnumOutcome>) {
        let mut partials = Vec::new();
        let mut done = None;
        enumerate_dir_batched(dir, batch, |chunk| {
            match chunk {
                EnumChunk::Partial(entries) => partials.push(entries),
                EnumChunk::Done(outcome) => done = Some(outcome),
            }
            true
        });
        (partials, done)
    }

    fn make_files(dir: &Path, count: usize) {
        for index in 0..count {
            std::fs::write(dir.join(format!("f{index:05}.txt")), b"x").unwrap();
        }
    }

    #[test]
    fn 임계_이하면_중간_조각이_한_번도_나가지_않는다() {
        // FR-69의 핵심 계약 — 작은 폴더에서는 동작이 종전과 완전히 같아야 한다
        let dir = make_temp_dir("batch_small");
        make_files(&dir, 5);
        let (partials, done) = collect_chunks(&dir, 10);
        assert!(partials.is_empty(), "임계 아래인데 중간 조각이 나갔다");
        let Some(EnumOutcome::Ok(entries)) = done else {
            panic!("완료 조각이 없다");
        };
        assert_eq!(entries.len(), 5);
    }

    #[test]
    fn 항목_수가_임계와_같으면_중간_조각이_나가지_않는다() {
        // 경계값 — 담자마자 재면 여기서 조각이 한 번 새어 나간다.
        // **다음 항목이 있음을 확인한 뒤에** 흘려보내야 이 시험이 통과한다
        let dir = make_temp_dir("batch_exact");
        make_files(&dir, 4);
        let (partials, done) = collect_chunks(&dir, 4);
        assert!(partials.is_empty(), "항목 수가 임계와 같은데 조각이 나갔다");
        let Some(EnumOutcome::Ok(entries)) = done else {
            panic!("완료 조각이 없다");
        };
        assert_eq!(entries.len(), 4);
    }

    #[test]
    fn 임계를_넘으면_조각으로_나뉘고_합계가_보존된다() {
        let dir = make_temp_dir("batch_many");
        make_files(&dir, 10);
        let (partials, done) = collect_chunks(&dir, 3);
        assert!(!partials.is_empty(), "임계를 넘었는데 조각이 없다");
        // 완료 조각은 **잔여분만** 싣는다 — 전량을 다시 실으면 메모리가 두 배가 된다
        let Some(EnumOutcome::Ok(rest)) = done else {
            panic!("완료 조각이 없다");
        };
        let sum: usize = partials.iter().map(Vec::len).sum::<usize>() + rest.len();
        assert_eq!(sum, 10, "조각 합계가 전체와 다르다");
        for batch in &partials {
            assert_eq!(batch.len(), 3, "중간 조각은 임계만큼 담긴다");
        }
    }

    #[test]
    fn 받는_쪽이_멈추라면_완료_조각을_보내지_않는다() {
        // 수신부가 사라진 폴더를 끝까지 읽을 이유가 없다
        let dir = make_temp_dir("batch_stop");
        make_files(&dir, 10);
        let mut seen = 0;
        let mut done_seen = false;
        enumerate_dir_batched(&dir, 3, |chunk| {
            match chunk {
                EnumChunk::Partial(_) => seen += 1,
                EnumChunk::Done(_) => done_seen = true,
            }
            false
        });
        assert_eq!(seen, 1, "첫 조각에서 멈춰야 한다");
        assert!(!done_seen, "멈춘 뒤에는 완료 조각을 보내지 않는다");
    }

    #[test]
    fn 없는_폴더는_배치_경로에서도_같은_사유로_끝난다() {
        let dir = make_temp_dir("batch_ghost");
        let ghost = dir.join("없는폴더");
        let (partials, done) = collect_chunks(&ghost, 3);
        assert!(partials.is_empty());
        assert!(matches!(done, Some(EnumOutcome::NotFound)));
    }
}
