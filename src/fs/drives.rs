//! 드라이브 줄 — 이 PC의 논리 드라이브와 그 연결 상태 (2026-08-17 사용자 요청).
//!
//! **트리가 아니라 앱이 이 목록을 소유한다** — 트리는 패널마다 있어(패널 셋이면 셋)
//! 각자 조회하면 셸·네트워크 왕복이 그만큼 되풀이되고, 연결 상태도 패널마다 갈려
//! 같은 드라이브에 X가 있는 트리와 없는 트리가 한 화면에 설 수 있다(즐겨찾기와 같은 구조).
//!
//! 조회를 **둘로 나눈 이유는 비용이 하늘과 땅 차이**라서다 — 목록 만들기(드라이브 열거 +
//! 셸 표시 이름·아이콘)는 수십 ms인데, 끊긴 네트워크 드라이브의 **접근 판정은 첫 시도가
//! 2.8초**다(실측). 한 함수로 묶으면 드라이브 줄이 화면에 서는 것 자체가 그만큼 늦고,
//! 시험도 끊긴 드라이브가 있는 PC에서 초 단위로 늘어진다.
use crate::fs::icons::IconCache;
use eframe::egui;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use windows::Win32::Storage::FileSystem::{
    GetDriveTypeW, GetFileAttributesW, GetLogicalDrives, INVALID_FILE_ATTRIBUTES,
};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::System::WindowsProgramming::DRIVE_REMOTE;
use windows::core::HSTRING;

/// 트리에 설 드라이브 한 줄.
///
/// 화면이 그리는 데 필요한 것만 담는다 — 용량·파일시스템 종류는 이 앱이 보이지 않는다
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveRow {
    /// 뿌리 경로 (`C:\`)
    pub path: PathBuf,
    /// 셸 표시 이름 (`로컬 디스크 (C:)`) — 얻지 못하면 경로 문자열이다
    pub label: String,
    /// 시스템 이미지 리스트의 아이콘 인덱스.
    ///
    /// 인덱스는 프로세스 전역이라 **워커가 얻어 UI 스레드가 그려도 된다**
    pub icon: i32,
    /// 네트워크 드라이브인가 (`GetDriveTypeW == DRIVE_REMOTE`).
    ///
    /// 연결 끊김 표시는 네트워크 드라이브에만 붙는다 — 탐색기도 빈 광학 드라이브 같은
    /// 로컬 자리에는 그 표식을 두지 않는다
    pub network: bool,
    /// 닿지 못하는 상태인가 — 참이면 트리가 아이콘에 X 배지를 겹친다.
    ///
    /// `list_drives`는 이 값을 **언제나 `false`로 시작**한다(판정 전에는 배지를 두지 않는다).
    /// 접근 판정 결과가 오거나 사용자가 그 드라이브를 열어 본 뒤에 채워진다
    pub offline: bool,
}

/// 워커가 앱에 올려보내는 것 — 목록과 판정이 **따로 도착한다**.
///
/// 채널을 둘로 나누지 않는 이유는 받는 자리가 둘이 되기 때문이다
#[derive(Debug)]
pub enum DriveScan {
    /// 드라이브 목록이 준비됐다 (먼저 온다)
    Listed(Vec<DriveRow>),
    /// 네트워크 드라이브의 접근 판정 `(뿌리 경로, 닿았는가)` (뒤이어 온다)
    Reachability(Vec<(PathBuf, bool)>),
    /// 즐겨찾기 가운데 **지금 없는 경로들** (맨 뒤에 온다 — FR-56 · FR-67).
    ///
    /// **비어 있어도 보낸다** — 그것이 「전부 있다」는 답이고, 받는 쪽은 이 소식으로
    /// 이번 확인이 끝났음을 안다
    Missing(Vec<PathBuf>),
}

