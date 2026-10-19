//! 전송 큐 — 무엇을 어디로 옮길지의 목록과 그 상태 (FR-36).
//!
//! **순수 모델이다** — 스레드도 I/O도 없다. 실제로 옮기는 것은 `remote::transfer`(T18)가
//! 이 큐에서 꺼내 연결 워커에 맡긴다. 나누는 이유는 큐의 규칙(자리 배정·필터·요약)을
//! 서버 없이 전부 시험할 수 있게 하기 위함이다 (D25).
//!
//! **방향·식별자는 새로 만들지 않는다** — `remote::connection`이 T4에서 이미
//! `TransferId`·`TransferDirection`을 도입했고(워커 명령이 그것을 싣는다), 여기서 같은 뜻의
//! 타입을 또 만들면 경계마다 옮겨 담게 된다 (plan T17 신규 심볼 목록의 `Direction`이 이것이다).
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::remote::connection::{TransferDirection, TransferId};
use crate::remote::types::{RemotePath, SiteId};

/// 크기를 모르거나 셀 수 없을 때 화면에 보일 값 (plan Edge Case: size 0 → 진행률 `—`)
pub const UNKNOWN: &str = "—";

/// 전송 한 건의 상태.
///
/// `Active`가 **보낸 바이트와 속도를 함께** 드는 이유: 화면이 둘을 같은 줄에 그리는데
/// (`전송 중 · 12.4 MB/s` — 인벤토리 #45), 따로 두면 한쪽만 갱신된 순간이 보인다
#[derive(Debug, Clone, PartialEq)]
pub enum TransferState {
    /// 자리를 기다린다
    Wait,
    Active {
        sent: u64,
        /// 초당 바이트 — 0이면 아직 잴 수 없다
        speed: u64,
    },
    Done,
    /// 서버가 준 사유를 그대로 보인다 (인벤토리 #48)
    Error {
        message: String,
    },
    /// 사용자가 `전송 취소`로 그만뒀다.
    ///
    /// **사유를 담지 않는다** — 서버가 준 문자열이 없고 사유가 하나뿐이라, 화면이 적을 말은
    /// 상태 자체에서 나온다(`이유` 열의 `사용자 취소`)
    Cancelled,
}

impl TransferState {
    pub fn is_active(&self) -> bool {
        matches!(self, TransferState::Active { .. })
    }

    pub fn is_done(&self) -> bool {
        matches!(self, TransferState::Done)
    }

    pub fn is_error(&self) -> bool {
        matches!(self, TransferState::Error { .. })
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, TransferState::Cancelled)
    }

    /// 아직 끝나지 않았는가 — 대기·진행이 여기 든다
    pub fn is_pending(&self) -> bool {
        matches!(self, TransferState::Wait | TransferState::Active { .. })
    }

    /// **다시 걸 수 있는가** — 서버가 거부한 것과 사용자가 그만둔 것이 함께 든다.
    ///
    /// `실패` 탭이 담는 것·`다시 시도`가 되살리는 것·행 메뉴가 그 항목을 세우는 조건이 모두
    /// 이 하나다. 각자 `is_error() || is_cancelled()`를 다시 쓰면 한쪽만 넓혀졌을 때
    /// 「탭에는 보이는데 다시 걸 수는 없는」 줄이 생긴다
    pub fn is_retryable(&self) -> bool {
        self.is_error() || self.is_cancelled()
    }
}

/// 큐에 담긴 전송 한 건.
///
/// **폴더가 아니라 파일 단위다** — 폴더 전송은 큐에 넣기 전에 펼친다(plan 비추상화 선언).
/// 그래야 진행률·이어받기·취소가 한 파일을 기준으로 단순해진다
#[derive(Debug, Clone, PartialEq)]
pub struct TransferItem {
    pub id: TransferId,
    /// 어느 사이트의 전송인가 — 연결별 탭(인벤토리 #36)과 자리 배정이 이것으로 갈린다
    pub site: SiteId,
    pub direction: TransferDirection,
    pub local: PathBuf,
    pub remote: RemotePath,
    /// 전체 크기. **0이면 모른다는 뜻**이다(서버가 알리지 않는 경우가 있다)
    pub size: u64,
    pub state: TransferState,
    /// 처음 전송이 시작된 시각(FILETIME) — 아직 시작하지 않았으면 `None`.
    ///
    /// **이어받기·재시도로 다시 시작해도 이 값은 그대로다** — 사용자가 알고 싶은 것은
    /// "언제부터 이 파일을 옮기기 시작했는가"이지 마지막 시도가 언제였는가가 아니다
    pub started_at: Option<u64>,
    /// 끝난 시각(FILETIME) — 완료든 실패든 마지막으로 끝난 때다. 아직이면 `None`
    pub finished_at: Option<u64>,
    /// 지금까지 **실패한** 횟수 (FR-37) — 자동 재시도가 몇 번째인지 여기서 센다.
    ///
    /// 사용자가 손으로 `다시 시도`를 누르면 0으로 돌아간다 — 서버를 고친 뒤 다시 거는 것을
    /// 새 시도로 보기 때문이다(2026-08-28 사용자 결정)
    pub attempts: u32,
    /// 이 시각(상대 초 — `start_ready`의 `now`와 같은 축) **전에는 배정하지 않는다**.
    ///
    /// 자동 재시도 사이에 지연을 두는 자리다. 살아 있는 연결에서 같은 오류가 반복될 때
    /// 서버를 쉬지 않고 두드리지 않게 한다. `None`이면 언제든 배정할 수 있다
    pub retry_at: Option<f64>,
}

impl TransferItem {
    /// 0.0~1.0 진행률. 크기를 모르면 `None`이다 — 화면이 `—`를 보인다
    pub fn progress(&self) -> Option<f32> {
        if self.size == 0 {
            return None;
        }
        let sent = match &self.state {
            TransferState::Active { sent, .. } => *sent,
            TransferState::Done => self.size,
            _ => 0,
        };
        Some((sent as f32 / self.size as f32).clamp(0.0, 1.0))
    }

    /// 아직 보내야 할 바이트 — 남은 시간 계산의 재료다
    fn remaining(&self) -> u64 {
        match &self.state {
            TransferState::Active { sent, .. } => self.size.saturating_sub(*sent),
            TransferState::Wait => self.size,
            _ => 0,
        }
    }
}

/// 큐 화면의 필터 (인벤토리 #29·#31·#32).
///
/// 셋은 **같은 항목 집합을 거른다** — `Done`·`Error`는 `All`의 부분집합이라
/// 건수를 합치면 전체와 맞는다 (README §6)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueueFilter {
    #[default]
    All,
    Done,
    Error,
}

impl QueueFilter {
    /// 이 거르개에 걸리는가 — `filter`·`count`·`counts_by_site` 셋이 같은 판정을 쓴다
    fn matches(self, state: &TransferState) -> bool {
        match self {
            QueueFilter::All => true,
            QueueFilter::Done => state.is_done(),
            // 취소도 「끝내지 못한 전송」이라 이 탭이 함께 담는다 — 사용자가 그만둔 것도
            // 무엇을 다시 걸지 고르는 자리에 있어야 한다 (2026-08-28 결정)
            QueueFilter::Error => state.is_retryable(),
        }
    }
}

