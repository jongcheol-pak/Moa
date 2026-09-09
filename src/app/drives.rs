//! 드라이브 줄 목록과 연결 상태 (2026-08-17 사용자 요청).
//!
//! **앱에 하나뿐이다** — 모든 패널의 트리가 같은 목록을 보므로, 같은 드라이브에 X가 있는
//! 트리와 없는 트리가 한 화면에 서지 않는다(즐겨찾기와 같은 구조).
//!
//! 이 모듈은 화면도 Win32도 모른다 — 조회는 `fs::drives`가 하고, 여기 있는 것은
//! **무엇이 들어오면 무엇이 되는가**라는 규칙뿐이다. 그 규칙을 화면 쪽(`ExplorerApp`)에
//! 두면 시험으로 덮을 수 없다 — 그 타입은 `eframe::CreationContext`가 있어야 만들어져
//! 단위 시험에서 세울 수 없기 때문이다(`favorites`가 같은 이유로 이 계층에 있다).
use crate::fs::drives::DriveRow;
use std::path::{Path, PathBuf};

/// 트리에 내려보낼 드라이브 줄 목록.
///
/// 값이 채워지는 길은 셋이고 **시점이 겹치지 않는다** — 시작할 때 `replace`(목록),
/// 뒤이어 `apply_reachable`(워커의 접근 판정), 그 뒤로 `observe`(사용자가 열어 본 결과).
///
/// 정렬 옵션·변경 통지를 두지 않는다 — 드라이브 순서는 OS가 주는 알파벳 순 그대로면
/// 충분하고, 화면은 매 프레임 이 목록을 그대로 읽는다
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DriveList {
    rows: Vec<DriveRow>,
}

impl DriveList {
    /// 화면이 그릴 줄들 — 워커 결과가 오기 전에는 비어 있다
    pub fn rows(&self) -> &[DriveRow] {
        &self.rows
    }

    /// 워커가 만든 목록으로 통째로 바꾼다 (시작할 때 1회).
    ///
    /// 이때 모든 줄의 `offline`은 `false`다 — 판정은 뒤이어 오는 `apply_reachable`이 채운다
    pub fn replace(&mut self, rows: Vec<DriveRow>) {
        self.rows = rows;
    }

    /// 워커의 접근 판정을 덮는다 — **목록은 다시 만들지 않는다**.
    ///
    /// `reachable`이 `false`인 드라이브만 끊긴 것으로 표시된다. 목록에 없는 경로는
    /// 조용히 지나간다(판정하는 사이에 드라이브가 빠질 수 있다)
    pub fn apply_reachable(&mut self, judged: &[(PathBuf, bool)]) {
        for (root, reachable) in judged {
            if let Some(row) = self.rows.iter_mut().find(|row| same_root(&row.path, root)) {
                row.offline = !reachable;
            }
        }
    }

    /// 목록만 새로 온 것을 반영한다 — **접근 판정이 뒤따르지 않는다**.
    ///
    /// `replace`와 가르는 것은 그 한 가지다. 저쪽은 시작·주기 확인의 첫 소식이라 곧
    /// `apply_reachable`이 배지를 채우지만, 이쪽은 드라이브가 꽂히거나 빠졌을 때 감시
    /// 워커가 목록만 보내는 길이라 판정이 오지 않는다. 그대로 `replace`를 쓰면 그 순간
    /// **끊긴 네트워크 드라이브의 X 배지가 사라졌다가 다음 주기 확인(30초)에야 돌아온다**.
    ///
    /// 그래서 **같은 뿌리의 `offline`을 이어받는다** — 새로 나타난 뿌리는 판정한 적이
    /// 없으므로 `false`이고(판정 전에는 배지를 두지 않는다), 목록에서 빠진 뿌리는
    /// 그 줄과 함께 사라진다
    pub fn refresh(&mut self, rows: Vec<DriveRow>) {
        // 옛 목록을 먼저 꺼낸다 — 그러지 않으면 아래 대입이 `self.rows`를 빌린 채 덮는다
        let old = std::mem::take(&mut self.rows);
        self.rows = rows
            .into_iter()
            .map(|mut row| {
                row.offline = old
                    .iter()
                    .find(|prev| same_root(&prev.path, &row.path))
                    .is_some_and(|prev| prev.offline);
                row
            })
            .collect();
    }

