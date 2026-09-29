//! 파일 썸네일 미리보기 — 워커 스레드 + LRU 캐시 (FR-24·NFR-9).
//!
//! 셸 형식 아이콘(`fs::icons`)과 달리 **파일마다 디스크를 읽으므로** UI 스레드에서 부를 수 없다
//! (AGENTS: UI 스레드 블로킹 I/O 금지). 요청을 워커에 보내고 결과를 채널로 받는다.
//!
//! 캐시는 **RGBA 이미지 단계에서** 상한을 건다 — 텍스처가 아니라 여기서 걸어야 메모리 상한이
//! 정확히 지켜진다(256×256 RGBA 한 장이 256KB, 200장이면 약 50MB — NFR-9).
//! 이 모듈은 UI를 모른다: 픽셀과 크기만 돌려주고 텍스처로 올리는 일은 `ui` 계층이 한다.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{DeleteObject, HBITMAP};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_RESIZETOFIT,
};
use windows::core::HSTRING;

use crate::fs::bitmap::bgra_from_hbitmap;
use crate::fs::enumerate::FileStamp;

/// 패널 하나가 들고 있을 썸네일 수 상한 (NFR-9 — 약 50MB).
/// 넘으면 가장 오래 안 쓴 것부터 버린다
pub const MAX_CACHED: usize = 200;

/// 캐시가 쥘 수 있는 픽셀 바이트 상한 (NFR-9 — 200장 × 256×256×4).
/// 장수 상한과 함께 걸어, 셸이 요청보다 큰 그림을 줘도 총량이 넘지 않게 한다
pub const MAX_CACHED_BYTES: usize = MAX_CACHED * (THUMB_PX * THUMB_PX * 4) as usize;

/// 만들 썸네일의 한 변 — 아주 큰 아이콘(256px)에 맞춘다.
/// 더 작은 보기 모드는 이 한 장을 줄여 쓴다(작게 만들어 두면 큰 모드에서 뭉개진다)
pub const THUMB_PX: i32 = 256;

/// 워커가 돌려주는 썸네일 픽셀. `ui` 계층이 이것을 텍스처로 올린다
#[derive(Clone, PartialEq, Debug)]
pub struct ThumbnailImage {
    pub width: usize,
    pub height: usize,
    /// RGBA 스트레이트 알파 (egui `ColorImage::from_rgba_unmultiplied`가 받는 형식)
    pub rgba: Vec<u8>,
}

/// 워커에게 보내는 요청. 세대 번호를 실어 보내 **늦게 도착한 이전 폴더의 결과**를 가려낸다.
///
/// **도장(`stamp`)도 실어 보낸다** — 결과가 어느 내용의 그림인지 되돌아와야, 그 사이 파일이
/// 다시 바뀌어 새 요청이 나간 뒤 늦게 온 옛 결과를 가려낼 수 있다
enum Request {
    Make {
        generation: u64,
        path: PathBuf,
        stamp: FileStamp,
    },
    Stop,
}

/// 워커가 돌려주는 결과 — `(세대, 경로, 도장, 그림)`
type Outcome = (u64, PathBuf, FileStamp, Option<ThumbnailImage>);

/// 한 번에 비운 요청들에서 **경로별 마지막 `Make`만** 남긴다 — `Stop`은 그대로 둔다.
///
/// 앞선 요청은 곧 대체될 내용이라 만들어 봐야 `accept`가 버린다. 남는 순서는 각 경로가
/// 마지막으로 나온 순서다
fn coalesce_requests(batch: Vec<Request>) -> Vec<Request> {
    let is_last = |index: usize, path: &Path| {
        !batch[index + 1..]
            .iter()
            .any(|later| matches!(later, Request::Make { path: p, .. } if p == path))
    };
    let keep: Vec<bool> = batch
        .iter()
        .enumerate()
        .map(|(index, request)| match request {
            Request::Make { path, .. } => is_last(index, path),
            Request::Stop => true,
        })
        .collect();
    batch
        .into_iter()
        .zip(keep)
        .filter_map(|(request, keep)| keep.then_some(request))
        .collect()
}

/// 썸네일 캐시 — 요청 큐·결과 수신·LRU 축출을 함께 관리한다.
///
/// 패널마다 하나씩 둔다(NFR-9의 상한이 패널당이다). 폴더를 떠나면 `clear`로 비운다
pub struct ThumbnailCache {
    tx: Sender<Request>,
    rx: Receiver<Outcome>,
    /// 완성된 썸네일과 **그것을 만든 내용의 도장**. `None`은 **만들 수 없는 파일**(썸네일 없는
    /// 형식)이며, 다시 요청하지 않기 위해 실패도 기억한다
    ready: HashMap<PathBuf, (FileStamp, Option<ThumbnailImage>)>,
    /// 최근 사용 순서 — 앞이 가장 오래됐다. 항목 수가 상한을 넘으면 앞에서 버린다
    order: Vec<PathBuf>,
    /// 요청을 보냈고 아직 결과가 안 온 것과 그 도장 — 같은 내용을 거듭 요청하지 않는다
    pending: Vec<(PathBuf, FileStamp)>,
    /// 지금 담긴 썸네일이 속한 폴더. **호출 경로마다 판정을 흩지 않으려고 캐시가 직접 든다** —
    /// 탐색·탭 전환·탭 닫기가 각자 다른 순서로 폴더를 바꾸므로, 바깥에서 비교하면
    /// 한 경로만 빠뜨려도 조용히 새어나간다 (F-7 B1·m1)
    folder: PathBuf,
    /// 폴더를 떠날 때마다 오르는 번호. 결과에 실려 돌아오며, 지금 세대와 다르면 버린다 —
    /// 폴더를 빠르게 오가면 이전 폴더의 요청이 나중에 도착해 캐시 자리를 차지한다
    /// (`ui::panel`의 `DirLoad`가 쓰는 것과 같은 방식)
    generation: u64,
    /// **내용이 바뀐 파일의 안정 대기** — `(새 도장, 그 도장을 처음 본 시각)` (2026-09-29).
    ///
    /// 내려받는 중인 파일처럼 계속 바뀌는 파일은 감시 갱신(약 0.3초)마다 도장이 달라져,
    /// 그때마다 다시 만들면 보이는 동안 셸이 파일을 계속 읽는다. 그래서 이미 그림이 있는
    /// 경로는 도장이 [`SETTLE`] 동안 그대로일 때만 다시 만든다(처음 보는 경로는 곧바로)
    settling: HashMap<PathBuf, (FileStamp, Instant)>,
}

