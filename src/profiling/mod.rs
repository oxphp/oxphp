//! Thread-local profiling context — shared infrastructure used by APM and
//! (future) profiler plugins. Tracks open and finished spans for the current
//! request, plus precision timestamps in nanoseconds.

pub mod decorators;
pub mod export;
pub mod filter;
pub mod flush;

pub use flush::{
    get_profiling_mode, is_profiling_paused, profiler_rshutdown_flush, set_profiling_mode,
    set_profiling_paused, snapshot_open_stack, OxSpanEvent,
};

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// Activation mode for a request's profiling context.
///
/// Set once per request via `ProfilingContext::reset` and read back by
/// consumers. Callers (the trigger in `ox_profiler`) decide which
/// mode to pass using widest-wins selection across interested plugins
/// (`ProfileAll` > `ApmOnly` > `Off`); this type does not perform the
/// selection itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProfilingMode {
    /// No tracing is happening on this request. Between requests or when
    /// no plugin asked for any span collection.
    #[default]
    Off,
    /// Only explicit hook sites (e.g. `#[Trace]` decorator, SQL / HTTP /
    /// exception emitters in the APM plugin) push spans. Matches the
    /// current pre-PR behaviour of the `ox_apm` plugin.
    ApmOnly,
    /// Every PHP function call — user and internal — pushes spans via the
    /// Observer API. Used by the `ox_profiler` plugin.
    ProfileAll,
}

/// Local span ID visible to PHP. `0` is reserved as no-op/invalid.
pub type SpanLocalId = u32;

/// Hex lookup table for fast byte-to-hex conversion.
const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

/// Semantic category of an event attached to a span. Exporters use this
/// to filter, group, or highlight events differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpanEventKind {
    /// User-placed mark from `oxphp_profile_mark` or `#[Mark]`.
    Mark,
    /// Database query (SQL) event, typically emitted by the APM hook.
    Sql,
    /// HTTP client call, typically emitted by the APM hook.
    Http,
    /// Exception thrown inside the span.
    Exception,
    /// Slow-threshold event, set when a decorated span exceeded its ms budget.
    Slow,
    /// Memory-spike event, set when a decorated span allocated beyond its KB budget.
    MemorySpike,
    /// Sampled heap allocation. Reserved — unused until the heap hook lands.
    Alloc,
    /// Any event that doesn't fit the above categories.
    #[default]
    Custom,
}

/// A named timestamp with attributes, attached to a span.
#[derive(Debug, Clone)]
pub struct SpanEvent {
    pub name: String,
    pub attributes: Vec<(Arc<str>, Arc<str>)>,
    pub timestamp_ns: u64,
    pub kind: SpanEventKind,
}

/// An open span that has not yet finished.
#[derive(Debug)]
pub struct PendingSpan {
    pub local_id: SpanLocalId,
    pub trace_id: Arc<str>,
    pub span_id: Arc<str>,
    pub parent_span_id: Arc<str>,
    pub name: Arc<str>,
    pub start_ns: u64,
    /// `Arc<str>` key/value pairs so that observer-attached static
    /// tags (shared via `FilterSpec.tags: Arc<[...]>`) can be
    /// appended with only refcount bumps, no per-pair String
    /// allocation on the BEGIN hot path.
    pub attributes: Vec<(Arc<str>, Arc<str>)>,
    pub events: Vec<SpanEvent>,
    /// 0 = Unset, 1 = Ok, 2 = Error.
    pub status_code: u8,
    pub status_message: Option<String>,
    /// CLOCK_THREAD_CPUTIME_ID at begin. Captured from the
    /// observer event's `cpu_ns`. 0 when the span came from a
    /// non-observer path (APM `TraceDecorator`, manual
    /// `oxphp_apm_start`).
    pub cpu_start_ns: u64,
    /// `zend_memory_usage(0)` at begin. 0 when not captured.
    pub mem_enter: i64,
    /// `zend_memory_peak_usage(0)` at begin. 0 when not captured.
    pub mem_peak_enter: i64,
}

/// A completed span ready for export.
#[derive(Debug, Clone)]
pub struct FinishedSpan {
    pub local_id: SpanLocalId,
    pub trace_id: Arc<str>,
    pub span_id: Arc<str>,
    pub parent_span_id: Arc<str>,
    pub name: Arc<str>,
    pub start_ns: u64,
    pub end_ns: u64,
    /// See [`PendingSpan::attributes`] — `Arc<str>` key/value pairs.
    pub attributes: Vec<(Arc<str>, Arc<str>)>,
    pub events: Vec<SpanEvent>,
    /// 0 = Unset, 1 = Ok, 2 = Error.
    pub status_code: u8,
    pub status_message: Option<String>,
    /// `true` if the span was force-closed (not explicitly popped).
    pub leaked: bool,
    /// Delta `CLOCK_THREAD_CPUTIME_ID` = `cpu_end - cpu_start`.
    /// 0 when no CPU measurement was taken (APM-pushed spans).
    /// Computed via `saturating_sub` so clock regressions never
    /// underflow.
    pub cpu_ns: u64,
    /// `zend_memory_usage(0)` at begin / end. Both 0 when
    /// not captured.
    pub mem_enter: i64,
    pub mem_exit: i64,
    /// Max peak observed across begin and end.
    /// `zend_memory_peak_usage` is monotonically non-decreasing
    /// within a request, so the `.max()` is defensive.
    pub mem_peak: i64,
}

