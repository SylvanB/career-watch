//! IP-based rate limiting middleware.
//!
//! Uses a sliding window counter stored in a DashMap.
//! Keyed by (client IP, route path) so different endpoints have
//! independent limits.
//!
//! Applied to auth endpoints to prevent brute-force attacks.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response};
use dashmap::DashMap;

/// Shared rate limiter state. Clone-cheap (wraps an Arc).
#[derive(Clone)]
pub struct RateLimiter {
    /// (ip, path) → timestamps of recent requests within the window
    buckets: Arc<DashMap<(String, String), Vec<Instant>>>,
    max_requests: usize,
    window: Duration,
}

impl RateLimiter {
    /// `max_requests` allowed per `window` per (IP, path) pair.
    pub fn new(max_requests: usize, window: Duration) -> Self {
        Self {
            buckets: Arc::new(DashMap::new()),
            max_requests,
            window,
        }
    }

    /// Returns `true` if the request should be allowed, `false` if it should
    /// be rejected. Records the request on allow.
    pub fn check(&self, ip: String, path: String) -> bool {
        let now = Instant::now();
        let cutoff = now - self.window;
        let mut bucket = self.buckets.entry((ip, path)).or_default();
        bucket.retain(|&t| t > cutoff);
        if bucket.len() >= self.max_requests {
            return false;
        }
        bucket.push(now);
        true
    }
}

/// Extract the client IP from standard proxy headers, falling back to a
/// placeholder when none are present (e.g. local dev without a proxy).
fn client_ip(req: &Request) -> String {
    req.headers()
        .get("x-forwarded-for")
        .or_else(|| req.headers().get("x-real-ip"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Axum middleware — rejects with 429 when the rate limit is exceeded.
pub async fn rate_limit(
    axum::extract::State(limiter): axum::extract::State<RateLimiter>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let ip = client_ip(&req);
    let path = req.uri().path().to_string();

    if !limiter.check(ip, path) {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    Ok(next.run(req).await)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn limiter(max: usize, secs: u64) -> RateLimiter {
        RateLimiter::new(max, Duration::from_secs(secs))
    }

    #[test]
    fn allows_requests_under_limit() {
        let l = limiter(5, 60);
        for _ in 0..5 {
            assert!(l.check("1.2.3.4".into(), "/auth/login".into()));
        }
    }

    #[test]
    fn blocks_on_limit_exceeded() {
        let l = limiter(3, 60);
        for _ in 0..3 {
            assert!(l.check("1.2.3.4".into(), "/auth/login".into()));
        }
        assert!(
            !l.check("1.2.3.4".into(), "/auth/login".into()),
            "4th request should be blocked"
        );
    }

    #[test]
    fn different_ips_are_independent() {
        let l = limiter(1, 60);
        assert!(l.check("1.1.1.1".into(), "/auth/login".into()));
        assert!(!l.check("1.1.1.1".into(), "/auth/login".into())); // blocked
        assert!(l.check("2.2.2.2".into(), "/auth/login".into())); // different IP, allowed
    }

    #[test]
    fn different_paths_are_independent() {
        let l = limiter(1, 60);
        assert!(l.check("1.2.3.4".into(), "/auth/login".into()));
        assert!(!l.check("1.2.3.4".into(), "/auth/login".into())); // blocked
        assert!(l.check("1.2.3.4".into(), "/auth/register".into())); // different path, allowed
    }

    #[test]
    fn window_eviction_allows_new_requests() {
        // Use a tiny window so we can test expiry without sleeping
        let l = limiter(2, 0); // 0-second window = always expired
        assert!(l.check("1.2.3.4".into(), "/".into()));
        assert!(l.check("1.2.3.4".into(), "/".into()));
        // With a 0-second window, old entries are always evicted → never blocked
        assert!(l.check("1.2.3.4".into(), "/".into()));
        assert!(l.check("1.2.3.4".into(), "/".into()));
    }

    #[test]
    fn burst_of_exact_limit_allowed() {
        let l = limiter(10, 60);
        let results: Vec<bool> = (0..10)
            .map(|_| l.check("host".into(), "/".into()))
            .collect();
        assert!(results.iter().all(|&r| r), "all 10 should be allowed");
        assert!(
            !l.check("host".into(), "/".into()),
            "11th should be blocked"
        );
    }
}
