//! zip 파일 안 읽기 — 목차와 풀기 (FR-78).
//!
//! zip 탭은 **읽기 전용**이라 이 모듈에는 zip 을 고치는 길이 없다. 목록은 머리글(중앙
//! 디렉터리)만 읽고, 풀기는 고른 항목을 대상 폴더에 흘려 쓴다. 둘 다 블로킹 I/O 라 워커
//! 스레드에서 부른다(AGENTS: UI 스레드 블로킹 금지).
//!
//! 이름 규칙은 파일 없이 재는 순수 함수로 떼어 둔다 — zip 이름은 만든 도구마다 인코딩
//! (UTF-8 표지·시스템 코드 페이지)과 구분자(`/`·`\`)가 달라, 바이트로 재야 갈래가 다 덮인다.
use crate::fs::enumerate::FileEntry;
use std::fmt;
use std::io;
use std::path::Path;

/// zip 안 항목 하나 — 이름 조각과 머리글이 준 사실
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipItem {
    pub parts: Vec<String>,
    pub is_dir: bool,
    pub size: u64,
    /// UTC FILETIME (100ns 단위) — 모르면 0
    pub modified: u64,
}

/// zip 을 읽지 못한 사유
#[derive(Debug)]
pub enum ZipError {
    Io(io::Error),
    /// zip 이 아니거나 머리글이 깨졌다
    Format(String),
}

impl fmt::Display for ZipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ZipError::Io(error) => error.fmt(f),
            ZipError::Format(message) => f.write_str(message),
        }
    }
}

/// 풀기 한 번의 결과 — 실패는 항목마다 (zip 안 경로, 사유)
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ExtractOutcome {
    pub written: usize,
    pub failed: Vec<(String, String)>,
}

/// 일반 목적 비트 11 — 서 있으면 이름이 UTF-8 이다 (APPNOTE 4.4.4)
const UTF8_FLAG: u16 = 0x0800;
/// 풀어 쓰는 동안의 임시 이름 꼬리 — 다 쓴 뒤 원래 이름으로 바꿔 놓는다
const PART_SUFFIX: &str = ".moa-part";

impl From<zip::result::ZipError> for ZipError {
    fn from(error: zip::result::ZipError) -> ZipError {
        match error {
            zip::result::ZipError::Io(error) => ZipError::Io(error),
            other => ZipError::Format(other.to_string()),
        }
    }
}

impl From<io::Error> for ZipError {
    fn from(error: io::Error) -> ZipError {
        ZipError::Io(error)
    }
}

/// 폴더처럼 들어갈 zip 인가 — 확장자만 본다(대소문자 무시)
pub fn is_zip_name(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
}

/// zip 이 담은 이름 바이트를 푼다 — UTF-8 표지가 서 있으면 UTF-8, 아니면 이 PC 의 시스템
/// 코드 페이지다. 표지 없는 zip 은 대개 그것을 만든 Windows 의 코드 페이지(한국어면 CP949)로
/// 이름을 적는다 — zip 크레이트의 `name()` 은 그 자리를 CP437 로 읽어 한글이 깨진다
pub fn decode_name(raw: &[u8], utf8: bool) -> String {
    if utf8 {
        String::from_utf8_lossy(raw).into_owned()
    } else {
        crate::fs::preview::decode_system_code_page(raw)
    }
}

