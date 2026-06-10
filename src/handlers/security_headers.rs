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

/// True if the serialized CSP value contains a `frame-ancestors` directive.
/// Parses at the directive level: the name must open a `;`-separated segment
/// and be followed by whitespace or the segment end — a URL that merely
/// contains the substring (e.g. `report-uri /frame-ancestors`) does not
/// count. Directive names are ASCII case-insensitive. Operates on raw bytes
/// because header values may carry obs-text (0x80–0xFF) that `to_str` rejects.
fn has_frame_ancestors_directive(value: &[u8]) -> bool {
    value.split(|&b| b == b';').any(|segment| {
        let start = segment
            .iter()
            .position(|b| !b.is_ascii_whitespace())
            .unwrap_or(segment.len());
        let segment = &segment[start..];
        let end = segment
            .iter()
            .position(|b| b.is_ascii_whitespace())
            .unwrap_or(segment.len());
        segment[..end].eq_ignore_ascii_case(b"frame-ancestors")
    })
}

impl EventHandler<ResponseBuilding> for SecurityHeadersHandler {
    #[inline]
    fn handle(&self, event: &mut ResponseBuilding) -> Propagation {
        let headers = event.response.headers_mut();

        // Application-set headers take precedence: server values fill in only
        // when the response carries no such header (insert-if-absent, like
        // Apache's `Header setifempty`).
        let app_xfo = headers.contains_key("x-frame-options");
        let app_csp = headers.contains_key("content-security-policy");
        // An app CSP that carries its own frame-ancestors directive owns the
        // framing policy outright.
        let app_csp_frame_ancestors = headers
            .get_all("content-security-policy")
            .iter()
            .any(|v| has_frame_ancestors_directive(v.as_bytes()));

        if !headers.contains_key(http::header::X_CONTENT_TYPE_OPTIONS) {
            headers.insert(
                http::header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            );
        }

        // Skip the server XFO if the app's own CSP already states a framing
        // policy — legacy (pre-CSP2) browsers ignore CSP and would otherwise
        // enforce a server XFO stricter than what the app declared.
        if !app_xfo && !app_csp_frame_ancestors {
            if let Some(ref fo) = self.frame_options {
                headers.insert("x-frame-options", fo.clone());
            }
        }
        // Skip the server CSP if the app set its own CSP *or* its own
        // X-Frame-Options — CSP frame-ancestors overrides XFO in modern
        // browsers, so inserting it would silently defeat an app-set XFO.
        if !app_csp && !app_xfo {
            if let Some(ref fa) = self.frame_ancestors {
                headers.insert("content-security-policy", fa.clone());
            }
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

    #[test]
    fn test_app_csp_preserved() {
        let handler = SecurityHeadersHandler::new("DENY");
        let mut event = make_event();
        event.response.headers_mut().insert(
            "content-security-policy",
            HeaderValue::from_static("script-src 'self'"),
        );

        handler.handle(&mut event);

        assert_eq!(
            event
                .response
                .headers()
                .get("content-security-policy")
                .unwrap()
                .to_str()
                .unwrap(),
            "script-src 'self'"
        );
        // The app CSP says nothing about framing, so the server XFO still applies.
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
    }

    #[test]
    fn test_app_csp_frame_ancestors_suppresses_server_xfo() {
        let handler = SecurityHeadersHandler::new("DENY");
        let mut event = make_event();
        event.response.headers_mut().insert(
            "content-security-policy",
            HeaderValue::from_static("script-src 'self'; FRAME-ANCESTORS 'self'"),
        );

        handler.handle(&mut event);

        assert_eq!(
            event
                .response
                .headers()
                .get("content-security-policy")
                .unwrap()
                .to_str()
                .unwrap(),
            "script-src 'self'; FRAME-ANCESTORS 'self'"
        );
        // The app CSP owns the framing policy; a server XFO DENY would
        // over-block in legacy browsers that ignore CSP.
        assert!(event.response.headers().get("x-frame-options").is_none());
    }

    #[test]
    fn test_app_xfo_preserved_and_suppresses_server_csp() {
        let handler = SecurityHeadersHandler::new("DENY");
        let mut event = make_event();
        event
            .response
            .headers_mut()
            .insert("x-frame-options", HeaderValue::from_static("SAMEORIGIN"));

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
        // Server CSP would override the app's XFO in modern browsers — must be skipped.
        assert!(event
            .response
            .headers()
            .get("content-security-policy")
            .is_none());
    }

    #[test]
    fn test_frame_ancestors_directive_parsing() {
        // Directive must open a segment — substring inside another
        // directive's value is not a framing policy.
        assert!(!has_frame_ancestors_directive(
            b"default-src 'self'; report-uri https://csp.example.com/frame-ancestors"
        ));
        assert!(has_frame_ancestors_directive(b"frame-ancestors 'self'"));
        assert!(has_frame_ancestors_directive(
            b"script-src 'self';  \tFRAME-ANCESTORS 'none'"
        ));
        // Valueless directive ends the segment immediately.
        assert!(has_frame_ancestors_directive(
            b"script-src 'self';frame-ancestors"
        ));
        assert!(!has_frame_ancestors_directive(
            b"frame-ancestors-extra 'self'"
        ));
        assert!(!has_frame_ancestors_directive(b""));
        // obs-text bytes elsewhere in the value must not mask the directive.
        assert!(has_frame_ancestors_directive(
            b"frame-ancestors 'self'; report-uri /r\xC3\xA9port"
        ));
    }

    #[test]
    fn test_app_csp_substring_false_positive_keeps_server_xfo() {
        let handler = SecurityHeadersHandler::new("DENY");
        let mut event = make_event();
        event.response.headers_mut().insert(
            "content-security-policy",
            HeaderValue::from_static(
                "default-src 'self'; report-uri https://csp.example.com/frame-ancestors",
            ),
        );

        handler.handle(&mut event);

        // No real framing directive in the app CSP — the server XFO must stay.
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
    }

    #[test]
    fn test_app_csp_with_obs_text_still_suppresses_server_xfo() {
        let handler = SecurityHeadersHandler::new("DENY");
        let mut event = make_event();
        event.response.headers_mut().insert(
            "content-security-policy",
            HeaderValue::from_bytes(b"frame-ancestors 'self'; report-uri /r\xE9port".as_slice())
                .unwrap(),
        );

        handler.handle(&mut event);

        // A non-ASCII byte elsewhere in the value must not hide the
        // app's framing directive from the suppression check.
        assert!(event.response.headers().get("x-frame-options").is_none());
    }

    #[test]
    fn test_app_nosniff_preserved() {
        let handler = SecurityHeadersHandler::new("DENY");
        let mut event = make_event();
        event.response.headers_mut().insert(
            "x-content-type-options",
            HeaderValue::from_static("custom-value"),
        );

        handler.handle(&mut event);

        assert_eq!(
            event
                .response
                .headers()
                .get("x-content-type-options")
                .unwrap()
                .to_str()
                .unwrap(),
            "custom-value"
        );
    }
}