/// Owned snapshot of a request's finished spans.
///
/// Produced by `ProfilingContext::finalize`; sent from the PHP worker
/// thread to the Tokio thread through `Arc<SpanTree>` on the oneshot
/// response channel.
#[derive(Debug, Clone)]
pub struct SpanTree {
    /// Finished spans in completion order (leaf spans first, enclosing
    /// spans after). `leaked` flag on an individual span means it was
    /// force-closed during `finalize` rather than naturally popped.
    pub finished: Vec<FinishedSpan>,

    /// Trace ID for this request, copied from the finalized context.
    pub trace_id: Arc<str>,

    /// Root span ID, copied from the finalized context.
    pub root_span_id: Arc<str>,

    /// Mode that was active during collection.
    pub mode: ProfilingMode,
}

impl SpanTree {
    /// True if this tree has no spans.
    pub fn is_empty(&self) -> bool {
        self.finished.is_empty()
    }

    /// Number of finished spans.
    pub fn len(&self) -> usize {
        self.finished.len()
    }

    /// Iterate finished spans in completion order.
    pub fn finished_spans(&self) -> &[FinishedSpan] {
        &self.finished
    }
}

/// Thread-local stack of open spans plus a finished-spans buffer.
///
/// The `spans` Vec acts as a stack: the last element is the "current" span
/// for implicit operations like `oxphp_trace_attribute('key', 'value')`.
pub struct ProfilingContext {
    pub mode: ProfilingMode,
    spans: Vec<PendingSpan>,
    finished: Vec<FinishedSpan>,
    next_id: SpanLocalId,
    trace_id: Arc<str>,
    root_span_id: Arc<str>,
    /// Map from C-side BEGIN seq to the Rust SpanLocalId returned
    /// by `push()`. Drained by matching END events arriving via
    /// `apply_events`. Inline-allocated for the common 0–32 open
    /// spans case (deeper recursion just heap-allocates and keeps
    /// working).
    seq_to_local: smallvec::SmallVec<[(u64, SpanLocalId); 32]>,
}

impl Default for ProfilingContext {
    fn default() -> Self {
        Self::new()
    }
}

impl ProfilingContext {
    /// Create an empty span stack with pre-allocated capacity.
    ///
    /// `finished` starts at 256 so a ProfileAll trace with a few thousand
    /// spans doesn't reallocate the backing storage a dozen times.
    pub fn new() -> Self {
        Self {
            mode: ProfilingMode::Off,
            spans: Vec::with_capacity(8),
            finished: Vec::with_capacity(256),
            next_id: 1,
            trace_id: Arc::from(""),
            root_span_id: Arc::from(""),
            seq_to_local: smallvec::SmallVec::new(),
        }
    }

    /// Reset the stack for a new request, clearing all spans and setting
    /// the profiling mode + trace context.
    pub fn reset(&mut self, mode: ProfilingMode, trace_id: String, root_span_id: String) {
        self.mode = mode;
        self.spans.clear();
        self.finished.clear();
        self.seq_to_local.clear();
        self.next_id = 1;
        self.trace_id = Arc::from(trace_id);
        self.root_span_id = Arc::from(root_span_id);
    }

    /// Push a new child span onto the stack with full metric capture.
    /// The observer-driven path (`apply_events`) calls this
    /// with the BEGIN event's `ts_ns` / `cpu_ns` / `mem` / `mem_peak`.
    /// APM and other decorator-driven paths use the thin wrapper
    /// `push`, which resolves `start_ns` from `now_ns()`.
    ///
    /// `name` is an already-interned `Arc<str>` — the observer hot
    /// path reuses names from the thread-local `read_name` cache, so
    /// a BEGIN on a repeat function name is a relaxed refcount bump
    /// with no allocation. Decorator callers wrap a freshly-built
    /// `String` via `Arc::from(s)`.
    pub fn push_with_metrics(
        &mut self,
        name: Arc<str>,
        attributes: Vec<(Arc<str>, Arc<str>)>,
        start_ns: u64,
        cpu_start_ns: u64,
        mem_enter: i64,
        mem_peak_enter: i64,
    ) -> SpanLocalId {
        let local_id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if self.next_id == 0 {
            self.next_id = 1;
        }

        let parent_span_id = self
            .spans
            .last()
            .map(|s| Arc::clone(&s.span_id))
            .unwrap_or_else(|| Arc::clone(&self.root_span_id));

        let span_id = generate_span_id();

        self.spans.push(PendingSpan {
            local_id,
            trace_id: Arc::clone(&self.trace_id),
            span_id,
            parent_span_id,
            name,
            start_ns,
            attributes,
            events: Vec::new(),
            status_code: 0,
            status_message: None,
            cpu_start_ns,
            mem_enter,
            mem_peak_enter,
        });

        local_id
    }

    /// Push a new child span onto the stack.
    ///
    /// The parent is the topmost open span, or `root_span_id` if the stack
    /// is empty. Returns the local ID assigned to the new span.
    ///
    /// Thin wrapper around `push_with_metrics` that samples `now_ns()`
    /// for the start timestamp and zero-inits the CPU / memory fields.
    /// Callers (APM, decorators, manual SDK) that don't have observer
    /// metric values use this; the observer-driven `apply_events` path
    /// uses `push_with_metrics` directly so the C-side monotonic
    /// timestamp flows through without a second syscall.
    #[inline]
    pub fn push(&mut self, name: Arc<str>, attributes: Vec<(Arc<str>, Arc<str>)>) -> SpanLocalId {
        self.push_with_metrics(name, attributes, now_ns(), 0, 0, 0)
    }