/// 이름을 조각으로 나눈다 — `\` 도 구분자로 받고 빈 조각·`.` 은 버린다.
///
/// **zip 바깥을 가리키는 이름이면 `None`** 이다 — `..` 조각, `/`·`\` 로 시작하는 절대 경로,
/// 첫 조각의 드라이브(`C:`). 그런 이름을 그대로 풀면 대상 폴더 밖에 쓰게 된다(zip slip)
pub fn safe_parts(name: &str) -> Option<Vec<String>> {
    let normalized = name.replace('\\', "/");
    if normalized.starts_with('/') || normalized.contains('\0') {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    for piece in normalized.split('/') {
        match piece {
            "" | "." => {}
            ".." => return None,
            _ if parts.is_empty() && piece.contains(':') => return None,
            _ => parts.push(piece.to_owned()),
        }
    }
    (!parts.is_empty()).then_some(parts)
}

/// zip 안 경로(`a/b`)를 조각으로 — 뿌리(`""`)는 빈 목록이다
fn inner_parts(inner: &str) -> Vec<&str> {
    inner.split('/').filter(|part| !part.is_empty()).collect()
}

/// `inner` 바로 아래 항목들 — 이름 차례.
///
/// **zip 은 중간 폴더를 따로 적지 않아도 된다** — `deep/x/y.txt` 하나만 있어도 `deep`·`x` 를
/// 폴더로 세운다. 명시한 폴더 항목과 겹치면 명시한 쪽(시각이 있다)을 남긴다
pub fn children(items: &[ZipItem], inner: &str) -> Vec<ZipItem> {
    use std::collections::btree_map::Entry;
    let base = inner_parts(inner);
    let mut found: std::collections::BTreeMap<(String, bool), ZipItem> = Default::default();
    for item in items {
        if item.parts.len() <= base.len()
            || !item
                .parts
                .iter()
                .zip(&base)
                .all(|(part, want)| part == want)
        {
            continue;
        }
        let name = item.parts[base.len()].clone();
        let direct = item.parts.len() == base.len() + 1;
        let child = if direct {
            ZipItem {
                parts: vec![name.clone()],
                ..item.clone()
            }
        } else {
            ZipItem {
                parts: vec![name.clone()],
                is_dir: true,
                size: 0,
                modified: 0,
            }
        };
        match found.entry((name, child.is_dir)) {
            Entry::Vacant(slot) => {
                slot.insert(child);
            }
            Entry::Occupied(mut slot) => {
                if direct {
                    slot.insert(child);
                }
            }
        }
    }
    found.into_values().collect()
}

/// zip 의 `inner` 바로 아래를 목록 항목으로 읽는다 — 머리글만 읽고 본문은 풀지 않는다
pub fn list(archive: &Path, inner: &str) -> Result<Vec<FileEntry>, ZipError> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)?;
    let mut items = Vec::with_capacity(zip.len());
    for index in 0..zip.len() {
        let file = zip.by_index_raw(index)?;
        let name = decode_name(file.name_raw(), file.flags().as_u16() & UTF8_FLAG != 0);
        let Some(parts) = safe_parts(&name) else {
            continue;
        };
        let modified = file.last_modified().map_or(0, |at| {
            dos_to_filetime(
                at.year(),
                at.month(),
                at.day(),
                at.hour(),
                at.minute(),
                at.second(),
            )
        });
        items.push(ZipItem {
            parts,
            is_dir: file.is_dir() || name.ends_with(['/', '\\']),
            size: file.size(),
            modified,
        });
    }
    Ok(children(&items, inner).into_iter().map(to_entry).collect())
}

/// 목록 항목으로 — 이름은 널 종단 UTF-16 이라는 `FileEntry` 의 불변식을 지킨다
fn to_entry(item: ZipItem) -> FileEntry {
    let name: Vec<u16> = item.parts[0]
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    FileEntry {
        name: name.into(),
        is_dir: item.is_dir,
        size: if item.is_dir { 0 } else { item.size },
        modified: item.modified,
        attributes: if item.is_dir {
            windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY.0
        } else {
            0
        },
        location: None,
    }
}

