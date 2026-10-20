//! 최근 읽은 폴더의 목록 캐시 — 다시 들어간 폴더를 즉시 그리기 위한 것이다 (FR-68).
//!
//! **순수 모델이다** — 화면도 파일시스템도 모르고, 무엇을 언제 담을지는 `ui`가 정한다.
//! 여기 있는 것은 「무엇을 담을 수 있고 언제 버리는가」뿐이다.
//!
//! **상한을 두는 이유**: 유휴 메모리 요구(NFR-2 150MB)에 여유가 없다 — 2026-08-14 실측이
//! 173.8MB로 이미 그 선을 넘었다. 그래서 폴더 수와 폴더당 항목 수를 **둘 다** 묶는다.
//! 항목이 많은 폴더는 담지 않고 점진 표시(FR-69)가 대신 받는다 — 역할이 갈린다.
use crate::fs::enumerate::FileEntry;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 폴더 목록 캐시 — 가장 오래 안 쓴 것부터 버린다
#[derive(Debug, Default)]
pub struct DirCache {
    entries: HashMap<PathBuf, Vec<FileEntry>>,
    /// 쓴 차례 — **앞이 가장 오래됐다**. `get`이 적중하면 그 폴더가 뒤로 간다
    order: Vec<PathBuf>,
}

impl DirCache {
    /// 담아 두는 폴더 수 상한
    pub const MAX_DIRS: usize = 8;
    /// 폴더 하나에 담을 항목 수 상한 — 이보다 큰 폴더는 **담지 않는다**
    pub const MAX_ENTRIES: usize = 5000;

    pub fn new() -> DirCache {
        DirCache::default()
    }

    /// 읽어 낸 목록을 담는다.
    ///
    /// **항목이 상한을 넘으면 담지 않는다** — 그런 폴더는 점진 표시(FR-69)가 받는다.
    /// 같은 폴더를 다시 담으면 덮어쓰고 차례가 맨 뒤로 간다(중복이 쌓이지 않는다)
    pub fn put(&mut self, dir: &Path, entries: &[FileEntry]) {
        if entries.len() > Self::MAX_ENTRIES {
            // 담지 못한 것을 지워 두지 않으면 옛 목록이 남아 재진입에서 그것이 선다
            self.invalidate(dir);
            return;
        }
        self.touch(dir);
        self.entries.insert(dir.to_path_buf(), entries.to_vec());
        self.evict();
    }

    /// 담아 둔 목록 — 없으면 `None`. **적중하면 차례가 갱신된다**(그래서 `&mut self`다)
    pub fn get(&mut self, dir: &Path) -> Option<&[FileEntry]> {
        if !self.entries.contains_key(dir) {
            return None;
        }
        self.touch(dir);
        self.entries.get(dir).map(Vec::as_slice)
    }

    /// 그 폴더의 캐시를 버린다 — 열어 보니 없거나 읽지 못한 폴더에 쓴다
    pub fn invalidate(&mut self, dir: &Path) {
        self.entries.remove(dir);
        self.order.retain(|kept| kept != dir);
    }

    /// 그 폴더를 차례의 맨 뒤(가장 최근)로 옮긴다
    fn touch(&mut self, dir: &Path) {
        self.order.retain(|kept| kept != dir);
        self.order.push(dir.to_path_buf());
    }

