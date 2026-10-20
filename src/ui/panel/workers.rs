//! 패널이 워커 스레드에 맡기는 일 — 폴더 열거·하위 폴더 검색과 새 폴더·새 파일 만들기.
//!
//! 둘 다 **UI 스레드에서 부르면 안 되는 블로킹 I/O**라 같은 방식(워커 + 채널 + 다시 그리기 요청)으로
//! 감싼다. 본체(`ui::panel`)의 자식 모듈이며, 상태를 들고 결과를 거두는 일만 한다.

use crate::fs::enumerate::{EnumChunk, EnumOutcome, FileEntry, enumerate_dir_batched};
use crate::fs::icons::{ExtInfo, lookup_ext};
use crate::fs::search::{SearchEvent, SearchRules, search_tree};
use eframe::egui;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};

/// 한 번에 흘려보낼 항목 수 — 이만큼 모은 **뒤에도 더 있을 때만** 중간 결과가 나간다 (FR-69).
///
/// 이 수 이하인 폴더는 종전대로 한 번에 도착한다. 값을 키우면 첫 화면이 늦고, 줄이면
/// 배치마다 도는 정렬·필터가 잦아진다 — 10만 항목 기준 50회 남짓이 되는 자리로 잡았다
pub(super) const PARTIAL_BATCH: usize = 2000;

/// 백그라운드 폴더 열거 상태.
///
/// 동기 열거를 자체 워커로 감싸고 **채널로 조각을 받는다**(`EnumChunk`) — UI 스레드에서
/// 직접 열거하면 10만 파일 폴더에서 창이 멈춘다(AGENTS: UI 스레드 블로킹 금지).
/// 완료를 창 메시지로 알리지 않고 채널만 쓰는 것은 egui 경로의 규칙이다(D7)
pub(super) struct DirLoad {
    /// 늦게 도착한 이전 폴더의 결과를 버리기 위한 세대 번호
    generation: u64,
    pending: Option<Receiver<(u64, EnumChunk, Vec<ExtInfo>)>>,
    /// 워커가 조각과 함께 보낸 확장자 정보 — 반영 전에 UI 캐시로 옮긴다(`take_ext_infos`)
    ext_infos: Vec<ExtInfo>,
    /// 지금까지 **실제로 받은** 확장자 — 다음 워커가 이것을 다시 셸에 묻지 않는다.
    ///
    /// 워커 안에서 물은 것이 아니라 채널로 받은 것만 센다 — 받기 전에 버려진 몫(폴더를 연달아
    /// 옮김)까지 세면 그 확장자가 UI 캐시에 영영 들어가지 않아 화면 쪽 동기 조회로 떨어진다.
    /// 앱의 `IconCache`는 확장자 항목을 지우지 않으므로 받은 것은 계속 캐시에 있다
    known_exts: HashSet<String>,
}

impl DirLoad {
    pub(super) fn new() -> DirLoad {
        DirLoad {
            generation: 0,
            pending: None,
            ext_infos: Vec::new(),
            known_exts: HashSet::new(),
        }
    }

    /// 기다리던 결과를 버린다 — 활성 탭이 원격으로 바뀌어 그 결과가 갈 곳이 없어졌을 때.
    ///
    /// 세대를 함께 올려 **이미 채널에 실린 답도** 폐기한다
    pub(super) fn cancel(&mut self) {
        self.generation += 1;
        self.pending = None;
    }

    /// 워커 스레드에서 열거를 시작한다. 이전 요청의 결과는 세대 불일치로 폐기된다.
    ///
    /// 조각을 보내기 전에 **그 조각에서 처음 보는 확장자의 종류 문자열·아이콘을 미리 묻는다** —
    /// 확장자마다 첫 조회가 3~60ms라(2026-09-28 실측) UI 스레드가 목록을 반영하며 물으면 프레임이
    /// 멎는다. 대가는 확장자 수만큼 첫 조각이 늦는 것이고, 그동안 창은 멈추지 않는다
    pub(super) fn start(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.generation += 1;
        let generation = self.generation;
        let (tx, rx) = channel();
        self.pending = Some(rx);
        let ctx = ctx.clone();
        let known = self.known_exts.clone();
        std::thread::spawn(move || {
            // 셸 조회(`lookup_ext`)는 COM을 쓴다 — `fs::drives` 워커와 같은 이유로 초기화한다.
            // 안전성: 이 스레드에서 열고 열거가 끝난 뒤 짝을 맞춰 닫는다
            let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
            let mut seen: HashSet<String> = known;
            // 임시 계측 (`crate::perf`) — **경로는 싣지 않고 개수와 소요만** 남긴다.
            // 배치로 나뉘므로 **합계와 배치 횟수를 함께** 적는다(첫 화면이 언제 섰는지 읽히도록)
            let t_enum = std::time::Instant::now();
            let mut total = 0usize;
            let mut batches = 0usize;
            let mut d_first = None;
            enumerate_dir_batched(&path, PARTIAL_BATCH, |chunk| {
                match &chunk {
                    EnumChunk::Partial(entries) => {
                        total += entries.len();
                        batches += 1;
                        d_first.get_or_insert_with(|| t_enum.elapsed());
                    }
                    EnumChunk::Done(EnumOutcome::Ok(entries)) => total += entries.len(),
                    EnumChunk::Done(_) => {}
                }
                let infos = match &chunk {
                    EnumChunk::Partial(entries) | EnumChunk::Done(EnumOutcome::Ok(entries)) => {
                        new_ext_infos(entries, &mut seen)
                    }
                    EnumChunk::Done(_) => Vec::new(),
                };
                // 수신부가 이미 버려졌으면(앱 종료·폴더 재이동) 전송 실패다 — 그러면 읽기를 멈춘다
                let sent = tx.send((generation, chunk, infos)).is_ok();
                if sent {
                    ctx.request_repaint();
                }
                sent
            });
            let d_enum = t_enum.elapsed();
            crate::perf::log(|| {
                let first = d_first.map_or(d_enum, |d| d);
                format!(
                    "enum items={total} batches={batches} | first={:.1} enumerate={:.1} (ms)",
                    first.as_secs_f32() * 1000.0,
                    d_enum.as_secs_f32() * 1000.0
                )
            });
            if com {
                // 안전성: 위에서 성공한 초기화와 짝을 맞춘다
                unsafe { CoUninitialize() };
            }
        });
    }

