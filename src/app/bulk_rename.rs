//! 일괄 이름 바꾸기의 규칙 — 새 이름을 짓고 막을 사유를 가린다 (FR-73).
//!
//! 화면을 모르는 순수 로직이다 — 대화(`ui::bulk_rename_dialog`)가 입력을 받아 여기서 미리보기를
//! 얻고, 적용할 `(경로, 새 이름)` 목록도 여기서 받는다. 충돌 판정에 쓰는 폴더 안 이름 목록은
//! 대화가 워커로 모아 넘긴다(숨김·필터로 안 보이는 항목까지 디스크 기준).
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// 이름을 짓는 방식 셋 (사용자 답 — 찾기/바꾸기·접두/접미+연번·정규식)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    /// `find`를 모두 `with`로 — 대소문자를 가린다. `find`가 비면 바꾸지 않는다
    Replace { find: String, with: String },
    /// `접두 + (원래 이름) + 번호 + 접미` — 번호는 항목 차례대로 `start`부터 `digits`자리
    Number {
        prefix: String,
        suffix: String,
        start: u32,
        digits: u32,
        keep_name: bool,
    },
    /// 정규식 바꾸기 — `with`에 `$1` 같은 묶음 참조를 쓴다
    Regex { pattern: String, with: String },
}

impl Rule {
    /// 연번 방식의 기본값 — 시작 1·자릿수 1 (사용자 답 Q5)
    pub fn number_default() -> Rule {
        Rule::Number {
            prefix: String::new(),
            suffix: String::new(),
            start: 1,
            digits: 1,
            keep_name: false,
        }
    }
}

/// 바꿀 항목 하나
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub path: PathBuf,
    pub is_dir: bool,
}

/// 적용을 막는 사유 — 하나라도 있으면 적용할 수 없다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    /// 새 이름이 비었다
    Empty,
    /// 쓸 수 없는 글자가 있다
    InvalidChars,
    /// 같은 폴더의 다른 새 이름과 겹친다
    Duplicate,
    /// 폴더에 이미 있는 이름과 겹친다
    Exists,
    /// 정규식이 잘못됐다
    BadPattern,
}

/// 미리보기 표의 한 줄 — 현재 → 새 이름
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub from: PathBuf,
    pub to: String,
    pub block: Option<Block>,
}

impl Row {
    /// 이름이 그대로인가 — 적용 대상에서 빠진다
    pub fn unchanged(&self) -> bool {
        self.from
            .file_name()
            .is_some_and(|name| name.to_string_lossy() == self.to)
    }
}

/// 폴더별 이름 목록의 열쇠 — 대소문자를 접은 경로 문자열
pub fn folder_key(dir: &Path) -> String {
    dir.to_string_lossy().to_lowercase()
}

/// 폴더 안 이름 목록 — 열쇠는 [`folder_key`], 값은 소문자로 접은 이름들
pub type FolderNames = HashMap<String, HashSet<String>>;

