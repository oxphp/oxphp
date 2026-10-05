use crate::config::AccessLogLevel;
use crate::events::RequestComplete;
use crate::events::{EventHandler, Priority, Propagation};

/// Tracing target every access log entry is emitted under.
///
/// The log filter carries a directive naming this target, so that a quiet
/// `LOG_LEVEL` cannot silently switch the access log off; both sides read the
/// name from here rather than spelling it twice. Directives match a target by
/// prefix, so renaming this to something another target starts with would widen
/// what that directive frees.
pub const TARGET: &str = "access_log";

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
        // Absent from the entry when the request sent none.
        let user_agent = event
            .user_agent
            .as_ref()
            .map(crate::events::user_agent_text);

        if let (Some(tid), Some(sid)) = (trace_id, span_id) {
            tracing::info!(
                target: TARGET,
                request_id = %event.request_id,
                trace_id = tid,
                span_id = sid,
                method = %event.method,
                path = %event.path,
                status = event.status,
                duration_us = event.duration.as_micros() as u64,
                remote_ip = %event.remote_addr.ip(),
                user_agent = user_agent.as_deref(),
                "request completed"
            );
        } else {
            tracing::info!(
                target: TARGET,
                request_id = %event.request_id,
                method = %event.method,
                path = %event.path,
                status = event.status,
                duration_us = event.duration.as_micros() as u64,
                remote_ip = %event.remote_addr.ip(),
                user_agent = user_agent.as_deref(),
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
            profile_tree: None,
            queue_wait_us: None,
            php_exec_us: None,
            shed_reason: None,
            user_agent: None,
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

    /// Collects what the subscriber writes, so a test reads the entry as it
    /// comes out rather than the event it was built from.
    #[derive(Clone, Default)]
    struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
        type Writer = Captured;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// The `fields` object of the one entry the handler writes for `event`,
    /// formatted the way the server's own subscriber formats it.
    fn logged_fields(mut event: RequestComplete) -> serde_json::Value {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_writer(captured.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            AccessLogHandler::new(AccessLogLevel::All).handle(&mut event);
        });
        let out = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        let mut lines = out.lines();
        let entry: serde_json::Value =
            serde_json::from_str(lines.next().expect("no access log entry")).unwrap();
        assert_eq!(lines.next(), None, "one entry per request");
        entry["fields"].clone()
    }

    #[test]
    fn test_entry_carries_the_user_agent() {
        // With and without trace context: the two are written by separate
        // calls, and both must carry it.
        for traced in [false, true] {
            let mut event = make_event(200);
            event.user_agent = Some(http::HeaderValue::from_static("test-bot/1.0"));
            if traced {
                event.metadata = vec![
                    ("trace_id".into(), "4bf92f3577b34da6a3ce929d0e0e4736".into()),
                    ("span_id".into(), "00f067aa0ba902b7".into()),
                ];
            }
            let fields = logged_fields(event);
            assert_eq!(fields["user_agent"], "test-bot/1.0", "traced: {traced}");
            assert_eq!(fields["trace_id"].is_string(), traced, "{fields}");
        }
    }

    /// The control: a request that sent none has no such field, rather than an
    /// empty one that reads as "sent an empty header".
    #[test]
    fn test_entry_without_a_user_agent_has_no_field() {
        let fields = logged_fields(make_event(200));
        assert_eq!(fields["message"], "request completed", "{fields}");
        assert!(fields.get("user_agent").is_none(), "{fields}");
    }

    /// A field value may carry bytes outside ASCII; the entry keeps the value,
    /// with what is not UTF-8 replaced.
    #[test]
    fn test_entry_keeps_a_user_agent_that_is_not_ascii() {
        let mut event = make_event(200);
        event.user_agent = Some(http::HeaderValue::from_bytes(b"test-bot/1.0 \xff").unwrap());
        let fields = logged_fields(event);
        assert_eq!(fields["user_agent"], "test-bot/1.0 \u{FFFD}");
    }

    /// The client chooses the value's length, up to the size of the request
    /// head; the entry gets the same 512 bytes the trace span does.
    #[test]
    fn test_entry_user_agent_is_capped() {
        let mut event = make_event(200);
        event.user_agent = Some(http::HeaderValue::from_bytes(&[b'a'; 4096]).unwrap());
        let fields = logged_fields(event);
        let logged = fields["user_agent"].as_str().unwrap();
        assert!(logged.len() <= 512, "{} bytes", logged.len());
        assert!(logged.ends_with("…(truncated)"), "{logged:?}");
    }
}
