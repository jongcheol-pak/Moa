//! 전송 실행기 — 큐(T17)와 연결 워커(T4)를 잇는 고리 (FR-37).
//!
//! 큐는 "무엇을 보낼지"만 알고 워커는 "어떻게 보낼지"만 안다. 그 사이에서 **누가 언제 어느
//! 연결로 보낼지**를 정하는 것이 여기다:
//!
//! - 사이트마다 배정된 전송 자리(D4)만큼만 동시에 시작한다
//! - 다운로드는 **`<이름>.part`로 받고 끝나면 이름을 바꾼다** — 받다 만 파일이 완성본처럼
//!   보이면 사용자가 그것을 열어 본다 (Acceptance ⑤)
//! - 끊긴 전송은 `.part` 크기에서 **이어받는다**. 사용자가 스스로 취소한 것은 `.part`를 지운다
//! - 속도는 여기서 잰다 — 워커는 누적 바이트만 올린다(시계를 두 곳에 두면 값이 갈린다)
//!
//! **I/O는 하지 않는다**(`.part` 이름 바꾸기·지우기 제외) — 실제 바이트는 워커 스레드가 옮긴다.
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::remote::connection::{
    ConnCommand, ConnPhase, ConnectionId, TransferDirection, TransferId, TransferRequest,
};
use crate::remote::manager::ConnectionManager;
use crate::remote::queue::{TransferQueue, TransferState};
use crate::remote::sites::SiteStore;
use crate::remote::types::SiteId;

/// 받는 중인 파일에 붙는 꼬리 (Acceptance ⑤)
const PART_SUFFIX: &str = ".part";

/// 이 단계의 연결에 전송을 맡겨도 되는가 (사용자 보고 2026-09-17).
///
/// **막는 것은 둘뿐이다** — 끊기거나 실패한 연결(`Failed`)과 접힌 연결(`Closed`). 그 둘은
/// 소켓이 없거나 죽어 있어, 맡기면 워커가 그 자리에서 실패를 돌려준다. 그러면 사용자가
/// 누르는 재시도마다 즉시 실패가 하나씩 쌓이고 자동 재시도 횟수까지 헛되이 소진된다.
///
/// **`Idle`·`Connecting`은 막지 않는다.** 그 둘은 곧 `Ready`가 될 연결이고, 명령은 채널에
/// 쌓여 순서대로 처리되므로 연결이 선 뒤에 나간다. 막으면 얻는 것 없이 첫 전송만 늦어진다
/// (`phase`는 `Connection::poll`이 이벤트를 소비해야 갱신되므로, 막 열린 연결은 화면이
/// 한 번 폴링하기 전까지 `Idle`로 보인다 — 거기에 게이트를 걸면 그 프레임을 통째로 버린다)
fn usable_for_transfer(phase: &ConnPhase) -> bool {
    !matches!(phase, ConnPhase::Failed { .. } | ConnPhase::Closed)
}

/// 이어받기 시작점을 정한다.
///
/// - 받다 만 것이 **원격보다 크거나 같으면** 처음부터 받는다 — 서버 쪽 파일이 바뀐 것이라
///   이어 붙이면 뒤섞인 파일이 된다 (plan Edge Case)
/// - 원격 크기를 모르면(0) 이어받지 않는다 — 어디까지가 맞는지 확인할 길이 없다
pub fn resume_offset(local_size: u64, remote_size: u64) -> u64 {
    if remote_size == 0 || local_size >= remote_size {
        return 0;
    }
    local_size
}

/// 로컬 폴더를 파일 단위로 펼친다 (FR-38 — 폴더 드래그, T17 규약).
///
/// 돌려주는 것은 `(파일 경로, 뿌리에서의 상대 경로)`다 — 상대 경로가 있어야 서버 쪽에
/// 같은 모양으로 놓을 수 있다. 파일 하나를 넘기면 그 하나만 돌려준다.
///
/// **깊이 상한 40** — 순환 심볼릭 링크(정션 포함)에서 영원히 도는 것을 막는다(plan Edge Case).
/// 읽지 못하는 가지는 건너뛴다 — 권한 없는 폴더 하나 때문에 나머지를 버릴 이유가 없다
pub fn expand_for_transfer(root: &Path) -> Expanded {
    const MAX_DEPTH: usize = 40;
    let name = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if root.is_file() {
        return Expanded {
            files: vec![(root.to_path_buf(), name)],
            skipped: 0,
        };
    }
    let mut found = Vec::new();
    let mut skipped = 0usize;
    let mut pending = vec![(root.to_path_buf(), name, 0usize)];
    while let Some((dir, prefix, depth)) = pending.pop() {
        if depth >= MAX_DEPTH {
            skipped += 1;
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            // 권한 없는 폴더는 흔하다 — 그 가지만 건너뛰되 **몇 개를 건너뛰었는지는 남긴다**
            skipped += 1;
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let child = format!("{prefix}/{}", entry.file_name().to_string_lossy());
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => pending.push((path, child, depth + 1)),
                // 심볼릭 링크는 따라가지 않는다 — 가리키는 곳이 트리 밖일 수 있다
                Ok(kind) if kind.is_file() => found.push((path, child)),
                _ => {}
            }
        }
    }
    Expanded {
        files: found,
        skipped,
    }
}

/// 펼친 결과 — 올릴 파일들과 읽지 못해 건너뛴 폴더 수
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Expanded {
    /// `(파일 경로, 뿌리에서의 상대 경로)`
    pub files: Vec<(PathBuf, String)>,
    /// 권한이 없거나 너무 깊어 건너뛴 폴더 수 — 호출부가 사용자에게 알린다
    pub skipped: usize,
}

/// 받는 중인 파일의 임시 이름 — `report.zip` → `report.zip.part`
pub fn part_path(local: &Path) -> PathBuf {
    let mut name = local.as_os_str().to_os_string();
    name.push(OsString::from(PART_SUFFIX));
    PathBuf::from(name)
}

/// 진행 보고를 속도로 바꾸는 계산기.
///
/// 워커는 누적 바이트만 올린다(100ms마다 — `connection.rs`의 `PROGRESS_INTERVAL`). 속도는
/// **직전 보고와의 차이 ÷ 걸린 시간**이며, 시간을 인자로 받아 시계를 하나로 둔다
/// (`Instant`를 쓰면 테스트가 실제로 기다려야 한다 — 토스트와 같은 판단).
#[derive(Debug, Clone, Copy, PartialEq)]
struct ProgressSink {
    last_bytes: u64,
    last_at: f64,
    /// 마지막으로 잰 속도 — 보고 간격이 너무 짧아 재지 못하면 이 값을 유지한다
    speed: u64,
}

impl ProgressSink {
    fn new(now: f64) -> ProgressSink {
        ProgressSink {
            last_bytes: 0,
            last_at: now,
            speed: 0,
        }
    }

    /// 새 누적 바이트를 받아 속도(바이트/초)를 갱신한다
    fn observe(&mut self, transferred: u64, now: f64) -> u64 {
        let elapsed = now - self.last_at;
        // 같은 프레임에 두 번 오면 나눌 수 없다 — 직전 속도를 유지한다
        if elapsed > 0.0 {
            let delta = transferred.saturating_sub(self.last_bytes);
            self.speed = (delta as f64 / elapsed) as u64;
            self.last_at = now;
            self.last_bytes = transferred;
        }
        self.speed
    }
}

/// 워커에 맡긴 전송 한 건의 기록 — 끝났을 때 무엇을 정리할지 여기에 있다
#[derive(Debug, Clone)]
struct Assignment {
    conn: ConnectionId,
    /// 완성됐을 때 놓일 자리
    final_path: PathBuf,
    /// 지금 쓰고 있는 자리 — 다운로드는 `.part`, 업로드는 원본 그대로다
    working_path: PathBuf,
    direction: TransferDirection,
    progress: ProgressSink,
}

