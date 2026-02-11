use std::panic::AssertUnwindSafe;

use crate::events::{
    EventHandler, Priority, Propagation, RequestComplete, RequestReceived, ResponseBuilding,
};

use super::cookies::{extract_plugin_cookies, format_set_cookie_header};
use super::handler::{
    PluginCompleteHandler, PluginCompleteView, PluginRequestActions, PluginRequestHandler,
    PluginRequestView, PluginResponseActions, PluginResponseHandler, PluginResponseView,
};

// ─── PluginRequestWrapper ────────────────────────────────────

/// Wraps a `PluginRequestHandler` into `EventHandler<RequestReceived>`.
pub(crate) struct PluginRequestWrapper<H: PluginRequestHandler> {
    pub handler: H,
    pub plugin_name: String,
    pub cookie_prefix: String,
}

impl<H: PluginRequestHandler + 'static> EventHandler<RequestReceived> for PluginRequestWrapper<H> {
    fn handle(&self, event: &mut RequestReceived) -> Propagation {
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let cookies = extract_plugin_cookies(&event.parts.headers, &self.cookie_prefix);
            let view = PluginRequestView::new(
                &event.parts.method,
                &event.parts.uri,
                event.remote_addr,
                &event.request_id,
                &event.parts.headers,
                cookies,
            );

            let mut actions = PluginRequestActions::new();
            self.handler.handle(&view, &mut actions);
            actions
        }));

        match result {
            Ok(actions) => {
                if let Some(resp) = actions.early_response {
                    event.early_response = Some(resp);
                }
                for (key, value) in actions.metadata {
                    event.metadata.insert(key, value);
                }
            }
            Err(_) => {
                tracing::error!(plugin = %self.plugin_name, "Plugin request handler panicked");
            }
        }

        Propagation::Continue
    }

    fn priority(&self) -> Priority {
        self.handler.priority()
    }
}

// ─── PluginResponseWrapper ───────────────────────────────────

/// Wraps a `PluginResponseHandler` into `EventHandler<ResponseBuilding>`.
pub(crate) struct PluginResponseWrapper<H: PluginResponseHandler> {
    pub handler: H,
    pub plugin_name: String,
    pub cookie_prefix: String,
}

impl<H: PluginResponseHandler + 'static> EventHandler<ResponseBuilding>
    for PluginResponseWrapper<H>
{
    fn handle(&self, event: &mut ResponseBuilding) -> Propagation {
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let view = PluginResponseView::new(
                event.response.status(),
                &event.request_id,
                event.response.headers(),
            );

            let mut actions = PluginResponseActions::new(&self.plugin_name);
            self.handler.handle(&view, &mut actions);
            actions
        }));

        match result {
            Ok(actions) => {
                // Apply added headers
                for (name, value) in actions.add_headers {
                    event.response.headers_mut().append(name, value);
                }
                // Apply Set-Cookie headers with prefix
                for cookie in &actions.set_cookies {
                    let header_value = format_set_cookie_header(&self.cookie_prefix, cookie);
                    if let Ok(hv) = http::HeaderValue::from_str(&header_value) {
                        event
                            .response
                            .headers_mut()
                            .append(http::header::SET_COOKIE, hv);
                    }
                }
            }
            Err(_) => {
                tracing::error!(plugin = %self.plugin_name, "Plugin response handler panicked");
            }
        }

        Propagation::Continue
    }

    fn priority(&self) -> Priority {
        self.handler.priority()
    }
}

// ─── PluginCompleteWrapper ───────────────────────────────────

/// Wraps a `PluginCompleteHandler` into `EventHandler<RequestComplete>`.
pub(crate) struct PluginCompleteWrapper<H: PluginCompleteHandler> {
    pub handler: H,
    pub plugin_name: String,
}

