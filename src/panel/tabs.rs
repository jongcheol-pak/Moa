//! 패널별 탭의 순수 모델 (FR-3) — 신원·차례·활성·탭별 히스토리를 담는다.
//!
//! 화면은 `ui::tabs`가 그린다.
use crate::panel::history::History;
use crate::remote::connection::ConnectionId;
use crate::remote::types::{FailureKind, RemotePath, SiteId};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 탭 하나를 앱이 사는 동안 유일하게 가리키는 값 (FR-54).
///
/// 인덱스로 탭을 가리키지 않는 이유: 앞의 탭이 닫히면 인덱스가 당겨져 **다른 탭을 가리키게**
/// 된다. 전송 대상은 탭을 오래 기억해야 하므로(배경 탭으로 밀려도 유지된다) 자리가 아니라
/// 신원으로 가리킨다.
///
/// 세션에 저장하지 않는다 — 앱을 다시 켜면 새로 매겨지고, 대상은 그때 다시 정해진다
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(u64);

/// 다음에 내줄 번호. **닫힌 탭의 번호는 다시 쓰지 않는다** — 재사용하면 옛 번호를 들고 있던
/// 쪽이 엉뚱한 탭을 가리키게 된다.
///
/// 형제 식별자들(`SiteId`·`ConnectionId`·`TransferId`)처럼 소유자의 `next_id` 필드를 쓰지 않고
/// **모듈 전역**에 두는 이유: 그쪽 소유자는 앱에 하나뿐이지만 `TabsModel`은 **패널마다** 있어,
/// 인스턴스마다 세면 다른 패널의 탭이 같은 번호를 받는다. 발급기를 주입 가능한 트레이트로
/// 만들지도 않는다 — 바꿔 낄 구현이 없고 테스트도 "서로 다르다"만 보면 된다
static NEXT_TAB_ID: AtomicU64 = AtomicU64::new(1);

