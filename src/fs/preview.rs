//! 미리보기 창(FR-76)이 보일 내용을 워커에서 만든다 — 그림 한 장 또는 글 앞부분.
//!
//! **디스크를 읽으므로 UI 스레드에서 부르지 않는다**(AGENTS). 요청은 마지막 것만 만들고
//! (화살표로 빠르게 넘기면 앞의 것은 곧 대체된다), 결과에 일련번호를 실어 늦게 온 옛 결과를
//! 가려낸다. 메모리는 한 번에 한 장이다 — 이 모듈은 결과를 쥐지 않고 보내기만 한다.
//!
//! 글 해석에 `remote::charset` 을 쓰지 않는다 — `fs` 는 `remote` 를 참조하지 않는다(계층 규약).
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Shell::{SIIGBF_RESIZETOFIT, SIIGBF_THUMBNAILONLY};

use crate::fs::thumbnail::{ThumbnailImage, shell_image};

/// 그림의 긴 변 상한 — 한 장이 최대 1024×1024×4 = 4MB 다 (원문 Q9 답)
pub const PREVIEW_PX: i32 = 1024;

/// 글은 앞의 이만큼만 읽는다
pub const TEXT_HEAD_BYTES: usize = 64 * 1024;

/// 셸 썸네일로 그리는 확장자 — 그 밖은 글로 읽어 본다(D13)
const IMAGE_EXTENSIONS: [&str; 10] = [
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "ico", "heic",
];

/// 미리보기 내용
#[derive(Debug, Clone, PartialEq)]
pub enum PreviewContent {
    Image(ThumbnailImage),
    Text(String),
    /// NUL 이 든 파일 — 글로 보이지 않는다
    Binary,
    /// 읽지 못했거나 셸이 그림을 주지 않았다
    Failed,
}

/// 확장자로 그림 파일인가를 가른다(대소문자 무시)
pub fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .is_some_and(|ext| IMAGE_EXTENSIONS.contains(&ext.as_str()))
}

/// 파일 앞부분을 글로 푼다 — 글이 아니면 `None`.
///
/// 차례: UTF-8 BOM · UTF-16 BOM(LE/BE) → NUL 이 있으면 바이너리 → UTF-8 → 시스템 코드 페이지.
/// **UTF-16 BOM 을 NUL 검사보다 먼저 본다** — UTF-16 본문에는 NUL 바이트가 들어 있다.
/// 앞부분만 읽어 끝에서 잘린 UTF-8 은 미완 글자(최대 3바이트)만 버린다
pub fn decode_text(bytes: &[u8]) -> Option<String> {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return Some(utf8_head(rest).unwrap_or_else(|| String::from_utf8_lossy(rest).into_owned()));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return Some(utf16(rest, u16::from_le_bytes));
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return Some(utf16(rest, u16::from_be_bytes));
    }
    if bytes.contains(&0) {
        return None;
    }
    Some(utf8_head(bytes).unwrap_or_else(|| decode_system_code_page(bytes)))
}

/// UTF-8 로 읽힌다면 그 글 — 끝의 미완 글자만 허용한다
fn utf8_head(bytes: &[u8]) -> Option<String> {
    match std::str::from_utf8(bytes) {
        Ok(text) => Some(text.to_owned()),
        // `error_len` 이 `None` 이면 끝에서 잘린 것이다 — 그 앞까지는 온전한 UTF-8 이다
        Err(error) if error.error_len().is_none() && bytes.len() - error.valid_up_to() <= 3 => {
            Some(String::from_utf8_lossy(&bytes[..error.valid_up_to()]).into_owned())
        }
        Err(_) => None,
    }
}

