use http::HeaderValue;

use crate::events::ResponseBuilding;
use crate::events::{EventHandler, Priority, Propagation};

/// Adds security headers to every response: `X-Content-Type-Options`, `X-Frame-Options`,
/// and `Content-Security-Policy: frame-ancestors`. Frame protection is controlled by the
/// `FRAME_OPTIONS` environment variable (default: `DENY`).
pub struct SecurityHeadersHandler {
    frame_options: Option<HeaderValue>,
    frame_ancestors: Option<HeaderValue>,
}

impl SecurityHeadersHandler {
    pub fn new(raw: &str) -> Self {
        let (fo, fa) = match raw.to_uppercase().as_str() {
            "DENY" => (
                Some(HeaderValue::from_static("DENY")),
                Some(HeaderValue::from_static("frame-ancestors 'none'")),
            ),
            "SAMEORIGIN" => (
                Some(HeaderValue::from_static("SAMEORIGIN")),
                Some(HeaderValue::from_static("frame-ancestors 'self'")),
            ),
            "OFF" => (None, None),
            _ => {
                tracing::warn!(
                    value = %raw,
                    "Invalid FRAME_OPTIONS value, defaulting to DENY"
                );
                (
                    Some(HeaderValue::from_static("DENY")),
                    Some(HeaderValue::from_static("frame-ancestors 'none'")),
                )
            }
        };

        Self {
            frame_options: fo,
            frame_ancestors: fa,
        }
    }
}

impl EventHandler<ResponseBuilding> for SecurityHeadersHandler {
    #[inline]
    fn handle(&self, event: &mut ResponseBuilding) -> Propagation {
        let headers = event.response.headers_mut();

        headers.insert(
            http::header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );

        if let Some(ref fo) = self.frame_options {
            headers.insert("x-frame-options", fo.clone());
        }
        if let Some(ref fa) = self.frame_ancestors {
            headers.insert("content-security-policy", fa.clone());
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

    fn make_event() -> ResponseBuilding {
        ResponseBuilding {
            request_id: "test123".to_string(),
            response: Response::builder()
                .status(200)
                .body(full_body(Bytes::from_static(b"OK")))
                .unwrap(),
            metadata: Vec::new(),
        }
    }

    #[test]
    fn test_default_frame_deny() {
        let handler = SecurityHeadersHandler::new("DENY");
        let mut event = make_event();

        handler.handle(&mut event);

        assert_eq!(
            event
                .response
                .headers()
                .get("x-frame-options")
                .unwrap()
                .to_str()
                .unwrap(),
            "DENY"
        );
        assert_eq!(
            event
                .response
                .headers()
                .get("content-security-policy")
                .unwrap()
                .to_str()
                .unwrap(),
            "frame-ancestors 'none'"
        );
    }

    #[test]
    fn test_frame_sameorigin() {
        let handler = SecurityHeadersHandler::new("SAMEORIGIN");
        let mut event = make_event();

        handler.handle(&mut event);

        assert_eq!(
            event
                .response
                .headers()
                .get("x-frame-options")
                .unwrap()
                .to_str()
                .unwrap(),
            "SAMEORIGIN"
        );
        assert_eq!(
            event
                .response
                .headers()
                .get("content-security-policy")
                .unwrap()
                .to_str()
                .unwrap(),
            "frame-ancestors 'self'"
        );
    }

    #[test]
    fn test_frame_off() {
        let handler = SecurityHeadersHandler::new("off");
        let mut event = make_event();

        handler.handle(&mut event);

        assert!(event.response.headers().get("x-frame-options").is_none());
        assert!(event
            .response
            .headers()
            .get("content-security-policy")
            .is_none());
    }

    #[test]
    fn test_nosniff_always() {
        let handler = SecurityHeadersHandler::new("off");
        let mut event = make_event();

        handler.handle(&mut event);

        assert_eq!(
            event
                .response
                .headers()
                .get("x-content-type-options")
                .unwrap()
                .to_str()
                .unwrap(),
            "nosniff"
        );
    }

    #[test]
    fn test_invalid_value_defaults_deny() {
        let handler = SecurityHeadersHandler::new("GARBAGE");
        let mut event = make_event();

        handler.handle(&mut event);

        assert_eq!(
            event
                .response
                .headers()
                .get("x-frame-options")
                .unwrap()
                .to_str()
                .unwrap(),
            "DENY"
        );
        assert_eq!(
            event
                .response
                .headers()
                .get("content-security-policy")
                .unwrap()
                .to_str()
                .unwrap(),
            "frame-ancestors 'none'"
        );
    }

    #[test]
    fn test_case_insensitive() {
        let handler = SecurityHeadersHandler::new("sameorigin");
        let mut event = make_event();

        handler.handle(&mut event);

        assert_eq!(
            event
                .response
                .headers()
                .get("x-frame-options")
                .unwrap()
                .to_str()
                .unwrap(),
            "SAMEORIGIN"
        );
    }

    #[test]
    fn test_priority() {
        let handler = SecurityHeadersHandler::new("DENY");
        assert_eq!(handler.priority(), 100);
    }
}