impl TabId {
    fn next() -> TabId {
        TabId(NEXT_TAB_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// 탭 하나의 탐색 상태 — 탭별 독립 소스·히스토리 (FR-3).
///
/// 커밋된 위치를 `PathBuf`가 아니라 `TabSource`로 드는 이유는 아래 `TabSource` 설명 참조.
///
/// **히스토리를 둘로 나눠 든다** — 로컬 탭은 `history`를, 원격 탭은 `remote_history`를 쓴다.
/// 하나로 합치지 않는 이유는 담는 타입이 다르기 때문이고(`RemotePath`를 `PathBuf`에 실으면
/// Windows의 경로 의미가 섞인다), 열거형으로 가르지 않는 이유는 **탭의 종류가 생성 후
/// 바뀌지 않아**(`set_committed`의 단언) 쓰이지 않는 쪽이 빈 `Vec` 하나로 남을 뿐이기 때문이다.
/// 가르면 기존 `tab.history.push/back/forward` 호출부가 전부 분기를 타야 한다
pub struct TabState {
    /// 이 탭의 신원 — 생성자가 스스로 매긴다(호출부는 넘기지 않는다)
    pub id: TabId,
    pub source: TabSource,
    /// 로컬 탭의 탐색 히스토리 — 원격 탭에서는 쓰이지 않는다
    pub history: History<PathBuf>,
    /// 원격 탭의 탐색 히스토리 — 로컬 탭에서는 쓰이지 않는다
    pub remote_history: History<RemotePath>,
}

impl TabState {
    pub fn new(path: PathBuf) -> TabState {
        TabState {
            id: TabId::next(),
            history: History::new(path.clone()),
            // 로컬 탭에서는 쓰이지 않는다 — 루트 한 칸으로 세워 둔다
            remote_history: History::new(RemotePath::root()),
            source: TabSource::Local(path),
        }
    }

    /// 원격 위치를 가리키는 탭 — 아직 연결하지 않은 상태로 만든다
    pub fn remote(site: SiteId, path: RemotePath) -> TabState {
        TabState {
            id: TabId::next(),
            // 로컬 히스토리는 원격 탭에서 쓰이지 않는다
            history: History::new(PathBuf::new()),
            remote_history: History::new(path.clone()),
            source: TabSource::Remote {
                site,
                conn: None,
                path,
                phase: TabPhase::New,
            },
        }
    }

    /// 로컬 탭의 커밋 경로. **원격 탭이면 빈 경로**다 —
    /// 로컬 전용 일(열거·감시·썸네일)은 `TabSource::local_path()`로 갈라야 한다
    pub fn committed(&self) -> &Path {
        self.source.local_path().unwrap_or(Path::new(""))
    }

    /// 로컬 위치를 옮긴다.
    ///
    /// **로컬 탭에만 쓴다** — 원격 탭에 부르면 연결·원격 경로가 조용히 사라지고 로컬 탭으로
    /// 둔갑한다. 부르는 곳은 로컬 열거가 성공한 자리뿐이며, 오용을 개발 중에 드러내려고
    /// 단언을 둔다(배포 빌드에서는 사라진다)
    pub fn set_committed(&mut self, path: PathBuf) {
        debug_assert!(
            !self.source.is_remote(),
            "원격 탭에 로컬 경로를 커밋하려 했다"
        );
        self.source = TabSource::Local(path);
    }
}

/// 원격 탭의 연결 단계 — 화면(T10)이 배지·본문으로 투영한다.
///
/// 연결 자체의 단계(`remote::connection::ConnPhase`)와 따로 두는 이유: 탭은 **연결이 없어도**
/// 존재한다(주소만 적힌 빈 탭·세션 복원 직후). 연결 쪽 단계를 그대로 쓰면 그 상태를 표현할 수 없다.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TabPhase {
    /// 아직 어디에도 연결하지 않았다 (배지 `연결 없음`)
    #[default]
    New,
    Connecting,
    Ok,
    Error {
        message: String,
        /// 실패 화면이 덧붙일 안내를 고르는 기준 (FR-32) — 연결 쪽에서 그대로 실려 온다
        kind: FailureKind,
    },
}

/// 탭이 가리키는 곳 — 로컬 폴더이거나 원격 위치다 (plan D8).
///
/// 소스를 패널이 아니라 **탭**에 두는 이유: 연결의 단위가 탭이라(README §4), 패널 단위로 두면
/// 한 패널에 로컬 탭과 원격 탭을 섞을 수 없어 FR-33(모든 패널에서 사이트를 새 탭으로)이 성립하지 않는다.
///
/// 트레이트 객체로 만들지 않는다 — 종류가 둘뿐이고 열거형이 분기를 한눈에 보이게 한다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabSource {
    Local(PathBuf),
    Remote {
        site: SiteId,
        /// 이 탭이 쓰는 연결. 아직 연결하지 않았으면 `None`이다
        conn: Option<ConnectionId>,
        path: RemotePath,
        phase: TabPhase,
    },
}

impl TabSource {
    /// 탭에 보일 이름.
    ///
    /// 원격 이름을 여기 사본으로 들지 않고 **호출부가 넘겨준다** — 사이트 이름은 `이름 바꾸기(R)`로
    /// 바뀌는데, 사본을 들면 탭만 옛 이름으로 남는다
    pub fn title(&self, site_name: Option<&str>) -> String {
        match self {
            TabSource::Local(path) => tab_title(path),
            TabSource::Remote { .. } => site_name
                .unwrap_or(crate::i18n::tabs_missing_site())
                .to_owned(),
        }
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, TabSource::Remote { .. })
    }

    /// 로컬 탭의 경로 — 원격 탭이면 `None`.
    /// 열거·감시·썸네일처럼 **로컬에만 있는 일**이 이것으로 갈린다
    pub fn local_path(&self) -> Option<&Path> {
        match self {
            TabSource::Local(path) => Some(path),
            TabSource::Remote { .. } => None,
        }
    }

    /// 원격 탭의 위치 — 로컬 탭이면 `None`
    pub fn remote_path(&self) -> Option<&RemotePath> {
        match self {
            TabSource::Local(_) => None,
            TabSource::Remote { path, .. } => Some(path),
        }
    }

    pub fn site(&self) -> Option<SiteId> {
        match self {
            TabSource::Local(_) => None,
            TabSource::Remote { site, .. } => Some(*site),
        }
    }
}

/// 탭 닫기 결과
#[derive(Debug, PartialEq, Eq)]
pub enum CloseOutcome {
    /// 탭 제거됨 — 새 활성 인덱스
    Removed(usize),
    /// 마지막 탭 — 패널 닫기로 연결해야 함 (호출부 몫)
    LastTab,
}

/// 탭 목록 순수 모델 — UI 비의존 (단위테스트 대상)
pub struct TabsModel {
    tabs: Vec<TabState>,
    active: usize,
}

impl TabsModel {
    pub fn new(first: TabState) -> TabsModel {
        TabsModel {
            tabs: vec![first],
            active: 0,
        }
    }

