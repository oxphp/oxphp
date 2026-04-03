//! Thread-local span stack for APM child spans.
//!
//! Each PHP worker thread maintains a [`SpanStack`] that tracks open and
//! finished spans for the current request. At request start, [`SpanStack::reset`]
//! is called with the trace context. During execution, hooks and the PHP SDK
//! push/pop spans. At request end, [`SpanStack::take_finished`] drains all
//! completed spans for OTel export.

use std::cell::RefCell;
use std::time::{SystemTime, UNIX_EPOCH};

/// Local span ID visible to PHP. `0` is reserved as no-op/invalid.
pub type SpanLocalId = u32;

/// Hex lookup table for fast byte-to-hex conversion.
const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

/// A named timestamp with attributes, attached to a span.
#[derive(Debug, Clone)]
pub struct SpanEvent {
    pub name: String,
    pub attributes: Vec<(String, String)>,
    pub timestamp_us: u64,
}

/// An open span that has not yet finished.
#[derive(Debug)]
pub struct PendingSpan {
    pub local_id: SpanLocalId,
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: String,
    pub name: String,
    pub start_us: u64,
    pub attributes: Vec<(String, String)>,
    pub events: Vec<SpanEvent>,
    /// 0 = Unset, 1 = Ok, 2 = Error.
    pub status_code: u8,
    pub status_message: Option<String>,
}

/// A completed span ready for export.
#[derive(Debug)]
pub struct FinishedSpan {
    pub local_id: SpanLocalId,
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: String,
    pub name: String,
    pub start_us: u64,
    pub end_us: u64,
    pub attributes: Vec<(String, String)>,
    pub events: Vec<SpanEvent>,
    /// 0 = Unset, 1 = Ok, 2 = Error.
    pub status_code: u8,
    pub status_message: Option<String>,
    /// `true` if the span was force-closed (not explicitly popped).
    pub leaked: bool,
}

/// Thread-local stack of open spans plus a finished-spans buffer.
///
/// The `spans` Vec acts as a stack: the last element is the "current" span
/// for implicit operations like `oxphp_trace_attribute('key', 'value')`.
pub struct SpanStack {
    spans: Vec<PendingSpan>,
    finished: Vec<FinishedSpan>,
    next_id: SpanLocalId,
    trace_id: String,
    root_span_id: String,
}

impl Default for SpanStack {
    fn default() -> Self {
        Self::new()
    }
}

impl SpanStack {
    /// Create an empty span stack with pre-allocated capacity.
    pub fn new() -> Self {
        Self {
            spans: Vec::with_capacity(8),
            finished: Vec::with_capacity(16),
            next_id: 1,
            trace_id: String::new(),
            root_span_id: String::new(),
        }
    }

    /// Reset the stack for a new request, clearing all spans and setting
    /// the trace context.
    pub fn reset(&mut self, trace_id: String, root_span_id: String) {
        self.spans.clear();
        self.finished.clear();
        self.next_id = 1;
        self.trace_id = trace_id;
        self.root_span_id = root_span_id;
    }

    /// Push a new child span onto the stack.
    ///
    /// The parent is the topmost open span, or `root_span_id` if the stack
    /// is empty. Returns the local ID assigned to the new span.
    pub fn push(&mut self, name: String, attributes: Vec<(String, String)>) -> SpanLocalId {
        let local_id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if self.next_id == 0 {
            self.next_id = 1;
        }

        let parent_span_id = self
            .spans
            .last()
            .map(|s| s.span_id.clone())
            .unwrap_or_else(|| self.root_span_id.clone());

        let span_id = generate_span_id();

        self.spans.push(PendingSpan {
            local_id,
            trace_id: self.trace_id.clone(),
            span_id,
            parent_span_id,
            name,
            start_us: now_us(),
            attributes,
            events: Vec::new(),
            status_code: 0,
            status_message: None,
        });

        local_id
    }