    /// 사용자가 열어 본 결과 하나를 반영한다.
    ///
    /// **네트워크 드라이브만** 바꾼다 — 로컬 드라이브의 실패는 연결 문제가 아니라서
    /// 배지를 둘 자리가 아니다(사용자 결정: 로컬에는 X를 두지 않는다).
    ///
    /// 받는 경로는 드라이브 뿌리가 아니어도 된다 — 그 경로가 속한 드라이브를 찾아 반영한다
    /// (사용자는 `Z:\Docs`를 열다 실패하고, 배지가 붙는 자리는 `Z:\` 줄이다)
    pub fn observe(&mut self, path: &Path, reachable: bool) {
        let Some(root) = drive_root_of(path) else {
            return;
        };
        if let Some(row) = self
            .rows
            .iter_mut()
            .find(|row| row.network && same_root(&row.path, &root))
        {
            row.offline = !reachable;
        }
    }
}

/// 이 경로가 속한 드라이브 뿌리 (`Z:\Docs\a.txt` → `Z:\`).
///
/// 드라이브 문자가 없는 경로(UNC·상대 경로)는 `None`이다 — UNC 공유는 드라이브 줄로
/// 서지 않으므로 반영할 자리가 없다
pub fn drive_root_of(path: &Path) -> Option<PathBuf> {
    let text = path.to_string_lossy();
    let mut chars = text.chars();
    let letter = chars.next()?;
    if !letter.is_ascii_alphabetic() || chars.next() != Some(':') {
        return None;
    }
    Some(PathBuf::from(format!("{}:\\", letter.to_ascii_uppercase())))
}

