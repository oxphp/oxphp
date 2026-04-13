use std::path::{Path, PathBuf};

use futures_util::future::BoxFuture;

use super::{ModeRouter, ResolveCtx, RouteResult};

/// Framework routing — `INDEX_FILE="index.php"` (single front controller).
///
/// Nginx equivalent:
/// ```nginx
/// location ~ \.(?!php$)[a-zA-Z0-9]+$ { try_files $uri =404; }
/// location / { rewrite ^ /index.php last; }
/// location = /index.php { fastcgi_param PATH_INFO $request_uri; ... }
/// ```
///
/// Semantics:
/// - Any request with a non-.php extension → must be a real static file on disk
///   (handled by common layer's disk check for `UriKind::OtherExtension`);
///   miss → hard 404.
/// - Everything else (no extension, `.php`, or `.php/extra`) → rewrites to
///   `/index.php` with `PATH_INFO` set to the original URI.
pub(crate) struct FrameworkRouter {
    index_file_path: PathBuf,
}

impl FrameworkRouter {
    pub(crate) fn new(document_root: &Path, index_file: &str) -> Self {
        Self {
            index_file_path: document_root.join(index_file),
        }
    }

    /// Rewrite target: always Execute(index.php) with PATH_INFO=`/` + sanitized.
    /// Falls back to `worker_route` if the front controller is missing.
    fn rewrite(&self, sanitized: &str, ctx: &ResolveCtx<'_>) -> RouteResult {
        // PATH_INFO carries the original URI (with leading `/`). Empty sanitized
        // means root — PATH_INFO is `/`.
        let path_info = if sanitized.is_empty() {
            String::from("/")
        } else {
            let mut s = String::with_capacity(sanitized.len() + 1);
            s.push('/');
            s.push_str(sanitized);
            s
        };

        // If someone removes index.php but still has a worker configured,
        // worker takes over. Disk check for the front controller is deferred
        // to the executor layer (same as before this refactor).
        if let Some(wr) = ctx.worker_route {
            return wr.clone();
        }
        RouteResult::Execute(self.index_file_path.clone(), Some(path_info))
    }
}

impl ModeRouter for FrameworkRouter {
    fn resolve_no_extension<'a>(
        &'a self,
        sanitized: &'a str,
        ctx: &'a ResolveCtx<'a>,
    ) -> BoxFuture<'a, RouteResult> {
        Box::pin(async move { self.rewrite(sanitized, ctx) })
    }

    fn resolve_php<'a>(
        &'a self,
        sanitized: &'a str,
        ctx: &'a ResolveCtx<'a>,
    ) -> BoxFuture<'a, RouteResult> {
        Box::pin(async move { self.rewrite(sanitized, ctx) })
    }

    fn resolve_static_miss<'a>(
        &'a self,
        _sanitized: &'a str,
        _ctx: &'a ResolveCtx<'a>,
    ) -> BoxFuture<'a, RouteResult> {
        Box::pin(async move { RouteResult::NotFound })
    }
}
