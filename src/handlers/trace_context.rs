use crate::events::{EventHandler, Priority, Propagation, RequestReceived, ResponseBuilding};
use crate::trace_context::TraceContext;

/// Look up a key in the metadata vector.
fn metadata_get<'a>(metadata: &'a [(String, String)], key: &str) -> Option<&'a str> {
    metadata
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// Parses or generates a W3C Trace Context from incoming request headers
/// and stores trace IDs in the event metadata for downstream handlers.
pub struct TraceContextRequestHandler {
    enabled: bool,
}

impl TraceContextRequestHandler {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}

impl EventHandler<RequestReceived> for TraceContextRequestHandler {
    #[inline]
    fn handle(&self, event: &mut RequestReceived) -> Propagation {
        if !self.enabled {
            return Propagation::Continue;
        }

        let ctx = TraceContext::from_headers(&event.parts.headers);

        event
            .metadata
            .push(("trace_id".to_string(), ctx.trace_id().to_string()));
        event
            .metadata
            .push(("span_id".to_string(), ctx.span_id().to_string()));
        event.metadata.push((
            "parent_span_id".to_string(),
            ctx.parent_span_id().unwrap_or("").to_string(),
        ));
        event
            .metadata
            .push(("trace_flags".to_string(), ctx.trace_flags_hex().to_string()));

        if let Some(ts) = ctx.tracestate() {
            event
                .metadata
                .push(("tracestate".to_string(), ts.to_string()));
        }

        Propagation::Continue
    }

    fn priority(&self) -> Priority {
        -95
    }
}

/// Injects `traceparent` (and optionally `tracestate`) response headers
/// from the metadata populated by `TraceContextRequestHandler`.
pub struct TraceContextResponseHandler {
    enabled: bool,
}

impl TraceContextResponseHandler {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}

impl EventHandler<ResponseBuilding> for TraceContextResponseHandler {
    #[inline]
    fn handle(&self, event: &mut ResponseBuilding) -> Propagation {
        if !self.enabled {
            return Propagation::Continue;
        }

        let trace_id = match metadata_get(&event.metadata, "trace_id") {
            Some(v) => v,
            None => return Propagation::Continue,
        };
        let span_id = match metadata_get(&event.metadata, "span_id") {
            Some(v) => v,
            None => return Propagation::Continue,
        };
        let trace_flags = match metadata_get(&event.metadata, "trace_flags") {
            Some(v) => v,
            None => return Propagation::Continue,
        };

        let traceparent = format!("00-{}-{}-{}", trace_id, span_id, trace_flags);
        if let Ok(hv) = http::HeaderValue::from_str(&traceparent) {
            event.response.headers_mut().insert("traceparent", hv);
        }

        if let Some(ts) = metadata_get(&event.metadata, "tracestate") {
            if let Ok(hv) = http::HeaderValue::from_str(ts) {
                event.response.headers_mut().insert("tracestate", hv);
            }
        }

        Propagation::Continue
    }

    fn priority(&self) -> Priority {
        -95
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventHandler;
    use crate::types::full_body;
    use bytes::Bytes;
    use http::{HeaderValue, Response};
    use std::net::{Ipv4Addr, SocketAddr};

    const VALID_TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    fn make_request_event() -> RequestReceived {
        let (parts, _) = http::Request::builder()
            .method(http::Method::GET)
            .uri("/test")
            .body(())
            .unwrap()
            .into_parts();

        RequestReceived {
            parts,
            remote_addr: SocketAddr::new(Ipv4Addr::new(127, 0, 0, 1).into(), 8080),
            request_id: String::new(),
            early_response: None,
            metadata: Vec::new(),
        }
    }

    fn make_response_event(metadata: Vec<(String, String)>) -> ResponseBuilding {
        ResponseBuilding {
            request_id: "test-id".to_string(),
            response: Response::builder()
                .status(200)
                .body(full_body(Bytes::from_static(b"OK")))
                .unwrap(),
            metadata,
        }
    }

    #[test]
    fn test_disabled_does_nothing() {
        let handler = TraceContextRequestHandler::new(false);
        let mut event = make_request_event();
        let result = handler.handle(&mut event);
        assert_eq!(result, Propagation::Continue);
        assert!(event.metadata.is_empty());
    }

    #[test]
    fn test_enabled_generates_trace_context() {
        let handler = TraceContextRequestHandler::new(true);
        let mut event = make_request_event();
        handler.handle(&mut event);

        // Should have trace_id, span_id, parent_span_id, trace_flags
        assert!(metadata_get(&event.metadata, "trace_id").is_some());
        assert!(metadata_get(&event.metadata, "span_id").is_some());
        assert!(metadata_get(&event.metadata, "parent_span_id").is_some());
        assert!(metadata_get(&event.metadata, "trace_flags").is_some());

        let trace_id = metadata_get(&event.metadata, "trace_id").unwrap();
        assert_eq!(trace_id.len(), 32);
        assert!(trace_id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));

        let span_id = metadata_get(&event.metadata, "span_id").unwrap();
        assert_eq!(span_id.len(), 16);

        // No parent since no incoming traceparent
        let parent = metadata_get(&event.metadata, "parent_span_id").unwrap();
        assert_eq!(parent, "");

        // Default flags: not sampled
        let flags = metadata_get(&event.metadata, "trace_flags").unwrap();
        assert_eq!(flags, "00");
    }

