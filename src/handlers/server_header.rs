use std::sync::LazyLock;

use http::HeaderValue;

use crate::events::ResponseBuilding;
use crate::events::{EventHandler, Priority, Propagation};

/// Pre-computed `Server` header value — avoids allocation per response.
static SERVER_HEADER_VALUE: LazyLock<HeaderValue> =
    LazyLock::new(|| HeaderValue::from_static(concat!("OxPHP/", env!("CARGO_PKG_VERSION"))));

/// Adds `Server` and `X-Request-ID` headers to every response.
pub struct ServerHeaderHandler;

impl EventHandler<ResponseBuilding> for ServerHeaderHandler {
    fn handle(&self, event: &mut ResponseBuilding) -> Propagation {
        event
            .response
            .headers_mut()
            .insert(http::header::SERVER, SERVER_HEADER_VALUE.clone());

        if let Ok(hv) = HeaderValue::from_str(&event.request_id) {
            event.response.headers_mut().insert("x-request-id", hv);
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
    use crate::types::full_body;
    use bytes::Bytes;
    use http::Response;

    #[test]
    fn test_adds_server_header() {
        let handler = ServerHeaderHandler;
        let mut event = ResponseBuilding {
            request_id: "abc123".to_string(),
            response: Response::builder()
                .status(200)
                .body(full_body(Bytes::from_static(b"OK")))
                .unwrap(),
        };

        handler.handle(&mut event);
        assert!(event.response.headers().contains_key("server"));
        assert!(event
            .response
            .headers()
            .get("server")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("OxPHP/"));
    }

    #[test]
    fn test_adds_request_id_header() {
        let handler = ServerHeaderHandler;
        let mut event = ResponseBuilding {
            request_id: "deadbeef12345678".to_string(),
            response: Response::builder()
                .status(200)
                .body(full_body(Bytes::from_static(b"OK")))
                .unwrap(),
        };

        handler.handle(&mut event);
        assert_eq!(
            event
                .response
                .headers()
                .get("x-request-id")
                .unwrap()
                .to_str()
                .unwrap(),
            "deadbeef12345678"
        );
    }

    #[test]
    fn test_priority() {
        assert_eq!(ServerHeaderHandler.priority(), 100);
    }
}
