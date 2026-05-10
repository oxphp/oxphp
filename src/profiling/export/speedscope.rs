//! Speedscope evented-profile exporter (Speedscope file-format v1).
//!
//! Output is a JSON document that Speedscope (https://speedscope.app)
//! imports directly. The "evented" profile type carries one `O`/`C`
//! event per BEGIN/END, with frames interned in a shared table.
//!
//! The exporter walks the tree's finished spans, collects unique
//! function names into a frames table, and emits open/close events
//! in chronological order. The earliest `start_ns` becomes the
//! profile's t=0; the latest `end_ns` becomes its end value. Units
//! are microseconds throughout.

use serde_json::{json, Value};
use std::collections::HashMap;

use crate::profiling::SpanTree;

/// Render the tree as a Speedscope JSON profile. Returns UTF-8
/// bytes (compact JSON, no trailing newline).
///
/// An empty tree yields a minimal valid document with an empty
/// events array and zero-length time window.
pub fn export_speedscope(tree: &SpanTree) -> Vec<u8> {
    // Profile time window: t=0 at the earliest start_ns; endValue at
    // the latest end_ns. Both expressed in µs relative to start.
    let (profile_start_ns, profile_end_ns) = if tree.finished.is_empty() {
        (0u64, 0u64)
    } else {
        let mut min_start = u64::MAX;
        let mut max_end = 0u64;
        for s in &tree.finished {
            if s.start_ns < min_start {
                min_start = s.start_ns;
            }
            if s.end_ns > max_end {
                max_end = s.end_ns;
            }
        }
        (min_start, max_end)
    };
    let end_us = profile_end_ns.saturating_sub(profile_start_ns) / 1000;

    // Build the frames table. Insertion order is the index order;
    // Speedscope events reference frames by index.
    let mut frame_index: HashMap<&str, u32> = HashMap::new();
    let mut frames: Vec<&str> = Vec::with_capacity(tree.finished.len());
    for span in &tree.finished {
        frame_index.entry(span.name.as_ref()).or_insert_with(|| {
            let idx = frames.len() as u32;
            frames.push(span.name.as_ref());
            idx
        });
    }

    // Reconstruct the call tree from `[start_ns, end_ns]` intervals and emit
    // events as an iterative DFS. This is robust to µs-bucket collisions:
    // because event order is determined by ns-precision interval nesting
    // (not by bucketed `at_us` plus a tiebreaker), the resulting stream is
    // LIFO-valid even when an entire span fits inside a single µs, when
    // siblings collapse to one bucket, etc.
    //
    // Sort key — `start_ns` asc, `end_ns` desc:
    //   * `start_ns` asc puts each span after its enclosing parent.
    //   * `end_ns` desc breaks ties so the longer span (the parent) is
    //     processed before the shorter one (the child) when their starts
    //     coincide. Without this an inner-but-equal-start child would be
    //     placed onto an empty stack before its parent.
    let mut order: Vec<usize> = (0..tree.finished.len()).collect();
    order.sort_by(|&a, &b| {
        let sa = &tree.finished[a];
        let sb = &tree.finished[b];
        sa.start_ns
            .cmp(&sb.start_ns)
            .then_with(|| sb.end_ns.cmp(&sa.end_ns))
    });

    let mut event_array: Vec<Value> = Vec::with_capacity(tree.finished.len() * 2);
    // Stack entries: (frame index, end_ns of the open span, close-event at_us)
    let mut stack: Vec<(u32, u64, u64)> = Vec::new();

    let close_us = |end_ns: u64| end_ns.saturating_sub(profile_start_ns) / 1000;
    let push_close = |events: &mut Vec<Value>, frame: u32, at: u64| {
        events.push(json!({"type": "C", "at": at, "frame": frame}));
    };

    for idx in order {
        let span = &tree.finished[idx];
        // Pop every still-open span that ended at or before this span starts.
        while let Some(&(top_frame, top_end_ns, top_close_us)) = stack.last() {
            if top_end_ns <= span.start_ns {
                stack.pop();
                push_close(&mut event_array, top_frame, top_close_us);
            } else {
                break;
            }
        }
        let frame = *frame_index.get(span.name.as_ref()).expect("interned");
        let open_us = span.start_ns.saturating_sub(profile_start_ns) / 1000;
        event_array.push(json!({"type": "O", "at": open_us, "frame": frame}));
        stack.push((frame, span.end_ns, close_us(span.end_ns)));
    }
    // Drain any spans still open after the last input span.
    while let Some((top_frame, _, top_close_us)) = stack.pop() {
        push_close(&mut event_array, top_frame, top_close_us);
    }

    let frame_array: Vec<Value> = frames
        .into_iter()
        .map(|name| json!({"name": name}))
        .collect();

    let profile_name = if tree.trace_id.is_empty() {
        "oxphp-profile".to_string()
    } else {
        tree.trace_id.to_string()
    };

    let doc = json!({
        "$schema": "https://www.speedscope.app/file-format-schema.json",
        "shared": { "frames": frame_array },
        "profiles": [{
            "type": "evented",
            "name": profile_name,
            "unit": "microseconds",
            "startValue": 0,
            "endValue": end_us,
            "events": event_array,
        }],
        "name": "OxPHP Profiler",
        "exporter": format!("oxphp/{}", env!("CARGO_PKG_VERSION")),
        "activeProfileIndex": 0,
    });
    serde_json::to_vec(&doc).expect("speedscope JSON serialisation should not fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiling::{FinishedSpan, ProfilingMode, SpanTree};
    use std::sync::Arc;

    fn mk_span(name: &str, start_ns: u64, end_ns: u64) -> FinishedSpan {
        FinishedSpan {
            local_id: 0,
            trace_id: Arc::<str>::from(""),
            span_id: Arc::<str>::from(""),
            parent_span_id: Arc::<str>::from(""),
            name: Arc::<str>::from(name),
            start_ns,
            end_ns,
            attributes: Vec::new(),
            events: Vec::new(),
            status_code: 0,
            status_message: None,
            leaked: false,
            cpu_ns: 0,
            mem_enter: 0,
            mem_exit: 0,
            mem_peak: 0,
        }
    }

    fn tree_of(finished: Vec<FinishedSpan>) -> SpanTree {
        SpanTree {
            mode: ProfilingMode::ProfileAll,
            trace_id: Arc::<str>::from(""),
            root_span_id: Arc::<str>::from(""),
            finished,
        }
    }

    /// Reconstruct the open/close stack from the emitted events and
    /// assert it never tries to close a frame that isn't on top.
    /// This mirrors what speedscope.app does at load time.
    fn assert_lifo(bytes: &[u8]) {
        let v: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        let events = v["profiles"][0]["events"].as_array().unwrap();
        let mut stack: Vec<u64> = Vec::new();
        for (i, ev) in events.iter().enumerate() {
            let kind = ev["type"].as_str().unwrap();
            let frame = ev["frame"].as_u64().unwrap();
            match kind {
                "O" => stack.push(frame),
                "C" => {
                    let top = stack.last().copied();
                    assert_eq!(
                        top,
                        Some(frame),
                        "LIFO violation at event #{i}: close frame={frame}, top={top:?}"
                    );
                    stack.pop();
                }
                _ => panic!("unknown event kind: {kind}"),
            }
        }
        assert!(
            stack.is_empty(),
            "events end with non-empty stack: {stack:?}"
        );
    }

    /// Parent and child both bucket into the same `start_ns / 1000` and
    /// the same `end_ns / 1000`. The previous tiebreaker (frame index)
    /// ordered them by hash-table insertion order, which had nothing to
    /// do with the actual call hierarchy — speedscope.app would refuse
    /// the file with "Tried to leave frame X while frame Y was at the
    /// top". Reproduces the Composer autoloader case observed in real
    /// profiles.
    #[test]
    fn nested_spans_sharing_microsecond_bucket() {
        // loadClass wraps findFile, both start within the same µs
        // and both end within the same µs.
        let spans = vec![
            mk_span("loadClass", 105_227_000, 105_294_999),
            mk_span("findFile", 105_227_500, 105_294_500),
        ];
        let tree = tree_of(spans);
        let bytes = export_speedscope(&tree);
        assert_lifo(&bytes);
    }

    /// Parent and child share `start_ns` exactly (not just µs bucket):
    /// the tiebreaker has to fall through to `end_ns` to put the parent
    /// first.
    #[test]
    fn nested_spans_with_identical_start_ns() {
        let spans = vec![
            mk_span("parent", 1_000, 5_000),
            mk_span("child", 1_000, 3_000),
        ];
        let bytes = export_speedscope(&tree_of(spans));
        assert_lifo(&bytes);
    }

    /// Three-deep nesting where the two inner spans both collapse onto
    /// the same close-µs as the outer.
    #[test]
    fn three_deep_close_at_same_microsecond() {
        let spans = vec![
            mk_span("a", 1_000, 9_999),
            mk_span("b", 2_000, 9_500),
            mk_span("c", 3_000, 9_200),
        ];
        let bytes = export_speedscope(&tree_of(spans));
        assert_lifo(&bytes);
    }

    /// Independent sibling spans that don't overlap should still
    /// produce a clean event stream.
    #[test]
    fn sibling_spans_round_trip() {
        let spans = vec![mk_span("a", 1_000, 2_000), mk_span("b", 3_000, 4_000)];
        let bytes = export_speedscope(&tree_of(spans));
        assert_lifo(&bytes);
    }

    /// Single span shorter than 1µs — open and close land in the same
    /// `at_us`. The old sort-based exporter emitted the close event
    /// before the open here.
    #[test]
    fn sub_microsecond_span() {
        let spans = vec![mk_span("tiny", 1_000, 1_500)];
        let bytes = export_speedscope(&tree_of(spans));
        assert_lifo(&bytes);
        assert_at_non_decreasing(&bytes);
    }

    /// Two non-overlapping siblings whose four events all fall into a
    /// single µs bucket. Previously the "all closes before all opens"
    /// rule produced `C-a, C-b, O-a, O-b`.
    #[test]
    fn sub_microsecond_siblings() {
        let spans = vec![mk_span("a", 1_000, 1_300), mk_span("b", 1_500, 1_900)];
        let bytes = export_speedscope(&tree_of(spans));
        assert_lifo(&bytes);
        assert_at_non_decreasing(&bytes);
    }

    /// Spans arrive in `tree.finished` in completion order (leaves
    /// first); the exporter must still produce a valid DFS regardless
    /// of input order.
    #[test]
    fn input_in_post_order_still_emits_dfs() {
        // Same shape as `three_deep_close_at_same_microsecond` but with
        // the innermost span first.
        let spans = vec![
            mk_span("c", 3_000, 9_200),
            mk_span("b", 2_000, 9_500),
            mk_span("a", 1_000, 9_999),
        ];
        let bytes = export_speedscope(&tree_of(spans));
        assert_lifo(&bytes);
        assert_at_non_decreasing(&bytes);
    }

    /// Speedscope expects events in non-decreasing `at` order. Verify
    /// it on a representative profile.
    fn assert_at_non_decreasing(bytes: &[u8]) {
        let v: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        let events = v["profiles"][0]["events"].as_array().unwrap();
        let mut prev = 0u64;
        for ev in events {
            let at = ev["at"].as_u64().unwrap();
            assert!(at >= prev, "at decreased: {prev} -> {at}");
            prev = at;
        }
    }

    /// Empty tree produces a valid (if useless) document with no events.
    #[test]
    fn empty_tree_produces_empty_events() {
        let bytes = export_speedscope(&tree_of(vec![]));
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["profiles"][0]["events"].as_array().unwrap().len(), 0);
        assert_eq!(v["profiles"][0]["endValue"].as_u64().unwrap(), 0);
    }
}
