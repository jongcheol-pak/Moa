//! 앱 자체 되돌리기(`Ctrl+Z`)의 기록과 실행 직전 대조 (FR-74).
//!
//! 셸 작업 결과(`fs::file_op`)를 되돌릴 단계로 바꿔 담고, 되돌리기 직전에 그 단계가 아직
//! 성립하는지 대조한다. **파일시스템을 직접 묻지 않는다** — 묻는 자리는 [`Probe`]로 받아,
//! 시험은 가짜를, 실행 워커는 실제 파일시스템을 넣는다.
use std::path::Path;
use std::time::SystemTime;

/// 항목 하나의 도장 — 기록한 뒤 내용이 바뀌었는지 가르는 값.
///
/// 파일은 크기와 수정 시각, 폴더는 수정 시각만 본다(폴더 크기는 0으로 둔다). **폴더의 수정
/// 시각은 바로 아래 항목이 바뀔 때만 바뀌어** 더 깊은 곳의 변경은 이 값이 못 본다 — 되돌리기가
/// 보내는 곳이 휴지통이라 되살릴 수 있어 받아들였다
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub len: u64,
    pub modified: Option<SystemTime>,
}

/// 지금 그 자리의 도장 — 없으면 `None`.
///
/// 링크는 따라가지 않는다(`symlink_metadata`) — 링크가 가리키는 쪽이 아니라 링크 자체가
/// 되돌릴 대상이다
pub fn stamp_of(path: &Path) -> Option<Stamp> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some(Stamp {
        len: if meta.is_dir() { 0 } else { meta.len() },
        modified: meta.modified().ok(),
    })
}
