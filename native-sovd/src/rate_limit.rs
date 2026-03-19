// Copyright (c) 2024 Contributors to the Eclipse OpenSOVD project
// SPDX-License-Identifier: Apache-2.0

// ─────────────────────────────────────────────────────────────────────────────
// Per-client rate limiter (A2.5) — token-bucket per caller identity
//
// Each authenticated client gets an independent bucket with configurable
// capacity and refill rate. Anonymous/unauthenticated clients share a single
// "anonymous" bucket. Expired buckets are reaped periodically.
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use serde::Deserialize;

/// Configuration for per-client rate limiting.
#[derive(Debug, Clone, Deserialize)]
pub struct RateLimitConfig {
    /// Whether rate limiting is enabled (default: false)
    #[serde(default)]
    pub enabled: bool,
    /// Maximum requests per window (token bucket capacity)
    #[serde(default = "RateLimitConfig::default_max_requests")]
    pub max_requests: u32,
    /// Window duration in seconds (refill interval)
    #[serde(default = "RateLimitConfig::default_window_secs")]
    pub window_secs: u64,
}

impl RateLimitConfig {
    fn default_max_requests() -> u32 {
        100
    }
    fn default_window_secs() -> u64 {
        60
    }
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_requests: Self::default_max_requests(),
            window_secs: Self::default_window_secs(),
        }
    }
}

/// Per-client token bucket state.
struct Bucket {
    tokens: u32,
    last_refill: Instant,
}

/// Shared rate limiter state, safe for concurrent access.
#[derive(Clone)]
pub struct RateLimiter {
    buckets: Arc<DashMap<String, Bucket>>,
    max_tokens: u32,
    window: Duration,
}

impl RateLimiter {
    /// Create a new rate limiter from configuration.
    pub fn new(config: &RateLimitConfig) -> Self {
        Self {
            buckets: Arc::new(DashMap::new()),
            max_tokens: config.max_requests,
            window: Duration::from_secs(config.window_secs),
        }
    }

    /// Try to acquire a token for the given client. Returns `true` if allowed,
    /// `false` if the client has exceeded their rate limit.
    pub fn check(&self, client_id: &str) -> bool {
        let now = Instant::now();
        let mut entry = self
            .buckets
            .entry(client_id.to_owned())
            .or_insert_with(|| Bucket {
                tokens: self.max_tokens,
                last_refill: now,
            });

        let bucket = entry.value_mut();

        // Refill tokens if window has elapsed
        let elapsed = now.duration_since(bucket.last_refill);
        if elapsed >= self.window {
            let windows_elapsed = elapsed.as_secs() / self.window.as_secs().max(1);
            #[allow(clippy::cast_possible_truncation)]
            let refill = (windows_elapsed as u32).saturating_mul(self.max_tokens);
            bucket.tokens = (bucket.tokens.saturating_add(refill)).min(self.max_tokens);
            bucket.last_refill = now;
        }

        // Try to consume a token
        if bucket.tokens > 0 {
            bucket.tokens -= 1;
            true
        } else {
            false
        }
    }

    /// Remove stale buckets that haven't been used for more than 2× the window.
    /// Called periodically (e.g. from a background task or lazy reap).
    pub fn reap_stale(&self) {
        let cutoff = Instant::now()
            .checked_sub(self.window * 2)
            .unwrap_or_else(Instant::now);
        self.buckets.retain(|_, bucket| bucket.last_refill > cutoff);
    }

    /// Number of tracked clients.
    pub fn client_count(&self) -> usize {
        self.buckets.len()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn test_limiter(max: u32, window_secs: u64) -> RateLimiter {
        RateLimiter::new(&RateLimitConfig {
            enabled: true,
            max_requests: max,
            window_secs,
        })
    }

    #[test]
    fn allows_up_to_max_requests() {
        let limiter = test_limiter(3, 60);
        assert!(limiter.check("alice"));
        assert!(limiter.check("alice"));
        assert!(limiter.check("alice"));
        // 4th request should be denied
        assert!(!limiter.check("alice"));
    }

    #[test]
    fn different_clients_have_independent_buckets() {
        let limiter = test_limiter(2, 60);
        assert!(limiter.check("alice"));
        assert!(limiter.check("alice"));
        assert!(!limiter.check("alice"));

        // Bob should still have his full quota
        assert!(limiter.check("bob"));
        assert!(limiter.check("bob"));
        assert!(!limiter.check("bob"));
    }

    #[test]
    fn tracks_client_count() {
        let limiter = test_limiter(10, 60);
        limiter.check("alice");
        limiter.check("bob");
        limiter.check("charlie");
        assert_eq!(limiter.client_count(), 3);
    }

    #[test]
    fn reap_removes_stale_entries() {
        let limiter = test_limiter(10, 1);
        limiter.check("alice");
        assert_eq!(limiter.client_count(), 1);

        // Manually set last_refill to far in the past
        if let Some(mut entry) = limiter.buckets.get_mut("alice") {
            entry.last_refill = Instant::now().checked_sub(Duration::from_secs(10)).unwrap();
        }
        limiter.reap_stale();
        assert_eq!(limiter.client_count(), 0);
    }

    #[test]
    fn default_config_is_sane() {
        let config = RateLimitConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.max_requests, 100);
        assert_eq!(config.window_secs, 60);
    }
}
