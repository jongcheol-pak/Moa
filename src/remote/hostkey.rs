//! 알려진 호스트(SSH 서버 지문) 저장소 — TOFU 판정 (D15).
//!
//! SFTP는 서버가 자기 공개키를 내미는데, 그것이 **처음 보는 키인지 바뀐 키인지**를 우리가
//! 판정하지 않으면 중간자 공격을 그대로 통과시킨다. 첫 연결에서 지문을 보여 주고 사용자가
//! 수락하면 저장했다가(Trust On First Use), 다음부터는 대조해서 다르면 **연결을 거부**한다.
//!
//! 확인 대화 화면은 `ui`(T10)가 만든다 — 이 모듈은 판정과 저장만 한다.
use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "known_hosts.json";

/// 서버 지문 대조 결과
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyCheck {
    /// 저장해 둔 지문과 같다 — 물어볼 것이 없다
    Match,
    /// 처음 보는 서버
    Unknown { fingerprint: String },
    /// **저장해 둔 지문과 다르다** — 서버를 다시 설치했거나, 중간에 누가 끼어 있다
    Changed { old: String, new: String },
}

/// 확인 대화가 돌려주는 사용자의 결정 (T10이 만든다)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyDecision {
    Accept,
    Reject,
}

/// 호스트·포트 → SHA256 지문 표.
///
/// 저장 위치는 **실행 파일 옆의 `known_hosts.json`**이며 설정 파일과 같은 폴더다.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownHosts {
    /// `호스트:포트` → `SHA256:...`
    #[serde(default)]
    entries: BTreeMap<String, String>,
}

impl KnownHosts {
    pub fn empty() -> KnownHosts {
        KnownHosts::default()
    }

    /// 파일에서 읽는다. **없거나 깨졌으면 빈 표**다 — 그러면 모든 서버가 `Unknown`이 되어
    /// 사용자에게 다시 묻는다. 깨진 파일을 무시하고 조용히 수락하는 경로를 두지 않는다.
    pub fn load() -> KnownHosts {
        let Some(path) = known_hosts_path() else {
            return KnownHosts::empty();
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return KnownHosts::empty();
        };
        Self::parse(&text).unwrap_or_else(KnownHosts::empty)
    }

    /// 파일 I/O와 분리한 해석부 (단위 테스트 대상)
    pub fn parse(text: &str) -> Option<KnownHosts> {
        serde_json::from_str(text).ok()
    }

    /// 지문을 대조한다. `host`의 대소문자는 구분하지 않는다
    pub fn check(&self, host: &str, port: u16, fingerprint: &str) -> HostKeyCheck {
        match self.entries.get(&entry_key(host, port)) {
            Some(known) if known == fingerprint => HostKeyCheck::Match,
            Some(known) => HostKeyCheck::Changed {
                old: known.clone(),
                new: fingerprint.to_owned(),
            },
            None => HostKeyCheck::Unknown {
                fingerprint: fingerprint.to_owned(),
            },
        }
    }

    /// 사용자가 수락한 지문을 표에 넣는다. **호출은 수락 결정 뒤에만 일어난다**
    pub fn accept(&mut self, host: &str, port: u16, fingerprint: &str) {
        self.entries
            .insert(entry_key(host, port), fingerprint.to_owned());
    }

    /// 파일로 저장한다. 실패는 조용히 생략한다 — 저장에 실패하면 다음 연결에서 다시 물을 뿐,
    /// 그 때문에 연결을 막을 이유는 없다 (설정 저장과 같은 규약)
    pub fn save(&self) {
        let Some(path) = known_hosts_path() else {
            return;
        };
        let Ok(json) = serde_json::to_string_pretty(self) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // 다 쓴 뒤에만 바꿔 끼운다 — 쓰다 끊겨 잘리면 받아들인 지문이 전부 사라진다
        let _ = crate::fs::atomic_write::write(&path, json.as_bytes());
    }

    #[cfg(test)]
    fn to_json(&self) -> String {
        serde_json::to_string(self).expect("직렬화")
    }
}

/// 지문 파일 경로 — **실행 파일과 같은 폴더**다(설정 파일과 같은 규약 — `app::settings`).
/// 실행 파일 위치를 알 수 없는 비정상 환경이면 `None`이다
fn known_hosts_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(FILE_NAME))
}

fn entry_key(host: &str, port: u16) -> String {
    format!("{}:{}", host.to_ascii_lowercase(), port)
}

/// 서버 공개키 해시를 `ssh-keygen -lf`와 같은 표기로 옮긴다 (`SHA256:` + 패딩 없는 base64).
///
/// 사용자가 화면에 뜬 지문을 서버에서 뽑은 값과 눈으로 대조해야 하므로, 표기가 다르면
/// 대조 자체를 할 수 없다.
pub fn fingerprint_sha256(hash: &[u8]) -> String {
    format!("SHA256:{}", base64_no_pad(hash))
}

