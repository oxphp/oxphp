use std::time::SystemTime;

use http::HeaderValue;

use crate::events::ResponseBuilding;
use crate::events::{EventHandler, Priority, Propagation};

/// Adds `Server`, `Date`, and `X-Request-ID` headers to every response.
pub struct ServerHeaderHandler;

impl EventHandler<ResponseBuilding> for ServerHeaderHandler {
    #[inline]
    fn handle(&self, event: &mut ResponseBuilding) -> Propagation {
        event
            .response
            .headers_mut()
            .insert(http::header::SERVER, HeaderValue::from_static("OxPHP"));

        let now = httpdate::fmt_http_date(SystemTime::now());
        event
            .response
            .headers_mut()
            .insert(http::header::DATE, HeaderValue::from_str(&now).unwrap());

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
            metadata: Vec::new(),
        };

        handler.handle(&mut event);
        assert!(event.response.headers().contains_key("server"));
        assert_eq!(
            event
                .response
                .headers()
                .get("server")
                .unwrap()
                .to_str()
                .unwrap(),
            "OxPHP"
        );
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
            metadata: Vec::new(),
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
    fn test_adds_date_header() {
        let handler = ServerHeaderHandler;
        let mut event = ResponseBuilding {
            request_id: "abc123".to_string(),
            response: Response::builder()
                .status(200)
                .body(full_body(Bytes::from_static(b"OK")))
                .unwrap(),
            metadata: Vec::new(),
        };

        handler.handle(&mut event);
        let date = event
            .response
            .headers()
            .get("date")
            .expect("Date header must be present")
            .to_str()
            .unwrap();
        assert!(date.ends_with(" GMT"), "Date must end with GMT: {date}");
        // Validate it parses back as valid HTTP-date
        httpdate::parse_http_date(date).expect("Date must be valid HTTP-date");
    }

    #[test]
    fn test_priority() {
        assert_eq!(ServerHeaderHandler.priority(), 100);
    }
}
