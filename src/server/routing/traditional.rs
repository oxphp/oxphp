use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{ResolveCtx, RouteResult};

/// Traditional routing — `ENTRY_FILE` unset (direct file mapping).
///
/// Nginx equivalent:
/// ```nginx
/// try_files $uri $uri/ /index.php /index.html =404;
/// location ~ \.php$ { split_path_info; try_files $uri =404; }
/// ```
///
/// Resolution order:
/// 1. `$uri` exact file → Serve (non-php) or Execute (`.php`)
/// 2. `$uri/` directory → `dir/index.php` or `dir/index.html`
/// 3. PATH_INFO split when URI contains `.php/`
/// 4. Root `/index.php` fallback
/// 5. Root `/index.html` fallback
/// 6. Worker route (if configured)
/// 7. NotFound
pub(crate) struct TraditionalRouter {
    root_index_php: PathBuf,
    root_index_html: PathBuf,
    root_index_php_key: String,
    root_index_html_key: String,
}

impl TraditionalRouter {
    pub(crate) fn new(document_root: &Path) -> Self {
        let root_index_php = document_root.join("index.php");
        let root_index_html = document_root.join("index.html");
        let root_index_php_key = root_index_php.to_string_lossy().into_owned();
        let root_index_html_key = root_index_html.to_string_lossy().into_owned();
        Self {
            root_index_php,
            root_index_html,
            root_index_php_key,
            root_index_html_key,
        }
    }

    /// Shared fallback chain: root `/index.php` → `/index.html` → worker → NotFound.
    async fn root_fallback(&self, ctx: &ResolveCtx<'_>) -> RouteResult {
        if ctx.file_cache.is_file(&self.root_index_php_key).await {
            return RouteResult::Execute(self.root_index_php.clone(), None, None);
        }
        if ctx.file_cache.is_file(&self.root_index_html_key).await {
            return RouteResult::Serve(self.root_index_html.clone());
        }
        if let Some(wr) = ctx.worker_route {
            return wr.clone();
        }
        RouteResult::NotFound
    }

    /// Walk `.php` component boundaries looking for a real script file.
    /// Returns `Some(Execute(script, path_info))` on hit, `None` otherwise.
    async fn try_split_path_info(
        &self,
        sanitized: &str,
        ctx: &ResolveCtx<'_>,
    ) -> Option<RouteResult> {
        let bytes = sanitized.as_bytes();
        let mut i = 0;
        while i + 4 <= bytes.len() {
            let is_php_marker = bytes[i] == b'.'
                && (bytes[i + 1] == b'p' || bytes[i + 1] == b'P')
                && (bytes[i + 2] == b'h' || bytes[i + 2] == b'H')
                && (bytes[i + 3] == b'p' || bytes[i + 3] == b'P');
            if !is_php_marker {
                i += 1;
                continue;
            }
            let end = i + 4;
            // Must be end-of-string or followed by '/'
            if end < bytes.len() && bytes[end] != b'/' {
                i = end;
                continue;
            }

            let script_part = &sanitized[..end];
            let candidate = ctx.document_root.join(script_part);
            if ctx.file_cache.is_file(&candidate.to_string_lossy()).await {
                let path_info = if end < bytes.len() {
                    Some(sanitized[end..].to_string())
                } else {
                    None
                };
                return Some(RouteResult::Execute(candidate, path_info, None));
            }
            i = end;
        }
        None
    }
}

impl TraditionalRouter {
    pub(crate) async fn resolve_no_extension(
        &self,
        sanitized: &str,
        ctx: &ResolveCtx<'_>,
    ) -> RouteResult {
        // Root request — skip disk probes of `$uri`, go to fallback chain.
        if sanitized.is_empty() {
            return self.root_fallback(ctx).await;
        }

        let file_path = ctx.document_root.join(sanitized);
        // `to_string_lossy()` returns `Cow::Borrowed` on UTF-8 paths (the
        // overwhelmingly common case on Linux), so this costs nothing on
        // the hot path. Only non-UTF-8 paths pay for an owned allocation.
        let file_key = file_path.to_string_lossy();

        // 1. `$uri` — exact file (no-extension file like `README`)
        if ctx.file_cache.is_file(&file_key).await {
            return RouteResult::Serve(file_path);
        }

        // 2. `$uri/` — directory → look for index.php, then index.html
        if ctx.file_cache.is_dir(&file_key).await {
            let php_idx = file_path.join("index.php");
            if ctx.file_cache.is_file(&php_idx.to_string_lossy()).await {
                return RouteResult::Execute(php_idx, None, None);
            }
            let html_idx = file_path.join("index.html");
            if ctx.file_cache.is_file(&html_idx.to_string_lossy()).await {
                return RouteResult::Serve(html_idx);
            }
            // Directory exists but has no index — fall through to root fallback.
        }

        // 3-6. Root fallback chain.
        self.root_fallback(ctx).await
    }

    pub(crate) async fn resolve_php(&self, sanitized: &str, ctx: &ResolveCtx<'_>) -> RouteResult {
        // PHP_DENY_PATHS check — runs *before* disk I/O so we never leak
        // existence info via timing. Only applied in Traditional mode; the
        // router itself is Traditional-specific.
        if let Some(deny) = ctx.php_deny {
            if let Some(pattern) = deny.matches(sanitized) {
                tracing::info!(
                    path = %sanitized,
                    pattern = %pattern,
                    "PHP execution denied by PHP_DENY_PATHS"
                );
                return match deny.fallback() {
                    crate::config::DenyFallback::Status(code) => RouteResult::Denied(*code),
                    crate::config::DenyFallback::Script { path, uri } => RouteResult::Execute(
                        path.clone(),
                        // path_info=None: SAPI reads the original URI from
                        // `denied_meta.path` instead — avoids a duplicate
                        // String allocation on the fallback path.
                        None,
                        Some(Arc::new(crate::config::DeniedMeta {
                            path: sanitized.to_string(),
                            pattern: pattern.to_string(),
                            fallback_script_uri: uri.clone(),
                        })),
                    ),
                };
            }
        }

        let file_path = ctx.document_root.join(sanitized);
        let file_key = file_path.to_string_lossy();

        // Exact `.php` file on disk
        if ctx.file_cache.is_file(&file_key).await {
            return RouteResult::Execute(file_path, None, None);
        }

        // PATH_INFO split — `$uri` contains `.php/` somewhere
        if let Some(result) = self.try_split_path_info(sanitized, ctx).await {
            return result;
        }

        // Fallback chain
        self.root_fallback(ctx).await
    }

    pub(crate) async fn resolve_static_miss(&self, ctx: &ResolveCtx<'_>) -> RouteResult {
        self.root_fallback(ctx).await
    }
}
