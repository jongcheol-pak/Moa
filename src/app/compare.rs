//! 두 패널 비교 판정 (FR-79) — 두 폴더 목록을 견줘 한쪽에만 있는 것과 다른 것을 가른다.
//!
//! 그리기·메뉴와 갈라 둔 순수 규칙이다. 화면은 이 결과를 이름으로 찾아 행을 칠한다.
use crate::fs::enumerate::FileEntry;
use std::collections::HashMap;

/// 수정일이 이보다 크게 차이 나야 「다름」이다 — 2초 (FILETIME 100ns 틱).
///
/// FAT·zip 은 시각을 2초 단위로 적어, 같은 파일을 옮겨 담기만 해도 1초 남짓 어긋난다
pub const MODIFIED_SLACK: u64 = 20_000_000;

/// 비교 표시
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// 이쪽에만 있다
    OnlyHere,
    /// 양쪽에 있는데 다르다
    Differs,
}

/// 한쪽 목록의 비교 결과 — 이름 키(소문자로 접은 이름) → 표시. 표시가 없는 항목은 같다
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompareMarks {
    marks: HashMap<String, Mark>,
}

impl CompareMarks {
    /// 그 이름의 표시 — 같거나 견주지 않는 항목이면 `None`
    pub fn get(&self, name: &str) -> Option<Mark> {
        self.marks.get(&name.to_lowercase()).copied()
    }

    /// (한쪽에만, 다름) 개수
    pub fn counts(&self) -> (usize, usize) {
        let only = self
            .marks
            .values()
            .filter(|mark| **mark == Mark::OnlyHere)
            .count();
        (only, self.marks.len() - only)
    }
}

/// 이름 키 → 그 항목 — `..` 줄(목록이 얹는 상위 이동)은 뺀다
fn by_name(rows: &[FileEntry]) -> HashMap<String, &FileEntry> {
    rows.iter()
        .map(|row| (row.name_string(), row))
        .filter(|(name, _)| name != "..")
        .map(|(name, row)| (name.to_lowercase(), row))
        .collect()
}

/// 양쪽에 있는 같은 이름이 다른가
fn differs(a: &FileEntry, b: &FileEntry) -> bool {
    match (a.is_dir, b.is_dir) {
        (true, true) => false,
        (false, false) => a.size != b.size || a.modified.abs_diff(b.modified) > MODIFIED_SLACK,
        _ => true,
    }
}

