use crate::CancellationToken;
use atlas_shared::{AtlasError, AtlasResult};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Token bucket shared by all concurrent downloads.
pub struct RateLimiter {
    rate: f64,
    state: Mutex<(Instant, f64)>,
}

impl RateLimiter {
    pub fn new(bytes_per_sec: u64) -> Self {
        Self { rate: bytes_per_sec as f64, state: Mutex::new((Instant::now(), bytes_per_sec as f64)) }
    }

    pub async fn acquire(&self, n: u64, cancel: &CancellationToken) -> AtlasResult<()> {
        let wait = {
            let mut g = self.state.lock().await;
            let now = Instant::now();
            let refill = now.duration_since(g.0).as_secs_f64() * self.rate;
            g.0 = now;
            g.1 = (g.1 + refill).min(self.rate);
            g.1 -= n as f64;
            if g.1 < 0.0 { Duration::from_secs_f64(-g.1 / self.rate) } else { Duration::ZERO }
        };
        if !wait.is_zero() {
            tokio::select! {
                _ = cancel.cancelled() => return Err(AtlasError::cancelled()),
                _ = tokio::time::sleep(wait) => {}
            }
        }
        Ok(())
    }
}
