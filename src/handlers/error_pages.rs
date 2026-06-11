use std::sync::Arc;

use http::HeaderValue;

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
                // Swap only the body and its describing headers. Rebuilding
                // the response from scratch would drop semantically required
                // headers the body does not own — Content-Range on 416,
                // Retry-After on 429/529, Allow on 405. Headers coupled to
                // the ORIGINAL body must go with it: a PHP 404 compressed by
                // ob_gzhandler carries Content-Encoding: gzip that would
                // mislabel the plain HTML page (and ETag/Last-Modified would
                // validate it in caches).
                let len = page_bytes.len();
                *event.response.body_mut() = full_body(page_bytes);
                let headers = event.response.headers_mut();
                headers.remove(http::header::CONTENT_ENCODING);
                headers.remove(http::header::ETAG);
                headers.remove(http::header::LAST_MODIFIED);
                headers.insert(
                    http::header::CONTENT_TYPE,
                    HeaderValue::from_static("text/html; charset=utf-8"),
                );
                headers.insert(http::header::CONTENT_LENGTH, HeaderValue::from(len));
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
    use http::{Response, StatusCode};
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
            metadata: Vec::new(),
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
            metadata: Vec::new(),
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
            metadata: Vec::new(),
        };

        handler.handle(&mut event);
        // 403 has no custom page, should not be modified
        assert_eq!(event.response.status(), StatusCode::FORBIDDEN);
        assert!(event.response.headers().get("content-type").is_none());
    }

    #[test]
    fn test_keeps_semantic_headers() {
        // A custom error page replaces the body, not the response semantics:
        // Content-Range on 416 (RFC 9110 §14.4) must survive the swap.
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("416.html"), "<h1>Bad range</h1>").unwrap();
        let path = dir.path().to_path_buf();
        std::mem::forget(dir);
        let handler = ErrorPagesHandler::new(Arc::new(ErrorPages::load(&path).unwrap()));

        let response = Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(http::header::CONTENT_RANGE, "bytes */1000")
            .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(full_body(Bytes::from_static(b"416 Range Not Satisfiable")))
            .unwrap();
        let mut event = ResponseBuilding {
            request_id: "test".to_string(),
            response,
            metadata: Vec::new(),
        };

        handler.handle(&mut event);
        assert_eq!(event.response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(
            event
                .response
                .headers()
                .get(http::header::CONTENT_RANGE)
                .unwrap(),
            "bytes */1000"
        );
        assert_eq!(
            event
                .response
                .headers()
                .get(http::header::CONTENT_TYPE)
                .unwrap(),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            event
                .response
                .headers()
                .get(http::header::CONTENT_LENGTH)
                .unwrap(),
            "18"
        );
    }

    #[test]
    fn test_drops_body_coupled_headers() {
        // The swapped-in HTML page is uncompressed and is not the entity the
        // app's validators describe: Content-Encoding/ETag/Last-Modified of
        // the original body must not survive (e.g. a PHP 404 compressed by
        // ob_gzhandler would otherwise label plain HTML as gzip).
        let pages = make_error_pages();
        let handler = ErrorPagesHandler::new(pages);

        let response = Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(http::header::CONTENT_ENCODING, "gzip")
            .header(http::header::ETAG, "\"app-etag\"")
            .header(http::header::LAST_MODIFIED, "Tue, 01 Jan 2030 00:00:00 GMT")
            .body(full_body(Bytes::from_static(b"\x1f\x8b...")))
            .unwrap();
        let mut event = ResponseBuilding {
            request_id: "test".to_string(),
            response,
            metadata: Vec::new(),
        };

        handler.handle(&mut event);
        let headers = event.response.headers();
        assert!(headers.get(http::header::CONTENT_ENCODING).is_none());
        assert!(headers.get(http::header::ETAG).is_none());
        assert!(headers.get(http::header::LAST_MODIFIED).is_none());
        assert_eq!(
            headers.get(http::header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
    }

    #[test]
    fn test_priority() {
        let dir = TempDir::new().unwrap();
        let pages = Arc::new(ErrorPages::load(dir.path()).unwrap());
        let handler = ErrorPagesHandler::new(pages);
        assert_eq!(handler.priority(), 60);
    }
}
