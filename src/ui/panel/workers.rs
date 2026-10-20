//! 패널이 워커 스레드에 맡기는 일 — 폴더 열거·하위 폴더 검색·폴더 크기와 새 폴더·새 파일 만들기.
//!
//! 둘 다 **UI 스레드에서 부르면 안 되는 블로킹 I/O**라 같은 방식(워커 + 채널 + 다시 그리기 요청)으로
//! 감싼다. 본체(`ui::panel`)의 자식 모듈이며, 상태를 들고 결과를 거두는 일만 한다.

use crate::fs::enumerate::{EnumChunk, EnumOutcome, FileEntry, enumerate_dir_batched};
use crate::fs::icons::{ExtInfo, lookup_ext};
use crate::fs::search::{SearchEvent, SearchRules, search_tree};
use eframe::egui;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
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

/// 폴더 크기 계산 상태 (FR-71) — 패널마다 워커 하나가 요청을 **차례로** 계산한다.
///
/// 보이는 폴더 행만 요청되므로 스크롤할 때마다 요청이 는다 — 한 줄로 세워야 같은 디스크를
/// 여러 스레드가 동시에 긁지 않는다. 세대는 워커와 나눠 들어, 폴더를 옮기면 **계산 중인 것도**
/// 그 자리에서 멈춘다. 값은 세션에 담지 않는다
pub(super) struct FolderSizes {
    generation: Arc<AtomicU64>,
    /// 요청 통로 — 첫 요청 때 워커와 함께 만든다. 패널이 사라져 이것이 버려지면 워커도 끝난다
    requests: Option<Sender<(u64, PathBuf)>>,
    results: Option<Receiver<(u64, PathBuf, Option<u64>)>>,
    known: HashMap<PathBuf, u64>,
    /// 이 세대에 이미 요청한 경로 — 계산 중·실패·네트워크인 것도 다시 묻지 않는다
    requested: HashSet<PathBuf>,
    /// 이 세대에서 아직 답이 오지 않은 요청 수
    waiting: usize,
    /// 잊은 뒤에도 옛 계산의 답이 아직 오지 않은 폴더 — 그 답은 받지 않는다 (plan D14)
    forgotten: HashSet<PathBuf>,
}

impl FolderSizes {
    pub(super) fn new() -> FolderSizes {
        FolderSizes {
            generation: Arc::new(AtomicU64::new(0)),
            requests: None,
            results: None,
            known: HashMap::new(),
            requested: HashSet::new(),
            waiting: 0,
            forgotten: HashSet::new(),
        }
    }

    /// 지금까지의 값과 진행 중인 계산을 버린다 — 폴더를 옮겼거나 `F5`를 눌렀을 때
    pub(super) fn reset(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.known.clear();
        self.requested.clear();
        self.waiting = 0;
        self.forgotten.clear();
    }

    /// 파일 작업이 손댄 자리를 품은 폴더의 값을 잊는다 — 보이면 다음 프레임에 다시 잰다
    /// (FR-71 · plan D14). 계산 중이던 것의 옛 답은 받지 않는다
    pub(super) fn forget(&mut self, affected: &[PathBuf]) {
        let stale: Vec<PathBuf> = self
            .requested
            .iter()
            .filter(|folder| affected.iter().any(|path| is_within(path, folder)))
            .cloned()
            .collect();
        for folder in stale {
            self.requested.remove(&folder);
            if self.known.remove(&folder).is_none() {
                // 값이 없으면 아직 계산 중이다 — 그 답은 손대기 전의 크기다
                self.forgotten.insert(folder);
            }
        }
    }

    /// 알려진 폴더 크기 — 자세히 보기가 크기 칸에 쓴다
    pub(super) fn known(&self) -> &HashMap<PathBuf, u64> {
        &self.known
    }

    /// 아직 요청하지 않은 폴더만 워커에 넘긴다. 새로 넘긴 수를 돌려준다
    pub(super) fn request(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) -> usize {
        let mut sent = 0;
        for path in paths {
            if !self.requested.insert(path.clone()) {
                continue;
            }
            let generation = self.generation.load(Ordering::Relaxed);
            let tx = self.requests.get_or_insert_with(|| {
                let (tx, rx) = channel();
                let (done_tx, done_rx) = channel();
                spawn_size_worker(rx, done_tx, Arc::clone(&self.generation), ctx.clone());
                self.results = Some(done_rx);
                tx
            });
            if tx.send((generation, path)).is_ok() {
                self.waiting += 1;
                sent += 1;
            }
        }
        sent
    }