    /// Pop a span by local ID with full metric capture.
    /// Computes `cpu_ns` as `cpu_end_ns - cpu_start_ns` (saturating
    /// to handle clock regressions) and stores `mem_exit` plus
    /// `mem_peak = max(peak_enter, peak_exit)` on the resulting
    /// FinishedSpan.
    ///
    /// `end_ns` comes from the observer event's `ts_ns` (monotonic)
    /// in the C-driven path; the thin wrapper `pop` resolves it
    /// from `now_ns()` for decorator / APM paths.
    ///
    /// Fast-path: when the topmost open span is the one being popped
    /// (the LIFO common case) this is O(1). Out-of-order closes fall
    /// back to a linear scan + shift which is O(depth) — capped by the
    /// observer's 32-deep open stack.
    ///
    /// Returns `None` if no span with the given `local_id` is found.
    pub fn pop_with_metrics(
        &mut self,
        local_id: SpanLocalId,
        end_ns: u64,
        cpu_end_ns: u64,
        mem_exit: i64,
        mem_peak_exit: i64,
    ) -> Option<()> {
        let pending = if self.spans.last().is_some_and(|s| s.local_id == local_id) {
            self.spans.pop().expect("last() was Some")
        } else {
            let idx = self.spans.iter().position(|s| s.local_id == local_id)?;
            self.spans.remove(idx)
        };
        let cpu_ns = cpu_end_ns.saturating_sub(pending.cpu_start_ns);
        let mem_peak = pending.mem_peak_enter.max(mem_peak_exit);
        self.finished.push(FinishedSpan {
            local_id: pending.local_id,
            trace_id: pending.trace_id,
            span_id: pending.span_id,
            parent_span_id: pending.parent_span_id,
            name: pending.name,
            start_ns: pending.start_ns,
            end_ns,
            attributes: pending.attributes,
            events: pending.events,
            status_code: pending.status_code,
            status_message: pending.status_message,
            leaked: false,
            cpu_ns,
            mem_enter: pending.mem_enter,
            mem_exit,
            mem_peak,
        });
        Some(())
    }

    /// Pop a span by local ID, moving it to the finished list.
    ///
    /// Returns `None` if no span with the given `local_id` is found.
    ///
    /// Thin wrapper around `pop_with_metrics` that samples `now_ns()`
    /// for the end timestamp and zero-inits CPU / memory. Use from APM
    /// / decorator paths that don't have observer-captured end values.
    #[inline]
    pub fn pop(&mut self, local_id: SpanLocalId) -> Option<()> {
        self.pop_with_metrics(local_id, now_ns(), 0, 0, 0)
    }

    /// Returns a mutable reference to the topmost open span.
    pub fn current_mut(&mut self) -> Option<&mut PendingSpan> {
        self.spans.last_mut()
    }

    /// Returns a read-only reference to the topmost open span.
    pub fn current(&self) -> Option<&PendingSpan> {
        self.spans.last()
    }

    /// Find a specific open span by local ID.
    pub fn get_mut(&mut self, local_id: SpanLocalId) -> Option<&mut PendingSpan> {
        self.spans.iter_mut().find(|s| s.local_id == local_id)
    }

    /// Force-close all open spans, marking them as leaked.
    ///
    /// Returns the number of spans that were force-closed.
    pub fn force_close_all(&mut self) -> usize {
        let count = self.spans.len();
        let now = now_ns();
        for pending in self.spans.drain(..) {
            self.finished.push(FinishedSpan {
                local_id: pending.local_id,
                trace_id: pending.trace_id,
                span_id: pending.span_id,
                parent_span_id: pending.parent_span_id,
                name: pending.name,
                start_ns: pending.start_ns,
                end_ns: now,
                attributes: pending.attributes,
                events: pending.events,
                status_code: pending.status_code,
                status_message: pending.status_message,
                leaked: true,
                // Leaked spans never had a clean END; emit
                // zero metrics rather than fabricating delta values.
                cpu_ns: 0,
                mem_enter: pending.mem_enter,
                mem_exit: 0,
                mem_peak: pending.mem_peak_enter,
            });
        }
        count
    }

    /// Attach a `Mark`-kind event to the topmost open span. No-op
    /// when no span is open. Used by the `OxPHP\Profile\mark()` PHP
    /// SDK and the `#[Mark]` attribute.
    pub fn attach_mark_on_current(&mut self, name: String, attributes: Vec<(Arc<str>, Arc<str>)>) {
        if let Some(span) = self.current_mut() {
            span.events.push(SpanEvent {
                name,
                attributes,
                timestamp_ns: now_ns(),
                kind: SpanEventKind::Mark,
            });
        }
    }

    /// Attach a metric attribute (`metric.<name>` = `<value>`) to
    /// the topmost open span. No-op when no span is open. Value is
    /// rendered with the default `{value}` formatter so integers
    /// print as `1234` and floats as `9.99`.
    pub fn attach_metric_on_current(&mut self, name: &str, value: f64) {
        if let Some(span) = self.current_mut() {
            let key: Arc<str> = Arc::from(format!("metric.{name}"));
            let val: Arc<str> = Arc::from(format!("{value}"));
            span.attributes.push((key, val));
        }
    }

