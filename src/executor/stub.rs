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
            body: Bytes::from_static(b"OK"),
            execution_time_us: 0,
            stream_rx: None,
            errors: Vec::new(),
            apm_spans_json: None,
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
            timeout_us: 0,
            trace_id: String::new(),
            span_id: String::new(),
            parent_span_id: String::new(),
            is_tls: false,
            version: http::Version::HTTP_11,
            path_info: None,
            forwarded_proto: None,
            forwarded_host: None,
            denied_meta: None,
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
        let config = crate::config::Config::test_minimal();
        assert_eq!(config.executor_type, "stub");
        let metrics = Arc::new(crate::metrics::Metrics::new());
        let executor = crate::executor::create_executor(&config, metrics);
        let result = executor.execute(make_request());
        let response = match result {
            ExecuteResult::Immediate(resp) => resp,
            ExecuteResult::Deferred(rx) => rx.blocking_recv().unwrap(),
        };
        assert_eq!(response.status, 200);
    }
}
