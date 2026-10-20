//! 앱 자체 되돌리기(`Ctrl+Z`)의 기록과 실행 직전 대조 (FR-74).
//!
//! 셸 작업 결과(`fs::file_op`)를 되돌릴 단계로 바꿔 담고, 되돌리기 직전에 그 단계가 아직
//! 성립하는지 대조한다. **파일시스템을 직접 묻지 않는다** — 묻는 자리는 [`Probe`]로 받아,
//! 시험은 가짜를, 실행 워커는 실제 파일시스템을 넣는다.
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::fs::file_op::Applied;

/// 항목 하나의 도장 — 기록한 뒤 내용이 바뀌었는지 가르는 값.
///
/// 파일은 크기와 수정 시각, 폴더는 수정 시각만 본다(폴더 크기는 0으로 둔다). **폴더의 수정
/// 시각은 바로 아래 항목이 바뀔 때만 바뀌어** 더 깊은 곳의 변경은 이 값이 못 본다 — 되돌리기가
/// 보내는 곳이 휴지통이라 되살릴 수 있어 받아들였다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

/// 지금 그 자리의 도장 — 없으면 `None`.
///
/// 링크는 따라가지 않는다(`symlink_metadata`) — 링크가 가리키는 쪽이 아니라 링크 자체가
/// 되돌릴 대상이다
pub fn stamp_of(path: &Path) -> Option<Stamp> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some(Stamp {
        len: if meta.is_dir() { 0 } else { meta.len() },
        modified: meta.modified().ok(),
    })
}

/// 기록을 몇 건까지 두는가 — 넘으면 가장 오래된 것부터 버린다 (FR-74 · 사용자 답 Q3)
pub const LIMIT: usize = 50;

/// 되돌릴 단계 하나 (plan D4)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoStep {
    /// 사본·새 항목을 휴지통으로 — `stamp`는 기록할 때의 도장이다
    Recycle { path: PathBuf, stamp: Option<Stamp> },
    /// 옮긴 것을 원래 자리로 — `from`은 지금 자리, `to`는 원래 자리(이름 포함)
    MoveBack { from: PathBuf, to: PathBuf },
    /// 바꾼 이름을 원래대로 — `path`는 지금 자리, `name`은 원래 이름
    RenameBack { path: PathBuf, name: String },
}

impl UndoStep {
    /// 이 단계가 지금 차지하고 있는 자리 — 되돌리면 비워진다
    pub fn current(&self) -> &Path {
        match self {
            UndoStep::Recycle { path, .. } | UndoStep::RenameBack { path, .. } => path,
            UndoStep::MoveBack { from, .. } => from,
        }
    }
}

/// 기록 한 건 — 사용자 조작 한 번이다(여러 항목이면 단계가 여럿)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoEntry {
    pub steps: Vec<UndoStep>,
}

/// 무엇을 한 결과인가 — 되돌리는 방식이 이것으로 갈린다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Done {
    /// 복사·붙여넣기(복사) — 사본을 휴지통으로
    Copied,
    /// 이동·붙여넣기(잘라내기) — 원래 자리로
    Moved,
    /// 이름 바꾸기·일괄 이름 바꾸기 — 원래 이름으로
    Renamed,
}

/// 셸 작업 결과를 기록 한 건으로 바꾼다 — 되돌릴 것이 없으면 `None` (plan D1·D6).
///
/// 작업 전부터 그 자리에 있던 것(`preexisting`)은 뺀다 — 되돌리면 원래 있던 것까지 잃는다
pub fn entry_for(done: Done, applied: &[Applied]) -> Option<UndoEntry> {
    let steps: Vec<UndoStep> = applied
        .iter()
        .filter(|one| !one.preexisting)
        .filter_map(|one| match done {
            Done::Copied => Some(UndoStep::Recycle {
                path: one.to.clone(),
                stamp: one.stamp,
            }),
            Done::Moved => Some(UndoStep::MoveBack {
                from: one.to.clone(),
                to: one.from.clone(),
            }),
            Done::Renamed => one.from.file_name().map(|name| UndoStep::RenameBack {
                path: one.to.clone(),
                name: name.to_string_lossy().into_owned(),
            }),
        })
        .collect();
    (!steps.is_empty()).then_some(UndoEntry { steps })
}

