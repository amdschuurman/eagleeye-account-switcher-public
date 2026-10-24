//! Time behind a trait so the switch/add timeouts are testable without real waiting.

use std::time::{SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync {
    /// Milliseconds since the UNIX epoch (for log cutoff and `lastUsed`).
    fn now_ms(&self) -> u64;
    /// A monotonic-ish millisecond counter for measuring elapsed time within one operation.
    fn elapsed_ms(&self) -> u64;
    /// Sleep (polling between log reads). Fakes return immediately.
    fn sleep_ms(&self, ms: u64);
}

/// The real clock.
pub struct SystemClock {
    start: std::time::Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock { start: std::time::Instant::now() }
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

#[cfg(test)]
pub mod test_support {
    use super::Clock;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A clock whose `elapsed_ms` advances by a fixed step on every `sleep_ms`, so polling loops
    /// terminate deterministically.
    pub struct FakeClock {
        now: u64,
        elapsed: AtomicU64,
        step: u64,
    }

    impl FakeClock {
        pub fn new(now_ms: u64, step_ms: u64) -> Self {
            FakeClock { now: now_ms, elapsed: AtomicU64::new(0), step: step_ms }
        }
    }

    impl Clock for FakeClock {
        fn now_ms(&self) -> u64 {
            self.now + self.elapsed.load(Ordering::SeqCst)
        }
        fn elapsed_ms(&self) -> u64 {
            self.elapsed.load(Ordering::SeqCst)
        }
        fn sleep_ms(&self, _ms: u64) {
            self.elapsed.fetch_add(self.step, Ordering::SeqCst);
        }
    }
}
