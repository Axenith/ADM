use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Shared bandwidth cap. Every segment of every download that holds the same limiter draws from one
/// budget, so a global cap and a per-download cap are the same mechanism. Changing the rate takes
/// effect on the next read; `0` means unlimited.
#[derive(Debug, Default)]
pub struct SpeedLimiter {
    bytes_per_sec: AtomicU64,
    /// Instant at which the budget for everything already granted runs out.
    next_free: Mutex<Option<Instant>>,
}

impl SpeedLimiter {
    pub fn new(bytes_per_sec: u64) -> Self {
        Self { bytes_per_sec: AtomicU64::new(bytes_per_sec), next_free: Mutex::new(None) }
    }

    pub fn set_limit(&self, bytes_per_sec: u64) {
        self.bytes_per_sec.store(bytes_per_sec, Ordering::Relaxed);
        *self.next_free.lock().unwrap() = None;
    }

    pub fn limit(&self) -> u64 {
        self.bytes_per_sec.load(Ordering::Relaxed)
    }

    /// How long the caller must wait before `n` more bytes may be read. Idle time is not banked, so
    /// a long pause never allows a burst above the cap afterwards.
    pub fn reserve(&self, n: u64) -> Duration {
        let rate = self.limit();
        if rate == 0 || n == 0 {
            return Duration::ZERO;
        }
        let now = Instant::now();
        let mut next = self.next_free.lock().unwrap();
        let start = next.map_or(now, |t| t.max(now));
        *next = Some(start + Duration::from_secs_f64(n as f64 / rate as f64));
        start.saturating_duration_since(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_never_waits() {
        assert_eq!(SpeedLimiter::new(0).reserve(1 << 30), Duration::ZERO);
    }

    #[test]
    fn grants_queue_up() {
        let l = SpeedLimiter::new(1000);
        assert_eq!(l.reserve(1000), Duration::ZERO); // first grant is free, books one second
        let w = l.reserve(1000);
        assert!(w > Duration::from_millis(900) && w <= Duration::from_secs(1), "{w:?}");
    }
}