/// 규칙으로 새 이름을 짓고 막을 사유를 가린다.
///
/// `include_ext`가 거짓이면 파일의 마지막 점 뒤(확장자)는 그대로 두고 앞부분만 바꾼다 —
/// 폴더는 이름 전체가 대상이다
pub fn preview(items: &[Item], rule: &Rule, include_ext: bool, names: &FolderNames) -> Vec<Row> {
    let pattern = match rule {
        Rule::Regex { pattern, .. } => match regex::Regex::new(pattern) {
            Ok(compiled) => Some(compiled),
            Err(_) => {
                return items
                    .iter()
                    .map(|item| Row {
                        from: item.path.clone(),
                        to: name_of(&item.path),
                        block: Some(Block::BadPattern),
                    })
                    .collect();
            }
        },
        _ => None,
    };
    let mut rows: Vec<Row> = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let current = name_of(&item.path);
            let (stem, ext) = split_name(&current, item.is_dir || include_ext);
            let new_stem = match rule {
                Rule::Replace { find, with } if !find.is_empty() => stem.replace(find, with),
                Rule::Replace { .. } => stem.to_owned(),
                Rule::Number {
                    prefix,
                    suffix,
                    start,
                    digits,
                    keep_name,
                } => {
                    let number = u64::from(*start) + index as u64;
                    let kept = if *keep_name { stem } else { "" };
                    format!(
                        "{prefix}{kept}{number:0width$}{suffix}",
                        width = *digits as usize
                    )
                }
                Rule::Regex { with, .. } => pattern
                    .as_ref()
                    .map(|compiled| compiled.replace_all(stem, with.as_str()).into_owned())
                    .unwrap_or_else(|| stem.to_owned()),
            };
            // 확장자만 남는 이름(`.txt`)도 빈 이름이다 — 앞부분을 지우려던 조작이 아니다
            let empty = new_stem.trim().is_empty() && current != format!("{new_stem}{ext}");
            Row {
                from: item.path.clone(),
                to: format!("{new_stem}{ext}"),
                block: empty.then_some(Block::Empty),
            }
        })
        .collect();
    let blocks: Vec<Option<Block>> = (0..rows.len())
        .map(|index| rows[index].block.or_else(|| block_of(&rows, index, names)))
        .collect();
    for (row, block) in rows.iter_mut().zip(blocks) {
        row.block = block;
    }
    rows
}

/// 이름을 (바꿀 부분, 그대로 둘 확장자)로 가른다 — `whole`이면 전체가 바꿀 부분이다.
///
/// 점으로 시작하는 이름(`.gitignore`)은 확장자가 없는 것으로 본다(탐색기와 같다)
fn split_name(name: &str, whole: bool) -> (&str, &str) {
    if whole {
        return (name, "");
    }
    match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name, ""),
    }
}

/// 한 줄의 막을 사유 — 빈 이름 → 쓸 수 없는 글자 → 묶음 안 겹침 → 디스크 겹침 차례다
fn block_of(rows: &[Row], index: usize, names: &FolderNames) -> Option<Block> {
    let row = &rows[index];
    if row.unchanged() {
        return None;
    }
    if row.to.trim().is_empty() {
        return Some(Block::Empty);
    }
    if crate::fs::file_op::invalid_name_reason(&row.to).is_some() {
        return Some(Block::InvalidChars);
    }
    let dir = row.from.parent();
    let wanted = row.to.to_lowercase();
    let same_dir = |other: &Row| other.from.parent() == dir;
    if rows
        .iter()
        .enumerate()
        .any(|(other, them)| other != index && same_dir(them) && them.to.to_lowercase() == wanted)
    {
        return Some(Block::Duplicate);
    }
    // 이 묶음이 비워 줄 이름(이름이 바뀌는 다른 항목의 지금 이름)과 자기 이름은 겹침이 아니다
    let vacated = rows.iter().any(|them| {
        same_dir(them) && !them.unchanged() && name_of(&them.from).to_lowercase() == wanted
    });
    let on_disk = dir
        .and_then(|dir| names.get(&folder_key(dir)))
        .is_some_and(|names| names.contains(&wanted));
    (on_disk && !vacated).then_some(Block::Exists)
}

/// 모든 항목의 폴더 이름 목록이 와 있는가 — 오기 전에는 겹침을 가를 수 없다
pub fn ready(items: &[Item], names: &FolderNames) -> bool {
    items.iter().all(|item| {
        item.path
            .parent()
            .is_some_and(|dir| names.contains_key(&folder_key(dir)))
    })
}

