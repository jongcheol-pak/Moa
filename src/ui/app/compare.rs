//! 두 패널 비교의 짝과 다시 견주기 (FR-79) — `WorkspaceView`의 자식 모듈이라 그 필드에 그대로 닿는다.
//!
//! 판정 규칙은 `app::compare`가 갖고, 여기는 **언제 견주고 결과를 어디에 내려 주는가**만 한다.
//! 견주기는 워커가 한다 — 10만 항목 두 벌의 이름 해시는 한 프레임을 넘길 수 있다(AGENTS: UI
//! 스레드 원칙). 두 목록의 판 번호 쌍이 곧 세대라, 그 사이 목록이 바뀌면 늦게 온 답은 버린다
use super::WorkspaceView;
use crate::app::compare::{CompareMarks, compare};
use crate::app::layout::PanelId;
use eframe::egui;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// 두 목록의 판 번호 쌍 — 이 값이 같으면 다시 견줄 것이 없다
type Versions = (u64, u64);

/// 워크스페이스 하나의 비교 짝 — 워크스페이스마다 하나다(새로 켜면 바뀐다)
pub(super) struct ComparePair {
    a: PanelId,
    b: PanelId,
    /// 지금 화면에 내려 준 결과의 판 번호 쌍 — 없으면 표시가 없다
    shown: Option<Versions>,
    /// 워커에 맡긴 판 번호 쌍과 그 답이 올 채널
    pending: Option<(Versions, Receiver<(CompareMarks, CompareMarks)>)>,
    /// 워커를 띄운 횟수 — 판이 그대로면 다시 띄우지 않는다는 것을 시험이 잰다
    #[cfg(test)]
    spawned: usize,
}

impl WorkspaceView {
    /// 지금 비교 중인 두 패널 — 패널 메뉴의 `비교` 줄이 켜짐을 보인다
    pub(super) fn compare_pair(&self) -> Option<(PanelId, PanelId)> {
        self.compare.as_ref().map(|pair| (pair.a, pair.b))
    }

    /// `a`와 `b`를 비교한다 — 종전 짝이 있으면 그 표시를 거두고 바꾼다
    pub(super) fn start_compare(&mut self, a: PanelId, b: PanelId) {
        self.stop_compare();
        if a == b || !self.panels.contains_key(&a) || !self.panels.contains_key(&b) {
            return;
        }
        self.compare = Some(ComparePair {
            a,
            b,
            shown: None,
            pending: None,
            #[cfg(test)]
            spawned: 0,
        });
    }

    /// 비교를 끄고 두 목록의 표시를 거둔다
    pub(super) fn stop_compare(&mut self) {
        if let Some(pair) = self.compare.take() {
            for id in [pair.a, pair.b] {
                if let Some(panel) = self.panels.get_mut(&id) {
                    panel.set_compare_marks(None);
                }
            }
        }
    }

