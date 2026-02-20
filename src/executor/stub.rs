use bytes::Bytes;
use http::{HeaderName, HeaderValue};

use crate::executor::{ExecuteResult, ScriptExecutor};
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
    fn execute(&self, _request: ScriptRequest) -> ExecuteResult {
        ExecuteResult::Immediate(ScriptResponse {
            status: 200,
            headers: vec![(
                HeaderName::from_static("content-type"),
                HeaderValue::from_static("text/plain"),
            )],
            body: Bytes::from_static(b"OxPHP Stub"),
            execution_time_us: 0,
        })
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

    #[test]
    fn test_stub_executor_returns_200() {
        let executor = StubExecutor::new();
        let result = executor.execute(make_request());
        let response = match result {
            ExecuteResult::Immediate(resp) => resp,
            ExecuteResult::Deferred(_) => panic!("StubExecutor should return Immediate"),
        };
        assert_eq!(response.status, 200);
        assert_eq!(response.body, &b"OxPHP Stub"[..]);
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
        let result = executor.execute(make_request());
        let response = match result {
            ExecuteResult::Immediate(resp) => resp,
            ExecuteResult::Deferred(rx) => rx.blocking_recv().unwrap(),
        };
        assert_eq!(response.status, 200);
        std::env::remove_var("EXECUTOR");
    }
}