    /// 도착한 조각을 하나 꺼낸다. 아직이면 `None`.
    ///
    /// **`Done`을 꺼낸 뒤에야 대기가 끝난다** — 중간 조각(`Partial`)에서는 수신부를 그대로
    /// 둬야 남은 배치가 이어서 온다. 부르는 쪽은 한 프레임에 채널이 빌 때까지 반복해 꺼낸다
    pub(super) fn poll(&mut self) -> Option<EnumChunk> {
        let rx = self.pending.as_ref()?;
        match rx.try_recv() {
            Ok((generation, chunk, infos)) => {
                // 확장자 정보는 세대와 무관하게 받는다 — 폴더가 아니라 확장자에 매인 값이라
                // 늦게 온 이전 폴더의 것도 틀리지 않는다
                self.known_exts
                    .extend(infos.iter().map(|info| info.ext.clone()));
                self.ext_infos.extend(infos);
                if matches!(chunk, EnumChunk::Done(_)) {
                    self.pending = None;
                }
                // 폴더를 연달아 이동하면 이전 결과가 나중에 도착할 수 있다
                (generation == self.generation).then_some(chunk)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                None
            }
        }
    }

    pub(super) fn is_loading(&self) -> bool {
        self.pending.is_some()
    }

    /// 워커가 미리 물어 둔 확장자 정보를 꺼낸다 — 조각을 반영하기 전에 UI 캐시에 넣는다
    pub(super) fn take_ext_infos(&mut self) -> Vec<ExtInfo> {
        std::mem::take(&mut self.ext_infos)
    }
}

/// 검색 결과를 이만큼에서 끊는다 (FR-70) — 드라이브 뿌리에서 한 글자로 찾으면 수백만 항목이
/// 메모리에 쌓인다. 점진 표시(FR-69)가 다루는 규모에 맞췄다
pub(super) const SEARCH_LIMIT: usize = 100_000;

/// 하위 폴더 이름 검색 상태 (FR-70) — `DirLoad`와 같은 방식(워커 + 세대 + 조각 채널)이다.
///
/// 멈춤 깃발을 따로 드는 이유: 일치가 드문 검색은 오래도록 아무것도 보내지 않아, 받는 쪽이
/// 사라진 것(전송 실패)으로는 워커를 세우지 못한다
pub(super) struct TreeSearch {
    generation: u64,
    stop: Arc<AtomicBool>,
    pending: Option<Receiver<(u64, SearchEvent, Vec<ExtInfo>)>>,
    ext_infos: Vec<ExtInfo>,
    /// 마지막 검색이 상한에서 끊겼는가 — 상태 줄이 알린다
    truncated: bool,
}

impl TreeSearch {
    pub(super) fn new() -> TreeSearch {
        TreeSearch {
            generation: 0,
            stop: Arc::new(AtomicBool::new(false)),
            pending: None,
            ext_infos: Vec::new(),
            truncated: false,
        }
    }

