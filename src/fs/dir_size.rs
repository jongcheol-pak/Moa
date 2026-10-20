//! 폴더 크기 계산 (FR-71) — 하위까지 파일 크기를 더한다. 워커 스레드에서 부른다
use crate::fs::enumerate::{EnumChunk, EnumOutcome, enumerate_dir_batched};
use std::path::{Path, PathBuf};
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

/// 한 폴더를 읽는 도중 멈춤 신호를 보는 간격(항목 수)
const READ_BATCH: usize = 2000;

/// `path` 아래 모든 파일 크기의 합.
///
/// 재분석 지점(정션·링크) 폴더로는 내려가지 않는다 — 탐색기 속성 창과 같은 셈이고, 정션이
/// 위 폴더를 가리키면 끝나지 않는다. 열지 못한 **하위** 폴더는 건너뛰고, 그 폴더 자체를
/// 열지 못했거나 `should_stop`이 참이 되면 `None`이다
pub fn folder_size(path: &Path, should_stop: impl Fn() -> bool) -> Option<u64> {
    let mut pending: Vec<PathBuf> = vec![path.to_path_buf()];
    let mut total = 0u64;
    let mut first = true;
    while let Some(dir) = pending.pop() {
        let mut stopped = false;
        let mut opened = true;
        enumerate_dir_batched(&dir, READ_BATCH, |chunk| {
            if should_stop() {
                stopped = true;
                return false;
            }
            let entries = match chunk {
                EnumChunk::Partial(entries) | EnumChunk::Done(EnumOutcome::Ok(entries)) => entries,
                EnumChunk::Done(_) => {
                    opened = false;
                    return true;
                }
            };
            for entry in entries {
                if !entry.is_dir {
                    total = total.saturating_add(entry.size);
                } else if entry.attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0 {
                    pending.push(dir.join(entry.name_string()));
                }
            }
            true
        });
        if stopped || (first && !opened) {
            return None;
        }
        first = false;
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fe_dirsize_test_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn 하위_폴더까지_파일_크기를_더한다() {
        let dir = make_temp_dir("nested");
        std::fs::write(dir.join("a.bin"), vec![0u8; 100]).unwrap();
        std::fs::create_dir_all(dir.join(r"sub\deep")).unwrap();
        std::fs::write(dir.join(r"sub\b.bin"), vec![0u8; 20]).unwrap();
        std::fs::write(dir.join(r"sub\deep\c.bin"), vec![0u8; 3]).unwrap();
        assert_eq!(folder_size(&dir, || false), Some(123));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 빈_폴더는_0이다() {
        let dir = make_temp_dir("empty");
        assert_eq!(folder_size(&dir, || false), Some(0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 멈추라면_값이_없다() {
        let dir = make_temp_dir("stop");
        std::fs::write(dir.join("a.bin"), b"x").unwrap();
        assert_eq!(folder_size(&dir, || true), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 없는_폴더는_값이_없다() {
        let dir = make_temp_dir("ghost").join("없는폴더");
        assert_eq!(folder_size(&dir, || false), None);
    }
}
