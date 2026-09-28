//! 시스템 아이콘 → egui 텍스처 변환 (FR-5)
//!
//! `IconCache`는 **시스템 이미지 리스트의 인덱스**만 들고 있다(ListView 전용 설계).
//! egui는 이미지 리스트를 그릴 수 없으므로 인덱스를 HICON으로 꺼내 RGBA 픽셀로 바꾼 뒤
//! 텍스처로 올린다. 변환·해제에 필요한 unsafe는 전부 이 파일에 격리한다.
use crate::fs::bitmap::bgra_from_hbitmap;
use crate::fs::thumbnail::{ThumbnailCache, ThumbnailImage};
use eframe::egui;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};
use windows::Win32::Graphics::Gdi::{DeleteObject, HBITMAP};
use windows::Win32::UI::Controls::{HIMAGELIST, ILD_TRANSPARENT, ImageList_GetIcon};
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};

/// (이미지 리스트, 인덱스) → egui 텍스처 캐시.
/// 같은 확장자 행이 수천 개여도 텍스처는 아이콘 종류 수만큼만 만들어진다.
///
/// **키에 이미지 리스트를 포함한다** — 아이콘 인덱스는 크기와 무관하게 같은 체계라
/// 인덱스만 키로 쓰면 16px 텍스처가 256px 자리에 그대로 나온다(그 반대도 마찬가지).
/// 보기 모드마다 다른 크기를 쓰므로 이 구분이 없으면 모드를 바꿔도 아이콘 크기가 안 바뀐다
pub struct IconTextures {
    by_key: HashMap<(isize, i32), Option<egui::TextureHandle>>,
    /// 이번 프레임에 만든 수 — 한 프레임에 몰리면 렌더가 수 초 멈춘다(PoC 실측 3096ms)
    created_this_frame: usize,
    /// 이번 프레임에 **다시** 시도한 수 — 처음 보는 아이콘의 몫과 따로 센다
    retried_this_frame: usize,
    /// 이번 프레임에 변환을 시도한 수(성공·실패·재시도 모두) — 첫 시도는 시간 예산과 무관하다
    attempted_this_frame: usize,
    /// 이번 프레임의 변환에 쓴 시간 — `MAX_NEW_TEXTURE_TIME`과 견준다
    spent_this_frame: Duration,
    /// 실패한 인덱스별 재시도 횟수. 성공하면 지운다 — 늘 실패하는 자리를 키당 상한으로 끊는다
    retries: HashMap<(isize, i32), u8>,
    /// 변환을 어디서 하는가 — 앱은 워커, 다른 모듈의 시험 하네스는 그 자리
    converter: Converter,
    /// 워커에 맡기고 아직 텍스처로 올리지 않은 키 — 같은 키를 두 번 맡기지 않는다
    pending: HashSet<(isize, i32)>,
    /// 워커가 돌려줬으나 이번 프레임 예산에 밀려 아직 올리지 못한 결과
    ready: VecDeque<((isize, i32), Option<egui::ColorImage>)>,
    /// 워커에 맡긴 횟수 — 대기 중 재요청이 없는지 시험이 본다
    #[cfg(test)]
    requests_sent: usize,
}

/// 아이콘 변환(HICON → RGBA)을 어디서 하는가.
///
/// **UI 스레드에서 하지 않는 것이 기본이다** — 새 아이콘 변환 한 번이 20~115ms 걸렸다
/// (2026-09-28 실측, 첫 호출·처음 보는 exe 아이콘). 워커는 처음 맡길 때 띄우고, 띄우지 못했거나
/// 사라지면 그 자리 변환으로 돌아간다 — 아이콘이 영영 비는 것보다 느린 편이 낫다
enum Converter {
    /// 그 자리에서 변환한다 — 시험 하네스와 워커를 쓸 수 없을 때
    Inline,
    NotStarted,
    Running(ConvertChannels),
}

/// 변환 워커와 잇는 통로 — `HIMAGELIST`는 `isize`로 넘긴다(시스템 이미지 리스트는 프로세스 전역이다)
struct ConvertChannels {
    requests: Sender<(isize, i32)>,
    results: Receiver<((isize, i32), Option<egui::ColorImage>)>,
}