/// 상태 표시줄·큐 머리글이 쓰는 요약 (인벤토리 #54)
#[derive(Debug, Clone, PartialEq)]
pub struct QueueSummary {
    /// 아직 끝나지 않은 건수(대기 + 진행)
    pub pending: usize,
    /// 지금 진행 중인 전송들의 속도 합 (바이트/초)
    pub speed: u64,
    /// 남은 시간(초). 속도가 0이면 잴 수 없어 `None`이다 — 화면이 `—`를 보인다
    pub eta_secs: Option<u64>,
}

/// 전송 큐 — 목록과 상태의 정본 (FR-36).
#[derive(Debug, Default)]
pub struct TransferQueue {
    items: Vec<TransferItem>,
    /// 다음에 발급할 번호
    next_id: u64,
    /// `⏸`로 멈춘 상태 — 멈춰 있으면 새 자리를 내주지 않는다(진행 중인 것은 러너가 멈춘다)
    paused: bool,
    /// 앱이 밀어 넣은 지금 벽시계 시각(FILETIME) — 상태 전이가 이 값을 시각으로 적는다.
    ///
    /// **큐가 직접 읽지 않는다**(`set_wall_now` doc 참조). 밀어 넣기 전이면 `None`이라
    /// 시각이 기록되지 않을 뿐, 상태 전이 자체는 그대로 돈다
    wall_now: Option<u64>,
}