/// 패딩 없는 표준 base64 (RFC 4648). 이것 하나 때문에 패키지를 들이지 않는다
fn base64_no_pad(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut triple = 0u32;
        for (index, byte) in chunk.iter().enumerate() {
            triple |= u32::from(*byte) << (16 - 8 * index);
        }
        // 3바이트는 4글자, 2바이트는 3글자, 1바이트는 2글자로 나온다
        for index in 0..chunk.len() + 1 {
            let six = (triple >> (18 - 6 * index)) & 0b11_1111;
            out.push(ALPHABET[six as usize] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FINGERPRINT_A: &str = "SHA256:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU";
    const FINGERPRINT_B: &str = "SHA256:LPJNul+wow4m6DsqxbninhsWHlwfp0JecwQzYpOLmCQ";

    fn store() -> KnownHosts {
        let mut hosts = KnownHosts::empty();
        hosts.accept("example.test", 22, FINGERPRINT_A);
        hosts
    }

    #[test]
    fn 지문_파일은_실행_파일_옆에_놓인다() {
        // 설정 파일과 같은 폴더 규약 (2026-08-21 결정 — `app::settings`와 같은 자리)
        let path = known_hosts_path().expect("실행 파일 경로");
        let exe = std::env::current_exe().expect("실행 파일 경로");
        assert_eq!(
            path.parent(),
            exe.parent(),
            "실행 파일과 같은 폴더가 아니다"
        );
        assert_eq!(
            path.file_name(),
            Some(std::ffi::OsStr::new("known_hosts.json"))
        );
    }

    #[test]
    fn 처음_보는_서버는_미등록으로_판정한다() {
        let hosts = KnownHosts::empty();
        assert_eq!(
            hosts.check("example.test", 22, FINGERPRINT_A),
            HostKeyCheck::Unknown {
                fingerprint: FINGERPRINT_A.to_owned()
            }
        );
    }

    #[test]
    fn 저장된_지문과_같으면_일치다() {
        assert_eq!(
            store().check("example.test", 22, FINGERPRINT_A),
            HostKeyCheck::Match
        );
        // 호스트 이름의 대소문자는 구분하지 않는다
        assert_eq!(
            store().check("EXAMPLE.test", 22, FINGERPRINT_A),
            HostKeyCheck::Match
        );
    }

    #[test]
    fn 지문이_바뀌면_변경으로_판정한다() {
        assert_eq!(
            store().check("example.test", 22, FINGERPRINT_B),
            HostKeyCheck::Changed {
                old: FINGERPRINT_A.to_owned(),
                new: FINGERPRINT_B.to_owned()
            }
        );
    }

    #[test]
    fn 포트가_다르면_다른_서버로_본다() {
        // 같은 기계에 SSH 서버를 여러 개 띄우는 경우가 있다
        assert!(matches!(
            store().check("example.test", 2222, FINGERPRINT_A),
            HostKeyCheck::Unknown { .. }
        ));
    }

    #[test]
    fn 표는_파일_왕복해도_같다() {
        let mut hosts = store();
        hosts.accept("other.test", 2222, FINGERPRINT_B);
        let json = hosts.to_json();
        let back = KnownHosts::parse(&json).expect("역직렬화");
        assert_eq!(back, hosts);
        assert_eq!(
            back.check("other.test", 2222, FINGERPRINT_B),
            HostKeyCheck::Match
        );
    }

    #[test]
    fn 깨진_지문_파일은_전부_미등록으로_돌린다() {
        // 조용히 수락하지 않는다 — 다시 묻는 쪽이 안전하다
        assert!(KnownHosts::parse("{ 깨진 내용").is_none());
        assert!(KnownHosts::parse("").is_none());
        // 빈 표로 폴백하면 모든 서버가 미등록이 된다
        let fallback = KnownHosts::parse("{ 깨진 내용").unwrap_or_else(KnownHosts::empty);
        assert!(matches!(
            fallback.check("example.test", 22, FINGERPRINT_A),
            HostKeyCheck::Unknown { .. }
        ));
    }

    #[test]
    fn 지문_표기는_ssh_keygen과_같다() {
        // RFC 4648 시험값 — 패딩(`=`)을 붙이지 않는 것이 OpenSSH 표기다
        assert_eq!(base64_no_pad(b""), "");
        assert_eq!(base64_no_pad(b"f"), "Zg");
        assert_eq!(base64_no_pad(b"fo"), "Zm8");
        assert_eq!(base64_no_pad(b"foo"), "Zm9v");
        assert_eq!(base64_no_pad(b"foob"), "Zm9vYg");
        assert_eq!(base64_no_pad(b"fooba"), "Zm9vYmE");
        assert_eq!(base64_no_pad(b"foobar"), "Zm9vYmFy");

        // 빈 입력의 SHA256 해시 32바이트 → OpenSSH가 보이는 지문
        let empty_sha256: [u8; 32] = [
            0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
            0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
            0x78, 0x52, 0xb8, 0x55,
        ];
        assert_eq!(fingerprint_sha256(&empty_sha256), FINGERPRINT_A);
    }
}