/// 내용이 바뀐 파일을 다시 만들기 전에 도장이 그대로여야 하는 시간 (2026-09-29 사용자 선택)
pub const SETTLE: Duration = Duration::from_secs(2);

impl ThumbnailCache {
    pub fn new() -> ThumbnailCache {
        let (request_tx, request_rx) = channel::<Request>();
        let (result_tx, result_rx) = channel();
        std::thread::spawn(move || worker(request_rx, result_tx));
        ThumbnailCache {
            tx: request_tx,
            rx: result_rx,
            ready: HashMap::new(),
            order: Vec::new(),
            pending: Vec::new(),
            folder: PathBuf::new(),
            generation: 0,
            settling: HashMap::new(),
        }
    }

    /// 표시 폴더가 바뀌었으면 비운다 (NFR-9). 같은 폴더면 그대로 둔다 —
    /// 감시 갱신(FR-10)마다 버리면 다른 앱이 파일 하나만 만들어도 폴더 전체를 다시 만든다
    pub fn set_folder(&mut self, folder: &Path) -> bool {
        if self.folder == folder {
            return false;
        }
        self.clear();
        self.folder = folder.to_path_buf();
        true
    }

    /// 썸네일을 요청한다. **같은 도장으로** 이미 있으면 **최근 사용으로 올리고** 끝낸다.
    ///
    /// 화면에 보이는 항목마다 매 프레임 불리므로, 여기서 올려야 보이는 것이 축출되지 않는다 —
    /// 그리기는 텍스처만 보고 픽셀 캐시를 건드리지 않아 이 경로가 유일한 갱신 지점이다.
    ///
    /// **도장이 다르면 다시 만든다**(2026-09-29) — 같은 이름의 파일이 새 내용으로 바뀐 것이다.
    /// 그동안 **옛 그림은 그대로 둔다**: 형식 아이콘으로 떨어졌다 돌아오는 깜빡임이 없고,
    /// 새 결과가 오면 `accept`가 갈아 끼운다. 다시 만드는 것은 도장이 [`SETTLE`] 동안 그대로일
    /// 때다(`request_at`)
    pub fn request(&mut self, path: &Path, stamp: FileStamp) {
        self.request_at(path, stamp, Instant::now());
    }

    /// [`request`]의 판정 — 현재 시각을 바깥에서 받는다(안정 대기를 시험에서 재현한다)
    pub fn request_at(&mut self, path: &Path, stamp: FileStamp, now: Instant) {
        let held = self.stamp_of(path);
        if held.is_some() {
            // 보이는 것이므로 도장과 무관하게 최근으로 올린다 — 새 그림을 기다리는 동안에도
            // 옛 그림이 축출되지 않게
            self.touch(path);
        }
        if held == Some(stamp) {
            // 담긴 그림과 같아졌다 — 기다리던 것이 있었다면 더는 기다릴 일이 없다
            self.settling.remove(path);
            return;
        }
        if self.pending.iter().any(|(p, s)| p == path && *s == stamp) {
            return;
        }
        // **이미 그림이 있는데 도장이 다르면 안정 대기** — 도장이 `SETTLE` 동안 그대로일 때만
        // 청한다. 도장이 또 바뀌면 그 시각부터 다시 잰다
        if held.is_some() {
            match self.settling.get(path) {
                Some((waiting, since)) if *waiting == stamp => {
                    if now.saturating_duration_since(*since) < SETTLE {
                        return;
                    }
                }
                _ => {
                    self.settling.insert(path.to_path_buf(), (stamp, now));
                    return;
                }
            }
        }
        self.settling.remove(path);
        // 옛 도장으로 기다리던 것이 있으면 새 도장으로 바꿔 단다 — 옛 결과는 `accept`가 버린다
        self.pending.retain(|(p, _)| p != path);
        self.pending.push((path.to_path_buf(), stamp));
        // 워커가 죽었으면(앱 종료 중) 전송 실패는 무해하다
        let _ = self.tx.send(Request::Make {
            generation: self.generation,
            path: path.to_path_buf(),
            stamp,
        });
    }