/// 아이콘·썸네일 텍스처 옵션 — **밉맵을 함께 만든다**.
///
/// 이 그림들은 늘 줄여 그린다(예: 256px 점보 아이콘 → 큰 아이콘 보기의 96px).
/// 밉맵이 없으면 축소가 원본 텍셀 넷만 섞어 계단·모아레가 남는다(화면에서 "자글자글"하게
/// 보이던 원인). 밉맵을 만들어 두면 셸이 그러듯 여러 단계를 거쳐 부드럽게 줄어든다.
/// 실제로 만드는 것은 glow 백엔드다(`egui_glow`의 `generate_mipmap`) — 이 앱의 백엔드가 그것이다
const IMAGE_TEXTURE: egui::TextureOptions = egui::TextureOptions {
    magnification: egui::TextureFilter::Linear,
    minification: egui::TextureFilter::Linear,
    wrap_mode: egui::TextureWrapMode::ClampToEdge,
    mipmap_mode: Some(egui::TextureFilter::Linear),
};

/// 한 프레임에 새로 올릴 썸네일 텍스처 수 상한.
/// 256×256짜리라 아이콘보다 무겁다 — 한꺼번에 올리면 스크롤이 끊긴다
const MAX_NEW_THUMBS_PER_FRAME: usize = 4;

/// 한 프레임에 새로 만들 텍스처 수 상한.
/// 실측에서 텍스처 다수가 한 프레임에 생성되며 3초급 스파이크가 났다 —
/// 넘치는 것은 다음 프레임으로 미루고 그 프레임에는 아이콘 없이 그린다(몇 프레임 안에 채워진다).
/// 개수만으로는 변환 한 번이 비싼 아이콘에서 부족해 시간 예산(`MAX_NEW_TEXTURE_TIME`)이 함께 건다
const MAX_NEW_TEXTURES_PER_FRAME: usize = 8;

/// 한 프레임에 새 아이콘 텍스처에 쓸 시간 — 넘으면 나머지는 다음 프레임으로 미룬다.
/// 처음 보는 exe 아이콘은 변환 한 번이 4~12ms라(2026-09-28 실측) 개수 상한 8만으로는
/// 한 프레임이 35~100ms 멈췄다. 변환이 워커로 간 뒤로는 주로 올리기(`load_texture`)와
/// 워커를 쓸 수 없을 때의 그 자리 변환을 잰다. **그 프레임의 첫 시도는 이 예산과 무관하다** —
/// 한 번이 예산보다 오래 걸려도 매 프레임 하나씩은 나아가야 아이콘이 끝내 채워진다
const MAX_NEW_TEXTURE_TIME: Duration = Duration::from_millis(6);

/// 한 프레임에 **다시** 시도할 실패 인덱스 수 상한.
/// 처음 보는 아이콘의 몫(`MAX_NEW_TEXTURES_PER_FRAME`)과 예산을 나눈다 — 같은 예산을 쓰면
/// 요청 순서가 프레임마다 같아 앞선 실패가 매번 그것을 소진하고, 뒤쪽 아이콘은 영영 못 올라온다
const MAX_FAILED_RETRIES_PER_FRAME: usize = 2;

/// 한 인덱스를 다시 시도할 횟수 상한.
/// 1bpp 마스크만 있는 흑백 아이콘처럼 **늘** 실패하는 자리가 재시도 예산을 영구 점유하고
/// 매 프레임 GDI 호출을 남기는 것을 막는다 — 이 횟수를 넘기면 포기한다
const MAX_RETRIES_PER_KEY: u8 = 3;

impl Default for IconTextures {
    fn default() -> IconTextures {
        IconTextures::new()
    }
}

impl IconTextures {
    pub fn new() -> IconTextures {
        IconTextures {
            by_key: HashMap::new(),
            created_this_frame: 0,
            retried_this_frame: 0,
            attempted_this_frame: 0,
            spent_this_frame: Duration::ZERO,
            retries: HashMap::new(),
            converter: Converter::NotStarted,
            pending: HashSet::new(),
            ready: VecDeque::new(),
            #[cfg(test)]
            requests_sent: 0,
        }
    }

    /// 시험 하네스용 — 변환을 그 자리에서 한다. 트리·목록 그리기를 재는 시험이 프레임 반복을
    /// 스레드 타이밍에 기대지 않게 한다
    #[cfg(test)]
    pub fn inline() -> IconTextures {
        IconTextures {
            converter: Converter::Inline,
            ..IconTextures::new()
        }
    }

    /// 워커가 변환 중인 아이콘이 남았는가 — 앱이 다시 그릴지 정하는 데 쓴다
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// 워커에 맡긴 변환 수 — 대기 중 같은 키를 다시 맡기지 않는지 시험이 본다
    #[cfg(test)]
    fn requests_sent(&self) -> usize {
        self.requests_sent
    }