/// 드라이브 줄을 워커 스레드에서 만들고, 결과를 받을 채널을 돌려준다.
///
/// **두 번 보낸다** — 목록(`Listed`)을 먼저, 네트워크 드라이브의 접근 판정(`Reachability`)을
/// 뒤이어. 한 번에 묶으면 끊긴 드라이브 하나가 화면의 드라이브 줄 전체를 몇 초씩 붙든다.
///
/// 도착할 때마다 `request_repaint`로 화면을 깨운다 — 워커는 프레임 흐름을 모르므로
/// 이 신호가 없으면 사용자가 마우스를 움직일 때까지 결과가 화면에 오르지 않는다
/// `favorites`는 실재를 함께 확인할 즐겨찾기 경로들이다 (FR-67 · D5) — **스레드를 따로
/// 띄우지 않으려고** 같은 워커에 얹는다. 둘 다 「이 경로가 아직 있는가」를 묻는 같은 성질의
/// 일이고, 나누면 스레드가 하나 늘고 주기도 따로 관리해야 한다
pub fn spawn_scan(ctx: &egui::Context, favorites: Vec<PathBuf>) -> Receiver<DriveScan> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        // 셸 조회(`SHGetFileInfoW`)는 COM을 쓴다 — 스레드마다 초기화가 필요하다
        // (`fs::thumbnail`의 썸네일 워커와 같은 방식). 실패해도 조회만 폴백되고 앱은 계속 돈다
        // 안전성: 이 스레드에서 열고 끝에서 반드시 닫는다
        let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        scan_into(&tx, &ctx, &favorites);
        if com {
            // 안전성: 위에서 성공한 초기화와 짝을 맞춘다
            unsafe { CoUninitialize() };
        }
    });
    rx
}

/// 워커 본체 — 목록을 보낸 뒤 판정을 보낸다. 받는 쪽이 사라지면 조용히 멎는다.
///
/// **보낼 때마다 화면을 깨운다** — 이 앱은 입력이 없으면 프레임을 돌리지 않으므로,
/// 마지막에 한 번만 깨우면 목록을 먼저 보낸 것이 헛일이 된다(드라이브 줄이 무거운
/// 접근 판정이 끝날 때까지 화면에 서지 않는다 — 조회를 둘로 나눈 이유가 사라진다)
fn scan_into(tx: &Sender<DriveScan>, ctx: &egui::Context, favorites: &[PathBuf]) {
    let mut icons = IconCache::new();
    let rows = list_drives(&mut icons);
    // 판정할 것을 먼저 챈다 — 목록은 곧 소유권을 넘긴다
    let network: Vec<PathBuf> = rows
        .iter()
        .filter(|row| row.network)
        .map(|row| row.path.clone())
        .collect();
    // 수신부가 이미 버려졌으면(앱 종료) 더 할 일이 없다 — 무거운 판정을 시작하지 않는다
    if tx.send(DriveScan::Listed(rows)).is_err() {
        return;
    }
    ctx.request_repaint();
    // 드라이브 판정이 먼저다 — 즐겨찾기 확인이 끊긴 경로에서 오래 걸릴 수 있어,
    // 뒤로 미뤄야 트리의 끊김 표식이 그만큼 늦지 않는다
    if !network.is_empty() {
        let judged = network
            .into_iter()
            .map(|root| {
                let reachable = is_reachable(&root);
                (root, reachable)
            })
            .collect();
        if tx.send(DriveScan::Reachability(judged)).is_err() {
            return;
        }
        ctx.request_repaint();
    }
    // **비어 있어도 보낸다** — 받는 쪽이 이 소식으로 이번 확인이 끝났음을 안다
    if tx
        .send(DriveScan::Missing(missing_favorites(favorites)))
        .is_ok()
    {
        ctx.request_repaint();
    }
}

/// 즐겨찾기 경로 가운데 **지금 없는 것들** (FR-56 · FR-67).
///
/// **폴더로 본다** — 즐겨찾기에는 폴더만 담긴다(FR-56). 끊긴 네트워크 경로에서는 이 조회가
/// 수십 초 걸릴 수 있어 위 `is_reachable`과 같은 이유로 **워커에서만** 부른다
fn missing_favorites(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter(|path| !path.is_dir())
        .cloned()
        .collect()
}

/// 드라이브 구성이 바뀌었는지 좇는다 — 논리 드라이브 비트마스크만 들고 있다
/// (2026-09-09 사용자 요청: USB를 꽂으면 트리에 곧바로 서야 한다).
///
/// **마스크만 보는 이유는 그것이 값싸기 때문이다** — 목록을 만들려면 드라이브마다 셸
/// 표시 이름·아이콘을 물어야 하는데, 그것을 매초 하면 감시가 감시 대상보다 비싸진다.
/// 비트 하나가 드라이브 문자 하나이므로 꽂힘·빠짐은 이 값의 변화로 그대로 드러난다.
///
/// **드라이브 문자가 그대로인 변화는 잡지 못한다** — 볼륨 레이블 변경, 문자가 이미 있는
/// 광학 드라이브의 디스크 삽입, 네트워크 드라이브 재연결. 그것들은 주기 확인(FR-67)이
/// 종전대로 덮는다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriveWatch {
    last: u32,
}

