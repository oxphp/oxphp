use std::path::{Path, PathBuf};

use futures_util::future::BoxFuture;

use super::{ModeRouter, ResolveCtx, RouteResult};

/// SPA routing — `INDEX_FILE="index.html"` (single-page application).
///
/// Nginx equivalent:
/// ```nginx
/// location ~ \.php$ { try_files $uri =404; }
/// location ~ \.     { try_files $uri =404; }
/// location /        { try_files /index.html =404; }  // no-ext → straight to index.html
/// ```
///
/// Semantics per URI kind:
/// - `UriKind::Php` — execute if file exists, else hard 404
/// - `UriKind::OtherExtension` — serve if file exists (common layer), else hard 404
/// - `UriKind::NoExtension` — serve `/index.html` directly, no disk probe of `$uri`
pub(crate) struct SpaRouter {
    index_file_path: PathBuf,
    index_file_key: String,
}

impl SpaRouter {
    pub(crate) fn new(document_root: &Path, index_file: &str) -> Self {
        let index_file_path = document_root.join(index_file);
        let index_file_key = index_file_path.to_string_lossy().into_owned();
        Self {
            index_file_path,
            index_file_key,
        }
    }
}

impl ModeRouter for SpaRouter {
    fn resolve_no_extension<'a>(
        &'a self,
        _sanitized: &'a str,
        ctx: &'a ResolveCtx<'a>,
    ) -> BoxFuture<'a, RouteResult> {
        Box::pin(async move {
            // No-extension URIs always resolve to the SPA index; the common
            // layer never probes disk for `$uri` in this branch.
            if ctx.file_cache.is_file(&self.index_file_key).await {
                return RouteResult::Serve(self.index_file_path.clone());
            }
            if let Some(wr) = ctx.worker_route {
                return wr.clone();
            }
            RouteResult::NotFound
        })
    }

    fn resolve_php<'a>(
        &'a self,
        sanitized: &'a str,
        ctx: &'a ResolveCtx<'a>,
    ) -> BoxFuture<'a, RouteResult> {
        Box::pin(async move {
            // Execute the PHP script only if it exists exactly as addressed.
            // No PATH_INFO split, no fallback to index.html — hard 404.
            let file_path = ctx.document_root.join(sanitized);
            if ctx.file_cache.is_file(&file_path.to_string_lossy()).await {
                return RouteResult::Execute(file_path, None);
            }
            RouteResult::NotFound
        })
    }

    fn resolve_static_miss<'a>(
        &'a self,
        _sanitized: &'a str,
        _ctx: &'a ResolveCtx<'a>,
    ) -> BoxFuture<'a, RouteResult> {
        Box::pin(async move { RouteResult::NotFound })
    }
}