/// 고른 zip 안 경로들(파일이면 그것, 폴더면 하위 전부)을 `dest` 에 푼다.
///
/// 각 항목은 **고른 것의 부모 기준 상대 경로**로 놓인다 — `docs/sub` 를 고르면 `dest\sub\…`.
/// 본문은 흘려 쓰고(파일 전체를 메모리에 담지 않는다) 임시 이름에 다 쓴 뒤 바꿔 놓는다 —
/// 덮어쓰는 도중 실패해도 있던 파일이 반쯤 지워진 채 남지 않는다. 풀지 못한 항목은
/// 건너뛰고 사유를 모은다
pub fn extract(archive: &Path, picks: &[String], dest: &Path) -> ExtractOutcome {
    let mut outcome = ExtractOutcome::default();
    let opened = std::fs::File::open(archive)
        .map_err(ZipError::from)
        .and_then(|file| zip::ZipArchive::new(file).map_err(ZipError::from));
    let mut zip = match opened {
        Ok(zip) => zip,
        Err(error) => {
            outcome.failed = picks
                .iter()
                .map(|pick| (pick.clone(), error.to_string()))
                .collect();
            return outcome;
        }
    };
    let picks: Vec<Vec<&str>> = picks
        .iter()
        .map(|pick| inner_parts(pick))
        .filter(|parts| !parts.is_empty())
        .collect();
    for index in 0..zip.len() {
        let (name, is_dir) = match zip.by_index_raw(index) {
            Ok(file) => {
                let name = decode_name(file.name_raw(), file.flags().as_u16() & UTF8_FLAG != 0);
                let is_dir = file.is_dir() || name.ends_with(['/', '\\']);
                (name, is_dir)
            }
            Err(error) => {
                outcome
                    .failed
                    .push((format!("#{index}"), error.to_string()));
                continue;
            }
        };
        let Some(parts) = safe_parts(&name) else {
            // 바깥을 가리키는 이름 — 고른 것 아래에 있었으면 실패로 센다
            let normalized = name.replace('\\', "/");
            if picks
                .iter()
                .any(|pick| normalized.starts_with(&format!("{}/", pick.join("/"))))
            {
                outcome
                    .failed
                    .push((normalized, crate::i18n::zip_unsafe_name().to_owned()));
            }
            continue;
        };
        let Some(pick) = picks.iter().find(|pick| {
            parts.len() >= pick.len() && parts.iter().zip(pick.iter()).all(|(a, b)| a == b)
        }) else {
            continue;
        };
        let relative = &parts[pick.len() - 1..];
        if !relative.iter().all(|part| windows_name_ok(part)) {
            outcome
                .failed
                .push((parts.join("/"), crate::i18n::zip_bad_name().to_owned()));
            continue;
        }
        let target = relative
            .iter()
            .fold(dest.to_path_buf(), |path, part| path.join(part));
        let written = if is_dir {
            std::fs::create_dir_all(&target).map(|()| false)
        } else {
            write_entry(&mut zip, index, &target).map(|()| true)
        };
        match written {
            Ok(true) => outcome.written += 1,
            Ok(false) => {}
            Err(error) => outcome.failed.push((parts.join("/"), error.to_string())),
        }
    }
    outcome
}

