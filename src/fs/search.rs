//! 하위 폴더까지 훑는 이름 검색 (FR-70) — 워커 스레드에서 부른다.
//!
//! 1단계 열거(`fs::enumerate`)를 폴더마다 되풀이한다. 흘려보내는 기준(폴더 수·일치 수)과
//! 내려가지 않을 폴더(링크·숨김)가 1단계 열거의 계약과 달라 따로 둔다
use crate::fs::enumerate::{EnumChunk, EnumOutcome, FileEntry, enumerate_dir_batched};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SYSTEM,
};

/// 이만큼 폴더를 훑을 때마다 모은 결과를 흘려보낸다
pub const FLUSH_DIRS: usize = 64;
/// 모은 결과가 이만큼이면 폴더 수와 무관하게 흘려보낸다
pub const FLUSH_HITS: usize = 500;
/// 한 폴더를 읽는 도중 멈춤 신호를 보는 간격(항목 수)
const READ_BATCH: usize = 2000;

/// 이름이 찾는 글자를 담는가 — 대소문자를 무시한 부분 일치다 (FR-65·FR-70).
///
/// 목록 필터(`ui::file_list`)와 검색 워커가 **이 한 규칙**을 함께 쓴다.
/// `needle_lower`는 이미 소문자로 접은 값이고, 비어 있으면 모두 걸린다
pub fn name_matches(name: &str, needle_lower: &str) -> bool {
    needle_lower.is_empty() || name.to_lowercase().contains(needle_lower)
}

/// 보이지 않는 항목을 검색에서 뺄지 — 목록의 숨김·시스템 표시 설정 그대로다 (FR-13)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchRules {
    pub show_hidden: bool,
    pub show_system: bool,
}

impl SearchRules {
    /// 이 속성의 항목이 목록에 보이는가 — `ui::file_list`의 숨김 판정과 같은 논리곱이다
    fn shows(self, attributes: u32) -> bool {
        (self.show_hidden || attributes & FILE_ATTRIBUTE_HIDDEN.0 == 0)
            && (self.show_system || attributes & FILE_ATTRIBUTE_SYSTEM.0 == 0)
    }
}

/// 검색이 흘려보내는 것
#[derive(Debug)]
pub enum SearchEvent {
    /// 새로 찾은 항목 — `location`이 뿌리 기준 상대 폴더다
    Hits(Vec<FileEntry>),
    /// 다 훑었거나 상한에 닿았다. `truncated`면 상한에서 끊긴 것이다
    Done { dirs: usize, truncated: bool },
}

