//! Internal HTTP routes for the profiler plugin. See spec §8.

use std::sync::Arc;

use bytes::Bytes;
use http::{HeaderValue, Response, StatusCode};
use subtle::ConstantTimeEq;

use crate::plugin::handler::{PluginInternalHandler, PluginInternalRequest};
use crate::plugin::PluginContext;
use crate::types::{full_body, ResponseBody};

use super::storage::Storage;

mod path {
    pub const LANDING: &str = "/__profiler/";
    pub const CONFIG: &str = "/__profiler/config";
    pub const STATS: &str = "/__profiler/stats";
    pub const RUNS: &str = "/__profiler/runs";
    pub const RUNS_PREFIX: &str = "/__profiler/runs/";
}

pub fn register(
    ctx: &mut PluginContext<'_>,
    storage: Arc<Storage>,
    auth_token: Option<Arc<str>>,
    config_view: serde_json::Value,
) {
    let router = Arc::new(ProfilerRouter {
        storage,
        auth_token,
        config_view,
    });
    ctx.internal_route(path::LANDING, RouterHandler(Arc::clone(&router)));
    ctx.internal_route(path::CONFIG, RouterHandler(Arc::clone(&router)));
    ctx.internal_route(path::STATS, RouterHandler(Arc::clone(&router)));
    ctx.internal_route(path::RUNS, RouterHandler(Arc::clone(&router)));
    ctx.internal_route_prefix(path::RUNS_PREFIX, RouterHandler(Arc::clone(&router)));
}

pub(crate) struct ProfilerRouter {
    pub(crate) storage: Arc<Storage>,
    pub(crate) auth_token: Option<Arc<str>>,
    pub(crate) config_view: serde_json::Value,
}

pub(crate) struct RouterHandler(pub(crate) Arc<ProfilerRouter>);

impl PluginInternalHandler for RouterHandler {
    fn handle(&self, req: &PluginInternalRequest) -> Response<ResponseBody> {
        self.0.dispatch(req)
    }
}

impl ProfilerRouter {
    fn dispatch(&self, req: &PluginInternalRequest) -> Response<ResponseBody> {
        if let Some(bad) = self.check_auth(req) {
            return bad;
        }
        match (req.method.as_str(), req.path) {
            ("GET", path::LANDING) => self.landing(),
            ("GET", path::CONFIG) => self.config_page(),
            ("GET", path::STATS) => self.stats_page(),
            ("GET", path::RUNS) => self.list_runs(req.query),
            _ if req.path.starts_with(path::RUNS_PREFIX) => self.per_run(req),
            _ => text(StatusCode::NOT_FOUND, "404 not found"),
        }
    }