    /// Pop a span by local ID, moving it to the finished list.
    ///
    /// Returns `None` if no span with the given `local_id` is found.
    pub fn pop(&mut self, local_id: SpanLocalId) -> Option<()> {
        let idx = self.spans.iter().position(|s| s.local_id == local_id)?;
        let pending = self.spans.remove(idx);
        self.finished.push(FinishedSpan {
            local_id: pending.local_id,
            trace_id: pending.trace_id,
            span_id: pending.span_id,
            parent_span_id: pending.parent_span_id,
            name: pending.name,
            start_us: pending.start_us,
            end_us: now_us(),
            attributes: pending.attributes,
            events: pending.events,
            status_code: pending.status_code,
            status_message: pending.status_message,
            leaked: false,
        });
        Some(())
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
        let now = now_us();
        for pending in self.spans.drain(..) {
            self.finished.push(FinishedSpan {
                local_id: pending.local_id,
                trace_id: pending.trace_id,
                span_id: pending.span_id,
                parent_span_id: pending.parent_span_id,
                name: pending.name,
                start_us: pending.start_us,
                end_us: now,
                attributes: pending.attributes,
                events: pending.events,
                status_code: pending.status_code,
                status_message: pending.status_message,
                leaked: true,
            });
        }
        count
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

/// Force-close all open spans, drain finished, and serialize to JSON for cross-thread transport.
/// Called on the PHP worker thread before sending ScriptResponse to Tokio.
pub fn drain_and_serialize() -> Option<String> {
    SPAN_STACK.with(|s| {
        let mut stack = s.borrow_mut();
        stack.force_close_all();
        let finished = stack.take_finished();
        if finished.is_empty() {
            return None;
        }

        // Serialize to a simple JSON array — avoid serde dep by manual formatting
        let mut json = String::from("[");
        for (i, span) in finished.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            let attrs_json: String = span
                .attributes
                .iter()
                .map(|(k, v)| {
                    format!(
                        "[\"{}\",\"{}\"]",
                        k.replace('\\', "\\\\").replace('"', "\\\""),
                        v.replace('\\', "\\\\").replace('"', "\\\"")
                    )
                })
                .collect::<Vec<_>>()
                .join(",");

            let events_json: String = span
                .events
                .iter()
                .map(|e| {
                    let ea: String = e
                        .attributes
                        .iter()
                        .map(|(k, v)| {
                            format!(
                                "[\"{}\",\"{}\"]",
                                k.replace('\\', "\\\\").replace('"', "\\\""),
                                v.replace('\\', "\\\\").replace('"', "\\\"")
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    format!(
                        "{{\"n\":\"{}\",\"a\":[{}]}}",
                        e.name.replace('\\', "\\\\").replace('"', "\\\""),
                        ea
                    )
                })
                .collect::<Vec<_>>()
                .join(",");

            json.push_str(&format!(
                concat!(
                    "{{\"tid\":\"{}\",\"sid\":\"{}\",\"pid\":\"{}\",",
                    "\"n\":\"{}\",\"s\":{},\"e\":{},\"a\":[{}],\"ev\":[{}],",
                    "\"sc\":{},\"sm\":{},\"l\":{}}}"
                ),
                span.trace_id,
                span.span_id,
                span.parent_span_id,
                span.name.replace('\\', "\\\\").replace('"', "\\\""),
                span.start_us,
                span.end_us,
                attrs_json,
                events_json,
                span.status_code,
                span.status_message
                    .as_ref()
                    .map(|s| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")))
                    .unwrap_or_else(|| "null".to_string()),
                if span.leaked { "true" } else { "false" },
            ));
        }
        json.push(']');
        Some(json)
    })
}

/// Return the current time as Unix epoch microseconds.
pub fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}

/// Generate a 16-char lowercase hex span ID using system time and thread ID.
fn generate_span_id() -> String {
    let mut raw = [0u8; 8];
    // Mix system time nanos with thread ID for per-thread uniqueness.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let tid = thread_id_hash();
    let mixed = now ^ tid;
    raw.copy_from_slice(&mixed.to_ne_bytes());

    let mut out = String::with_capacity(16);
    for byte in &raw {
        out.push(HEX_CHARS[(byte >> 4) as usize] as char);
        out.push(HEX_CHARS[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Hash the current thread ID into a u64 for mixing into span IDs.
fn thread_id_hash() -> u64 {
    // ThreadId doesn't expose its numeric value directly; use Debug format.
    let tid = std::thread::current().id();
    let s = format!("{tid:?}");
    let mut hash: u64 = 0xcbf29ce484222325; // FNV offset basis
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV prime
    }
    hash
}

thread_local! {
    /// Per-worker-thread span stack.
    pub static SPAN_STACK: RefCell<SpanStack> = RefCell::new(SpanStack::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_pop_basic() {
        let mut stack = SpanStack::new();
        stack.reset("trace123".into(), "root456".into());

        let id = stack.push("my-span".into(), vec![]);
        assert_eq!(id, 1);
        assert_eq!(stack.open_count(), 1);

        stack.pop(id).expect("pop should succeed");
        assert_eq!(stack.open_count(), 0);
        assert_eq!(stack.finished_count(), 1);

        let finished = stack.take_finished();
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].name, "my-span");
        assert_eq!(finished[0].trace_id, "trace123");
        assert_eq!(finished[0].parent_span_id, "root456");
        assert!(!finished[0].leaked);
    }

    #[test]
    fn test_nested_parent_tracking() {
        let mut stack = SpanStack::new();
        stack.reset("trace-abc".into(), "root-def".into());

        let outer = stack.push("outer".into(), vec![]);
        let outer_span_id = stack.current().unwrap().span_id.clone();

        let inner = stack.push("inner".into(), vec![]);
        let inner_parent = stack.current().unwrap().parent_span_id.clone();

        // Inner's parent should be outer's span_id.
        assert_eq!(inner_parent, outer_span_id);

        stack.pop(inner).unwrap();
        stack.pop(outer).unwrap();

        let finished = stack.take_finished();
        assert_eq!(finished.len(), 2);
        // Inner was popped first.
        assert_eq!(finished[0].name, "inner");
        assert_eq!(finished[0].parent_span_id, outer_span_id);
        assert_eq!(finished[1].name, "outer");
        assert_eq!(finished[1].parent_span_id, "root-def");
    }

    #[test]
    fn test_force_close_marks_leaked() {
        let mut stack = SpanStack::new();
        stack.reset("t1".into(), "r1".into());

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
        let mut stack = SpanStack::new();
        stack.reset("t".into(), "r".into());

        assert!(stack.pop(999).is_none());

        let id = stack.push("s".into(), vec![]);
        assert!(stack.pop(id + 1).is_none());
    }

    #[test]
    fn test_reset_clears_all() {
        let mut stack = SpanStack::new();
        stack.reset("t1".into(), "r1".into());

        let id1 = stack.push("a".into(), vec![]);
        stack.pop(id1).unwrap();
        stack.push("b".into(), vec![]);

        assert_eq!(stack.open_count(), 1);
        assert_eq!(stack.finished_count(), 1);

        stack.reset("t2".into(), "r2".into());
        assert_eq!(stack.open_count(), 0);
        assert_eq!(stack.finished_count(), 0);
        assert_eq!(stack.trace_id(), "t2");
        assert_eq!(stack.root_span_id(), "r2");
    }

    #[test]
    fn test_current_mut() {
        let mut stack = SpanStack::new();
        stack.reset("t".into(), "r".into());

        let id = stack.push("span".into(), vec![]);
        stack
            .current_mut()
            .unwrap()
            .attributes
            .push(("key".into(), "value".into()));

        stack.pop(id).unwrap();
        let finished = stack.take_finished();
        assert_eq!(finished[0].attributes.len(), 1);
        assert_eq!(finished[0].attributes[0].0, "key");
        assert_eq!(finished[0].attributes[0].1, "value");
    }

    #[test]
    fn test_get_mut_specific_span() {
        let mut stack = SpanStack::new();
        stack.reset("t".into(), "r".into());

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
        assert_eq!(outer.attributes[0].0, "modified");
    }

    #[test]
    fn test_span_id_zero_skipped() {
        let mut stack = SpanStack::new();
        stack.reset("t".into(), "r".into());

        // First push should return 1, not 0.
        let id = stack.push("first".into(), vec![]);
        assert_eq!(id, 1);
    }

    #[test]
    fn test_attributes_in_push() {
        let mut stack = SpanStack::new();
        stack.reset("t".into(), "r".into());

        let attrs = vec![
            ("db.system".into(), "mysql".into()),
            ("db.name".into(), "users".into()),
        ];
        let id = stack.push("db-query".into(), attrs);
        stack.pop(id).unwrap();

        let finished = stack.take_finished();
        assert_eq!(finished[0].attributes.len(), 2);
        assert_eq!(finished[0].attributes[0].0, "db.system");
        assert_eq!(finished[0].attributes[0].1, "mysql");
        assert_eq!(finished[0].attributes[1].0, "db.name");
        assert_eq!(finished[0].attributes[1].1, "users");
    }

    #[test]
    fn test_events_on_span() {
        let mut stack = SpanStack::new();
        stack.reset("t".into(), "r".into());

        let id = stack.push("work".into(), vec![]);
        stack.current_mut().unwrap().events.push(SpanEvent {
            name: "checkpoint".into(),
            attributes: vec![("msg".into(), "halfway".into())],
            timestamp_us: now_us(),
        });

        stack.pop(id).unwrap();
        let finished = stack.take_finished();
        assert_eq!(finished[0].events.len(), 1);
        assert_eq!(finished[0].events[0].name, "checkpoint");
        assert_eq!(finished[0].events[0].attributes[0].0, "msg");
        assert_eq!(finished[0].events[0].attributes[0].1, "halfway");
    }

    #[test]
    fn test_status_on_span() {
        let mut stack = SpanStack::new();
        stack.reset("t".into(), "r".into());

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
}