fn utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| unit([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

/// 시스템 코드 페이지(`CP_ACP` — 한국어 Windows 면 CP949)로 푼다.
///
/// 안전성: `MultiByteToWideChar` 는 길이를 바이트/문자 수로 받고 버퍼 밖을 건드리지 않는다.
/// 필요한 길이를 먼저 물어 그만큼만 잡아 두 번째 호출에 넘긴다
pub(crate) fn decode_system_code_page(bytes: &[u8]) -> String {
    use windows::Win32::Globalization::{CP_ACP, MultiByteToWideChar};
    let needed = unsafe { MultiByteToWideChar(CP_ACP, Default::default(), bytes, None) };
    if needed <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut wide = vec![0u16; needed as usize];
    let written = unsafe {
        MultiByteToWideChar(CP_ACP, Default::default(), bytes, Some(wide.as_mut_slice()))
    };
    if written <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    String::from_utf16_lossy(&wide[..written as usize])
}

/// 파일 하나의 미리보기를 만든다 — COM 이 초기화된 스레드에서 부른다(그림이 셸을 쓴다)
fn make_preview(path: &Path) -> PreviewContent {
    if is_image(path) {
        // THUMBNAILONLY — 만들 수 없으면 형식 아이콘 대신 실패를 받는다(docs/conventions.md 2026-09-29)
        return shell_image(path, PREVIEW_PX, SIIGBF_RESIZETOFIT | SIIGBF_THUMBNAILONLY)
            .map_or(PreviewContent::Failed, PreviewContent::Image);
    }
    let mut head = Vec::with_capacity(TEXT_HEAD_BYTES);
    let read = std::fs::File::open(path)
        .and_then(|file| file.take(TEXT_HEAD_BYTES as u64).read_to_end(&mut head));
    match read {
        Ok(_) => decode_text(&head).map_or(PreviewContent::Binary, PreviewContent::Text),
        Err(_) => PreviewContent::Failed,
    }
}

enum Request {
    Make { seq: u64, path: PathBuf },
    Stop,
}

/// 미리보기 워커 — 만들면 스레드가 뜨고, 버리면 멈춘다
pub struct PreviewWorker {
    tx: Sender<Request>,
    rx: Receiver<(u64, PreviewContent)>,
    /// 마지막으로 보낸 요청의 번호 — 이것과 다른 결과는 버린다
    latest: u64,
}

impl PreviewWorker {
    pub fn start() -> PreviewWorker {
        let (tx, worker_rx) = channel();
        let (worker_tx, rx) = channel();
        std::thread::spawn(move || worker(worker_rx, worker_tx));
        PreviewWorker { tx, rx, latest: 0 }
    }

    /// 그 파일의 미리보기를 청한다 — 앞서 청한 것은 곧 버려진다
    pub fn request(&mut self, path: PathBuf) {
        self.latest += 1;
        let _ = self.tx.send(Request::Make {
            seq: self.latest,
            path,
        });
    }

    /// 마지막 요청의 결과가 왔으면 그것 — 옛 요청의 결과는 여기서 버린다
    pub fn poll(&mut self) -> Option<PreviewContent> {
        let mut found = None;
        for (seq, content) in self.rx.try_iter() {
            if seq == self.latest {
                found = Some(content);
            }
        }
        found
    }
}

impl Drop for PreviewWorker {
    fn drop(&mut self) {
        let _ = self.tx.send(Request::Stop);
    }
}

/// 워커 본체 — 쌓인 요청 중 마지막 것만 만든다
fn worker(rx: Receiver<Request>, tx: Sender<(u64, PreviewContent)>) {
    // 안전성: 이 스레드에서 초기화하고 성공했을 때만 같은 스레드에서 해제한다
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
    while let Ok(first) = rx.recv() {
        let mut last = first;
        for next in rx.try_iter() {
            last = next;
        }
        let Request::Make { seq, path } = last else {
            break;
        };
        if tx.send((seq, make_preview(&path))).is_err() {
            break;
        }
    }
    if initialized {
        // 안전성: 위에서 성공한 초기화와 같은 스레드에서 1회 호출
        unsafe {
            CoUninitialize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_은_그대로_읽는다() {
        assert_eq!(
            decode_text("안녕 hello".as_bytes()).as_deref(),
            Some("안녕 hello")
        );
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice("가나".as_bytes());
        assert_eq!(decode_text(&bom).as_deref(), Some("가나"), "BOM 을 남겼다");
    }

    #[test]
    fn 끝에서_잘린_utf8_은_미완_글자만_버린다() {
        let text = "가나다".as_bytes();
        // 「다」(3바이트)의 앞 두 바이트에서 잘렸다
        let cut = &text[..text.len() - 1];
        assert_eq!(decode_text(cut).as_deref(), Some("가나"));
    }

    #[test]
    fn utf16le_bom_을_읽는다() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "가a".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_text(&bytes).as_deref(), Some("가a"));
    }

    #[test]
    fn utf16be_bom_을_읽는다() {
        let mut bytes = vec![0xFE, 0xFF];
        for unit in "가a".encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        assert_eq!(decode_text(&bytes).as_deref(), Some("가a"));
    }

    #[test]
    fn nul_이_있으면_바이너리다() {
        assert_eq!(decode_text(b"MZ\x90\x00\x03"), None);
    }

    #[test]
    fn utf8_이_아니면_시스템_코드_페이지로_읽는다() {
        // CP949 의 「가」(B0 A1) — UTF-8 로는 잘못된 바이트열이다
        let bytes = [0xB0, 0xA1];
        let text = decode_text(&bytes).expect("글로 읽혀야 한다");
        let acp = unsafe { windows::Win32::Globalization::GetACP() };
        if acp == 949 {
            assert_eq!(text, "가");
        } else {
            // 다른 코드 페이지의 PC 에서도 무엇인가로는 읽힌다 — UTF-8 대체 글자로 물러서지 않는다
            assert!(!text.is_empty());
        }
    }

    #[test]
    fn 확장자로_이미지를_가린다() {
        assert!(is_image(Path::new(r"C:\a\사진.JPG")));
        assert!(is_image(Path::new(r"C:\a\b.webp")));
        assert!(!is_image(Path::new(r"C:\a\b.txt")));
        assert!(!is_image(Path::new(r"C:\a\png")));
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fe_preview_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("임시 폴더");
        dir
    }

    /// 마지막 요청의 결과를 기다린다 — 셸 썸네일은 첫 호출이 느릴 수 있다
    fn wait(worker: &mut PreviewWorker) -> Option<PreviewContent> {
        for _ in 0..2000 {
            if let Some(content) = worker.poll() {
                return Some(content);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    #[test]
    fn 텍스트_파일은_앞_64kb_만_읽는다() {
        let dir = temp_dir("text");
        let file = dir.join("긴글.txt");
        std::fs::write(&file, "a".repeat(TEXT_HEAD_BYTES + 5000)).expect("쓰기");
        let mut worker = PreviewWorker::start();
        worker.request(file);
        let got = wait(&mut worker);
        let _ = std::fs::remove_dir_all(&dir);
        match got {
            Some(PreviewContent::Text(text)) => assert_eq!(text.len(), TEXT_HEAD_BYTES),
            other => panic!("글이 오지 않았다: {other:?}"),
        }
    }

    #[test]
    fn 큰_그림도_긴_변이_1024를_넘지_않는다() {
        let dir = temp_dir("image");
        let file = dir.join("큰그림.png");
        image::RgbaImage::from_pixel(2000, 1000, image::Rgba([0, 128, 255, 255]))
            .save(&file)
            .expect("PNG 쓰기");
        let mut worker = PreviewWorker::start();
        worker.request(file);
        let got = wait(&mut worker);
        let _ = std::fs::remove_dir_all(&dir);
        match got {
            Some(PreviewContent::Image(image)) => {
                assert!(
                    image.width.max(image.height) <= PREVIEW_PX as usize,
                    "{}x{}",
                    image.width,
                    image.height
                );
                assert!(
                    image.width > 256,
                    "썸네일 크기(256)로 왔다: {}x{}",
                    image.width,
                    image.height
                );
            }
            other => panic!("그림이 오지 않았다: {other:?}"),
        }
    }

    #[test]
    fn 바이너리_파일은_미리보지_않는다() {
        let dir = temp_dir("binary");
        let file = dir.join("프로그램.bin");
        std::fs::write(&file, b"MZ\x90\x00\x03\x00\x00\x00").expect("쓰기");
        let mut worker = PreviewWorker::start();
        worker.request(file);
        let got = wait(&mut worker);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(got, Some(PreviewContent::Binary));
    }

    #[test]
    fn 나중_요청이_앞_요청을_대체한다() {
        let dir = temp_dir("latest");
        let first = dir.join("첫째.txt");
        let second = dir.join("둘째.txt");
        std::fs::write(&first, "첫째").expect("쓰기");
        std::fs::write(&second, "둘째").expect("쓰기");
        let mut worker = PreviewWorker::start();
        worker.request(first);
        worker.request(second);
        let got = wait(&mut worker);
        // 앞 요청의 결과가 뒤늦게 와도 받지 않는다
        std::thread::sleep(std::time::Duration::from_millis(100));
        let late = worker.poll();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(got, Some(PreviewContent::Text("둘째".to_owned())));
        assert_eq!(late, None, "옛 요청의 결과를 받았다");
    }
}
