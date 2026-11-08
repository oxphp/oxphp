//! HTTP push for captured profiles. Single PROFILER_EXPORT_URL +
//! PROFILER_EXPORT_FORMAT. Shared reqwest::Client (rustls-tls).
//! Bearer auth via PROFILER_EXPORT_AUTH_TOKEN. 3× retry with
//! exponential backoff (100/200/400 ms), 5-second wallclock cap.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use rand::RngExt;

use crate::profiling::SpanTree;

use super::disk::OutputFormat;
use super::RunMeta;

const RETRY_BACKOFFS: [Duration; 3] = [
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
];
const TOTAL_TIMEOUT: Duration = Duration::from_secs(5);

pub struct HttpPusher {
    pub url: String,
    pub format: OutputFormat,
    pub auth_token: Option<String>,
    client: reqwest::Client,
    metrics: Arc<super::StorageMetrics>,
}

impl HttpPusher {
    /// Build a pusher with a long-lived shared `reqwest::Client`.
    pub fn new(
        url: String,
        format: OutputFormat,
        auth_token: Option<String>,
        metrics: Arc<super::StorageMetrics>,
    ) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder().timeout(TOTAL_TIMEOUT).build()?;
        Ok(Self {
            url,
            format,
            auth_token,
            client,
            metrics,
        })
    }

    /// Render + push the chosen format. Retries up to 3 times with
    /// exponential backoff, capped at 5 s total wallclock. Failures
    /// are logged and the profile is dropped (does not block).
    pub async fn push(self: &Arc<Self>, meta: &RunMeta, tree: &Arc<SpanTree>) {
        let bytes = self.format.render(tree);
        // Wrap the rendered payload in `Bytes` once so retries are
        // cheap refcount bumps rather than per-attempt clones of an
        // MB-scale profile. `reqwest::Body::from(Bytes)` is zero-copy.
        let body = Bytes::from(bytes);
        let content_type = content_type_for(self.format);
        let started = Instant::now();
        let mut attempt: u32 = 0;

        loop {
            attempt += 1;
            let mut req = self
                .client
                .post(&self.url)
                .header("Content-Type", content_type)
                .body(body.clone());
            if let Some(ref token) = self.auth_token {
                req = req.bearer_auth(token);
            }
            let result = req.send().await;
            match result {
                Ok(resp) if resp.status().is_success() => {
                    tracing::debug!(
                        plugin = "profiler",
                        run_id = %meta.run_id,
                        attempt,
                        status = resp.status().as_u16(),
                        "http push ok"
                    );
                    return;
                }
                Ok(resp) => {
                    tracing::warn!(
                        plugin = "profiler",
                        run_id = %meta.run_id,
                        attempt,
                        status = resp.status().as_u16(),
                        "http push non-success status"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        plugin = "profiler",
                        run_id = %meta.run_id,
                        attempt,
                        error = %e,
                        "http push attempt failed"
                    );
                }
            }
            if attempt as usize >= RETRY_BACKOFFS.len() || started.elapsed() >= TOTAL_TIMEOUT {
                tracing::error!(
                    plugin = "profiler",
                    run_id = %meta.run_id,
                    attempts = attempt,
                    "http push failed after retries; dropping"
                );
                self.metrics.inc_http_push_failure();
                return;
            }
            // ±20% jitter on the backoff constant so a backend flap
            // doesn't resync all pushers into a thundering herd.
            let base = RETRY_BACKOFFS[attempt as usize - 1];
            let factor: f64 = rand::rng().random_range(0.8..1.2);
            let delay = base.mul_f64(factor);
            tokio::time::sleep(delay).await;
        }
    }
}

fn content_type_for(fmt: OutputFormat) -> &'static str {
    match fmt {
        OutputFormat::Xhprof => "application/json",
        OutputFormat::Collapsed => "text/plain",
    }
}
