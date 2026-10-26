//! Failed-Claim throttling per client IP, and the global relay speed limit.

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(60);

/// Counts failed Claims per IP in fixed one-minute windows.
pub struct FailedClaims {
    max: u32,
    windows: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

impl FailedClaims {
    pub fn new(max_per_minute: u32) -> Self {
        Self {
            max: max_per_minute,
            windows: Mutex::default(),
        }
    }

    pub fn is_blocked(&self, ip: IpAddr) -> bool {
        let windows = self.windows.lock().unwrap();
        windows
            .get(&ip)
            .is_some_and(|&(start, n)| start.elapsed() < WINDOW && n >= self.max)
    }

    pub fn record(&self, ip: IpAddr) {
        let mut windows = self.windows.lock().unwrap();
        if windows.len() > 10_000 {
            windows.retain(|_, (start, _)| start.elapsed() < WINDOW);
        }
        let entry = windows.entry(ip).or_insert((Instant::now(), 0));
        if entry.0.elapsed() >= WINDOW {
            *entry = (Instant::now(), 0);
        }
        entry.1 += 1;
    }
}

/// A token bucket shared by every Transfer. The tokio mutex queues waiters in FIFO order,
/// so concurrent Transfers take turns and share the rate roughly evenly.
pub struct RateLimiter {
    bytes_per_sec: f64,
    state: tokio::sync::Mutex<(f64, tokio::time::Instant)>,
}

impl RateLimiter {
    pub fn new(bytes_per_sec: u64) -> Self {
        Self {
            bytes_per_sec: bytes_per_sec as f64,
            state: tokio::sync::Mutex::new((0.0, tokio::time::Instant::now())),
        }
    }

    pub async fn acquire(&self, bytes: usize) {
        let mut state = self.state.lock().await;
        let (tokens, last) = &mut *state;
        let now = tokio::time::Instant::now();
        // Allow at most a 100 ms burst after idling.
        *tokens = (*tokens + now.duration_since(*last).as_secs_f64() * self.bytes_per_sec)
            .min(self.bytes_per_sec / 10.0);
        *last = now;
        *tokens -= bytes as f64;
        if *tokens < 0.0 {
            tokio::time::sleep(Duration::from_secs_f64(-*tokens / self.bytes_per_sec)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_after_max_failures() {
        let fc = FailedClaims::new(3);
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        for _ in 0..3 {
            assert!(!fc.is_blocked(ip));
            fc.record(ip);
        }
        assert!(fc.is_blocked(ip));
        assert!(!fc.is_blocked("203.0.113.8".parse().unwrap()));
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limiter_paces_bytes() {
        let rl = RateLimiter::new(1000);
        let start = tokio::time::Instant::now();
        for _ in 0..5 {
            rl.acquire(1000).await;
        }
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(4900), "{elapsed:?}");
    }
}
