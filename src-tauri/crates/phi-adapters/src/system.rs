use std::time::Instant;

use phi_kernel::ports::{ClockPort, IdPort, LogPort};

pub struct SystemClock {
    started: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
}

impl ClockPort for SystemClock {
    fn now_rfc3339(&self) -> String {
        chrono::Utc::now().to_rfc3339()
    }

    fn monotonic_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }
}

pub struct UuidIds;

impl IdPort for UuidIds {
    fn new_id(&self, prefix: &str) -> String {
        format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
    }
}

pub struct StderrLog;

impl LogPort for StderrLog {
    fn info(&self, msg: &str) {
        eprintln!("[phi][info] {msg}");
    }

    fn warn(&self, msg: &str) {
        eprintln!("[phi][warn] {msg}");
    }

    fn error(&self, msg: &str) {
        eprintln!("[phi][error] {msg}");
    }
}
