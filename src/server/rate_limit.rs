use std::net::IpAddr;
use std::time::Instant;

use bytes::Bytes;
use dashmap::DashMap;
use http::{HeaderValue, Response, StatusCode};

use crate::types::{full_body, ResponseBody};

/// Maximum number of tracked IPs before forced eviction.
/// Prevents unbounded memory growth under IP rotation attacks.
const MAX_TRACKED_IPS: usize = 100_000;

pub struct RateLimiter {
    limits: DashMap<IpAddr, (u32, Instant)>,
    max_requests: u32,
    window_secs: u64,
}

impl RateLimiter {
    pub fn new(max_requests: u32, window_secs: u64) -> Self {
        Self {
            limits: DashMap::new(),
            max_requests,
            window_secs,
        }
    }

    /// Check if the IP is rate limited. Returns `Some(Response)` with 429 if exceeded.
    pub fn check_rate_limited(
        &self,
        ip: IpAddr,
        request_id: &str,
    ) -> Option<Response<ResponseBody>> {
        // Hard cap check BEFORE acquiring entry lock to prevent OOM under IP rotation
        if self.limits.len() > MAX_TRACKED_IPS {
            self.cleanup();
        }

        let now = Instant::now();
        let mut entry = self.limits.entry(ip).or_insert((0, now));
        let (count, window_start) = entry.value_mut();

        // Reset window if expired
        if now.duration_since(*window_start).as_secs() >= self.window_secs {
            *count = 0;
            *window_start = now;
        }

        *count += 1;

        if *count > self.max_requests {
            let remaining = 0u32;
            let reset_secs = self
                .window_secs
                .saturating_sub(now.duration_since(*window_start).as_secs());

            let mut response = Response::builder()
                .status(StatusCode::TOO_MANY_REQUESTS)
                .header("retry-after", reset_secs.to_string())
                .header("x-ratelimit-limit", self.max_requests.to_string())
                .header("x-ratelimit-remaining", remaining.to_string())
                .header("x-ratelimit-reset", reset_secs.to_string())
                .body(full_body(Bytes::from_static(b"429 Too Many Requests")))
                .unwrap();

            if let Ok(hv) = HeaderValue::from_str(request_id) {
                response.headers_mut().insert("x-request-id", hv);
            }

            Some(response)
        } else {
            None
        }
    }

    /// Remove expired entries. Called periodically from a background task.
    pub fn cleanup(&self) {
        let now = Instant::now();
        self.limits.retain(|_, (_, window_start)| {
            now.duration_since(*window_start).as_secs() < self.window_secs * 2
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_allows_under_limit() {
        let limiter = RateLimiter::new(5, 60);
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));

        for _ in 0..5 {
            assert!(limiter.check_rate_limited(ip, "test").is_none());
        }
    }

    #[test]
    fn test_blocks_over_limit() {
        let limiter = RateLimiter::new(3, 60);
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));

        for _ in 0..3 {
            assert!(limiter.check_rate_limited(ip, "test").is_none());
        }

        let resp = limiter.check_rate_limited(ip, "test");
        assert!(resp.is_some());
        let resp = resp.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(resp.headers().contains_key("retry-after"));
        assert!(resp.headers().contains_key("x-ratelimit-limit"));
        assert!(resp.headers().contains_key("x-ratelimit-remaining"));
    }

    #[test]
    fn test_different_ips_independent() {
        let limiter = RateLimiter::new(2, 60);
        let ip1 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1));
        let ip2 = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2));

        // Exhaust ip1
        limiter.check_rate_limited(ip1, "t");
        limiter.check_rate_limited(ip1, "t");
        assert!(limiter.check_rate_limited(ip1, "t").is_some());

        // ip2 should still be allowed
        assert!(limiter.check_rate_limited(ip2, "t").is_none());
    }

    #[test]
    fn test_cleanup() {
        let limiter = RateLimiter::new(10, 0); // 0-sec window = always expired
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        limiter.check_rate_limited(ip, "t");
        assert_eq!(limiter.limits.len(), 1);
        limiter.cleanup();
        assert_eq!(limiter.limits.len(), 0);
    }
}
