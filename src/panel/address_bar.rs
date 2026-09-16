//! 주소창 입력 정규화 (FR-6) — 따옴표·공백을 벗기고 상대 경로를 절대화한다.
//!
//! 화면은 `ui::address_bar`가 그린다. 여기 남은 것은 그 입력 처리 규칙 둘이다 —
//! 로컬용과 원격용. **두 규칙을 한 함수로 합치지 않는다**: 로컬은 `\`도 구분자로 보고
//! `\\서버\공유`를 절대 경로로 받지만, 원격 경로에는 `\`가 파일 이름 글자로 들어갈 수 있다
//! (`remote::types::RemotePath`의 문서 주석).
use crate::remote::types::RemotePath;
use std::path::{Path, PathBuf};

/// 입력 문자열 정규화 — 따옴표·공백 제거 후, 상대 경로면 현재 경로 기준 절대화 (T5 Edge)
pub fn normalize_input(current: &Path, input: &str) -> Option<PathBuf> {
    let trimmed = input.trim().trim_matches('"').trim();
    if trimmed.is_empty() {
        return None;
    }
    let p = Path::new(trimmed);
    if p.is_absolute() || trimmed.starts_with(r"\\") {
        Some(p.to_path_buf())
    } else {
        Some(current.join(p))
    }
}

/// 원격 주소창 입력 정규화 — 따옴표·공백을 벗기고, 상대 경로면 보고 있는 위치 기준으로 붙인다.
///
/// `/`로 시작하면 서버 안 절대 경로이고, 아니면 `current` 아래로 붙인다. 빈 입력은 `None`이다.
///
/// **서버의 현재 작업 디렉터리는 보지 않는다** — 그것을 알려면 서버에 물어야 하고, 그래서
/// `RemotePath::parent()`도 상대 경로의 단일 세그먼트를 `None`으로 둔다. 기준은 화면이 지금
/// 가리키는 경로 하나뿐이다.
///
/// 중복 슬래시 접기·후행 슬래시 제거·`.`·`..` 해석은 하지 않는다 — 앞의 둘은
/// `RemotePath::new`가 이미 하고, 뒤의 둘은 서버가 해석할 몫이다
pub fn normalize_remote_input(current: &RemotePath, input: &str) -> Option<RemotePath> {
    let trimmed = input.trim().trim_matches('"').trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('/') {
        Some(RemotePath::new(trimmed))
    } else {
        Some(current.join(trimmed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 절대_경로는_그대로() {
        let cur = Path::new("C:\\base");
        assert_eq!(
            normalize_input(cur, r"D:\data").unwrap(),
            PathBuf::from(r"D:\data")
        );
    }

    #[test]
    fn 따옴표와_공백을_벗긴다() {
        let cur = Path::new("C:\\base");
        assert_eq!(
            normalize_input(cur, "  \"D:\\my folder\"  ").unwrap(),
            PathBuf::from(r"D:\my folder")
        );
    }

    #[test]
    fn 상대_경로는_현재_기준_절대화() {
        let cur = Path::new(r"C:\base");
        assert_eq!(
            normalize_input(cur, "sub\\dir").unwrap(),
            PathBuf::from(r"C:\base\sub\dir")
        );
    }

    #[test]
    fn 빈_입력은_none() {
        assert!(normalize_input(Path::new("C:\\"), "   ").is_none());
    }

    #[test]
    fn unc_경로_지원() {
        assert!(
            normalize_input(Path::new("C:\\"), r"\\server\share")
                .unwrap()
                .to_string_lossy()
                .starts_with(r"\\server")
        );
    }

    fn r(s: &str) -> RemotePath {
        RemotePath::new(s)
    }

    #[test]
    fn 원격_슬래시로_시작하면_서버_절대_경로다() {
        assert_eq!(
            normalize_remote_input(&r("/var/www"), "/etc").unwrap(),
            r("/etc")
        );
    }

    #[test]
    fn 원격_상대_입력은_보고_있는_경로_아래로_붙는다() {
        assert_eq!(
            normalize_remote_input(&r("/var"), "www").unwrap(),
            r("/var/www")
        );
        // 여러 세그먼트도 한 번에 받는다
        assert_eq!(
            normalize_remote_input(&r("/var"), "www/html").unwrap(),
            r("/var/www/html")
        );
        // 루트에서도 슬래시가 겹치지 않는다
        assert_eq!(normalize_remote_input(&r("/"), "pub").unwrap(), r("/pub"));
    }

    #[test]
    fn 원격_따옴표와_공백을_벗긴다() {
        assert_eq!(
            normalize_remote_input(&r("/"), "  \"/my folder\"  ").unwrap(),
            r("/my folder")
        );
    }

    #[test]
    fn 원격_빈_입력은_none() {
        assert!(normalize_remote_input(&r("/var"), "   ").is_none());
        assert!(normalize_remote_input(&r("/var"), "\"\"").is_none());
    }

    #[test]
    fn 원격_경로의_역슬래시는_구분자가_아니라_이름_글자다() {
        // 로컬 정규화는 `\`를 구분자로 보지만 원격 파일 이름에는 그 글자가 그대로 들어간다.
        // 그래서 두 규칙을 한 함수로 합칠 수 없다 (모듈 주석)
        let 붙인_것 = normalize_remote_input(&r("/var"), r"a\b").unwrap();
        assert_eq!(붙인_것.as_str(), r"/var/a\b");
        assert_eq!(붙인_것.segments().count(), 2);
    }
}