    /// 도착한 결과를 받아들인다. **새로 준비된 경로들**을 돌려준다 —
    /// 호출부(`ui`)가 그것만 텍스처로 올리면 된다
    pub fn poll(&mut self) -> Vec<PathBuf> {
        let mut arrived = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok((generation, path, stamp, image)) => {
                    // 폴더를 떠난 뒤·파일이 다시 바뀐 뒤 도착한 결과는 `accept`가 걸러낸다
                    if self.accept(generation, path.clone(), stamp, image) {
                        arrived.push(path);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        arrived
    }

    /// 보냈지만 아직 결과가 안 온 요청이 있는가.
    ///
    /// 워커 결과는 채널로만 오고 이 모듈은 egui를 모르므로(계층 단방향), 화면 쪽이
    /// **기다리는 동안 스스로 깨어나야** 도착을 알아챈다 — 그 판단에 쓰인다
    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// 준비된 썸네일. 없으면 `None`(아직이거나 만들 수 없는 파일).
    /// 꺼내 쓰면 **최근 사용으로 올린다** — 화면에 보이는 것이 먼저 버려지지 않게 한다
    pub fn get(&mut self, path: &Path) -> Option<&ThumbnailImage> {
        if self.ready.contains_key(path) {
            self.touch(path);
        }
        self.ready.get(path)?.1.as_ref()
    }

    /// 폴더를 떠날 때 호출 — 그 폴더의 썸네일을 즉시 놓는다 (NFR-9).
    ///
    /// **세대를 올려** 진행 중이던 요청의 결과가 나중에 도착해도 버려지게 한다.
    /// 워커는 이미 만들던 것을 끝까지 만들지만 그 결과는 `poll`이 걸러낸다
    pub fn clear(&mut self) {
        self.ready.clear();
        self.order.clear();
        self.pending.clear();
        self.settling.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    /// 안정 대기 중 **만기가 아직 오지 않은** 것 가운데 가장 가까운 만기까지 남은 시간.
    ///
    /// 화면은 이 시간 뒤에 다시 그려야 재요청 시점이 온다 — 파일이 멈춘 뒤에는 감시 통지도
    /// 입력도 없어 프레임이 돌지 않는다. **만기가 지난 항목은 세지 않는다**: 만기 시각에 한 번
    /// 깨웠으면 그 몫은 끝났고, 그 뒤에도 남은 것은 화면에서 빠진 경로라 세면 매 프레임
    /// 다시 그리게 된다
    pub fn settle_due(&self, now: Instant) -> Option<Duration> {
        self.settling
            .values()
            .map(|(_, since)| (*since + SETTLE).saturating_duration_since(now))
            .filter(|left| !left.is_zero())
            .min()
    }

    /// 캐시에 든 항목 수 (실패로 기억한 것 포함) — 상한 검증용
    pub fn len(&self) -> usize {
        self.ready.len()
    }

    /// 테스트에서 워커를 거치지 않고 결과를 넣는다 —
    /// 텍스처 캐시 동기화처럼 셸 호출과 무관한 로직을 검증하는 데 쓴다
    #[cfg(test)]
    pub fn accept_for_test(&mut self, path: PathBuf, image: Option<ThumbnailImage>) {
        self.insert(path, FileStamp::default(), image);
    }

    /// 위와 같되 도장을 정해 넣는다 — 같은 경로의 내용이 바뀐 경우를 시험한다
    #[cfg(test)]
    pub fn accept_for_test_stamped(
        &mut self,
        path: PathBuf,
        stamp: FileStamp,
        image: Option<ThumbnailImage>,
    ) {
        self.insert(path, stamp, image);
    }

    /// 만들어진 썸네일이 있는 경로들 — 텍스처 캐시가 동기화에 쓴다.
    /// 실패로 기억한 것(`None`)은 올릴 그림이 없으므로 뺀다
    pub fn ready_paths(&self) -> Vec<PathBuf> {
        self.ready
            .iter()
            .filter(|(_, (_, image))| image.is_some())
            .map(|(path, _)| path.clone())
            .collect()
    }

    /// 이 경로의 썸네일이 캐시에 있는가 (실패 기억은 제외).
    /// 텍스처 캐시가 "픽셀이 사라진 텍스처"를 찾아내는 데 쓴다
    pub fn has_image(&self, path: &Path) -> bool {
        self.ready
            .get(path)
            .is_some_and(|(_, image)| image.is_some())
    }

    /// 담긴 그림을 만든 내용의 도장 — 텍스처 캐시가 「그림이 바뀌었는가」를 가르는 데 쓴다
    pub fn stamp_of(&self, path: &Path) -> Option<FileStamp> {
        self.ready.get(path).map(|(stamp, _)| *stamp)
    }

    /// 최근 사용 순서를 바꾸지 않고 들여다본다 — 동기화 중에는 순서를 흔들면 안 된다
    pub fn peek(&self, path: &Path) -> Option<&ThumbnailImage> {
        self.ready.get(path)?.1.as_ref()
    }

    /// 캐시가 쥐고 있는 픽셀 바이트 합 — NFR-9 상한이 실제로 지켜지는지 재는 데 쓴다.
    /// 이론값(200 × 256KB)이 아니라 **실제 담긴 이미지**의 크기다 —
    /// 썸네일은 비율을 지켜 만들어져 원본이 정사각형이 아니면 256×256보다 작다
    pub fn memory_bytes(&self) -> usize {
        self.ready
            .values()
            .filter_map(|(_, image)| image.as_ref())
            .map(|image| image.rgba.len())
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.ready.is_empty()
    }

    /// 도착한 결과 하나를 세대 검사 후 받아들인다. 담았으면 `true`.
    /// `poll`과 테스트가 같은 판정을 쓰도록 한 곳에 둔다
    fn accept(
        &mut self,
        generation: u64,
        path: PathBuf,
        stamp: FileStamp,
        image: Option<ThumbnailImage>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        // **파일이 또 바뀌어 새 도장으로 기다리는 중이면 옛 결과는 버린다** — 담으면 새 그림이
        // 오기 전까지 더 옛 그림이 한 번 비친다
        if self
            .pending
            .iter()
            .any(|(p, waiting)| p == &path && *waiting != stamp)
        {
            return false;
        }
        self.pending.retain(|(p, _)| p != &path);
        let is_image = image.is_some();
        self.insert(path, stamp, image);
        is_image
    }

    fn insert(&mut self, path: PathBuf, stamp: FileStamp, image: Option<ThumbnailImage>) {
        if self.ready.insert(path.clone(), (stamp, image)).is_none() {
            self.order.push(path);
        }
        self.evict();
    }

    /// 최근 사용으로 올린다
    fn touch(&mut self, path: &Path) {
        if let Some(index) = self.order.iter().position(|p| p == path) {
            let entry = self.order.remove(index);
            self.order.push(entry);
        }
    }

    /// 상한을 넘으면 가장 오래 안 쓴 것부터 버린다.
    ///
    /// **장수와 바이트를 함께 본다** — 셸이 요청보다 큰 그림을 주는 경우를 대비한 이중 안전이다.
    /// 장수만 세면 한 장이 커질 때 전체 메모리가 상한을 넘는다 (NFR-9)
    fn evict(&mut self) {
        while self.order.len() > MAX_CACHED
            || (self.memory_bytes() > MAX_CACHED_BYTES && self.order.len() > 1)
        {
            let oldest = self.order.remove(0);
            self.ready.remove(&oldest);
            // 축출된 경로는 다음에 「처음 보는 경로」로 곧바로 청해진다 — 기다림은 뜻을 잃었다
            self.settling.remove(&oldest);
        }
    }
}

impl Default for ThumbnailCache {
    fn default() -> ThumbnailCache {
        ThumbnailCache::new()
    }
}

impl Drop for ThumbnailCache {
    fn drop(&mut self) {
        // 워커를 세운다 — 보내지 못해도(이미 죽음) 무해하다
        let _ = self.tx.send(Request::Stop);
    }
}

/// 워커 스레드 본체 — 요청을 받아 썸네일을 만들고 결과를 돌려준다.
///
/// **스레드마다 COM을 따로 초기화한다** — 셸 인터페이스는 아파트 단위라
/// 메인 스레드의 초기화가 여기까지 미치지 않는다
fn worker(rx: Receiver<Request>, tx: Sender<Outcome>) {
    // 안전성: 이 스레드에서 초기화하고, **성공했을 때만** 같은 스레드에서 해제한다 —
    // 실패한 초기화를 짝지어 해제하면 COM 참조 수가 어긋난다.
    // 실패해도 셸 호출이 동작하는 경우가 있어 작업 자체는 계속 시도한다
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    'outer: while let Ok(first) = rx.recv() {
        // 기다리는 동안 쌓인 것을 함께 비워 **경로별 마지막 요청만** 만든다 — 곧 대체될
        // 내용을 만들어 봐야 `accept`가 버린다
        let mut batch = vec![first];
        batch.extend(rx.try_iter());
        for request in coalesce_requests(batch) {
            match request {
                Request::Make {
                    generation,
                    path,
                    stamp,
                } => {
                    let image = make_thumbnail(&path);
                    if tx.send((generation, path, stamp, image)).is_err() {
                        break 'outer; // 수신부가 사라졌다 — 패널이 닫혔거나 앱이 끝났다
                    }
                }
                Request::Stop => break 'outer,
            }
        }
    }
    if initialized {
        // 안전성: 위에서 성공한 초기화와 같은 스레드에서 1회 호출
        unsafe {
            CoUninitialize();
        }
    }
}

/// 파일 하나의 썸네일을 만든다. 만들 수 없으면 `None`(형식 아이콘으로 폴백된다).
///
/// 안전성: COM이 초기화된 스레드에서만 호출한다. 얻은 HBITMAP은 이 함수 안에서 해제한다
fn make_thumbnail(path: &Path) -> Option<ThumbnailImage> {
    unsafe {
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None).ok()?;
        let size = SIZE {
            cx: THUMB_PX,
            cy: THUMB_PX,
        };
        // RESIZETOFIT만 준다 — 비율을 지키며 요청 크기 **안으로** 맞춘다.
        // `BIGGERSIZEOK`을 함께 주면 셸이 요청보다 큰 그림을 돌려줄 수 있어 한 장이
        // 256×256×4를 넘고, 그러면 장수로만 거는 상한(NFR-9)이 바이트를 보장하지 못한다.
        // ICONONLY를 주지 않으므로 썸네일이 있으면 그것이 온다
        let bitmap = factory.GetImage(size, SIIGBF_RESIZETOFIT).ok()?;
        let image = bitmap_to_rgba(bitmap);
        let _ = DeleteObject(bitmap.into());
        image
    }
}

/// 셸이 준 비트맵을 이 모듈이 쓰는 RGBA 이미지로 바꾼다.
///
/// GDI로 픽셀을 읽는 것은 `fs::bitmap`이 하고(그 절차가 네 곳에서 같았다), 여기서는
/// **BGRA → RGBA 뒤집기와 알파 되돌리기**만 한다 — 그 후처리가 사용처마다 달라 공용
/// 모듈에 넣지 않았다.
fn bitmap_to_rgba(bitmap: HBITMAP) -> Option<ThumbnailImage> {
    let (width, height, mut pixels) = bgra_from_hbitmap(bitmap)?;
    let (width, height) = (width as usize, height as usize);

    // 알파 채널을 **쓰지 않는** 비트맵인지 먼저 판정한다 — 전부 0이면 그렇다.
    // 픽셀마다 판정하면 진짜 투명한 부분(로고 주변 등)까지 불투명으로 메워
    // 검은 테두리가 생긴다. `ui::icon_tex`의 아이콘 변환과 같은 규칙이다
    let opaque_bitmap = pixels.chunks_exact(4).all(|px| px[3] == 0);
    for px in pixels.chunks_exact_mut(4) {
        let (b, g, r, a) = (px[0], px[1], px[2], px[3]);
        if opaque_bitmap {
            // 알파를 안 쓰는 비트맵 — 색만 옮기고 불투명으로 둔다
            px.copy_from_slice(&[r, g, b, 255]);
            continue;
        }
        if a == 0 {
            // 실제로 투명한 픽셀이다 — 색까지 지워야 가장자리에 잔상이 남지 않는다
            px.copy_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        // GDI는 프리멀티플라이 알파를 줄 수 있다 — 스트레이트 알파로 되돌린다
        let unmul = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
        px[0] = unmul(r);
        px[1] = unmul(g);
        px[2] = unmul(b);
        px[3] = a;
    }
    Some(ThumbnailImage {
        width,
        height,
        rgba: pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 도장을 가리지 않는 시험이 쓰는 한 가지 도장
    const S: FileStamp = FileStamp {
        size: 0,
        modified: 0,
    };

    fn image(size: usize) -> ThumbnailImage {
        ThumbnailImage {
            width: size,
            height: size,
            rgba: vec![0; size * size * 4],
        }
    }

    /// 워커를 띄우지 않고 캐시 자료구조만 검사한다 — 축출 규칙은 셸과 무관하다
    fn cache() -> ThumbnailCache {
        ThumbnailCache::new()
    }

    #[test]
    fn 상한을_넘으면_가장_오래된_것부터_버린다() {
        // 상한이 없으면 큰 폴더를 훑는 동안 메모리가 끝없이 는다 (NFR-9)
        let mut cache = cache();
        for index in 0..MAX_CACHED + 10 {
            cache.insert(PathBuf::from(format!("f{index}.jpg")), S, Some(image(1)));
        }
        assert_eq!(cache.len(), MAX_CACHED, "상한을 넘겨 들고 있다");
        // 처음 10개는 밀려났다
        assert!(cache.get(Path::new("f0.jpg")).is_none());
        assert!(cache.get(Path::new("f9.jpg")).is_none());
        assert!(cache.get(Path::new("f10.jpg")).is_some());
    }

    #[test]
    fn 최근에_쓴_것은_살아남는다() {
        // 화면에 보이는 썸네일이 먼저 버려지면 스크롤할 때마다 다시 만든다
        let mut cache = cache();
        for index in 0..MAX_CACHED {
            cache.insert(PathBuf::from(format!("f{index}.jpg")), S, Some(image(1)));
        }
        // 가장 오래된 것을 한 번 쓰면 최근으로 올라간다
        assert!(cache.get(Path::new("f0.jpg")).is_some());
        cache.insert(PathBuf::from("new.jpg"), S, Some(image(1)));
        assert!(
            cache.get(Path::new("f0.jpg")).is_some(),
            "방금 쓴 것이 버려졌다"
        );
        assert!(
            cache.get(Path::new("f1.jpg")).is_none(),
            "그다음이 밀려야 한다"
        );
    }

    #[test]
    fn 만들_수_없는_파일은_실패를_기억한다() {
        // 기억하지 않으면 스크롤할 때마다 같은 파일을 다시 요청한다
        let mut cache = cache();
        let path = PathBuf::from("문서.txt");
        cache.insert(path.clone(), S, None);
        assert!(
            cache.get(&path).is_none(),
            "만들 수 없는데 무언가를 돌려줬다"
        );
        assert_eq!(cache.len(), 1, "실패가 기억되지 않았다");
        // 이미 아는 파일은 다시 요청하지 않는다
        cache.request(&path, S);
        assert!(cache.pending.is_empty());
    }

    #[test]
    fn 같은_파일을_거듭_요청하지_않는다() {
        let mut cache = cache();
        let path = PathBuf::from("사진.jpg");
        cache.request(&path, S);
        cache.request(&path, S);
        cache.request(&path, S);
        assert_eq!(cache.pending.len(), 1, "같은 요청이 쌓였다");
    }

    #[test]
    fn 보이는_항목을_다시_요청하면_최근으로_올라간다() {
        // 그리기는 텍스처만 보고 픽셀 캐시를 건드리지 않는다 — 화면에 보이는 항목마다
        // 매 프레임 불리는 `request`가 유일한 LRU 갱신 지점이다.
        // 이것이 없으면 지금 보고 있는 썸네일이 축출돼 스크롤할 때마다 다시 만든다
        let mut cache = cache();
        for index in 0..MAX_CACHED {
            cache.insert(PathBuf::from(format!("f{index}.jpg")), S, Some(image(1)));
        }
        let oldest = PathBuf::from("f0.jpg");
        cache.request(&oldest, S); // 화면에 보여서 다시 요청됐다
        cache.insert(PathBuf::from("new.jpg"), S, Some(image(1)));
        assert!(
            cache.has_image(&oldest),
            "보이는 항목인데 축출됐다 — request가 LRU를 갱신하지 않는다"
        );
        assert!(
            !cache.has_image(Path::new("f1.jpg")),
            "그다음이 밀려야 한다"
        );
    }

    #[test]
    fn 준비된_경로만_동기화_대상이다() {
        // 실패로 기억한 것(None)은 올릴 그림이 없다 — 텍스처 캐시가 헛돌면 안 된다
        let mut cache = cache();
        cache.insert(PathBuf::from("사진.jpg"), S, Some(image(1)));
        cache.insert(PathBuf::from("문서.txt"), S, None);
        let paths = cache.ready_paths();
        assert_eq!(paths.len(), 1);
        assert!(paths[0].ends_with("사진.jpg"));
        assert!(cache.has_image(Path::new("사진.jpg")));
        assert!(!cache.has_image(Path::new("문서.txt")));
        assert!(cache.peek(Path::new("문서.txt")).is_none());
    }

    #[test]
    fn 들여다보기는_순서를_바꾸지_않는다() {
        // 동기화 중에 순서가 흔들리면 축출 대상이 프레임마다 달라진다
        let mut cache = cache();
        for index in 0..3 {
            cache.insert(PathBuf::from(format!("f{index}.jpg")), S, Some(image(1)));
        }
        let before = cache.order.clone();
        let _ = cache.peek(Path::new("f0.jpg"));
        let _ = cache.ready_paths();
        assert_eq!(cache.order, before, "들여다보기가 순서를 바꿨다");
    }

    #[test]
    fn 폴더를_떠난_뒤_도착한_결과는_버린다() {
        // 폴더를 빠르게 오가면 이전 폴더의 요청이 나중에 도착한다 — 담아 두면
        // 지금 폴더의 캐시 자리를 뺏는다 (`DirLoad`와 같은 세대 방식)
        let mut cache = cache();
        let old = PathBuf::from("이전폴더/사진.jpg");
        cache.request(&old, S);
        let stale_generation = cache.generation;
        cache.clear(); // 폴더 이동 — 세대가 오른다
        assert_ne!(cache.generation, stale_generation, "세대가 오르지 않았다");

        // 워커가 이전 세대로 보낸 결과가 뒤늦게 도착한 상황을 그대로 재현한다
        cache.accept(stale_generation, old.clone(), S, Some(image(1)));
        assert!(cache.is_empty(), "떠난 폴더의 결과가 담겼다");

        // 지금 세대의 결과는 정상으로 담긴다
        let now = PathBuf::from("현재폴더/사진.jpg");
        let generation = cache.generation;
        cache.accept(generation, now.clone(), S, Some(image(1)));
        assert!(cache.get(&now).is_some());
    }

    #[test]
    fn 폴더를_떠나면_비운다() {
        // 떠난 폴더의 썸네일을 들고 있으면 여러 폴더를 오갈 때 상한이 의미를 잃는다 (NFR-9)
        let mut cache = cache();
        for index in 0..20 {
            cache.insert(PathBuf::from(format!("f{index}.jpg")), S, Some(image(1)));
        }
        cache.request(Path::new("진행중.jpg"), S);
        cache.clear();
        assert!(cache.is_empty());
        assert!(cache.pending.is_empty(), "진행 중 표시가 남았다");
    }

    #[test]
    fn 같은_경로를_다시_넣어도_두_번_세지_않는다() {
        // 감시 갱신 등으로 같은 파일이 다시 오면 순서 목록에 중복이 쌓일 수 있다
        let mut cache = cache();
        let path = PathBuf::from("사진.jpg");
        cache.insert(path.clone(), S, Some(image(1)));
        cache.insert(path.clone(), S, Some(image(2)));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.order.len(), 1, "순서 목록에 중복이 쌓였다");
    }

    /// 워커를 거쳐 결과가 도착할 때까지 기다린다(최대 `timeout_ms`).
    ///
    /// **성공·실패를 가리지 않고 "결과가 왔는지"로 판정한다** — `poll`은 텍스처로 올릴
    /// 성공분만 돌려주므로, 그것만 보면 만들 수 없는 파일에서 영영 기다린다.
    /// 첫 호출은 COM 초기화까지 겹쳐 수 초가 걸리므로 여유를 넉넉히 둔다
    fn wait_for(cache: &mut ThumbnailCache, path: &Path, timeout_ms: u64) -> bool {
        let start = std::time::Instant::now();
        cache.request(path, S);
        while start.elapsed().as_millis() < timeout_ms as u128 {
            cache.poll();
            if cache.ready.contains_key(path) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    /// 도장을 정해 요청하고, 그 도장의 결과가 담길 때까지 기다린다
    fn wait_for_stamp(
        cache: &mut ThumbnailCache,
        path: &Path,
        stamp: FileStamp,
        timeout_ms: u64,
    ) -> bool {
        let start = std::time::Instant::now();
        // 내용이 바뀐 경로는 도장이 `SETTLE` 동안 그대로여야 다시 청한다 — 지금 한 번 보이고
        // `SETTLE` 뒤에 한 번 더 보인 것으로 흉내 낸다(처음 보는 경로는 첫 호출에서 청한다)
        cache.request_at(path, stamp, start);
        cache.request_at(path, stamp, start + SETTLE);
        while start.elapsed().as_millis() < timeout_ms as u128 {
            cache.poll();
            if cache.stamp_of(path) == Some(stamp) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    fn 도장(size: u64, modified: u64) -> FileStamp {
        FileStamp { size, modified }
    }

    #[test]
    fn 도장이_바뀌면_옛_그림을_둔_채_다시_만든다() {
        // 같은 이름의 파일이 새 내용으로 바뀌었다 — 경로만 보면 옛 그림을 계속 준다
        let mut cache = cache();
        let path = PathBuf::from("사진.png");
        cache.insert(path.clone(), 도장(0, 1), Some(image(1)));
        let t0 = Instant::now();
        cache.request_at(&path, 도장(100, 2), t0);
        cache.request_at(&path, 도장(100, 2), t0 + SETTLE);
        assert!(cache.is_pending(), "도장이 바뀌었는데 다시 청하지 않았다");
        assert_eq!(
            cache.peek(&path).map(|img| img.width),
            Some(1),
            "새 그림이 오기 전에 옛 그림을 치웠다 — 형식 아이콘으로 깜빡인다"
        );
        // 파일이 또 바뀌기 전에 옛 도장으로 늦게 온 결과는 버린다
        let generation = cache.generation;
        cache.accept(generation, path.clone(), 도장(0, 1), Some(image(3)));
        assert_eq!(cache.peek(&path).map(|img| img.width), Some(1));
        // 새 도장의 결과가 오면 갈아 끼운다
        cache.accept(generation, path.clone(), 도장(100, 2), Some(image(2)));
        assert_eq!(cache.peek(&path).map(|img| img.width), Some(2));
        assert_eq!(cache.stamp_of(&path), Some(도장(100, 2)));
        assert!(!cache.is_pending());
    }

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn 바뀐_내용은_도장이_2초_그대로여야_다시_청한다() {
        // 내려받는 중인 파일은 감시 갱신마다 도장이 바뀐다 — 그때마다 다시 만들면
        // 보이는 동안 셸이 파일을 계속 읽는다
        let mut cache = cache();
        let path = PathBuf::from("받는중.mp4");
        cache.insert(path.clone(), 도장(1, 1), Some(image(1)));
        let t0 = Instant::now();
        cache.request_at(&path, 도장(2, 2), t0);
        assert!(!cache.is_pending(), "바뀌자마자 다시 청했다");
        cache.request_at(&path, 도장(2, 2), t0 + ms(1_900));
        assert!(!cache.is_pending(), "2초가 되기 전에 청했다");
        assert_eq!(cache.settle_due(t0 + ms(500)), Some(ms(1_500)));
        cache.request_at(&path, 도장(2, 2), t0 + SETTLE);
        assert!(cache.is_pending(), "2초 동안 그대로인데 청하지 않았다");
        assert_eq!(
            cache.settle_due(t0 + SETTLE),
            None,
            "청한 뒤에도 기다림이 남았다"
        );
    }

    #[test]
    fn 도장이_또_바뀌면_기다림을_새로_시작한다() {
        let mut cache = cache();
        let path = PathBuf::from("받는중.mp4");
        cache.insert(path.clone(), 도장(1, 1), Some(image(1)));
        let t0 = Instant::now();
        cache.request_at(&path, 도장(2, 2), t0);
        cache.request_at(&path, 도장(3, 3), t0 + ms(1_000));
        cache.request_at(&path, 도장(3, 3), t0 + SETTLE);
        assert!(!cache.is_pending(), "새 도장의 2초가 차기 전에 청했다");
        cache.request_at(&path, 도장(3, 3), t0 + ms(3_000));
        assert!(cache.is_pending());
    }

    #[test]
    fn 처음_보는_경로는_곧바로_청한다() {
        let mut cache = cache();
        cache.request_at(Path::new("새것.jpg"), 도장(1, 1), Instant::now());
        assert!(cache.is_pending());
        assert_eq!(cache.settle_due(Instant::now()), None);
    }

    #[test]
    fn 만기가_지나고_다시_보이지_않은_기다림은_깨우지_않는다() {
        // 스크롤로 빠지거나 이름이 바뀐 경로의 기다림을 세면 `Some(ZERO)`가 이어져
        // 폴더를 떠날 때까지 매 프레임 다시 그린다
        let mut cache = cache();
        let path = PathBuf::from("사라짐.mp4");
        cache.insert(path.clone(), 도장(1, 1), Some(image(1)));
        let t0 = Instant::now();
        cache.request_at(&path, 도장(2, 2), t0);
        assert_eq!(cache.settle_due(t0 + ms(5_000)), None);
    }

    #[test]
    fn 쌓인_요청은_경로별_마지막_것만_남는다() {
        let make = |path: &str, modified| Request::Make {
            generation: 0,
            path: PathBuf::from(path),
            stamp: 도장(1, modified),
        };
        let kept = coalesce_requests(vec![
            make("a", 1),
            make("b", 1),
            make("a", 2),
            Request::Stop,
        ]);
        let shape: Vec<(String, u64)> = kept
            .iter()
            .map(|request| match request {
                Request::Make { path, stamp, .. } => {
                    (path.to_string_lossy().into_owned(), stamp.modified)
                }
                Request::Stop => ("STOP".into(), 0),
            })
            .collect();
        assert_eq!(
            shape,
            vec![("b".into(), 1), ("a".into(), 2), ("STOP".into(), 0)]
        );
    }

    #[test]
    fn 같은_도장이면_다시_청하지_않는다() {
        let mut cache = cache();
        let path = PathBuf::from("사진.png");
        cache.insert(path.clone(), 도장(5, 5), Some(image(1)));
        cache.request(&path, 도장(5, 5));
        assert!(!cache.is_pending());
    }

    #[test]
    fn 영바이트로_받은_아이콘은_내용이_쓰이면_실제_그림으로_바뀐다() {
        // 2026-09-29 재현 — 복사 시작 직후(0바이트) 셸은 썸네일 대신 형식 아이콘을 준다.
        // 경로만 키로 삼던 캐시는 내용이 다 쓰인 뒤에도 그 아이콘을 계속 줬다
        let dir = std::env::temp_dir().join(format!("fe_thumb_stamp_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("빨강.png");
        std::fs::write(&file, b"").unwrap();

        let mut cache = ThumbnailCache::new();
        assert!(wait_for_stamp(&mut cache, &file, 도장(0, 1), 20_000));

        image::RgbaImage::from_pixel(600, 400, image::Rgba([255, 0, 0, 255]))
            .save(&file)
            .unwrap();
        let arrived = wait_for_stamp(&mut cache, &file, 도장(1, 2), 20_000);
        let 가운데 = cache.peek(&file).map(|img| {
            let i = ((img.height / 2) * img.width + img.width / 2) * 4;
            [img.rgba[i], img.rgba[i + 1], img.rgba[i + 2]]
        });
        let _ = std::fs::remove_dir_all(&dir);
        assert!(arrived, "새 도장의 결과가 오지 않았다");
        assert_eq!(가운데, Some([255, 0, 0]), "여전히 옛 그림이다");
    }

    #[test]
    fn 실제_파일의_썸네일을_만든다() {
        // 셸 호출 경로가 살아 있는지 본다 — 자료구조 테스트만으로는 GetImage가
        // 실제로 그림을 주는지 알 수 없다. 아이콘이라도 오면 경로는 성립한 것이다
        let dir = std::env::temp_dir().join(format!("fe_thumb_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("샘플.txt");
        std::fs::write(&file, b"hello").unwrap();

        let mut cache = ThumbnailCache::new();
        let arrived = wait_for(&mut cache, &file, 20_000);
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            arrived,
            "20초 안에 결과가 오지 않았다 — 워커나 셸 호출이 막혔다"
        );
        let image = cache.get(&file).expect("결과는 왔는데 그림이 없다");
        assert!(image.width > 0 && image.height > 0);
        assert_eq!(
            image.rgba.len(),
            image.width * image.height * 4,
            "픽셀 수와 크기가 어긋난다"
        );
        // 전부 투명하면 화면에 아무것도 안 보인다 — 알파 처리가 잘못된 경우다
        assert!(
            image.rgba.chunks_exact(4).any(|px| px[3] > 0),
            "모든 픽셀이 투명하다"
        );
    }

    #[test]
    fn 캐시가_가득_차도_상한_안에_머문다() {
        // NFR-9의 실질 — 상한이 "장수"로만 걸려 있으면 한 장이 커질 때 메모리가 함께 는다.
        // 여기서는 **실제 담긴 바이트**로 잰다(장당 최대 256×256×4 = 256KB)
        const PER_IMAGE: usize = (THUMB_PX * THUMB_PX * 4) as usize;
        let mut cache = cache();
        for index in 0..MAX_CACHED + 50 {
            cache.insert(
                PathBuf::from(format!("f{index}.jpg")),
                S,
                Some(ThumbnailImage {
                    width: THUMB_PX as usize,
                    height: THUMB_PX as usize,
                    rgba: vec![0; PER_IMAGE],
                }),
            );
        }
        let bytes = cache.memory_bytes();
        assert_eq!(cache.len(), MAX_CACHED, "장수 상한이 깨졌다");
        assert_eq!(
            bytes,
            MAX_CACHED * PER_IMAGE,
            "가득 찬 캐시의 실제 크기가 예상과 다르다"
        );
        // 약 50MB — NFR-9가 정한 패널당 상한
        assert!(
            bytes <= 55 * 1024 * 1024,
            "가득 찬 캐시가 {}MB로 상한을 넘는다",
            bytes / (1024 * 1024)
        );
    }

    #[test]
    fn 실제_썸네일의_장당_크기를_잰다() {
        // 실측 — 이 값이 plan의 메모리 기록 근거다. 썸네일은 비율을 지켜 만들어지므로
        // 정사각형이 아닌 원본은 256×256보다 작게 나온다
        let dir = std::env::temp_dir().join(format!("fe_thumb_mem_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("샘플.txt");
        std::fs::write(&file, b"hello").unwrap();

        let mut cache = ThumbnailCache::new();
        let arrived = wait_for(&mut cache, &file, 20_000);
        let bytes = cache.memory_bytes();
        let size = cache.get(&file).map(|i| (i.width, i.height));
        let _ = std::fs::remove_dir_all(&dir);

        assert!(arrived, "결과가 오지 않았다");
        println!("MEASURED 장당 {bytes} bytes, 크기 {size:?}");
        assert!(bytes > 0);
        assert!(
            bytes <= (THUMB_PX * THUMB_PX * 4) as usize,
            "한 장이 256×256 RGBA보다 크다 — 상한 산정이 어긋난다"
        );
    }

    #[test]
    fn 썸네일_크기는_가장_큰_보기_모드에_맞춘다() {
        // 작게 만들어 두면 아주 큰 아이콘(256px)에서 늘려야 해 뭉개진다
        assert_eq!(THUMB_PX, 256);
    }
}
