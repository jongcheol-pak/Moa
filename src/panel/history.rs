//! 탐색 히스토리 — 탭당 독립 (순수 로직, 단위테스트 대상. plan D9)

/// 브라우저식 히스토리: 커서 뒤를 절단하며 push, back/forward로 이동.
///
/// **담는 것이 무엇인지 고르게 열어 둔다** — 로컬 탭은 `PathBuf`를, 원격 탭은 `RemotePath`를
/// 담는다. 원격 경로를 `PathBuf`에 실으면 Windows의 경로 의미(`\` 구분자·`is_absolute`)가
/// 서버 경로에 섞인다(`remote::types::RemotePath`가 `PathBuf`를 쓰지 않는 것과 같은 이유다).
///
/// **복제할 수 있다** — 낙관적으로 옮긴 뒤 되돌려야 할 때 통째로 스냅샷해 둔다(FR-68).
/// 조작을 역연산으로 되돌릴 수는 없다: `push`가 **앞으로 가기 목록을 잘라내므로**
/// 잘린 항목은 어떤 역연산으로도 복원되지 않는다. 담는 것이 경로 몇 개뿐이라 복제가 싸다
#[derive(Clone)]
pub struct History<T> {
    items: Vec<T>,
    /// 현재 위치 (items가 비어있지 않으면 항상 유효 인덱스)
    cursor: usize,
}

impl<T: PartialEq> History<T> {
    /// 시작 경로 1개로 초기화
    pub fn new(start: T) -> History<T> {
        History {
            items: vec![start],
            cursor: 0,
        }
    }

    pub fn current(&self) -> &T {
        &self.items[self.cursor]
    }

    /// 새 이동 — 커서 뒤(앞으로 가기 목록)를 절단하고 추가.
    /// 현재 위치와 같은 경로면 중복 추가하지 않는다
    pub fn push(&mut self, path: T) {
        if self.items[self.cursor] == path {
            return;
        }
        self.items.truncate(self.cursor + 1);
        self.items.push(path);
        self.cursor += 1;
    }