    /// 프레임 시작 시 호출 — 프레임당 생성·재시도 상한을 초기화한다.
    /// `retries`는 **키의 이력**이라 프레임 경계로 지우지 않는다
    pub fn begin_frame(&mut self) {
        self.created_this_frame = 0;
        self.retried_this_frame = 0;
        self.attempted_this_frame = 0;
        self.spent_this_frame = Duration::ZERO;
    }

    /// 인덱스에 해당하는 텍스처.
    ///
    /// **한 번 실패한 인덱스도 다시 시도한다** — 셸 경합 같은 일시적 실패가 영구가 되지
    /// 않게 하기 위함이다. 재시도는 둘로 제한한다: 프레임당 `MAX_FAILED_RETRIES_PER_FRAME`회
    /// (처음 보는 아이콘의 몫을 잠식하지 않게) · 키당 `MAX_RETRIES_PER_KEY`회(늘 실패하는
    /// 자리가 그 예산을 영구 점유하지 않게). 그래서 실패 키가 여럿이면 한 프레임에는 앞쪽
    /// 몇 개만 차례가 오지만, 앞쪽이 키 상한으로 비켜 주므로 뒤쪽도 몇 프레임 뒤 차례를 받는다
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        himl: HIMAGELIST,
        index: i32,
    ) -> Option<&egui::TextureHandle> {
        let key = (himl.0, index);
        self.collect_results();
        self.upload_ready(ctx);
        // 워커가 변환 중인 키는 처음 보는 것도 실패한 것도 아니다 — 다시 맡기지 않는다
        let waiting = self.pending.contains(&key);
        let entry = self.by_key.get(&key);
        let unseen = entry.is_none() && !waiting;
        let known_failure = matches!(entry, Some(None)) && !waiting;

        if unseen {
            // 상한(개수·시간)을 넘으면 이번 프레임에는 만들지 않는다 — 캐시에 넣지도 않으므로
            // 다음 프레임에 재시도된다. 그 프레임의 첫 시도는 시간 예산과 무관하다
            let within_time =
                self.attempted_this_frame == 0 || self.spent_this_frame < MAX_NEW_TEXTURE_TIME;
            if self.created_this_frame < MAX_NEW_TEXTURES_PER_FRAME && within_time {
                self.convert_into(ctx, himl, index, key);
            }
        } else if known_failure {
            let tried = self.retries.get(&key).copied().unwrap_or(0);
            if tried < MAX_RETRIES_PER_KEY && self.retried_this_frame < MAX_FAILED_RETRIES_PER_FRAME
            {
                self.retried_this_frame += 1;
                self.retries.insert(key, tried + 1);
                self.convert_into(ctx, himl, index, key);
            }
        }
        self.by_key.get(&key).and_then(|h| h.as_ref())
    }

    /// 변환을 맡기거나(워커) 그 자리에서 한다.
    ///
    /// 음수 인덱스는 셸에 닿기 전에 실패하므로 워커를 거치지 않는다. 워커를 띄우지 못하면
    /// 그 자리 변환으로 돌아간다
    fn convert_into(
        &mut self,
        ctx: &egui::Context,
        himl: HIMAGELIST,
        index: i32,
        key: (isize, i32),
    ) {
        if index >= 0 && self.dispatch(key) {
            return;
        }
        let started = Instant::now();
        let image = icon_to_image(himl, index);
        self.store(ctx, key, image, started);
    }

    /// 워커에 맡긴다 — 맡겼으면 참, 워커를 쓸 수 없으면 거짓
    fn dispatch(&mut self, key: (isize, i32)) -> bool {
        if matches!(self.converter, Converter::NotStarted) {
            self.converter = spawn_converter().map_or(Converter::Inline, Converter::Running);
        }
        let Converter::Running(channels) = &self.converter else {
            return false;
        };
        if channels.requests.send(key).is_err() {
            self.converter = Converter::Inline;
            return false;
        }
        #[cfg(test)]
        {
            self.requests_sent += 1;
        }
        self.pending.insert(key);
        true
    }

    /// 워커가 돌려준 결과를 받아 둔다 — 올리는 것은 `upload_ready`가 예산 안에서 한다.
    ///
    /// 워커가 사라졌으면(채널 끊김) 받지 못한 키의 대기를 풀고 그 자리 변환으로 돌아간다 —
    /// 남겨 두면 그 아이콘은 다시 맡겨지지도 않고 앱은 영영 다시 그리기를 청한다
    fn collect_results(&mut self) {
        let Converter::Running(channels) = &self.converter else {
            return;
        };
        let mut lost = false;
        loop {
            match channels.results.try_recv() {
                Ok(result) => self.ready.push_back(result),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    lost = true;
                    break;
                }
            }
        }
        if lost {
            self.converter = Converter::Inline;
            let ready = &self.ready;
            self.pending
                .retain(|key| ready.iter().any(|(done, _)| done == key));
        }
    }

    /// 받아 둔 결과를 텍스처로 올린다 — 처음 보는 아이콘과 같은 개수·시간 예산 안에서
    fn upload_ready(&mut self, ctx: &egui::Context) {
        while !self.ready.is_empty()
            && self.created_this_frame < MAX_NEW_TEXTURES_PER_FRAME
            && (self.attempted_this_frame == 0 || self.spent_this_frame < MAX_NEW_TEXTURE_TIME)
        {
            let Some((key, image)) = self.ready.pop_front() else {
                break;
            };
            self.pending.remove(&key);
            self.store(ctx, key, image, Instant::now());
        }
    }

    /// 변환 결과를 캐시에 담는다 — 실패도 `None`으로 담아 다음 프레임이 재시도 대상으로 알아본다.
    /// 성공하면 텍스처를 올리고 `created_this_frame`을 올리며 재시도 이력을 지운다.
    /// 성공·실패와 무관하게 시도 수와 `started`부터 걸린 시간을 이번 프레임 몫에 더한다 —
    /// 시험 빌드의 그 자리 변환에서는 `icon_to_image`의 셸 잠금 대기도 여기 섞이지만, 첫 시도는
    /// 예산과 무관해 진행은 멈추지 않는다
    fn store(
        &mut self,
        ctx: &egui::Context,
        key: (isize, i32),
        image: Option<egui::ColorImage>,
        started: Instant,
    ) {
        let handle = image.map(|img| {
            self.created_this_frame += 1;
            ctx.load_texture(format!("icon{}_{}", key.0, key.1), img, IMAGE_TEXTURE)
        });
        self.attempted_this_frame += 1;
        self.spent_this_frame += started.elapsed();
        if handle.is_some() {
            self.retries.remove(&key);
        }
        self.by_key.insert(key, handle);
    }
}

