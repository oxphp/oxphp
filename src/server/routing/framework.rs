use std::path::{Path, PathBuf};

use super::{ResolveCtx, RouteResult};

/// Framework routing — `*.php` `ENTRY_FILE` (single front controller).
///
/// Nginx equivalent:
/// ```nginx
/// location ~ \.(?!php$)[a-zA-Z0-9]+$ { try_files $uri /index.php; }
/// location / { rewrite ^ /index.php last; }
/// location = /index.php {
///     fastcgi_split_path_info ^(.+\.php)(/.*)$;
///     fastcgi_param PATH_INFO $fastcgi_path_info; ...
/// }
/// ```
///
/// Semantics:
/// - A request with a non-.php extension is served as a static file when it
///   exists on disk (handled by the common layer's disk check for
///   `UriKind::OtherExtension`); on a miss it falls back to the front
///   controller, matching the canonical `try_files $uri /index.php` config.
/// - Everything else (no extension, `.php`, or `.php/extra`) → rewrites to
///   `/index.php`. `PATH_INFO` is set only when the request explicitly names
///   the entry file with a trailing segment (`/index.php/extra` → `/extra`);
///   for app routes the original path is exposed via `REQUEST_URI`.
pub(crate) struct FrameworkRouter {
    index_file_path: PathBuf,
    index_file_key: String,
    /// Front-controller filename, e.g. `index.php` (no leading slash, matching
    /// the leading-slash-free sanitized request path). Used to split honest CGI
    /// PATH_INFO from an explicit `/index.php/extra` request.
    entry_segment: String,
}

impl FrameworkRouter {
    pub(crate) fn new(document_root: &Path, index_file: &str) -> Self {
        let index_file_path = document_root.join(index_file);
        let index_file_key = index_file_path.to_string_lossy().into_owned();
        Self {
            index_file_path,
            index_file_key,
            entry_segment: index_file.to_string(),
        }
    }

    /// Rewrite target: `Execute(index.php)`. PATH_INFO is set only for an
    /// explicit `/index.php/extra` request (honest CGI split), else `None`.
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

        // Honest CGI PATH_INFO: set only when the entry file is explicitly
        // addressed with a trailing segment (`/index.php/news` → `/news`).
        // A bare app route (`/users/42`), the entry itself (`/index.php`), or a
        // trailing slash (`/index.php/`, which `sanitize` collapses) carries no
        // PATH_INFO — the original path lives in REQUEST_URI. `sanitized` has no
        // leading slash, so match against the entry segment (the front-
        // controller filename); the remainder must open a new `/` segment.
        // This allocates only on a real match — the common case skips it.
        let path_info = sanitized
            .strip_prefix(self.entry_segment.as_str())
            .filter(|rest| rest.starts_with('/'))
            .map(str::to_string);

        RouteResult::Execute(self.index_file_path.clone(), path_info, None)
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

    /// Static-asset miss — fall back to the front controller instead of a hard
    /// 404, so the application router sees the request and can render its own
    /// 404. The original URI is read from `REQUEST_URI` (no `PATH_INFO`, since
    /// the rewrite target is not named in the URL). Mirrors the canonical
    /// `try_files $uri /index.php` front-controller config.
    pub(crate) async fn resolve_static_miss(
        &self,
        sanitized: &str,
        ctx: &ResolveCtx<'_>,
    ) -> RouteResult {
        self.rewrite(sanitized, ctx).await
    }
}
