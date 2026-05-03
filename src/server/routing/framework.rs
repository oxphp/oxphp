use std::path::{Path, PathBuf};

use super::{ResolveCtx, RouteResult};

/// Framework routing — `*.php` `ENTRY_FILE` (single front controller).
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
    index_file_key: String,
}

impl FrameworkRouter {
    pub(crate) fn new(document_root: &Path, index_file: &str) -> Self {
        let index_file_path = document_root.join(index_file);
        let index_file_key = index_file_path.to_string_lossy().into_owned();
        Self {
            index_file_path,
            index_file_key,
        }
    }

    /// Rewrite target: `Execute(index.php)` with PATH_INFO=`/` + sanitized.
    /// Falls back to `worker_route` when the front controller is missing on
    /// disk, otherwise returns `NotFound`. The file_cache probe is O(1) on
    /// cache hit — in Framework mode the same `index.php` is resolved on
    /// every request, so the entry stays pinned in the meta cache.
    async fn rewrite(&self, sanitized: &str, ctx: &ResolveCtx<'_>) -> RouteResult {
        if !ctx.file_cache.is_file(&self.index_file_key).await {
            // Front controller missing — admin-configured worker wins
            // if present, otherwise hard 404.
            if let Some(wr) = ctx.worker_route {
                return wr.clone();
            }
            return RouteResult::NotFound;
        }

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

        RouteResult::Execute(self.index_file_path.clone(), Some(path_info), None)
    }
}

impl FrameworkRouter {
    pub(crate) async fn resolve_no_extension(
        &self,
        sanitized: &str,
        ctx: &ResolveCtx<'_>,
    ) -> RouteResult {
        self.rewrite(sanitized, ctx).await
    }

    pub(crate) async fn resolve_php(&self, sanitized: &str, ctx: &ResolveCtx<'_>) -> RouteResult {
        self.rewrite(sanitized, ctx).await
    }
}
