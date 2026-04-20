use std::sync::Arc;

use crate::events::RequestReceived;
use crate::events::{EventHandler, Priority, Propagation};
use crate::metrics::Metrics;
use crate::server::rate_limit::RateLimiter;

/// Wraps `RateLimiter::check_rate_limited()` as an event handler.
/// Sets `early_response` on the event if the request is rate-limited.
pub struct RateLimitHandler {
    limiter: Arc<RateLimiter>,
    metrics: Arc<Metrics>,
}

impl RateLimitHandler {
    pub fn new(limiter: Arc<RateLimiter>, metrics: Arc<Metrics>) -> Self {
        Self { limiter, metrics }
    }
}

impl EventHandler<RequestReceived> for RateLimitHandler {
    #[inline]
    fn handle(&self, event: &mut RequestReceived) -> Propagation {
        if let Some(resp) = self
            .limiter
            .check_rate_limited(event.remote_addr.ip(), &event.request_id)
        {
            self.metrics.rate_limited();
            event.early_response = Some(resp);
        }
        Propagation::Continue
    }

    fn priority(&self) -> Priority {
        -50
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventHandler;
    use http::Method;
    use std::net::{Ipv4Addr, SocketAddr};
    fn make_event() -> RequestReceived {
        let (parts, _) = http::Request::builder()
            .method(Method::GET)
            .uri("/test")
            .body(())
            .unwrap()
            .into_parts();

        RequestReceived {
            parts,
            remote_addr: SocketAddr::new(Ipv4Addr::new(127, 0, 0, 1).into(), 8080),
            request_id: "test123".to_string(),
            early_response: None,
            metadata: Vec::new(),
            profiling_mode: None,
            profiling_run_id: None,
        }
    }

    fn make_handler(limit: u32, window: u64) -> RateLimitHandler {
        RateLimitHandler::new(
            Arc::new(RateLimiter::new(limit, window)),
            Arc::new(Metrics::new()),
        )
    }

    #[test]
    fn test_allows_under_limit() {
        let handler = make_handler(10, 60);

        let mut event = make_event();
        let result = handler.handle(&mut event);
        assert_eq!(result, Propagation::Continue);
        assert!(event.early_response.is_none());
    }

    #[test]
    fn test_blocks_over_limit() {
        let metrics = Arc::new(Metrics::new());
        let handler =
            RateLimitHandler::new(Arc::new(RateLimiter::new(1, 60)), Arc::clone(&metrics));

        // First request is allowed
        let mut event = make_event();
        handler.handle(&mut event);
        assert!(event.early_response.is_none());

        // Second request is rate-limited (early_response set, but Continue returned)
        let mut event = make_event();
        let result = handler.handle(&mut event);
        assert_eq!(result, Propagation::Continue);
        assert!(event.early_response.is_some());
        assert_eq!(
            event.early_response.unwrap().status(),
            http::StatusCode::TOO_MANY_REQUESTS
        );
        let prom = metrics.to_prometheus();
        assert!(prom.contains("oxphp_rate_limited_total 1"));
    }

    #[test]
    fn test_priority() {
        let handler = make_handler(10, 60);
        assert_eq!(handler.priority(), -50);
    }
}