/// 새 폴더·새 파일을 기록 한 건으로 바꾼다 — `\\?\` 접두는 뗀다 (plan D13)
pub fn entry_for_created(path: &Path, stamp: Option<Stamp>) -> UndoEntry {
    UndoEntry {
        steps: vec![UndoStep::Recycle {
            path: plain_path(path),
            stamp,
        }],
    }
}

/// 확장 접두(`\\?\`·`\\?\UNC\`)를 뗀 경로 — 셸 항목 이름 해석에는 일반 경로를 준다
fn plain_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

/// 최근 기록 — 메모리에만 둔다(사용자 답 Q3)
#[derive(Debug, Default)]
pub struct UndoLog {
    entries: VecDeque<UndoEntry>,
}

impl UndoLog {
    pub fn push(&mut self, entry: UndoEntry) {
        if self.entries.len() == LIMIT {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    /// 가장 최근 기록을 꺼낸다
    pub fn pop(&mut self) -> Option<UndoEntry> {
        self.entries.pop_back()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// 대조가 파일시스템에 묻는 것 — 시험은 가짜를 넣는다
pub trait Probe {
    /// 그 자리의 도장 — 없으면 `None`
    fn stamp(&self, path: &Path) -> Option<Stamp>;
    /// 네트워크 너머인가 — 휴지통이 없어 셸이 영구 삭제를 묻는 자리다
    fn is_network(&self, path: &Path) -> bool;
}

/// 실제 파일시스템 — 되돌리기 워커가 쓴다
pub struct Disk;

impl Probe for Disk {
    fn stamp(&self, path: &Path) -> Option<Stamp> {
        stamp_of(path)
    }

    fn is_network(&self, path: &Path) -> bool {
        crate::fs::drives::is_network_path(path)
    }
}

/// 대조를 지난 단계와 건너뛴 수
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub steps: Vec<UndoStep>,
    pub skipped: usize,
}

/// 되돌리기 직전에 각 단계가 아직 성립하는지 대조한다 (plan D4 · 사용자 답 Q6).
///
/// 어긋난 단계는 그것만 건너뛰고 수를 센다. 「원래 자리가 비었는가」에서 **같은 기록의 다른
/// 단계가 비워 줄 자리**와 **대소문자만 다른 자기 자리**는 빈 것으로 본다
pub fn check(steps: &[UndoStep], probe: &impl Probe) -> Checked {
    let vacated: Vec<&Path> = steps.iter().map(UndoStep::current).collect();
    // 돌아갈 자리가 비었는가 — 같은 기록이 비워 줄 자리·대소문자만 다른 자기 자리는 빈 것이다
    let free = |target: &Path, me: &Path| {
        same_path(target, me)
            || vacated.iter().any(|path| same_path(path, target))
            || probe.stamp(target).is_none()
    };
    let passing: Vec<UndoStep> = steps
        .iter()
        .filter(|step| match step {
            UndoStep::Recycle { path, stamp } => {
                !probe.is_network(path) && stamp.is_some() && probe.stamp(path) == *stamp
            }
            UndoStep::MoveBack { from, to } => probe.stamp(from).is_some() && free(to, from),
            UndoStep::RenameBack { path, name } => {
                probe.stamp(path).is_some() && free(&path.with_file_name(name), path)
            }
        })
        .cloned()
        .collect();
    Checked {
        skipped: steps.len() - passing.len(),
        steps: passing,
    }
}

/// 두 경로가 같은 자리인가 — ASCII 대소문자를 접어 견준다(Windows 파일시스템 규칙)
fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// 가짜 파일시스템 — 경로(소문자)마다 도장
    #[derive(Default)]
    struct 가짜 {
        있는것: HashMap<String, Stamp>,
        네트워크: Vec<String>,
    }

    impl 가짜 {
        fn 둔다(mut self, path: &str, stamp: Stamp) -> 가짜 {
            self.있는것.insert(path.to_ascii_lowercase(), stamp);
            self
        }
    }

    impl Probe for 가짜 {
        fn stamp(&self, path: &Path) -> Option<Stamp> {
            self.있는것
                .get(&path.to_string_lossy().to_ascii_lowercase())
                .copied()
        }
        fn is_network(&self, path: &Path) -> bool {
            let text = path.to_string_lossy().to_ascii_lowercase();
            self.네트워크.iter().any(|prefix| text.starts_with(prefix))
        }
    }

    fn 도장(len: u64) -> Stamp {
        Stamp {
            len,
            modified: None,
        }
    }

    fn 적용(from: &str, to: &str, preexisting: bool) -> Applied {
        Applied {
            from: PathBuf::from(from),
            to: PathBuf::from(to),
            stamp: Some(도장(1)),
            preexisting,
        }
    }

    fn 한건(n: usize) -> UndoEntry {
        UndoEntry {
            steps: vec![UndoStep::Recycle {
                path: PathBuf::from(format!(r"C:\w\{n}.txt")),
                stamp: None,
            }],
        }
    }

    #[test]
    fn 오십_건을_넘으면_가장_오래된_것을_버린다() {
        let mut log = UndoLog::default();
        for n in 0..LIMIT + 3 {
            log.push(한건(n));
        }
        assert_eq!(log.len(), LIMIT);
        assert_eq!(
            log.pop(),
            Some(한건(LIMIT + 2)),
            "가장 최근 것이 먼저 나온다"
        );
        let mut 남은 = Vec::new();
        while let Some(entry) = log.pop() {
            남은.push(entry);
        }
        assert_eq!(남은.last(), Some(&한건(3)), "앞의 셋이 버려졌다");
    }

    #[test]
    fn 결과에서_되돌릴_단계를_만든다() {
        let 복사 = entry_for(Done::Copied, &[적용(r"C:\a\x.txt", r"C:\b\x.txt", false)]);
        assert_eq!(
            복사.map(|entry| entry.steps),
            Some(vec![UndoStep::Recycle {
                path: PathBuf::from(r"C:\b\x.txt"),
                stamp: Some(도장(1)),
            }])
        );
        let 이동 = entry_for(
            Done::Moved,
            &[적용(r"C:\a\x.txt", r"C:\b\x (2).txt", false)],
        );
        assert_eq!(
            이동.map(|entry| entry.steps),
            Some(vec![UndoStep::MoveBack {
                from: PathBuf::from(r"C:\b\x (2).txt"),
                to: PathBuf::from(r"C:\a\x.txt"),
            }])
        );
        let 이름 = entry_for(Done::Renamed, &[적용(r"C:\a\x.txt", r"C:\a\y.txt", false)]);
        assert_eq!(
            이름.map(|entry| entry.steps),
            Some(vec![UndoStep::RenameBack {
                path: PathBuf::from(r"C:\a\y.txt"),
                name: "x.txt".to_owned(),
            }])
        );
        let 새것 = entry_for_created(Path::new(r"\\?\C:\a\새 폴더"), Some(도장(0)));
        assert_eq!(
            새것.steps,
            vec![UndoStep::Recycle {
                path: PathBuf::from(r"C:\a\새 폴더"),
                stamp: Some(도장(0)),
            }],
            "확장 접두를 뗀다"
        );
    }

    #[test]
    fn 덮어쓴_자리는_기록하지_않는다() {
        let entry = entry_for(
            Done::Copied,
            &[
                적용(r"C:\a\x.txt", r"C:\b\x.txt", true),
                적용(r"C:\a\y.txt", r"C:\b\y.txt", false),
            ],
        );
        assert_eq!(entry.map(|entry| entry.steps.len()), Some(1));
    }

    #[test]
    fn 결과가_없으면_기록하지_않는다() {
        assert_eq!(entry_for(Done::Copied, &[]), None);
        assert_eq!(
            entry_for(Done::Moved, &[적용(r"C:\a\x", r"C:\b\x", true)]),
            None,
            "전부 덮어쓴 자리면 기록할 것이 없다"
        );
    }

    #[test]
    fn 사라진_대상과_찬_자리는_건너뛴다() {
        let 디스크 = 가짜::default()
            .둔다(r"C:\b\x.txt", 도장(1))
            .둔다(r"C:\a\x.txt", 도장(9))
            .둔다(r"C:\b\y.txt", 도장(1));
        let 단계 = vec![
            // 원래 자리가 다른 것으로 찼다
            UndoStep::MoveBack {
                from: PathBuf::from(r"C:\b\x.txt"),
                to: PathBuf::from(r"C:\a\x.txt"),
            },
            // 옮긴 것이 사라졌다
            UndoStep::MoveBack {
                from: PathBuf::from(r"C:\b\gone.txt"),
                to: PathBuf::from(r"C:\a\gone.txt"),
            },
            // 이건 된다
            UndoStep::MoveBack {
                from: PathBuf::from(r"C:\b\y.txt"),
                to: PathBuf::from(r"C:\a\y.txt"),
            },
        ];
        let 결과 = check(&단계, &디스크);
        assert_eq!(결과.skipped, 2);
        assert_eq!(결과.steps, vec![단계[2].clone()]);
    }

    #[test]
    fn 내용이_바뀐_사본은_휴지통으로_보내지_않는다() {
        let 디스크 = 가짜::default()
            .둔다(r"C:\b\x.txt", 도장(5))
            .둔다(r"C:\b\y.txt", 도장(1));
        let 단계 = vec![
            UndoStep::Recycle {
                path: PathBuf::from(r"C:\b\x.txt"),
                stamp: Some(도장(1)),
            },
            UndoStep::Recycle {
                path: PathBuf::from(r"C:\b\y.txt"),
                stamp: Some(도장(1)),
            },
        ];
        let 결과 = check(&단계, &디스크);
        assert_eq!(결과.skipped, 1);
        assert_eq!(결과.steps, vec![단계[1].clone()]);
    }

    #[test]
    fn 같은_묶음이_비워_줄_자리는_빈_것으로_본다() {
        // a↔b 맞바꿈을 되돌린다 — 서로의 원래 이름 자리를 상대가 비워 준다
        let 디스크 = 가짜::default()
            .둔다(r"C:\w\a.txt", 도장(1))
            .둔다(r"C:\w\b.txt", 도장(1));
        let 단계 = vec![
            UndoStep::RenameBack {
                path: PathBuf::from(r"C:\w\b.txt"),
                name: "a.txt".to_owned(),
            },
            UndoStep::RenameBack {
                path: PathBuf::from(r"C:\w\a.txt"),
                name: "b.txt".to_owned(),
            },
        ];
        let 결과 = check(&단계, &디스크);
        assert_eq!(결과.skipped, 0);
        assert_eq!(결과.steps.len(), 2);
    }

    #[test]
    fn 대소문자만_다른_자기_자리는_빈_것으로_본다() {
        let 디스크 = 가짜::default().둔다(r"C:\w\A.txt", 도장(1));
        let 단계 = vec![UndoStep::RenameBack {
            path: PathBuf::from(r"C:\w\A.txt"),
            name: "a.txt".to_owned(),
        }];
        assert_eq!(check(&단계, &디스크).skipped, 0);
    }

    #[test]
    fn 네트워크_경로의_휴지통_단계는_건너뛴다() {
        let mut 디스크 = 가짜::default().둔다(r"\\srv\share\x.txt", 도장(1));
        디스크.네트워크.push(r"\\srv".to_owned());
        let 단계 = vec![UndoStep::Recycle {
            path: PathBuf::from(r"\\srv\share\x.txt"),
            stamp: Some(도장(1)),
        }];
        let 결과 = check(&단계, &디스크);
        assert_eq!(결과.skipped, 1, "영구 삭제로 새는 길을 만들지 않는다");
        assert!(결과.steps.is_empty());
    }
}