    /// 세션 복원용 재구성 — 빈 목록이면 None, 활성 인덱스는 범위로 클램프
    pub fn from_tabs(tabs: Vec<TabState>, active: usize) -> Option<TabsModel> {
        if tabs.is_empty() {
            return None;
        }
        let active = active.min(tabs.len() - 1);
        Some(TabsModel { tabs, active })
    }

    /// 탭들을 그대로 본다 — **신원(`TabId`)과 소스를 함께 봐야 하는 곳**이 쓴다
    /// (탭 스트립이 어느 탭에 전송 아이콘을 그릴지 고르는 자리 — FR-54).
    /// `sources()`는 소스만 복제해 주므로 그 판정에 쓸 수 없다
    pub fn tabs(&self) -> &[TabState] {
        &self.tabs
    }

    /// 탭별 소스 (탭 순서 유지) — 세션 저장·탭 라벨이 쓴다
    pub fn sources(&self) -> Vec<TabSource> {
        self.tabs.iter().map(|t| t.source.clone()).collect()
    }

    /// 모든 탭의 소스를 훑는다 — 연결 단계 갱신처럼 **활성 탭에 한정되지 않는** 일에 쓴다.
    /// 배경 탭이 옛 단계로 남으면 그 탭으로 돌아갔을 때 화면이 실제와 어긋난다
    pub fn sources_mut(&mut self) -> impl Iterator<Item = &mut TabSource> {
        self.tabs.iter_mut().map(|tab| &mut tab.source)
    }

    /// 탭별 로컬 경로 (탭 순서 유지). **원격 탭은 빈 경로**로 나온다 —
    /// 원격까지 담는 세션 저장은 T25가 `sources()`로 바꾼다
    pub fn paths(&self) -> Vec<PathBuf> {
        self.tabs
            .iter()
            .map(|t| t.committed().to_path_buf())
            .collect()
    }

    /// 탭 수 — 세션 저장(part2 T4)·테스트가 소비.
    /// is_empty는 제공하지 않음 — 탭은 항상 1개 이상(불변식)이라 항상 false로 오해만 낳는다
    #[expect(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_index(&self) -> usize {
        self.active
    }

    pub fn active(&self) -> &TabState {
        &self.tabs[self.active]
    }

    /// 지금 보고 있는 탭의 신원 (FR-54)
    pub fn active_id(&self) -> TabId {
        self.tabs[self.active].id
    }

    pub fn active_mut(&mut self) -> &mut TabState {
        &mut self.tabs[self.active]
    }

    /// 새 탭 — 현재 활성 탭 바로 뒤에 추가하고 활성화 (plan D4: 현재 경로 복제는 호출부가 전달)
    pub fn add(&mut self, state: TabState) -> usize {
        let at = self.active + 1;
        self.tabs.insert(at, state);
        self.active = at;
        at
    }

    /// 활성 탭 닫기 — 마지막 1개면 LastTab (패널 닫기로 연결)
    pub fn close_active(&mut self) -> CloseOutcome {
        if self.tabs.len() <= 1 {
            return CloseOutcome::LastTab;
        }
        self.tabs.remove(self.active);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len() - 1;
        }
        CloseOutcome::Removed(self.active)
    }

    /// 지정한 탭 닫기 — **보고 있던 탭은 그대로 유지**한다 (마지막 1개면 LastTab).
    ///
    /// `close_active`는 "활성 탭 자신을 닫는" 경우만 상정한 인덱스 보정을 하므로,
    /// 배경 탭을 닫으려고 `switch` 후 `close_active`를 부르면 활성 탭이 옆으로 튄다.
    /// 임의 탭에 닫기 버튼이 달린 UI(egui 탭 스트립)는 이 함수를 쓴다.
    ///
    /// 주의: **범위 밖 인덱스는 아무것도 제거하지 않고** 현재 활성 인덱스를 그대로 담아
    /// `Removed`를 돌려준다 — 반환값만 보고 "탭이 하나 줄었다"고 단정하면 안 된다
    pub fn close(&mut self, index: usize) -> CloseOutcome {
        if self.tabs.len() <= 1 {
            return CloseOutcome::LastTab;
        }
        if index >= self.tabs.len() {
            return CloseOutcome::Removed(self.active);
        }
        self.tabs.remove(index);
        self.active = match self.active.cmp(&index) {
            // 닫힌 탭보다 앞이면 인덱스가 그대로다
            std::cmp::Ordering::Less => self.active,
            // 활성 탭 자신을 닫았으면 그 자리를 이어받되 끝을 넘지 않는다
            std::cmp::Ordering::Equal => self.active.min(self.tabs.len() - 1),
            // 닫힌 탭보다 뒤면 한 칸 당겨진다 — 보던 탭이 그대로 유지된다
            std::cmp::Ordering::Greater => self.active - 1,
        };
        CloseOutcome::Removed(self.active)
    }

    /// 탭 전환 (범위 밖 인덱스는 무시)
    pub fn switch(&mut self, index: usize) -> bool {
        if index < self.tabs.len() && index != self.active {
            self.active = index;
            true
        } else {
            false
        }
    }
}