    fn check_auth(&self, req: &PluginInternalRequest) -> Option<Response<ResponseBody>> {
        let configured = self.auth_token.as_deref()?;
        let provided = req
            .header("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .unwrap_or("");
        if bool::from(configured.as_bytes().ct_eq(provided.as_bytes())) {
            None
        } else {
            let mut r = text(
                StatusCode::UNAUTHORIZED,
                "401 unauthorized - include `Authorization: Bearer <PROFILER_AUTH_TOKEN>`",
            );
            r.headers_mut().insert(
                http::header::WWW_AUTHENTICATE,
                HeaderValue::from_static("Bearer realm=\"profiler\""),
            );
            Some(r)
        }
    }

    fn landing(&self) -> Response<ResponseBody> {
        Response::builder()
            .status(StatusCode::OK)
            .header(http::header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(full_body(Bytes::from_static(LANDING_HTML)))
            .unwrap()
    }

    fn config_page(&self) -> Response<ResponseBody> {
        let mut redacted = self.config_view.clone();
        redact_in_place(&mut redacted);
        let body = serde_json::to_string_pretty(&redacted).unwrap_or_else(|_| "{}".into());
        json_response(StatusCode::OK, body)
    }

    fn stats_page(&self) -> Response<ResponseBody> {
        use std::sync::atomic::Ordering;
        let m = &self.storage.metrics;
        let body = serde_json::json!({
            "runs_total": {
                "header": m.runs_total[0].load(Ordering::Relaxed),
                "cookie": m.runs_total[1].load(Ordering::Relaxed),
                "query":  m.runs_total[2].load(Ordering::Relaxed),
                "sample": m.runs_total[3].load(Ordering::Relaxed),
            },
            "spans_collected_total": m.spans_collected_total.load(Ordering::Relaxed),
            "disk_drops_total": m.disk_drops_total.load(Ordering::Relaxed),
            "http_push_failures_total": m.http_push_failures_total.load(Ordering::Relaxed),
            "truncated_total": m.truncated_total.load(Ordering::Relaxed),
            "bytes_written_total": {
                "xhprof":     m.bytes_written_total[0].load(Ordering::Relaxed),
                "speedscope": m.bytes_written_total[1].load(Ordering::Relaxed),
                "pprof":      m.bytes_written_total[2].load(Ordering::Relaxed),
                "collapsed":  m.bytes_written_total[3].load(Ordering::Relaxed),
            },
            "in_memory_runs": self.storage.cache.len(),
        });
        json_response(StatusCode::OK, body.to_string())
    }

    fn list_runs(&self, query: Option<&str>) -> Response<ResponseBody> {
        let (limit, offset) = parse_pagination(query);

        let Some(disk) = self.storage.disk.as_deref() else {
            return json_response(
                StatusCode::OK,
                r#"{"runs":[],"total":0,"limit":0,"offset":0,"error":"disk storage disabled"}"#
                    .to_string(),
            );
        };
        let index = disk.output_dir.join("index.json");
        // TODO(async-discipline): PluginInternalHandler::handle is sync; we
        // cannot `.await` tokio::fs here. block_in_place requires a
        // multi_thread runtime and would panic in routes.rs unit tests which
        // invoke handle() outside any runtime. The internal admin server is
        // not on the main HTTP path — accepting the blocking read for now.
        let body = match std::fs::read_to_string(&index) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                tracing::warn!(
                    plugin = "profiler", path = %index.display(), error = %e,
                    "index.json read failed"
                );
                return text(StatusCode::INTERNAL_SERVER_ERROR, "index read failed");
            }
        };
        let mut runs: Vec<super::storage::RunMeta> = body
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        runs.sort_by_key(|r| std::cmp::Reverse(r.timestamp_ms));
        let total = runs.len();
        let page: Vec<_> = runs.into_iter().skip(offset).take(limit).collect();
        let body = serde_json::json!({
            "runs": page, "total": total, "limit": limit, "offset": offset,
        });
        json_response(StatusCode::OK, body.to_string())
    }

    fn per_run(&self, req: &PluginInternalRequest) -> Response<ResponseBody> {
        let tail = &req.path[path::RUNS_PREFIX.len()..];
        let (run_id, modifier) = split_run_id_modifier(tail);
        if !super::storage::disk::run_id_is_safe(run_id) {
            return text(StatusCode::BAD_REQUEST, "400 invalid run_id");
        }
        match (req.method.as_str(), modifier) {
            ("GET", RunModifier::None) => self.run_metadata_json(run_id),
            ("GET", RunModifier::Format(ext)) => self.run_format_bytes(run_id, ext),
            ("GET", RunModifier::Speedscope) => self.run_speedscope_redirect(req, run_id),
            ("DELETE", RunModifier::None) => self.run_delete(run_id),
            _ => text(StatusCode::METHOD_NOT_ALLOWED, "405 method not allowed"),
        }
    }

