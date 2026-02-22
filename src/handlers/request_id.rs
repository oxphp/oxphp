use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::SystemTime;

use crate::events::{EventHandler, Priority, Propagation, RequestReceived};

/// Atomic counter for request ID generation.
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Per-process unique identifier (lower 16 bits of PID XOR'd with startup nanos).
/// Differentiates IDs across multiple instances (containers, replicas).
static PROCESS_ID: OnceLock<u16> = OnceLock::new();

fn process_id() -> u16 {
    *PROCESS_ID.get_or_init(|| {
        let pid = std::process::id() as u64;
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as u64;
        (pid ^ nanos) as u16
    })
}

/// Generate a request ID: `{timestamp:08x}{process:04x}{counter:08x}` (20 hex chars).
/// Unique across processes and restarts via process_id component.
fn generate_request_id() -> String {
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as u32;
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed) as u32;
    let pid = process_id();
    let mut id = String::with_capacity(20);
    write!(id, "{ts:08x}{pid:04x}{counter:08x}").unwrap();
    id
}

/// Generates a request ID or preserves an incoming `X-Request-ID` header.
pub struct RequestIdGenerator;

impl EventHandler<RequestReceived> for RequestIdGenerator {
    #[inline]
    fn handle(&self, event: &mut RequestReceived) -> Propagation {
        // Honor incoming X-Request-ID header, or generate one
        let id = event
            .parts
            .headers
            .get("x-request-id")
            .and_then(|v: &http::HeaderValue| v.to_str().ok())
            .map(|s: &str| s.to_string())
            .unwrap_or_else(generate_request_id);

        event.request_id = id;
        Propagation::Continue
    }

    fn priority(&self) -> Priority {
        -100
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventHandler;
    use http::{HeaderValue, Method};
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
            request_id: String::new(),
            early_response: None,
            metadata: Vec::new(),
        }
    }

    #[test]
    fn test_generates_request_id() {
        let handler = RequestIdGenerator;
        let mut event = make_event();
        handler.handle(&mut event);

        assert_eq!(event.request_id.len(), 20);
        assert!(event.request_id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_preserves_incoming_id() {
        let handler = RequestIdGenerator;
        let mut event = make_event();
        event
            .parts
            .headers
            .insert("x-request-id", HeaderValue::from_static("my-custom-id"));

        handler.handle(&mut event);
        assert_eq!(event.request_id, "my-custom-id");
    }

    #[test]
    fn test_unique_ids() {
        let handler = RequestIdGenerator;
        let ids: Vec<String> = (0..100)
            .map(|_| {
                let mut event = make_event();
                handler.handle(&mut event);
                event.request_id
            })
            .collect();

        let mut deduped = ids.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(ids.len(), deduped.len());
    }

    #[test]
    fn test_priority() {
        assert_eq!(RequestIdGenerator.priority(), -100);
    }
}