/// 아이콘 변환 워커를 띄운다 — 스레드를 만들지 못하면 `None`(부르는 쪽이 그 자리 변환으로 간다).
///
/// 요청 통로가 닫히면(캐시가 버려지면) 끝난다. 셸 잠금은 `icon_to_image`가 안에서 잡으므로
/// 여기서 다시 잡지 않는다(재진입 데드락 — `fs::icons::SHELL_LOCK` 주석)
fn spawn_converter() -> Option<ConvertChannels> {
    let (request_tx, request_rx) = channel::<(isize, i32)>();
    let (result_tx, result_rx) = channel();
    std::thread::Builder::new()
        .name("icon-tex".into())
        .spawn(move || {
            for (himl, index) in request_rx {
                let image = icon_to_image(HIMAGELIST(himl), index);
                if result_tx.send(((himl, index), image)).is_err() {
                    break;
                }
            }
        })
        .ok()?;
    Some(ConvertChannels {
        requests: request_tx,
        results: result_rx,
    })
}

/// 경로별 썸네일 텍스처 (FR-24).
///
/// 픽셀은 `fs::thumbnail`이 만들고 여기서는 **텍스처로 올리기만** 한다.
/// 매 프레임 `sync`가 픽셀 캐시와 항목 집합을 맞춘다 — 픽셀이 LRU로 축출되면 그 텍스처도
/// 함께 버려야 NFR-9 상한이 GPU 쪽에서도 지켜지고, 프레임 상한에 걸려 못 올린 것도
/// 다음 프레임에 실제로 다시 시도된다
pub struct ThumbnailTextures {
    by_path: HashMap<PathBuf, egui::TextureHandle>,
    created_this_frame: usize,
}

impl Default for ThumbnailTextures {
    fn default() -> ThumbnailTextures {
        ThumbnailTextures::new()
    }
}

impl ThumbnailTextures {
    pub fn new() -> ThumbnailTextures {
        ThumbnailTextures {
            by_path: HashMap::new(),
            created_this_frame: 0,
        }
    }

    /// 픽셀 캐시와 항목 집합을 맞춘다 — 프레임마다 한 번 부른다.
    ///
    /// ① 픽셀이 사라진(축출된) 텍스처를 버리고 ② 아직 안 올라간 것을 상한까지 올린다.
    /// **`poll`이 돌려준 "방금 도착한 것"만 보면 안 된다** — 상한에 걸려 건너뛴 경로는
    /// 그 목록에 다시 나오지 않아 영영 형식 아이콘으로 남는다
    pub fn sync(&mut self, ctx: &egui::Context, cache: &ThumbnailCache) {
        self.created_this_frame = 0;
        // 픽셀이 축출된 텍스처는 GPU에서도 놓는다 (NFR-9)
        self.by_path.retain(|path, _| cache.has_image(path));
        for path in cache.ready_paths() {
            if self.created_this_frame >= MAX_NEW_THUMBS_PER_FRAME {
                break;
            }
            if let Some(image) = cache.peek(&path) {
                self.upload(ctx, &path, image);
            }
        }
    }

