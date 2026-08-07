//! A small per-key sliding-window limiter.
//!
//! Used for the private-chat NSFW test, where a single user could otherwise
//! pin the detector by spamming images.

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use moka::future::Cache;

#[derive(Clone)]
pub struct RateLimiter {
    windows: Cache<i64, Arc<Mutex<Vec<Instant>>>>,
    limit: usize,
    window: Duration,
}

impl RateLimiter {
    pub fn new(limit: u32, window: Duration) -> Self {
        Self {
            windows: Cache::builder()
                // Entries idle for two windows can never affect a decision.
                .time_to_idle(window * 2)
                .max_capacity(50_000)
                .build(),
            limit: limit.max(1) as usize,
            window,
        }
    }

    /// Record an attempt. `Ok(())` means allowed; `Err(retry_after)` means the
    /// caller should back off for that long.
    pub async fn check(&self, key: i64) -> Result<(), Duration> {
        let slot = self
            .windows
            .get_with(key, async { Arc::new(Mutex::new(Vec::new())) })
            .await;

        let now = Instant::now();
        let mut hits = slot.lock().expect("rate limiter mutex poisoned");
        hits.retain(|t| now.duration_since(*t) < self.window);

        if hits.len() >= self.limit {
            let oldest = hits[0];
            return Err(self.window.saturating_sub(now.duration_since(oldest)));
        }

        hits.push(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn allows_up_to_the_limit_then_blocks() {
        let rl = RateLimiter::new(3, Duration::from_secs(60));

        for _ in 0..3 {
            assert!(rl.check(1).await.is_ok());
        }
        assert!(rl.check(1).await.is_err());
    }

    #[tokio::test]
    async fn keys_are_independent() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));

        assert!(rl.check(1).await.is_ok());
        assert!(rl.check(1).await.is_err());
        assert!(rl.check(2).await.is_ok());
    }

    #[tokio::test]
    async fn window_expiry_frees_capacity() {
        let rl = RateLimiter::new(1, Duration::from_millis(40));

        assert!(rl.check(7).await.is_ok());
        assert!(rl.check(7).await.is_err());
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(rl.check(7).await.is_ok());
    }
}