    /// 도착한 결과를 거둔다
    pub(super) fn poll(&mut self) {
        let mut done = Vec::new();
        if let Some(rx) = self.results.as_ref() {
            while let Ok(result) = rx.try_recv() {
                done.push(result);
            }
        }
        for (generation, path, size) in done {
            self.accept(generation, path, size);
        }
    }

    /// 결과 하나를 반영한다 — 옛 세대의 것은 버린다(폴더를 옮기기 전에 끝난 계산)
    fn accept(&mut self, generation: u64, path: PathBuf, size: Option<u64>) {
        if generation != self.generation.load(Ordering::Relaxed) {
            return;
        }
        self.waiting = self.waiting.saturating_sub(1);
        if self.forgotten.remove(&path) {
            return;
        }
        if let Some(size) = size {
            self.known.insert(path, size);
        }
    }

    /// 아직 답을 기다리는 계산이 있는가 — 시험이 기다릴 때 쓴다
    #[cfg(test)]
    pub(super) fn is_waiting(&self) -> bool {
        self.waiting > 0
    }
}

/// `path`가 `folder` 자신이거나 그 안에 있는가 — 구성요소마다 ASCII 대소문자를 접어 견준다
fn is_within(path: &Path, folder: &Path) -> bool {
    let mut inside = path.components();
    folder.components().all(|part| {
        inside.next().is_some_and(|other| {
            other
                .as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&part.as_os_str().to_string_lossy())
        })
    })
}

/// 폴더 크기 워커 — 요청 통로가 닫힐 때까지(패널이 사라질 때까지) 요청을 차례로 계산한다.
///
/// 네트워크 너머의 폴더는 계산하지 않는다(FR-71) — 수만 항목을 네트워크로 훑으면 분 단위가 걸리고
/// 그동안 다른 폴더의 요청이 뒤에서 기다린다. 판정도 Win32 호출이라 여기서 한다
fn spawn_size_worker(
    requests: Receiver<(u64, PathBuf)>,
    results: Sender<(u64, PathBuf, Option<u64>)>,
    generation: Arc<AtomicU64>,
    ctx: egui::Context,
) {
    std::thread::spawn(move || {
        while let Ok((asked, path)) = requests.recv() {
            if asked != generation.load(Ordering::Relaxed) {
                continue;
            }
            let size = if crate::fs::drives::is_network_path(&path) {
                None
            } else {
                crate::fs::dir_size::folder_size(&path, || {
                    generation.load(Ordering::Relaxed) != asked
                })
            };
            if results.send((asked, path, size)).is_err() {
                return;
            }
            ctx.request_repaint();
        }
    });
}

/// zip 목차 한 번의 답 — 세대, (목록 | 실패 사유), 미리 물어 둔 확장자 정보
type ZipReply = (u64, Result<Vec<FileEntry>, String>, Vec<ExtInfo>);

/// zip 안 목차 읽기 상태 (FR-78) — `DirLoad`와 같은 방식(워커 + 세대 + 채널)이다.
///
/// 목차는 조각으로 나누지 않고 한 번에 보낸다 — 머리글만 읽어 열거보다 짧고, 조각으로 나누면
/// 「이어 붙이기」 갈래가 zip 에도 생긴다
pub(super) struct ZipLoad {
    generation: u64,
    pending: Option<Receiver<ZipReply>>,
    ext_infos: Vec<ExtInfo>,
    /// 지금까지 받은 확장자 — `DirLoad::known_exts` 와 같은 구실이다
    known_exts: HashSet<String>,
}

impl ZipLoad {
    pub(super) fn new() -> ZipLoad {
        ZipLoad {
            generation: 0,
            pending: None,
            ext_infos: Vec::new(),
            known_exts: HashSet::new(),
        }
    }

    /// 기다리던 답을 버린다 — 세대를 올려 이미 채널에 실린 답도 폐기한다
    pub(super) fn cancel(&mut self) {
        self.generation += 1;
        self.pending = None;
    }

