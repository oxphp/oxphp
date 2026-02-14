use std::sync::Arc;

use http::{Response, StatusCode};

use crate::events::{EventHandler, Priority, Propagation, ResponseBuilding};
use crate::server::error_pages::ErrorPages;
use crate::types::full_body;

/// Replaces error response bodies with pre-loaded custom HTML error pages.
pub struct ErrorPagesHandler {
    pages: Arc<ErrorPages>,
}

impl ErrorPagesHandler {
    pub fn new(pages: Arc<ErrorPages>) -> Self {
        Self { pages }
    }
}

impl EventHandler<ResponseBuilding> for ErrorPagesHandler {
    #[inline]
    fn handle(&self, event: &mut ResponseBuilding) -> Propagation {
        let status = event.response.status().as_u16();
        if status >= 400 {
            if let Some(page_bytes) = self.pages.get(status) {
                event.response = Response::builder()
                    .status(
                        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    )
                    .header(http::header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .header(http::header::CONTENT_LENGTH, page_bytes.len())
                    .body(full_body(page_bytes))
                    .unwrap();
            }
        }
        Propagation::Continue
    }

    fn priority(&self) -> Priority {
        60
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventHandler;
    use crate::types::ResponseBody;
    use bytes::Bytes;
    use tempfile::TempDir;

    fn make_error_pages() -> Arc<ErrorPages> {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("404.html"), "<h1>Custom 404</h1>").unwrap();
        std::fs::write(dir.path().join("500.html"), "<h1>Custom 500</h1>").unwrap();
        // Leak the TempDir so it doesn't get cleaned up during the test
        let path = dir.path().to_path_buf();
        std::mem::forget(dir);
        Arc::new(ErrorPages::load(&path).unwrap())
    }

    fn make_response(status: u16) -> Response<ResponseBody> {
        Response::builder()
            .status(StatusCode::from_u16(status).unwrap())
            .body(full_body(Bytes::from_static(b"original")))
            .unwrap()
    }

    #[test]
    fn test_applies_error_page() {
        let pages = make_error_pages();
        let handler = ErrorPagesHandler::new(pages);

        let mut event = ResponseBuilding {
            request_id: "test".to_string(),
            response: make_response(404),
        };

        handler.handle(&mut event);
        assert_eq!(event.response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            event.response.headers().get("content-type").unwrap(),
            "text/html; charset=utf-8"
        );
    }

    #[test]
    fn test_skips_success_response() {
        let pages = make_error_pages();
        let handler = ErrorPagesHandler::new(pages);

        let mut event = ResponseBuilding {
            request_id: "test".to_string(),
            response: make_response(200),
        };

        handler.handle(&mut event);
        // 200 response should not be modified
        assert_eq!(event.response.status(), StatusCode::OK);
        assert!(event.response.headers().get("content-type").is_none());
    }

    #[test]
    fn test_skips_missing_page() {
        let pages = make_error_pages();
        let handler = ErrorPagesHandler::new(pages);

        let mut event = ResponseBuilding {
            request_id: "test".to_string(),
            response: make_response(403),
        };

        handler.handle(&mut event);
        // 403 has no custom page, should not be modified
        assert_eq!(event.response.status(), StatusCode::FORBIDDEN);
        assert!(event.response.headers().get("content-type").is_none());
    }

    #[test]
    fn test_priority() {
        let dir = TempDir::new().unwrap();
        let pages = Arc::new(ErrorPages::load(dir.path()).unwrap());
        let handler = ErrorPagesHandler::new(pages);
        assert_eq!(handler.priority(), 60);
    }
}
