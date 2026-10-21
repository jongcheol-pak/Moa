//! 빠른 이동 팔레트(`Ctrl+P`, FR-75)의 후보와 순위.
//!
//! 화면(`ui::palette`)은 입력과 그리기만 하고, 무엇을 후보로 세우고 어떤 차례로 보일지는 여기서
//! 정한다 — `ExplorerApp` 은 시험에서 만들 수 없어 규칙을 그 안에 두면 덮을 길이 없다.
//! 탭은 `panel::tabs::TabId` 대신 자리 번호로 가리킨다 — 이 계층은 `panel` 을 참조하지 않는다.
use std::path::{Path, PathBuf};

use crate::app::layout::PanelId;
use crate::app::recent::same_path;
use crate::app::workspace::WorkspaceId;
use crate::fs::search::name_matches;

/// 한 번에 보이는 후보 줄 수 상한
pub const MAX_SHOWN: usize = 50;

/// 후보가 어디서 왔는가 — 질의가 비었을 때 이 차례로 선다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Recent,
    Favorite,
    Tab,
    Workspace,
}

/// 고르면 갈 곳
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// 로컬 폴더 — 최근 경로·즐겨찾기
    Path(PathBuf),
    /// 이번 실행에서 연 워크스페이스의 로컬 탭 — `index` 는 그 패널 안의 탭 자리
    Tab {
        workspace: WorkspaceId,
        panel: PanelId,
        index: usize,
    },
    Workspace(WorkspaceId),
}

/// 팔레트 한 줄
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub source: Source,
    /// 굵게 보이는 이름 — 순위의 「이름」 이 이것이다
    pub label: String,
    /// 이름 옆의 흐린 설명 — 경로(탭은 「워크스페이스 · 경로」)
    pub detail: String,
    pub target: Target,
}

/// 열린 로컬 탭 하나 — 앱이 뷰를 훑어 넘긴다
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabInfo {
    pub workspace: WorkspaceId,
    pub workspace_name: String,
    pub panel: PanelId,
    pub index: usize,
    pub path: PathBuf,
}

/// 네 출처를 한 목록으로 잇는다 — 최근 → 즐겨찾기 → 열린 탭 → 워크스페이스.
///
/// 즐겨찾기와 같은 곳을 가리키는 최근 경로는 뺀다 — 즐겨찾기 이름(바탕 화면 등)이 더 읽기 쉽다.
/// `favorites` 는 `(경로, 표시 이름)` 이며 이름이 없으면 폴더명을 쓴다
pub fn collect(
    recent: &[PathBuf],
    favorites: &[(PathBuf, Option<String>)],
    tabs: &[TabInfo],
    workspaces: &[(WorkspaceId, String)],
) -> Vec<Candidate> {
    let mut out = Vec::new();
    for path in recent {
        if favorites.iter().any(|(fav, _)| same_path(fav, path)) {
            continue;
        }
        out.push(Candidate {
            source: Source::Recent,
            label: folder_name(path),
            detail: display(path),
            target: Target::Path(path.clone()),
        });
    }
    for (path, label) in favorites {
        out.push(Candidate {
            source: Source::Favorite,
            label: label.clone().unwrap_or_else(|| folder_name(path)),
            detail: display(path),
            target: Target::Path(path.clone()),
        });
    }
    for tab in tabs {
        out.push(Candidate {
            source: Source::Tab,
            label: folder_name(&tab.path),
            detail: format!("{} · {}", tab.workspace_name, display(&tab.path)),
            target: Target::Tab {
                workspace: tab.workspace,
                panel: tab.panel,
                index: tab.index,
            },
        });
    }
    for (id, name) in workspaces {
        out.push(Candidate {
            source: Source::Workspace,
            label: name.clone(),
            detail: String::new(),
            target: Target::Workspace(*id),
        });
    }
    out
}

/// 질의에 걸리는 후보의 자리들을 보일 차례로 — 최대 `MAX_SHOWN` 줄.
///
/// 대소문자를 무시한 부분 일치(목록 필터와 같은 규칙)로 거르고, 이름 앞 일치 > 이름 포함 >
/// 설명(경로) 포함 순으로 세운다. 같은 등급은 `collect` 의 차례를 지킨다. 질의가 비면 그 차례 그대로다
pub fn rank(candidates: &[Candidate], query: &str) -> Vec<usize> {
    let needle = query.trim().to_lowercase();
    let mut hits: Vec<(u8, usize)> = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| tier(candidate, &needle).map(|tier| (tier, index)))
        .collect();
    // 안정 정렬이라 같은 등급 안에서는 원래 차례가 남는다
    hits.sort_by_key(|(tier, _)| *tier);
    hits.into_iter()
        .take(MAX_SHOWN)
        .map(|(_, index)| index)
        .collect()
}

