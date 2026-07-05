//! HTTP push for captured profiles. Single PROFILER_EXPORT_URL +
//! PROFILER_EXPORT_FORMAT. Shared reqwest::Client (rustls-tls).
//! Bearer auth via PROFILER_EXPORT_AUTH_TOKEN. 3× retry with
//! exponential backoff (100/200/400 ms), 5-second wallclock cap.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use rand::RngExt;

use crate::profiling::export::{
    export_collapsed, export_pprof, export_speedscope, export_xhprof, export_xhprof_buggregator,
    BuggregatorMeta, CollapsedMetric, XhprofMode,
};
use crate::profiling::SpanTree;

use super::disk::OutputFormat;
use super::RunMeta;

const RETRY_BACKOFFS: [Duration; 3] = [
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
];
const TOTAL_TIMEOUT: Duration = Duration::from_secs(5);

/// The push-time xhprof envelope, resolved once from config so that "xhgui and
/// buggregator at the same time" is unrepresentable: the pusher stores exactly
/// one variant and `render` matches it. The Buggregator variant carries its
/// prebuilt [`BuggregatorMeta`] (app_name/tags/hostname); the per-request
/// `date` is filled at push time. For non-xhprof formats the variant is
/// irrelevant.
#[derive(Debug, Clone)]
enum PushEnvelope {
    Raw,
    Xhgui,
    Buggregator(BuggregatorMeta),
}

pub struct HttpPusher {
    pub url: String,
    pub format: OutputFormat,
    pub auth_token: Option<String>,
    envelope: PushEnvelope,
    client: reqwest::Client,
    metrics: Arc<super::StorageMetrics>,
}

impl HttpPusher {
    /// Build a pusher with a long-lived shared `reqwest::Client`. The envelope
    /// is decided by the config layer and collapsed here into a single
    /// [`PushEnvelope`]: a `Some(buggregator)` template wins over
    /// `xhgui_envelope` (config never sets both), and the pusher does no URL
    /// re-detection. The Buggregator template is prebuilt by the caller so
    /// nothing is rebuilt per push.
    pub fn new(
        url: String,
        format: OutputFormat,
        auth_token: Option<String>,
        xhgui_envelope: bool,
        buggregator: Option<BuggregatorMeta>,
        metrics: Arc<super::StorageMetrics>,
    ) -> Result<Self, reqwest::Error> {
        let envelope = match buggregator {
            Some(meta) => PushEnvelope::Buggregator(meta),
            None if xhgui_envelope => PushEnvelope::Xhgui,
            None => PushEnvelope::Raw,
        };
        let client = reqwest::Client::builder().timeout(TOTAL_TIMEOUT).build()?;
        Ok(Self {
            url,
            format,
            auth_token,
            envelope,
            client,
            metrics,
        })
    }

    /// Render + push the chosen format. Retries up to 3 times with
    /// exponential backoff, capped at 5 s total wallclock. Failures
    /// are logged and the profile is dropped (does not block).
    pub async fn push(self: &Arc<Self>, meta: &RunMeta, tree: &Arc<SpanTree>) {
        // pprof's gzip step takes milliseconds — detour it through the
        // blocking pool so the Tokio reactor isn't stalled. Other
        // formats are pure serde_json and render inline.
        // Only a Raw-envelope pprof push actually renders protobuf+gzip; an
        // active xhgui/buggregator envelope always emits JSON (fast), so it
        // renders inline regardless of the format knob.
        let renders_pprof =
            matches!(self.envelope, PushEnvelope::Raw) && self.format == OutputFormat::Pprof;
        let bytes = if renders_pprof {
            let tree_c = Arc::clone(tree);
            let meta_c = meta.clone();
            let fmt = self.format;
            // pprof's gzip is CPU-heavy — detour it off the reactor.
            match tokio::task::spawn_blocking(move || {
                render(&tree_c, &meta_c, fmt, &PushEnvelope::Raw)
            })
            .await
            {
                Ok(b) => b,
                Err(e) => {
                    tracing::error!(
                        plugin = "profiler",
                        run_id = %meta.run_id,
                        error = %e,
                        "pprof render task panicked; dropping push"
                    );
                    self.metrics.inc_http_push_failure();
                    return;
                }
            }
        } else {
            render(tree, meta, self.format, &self.envelope)
        };
        // Wrap the rendered payload in `Bytes` once so retries are
        // cheap refcount bumps rather than per-attempt clones of an
        // MB-scale profile. `reqwest::Body::from(Bytes)` is zero-copy.
        let body = Bytes::from(bytes);
        // Content-Type must follow the *rendered body*, not the format knob: an
        // active xhgui/buggregator envelope always emits JSON even when
        // PROFILER_EXPORT_FORMAT names pprof (protobuf) or collapsed (text), so
        // tagging the push by `self.format` would mislabel the body and get it
        // rejected by a strict receiver.
        let content_type = match self.envelope {
            PushEnvelope::Raw => content_type_for(self.format),
            PushEnvelope::Xhgui | PushEnvelope::Buggregator(_) => "application/json",
        };
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

fn render(tree: &SpanTree, meta: &RunMeta, fmt: OutputFormat, envelope: &PushEnvelope) -> Vec<u8> {
    match envelope {
        // xhgui and buggregator are xhprof-based envelopes: they always emit
        // their xhprof body regardless of PROFILER_EXPORT_FORMAT. (Config warns
        // when a non-xhprof format is set alongside an envelope, since the
        // format knob is ignored here — that beats either crashing startup or
        // silently pushing an un-enveloped body the receiver can't parse.)
        PushEnvelope::Buggregator(bmeta) => {
            export_xhprof_buggregator(tree, bmeta, meta.timestamp_secs())
        }
        PushEnvelope::Xhgui => export_xhprof(tree, XhprofMode::Xhgui, Some(meta.to_xhgui_meta())),
        // Raw: PROFILER_EXPORT_FORMAT selects the serializer.
        PushEnvelope::Raw => match fmt {
            OutputFormat::Xhprof => export_xhprof(tree, XhprofMode::Raw, None),
            OutputFormat::Speedscope => export_speedscope(tree),
            OutputFormat::Pprof => export_pprof(tree),
            OutputFormat::Collapsed => export_collapsed(tree, CollapsedMetric::Wall),
        },
    }
}

fn content_type_for(fmt: OutputFormat) -> &'static str {
    match fmt {
        OutputFormat::Xhprof | OutputFormat::Speedscope => "application/json",
        OutputFormat::Pprof => "application/vnd.google.pprof+proto+gzip",
        OutputFormat::Collapsed => "text/plain",
    }
}