    /// 프레임마다 — 두 목록이 바뀌었으면 워커에 다시 맡기고, 온 답을 내려 준다.
    ///
    /// 한쪽이라도 로컬 폴더 목록이 아니면(원격·zip·검색 결과·읽는 중) 두 표시를 거둔다 —
    /// 짝은 남아 다시 로컬 목록이 서면 저절로 견준다. 짝 패널이 닫혔으면 비교를 끈다
    pub(super) fn sync_compare(&mut self, ctx: &egui::Context) {
        let Some(pair) = self.compare.as_mut() else {
            return;
        };
        let (a, b) = (pair.a, pair.b);
        if !self.panels.contains_key(&a) || !self.panels.contains_key(&b) {
            self.stop_compare();
            return;
        }
        let snapshots = (
            self.panels[&a].compare_snapshot(),
            self.panels[&b].compare_snapshot(),
        );
        let ((version_a, rows_a), (version_b, rows_b)) = match snapshots {
            (Some(left), Some(right)) => (left, right),
            _ => {
                pair.shown = None;
                pair.pending = None;
                for id in [a, b] {
                    if let Some(panel) = self.panels.get_mut(&id) {
                        panel.set_compare_marks(None);
                    }
                }
                return;
            }
        };
        let versions = (version_a, version_b);
        // 온 답 — 지금 판의 것만 쓴다
        let mut arrived = None;
        if let Some((asked, rx)) = &pair.pending {
            match rx.try_recv() {
                Ok(marks) => {
                    if *asked == versions {
                        arrived = Some(marks);
                    }
                    pair.pending = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => pair.pending = None,
            }
        }
        let asked_now = pair.pending.as_ref().map(|(asked, _)| *asked) == Some(versions);
        if arrived.is_none() && pair.shown != Some(versions) && !asked_now {
            let (tx, rx) = channel();
            let (left, right) = (rows_a.to_vec(), rows_b.to_vec());
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                if tx.send(compare(&left, &right)).is_ok() {
                    ctx.request_repaint();
                }
            });
            pair.pending = Some((versions, rx));
            #[cfg(test)]
            {
                pair.spawned += 1;
            }
        }
        let stale = pair.shown.is_some() && pair.shown != Some(versions);
        if let Some((left, right)) = arrived {
            pair.shown = Some(versions);
            if let Some(panel) = self.panels.get_mut(&a) {
                panel.set_compare_marks(Some(Arc::new(left)));
            }
            if let Some(panel) = self.panels.get_mut(&b) {
                panel.set_compare_marks(Some(Arc::new(right)));
            }
        } else if stale {
            // 목록이 바뀌었다 — 옛 결과의 칠이 새 목록의 같은 이름에 남지 않게 거둔다
            pair.shown = None;
            for id in [a, b] {
                if let Some(panel) = self.panels.get_mut(&id) {
                    panel.set_compare_marks(None);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::layout::{Rect as LayoutRect, SplitDir, SplitPlace};
    use crate::fs::icons::IconCache;
    use crate::panel::dir_cache::DirCache;
    use crate::remote::types::{RemotePath, SiteId};
    use crate::ui::panel::PanelState;
    use std::path::{Path, PathBuf};

    /// (이름, 내용) 들로 시험 폴더를 만든다
    fn folder(tag: &str, side: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("moa_compare_{tag}_{}", std::process::id()))
            .join(side);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("시험 폴더");
        for (name, body) in files {
            std::fs::write(dir.join(name), body).expect("시험 파일");
        }
        dir
    }

    struct Fixture {
        view: WorkspaceView,
        left: PanelId,
        right: PanelId,
        ctx: egui::Context,
        icons: IconCache,
        cache: DirCache,
        root: PathBuf,
    }

    impl Fixture {
        /// 왼쪽: same·diff(3바이트)·left-only · 오른쪽: same·diff(5바이트)
        fn new(tag: &str) -> Fixture {
            let left_dir = folder(
                tag,
                "L",
                &[
                    ("same.txt", "x"),
                    ("diff.txt", "abc"),
                    ("left-only.txt", "l"),
                ],
            );
            let right_dir = folder(tag, "R", &[("same.txt", "x"), ("diff.txt", "abcde")]);
            // 같은 파일의 수정일이 2초 안이 되게 맞춘다 — 만든 차례로 몇 밀리초 갈린다
            let at = std::fs::metadata(left_dir.join("same.txt"))
                .and_then(|meta| meta.modified())
                .expect("시각");
            std::fs::File::options()
                .write(true)
                .open(right_dir.join("same.txt"))
                .and_then(|file| file.set_modified(at))
                .expect("시각 맞추기");
            let mut view = WorkspaceView::new(left_dir.clone());
            let left = view.active;
            let right = view
                .layout
                .split(
                    left,
                    SplitDir::Horizontal,
                    SplitPlace::After,
                    LayoutRect {
                        x: 0,
                        y: 0,
                        w: 1200,
                        h: 800,
                    },
                )
                .expect("분할");
            view.panels.insert(right, PanelState::new(right_dir));
            let mut fixture = Fixture {
                view,
                left,
                right,
                ctx: egui::Context::default(),
                icons: IconCache::new(),
                cache: DirCache::new(),
                root: left_dir.parent().expect("뿌리").to_path_buf(),
            };
            fixture.settle_panels();
            fixture
        }

        /// 두 패널의 열거가 끝날 때까지 돌린다 — 비교 답은 거두지 않는다
        fn settle_panels(&mut self) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                for panel in self.view.panels.values_mut() {
                    panel.poll(&self.ctx, &mut self.icons, &mut self.cache);
                }
                if self.view.panels.values().all(|panel| !panel.is_loading()) {
                    return;
                }
                assert!(std::time::Instant::now() < deadline, "열거가 끝나지 않았다");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        /// 비교 답이 와서 표시가 설 때까지(또는 거둬질 때까지) `sync_compare`를 돌린다
        fn sync_until(&mut self, want_marks: bool) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                self.view.sync_compare(&self.ctx);
                let has = self.view.panels[&self.left].compare_counts().is_some();
                if has == want_marks {
                    return;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "비교 답이 오지 않았다"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }

        fn counts(&self, id: PanelId) -> Option<(usize, usize)> {
            self.view.panels[&id].compare_counts()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn 비교를_켜면_두_목록에_표시가_선다() {
        let mut f = Fixture::new("on");
        f.view.start_compare(f.left, f.right);
        f.sync_until(true);
        // 왼쪽: left-only(한쪽에만) · diff(다름) / 오른쪽: diff(다름)
        assert_eq!(f.counts(f.left), Some((1, 1)));
        assert_eq!(f.counts(f.right), Some((0, 1)));
        assert_eq!(f.view.compare_pair(), Some((f.left, f.right)));
    }

    #[test]
    fn 폴더를_옮기면_다시_비교한다() {
        let mut f = Fixture::new("move");
        f.view.start_compare(f.left, f.right);
        f.sync_until(true);
        // 오른쪽을 왼쪽과 같은 폴더로 옮기면 차이가 없다
        let left_dir = f.view.panels[&f.left].dir().to_path_buf();
        f.view
            .panels
            .get_mut(&f.right)
            .unwrap()
            .navigate_to(left_dir, &f.ctx);
        f.settle_panels();
        f.view.sync_compare(&f.ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while f.counts(f.left) != Some((0, 0)) {
            assert!(std::time::Instant::now() < deadline, "다시 견주지 않았다");
            std::thread::sleep(std::time::Duration::from_millis(5));
            f.view.sync_compare(&f.ctx);
        }
        assert_eq!(f.counts(f.right), Some((0, 0)));
    }

    #[test]
    fn 다시_누르면_비교가_꺼진다() {
        let mut f = Fixture::new("off");
        f.view.start_compare(f.left, f.right);
        f.sync_until(true);
        f.view.stop_compare();
        assert_eq!(f.view.compare_pair(), None);
        assert_eq!(f.counts(f.left), None);
        assert_eq!(f.counts(f.right), None);
    }

    #[test]
    fn 한쪽이_원격이면_표시를_거둔다() {
        let mut f = Fixture::new("remote");
        f.view.start_compare(f.left, f.right);
        f.sync_until(true);
        f.view
            .panels
            .get_mut(&f.right)
            .unwrap()
            .open_remote_tab(SiteId(1), RemotePath::new("/pub"));
        f.view.sync_compare(&f.ctx);
        assert_eq!(f.counts(f.left), None, "원격 탭과 견준 표시가 남았다");
        assert_eq!(
            f.view.compare_pair(),
            Some((f.left, f.right)),
            "짝은 남는다"
        );
    }

    #[test]
    fn 짝_패널이_닫히면_비교가_꺼진다() {
        let mut f = Fixture::new("closed");
        f.view.start_compare(f.left, f.right);
        f.sync_until(true);
        f.view.panels.remove(&f.right);
        f.view.sync_compare(&f.ctx);
        assert_eq!(f.view.compare_pair(), None);
        assert_eq!(f.counts(f.left), None);
    }

    #[test]
    fn 판이_그대로면_다시_견주지_않는다() {
        let mut f = Fixture::new("same_version");
        f.view.start_compare(f.left, f.right);
        f.sync_until(true);
        for _ in 0..5 {
            f.view.sync_compare(&f.ctx);
        }
        assert_eq!(f.view.compare.as_ref().map(|pair| pair.spawned), Some(1));
    }

    #[test]
    fn 읽는_중이면_표시를_거둔다() {
        let mut f = Fixture::new("loading");
        f.view.start_compare(f.left, f.right);
        f.sync_until(true);
        f.view.panels.get_mut(&f.left).unwrap().refresh(&f.ctx);
        f.view.sync_compare(&f.ctx);
        assert_eq!(f.counts(f.left), None, "읽는 동안 옛 결과를 칠했다");
    }

    #[test]
    fn 늦게_온_비교_답은_버린다() {
        let mut f = Fixture::new("stale");
        f.view.start_compare(f.left, f.right);
        // 첫 판을 맡겨 두고 답을 거두기 전에 오른쪽을 왼쪽과 같은 폴더로 옮긴다
        f.view.sync_compare(&f.ctx);
        let left_dir: &Path = f.view.panels[&f.left].dir();
        let left_dir = left_dir.to_path_buf();
        f.view
            .panels
            .get_mut(&f.right)
            .unwrap()
            .navigate_to(left_dir, &f.ctx);
        f.settle_panels();
        std::thread::sleep(std::time::Duration::from_millis(50));
        // 첫 판의 답(차이 있음)은 버리고, 새 판의 답(차이 없음)만 선다
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            f.view.sync_compare(&f.ctx);
            if let Some(counts) = f.counts(f.left) {
                assert_eq!(counts, (0, 0), "옛 판의 답을 칠했다");
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "새 판의 답이 오지 않았다"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}