/// 탭 제목 — 폴더 이름 (루트는 드라이브 표기 그대로)
pub fn tab_title(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(p: &str) -> TabState {
        TabState::new(PathBuf::from(p))
    }

    #[test]
    fn 탭마다_다른_신원을_받는다() {
        // 인덱스가 아니라 신원으로 가리키는 이유가 이것이다 — 앞 탭이 닫혀도 남은 탭의
        // 값이 흔들리지 않아야 전송 대상이 엉뚱한 곳으로 옮겨가지 않는다 (FR-54)
        let mut m = TabsModel::new(tab(r"C:\a"));
        m.add(tab(r"C:\b"));
        m.add(tab(r"C:\c"));
        let ids: Vec<TabId> = m.tabs().iter().map(|t| t.id).collect();
        assert_eq!(ids.len(), 3);
        for (at, id) in ids.iter().enumerate() {
            assert!(
                !ids[at + 1..].contains(id),
                "같은 신원을 두 탭이 나눠 가졌다: {id:?}"
            );
        }
    }

    #[test]
    fn 닫힌_탭의_신원은_다시_쓰이지_않는다() {
        // 재사용하면 옛 값을 들고 있던 쪽(전송 대상)이 새 탭을 옛 탭으로 착각한다
        let mut m = TabsModel::new(tab(r"C:\a"));
        m.add(tab(r"C:\b"));
        let 닫힌_신원 = m.tabs()[1].id;
        m.close(1);
        m.add(tab(r"C:\c"));
        assert!(
            m.tabs().iter().all(|t| t.id != 닫힌_신원),
            "닫힌 탭의 신원이 새 탭에 다시 매겨졌다"
        );
    }

    #[test]
    fn 세션에서_되살린_탭들도_신원이_서로_다르다() {
        // 복원 경로도 생성자를 거치므로 자동으로 성립한다 — 그 계약을 굳혀 둔다
        let m = TabsModel::from_tabs(vec![tab(r"C:\a"), tab(r"C:\b")], 5).expect("탭 둘");
        assert_ne!(m.tabs()[0].id, m.tabs()[1].id);
        // 활성 인덱스는 범위로 눌리고, 그 자리의 신원이 active_id다
        assert_eq!(m.active_index(), 1);
        assert_eq!(m.active_id(), m.tabs()[1].id);
    }

    #[test]
    fn 새_탭은_활성_뒤에_추가되고_활성화된다() {
        let mut m = TabsModel::new(tab("C:\\a"));
        m.add(tab("C:\\b"));
        assert_eq!(m.len(), 2);
        assert_eq!(m.active_index(), 1);
        assert_eq!(m.active().committed(), PathBuf::from("C:\\b"));

        // 첫 탭으로 돌아가 중간 삽입 확인
        m.switch(0);
        m.add(tab("C:\\c"));
        assert_eq!(m.active_index(), 1);
        assert_eq!(m.active().committed(), PathBuf::from("C:\\c"));
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn 탭_닫기와_활성_보정() {
        let mut m = TabsModel::new(tab("C:\\a"));
        m.add(tab("C:\\b"));
        m.add(tab("C:\\c")); // 활성 = 2 (c)
        assert_eq!(m.close_active(), CloseOutcome::Removed(1)); // c 제거 → b 활성
        assert_eq!(m.active().committed(), PathBuf::from("C:\\b"));
        assert_eq!(m.close_active(), CloseOutcome::Removed(0)); // b 제거 → a 활성
        assert_eq!(m.close_active(), CloseOutcome::LastTab); // 마지막 — 제거 안 됨
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn 배경_탭을_닫아도_보던_탭이_유지된다() {
        let mut m = TabsModel::new(tab("C:\\a"));
        m.add(tab("C:\\b"));
        m.add(tab("C:\\c"));
        m.add(tab("C:\\d")); // [a, b, c, d], 활성 = 3 (d)

        // 활성보다 앞의 배경 탭(b)을 닫아도 계속 d를 보고 있어야 한다
        assert_eq!(m.close(1), CloseOutcome::Removed(2));
        assert_eq!(m.active().committed(), PathBuf::from("C:\\d"));

        // 활성보다 뒤는 없지만, 앞쪽(a)을 하나 더 닫아도 마찬가지
        assert_eq!(m.close(0), CloseOutcome::Removed(1));
        assert_eq!(m.active().committed(), PathBuf::from("C:\\d"));
    }

    #[test]
    fn 활성_탭을_인덱스로_닫으면_옆_탭이_활성된다() {
        let mut m = TabsModel::new(tab("C:\\a"));
        m.add(tab("C:\\b"));
        m.add(tab("C:\\c")); // 활성 = 2 (c)

        assert_eq!(m.close(2), CloseOutcome::Removed(1));
        assert_eq!(m.active().committed(), PathBuf::from("C:\\b"));

        // 활성(1=b)보다 뒤가 없으니 앞의 a를 닫으면 b가 0번으로 당겨진다
        assert_eq!(m.close(0), CloseOutcome::Removed(0));
        assert_eq!(m.active().committed(), PathBuf::from("C:\\b"));
    }

    #[test]
    fn 마지막_탭은_인덱스로도_닫히지_않는다() {
        let mut m = TabsModel::new(tab("C:\\a"));
        assert_eq!(m.close(0), CloseOutcome::LastTab);
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn 범위_밖_인덱스_닫기는_무시된다() {
        let mut m = TabsModel::new(tab("C:\\a"));
        m.add(tab("C:\\b")); // 활성 = 1
        assert_eq!(m.close(9), CloseOutcome::Removed(1));
        assert_eq!(m.len(), 2);
        assert_eq!(m.active().committed(), PathBuf::from("C:\\b"));
    }

    #[test]
    fn 탭_전환은_범위_검사한다() {
        let mut m = TabsModel::new(tab("C:\\a"));
        m.add(tab("C:\\b"));
        assert!(m.switch(0));
        assert!(!m.switch(0)); // 동일 인덱스 — 변화 없음
        assert!(!m.switch(9)); // 범위 밖
        assert_eq!(m.active_index(), 0);
    }

    #[test]
    fn 탭별_히스토리는_독립이다() {
        let mut m = TabsModel::new(tab("C:\\a"));
        m.active_mut().history.push(PathBuf::from("C:\\a\\1"));
        m.add(tab("C:\\b"));
        assert!(!m.active().history.can_back()); // 새 탭은 독립 히스토리
        m.switch(0);
        assert!(m.active().history.can_back());
    }

    #[test]
    fn 탭_제목은_폴더_이름이다() {
        assert_eq!(tab_title(Path::new(r"C:\Users\me\문서")), "문서");
        assert_eq!(tab_title(Path::new(r"C:\")), r"C:\");
    }

    #[test]
    fn 탭_소스의_이름은_로컬은_폴더_원격은_사이트다() {
        let local = TabSource::Local(PathBuf::from(r"C:\Users\me\문서"));
        assert_eq!(local.title(None), "문서");
        // 로컬 탭은 넘겨준 사이트 이름을 쓰지 않는다
        assert_eq!(local.title(Some("배포 서버")), "문서");

        let remote = TabSource::Remote {
            site: SiteId(3),
            conn: None,
            path: RemotePath::new("/var/www"),
            phase: TabPhase::New,
        };
        assert_eq!(remote.title(Some("배포 서버")), "배포 서버");
        // 사이트가 지워진 뒤 남은 탭
        assert_eq!(remote.title(None), "알 수 없는 사이트");
    }

    #[test]
    fn 탭_소스는_로컬과_원격을_가른다() {
        let local = TabSource::Local(PathBuf::from(r"C:\"));
        assert!(!local.is_remote());
        assert_eq!(local.local_path(), Some(Path::new(r"C:\")));
        assert_eq!(local.remote_path(), None);
        assert_eq!(local.site(), None);

        let remote = TabSource::Remote {
            site: SiteId(7),
            conn: None,
            path: RemotePath::new("/pub"),
            phase: TabPhase::Connecting,
        };
        assert!(remote.is_remote());
        // 열거·감시·썸네일은 이 `None`으로 갈린다
        assert_eq!(remote.local_path(), None);
        assert_eq!(remote.remote_path().map(|p| p.as_str()), Some("/pub"));
        assert_eq!(remote.site(), Some(SiteId(7)));
    }

    #[test]
    fn 새_원격_탭의_기본_단계는_연결_없음이다() {
        assert_eq!(TabPhase::default(), TabPhase::New);
    }
}
