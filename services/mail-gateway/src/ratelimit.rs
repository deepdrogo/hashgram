//! Per-source-IP limits for the inbound listener.
//!
//! In memory on purpose: connection bursts are a property of *this*
//! process's uptime, and a restart forgetting them is harmless. Per-user
//! outbound limits, which must survive restarts to mean anything, live in
//! SQLite (`Store::rate_hit`).

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A sliding-window counter keyed by IP.
#[derive(Debug)]
pub struct IpLimiter {
    window: Duration,
    limit: u32,
    hits: Mutex<HashMap<IpAddr, Vec<Instant>>>,
}

impl IpLimiter {
    /// `limit` events per `window` per IP; `limit == 0` disables.
    #[must_use]
    pub fn new(window: Duration, limit: u32) -> Self {
        Self {
            window,
            limit,
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// Records an event and says whether it is within the limit.
    pub fn allow(&self, ip: IpAddr) -> bool {
        self.allow_at(ip, Instant::now())
    }

    fn allow_at(&self, ip: IpAddr, now: Instant) -> bool {
        if self.limit == 0 {
            return true;
        }
        let Ok(mut map) = self.hits.lock() else {
            // A poisoned lock means a panic elsewhere; failing open keeps
            // mail flowing, and the panic is already logged.
            return true;
        };
        // Bound the table: a scan attack from many addresses must not
        // grow it without limit.
        if map.len() > 10_000 {
            map.retain(|_, v| {
                v.last()
                    .is_some_and(|t| now.duration_since(*t) < self.window)
            });
            if map.len() > 10_000 {
                map.clear();
            }
        }
        let v = map.entry(ip).or_default();
        v.retain(|t| now.duration_since(*t) < self.window);
        if v.len() as u32 >= self.limit {
            return false;
        }
        v.push(now);
        true
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn sliding_window() {
        let l = IpLimiter::new(Duration::from_secs(60), 2);
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let other: IpAddr = "10.0.0.2".parse().unwrap();
        let t0 = Instant::now();
        assert!(l.allow_at(ip, t0));
        assert!(l.allow_at(ip, t0 + Duration::from_secs(1)));
        assert!(!l.allow_at(ip, t0 + Duration::from_secs(2)));
        assert!(l.allow_at(other, t0 + Duration::from_secs(2)));
        assert!(l.allow_at(ip, t0 + Duration::from_secs(61)));
    }

    #[test]
    fn zero_disables() {
        let l = IpLimiter::new(Duration::from_secs(60), 0);
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        for _ in 0..100 {
            assert!(l.allow(ip));
        }
    }
}
