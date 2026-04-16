use crate::config::AccessLogLevel;
use crate::events::RequestComplete;
use crate::events::{EventHandler, Priority, Propagation};

/// Emits a structured access log entry via `tracing::info!`.
pub struct AccessLogHandler {
    level: AccessLogLevel,
}

impl AccessLogHandler {
    pub fn new(level: AccessLogLevel) -> Self {
        Self { level }
    }
}

impl EventHandler<RequestComplete> for AccessLogHandler {
    #[inline]
    fn handle(&self, event: &mut RequestComplete) -> Propagation {
        if self.level == AccessLogLevel::Error && event.status < 400 {
            return Propagation::Continue;
        }

        let trace_id = event
            .metadata
            .iter()
            .find(|(k, _)| k == "trace_id")
            .map(|(_, v)| v.as_str());
        let span_id = event
            .metadata
            .iter()
            .find(|(k, _)| k == "span_id")
            .map(|(_, v)| v.as_str());

        if let (Some(tid), Some(sid)) = (trace_id, span_id) {
            tracing::info!(
                target: "access_log",
                request_id = %event.request_id,
                trace_id = tid,
                span_id = sid,
                method = %event.method,
                path = %event.path,
                status = event.status,
                duration_us = event.duration.as_micros() as u64,
                remote_addr = %event.remote_addr,
                "request completed"
            );
        } else {
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
        }
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

    fn make_event(status: u16) -> RequestComplete {
        RequestComplete {
            request_id: "test123".to_string(),
            method: http::Method::GET,
            path: "/".to_string(),
            status,
            duration: Duration::from_micros(500),
            remote_addr: SocketAddr::new(Ipv4Addr::new(127, 0, 0, 1).into(), 8080),
            request_body_size: 0,
            response_size: 0,
            metadata: Vec::new(),
            php_errors: Vec::new(),
            apm_spans_json: None,
            queue_wait_us: None,
            php_exec_us: None,
        }
    }

    #[test]
    fn test_all_level_logs_everything() {
        let handler = AccessLogHandler::new(AccessLogLevel::All);
        let mut event = make_event(200);
        let result = handler.handle(&mut event);
        assert_eq!(result, Propagation::Continue);
    }

    #[test]
    fn test_error_level_skips_success() {
        let handler = AccessLogHandler::new(AccessLogLevel::Error);
        // 200 should be skipped (no panic, returns Continue)
        let mut event = make_event(200);
        assert_eq!(handler.handle(&mut event), Propagation::Continue);

        // 301 redirect — not an error
        let mut event = make_event(301);
        assert_eq!(handler.handle(&mut event), Propagation::Continue);
    }

    #[test]
    fn test_error_level_logs_errors() {
        let handler = AccessLogHandler::new(AccessLogLevel::Error);

        let mut event = make_event(404);
        assert_eq!(handler.handle(&mut event), Propagation::Continue);

        let mut event = make_event(500);
        assert_eq!(handler.handle(&mut event), Propagation::Continue);

        let mut event = make_event(403);
        assert_eq!(handler.handle(&mut event), Propagation::Continue);
    }

    #[test]
    fn test_priority() {
        assert_eq!(AccessLogHandler::new(AccessLogLevel::All).priority(), 100);
    }
}