/// 적용할 `(경로, 새 이름)` — 막힌 줄이 있거나 바뀌는 것이 없으면 `None`
pub fn pairs(rows: &[Row]) -> Option<Vec<(PathBuf, String)>> {
    if rows.iter().any(|row| row.block.is_some()) {
        return None;
    }
    let changed: Vec<(PathBuf, String)> = rows
        .iter()
        .filter(|row| !row.unchanged())
        .map(|row| (row.from.clone(), row.to.clone()))
        .collect();
    (!changed.is_empty()).then_some(changed)
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn 파일(path: &str) -> Item {
        Item {
            path: PathBuf::from(path),
            is_dir: false,
        }
    }

    fn 이름표(entries: &[(&str, &[&str])]) -> FolderNames {
        entries
            .iter()
            .map(|(dir, names)| {
                (
                    folder_key(Path::new(dir)),
                    names.iter().map(|name| name.to_lowercase()).collect(),
                )
            })
            .collect()
    }

    fn 새_이름(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|row| row.to.as_str()).collect()
    }

    fn 바꾸기(find: &str, with: &str) -> Rule {
        Rule::Replace {
            find: find.to_owned(),
            with: with.to_owned(),
        }
    }

    #[test]
    fn 찾기_바꾸기는_확장자를_건드리지_않는다() {
        let items = [파일(r"C:\w\txt_note.txt")];
        let names = 이름표(&[(r"C:\w", &["txt_note.txt"])]);
        let rows = preview(&items, &바꾸기("txt", "doc"), false, &names);
        assert_eq!(새_이름(&rows), ["doc_note.txt"]);
        assert_eq!(rows[0].block, None);
    }

    #[test]
    fn 확장자_포함을_켜면_확장자도_바꾼다() {
        let items = [파일(r"C:\w\txt_note.txt")];
        let names = 이름표(&[(r"C:\w", &["txt_note.txt"])]);
        let rows = preview(&items, &바꾸기("txt", "doc"), true, &names);
        assert_eq!(새_이름(&rows), ["doc_note.doc"]);
    }

    #[test]
    fn 연번은_시작과_자릿수를_따른다() {
        let items = [
            파일(r"C:\w\a.jpg"),
            파일(r"C:\w\b.jpg"),
            파일(r"C:\w\c.jpg"),
        ];
        let names = 이름표(&[(r"C:\w", &["a.jpg", "b.jpg", "c.jpg"])]);
        let rule = Rule::Number {
            prefix: "img_".to_owned(),
            suffix: "_x".to_owned(),
            start: 9,
            digits: 3,
            keep_name: false,
        };
        let rows = preview(&items, &rule, false, &names);
        assert_eq!(
            새_이름(&rows),
            ["img_009_x.jpg", "img_010_x.jpg", "img_011_x.jpg"]
        );
        // 기본값은 시작 1·자릿수 1 (Q5)
        let rows = preview(&items, &Rule::number_default(), false, &names);
        assert_eq!(새_이름(&rows), ["1.jpg", "2.jpg", "3.jpg"]);
        // 원래 이름을 남기면 접두 + 원래 이름 + 번호 + 접미
        let keep = Rule::Number {
            prefix: "p_".to_owned(),
            suffix: String::new(),
            start: 1,
            digits: 2,
            keep_name: true,
        };
        let rows = preview(&items[..1], &keep, false, &names);
        assert_eq!(새_이름(&rows), ["p_a01.jpg"]);
    }

    #[test]
    fn 정규식은_묶음을_새_이름에_쓴다() {
        let items = [파일(r"C:\w\2024-05 report.pdf")];
        let names = 이름표(&[(r"C:\w", &["2024-05 report.pdf"])]);
        let rule = Rule::Regex {
            pattern: r"(\d{4})-(\d{2}) (.+)".to_owned(),
            with: "$3 $1$2".to_owned(),
        };
        let rows = preview(&items, &rule, false, &names);
        assert_eq!(새_이름(&rows), ["report 202405.pdf"]);
    }

    #[test]
    fn 잘못된_정규식은_막는다() {
        let items = [파일(r"C:\w\a.txt")];
        let names = 이름표(&[(r"C:\w", &["a.txt"])]);
        let rule = Rule::Regex {
            pattern: "(".to_owned(),
            with: String::new(),
        };
        let rows = preview(&items, &rule, false, &names);
        assert_eq!(rows[0].block, Some(Block::BadPattern));
        assert_eq!(pairs(&rows), None);
    }

    #[test]
    fn 같은_폴더의_새_이름끼리_겹치면_막는다() {
        let items = [파일(r"C:\w\a1.txt"), 파일(r"C:\w\a2.txt")];
        let names = 이름표(&[(r"C:\w", &["a1.txt", "a2.txt"])]);
        let rule = Rule::Regex {
            pattern: r"\d".to_owned(),
            with: String::new(),
        };
        let rows = preview(&items, &rule, false, &names);
        assert_eq!(새_이름(&rows), ["a.txt", "a.txt"]);
        assert!(rows.iter().all(|row| row.block == Some(Block::Duplicate)));
        assert_eq!(pairs(&rows), None);
    }

    #[test]
    fn 다른_폴더면_같은_새_이름도_된다() {
        // 하위 폴더 검색 결과에서 고른 것은 폴더가 제각각이다 — 겹침은 폴더마다 본다
        let items = [파일(r"C:\w\x\a1.txt"), 파일(r"C:\w\y\a2.txt")];
        let names = 이름표(&[(r"C:\w\x", &["a1.txt"]), (r"C:\w\y", &["a2.txt"])]);
        let rule = Rule::Regex {
            pattern: r"\d".to_owned(),
            with: String::new(),
        };
        let rows = preview(&items, &rule, false, &names);
        assert!(rows.iter().all(|row| row.block.is_none()), "{rows:?}");
    }

    #[test]
    fn 디스크에_있는_이름과_겹치면_막는다() {
        // 숨김·필터로 목록에 안 보이는 항목도 디스크에 있으면 겹침이다
        let items = [파일(r"C:\w\a.txt")];
        let names = 이름표(&[(r"C:\w", &["a.txt", "B.TXT"])]);
        let rows = preview(&items, &바꾸기("a", "b"), false, &names);
        assert_eq!(rows[0].block, Some(Block::Exists));
    }

    #[test]
    fn 맞바꾸는_이름은_겹침이_아니다() {
        // 번호를 하나씩 밀면 1→2·2→3 — 2.txt 는 묶음이 비워 줄 자리라 겹침이 아니다
        let items = [파일(r"C:\w\1.txt"), 파일(r"C:\w\2.txt")];
        let names = 이름표(&[(r"C:\w", &["1.txt", "2.txt"])]);
        let rule = Rule::Number {
            prefix: String::new(),
            suffix: String::new(),
            start: 2,
            digits: 1,
            keep_name: false,
        };
        let rows = preview(&items, &rule, false, &names);
        assert_eq!(새_이름(&rows), ["2.txt", "3.txt"]);
        assert!(rows.iter().all(|row| row.block.is_none()), "{rows:?}");
        // 대소문자만 바꾸는 자기 이름도 겹침이 아니다
        let 대소문자 = preview(&items[..1], &바꾸기("1", "1"), false, &names);
        assert_eq!(대소문자[0].block, None);
        let 자기 = preview(
            &[파일(r"C:\w\a.txt")],
            &바꾸기("a", "A"),
            false,
            &이름표(&[(r"C:\w", &["a.txt"])]),
        );
        assert_eq!(자기[0].block, None, "자기 자리는 겹침이 아니다");
    }

    #[test]
    fn 쓸_수_없는_글자는_막는다() {
        let items = [파일(r"C:\w\a.txt")];
        let names = 이름표(&[(r"C:\w", &["a.txt"])]);
        let rows = preview(&items, &바꾸기("a", "a?"), false, &names);
        assert_eq!(rows[0].block, Some(Block::InvalidChars));
        let 빈 = preview(&items, &바꾸기("a", ""), false, &names);
        assert_eq!(빈[0].block, Some(Block::Empty));
    }

    #[test]
    fn 바뀌는_줄만_적용한다() {
        let items = [파일(r"C:\w\a.txt"), 파일(r"C:\w\c.txt")];
        let names = 이름표(&[(r"C:\w", &["a.txt", "c.txt"])]);
        let rows = preview(&items, &바꾸기("a", "z"), false, &names);
        assert_eq!(
            pairs(&rows),
            Some(vec![(PathBuf::from(r"C:\w\a.txt"), "z.txt".to_owned())])
        );
        assert!(ready(&items, &names));
        assert!(!ready(&items, &FolderNames::new()), "이름 목록이 오기 전");
    }
}