/// 큐를 실제 전송으로 옮기는 실행기 (FR-37).
///
/// 상태를 큐와 나눠 갖지 않는다 — **항목의** 진행·결과는 전부 큐에 쓰고, 여기에는 **워커에
/// 맡긴 것과 그 뒤처리 정보**만 둔다. 두 곳에 같은 상태를 두면 어긋난 순간이 화면에 보인다.
///
/// **예외가 하나 있다 — 자주 바뀌지 않는 전역 설정의 사본**(`retry_limit`)이다. 그것은 어느
/// 항목의 상태도 아니라 위 규칙이 겨냥하는 어긋남을 만들지 않는다(설정이 정본이고 앱이 민다).
#[derive(Debug, Default)]
pub struct TransferRunner {
    assigned: HashMap<TransferId, Assignment>,
    /// 취소를 알렸고 **워커가 파일을 놓기를 기다리는** 것들 — 어느 연결에 맡겼는지와
    /// 받다 만 파일의 자리를 함께 든다.
    ///
    /// 그 자리에서 곧바로 지우지 않는 이유: 워커는 아직 그 파일을 열어 쓰고 있고(취소 신호는
    /// 64KB마다 살펴진다), Windows는 열려 있는 파일을 지우지 못한다. 지운 셈 치고 넘어가면
    /// 받다 만 파일이 그대로 남는다.
    ///
    /// **연결까지 드는 이유**: 취소 직후 그 연결이 닫히면 워커의 `TransferDone`이 채널째로
    /// 버려져 `on_done`이 영영 불리지 않는다 — 그때는 `forget_connection`이 이 자리를 넘겨받아
    /// 지운다(연결이 사라졌다는 것은 파일도 이미 놓였다는 뜻이다)
    cancelling: HashMap<TransferId, (ConnectionId, PathBuf)>,
    /// 지울 차례가 됐지만 아직 못 지운 `.part`들 — 백신 검사·핸들 지연으로 한 번에 실패할 수 있어
    /// 매 `start_ready`마다 다시 시도한다. 조용히 삼키면 받다 만 파일이 영영 남는다
    pending_delete: Vec<PathBuf>,
    /// 전송이 실패했을 때 자동으로 다시 걸어 볼 횟수 (FR-37) — 설정이 정본이며 앱이 민다.
    ///
    /// **인자가 아니라 필드로 드는 이유**: 이 값이 필요한 자리는 `on_done` 하나인데 그것을
    /// 인자로 늘리면 시험 다섯 곳이 함께 바뀐다. 값은 좀처럼 변하지 않고(설정 화면에서만),
    /// 앱이 시작할 때와 설정이 바뀔 때 밀어 넣으면 된다.
    ///
    /// **기본이 0인 것은 「설정을 아직 안 밀었다」는 뜻**이다 — 그 상태에서는 한 번도
    /// 재시도하지 않는다(설정이 정본이므로 기본값 3을 여기 또 적지 않는다)
    retry_limit: u32,
}

impl TransferRunner {
    pub fn new() -> TransferRunner {
        TransferRunner::default()
    }

    /// 설정의 재시도 횟수를 실행기에 민다 (FR-37) — 앱이 시작할 때와 설정이 바뀔 때 부른다.
    ///
    /// **이미 도는 전송에는 적용되지 않는다** — 다음 실패부터다(그 전송이 이미 쓴 횟수를
    /// 새 상한으로 다시 재면 사용자가 값을 줄인 순간 진행 중이던 것이 갑자기 굳는다)
    pub fn set_retry_limit(&mut self, limit: u32) {
        self.retry_limit = limit;
    }

    /// 지금 워커가 붙들고 있는 전송 수 — 화면·테스트가 진행 여부를 볼 때 쓴다
    pub fn in_flight(&self) -> usize {
        self.assigned.len()
    }

    /// 대기 중인 항목을 자리가 나는 대로 워커에 맡긴다.
    ///
    /// **연결이 없는 사이트는 건너뛴다** — 큐에 남아 있다가 다시 연결되면 그때 나간다.
    /// 한 연결은 한 번에 한 건만 옮긴다(워커가 명령을 차례로 처리하므로, 두 건을 한 연결에
    /// 밀어 넣으면 뒤엣것이 앞엣것을 기다리며 "진행 중"으로 잘못 보인다).
    pub fn start_ready(
        &mut self,
        queue: &mut TransferQueue,
        manager: &ConnectionManager,
        sites: &SiteStore,
        now: f64,
    ) {
        // 지우지 못한 `.part`가 있을 때만 다시 시도한다 — 이 자리는 매 프레임 불리므로
        // 목록이 비었으면 파일시스템을 건드리지 않는다 (AGENTS: UI 스레드 블로킹 I/O)
        if !self.pending_delete.is_empty() {
            self.sweep_pending_delete();
        }
        if queue.is_paused() {
            return;
        }
        // 사이트별로 쓸 수 있는 연결을 모은다
        let mut idle: HashMap<SiteId, Vec<ConnectionId>> = HashMap::new();
        let busy: HashSet<ConnectionId> = self.assigned.values().map(|a| a.conn).collect();
        for id in manager.ids() {
            if busy.contains(id) {
                continue;
            }
            if let Some(connection) = manager.get(*id) {
                if !usable_for_transfer(connection.phase()) {
                    continue;
                }
                idle.entry(connection.site).or_default().push(*id);
            }
        }

        for (site, mut conns) in idle {
            let Some(record) = sites.get(site) else {
                continue;
            };
            // 자리 수와 실제로 노는 연결 수 중 **작은 쪽**이 이번에 시작할 수 있는 최대다
            let slots = manager.transfer_slots(record).max(1) as usize;
            let ready = queue.next_for(site, slots as u8, now);
            for id in ready {
                // **정리를 기다리는 번호는 그 연결이 살아 있는 동안만 붙잡는다.**
                //
                // 취소한 전송의 `.part`는 워커의 완료 통지가 온 뒤에야 지워지는데(`cancelling`),
                // 그 사이에 같은 번호가 다시 배정되면 그 통지가 **지금 받는 중인 파일**을 지운다.
                // 취소한 줄이 목록에 남아 `다시 시도`로 되살아나면서 열린 길이다.
                //
                // 다만 **연결이 사라졌으면 그 통지는 영영 오지 않으므로** 여기서 거둔다 —
                // 붙잡기만 하면 그 번호가 `대기 중`인 채 앱을 다시 켤 때까지 나가지 못한다
                // (`forget_connection`이 하는 판단과 같다)
                let waiting_on = self.cancelling.get(&id).map(|(conn, _)| *conn);
                if let Some(owner) = waiting_on {
                    if manager.get(owner).is_some() {
                        continue;
                    }
                    // **삭제를 예약하지 않고 항목만 버린다** — 그 `.part`의 새 주인은 바로
                    // 아래에서 나가는 이 전송이다. 예약하면 다음 프레임의 `sweep_pending_delete`가
                    // 지금 받는 중인 파일을 지우려 들어(성공하면 이어받을 조각이 사라지고,
                    // 실패하면 그 전송이 끝날 때까지 매 프레임 파일시스템을 두드린다)
                    // 위 가드가 막으려던 것이 좁은 형태로 되돌아온다.
                    //
                    // **그럼 그 조각은 누가 지우나** — 사용자가 다시 걸지 않으면 이 갈래에
                    // 오지도 않는다(`ready`는 대기 항목만 담는다). 그때는 `cancelling`에 남아
                    // 앱이 닫힐 때 `shutdown`이 거둔다. 다시 걸었으면 그 조각은 찌꺼기가
                    // 아니라 **이어받을 자산**이라 남는 것이 맞다(실패한 `.part`를 남기는 규칙과 같다)
                    self.cancelling.remove(&id);
                }
                let Some(conn) = conns.pop() else {
                    break;
                };
                let Some(item) = queue.get(id) else {
                    continue;
                };
                let working_path = match item.direction {
                    // 받는 중에는 `.part`에 쓴다. **이어받기 지점은 워커가 정한다** —
                    // 받다 만 파일의 크기를 재는 것은 파일시스템 호출이라 화면 쪽에서 하면
                    // 프레임이 멈춘다 (AGENTS: UI 스레드 블로킹 I/O 금지 — T19 quality 리뷰 M1)
                    TransferDirection::Download => part_path(&item.local),
                    // 올리기는 원본을 읽기만 한다 — 이어 올리기 지점은 서버가 가진 크기라
                    // 워커가 APPE로 처리한다 (T2 결정)
                    TransferDirection::Upload => item.local.clone(),
                };
                let request = TransferRequest {
                    id,
                    direction: item.direction,
                    remote: item.remote.clone(),
                    local: working_path.clone(),
                    offset: 0,
                    remote_size: item.size,
                };
                if !manager.send(conn, ConnCommand::Transfer(request)) {
                    // 워커가 죽었다 — 대기로 두면 다음 연결에서 다시 나간다
                    continue;
                }
                self.assigned.insert(
                    id,
                    Assignment {
                        conn,
                        final_path: item.local.clone(),
                        working_path,
                        direction: item.direction,
                        progress: ProgressSink::new(now),
                    },
                );
                // 이어받는 중이면 워커의 첫 진행 보고가 곧바로 실제 지점을 알려 준다
                queue.update(id, TransferState::Active { sent: 0, speed: 0 });
            }
        }
    }

    /// 워커가 올린 진행 보고를 큐에 반영한다 — 속도는 여기서 잰다
    pub fn on_progress(&mut self, queue: &mut TransferQueue, id: TransferId, sent: u64, now: f64) {
        let Some(assignment) = self.assigned.get_mut(&id) else {
            return;
        };
        let speed = assignment.progress.observe(sent, now);
        queue.update(id, TransferState::Active { sent, speed });
    }

