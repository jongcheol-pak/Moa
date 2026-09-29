//! 임시 성능 계측 — 어느 단계가 실제로 느린지 실측으로 가르기 위한 것이며 **기본은 꺼져 있다**.
//!
//! `MOA_PERF_LOG=1`로 켜면 **실행 파일 옆** `moa_perf.log`에 한 줄씩 덧붙인다. 켜지 않으면
//! 형식 문자열조차 만들지 않는다 — `log`가 클로저를 받아 `enabled()`가 거짓이면 부르지 않는다.
//!
//! **경로·파일 이름을 적지 않는다** — 이 파일은 사용자가 그대로 보내 오는 것이라 개인 폴더
//! 구조가 실려 나가면 안 된다. 남기는 것은 소요 시간과 개수뿐이다.
//!
//! **켰을 때는 줄마다 파일을 열고 닫는다** — UI 스레드에서 도는 블로킹 I/O지만, 계측을 켠
//! 동안만 그렇고 재는 구간 **밖**에서 기록하므로 측정값 자체는 왜곡되지 않는다. 원인을
//! 가른 뒤에는 이 모듈째 걷어낸다.
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

static ENABLED: OnceLock<bool> = OnceLock::new();
static START: OnceLock<Instant> = OnceLock::new();

/// 계측이 켜져 있는가 — 환경변수를 한 번만 읽는다
pub fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        std::env::var_os("MOA_PERF_LOG").is_some_and(|value| value != "0" && !value.is_empty())
    })
}

/// 한 줄 남긴다 — 꺼져 있으면 `make`를 부르지 않는다.
///
/// 실패(경로를 얻지 못함·열지 못함)는 조용히 지나간다. 계측이 실행을 막아서는 안 된다
pub fn log(make: impl FnOnce() -> String) {
    if !enabled() {
        return;
    }
    let since = START.get_or_init(Instant::now).elapsed().as_secs_f32();
    let Some(path) = log_path() else {
        return;
    };
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "[{since:9.3}] {}", make());
}

/// 느린 프레임 문턱 — 이 이상 걸린 프레임을 구간별로 남긴다 (2026-09-29)
pub const SLOW_FRAME: Duration = Duration::from_millis(100);

/// 한 프레임의 구간 시계 — 메뉴 밖에서 가끔 나는 UI 멈춤의 자리를 가르려고 둔다 (2026-09-29).
///
/// **계측이 꺼져 있으면 시각도 잡지 않는다** — `start`가 시작 시각을 두지 않으면 `mark`는
/// 아무 일도 하지 않는다. 구간 이름은 `'static` 문자열이라 기록하지 않는 프레임에 할당이 없다
#[derive(Default)]
pub struct FrameTimer {
    last: Option<Instant>,
    phases: Vec<(&'static str, Duration)>,
}

impl FrameTimer {
    /// 프레임을 시작한다 — 계측이 꺼져 있으면 빈 시계다
    pub fn start() -> FrameTimer {
        FrameTimer {
            last: enabled().then(Instant::now),
            phases: Vec::new(),
        }
    }

    /// 앞 표시부터 여기까지를 `name` 구간으로 적는다
    pub fn mark(&mut self, name: &'static str) {
        let Some(last) = self.last else {
            return;
        };
        let now = Instant::now();
        self.phases.push((name, now - last));
        self.last = Some(now);
    }

    /// 적은 구간들 — 계측이 꺼져 있으면 비어 있다
    pub fn phases(&self) -> &[(&'static str, Duration)] {
        &self.phases
    }
}

/// 구간 합이 문턱 이상이면 한 줄을 만든다 — `slow-frame total=… | 이름=…`(ms).
///
/// **경로·파일 이름을 담지 않는다** — 구간 이름과 시간뿐이다(이 모듈의 규칙)
pub fn slow_frame_line(phases: &[(&str, Duration)], threshold: Duration) -> Option<String> {
    let total: Duration = phases.iter().map(|(_, spent)| *spent).sum();
    if total < threshold {
        return None;
    }
    let ms = |d: Duration| d.as_secs_f32() * 1000.0;
    let parts: Vec<String> = phases
        .iter()
        .map(|(name, spent)| format!("{name}={:.1}", ms(*spent)))
        .collect();
    Some(format!(
        "slow-frame total={:.1} | {} (ms)",
        ms(total),
        parts.join(" ")
    ))
}

/// 기록 파일 자리 — 실행 파일 옆(`settings.json`과 같은 규칙)
fn log_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join("moa_perf.log"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(value: u64) -> Duration {
        Duration::from_millis(value)
    }

    #[test]
    fn 문턱에_못_미치면_줄을_만들지_않는다() {
        let phases = [("logic", ms(60)), ("ui", ms(39))];
        assert_eq!(slow_frame_line(&phases, SLOW_FRAME), None);
    }

    #[test]
    fn 문턱_이상이면_모든_구간을_적는다() {
        let phases = [("logic", ms(60)), ("central", ms(30)), ("menus", ms(10))];
        let line = slow_frame_line(&phases, SLOW_FRAME).expect("100ms면 적는다");
        assert!(line.starts_with("slow-frame total=100.0"), "{line}");
        for name in ["logic=60.0", "central=30.0", "menus=10.0"] {
            assert!(line.contains(name), "{name}가 빠졌다: {line}");
        }
    }

    #[test]
    fn 꺼진_시계는_구간을_적지_않는다() {
        let mut timer = FrameTimer::default();
        timer.mark("logic");
        assert!(timer.phases().is_empty());
    }
}
