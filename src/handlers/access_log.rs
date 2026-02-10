use crate::events::RequestComplete;
use crate::events::{EventHandler, Priority, Propagation};

/// Emits a structured access log entry via `tracing::info!`.
pub struct AccessLogHandler;

impl EventHandler<RequestComplete> for AccessLogHandler {
    fn handle(&self, event: &mut RequestComplete) -> Propagation {
        tracing::info!(
            target: "access_log",
            request_id = %event.request_id,
            method = %event.method,
            path = %event.path,
            status = event.status,
            duration_us = event.duration.as_micros() as u64,
            remote_addr = %event.remote_addr,
            "request completed"
        );
        Propagation::Continue
    }

    fn priority(&self) -> Priority {
        100
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventHandler;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::time::Duration;

    #[test]
    fn test_access_log_handler() {
        let handler = AccessLogHandler;
        let mut event = RequestComplete {
            request_id: "test123".to_string(),
            method: "GET".to_string(),
            path: "/".to_string(),
            status: 200,
            duration: Duration::from_micros(500),
            remote_addr: SocketAddr::new(Ipv4Addr::new(127, 0, 0, 1).into(), 8080),
        };

        // Should not panic
        let result = handler.handle(&mut event);
        assert_eq!(result, Propagation::Continue);
    }

    #[test]
    fn test_priority() {
        assert_eq!(AccessLogHandler.priority(), 100);
    }
}