impl<H: PluginCompleteHandler + 'static> EventHandler<RequestComplete>
    for PluginCompleteWrapper<H>
{
    fn handle(&self, event: &mut RequestComplete) -> Propagation {
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let view = PluginCompleteView {
                request_id: &event.request_id,
                method: &event.method,
                path: &event.path,
                status: event.status,
                duration: event.duration,
                remote_addr: event.remote_addr,
            };
            self.handler.handle(&view);
        }));

        if result.is_err() {
            tracing::error!(plugin = %self.plugin_name, "Plugin complete handler panicked");
        }

        Propagation::Continue
    }

    fn priority(&self) -> Priority {
        self.handler.priority()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventHandler;
    use bytes::Bytes;
    use std::collections::HashMap;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

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
            request_id: "test123".to_string(),
            early_response: None,
            metadata: HashMap::new(),
        }
    }

    // ── Panic isolation tests ──

    struct PanicRequestHandler;
    impl PluginRequestHandler for PanicRequestHandler {
        fn handle(&self, _: &PluginRequestView, _: &mut PluginRequestActions) {
            panic!("request handler bug");
        }
    }

    #[test]
    fn test_request_wrapper_panic_isolation() {
        let wrapper = PluginRequestWrapper {
            handler: PanicRequestHandler,
            plugin_name: "buggy".into(),
            cookie_prefix: "__oxp_buggy_".into(),
        };

        let mut event = make_request_event();
        let result = EventHandler::<RequestReceived>::handle(&wrapper, &mut event);
        assert_eq!(result, Propagation::Continue);
        assert!(event.early_response.is_none());
    }

    struct PanicResponseHandler;
    impl PluginResponseHandler for PanicResponseHandler {
        fn handle(&self, _: &PluginResponseView, _: &mut PluginResponseActions<'_>) {
            panic!("response handler bug");
        }
    }

    #[test]
    fn test_response_wrapper_panic_isolation() {
        let wrapper = PluginResponseWrapper {
            handler: PanicResponseHandler,
            plugin_name: "buggy".into(),
            cookie_prefix: "__oxp_buggy_".into(),
        };

        let response = http::Response::builder()
            .status(200)
            .body(crate::types::full_body(Bytes::from_static(b"ok")))
            .unwrap();
        let mut event = ResponseBuilding {
            request_id: "test123".into(),
            response,
        };
        let result = EventHandler::<ResponseBuilding>::handle(&wrapper, &mut event);
        assert_eq!(result, Propagation::Continue);
    }

    struct PanicCompleteHandler;
    impl PluginCompleteHandler for PanicCompleteHandler {
        fn handle(&self, _: &PluginCompleteView) {
            panic!("complete handler bug");
        }
    }

    #[test]
    fn test_complete_wrapper_panic_isolation() {
        let wrapper = PluginCompleteWrapper {
            handler: PanicCompleteHandler,
            plugin_name: "buggy".into(),
        };

        let mut event = RequestComplete {
            request_id: "test123".into(),
            method: "GET".into(),
            path: "/test".into(),
            status: 200,
            duration: std::time::Duration::from_millis(10),
            remote_addr: SocketAddr::new(Ipv4Addr::new(127, 0, 0, 1).into(), 8080),
        };
        let result = EventHandler::<RequestComplete>::handle(&wrapper, &mut event);
        assert_eq!(result, Propagation::Continue);
    }

    // ── Action application tests ──

    struct MetadataHandler;
    impl PluginRequestHandler for MetadataHandler {
        fn handle(&self, _: &PluginRequestView, actions: &mut PluginRequestActions) {
            actions.set_metadata("user_id", "42");
        }
    }

    #[test]
    fn test_request_wrapper_applies_metadata() {
        let wrapper = PluginRequestWrapper {
            handler: MetadataHandler,
            plugin_name: "test".into(),
            cookie_prefix: "__oxp_test_".into(),
        };

        let mut event = make_request_event();
        EventHandler::<RequestReceived>::handle(&wrapper, &mut event);
        assert_eq!(event.metadata.get("user_id"), Some(&"42".to_string()));
    }

    struct HeaderHandler;
    impl PluginResponseHandler for HeaderHandler {
        fn handle(&self, _: &PluginResponseView, actions: &mut PluginResponseActions<'_>) {
            actions.add_header("x-plugin", "value".parse().unwrap());
            actions.set_cookie(
                "token",
                "abc",
                super::super::cookies::CookieOptions::default(),
            );
        }
    }

    #[test]
    fn test_response_wrapper_applies_headers_and_cookies() {
        let wrapper = PluginResponseWrapper {
            handler: HeaderHandler,
            plugin_name: "test".into(),
            cookie_prefix: "__oxp_test_".into(),
        };

        let response = http::Response::builder()
            .status(200)
            .body(crate::types::full_body(bytes::Bytes::from_static(b"ok")))
            .unwrap();
        let mut event = ResponseBuilding {
            request_id: "req1".into(),
            response,
        };
        EventHandler::<ResponseBuilding>::handle(&wrapper, &mut event);

        assert_eq!(event.response.headers().get("x-plugin").unwrap(), "value");
        let set_cookie = event
            .response
            .headers()
            .get(http::header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(set_cookie.starts_with("__oxp_test_token=abc"));
    }

    struct CalledHandler {
        called: Arc<AtomicBool>,
    }
    impl PluginCompleteHandler for CalledHandler {
        fn handle(&self, view: &PluginCompleteView) {
            assert_eq!(view.status, 200);
            self.called.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn test_complete_wrapper_calls_handler() {
        let called = Arc::new(AtomicBool::new(false));
        let wrapper = PluginCompleteWrapper {
            handler: CalledHandler {
                called: Arc::clone(&called),
            },
            plugin_name: "test".into(),
        };

        let mut event = RequestComplete {
            request_id: "req1".into(),
            method: "GET".into(),
            path: "/test".into(),
            status: 200,
            duration: std::time::Duration::from_millis(5),
            remote_addr: SocketAddr::new(Ipv4Addr::new(127, 0, 0, 1).into(), 8080),
        };
        EventHandler::<RequestComplete>::handle(&wrapper, &mut event);
        assert!(called.load(Ordering::SeqCst));
    }
}