    /// Apply a batch of `OxSpanEvent`s drained from the C-side observer's
    /// ring buffer. BEGIN events `push` a new span and remember the
    /// `seq → local_id` mapping; END events look up the matching span
    /// by `seq` and `pop` it.
    ///
    /// END events without a matching BEGIN are silently dropped (a
    /// metric counts these). BEGIN events without a matching
    /// END remain open and are force-closed by `finalize` with
    /// `leaked = true`.
    ///
    /// Skipped wholesale when `mode != ProfileAll` — defensive against
    /// a stale flush that arrives after a mode change. Should not
    /// happen because `set_profiling_mode(OFF)` resets `buf_len` on
    /// the C side, but the check is cheap.
    pub fn apply_events(&mut self, events: &[OxSpanEvent]) {
        if self.mode != ProfilingMode::ProfileAll {
            return;
        }
        for ev in events {
            match ev.kind {
                flush::SPAN_EVENT_KIND_BEGIN => {
                    let name = flush::read_name(ev);
                    // Propagate ts_ns / cpu_ns / mem / mem_peak from the
                    // C-side BEGIN event so exporters (collapsed,
                    // xhprof, …) have real wall, CPU and memory figures.
                    // Using `ev.ts_ns` here (monotonic, already sampled
                    // by the observer) avoids a second clock read.
                    let local_id = self.push_with_metrics(
                        name,
                        Vec::new(),
                        ev.ts_ns,
                        ev.cpu_ns,
                        ev.mem,
                        ev.mem_peak,
                    );
                    self.seq_to_local.push((ev.seq, local_id));
                    // Bounded safety net — should never fire under
                    // normal use (32-deep open-span recursion).
                    if self.seq_to_local.len() > 4096 {
                        self.seq_to_local.drain(..2048);
                    }

                    // spec_id arrives in reserved2. Non-zero
                    // means the function carries one of #[Profile] /
                    // #[Exclude] / #[Sample] / #[Tag] (Excluded
                    // variants short-circuit in C, so any spec_id
                    // reaching here implies a non-excluded match).
                    // Look up tags and append them to the freshly
                    // pushed span.
                    let spec_id = ev.reserved2 as u32;
                    if spec_id != 0 {
                        if let Some(spec) = filter::get_spec(spec_id) {
                            if !spec.tags.is_empty() {
                                if let Some(span) = self.get_mut(local_id) {
                                    // `spec.tags` is an `Arc<[...]>` of
                                    // `(Arc<str>, Arc<str>)` pairs. Each
                                    // clone is two atomic refcount bumps
                                    // — no String allocation on the hot
                                    // path.
                                    span.attributes.extend(spec.tags.iter().cloned());
                                }
                            }
                        }
                    }
                }
                flush::SPAN_EVENT_KIND_END => {
                    if ev.seq == 0 {
                        // C-side emitted an unmatched END (open_stack
                        // was empty). Drop silently.
                        continue;
                    }
                    if let Some(pos) = self.seq_to_local.iter().rposition(|&(s, _)| s == ev.seq) {
                        let (_, local_id) = self.seq_to_local.swap_remove(pos);
                        // Propagate ts_ns / cpu_end / mem / mem_peak
                        // from END event; pop_with_metrics computes
                        // the cpu delta and stores everything. Using
                        // `ev.ts_ns` skips a second clock read and
                        // keeps start/end on the same clock source
                        // as BEGIN.
                        let _ = self.pop_with_metrics(
                            local_id,
                            ev.ts_ns,
                            ev.cpu_ns,
                            ev.mem,
                            ev.mem_peak,
                        );
                    }
                    // Otherwise: unmatched END → drop silently.
                }
                _ => {
                    // Unknown kind tag — ignore for forward compat.
                }
            }
        }
    }

    /// Close all currently-open spans (marking them `leaked = true`),
    /// drain the finished list, and return an owned `Arc<SpanTree>` ready
    /// to be sent across the Tokio channel.
    ///
    /// After this call the context is left in a "reset-needed" state: its
    /// `spans` and `finished` lists are empty, but `trace_id`, `root_span_id`,
    /// and `mode` are preserved so callers can inspect them.
    pub fn finalize(&mut self) -> Arc<SpanTree> {
        let leaked_count = self.force_close_all();
        let _ = leaked_count; // retained for future metrics

        let finished = std::mem::take(&mut self.finished);
        Arc::new(SpanTree {
            finished,
            trace_id: Arc::clone(&self.trace_id),
            root_span_id: Arc::clone(&self.root_span_id),
            mode: self.mode,
        })
    }

    /// Drain all finished spans for export.
    pub fn take_finished(&mut self) -> Vec<FinishedSpan> {
        std::mem::take(&mut self.finished)
    }

    /// Number of currently open spans.
    pub fn open_count(&self) -> usize {
        self.spans.len()
    }

    /// Number of finished spans waiting to be drained.
    pub fn finished_count(&self) -> usize {
        self.finished.len()
    }

    /// The trace ID for the current request.
    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }

    /// The root span ID for the current request.
    pub fn root_span_id(&self) -> &str {
        &self.root_span_id
    }
}

/// Return the current time as Unix epoch nanoseconds.
pub fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

