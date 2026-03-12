use std::sync::Arc;

use crate::events::{EventHandler, Priority, Propagation};
use crate::events::{RequestComplete, RequestReceived};
use crate::metrics::Metrics;

/// Records incoming request metrics (`record_request()`).
pub struct MetricsRequestHandler {
    metrics: Arc<Metrics>,
}

impl MetricsRequestHandler {
    pub fn new(metrics: Arc<Metrics>) -> Self {
        Self { metrics }
    }
}

impl EventHandler<RequestReceived> for MetricsRequestHandler {
    #[inline]
    fn handle(&self, event: &mut RequestReceived) -> Propagation {
        self.metrics.record_request(&event.parts.method);
        Propagation::Continue
    }

    fn priority(&self) -> Priority {
        0
    }
}

/// Records response metrics (`record_response()`).
pub struct MetricsResponseHandler {
    metrics: Arc<Metrics>,
}

impl MetricsResponseHandler {
    pub fn new(metrics: Arc<Metrics>) -> Self {
        Self { metrics }
    }
}

impl EventHandler<RequestComplete> for MetricsResponseHandler {
    #[inline]
    fn handle(&self, event: &mut RequestComplete) -> Propagation {
        self.metrics.record_response(
            event.status,
            event.duration,
            event.request_body_size,
            event.response_size,
        );
        Propagation::Continue
    }

    fn priority(&self) -> Priority {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventHandler;
    use http::Method;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::time::Duration;

    #[test]
    fn test_metrics_request_handler() {
        let metrics = Arc::new(Metrics::new());
        let handler = MetricsRequestHandler::new(Arc::clone(&metrics));

        let (parts, _) = http::Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(())
            .unwrap()
            .into_parts();

        let mut event = RequestReceived {
            parts,
            remote_addr: SocketAddr::new(Ipv4Addr::new(127, 0, 0, 1).into(), 8080),
            request_id: "test".to_string(),
            early_response: None,
            metadata: Vec::new(),
        };

        let result = handler.handle(&mut event);
        assert_eq!(result, Propagation::Continue);
        assert_eq!(metrics.total_requests(), 1);
    }

    #[test]
    fn test_metrics_response_handler() {
        let metrics = Arc::new(Metrics::new());
        let handler = MetricsResponseHandler::new(Arc::clone(&metrics));

        let mut event = RequestComplete {
            request_id: "test".to_string(),
            method: http::Method::GET,
            path: "/".to_string(),
            status: 200,
            duration: Duration::from_micros(500),
            remote_addr: SocketAddr::new(Ipv4Addr::new(127, 0, 0, 1).into(), 8080),
            request_body_size: 100,
            response_size: 500,
        };

        let result = handler.handle(&mut event);
        assert_eq!(result, Propagation::Continue);

        // Check that 2xx counter was incremented
        let prom = metrics.to_prometheus();
        assert!(prom.contains("oxphp_responses_by_status_total{status=\"2xx\"} 1"));
    }
}