impl DriveWatch {
    /// **지금 구성을 기준선으로 삼아** 시작한다.
    ///
    /// 0으로 시작하지 않는 이유는 그러면 첫 확인이 언제나 「바뀌었다」가 되기 때문이다 —
    /// 앱이 뜰 때 이미 목록을 만든 직후라, 그 조회가 한 번 헛되이 더 돈다
    pub fn new(mask: u32) -> Self {
        Self { last: mask }
    }

    /// 이번에 읽은 마스크를 넣는다 — 직전과 다르면 참이고, 기준선이 그 값으로 옮겨간다.
    ///
    /// 참을 돌려준 뒤 같은 값이 다시 오면 거짓이다(변화는 한 번만 알린다)
    pub fn observe(&mut self, mask: u32) -> bool {
        let changed = mask != self.last;
        self.last = mask;
        changed
    }
}

/// 드라이브 구성을 얼마나 자주 살피는가 (2026-09-09 사용자 요청).
///
/// **1초인 이유는 이 확인이 값싸기 때문이다** — 한 바퀴에 하는 일이 `GetLogicalDrives()`
/// 하나뿐이고, 값이 그대로면 목록도 만들지 않고 화면도 깨우지 않는다. 사용자가 USB를
/// 꽂고 "바로"라고 느끼는 하한이기도 하다
const WATCH_PERIOD: std::time::Duration = std::time::Duration::from_secs(1);

/// 지금 이 PC의 논리 드라이브 비트마스크 — 비트 하나가 드라이브 문자 하나다
fn drive_mask() -> u32 {
    // 안전성: 인자 없는 조회 — 현재 드라이브 비트마스크만 반환한다
    unsafe { GetLogicalDrives() }
}

/// 드라이브가 꽂히거나 빠지는 것을 지켜보다가 **목록만** 다시 만들어 보낸다 (FR-9).
///
/// **`spawn_scan`과 하는 일이 다르다** — 저쪽은 시작·30초 주기에 목록 + 접근 판정 +
/// 즐겨찾기 실재를 한 벌로 확인하고 끝난다. 이쪽은 끝나지 않고 상주하며, 구성이 바뀐
/// 순간에만 목록을 보낸다. **접근 판정을 함께 하지 않는 것이 요점이다** — 끊긴 네트워크
/// 드라이브 하나가 2.8초를 물어(`is_reachable`), 함께 묶으면 USB가 그만큼 늦게 뜬다.
///
/// 그래서 `DriveScan`을 재사용하지 않고 목록만 보낸다 — 그 열거에는 *목록 → 판정 →
/// 즐겨찾기* 순서와 "마지막 소식이 오면 통로를 놓는다"는 종료 규약이 걸려 있어,
/// 끝나지 않는 이 워커가 쓰면 받는 쪽이 통로를 놓아 버린다.
///
/// **화면은 보낼 때만 깨운다** — 변화가 없는 바퀴에서 `request_repaint`를 부르면 이 앱이
/// 유휴에 프레임을 돌리지 않는다는 전제가 무너져, 매초 깨는 것과 같아진다
pub fn spawn_watch(ctx: &egui::Context) -> Receiver<Vec<DriveRow>> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        // 셸 조회(`SHGetFileInfoW`)는 COM을 쓴다 — `spawn_scan`과 같은 이유로 초기화한다.
        // 안전성: 이 스레드에서 열고 루프를 벗어난 뒤 반드시 닫는다
        let com = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
        watch_into(&tx, &ctx);
        if com {
            // 안전성: 위에서 성공한 초기화와 짝을 맞춘다
            unsafe { CoUninitialize() };
        }
    });
    rx
}

