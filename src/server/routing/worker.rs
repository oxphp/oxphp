use super::{ResolveCtx, RouteResult};

/// Worker routing — `WORKER_MODE_ENABLED=true` with a `.php` `ENTRY_FILE`.
///
/// Nginx-style contract: static assets are served from disk, everything else
/// is dispatched to the persistent worker script. The worker is the single
/// front controller — no direct execution of arbitrary `.php` files, no
/// directory-index lookup, no root `index.php` fallback. This mirrors the
/// FrankenPHP / RoadRunner worker model.
///
/// Semantics per URI kind:
/// - `UriKind::Php` — dispatch to the worker (the script on disk, if any, is
///   never executed per-request)
/// - `UriKind::OtherExtension` — serve if the file exists (common layer),
///   else dispatch to the worker
/// - `UriKind::NoExtension` — dispatch to the worker
pub(crate) struct WorkerRouter;

impl WorkerRouter {
    /// Dispatch to the worker route. `worker_route` is set at startup via
    /// `RouteConfig::set_worker_route`; the `None` arm is a defensive
    /// fallback for the window before it is wired up.
    fn dispatch(&self, ctx: &ResolveCtx<'_>) -> RouteResult {
        match ctx.worker_route {
            Some(wr) => wr.clone(),
            None => RouteResult::NotFound,
        }
    }

    pub(crate) async fn resolve_no_extension(&self, ctx: &ResolveCtx<'_>) -> RouteResult {
        self.dispatch(ctx)
    }

    pub(crate) async fn resolve_php(&self, ctx: &ResolveCtx<'_>) -> RouteResult {
        self.dispatch(ctx)
    }

    pub(crate) async fn resolve_static_miss(&self, ctx: &ResolveCtx<'_>) -> RouteResult {
        self.dispatch(ctx)
    }
}