    /// 폴더 수가 상한을 넘으면 **가장 오래 안 쓴 것부터** 버린다
    fn evict(&mut self) {
        while self.order.len() > Self::MAX_DIRS {
            let oldest = self.order.remove(0);
            self.entries.remove(&oldest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> FileEntry {
        let mut wide: Vec<u16> = name.encode_utf16().collect();
        wide.push(0);
        FileEntry {
            name: wide.into(),
            is_dir: false,
            size: 0,
            modified: 0,
            attributes: 0,
            location: None,
        }
    }

    fn rows(count: usize) -> Vec<FileEntry> {
        (0..count).map(|i| entry(&format!("f{i}.txt"))).collect()
    }

    fn dir(name: &str) -> PathBuf {
        PathBuf::from(format!(r"C:\{name}"))
    }

    #[test]
    fn 담은_것을_그대로_돌려준다() {
        let mut cache = DirCache::new();
        cache.put(&dir("a"), &rows(3));
        assert_eq!(cache.get(&dir("a")).map(<[FileEntry]>::len), Some(3));
        assert!(cache.get(&dir("없는곳")).is_none());
    }

    #[test]
    fn 빈_목록도_유효한_캐시다() {
        // 빈 폴더를 다시 열면 `이 폴더는 비어 있습니다`가 즉시 서야 한다 —
        // 항목이 0개인 것과 담지 않은 것은 다르다
        let mut cache = DirCache::new();
        cache.put(&dir("빈폴더"), &[]);
        assert_eq!(cache.get(&dir("빈폴더")).map(<[FileEntry]>::len), Some(0));
    }

    #[test]
    fn 상한을_넘는_폴더는_담지_않는다() {
        let mut cache = DirCache::new();
        cache.put(&dir("큰곳"), &rows(DirCache::MAX_ENTRIES + 1));
        assert!(cache.get(&dir("큰곳")).is_none(), "상한을 넘었는데 담겼다");
        // 경계값 — 딱 상한이면 담긴다
        cache.put(&dir("경계"), &rows(DirCache::MAX_ENTRIES));
        assert!(cache.get(&dir("경계")).is_some(), "경계값이 담기지 않았다");
    }

    #[test]
    fn 상한을_넘어_다시_담으면_옛_목록이_남지_않는다() {
        // 폴더가 커진 경우 — 담지 못한 채 옛 목록을 두면 재진입에서 그것이 선다
        let mut cache = DirCache::new();
        cache.put(&dir("자란곳"), &rows(3));
        cache.put(&dir("자란곳"), &rows(DirCache::MAX_ENTRIES + 1));
        assert!(cache.get(&dir("자란곳")).is_none());
    }

    #[test]
    fn 상한을_넘으면_가장_오래_안_쓴_폴더가_빠진다() {
        let mut cache = DirCache::new();
        for index in 0..DirCache::MAX_DIRS {
            cache.put(&dir(&index.to_string()), &rows(1));
        }
        cache.put(&dir("새것"), &rows(1));
        assert!(cache.get(&dir("0")).is_none(), "가장 오래된 것이 남았다");
        assert!(cache.get(&dir("1")).is_some());
        assert!(cache.get(&dir("새것")).is_some());
    }

    #[test]
    fn 조회한_폴더는_차례가_갱신돼_살아남는다() {
        // `get`이 차례를 갱신하지 않으면 방금 본 폴더가 축출된다
        let mut cache = DirCache::new();
        for index in 0..DirCache::MAX_DIRS {
            cache.put(&dir(&index.to_string()), &rows(1));
        }
        assert!(cache.get(&dir("0")).is_some());
        cache.put(&dir("새것"), &rows(1));
        assert!(cache.get(&dir("0")).is_some(), "조회한 폴더가 축출됐다");
        assert!(
            cache.get(&dir("1")).is_none(),
            "그다음으로 오래된 것이 빠져야 한다"
        );
    }

    #[test]
    fn 같은_폴더를_다시_담아도_중복이_쌓이지_않는다() {
        let mut cache = DirCache::new();
        for _ in 0..DirCache::MAX_DIRS * 2 {
            cache.put(&dir("같은곳"), &rows(1));
        }
        assert_eq!(cache.order.len(), 1, "차례에 중복이 쌓였다");
        cache.put(&dir("다른곳"), &rows(2));
        assert_eq!(cache.get(&dir("같은곳")).map(<[FileEntry]>::len), Some(1));
        assert_eq!(cache.get(&dir("다른곳")).map(<[FileEntry]>::len), Some(2));
    }

    #[test]
    fn 버린_폴더는_다시_주지_않는다() {
        let mut cache = DirCache::new();
        cache.put(&dir("사라진곳"), &rows(2));
        cache.invalidate(&dir("사라진곳"));
        assert!(cache.get(&dir("사라진곳")).is_none());
        assert!(cache.order.is_empty(), "차례에 흔적이 남았다");
    }

    #[test]
    fn 다시_담은_폴더는_차례의_맨_뒤로_간다() {
        // 덮어쓰기만으로는 부족하다 — 차례가 갱신되지 않으면 방금 다시 읽은 폴더가 먼저 빠진다
        let mut cache = DirCache::new();
        for index in 0..DirCache::MAX_DIRS {
            cache.put(&dir(&index.to_string()), &rows(1));
        }
        cache.put(&dir("0"), &rows(2)); // 가장 오래된 것을 다시 담는다
        cache.put(&dir("새것"), &rows(1));
        assert!(cache.get(&dir("0")).is_some(), "다시 담은 폴더가 축출됐다");
        assert!(
            cache.get(&dir("1")).is_none(),
            "그다음으로 오래된 것이 빠져야 한다"
        );
    }

    #[test]
    fn 담지_못한_폴더는_축출을_일으키지_않는다() {
        // 상한을 넘는 폴더는 자리를 차지하지 않는다 — 그것 때문에 멀쩡한 캐시가 빠지면
        // 큰 폴더를 한 번 지나가는 것만으로 담아 둔 것이 전부 사라진다
        let mut cache = DirCache::new();
        for index in 0..DirCache::MAX_DIRS {
            cache.put(&dir(&index.to_string()), &rows(1));
        }
        cache.put(&dir("아주큰곳"), &rows(DirCache::MAX_ENTRIES + 1));
        assert!(
            cache.get(&dir("0")).is_some(),
            "담기지도 않은 폴더가 축출을 일으켰다"
        );
        assert_eq!(cache.order.len(), DirCache::MAX_DIRS);
    }
}