    fn run_format_bytes(&self, run_id: &str, ext: &str) -> Response<ResponseBody> {
        use super::storage::disk::OutputFormat;
        let Some(fmt) = OutputFormat::from_str_opt(ext) else {
            return text(StatusCode::NOT_FOUND, "404 unknown format");
        };
        let ct = content_type_for_format(fmt);
        if let Some(tree) = self.storage.cache.get(run_id) {
            let bytes = export_for(fmt, &tree);
            return Response::builder()
                .status(StatusCode::OK)
                .header(http::header::CONTENT_TYPE, ct)
                .body(full_body(Bytes::from(bytes)))
                .unwrap();
        }
        let Some(disk) = self.storage.disk.as_deref() else {
            return text(StatusCode::NOT_FOUND, "404 not in cache and disk disabled");
        };
        let file = disk
            .output_dir
            .join(format!("{}.{}", run_id, fmt.extension()));
        // TODO(async-discipline): see note in list_runs — sync fs on admin path.
        match std::fs::read(&file) {
            Ok(bytes) => Response::builder()
                .status(StatusCode::OK)
                .header(http::header::CONTENT_TYPE, ct)
                .body(full_body(Bytes::from(bytes)))
                .unwrap(),
            Err(_) => text(StatusCode::NOT_FOUND, "404 format not on disk"),
        }
    }

    fn run_speedscope_redirect(
        &self,
        req: &PluginInternalRequest,
        run_id: &str,
    ) -> Response<ResponseBody> {
        let host = req
            .header("host")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("127.0.0.1");
        let profile_url = format!("http://{}/__profiler/runs/{}.speedscope.json", host, run_id);
        let location = format!(
            "https://www.speedscope.app/#profileURL={}",
            percent_encode(&profile_url)
        );
        Response::builder()
            .status(StatusCode::FOUND)
            .header(http::header::LOCATION, location)
            .body(full_body(Bytes::new()))
            .unwrap()
    }

    fn run_delete(&self, run_id: &str) -> Response<ResponseBody> {
        let Some(disk) = self.storage.disk.as_deref() else {
            return text(StatusCode::NOT_FOUND, "404 disk storage disabled");
        };
        let index_path = disk.output_dir.join("index.json");
        // Serialize with DiskWriter's append path and the retention
        // sweep. The plugin trait calls handle() synchronously, but the
        // internal HTTP server itself runs the dispatcher inside hyper's
        // async service — so this thread IS a tokio worker. A naked
        // `blocking_lock()` panics with "Cannot block the current thread
        // from within a runtime". A try_lock retry with a hard deadline
        // is correct in every runtime (current_thread or multi_thread)
        // and degrades to 503 instead of panicking when truly contended.
        let _guard = {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                if let Ok(g) = disk.index_lock.try_lock() {
                    break g;
                }
                if std::time::Instant::now() >= deadline {
                    return text(StatusCode::SERVICE_UNAVAILABLE, "503 index lock contended");
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        };
        // TODO(async-discipline): sync fs in sync trait handler; see list_runs.
        let contents = match std::fs::read_to_string(&index_path) {
            Ok(s) => s,
            Err(_) => return text(StatusCode::NOT_FOUND, "404 no runs"),
        };
        let mut found: Option<super::storage::RunMeta> = None;
        let mut kept = String::new();
        for line in contents.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(meta) = serde_json::from_str::<super::storage::RunMeta>(line) else {
                kept.push_str(line);
                kept.push('\n');
                continue;
            };
            if meta.run_id == run_id {
                found = Some(meta);
            } else {
                kept.push_str(line);
                kept.push('\n');
            }
        }
        let Some(meta) = found else {
            return text(StatusCode::NOT_FOUND, "404 run not found");
        };
        let tmp_path = disk.output_dir.join("index.json.tmp");
        // TODO(async-discipline): sync write+rename+remove block below; see list_runs.
        if let Err(e) = std::fs::write(&tmp_path, kept.as_bytes()) {
            tracing::warn!(
                plugin = "profiler", error = %e, path = %tmp_path.display(),
                "index.json.tmp write failed during DELETE"
            );
            return text(
                StatusCode::INTERNAL_SERVER_ERROR,
                "500 index rewrite failed",
            );
        }
        if let Err(e) = std::fs::rename(&tmp_path, &index_path) {
            tracing::warn!(
                plugin = "profiler", error = %e,
                "index.json rename failed during DELETE"
            );
            return text(
                StatusCode::INTERNAL_SERVER_ERROR,
                "500 index rewrite failed",
            );
        }
        for fmt_ext in &meta.formats {
            let p = disk.output_dir.join(format!("{}.{}", run_id, fmt_ext));
            let _ = std::fs::remove_file(&p);
        }
        Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(full_body(Bytes::new()))
            .unwrap()
    }