    /// 준비된 썸네일을 텍스처로 올린다. 이미 있으면 아무 일도 하지 않는다
    fn upload(&mut self, ctx: &egui::Context, path: &Path, image: &ThumbnailImage) {
        if self.by_path.contains_key(path) {
            return;
        }
        let color =
            egui::ColorImage::from_rgba_unmultiplied([image.width, image.height], &image.rgba);
        let handle = ctx.load_texture(
            format!("thumb{}", path.to_string_lossy()),
            color,
            IMAGE_TEXTURE,
        );
        self.created_this_frame += 1;
        self.by_path.insert(path.to_path_buf(), handle);
    }

    pub fn get(&self, path: &Path) -> Option<&egui::TextureHandle> {
        self.by_path.get(path)
    }

    /// 폴더를 떠날 때 — 픽셀 캐시와 함께 비운다 (NFR-9)
    pub fn clear(&mut self) {
        self.by_path.clear();
    }

    /// 올라간 텍스처 수 — 상한 검증용
    pub fn len(&self) -> usize {
        self.by_path.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }
}

/// 이미지 리스트의 아이콘 하나를 RGBA 이미지로 변환한다.
/// 실패(잘못된 인덱스·비트맵 조회 실패 등)하면 None — 호출부는 아이콘 없이 그린다.
fn icon_to_image(himl: HIMAGELIST, index: i32) -> Option<egui::ColorImage> {
    if index < 0 {
        return None;
    }
    // `ImageList_GetIcon`도 프로세스 전역 셸 상태를 만진다 — 조기 반환 뒤에 잡는다
    let _guard = crate::fs::icons::shell_guard();
    // 안전성: 이미지 리스트에서 아이콘 사본을 얻고, 이 함수를 벗어나기 전에 반드시 해제한다.
    // 중간 실패 경로에서도 DestroyIcon이 호출되도록 결과를 받아 마지막에 정리한다
    unsafe {
        let hicon = ImageList_GetIcon(himl, index, ILD_TRANSPARENT);
        if hicon.is_invalid() {
            return None;
        }
        let image = hicon_to_image(hicon);
        let _ = DestroyIcon(hicon);
        image
    }
}

/// HICON → RGBA. 32bpp 컬러 비트맵을 top-down으로 읽어 BGRA를 RGBA로 바꾼다.
///
/// 안전성 주의: 유효한 HICON에만 호출한다. 내부에서 얻은 GDI 개체는 모두 이 함수에서 해제한다
unsafe fn hicon_to_image(hicon: HICON) -> Option<egui::ColorImage> {
    unsafe {
        let mut info = ICONINFO::default();
        GetIconInfo(hicon, &mut info).ok()?;
        // 마스크·컬러 비트맵은 GetIconInfo가 사본을 주므로 호출부가 해제해야 한다
        let result = color_bitmap_to_image(info.hbmColor);
        if !info.hbmColor.is_invalid() {
            let _ = DeleteObject(info.hbmColor.into());
        }
        if !info.hbmMask.is_invalid() {
            let _ = DeleteObject(info.hbmMask.into());
        }
        result
    }
}

/// 셸 아이콘의 컬러 비트맵을 egui 이미지로 바꾼다.
///
/// GDI로 픽셀을 읽는 것은 `fs::bitmap`이 하고, 뒤집기·알파 되돌리기는 아래
/// [`bgra_to_color_image`]가 한다
fn color_bitmap_to_image(hbm: HBITMAP) -> Option<egui::ColorImage> {
    let (width, height, pixels) = bgra_from_hbitmap(hbm)?;
    Some(bgra_to_color_image(width as usize, height as usize, pixels))
}