impl TransferQueue {
    pub fn new() -> TransferQueue {
        TransferQueue::default()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn items(&self) -> &[TransferItem] {
        &self.items
    }

    pub fn get(&self, id: TransferId) -> Option<&TransferItem> {
        self.items.iter().find(|item| item.id == id)
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// 전송 한 건을 대기로 넣고 그 번호를 돌려준다.
    ///
    /// **같은 파일을 두 번 넣어도 막지 않는다** — 덮어쓰기 확인은 실제로 쓰기 직전에
    /// 실행기가 한다(plan Edge Case). 여기서 막으면 사용자가 일부러 다시 받는 것도 막힌다
    pub fn enqueue(
        &mut self,
        site: SiteId,
        direction: TransferDirection,
        local: PathBuf,
        remote: RemotePath,
        size: u64,
    ) -> TransferId {
        let id = TransferId(self.next_id);
        self.next_id += 1;
        self.items.push(TransferItem {
            id,
            site,
            direction,
            local,
            remote,
            size,
            state: TransferState::Wait,
            // 아직 시작하지 않았다 — 자리를 배정받아 `Active`가 될 때 채운다
            started_at: None,
            finished_at: None,
            attempts: 0,
            retry_at: None,
        });
        id
    }

    /// 그 사이트에서 **지금 시작해도 되는** 전송들을 앞에서부터 골라 준다.
    ///
    /// `slots`는 연결 관리자가 배정한 전송 채널 수다 (D4). **`0`을 받아도 1로 본다** —
    /// 상한이 1인 사이트는 탐색 채널이 전송을 겸하므로(매니저만 아는 사정) 0을 그대로 쓰면
    /// 그 설정에서 전송이 영원히 시작되지 않는다.
    ///
    /// 이미 진행 중인 건수를 빼고 남는 자리만 내준다 — 한 사이트가 가득 차도 다른 사이트는
    /// 그대로 나온다(NFR-11)
    pub fn next_for(&self, site: SiteId, slots: u8, now: f64) -> Vec<TransferId> {
        if self.paused {
            return Vec::new();
        }
        let slots = slots.max(1) as usize;
        let active = self
            .items
            .iter()
            .filter(|item| item.site == site && item.state.is_active())
            .count();
        let free = slots.saturating_sub(active);
        self.items
            .iter()
            .filter(|item| item.site == site && item.state == TransferState::Wait)
            // 자동 재시도로 되돌아온 항목은 **지연이 지나야** 다시 나간다 (FR-37).
            // 그 자리를 건너뛸 뿐 뒤의 항목은 그대로 나가므로, 한 건이 쉬는 동안
            // 다른 전송이 멈추지 않는다
            .filter(|item| item.retry_at.is_none_or(|at| now >= at))
            .take(free)
            .map(|item| item.id)
            .collect()
    }

    /// 상태를 갈아 끼우고 **그 전이에 딸린 시각을 기록한다**. 없는 번호면 아무 일도 하지 않는다.
    ///
    /// 시각의 출처는 `set_wall_now`로 밀어 넣은 값이다 — 큐는 순수 모델이라 시계를 읽지 않는다
    /// (속도 계산이 `now`를 인자로 받는 것과 같은 이유: 시계를 두면 시험이 실제로 기다려야 한다).
    ///
    /// **기록 규칙 셋**:
    /// - `Active`로 갈 때 `started_at`이 **비어 있을 때만** 채운다 — 이어받기·재시도로 다시
    ///   `Active`가 돼도 처음 시작한 시각을 지킨다
    /// - `Done`·`Error`로 갈 때 `finished_at`을 덮어쓴다 — 재시도했으면 마지막 결과의 시각이다
    /// - `Wait`으로 되돌아가면 `finished_at`을 비운다 — 끝나지 않은 것에 끝 시각이 남아 있으면
    ///   화면이 "끝났는데 대기 중"이라는 없는 말을 하게 된다
    pub fn update(&mut self, id: TransferId, state: TransferState) {
        let now = self.wall_now;
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            match &state {
                TransferState::Active { .. } => {
                    item.started_at = item.started_at.or(now);
                }
                TransferState::Done | TransferState::Error { .. } | TransferState::Cancelled => {
                    item.finished_at = now;
                }
                TransferState::Wait => {
                    enter_wait(item);
                    return;
                }
            }
            item.state = state;
        }
    }

    /// 지금 벽시계 시각(FILETIME)을 큐에 알린다 — 앱이 프레임마다 한 번 민다.
    ///
    /// **큐가 스스로 시계를 읽지 않는 이유**: 순수 모델이어야 서버도 시계도 없이 상태 전이를
    /// 전부 시험할 수 있다. 밀어 넣지 않으면 `None`이라 시각이 기록되지 않을 뿐 동작은 같다
    pub fn set_wall_now(&mut self, now: u64) {
        // **`0`은 「모른다」다** — 시계를 읽지 못한 경우가 그 값으로 오는데(`ui::app`의
        // `system_time_now`), 그대로 담으면 시각 칸에 FILETIME 기점인 `1601-01-01`을
        // 가리키는 값이 기록된다. 코드베이스가 그 값을 「모른다」로 다루는 것과 맞춘다
        // (`panel::file_list::local_time_parts`·세션 왕복의 `0 ↔ None`)
        self.wall_now = (now != 0).then_some(now);
    }

    /// 저장된 시각을 그대로 심는다 — **세션 복원 전용**이다.
    ///
    /// `update`를 거치지 않는 이유: 복원은 `enqueue` 뒤에 실패 항목만 `update(Error)`를 부르는데
    /// 그 전이가 위 기록 규칙대로 `finished_at`을 **지금 시각으로 덮어쓴다**. 저장해 둔 값을
    /// 되살리려면 그 뒤에 이 함수로 다시 심어야 한다(`ui::session::restore_queue`).
    ///
    /// **복원 항목 전부에 부른다** — 실패만이 아니라 대기로 복원되는 것도 `started_at`을
    /// 갖고 있다(저장 대상은 `Done`이 아닌 전부다)
    pub fn set_times(&mut self, id: TransferId, started: Option<u64>, finished: Option<u64>) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == id) {
            item.started_at = started;
            item.finished_at = finished;
        }
    }

    /// 취소 — 항목을 **`취소됨`으로 남긴다**. 활성 자리는 그 자리에서 비워진다 (Acceptance ④).
    ///
    /// **종전에는 목록에서 지웠다**(「실패 탭은 서버가 거부한 것을 모아 보는 자리이므로 사용자가
    /// 스스로 그만둔 것을 섞지 않는다」). 2026-08-28에 사용자가 그 판단을 뒤집었다 — 지우면
    /// 무엇을 그만뒀는지 남지 않아 다시 걸 수도, 되짚을 수도 없다. 이제 `실패` 탭이 둘을 함께
    /// 담되 `이유` 열이 `사용자 취소`로 갈라 보인다.
    ///
    /// 목록에서 실제로 지우는 것은 `remove`(행 메뉴 `삭제`)다.
    ///
    /// 돌아오는 값은 **이번에 상태를 바꿨는가**다
    pub fn cancel(&mut self, id: TransferId) -> bool {
        match self.items.iter().find(|item| item.id == id) {
            // 이미 그만둔 것을 또 그만둘 것은 없다 — 끝 시각도 처음 것을 지킨다
            Some(item) if item.state.is_cancelled() => false,
            Some(_) => {
                self.update(id, TransferState::Cancelled);
                true
            }
            None => false,
        }
    }

    /// 실패한 전송의 수 — 상태 표시줄의 실패 알약이 쓴다 (인벤토리 #57).
    ///
    /// **`QueueFilter::Error`로 세지 않는다** — 그 거르개는 `실패` 탭이 취소분까지 담도록
    /// 넓혀졌는데, 사용자가 스스로 그만둔 것을 「실패 N건」으로 알리면 거짓말이 된다.
    /// 탭 배지·연결별 건수는 필터를 따르고(보이는 목록과 수가 맞아야 한다) 이 알약만 다르다
    pub fn failure_count(&self) -> usize {
        self.items
            .iter()
            .filter(|item| item.state.is_error())
            .count()
    }

    /// `⏸` 토글 — 멈추면 새 자리를 내주지 않는다.
    ///
    /// 진행 중인 전송을 여기서 되돌리지 않는다(그것은 워커에 닿아야 하는 일이라 T18의 몫이다) —
    /// 큐는 "새로 시작하지 않는다"만 안다
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// 고른 항목들을 다시 대기로 되돌린다 (행 메뉴 `다시 시도`·`전체 다시 시도`).
    ///
    /// **실패·취소만** 바꾼다 — 목록에 완료·진행 중이 섞여 있어도 그것들은 건드리지 않는다
    /// (`전체`라는 말이 "보이는 목록"을 뜻하지 "모든 상태"를 뜻하지는 않는다). 취소를 함께
    /// 담는 이유는 그것이 목록에 남는 목적이기 때문이다 — 그만둔 것을 그 자리에서 다시 건다
    pub fn retry(&mut self, ids: &[TransferId]) {
        let ids: HashSet<TransferId> = ids.iter().copied().collect();
        for item in self
            .items
            .iter_mut()
            .filter(|item| ids.contains(&item.id) && item.state.is_retryable())
        {
            // **자동 재시도 횟수를 0으로 되돌린다** (FR-37 · 2026-08-28 사용자 결정) —
            // 손으로 다시 거는 것은 서버를 고친 뒤의 새 시도라, 이미 쓴 횟수를 물려받으면
            // 한 번 만에 다시 굳는다. 지연도 두지 않는다(지금 걸겠다는 뜻이다)
            item.attempts = 0;
            item.retry_at = None;
            enter_wait(item);
        }
    }

    /// 자동 재시도로 그 항목을 대기로 되돌린다 — 실행기가 부른다 (FR-37).
    ///
    /// 되돌렸으면 `true`, 상한을 다 썼으면 `false`(그때는 실행기가 실패로 굳힌다)
    pub fn retry_automatically(&mut self, id: TransferId, limit: u32, now: f64) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| item.id == id) else {
            return false;
        };
        if item.attempts >= limit {
            return false;
        }
        enter_retry_wait(item, now);
        true
    }

    /// 고른 항목들을 목록에서 지운다 (행 메뉴 `삭제`·`전체 삭제`의 여러 건 판).
    ///
    /// 진행 중인 것의 워커 정지와 `.part` 정리는 `transfer::TransferRunner::cancel`이 맡는다 —
    /// 큐는 목록만 안다(`cancel`과 같은 분담)
    pub fn remove(&mut self, ids: &[TransferId]) {
        let ids: HashSet<TransferId> = ids.iter().copied().collect();
        self.items.retain(|item| !ids.contains(&item.id));
    }

    /// 끝난 것들을 치운다 (`✕` — 인벤토리 #33). 실패는 남긴다 — 다시 걸 수 있어야 한다
    pub fn clear_done(&mut self) {
        self.items.retain(|item| !item.state.is_done());
    }

    /// 연결이 끊긴 사이트의 진행 중 항목을 **대기로 되돌린다** (plan Edge Case).
    ///
    /// 실패로 두지 않는 이유: 서버가 거부한 것이 아니라 우리 쪽 연결이 사라진 것이고,
    /// 다시 연결되면 그대로 이어 보내면 된다
    pub fn requeue_site(&mut self, site: SiteId) {
        for item in self
            .items
            .iter_mut()
            .filter(|item| item.site == site && item.state.is_active())
        {
            enter_wait(item);
        }
    }

    /// 필터에 걸리는 항목들 — 화면이 그대로 그린다
    pub fn filter(&self, filter: QueueFilter) -> Vec<&TransferItem> {
        self.iter_filter(filter).collect()
    }

    /// 필터에 걸리는 항목들을 모으지 않고 차례로 준다 — 다른 조건을 더 걸어 한 번에 모으는 쪽이 쓴다
    pub fn iter_filter(&self, filter: QueueFilter) -> impl Iterator<Item = &TransferItem> {
        self.items
            .iter()
            .filter(move |item| filter.matches(&item.state))
    }

    /// 그 사이트의 항목들 — 등록 순서 그대로다.
    ///
    /// 사이트를 목록에서 지울 때(`ui::app::detach_site`) 쓴다. **한 번 훑어 두 가지를 얻는다** —
    /// 지울 번호와 아직 끝나지 않은 건수(확인 대화를 띄울지 가른다). 상태로 거르지 않는 것은
    /// 그 판정이 호출부마다 다르기 때문이다
    pub fn site_items(&self, site: SiteId) -> Vec<&TransferItem> {
        self.items.iter().filter(|item| item.site == site).collect()
    }

    /// 필터별 건수 — 탭 라벨의 `(N)`이다 (인벤토리 #29·#31·#32)
    pub fn count(&self, filter: QueueFilter) -> usize {
        self.items
            .iter()
            .filter(|item| filter.matches(&item.state))
            .count()
    }

    /// 대기 건수·속도 합·남은 시간 (인벤토리 #54).
    ///
    /// 남은 시간은 **남은 바이트 ÷ 지금 속도**다 — 속도가 0이면 나눌 수 없어 `None`이고
    /// 화면은 `—`를 보인다(0초로 적으면 곧 끝난다는 거짓말이 된다)
    pub fn summary(&self) -> QueueSummary {
        let mut pending = 0usize;
        let mut speed = 0u64;
        let mut remaining = 0u64;
        for item in &self.items {
            if item.state.is_pending() {
                pending += 1;
                remaining = remaining.saturating_add(item.remaining());
            }
            if let TransferState::Active { speed: rate, .. } = &item.state {
                speed = speed.saturating_add(*rate);
            }
        }
        QueueSummary {
            pending,
            speed,
            eta_secs: (speed > 0).then(|| remaining / speed),
        }
    }

    /// 전체 진행률 0.0~1.0 — 상태 표시줄의 막대다 (인벤토리 #55).
    ///
    /// 크기를 아는 항목만 센다. 아무것도 없으면 `None`이라 막대를 비워 둔다
    pub fn overall_progress(&self) -> Option<f32> {
        let mut total = 0u64;
        let mut sent = 0u64;
        for item in self.items.iter().filter(|item| item.size > 0) {
            total = total.saturating_add(item.size);
            sent = sent.saturating_add(match &item.state {
                TransferState::Active { sent, .. } => *sent,
                TransferState::Done => item.size,
                _ => 0,
            });
        }
        (total > 0).then(|| (sent as f32 / total as f32).clamp(0.0, 1.0))
    }

    /// 사이트별 건수 — 연결별 탭의 `(N)`이다 (인벤토리 #36).
    ///
    /// **거르개를 함께 받는다** — `성공` 탭을 보는데 아래 줄이 전체 건수를 적으면
    /// 목록이 비어 있는데 `(1)`이 서 있게 된다 (2026-08-18 사용자 보고).
    /// 탭에 어떤 사이트가 서는지(멤버십)는 이 건수로 정하지 않는다 — 그것까지 걸러 내면
    /// 그 거르개에 항목이 없는 서버가 탭에서 통째로 사라진다(호출부가 `All`로 따로 구한다)
    pub fn counts_by_site(&self, filter: QueueFilter) -> HashMap<SiteId, usize> {
        let mut counts = HashMap::new();
        for item in self.items.iter().filter(|item| filter.matches(&item.state)) {
            *counts.entry(item.site).or_insert(0) += 1;
        }
        counts
    }
}