    /// 전송이 끝났다 — 성공이면 `.part`를 제 이름으로 바꾸고, 실패면 그대로 남겨 다음에 이어받는다.
    ///
    /// 실패한 `.part`를 지우지 않는 이유가 이어받기의 전부다(Acceptance ③) — 지우면
    /// 재시도가 언제나 처음부터가 된다
    pub fn on_done(
        &mut self,
        queue: &mut TransferQueue,
        id: TransferId,
        result: Result<u64, String>,
        now: f64,
    ) {
        // 사용자가 그만둔 것이면 이제야 파일을 지울 수 있다 — 워커가 방금 놓았다.
        // **자동 재시도 대상이 아니다** — 그만두라고 한 것을 다시 걸면 안 된다
        if let Some((_, part)) = self.cancelling.remove(&id) {
            self.pending_delete.push(part);
            self.sweep_pending_delete();
            return;
        }
        // `⏸`로 되돌린 것도 여기서 걸린다(`pause`가 `assigned`를 비웠다) — 멈춘 것을
        // 실패로 세면 다시 눌렀을 때 이미 쓴 횟수만큼 재시도 여지가 줄어든다
        let Some(assignment) = self.assigned.remove(&id) else {
            return;
        };
        match result {
            Ok(_) => {
                if assignment.direction == TransferDirection::Download
                    && let Err(err) =
                        std::fs::rename(&assignment.working_path, &assignment.final_path)
                {
                    // 옮기지 못하면 완료가 아니다 — `.part`가 남아 있어 다시 걸 수 있다.
                    // **이 자리도 재시도 대상이다** — 백신 검사·핸들 지연으로 잠깐 못 옮기는
                    // 것이 흔하고, `.part`가 남아 있어 재시도는 이어받기로 값싸게 끝난다
                    // (같은 파일의 `pending_delete`가 `remove_file` 실패에 쓰는 판단과 같다)
                    self.fail_or_retry(
                        queue,
                        id,
                        crate::i18n::dynamic::transfer_finalize_failed(&err.to_string()),
                        now,
                    );
                    return;
                }
                queue.update(id, TransferState::Done);
            }
            Err(message) => self.fail_or_retry(queue, id, message, now),
        }
    }

    /// 실패를 **다시 걸거나 굳힌다** (FR-37).
    ///
    /// `on_done`이 실패로 마킹하려는 자리가 둘이라(`Err` 분기와 `Ok` 분기 안의 이름 바꾸기
    /// 실패) 그 판정을 한곳에 모은다 — 나뉘어 있으면 한쪽만 재시도하게 되고, 그 어긋남은
    /// 화면에서 「어떤 실패는 다시 걸리고 어떤 실패는 안 걸린다」로 보인다.
    ///
    /// **사용자 취소와 `⏸`는 여기 오지 않는다** — 부르는 쪽이 이미 걸러 냈다
    fn fail_or_retry(
        &mut self,
        queue: &mut TransferQueue,
        id: TransferId,
        message: String,
        now: f64,
    ) {
        if queue.retry_automatically(id, self.retry_limit, now) {
            return;
        }
        // 상한을 다 썼다 — 사유에 몇 번 시도했는지를 붙여 굳힌다.
        // 그 숫자가 없으면 사용자는 앱이 한 번만 해 보고 포기한 줄 안다
        let attempts = queue.get(id).map(|item| item.attempts).unwrap_or_default();
        let message = if attempts > 0 {
            crate::i18n::dynamic::transfer_failed_after(&message, attempts + 1)
        } else {
            message
        };
        queue.update(id, TransferState::Error { message });
    }

    /// 사용자가 그만뒀다 — 워커를 멈추고 **받다 만 파일을 지운다** (Acceptance ⑤).
    ///
    /// 큐에서 항목을 빼는 것은 `TransferQueue::cancel`이 한다. 여기서는 워커와 파일만 정리한다
    pub fn cancel(&mut self, manager: &ConnectionManager, id: TransferId) {
        let Some(assignment) = self.assigned.remove(&id) else {
            return;
        };
        manager.cancel_transfer(assignment.conn, id);
        if assignment.direction == TransferDirection::Download {
            // 워커가 놓을 때까지 기다렸다 지운다 (`cancelling` 주석 참조)
            self.cancelling
                .insert(id, (assignment.conn, assignment.working_path));
        }
    }

    /// 아직 정리하지 못한 `.part` 건수 — 기다리는 것과 지우기를 다시 시도할 것을 합친다
    pub fn pending_cleanup(&self) -> usize {
        self.cancelling.len() + self.pending_delete.len()
    }

    /// 지울 차례가 된 `.part`를 지운다. **못 지운 것은 목록에 남겨 다음에 다시 시도한다** —
    /// 이미 사라진 것은 성공으로 본다(다른 쪽에서 치웠거나 애초에 만들어지지 않았다)
    fn sweep_pending_delete(&mut self) {
        self.pending_delete
            .retain(|part| part.exists() && std::fs::remove_file(part).is_err());
    }

    /// `⏸` — 진행 중인 전송을 멈추고 **대기로 되돌린다** (Acceptance ④).
    ///
    /// 취소와 다르다: `.part`를 **남긴다.** 다시 누르면 그 크기에서 이어받으므로
    /// 사용자가 보기에 "멈췄다 이어간다"가 된다
    pub fn pause(&mut self, queue: &mut TransferQueue, manager: &ConnectionManager) {
        queue.set_paused(true);
        for (id, assignment) in self.assigned.drain() {
            manager.cancel_transfer(assignment.conn, id);
            queue.update(id, TransferState::Wait);
        }
    }

    /// `⏸`를 다시 눌렀다 — 다음 `start_ready`가 이어받는다
    pub fn resume(&mut self, queue: &mut TransferQueue) {
        queue.set_paused(false);
    }

    /// 앱이 닫힌다 — 진행 중이던 것을 **대기로 되돌리고** 취소분의 `.part`를 마지막으로 치운다
    /// (plan Edge Case: 전송 중 앱 종료 → `.part` 정리).
    ///
    /// **진행 중이던 `.part`는 지우지 않는다** — 큐는 저장돼 다음 실행에서 그대로 복원되고(T25),
    /// 사용자가 다시 걸면 그 조각에서 이어받는다. 여기서 지우면 받아 둔 것을 매번 버리게 된다.
    /// 대신 상태를 `Active`로 남기지 않는다: 저장된 큐가 "전송 중"이라고 주장하면 다음 실행의
    /// 화면이 실제로는 아무것도 돌지 않는데 진행 중으로 보인다.
    ///
    /// 호출부는 앱의 종료 처리(`ExplorerApp::on_exit`)다 — 실행기를 앱에 배선하는 T19가 잇는다
    pub fn shutdown(&mut self, queue: &mut TransferQueue) {
        for (id, _) in self.assigned.drain() {
            queue.update(id, TransferState::Wait);
        }
        // 취소해 두고 아직 못 치운 조각은 지금이 마지막 기회다 — 워커는 이미 멈췄다
        let leftovers: Vec<PathBuf> = self.cancelling.drain().map(|(_, (_, part))| part).collect();
        self.pending_delete.extend(leftovers);
        self.sweep_pending_delete();
    }