/// Process-wide monotonic counter for span-ID generation. Each
/// `fetch_add` is globally unique for the process lifetime, which
/// is all we need — exporters hash these IDs before using them as
/// keys, so sequential values are fine.
static SPAN_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a 16-char lowercase hex span ID.
///
/// Big-endian encoding of the atomic counter. No TLS access, no
/// thread hash, deterministic across runs — same counter value
/// always produces the same ID, which keeps pprof/collapsed
/// snapshots diff-friendly.
///
/// One heap allocation — directly from the stack-buffer into
/// `Arc<str>`. Avoids the former `String::with_capacity(16)` +
/// `Arc::from(String)` two-alloc path.
fn generate_span_id() -> Arc<str> {
    let counter = SPAN_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    let raw = counter.to_be_bytes();

    let mut buf = [0u8; 16];
    for (i, byte) in raw.iter().enumerate() {
        buf[i * 2] = HEX_CHARS[(byte >> 4) as usize];
        buf[i * 2 + 1] = HEX_CHARS[(byte & 0x0f) as usize];
    }
    // SAFETY: HEX_CHARS is ASCII-only, so `buf` is guaranteed valid
    // UTF-8. Going through `from_utf8_unchecked` avoids a redundant
    // validation pass in `Arc::<str>::from`.
    Arc::from(unsafe { std::str::from_utf8_unchecked(&buf) })
}