/// 감시 워커 본체 — 받는 쪽이 사라지면(앱 종료) 조용히 멎는다.
///
/// **아이콘 캐시를 바깥에 두고 재사용한다** — 바퀴마다 새로 만들면 드라이브가 바뀔 때마다
/// 셸 아이콘을 처음부터 다시 묻는다
fn watch_into(tx: &Sender<Vec<DriveRow>>, ctx: &egui::Context) {
    let mut icons = IconCache::new();
    let mut watch = DriveWatch::new(drive_mask());
    loop {
        std::thread::sleep(WATCH_PERIOD);
        if !watch.observe(drive_mask()) {
            continue;
        }
        if tx.send(list_drives(&mut icons)).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// 이 PC의 논리 드라이브 목록 (`C:\`, `D:\` …).
///
/// 비트마스크의 비트 순서가 곧 알파벳 순이라 따로 정렬하지 않는다.
/// **접근 판정을 하지 않는다** — 끊긴 네트워크 드라이브에서도 곧 돌아온다(모듈 주석)
pub fn list_drives(icons: &mut IconCache) -> Vec<DriveRow> {
    // 안전성: 인자 없는 조회 — 현재 드라이브 비트마스크만 반환한다
    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| PathBuf::from(format!("{}:\\", (b'A' + i as u8) as char)))
        .map(|path| {
            let text = path.to_string_lossy();
            // 탐색기처럼 `로컬 디스크 (C:)`로 보인다 — 이름을 우리가 조립하지 않는다
            let label = icons
                .shell_display_name(&text)
                .unwrap_or_else(|| text.clone().into_owned());
            DriveRow {
                icon: icons.icon_index_for_path(&text),
                network: is_network_drive(&path),
                label,
                path,
                offline: false,
            }
        })
        .collect()
}

/// 이 드라이브가 네트워크 드라이브인가.
///
/// 종류만 묻는 조회라 끊긴 드라이브에서도 즉시 돌아온다(실측 0ms) — 접근을 시도하는
/// `is_reachable`과 비용이 다르다
fn is_network_drive(root: &Path) -> bool {
    let root = HSTRING::from(root.to_string_lossy().as_ref());
    // 안전성: 널 종단 문자열을 넘기는 읽기 전용 조회 — 알 수 없는 드라이브는 0을 반환한다
    unsafe { GetDriveTypeW(&root) == DRIVE_REMOTE }
}

/// 이 드라이브에 지금 닿을 수 있는가.
///
/// **끊긴 네트워크 드라이브에서는 첫 시도가 2.8초까지 걸린다**(실측 — 윈도우가 연결을
/// 다시 맺어 보기 때문이다). 그래서 UI 스레드에서 부르면 안 되고, 워커에서만 쓴다.
///
/// 속성 조회로 판정하는 이유는 그것이 **가장 가벼운 접근**이라서다 — 목록을 읽으면
/// 파일이 많은 뿌리에서 값을 얻는 것과 무관한 비용을 문다
pub fn is_reachable(root: &Path) -> bool {
    let root = HSTRING::from(root.to_string_lossy().as_ref());
    // 안전성: 널 종단 문자열을 넘기는 읽기 전용 조회 — 실패는 INVALID_FILE_ATTRIBUTES로 온다
    unsafe { GetFileAttributesW(&root) != INVALID_FILE_ATTRIBUTES }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 시작_직후_같은_구성은_변화가_아니다() {
        // D5 — 기준선을 지금 구성으로 잡는다. 0으로 시작하면 첫 확인이 늘 참이 되고,
        // 앱이 뜰 때 이미 만든 목록을 한 번 더 만들게 된다
        let mut watch = DriveWatch::new(0b1101);
        assert!(!watch.observe(0b1101), "구성이 그대로인데 변화로 봤다");
    }

    #[test]
    fn 드라이브가_늘면_변화다() {
        // USB를 꽂는 경우 — 비트 하나가 선다
        let mut watch = DriveWatch::new(0b0101);
        assert!(watch.observe(0b1101), "드라이브가 늘었는데 알리지 않았다");
    }

    #[test]
    fn 드라이브가_빠지면_변화다() {
        // USB를 뽑는 경우 — 트리에서 줄이 사라져야 한다
        let mut watch = DriveWatch::new(0b1101);
        assert!(watch.observe(0b0101), "드라이브가 빠졌는데 알리지 않았다");
    }

    #[test]
    fn 변화는_한_번만_알린다() {
        // 알린 뒤 기준선이 새 값으로 옮겨간다 — 그러지 않으면 꽂은 뒤로 매초 목록을 만든다
        let mut watch = DriveWatch::new(0b0101);
        assert!(watch.observe(0b1101));
        assert!(!watch.observe(0b1101), "같은 구성을 두 번 알렸다");
    }

    #[test]
    fn 구성이_그대로면_감시_워커는_아무것도_보내지_않는다() {
        // T2 Acceptance — 매 바퀴 목록을 보내면 트리가 1초마다 갈아 끼워지고,
        // `request_repaint`가 함께 돌아 유휴에도 프레임이 계속 돈다.
        //
        // **주기의 두 배를 기다린다** — 첫 sleep 시작 시점의 오차와 스레드 스케줄링
        // 지연 때문에 1.5초에서는 워커가 아직 첫 순회를 마치지 않은 채 통과할 수 있다.
        // 시험 도중 드라이브를 꽂지 않는 한 구성은 그대로다
        let rx = spawn_watch(&egui::Context::default());
        std::thread::sleep(WATCH_PERIOD * 2);
        match rx.try_recv() {
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                panic!("워커가 첫 바퀴에 죽었다")
            }
            Ok(rows) => panic!("구성이 그대로인데 목록 {}줄을 보냈다", rows.len()),
        }
    }

    #[test]
    fn 없는_즐겨찾기_경로만_골라낸다() {
        // FR-56 · FR-67 — **폴더로 본다**(즐겨찾기에는 폴더만 담긴다).
        // 실제 폴더에 기대는 시험은 만들지 않는다 — 어느 PC에나 있는 것과 결코 없는 것만 쓴다
        let 있는_폴더 = std::env::temp_dir();
        let 없는_폴더 = 있는_폴더.join("모아_없는_폴더_시험용");
        let missing = missing_favorites(&[있는_폴더.clone(), 없는_폴더.clone()]);
        assert_eq!(missing, vec![없는_폴더]);
    }

    #[test]
    fn 확인할_경로가_없으면_없는_것도_없다() {
        assert!(missing_favorites(&[]).is_empty());
    }

    #[test]
    fn 드라이브_루트는_루트_경로_형태다() {
        // 실제 구성은 PC마다 다르므로 형태만 검증한다 (C: 드라이브는 항상 있다)
        let mut icons = IconCache::new();
        let rows = list_drives(&mut icons);
        assert!(rows.iter().any(|row| row.path == Path::new(r"C:\")));
        assert!(rows.iter().all(|row| row.path.parent().is_none()));
    }

    #[test]
    fn 드라이브는_셸_표시_이름으로_보인다() {
        // `C:\`가 아니라 `로컬 디스크 (C:)` 같은 이름이다.
        // 값 자체는 OS 언어·볼륨 레이블을 따르므로 "경로와 다르다"만 본다
        let mut icons = IconCache::new();
        let rows = list_drives(&mut icons);
        let row = rows
            .iter()
            .find(|row| row.path == Path::new(r"C:\"))
            .expect("C 드라이브");
        assert!(!row.label.is_empty(), "이름이 비었다");
        assert_ne!(
            row.label,
            row.path.to_string_lossy(),
            "경로가 그대로 왔다 — 셸 표시 이름을 거치지 않았다"
        );
    }

    #[test]
    fn 목록은_판정_전이라_아무것도_끊긴_것으로_두지_않는다() {
        // T3·T4 — `list_drives`는 접근을 시도하지 않는다. 배지는 판정이 온 뒤에만 붙는다
        let mut icons = IconCache::new();
        let rows = list_drives(&mut icons);
        assert!(
            rows.iter().all(|row| !row.offline),
            "판정도 하지 않고 끊긴 것으로 표시했다"
        );
    }

    #[test]
    fn 로컬_드라이브는_네트워크가_아니고_닿는다() {
        // C:는 이 PC의 고정 디스크다 — 두 판정의 기준선이 된다
        let root = Path::new(r"C:\");
        assert!(!is_network_drive(root), "고정 디스크를 네트워크로 봤다");
        assert!(is_reachable(root), "C 드라이브에 닿지 못했다");
    }

    #[test]
    fn 없는_드라이브에는_닿지_못한다() {
        // 판정이 늘 참을 돌려주면 배지가 영영 붙지 않는다 — 확실히 닿지 않는 자리로 견준다.
        // 드라이브 문자를 못 박지 않는 이유: PC마다 구성이 달라 `Q:`가 실재할 수 있다.
        // **비트마스크에 없는 문자**를 골라야 이 시험이 어느 PC에서나 같은 것을 본다
        // 안전성: 인자 없는 조회 — 현재 드라이브 비트마스크만 반환한다
        let mask = unsafe { GetLogicalDrives() };
        let unused = (0..26u32)
            .find(|i| mask & (1 << i) == 0)
            .map(|i| PathBuf::from(format!("{}:\\", (b'A' + i as u8) as char)));
        let Some(unused) = unused else {
            // A~Z가 전부 쓰이는 PC — 견줄 자리가 없어 이 시험은 할 일이 없다
            return;
        };
        assert!(
            !is_reachable(&unused),
            "쓰이지 않는 드라이브({})에 닿았다고 했다",
            unused.display()
        );
    }
}