    /// 연결이 사라졌다 — 그 연결에 맡겼던 전송을 놓아준다.
    /// 큐 쪽 되돌리기는 `TransferQueue::requeue_site`가 한다.
    ///
    /// **취소 뒤 정리를 기다리던 것도 여기서 거둔다** — 연결이 닫히면 워커의 완료 통지가
    /// 채널째로 버려져 `on_done`이 불리지 않는다. 그대로 두면 받다 만 파일이 영영 남는다
    pub fn forget_connection(&mut self, conn: ConnectionId) {
        self.assigned
            .retain(|_, assignment| assignment.conn != conn);
        let orphaned: Vec<TransferId> = self
            .cancelling
            .iter()
            .filter(|(_, (owner, _))| *owner == conn)
            .map(|(id, _)| *id)
            .collect();
        for id in orphaned {
            if let Some((_, part)) = self.cancelling.remove(&id) {
                self.pending_delete.push(part);
            }
        }
        self.sweep_pending_delete();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::*;
    use crate::remote::connection::{ConnEvent, ConnPhase};
    use crate::remote::testing::{FakeServer, FakeSession, pattern_byte};

    /// 시험용 배정 — 워커에 실제로 내보내지 않고 **맡긴 것으로만 표시**한다.
    ///
    /// `on_done`이 `assigned`에 있는 것만 처리하므로, 실패 판정만 보려는 시험이 실제 전송을
    /// 돌리지 않아도 되게 한다(그러지 않으면 재시도 시험마다 가짜 서버로 파일을 옮겨야 한다).
    ///
    /// 실코드 `impl`이 아니라 **여기 자유 함수로 두는 것**이 이 파일의 관례다 —
    /// 같은 모듈의 시험은 `assigned`에 직접 닿을 수 있어(아래 두 시험이 그렇게 한다)
    /// 본체에 시험 전용 메서드를 남길 이유가 없다
    fn assign_for_test(
        runner: &mut TransferRunner,
        id: TransferId,
        manager: &ConnectionManager,
        site: SiteId,
    ) {
        let conn = manager
            .ids()
            .iter()
            .find(|id| manager.get(**id).is_some_and(|c| c.site == site))
            .copied()
            .expect("그 사이트의 연결이 있어야 한다");
        runner.assigned.insert(
            id,
            Assignment {
                conn,
                final_path: PathBuf::from("test"),
                working_path: PathBuf::from("test"),
                direction: TransferDirection::Upload,
                progress: ProgressSink::new(0.0),
            },
        );
    }

    /// 위와 같되 **받기**로 맡긴 것으로 표시한다 — 이름 바꾸기 실패 갈래를 보는 시험이 쓴다
    fn assign_download_for_test(
        runner: &mut TransferRunner,
        id: TransferId,
        manager: &ConnectionManager,
        site: SiteId,
        working: &std::path::Path,
        final_path: &std::path::Path,
    ) {
        assign_for_test(runner, id, manager, site);
        if let Some(assignment) = runner.assigned.get_mut(&id) {
            assignment.direction = TransferDirection::Download;
            assignment.working_path = working.to_path_buf();
            assignment.final_path = final_path.to_path_buf();
        }
    }
    use crate::remote::types::{RemoteError, RemotePath, RemoteSession, SiteRecord};

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join("moa_transfer_tests");
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// 같은 이름이 겹치지 않게 번호를 붙인 임시 파일 경로
    fn temp_file(tag: &str) -> PathBuf {
        let unique = format!(
            "{tag}_{}_{:?}.bin",
            std::process::id(),
            std::thread::current().id()
        );
        temp_dir().join(unique)
    }

    /// 재시도 지연은 1 → 2 → 4로 늘고 **60초에서 죈다** (FR-37).
    ///
    /// 늘리는 이유는 같은 오류가 곧바로 반복될 때 서버를 쉬지 않고 두드리지 않기 위해서고,
    /// 죄는 이유는 상한이 10이면 마지막이 8분 반이 되어 사용자가 멈춘 것으로 보기 때문이다
    #[test]
    fn 재시도_지연은_배로_늘다_예순_초에서_죈다() {
        use crate::remote::queue::retry_delay_secs;
        assert_eq!(retry_delay_secs(1), 1.0);
        assert_eq!(retry_delay_secs(2), 2.0);
        assert_eq!(retry_delay_secs(3), 4.0);
        assert_eq!(retry_delay_secs(4), 8.0);
        assert_eq!(retry_delay_secs(7), 60.0, "64초가 아니라 60초에서 죈다");
        assert_eq!(retry_delay_secs(10), 60.0);
        // 터무니없이 큰 값이 와도 넘치지 않는다
        assert_eq!(retry_delay_secs(u32::MAX), 60.0);
    }

    /// 상한을 다 쓸 때까지 다시 걸고, 넘으면 굳힌다 (FR-37).
    ///
    /// 서버 없이 큐와 실행기만으로 돈다 — `on_done`에 실패를 직접 먹여 자동 재시도 판정만 본다
    #[test]
    fn 실패는_설정된_횟수만큼_다시_걸린다() {
        let _guard =
            crate::i18n::LanguageGuard::lock(crate::app::settings::LanguageSetting::Korean);
        let server = FakeServer::new();
        let (manager, _sites, site) = manager_with_site(&server);
        let mut queue = TransferQueue::new();
        let mut runner = TransferRunner::new();
        runner.set_retry_limit(3);
        let id = queue.enqueue(
            site,
            TransferDirection::Upload,
            PathBuf::from(r"C:.txt"),
            RemotePath::new("/a.txt"),
            100,
        );

        // 세 번은 다시 걸린다 — 그때마다 대기로 돌아가고 회차가 오른다
        let mut now = 0.0;
        for 회차 in 1..=3u32 {
            queue.update(id, TransferState::Active { sent: 0, speed: 0 });
            assign_for_test(&mut runner, id, &manager, site);
            runner.on_done(&mut queue, id, Err("550 실패".to_owned()), now);
            let item = queue.get(id).expect("항목");
            assert_eq!(item.state, TransferState::Wait, "{회차}번째에 굳었다");
            assert_eq!(item.attempts, 회차);
            // 지연이 지나야 다시 나간다
            assert!(
                queue.next_for(site, 2, now).is_empty(),
                "{회차}번째 재시도가 지연 없이 바로 나갔다"
            );
            now += crate::remote::queue::retry_delay_secs(회차) + 0.1;
            assert_eq!(
                queue.next_for(site, 2, now),
                vec![id],
                "{회차}번째 지연이 지났는데도 나가지 않는다"
            );
        }

        // 네 번째 실패는 굳는다 — 사유에 시도 횟수가 붙는다
        queue.update(id, TransferState::Active { sent: 0, speed: 0 });
        assign_for_test(&mut runner, id, &manager, site);
        runner.on_done(&mut queue, id, Err("550 실패".to_owned()), now);
        let item = queue.get(id).expect("항목");
        assert!(item.state.is_error(), "상한을 넘겼는데 또 걸렸다");
        match &item.state {
            TransferState::Error { message } => {
                assert_eq!(message, "550 실패 (4회 시도)", "시도 횟수가 사유에 없다");
            }
            other => panic!("실패가 아니다: {other:?}"),
        }

        // 사용자가 손으로 다시 걸면 횟수가 0으로 돌아간다 (2026-08-28 결정)
        queue.retry(&[id]);
        let item = queue.get(id).expect("항목");
        assert_eq!(item.attempts, 0);
        assert_eq!(item.retry_at, None, "손으로 건 것에는 지연을 두지 않는다");
    }

    /// **`Ok` 분기 안의 이름 바꾸기 실패도 재시도 대상이다** (FR-37).
    ///
    /// 그 자리는 `Err` 분기를 거치지 않아, 재시도 판정을 한쪽에만 두면 조용히 샌다.
    /// 백신 검사·핸들 지연으로 잠깐 못 옮기는 것이 흔하고 `.part`가 남아 있어 재시도는
    /// 이어받기로 값싸게 끝난다
    #[test]
    fn 받은_파일을_옮기지_못한_것도_다시_걸린다() {
        let server = FakeServer::new();
        let (manager, _sites, site) = manager_with_site(&server);
        let mut queue = TransferQueue::new();
        let mut runner = TransferRunner::new();
        runner.set_retry_limit(2);

        // 옮길 곳을 **폴더로 막아 둔다** — 그 이름으로는 파일을 놓을 수 없어 rename이 실패한다
        let dir = std::env::temp_dir().join(format!("moa-finalize-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let 막힌_자리 = dir.join("막힘");
        let _ = std::fs::create_dir_all(&막힌_자리);
        let part = dir.join("받는중.part");
        std::fs::write(&part, b"x").expect("조각 파일");

        let id = queue.enqueue(
            site,
            TransferDirection::Download,
            막힌_자리.clone(),
            RemotePath::new("/a.txt"),
            1,
        );
        queue.update(id, TransferState::Active { sent: 0, speed: 0 });
        assign_download_for_test(&mut runner, id, &manager, site, &part, &막힌_자리);

        // 워커는 성공을 알렸는데 옮기지 못했다 — 그래도 다시 걸려야 한다
        runner.on_done(&mut queue, id, Ok(1), 0.0);
        let item = queue.get(id).expect("항목");
        assert_eq!(
            item.state,
            TransferState::Wait,
            "옮기기 실패가 곧바로 굳었다"
        );
        assert_eq!(item.attempts, 1, "재시도 횟수를 세지 않았다");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `⏸`로 되돌린 것은 실패가 아니다 — 재시도 횟수를 먹지 않는다 (FR-37 Edge Case)
    #[test]
    fn 일시정지는_재시도_횟수를_먹지_않는다() {
        let server = FakeServer::new();
        let (manager, _sites, site) = manager_with_site(&server);
        let mut queue = TransferQueue::new();
        let mut runner = TransferRunner::new();
        runner.set_retry_limit(3);
        let id = queue.enqueue(
            site,
            TransferDirection::Download,
            PathBuf::from(r"C:.txt"),
            RemotePath::new("/a.txt"),
            100,
        );
        queue.update(id, TransferState::Active { sent: 0, speed: 0 });
        assign_for_test(&mut runner, id, &manager, site);

        // `⏸`가 `assigned`를 비우고 대기로 되돌린다
        runner.pause(&mut queue, &manager);
        assert_eq!(queue.get(id).expect("항목").attempts, 0);

        // 뒤늦게 온 워커의 실패 통지는 `assigned`에 없어 그냥 버려진다
        runner.on_done(&mut queue, id, Err("취소됨".to_owned()), 0.0);
        let item = queue.get(id).expect("항목");
        assert_eq!(item.attempts, 0, "멈춘 것이 실패로 세어졌다");
        assert_eq!(item.state, TransferState::Wait);
    }

    fn manager_with_site(server: &Arc<FakeServer>) -> (ConnectionManager, SiteStore, SiteId) {
        let mut sites = SiteStore::new();
        let site = sites.add("가짜 서버");
        if let Some(record) = sites.get_mut(site) {
            record.host = "example.test".to_owned();
        }
        let record: SiteRecord = sites.get(site).expect("사이트").clone();
        let mut manager = ConnectionManager::new(Arc::new(|| {}));
        let session: Box<dyn RemoteSession> = Box::new(FakeSession::new(Arc::clone(server)));
        manager.open(&record, String::new(), session);
        (manager, sites, site)
    }

    /// 이벤트가 올 때까지 짧게 기다린다 — 워커가 다른 스레드라 즉시 오지 않는다
    fn drain_until<F>(
        manager: &mut ConnectionManager,
        limit: Duration,
        mut done: F,
    ) -> Vec<ConnEvent>
    where
        F: FnMut(&[ConnEvent]) -> bool,
    {
        let deadline = Instant::now() + limit;
        let mut events = Vec::new();
        while Instant::now() < deadline {
            events.extend(manager.poll().into_iter().map(|(_, event)| event));
            if done(&events) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        events
    }

    #[test]
    fn 폴더는_파일_단위로_펼쳐진다() {
        // Acceptance ④ — 큐는 파일 단위만 안다 (T17 규약)
        let root = temp_dir().join(format!("expand_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("안쪽")).expect("폴더");
        std::fs::write(root.join("겉.txt"), b"a").expect("파일");
        std::fs::write(root.join("안쪽").join("속.bin"), b"bb").expect("파일");

        let mut expanded = expand_for_transfer(&root);
        expanded.files.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(expanded.skipped, 0, "읽지 못한 폴더가 없어야 한다");
        let found = expanded.files;
        let names: Vec<&str> = found.iter().map(|(_, rel)| rel.as_str()).collect();
        let root_name = root
            .file_name()
            .expect("이름")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            names,
            vec![
                format!("{root_name}/겉.txt").as_str(),
                format!("{root_name}/안쪽/속.bin").as_str()
            ],
            "상대 경로가 뿌리 이름부터 시작해야 서버에 같은 모양으로 놓인다"
        );
        assert!(found.iter().all(|(path, _)| path.is_file()));

        // 파일 하나를 넘기면 그 하나만 돌려준다
        let single = expand_for_transfer(&root.join("겉.txt"));
        assert_eq!(single.files.len(), 1);
        assert_eq!(single.files[0].1, "겉.txt");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 이어받기_시작점은_받다_만_크기다() {
        // Acceptance ③의 계산 부분
        assert_eq!(resume_offset(0, 1000), 0);
        assert_eq!(resume_offset(400, 1000), 400);
        // 받다 만 것이 더 크면 서버 쪽이 바뀐 것이다 — 이어 붙이면 뒤섞인 파일이 된다
        assert_eq!(resume_offset(1200, 1000), 0);
        assert_eq!(resume_offset(1000, 1000), 0);
        // 원격 크기를 모르면 이어받지 않는다
        assert_eq!(resume_offset(500, 0), 0);
    }

    #[test]
    fn 받는_중에는_part_이름을_쓴다() {
        // Acceptance ⑤ — 받다 만 파일이 완성본처럼 보이면 사용자가 그것을 열어 본다
        let path = PathBuf::from(r"C:\down\report.zip");
        assert_eq!(part_path(&path), PathBuf::from(r"C:\down\report.zip.part"));
        // 확장자가 없어도 뒤에 붙는다
        assert_eq!(
            part_path(&PathBuf::from(r"C:\down\LICENSE")),
            PathBuf::from(r"C:\down\LICENSE.part")
        );
    }

    #[test]
    fn 속도는_직전_보고와의_차이로_잰다() {
        let mut sink = ProgressSink::new(0.0);
        // 1초에 1000바이트
        assert_eq!(sink.observe(1000, 1.0), 1000);
        // 0.5초에 500바이트 → 여전히 초당 1000
        assert_eq!(sink.observe(1500, 1.5), 1000);
        // 시간이 흐르지 않았으면 직전 값을 유지한다(0으로 나누지 않는다)
        assert_eq!(sink.observe(2000, 1.5), 1000);
    }

    #[test]
    fn 큐의_대기_항목이_연결로_나가_전송된다() {
        // Acceptance ① — 200KB를 받으면 진행 보고가 오고 마지막이 100%다
        let server = FakeServer::new();
        server.set_download_size(200 * 1024);
        let (mut manager, sites, site) = manager_with_site(&server);
        let mut queue = TransferQueue::new();
        let mut runner = TransferRunner::new();
        let local = temp_file("download");
        let _ = std::fs::remove_file(&local);
        let _ = std::fs::remove_file(part_path(&local));

        let id = queue.enqueue(
            site,
            TransferDirection::Download,
            local.clone(),
            RemotePath::new("/big.bin"),
            200 * 1024,
        );
        // 연결이 설 때까지 기다린 뒤 배정한다
        let _ = drain_until(&mut manager, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Phase(ConnPhase::Ready)))
        });
        runner.start_ready(&mut queue, &manager, &sites, 0.0);
        assert_eq!(runner.in_flight(), 1, "전송이 워커로 나가지 않았다");
        assert!(queue.get(id).expect("항목").state.is_active());

        // 진행·완료 이벤트를 큐에 반영한다
        let mut now = 0.0;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && runner.in_flight() > 0 {
            for (_, event) in manager.poll() {
                now += 0.1;
                match event {
                    ConnEvent::TransferProgress { id, transferred } => {
                        runner.on_progress(&mut queue, id, transferred, now);
                    }
                    ConnEvent::TransferDone { id, result } => {
                        runner.on_done(&mut queue, id, result.map_err(|e| e.to_string()), 0.0);
                    }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        assert!(
            queue.get(id).expect("항목").state.is_done(),
            "전송이 끝나지 않았다: {:?}",
            queue.get(id).map(|item| &item.state)
        );
        assert_eq!(queue.get(id).expect("항목").progress(), Some(1.0));
        // `.part`는 사라지고 제 이름의 파일이 남는다 (Acceptance ⑤)
        assert!(local.exists(), "받은 파일이 제자리에 없다");
        assert!(!part_path(&local).exists(), "`.part`가 남았다");
        assert_eq!(
            std::fs::metadata(&local).expect("파일").len(),
            200 * 1024,
            "받은 크기가 다르다"
        );
        let _ = std::fs::remove_file(&local);
    }

    #[test]
    fn 가짜_세션에서_200kb를_받으면_진행_보고가_네_번_이상_온다() {
        // Acceptance ① — 64KB마다 보고하므로 200KB면 네 번이다(D12).
        // 워커를 거치면 100ms 간격으로 묶여 나가므로(`PROGRESS_INTERVAL`) 그 아래층인
        // 세션에서 잰다 — 묶기는 화면을 위한 것이고, 이 단언은 스트리밍 자체를 본다
        struct Counting {
            calls: usize,
            last: u64,
        }
        impl crate::remote::types::Progress for Counting {
            fn report(&mut self, transferred: u64) -> bool {
                self.calls += 1;
                self.last = transferred;
                true
            }
        }

        let server = FakeServer::new();
        server.set_download_size(200 * 1024);
        let mut session = FakeSession::new(Arc::clone(&server));
        let record = SiteRecord::new(SiteId(0), "가짜".to_owned());
        session.connect(&record).expect("연결");
        let mut sink: Vec<u8> = Vec::new();
        let mut progress = Counting { calls: 0, last: 0 };

        let moved = session
            .download(&RemotePath::new("/big.bin"), &mut sink, 0, &mut progress)
            .expect("전송");
        assert_eq!(moved, 200 * 1024);
        assert!(
            progress.calls >= 4,
            "64KB마다 보고해야 한다 (실제 {}회)",
            progress.calls
        );
        assert_eq!(progress.last, 200 * 1024, "마지막 보고가 100%가 아니다");
    }

    #[test]
    fn 이어받은_파일이_원본과_바이트까지_같다() {
        // Acceptance ③ — plan이 "결과가 원본과 같다(해시 비교)"로 못 박은 것.
        //
        // 가짜 서버가 **자리마다 다른 바이트**를 주므로(`testing::pattern_byte`), 이어 붙이는
        // 지점이 한 바이트라도 겹치거나 빠지면 대조에서 드러난다. 균일한 값으로 채우면
        // 어긋난 이어받기도 "같아 보여" 이 단언이 아무것도 지키지 못한다
        const TOTAL: u64 = 200 * 1024;
        const ALREADY: u64 = 40 * 1024 + 7; // 블록 경계와 어긋난 지점에서 이어받는다

        let server = FakeServer::new();
        server.set_download_size(TOTAL);
        let (mut manager, sites, site) = manager_with_site(&server);
        let mut queue = TransferQueue::new();
        let mut runner = TransferRunner::new();
        let local = temp_file("resume_e2e");
        let part = part_path(&local);
        let _ = std::fs::remove_file(&local);

        // 앞부분만 받아 둔 상태를 만든다 — 실제로 끊겼을 때 디스크에 남는 것과 같은 모습이다
        let head: Vec<u8> = (0..ALREADY).map(pattern_byte).collect();
        std::fs::write(&part, &head).expect("부분 파일");

        let id = queue.enqueue(
            site,
            TransferDirection::Download,
            local.clone(),
            RemotePath::new("/big.bin"),
            TOTAL,
        );
        let _ = drain_until(&mut manager, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Phase(ConnPhase::Ready)))
        });
        runner.start_ready(&mut queue, &manager, &sites, 0.0);
        assert_eq!(runner.in_flight(), 1, "이어받기가 시작되지 않았다");

        // 이어받는 중의 진행 보고는 **파일 전체 기준**이어야 한다 — 이번에 옮긴 만큼만
        // 올리면 화면의 진행률이 뒤로 튄다 (T19 quality 리뷰 M1과 함께 고친 것)
        let mut first_report = None;
        let mut now = 0.0;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && runner.in_flight() > 0 {
            for (_, event) in manager.poll() {
                now += 0.1;
                match event {
                    ConnEvent::TransferProgress { id, transferred } => {
                        first_report.get_or_insert(transferred);
                        runner.on_progress(&mut queue, id, transferred, now)
                    }
                    ConnEvent::TransferDone { id, result } => {
                        runner.on_done(&mut queue, id, result.map_err(|e| e.to_string()), 0.0)
                    }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        if let Some(first) = first_report {
            assert!(
                first >= ALREADY,
                "진행 보고가 받아 둔 만큼을 빼먹었다: {first} < {ALREADY}"
            );
        }
        assert!(
            queue.get(id).expect("항목").state.is_done(),
            "이어받기가 끝나지 않았다: {:?}",
            queue.get(id).map(|item| &item.state)
        );
        let got = std::fs::read(&local).expect("받은 파일");
        let expected: Vec<u8> = (0..TOTAL).map(pattern_byte).collect();
        assert_eq!(got.len(), expected.len(), "받은 크기가 원본과 다르다");
        assert!(
            got == expected,
            "이어 붙인 자리가 어긋났다 — 처음 다른 바이트: {:?}",
            got.iter()
                .zip(expected.iter())
                .position(|(a, b)| a != b)
                .map(|at| (at, got[at], expected[at]))
        );
        assert!(!part.exists(), "`.part`가 남았다");
        let _ = std::fs::remove_file(&local);
    }

    #[test]
    fn 일시정지_뒤_다시_누르면_이어받아_끝난다() {
        // Acceptance ④ 뒷문장 (spec 리뷰 M1) — 멈추는 것만이 아니라 **이어서 끝나는 것**까지 본다.
        // 멈춘 지점은 기계 속도에 따라 다르므로, 받아 둔 조각을 만들어 둔 상태에서
        // 멈춤 → 다시 누름 → 완료의 흐름을 확인한다
        const TOTAL: u64 = 128 * 1024;
        const ALREADY: u64 = 30 * 1024;

        let server = FakeServer::new();
        server.set_download_size(TOTAL);
        let (mut manager, sites, site) = manager_with_site(&server);
        let mut queue = TransferQueue::new();
        let mut runner = TransferRunner::new();
        let local = temp_file("pause_resume_e2e");
        let part = part_path(&local);
        let _ = std::fs::remove_file(&local);
        std::fs::write(&part, (0..ALREADY).map(pattern_byte).collect::<Vec<u8>>())
            .expect("부분 파일");

        let id = queue.enqueue(
            site,
            TransferDirection::Download,
            local.clone(),
            RemotePath::new("/big.bin"),
            TOTAL,
        );
        let _ = drain_until(&mut manager, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Phase(ConnPhase::Ready)))
        });

        // 멈춰 있는 동안에는 시작되지 않는다
        runner.pause(&mut queue, &manager);
        runner.start_ready(&mut queue, &manager, &sites, 0.0);
        assert_eq!(runner.in_flight(), 0);
        assert_eq!(queue.get(id).expect("항목").state, TransferState::Wait);

        // 다시 누르면 받아 둔 자리에서 이어간다
        runner.resume(&mut queue);
        runner.start_ready(&mut queue, &manager, &sites, 1.0);
        assert_eq!(runner.in_flight(), 1, "다시 눌렀는데 시작되지 않았다");

        let mut now = 1.0;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && runner.in_flight() > 0 {
            for (_, event) in manager.poll() {
                now += 0.1;
                match event {
                    ConnEvent::TransferProgress { id, transferred } => {
                        runner.on_progress(&mut queue, id, transferred, now)
                    }
                    ConnEvent::TransferDone { id, result } => {
                        runner.on_done(&mut queue, id, result.map_err(|e| e.to_string()), 0.0)
                    }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        assert!(
            queue.get(id).expect("항목").state.is_done(),
            "끝나지 않았다"
        );
        let got = std::fs::read(&local).expect("받은 파일");
        assert_eq!(got, (0..TOTAL).map(pattern_byte).collect::<Vec<u8>>());
        let _ = std::fs::remove_file(&local);
    }

    /// 정리를 기다리는 번호는 **다시 배정되지 않는다**.
    ///
    /// 취소한 받기의 `.part`는 워커의 완료 통지가 온 뒤에야 지워진다(`cancelling`). 그 사이에
    /// 사용자가 `다시 시도`를 눌러 같은 번호가 다시 나가면, 뒤늦게 도착한 1차 시도의 통지가
    /// **지금 받는 중인 파일**을 지운다. 취소한 줄이 목록에 남게 되면서 열린 길이다
    #[test]
    fn 정리를_기다리는_번호는_다시_나가지_않는다() {
        let server = FakeServer::new();
        let (mut manager, sites, site) = manager_with_site(&server);
        let mut queue = TransferQueue::new();
        let mut runner = TransferRunner::new();
        let local = temp_file("cleanup_guard");
        let id = queue.enqueue(
            site,
            TransferDirection::Download,
            local.clone(),
            RemotePath::new("/big.bin"),
            1024,
        );
        let _ = drain_until(&mut manager, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Phase(ConnPhase::Ready)))
        });

        // 취소해 정리 대기로 만든 뒤, 사용자가 그 자리에서 다시 건다.
        // **살아 있는 연결에 맡긴다** — 그래야 「통지를 기다린다」가 성립한다
        let conn = manager.ids()[0];
        runner.cancelling.insert(id, (conn, part_path(&local)));
        queue.cancel(id);
        queue.retry(&[id]);
        assert_eq!(
            queue.get(id).expect("항목").state,
            TransferState::Wait,
            "다시 시도가 대기로 되돌린다"
        );

        runner.start_ready(&mut queue, &manager, &sites, 0.0);
        assert_eq!(
            runner.in_flight(),
            0,
            "정리를 기다리는 번호가 다시 나갔다 — 뒤늦은 취소 통지가 그 파일을 지운다"
        );

        // 정리가 끝나면 그때 나간다
        runner.cancelling.remove(&id);
        runner.start_ready(&mut queue, &manager, &sites, 0.0);
        assert_eq!(runner.in_flight(), 1, "정리가 끝난 뒤에는 나가야 한다");
    }

    /// **연결이 사라진 정리 대기는 붙잡지 않는다** — 그 통지는 영영 오지 않는다.
    ///
    /// 붙잡기만 하면 그 번호가 `대기 중`인 채 앱을 다시 켤 때까지 나가지 못한다(사용자에게는
    /// `다시 시도`가 소리 없이 삼켜지는 것으로 보인다). 위 가드가 새로 연 구멍이라 함께 막는다
    #[test]
    fn 연결이_사라진_정리_대기는_그_자리에서_거둔다() {
        let server = FakeServer::new();
        let (mut manager, sites, site) = manager_with_site(&server);
        let mut queue = TransferQueue::new();
        let mut runner = TransferRunner::new();
        let local = temp_file("orphan_cleanup");
        let id = queue.enqueue(
            site,
            TransferDirection::Download,
            local.clone(),
            RemotePath::new("/big.bin"),
            1024,
        );
        let _ = drain_until(&mut manager, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Phase(ConnPhase::Ready)))
        });

        // **없는 연결**에 맡긴 채 정리를 기다린다 — 탭을 닫아 연결이 사라진 상황이다
        let part = part_path(&local);
        std::fs::write(&part, b"x").expect("받다 만 파일");
        runner
            .cancelling
            .insert(id, (ConnectionId(9999), part.clone()));
        queue.cancel(id);
        queue.retry(&[id]);

        runner.start_ready(&mut queue, &manager, &sites, 0.0);
        assert_eq!(
            runner.in_flight(),
            1,
            "올 리 없는 통지를 기다리느라 번호가 갇혔다"
        );
        assert!(
            !runner.cancelling.contains_key(&id),
            "고아가 된 정리 대기를 거두지 않았다"
        );
        assert_eq!(
            runner.pending_cleanup(),
            0,
            "삭제를 예약하면 다음 프레임이 지금 받는 중인 파일을 지우려 든다"
        );
        assert!(
            part.exists(),
            "받다 만 조각은 남아야 한다 — 지금 나가는 전송이 그것을 이어받는다"
        );
        let _ = std::fs::remove_file(&part);
    }

    /// 취소가 **전송이 시작되기 전에** 닿아도 유실되지 않는다.
    ///
    /// 취소는 명령 채널 밖의 신호라 `Transfer` 명령보다 먼저 도착할 수 있다. 그 신호를
    /// 참·거짓으로만 두면 막 시작한 전송이 들어오면서 그것을 지워, 사용자는 취소했는데
    /// 파일은 끝까지 올라간다
    #[test]
    fn 시작하기_전에_누른_취소도_유실되지_않는다() {
        let server = FakeServer::new();
        let (mut manager, sites, site) = manager_with_site(&server);
        let mut queue = TransferQueue::new();
        let mut runner = TransferRunner::new();
        let local = temp_file("cancel_before_start");
        std::fs::write(&local, vec![3u8; 128 * 1024]).expect("올릴 파일");
        let id = queue.enqueue(
            site,
            TransferDirection::Upload,
            local.clone(),
            RemotePath::new("/small.bin"),
            128 * 1024,
        );
        let _ = drain_until(&mut manager, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Phase(ConnPhase::Ready)))
        });

        // **기계 속도에 기대지 않는다** — 워커를 다른 명령에 붙들어 두면 그동안 보낸 전송은
        // 반드시 아직 시작되지 않은 상태다
        server.set_hang(true);
        let conn = manager.ids()[0];
        assert!(manager.send(conn, ConnCommand::Cwd(RemotePath::root())));
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && !server.calls().iter().any(|call| call == "cwd") {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            server.calls().iter().any(|call| call == "cwd"),
            "워커를 붙들지 못했다"
        );

        runner.start_ready(&mut queue, &manager, &sites, 0.0);
        assert_eq!(runner.in_flight(), 1);
        // 아직 한 바이트도 옮기지 않은 전송을 취소한다
        runner.cancel(&manager, id);
        assert!(queue.cancel(id), "큐에서도 `취소됨`으로 바뀐다");
        server.set_hang(false);

        let events = drain_until(&mut manager, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::TransferDone { .. }))
        });
        let _ = std::fs::remove_file(&local);
        assert!(
            events.iter().any(|event| matches!(
                event,
                ConnEvent::TransferDone {
                    result: Err(RemoteError::Cancelled),
                    ..
                }
            )),
            "시작하기 전에 누른 취소가 유실됐다: {events:?}"
        );
    }

    /// 취소는 **전송이 도중인 워커에도 그 자리에서 닿아야 한다**.
    ///
    /// 닿지 않으면 그 연결에 이어 넣은 명령이 전부 그 전송 뒤로 밀린다 — 다시 전송을 걸어도
    /// 같은 이름 확인 조회(FR-55)가 처리되지 않아 대화도 뜨지 않고 큐에도 들어가지 않는다.
    /// 사용자에게는 "취소하고 다시 전송하면 아무 반응이 없다"로 보인다
    #[test]
    fn 취소는_전송_중인_워커를_그_자리에서_멈춘다() {
        let server = FakeServer::new();
        // 64KB 덩이마다 60ms 쉰다 — 1MB를 끝까지 올리면 0.9초를 넘긴다.
        // **취소가 먹으면 한 덩이 안에 끝나므로** 아래 대기 창과 3배 넘게 벌어진다
        // (마진이 얇으면 전체 시험을 함께 돌릴 때 부하로 산발 실패한다 — 실측)
        server.set_transfer_delay(Duration::from_millis(60));
        let (mut manager, sites, site) = manager_with_site(&server);
        let mut queue = TransferQueue::new();
        let mut runner = TransferRunner::new();
        let local = temp_file("cancel_midflight");
        std::fs::write(&local, vec![7u8; 1024 * 1024]).expect("올릴 파일");
        let id = queue.enqueue(
            site,
            TransferDirection::Upload,
            local.clone(),
            RemotePath::new("/big.bin"),
            1024 * 1024,
        );
        let _ = drain_until(&mut manager, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Phase(ConnPhase::Ready)))
        });
        runner.start_ready(&mut queue, &manager, &sites, 0.0);
        assert_eq!(runner.in_flight(), 1);

        // 워커가 실제로 올리기 시작한 뒤에 취소해야 "도중에 끼어든 취소"가 된다
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline && !server.calls().iter().any(|call| call == "upload") {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            server.calls().iter().any(|call| call == "upload"),
            "올리기가 시작되지 않았다"
        );

        runner.cancel(&manager, id);
        assert!(queue.cancel(id), "큐에서도 `취소됨`으로 바뀐다");
        // 취소가 닿았으면 다음 덩이에서 멈춘다. 끝까지 올려 버렸다면 이 창을 넘긴다
        let events = drain_until(&mut manager, Duration::from_millis(300), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::TransferDone { .. }))
        });
        let _ = std::fs::remove_file(&local);
        assert!(
            events.iter().any(|event| matches!(
                event,
                ConnEvent::TransferDone {
                    result: Err(RemoteError::Cancelled),
                    ..
                }
            )),
            "취소가 전송 중인 워커에 닿지 않았다: {events:?}"
        );
    }

    #[test]
    fn 취소하면_받다_만_파일이_남지_않는다() {
        // Acceptance ⑤
        let server = FakeServer::new();
        server.set_download_size(64 * 1024 * 64);
        let (mut manager, sites, site) = manager_with_site(&server);
        let mut queue = TransferQueue::new();
        let mut runner = TransferRunner::new();
        let local = temp_file("cancel");
        let id = queue.enqueue(
            site,
            TransferDirection::Download,
            local.clone(),
            RemotePath::new("/big.bin"),
            64 * 1024 * 64,
        );
        let _ = drain_until(&mut manager, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Phase(ConnPhase::Ready)))
        });
        runner.start_ready(&mut queue, &manager, &sites, 0.0);
        assert_eq!(runner.in_flight(), 1);

        runner.cancel(&manager, id);
        assert!(queue.cancel(id), "큐에서도 `취소됨`으로 바뀐다");
        assert_eq!(runner.in_flight(), 0);
        assert_eq!(runner.pending_cleanup(), 1, "정리를 기다리는 중이어야 한다");

        // 워커가 파일을 놓았다고 알려 오면 그때 지운다 — 열려 있는 파일은 지울 수 없다
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && runner.pending_cleanup() > 0 {
            for (_, event) in manager.poll() {
                if let ConnEvent::TransferDone { id, result } = event {
                    runner.on_done(&mut queue, id, result.map_err(|e| e.to_string()), 0.0);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(runner.pending_cleanup(), 0, "정리가 끝나지 않았다");
        assert!(!part_path(&local).exists(), "받다 만 파일이 남았다");
        assert!(!local.exists(), "완성되지도 않은 파일이 생겼다");
    }

    #[test]
    fn 일시정지는_대기로_되돌리고_part를_남긴다() {
        // Acceptance ④ — 취소와 다르다. 다시 누르면 그 크기에서 이어받는다
        let server = FakeServer::new();
        server.set_download_size(64 * 1024 * 64);
        let (mut manager, sites, site) = manager_with_site(&server);
        let mut queue = TransferQueue::new();
        let mut runner = TransferRunner::new();
        let local = temp_file("pause");
        let id = queue.enqueue(
            site,
            TransferDirection::Download,
            local.clone(),
            RemotePath::new("/big.bin"),
            64 * 1024 * 64,
        );
        let _ = drain_until(&mut manager, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Phase(ConnPhase::Ready)))
        });
        runner.start_ready(&mut queue, &manager, &sites, 0.0);

        runner.pause(&mut queue, &manager);
        assert!(queue.is_paused());
        assert_eq!(queue.get(id).expect("항목").state, TransferState::Wait);
        assert_eq!(runner.in_flight(), 0);
        // 멈춘 동안에는 새로 시작하지 않는다
        runner.start_ready(&mut queue, &manager, &sites, 1.0);
        assert_eq!(runner.in_flight(), 0);

        runner.resume(&mut queue);
        assert!(!queue.is_paused());
        let _ = drain_until(&mut manager, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::TransferDone { .. }))
        });
        let _ = std::fs::remove_file(part_path(&local));
        let _ = std::fs::remove_file(&local);
    }

    #[test]
    fn 취소_직후_연결이_닫혀도_받다_만_파일이_치워진다() {
        // quality 리뷰 M1 — 연결이 닫히면 워커의 완료 통지가 채널째로 버려져 `on_done`이
        // 불리지 않는다. 그때 정리를 넘겨받지 않으면 `.part`가 영영 남는다
        let mut runner = TransferRunner::new();
        let local = temp_file("orphan");
        let part = part_path(&local);
        std::fs::write(&part, vec![0u8; 32]).expect("부분 파일");

        let id = TransferId(1);
        let conn = ConnectionId(3);
        runner.cancelling.insert(id, (conn, part.clone()));
        assert_eq!(runner.pending_cleanup(), 1);

        runner.forget_connection(conn);
        assert_eq!(runner.pending_cleanup(), 0, "정리가 넘겨지지 않았다");
        assert!(!part.exists(), "받다 만 파일이 남았다");
    }

    #[test]
    fn 지우지_못한_조각은_다음_기회에_다시_치운다() {
        // quality 리뷰 M3 — 백신 검사·핸들 지연으로 한 번에 실패할 수 있다.
        // 조용히 삼키면 받다 만 파일이 영영 남는다
        let mut runner = TransferRunner::new();
        let mut queue = TransferQueue::new();
        let sites = SiteStore::new();
        let manager = ConnectionManager::new(Arc::new(|| {}));
        let local = temp_file("retry_cleanup");
        let part = part_path(&local);
        std::fs::write(&part, vec![0u8; 16]).expect("부분 파일");

        // 파일을 붙들고 있는 동안에는 지우지 못한다(Windows) — 목록에 남아야 한다
        let handle = std::fs::File::open(&part).expect("열기");
        runner.pending_delete.push(part.clone());
        runner.start_ready(&mut queue, &manager, &sites, 0.0);
        // 열려 있어도 지워지는 환경이 있어(공유 삭제 허용) 둘 다 정상으로 본다 —
        // 중요한 것은 **남았으면 다시 시도한다**는 것이다
        if part.exists() {
            assert_eq!(runner.pending_cleanup(), 1, "다시 시도할 목록에서 사라졌다");
        }
        drop(handle);

        runner.start_ready(&mut queue, &manager, &sites, 1.0);
        assert!(!part.exists(), "놓아준 뒤에도 치우지 못했다");
        assert_eq!(runner.pending_cleanup(), 0);
    }

    #[test]
    fn 앱이_닫히면_진행_중이던_것이_대기로_돌아가고_취소분만_치워진다() {
        // plan Edge Case — 저장된 큐가 "전송 중"이라고 주장하면 다음 실행 화면이 거짓말을 한다.
        // 반대로 받아 둔 조각을 지우면 이어받기가 매번 처음부터가 된다
        let mut runner = TransferRunner::new();
        let mut queue = TransferQueue::new();
        let running_local = temp_file("shutdown_running");
        let running_part = part_path(&running_local);
        std::fs::write(&running_part, vec![0u8; 64]).expect("부분 파일");
        let cancelled_part = part_path(&temp_file("shutdown_cancelled"));
        std::fs::write(&cancelled_part, vec![0u8; 32]).expect("부분 파일");

        let id = queue.enqueue(
            SiteId(1),
            TransferDirection::Download,
            running_local.clone(),
            RemotePath::new("/x.bin"),
            1000,
        );
        queue.update(id, TransferState::Active { sent: 64, speed: 1 });
        runner.assigned.insert(
            id,
            Assignment {
                conn: ConnectionId(0),
                final_path: running_local.clone(),
                working_path: running_part.clone(),
                direction: TransferDirection::Download,
                progress: ProgressSink::new(0.0),
            },
        );
        runner
            .cancelling
            .insert(TransferId(99), (ConnectionId(0), cancelled_part.clone()));

        runner.shutdown(&mut queue);

        assert_eq!(queue.get(id).expect("항목").state, TransferState::Wait);
        assert!(running_part.exists(), "이어받을 조각까지 지웠다");
        assert!(!cancelled_part.exists(), "취소분이 남았다");
        assert_eq!(runner.in_flight(), 0);
        assert_eq!(runner.pending_cleanup(), 0);
        let _ = std::fs::remove_file(&running_part);
    }

    /// 끊긴 연결에는 전송을 맡기지 않는다 (사용자 보고 2026-09-17).
    ///
    /// 맡기면 워커가 그 자리에서 실패를 돌려주므로, 사용자가 누르는 `다시 시도`마다 즉시
    /// 실패가 하나씩 쌓이고 자동 재시도 횟수까지 헛되이 소진된다. 큐에 대기로 남겨 두면
    /// 다시 붙었을 때 저절로 나간다 — 연결이 **아예 없는** 사이트에 쓰는 규칙과 같다
    #[test]
    fn 끊긴_연결에는_전송을_맡기지_않는다() {
        let server = FakeServer::new();
        server.set_entries("/", vec![]);
        let (mut manager, sites, site) = manager_with_site(&server);
        let mut queue = TransferQueue::new();
        let mut runner = TransferRunner::new();
        let _ = drain_until(&mut manager, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Phase(ConnPhase::Ready)))
        });

        // 선 연결을 죽인다 — 목록 조회가 실패하면서 워커가 끊김 단계를 올린다
        server.set_link_down(true);
        let conn = *manager.ids().first().expect("연결");
        manager.send(
            conn,
            ConnCommand::List {
                source: crate::remote::connection::ListSource::Tree { seq: 1 },
                path: RemotePath::root(),
                quiet: true,
            },
        );
        let _ = drain_until(&mut manager, Duration::from_secs(3), |events| {
            events
                .iter()
                .any(|event| matches!(event, ConnEvent::Phase(ConnPhase::Failed { .. })))
        });
        assert!(
            matches!(
                manager.get(conn).expect("연결").phase(),
                ConnPhase::Failed { .. }
            ),
            "연결이 끊김으로 표시되지 않았다 — 이 시험이 재려는 상태가 아니다"
        );

        let id = queue.enqueue(
            site,
            TransferDirection::Download,
            PathBuf::from(r"C:\x.bin"),
            RemotePath::new("/x.bin"),
            10,
        );
        runner.start_ready(&mut queue, &manager, &sites, 0.0);
        assert_eq!(runner.in_flight(), 0, "끊긴 연결에 전송을 맡겼다");
        assert_eq!(
            queue.get(id).expect("항목").state,
            TransferState::Wait,
            "대기로 남아야 다시 붙었을 때 저절로 나간다"
        );
    }

    #[test]
    fn 연결이_없는_사이트는_그대로_대기한다() {
        // 큐에 남아 있다가 다시 연결되면 그때 나간다
        let mut queue = TransferQueue::new();
        let mut runner = TransferRunner::new();
        let sites = SiteStore::new();
        let manager = ConnectionManager::new(Arc::new(|| {}));
        let id = queue.enqueue(
            SiteId(9),
            TransferDirection::Download,
            PathBuf::from(r"C:\x.bin"),
            RemotePath::new("/x.bin"),
            10,
        );
        runner.start_ready(&mut queue, &manager, &sites, 0.0);
        assert_eq!(runner.in_flight(), 0);
        assert_eq!(queue.get(id).expect("항목").state, TransferState::Wait);
    }

    #[test]
    fn 실패한_전송은_part를_남겨_다음에_이어받는다() {
        // Acceptance ③ — 실패한 `.part`를 지우면 재시도가 언제나 처음부터가 된다
        let mut queue = TransferQueue::new();
        let mut runner = TransferRunner::new();
        let local = temp_file("resume");
        let part = part_path(&local);
        std::fs::write(&part, vec![0u8; 400]).expect("부분 파일");

        let id = queue.enqueue(
            SiteId(1),
            TransferDirection::Download,
            local.clone(),
            RemotePath::new("/x.bin"),
            1000,
        );
        // 워커 없이 결과만 흘려 넣는다 — 뒤처리 규칙만 보는 자리다
        runner.assigned.insert(
            id,
            Assignment {
                conn: ConnectionId(0),
                final_path: local.clone(),
                working_path: part.clone(),
                direction: TransferDirection::Download,
                progress: ProgressSink::new(0.0),
            },
        );
        runner.on_done(&mut queue, id, Err("연결이 끊겼습니다".to_owned()), 0.0);

        assert!(queue.get(id).expect("항목").state.is_error());
        assert!(part.exists(), "이어받을 조각이 사라졌다");
        // 다시 걸면 그 크기에서 이어받는다
        assert_eq!(
            resume_offset(std::fs::metadata(&part).expect("조각").len(), 1000),
            400
        );
        let _ = std::fs::remove_file(&part);
    }
}