    #[test]
    fn test_enabled_parses_incoming_traceparent() {
        let handler = TraceContextRequestHandler::new(true);
        let mut event = make_request_event();
        event
            .parts
            .headers
            .insert("traceparent", HeaderValue::from_static(VALID_TRACEPARENT));

        handler.handle(&mut event);

        let trace_id = metadata_get(&event.metadata, "trace_id").unwrap();
        assert_eq!(trace_id, "4bf92f3577b34da6a3ce929d0e0e4736");

        let parent = metadata_get(&event.metadata, "parent_span_id").unwrap();
        assert_eq!(parent, "00f067aa0ba902b7");

        let flags = metadata_get(&event.metadata, "trace_flags").unwrap();
        assert_eq!(flags, "01");

        // span_id should be newly generated, not the incoming one
        let span_id = metadata_get(&event.metadata, "span_id").unwrap();
        assert_ne!(span_id, "00f067aa0ba902b7");
        assert_eq!(span_id.len(), 16);
    }

    #[test]
    fn test_priority_is_minus_95() {
        assert_eq!(TraceContextRequestHandler::new(true).priority(), -95);
        assert_eq!(TraceContextResponseHandler::new(true).priority(), -95);
    }

    #[test]
    fn test_response_handler_injects_traceparent() {
        let handler = TraceContextResponseHandler::new(true);
        let metadata = vec![
            (
                "trace_id".to_string(),
                "4bf92f3577b34da6a3ce929d0e0e4736".to_string(),
            ),
            ("span_id".to_string(), "00f067aa0ba902b7".to_string()),
            ("trace_flags".to_string(), "01".to_string()),
        ];
        let mut event = make_response_event(metadata);

        handler.handle(&mut event);

        let tp = event
            .response
            .headers()
            .get("traceparent")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(
            tp,
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        );
    }

    #[test]
    fn test_response_handler_disabled_skips() {
        let handler = TraceContextResponseHandler::new(false);
        let metadata = vec![
            (
                "trace_id".to_string(),
                "4bf92f3577b34da6a3ce929d0e0e4736".to_string(),
            ),
            ("span_id".to_string(), "00f067aa0ba902b7".to_string()),
            ("trace_flags".to_string(), "01".to_string()),
        ];
        let mut event = make_response_event(metadata);

        handler.handle(&mut event);

        assert!(event.response.headers().get("traceparent").is_none());
    }

    #[test]
    fn test_response_handler_injects_tracestate() {
        let handler = TraceContextResponseHandler::new(true);
        let metadata = vec![
            (
                "trace_id".to_string(),
                "4bf92f3577b34da6a3ce929d0e0e4736".to_string(),
            ),
            ("span_id".to_string(), "00f067aa0ba902b7".to_string()),
            ("trace_flags".to_string(), "01".to_string()),
            (
                "tracestate".to_string(),
                "vendor1=value1,vendor2=value2".to_string(),
            ),
        ];
        let mut event = make_response_event(metadata);

        handler.handle(&mut event);

        let ts = event
            .response
            .headers()
            .get("tracestate")
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(ts, "vendor1=value1,vendor2=value2");
    }

    #[test]
    fn test_request_handler_writes_tracestate_to_metadata() {
        let handler = TraceContextRequestHandler::new(true);
        let mut event = make_request_event();
        event
            .parts
            .headers
            .insert("traceparent", HeaderValue::from_static(VALID_TRACEPARENT));
        event.parts.headers.insert(
            "tracestate",
            HeaderValue::from_static("vendor1=value1,vendor2=value2"),
        );

        handler.handle(&mut event);

        let ts = metadata_get(&event.metadata, "tracestate").unwrap();
        assert_eq!(ts, "vendor1=value1,vendor2=value2");
    }
}