thread_local! {
    /// Per-worker-thread profiling context.
    pub static PROFILING_CONTEXT: RefCell<ProfilingContext> = RefCell::new(ProfilingContext::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_pop_basic() {
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::ApmOnly, "trace123".into(), "root456".into());

        let id = stack.push("my-span".into(), vec![]);
        assert_eq!(id, 1);
        assert_eq!(stack.open_count(), 1);

        stack.pop(id).expect("pop should succeed");
        assert_eq!(stack.open_count(), 0);
        assert_eq!(stack.finished_count(), 1);

        let finished = stack.take_finished();
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].name.as_ref(), "my-span");
        assert_eq!(finished[0].trace_id.as_ref(), "trace123");
        assert_eq!(finished[0].parent_span_id.as_ref(), "root456");
        assert!(!finished[0].leaked);
    }

    #[test]
    fn test_nested_parent_tracking() {
        let mut stack = ProfilingContext::new();
        stack.reset(
            ProfilingMode::ApmOnly,
            "trace-abc".into(),
            "root-def".into(),
        );

        let outer = stack.push("outer".into(), vec![]);
        let outer_span_id = Arc::clone(&stack.current().unwrap().span_id);

        let inner = stack.push("inner".into(), vec![]);
        let inner_parent = Arc::clone(&stack.current().unwrap().parent_span_id);

        // Inner's parent should be outer's span_id.
        assert_eq!(inner_parent, outer_span_id);

        stack.pop(inner).unwrap();
        stack.pop(outer).unwrap();

        let finished = stack.take_finished();
        assert_eq!(finished.len(), 2);
        // Inner was popped first.
        assert_eq!(finished[0].name.as_ref(), "inner");
        assert_eq!(finished[0].parent_span_id, outer_span_id);
        assert_eq!(finished[1].name.as_ref(), "outer");
        assert_eq!(finished[1].parent_span_id.as_ref(), "root-def");
    }

    #[test]
    fn test_force_close_marks_leaked() {
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::ApmOnly, "t1".into(), "r1".into());

        stack.push("span-a".into(), vec![]);
        stack.push("span-b".into(), vec![]);
        assert_eq!(stack.open_count(), 2);

        let closed = stack.force_close_all();
        assert_eq!(closed, 2);
        assert_eq!(stack.open_count(), 0);
        assert_eq!(stack.finished_count(), 2);

        let finished = stack.take_finished();
        assert!(finished[0].leaked);
        assert!(finished[1].leaked);
    }

    #[test]
    fn test_pop_nonexistent_returns_none() {
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::ApmOnly, "t".into(), "r".into());

        assert!(stack.pop(999).is_none());

        let id = stack.push("s".into(), vec![]);
        assert!(stack.pop(id + 1).is_none());
    }

    #[test]
    fn test_reset_clears_all() {
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::ApmOnly, "t1".into(), "r1".into());

        let id1 = stack.push("a".into(), vec![]);
        stack.pop(id1).unwrap();
        stack.push("b".into(), vec![]);

        assert_eq!(stack.open_count(), 1);
        assert_eq!(stack.finished_count(), 1);

        stack.reset(ProfilingMode::ApmOnly, "t2".into(), "r2".into());
        assert_eq!(stack.open_count(), 0);
        assert_eq!(stack.finished_count(), 0);
        assert_eq!(stack.trace_id(), "t2");
        assert_eq!(stack.root_span_id(), "r2");
        assert_eq!(stack.mode, ProfilingMode::ApmOnly);
    }

    #[test]
    fn test_default_mode_is_off() {
        let stack = ProfilingContext::new();
        assert_eq!(stack.mode, ProfilingMode::Off);
    }

    #[test]
    fn test_reset_preserves_mode_argument() {
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        assert_eq!(stack.mode, ProfilingMode::ProfileAll);
        stack.reset(ProfilingMode::Off, "t2".into(), "r2".into());
        assert_eq!(stack.mode, ProfilingMode::Off);
    }

    #[test]
    fn test_current_mut() {
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::ApmOnly, "t".into(), "r".into());

        let id = stack.push("span".into(), vec![]);
        stack
            .current_mut()
            .unwrap()
            .attributes
            .push(("key".into(), "value".into()));

        stack.pop(id).unwrap();
        let finished = stack.take_finished();
        assert_eq!(finished[0].attributes.len(), 1);
        assert_eq!(finished[0].attributes[0].0.as_ref(), "key");
        assert_eq!(finished[0].attributes[0].1.as_ref(), "value");
    }

    #[test]
    fn test_get_mut_specific_span() {
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::ApmOnly, "t".into(), "r".into());

        let outer_id = stack.push("outer".into(), vec![]);
        let _inner_id = stack.push("inner".into(), vec![]);

        // Modify the outer span by ID while inner is on top.
        stack
            .get_mut(outer_id)
            .unwrap()
            .attributes
            .push(("modified".into(), "yes".into()));

        // Verify it was modified.
        let outer = stack.get_mut(outer_id).unwrap();
        assert_eq!(outer.attributes.len(), 1);
        assert_eq!(outer.attributes[0].0.as_ref(), "modified");
    }

    #[test]
    fn test_span_id_zero_skipped() {
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::ApmOnly, "t".into(), "r".into());

        // First push should return 1, not 0.
        let id = stack.push("first".into(), vec![]);
        assert_eq!(id, 1);
    }

    #[test]
    fn test_attributes_in_push() {
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::ApmOnly, "t".into(), "r".into());

        let attrs = vec![
            ("db.system".into(), "mysql".into()),
            ("db.name".into(), "users".into()),
        ];
        let id = stack.push("db-query".into(), attrs);
        stack.pop(id).unwrap();

        let finished = stack.take_finished();
        assert_eq!(finished[0].attributes.len(), 2);
        assert_eq!(finished[0].attributes[0].0.as_ref(), "db.system");
        assert_eq!(finished[0].attributes[0].1.as_ref(), "mysql");
        assert_eq!(finished[0].attributes[1].0.as_ref(), "db.name");
        assert_eq!(finished[0].attributes[1].1.as_ref(), "users");
    }

    #[test]
    fn test_finalize_returns_tree_with_finished_spans() {
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::ApmOnly, "trace-x".into(), "root-y".into());
        let a = stack.push("a".into(), vec![]);
        stack.pop(a).unwrap();
        let b = stack.push("b".into(), vec![]);
        stack.pop(b).unwrap();

        let tree = stack.finalize();
        assert_eq!(tree.len(), 2);
        assert_eq!(tree.finished_spans()[0].name.as_ref(), "a");
        assert_eq!(tree.finished_spans()[1].name.as_ref(), "b");
        assert_eq!(tree.trace_id.as_ref(), "trace-x");
        assert_eq!(tree.root_span_id.as_ref(), "root-y");
        assert_eq!(tree.mode, ProfilingMode::ApmOnly);
        assert!(!tree.is_empty());
    }

    #[test]
    fn test_finalize_force_closes_open_spans_as_leaked() {
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::ApmOnly, "t".into(), "r".into());
        stack.push("still-open".into(), vec![]);

        let tree = stack.finalize();
        assert_eq!(tree.len(), 1);
        assert!(tree.finished_spans()[0].leaked);
    }

    #[test]
    fn test_finalize_empty_tree() {
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::Off, "t".into(), "r".into());
        let tree = stack.finalize();
        assert!(tree.is_empty());
        assert_eq!(tree.len(), 0);
    }

    #[test]
    fn test_finalize_leaves_context_empty_but_preserves_metadata() {
        let mut stack = ProfilingContext::new();
        stack.reset(
            ProfilingMode::ProfileAll,
            "keep-me".into(),
            "preserved".into(),
        );
        let id = stack.push("f".into(), vec![]);
        stack.pop(id).unwrap();

        let _tree = stack.finalize();
        assert_eq!(stack.finished_count(), 0);
        assert_eq!(stack.open_count(), 0);
        // Metadata preserved:
        assert_eq!(stack.trace_id(), "keep-me");
        assert_eq!(stack.root_span_id(), "preserved");
        assert_eq!(stack.mode, ProfilingMode::ProfileAll);
    }

    #[test]
    fn test_events_on_span() {
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::ApmOnly, "t".into(), "r".into());

        let id = stack.push("work".into(), vec![]);
        stack.current_mut().unwrap().events.push(SpanEvent {
            name: "checkpoint".into(),
            attributes: vec![(Arc::from("msg"), Arc::from("halfway"))],
            timestamp_ns: now_ns(),
            kind: SpanEventKind::Mark,
        });

        stack.pop(id).unwrap();
        let finished = stack.take_finished();
        assert_eq!(finished[0].events.len(), 1);
        assert_eq!(finished[0].events[0].name, "checkpoint");
        assert_eq!(finished[0].events[0].attributes[0].0.as_ref(), "msg");
        assert_eq!(finished[0].events[0].attributes[0].1.as_ref(), "halfway");
    }

    #[test]
    fn test_status_on_span() {
        let mut stack = ProfilingContext::new();
        stack.reset(ProfilingMode::ApmOnly, "t".into(), "r".into());

        let id = stack.push("failing".into(), vec![]);
        {
            let span = stack.current_mut().unwrap();
            span.status_code = 2; // Error
            span.status_message = Some("something went wrong".into());
        }

        stack.pop(id).unwrap();
        let finished = stack.take_finished();
        assert_eq!(finished[0].status_code, 2);
        assert_eq!(
            finished[0].status_message.as_deref(),
            Some("something went wrong")
        );
    }

    #[test]
    fn test_span_event_kind_default_is_custom() {
        assert_eq!(SpanEventKind::default(), SpanEventKind::Custom);
    }

    // ── apply_events ───────────────────────────

    fn ev(kind: u8, seq: u64, name: &'static [u8]) -> OxSpanEvent {
        OxSpanEvent {
            kind,
            reserved0: 0,
            name_len: name.len() as u16,
            reserved1: 0,
            seq,
            ts_ns: seq * 100,
            cpu_ns: seq * 10,
            mem: 0,
            mem_peak: 0,
            name_ptr: name.as_ptr() as *const std::os::raw::c_char,
            reserved2: 0,
        }
    }

    #[test]
    fn apply_events_well_formed_lifo() {
        let mut ctx = ProfilingContext::new();
        ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());

        let events = [
            ev(flush::SPAN_EVENT_KIND_BEGIN, 1, b"outer"),
            ev(flush::SPAN_EVENT_KIND_BEGIN, 2, b"inner"),
            ev(flush::SPAN_EVENT_KIND_END, 2, b""),
            ev(flush::SPAN_EVENT_KIND_END, 1, b""),
        ];
        ctx.apply_events(&events);

        assert_eq!(ctx.open_count(), 0);
        let tree = ctx.finalize();
        assert_eq!(tree.finished.len(), 2);
        // pop order: inner finishes before outer (LIFO).
        assert_eq!(tree.finished[0].name.as_ref(), "inner");
        assert_eq!(tree.finished[1].name.as_ref(), "outer");
        assert!(!tree.finished[0].leaked);
        assert!(!tree.finished[1].leaked);
        // Parent linkage: inner.parent_span_id == outer.span_id.
        assert_eq!(tree.finished[0].parent_span_id, tree.finished[1].span_id);
    }

    #[test]
    fn apply_events_skips_when_mode_off() {
        let mut ctx = ProfilingContext::new();
        ctx.reset(ProfilingMode::ApmOnly, "trace".into(), "root".into());
        let events = [ev(flush::SPAN_EVENT_KIND_BEGIN, 1, b"a")];
        ctx.apply_events(&events);
        assert_eq!(ctx.open_count(), 0);
        assert_eq!(ctx.finished_count(), 0);
    }

    #[test]
    fn apply_events_unmatched_end_dropped() {
        let mut ctx = ProfilingContext::new();
        ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
        let events = [
            ev(flush::SPAN_EVENT_KIND_END, 99, b""),
            ev(flush::SPAN_EVENT_KIND_BEGIN, 1, b"a"),
            ev(flush::SPAN_EVENT_KIND_END, 1, b""),
        ];
        ctx.apply_events(&events);
        let tree = ctx.finalize();
        assert_eq!(tree.finished.len(), 1);
        assert_eq!(tree.finished[0].name.as_ref(), "a");
        assert!(!tree.finished[0].leaked);
    }

    #[test]
    fn apply_events_unmatched_end_zero_seq_dropped() {
        // C side emits seq=0 when its open_stack is empty.
        let mut ctx = ProfilingContext::new();
        ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
        let events = [
            ev(flush::SPAN_EVENT_KIND_BEGIN, 1, b"a"),
            ev(flush::SPAN_EVENT_KIND_END, 0, b""),
            ev(flush::SPAN_EVENT_KIND_END, 1, b""),
        ];
        ctx.apply_events(&events);
        let tree = ctx.finalize();
        assert_eq!(tree.finished.len(), 1);
        assert_eq!(tree.finished[0].name.as_ref(), "a");
        assert!(!tree.finished[0].leaked);
    }

    #[test]
    fn apply_events_unmatched_begin_leaked() {
        let mut ctx = ProfilingContext::new();
        ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
        let events = [ev(flush::SPAN_EVENT_KIND_BEGIN, 1, b"leaks")];
        ctx.apply_events(&events);
        let tree = ctx.finalize();
        assert_eq!(tree.finished.len(), 1);
        assert!(tree.finished[0].leaked);
    }

    #[test]
    fn apply_events_unknown_kind_ignored() {
        let mut ctx = ProfilingContext::new();
        ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
        let events = [ev(255, 1, b"junk")];
        ctx.apply_events(&events);
        assert_eq!(ctx.open_count(), 0);
        assert_eq!(ctx.finished_count(), 0);
    }

    #[test]
    fn set_profiling_mode_no_op_in_test_build() {
        // Without `feature = "php"` the wrapper is a no-op stub. We
        // call it to make sure the symbol resolves and types match.
        set_profiling_mode(ProfilingMode::ProfileAll);
        set_profiling_mode(ProfilingMode::ApmOnly);
        set_profiling_mode(ProfilingMode::Off);
    }

    // ── attach_mark_on_current / attach_metric_on_current

    #[test]
    fn attach_mark_appends_event_to_current_span() {
        let mut ctx = ProfilingContext::new();
        ctx.reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        let id = ctx.push("outer".into(), vec![]);
        ctx.attach_mark_on_current(
            "checkpoint".into(),
            vec![(Arc::from("user"), Arc::from("42"))],
        );
        let span = ctx.get_mut(id).expect("span open");
        assert_eq!(span.events.len(), 1);
        assert_eq!(span.events[0].name, "checkpoint");
        assert_eq!(span.events[0].kind, SpanEventKind::Mark);
        assert_eq!(
            span.events[0].attributes[0],
            (Arc::from("user"), Arc::from("42"))
        );
    }

    #[test]
    fn attach_mark_with_no_open_span_is_noop() {
        let mut ctx = ProfilingContext::new();
        ctx.reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        ctx.attach_mark_on_current("orphan".into(), vec![]);
        assert_eq!(ctx.open_count(), 0);
        assert_eq!(ctx.finished_count(), 0);
    }

    #[test]
    fn attach_metric_appends_metric_dotted_attribute() {
        let mut ctx = ProfilingContext::new();
        ctx.reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        let id = ctx.push("op".into(), vec![]);
        ctx.attach_metric_on_current("rows", 1234.0);
        ctx.attach_metric_on_current("price", 9.99);
        let span = ctx.get_mut(id).expect("span open");
        assert!(span
            .attributes
            .iter()
            .any(|(k, v)| k.as_ref() == "metric.rows" && v.as_ref() == "1234"));
        assert!(span
            .attributes
            .iter()
            .any(|(k, v)| k.as_ref() == "metric.price" && v.as_ref() == "9.99"));
    }

    #[test]
    fn attach_metric_with_no_open_span_is_noop() {
        let mut ctx = ProfilingContext::new();
        ctx.reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        ctx.attach_metric_on_current("orphan", 1.0);
        assert_eq!(ctx.open_count(), 0);
        assert_eq!(ctx.finished_count(), 0);
    }

    // ── data model extension (cpu_ns + mem fields) ──

    #[test]
    fn push_with_metrics_carries_cpu_and_mem_through_pop() {
        let mut ctx = ProfilingContext::new();
        ctx.reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        let id = ctx.push_with_metrics("op".into(), vec![], 100, 1000, 50_000, 75_000);
        ctx.pop_with_metrics(id, 300, 1500, 60_000, 80_000);
        let tree = ctx.finalize();
        assert_eq!(tree.finished.len(), 1);
        let s = &tree.finished[0];
        assert_eq!(s.start_ns, 100);
        assert_eq!(s.end_ns, 300);
        assert_eq!(s.cpu_ns, 500, "delta CPU = end - start");
        assert_eq!(s.mem_enter, 50_000);
        assert_eq!(s.mem_exit, 60_000);
        assert_eq!(s.mem_peak, 80_000, "max(75_000, 80_000)");
    }

    #[test]
    fn legacy_push_pop_zero_init_metrics() {
        // APM / decorator path: push(name, attrs) without metrics.
        // Result span reports zeros — the honest "no measurement"
        // signal that exporters can detect and skip.
        let mut ctx = ProfilingContext::new();
        ctx.reset(ProfilingMode::ApmOnly, "t".into(), "r".into());
        let id = ctx.push("trace".into(), vec![]);
        ctx.pop(id);
        let tree = ctx.finalize();
        assert_eq!(tree.finished[0].cpu_ns, 0);
        assert_eq!(tree.finished[0].mem_enter, 0);
        assert_eq!(tree.finished[0].mem_exit, 0);
        assert_eq!(tree.finished[0].mem_peak, 0);
    }

    #[test]
    fn pop_with_metrics_handles_cpu_clock_regression_safely() {
        // CPU time CAN decrease across the pop (clock_gettime returned
        // an earlier sample for end than for begin — rare but possible
        // on virtualised platforms). saturating_sub keeps cpu_ns at 0
        // instead of underflowing to ~u64::MAX.
        let mut ctx = ProfilingContext::new();
        ctx.reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        let id = ctx.push_with_metrics("regress".into(), vec![], 100, 5000, 0, 0);
        ctx.pop_with_metrics(id, 200, 1000, 0, 0);
        let tree = ctx.finalize();
        assert_eq!(tree.finished[0].cpu_ns, 0);
    }

    #[test]
    fn apply_events_carries_metrics_from_observer_events() {
        let mut ctx = ProfilingContext::new();
        ctx.reset(ProfilingMode::ProfileAll, "t".into(), "r".into());

        let begin = OxSpanEvent {
            kind: flush::SPAN_EVENT_KIND_BEGIN,
            reserved0: 0,
            name_len: 4,
            reserved1: 0,
            seq: 1,
            ts_ns: 100,
            cpu_ns: 200,
            mem: 1000,
            mem_peak: 2000,
            name_ptr: b"work".as_ptr() as *const std::os::raw::c_char,
            reserved2: 0,
        };
        let end = OxSpanEvent {
            kind: flush::SPAN_EVENT_KIND_END,
            reserved0: 0,
            name_len: 0,
            reserved1: 0,
            seq: 1,
            ts_ns: 500,
            cpu_ns: 350,
            mem: 1200,
            mem_peak: 2500,
            name_ptr: std::ptr::null(),
            reserved2: 0,
        };
        ctx.apply_events(&[begin, end]);
        let tree = ctx.finalize();
        assert_eq!(tree.finished.len(), 1);
        let s = &tree.finished[0];
        // Wall times flow through from ev.ts_ns — no second clock read.
        assert_eq!(s.start_ns, 100);
        assert_eq!(s.end_ns, 500);
        assert_eq!(s.cpu_ns, 150);
        assert_eq!(s.mem_enter, 1000);
        assert_eq!(s.mem_exit, 1200);
        assert_eq!(s.mem_peak, 2500);
    }
}