/// 항목을 대기로 되돌린다 — **상태와 끝 시각을 함께 손대는 것이 한 벌이다**.
///
/// 대기로 가는 길이 셋이라(`update`의 `Wait` 전이 · `retry` 전체 다시 시도 ·
/// `requeue_site` 연결 끊김) 각자 상태만 대입하면 그중 하나가 끝 시각을 남긴다 —
/// 실제로 `retry`가 그랬고, 그러면 화면이 「대기 중인데 끝난 시각이 있다」는
/// 없는 말을 하게 된다. 규칙을 한 함수에 모아 셋이 같은 길로만 지나가게 한다
fn enter_wait(item: &mut TransferItem) {
    item.state = TransferState::Wait;
    item.finished_at = None;
}

/// 자동 재시도로 대기로 되돌린다 — 횟수를 올리고 다음 시도 시각을 잡는다 (FR-37).
///
/// **`retry`(사용자가 손으로 누른 것)와 다른 함수인 이유**: 그쪽은 횟수를 0으로 되돌리고
/// 지연도 두지 않는다(사용자가 서버를 고친 뒤 지금 다시 걸겠다는 뜻이다). 여기는 그 반대로,
/// 자동으로 도는 것이라 횟수를 세고 사이를 띄운다
fn enter_retry_wait(item: &mut TransferItem, now: f64) {
    item.attempts += 1;
    item.retry_at = Some(now + retry_delay_secs(item.attempts));
    enter_wait(item);
}