/// 두 목록을 견준다 — (왼쪽 결과, 오른쪽 결과).
///
/// 이름은 대소문자를 가리지 않고 견준다(Windows 파일 시스템이 그렇다). 파일끼리는 크기가
/// 다르거나 수정일이 2초 넘게 차이 나면 다르고, 폴더끼리는 있음/없음만 본다(안을 훑지 않는다).
/// 같은 이름이 한쪽은 파일·한쪽은 폴더면 다르다. `..` 줄은 견주지 않는다
pub fn compare(left: &[FileEntry], right: &[FileEntry]) -> (CompareMarks, CompareMarks) {
    let left = by_name(left);
    let right = by_name(right);
    let side = |here: &HashMap<String, &FileEntry>, there: &HashMap<String, &FileEntry>| {
        let marks = here
            .iter()
            .filter_map(|(key, row)| {
                let mark = match there.get(key) {
                    None => Mark::OnlyHere,
                    Some(other) if differs(row, other) => Mark::Differs,
                    Some(_) => return None,
                };
                Some((key.clone(), mark))
            })
            .collect();
        CompareMarks { marks }
    };
    (side(&left, &right), side(&right, &left))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64, modified: u64) -> FileEntry {
        FileEntry {
            name: name.encode_utf16().chain(std::iter::once(0)).collect(),
            is_dir: false,
            size,
            modified,
            attributes: 0,
            location: None,
        }
    }

    fn dir(name: &str, modified: u64) -> FileEntry {
        FileEntry {
            is_dir: true,
            size: 0,
            ..file(name, 0, modified)
        }
    }

    const SEC: u64 = 10_000_000;

    #[test]
    fn 한쪽에만_있는_것을_가른다() {
        let (left, right) = compare(
            &[file("a.txt", 1, SEC), file("only-left.txt", 1, SEC)],
            &[file("a.txt", 1, SEC), file("only-right.txt", 1, SEC)],
        );
        assert_eq!(left.get("only-left.txt"), Some(Mark::OnlyHere));
        assert_eq!(right.get("only-right.txt"), Some(Mark::OnlyHere));
        assert_eq!(left.get("a.txt"), None);
        assert_eq!(right.get("a.txt"), None);
        assert_eq!(
            left.get("only-right.txt"),
            None,
            "반대쪽 것을 이쪽에 적었다"
        );
    }

    #[test]
    fn 이름은_대소문자를_무시한다() {
        let (left, right) = compare(&[file("Report.TXT", 1, SEC)], &[file("report.txt", 1, SEC)]);
        assert_eq!(left.get("Report.TXT"), None);
        assert_eq!(right.get("report.txt"), None);
    }

    #[test]
    fn 크기가_다르면_다름이다() {
        let (left, right) = compare(&[file("a.txt", 1, SEC)], &[file("a.txt", 2, SEC)]);
        assert_eq!(left.get("a.txt"), Some(Mark::Differs));
        assert_eq!(right.get("a.txt"), Some(Mark::Differs));
    }

    #[test]
    fn 수정일이_2초_넘게_차이나면_다름이다() {
        let (left, _) = compare(
            &[file("a.txt", 1, 100 * SEC)],
            &[file("a.txt", 1, 100 * SEC + 2 * SEC + 1)],
        );
        assert_eq!(left.get("a.txt"), Some(Mark::Differs));
        // 방향을 바꿔도 같다(빼는 차례로 넘치지 않는다)
        let (left, _) = compare(
            &[file("a.txt", 1, 100 * SEC + 3 * SEC)],
            &[file("a.txt", 1, 100 * SEC)],
        );
        assert_eq!(left.get("a.txt"), Some(Mark::Differs));
    }

    #[test]
    fn 수정일이_2초_안이면_같다() {
        let (left, right) = compare(
            &[file("a.txt", 1, 100 * SEC)],
            &[file("a.txt", 1, 100 * SEC + 2 * SEC)],
        );
        assert_eq!(left.get("a.txt"), None);
        assert_eq!(right.get("a.txt"), None);
    }

    #[test]
    fn 폴더는_있음과_없음만_본다() {
        let (left, right) = compare(
            &[dir("사진", SEC), dir("왼쪽만", SEC)],
            &[dir("사진", 999 * SEC)],
        );
        assert_eq!(left.get("사진"), None, "폴더 시각 차이를 다름으로 봤다");
        assert_eq!(right.get("사진"), None);
        assert_eq!(left.get("왼쪽만"), Some(Mark::OnlyHere));
    }

    #[test]
    fn 파일과_폴더가_같은_이름이면_다름이다() {
        let (left, right) = compare(&[file("x", 1, SEC)], &[dir("x", SEC)]);
        assert_eq!(left.get("x"), Some(Mark::Differs));
        assert_eq!(right.get("x"), Some(Mark::Differs));
    }

    #[test]
    fn 상위_줄은_견주지_않는다() {
        let (left, _) = compare(&[dir("..", 0), file("a.txt", 1, SEC)], &[]);
        assert_eq!(left.get(".."), None);
        assert_eq!(left.get("a.txt"), Some(Mark::OnlyHere));
    }

    #[test]
    fn 세기는_표시별로_센다() {
        let (left, right) = compare(
            &[
                file("same.txt", 1, SEC),
                file("diff.txt", 1, SEC),
                file("only1", 1, SEC),
                file("only2", 1, SEC),
            ],
            &[file("same.txt", 1, SEC), file("diff.txt", 9, SEC)],
        );
        assert_eq!(left.counts(), (2, 1));
        assert_eq!(right.counts(), (0, 1));
    }
}