/// `root` 아래를 모두 훑어 이름이 `needle_lower`를 담는 항목을 찾는다.
///
/// - 재분석 지점(정션·심볼릭 링크) 폴더로는 내려가지 않는다 — 정션이 위 폴더를 가리키면 끝없이 돈다
/// - `rules`가 숨기는 항목은 결과에 넣지 않고 그런 폴더로 내려가지도 않는다
/// - 결과가 `limit`에 닿으면 거기서 멈추고 `Done { truncated: true }`를 보낸다
/// - `should_stop`이 참이거나 `on_event`가 거짓이면(받는 쪽이 사라졌다) `Done` 없이 멈춘다
/// - 열지 못한 하위 폴더(권한 없음 등)는 건너뛴다
pub fn search_tree(
    root: &Path,
    needle_lower: &str,
    rules: SearchRules,
    limit: usize,
    should_stop: impl Fn() -> bool,
    mut on_event: impl FnMut(SearchEvent) -> bool,
) {
    // 아직 훑지 않은 폴더 — 뿌리 기준 상대 경로이며 뿌리 자신은 `None`이다
    let mut pending_dirs: Vec<Option<Arc<Path>>> = vec![None];
    let mut hits: Vec<FileEntry> = Vec::new();
    let (mut dirs, mut found) = (0usize, 0usize);
    while let Some(location) = pending_dirs.pop() {
        let dir = match &location {
            Some(sub) => root.join(sub),
            None => root.to_path_buf(),
        };
        let mut stopped = false;
        let mut full = false;
        enumerate_dir_batched(&dir, READ_BATCH, |chunk| {
            if should_stop() {
                stopped = true;
                return false;
            }
            let entries = match chunk {
                EnumChunk::Partial(entries) | EnumChunk::Done(EnumOutcome::Ok(entries)) => entries,
                EnumChunk::Done(_) => return true,
            };
            for mut entry in entries {
                if !rules.shows(entry.attributes) {
                    continue;
                }
                let name = entry.name_string();
                if entry.is_dir && entry.attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0 {
                    let child = match &location {
                        Some(sub) => sub.join(&name),
                        None => PathBuf::from(&name),
                    };
                    pending_dirs.push(Some(Arc::from(child)));
                }
                if name_matches(&name, needle_lower) {
                    entry.location = location.clone();
                    hits.push(entry);
                    found += 1;
                    if found >= limit {
                        full = true;
                        return false;
                    }
                }
            }
            true
        });
        if stopped {
            return;
        }
        dirs += 1;
        if full {
            if !hits.is_empty() && !on_event(SearchEvent::Hits(std::mem::take(&mut hits))) {
                return;
            }
            on_event(SearchEvent::Done {
                dirs,
                truncated: true,
            });
            return;
        }
        if (dirs % FLUSH_DIRS == 0 || hits.len() >= FLUSH_HITS)
            && !hits.is_empty()
            && !on_event(SearchEvent::Hits(std::mem::take(&mut hits)))
        {
            return;
        }
    }
    if !hits.is_empty() && !on_event(SearchEvent::Hits(hits)) {
        return;
    }
    on_event(SearchEvent::Done {
        dirs,
        truncated: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use windows::Win32::Storage::FileSystem::SetFileAttributesW;
    use windows::core::{HSTRING, PCWSTR};

    fn make_temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fe_search_test_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn set_hidden(path: &Path) {
        let wide = HSTRING::from(path.as_os_str());
        // 안전성: 널 종단 경로와 속성 상수를 넘기는 단순 호출이다
        unsafe {
            SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_HIDDEN).expect("속성 설정");
        }
    }

    /// 시험 나무 — 뿌리·하위·깊은 하위·숨김 폴더에 `alpha`가 든 이름을 하나씩 둔다
    fn tree(name: &str) -> PathBuf {
        let root = make_temp_dir(name);
        std::fs::write(root.join("Alpha.txt"), b"x").unwrap();
        std::fs::write(root.join("other.txt"), b"x").unwrap();
        std::fs::create_dir_all(root.join(r"sub\deep\alpha_dir")).unwrap();
        std::fs::write(root.join(r"sub\beta_ALPHA.log"), b"x").unwrap();
        std::fs::write(root.join(r"sub\deep\gamma.txt"), b"x").unwrap();
        std::fs::create_dir(root.join("hid")).unwrap();
        std::fs::write(root.join(r"hid\alpha_hidden.txt"), b"x").unwrap();
        set_hidden(&root.join("hid"));
        root
    }

    /// 검색을 끝까지 돌려 (상대 경로 목록, 완료 사건)을 돌려준다
    fn run(
        root: &Path,
        needle: &str,
        rules: SearchRules,
        limit: usize,
    ) -> (Vec<String>, Option<(usize, bool)>) {
        let mut hits = Vec::new();
        let mut done = None;
        search_tree(
            root,
            needle,
            rules,
            limit,
            || false,
            |event| {
                match event {
                    SearchEvent::Hits(entries) => {
                        hits.extend(entries.into_iter().map(|entry| match &entry.location {
                            Some(sub) => {
                                sub.join(entry.name_string()).to_string_lossy().into_owned()
                            }
                            None => entry.name_string(),
                        }))
                    }
                    SearchEvent::Done { dirs, truncated } => done = Some((dirs, truncated)),
                }
                true
            },
        );
        hits.sort();
        (hits, done)
    }

    const VISIBLE: SearchRules = SearchRules {
        show_hidden: false,
        show_system: false,
    };

    #[test]
    fn 하위_폴더까지_대소문자를_무시하고_찾고_상대_폴더를_싣는다() {
        let root = tree("basic");
        let (hits, done) = run(&root, "alpha", VISIBLE, usize::MAX);
        assert_eq!(
            hits,
            vec![
                "Alpha.txt".to_owned(),
                r"sub\beta_ALPHA.log".to_owned(),
                r"sub\deep\alpha_dir".to_owned(),
            ]
        );
        let Some((dirs, truncated)) = done else {
            panic!("완료 사건이 없다");
        };
        assert!(!truncated);
        // 뿌리·sub·deep·alpha_dir — 숨긴 폴더는 훑지 않는다
        assert_eq!(dirs, 4);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 숨김을_보이면_숨긴_폴더_안도_찾는다() {
        let root = tree("hidden_on");
        let rules = SearchRules {
            show_hidden: true,
            show_system: false,
        };
        let (hits, _) = run(&root, "alpha", rules, usize::MAX);
        assert!(
            hits.contains(&r"hid\alpha_hidden.txt".to_owned()),
            "{hits:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 정션은_따라가지_않는다() {
        let root = tree("junction");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(root.join("link"))
            .arg(root.join("sub"))
            .output()
            .unwrap();
        assert!(status.status.success(), "정션을 만들지 못했다");
        let (hits, _) = run(&root, "beta", VISIBLE, usize::MAX);
        // 정션을 따라가면 `link\beta_ALPHA.log`가 한 번 더 나온다
        assert_eq!(hits, vec![r"sub\beta_ALPHA.log".to_owned()]);
        let _ = std::fs::remove_dir(root.join("link"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 상한에_닿으면_끊고_잘렸다고_알린다() {
        let root = tree("limit");
        let (hits, done) = run(&root, "alpha", VISIBLE, 2);
        assert_eq!(hits.len(), 2);
        assert_eq!(done.map(|(_, truncated)| truncated), Some(true));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 멈추라면_결과도_완료도_보내지_않는다() {
        let root = tree("stop");
        let events = Cell::new(0);
        search_tree(
            &root,
            "alpha",
            VISIBLE,
            usize::MAX,
            || true,
            |_| {
                events.set(events.get() + 1);
                true
            },
        );
        assert_eq!(events.get(), 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 걸리는_것이_없으면_빈_채로_끝난다() {
        let root = tree("none");
        let (hits, done) = run(&root, "zzz_없음", VISIBLE, usize::MAX);
        assert!(hits.is_empty());
        assert_eq!(done.map(|(_, truncated)| truncated), Some(false));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 이름_일치는_대소문자를_무시한_부분_일치다() {
        assert!(name_matches("Report_FINAL.docx", "final"));
        assert!(name_matches("아무거나", ""));
        assert!(!name_matches("report.docx", "final"));
    }
}