/// 다음 재시도까지 기다릴 시간(초) — 1 → 2 → 4로 늘리되 **60초에서 죈다**.
///
/// 늘리는 이유: 같은 오류가 곧바로 반복될 때 서버를 쉬지 않고 두드리지 않기 위해서다.
/// 죄는 이유: 상한이 10이면 마지막은 `2^9 = 512초`(8분 반)가 되는데, 그쯤이면 사용자가
/// 앱이 멈춘 것으로 본다
pub fn retry_delay_secs(attempts: u32) -> f64 {
    /// 그보다 길면 사용자가 「멈췄다」고 본다
    const MAX_DELAY_SECS: f64 = 60.0;
    let steps = attempts.saturating_sub(1).min(u32::BITS - 1);
    f64::from(1u32 << steps).min(MAX_DELAY_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(n: u32) -> SiteId {
        SiteId(n)
    }

    /// 시험용 FILETIME — 실제 시각일 필요는 없고 서로 다르기만 하면 된다
    const 시각_A: u64 = 133_000_000_000_000_000;
    const 시각_B: u64 = 133_000_000_010_000_000;
    const 시각_C: u64 = 133_000_000_020_000_000;

    fn 한건() -> (TransferQueue, TransferId) {
        let mut queue = TransferQueue::new();
        let id = queue.enqueue(
            site(1),
            TransferDirection::Download,
            PathBuf::from(r"C:\받은 것\a.txt"),
            RemotePath::new("/pub/a.txt"),
            100,
        );
        (queue, id)
    }

    #[test]
    fn 시작과_완료_시각을_전이에서_적는다() {
        let (mut queue, id) = 한건();
        // 아직 시작하지 않았다
        assert_eq!(queue.get(id).unwrap().started_at, None);
        assert_eq!(queue.get(id).unwrap().finished_at, None);

        queue.set_wall_now(시각_A);
        queue.update(id, TransferState::Active { sent: 0, speed: 0 });
        assert_eq!(queue.get(id).unwrap().started_at, Some(시각_A));
        assert_eq!(
            queue.get(id).unwrap().finished_at,
            None,
            "아직 끝나지 않았다"
        );

        queue.set_wall_now(시각_B);
        queue.update(id, TransferState::Done);
        assert_eq!(
            queue.get(id).unwrap().started_at,
            Some(시각_A),
            "시작은 그대로다"
        );
        assert_eq!(queue.get(id).unwrap().finished_at, Some(시각_B));
    }

    #[test]
    fn 이어받아_다시_시작해도_시작_시각은_처음_값이다() {
        // 사용자가 알고 싶은 것은 「언제부터 이 파일을 옮기기 시작했는가」이지
        // 마지막 시도가 언제였는가가 아니다 (`⏸` → 재개, 재시도 모두 이 길로 온다)
        let (mut queue, id) = 한건();
        queue.set_wall_now(시각_A);
        queue.update(id, TransferState::Active { sent: 0, speed: 0 });

        // 멈췄다가
        queue.set_wall_now(시각_B);
        queue.update(id, TransferState::Wait);
        assert_eq!(queue.get(id).unwrap().started_at, Some(시각_A));
        assert_eq!(
            queue.get(id).unwrap().finished_at,
            None,
            "대기로 돌아가면 끝 시각을 비운다 — 끝나지 않은 것에 끝 시각이 남으면 안 된다"
        );

        // 다시 시작해도 처음 시각을 지킨다
        queue.set_wall_now(시각_C);
        queue.update(
            id,
            TransferState::Active {
                sent: 50,
                speed: 10,
            },
        );
        assert_eq!(queue.get(id).unwrap().started_at, Some(시각_A));
    }

    #[test]
    fn 실패했다_다시_끝나면_끝_시각은_마지막_것이다() {
        let (mut queue, id) = 한건();
        queue.set_wall_now(시각_A);
        queue.update(id, TransferState::Active { sent: 0, speed: 0 });
        queue.set_wall_now(시각_B);
        queue.update(
            id,
            TransferState::Error {
                message: "550 실패".to_owned(),
            },
        );
        assert_eq!(queue.get(id).unwrap().finished_at, Some(시각_B));

        // 다시 걸어 이번엔 성공 — 끝 시각이 갱신된다
        queue.update(id, TransferState::Wait);
        queue.set_wall_now(시각_C);
        queue.update(id, TransferState::Done);
        assert_eq!(queue.get(id).unwrap().finished_at, Some(시각_C));
        assert_eq!(queue.get(id).unwrap().started_at, Some(시각_A));
    }

    #[test]
    fn 시각을_밀어_넣지_않으면_기록하지_않는다() {
        // 큐는 스스로 시계를 읽지 않는다 — 밀어 넣기 전이면 시각이 비고 상태 전이만 돈다
        let (mut queue, id) = 한건();
        queue.update(id, TransferState::Active { sent: 0, speed: 0 });
        queue.update(id, TransferState::Done);
        assert_eq!(queue.get(id).unwrap().started_at, None);
        assert_eq!(queue.get(id).unwrap().finished_at, None);
        assert!(
            queue.get(id).unwrap().state.is_done(),
            "상태 전이는 그대로 돈다"
        );
    }

    #[test]
    fn 대기로_되돌리는_세_길이_모두_끝_시각을_비운다() {
        // **개별 `다시 시도`는 `update`를, `전체 다시 시도`는 `retry`를, 연결 끊김은
        // `requeue_site`를 지난다** — 셋 중 하나만 끝 시각을 남겨도 화면이 「대기 중인데
        // 끝난 시각이 있다」는 없는 말을 한다(리뷰가 `retry`에서 실제로 잡았다)
        let 실패한_큐 = || {
            let (mut queue, id) = 한건();
            queue.set_wall_now(시각_A);
            queue.update(id, TransferState::Active { sent: 0, speed: 0 });
            queue.set_wall_now(시각_B);
            queue.update(
                id,
                TransferState::Error {
                    message: "550 실패".to_owned(),
                },
            );
            assert_eq!(queue.get(id).unwrap().finished_at, Some(시각_B));
            (queue, id)
        };

        // ① 그냥 대기로 되돌리는 길 (`update`의 `Wait` 전이 — 연결 끊김 복구 등)
        let (mut queue, id) = 실패한_큐();
        queue.update(id, TransferState::Wait);
        assert_eq!(queue.get(id).unwrap().finished_at, None, "대기 전이");

        // ② 손으로 누른 `다시 시도`·`전체 다시 시도` (`ui::app`의 `QueueAction::Retry`·`RetryAll`
        //    이 **둘 다** 이 길을 쓴다 — 2026-08-28에 단건도 여기로 통일했다)
        let (mut queue, id) = 실패한_큐();
        queue.retry(&[id]);
        assert!(queue.get(id).unwrap().state == TransferState::Wait);
        assert_eq!(queue.get(id).unwrap().finished_at, None, "전체 다시 시도");
        assert_eq!(
            queue.get(id).unwrap().started_at,
            Some(시각_A),
            "다시 걸어도 처음 시작 시각은 지킨다"
        );

        // ③ 연결이 끊겨 되돌아간 길 — 진행 중이던 것이 대상이라 끝 시각이 원래 없지만,
        //    같은 규칙을 지나야 나중에 갈리지 않는다
        let (mut queue, id) = 한건();
        queue.set_wall_now(시각_A);
        queue.update(id, TransferState::Active { sent: 0, speed: 0 });
        queue.requeue_site(site(1));
        assert!(queue.get(id).unwrap().state == TransferState::Wait);
        assert_eq!(queue.get(id).unwrap().finished_at, None, "연결 끊김");
    }

    #[test]
    fn 시계를_읽지_못한_프레임은_시각을_적지_않는다() {
        // `system_time_now`가 실패하면 0을 준다 — 그대로 담으면 FILETIME 기점인
        // 1601년을 가리키는 값이 기록된다
        let (mut queue, id) = 한건();
        queue.set_wall_now(0);
        queue.update(id, TransferState::Active { sent: 0, speed: 0 });
        queue.update(id, TransferState::Done);
        assert_eq!(queue.get(id).unwrap().started_at, None);
        assert_eq!(queue.get(id).unwrap().finished_at, None);
    }

    #[test]
    fn 복원_전용_설정은_상태_전이를_거치지_않는다() {
        // `set_times`는 저장된 값을 그대로 심는다 — `update`의 기록 규칙에 걸리지 않아야
        // 복원이 「지금 시각」으로 덮이지 않는다
        let (mut queue, id) = 한건();
        queue.set_wall_now(시각_C);
        queue.set_times(id, Some(시각_A), Some(시각_B));
        assert_eq!(queue.get(id).unwrap().started_at, Some(시각_A));
        assert_eq!(queue.get(id).unwrap().finished_at, Some(시각_B));
    }

    fn queue_with(count: usize, site_id: SiteId) -> (TransferQueue, Vec<TransferId>) {
        let mut queue = TransferQueue::new();
        let ids = (0..count)
            .map(|i| {
                queue.enqueue(
                    site_id,
                    TransferDirection::Download,
                    PathBuf::from(format!(r"C:\down\{i}.bin")),
                    RemotePath::new(&format!("/pub/{i}.bin")),
                    1000,
                )
            })
            .collect();
        (queue, ids)
    }

    #[test]
    fn 배정된_자리_수만큼만_활성으로_내준다() {
        // Acceptance ① — 고정 2가 아니라 매니저가 보고한 값이다 (D4)
        let (mut queue, ids) = queue_with(5, site(1));
        assert_eq!(queue.next_for(site(1), 2, 0.0).len(), 2);

        queue.update(ids[0], TransferState::Active { sent: 0, speed: 0 });
        queue.update(ids[1], TransferState::Active { sent: 0, speed: 0 });
        assert!(
            queue.next_for(site(1), 2, 0.0).is_empty(),
            "자리가 찼는데 더 내줬다"
        );
        // 한 건이 끝나면 그 자리가 다음 대기에게 간다
        queue.update(ids[0], TransferState::Done);
        assert_eq!(queue.next_for(site(1), 2, 0.0), vec![ids[2]]);
    }

    #[test]
    fn 상한이_1인_사이트에도_한_건은_내준다() {
        // 2차 리뷰 M1 — 0을 그대로 쓰면 그 설정에서 전송이 영원히 시작되지 않는다.
        // 큐는 탐색 채널이 전송을 겸한다는 사정을 모른다
        let (queue, ids) = queue_with(3, site(1));
        assert_eq!(queue.next_for(site(1), 0, 0.0), vec![ids[0]]);
        assert_eq!(queue.next_for(site(1), 1, 0.0), vec![ids[0]]);
    }

    #[test]
    fn 한_사이트가_가득_차도_다른_사이트는_계속_나온다() {
        // NFR-11 — 한 서버의 지연이 다른 서버의 전송을 막지 않는다
        let mut queue = TransferQueue::new();
        let busy = queue.enqueue(
            site(1),
            TransferDirection::Upload,
            PathBuf::from(r"C:\a.bin"),
            RemotePath::new("/a.bin"),
            10,
        );
        let other = queue.enqueue(
            site(2),
            TransferDirection::Upload,
            PathBuf::from(r"C:\b.bin"),
            RemotePath::new("/b.bin"),
            10,
        );
        queue.update(busy, TransferState::Active { sent: 0, speed: 0 });

        assert!(queue.next_for(site(1), 1, 0.0).is_empty());
        assert_eq!(queue.next_for(site(2), 1, 0.0), vec![other]);
    }

    #[test]
    fn 필터는_같은_집합을_거르고_건수가_전체와_맞는다() {
        // Acceptance ② (README §6) — 성공·실패 탭이 전체의 부분집합이다
        let (mut queue, ids) = queue_with(4, site(1));
        queue.update(ids[0], TransferState::Done);
        queue.update(ids[1], TransferState::Done);
        queue.update(
            ids[2],
            TransferState::Error {
                message: "550 권한 거부".to_owned(),
            },
        );

        assert_eq!(queue.count(QueueFilter::All), 4);
        assert_eq!(queue.count(QueueFilter::Done), 2);
        assert_eq!(queue.count(QueueFilter::Error), 1);
        // 끝나지 않은 것 1건을 더하면 전체가 된다
        let pending = queue
            .items()
            .iter()
            .filter(|item| item.state.is_pending())
            .count();
        assert_eq!(
            queue.count(QueueFilter::Done) + queue.count(QueueFilter::Error) + pending,
            queue.count(QueueFilter::All)
        );
        // 거른 항목이 실제로 그 상태다
        assert!(
            queue
                .filter(QueueFilter::Done)
                .iter()
                .all(|item| item.state.is_done())
        );
        // `실패` 탭은 서버가 거부한 것과 사용자가 그만둔 것을 함께 담는다 (2026-08-28)
        assert!(
            queue
                .filter(QueueFilter::Error)
                .iter()
                .all(|item| item.state.is_retryable())
        );
    }

    #[test]
    fn 요약은_대기_건수와_속도와_남은_시간을_센다() {
        // Acceptance ③
        let (mut queue, ids) = queue_with(3, site(1));
        queue.update(
            ids[0],
            TransferState::Active {
                sent: 400,
                speed: 200,
            },
        );
        queue.update(ids[1], TransferState::Done);

        let summary = queue.summary();
        assert_eq!(summary.pending, 2, "진행 1 + 대기 1");
        assert_eq!(summary.speed, 200);
        // 남은 바이트 = 진행 600 + 대기 1000 = 1600, 속도 200 → 8초
        assert_eq!(summary.eta_secs, Some(8));
    }

    #[test]
    fn 속도가_0이면_남은_시간을_적지_않는다() {
        // Acceptance ③ 뒷문장 — 0초로 적으면 곧 끝난다는 거짓말이 된다
        let (queue, _) = queue_with(2, site(1));
        let summary = queue.summary();
        assert_eq!(summary.pending, 2);
        assert_eq!(summary.speed, 0);
        assert_eq!(summary.eta_secs, None);
        assert_eq!(UNKNOWN, "—");
    }

    #[test]
    fn 취소하면_활성_자리가_곧바로_빈다() {
        // Acceptance ④ — 자리는 비우되 **항목은 남긴다**(2026-08-28 결정)
        let (mut queue, ids) = queue_with(3, site(1));
        queue.update(ids[0], TransferState::Active { sent: 10, speed: 5 });
        assert!(queue.next_for(site(1), 1, 0.0).is_empty());

        assert!(queue.cancel(ids[0]));
        assert_eq!(queue.len(), 3, "취소한 항목은 목록에 남는다");
        assert_eq!(
            queue.get(ids[0]).expect("항목").state,
            TransferState::Cancelled
        );
        assert_eq!(
            queue.next_for(site(1), 1, 0.0),
            vec![ids[1]],
            "취소한 자리는 그 자리에서 다음 것에 내준다"
        );

        // 이미 그만둔 것을 또 그만둘 것은 없다
        assert!(!queue.cancel(ids[0]));
        // 없는 번호를 취소해도 조용히 아무 일도 없다
        assert!(!queue.cancel(TransferId(9999)));
    }

    /// 취소한 시각이 남아 `시간` 열이 「시작 ~ 취소」를 적을 수 있다
    #[test]
    fn 취소한_시각이_끝_시각으로_남는다() {
        let (mut queue, ids) = queue_with(1, site(1));
        queue.set_wall_now(1_000);
        queue.update(ids[0], TransferState::Active { sent: 1, speed: 1 });
        queue.set_wall_now(2_000);
        queue.cancel(ids[0]);

        let item = queue.get(ids[0]).expect("항목");
        assert_eq!(item.started_at, Some(1_000), "시작 시각은 그대로다");
        assert_eq!(item.finished_at, Some(2_000), "취소한 시각이 끝 시각이다");
    }

    /// `삭제`는 종전대로 목록에서 지운다 — `전송 취소`와 하는 일이 갈렸다
    #[test]
    fn 삭제는_취소와_달리_목록에서_지운다() {
        let (mut queue, ids) = queue_with(2, site(1));
        queue.cancel(ids[0]);
        assert_eq!(queue.len(), 2);

        queue.remove(&[ids[0]]);
        assert_eq!(queue.len(), 1, "삭제한 항목은 사라진다");
        assert!(queue.get(ids[0]).is_none());
    }

    /// 취소분은 `실패` 탭에 서고 `성공` 탭에는 서지 않는다
    #[test]
    fn 취소분은_실패_거르개에_담긴다() {
        let (mut queue, ids) = queue_with(3, site(1));
        queue.cancel(ids[0]);
        queue.update(ids[1], TransferState::Done);
        queue.update(
            ids[2],
            TransferState::Error {
                message: "550 거부".to_owned(),
            },
        );

        assert_eq!(queue.count(QueueFilter::All), 3);
        assert_eq!(queue.count(QueueFilter::Done), 1, "취소는 성공이 아니다");
        assert_eq!(
            queue.count(QueueFilter::Error),
            2,
            "취소와 실패가 함께 담긴다"
        );
    }

    /// 상태 표시줄의 실패 알약만은 취소를 세지 않는다 — 스스로 그만둔 것을 실패로 알리지 않는다
    #[test]
    fn 실패_알약은_취소를_세지_않는다() {
        let (mut queue, ids) = queue_with(2, site(1));
        queue.cancel(ids[0]);
        assert_eq!(queue.failure_count(), 0);

        queue.update(
            ids[1],
            TransferState::Error {
                message: "550 거부".to_owned(),
            },
        );
        assert_eq!(queue.failure_count(), 1);
        assert_eq!(
            queue.count(QueueFilter::Error),
            2,
            "탭 배지는 둘을 함께 센다 — 보이는 목록과 수가 맞아야 한다"
        );
    }

    /// 손으로 누른 `다시 시도`는 취소분도 되살리고 자동 재시도 횟수를 0으로 되돌린다
    #[test]
    fn 다시_시도는_취소분도_되살린다() {
        let (mut queue, ids) = queue_with(2, site(1));
        queue.retry_automatically(ids[0], 3, 0.0);
        queue.update(ids[0], TransferState::Active { sent: 1, speed: 1 });
        queue.cancel(ids[0]);
        queue.update(ids[1], TransferState::Done);

        queue.retry(&[ids[0], ids[1]]);
        let 취소분 = queue.get(ids[0]).expect("항목");
        assert_eq!(취소분.state, TransferState::Wait, "취소분이 되살아난다");
        assert_eq!(취소분.attempts, 0, "자동 재시도 횟수가 0으로 돌아간다");
        assert_eq!(
            queue.get(ids[1]).expect("항목").state,
            TransferState::Done,
            "끝난 것은 건드리지 않는다"
        );
    }

    #[test]
    fn 사이트별_건수가_연결별_탭의_숫자다() {
        // Acceptance ⑤ (인벤토리 #36)
        let mut queue = TransferQueue::new();
        for _ in 0..3 {
            queue.enqueue(
                site(1),
                TransferDirection::Download,
                PathBuf::from(r"C:\a"),
                RemotePath::new("/a"),
                1,
            );
        }
        queue.enqueue(
            site(2),
            TransferDirection::Download,
            PathBuf::from(r"C:\b"),
            RemotePath::new("/b"),
            1,
        );

        let counts = queue.counts_by_site(QueueFilter::All);
        assert_eq!(counts.get(&site(1)), Some(&3));
        assert_eq!(counts.get(&site(2)), Some(&1));
        assert_eq!(counts.values().sum::<usize>(), queue.len());
    }

    #[test]
    fn 목록_단위로_다시_걸고_지운다() {
        // 2026-08-18 행 메뉴 `전체 다시 시도`·`전체 삭제` — 대상은 호출부가 고른 번호들이다
        let mut queue = TransferQueue::new();
        let 건다 = |queue: &mut TransferQueue, n: u8| {
            queue.enqueue(
                site(1),
                TransferDirection::Upload,
                PathBuf::from(format!(r"C:\{n}")),
                RemotePath::new(&format!("/{n}")),
                1,
            )
        };
        let 실패 = 건다(&mut queue, 1);
        let 완료 = 건다(&mut queue, 2);
        let 대기 = 건다(&mut queue, 3);
        queue.update(
            실패,
            TransferState::Error {
                message: "550".to_owned(),
            },
        );
        queue.update(완료, TransferState::Done);

        // `retry`는 **실패한 것만** 되돌린다 — 목록에 섞인 완료·대기는 그대로
        queue.retry(&[실패, 완료, 대기]);
        assert_eq!(queue.count(QueueFilter::Error), 0);
        assert_eq!(queue.count(QueueFilter::Done), 1, "완료는 그대로여야 한다");
        assert_eq!(queue.len(), 3);

        // `remove`는 상태를 가리지 않고 고른 것을 지운다
        queue.remove(&[실패, 대기]);
        assert_eq!(queue.len(), 1);
        assert!(queue.get(완료).is_some());

        // 목록에 없는 번호를 섞어도 조용히 지나간다
        queue.remove(&[TransferId(9999)]);
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn 사이트별_건수가_거르개를_따른다() {
        // 2026-08-18 — `성공` 탭인데 아래 줄이 전체 건수를 적던 것을 고친다
        let mut queue = TransferQueue::new();
        let 실패 = queue.enqueue(
            site(1),
            TransferDirection::Upload,
            PathBuf::from(r"C:\a"),
            RemotePath::new("/a"),
            1,
        );
        let 완료 = queue.enqueue(
            site(1),
            TransferDirection::Upload,
            PathBuf::from(r"C:\b"),
            RemotePath::new("/b"),
            1,
        );
        queue.update(
            실패,
            TransferState::Error {
                message: "550".to_owned(),
            },
        );
        queue.update(완료, TransferState::Done);

        assert_eq!(
            queue.counts_by_site(QueueFilter::All).get(&site(1)),
            Some(&2)
        );
        assert_eq!(
            queue.counts_by_site(QueueFilter::Done).get(&site(1)),
            Some(&1)
        );
        assert_eq!(
            queue.counts_by_site(QueueFilter::Error).get(&site(1)),
            Some(&1)
        );
        // 거르개에 하나도 안 걸리면 그 사이트 자리 자체가 없다 —
        // 탭을 세우는 쪽은 `All` 집계를 따로 봐야 한다(호출부 규약)
        let mut 완료만 = TransferQueue::new();
        let 그것 = 완료만.enqueue(
            site(2),
            TransferDirection::Upload,
            PathBuf::from(r"C:\c"),
            RemotePath::new("/c"),
            1,
        );
        완료만.update(그것, TransferState::Done);
        assert!(
            !완료만
                .counts_by_site(QueueFilter::Error)
                .contains_key(&site(2))
        );
    }

    #[test]
    fn 일시정지_중에는_새_전송이_시작되지_않는다() {
        // plan Edge Case — 멈춘 동안 등록한 것은 대기에만 쌓인다
        let (mut queue, ids) = queue_with(2, site(1));
        queue.set_paused(true);
        assert!(queue.is_paused());
        assert!(queue.next_for(site(1), 2, 0.0).is_empty());

        queue.enqueue(
            site(1),
            TransferDirection::Upload,
            PathBuf::from(r"C:\c"),
            RemotePath::new("/c"),
            1,
        );
        assert_eq!(queue.len(), 3);

        queue.set_paused(false);
        assert_eq!(queue.next_for(site(1), 1, 0.0), vec![ids[0]]);
    }

    #[test]
    fn 사이트로_고른_항목은_그_사이트_것뿐이다() {
        // 사이트를 목록에서 지울 때 지울 번호를 모으는 길이다 — 다른 사이트가 섞이면
        // 멀쩡한 전송이 함께 취소된다
        let (mut queue, first) = queue_with(2, site(1));
        let second = queue.enqueue(
            site(2),
            TransferDirection::Upload,
            PathBuf::from(r"C:\b"),
            RemotePath::new("/b"),
            1,
        );
        queue.update(first[0], TransferState::Done);

        let picked: Vec<TransferId> = queue.site_items(site(1)).iter().map(|i| i.id).collect();
        // 끝난 것도 함께 나온다 — 거르는 것은 호출부의 몫이다
        assert_eq!(picked, first);
        assert_eq!(
            queue
                .site_items(site(2))
                .iter()
                .map(|i| i.id)
                .collect::<Vec<_>>(),
            vec![second]
        );
        assert!(queue.site_items(site(9)).is_empty(), "없는 사이트");
    }

    #[test]
    fn 연결이_끊기면_진행_중이던_것이_대기로_돌아간다() {
        // plan Edge Case — 서버가 거부한 것이 아니라 우리 쪽 연결이 사라진 것이다
        let (mut queue, ids) = queue_with(2, site(1));
        queue.update(
            ids[0],
            TransferState::Active {
                sent: 50,
                speed: 10,
            },
        );
        queue.requeue_site(site(1));
        assert_eq!(queue.get(ids[0]).expect("항목").state, TransferState::Wait);
        // 실패로 남지 않아 다시 걸 것이 없다
        assert_eq!(queue.count(QueueFilter::Error), 0);
    }

    #[test]
    fn 끝난_것만_치우고_실패는_남긴다() {
        let (mut queue, ids) = queue_with(3, site(1));
        queue.update(ids[0], TransferState::Done);
        queue.update(
            ids[1],
            TransferState::Error {
                message: "550".to_owned(),
            },
        );
        queue.clear_done();
        assert_eq!(queue.len(), 2, "실패와 대기는 남는다");
        assert_eq!(queue.count(QueueFilter::Done), 0);
        assert_eq!(queue.count(QueueFilter::Error), 1);
    }

    #[test]
    fn 크기를_모르면_진행률이_없다() {
        // plan Edge Case — 0으로 나눌 수 없고, 0%로 적으면 시작도 안 한 것처럼 보인다
        let mut queue = TransferQueue::new();
        let id = queue.enqueue(
            site(1),
            TransferDirection::Download,
            PathBuf::from(r"C:\x"),
            RemotePath::new("/x"),
            0,
        );
        assert_eq!(queue.get(id).expect("항목").progress(), None);
        // 크기를 아는 항목이 하나도 없으면 전체 진행률도 없다
        assert_eq!(queue.overall_progress(), None);
    }

    #[test]
    fn 전체_진행률은_크기를_아는_항목만_센다() {
        let (mut queue, ids) = queue_with(2, site(1));
        queue.update(ids[0], TransferState::Done);
        queue.update(
            ids[1],
            TransferState::Active {
                sent: 500,
                speed: 1,
            },
        );
        // (1000 + 500) / 2000
        assert_eq!(queue.overall_progress(), Some(0.75));
        assert_eq!(queue.get(ids[1]).expect("항목").progress(), Some(0.5));
        assert_eq!(queue.get(ids[0]).expect("항목").progress(), Some(1.0));
    }

    #[test]
    fn 같은_파일을_두_번_넣어도_막지_않는다() {
        // plan Edge Case — 덮어쓰기 확인은 쓰기 직전에 실행기가 한다.
        // 여기서 막으면 사용자가 일부러 다시 받는 것도 막힌다
        let mut queue = TransferQueue::new();
        let first = queue.enqueue(
            site(1),
            TransferDirection::Download,
            PathBuf::from(r"C:\same.bin"),
            RemotePath::new("/same.bin"),
            10,
        );
        let second = queue.enqueue(
            site(1),
            TransferDirection::Download,
            PathBuf::from(r"C:\same.bin"),
            RemotePath::new("/same.bin"),
            10,
        );
        assert_ne!(first, second, "번호는 따로 발급된다");
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn 빈_큐도_말이_되는_값을_준다() {
        let queue = TransferQueue::new();
        assert!(queue.is_empty());
        assert_eq!(queue.count(QueueFilter::All), 0);
        assert_eq!(queue.overall_progress(), None);
        assert!(queue.counts_by_site(QueueFilter::All).is_empty());
        let summary = queue.summary();
        assert_eq!(summary.pending, 0);
        assert_eq!(summary.eta_secs, None);
        assert!(queue.next_for(site(1), 3, 0.0).is_empty());
    }

    #[test]
    fn 만_건에서도_요약과_필터가_한_번씩만_훑는다() {
        // Acceptance ⑥ — 화면이 매 프레임 부르므로 항목마다 다시 훑으면 제곱이 된다.
        // 계산량 자체는 재기 어려워, **한 번 훑어 나오는 값**과 일치하는지로 고정한다
        let (mut queue, ids) = queue_with(10_000, site(1));
        for id in ids.iter().take(2_000) {
            queue.update(*id, TransferState::Done);
        }
        let summary = queue.summary();
        assert_eq!(summary.pending, 8_000);
        assert_eq!(queue.count(QueueFilter::Done), 2_000);
        assert_eq!(queue.filter(QueueFilter::All).len(), 10_000);
        assert_eq!(queue.overall_progress(), Some(0.2));
    }

    #[test]
    fn 전부_실패해도_요약이_성립한다() {
        // plan Edge Case
        let (mut queue, ids) = queue_with(3, site(1));
        for id in &ids {
            queue.update(
                *id,
                TransferState::Error {
                    message: "550 권한 거부".to_owned(),
                },
            );
        }
        let summary = queue.summary();
        assert_eq!(summary.pending, 0);
        assert_eq!(summary.speed, 0);
        assert_eq!(summary.eta_secs, None);
        assert_eq!(queue.count(QueueFilter::Error), 3);
        // 실패는 보낸 것이 없으니 전체 진행률은 0이다(막대가 비어 보인다)
        assert_eq!(queue.overall_progress(), Some(0.0));
    }
}