    pub fn can_back(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_forward(&self) -> bool {
        self.cursor + 1 < self.items.len()
    }

    /// 뒤로 — 이동했으면 새 현재 경로 반환 (항목은 보존 — 삭제된 폴더도 다시 시도 가능)
    pub fn back(&mut self) -> Option<&T> {
        if self.can_back() {
            self.cursor -= 1;
            Some(self.current())
        } else {
            None
        }
    }

    /// 앞으로 — 이동했으면 새 현재 경로 반환
    pub fn forward(&mut self) -> Option<&T> {
        if self.can_forward() {
            self.cursor += 1;
            Some(self.current())
        } else {
            None
        }
    }

    /// 뒤로 대상 미리보기 — 커서를 옮기지 않는다 (열거 성공 시에만 back()으로 커밋)
    pub fn peek_back(&self) -> Option<&T> {
        if self.can_back() {
            Some(&self.items[self.cursor - 1])
        } else {
            None
        }
    }

    /// 앞으로 대상 미리보기 — 커서를 옮기지 않는다
    pub fn peek_forward(&self) -> Option<&T> {
        if self.can_forward() {
            Some(&self.items[self.cursor + 1])
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::types::RemotePath;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn 기본_push_back_forward() {
        let mut h = History::new(p("C:\\a"));
        h.push(p("C:\\b"));
        h.push(p("C:\\c"));
        assert!(h.can_back());
        assert!(!h.can_forward());

        assert_eq!(h.back().unwrap(), &p("C:\\b"));
        assert_eq!(h.back().unwrap(), &p("C:\\a"));
        assert!(h.back().is_none());
        assert!(h.can_forward());

        assert_eq!(h.forward().unwrap(), &p("C:\\b"));
        assert_eq!(h.forward().unwrap(), &p("C:\\c"));
        assert!(h.forward().is_none());
    }

    #[test]
    fn 분기_이동은_앞으로_목록을_절단한다() {
        let mut h = History::new(p("C:\\a"));
        h.push(p("C:\\b"));
        h.push(p("C:\\c"));
        h.back();
        h.back(); // 현재 a
        h.push(p("C:\\x")); // b, c 절단
        assert_eq!(h.current(), &p("C:\\x"));
        assert!(!h.can_forward());
        assert_eq!(h.back().unwrap(), &p("C:\\a"));
        assert_eq!(h.forward().unwrap(), &p("C:\\x"));
    }

    #[test]
    fn 같은_경로_중복_push는_무시된다() {
        let mut h = History::new(p("C:\\a"));
        h.push(p("C:\\a"));
        assert!(!h.can_back());
    }

    #[test]
    fn peek은_커서를_옮기지_않는다() {
        let mut h = History::new(p("C:\\a"));
        h.push(p("C:\\b"));
        assert_eq!(h.peek_back().unwrap(), &p("C:\\a"));
        assert_eq!(h.current(), &p("C:\\b")); // 커서 불변
        h.back();
        assert_eq!(h.peek_forward().unwrap(), &p("C:\\b"));
        assert_eq!(h.current(), &p("C:\\a"));
    }

    #[test]
    fn back_대상_경로는_삭제돼도_항목이_보존된다() {
        // Edge: back 대상 폴더가 사이에 삭제됨 → 히스토리 항목은 보존 (재시도 가능)
        let mut h = History::new(p("C:\\a"));
        h.push(p("C:\\살아있다"));
        h.push(p("C:\\b"));
        h.back(); // 삭제됐다고 가정해도 항목 유지
        assert_eq!(h.current(), &p("C:\\살아있다"));
        assert!(h.can_forward());
    }

    #[test]
    fn 스냅샷으로_되돌리면_앞으로_가기_목록까지_살아난다() {
        // FR-68 — 낙관적으로 옮긴 뒤 그 폴더가 없으면 되돌려야 하는데, `push`가
        // **앞으로 가기 목록을 잘라내므로**(truncate) 조작 역연산으로는 복원되지 않는다.
        // 통째 스냅샷만이 잘린 항목을 되살린다
        let mut h = History::new(p(r"C:\a"));
        h.push(p(r"C:\b"));
        h.back();
        assert!(h.can_forward(), "되돌리기 전 상태를 잘못 세웠다");

        let snapshot = h.clone();
        // 낙관적 이동 — 여기서 `C:\b`가 잘린다
        h.push(p(r"C:\c"));
        assert!(!h.can_forward(), "push가 앞으로 가기 목록을 자르지 않았다");

        // 열어 보니 없는 폴더였다 — 스냅샷으로 되돌린다
        h = snapshot;
        assert_eq!(h.current(), &p(r"C:\a"));
        assert!(h.can_forward(), "앞으로 가기 목록이 되살아나지 않았다");
        assert_eq!(h.peek_forward(), Some(&p(r"C:\b")));
    }

    #[test]
    fn 원격_경로도_같은_규칙으로_돈다() {
        // 원격 탭의 히스토리가 담는 것은 `RemotePath`다 — `PathBuf`로 실으면 Windows가
        // `/`를 `\`로 정규화해 서버 경로가 손상된다(`RemotePath`의 문서 주석)
        let r = RemotePath::new;
        let mut h = History::new(r("/"));
        h.push(r("/DB"));
        h.push(r("/DB/sub"));
        assert!(h.can_back());
        assert!(!h.can_forward());

        assert_eq!(h.back().unwrap(), &r("/DB"));
        assert_eq!(h.back().unwrap(), &r("/"));
        assert!(h.back().is_none());

        // 분기 이동은 여기서도 앞으로 가기 목록을 자른다
        h.push(r("/etc"));
        assert!(!h.can_forward());
        assert_eq!(h.current().as_str(), "/etc");

        // 같은 자리로 다시 push 하면 쌓이지 않는다 — 새로 고침이 히스토리를 늘리면 안 된다
        let 이전 = h.clone();
        h.push(r("/etc"));
        assert_eq!(h.peek_back(), 이전.peek_back());
        assert!(!h.can_forward());
    }
}