    /// 워커에서 `root` 아래를 찾기 시작한다 — 앞선 검색은 멈추고 그 결과는 세대로 버린다
    pub(super) fn start(
        &mut self,
        root: PathBuf,
        needle_lower: String,
        rules: SearchRules,
        ctx: &egui::Context,
    ) {
        self.cancel();
        let generation = self.generation;
        let stop = Arc::clone(&self.stop);
        let (tx, rx) = channel();
        self.pending = Some(rx);
        self.truncated = false;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // 확장자 선조회(`lookup_ext`)가 COM을 쓴다 — `DirLoad`와 같은 이유로 초기화한다.
            // 안전성: 이 스레드에서 열고 검색이 끝난 뒤 짝을 맞춰 닫는다
            let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
            let mut seen = HashSet::new();
            search_tree(
                &root,
                &needle_lower,
                rules,
                SEARCH_LIMIT,
                || stop.load(Ordering::Relaxed),
                |event| {
                    let infos = match &event {
                        SearchEvent::Hits(entries) => new_ext_infos(entries, &mut seen),
                        SearchEvent::Done { .. } => Vec::new(),
                    };
                    let sent = tx.send((generation, event, infos)).is_ok();
                    if sent {
                        ctx.request_repaint();
                    }
                    sent
                },
            );
            if com {
                // 안전성: 위에서 성공한 초기화와 짝을 맞춘다
                unsafe { CoUninitialize() };
            }
        });
    }

    /// 진행 중인 검색을 멈추고 이미 채널에 실린 결과도 버린다
    pub(super) fn cancel(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // 다음 검색은 새 깃발을 쓴다 — 멈춘 워커가 늦게 깃발을 읽어도 새 검색이 서지 않게
        self.stop = Arc::new(AtomicBool::new(false));
        self.generation += 1;
        self.pending = None;
    }

    /// 도착한 사건을 하나 꺼낸다. `Done`을 꺼내면 대기가 끝난다
    pub(super) fn poll(&mut self) -> Option<SearchEvent> {
        let rx = self.pending.as_ref()?;
        match rx.try_recv() {
            Ok((generation, event, infos)) => {
                self.ext_infos.extend(infos);
                if let SearchEvent::Done { truncated, .. } = &event {
                    self.pending = None;
                    if generation == self.generation {
                        self.truncated = *truncated;
                    }
                }
                (generation == self.generation).then_some(event)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                None
            }
        }
    }

    pub(super) fn is_running(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn truncated(&self) -> bool {
        self.truncated
    }

    /// 워커가 미리 물어 둔 확장자 정보를 꺼낸다 — 결과를 붙이기 전에 UI 캐시에 넣는다
    pub(super) fn take_ext_infos(&mut self) -> Vec<ExtInfo> {
        std::mem::take(&mut self.ext_infos)
    }
}

/// 이 조각에서 처음 보는 파일 확장자를 셸에 묻는다 — 폴더는 `IconCache`가 이미 안다
fn new_ext_infos(entries: &[FileEntry], seen: &mut HashSet<String>) -> Vec<ExtInfo> {
    entries
        .iter()
        .filter(|entry| !entry.is_dir)
        .map(FileEntry::extension)
        .filter(|ext| seen.insert(ext.clone()))
        .map(|ext| lookup_ext(&ext))
        .collect()
}

/// 새 폴더·새 파일 생성 상태 (FR-25).
///
/// 생성도 열거와 같이 **워커 스레드에서** 한다 — `CreateDirectoryW`·`CreateFileW`는 로컬
/// 디스크에서는 순식간이지만 네트워크 드라이브에서는 수 초가 걸릴 수 있고, 이름이 겹치면
/// 그만큼 재시도가 이어진다. UI 스레드에서 부르면 그동안 창이 멈춘다
/// (AGENTS: UI 스레드 블로킹 I/O 금지 — `DirLoad`와 같은 규칙)
pub(super) struct CreateOp {
    /// (무엇을 만들었는지, 결과) — 실패 문구에 종류를 넣기 위해 함께 보낸다
    pending: Option<Receiver<(&'static str, std::io::Result<PathBuf>)>>,
}

impl CreateOp {
    pub(super) fn new() -> CreateOp {
        CreateOp { pending: None }
    }

    /// 워커에서 생성을 시작한다. 이미 진행 중이면 무시한다 —
    /// 메뉴를 연달아 눌러도 한 번에 하나만 만든다
    pub(super) fn start(
        &mut self,
        dir: PathBuf,
        kind: &'static str,
        make: fn(&Path) -> std::io::Result<PathBuf>,
        ctx: &egui::Context,
    ) {
        if self.pending.is_some() {
            return;
        }
        let (tx, rx) = channel();
        self.pending = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // 수신부가 이미 버려졌으면(패널 닫힘·앱 종료) 전송 실패는 무해하다
            let _ = tx.send((kind, make(&dir)));
            ctx.request_repaint();
        });
    }

    /// 완료된 결과를 꺼낸다. 아직이면 `None`
    pub(super) fn poll(&mut self) -> Option<(&'static str, std::io::Result<PathBuf>)> {
        let rx = self.pending.as_ref()?;
        match rx.try_recv() {
            Ok(done) => {
                self.pending = None;
                Some(done)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                None
            }
        }
    }

    /// 만들기가 진행 중인가 — 원격 탭에서 로컬 생성이 시작되지 않았는지 가리는 신호다.
    /// 본체는 결과만 거두면 되므로(`poll`) 이 물음은 테스트에서만 쓴다
    #[cfg(test)]
    pub(super) fn is_running(&self) -> bool {
        self.pending.is_some()
    }
}