/// 두 뿌리 경로가 같은 드라이브인가 — 드라이브 문자는 대소문자를 가리지 않는다
fn same_root(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 시험용 줄 하나 — 조회를 거치지 않고 손으로 세운다
    fn row(path: &str, network: bool) -> DriveRow {
        DriveRow {
            path: PathBuf::from(path),
            label: format!("드라이브 ({path})"),
            icon: 0,
            network,
            offline: false,
        }
    }

    fn list() -> DriveList {
        let mut drives = DriveList::default();
        drives.replace(vec![row(r"C:\", false), row(r"Z:\", true)]);
        drives
    }

    #[test]
    fn 네트워크_드라이브_아래의_실패는_그_드라이브_줄에_붙는다() {
        // T3 Acceptance — 사용자는 `Z:\Docs`를 열다 실패하고, 배지가 붙는 자리는 `Z:\`다
        let mut drives = list();
        drives.observe(Path::new(r"Z:\Docs"), false);
        let z = drives
            .rows()
            .iter()
            .find(|row| row.path == Path::new(r"Z:\"))
            .expect("Z 드라이브");
        assert!(z.offline, "끊긴 것으로 표시되지 않았다");
        let c = drives
            .rows()
            .iter()
            .find(|row| row.path == Path::new(r"C:\"))
            .expect("C 드라이브");
        assert!(!c.offline, "무관한 드라이브가 함께 바뀌었다");
    }

    #[test]
    fn 로컬_드라이브의_실패는_배지를_부르지_않는다() {
        // 사용자 결정 — 탐색기도 로컬 자리에는 연결 끊김 표식을 두지 않는다
        let mut drives = list();
        drives.observe(Path::new(r"C:\Users"), false);
        assert!(
            drives.rows().iter().all(|row| !row.offline),
            "로컬 드라이브에 배지가 붙었다"
        );
    }

    #[test]
    fn 열어_본_뒤_닿으면_배지가_사라진다() {
        // 연결이 복구된 뒤 그 드라이브를 열어 성공하는 경로
        let mut drives = list();
        drives.observe(Path::new(r"Z:\"), false);
        drives.observe(Path::new(r"Z:\"), true);
        assert!(
            drives.rows().iter().all(|row| !row.offline),
            "배지가 그대로 남았다"
        );
    }

    #[test]
    fn 목록에_없는_드라이브의_관측은_조용히_지나간다() {
        let mut drives = list();
        drives.observe(Path::new(r"Q:\없는곳"), false);
        assert!(drives.rows().iter().all(|row| !row.offline));
        assert_eq!(drives.rows().len(), 2, "목록이 바뀌었다");
    }

    #[test]
    fn 접근_판정은_목록을_다시_만들지_않고_덮는다() {
        // T3 Acceptance — `replace` 뒤 `apply_reachable`이 오면 판정 결과대로만 남는다
        let mut drives = list();
        drives.observe(Path::new(r"Z:\"), false);
        drives.replace(vec![row(r"C:\", false), row(r"Z:\", true)]);
        drives.apply_reachable(&[(PathBuf::from(r"Z:\"), true)]);
        assert!(
            drives.rows().iter().all(|row| !row.offline),
            "옛 값이 살아남았다"
        );
        drives.apply_reachable(&[(PathBuf::from(r"Z:\"), false)]);
        let z = drives
            .rows()
            .iter()
            .find(|r| r.network)
            .expect("Z 드라이브");
        assert!(z.offline, "판정이 반영되지 않았다");
    }

    #[test]
    fn 드라이브_문자는_대소문자를_가리지_않는다() {
        // 주소창·세션·셸이 주는 경로의 대소문자가 섞여도 같은 줄을 찾아야 한다
        let mut drives = list();
        drives.observe(Path::new(r"z:\docs"), false);
        let z = drives
            .rows()
            .iter()
            .find(|r| r.network)
            .expect("Z 드라이브");
        assert!(z.offline, "소문자 경로를 다른 드라이브로 봤다");
    }

    #[test]
    fn 경로에서_드라이브_뿌리를_뽑는다() {
        assert_eq!(
            drive_root_of(Path::new(r"Z:\Docs\a.txt")),
            Some(PathBuf::from(r"Z:\"))
        );
        assert_eq!(
            drive_root_of(Path::new(r"Z:\")),
            Some(PathBuf::from(r"Z:\"))
        );
        // 소문자는 대문자로 맞춘다 — 뿌리 경로 표기를 하나로 둔다
        assert_eq!(
            drive_root_of(Path::new(r"z:\")),
            Some(PathBuf::from(r"Z:\"))
        );
        // UNC 공유는 드라이브 줄로 서지 않는다
        assert_eq!(drive_root_of(Path::new(r"\\host\share\x")), None);
        assert_eq!(drive_root_of(Path::new("relative/path")), None);
    }

    #[test]
    fn 목록만_새로_와도_끊김_배지가_이어진다() {
        // T1 Acceptance — 감시 워커는 접근 판정을 하지 않는다. `replace`를 쓰면 이 자리에서
        // 배지가 사라졌다가 다음 주기 확인(30초)에야 돌아온다
        let mut drives = list();
        drives.apply_reachable(&[(PathBuf::from(r"Z:\"), false)]);
        drives.refresh(vec![
            row(r"C:\", false),
            row(r"D:\", false),
            row(r"Z:\", true),
        ]);
        let z = drives
            .rows()
            .iter()
            .find(|r| r.network)
            .expect("Z 드라이브");
        assert!(z.offline, "목록을 새로 받으며 배지가 날아갔다");
        assert_eq!(drives.rows().len(), 3, "꽂힌 드라이브가 서지 않았다");
    }

    #[test]
    fn 새로_나타난_드라이브에는_배지가_없다() {
        // 판정한 적이 없는 뿌리다 — 판정 전에는 배지를 두지 않는다(`list_drives`와 같은 규칙)
        let mut drives = list();
        drives.refresh(vec![
            row(r"C:\", false),
            row(r"Z:\", true),
            row(r"E:\", true),
        ]);
        let e = drives
            .rows()
            .iter()
            .find(|r| r.path == Path::new(r"E:\"))
            .expect("E 드라이브");
        assert!(!e.offline, "판정도 하지 않고 끊긴 것으로 표시했다");
    }

    #[test]
    fn 빠진_드라이브는_배지째_사라진다() {
        // 뽑힌 드라이브의 상태를 붙들고 있으면 다시 꽂았을 때 옛 판정이 되살아난다
        let mut drives = list();
        drives.apply_reachable(&[(PathBuf::from(r"Z:\"), false)]);
        drives.refresh(vec![row(r"C:\", false)]);
        assert_eq!(drives.rows().len(), 1);
        assert!(
            drives.rows().iter().all(|r| r.path == Path::new(r"C:\")),
            "빠진 드라이브가 목록에 남았다"
        );
    }

    #[test]
    fn 목록이_비어도_관측은_안전하다() {
        // 워커 결과가 오기 전에 사용자가 폴더를 여는 경우
        let mut drives = DriveList::default();
        drives.observe(Path::new(r"Z:\"), false);
        drives.apply_reachable(&[(PathBuf::from(r"Z:\"), false)]);
        assert!(drives.rows().is_empty());
    }
}
