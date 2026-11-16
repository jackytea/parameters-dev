use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use parameters_engine::Sandbox;

const PRUNE_ABOVE: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub sandbox: Sandbox,
    pub max_steps: Option<usize>,
    pub max_run_time: Option<Duration>,
    pub max_active_runs: Option<i64>,
    pub requests_per_minute: Option<u32>,
}

impl Capabilities {
    pub const LOCAL: Self = Self {
        sandbox: Sandbox::OPEN,
        max_steps: None,
        max_run_time: None,
        max_active_runs: None,
        requests_per_minute: None,
    };

    pub const MANAGED: Self = Self {
        sandbox: Sandbox::MANAGED,
        max_steps: Some(500),
        max_run_time: Some(Duration::from_mins(15)),
        max_active_runs: Some(5),
        requests_per_minute: Some(600),
    };
}

struct Bucket {
    tokens: f64,
    updated: Instant,
}

#[derive(Default)]
pub(crate) struct RateLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    pub(crate) fn allow(&self, key: &str, capacity: u32, period: Duration) -> bool {
        let now = Instant::now();
        let capacity = f64::from(capacity);
        let rate = capacity / period.as_secs_f64();
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        if buckets.len() > PRUNE_ABOVE {
            buckets.retain(|_, bucket| {
                bucket.tokens + now.duration_since(bucket.updated).as_secs_f64() * rate < capacity
            });
        }
        let bucket = buckets.entry(key.to_owned()).or_insert(Bucket {
            tokens: capacity,
            updated: now,
        });
        bucket.tokens =
            (bucket.tokens + now.duration_since(bucket.updated).as_secs_f64() * rate).min(capacity);
        bucket.updated = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets_allow_bursts_up_to_capacity_per_key() {
        let limiter = RateLimiter::default();
        let hour = Duration::from_secs(3_600);
        assert!((0..3).all(|_| limiter.allow("a", 3, hour)));
        assert!(!limiter.allow("a", 3, hour));
        assert!(limiter.allow("b", 3, hour));
        assert!(limiter.allow("fast", 1, Duration::from_nanos(1)));
        assert!(limiter.allow("fast", 1, Duration::from_nanos(1)));
    }
}
