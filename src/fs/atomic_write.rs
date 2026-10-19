//! 파일을 **다 쓴 뒤에만** 바꿔 끼운다 — 쓰는 도중 끊겨도 옛 파일이 온전히 남는다.
//!
//! `std::fs::write`는 대상을 먼저 잘라 내고 쓴다. 그 사이에 프로세스가 죽거나 전원이 나가면
//! 잘린 파일이 남고, 설정 파일이면 다음 실행이 그것을 읽지 못해 사이트 목록까지 통째로
//! 기본값으로 돌아간다. 그래서 같은 폴더의 임시 파일에 쓰고 디스크에 내린 뒤 이름을 바꾼다.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 쓰기와 교체를 한 번에 하나로 묶는 잠금.
///
/// 임시 파일 이름이 대상마다 고정이라(언인스톨러가 그 이름으로 지운다), 두 스레드가 같은
/// 파일을 동시에 저장하면 한 임시 파일에 섞여 쓸 수 있다 — SFTP 연결 워커 둘이 거의 같은
/// 때에 지문을 받아들이는 경우다
static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// 대상 옆의 임시 파일 경로 — `settings.json` → `settings.json.tmp`
pub fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

/// `bytes`를 `path`에 원자적으로 쓴다 — 임시 파일에 쓰고 `sync_all`로 디스크에 내린 뒤 교체한다.
///
/// 실패하면 임시 파일을 지우고 오류를 돌린다. 대상은 그대로 남는다
pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    // 다른 스레드가 잠금을 쥔 채 패닉했어도 지킬 상태가 없다 — 그대로 이어 쓴다
    let _guard = WRITE_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp = temp_path(path);
    let result = write_and_replace(path, &temp, bytes);
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

fn write_and_replace(path: &Path, temp: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::File::create(temp)?;
    file.write_all(bytes)?;
    // 내용을 먼저 디스크에 내린다 — 내리지 않고 바꾸면 전원이 나갔을 때 이름 바꾸기만
    // 기록되고 내용은 비어 있을 수 있다
    file.sync_all()?;
    drop(file);
    std::fs::rename(temp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 시험마다 따로 쓰는 빈 폴더
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("moa-atomic-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("임시 폴더");
        dir
    }

    #[test]
    fn 없는_파일을_만든다() {
        let dir = temp_dir("new");
        let path = dir.join("settings.json");
        write(&path, b"hello").expect("쓰기");
        assert_eq!(std::fs::read(&path).expect("읽기"), b"hello");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 있는_파일을_통째로_바꾼다() {
        // 짧은 내용으로 바꿔도 옛 꼬리가 남지 않아야 한다
        let dir = temp_dir("replace");
        let path = dir.join("settings.json");
        std::fs::write(&path, b"a much longer old content").expect("옛 파일");
        write(&path, b"new").expect("쓰기");
        assert_eq!(std::fs::read(&path).expect("읽기"), b"new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 성공하면_임시_파일이_남지_않는다() {
        let dir = temp_dir("clean");
        let path = dir.join("known_hosts.json");
        write(&path, b"{}").expect("쓰기");
        assert!(path.exists());
        assert!(
            !dir.join("known_hosts.json.tmp").exists(),
            "임시 파일이 남았다"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn 교체에_실패하면_오류를_돌리고_임시_파일을_지운다() {
        // 대상 자리에 폴더가 있으면 파일로 바꿔 끼울 수 없다
        let dir = temp_dir("fail");
        let path = dir.join("settings.json");
        std::fs::create_dir_all(&path).expect("방해하는 폴더");
        assert!(write(&path, b"x").is_err(), "교체가 실패해야 한다");
        assert!(path.is_dir(), "대상이 바뀌었다");
        assert!(
            !dir.join("settings.json.tmp").exists(),
            "실패한 임시 파일이 남았다"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