    /// 워커 스레드에서 `archive` 의 `inner` 바로 아래를 읽는다. 앞선 요청의 답은 세대로 버려진다
    pub(super) fn start(&mut self, archive: PathBuf, inner: String, ctx: &egui::Context) {
        self.generation += 1;
        let generation = self.generation;
        let (tx, rx) = channel();
        self.pending = Some(rx);
        let ctx = ctx.clone();
        let mut seen = self.known_exts.clone();
        std::thread::spawn(move || {
            let listed = crate::fs::zip_archive::list(&archive, &inner).map_err(|e| e.to_string());
            // 종류 문자열은 셸(COM)에 묻는다 — `DirLoad`와 같은 이유로 이 스레드에서 연다.
            // 안전성: 이 스레드에서 열고 물은 뒤 짝을 맞춰 닫는다
            let infos = match &listed {
                Ok(entries) => {
                    let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
                    let infos = new_ext_infos(entries, &mut seen);
                    if com {
                        // 안전성: 위에서 성공한 초기화와 짝을 맞춘다
                        unsafe { CoUninitialize() };
                    }
                    infos
                }
                Err(_) => Vec::new(),
            };
            if tx.send((generation, listed, infos)).is_ok() {
                ctx.request_repaint();
            }
        });
    }

    /// 도착한 답 — 아직이거나 옛 요청의 것이면 `None`
    pub(super) fn poll(&mut self) -> Option<Result<Vec<FileEntry>, String>> {
        let rx = self.pending.as_ref()?;
        match rx.try_recv() {
            Ok((generation, listed, infos)) => {
                self.pending = None;
                // 확장자 정보는 세대와 무관하게 받는다 — 확장자에 매인 값이라 늦게 온 것도 틀리지 않는다
                self.known_exts
                    .extend(infos.iter().map(|info| info.ext.clone()));
                self.ext_infos.extend(infos);
                (generation == self.generation).then_some(listed)
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

    /// 워커가 미리 물어 둔 확장자 정보를 꺼낸다 — 목록을 세우기 전에 UI 캐시에 넣는다
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

/// 새 폴더·새 파일 하나를 만든 결과 — 만든 자리와 그 도장
pub(super) type CreateResult = std::io::Result<(PathBuf, Option<crate::fs::undo::Stamp>)>;

/// 새 폴더·새 파일 생성 상태 (FR-25).
///
/// 생성도 열거와 같이 **워커 스레드에서** 한다 — `CreateDirectoryW`·`CreateFileW`는 로컬
/// 디스크에서는 순식간이지만 네트워크 드라이브에서는 수 초가 걸릴 수 있고, 이름이 겹치면
/// 그만큼 재시도가 이어진다. UI 스레드에서 부르면 그동안 창이 멈춘다
/// (AGENTS: UI 스레드 블로킹 I/O 금지 — `DirLoad`와 같은 규칙)
pub(super) struct CreateOp {
    /// (무엇을 만들었는지, 결과) — 실패 문구에 종류를 넣기 위해 함께 보낸다.
    /// 결과에는 만든 자리의 도장이 붙는다 — 되돌리기(FR-74)가 그 사이 바뀌었는지 가른다
    pending: Option<Receiver<(&'static str, CreateResult)>>,
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
            // 도장은 여기(워커)서 잰다 — UI 스레드에서 파일시스템을 묻지 않는다
            let made = make(&dir).map(|path| {
                let stamp = crate::fs::undo::stamp_of(&path);
                (path, stamp)
            });
            // 수신부가 이미 버려졌으면(패널 닫힘·앱 종료) 전송 실패는 무해하다
            let _ = tx.send((kind, made));
            ctx.request_repaint();
        });
    }

    /// 완료된 결과를 꺼낸다. 아직이면 `None`
    pub(super) fn poll(&mut self) -> Option<(&'static str, CreateResult)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 같은_폴더는_한_번만_요청한다() {
        let ctx = egui::Context::default();
        let mut sizes = FolderSizes::new();
        let dir = std::env::temp_dir();
        assert_eq!(sizes.request(vec![dir.clone(), dir.clone()], &ctx), 1);
        assert_eq!(
            sizes.request(vec![dir], &ctx),
            0,
            "이미 요청한 폴더를 다시 보냈다"
        );
    }

    #[test]
    fn 비운_뒤에_도착한_옛_결과는_버린다() {
        // 폴더를 옮기기 전에 끝난 계산이 새 폴더의 같은 이름 자리에 서면 안 된다
        let mut sizes = FolderSizes::new();
        let old = sizes.generation.load(Ordering::Relaxed);
        sizes.reset();
        sizes.accept(old, PathBuf::from(r"C:\옛\사진"), Some(10));
        assert!(sizes.known().is_empty());
        let now = sizes.generation.load(Ordering::Relaxed);
        sizes.accept(now, PathBuf::from(r"C:\새\사진"), Some(20));
        assert_eq!(sizes.known().get(&PathBuf::from(r"C:\새\사진")), Some(&20));
    }

    #[test]
    fn 워커가_잰_크기가_돌아온다() {
        let dir = std::env::temp_dir().join(format!("fe_sizes_worker_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join(r"sub\a.bin"), vec![0u8; 7]).unwrap();
        let ctx = egui::Context::default();
        let mut sizes = FolderSizes::new();
        sizes.request(vec![dir.clone()], &ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while sizes.is_waiting() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            sizes.poll();
        }
        assert_eq!(sizes.known().get(&dir), Some(&7));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 값을 손으로 넣은 크기 상태 — 워커 없이 잊기 규칙만 잰다
    fn 알려진_크기(folders: &[&str]) -> FolderSizes {
        let mut sizes = FolderSizes::new();
        for folder in folders {
            sizes.requested.insert(PathBuf::from(folder));
            sizes.known.insert(PathBuf::from(folder), 10);
        }
        sizes
    }

    #[test]
    fn 영향받은_폴더의_크기를_잊는다() {
        // 하위 폴더로 파일을 옮기면 그 폴더의 크기가 바뀐다 (plan D14)
        let mut sizes = 알려진_크기(&[r"C:\w\sub", r"C:\w\other"]);
        sizes.forget(&[PathBuf::from(r"C:\W\SUB\deep\a.txt")]);
        assert_eq!(sizes.known().get(Path::new(r"C:\w\sub")), None);
    }

    #[test]
    fn 영향_밖_폴더의_크기는_남는다() {
        let mut sizes = 알려진_크기(&[r"C:\w\sub", r"C:\w\subway"]);
        sizes.forget(&[PathBuf::from(r"C:\w\sub\a.txt")]);
        assert_eq!(
            sizes.known().get(Path::new(r"C:\w\subway")),
            Some(&10),
            "이름 앞부분만 같은 다른 폴더다"
        );
    }

    #[test]
    fn 잊은_폴더는_다시_요청된다() {
        let ctx = egui::Context::default();
        let mut sizes = 알려진_크기(&[]);
        let dir = std::env::temp_dir();
        // 계산 중(값 없이 요청만 됨)에 잊으면 옛 답은 받지 않고 다시 요청한다
        sizes.requested.insert(dir.clone());
        sizes.forget(&[dir.join("x.txt")]);
        assert_eq!(sizes.request(vec![dir.clone()], &ctx), 1, "다시 요청한다");
        let generation = sizes.generation.load(Ordering::Relaxed);
        sizes.accept(generation, dir.clone(), Some(1));
        assert_eq!(
            sizes.known().get(&dir),
            None,
            "잊기 전의 옛 답은 받지 않는다"
        );
        sizes.accept(generation, dir.clone(), Some(2));
        assert_eq!(sizes.known().get(&dir), Some(&2));
    }

    /// `a.txt` 하나와 `sub/b.txt` 를 담은 시험 zip
    fn small_zip(tag: &str) -> (PathBuf, PathBuf) {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("moa_zipload_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("시험 폴더");
        let archive = dir.join("a.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&archive).expect("zip"));
        let options = zip::write::SimpleFileOptions::default();
        for name in ["a.txt", "sub/b.txt"] {
            writer.start_file(name, options).expect("항목");
            writer.write_all(b"x").expect("본문");
        }
        writer.finish().expect("zip 닫기");
        (dir, archive)
    }

    fn wait(load: &mut ZipLoad) -> Option<Result<Vec<FileEntry>, String>> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Some(reply) = load.poll() {
                return Some(reply);
            }
            if !load.is_loading() {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        None
    }

    #[test]
    fn zip_목차_워커가_답을_낸다() {
        let (dir, archive) = small_zip("reply");
        let ctx = egui::Context::default();
        let mut load = ZipLoad::new();
        load.start(archive, String::new(), &ctx);
        let reply = wait(&mut load).expect("답이 오지 않았다");
        let mut names: Vec<String> = reply
            .expect("목차")
            .iter()
            .map(FileEntry::name_string)
            .collect();
        names.sort();
        assert_eq!(names, ["a.txt", "sub"]);
        assert!(!load.is_loading());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 옛_zip_요청의_답은_버린다() {
        // 연달아 옮기면 먼저 건 요청의 답이 나중에 올 수 있다 — 마지막 요청의 답만 받는다
        let (dir, archive) = small_zip("stale");
        let ctx = egui::Context::default();
        let mut load = ZipLoad::new();
        load.start(archive.clone(), String::new(), &ctx);
        load.start(archive, "sub".to_owned(), &ctx);
        let reply = wait(&mut load).expect("답이 오지 않았다").expect("목차");
        let names: Vec<String> = reply.iter().map(FileEntry::name_string).collect();
        assert_eq!(names, ["b.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