    fn run_metadata_json(&self, run_id: &str) -> Response<ResponseBody> {
        let Some(disk) = self.storage.disk.as_deref() else {
            return text(StatusCode::NOT_FOUND, "404 no disk storage");
        };
        // TODO(async-discipline): sync fs in sync trait handler; see list_runs.
        let body = match std::fs::read_to_string(disk.output_dir.join("index.json")) {
            Ok(s) => s,
            Err(_) => return text(StatusCode::NOT_FOUND, "404 run not found"),
        };
        for line in body.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(meta) = serde_json::from_str::<super::storage::RunMeta>(line) else {
                continue;
            };
            if meta.run_id == run_id {
                return json_response(
                    StatusCode::OK,
                    serde_json::to_string(&meta).unwrap_or_else(|_| "{}".into()),
                );
            }
        }
        text(StatusCode::NOT_FOUND, "404 run not found")
    }
}

fn parse_pagination(query: Option<&str>) -> (usize, usize) {
    const DEFAULT_LIMIT: usize = 100;
    const MAX_LIMIT: usize = 1000;
    let mut limit = DEFAULT_LIMIT;
    let mut offset = 0usize;
    let Some(q) = query else {
        return (limit, offset);
    };
    for pair in q.split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        match k {
            "limit" => {
                if let Ok(n) = v.parse::<usize>() {
                    limit = n.min(MAX_LIMIT);
                }
            }
            "offset" => {
                if let Ok(n) = v.parse::<usize>() {
                    offset = n;
                }
            }
            _ => {}
        }
    }
    (limit, offset)
}

#[derive(Debug, PartialEq, Eq)]
enum RunModifier<'a> {
    None,
    Format(&'a str),
    Speedscope,
}

fn split_run_id_modifier(tail: &str) -> (&str, RunModifier<'_>) {
    if let Some((id, fmt)) = tail.split_once('.') {
        return (id, RunModifier::Format(fmt));
    }
    if let Some((id, rest)) = tail.split_once('/') {
        if rest == "speedscope" {
            return (id, RunModifier::Speedscope);
        }
    }
    (tail, RunModifier::None)
}

fn text(status: StatusCode, body: &'static str) -> Response<ResponseBody> {
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(full_body(Bytes::from_static(body.as_bytes())))
        .unwrap()
}

fn json_response(status: StatusCode, body: String) -> Response<ResponseBody> {
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(full_body(Bytes::from(body)))
        .unwrap()
}

fn content_type_for_format(fmt: super::storage::disk::OutputFormat) -> &'static str {
    use super::storage::disk::OutputFormat;
    match fmt {
        OutputFormat::Xhprof => "application/json",
        OutputFormat::Speedscope => "application/json",
        OutputFormat::Pprof => "application/octet-stream",
        OutputFormat::Collapsed => "text/plain; charset=utf-8",
    }
}

fn export_for(
    fmt: super::storage::disk::OutputFormat,
    tree: &std::sync::Arc<crate::profiling::SpanTree>,
) -> Vec<u8> {
    use super::storage::disk::OutputFormat;
    use crate::profiling::export::*;
    match fmt {
        OutputFormat::Xhprof => export_xhprof(tree, XhprofMode::Raw, None),
        OutputFormat::Speedscope => export_speedscope(tree),
        OutputFormat::Pprof => export_pprof(tree),
        OutputFormat::Collapsed => export_collapsed(tree, CollapsedMetric::Wall),
    }
}

fn percent_encode(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.' || b == b'~' {
            out.push(b as char);
        } else {
            let _ = write!(&mut out, "%{:02X}", b);
        }
    }
    out
}