/// Windows 파일 이름으로 쓸 수 있는 조각인가 — 금지 글자·제어 문자가 없고 점·공백으로 끝나지 않는다
fn windows_name_ok(part: &str) -> bool {
    !part
        .chars()
        .any(|ch| matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*') || u32::from(ch) < 32)
        && !part.ends_with(['.', ' '])
}

/// 항목 하나를 임시 이름에 다 쓴 뒤 `target` 으로 바꿔 놓는다 — 실패하면 임시 파일만 지운다
fn write_entry(
    zip: &mut zip::ZipArchive<std::fs::File>,
    index: usize,
    target: &Path,
) -> io::Result<()> {
    let mut file = zip.by_index(index).map_err(io::Error::other)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut part = target.as_os_str().to_owned();
    part.push(PART_SUFFIX);
    let part = std::path::PathBuf::from(part);
    let result = std::fs::File::create(&part)
        .and_then(|mut out| io::copy(&mut file, &mut out).map(|_| ()))
        .and_then(|()| std::fs::rename(&part, target));
    if result.is_err() {
        // 이번 작업이 만든 임시 파일이라 지워도 잃을 사용자 내용이 없다
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// zip 의 MS-DOS 시각(그 PC 의 지역 벽시계)을 UTC FILETIME 으로 — 틀린 시각이면 0
fn dos_to_filetime(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> u64 {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{SystemTimeToFileTime, TzSpecificLocalTimeToSystemTime};
    let local = SYSTEMTIME {
        wYear: year,
        wMonth: month.into(),
        wDay: day.into(),
        wHour: hour.into(),
        wMinute: minute.into(),
        wSecond: second.into(),
        ..Default::default()
    };
    let mut utc = SYSTEMTIME::default();
    let mut ft = FILETIME::default();
    // 안전성: 인자는 모두 스택 소유 값이고 두 함수는 넘긴 구조체 밖을 건드리지 않는다
    let ok = unsafe {
        TzSpecificLocalTimeToSystemTime(None, &local, &mut utc).is_ok()
            && SystemTimeToFileTime(&utc, &mut ft).is_ok()
    };
    if !ok {
        return 0;
    }
    u64::from(ft.dwLowDateTime) | (u64::from(ft.dwHighDateTime) << 32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;

    fn item(path: &str, is_dir: bool) -> ZipItem {
        ZipItem {
            parts: path.split('/').map(str::to_owned).collect(),
            is_dir,
            size: if is_dir { 0 } else { 7 },
            modified: 1,
        }
    }

    fn names(items: &[ZipItem]) -> Vec<(String, bool)> {
        items
            .iter()
            .map(|item| (item.parts.join("/"), item.is_dir))
            .collect()
    }

    /// 시험마다 새 폴더 — 프로세스 번호를 붙여 병렬 시험끼리 겹치지 않게 한다
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("moa_zip_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("시험 폴더");
        dir
    }

    /// (이름, 내용) 들로 zip 을 만든다 — 내용이 `None` 이면 폴더 항목이다
    fn make_zip(path: &Path, entries: &[(&str, Option<&[u8]>)], method: CompressionMethod) {
        let file = std::fs::File::create(path).expect("zip 만들기");
        let mut writer = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(method);
        for (name, content) in entries {
            match content {
                Some(bytes) => {
                    writer.start_file(*name, options).expect("항목 시작");
                    writer.write_all(bytes).expect("항목 쓰기");
                }
                None => writer.add_directory(*name, options).expect("폴더 항목"),
            }
        }
        writer.finish().expect("zip 닫기");
    }

    fn entry_names(entries: &[FileEntry]) -> Vec<(String, bool, u64)> {
        let mut out: Vec<(String, bool, u64)> = entries
            .iter()
            .map(|entry| (entry.name_string(), entry.is_dir, entry.size))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn utf8_표지면_utf8로_푼다() {
        assert_eq!(decode_name("한글문서.txt".as_bytes(), true), "한글문서.txt");
    }

    #[test]
    fn 표지가_없으면_시스템_코드_페이지로_푼다() {
        // 2026-10-20 실측 — Windows PowerShell 이 CP949 로 만든 zip 의 이름 원시 바이트
        let raw = [
            0xc7, 0xd1, 0xb1, 0xdb, 0xb9, 0xae, 0xbc, 0xad, 0x2e, 0x74, 0x78, 0x74,
        ];
        let decoded = decode_name(&raw, false);
        // 시스템 코드 페이지가 949 인 PC(이 레포의 개발 PC)에서만 한글로 풀린다
        if unsafe { windows::Win32::Globalization::GetACP() } == 949 {
            assert_eq!(decoded, "한글문서.txt");
        } else {
            assert!(decoded.ends_with(".txt"), "{decoded}");
        }
    }

    #[test]
    fn 역슬래시_구분자를_슬래시로_맞춘다() {
        assert_eq!(
            safe_parts(r"하위폴더\b.txt"),
            Some(vec!["하위폴더".to_owned(), "b.txt".to_owned()])
        );
        assert_eq!(
            safe_parts("a//./b/"),
            Some(vec!["a".to_owned(), "b".to_owned()])
        );
    }

    #[test]
    fn 바깥을_가리키는_이름은_뺀다() {
        for name in [
            "../x",
            "a/../../x",
            "a/../x",
            "/etc/x",
            "C:/x",
            r"C:\x",
            r"\x",
        ] {
            assert_eq!(safe_parts(name), None, "{name}");
        }
    }

    #[test]
    fn 명시되지_않은_중간_폴더를_세운다() {
        let items = [item("deep/x/y.txt", false)];
        assert_eq!(names(&children(&items, "")), [("deep".to_owned(), true)]);
        assert_eq!(names(&children(&items, "deep")), [("x".to_owned(), true)]);
        assert_eq!(
            names(&children(&items, "deep/x")),
            [("y.txt".to_owned(), false)]
        );
    }

    #[test]
    fn 그_자리의_바로_아래만_낸다() {
        let items = [
            item("docs", true),
            item("docs/a.txt", false),
            item("docs/sub/b.txt", false),
            item("top.txt", false),
        ];
        assert_eq!(
            names(&children(&items, "")),
            [("docs".to_owned(), true), ("top.txt".to_owned(), false)]
        );
        assert_eq!(
            names(&children(&items, "docs")),
            [("a.txt".to_owned(), false), ("sub".to_owned(), true)]
        );
        assert!(children(&items, "없는곳").is_empty());
    }

    #[test]
    fn 같은_이름의_폴더는_하나다() {
        // 명시한 폴더 항목과 그 아래 파일이 세우는 함축 폴더가 겹친다 — 명시 항목이 이긴다
        let mut docs = item("docs", true);
        docs.modified = 42;
        let items = [item("docs/a.txt", false), docs, item("docs/b.txt", false)];
        let found = children(&items, "");
        assert_eq!(names(&found), [("docs".to_owned(), true)]);
        assert_eq!(found[0].modified, 42);
    }

    #[test]
    fn dos_시각을_utc_filetime으로_바꾼다() {
        use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
        use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
        let ticks = dos_to_filetime(2026, 10, 20, 22, 4, 24);
        assert!(ticks > 0);
        // 거꾸로 — UTC FILETIME 을 이 PC 의 지역 시각으로 되돌리면 넣은 벽시계가 나와야 한다
        let ft = FILETIME {
            dwLowDateTime: (ticks & 0xffff_ffff) as u32,
            dwHighDateTime: (ticks >> 32) as u32,
        };
        let mut utc = SYSTEMTIME::default();
        let mut local = SYSTEMTIME::default();
        unsafe {
            FileTimeToSystemTime(&ft, &mut utc).expect("utc");
            SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).expect("local");
        }
        assert_eq!(
            (
                local.wYear,
                local.wMonth,
                local.wDay,
                local.wHour,
                local.wMinute,
                local.wSecond
            ),
            (2026, 10, 20, 22, 4, 24)
        );
    }

    #[test]
    fn zip_을_목록으로_읽는다() {
        let dir = temp_dir("list");
        let archive = dir.join("시험.zip");
        make_zip(
            &archive,
            &[
                ("docs/", None),
                ("docs/a.txt", Some(b"hello")),
                ("top.txt", Some(b"top")),
                ("deep/x/y.txt", Some(b"y")),
            ],
            CompressionMethod::Deflated,
        );
        let root = list(&archive, "").expect("목록");
        assert_eq!(
            entry_names(&root),
            [
                ("deep".to_owned(), true, 0),
                ("docs".to_owned(), true, 0),
                ("top.txt".to_owned(), false, 3),
            ]
        );
        // 명시한 항목은 시각이 있고, 이름에서만 세운 중간 폴더(`deep`)는 시각을 모른다
        for entry in &root {
            let implied = entry.name_string() == "deep";
            assert_eq!(entry.modified == 0, implied, "{}", entry.name_string());
        }
        assert!(root.iter().all(|entry| entry.location.is_none()));
        let docs = list(&archive, "docs").expect("목록");
        assert_eq!(entry_names(&docs), [("a.txt".to_owned(), false, 5)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zip이_아니면_오류다() {
        let dir = temp_dir("notzip");
        let fake = dir.join("가짜.zip");
        std::fs::write(&fake, b"not a zip").expect("파일");
        assert!(list(&fake, "").is_err());
        assert!(list(&dir.join("없는.zip"), "").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zip_이름은_확장자로_가린다() {
        assert!(is_zip_name("자료.zip"));
        assert!(is_zip_name("DATA.ZIP"));
        assert!(!is_zip_name("자료.zipx"));
        assert!(!is_zip_name("zip"));
        assert!(!is_zip_name("a.7z"));
    }

    #[test]
    fn 고른_파일을_대상에_푼다() {
        let dir = temp_dir("extract_file");
        let archive = dir.join("a.zip");
        make_zip(
            &archive,
            &[("docs/a.txt", Some(b"hello")), ("docs/b.txt", Some(b"bee"))],
            CompressionMethod::Deflated,
        );
        let dest = dir.join("out");
        std::fs::create_dir_all(&dest).expect("대상");
        let outcome = extract(&archive, &["docs/a.txt".to_owned()], &dest);
        assert_eq!(
            outcome,
            ExtractOutcome {
                written: 1,
                failed: Vec::new()
            }
        );
        assert_eq!(
            std::fs::read(dest.join("a.txt")).expect("풀린 파일"),
            b"hello"
        );
        assert!(!dest.join("b.txt").exists(), "고르지 않은 것까지 풀었다");
        assert!(!dest.join("a.txt.moa-part").exists(), "임시 파일이 남았다");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 고른_폴더는_하위까지_푼다() {
        let dir = temp_dir("extract_dir");
        let archive = dir.join("a.zip");
        make_zip(
            &archive,
            &[
                ("root/docs/", None),
                ("root/docs/a.txt", Some(b"hello")),
                ("root/docs/sub/b.txt", Some(b"bee")),
                ("root/other.txt", Some(b"no")),
            ],
            CompressionMethod::Stored,
        );
        let dest = dir.join("out");
        std::fs::create_dir_all(&dest).expect("대상");
        let outcome = extract(&archive, &["root/docs".to_owned()], &dest);
        assert_eq!(outcome.failed, Vec::new());
        assert_eq!(outcome.written, 2);
        assert_eq!(
            std::fs::read(dest.join(r"docs\a.txt")).expect("a"),
            b"hello"
        );
        assert_eq!(
            std::fs::read(dest.join(r"docs\sub\b.txt")).expect("b"),
            b"bee"
        );
        assert!(!dest.join("other.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 바깥을_가리키는_항목은_풀지_않고_실패로_센다() {
        let dir = temp_dir("extract_slip");
        let archive = dir.join("a.zip");
        make_zip(
            &archive,
            &[
                ("a/ok.txt", Some(b"ok")),
                ("a/../../evil.txt", Some(b"evil")),
            ],
            CompressionMethod::Stored,
        );
        let dest = dir.join("out");
        std::fs::create_dir_all(&dest).expect("대상");
        let outcome = extract(&archive, &["a".to_owned()], &dest);
        assert_eq!(outcome.written, 1);
        assert_eq!(outcome.failed.len(), 1, "{:?}", outcome.failed);
        assert!(!dir.join("evil.txt").exists(), "zip 바깥에 썼다");
        assert!(!dest.join("evil.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 덮어쓸_때_도중_실패면_있던_파일이_남는다() {
        let dir = temp_dir("extract_broken");
        let archive = dir.join("a.zip");
        make_zip(
            &archive,
            &[("a.txt", Some(b"NEWDATA-UNIQUE-0123456789"))],
            CompressionMethod::Stored,
        );
        // 저장(Stored) 항목의 본문 한 글자를 바꿔 CRC 를 깨뜨린다 — 읽다가 끝에서 실패한다
        let mut bytes = std::fs::read(&archive).expect("zip 읽기");
        let at = bytes
            .windows(7)
            .position(|window| window == b"NEWDATA")
            .expect("본문 위치");
        bytes[at] = b'X';
        std::fs::write(&archive, &bytes).expect("zip 쓰기");
        let dest = dir.join("out");
        std::fs::create_dir_all(&dest).expect("대상");
        std::fs::write(dest.join("a.txt"), b"ORIGINAL").expect("있던 파일");
        let outcome = extract(&archive, &["a.txt".to_owned()], &dest);
        assert_eq!(outcome.written, 0);
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(
            std::fs::read(dest.join("a.txt")).expect("있던 파일"),
            b"ORIGINAL"
        );
        assert!(!dest.join("a.txt.moa-part").exists(), "임시 파일이 남았다");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 쓸_수_없는_이름은_그_항목만_실패다() {
        let dir = temp_dir("extract_badname");
        let archive = dir.join("a.zip");
        make_zip(
            &archive,
            &[
                ("d/a?b.txt", Some(b"q")),
                ("d/ok.txt", Some(b"ok")),
                ("d/끝점.", Some(b"dot")),
            ],
            CompressionMethod::Stored,
        );
        let dest = dir.join("out");
        std::fs::create_dir_all(&dest).expect("대상");
        let outcome = extract(&archive, &["d".to_owned()], &dest);
        assert_eq!(outcome.written, 1);
        assert_eq!(outcome.failed.len(), 2, "{:?}", outcome.failed);
        assert_eq!(std::fs::read(dest.join(r"d\ok.txt")).expect("ok"), b"ok");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