/// GDI가 준 BGRA를 egui 이미지로 바꾼다 — **뒤집기와 알파 되돌리기**.
///
/// **셸 아이콘과 컨텍스트 메뉴 아이콘이 함께 쓴다**(`ui::shell_context_menu`) — 둘 다 셸이
/// 준 32bpp 비트맵이라 후처리가 똑같고, 각자 하면 한쪽이 알파를 잘못 다뤄 가장자리가 달라진다.
/// 이것을 `fs::bitmap`에 두지 않는 이유는 egui 타입을 돌려주기 때문이다(`fs`는 `ui`를 모른다).
///
/// **알파를 안 쓰는 비트맵은 통째로 불투명으로 되살린다** — 픽셀마다 판정하면 진짜 투명한
/// 자리까지 메워 검은 테두리가 생긴다(`fs::thumbnail`의 같은 규칙)
pub(crate) fn bgra_to_color_image(
    width: usize,
    height: usize,
    mut pixels: Vec<u8>,
) -> egui::ColorImage {
    // 알파를 쓰지 않는 비트맵인지 **바꾸기 전에** 판정한다 — 되돌린 뒤에는 알 수 없다
    let opaque_bitmap = pixels.chunks_exact(4).all(|px| px[3] == 0);
    for px in pixels.chunks_exact_mut(4) {
        let (b, g, r, a) = (px[0], px[1], px[2], px[3]);
        if opaque_bitmap {
            // 알파를 안 쓰는 비트맵 — 색만 옮기고 불투명으로 둔다
            px.copy_from_slice(&[r, g, b, 255]);
            continue;
        }
        if a == 0 {
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
    egui::ColorImage::from_rgba_unmultiplied([width, height], &pixels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::thumbnail::ThumbnailImage;

    /// 반드시 실패하는 인덱스 — 음수는 `icon_to_image`가 첫 줄에서 걸러낸다
    fn 실패_인덱스(n: i32) -> i32 {
        -1 - n
    }

    /// 실패 키 `count`개를 캐시에 담은 상태를 만든다 (다음 프레임의 재시도 대상이 된다)
    fn 실패_키를_담는다(
        textures: &mut IconTextures,
        ctx: &egui::Context,
        himl: HIMAGELIST,
        count: i32,
    ) {
        textures.begin_frame();
        for n in 0..count {
            textures.get(ctx, himl, 실패_인덱스(n));
        }
    }

    #[test]
    fn 한_번_실패한_아이콘도_다음_프레임에_다시_시도한다() {
        let ctx = egui::Context::default();
        // 음수 인덱스는 이미지 리스트에 닿기 전에 걸러지므로 핸들이 유효하지 않아도 안전하다
        let himl = HIMAGELIST(1);
        let mut textures = IconTextures::new();

        textures.begin_frame();
        assert!(textures.get(&ctx, himl, 실패_인덱스(0)).is_none());
        assert_eq!(
            textures.retried_this_frame, 0,
            "첫 프레임은 처음 보는 키라 재시도가 아니다"
        );

        textures.begin_frame();
        assert!(textures.get(&ctx, himl, 실패_인덱스(0)).is_none());
        assert_eq!(
            textures.retried_this_frame, 1,
            "실패로 아는 키는 다음 프레임에 다시 시도된다 (종전에는 영영 시도하지 않았다)"
        );
    }

    #[test]
    fn 성공한_아이콘은_다시_변환하지_않는다() {
        let ctx = egui::Context::default();
        let icons = crate::fs::icons::IconCache::new();
        let (himl, index) = (icons.himl(), icons.dir_icon());

        let mut probe = IconTextures::inline();
        probe.begin_frame();
        if probe.get(&ctx, himl, index).is_none() {
            // 셸에서 이미지 리스트를 얻지 못하는 환경 — 이 시험은 성립하지 않는다.
            // 조용히 지나가면 "검증했다"와 구분되지 않으므로 건너뛴 사실을 남긴다
            eprintln!("[skip] 성공한_아이콘은_다시_변환하지_않는다 — 이미지 리스트 미가용");
            return;
        }

        let mut textures = IconTextures::inline();
        textures.begin_frame();
        assert!(textures.get(&ctx, himl, index).is_some());
        assert_eq!(textures.created_this_frame, 1);

        textures.begin_frame();
        assert!(textures.get(&ctx, himl, index).is_some());
        assert_eq!(
            textures.created_this_frame, 0,
            "캐시 히트는 변환하지 않는다"
        );
    }

    #[test]
    fn 재시도는_프레임당_두_번까지만_한다() {
        let ctx = egui::Context::default();
        let himl = HIMAGELIST(1);
        let mut textures = IconTextures::new();

        실패_키를_담는다(&mut textures, &ctx, himl, 8);

        textures.begin_frame();
        for n in 0..8 {
            textures.get(&ctx, himl, 실패_인덱스(n));
        }
        assert_eq!(
            textures.retried_this_frame, MAX_FAILED_RETRIES_PER_FRAME,
            "재시도가 프레임 예산을 넘지 않는다"
        );
    }

    #[test]
    fn 재시도가_처음_보는_아이콘을_굶기지_않는다() {
        let ctx = egui::Context::default();
        let icons = crate::fs::icons::IconCache::new();
        let (himl, index) = (icons.himl(), icons.dir_icon());

        let mut probe = IconTextures::inline();
        probe.begin_frame();
        if probe.get(&ctx, himl, index).is_none() {
            // 셸 미가용 환경 — 성공 변환을 전제로 하는 시험이라 성립하지 않는다
            eprintln!("[skip] 재시도가_처음_보는_아이콘을_굶기지_않는다 — 이미지 리스트 미가용");
            return;
        }

        let mut textures = IconTextures::inline();
        실패_키를_담는다(&mut textures, &ctx, himl, 8);

        textures.begin_frame();
        for n in 0..8 {
            textures.get(&ctx, himl, 실패_인덱스(n)); // 실패 재시도가 **먼저** 온다
        }
        assert!(
            textures.get(&ctx, himl, index).is_some(),
            "재시도는 별도 예산을 쓰므로 처음 보는 아이콘이 같은 프레임에 올라간다"
        );
    }

    #[test]
    fn 늘_실패하는_아이콘은_세_번_뒤_재시도를_멈춘다() {
        let ctx = egui::Context::default();
        let himl = HIMAGELIST(1);
        let mut textures = IconTextures::new();
        let index = 실패_인덱스(0);

        textures.begin_frame();
        textures.get(&ctx, himl, index); // 첫 시도 — 재시도가 아니다

        let mut 재시도 = Vec::new();
        for _ in 0..5 {
            textures.begin_frame();
            textures.get(&ctx, himl, index);
            재시도.push(textures.retried_this_frame);
        }
        assert_eq!(
            재시도,
            vec![1, 1, 1, 0, 0],
            "키당 세 번까지만 다시 시도하고 그 뒤로는 예산도 GDI 호출도 쓰지 않는다"
        );
    }

    #[test]
    fn 처음_보는_아이콘은_워커가_변환하고_그_프레임에는_기다리지_않는다() {
        // 새 아이콘 변환 한 번이 UI 스레드에서 20~115ms 걸렸다(2026-09-28 실측) —
        // 그 자리에서 기다리지 않고 맡긴 뒤, 결과가 오면 다음 프레임에 올린다
        let ctx = egui::Context::default();
        let icons = crate::fs::icons::IconCache::new();
        let (himl, index) = (icons.himl(), icons.dir_icon());
        let mut textures = IconTextures::new();

        textures.begin_frame();
        assert!(
            textures.get(&ctx, himl, index).is_none(),
            "첫 요청에서 곧바로 변환했다 — UI 스레드가 기다린다"
        );
        assert!(textures.has_pending(), "맡긴 변환이 대기로 잡히지 않았다");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ready = false;
        while !ready && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
            textures.begin_frame();
            ready = textures.get(&ctx, himl, index).is_some();
        }
        if !ready {
            // 셸에서 이미지 리스트를 얻지 못하는 환경이면 변환 자체가 실패한다
            let mut probe = IconTextures::inline();
            probe.begin_frame();
            if probe.get(&ctx, himl, index).is_none() {
                eprintln!("[skip] 처음_보는_아이콘은_워커가_변환하고… — 이미지 리스트 미가용");
                return;
            }
        }
        assert!(ready, "5초 안에 워커 변환 결과가 올라오지 않았다");
        assert!(!textures.has_pending(), "올린 뒤에도 대기가 남았다");
    }

    #[test]
    fn 변환_중인_아이콘은_다시_맡기지_않는다() {
        let ctx = egui::Context::default();
        let icons = crate::fs::icons::IconCache::new();
        let (himl, index) = (icons.himl(), icons.dir_icon());
        let mut textures = IconTextures::new();

        textures.begin_frame();
        textures.get(&ctx, himl, index);
        textures.get(&ctx, himl, index);

        assert_eq!(textures.requests_sent(), 1, "같은 아이콘을 두 번 맡겼다");
    }

    /// 이번 프레임에 이미 시도가 있었고 시간 예산을 다 쓴 상태로 만든다
    fn 예산을_다_쓴다(textures: &mut IconTextures) {
        textures.attempted_this_frame = 1;
        textures.spent_this_frame = MAX_NEW_TEXTURE_TIME;
    }

    #[test]
    fn 시간_예산을_넘긴_프레임에는_처음_보는_아이콘을_변환하지_않는다() {
        let ctx = egui::Context::default();
        let himl = HIMAGELIST(1);
        let mut textures = IconTextures::new();
        textures.begin_frame();
        예산을_다_쓴다(&mut textures);

        textures.get(&ctx, himl, 실패_인덱스(0));

        assert!(
            !textures.by_key.contains_key(&(himl.0, 실패_인덱스(0))),
            "예산을 넘겼는데 변환을 시도했다 — 한 프레임에 변환이 몰린다"
        );
    }

    #[test]
    fn 시도가_없던_프레임의_첫_변환은_예산과_무관하게_한다() {
        // 변환 한 번이 예산보다 오래 걸려도 매 프레임 하나씩은 나아가야 한다
        let ctx = egui::Context::default();
        let himl = HIMAGELIST(1);
        let mut textures = IconTextures::new();
        textures.begin_frame();
        textures.spent_this_frame = MAX_NEW_TEXTURE_TIME;

        textures.get(&ctx, himl, 실패_인덱스(0));

        assert!(
            textures.by_key.contains_key(&(himl.0, 실패_인덱스(0))),
            "그 프레임의 첫 시도까지 막았다 — 아이콘이 영영 채워지지 않는다"
        );
    }

    #[test]
    fn 예산에_밀린_아이콘은_다음_프레임에_변환된다() {
        let ctx = egui::Context::default();
        let himl = HIMAGELIST(1);
        let mut textures = IconTextures::new();
        textures.begin_frame();
        예산을_다_쓴다(&mut textures);
        textures.get(&ctx, himl, 실패_인덱스(0));
        assert!(!textures.by_key.contains_key(&(himl.0, 실패_인덱스(0))));

        textures.begin_frame();
        textures.get(&ctx, himl, 실패_인덱스(0));

        assert!(
            textures.by_key.contains_key(&(himl.0, 실패_인덱스(0))),
            "프레임이 바뀌어도 예산이 풀리지 않았다"
        );
    }

    fn image() -> ThumbnailImage {
        ThumbnailImage {
            width: 2,
            height: 2,
            rgba: vec![255; 2 * 2 * 4],
        }
    }

    /// 픽셀 캐시에 `count`개를 채운 상태를 만든다 (워커를 거치지 않는다)
    fn filled_cache(count: usize) -> ThumbnailCache {
        let mut cache = ThumbnailCache::new();
        for index in 0..count {
            cache.accept_for_test(PathBuf::from(format!("f{index}.jpg")), Some(image()));
        }
        cache
    }

    #[test]
    fn 상한을_넘긴_썸네일도_다음_프레임에_올라간다() {
        // 프레임 상한(4)에 걸린 경로가 영영 안 올라가면 폴더 진입 직후 일부가
        // 형식 아이콘인 채로 남는다 — 이 회귀를 막는다
        let ctx = egui::Context::default();
        let cache = filled_cache(10);
        let mut textures = ThumbnailTextures::new();

        textures.sync(&ctx, &cache);
        assert_eq!(
            textures.len(),
            MAX_NEW_THUMBS_PER_FRAME,
            "첫 프레임에 상한만큼만 올라가야 한다"
        );
        textures.sync(&ctx, &cache);
        assert_eq!(
            textures.len(),
            MAX_NEW_THUMBS_PER_FRAME * 2,
            "다음 프레임에 이어 올라가지 않았다"
        );
        // 몇 프레임 더 돌면 전부 올라간다
        for _ in 0..3 {
            textures.sync(&ctx, &cache);
        }
        assert_eq!(textures.len(), 10, "끝내 전부 올라가지 않았다");
    }

    #[test]
    fn 픽셀이_축출되면_텍스처도_버린다() {
        // 픽셀은 LRU로 줄어드는데 텍스처만 남으면 같은 폴더에서 스크롤만 해도
        // GPU 메모리가 무제한 는다 (NFR-9가 텍스처 쪽에서 깨진다)
        let ctx = egui::Context::default();
        let mut cache = filled_cache(4);
        let mut textures = ThumbnailTextures::new();
        textures.sync(&ctx, &cache);
        assert_eq!(textures.len(), 4);

        cache.clear(); // 폴더 이동 — 픽셀이 통째로 사라진다
        textures.sync(&ctx, &cache);
        assert!(textures.is_empty(), "픽셀이 사라졌는데 텍스처가 남았다");
    }

    #[test]
    fn 같은_썸네일을_두_번_올리지_않는다() {
        let ctx = egui::Context::default();
        let cache = filled_cache(2);
        let mut textures = ThumbnailTextures::new();
        textures.sync(&ctx, &cache);
        textures.sync(&ctx, &cache);
        textures.sync(&ctx, &cache);
        assert_eq!(textures.len(), 2, "중복으로 올라갔다");
    }
}
