//! 최근에 가 본 로컬 폴더 (FR-75) — 빠른 이동 팔레트(`Ctrl+P`)의 한 출처.
//!
//! **앱에 하나뿐이고 세션에 담긴다** — 모든 워크스페이스·패널의 이동이 같은 목록에 쌓인다.
//! 무엇을 「가 본 것」으로 세는지는 패널이 정하고(`ui::panel`), 여기는 쌓는 규칙만 갖는다.
use std::path::{Path, PathBuf};

/// 담아 두는 곳의 수 상한 — 넘으면 가장 오래된 것부터 버린다
pub const RECENT_LIMIT: usize = 50;

/// 최근에 가 본 폴더들 — **가장 최근이 앞**이다
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecentPaths {
    paths: Vec<PathBuf>,
}

impl RecentPaths {
    pub fn new() -> RecentPaths {
        RecentPaths::default()
    }

    /// 저장된 문자열에서 되살린다 — 손으로 고친 파일의 중복·초과분도 같은 규칙으로 거른다.
    /// 저장 차례가 「최근이 앞」이라 뒤에서부터 다시 쌓는다
    pub fn from_saved(saved: &[String]) -> RecentPaths {
        let mut recent = RecentPaths::new();
        // 앞쪽이 최근이라 상한 안의 것만 본다 — 뒤에서부터 쌓아야 차례가 그대로 남는다
        for path in saved.iter().take(RECENT_LIMIT).rev() {
            recent.visit(Path::new(path));
        }
        recent
    }

    /// 그 폴더에 갔다 — 맨 앞에 두고, 이미 있으면 옛 자리에서 뺀다
    pub fn visit(&mut self, path: &Path) {
        self.paths.retain(|kept| !same_path(kept, path));
        self.paths.insert(0, path.to_path_buf());
        self.paths.truncate(RECENT_LIMIT);
    }

    /// 최근이 앞인 목록 — 세션 저장과 팔레트가 쓴다
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }
}

/// 두 경로가 같은 곳인가 — Windows 파일 시스템처럼 **대소문자를 가리지 않는다**.
/// 팔레트가 즐겨찾기와 겹치는 최근 경로를 거를 때도 같은 규칙을 쓴다
pub fn same_path(a: &Path, b: &Path) -> bool {
    a.as_os_str().to_string_lossy().to_lowercase() == b.as_os_str().to_string_lossy().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn 가장_최근이_앞이다() {
        let mut recent = RecentPaths::new();
        recent.visit(&p(r"C:\A"));
        recent.visit(&p(r"C:\B"));
        assert_eq!(recent.paths(), [p(r"C:\B"), p(r"C:\A")]);
    }

    #[test]
    fn 같은_경로는_앞으로_옮긴다() {
        let mut recent = RecentPaths::new();
        for path in [r"C:\A", r"C:\B", r"C:\C", r"C:\A"] {
            recent.visit(&p(path));
        }
        assert_eq!(recent.paths(), [p(r"C:\A"), p(r"C:\C"), p(r"C:\B")]);
    }

    #[test]
    fn 대소문자만_다르면_같은_경로다() {
        // Windows 파일 시스템은 대소문자를 가리지 않는다 — 두 줄이 되면 상한을 헛되이 쓴다
        let mut recent = RecentPaths::new();
        recent.visit(&p(r"C:\Users"));
        recent.visit(&p(r"c:\users"));
        assert_eq!(recent.paths(), [p(r"c:\users")]);
    }

    #[test]
    fn 오십을_넘으면_가장_오래된_것을_버린다() {
        let mut recent = RecentPaths::new();
        for i in 0..=RECENT_LIMIT {
            recent.visit(&p(&format!(r"C:\d{i}")));
        }
        assert_eq!(recent.paths().len(), RECENT_LIMIT);
        assert_eq!(recent.paths()[0], p(&format!(r"C:\d{RECENT_LIMIT}")));
        assert!(
            !recent.paths().contains(&p(r"C:\d0")),
            "가장 오래된 것이 남았다"
        );
    }

    #[test]
    fn 저장값에서_되살린다() {
        let saved = vec![r"C:\B".to_owned(), r"C:\A".to_owned(), r"c:\b".to_owned()];
        let recent = RecentPaths::from_saved(&saved);
        // 저장 차례(최근이 앞)를 지키고, 뒤쪽의 중복은 앞의 것에 흡수된다
        assert_eq!(recent.paths(), [p(r"C:\B"), p(r"C:\A")]);
        let many: Vec<String> = (0..RECENT_LIMIT + 5).map(|i| format!(r"C:\d{i}")).collect();
        let recent = RecentPaths::from_saved(&many);
        assert_eq!(recent.paths().len(), RECENT_LIMIT);
        assert_eq!(recent.paths()[0], p(r"C:\d0"), "가장 최근 것이 잘렸다");
    }
}