/// 걸리면 등급(작을수록 앞), 안 걸리면 `None`
fn tier(candidate: &Candidate, needle: &str) -> Option<u8> {
    if needle.is_empty() {
        return Some(0);
    }
    let label = candidate.label.to_lowercase();
    if label.starts_with(needle) {
        Some(0)
    } else if label.contains(needle) {
        Some(1)
    } else if name_matches(&candidate.detail, needle) {
        Some(2)
    } else {
        None
    }
}

/// 폴더명 — 드라이브 뿌리(`C:\`)처럼 이름이 없으면 경로 그대로다
fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| display(path))
}

fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn tab(path: &str) -> TabInfo {
        TabInfo {
            workspace: WorkspaceId(1),
            workspace_name: "작업".to_owned(),
            panel: PanelId(2),
            index: 3,
            path: p(path),
        }
    }

    fn sample() -> Vec<Candidate> {
        collect(
            &[p(r"C:\프로젝트\moa"), p(r"D:\사진")],
            &[(p(r"C:\Users\me\Desktop"), Some("바탕 화면".to_owned()))],
            &[tab(r"E:\문서")],
            &[(WorkspaceId(7), "기본".to_owned())],
        )
    }

    fn labels(candidates: &[Candidate], order: &[usize]) -> Vec<String> {
        order.iter().map(|&i| candidates[i].label.clone()).collect()
    }

    #[test]
    fn 네_출처가_모두_담긴다() {
        let all = sample();
        let sources: Vec<Source> = all.iter().map(|c| c.source).collect();
        assert_eq!(
            sources,
            vec![
                Source::Recent,
                Source::Recent,
                Source::Favorite,
                Source::Tab,
                Source::Workspace
            ]
        );
        assert_eq!(
            all[3].target,
            Target::Tab {
                workspace: WorkspaceId(1),
                panel: PanelId(2),
                index: 3
            }
        );
        assert_eq!(all[4].target, Target::Workspace(WorkspaceId(7)));
        assert_eq!(
            all[2].label, "바탕 화면",
            "즐겨찾기 표시 이름을 쓰지 않았다"
        );
    }

    #[test]
    fn 빈_질의는_출처_차례다() {
        let all = sample();
        assert_eq!(rank(&all, ""), vec![0, 1, 2, 3, 4]);
        assert_eq!(
            rank(&all, "   "),
            vec![0, 1, 2, 3, 4],
            "공백만인 질의도 빈 질의다"
        );
    }

    #[test]
    fn 즐겨찾기와_같은_최근_경로는_뺀다() {
        let all = collect(
            &[p(r"c:\users\me\desktop"), p(r"D:\사진")],
            &[(p(r"C:\Users\me\Desktop"), Some("바탕 화면".to_owned()))],
            &[],
            &[],
        );
        assert_eq!(labels(&all, &rank(&all, "")), vec!["사진", "바탕 화면"]);
    }

    #[test]
    fn 이름_앞_일치가_이름_포함보다_앞이다() {
        let all = collect(&[p(r"C:\옛moa"), p(r"C:\moa새")], &[], &[], &[]);
        // 차례로는 「옛moa」 가 앞이지만, 이름이 질의로 시작하는 「moa새」 가 먼저 선다
        assert_eq!(labels(&all, &rank(&all, "moa")), vec!["moa새", "옛moa"]);
    }

    #[test]
    fn 경로에만_있는_글자도_찾는다() {
        let all = sample();
        // 「프로젝트」 는 moa 의 상위 폴더 이름이다 — 이름에는 없고 경로에만 있다
        assert_eq!(labels(&all, &rank(&all, "프로젝트")), vec!["moa"]);
        // 이름 일치가 경로 일치보다 앞이다
        let all = collect(&[p(r"C:\작업\x"), p(r"C:\y\작업")], &[], &[], &[]);
        assert_eq!(labels(&all, &rank(&all, "작업")), vec!["작업", "x"]);
    }

    #[test]
    fn 대소문자를_가리지_않는다() {
        let all = sample();
        assert_eq!(labels(&all, &rank(&all, "MOA")), vec!["moa"]);
    }

    #[test]
    fn 걸리는_것이_없으면_비었다() {
        assert!(rank(&sample(), "없는이름").is_empty());
    }

    #[test]
    fn 오십_줄을_넘지_않는다() {
        let many: Vec<PathBuf> = (0..MAX_SHOWN + 10)
            .map(|i| p(&format!(r"C:\d{i}")))
            .collect();
        let all = collect(&many, &[], &[], &[]);
        assert_eq!(rank(&all, "").len(), MAX_SHOWN);
        assert_eq!(rank(&all, "d").len(), MAX_SHOWN);
    }
}
