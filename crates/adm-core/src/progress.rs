use std::collections::VecDeque;
use std::time::Instant;

/// Turns byte counters sampled every ~500 ms into a smoothed speed and an ETA.
pub struct SpeedTracker {
    last_at: Instant,
    last_bytes: u64,
    samples: VecDeque<f64>,
}

const WINDOW: usize = 6;

impl SpeedTracker {
    pub fn new(downloaded: u64) -> Self {
        Self { last_at: Instant::now(), last_bytes: downloaded, samples: VecDeque::with_capacity(WINDOW) }
    }

    /// `session_bytes` only ever grows during a run; it excludes bytes restored from a previous run.
    pub fn sample(&mut self, session_bytes: u64) -> f64 {
        let now = Instant::now();
        let dt = now.duration_since(self.last_at).as_secs_f64();
        if dt >= 0.05 {
            let instant = session_bytes.saturating_sub(self.last_bytes) as f64 / dt;
            if self.samples.len() == WINDOW {
                self.samples.pop_front();
            }
            self.samples.push_back(instant);
            self.last_at = now;
            self.last_bytes = session_bytes;
        }
        self.speed()
    }

    pub fn speed(&self) -> f64 {
        if self.samples.is_empty() {
            0.0
        } else {
            self.samples.iter().sum::<f64>() / self.samples.len() as f64
        }
    }
}

pub fn eta_secs(remaining: u64, speed: f64) -> Option<u64> {
    (speed >= 1.0).then(|| (remaining as f64 / speed).ceil() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eta() {
        assert_eq!(eta_secs(1000, 100.0), Some(10));
        assert_eq!(eta_secs(1000, 0.0), None);
    }
}
