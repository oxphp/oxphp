use bytes::Bytes;
use http::{HeaderName, HeaderValue};

use crate::executor::ScriptExecutor;
use crate::types::{ScriptRequest, ScriptResponse};

pub struct StubExecutor;

impl Default for StubExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl StubExecutor {
    pub fn new() -> Self {
        tracing::info!("StubExecutor initialized (benchmark mode)");
        Self
    }
}

impl ScriptExecutor for StubExecutor {
    fn execute(&self, _request: ScriptRequest) -> tokio::sync::oneshot::Receiver<ScriptResponse> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let _ = tx.send(ScriptResponse {
            status: 200,
            headers: vec![(
                HeaderName::from_static("content-type"),
                HeaderValue::from_static("text/plain"),
            )],
            body: Bytes::from_static(b"OK"),
            execution_time_us: 0,
        });
        rx
    }

    fn shutdown(&self) {
        // No-op
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    use bytes::Bytes;
    use http::{HeaderMap, Method, Uri};

    fn make_request() -> ScriptRequest {
        ScriptRequest {
            request_id: "test-1".to_string(),
            script_path: PathBuf::from("/var/www/html/test.php"),
            method: Method::GET,
            uri: Uri::from_static("/test.php"),
            query_string: String::new(),
            headers: HeaderMap::new(),
            body: Bytes::new(),
            remote_addr: "127.0.0.1:0".parse().unwrap(),
            document_root: Arc::new(PathBuf::from("/var/www/html")),
        }
    }

    #[tokio::test]
    async fn test_stub_executor_returns_200() {
        let executor = StubExecutor::new();
        let rx = executor.execute(make_request());
        let response = rx.await.unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, &b"OK"[..]);
        assert_eq!(response.headers.len(), 1);
        assert_eq!(response.headers[0].0, "content-type");
        assert_eq!(response.headers[0].1, "text/plain");
    }

    #[test]
    fn test_stub_executor_shutdown() {
        let executor = StubExecutor::new();
        executor.shutdown(); // should not panic
    }

    #[test]
    fn test_create_executor_stub() {
        std::env::set_var("EXECUTOR", "stub");
        let metrics = Arc::new(crate::metrics::Metrics::new());
        let executor = crate::executor::create_executor(metrics);
        // Verify it works by executing a request
        let rx = executor.execute(make_request());
        // The StubExecutor sends the response synchronously, so try_recv works
        let response = rx.blocking_recv().unwrap();
        assert_eq!(response.status, 200);
        std::env::remove_var("EXECUTOR");
    }
}