fn redact_in_place(v: &mut serde_json::Value) {
    if let serde_json::Value::Object(map) = v {
        for (k, val) in map.iter_mut() {
            let kl = k.to_ascii_lowercase();
            if kl.ends_with("_token") || kl.ends_with("_key") || kl == "password" {
                *val = serde_json::Value::String("<redacted>".into());
            } else {
                redact_in_place(val);
            }
        }
    }
}

const LANDING_HTML: &[u8] = br#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>OxPHP Profiler</title>
<style>body{font:14px/1.5 system-ui,sans-serif;max-width:720px;margin:2em auto;padding:0 1em}
code{background:#f3f3f3;padding:.1em .3em;border-radius:3px}
a{color:#0366d6;text-decoration:none}a:hover{text-decoration:underline}
table{border-collapse:collapse;margin-top:1em}th,td{text-align:left;padding:.35em .75em;border-bottom:1px solid #eee}</style></head>
<body><h1>OxPHP Profiler</h1>
<p>Internal endpoints for the <code>ox_profiler</code> plugin. Send
<code>Authorization: Bearer &lt;PROFILER_AUTH_TOKEN&gt;</code> when a token is configured.</p>
<table>
<tr><th>Route</th><th>Purpose</th></tr>
<tr><td><a href="./runs">GET /runs</a></td><td>List captured runs (<code>?limit=N&amp;offset=M</code>).</td></tr>
<tr><td><code>GET /runs/{run_id}</code></td><td>JSON metadata for one run.</td></tr>
<tr><td><code>GET /runs/{run_id}.{xhprof.json|speedscope.json|pprof|collapsed}</code></td><td>Raw profile bytes.</td></tr>
<tr><td><code>GET /runs/{run_id}/speedscope</code></td><td>302 to speedscope.app.</td></tr>
<tr><td><code>DELETE /runs/{run_id}</code></td><td>Remove all format files + index entry.</td></tr>
<tr><td><a href="./config">GET /config</a></td><td>Active plugin configuration (tokens redacted).</td></tr>
<tr><td><a href="./stats">GET /stats</a></td><td>Counter snapshot in JSON.</td></tr>
</table></body></html>
"#;

/// Test-only constructor used by `tests/profiler_routes_tests.rs`.
///
/// Not gated by `#[cfg(test)]` because integration tests are a separate
/// crate that links against `oxphp` without the `test` cfg. Hidden from
/// rustdoc and from the dead-code lint — it's intentionally unused
/// inside the library itself.
#[doc(hidden)]
#[allow(dead_code)]
pub fn test_new_router(
    storage: Arc<Storage>,
    auth_token: Option<Arc<str>>,
    config_view: serde_json::Value,
) -> Arc<dyn PluginInternalHandler> {
    Arc::new(RouterHandler(Arc::new(ProfilerRouter {
        storage,
        auth_token,
        config_view,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::ox_profiler::storage::{ProfileCache, StorageMetrics};

    fn dummy_router(auth: Option<&str>) -> ProfilerRouter {
        let metrics = StorageMetrics::new();
        let cache = Arc::new(ProfileCache::new(8));
        let storage = Arc::new(Storage {
            cache,
            disk: None,
            http: None,
            metrics,
        });
        ProfilerRouter {
            storage,
            auth_token: auth.map(Arc::<str>::from),
            config_view: serde_json::json!({ "enabled": true, "auth_token": "secret" }),
        }
    }

    #[test]
    fn auth_passes_when_no_token_configured() {
        let r = dummy_router(None);
        let headers = http::HeaderMap::new();
        let req = PluginInternalRequest {
            method: &http::Method::GET,
            path: "/__profiler/stats",
            headers: &headers,
            query: None,
        };
        assert!(r.check_auth(&req).is_none());
    }

    #[test]
    fn auth_rejects_missing_header() {
        let r = dummy_router(Some("s3cret"));
        let headers = http::HeaderMap::new();
        let req = PluginInternalRequest {
            method: &http::Method::GET,
            path: "/__profiler/stats",
            headers: &headers,
            query: None,
        };
        assert_eq!(
            r.check_auth(&req).unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn auth_rejects_wrong_token() {
        let r = dummy_router(Some("s3cret"));
        let mut headers = http::HeaderMap::new();
        headers.insert("authorization", "Bearer nope".parse().unwrap());
        let req = PluginInternalRequest {
            method: &http::Method::GET,
            path: "/__profiler/stats",
            headers: &headers,
            query: None,
        };
        assert_eq!(
            r.check_auth(&req).unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn auth_accepts_correct_token() {
        let r = dummy_router(Some("s3cret"));
        let mut headers = http::HeaderMap::new();
        headers.insert("authorization", "Bearer s3cret".parse().unwrap());
        let req = PluginInternalRequest {
            method: &http::Method::GET,
            path: "/__profiler/stats",
            headers: &headers,
            query: None,
        };
        assert!(r.check_auth(&req).is_none());
    }

    #[test]
    fn landing_is_html() {
        let r = dummy_router(None);
        let resp = r.landing();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp.headers().get(http::header::CONTENT_TYPE).unwrap();
        assert!(ct.to_str().unwrap().starts_with("text/html"));
    }

    #[test]
    fn config_redacts_tokens() {
        let r = dummy_router(Some("s3cret"));
        let resp = r.config_page();
        assert_eq!(resp.status(), StatusCode::OK);
        let mut v = r.config_view.clone();
        redact_in_place(&mut v);
        assert_eq!(
            v["auth_token"],
            serde_json::Value::String("<redacted>".into())
        );
    }

    #[test]
    fn stats_returns_application_json() {
        let r = dummy_router(None);
        let resp = r.stats_page();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp.headers().get(http::header::CONTENT_TYPE).unwrap();
        assert_eq!(ct, "application/json");
    }

    #[test]
    fn split_run_id_modifier_variants() {
        assert_eq!(split_run_id_modifier("abc"), ("abc", RunModifier::None));
        assert_eq!(
            split_run_id_modifier("abc.xhprof.json"),
            ("abc", RunModifier::Format("xhprof.json"))
        );
        assert_eq!(
            split_run_id_modifier("abc.pprof"),
            ("abc", RunModifier::Format("pprof"))
        );
        assert_eq!(
            split_run_id_modifier("abc/speedscope"),
            ("abc", RunModifier::Speedscope)
        );
        // Unknown tail — treat full tail as id; run_id_is_safe will reject it at the call site.
        assert_eq!(
            split_run_id_modifier("abc/other"),
            ("abc/other", RunModifier::None)
        );
    }

    #[test]
    fn parse_pagination_caps_limit_and_parses_offset() {
        assert_eq!(parse_pagination(None), (100, 0));
        assert_eq!(parse_pagination(Some("limit=50")), (50, 0));
        assert_eq!(parse_pagination(Some("limit=5000")), (1000, 0));
        assert_eq!(parse_pagination(Some("limit=25&offset=10")), (25, 10));
        assert_eq!(parse_pagination(Some("garbage=x")), (100, 0));
        assert_eq!(parse_pagination(Some("limit=abc&offset=xyz")), (100, 0));
    }

    #[test]
    fn percent_encode_basic() {
        assert_eq!(percent_encode("abc_123.~"), "abc_123.~");
        assert_eq!(percent_encode("/foo bar"), "%2Ffoo%20bar");
        assert_eq!(percent_encode(":/?#"), "%3A%2F%3F%23");
    }

    #[test]
    fn content_type_for_each_format() {
        use super::super::storage::disk::OutputFormat;
        assert_eq!(
            content_type_for_format(OutputFormat::Xhprof),
            "application/json"
        );
        assert_eq!(
            content_type_for_format(OutputFormat::Speedscope),
            "application/json"
        );
        assert_eq!(
            content_type_for_format(OutputFormat::Pprof),
            "application/octet-stream"
        );
        assert!(content_type_for_format(OutputFormat::Collapsed).starts_with("text/plain"));
    }
}
