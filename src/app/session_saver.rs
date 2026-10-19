//! 세션 저장 워커 — UI 스레드는 직렬화한 문자열만 넘기고 디스크 쓰기는 이 스레드가 한다.
//!
//! 저장은 임시 파일을 디스크에 내린 뒤 바꿔 끼운다(`fs::atomic_write`). 디스크에 내리는
//! 일(`sync_all`)이 디스크·백신 상태에 따라 수십~수백 ms가 걸려, 0.4초마다 도는 자동 저장을
//! UI 스레드에서 하면 그때마다 창이 멈춘다(AGENTS: UI 스레드 블로킹 I/O 금지).
//!
//! **쌓인 것 중 마지막 것만 쓴다** — 쓰는 동안 새 상태가 여럿 오면 앞의 것은 이미 낡았다.
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

/// 직렬화한 세션을 실제로 적는 함수 — 제품은 `settings::write_session_file`, 시험은 기록용 함수다
pub type WriteFn = fn(&str) -> std::io::Result<()>;

pub struct SessionSaver {
    /// `None`이면 워커가 없다 — 띄우지 못했거나 이미 `finish`했다. 그때는 그 자리에서 쓴다
    tx: Option<Sender<String>>,
    worker: Option<JoinHandle<()>>,
    write: WriteFn,
}

impl SessionSaver {
    /// 저장 워커를 띄운다. 띄우지 못하면 `save`가 부르는 쪽 스레드에서 곧바로 쓴다
    pub fn spawn(write: WriteFn) -> SessionSaver {
        let (tx, rx) = channel::<String>();
        let worker = std::thread::Builder::new()
            .name("session-saver".to_owned())
            .spawn(move || run(&rx, write))
            .ok();
        SessionSaver {
            tx: worker.as_ref().map(|_| tx),
            worker,
            write,
        }
    }

    /// 세션을 직렬화해 워커에 넘긴다 — 쓰기를 기다리지 않는다. 실패는 조용히 넘긴다
    /// (다음 변화가 생기면 다시 쓰인다 — 옛 파일은 온전히 남는다)
    pub fn save(&self, session: &super::settings::Session) {
        let Some(json) = super::settings::session_json(session) else {
            return;
        };
        self.save_json(json);
    }

    fn save_json(&self, json: String) {
        let unsent = match &self.tx {
            Some(tx) => tx.send(json).err().map(|error| error.0),
            None => Some(json),
        };
        // 워커가 없거나 이미 죽었다 — 저장을 잃지 않게 이 자리에서 쓴다
        if let Some(json) = unsent {
            let _ = (self.write)(&json);
        }
    }

    /// 넘긴 것을 다 쓸 때까지 기다린다 — 앱이 닫힐 때 부른다. 그 뒤의 `save`는 그 자리에서 쓴다
    pub fn finish(&mut self) {
        // 송신부를 닫아야 워커의 `recv`가 끝난다
        self.tx = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for SessionSaver {
    fn drop(&mut self) {
        self.finish();
    }
}

/// 워커 본체 — 하나를 받으면 그 사이 쌓인 것까지 비워 마지막 것만 쓴다
fn run(rx: &Receiver<String>, write: WriteFn) {
    while let Ok(mut json) = rx.recv() {
        while let Ok(newer) = rx.try_recv() {
            json = newer;
        }
        let _ = write(&json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `WriteFn`은 함수 포인터라 상태를 붙잡지 못한다 — 시험마다 전역 기록을 따로 쓴다
    static WRITTEN_LAST: Mutex<Vec<String>> = Mutex::new(Vec::new());
    static WRITTEN_GATE: Mutex<Vec<String>> = Mutex::new(Vec::new());
    /// 막힘 시험의 문 — 열릴 때까지 쓰는 함수가 기다린다
    static GATE: Mutex<Option<Receiver<()>>> = Mutex::new(None);

    fn record_last(json: &str) -> std::io::Result<()> {
        WRITTEN_LAST.lock().expect("기록").push(json.to_owned());
        Ok(())
    }

    fn record_after_gate(json: &str) -> std::io::Result<()> {
        // 잠금을 먼저 놓는다 — `if let` 조건의 잠금은 그 블록이 끝날 때까지 살아 있어,
        // 문 앞에서 기다리는 동안 시험 쪽이 같은 잠금을 기다리며 서로 멈춘다
        let gate = GATE.lock().expect("문").take();
        if let Some(gate) = gate {
            let _ = gate.recv();
        }
        WRITTEN_GATE.lock().expect("기록").push(json.to_owned());
        Ok(())
    }

    #[test]
    fn 여러_번_넘기고_끝내면_마지막_것이_쓰인다() {
        let mut saver = SessionSaver::spawn(record_last);
        for index in 0..20 {
            saver.save_json(format!("state-{index}"));
        }
        saver.finish();
        let written = WRITTEN_LAST.lock().expect("기록").clone();
        assert_eq!(
            written.last().map(String::as_str),
            Some("state-19"),
            "마지막 상태가 쓰이지 않았다"
        );
    }

    #[test]
    fn 쓰기가_막혀_있어도_save는_곧_돌아온다() {
        // UI 스레드가 디스크를 기다리지 않는다는 것을 잰다 — 첫 쓰기를 문으로 막아 둔다
        let (open, gate) = channel();
        *GATE.lock().expect("문") = Some(gate);
        let mut saver = SessionSaver::spawn(record_after_gate);
        saver.save_json("first".to_owned());
        // 워커가 첫 것을 받아 문 앞에서 기다리는 동안 둘을 더 넘긴다 — 여기서 막히면 시험이 멈춘다
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while GATE.lock().expect("문").is_some() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        saver.save_json("second".to_owned());
        saver.save_json("third".to_owned());
        assert!(
            WRITTEN_GATE.lock().expect("기록").is_empty(),
            "막혀 있어야 할 쓰기가 끝났다"
        );
        let _ = open.send(());
        saver.finish();
        let written = WRITTEN_GATE.lock().expect("기록").clone();
        assert_eq!(
            written,
            vec!["first".to_owned(), "third".to_owned()],
            "쌓인 것 중 마지막 것만 써야 한다"
        );
    }
}
